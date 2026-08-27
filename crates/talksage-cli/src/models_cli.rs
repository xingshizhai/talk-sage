//! 模型子命令：list / download / remove / gpu。

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::json;
use talksage_asr::{EngineKind, GpuBackend};
use talksage_pipeline::TalkSageService;

use crate::args::{ModelsAction, ModelsArgs};

const MIB: f64 = 1024.0 * 1024.0;

pub fn dispatch(args: ModelsArgs, json: bool) -> ExitCode {
    match args.command {
        ModelsAction::List => cmd_list(json),
        ModelsAction::Download { engine } => cmd_download(&engine, json),
        ModelsAction::Remove { engine, yes } => cmd_remove(&engine, yes, json),
        ModelsAction::Gpu => cmd_gpu(json),
    }
}

fn fail(json: bool, msg: String) -> ExitCode {
    if json {
        eprintln!("{}", json!({"ok": false, "error": msg}));
    } else {
        eprintln!("{msg}");
    }
    ExitCode::FAILURE
}

fn succeed(json: bool, value: serde_json::Value, text: impl FnOnce()) -> ExitCode {
    if json {
        println!("{value}");
    } else {
        text();
    }
    ExitCode::SUCCESS
}

fn models_root() -> Result<std::path::PathBuf, String> {
    TalkSageService::resolve_models_dir().ok_or_else(|| "未找到 models/ 目录（可设 TALKSAGE_MODELS_DIR）".into())
}

pub fn collect_models(root: Option<&Path>) -> Vec<serde_json::Value> {
    let mut models: Vec<serde_json::Value> = EngineKind::ALL
        .iter()
        .map(|&kind| {
            let p = kind.profile();
            json!({
                "id": kind.display_name(),
                "label": p.label,
                "languages": p.languages,
                "streaming": p.streaming,
                "speed": p.speed,
                "description": p.description,
                "selectable": p.selectable,
                "installed": root.is_some_and(|r| kind.is_available(r)),
                "size_mb": root.map(|r| talksage_asr::models::installed_size_mb(kind, r)).unwrap_or(0),
                "download_size_mb": talksage_asr::models::download_size_mb(kind),
                "downloading": root.is_some_and(|r| talksage_asr::models::is_downloading(kind, r)),
            })
        })
        .collect();
    models.push(json!({
        "id": "punct",
        "label": "标点恢复模型",
        "languages": "zh,en",
        "streaming": true,
        "speed": "fast",
        "description": "CT-Transformer 中英文标点预测",
        "selectable": false,
        "installed": root.is_some_and(|r| talksage_asr::is_punct_model_installed(r)),
        "size_mb": 0,
        "download_size_mb": talksage_asr::punct_download_size_mb(),
        "downloading": false,
    }));
    let vad = root.is_some_and(|r| r.join("silero-vad").join("silero_vad.onnx").is_file());
    models.push(json!({
        "id": "silero-vad",
        "label": "Silero VAD",
        "languages": "*",
        "streaming": true,
        "speed": "fast",
        "description": "语音活动检测；转写依赖。本命令不提供下载，请运行 scripts/download_models.py silero-vad",
        "selectable": false,
        "installed": vad,
        "size_mb": 0,
        "download_size_mb": 1,
        "downloading": false,
    }));
    models
}

fn cmd_list(json: bool) -> ExitCode {
    let root = TalkSageService::resolve_models_dir();
    let models = collect_models(root.as_deref());
    succeed(json, json!({ "models": models, "dir": root.as_ref().map(|p| p.display().to_string()) }), || {
        println!(
            "模型目录: {}",
            root.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "（未找到）".into())
        );
        println!(
            "{:<32} {:<6} {:>8} {:>8}  {}",
            "ID", "已装", "占用MB", "下载MB", "说明"
        );
        println!("{}", "-".repeat(80));
        for m in &models {
            let installed = m["installed"].as_bool().unwrap_or(false);
            println!(
                "{:<32} {:<6} {:>8} {:>8}  {}",
                m["id"].as_str().unwrap_or("-"),
                if installed { "是" } else { "否" },
                m["size_mb"].as_u64().unwrap_or(0),
                m["download_size_mb"].as_u64().unwrap_or(0),
                m["label"].as_str().unwrap_or(""),
            );
        }
    })
}

fn proxy_url() -> Option<String> {
    let mgr = talksage_config::ConfigManager::load(None, None).ok()?;
    let snap = mgr.snapshot();
    snap.network.proxy_url().map(str::to_string)
}

fn cmd_download(engine: &str, json: bool) -> ExitCode {
    let engine = engine.trim();
    if engine.eq_ignore_ascii_case("silero-vad") || engine.eq_ignore_ascii_case("wespeaker") {
        return fail(
            json,
            format!("`{engine}` 请用 `python scripts/download_models.py {engine}` 安装"),
        );
    }
    let root = match models_root() {
        Ok(r) => r,
        Err(e) => return fail(json, e),
    };
    if engine.eq_ignore_ascii_case("punct") {
        if talksage_asr::is_punct_model_installed(&root) {
            return succeed(json, json!({"ok": true, "engine": "punct", "skipped": true}), || {
                println!("标点模型已安装，跳过");
            });
        }
        let cancel = Arc::new(AtomicBool::new(false));
        if !json {
            eprintln!("正在下载 punct…");
        }
        return match talksage_asr::download_punct_model(&root, cancel, None, proxy_url().as_deref()) {
            Ok(()) => succeed(json, json!({"ok": true, "engine": "punct"}), || {
                println!("已安装 punct");
            }),
            Err(e) => fail(json, format!("下载失败: {e}")),
        };
    }
    let kind = match EngineKind::from_name(engine) {
        Some(k) => k,
        None => {
            return fail(
                json,
                format!(
                    "未知引擎: {engine}（可选 {} | punct）",
                    EngineKind::ALL
                        .iter()
                        .map(|k| k.display_name())
                        .collect::<Vec<_>>()
                        .join(" | ")
                ),
            );
        }
    };
    if kind == EngineKind::AliyunCloud {
        return succeed(json, json!({"ok": true, "engine": engine, "skipped": true}), || {
            println!("阿里云为云端引擎，无需下载本地模型");
        });
    }
    if !kind.is_product_model() {
        return fail(json, format!("旧模型 `{engine}` 已从产品模型管理移除"));
    }
    if kind.is_available(&root) {
        return succeed(
            json,
            json!({"ok": true, "engine": kind.display_name(), "skipped": true}),
            || println!("{} 已安装，跳过", kind.display_name()),
        );
    }
    let fallback = talksage_asr::models::download_size_mb(kind) * 1024 * 1024;
    let id = kind.display_name().to_string();
    let progress = move |received: u64, total: u64| {
        let effective = if total > 0 { total } else { fallback };
        let pct = if effective > 0 {
            ((received as f64 / effective as f64) * 100.0).min(99.0) as u32
        } else {
            0
        };
        eprint!(
            "\r下载 {id}: {pct}% ({:.1}/{:.1} MB)",
            received as f64 / MIB,
            effective as f64 / MIB
        );
        let _ = std::io::stderr().flush();
    };
    let cancel = AtomicBool::new(false);
    let result = talksage_asr::models::download_engine(
        kind,
        &root,
        Some(&progress),
        Some(&cancel),
        proxy_url().as_deref(),
    );
    eprintln!();
    match result {
        Ok(()) => succeed(json, json!({"ok": true, "engine": kind.display_name()}), || {
            println!("已安装 {}", kind.display_name());
        }),
        Err(e) => fail(json, format!("下载失败: {e}")),
    }
}

fn cmd_remove(engine: &str, yes: bool, json: bool) -> ExitCode {
    if !yes {
        return fail(json, format!("删除 `{engine}` 请加 --yes 确认"));
    }
    let root = match models_root() {
        Ok(r) => r,
        Err(e) => return fail(json, e),
    };
    if engine.eq_ignore_ascii_case("punct") {
        return match talksage_asr::remove_punct_model(&root) {
            Ok(()) => succeed(json, json!({"ok": true, "engine": "punct"}), || {
                println!("已删除 punct");
            }),
            Err(e) => fail(json, format!("删除失败: {e}")),
        };
    }
    let kind = match EngineKind::from_name(engine) {
        Some(k) => k,
        None => return fail(json, format!("未知引擎: {engine}")),
    };
    if !kind.is_product_model() {
        return fail(json, format!("旧模型 `{engine}` 已从产品模型管理移除"));
    }
    match talksage_asr::models::remove_engine(kind, &root) {
        Ok(()) => succeed(json, json!({"ok": true, "engine": kind.display_name()}), || {
            println!("已删除 {}", kind.display_name());
        }),
        Err(e) => fail(json, format!("删除失败: {e}")),
    }
}

fn gpu_status() -> serde_json::Value {
    let gpu = GpuBackend::detect();
    let cfg = talksage_config::ConfigManager::load(None, None)
        .ok()
        .map(|m| m.snapshot());
    let route = cfg.as_ref().and_then(|c| {
        talksage_asr::resolve_asr_route(
            &c.asr.asr_mode,
            &c.asr.backend,
            gpu,
            talksage_asr::CloudCredentials {
                access_key_id: &c.asr.aliyun_access_key_id,
                access_key_secret: &c.asr.aliyun_access_key_secret,
                app_key: &c.asr.aliyun_app_key,
            },
        )
        .ok()
    });
    json!({
        "backend": gpu.provider_str(),
        "display_name": gpu.display_name(),
        "hardware_candidate": GpuBackend::hardware_candidate(),
        "availability_note": GpuBackend::availability_note(),
        "is_accelerated": gpu.is_accelerated(),
        "effective_route": route.map(|r| r.display_name()),
    })
}

fn cmd_gpu(json: bool) -> ExitCode {
    let v = gpu_status();
    succeed(json, v.clone(), || {
        println!("GPU: {} ({})", v["display_name"], v["backend"]);
        println!("硬件候选: {}", v["hardware_candidate"]);
        println!("加速: {}", if v["is_accelerated"].as_bool().unwrap_or(false) { "是" } else { "否" });
        if let Some(r) = v["effective_route"].as_str() {
            println!("当前 ASR 路由: {r}");
        }
        println!("{}", v["availability_note"].as_str().unwrap_or(""));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_models_includes_product_and_support() {
        let rows = collect_models(None);
        let ids: Vec<&str> = rows.iter().filter_map(|m| m["id"].as_str()).collect();
        assert!(ids.contains(&"qwen3-asr"));
        assert!(ids.contains(&"punct"));
        assert!(ids.contains(&"silero-vad"));
        assert!(rows.iter().all(|m| m["installed"] == false));
    }

    #[test]
    fn gpu_status_has_backend() {
        let v = gpu_status();
        assert!(v["backend"].as_str().is_some());
        assert!(v["display_name"].as_str().is_some());
    }
}
