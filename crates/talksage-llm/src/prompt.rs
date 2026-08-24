//! Prompt 模板插值。各 crate 自持 `.txt`（谁调用、谁持有）；此处只提供 `{name}` 替换。
//! 未声明的 `{...}` 原样保留（便于 system prompt 里写 JSON 示例）。
//! 插入值不再二次扫描，避免转写文本里碰巧出现的 `{foo}` 被替换。

/// 将 `vars` 中的 `{name}` 替换为对应值。
pub fn render_prompt(template: &str, vars: &[(&str, &str)]) -> String {
    let template = template.trim_end();
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        match rest.find('}') {
            Some(end) => {
                let key = &rest[1..end];
                if let Some((_, value)) = vars.iter().find(|(k, _)| *k == key) {
                    out.push_str(value);
                    rest = &rest[end + 1..];
                } else {
                    out.push('{');
                    rest = &rest[1..];
                }
            }
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::render_prompt;

    #[test]
    fn replaces_declared_placeholders() {
        let out = render_prompt(
            "你好 {name}，目标：{target}",
            &[("name", "张三"), ("target", "英文")],
        );
        assert_eq!(out, "你好 张三，目标：英文");
    }

    #[test]
    fn leaves_unknown_braces_and_json_examples() {
        let tmpl = r#"只输出 JSON：{"points":["要点"]} 然后 {text}"#;
        let out = render_prompt(tmpl, &[("text", "hello")]);
        assert_eq!(out, r#"只输出 JSON：{"points":["要点"]} 然后 hello"#);
    }

    #[test]
    fn does_not_rescan_inserted_values() {
        let out = render_prompt("A{a}B", &[("a", "{b}字"), ("b", "不该出现")]);
        assert_eq!(out, "A{b}字B");
    }
}
