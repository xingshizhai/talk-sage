//! TalkSage v2 本地知识库：索引 `.md/.txt` 文件夹，关键词 Jaccard 检索（零依赖）。
//! 中文按 2-gram 分词，英文/数字按词。兼容 Obsidian 仓库（跳过 `.obsidian` / `.trash`）。

use std::path::Path;

/// 检索命中。
#[derive(Debug, Clone)]
pub struct KBHit {
    pub text: String,
    pub source: String,
    pub heading: String,
    pub score: f32,
}

/// 本地知识库。
pub struct KnowledgeBase {
    chunks: Vec<KBChunk>,
}

struct KBChunk {
    text: String,
    source: String,
    heading: String,
}

impl KnowledgeBase {
    pub fn new() -> Self {
        Self { chunks: Vec::new() }
    }

    /// 索引文件夹下的 .md/.txt（递归）。跳过 `.obsidian` / `.trash` / `.git`。返回 chunk 数。
    pub fn index_folder(&mut self, folder: &Path) -> usize {
        self.chunks.clear();
        if !folder.is_dir() {
            return 0;
        }
        let mut files: Vec<_> = walk_files(folder);
        files.sort();
        for file in files {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let rel = file.strip_prefix(folder).unwrap_or(&file).to_string_lossy().to_string();
            self.chunks.extend(chunk_file(&text, &rel));
        }
        self.chunks.len()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Jaccard 关键词检索。
    pub fn search(&self, query: &str, top_k: usize, min_score: f32) -> Vec<KBHit> {
        if self.chunks.is_empty() || query.trim().is_empty() {
            return Vec::new();
        }
        let q_tokens = tokenize(query);
        if q_tokens.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<KBHit> = Vec::new();
        for chunk in &self.chunks {
            let c_tokens = tokenize(&chunk.text);
            if c_tokens.is_empty() {
                continue;
            }
            let inter = q_tokens.intersection(&c_tokens).count();
            let union = q_tokens.union(&c_tokens).count();
            if union == 0 {
                continue;
            }
            let mut score = inter as f32 / union as f32;
            // 短查询的精确命中加成
            if inter > 0 {
                score = score.max(inter as f32 / q_tokens.len() as f32 * 0.5);
            }
            if score >= min_score {
                scored.push(KBHit {
                    text: chunk.text.clone(),
                    source: chunk.source.clone(),
                    heading: chunk.heading.clone(),
                    score,
                });
            }
        }
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }
}

impl Default for KnowledgeBase {
    fn default() -> Self {
        Self::new()
    }
}

/// 递归收集 .md/.txt 文件。跳过 Obsidian 元数据/回收站和 .git。
fn walk_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if is_skipped_dir(&p) {
                    continue;
                }
                out.extend(walk_files(&p));
            } else if matches!(p.extension().and_then(|e| e.to_str()), Some("md") | Some("txt")) {
                out.push(p);
            }
        }
    }
    out
}

fn is_skipped_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some(".obsidian" | ".trash" | ".git")
    )
}

/// 按标题分块（超出 800 字再按段落切）。
fn chunk_file(text: &str, source: &str) -> Vec<KBChunk> {
    let mut out = Vec::new();
    let mut parts: Vec<&str> = text
        .split("\n#")
        .flat_map(|p| p.split("\n##").flat_map(|q| q.split("\n###")))
        .collect();
    if parts.is_empty() {
        parts.push(text);
    }
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let heading = part
            .lines()
            .next()
            .unwrap_or("")
            .trim_start_matches('#')
            .trim()
            .to_string();
        if part.chars().count() > 800 {
            let paragraphs: Vec<&str> = part.split("\n\n").map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
            let mut buf = String::new();
            for p in paragraphs {
                if buf.chars().count() + p.chars().count() >= 400 && !buf.is_empty() {
                    out.push(KBChunk { text: buf.trim().to_string(), source: source.to_string(), heading: heading.clone() });
                    buf.clear();
                }
                buf.push_str(p);
                buf.push('\n');
            }
            if !buf.trim().is_empty() {
                out.push(KBChunk { text: buf.trim().to_string(), source: source.to_string(), heading });
            }
        } else {
            out.push(KBChunk { text: part.to_string(), source: source.to_string(), heading });
        }
    }
    out
}

/// 分词：英文/数字词（≥2）+ 连续汉字的 2-gram。
///
/// 整段汉字当一个 token 时，「样品交期」无法命中「客户关注样品交期与最小起订量」。
fn tokenize(text: &str) -> std::collections::HashSet<String> {
    let mut tokens = std::collections::HashSet::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_alphanumeric() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_alphanumeric() {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect::<String>().to_lowercase();
            if word.len() >= 2 {
                tokens.insert(word);
            }
        } else if is_cjk(c) {
            let start = i;
            while i < chars.len() && is_cjk(chars[i]) {
                i += 1;
            }
            let run = &chars[start..i];
            if run.len() >= 2 {
                for window in run.windows(2) {
                    tokens.insert(window.iter().collect());
                }
            }
        } else {
            i += 1;
        }
    }
    tokens
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("talksage-kb-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn index_and_search_english_term() {
        let dir = temp_dir("en");
        std::fs::write(
            dir.join("client.md"),
            "# 客户简报\n\n客户关注 NPI 样品交期与 MOQ。NPI 需要两周。",
        )
        .unwrap();
        let mut kb = KnowledgeBase::new();
        assert_eq!(kb.index_folder(&dir), 1);
        let hits = kb.search("NPI MOQ", 3, 0.05);
        assert!(!hits.is_empty(), "应有命中");
        assert!(hits[0].text.contains("NPI"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_hits_chinese_phrase_inside_longer_sentence() {
        let dir = temp_dir("zh");
        std::fs::write(dir.join("brief.md"), "# 简报\n\n客户关注样品交期与最小起订量。").unwrap();
        let mut kb = KnowledgeBase::new();
        kb.index_folder(&dir);
        let hits = kb.search("样品交期", 3, 0.05);
        assert!(!hits.is_empty(), "中文短语应命中包含该短语的更长句子");
        assert!(hits[0].text.contains("样品交期"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_returns_empty_when_kb_empty() {
        let kb = KnowledgeBase::new();
        assert!(kb.search("anything", 3, 0.05).is_empty());
    }

    #[test]
    fn index_missing_folder_returns_zero() {
        let mut kb = KnowledgeBase::new();
        assert_eq!(kb.index_folder(Path::new("C:/definitely/not/exist")), 0);
    }

    #[test]
    fn index_skips_obsidian_metadata_and_trash() {
        let dir = temp_dir("obsidian");
        std::fs::write(dir.join("note.md"), "# 笔记\n\n客户关注样品交期。").unwrap();
        let meta = dir.join(".obsidian");
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(meta.join("workspace.md"), "# 不该被索引\n\n这是 Obsidian 配置。").unwrap();
        let trash = dir.join(".trash");
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::write(trash.join("deleted.md"), "# 已删除\n\n样品交期旧稿。").unwrap();
        let mut kb = KnowledgeBase::new();
        assert_eq!(kb.index_folder(&dir), 1, "只应索引仓库笔记，跳过 .obsidian 与 .trash");
        let hits = kb.search("样品交期", 3, 0.05);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].source.contains("note.md"));
        assert!(!hits[0].source.contains(".obsidian"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
