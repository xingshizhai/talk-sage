//! sherpa-onnx 流式 ASR PoC：验证流式延迟与准确率。
//!
//! 用法:
//!   cargo run -p talksage-asr --bin poc_asr -- <engine> <model-dir> <wav> [--chunk-ms 200]
//!
//!   engine: paraformer-zh | zipformer-en
//!
//! 行为：把 wav 按 chunk-ms 分块喂入流式引擎，模拟实时识别；
//! 打印每块增量文本、decode 耗时，并统计首次出字延迟与实时因子(RTF)。

use std::path::PathBuf;
use std::time::Instant;

use talksage_asr::{EngineKind, SherpaStreamingEngine, StreamingASREngine};
use sherpa_onnx::Wave;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("用法: poc_asr <paraformer-zh|zipformer-en> <model-dir> <wav> [--chunk-ms N]");
        std::process::exit(2);
    }
    let kind = EngineKind::from_name(&args[1])
        .ok_or_else(|| anyhow::anyhow!("未知引擎: {}", args[1]))?;
    let model_dir = PathBuf::from(&args[2]);
    let wav_path = &args[3];
    let chunk_ms: u64 = args
        .windows(2)
        .find(|w| w[0] == "--chunk-ms")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(200);

    let wave = Wave::read(wav_path).ok_or_else(|| anyhow::anyhow!("读取 wav 失败: {wav_path}"))?;
    let sample_rate = wave.sample_rate();
    let samples = wave.samples();
    if sample_rate != 16000 {
        anyhow::bail!("PoC 要求 16kHz wav，当前 {sample_rate}Hz（请先重采样）");
    }
    println!(
        "== {} ==\n模型目录: {}\n音频: {} ({}s, {} samples)  分块: {}ms",
        kind.display_name(),
        model_dir.display(),
        wav_path,
        samples.len() as f64 / sample_rate as f64,
        samples.len(),
        chunk_ms,
    );

    let mut engine = SherpaStreamingEngine::new(kind, &model_dir, 2)?;
    println!("模型加载完成。\n");

    let chunk_size = (sample_rate as u64 * chunk_ms / 1000) as usize;
    let mut first_text_ts: Option<f64> = None;
    let mut last_text = String::new();
    let mut total_decode_us = 0u128;
    let mut decode_calls = 0u32;

    for (i, chunk) in samples.chunks(chunk_size).enumerate() {
        let audio_ts = i as f64 * chunk_ms as f64 / 1000.0;
        let t = Instant::now();
        let text = engine.accept(chunk);
        total_decode_us += t.elapsed().as_micros();
        decode_calls += 1;

        let text = text.unwrap_or_default();
        if !text.trim().is_empty() && first_text_ts.is_none() {
            first_text_ts = Some(audio_ts);
        }
        let new_chars = text.chars().count().saturating_sub(last_text.chars().count());
        if new_chars > 0 {
            println!(
                "[{audio_ts:6.2}s] +{new_chars}字  decode={:6.2}ms  |> {}",
                t.elapsed().as_micros() as f64 / 1000.0,
                text
            );
            last_text = text;
        }
    }

    // 刷新尾部
    engine.finish();
    if let Some(final_text) = engine.accept(&[]) {
        let final_text = final_text.trim().to_string();
        if !final_text.is_empty() && final_text != last_text.trim() {
            println!("[结束] 最终  |> {final_text}");
            last_text = final_text;
        }
    }

    let total_audio_s = samples.len() as f64 / sample_rate as f64;
    let avg_decode_ms = if decode_calls > 0 {
        total_decode_us as f64 / 1000.0 / decode_calls as f64
    } else {
        0.0
    };
    let rtf = total_decode_us as f64 / 1e6 / total_audio_s;

    println!("\n===== 统计 =====");
    println!("音频时长        : {total_audio_s:.2}s");
    println!("识别文本        : {}", last_text);
    println!("文本字数        : {}", last_text.chars().count());
    match first_text_ts {
        Some(ts) => println!("首次出字(音频起点)→ {ts:.2}s（含语音前置/静音）"),
        None => println!("首次出字          : 未出字！"),
    }
    println!("平均单块 decode  : {avg_decode_ms:.2}ms");
    println!("实时因子 RTF     : {rtf:.3}（<1 表示快于实时）");
    Ok(())
}
