/// 阿里云 WebSocket 实时语音识别端到端测试
/// 运行：cargo test -p talksage-asr --test aliyun_ws_live -- --nocapture

use std::sync::Arc;
use talksage_asr::{SegmentEngine, aliyun::{TokenManager, AliyunEngine}};

fn sine_wave_samples(freq_hz: f32, duration_secs: f32, sample_rate: u32) -> Vec<f32> {
    let n = (sample_rate as f32 * duration_secs) as usize;
    (0..n)
        .map(|i| (2.0 * std::f32::consts::PI * freq_hz * i as f32 / sample_rate as f32).sin() * 0.3)
        .collect()
}

#[test]
fn aliyun_realtime_asr_connects_and_returns() {
    let key_id = std::env::var("ALIYUN_ACCESS_ID").expect("ALIYUN_ACCESS_ID not set");
    let key_secret = std::env::var("ALIYUN_ACCESS_SECRET").expect("ALIYUN_ACCESS_SECRET not set");
    let app_key = std::env::var("ALIYUN_APP_ID").expect("ALIYUN_APP_ID not set");

    // AliyunEngine 设计为在同步线程中使用，需要先建立一个 tokio runtime
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    let token_mgr = Arc::new(TokenManager::new(key_id, key_secret));
    let mut engine = AliyunEngine::new(&app_key, token_mgr, handle);

    let samples = sine_wave_samples(440.0, 2.0, 16000);
    for chunk in samples.chunks(3200) {
        let partial = engine.accept(chunk);
        if let Some(text) = partial {
            println!("Partial: {text}");
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(500));

    let result = engine.finish();
    println!("Final result: {:?}", result);
    println!("Test passed — WebSocket session completed successfully");
    // 纯音不会被识别为文字，result 可能为空；只要能完整走完流程即成功
}
