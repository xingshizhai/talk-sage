//! 多模型 CER 基准测试工具
//!
//! 用法：
//!   cargo run -p talksage-asr --bin bench_cer --release -- \
//!     <models-root> <wav-dir> <transcript-file> \
//!     [--engines paraformer-zh,whisper-base,whisper-small,qwen3-asr] \
//!     [--max N]   # 只测前 N 条（默认全量）
//!
//! transcript-file 格式（AISHELL-1 标准）：
//!   BAC009S0002W0122 而且 目前 中国 的 飞机 比较 多
//!   每行：<wav_id> <空格分隔参考文本>
//!   wav 文件名 = <wav_id>.wav，位于 wav-dir 下（递归查找）

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use talksage_asr::{EngineKind, OfflineSegmentEngine, SegmentEngine, SherpaStreamingEngine};
use sherpa_onnx::Wave;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "用法: bench_cer <models-root> <wav-dir> <transcript-file> \
             [--engines paraformer-zh,whisper-base,...] [--max N]"
        );
        std::process::exit(2);
    }

    let models_root = PathBuf::from(&args[1]);
    let wav_dir = PathBuf::from(&args[2]);
    let transcript_file = &args[3];

    let engines_arg = args.windows(2)
        .find(|w| w[0] == "--engines")
        .map(|w| w[1].clone());
    let max_samples: Option<usize> = args.windows(2)
        .find(|w| w[0] == "--max")
        .and_then(|w| w[1].parse().ok());

    let requested_kinds: Vec<EngineKind> = match engines_arg {
        Some(ref s) => s.split(',')
            .filter_map(|name| EngineKind::from_name(name.trim()))
            .collect(),
        None => vec![
            EngineKind::ParaformerZh,
            EngineKind::WhisperBase,
            EngineKind::WhisperSmall,
            EngineKind::Qwen3Asr,
        ],
    };

    let kinds: Vec<EngineKind> = requested_kinds.into_iter()
        .filter(|k| {
            let avail = k.is_available(&models_root);
            if !avail {
                println!("跳过 {} — 模型未安装", k.display_name());
            }
            avail
        })
        .collect();

    if kinds.is_empty() {
        anyhow::bail!("没有可用的引擎。请先在设置里下载模型，或用 --engines 指定已安装的引擎。");
    }

    let entries = parse_transcript(transcript_file, max_samples)?;
    println!("加载 {} 条转写记录", entries.len());

    let wav_index = build_wav_index(&wav_dir)?;
    println!("在 {} 下找到 {} 个 wav 文件\n", wav_dir.display(), wav_index.len());

    if entries.is_empty() {
        anyhow::bail!("transcript 里没有有效记录。请确认文件格式：每行 <wav_id> <参考文本>。");
    }

    struct BenchResult {
        cer: f64,
        rtf: f64,
        max_infer_ms: u64,
        samples_tested: usize,
    }

    let mut results: Vec<(String, BenchResult)> = Vec::new();

    for kind in &kinds {
        let model_dir = models_root.join(kind.model_dir_name());
        println!("━━ {} ━━", kind.display_name());
        print!("  加载模型...");
        use std::io::Write as _;
        std::io::stdout().flush().ok();

        let result = if kind.is_streaming() {
            match SherpaStreamingEngine::new(*kind, &model_dir, 2) {
                Ok(mut engine) => {
                    println!(" 完成");
                    bench_engine(&mut engine, &entries, &wav_index, entries.len())
                }
                Err(e) => { println!(" 失败: {e}"); continue; }
            }
        } else {
            match OfflineSegmentEngine::new(*kind, &model_dir, 2) {
                Ok(mut engine) => {
                    println!(" 完成");
                    bench_engine(&mut engine, &entries, &wav_index, entries.len())
                }
                Err(e) => { println!(" 失败: {e}"); continue; }
            }
        };

        match result {
            Ok((cer, rtf, max_ms, n)) => {
                println!(
                    "  CER={:.2}%  RTF={:.3}  最长推理={:.0}ms  样本={}",
                    cer * 100.0, rtf, max_ms, n
                );
                results.push((kind.display_name().to_string(), BenchResult { cer, rtf, max_infer_ms: max_ms, samples_tested: n }));
            }
            Err(e) => println!("  评测失败: {e}"),
        }
        println!();
    }

    // 汇总表
    let sep = "─".repeat(62);
    println!("\n{sep}");
    println!("{:<22} {:>6} {:>8} {:>8} {:>12}", "引擎", "样本", "CER", "RTF", "最长推理(ms)");
    println!("{sep}");
    results.sort_by(|a, b| a.1.cer.partial_cmp(&b.1.cer).unwrap());
    for (name, r) in &results {
        println!(
            "{:<22} {:>6} {:>7.2}% {:>8.3} {:>12}",
            name, r.samples_tested, r.cer * 100.0, r.rtf, r.max_infer_ms
        );
    }
    println!("{sep}");
    if let Some(best) = results.first() {
        println!("最低 CER: {} ({:.2}%)", best.0, best.1.cer * 100.0);
    }

    Ok(())
}

// 统一的引擎评测函数（流式和离线都用 SegmentEngine trait）
fn bench_engine(
    engine: &mut dyn SegmentEngine,
    entries: &[(String, String)],
    wav_index: &HashMap<String, PathBuf>,
    total: usize,
) -> anyhow::Result<(f64, f64, u64, usize)> {
    let mut total_ref_chars = 0usize;
    let mut total_edits = 0usize;
    let mut total_audio_s = 0.0f64;
    let mut total_infer_s = 0.0f64;
    let mut max_infer_ms = 0u64;
    let mut count = 0usize;

    for (id, ref_text) in entries {
        let wav_path = match wav_index.get(id) {
            Some(p) => p,
            None => { eprintln!("  警告: 找不到 {id}.wav，跳过"); continue; }
        };
        let wave = match Wave::read(wav_path.to_str().unwrap_or("")) {
            Some(w) => w,
            None => { eprintln!("  警告: 读 {} 失败，跳过", wav_path.display()); continue; }
        };
        if wave.sample_rate() != 16000 {
            eprintln!("  警告: {id} 采样率 {}≠16000，跳过", wave.sample_rate());
            continue;
        }

        let samples = wave.samples();
        let audio_s = samples.len() as f64 / 16000.0;

        let t = Instant::now();
        // 分块喂入（200ms 块；离线引擎会缓冲到 finish）
        for chunk in samples.chunks(3200) {
            engine.accept(chunk);
        }
        let hyp = engine.finish().trim().to_string();
        engine.reset();
        let elapsed = t.elapsed().as_secs_f64();

        total_audio_s += audio_s;
        total_infer_s += elapsed;
        max_infer_ms = max_infer_ms.max((elapsed * 1000.0) as u64);

        let (ref_n, edits) = cer_counts(&normalize(ref_text), &normalize(&hyp));
        total_ref_chars += ref_n;
        total_edits += edits;
        count += 1;

        if count % 500 == 0 {
            println!("  {count}/{total}");
        }
    }

    let cer = if total_ref_chars > 0 { total_edits as f64 / total_ref_chars as f64 } else { 0.0 };
    let rtf = if total_audio_s > 0.0 { total_infer_s / total_audio_s } else { 0.0 };
    Ok((cer, rtf, max_infer_ms, count))
}

// 去标点、去空白、英文转小写，保留 CJK + 字母 + 数字
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

// 字符级 CER：返回 (参考字数, 编辑距离)
fn cer_counts(reference: &str, hypothesis: &str) -> (usize, usize) {
    let r: Vec<char> = reference.chars().collect();
    let h: Vec<char> = hypothesis.chars().collect();
    let n = r.len();
    let m = h.len();
    if n == 0 { return (0, m); }
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..=n { dp[i][0] = i; }
    for j in 0..=m { dp[0][j] = j; }
    for i in 1..=n {
        for j in 1..=m {
            dp[i][j] = if r[i - 1] == h[j - 1] {
                dp[i - 1][j - 1]
            } else {
                1 + dp[i - 1][j - 1].min(dp[i - 1][j]).min(dp[i][j - 1])
            };
        }
    }
    (n, dp[n][m])
}

// 解析 AISHELL-1 格式 transcript
fn parse_transcript(path: &str, max: Option<usize>) -> anyhow::Result<Vec<(String, String)>> {
    let content = std::fs::read_to_string(path)?;
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let mut parts = line.splitn(2, char::is_whitespace);
        let id = match parts.next() { Some(s) => s.to_string(), None => continue };
        // 参考文本去掉空格（AISHELL-1 里是字之间有空格分隔）
        let text = match parts.next() {
            Some(s) => s.split_whitespace().collect::<Vec<_>>().join(""),
            None => continue,
        };
        entries.push((id, text));
        if max.map(|m| entries.len() >= m).unwrap_or(false) { break; }
    }
    Ok(entries)
}

// 递归建 wav_id → 路径索引
fn build_wav_index(dir: &Path) -> anyhow::Result<HashMap<String, PathBuf>> {
    let mut index = HashMap::new();
    walk_wav(dir, &mut index)?;
    Ok(index)
}

fn walk_wav(dir: &Path, index: &mut HashMap<String, PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_wav(&path, index)?;
        } else if path.extension().map(|e| e.eq_ignore_ascii_case("wav")).unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                index.insert(stem.to_string(), path);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_punct_and_spaces() {
        assert_eq!(normalize("你好，世界！"), "你好世界");
        assert_eq!(normalize("Hello World."), "helloworld");
        assert_eq!(normalize("中英 mixed OK"), "中英mixedok");
    }

    #[test]
    fn cer_identical() {
        let (n, d) = cer_counts("你好世界", "你好世界");
        assert_eq!(d, 0);
        assert_eq!(n, 4);
    }

    #[test]
    fn cer_one_substitution() {
        // "你好世界" vs "你好大界"：只有第3字不同
        let (n, d) = cer_counts("你好世界", "你好大界");
        assert_eq!(d, 1);
        assert_eq!(n, 4);
    }

    #[test]
    fn cer_empty_reference() {
        let (n, d) = cer_counts("", "你好");
        assert_eq!(n, 0);
        assert_eq!(d, 2); // 2 insertions
    }
}
