//! 进程内知识库索引：由源插件配置驱动，给会中 / 纪要 / 助手共用。

use std::path::{Path, PathBuf};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use talksage_config::ConfigManager;
use talksage_knowledge::{
    format_knowledge_block, KnowledgeBase, KnowledgeDocument, KnowledgeSource, KBHit, ObsidianSource,
};
use talksage_core::{DomainEvent, KnowledgeHit as EventKnowledgeHit, KeyPointCategory, ResultStatus};

const SEARCH_MIN_SCORE: f32 = 0.05;

/// 会话级知识检索协调器：只消费已经形成语义的 final 要点/术语，以及明确问句。
/// 文档目录本身不是“命中”，不会从这里发往实时界面。
pub struct LiveKnowledgeRetriever {
    kb: Arc<KnowledgeBase>,
    pinned: HashSet<String>,
    seen_queries: Mutex<HashSet<String>>,
}

impl LiveKnowledgeRetriever {
    pub fn new(kb: Arc<KnowledgeBase>, pinned_paths: &[String]) -> Self {
        Self {
            kb,
            pinned: pinned_paths.iter().cloned().collect(),
            seen_queries: Mutex::new(HashSet::new()),
        }
    }

    pub fn observe(&self, event: &DomainEvent) -> Option<DomainEvent> {
        let (trigger, raw_query) = match event {
            DomainEvent::KeyPoint { status: ResultStatus::Final, category, content, .. } => {
                let category = match category {
                    KeyPointCategory::Requirement => "要求",
                    KeyPointCategory::Technical => "技术",
                    KeyPointCategory::Question => "问题",
                    KeyPointCategory::Decision => "决策",
                    KeyPointCategory::Action => "行动",
                    KeyPointCategory::Other => "要点",
                };
                ("key_point", format!("{category} {content}"))
            }
            DomainEvent::Term { status: ResultStatus::Final, content, .. } if !content.trim().is_empty() => {
                // 展示格式通常是“术语：解释”；检索以术语为主，避免长篇通用解释稀释关键词。
                let terms = content.lines().filter_map(|line| {
                    line.split_once(['：', ':']).map(|(term, _)| term).or(Some(line))
                }).map(str::trim).filter(|term| !term.is_empty()).collect::<Vec<_>>();
                ("term", terms.join(" "))
            }
            DomainEvent::Segment { is_partial: false, text, .. } if is_explicit_question(text) => {
                ("question", text.clone())
            }
            _ => return None,
        };
        let query = raw_query.trim().chars().take(240).collect::<String>();
        if query.chars().count() < 3 {
            return None;
        }
        let fingerprint = normalize_query(&query);
        if fingerprint.is_empty() || !self.seen_queries.lock().unwrap().insert(fingerprint.clone()) {
            return None;
        }

        let (scope, hits) = if self.pinned.is_empty() {
            ("all", self.kb.search(&query, 3, SEARCH_MIN_SCORE))
        } else {
            let pinned = self.kb.search_in_paths(&query, &self.pinned, 3, SEARCH_MIN_SCORE);
            if pinned.is_empty() {
                ("pinned_then_all", self.kb.search(&query, 3, SEARCH_MIN_SCORE))
            } else {
                ("pinned", pinned)
            }
        };
        let hits = hits.into_iter().map(|hit| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            hit.source.hash(&mut hasher);
            hit.heading.hash(&mut hasher);
            hit.text.hash(&mut hasher);
            EventKnowledgeHit {
                hit_id: format!("kb-{:016x}", hasher.finish()),
                pinned: self.pinned.contains(&hit.source),
                path: hit.source,
                heading: hit.heading,
                excerpt: hit.text.chars().take(360).collect(),
                score: hit.score,
            }
        }).collect();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        trigger.hash(&mut hasher);
        fingerprint.hash(&mut hasher);
        Some(DomainEvent::KnowledgeQuery {
            query_id: format!("query-{:016x}", hasher.finish()),
            trigger: trigger.into(),
            query,
            scope: scope.into(),
            hits,
        })
    }
}

fn normalize_query(query: &str) -> String {
    query.chars()
        .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation() && !matches!(c, '，' | '。' | '？' | '！' | '：' | '；'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_explicit_question(text: &str) -> bool {
    let text = text.trim();
    text.chars().count() >= 8
        && (text.contains(['?', '？'])
            || ["是什么", "为什么", "怎么", "多少", "什么时候", "是否", "能否", "哪一个"]
                .iter()
                .any(|marker| text.contains(marker)))
}

/// 配置驱动的知识索引。指纹变化才重读磁盘。
pub struct KnowledgeHub {
    config: Arc<ConfigManager>,
    index: Mutex<Arc<KnowledgeBase>>,
    fingerprint: Mutex<String>,
}

impl KnowledgeHub {
    pub fn new(config: Arc<ConfigManager>) -> Self {
        Self {
            config,
            index: Mutex::new(Arc::new(KnowledgeBase::new())),
            fingerprint: Mutex::new(String::new()),
        }
    }

    /// 源开关与路径。CLI `kb_folder_override` 视为强制启用该目录。
    pub fn source_folder(config: &talksage_config::Config, folder_override: Option<&Path>) -> (bool, PathBuf) {
        if let Some(path) = folder_override {
            return (true, path.to_path_buf());
        }
        let folder = config.plugins.get_str("knowledge_obsidian", "folder", "");
        let enabled = config.plugins.get_bool("knowledge_obsidian", "enabled", false);
        if !folder.is_empty() {
            return (enabled, PathBuf::from(folder));
        }
        (
            config.knowledge_base.enabled,
            PathBuf::from(&config.knowledge_base.folder),
        )
    }

    pub fn refresh(&self) {
        self.refresh_with_folder(None);
    }

    pub fn refresh_with_folder(&self, folder_override: Option<&Path>) {
        let snapshot = self.config.snapshot();
        let (enabled, folder) = Self::source_folder(&snapshot, folder_override);
        let fp = format!("{enabled}|{}", folder.display());
        {
            let mut prev = self.fingerprint.lock().unwrap();
            if *prev == fp && folder_override.is_none() {
                return;
            }
            *prev = fp;
        }
        let mut kb = KnowledgeBase::new();
        if enabled && !folder.as_os_str().is_empty() {
            match ObsidianSource::new(&folder).load_snippets() {
                Ok(snippets) => {
                    kb.rebuild(&snippets);
                }
                Err(e) => {
                    log::warn!("知识源刷新失败: {e}");
                }
            }
        }
        *self.index.lock().unwrap() = Arc::new(kb);
    }

    pub fn refresh_if_stale(&self) {
        let snapshot = self.config.snapshot();
        let (enabled, folder) = Self::source_folder(&snapshot, None);
        let fp = format!("{enabled}|{}", folder.display());
        if *self.fingerprint.lock().unwrap() != fp {
            self.refresh();
        }
    }

    pub fn invalidate(&self) {
        *self.fingerprint.lock().unwrap() = String::new();
    }

    pub fn is_ready(&self) -> bool {
        self.index.lock().unwrap().is_ready()
    }

    pub fn index(&self) -> Arc<KnowledgeBase> {
        self.index.lock().unwrap().clone()
    }

    pub fn search(&self, query: &str, top_k: usize) -> Vec<KBHit> {
        self.refresh_if_stale();
        self.index.lock().unwrap().search(query, top_k, SEARCH_MIN_SCORE)
    }

    pub fn list_documents(&self) -> Vec<KnowledgeDocument> {
        self.refresh_if_stale();
        self.index.lock().unwrap().list_documents()
    }

    pub fn text_for_path(&self, path: &str) -> Option<String> {
        self.refresh_if_stale();
        self.index.lock().unwrap().text_for_path(path)
    }

    pub fn block_for_query(&self, query: &str, top_k: usize) -> String {
        format_knowledge_block(&self.search(query, top_k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_config::{Config, ConfigManager};

    fn vault(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("talksage-hub-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("npi.md"), "# NPI\n\n客户关注样品交期与最小起订量。").unwrap();
        d
    }

    fn hub_with(enabled: bool, folder: &str) -> (KnowledgeHub, std::path::PathBuf) {
        let data = std::env::temp_dir().join(format!("talksage-hub-data-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&data);
        let mut cfg = Config::default();
        cfg.plugins.merge_entry(
            "knowledge_obsidian",
            &serde_json::json!({ "enabled": enabled, "folder": folder }),
        );
        talksage_config::sync_knowledge_source(&mut cfg);
        let hub = KnowledgeHub::new(Arc::new(ConfigManager::from_config(cfg, data.clone())));
        (hub, data)
    }

    #[test]
    fn refresh_indexes_enabled_vault_and_clears_when_disabled() {
        let dir = vault("on");
        let (hub, _) = hub_with(true, &dir.to_string_lossy());
        hub.refresh();
        assert!(hub.is_ready());
        assert!(!hub.search("样品交期", 2).is_empty());

        let (hub_off, _) = hub_with(false, &dir.to_string_lossy());
        hub_off.refresh();
        assert!(!hub_off.is_ready());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn folder_override_indexes_even_if_plugin_disabled() {
        let dir = vault("ov");
        let (hub, _) = hub_with(false, "");
        hub.refresh_with_folder(Some(&dir));
        assert!(hub.is_ready());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn live_retrieval_is_triggered_by_final_terms_and_deduplicates_queries() {
        let dir = vault("live-query");
        let mut kb = KnowledgeBase::new();
        kb.index_folder(&dir);
        let retriever = LiveKnowledgeRetriever::new(
            Arc::new(kb),
            &["npi.md".to_string()],
        );
        let event = DomainEvent::Term {
            result_id: "term-1".into(),
            status: ResultStatus::Final,
            content: "样品交期：首批样品的计划交付日期".into(),
        };
        let first = retriever.observe(&event).expect("final 术语应触发查询");
        match first {
            DomainEvent::KnowledgeQuery { trigger, scope, hits, .. } => {
                assert_eq!(trigger, "term");
                assert_eq!(scope, "pinned");
                assert!(!hits.is_empty());
                assert!(hits.iter().all(|hit| hit.pinned));
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(retriever.observe(&event).is_none(), "相同查询在本场只执行一次");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn partial_or_plain_statements_do_not_trigger_live_retrieval() {
        let retriever = LiveKnowledgeRetriever::new(Arc::new(KnowledgeBase::new()), &[]);
        let segment = |is_partial, text: &str| DomainEvent::Segment {
            speaker_id: 0,
            speaker_label: "我".into(),
            speaker_attribution: None,
            text: text.into(),
            is_partial,
            ts_ms: 0,
            duration_ms: 0,
            rms: 0.0,
            revision: 0,
            start_sample: 0,
            end_sample: 0,
        };
        assert!(retriever.observe(&segment(true, "样品交期是多少？")).is_none());
        assert!(retriever.observe(&segment(false, "我们继续讨论样品交期")).is_none());
        assert!(retriever.observe(&segment(false, "请问样品交期是多少？")).is_some());
    }
}
