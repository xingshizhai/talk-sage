//! TalkSage v2 纪要模板化。
//!
//! `Template`：结构化纪要模板（标题 + 分节指令 + 输出格式），
//! `NotesGenerator`：基于会话转写/术语/翻译，按模板分节指令生成 Markdown 纪要。
//!
//! 结构参考 Meetily summary/templates（title/instruction/format/item_format）。

use std::sync::Arc;

use anyhow::{anyhow, Result};
use talksage_core::TranscriptSegment;
use talksage_llm::LLMProvider;

/// 分节输出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionFormat {
    Paragraph,
    List,
    String,
}

impl SectionFormat {
    fn as_str(&self) -> &'static str {
        match self {
            SectionFormat::Paragraph => "paragraph",
            SectionFormat::List => "list",
            SectionFormat::String => "string",
        }
    }
}

/// 模板分节。
#[derive(Debug, Clone)]
pub struct TemplateSection {
    pub title: String,
    pub instruction: String,
    pub format: SectionFormat,
    /// 可选条目格式提示（如表格列头）。
    pub item_format: Option<String>,
}

/// 纪要模板。
#[derive(Debug, Clone)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub description: String,
    pub sections: Vec<TemplateSection>,
}

impl Template {
    /// 校验模板结构。
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() || self.name.trim().is_empty() {
            return Err(anyhow!("模板 id/name 不能为空"));
        }
        if self.sections.is_empty() {
            return Err(anyhow!("模板 '{name}' 至少需要一个分节", name = self.name));
        }
        for (i, s) in self.sections.iter().enumerate() {
            if s.title.trim().is_empty() || s.instruction.trim().is_empty() {
                return Err(anyhow!("模板 '{}' 分节 {} 标题/指令为空", self.name, i));
            }
        }
        Ok(())
    }

    /// 生成 Markdown 骨架（标题 + 各分节占位）。
    pub fn to_markdown_structure(&self) -> String {
        let mut md = String::from("# （会议标题）\n\n");
        for section in &self.sections {
            md.push_str(&format!("## {}\n\n", section.title));
        }
        md
    }

    /// 生成给 LLM 的分节指令文本。
    pub fn to_instructions(&self) -> String {
        let mut s = String::from("- 为整个纪要生成简洁标题（# 一级标题）。\n");
        for section in &self.sections {
            s.push_str(&format!("- 分节「{}」（{}）：{}\n", section.title, section.format.as_str(), section.instruction));
            if let Some(ifmt) = &section.item_format {
                s.push_str(&format!("  - 条目格式：`{ifmt}`\n"));
            }
        }
        s
    }
}

/// 内置模板清单。
pub fn builtin_templates() -> Vec<Template> {
    vec![
        Template {
            id: "standard_meeting".into(),
            name: "标准会议纪要".into(),
            description: "常规会议：摘要、关键决策、行动项、讨论亮点".into(),
            sections: vec![
                TemplateSection {
                    title: "摘要".into(),
                    instruction: "用一段话总结整场会议。".into(),
                    format: SectionFormat::Paragraph,
                    item_format: None,
                },
                TemplateSection {
                    title: "关键决策".into(),
                    instruction: "列出会议中达成的重要决定。".into(),
                    format: SectionFormat::List,
                    item_format: None,
                },
                TemplateSection {
                    title: "行动项".into(),
                    instruction: "列出负责人与任务；附相关转写引用。".into(),
                    format: SectionFormat::List,
                    item_format: Some("| 负责人 | 任务 | 截止 | 转写引用 |".into()),
                },
                TemplateSection {
                    title: "讨论亮点".into(),
                    instruction: "总结主要议题、论点与关键信息。".into(),
                    format: SectionFormat::Paragraph,
                    item_format: None,
                },
            ],
        },
        Template {
            id: "negotiation".into(),
            name: "商务谈判记录".into(),
            description: "谈判：我方立场、对方诉求、让步点、未决事项".into(),
            sections: vec![
                TemplateSection {
                    title: "会谈摘要".into(),
                    instruction: "一段话概述谈判进程与结果。".into(),
                    format: SectionFormat::Paragraph,
                    item_format: None,
                },
                TemplateSection {
                    title: "对方诉求".into(),
                    instruction: "列出客户/对方的明确诉求与底线信号。".into(),
                    format: SectionFormat::List,
                    item_format: None,
                },
                TemplateSection {
                    title: "我方让步与承诺".into(),
                    instruction: "记录我方给出的让步、承诺与条件。".into(),
                    format: SectionFormat::List,
                    item_format: None,
                },
                TemplateSection {
                    title: "未决事项".into(),
                    instruction: "列出待跟进/待确认事项与时间点。".into(),
                    format: SectionFormat::List,
                    item_format: Some("| 事项 | 责任人 | 时间点 |".into()),
                },
            ],
        },
        Template {
            id: "tech_review".into(),
            name: "技术评审纪要".into(),
            description: "技术方案讨论：需求、方案、风险、结论".into(),
            sections: vec![
                TemplateSection {
                    title: "评审摘要".into(),
                    instruction: "一段话概述技术讨论与结论。".into(),
                    format: SectionFormat::Paragraph,
                    item_format: None,
                },
                TemplateSection {
                    title: "需求与约束".into(),
                    instruction: "列出客户需求与技术约束。".into(),
                    format: SectionFormat::List,
                    item_format: None,
                },
                TemplateSection {
                    title: "方案与权衡".into(),
                    instruction: "总结讨论过的方案与取舍理由。".into(),
                    format: SectionFormat::Paragraph,
                    item_format: None,
                },
                TemplateSection {
                    title: "风险与开放问题".into(),
                    instruction: "列出风险点与待解决技术问题。".into(),
                    format: SectionFormat::List,
                    item_format: None,
                },
            ],
        },
        Template {
            id: "daily_standup".into(),
            name: "每日站会".into(),
            description: "站会：昨日、今日、阻塞".into(),
            sections: vec![
                TemplateSection {
                    title: "昨日完成".into(),
                    instruction: "列出昨日完成事项。".into(),
                    format: SectionFormat::List,
                    item_format: None,
                },
                TemplateSection {
                    title: "今日计划".into(),
                    instruction: "列出今日计划事项。".into(),
                    format: SectionFormat::List,
                    item_format: None,
                },
                TemplateSection {
                    title: "阻塞与协助".into(),
                    instruction: "列出阻塞项与需要的协助。".into(),
                    format: SectionFormat::List,
                    item_format: None,
                },
            ],
        },
    ]
}

/// 按 id 获取内置模板。
pub fn get_template(id: &str) -> Option<Template> {
    builtin_templates().into_iter().find(|t| t.id == id)
}

/// 纪要生成器。
pub struct NotesGenerator {
    llm: Arc<dyn LLMProvider>,
}

impl NotesGenerator {
    pub fn new(llm: Arc<dyn LLMProvider>) -> Self {
        Self { llm }
    }

    /// 基于会话内容按模板生成 Markdown 纪要。
    pub fn generate(
        &self,
        transcript: &[TranscriptSegment],
        terms: &[String],
        translations: &[String],
        template: &Template,
    ) -> Result<String> {
        template.validate()?;
        if transcript.is_empty() {
            return Err(anyhow!("会话无转写内容，无法生成纪要"));
        }
        let system = format!(
            "你是中英双语商务会议秘书。根据转写与辅助信息，按给定模板结构生成简洁、条理的 Markdown 会议纪要。\
             使用中文输出（除非内容是专有名词/缩写）。遵守模板分节与条目格式。"
        );
        let transcript_block = transcript
            .iter()
            .map(|s| format!("[{}] {}", s.speaker_label, s.text))
            .collect::<Vec<_>>()
            .join("\n");
        let terms_block = if terms.is_empty() { "（无）".to_string() } else { terms.join("；") };
        let translations_block = if translations.is_empty() { "（无）".to_string() } else { translations.join("\n") };

        let prompt = format!(
            "## 模板\n{}\n\n## 模板分节指令\n{}\n\n## 转写\n{}\n\n## 术语\n{}\n\n## 翻译参考\n{}\n\n请生成纪要。",
            template.to_markdown_structure(),
            template.to_instructions(),
            transcript_block,
            terms_block,
            translations_block,
        );
        let notes = self.llm.complete(&prompt, &system)?;
        if notes.trim().is_empty() {
            return Err(anyhow!("纪要生成结果为空"));
        }
        Ok(notes.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_llm::MockProvider;

    #[test]
    fn builtin_templates_validate() {
        let templates = builtin_templates();
        assert!(templates.len() >= 4);
        for t in &templates {
            assert!(t.validate().is_ok(), "模板 {} 校验失败", t.id);
        }
    }

    #[test]
    fn get_template_by_id() {
        assert!(get_template("standard_meeting").is_some());
        assert!(get_template("negotiation").is_some());
        assert!(get_template("nonexistent").is_none());
    }

    #[test]
    fn template_instructions_include_sections() {
        let t = get_template("standard_meeting").unwrap();
        let instr = t.to_instructions();
        assert!(instr.contains("摘要"));
        assert!(instr.contains("行动项"));
        assert!(instr.contains("| 负责人 | 任务 | 截止 | 转写引用 |"));
    }

    #[test]
    fn invalid_template_rejected() {
        let t = Template {
            id: "bad".into(),
            name: "Bad".into(),
            description: "".into(),
            sections: vec![],
        };
        assert!(t.validate().is_err());
    }

    #[test]
    fn generate_produces_markdown_with_mock_llm() {
        let mock = MockProvider { response: "# 会议纪要\n\n## 摘要\n讨论了 NPI 项目进度。\n\n## 关键决策\n- 确认方案 A".into() };
        let gen = NotesGenerator::new(Arc::new(mock));
        let segs = vec![
            TranscriptSegment { speaker_id: 1, speaker_label: "客户".into(), text: "We need NPI samples by Friday.".into(), is_partial: false, ts_ms: 0, duration_ms: 500, rms: 0.2 },
            TranscriptSegment { speaker_id: 0, speaker_label: "我".into(), text: "我们确认可以安排。".into(), is_partial: false, ts_ms: 1, duration_ms: 400, rms: 0.15 },
        ];
        let t = get_template("standard_meeting").unwrap();
        let notes = gen.generate(&segs, &["NPI = 新产品导入".into()], &[], &t).unwrap();
        assert!(notes.contains("会议纪要"));
        assert!(notes.contains("NPI"));
    }

    #[test]
    fn empty_transcript_rejected() {
        let mock = MockProvider { response: "x".into() };
        let gen = NotesGenerator::new(Arc::new(mock));
        let t = get_template("standard_meeting").unwrap();
        assert!(gen.generate(&[], &[], &[], &t).is_err());
    }
}
