//! TalkSage v2 本地知识库：源插件切块 + 词法检索（零依赖）。
//! 中文按 2-gram 分词，英文/数字按词。Obsidian 仓库跳过 `.obsidian` / `.trash` / `.git`。

use std::path::{Path, PathBuf};

/// 一期 Obsidian 源插件 id，与 `talksage-plugins` 的 `knowledge_obsidian` 对齐。
pub const OBSIDIAN_SOURCE_ID: &str = "knowledge_obsidian";

/// 源插件产出的一块笔记。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeSnippet {
    pub source_id: String,
    pub path: String,
    pub heading: String,
    pub text: String,
}

/// 材料包用的文档条目（同一 path 多 chunk 去重为一条）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeDocument {
    pub path: String,
    pub title: String,
    pub text: String,
}

/// 知识源：从某处读出片段。一期只有本地 Obsidian。
pub trait KnowledgeSource: Send + Sync {
    fn id(&self) -> &'static str;
    fn load_snippets(&self) -> Result<Vec<KnowledgeSnippet>, String>;
}

/// 本地 Obsidian vault（一个根路径）。
#[derive(Debug, Clone)]
pub struct ObsidianSource {
    root: PathBuf,
}

impl ObsidianSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl KnowledgeSource for ObsidianSource {
    fn id(&self) -> &'static str {
        OBSIDIAN_SOURCE_ID
    }

    fn load_snippets(&self) -> Result<Vec<KnowledgeSnippet>, String> {
        if !self.root.is_dir() {
            return Err(format!("知识库路径不是有效目录: {}", self.root.display()));
        }
        let mut files: Vec<_> = walk_files(&self.root);
        files.sort();
        let mut snippets = Vec::new();
        for file in files {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let rel = file
                .strip_prefix(&self.root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            snippets.extend(chunk_file(&text, &rel, OBSIDIAN_SOURCE_ID));
        }
        Ok(snippets)
    }
}

/// 检索命中。
#[derive(Debug, Clone)]
pub struct KBHit {
    pub text: String,
    pub source: String,
    pub source_id: String,
    pub heading: String,
    pub score: f32,
}

/// 把命中格式化成纪要/助手可用的知识块。无命中返回空串。
pub fn format_knowledge_block(hits: &[KBHit]) -> String {
    if hits.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "相关知识（来自用户知识库；只能引用下列片段中的事实，没有的写「转写中未提及/知识库未记载」）\n",
    );
    for hit in hits {
        let heading = if hit.heading.is_empty() {
            String::new()
        } else {
            format!(" — {}", hit.heading)
        };
        out.push_str(&format!(
            "### {}:{}{}\n{}\n\n",
            hit.source_id, hit.source, heading, hit.text
        ));
    }
    out
}

/// 命中门槛：查询里必须有一段连续 N 个 2-gram（≈ N+1 个汉字）整体出现在笔记里。
///
/// 只靠零散的二字词重叠（「我们」+「一下」）就判命中，是"一开口就贴出无关笔记"的
/// 根本原因 —— 那是巧合，不是这句话真的提到了笔记里的东西。
const MIN_PHRASE_BIGRAMS: usize = 3;
/// 英文/数字词达到这个长度就自带辨识度（MOQ、NPI、oauth），单独命中即可。
const MIN_WORD_CHARS: usize = 3;

/// 词太常见就不再当作线索：出现在超过这个比例的 chunk 里 → 视为口水词。
const UBIQUITOUS_DF_RATIO: f32 = 0.2;
/// 小知识库不做上面的过滤：几篇笔记里"每篇都有"说明不了任何问题。
const MIN_CHUNKS_FOR_DF_FILTER: usize = 50;

/// 本地知识库。
pub struct KnowledgeBase {
    chunks: Vec<KBChunk>,
    /// 词 → 包含它的 chunk 数（IDF 与口水词过滤都用它）。
    doc_freq: std::collections::HashMap<String, usize>,
}

struct KBChunk {
    text: String,
    source: String,
    source_id: String,
    heading: String,
    /// 索引期就分好词：检索每段发言都重新给整个仓库分词太贵。
    tokens: std::collections::HashSet<String>,
}

impl KBChunk {
    fn from_snippet(snippet: KnowledgeSnippet) -> Self {
        let tokens = tokenize(&snippet.text);
        Self {
            text: snippet.text,
            source: snippet.path,
            source_id: snippet.source_id,
            heading: snippet.heading,
            tokens,
        }
    }
}

impl KnowledgeBase {
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            doc_freq: std::collections::HashMap::new(),
        }
    }

    /// 用源插件产出的片段重建索引。返回 chunk 数。
    pub fn rebuild(&mut self, snippets: &[KnowledgeSnippet]) -> usize {
        self.chunks.clear();
        self.doc_freq.clear();
        self.chunks
            .extend(snippets.iter().cloned().map(KBChunk::from_snippet));
        for chunk in &self.chunks {
            for token in &chunk.tokens {
                *self.doc_freq.entry(token.clone()).or_insert(0) += 1;
            }
        }
        self.chunks.len()
    }

    /// 索引文件夹下的 .md/.txt（递归）。跳过 `.obsidian` / `.trash` / `.git`。返回 chunk 数。
    ///
    /// 路径无效时返回 0（兼容旧调用）。需要区分「不是目录」时用 [`ObsidianSource::load_snippets`]。
    pub fn index_folder(&mut self, folder: &Path) -> usize {
        match ObsidianSource::new(folder).load_snippets() {
            Ok(snippets) => self.rebuild(&snippets),
            Err(_) => {
                self.chunks.clear();
                self.doc_freq.clear();
                0
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.chunks.is_empty()
    }

    /// 材料包：同一 path 只出现一次。
    pub fn list_documents(&self) -> Vec<KnowledgeDocument> {
        let mut order: Vec<String> = Vec::new();
        let mut texts: std::collections::BTreeMap<String, (String, String)> = std::collections::BTreeMap::new();
        for chunk in &self.chunks {
            if !texts.contains_key(&chunk.source) {
                order.push(chunk.source.clone());
                let title = if chunk.heading.is_empty() {
                    Path::new(&chunk.source)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(chunk.source.as_str())
                        .to_string()
                } else {
                    chunk.heading.clone()
                };
                texts.insert(chunk.source.clone(), (title, chunk.text.clone()));
            } else if let Some((_, body)) = texts.get_mut(&chunk.source) {
                body.push_str("\n\n");
                body.push_str(&chunk.text);
            }
        }
        order
            .into_iter()
            .filter_map(|path| {
                texts.remove(&path).map(|(title, text)| KnowledgeDocument { path, title, text })
            })
            .collect()
    }

    /// 钉住一篇笔记时拼接该 path 下全部 chunk。
    pub fn text_for_path(&self, path: &str) -> Option<String> {
        let mut parts = Vec::new();
        for chunk in &self.chunks {
            if chunk.source == path {
                parts.push(chunk.text.clone());
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }

    /// 词的信息量（IDF）。越常见越接近 0，独有词最大。
    fn idf(&self, token: &str) -> f32 {
        let n = self.chunks.len() as f32;
        let df = self.doc_freq.get(token).copied().unwrap_or(0) as f32;
        (1.0 + n / (1.0 + df)).ln()
    }

    /// 是否值得当作检索线索。仓库够大时，出现在 20% 以上 chunk 的词（「我们」
    /// 「这边」「一下」这类）一律不算 —— 它们只会让任何一句话都"命中"。
    fn is_informative(&self, token: &str) -> bool {
        if self.chunks.len() < MIN_CHUNKS_FOR_DF_FILTER {
            return true;
        }
        let df = self.doc_freq.get(token).copied().unwrap_or(0) as f32;
        df <= self.chunks.len() as f32 * UBIQUITOUS_DF_RATIO
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// IDF 加权的关键词检索：命中词越少见，分越高。
    ///
    /// 分数 = 命中词的信息量 / 查询词的信息量总和（0..1）。原来的 Jaccard 加上
    /// 「短查询命中加成」会让「我们/这边/一下」这类口水词单独就把分数顶过阈值，
    /// 一开口就贴出整段无关笔记 —— 现在这类词的 IDF 接近 0，且在大仓库里直接不算线索。
    pub fn search(&self, query: &str, top_k: usize, min_score: f32) -> Vec<KBHit> {
        self.search_filtered(query, top_k, min_score, |_| true)
    }

    /// 只在指定文档路径中检索，供会中“材料包优先”策略使用。
    pub fn search_in_paths(
        &self,
        query: &str,
        paths: &std::collections::HashSet<String>,
        top_k: usize,
        min_score: f32,
    ) -> Vec<KBHit> {
        self.search_filtered(query, top_k, min_score, |chunk| paths.contains(&chunk.source))
    }

    fn search_filtered(
        &self,
        query: &str,
        top_k: usize,
        min_score: f32,
        include: impl Fn(&KBChunk) -> bool,
    ) -> Vec<KBHit> {
        if self.chunks.is_empty() || query.trim().is_empty() {
            return Vec::new();
        }
        let q_tokens: std::collections::HashSet<String> = tokenize(query)
            .into_iter()
            .filter(|t| self.is_informative(t))
            .collect();
        if q_tokens.is_empty() {
            return Vec::new(); // 整句都是口水词：没什么可查的
        }
        let q_mass: f32 = q_tokens.iter().map(|t| self.idf(t)).sum();
        if q_mass <= 0.0 {
            return Vec::new();
        }
        // 词组门槛用的是"有序"分词：连续的 2-gram 都命中，才说明笔记里真有这段话
        let (q_runs, q_words) = tokenize_ordered(query);
        let mut scored: Vec<KBHit> = Vec::new();
        for chunk in &self.chunks {
            if !include(chunk) {
                continue;
            }
            if !chunk_has_phrase(chunk, &q_runs, &q_words) {
                continue;
            }
            let hit_mass: f32 = q_tokens
                .iter()
                .filter(|t| chunk.tokens.contains(*t))
                .map(|t| self.idf(t))
                .sum();
            if hit_mass <= 0.0 {
                continue;
            }
            let score = hit_mass / q_mass;
            if score >= min_score {
                scored.push(KBHit {
                    text: chunk.text.clone(),
                    source: chunk.source.clone(),
                    source_id: chunk.source_id.clone(),
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
fn chunk_file(text: &str, path: &str, source_id: &str) -> Vec<KnowledgeSnippet> {
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
                    out.push(KnowledgeSnippet {
                        source_id: source_id.to_string(),
                        path: path.to_string(),
                        heading: heading.clone(),
                        text: buf.trim().to_string(),
                    });
                    buf.clear();
                }
                buf.push_str(p);
                buf.push('\n');
            }
            if !buf.trim().is_empty() {
                out.push(KnowledgeSnippet {
                    source_id: source_id.to_string(),
                    path: path.to_string(),
                    heading,
                    text: buf.trim().to_string(),
                });
            }
        } else {
            out.push(KnowledgeSnippet {
                source_id: source_id.to_string(),
                path: path.to_string(),
                heading,
                text: part.to_string(),
            });
        }
    }
    out
}

/// 命中门槛：笔记里出现了查询中的一个词组，或一个够长的英文/数字词。
fn chunk_has_phrase(chunk: &KBChunk, q_runs: &[Vec<String>], q_words: &[String]) -> bool {
    for run in q_runs {
        let mut streak = 0usize;
        for bigram in run {
            if chunk.tokens.contains(bigram) {
                streak += 1;
                if streak >= MIN_PHRASE_BIGRAMS {
                    return true;
                }
            } else {
                streak = 0;
            }
        }
    }
    q_words
        .iter()
        .any(|w| w.chars().count() >= MIN_WORD_CHARS && chunk.tokens.contains(w))
}

/// 有序分词：按汉字连续段给出各自的 2-gram 序列，另附英文/数字词。
///
/// [`tokenize`] 返回的是无序集合，判断不了"连续"，而词组门槛正需要顺序。
fn tokenize_ordered(text: &str) -> (Vec<Vec<String>>, Vec<String>) {
    let mut runs = Vec::new();
    let mut words = Vec::new();
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
                words.push(word);
            }
        } else if is_cjk(c) {
            let start = i;
            while i < chars.len() && is_cjk(chars[i]) {
                i += 1;
            }
            let run = &chars[start..i];
            if run.len() >= 2 {
                runs.push(run.windows(2).map(|w| w.iter().collect::<String>()).collect());
            }
        } else {
            i += 1;
        }
    }
    (runs, words)
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

    /// 只共享「我们/这边」这类常用说法的笔记不该被检索出来。
    ///
    /// 复现线上现象：实时转写一开始，「知识库命中」就贴出一大段与发言无关的内容。
    #[test]
    fn common_chinese_bigrams_alone_do_not_retrieve_unrelated_notes() {
        let dir = temp_dir("noise");
        // 一个真实规模的仓库：几十篇日常笔记，都带着口语里最常见的那几个词
        for i in 0..60 {
            std::fs::write(
                dir.join(format!("note{i}.md")),
                format!("# 日常{i}

我们这边今天讨论了排班和值班的事情，大家都觉得可以，下周再看一下。"),
            )
            .unwrap();
        }
        let mut kb = KnowledgeBase::new();
        assert!(kb.index_folder(&dir) >= 60);

        // 一句与仓库内容无关的发言，只在「我们/这边/一下」这类词上有重叠
        let hits = kb.search("我们这边先看一下报价单能不能压到位", 2, 0.05);
        assert!(
            hits.is_empty(),
            "只共享常用词不该算命中，实际命中 {} 条，score={:?}",
            hits.len(),
            hits.iter().map(|h| h.score).collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 反向保证：同样的大仓库里，带稀有词的提问必须命中那一篇，
    /// 不能因为过滤口水词把检索一起关掉。
    #[test]
    fn rare_term_still_retrieves_its_note_in_a_big_vault() {
        let dir = temp_dir("signal");
        for i in 0..60 {
            std::fs::write(
                dir.join(format!("note{i}.md")),
                format!("# 日常{i}

我们这边今天讨论了排班和值班的事情，大家都觉得可以，下周再看一下。"),
            )
            .unwrap();
        }
        std::fs::write(
            dir.join("client.md"),
            "# 客户简报

我们这边跟客户确认过：MOQ 是 1000 片，样品交期两周。",
        )
        .unwrap();
        let mut kb = KnowledgeBase::new();
        kb.index_folder(&dir);

        // 提问里既有口水词也有稀有词，应当只命中客户简报
        let hits = kb.search("我们这边再确认一下样品交期和 MOQ", 3, 0.05);
        assert_eq!(hits.len(), 1, "只应命中含稀有词的那篇: {:?}", hits.iter().map(|h| &h.source).collect::<Vec<_>>());
        assert!(hits[0].source.contains("client.md"));
        assert!(hits[0].score > 0.3, "命中稀有词的分数应明显高于阈值: {}", hits[0].score);
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

    fn write_vault_layout(dir: &Path) {
        std::fs::create_dir_all(dir.join("200-Wiki")).unwrap();
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::create_dir_all(dir.join(".trash")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".obsidian").join("app.json"), "{}").unwrap();
        std::fs::write(dir.join(".trash").join("x.md"), "# 垃圾\n\n样品交期旧稿").unwrap();
        std::fs::write(dir.join(".git").join("ignored.md"), "# git\n\n样品交期").unwrap();
        std::fs::write(
            dir.join("200-Wiki").join("npi.md"),
            "# NPI\n\n客户关注样品交期与最小起订量。",
        )
        .unwrap();
    }

    #[test]
    fn obsidian_source_loads_only_vault_notes() {
        let dir = temp_dir("source");
        write_vault_layout(&dir);
        let snippets = ObsidianSource::new(&dir).load_snippets().unwrap();
        assert_eq!(snippets.len(), 1, "只应读到 wiki 笔记: {snippets:?}");
        assert_eq!(snippets[0].source_id, OBSIDIAN_SOURCE_ID);
        assert_eq!(snippets[0].path, "200-Wiki/npi.md");
        assert!(snippets[0].text.contains("样品交期"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn obsidian_source_missing_root_is_err() {
        let err = ObsidianSource::new(Path::new("C:/definitely/not/exist"))
            .load_snippets()
            .unwrap_err();
        assert!(err.contains("不是有效目录"), "{err}");
    }

    #[test]
    fn search_hits_include_source_id() {
        let dir = temp_dir("sid");
        std::fs::write(dir.join("client.md"), "# 客户简报\n\n客户关注 NPI 样品交期与 MOQ。").unwrap();
        let mut kb = KnowledgeBase::new();
        kb.index_folder(&dir);
        let hits = kb.search("NPI MOQ", 3, 0.05);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].source_id, OBSIDIAN_SOURCE_ID);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scoped_search_only_returns_selected_paths() {
        let dir = temp_dir("scoped");
        std::fs::write(dir.join("a.md"), "# A\n\nNPI 样品交期为下周三。").unwrap();
        std::fs::write(dir.join("b.md"), "# B\n\nNPI 样品交期为下周五。").unwrap();
        let mut kb = KnowledgeBase::new();
        kb.index_folder(&dir);
        let paths = ["b.md".to_string()].into_iter().collect();
        let hits = kb.search_in_paths("NPI 样品交期", &paths, 3, 0.05);
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|hit| hit.source == "b.md"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_documents_dedupes_chunks_from_the_same_file() {
        let snippets = vec![
            KnowledgeSnippet {
                source_id: OBSIDIAN_SOURCE_ID.into(),
                path: "a.md".into(),
                heading: "一".into(),
                text: "第一块".into(),
            },
            KnowledgeSnippet {
                source_id: OBSIDIAN_SOURCE_ID.into(),
                path: "a.md".into(),
                heading: "二".into(),
                text: "第二块".into(),
            },
        ];
        let mut kb = KnowledgeBase::new();
        kb.rebuild(&snippets);
        let docs = kb.list_documents();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].path, "a.md");
        assert_eq!(docs[0].title, "一");
        assert!(docs[0].text.contains("第一块"));
        assert!(docs[0].text.contains("第二块"));
    }

    #[test]
    fn format_knowledge_block_empty_and_hit() {
        assert!(format_knowledge_block(&[]).is_empty());
        let block = format_knowledge_block(&[KBHit {
            text: "MOQ 1000".into(),
            source: "200-Wiki/npi.md".into(),
            source_id: OBSIDIAN_SOURCE_ID.into(),
            heading: "商务".into(),
            score: 0.9,
        }]);
        assert!(block.contains("knowledge_obsidian:200-Wiki/npi.md"));
        assert!(block.contains("MOQ 1000"));
    }
}
