//! 模型管理：下载 / 校验 / 删除 / 磁盘占用。
//!
//! 用途：应用内「转写引擎」页直接安装/卸载模型，无需用户跑 Python 脚本。
//! 下载源与 `scripts/download_models.py` 保持一致（该脚本仍是批量安装工具；
//! 这里的实现供桌面/headless 的模型管理 UI 调用）。
//!
//! 模型布局（与 [`EngineKind::model_dir_name`] 对齐）：
//! - 流式/whisper：models/<dir>/ 下的 onnx + tokens
//! - qwen3-asr：models/sherpa-onnx-qwen3-asr-0.6b/（官方 int8 包解压，
//!   conv_frontend.onnx / encoder.int8.onnx / decoder.int8.onnx / tokenizer/）

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::EngineKind;

/// 下载进度回调（received = 已下载字节，total = 总字节，未知为 0）。
pub type ProgressFn = dyn Fn(u64, u64) + Send + Sync;

/// 每个引擎的下载源（文件名 → 显式 URL；空 URL = 走 HF resolve/main）。
/// 与 `scripts/download_models.py` 的 TARGETS 保持一致。
fn sources(kind: EngineKind) -> Vec<(String, String)> {
    match kind {
        EngineKind::ParaformerZh => vec![
            ("encoder.onnx", ""),
            ("decoder.onnx", ""),
            ("encoder.int8.onnx", ""),
            ("decoder.int8.onnx", ""),
            ("tokens.txt", ""),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect(),
        EngineKind::ZipformerEn => vec![
            ("encoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx", ""),
            ("decoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx", ""),
            ("joiner-epoch-99-avg-1-chunk-16-left-64.int8.onnx", ""),
            ("bpe.model", ""),
            ("tokens.txt", ""),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect(),
        EngineKind::WhisperBase | EngineKind::WhisperSmall => {
            let stem = if kind == EngineKind::WhisperBase { "base" } else { "small" };
            [("encoder.onnx", ""), ("decoder.onnx", ""), ("encoder.int8.onnx", ""), ("decoder.int8.onnx", ""), ("tokens.txt", "")]
                .into_iter()
                .map(|(suffix, url)| (format!("{stem}-{suffix}"), url.to_string()))
                .collect()
        }
        EngineKind::Qwen3Asr => vec![
            // 官方仅发布 int8 包（GitHub release tar.bz2）；HF 仓库 gated。
            // 由 `download_qwen3_asr` 走归档下载。
            (String::new(), String::new()),
        ],
    }
}

/// 各引擎的 HF 仓库名（文件名 URL 为空时拼接 resolve/main）。
fn hf_repo(kind: EngineKind) -> &'static str {
    match kind {
        EngineKind::ParaformerZh => "csukuangfj/sherpa-onnx-streaming-paraformer-zh",
        EngineKind::ZipformerEn => "csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26",
        EngineKind::WhisperBase => "csukuangfj/sherpa-onnx-whisper-base",
        EngineKind::WhisperSmall => "csukuangfj/sherpa-onnx-whisper-small",
        EngineKind::Qwen3Asr => "",
    }
}

/// 引擎下载的预计总大小（MB，用于 UI 提示；qwen3 为归档大小）。
pub fn download_size_mb(kind: EngineKind) -> u64 {
    match kind {
        EngineKind::ParaformerZh => 700,
        EngineKind::ZipformerEn => 400,
        EngineKind::WhisperBase => 280,
        EngineKind::WhisperSmall => 950,
        EngineKind::Qwen3Asr => 878,
    }
}

/// 模型目录磁盘占用（MB）。
pub fn installed_size_mb(kind: EngineKind, models_root: &Path) -> u64 {
    let dir = models_root.join(kind.model_dir_name());
    dir_size(&dir) / (1024 * 1024)
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// 是否正在下载 qwen3-asr 归档（下载中会残留 <dir>.tar.bz2.part / 目录未就绪）。
pub fn is_downloading(kind: EngineKind, models_root: &Path) -> bool {
    models_root.join(format!("{}.part", kind.model_dir_name())).exists()
        || models_root.join(format!("{}.staging", kind.model_dir_name())).exists()
}

/// 删除模型目录（已安装才有效）。
pub fn remove_engine(kind: EngineKind, models_root: &Path) -> std::io::Result<()> {
    let dir = models_root.join(kind.model_dir_name());
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    // 顺手清理可能的残留
    for suffix in [".part", ".staging", ".tar.bz2"] {
        let p = models_root.join(format!("{}{}", kind.model_dir_name(), suffix));
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    Ok(())
}

/// 下载并安装引擎（同步阻塞；调用方应放入 spawn_blocking）。
/// 进度经 `progress` 回调（received/total 字节，total 未知为 0）。
pub fn download_engine(
    kind: EngineKind,
    models_root: &Path,
    progress: Option<&ProgressFn>,
) -> anyhow::Result<()> {
    if kind.is_available(models_root) {
        return Ok(()); // 已安装
    }
    if kind == EngineKind::Qwen3Asr {
        return download_qwen3_asr(models_root, progress);
    }
    std::fs::create_dir_all(models_root)?;
    let out_dir = models_root.join(kind.model_dir_name());
    std::fs::create_dir_all(&out_dir)?;
    let repo = hf_repo(kind);
    for (file, explicit) in sources(kind) {
        if file.is_empty() {
            continue;
        }
        let target = out_dir.join(&file);
        if target.exists() && target.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            continue;
        }
        let url = if explicit.is_empty() {
            format!("https://huggingface.co/{repo}/resolve/main/{file}")
        } else {
            explicit.to_string()
        };
        download_file(&url, &target, progress)?;
    }
    Ok(())
}

/// Qwen3-ASR：下载官方 GitHub release 归档并解压到模型目录。
fn download_qwen3_asr(models_root: &Path, progress: Option<&ProgressFn>) -> anyhow::Result<()> {
    const URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2";
    std::fs::create_dir_all(models_root)?;
    let final_dir = models_root.join(EngineKind::Qwen3Asr.model_dir_name());
    if EngineKind::Qwen3Asr.is_available(models_root) {
        return Ok(());
    }
    let staging = models_root.join(format!("{}.staging", EngineKind::Qwen3Asr.model_dir_name()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    // 归档下载到 models/<dir>.tar.bz2
    let archive = models_root.join(format!("{}.tar.bz2", EngineKind::Qwen3Asr.model_dir_name()));
    download_file(URL, &archive, progress)?;
    // 解压（tar.bz2 → staging/，剥离顶层目录）
    let file = std::fs::File::open(&archive)?;
    let decoder = bzip2::read::MultiBzDecoder::new(file);
    let mut ar = tar::Archive::new(decoder);
    let mut top: Option<String> = None;
    let entries: Vec<_> = ar
        .entries()?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("读取归档失败: {e}"))?;
    for entry in entries {
        let path = entry.path()?.into_owned();
        let mut parts = path.components();
        let head = parts.next().map(|c| c.as_os_str().to_string_lossy().into_owned());
        if top.is_none() {
            top = head;
        }
        let rel: PathBuf = parts.as_path().to_path_buf();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let dest = staging.join(&rel);
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&dest)?;
        } else if entry.header().entry_type().is_file() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&dest)?;
            let mut reader = entry;
            std::io::copy(&mut reader, &mut out)?;
        }
    }
    drop(ar);
    // 校验关键文件
    if !staging.join("conv_frontend.onnx").is_file()
        || !staging.join("encoder.int8.onnx").is_file()
        || !staging.join("decoder.int8.onnx").is_file()
    {
        let _ = std::fs::remove_dir_all(&staging);
        anyhow::bail!("Qwen3-ASR 归档缺少关键文件（下载损坏？）");
    }
    if !staging.join("tokenizer").is_dir() {
        // 兼容旧约定：tokenizer.json 单文件
        if !staging.join("tokenizer.json").is_file() {
            let _ = std::fs::remove_dir_all(&staging);
            anyhow::bail!("Qwen3-ASR 归档缺少 tokenizer");
        }
    }
    // staging → 正式目录（原子替换）
    if final_dir.exists() {
        std::fs::remove_dir_all(&final_dir)?;
    }
    std::fs::rename(&staging, &final_dir)?;
    let _ = std::fs::remove_file(&archive);
    Ok(())
}

/// 流式下载单个文件到目标路径（进度回调；不覆盖已存在的非空文件）。
pub fn download_file(
    url: &str,
    target: &Path,
    progress: Option<&ProgressFn>,
) -> anyhow::Result<()> {
    if target.exists() && target.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let part = target.with_extension("part");
    let resp = ureq::get(url)
        .timeout(std::time::Duration::from_secs(900))
        .call()
        .map_err(|e| anyhow::anyhow!("下载失败 {url}: {e}"))?;
    let total: u64 = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut out = std::fs::File::create(&part)?;
    let mut buf = [0u8; 262144];
    let mut received = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| anyhow::anyhow!("读取下载流失败: {e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        received += n as u64;
        if let Some(cb) = progress {
            cb(received, total);
        }
    }
    out.flush()?;
    drop(out);
    std::fs::rename(&part, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_size_mb_positive() {
        for kind in EngineKind::ALL {
            assert!(download_size_mb(kind) > 0, "{} 应有预估大小", kind.display_name());
        }
    }

    #[test]
    fn dir_size_counts_recursive() {
        let tmp = std::env::temp_dir().join(format!("talksage-models-{}", std::process::id()));
        let sub = tmp.join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("x.bin"), vec![0u8; 4096]).unwrap();
        assert_eq!(dir_size(&tmp), 4096);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn remove_engine_cleans_partials() {
        let tmp = std::env::temp_dir().join(format!("talksage-models-rm-{}", std::process::id()));
        let dir = tmp.join(EngineKind::WhisperBase.model_dir_name());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(tmp.join(format!("{}.part", EngineKind::WhisperBase.model_dir_name())), b"x").unwrap();
        remove_engine(EngineKind::WhisperBase, &tmp).unwrap();
        assert!(!dir.exists());
        assert!(!tmp.join(format!("{}.part", EngineKind::WhisperBase.model_dir_name())).exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn sources_have_nonempty_names_for_non_qwen() {
        for kind in [EngineKind::ParaformerZh, EngineKind::ZipformerEn, EngineKind::WhisperBase, EngineKind::WhisperSmall] {
            let names = sources(kind);
            assert!(!names.is_empty());
            assert!(names.iter().all(|(f, _)| !f.is_empty()), "{} 文件名不应为空", kind.display_name());
        }
    }
}
