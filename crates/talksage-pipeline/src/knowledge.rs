//! 进程内知识库索引：由源插件配置驱动，给会中 / 纪要 / 助手共用。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use talksage_config::ConfigManager;
use talksage_knowledge::{
    format_knowledge_block, KnowledgeBase, KnowledgeDocument, KnowledgeSource, KBHit, ObsidianSource,
};

const SEARCH_MIN_SCORE: f32 = 0.05;

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
}
