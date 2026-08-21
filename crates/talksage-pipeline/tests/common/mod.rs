//! 集成测试共享辅助：模型资源探测与「跳过 vs 失败」判定。
//!
//! 这些辅助曾在 pipeline_live.rs / speaker_live.rs / characterization.rs 里各抄一份，
//! 并且已经漂移过（候选目录列表长短不一）。漂移在这里格外危险：失败模式是
//! 某个测试在别的机器上「静默跳过」而其余测试照跑 —— 看上去全绿，安全网却没了。
//!
//! 注意：`tests/common/mod.rs` 会被 tests/ 下每个测试二进制各编译一次，
//! 因此单个二进制用不到的函数会触发 dead_code 警告 —— 故在此整体 allow。

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// 缺资源时是否必须失败（而不是跳过）。`env` 为 `TALKSAGE_REQUIRE_MODELS` 的值。
///
/// 只认 1/true：用 `is_ok()` 会让 `TALKSAGE_REQUIRE_MODELS=0`（本意是关闭）
/// 也变成「必须失败」。
pub fn must_fail_on_missing(env: Option<&str>) -> bool {
    matches!(env, Some("1") | Some("true"))
}

/// 资源缺失：默认打印并跳过；`TALKSAGE_REQUIRE_MODELS=1` 时直接失败，
/// 避免 CI 上「因跳过而全绿」掩盖回归。
pub fn skip(reason: &str) {
    let env = std::env::var("TALKSAGE_REQUIRE_MODELS").ok();
    assert!(
        !must_fail_on_missing(env.as_deref()),
        "集成测试资源缺失（TALKSAGE_REQUIRE_MODELS=1 要求必须真实运行）: {reason}"
    );
    eprintln!("跳过：{reason}");
}

/// 可选引擎（whisper / qwen3 等，不在必需模型集里）缺失：始终跳过。
///
/// 与 [`skip`] 的区别是不受 `TALKSAGE_REQUIRE_MODELS` 影响 —— 这些模型
/// 本来就不要求在 CI 上存在，强制失败只会制造噪音。
pub fn skip_optional(reason: &str) {
    eprintln!("跳过（可选资源）：{reason}");
}

/// 解析模型根目录（TALKSAGE_MODELS_DIR 优先，其次相对 CARGO_MANIFEST_DIR 探测）。
///
/// 候选列表必须保持完整：少一个候选就意味着换个工作目录跑时本测试静默跳过。
pub fn model_root() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
        }
    }
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    for cand in [
        here.join("../../models"),
        here.join("../../../models"),
        PathBuf::from("models"),
        PathBuf::from("../models"),
    ] {
        if cand.is_dir() {
            return Some(cand);
        }
    }
    None
}

#[test]
fn require_models_flag_controls_skip_vs_fail() {
    assert!(!must_fail_on_missing(None), "未设置时应跳过");
    assert!(!must_fail_on_missing(Some("0")), "0 应跳过");
    assert!(!must_fail_on_missing(Some("")), "空值应跳过");
    assert!(must_fail_on_missing(Some("1")), "1 应失败");
    assert!(must_fail_on_missing(Some("true")), "true 应失败");
}
