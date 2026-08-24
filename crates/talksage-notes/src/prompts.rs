//! 纪要 / 三段式 / 要点整理的 LLM prompt。正文在 crate 根目录 `prompts/`，此处仅 `include_str!`。
//!
//! 会后生成器不是 plugin；文案留在本 crate，不与会中插件 prompt 混放。
//! 插值用 `talksage_llm::render_prompt`。模板分节 instruction 仍在 `builtin_templates()`。

pub const NOTES_SYSTEM: &str = include_str!("../prompts/notes_system.txt");
pub const NOTES_USER: &str = include_str!("../prompts/notes_user.txt");
pub const TRIO_OVERVIEW_SYSTEM: &str = include_str!("../prompts/trio_overview_system.txt");
pub const TRIO_KEY_POINTS_SYSTEM: &str = include_str!("../prompts/trio_key_points_system.txt");
pub const TRIO_ACTION_ITEMS_SYSTEM: &str = include_str!("../prompts/trio_action_items_system.txt");
pub const TRIO_USER: &str = include_str!("../prompts/trio_user.txt");
pub const HIGHLIGHTS_SYSTEM: &str = include_str!("../prompts/highlights_system.txt");
pub const HIGHLIGHTS_USER_FROM_KEY_POINTS: &str =
    include_str!("../prompts/highlights_user_from_key_points.txt");
pub const HIGHLIGHTS_USER_FROM_TRANSCRIPT: &str =
    include_str!("../prompts/highlights_user_from_transcript.txt");
