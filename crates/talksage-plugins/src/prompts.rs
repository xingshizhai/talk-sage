//! 会中插件的 LLM prompt。正文在 crate 根目录 `prompts/`，此处仅 `include_str!`。
//!
//! 归属约定：谁调用、谁持有。宿主只注入 `LLMProvider`，不设中央 prompt 目录。
//! 新 LLM 插件把 `<id>_system.txt` / `<id>_user.txt` 放在本目录，文件名对齐插件 id。

pub const TERM_EXPLAINER_SYSTEM: &str = include_str!("../prompts/term_explainer_system.txt");
pub const TERM_EXPLAINER_USER: &str = include_str!("../prompts/term_explainer_user.txt");
pub const TERM_LOOKUP_SYSTEM: &str = include_str!("../prompts/term_lookup_system.txt");
pub const TERM_LOOKUP_USER: &str = include_str!("../prompts/term_lookup_user.txt");
pub const TRANSLATOR_SYSTEM: &str = include_str!("../prompts/translator_system.txt");
pub const TRANSLATOR_USER: &str = include_str!("../prompts/translator_user.txt");
