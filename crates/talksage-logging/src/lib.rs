//! TalkSage v2 结构化日志（tracing）。
//!
//! 输出：
//! - 控制台：人类可读文本（级别着色）
//! - 文件：`<data_dir>/logs/talksage.YYYY-MM-DD.log`，**JSON lines**（每行一个事件，AI Agent 可直接解析）
//!   - `TALKSAGE_LOG_JSON=0` 时文件也用文本格式
//!
//! 级别控制：`RUST_LOG` 或 `TALKSAGE_LOG`（如 `talksage=debug`、`info`）。
//! 初始化返回 `LogGuard`，程序退出前 drop 以 flush 日志。

use std::path::PathBuf;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;

/// 日志守卫：持有 non-blocking writer，drop 时 flush。
pub struct LogGuard {
    _worker_guard: tracing_appender::non_blocking::WorkerGuard,
}

/// 当前过滤级别。
pub fn level_from_env() -> String {
    std::env::var("RUST_LOG")
        .or_else(|_| std::env::var("TALKSAGE_LOG"))
        .unwrap_or_else(|_| "info".to_string())
}

/// 解析日志目录：`TALKSAGE_LOG_DIR` > 数据目录 `logs/`。
pub fn log_dir(data_dir: Option<&PathBuf>) -> PathBuf {
    if let Ok(d) = std::env::var("TALKSAGE_LOG_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    let base = data_dir
        .cloned()
        .unwrap_or_else(talksage_config::default_data_dir);
    base.join("logs")
}

/// 初始化全局日志。重复调用只生效一次（后续返回空守卫）。
pub fn init(data_dir: Option<&PathBuf>) -> LogGuard {
    use tracing_subscriber::prelude::*;

    let dir = log_dir(data_dir);
    let _ = std::fs::create_dir_all(&dir);
    let file_appender = tracing_appender::rolling::daily(&dir, "talksage.log");
    let (file_writer, worker_guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_new(level_from_env()).unwrap_or_else(|_| EnvFilter::new("info"));
    let json_file = std::env::var("TALKSAGE_LOG_JSON")
        .map(|v| v != "0" && v.to_lowercase() != "false")
        .unwrap_or(true);

    if json_file {
        // 控制台文本 + 文件 JSON lines
        let console = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_span_events(FmtSpan::NONE);
        let file_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_current_span(true)
            .with_span_list(true)
            .with_writer(file_writer);
        tracing_subscriber::registry()
            .with(filter)
            .with(console)
            .with(file_layer)
            .init();
    } else {
        // 双文本（控制台 + 文件）
        let console = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_span_events(FmtSpan::NONE);
        let file_layer = tracing_subscriber::fmt::layer()
            .with_span_events(FmtSpan::NONE)
            .with_writer(file_writer);
        tracing_subscriber::registry()
            .with(filter)
            .with(console)
            .with(file_layer)
            .init();
    }
    // 桥接 `log` crate 宏到 tracing（项目内既有 log::* 调用）
    let _ = tracing_log::LogTracer::init();

    LogGuard {
        _worker_guard: worker_guard,
    }
}
