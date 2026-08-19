//! logging 集成测试：验证日志文件写出（JSON lines）与级别控制。

#[test]
fn init_writes_json_lines_to_log_file() {
    // 用 workspace target 下临时日志目录（避免系统 temp 权限问题）
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-logs")
        .join(format!("log-test-{}", std::process::id()));
    std::env::set_var("TALKSAGE_LOG_DIR", &dir);
    std::env::set_var("TALKSAGE_LOG", "debug");

    let _guard = talksage_logging::init(None);
    log::info!("测试日志消息");
    log::warn!("测试警告消息 {}", 42);
    log::debug!("测试调试消息");

    // non-blocking writer 需要一点时间落盘；guard drop 会 flush
    drop(_guard);

    // 轮询等待日志文件出现（非阻塞 writer 异步落盘）
    let mut files: Vec<_> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        files = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("talksage.log"))
            .collect();
        if !files.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(!files.is_empty(), "应生成日志文件，目录内容: {:?}",
        std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect::<Vec<_>>()).unwrap_or_default());

    let path = files[0].path();
    let content = std::fs::read_to_string(&path).unwrap();
    // JSON lines：每行一个完整 JSON 对象，含 timestamp/level/message
    assert!(content.contains("\"level\":\"INFO\""), "应有 INFO 事件");
    assert!(content.contains("测试日志消息"), "应有测试消息");
    assert!(content.contains("\"level\":\"WARN\""), "应有 WARN 事件");
    assert!(content.contains("测试警告消息 42"));
    assert!(content.contains("\"level\":\"DEBUG\""), "应有 DEBUG 事件");

    // 清理
    std::env::remove_var("TALKSAGE_LOG_DIR");
    std::env::remove_var("TALKSAGE_LOG");
    let _ = std::fs::remove_dir_all(&dir);
}
