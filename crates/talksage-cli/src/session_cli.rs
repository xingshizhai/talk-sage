//! 会话子命令：list / show / search / rename / delete / export / notes / trio。

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;
use talksage_pipeline::TalkSageService;
use talksage_session::{SessionRecord, SessionStore};

use crate::args::{ExportFormat, SessionAction, SessionArgs};

pub fn dispatch(args: SessionArgs, json: bool) -> ExitCode {
    match args.resolve() {
        Ok(action) => run(action, json, ExportDest::SessionDir),
        Err(msg) => fail(json, msg),
    }
}

pub fn list_alias(limit: u32, json: bool) -> ExitCode {
    run(SessionAction::List { limit }, json, ExportDest::SessionDir)
}

/// 顶层 `export`：默认写当前目录，保持旧脚本行为。
pub fn export_alias(id: Option<i64>, output: Option<String>, json: bool) -> ExitCode {
    run(
        SessionAction::Export {
            id,
            format: ExportFormat::Md,
            output,
        },
        json,
        ExportDest::Cwd,
    )
}

#[derive(Clone, Copy)]
enum ExportDest {
    /// `session export`：未指定 -o 时写入 `<data>/sessions/<id>/exports/`
    SessionDir,
    /// 顶层 `export`：未指定 -o 时写入当前目录 `session-<id>.md`
    Cwd,
}

fn run(action: SessionAction, json: bool, dest: ExportDest) -> ExitCode {
    match action {
        SessionAction::List { limit } => cmd_list(limit, json),
        SessionAction::Show { id, dup_only } => cmd_show(id, dup_only, json),
        SessionAction::Search { query, limit } => cmd_search(&query, limit, json),
        SessionAction::Rename { id, title } => cmd_rename(id, &title, json),
        SessionAction::Delete { id, yes } => cmd_delete(id, yes, json),
        SessionAction::Export { id, format, output } => cmd_export(id, format, output.as_deref(), json, dest),
        SessionAction::Notes { id, template } => cmd_notes(id, &template, json),
        SessionAction::Trio { id, name, desc } => cmd_trio(id, name.as_deref(), desc.as_deref(), json),
    }
}

fn open_store() -> Result<SessionStore, String> {
    let db = talksage_config::default_data_dir().join("sessions.db");
    SessionStore::open(&db.to_string_lossy()).map_err(|e| format!("打开会话库失败: {e}（数据库: {}）", db.display()))
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

pub fn list_json(rows: &[SessionRecord]) -> serde_json::Value {
    json!({ "sessions": rows })
}

fn cmd_list(limit: u32, json: bool) -> ExitCode {
    let store = match open_store() {
        Ok(s) => s,
        Err(e) => return fail(json, e),
    };
    let list = match store.list_sessions(limit) {
        Ok(l) => l,
        Err(e) => return fail(json, format!("列出会话失败: {e}")),
    };
    succeed(json, list_json(&list), || {
        if list.is_empty() {
            println!("（无会话记录）");
            return;
        }
        println!("{:>5}  {:<20}  {:>8}  {}", "ID", "开始时间", "时长(s)", "摘要");
        println!("{}", "-".repeat(60));
        for r in &list {
            let started = chrono_fmt(r.started_at);
            let duration = r.ended_at.map(|e| (e - r.started_at).max(0)).unwrap_or(0);
            let title = r.title.as_deref().unwrap_or("-");
            let quality = r.quality.as_deref().unwrap_or("-");
            println!(
                "{:>5}  {:<20}  {:>8}  {} segs={} quality={}",
                r.id, started, duration, title, r.segment_count, quality
            );
        }
    })
}

fn cmd_show(id: i64, dup_only: bool, json: bool) -> ExitCode {
    let store = match open_store() {
        Ok(s) => s,
        Err(e) => return fail(json, e),
    };
    let detail = match store.get_session(id) {
        Ok(d) => d,
        Err(e) => return fail(json, format!("读取会话 #{id} 失败: {e}")),
    };
    let dups = talksage_session::find_duplicate_segments(&detail.segments);
    if json {
        let trio = detail
            .trio
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
        println!(
            "{}",
            json!({
                "id": detail.id,
                "title": detail.title,
                "started_at": detail.started_at,
                "ended_at": detail.ended_at,
                "notes": detail.notes,
                "trio": trio,
                "quality": detail.meta.as_ref().map(|m| m.quality_label()),
                "segments": detail.segments.iter().map(|s| json!({
                    "speaker_label": s.speaker_label,
                    "text": s.text,
                    "ts_ms": s.ts_ms,
                    "duration_ms": s.duration_ms,
                })).collect::<Vec<_>>(),
                "duplicates": dups.iter().map(|d| json!({
                    "idx_a": d.idx_a,
                    "idx_b": d.idx_b,
                    "speaker": d.speaker,
                    "similarity": d.similarity,
                    "gap_ms": d.gap_ms,
                })).collect::<Vec<_>>(),
            })
        );
        return ExitCode::SUCCESS;
    }

    println!("== 会话 #{id} 原始转写段 ==");
    println!(
        "{} 段 · 开始 {} · 结束 {:?} · 质量 {:?}",
        detail.segments.len(),
        detail.started_at,
        detail.ended_at,
        detail.meta.as_ref().map(|m| m.quality_label()),
    );
    if let Some(title) = &detail.title {
        println!("标题: {title}");
    }
    if let Some(ri) = detail.meta.as_ref().and_then(|m| m.runtime_info.as_ref()) {
        println!(
            "运行环境: v{} 场景={} 引擎={}{} VAD={} 降噪={} 最短提交={}ms 增益={}dB 说话人={} 采样率={}Hz",
            ri.app_version,
            ri.scene_mode,
            ri.user_engine,
            if ri.client_enabled {
                format!("+{}", ri.client_engine.as_deref().unwrap_or("?"))
            } else {
                "（单流）".into()
            },
            ri.vad_preset,
            if ri.denoise_enabled { "开" } else { "关" },
            ri.min_segment_ms,
            ri.input_gain_db,
            ri.speaker_mode,
            ri.sample_rate,
        );
    } else {
        println!("运行环境: （旧会话，无配置快照）");
    }
    if !dup_only {
        for (i, s) in detail.segments.iter().enumerate() {
            let start_ms = s.ts_ms.saturating_sub(s.duration_ms);
            println!(
                "  #{:<3} [{}] start={:>8}ms end={:>8}ms dur={:>5}ms | {}",
                i, s.speaker_label, start_ms, s.ts_ms, s.duration_ms, s.text,
            );
        }
    }
    if dups.is_empty() {
        println!("\n疑似重复段: 无（同说话人相邻段相似度均 < 0.9）");
    } else {
        println!("\n疑似重复段（同说话人、时间窗 5s 内、相似度 ≥ 0.9）:");
        for d in &dups {
            println!(
                "  #{:<3} 与 #{:<3} [{}] 相似度={:.2} 间隔={}ms",
                d.idx_a, d.idx_b, d.speaker, d.similarity, d.gap_ms
            );
            println!("    A: {}", detail.segments[d.idx_a].text);
            println!("    B: {}", detail.segments[d.idx_b].text);
        }
    }
    ExitCode::SUCCESS
}

fn cmd_search(query: &str, limit: u32, json: bool) -> ExitCode {
    let store = match open_store() {
        Ok(s) => s,
        Err(e) => return fail(json, e),
    };
    let q = query.trim();
    if q.is_empty() {
        return fail(json, "搜索关键词不能为空".into());
    }
    let hits = match store.search(q, limit) {
        Ok(h) => h,
        Err(e) => return fail(json, format!("搜索失败: {e}")),
    };
    succeed(json, json!({ "query": q, "hits": hits }), || {
        if hits.is_empty() {
            println!("（无命中）");
            return;
        }
        println!("{:>8}  {:<8}  {}", "会话", "说话人", "文本");
        println!("{}", "-".repeat(60));
        for h in &hits {
            println!("{:>8}  {:<8}  {}", h.session_id, h.speaker_label, h.text);
        }
    })
}

fn cmd_rename(id: i64, title: &str, json: bool) -> ExitCode {
    let store = match open_store() {
        Ok(s) => s,
        Err(e) => return fail(json, e),
    };
    if title.trim().is_empty() {
        return fail(json, "标题不能为空".into());
    }
    if let Err(e) = store.get_session(id) {
        return fail(json, format!("读取会话 #{id} 失败: {e}"));
    }
    if let Err(e) = store.set_session_title(id, title) {
        return fail(json, format!("重命名失败: {e}"));
    }
    succeed(json, json!({"ok": true, "id": id, "title": title}), || {
        println!("已将会话 #{id} 重命名为：{title}");
    })
}

fn cmd_delete(id: i64, yes: bool, json: bool) -> ExitCode {
    if !yes {
        return fail(
            json,
            format!("删除会话 #{id} 不可恢复，请加 --yes 确认（只删数据库记录，录音文件仍保留）"),
        );
    }
    let store = match open_store() {
        Ok(s) => s,
        Err(e) => return fail(json, e),
    };
    if let Err(e) = store.get_session(id) {
        return fail(json, format!("读取会话 #{id} 失败: {e}"));
    }
    if let Err(e) = store.delete_session(id) {
        return fail(json, format!("删除失败: {e}"));
    }
    succeed(json, json!({"ok": true, "id": id}), || {
        println!("已删除会话 #{id}");
    })
}

fn resolve_session_id(store: &SessionStore, id: Option<i64>) -> Result<i64, String> {
    match id {
        Some(i) => Ok(i),
        None => match store.list_sessions(1) {
            Ok(list) if !list.is_empty() => Ok(list[0].id),
            _ => Err("无会话记录".into()),
        },
    }
}

fn cmd_export(
    id: Option<i64>,
    format: ExportFormat,
    output: Option<&str>,
    json: bool,
    dest: ExportDest,
) -> ExitCode {
    let store = match open_store() {
        Ok(s) => s,
        Err(e) => return fail(json, e),
    };
    let session_id = match resolve_session_id(&store, id) {
        Ok(i) => i,
        Err(e) => return fail(json, e),
    };
    let detail = match store.get_session(session_id) {
        Ok(d) => d,
        Err(e) => return fail(json, format!("读取会话 #{session_id} 失败: {e}")),
    };
    let data_dir = talksage_config::default_data_dir();
    let default_path = match dest {
        ExportDest::Cwd => PathBuf::from(format!("session-{session_id}.{}", format.ext())),
        ExportDest::SessionDir => {
            talksage_config::session_exports_dir(&data_dir, session_id).join(format!("session-{session_id}.{}", format.ext()))
        }
    };
    let path = output.map(PathBuf::from).unwrap_or(default_path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return fail(json, format!("创建导出目录失败: {e}"));
            }
        }
    }

    let write_result = match format {
        ExportFormat::Md => std::fs::write(&path, talksage_session::export_markdown(&detail)).map(|_| ()),
        ExportFormat::Txt => std::fs::write(&path, talksage_session::export_transcript_text(&detail)).map(|_| ()),
        ExportFormat::Audio => {
            let master = match detail.meta.as_ref().and_then(|m| m.master_recording.clone()) {
                Some(p) => p,
                None => {
                    return fail(
                        json,
                        "该会话没有完整录音（可能未开启录音，或录音文件缺失）".into(),
                    );
                }
            };
            let src = PathBuf::from(&master);
            if !src.is_file() {
                return fail(json, format!("录音文件不存在: {}", src.display()));
            }
            std::fs::copy(&src, &path).map(|_| ())
        }
    };
    if let Err(e) = write_result {
        return fail(json, format!("写入 {} 失败: {e}", path.display()));
    }
    succeed(
        json,
        json!({
            "ok": true,
            "id": session_id,
            "format": format.as_str(),
            "path": path.display().to_string(),
        }),
        || {
            println!(
                "已导出会话 #{session_id}（{} 段，{}）→ {}",
                detail.segments.len(),
                format.as_str(),
                path.display()
            );
        },
    )
}

fn cmd_notes(id: i64, template_id: &str, json: bool) -> ExitCode {
    let store = match open_store() {
        Ok(s) => s,
        Err(e) => return fail(json, e),
    };
    let detail = match store.get_session(id) {
        Ok(d) => d,
        Err(e) => return fail(json, format!("读取会话 #{id} 失败: {e}")),
    };
    let Some(template) = talksage_notes::get_template(template_id) else {
        let known = talksage_notes::builtin_templates()
            .into_iter()
            .map(|t| format!("{}（{}）", t.id, t.name))
            .collect::<Vec<_>>()
            .join("、");
        return fail(json, format!("未知模板: {template_id}。可选：{known}"));
    };
    let mgr = match talksage_config::ConfigManager::load(None, None) {
        Ok(m) => std::sync::Arc::new(m),
        Err(e) => return fail(json, format!("配置加载失败: {e}")),
    };
    let Some(llm) = TalkSageService::build_llm(&mgr) else {
        return fail(
            json,
            "未配置 LLM（请设置 llm.providers.<provider>.api_key）".into(),
        );
    };
    let gen = talksage_notes::NotesGenerator::new(llm);
    let notes = match gen.generate(
        &detail.segments,
        &detail.terms,
        &detail.translations,
        &detail.key_points,
        &template,
    ) {
        Ok(n) => n,
        Err(e) => return fail(json, format!("纪要生成失败: {e}")),
    };
    if let Err(e) = store.set_notes(id, &notes) {
        return fail(json, format!("保存纪要失败: {e}"));
    }
    succeed(
        json,
        json!({"ok": true, "id": id, "template": template_id, "notes": notes}),
        || {
            println!("已生成并保存会话 #{id} 纪要（模板 {template_id}）：\n{notes}");
        },
    )
}

fn cmd_trio(id: i64, name: Option<&str>, desc: Option<&str>, json: bool) -> ExitCode {
    let store = match open_store() {
        Ok(s) => s,
        Err(e) => return fail(json, e),
    };
    let detail = match store.get_session(id) {
        Ok(d) => d,
        Err(e) => return fail(json, format!("读取会话 #{id} 失败: {e}")),
    };
    let mgr = match talksage_config::ConfigManager::load(None, None) {
        Ok(m) => std::sync::Arc::new(m),
        Err(e) => return fail(json, format!("配置加载失败: {e}")),
    };
    let Some(llm) = TalkSageService::build_llm(&mgr) else {
        return fail(
            json,
            "未配置 LLM（请设置 llm.providers.<provider>.api_key）".into(),
        );
    };
    let gen = talksage_notes::TrioGenerator::new(llm);
    let trio = match gen.generate(&detail.segments, &detail.key_points, name, desc) {
        Ok(t) => t,
        Err(e) => return fail(json, format!("智能纪要生成失败: {e}")),
    };
    let value = match serde_json::to_value(&trio) {
        Ok(v) => v,
        Err(e) => return fail(json, format!("序列化纪要失败: {e}")),
    };
    if let Err(e) = store.set_trio(id, &value.to_string()) {
        return fail(json, format!("保存智能纪要失败: {e}"));
    }
    succeed(
        json,
        json!({"ok": true, "id": id, "trio": value}),
        || {
            println!("已生成并保存会话 #{id} 三段式纪要：");
            println!("\n## 概述\n{}", trio.short_overview);
            println!("\n## 要点");
            for kp in &trio.key_points {
                println!("- {}：{}", kp.topic, kp.points.join("；"));
            }
            println!("\n## 行动项");
            for item in &trio.action_items {
                println!("- {item}");
            }
        },
    )
}

fn chrono_fmt(unix_secs: i64) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let Some(_t) = UNIX_EPOCH.checked_add(Duration::from_secs(unix_secs as u64)) else {
        return unix_secs.to_string();
    };
    let secs_today = unix_secs.rem_euclid(86400);
    let h = secs_today / 3600;
    let m = (secs_today % 3600) / 60;
    let s = secs_today % 60;
    let days = unix_secs.div_euclid(86400);
    let epoch_days = 719_162i64;
    let z = days + epoch_days;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_core::TranscriptSegment;

    fn seg(text: &str) -> TranscriptSegment {
        TranscriptSegment {
            speaker_id: 0,
            speaker_label: "我".into(),
            speaker_attribution: None,
            text: text.into(),
            is_partial: false,
            ts_ms: 1000,
            duration_ms: 800,
            rms: 0.2,
        }
    }

    #[test]
    fn list_json_includes_title() {
        let store = SessionStore::open(":memory:").unwrap();
        let id = store.start_session(1_700_000_000).unwrap();
        store.set_session_title(id, "合同评审").unwrap();
        let rows = store.list_sessions(20).unwrap();
        let v = list_json(&rows);
        assert_eq!(v["sessions"][0]["id"], id);
        assert_eq!(v["sessions"][0]["title"], "合同评审");
    }

    #[test]
    fn search_hits_contract_text() {
        let store = SessionStore::open(":memory:").unwrap();
        let id = store.start_session(1_700_000_000).unwrap();
        store.add_segment(id, &seg("下周签合同")).unwrap();
        store.add_segment(id, &seg("天气不错")).unwrap();
        let hits = store.search("合同", 50).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, id);
        assert!(hits[0].text.contains("合同"));
    }

    #[test]
    fn rename_and_delete_roundtrip() {
        let store = SessionStore::open(":memory:").unwrap();
        let id = store.start_session(1).unwrap();
        store.set_session_title(id, "旧名").unwrap();
        store.set_session_title(id, "新名").unwrap();
        assert_eq!(store.get_session(id).unwrap().title.as_deref(), Some("新名"));
        store.delete_session(id).unwrap();
        assert!(store.get_session(id).is_err());
    }
}
