//! 配置与日志：`config path/get/set`、`logs`。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{json, Value};
use talksage_config::SecretPolicy;

use crate::args::{ConfigAction, ConfigArgs};

pub fn dispatch(args: ConfigArgs, json: bool) -> ExitCode {
    match args.command {
        ConfigAction::Path => cmd_path(json),
        ConfigAction::Get { path } => cmd_get(path.as_deref(), json),
        ConfigAction::Set { path, value } => cmd_set(&path, &value, json),
    }
}

pub fn logs(lines: usize, json: bool) -> ExitCode {
    let data_dir = talksage_config::default_data_dir();
    let dir = talksage_logging::log_dir(Some(&data_dir));
    let latest = match latest_log_file(&dir) {
        Ok(p) => p,
        Err(e) => return fail(json, e),
    };
    let Some(path) = latest else {
        return succeed(
            json,
            json!({
                "ok": true,
                "path": Value::Null,
                "lines": [],
            }),
            || println!("（暂无日志）"),
        );
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return fail(json, format!("读取日志失败: {e}")),
    };
    let tail = tail_lines(&content, lines);
    succeed(
        json,
        json!({
            "ok": true,
            "path": path.display().to_string(),
            "lines": tail.lines().collect::<Vec<_>>(),
        }),
        || {
            if tail.is_empty() {
                println!("（日志为空）");
            } else {
                println!("{tail}");
            }
        },
    )
}

fn cmd_path(json: bool) -> ExitCode {
    let data_dir = talksage_config::default_data_dir();
    let config_file = talksage_config::default_config_file(&data_dir);
    let log_dir = talksage_logging::log_dir(Some(&data_dir));
    succeed(
        json,
        json!({
            "ok": true,
            "config_file": config_file.display().to_string(),
            "data_dir": data_dir.display().to_string(),
            "log_dir": log_dir.display().to_string(),
        }),
        || {
            println!("config file : {}", config_file.display());
            println!("data dir    : {}", data_dir.display());
            println!("log dir     : {}", log_dir.display());
        },
    )
}

fn cmd_get(path: Option<&str>, json: bool) -> ExitCode {
    let snap = match load_snapshot() {
        Ok(s) => s,
        Err(e) => return fail(json, e),
    };
    let view = masked_config(&snap);
    let target = match path {
        Some(p) if !p.trim().is_empty() => match value_at(&view, p.trim()) {
            Ok(v) => v.clone(),
            Err(e) => return fail(json, e),
        },
        _ => view.clone(),
    };
    succeed(
        json,
        json!({
            "ok": true,
            "path": path.filter(|p| !p.trim().is_empty()),
            "value": target,
        }),
        || match path.filter(|p| !p.trim().is_empty()) {
            Some(_) => println!("{}", format_value(&target)),
            None => match serde_json::to_string_pretty(&target) {
                Ok(s) => println!("{s}"),
                Err(e) => eprintln!("序列化配置失败: {e}"),
            },
        },
    )
}

fn cmd_set(path: &str, raw: &str, json: bool) -> ExitCode {
    let path = path.trim();
    if path.is_empty() {
        return fail(json, "配置路径不能为空".into());
    }
    let nested = match nest_path(path, parse_cli_value(raw)) {
        Ok(v) => v,
        Err(e) => return fail(json, e),
    };
    let mgr = match talksage_config::ConfigManager::load(None, None) {
        Ok(m) => m,
        Err(e) => return fail(json, format!("配置加载失败: {e}")),
    };
    let mut probe = mgr.snapshot();
    talksage_config::apply_updates(&mut probe, &nested);
    if value_at(&masked_config(&probe), path).is_err() {
        return fail(
            json,
            format!("无法写入 {path}（未知配置项，或该路径不支持通过 CLI 修改）"),
        );
    }
    if let Err(e) = mgr.update(|c| talksage_config::apply_updates(c, &nested)) {
        return fail(json, format!("保存配置失败: {e}"));
    }
    let stored = value_at(&masked_config(&mgr.snapshot()), path)
        .cloned()
        .unwrap_or(Value::Null);
    succeed(
        json,
        json!({
            "ok": true,
            "path": path,
            "value": stored,
        }),
        || println!("{path} = {}", format_value(&stored)),
    )
}

fn load_snapshot() -> Result<talksage_config::Config, String> {
    talksage_config::ConfigManager::load(None, None)
        .map(|m| m.snapshot())
        .map_err(|e| format!("配置加载失败: {e}"))
}

fn masked_config(config: &talksage_config::Config) -> Value {
    talksage_config::ui_config_json(
        config,
        talksage_plugins::effective_plugin_configs(&config.plugins.entries),
        SecretPolicy::Mask,
    )
}

fn fail(json: bool, msg: String) -> ExitCode {
    if json {
        eprintln!("{}", json!({"ok": false, "error": msg}));
    } else {
        eprintln!("{msg}");
    }
    ExitCode::FAILURE
}

fn succeed(json: bool, value: Value, text: impl FnOnce()) -> ExitCode {
    if json {
        println!("{value}");
    } else {
        text();
    }
    ExitCode::SUCCESS
}

fn format_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

fn parse_cli_value(raw: &str) -> Value {
    let t = raw.trim();
    if t.eq_ignore_ascii_case("true") {
        return json!(true);
    }
    if t.eq_ignore_ascii_case("false") {
        return json!(false);
    }
    if t.eq_ignore_ascii_case("null") {
        return Value::Null;
    }
    if let Ok(n) = t.parse::<i64>() {
        return json!(n);
    }
    if let Ok(n) = t.parse::<f64>() {
        return json!(n);
    }
    if (t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']')) {
        if let Ok(v) = serde_json::from_str::<Value>(t) {
            return v;
        }
    }
    json!(t)
}

fn nest_path(path: &str, value: Value) -> Result<Value, String> {
    let parts: Vec<&str> = path.split('.').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err("配置路径不能为空".into());
    }
    let mut acc = value;
    for part in parts.into_iter().rev() {
        acc = json!({ part: acc });
    }
    Ok(acc)
}

fn value_at<'a>(root: &'a Value, path: &str) -> Result<&'a Value, String> {
    if path.trim().is_empty() {
        return Ok(root);
    }
    let mut cur = root;
    for part in path.split('.') {
        if part.is_empty() {
            return Err(format!("无效配置路径: {path}"));
        }
        cur = cur
            .get(part)
            .ok_or_else(|| format!("没有配置项: {path}"))?;
    }
    Ok(cur)
}

fn latest_log_file(dir: &Path) -> Result<Option<PathBuf>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取日志目录失败: {e}")),
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("talksage") && name.ends_with(".log")
        })
        .collect();
    if files.is_empty() {
        return Ok(None);
    }
    files.sort_by_key(|e| {
        e.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    Ok(files.last().map(|e| e.path()))
}

fn tail_lines(content: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_value_kinds() {
        assert_eq!(parse_cli_value("true"), json!(true));
        assert_eq!(parse_cli_value("false"), json!(false));
        assert_eq!(parse_cli_value("12"), json!(12));
        assert_eq!(parse_cli_value("1.5"), json!(1.5));
        assert_eq!(parse_cli_value("qwen3-asr"), json!("qwen3-asr"));
        assert_eq!(parse_cli_value(r#"["a","b"]"#), json!(["a", "b"]));
    }

    #[test]
    fn nest_and_get_dotted_path() {
        let nested = nest_path("asr.engine_zh", json!("qwen3-asr")).unwrap();
        assert_eq!(nested, json!({"asr": {"engine_zh": "qwen3-asr"}}));
        assert_eq!(
            value_at(&nested, "asr.engine_zh").unwrap(),
            &json!("qwen3-asr")
        );
        assert!(value_at(&nested, "asr.missing").is_err());
        assert!(nest_path("", json!(1)).is_err());
    }

    #[test]
    fn tail_lines_keeps_last_n() {
        let content = "a\nb\nc\nd\n";
        assert_eq!(tail_lines(content, 2), "c\nd");
        assert_eq!(tail_lines(content, 10), "a\nb\nc\nd");
        assert_eq!(tail_lines(content, 0), "");
    }

    #[test]
    fn secrets_are_masked_in_ui_json() {
        let mut cfg = talksage_config::Config::default();
        cfg.llm.providers.get_mut("deepseek").unwrap().api_key = "sk-abcdefghijklmnop".into();
        let view = masked_config(&cfg);
        let key = value_at(&view, "llm.providers.deepseek.api_key")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(key, talksage_config::mask_secret("sk-abcdefghijklmnop"));
        assert!(!key.contains("efghijkl"));
    }

    #[test]
    fn apply_updates_dotted_path_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "talksage-config-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = talksage_config::ConfigManager::from_config(talksage_config::Config::default(), dir.clone());
        let nested = nest_path("asr.engine_zh", json!("whisper-medium-metal")).unwrap();
        mgr.update(|c| talksage_config::apply_updates(c, &nested)).unwrap();
        assert_eq!(mgr.snapshot().asr.engine_zh, "whisper-medium-metal");

        let mut cfg = talksage_config::Config::default();
        cfg.llm.providers.get_mut("deepseek").unwrap().api_key = "sk-abcdefghijklmnop".into();
        let mgr = talksage_config::ConfigManager::from_config(cfg, dir.clone());
        let mask = talksage_config::mask_secret("sk-abcdefghijklmnop");
        let nested = nest_path("llm.providers.deepseek.api_key", json!(mask)).unwrap();
        mgr.update(|c| talksage_config::apply_updates(c, &nested)).unwrap();
        assert_eq!(
            mgr.snapshot().llm.providers["deepseek"].api_key,
            "sk-abcdefghijklmnop"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_path_rejected_after_apply() {
        let mut cfg = talksage_config::Config::default();
        let nested = nest_path("asr.not_a_field", json!("x")).unwrap();
        talksage_config::apply_updates(&mut cfg, &nested);
        assert!(value_at(&masked_config(&cfg), "asr.not_a_field").is_err());
        assert!(value_at(&masked_config(&cfg), "asr.engine_zh").is_ok());
    }
}
