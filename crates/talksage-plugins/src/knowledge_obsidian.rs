//! 知识源插件：本地 Obsidian 单 vault。不订阅转写段，只提供配置身份。

use super::{PluginContext, PluginCategory, PluginPhase};

pub struct KnowledgeObsidianPlugin;

impl crate::registry::Plugin for KnowledgeObsidianPlugin {
    fn descriptor(&self) -> &'static crate::PluginDescriptor {
        static D: crate::PluginDescriptor = crate::PluginDescriptor {
            id: "knowledge_obsidian",
            label: "Obsidian 仓库",
            description: "本地 Obsidian vault（一个根路径）作为知识源；会中材料包、纪要与助手共用",
            category: PluginCategory::KnowledgeSource,
            phase: PluginPhase::Source,
            capabilities: &[],
            host_managed: &[],
            after: &[],
        };
        &D
    }

    fn default_config(&self) -> crate::registry::PluginConfig {
        crate::registry::PluginConfig::from_value(serde_json::json!({
            "enabled": false,
            "folder": "",
        }))
    }

    fn register(
        &self,
        _cfg: &crate::registry::PluginConfig,
        _ctx: &PluginContext,
        _hooks: &mut crate::registry::HookRegistry,
    ) {
        // 源插件不挂转写钩子；索引由 KnowledgeHub 读取本配置完成。
    }
}
