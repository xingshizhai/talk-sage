//! 云端 live 测试的公共门槛：凭据从哪来、缺了怎么办。
//!
//! 与模型集成测试的 `TALKSAGE_REQUIRE_MODELS` 是**两种资源**，故用两个开关：
//! 模型能靠 `scripts/download_models.py` 在 CI 里拉下来，云服务凭据不能。
//! CI 的 integration job 有模型没凭据，共用一个开关会把它逼成必然失败。
//!
//! 取值顺序：进程环境变量 > 仓库根 `.env`（gitignore，开发机本地凭据）。
//! 两处都没有时默认打印并跳过；`TALKSAGE_REQUIRE_ALIYUN=1` 则直接失败 ——
//! 配了凭据的环境里，「因跳过而全绿」同样是没有守门。

#![allow(dead_code)] // 每个 live 测试各用其中一部分

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 缺凭据时是否必须失败（而不是跳过）。
fn must_fail_on_missing() -> bool {
    matches!(
        std::env::var("TALKSAGE_REQUIRE_ALIYUN").ok().as_deref(),
        Some("1") | Some("true")
    )
}

/// 凭据缺失：默认打印并跳过；`TALKSAGE_REQUIRE_ALIYUN=1` 时直接失败。
pub fn skip(reason: &str) {
    assert!(
        !must_fail_on_missing(),
        "云端 live 测试凭据缺失（TALKSAGE_REQUIRE_ALIYUN=1 要求必须真实运行）: {reason}"
    );
    eprintln!("跳过：{reason}");
}

/// 读一组必需的凭据。任一缺失或为空则返回 `Err(缺失的变量名)`。
///
/// 变量名与 `config/talksage.example.toml` 承诺的敏感字段 env 覆盖一致
/// （ALIYUN_ACCESS_ID / ALIYUN_ACCESS_SECRET / ALIYUN_APP_ID）。
pub fn env_all(keys: &[&str]) -> Result<Vec<String>, String> {
    load_dotenv_once();
    let mut values = Vec::with_capacity(keys.len());
    let mut missing = Vec::new();
    for key in keys {
        match std::env::var(key) {
            Ok(v) if !v.trim().is_empty() => values.push(v),
            _ => missing.push(*key),
        }
    }
    if missing.is_empty() {
        Ok(values)
    } else {
        Err(missing.join(" / "))
    }
}

/// 把仓库根 `.env` 里的键读进进程环境，**已存在的变量不覆盖**
/// （CI/命令行显式给的值优先于开发机上那份 `.env`）。
fn load_dotenv_once() {
    static LOADED: OnceLock<()> = OnceLock::new();
    LOADED.get_or_init(|| {
        let Some(path) = find_dotenv() else { return };
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let key = key.trim();
            // 值可能带引号；两侧同种引号才剥，避免吃掉密钥里的字符。
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            if key.is_empty() || std::env::var_os(key).is_some() {
                continue;
            }
            std::env::set_var(key, value);
        }
        eprintln!("已加载本机凭据: {}", path.display());
    });
}

/// 从 crate 目录向上找 `.env`（worktree 里仓库根不是 `../..`，所以逐级往上找）。
fn find_dotenv() -> Option<PathBuf> {
    let mut dir: Option<&Path> = Some(Path::new(env!("CARGO_MANIFEST_DIR")));
    while let Some(d) = dir {
        let candidate = d.join(".env");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}
