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
//! - Apple Metal：models/whisper.cpp-large-v3-turbo-q5_0/ggml-large-v3-turbo-q5_0.bin

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::EngineKind;
use sha1::{Digest, Sha1};

const MIB: u64 = 1024 * 1024;
const METAL_MODEL_SHA1: &str = "e050f7970618a659205450ad97eb95a18d69c9ee";

/// 下载进度回调（received = 已下载字节，total = 总字节，未知为 0）。
pub type ProgressFn = dyn Fn(u64, u64) + Send + Sync;

/// 下载被用户取消时返回的错误（上层据此发"已取消"事件而非"失败"）。
#[derive(Debug)]
pub struct DownloadCancelled;

impl std::fmt::Display for DownloadCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "下载已取消")
    }
}

impl std::error::Error for DownloadCancelled {}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}

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
        EngineKind::WhisperLargeV3TurboMetal => vec![(
            "ggml-large-v3-turbo-q5_0.bin".to_string(),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin".to_string(),
        )],
        EngineKind::AliyunCloud => vec![], // 云端引擎：无本地模型文件
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
        EngineKind::WhisperLargeV3TurboMetal => "",
        EngineKind::AliyunCloud => "",
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
        EngineKind::WhisperLargeV3TurboMetal => 547,
        EngineKind::AliyunCloud => 0, // 云端引擎：无下载
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
    let dir = models_root.join(kind.model_dir_name());
    models_root.join(format!("{}.part", kind.model_dir_name())).exists()
        || models_root.join(format!("{}.staging", kind.model_dir_name())).exists()
        || std::fs::read_dir(dir).ok().is_some_and(|entries| {
            entries.flatten().any(|entry| entry.path().extension().is_some_and(|ext| ext == "part"))
        })
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

/// True if the punct ONNX model file is present on disk.
pub fn is_punct_model_installed(models_root: &Path) -> bool {
    crate::punct::is_punct_model_available(models_root)
}

/// Approximate download size for the punct model in MB.
pub fn punct_download_size_mb() -> u64 {
    294
}

/// Download the punctuation model into `<models_root>/punct-ct-transformer/`.
pub fn download_punct_model(
    models_root: &Path,
    cancel: Arc<AtomicBool>,
    tx: Option<std::sync::mpsc::Sender<(u64, u64)>>,
) -> anyhow::Result<()> {
    use crate::punct::PUNCT_MODEL_DIR;
    if is_punct_model_installed(models_root) {
        log::info!("标点恢复模型已安装，跳过下载");
        return Ok(());
    }
    std::fs::create_dir_all(models_root)?;
    let required = 900 * MIB;
    if let Some(available) = available_space(models_root)? {
        if available < required {
            anyhow::bail!(
                "磁盘空间不足：安装标点恢复模型至少需要 {:.1} GiB，当前仅 {:.1} GiB",
                required as f64 / 1024_f64.powi(3),
                available as f64 / 1024_f64.powi(3)
            );
        }
    }
    let dir = models_root.join(PUNCT_MODEL_DIR);
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join("model.onnx");
    let archive = models_root.join("sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12.tar.bz2");
    const GITHUB_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/punctuation-models/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12.tar.bz2";
    const HF_FALLBACK_URL: &str = "https://huggingface.co/csukuangfj/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12/resolve/main/model.onnx";
    let progress_box: Option<Box<ProgressFn>> = tx.map(|sender| {
        Box::new(move |received: u64, total: u64| {
            let _ = sender.send((received, total));
        }) as Box<ProgressFn>
    });
    log::info!("标点恢复模型安装开始: primary={GITHUB_URL} fallback={HF_FALLBACK_URL}");
    let primary = download_file(GITHUB_URL, &archive, progress_box.as_deref(), Some(cancel.as_ref()))
        .and_then(|_| extract_punct_model(&archive, &dest, Some(cancel.as_ref())));
    if let Err(primary_error) = primary {
        if primary_error.downcast_ref::<DownloadCancelled>().is_some() {
            return Err(primary_error);
        }
        log::warn!("标点模型 GitHub 主源失败，切换 Hugging Face 备用源: {primary_error}");
        download_file(HF_FALLBACK_URL, &dest, progress_box.as_deref(), Some(cancel.as_ref()))
            .map_err(|fallback_error| anyhow::anyhow!(
                "标点模型主源与备用源均失败；GitHub: {primary_error}；Hugging Face: {fallback_error}"
            ))?;
    }
    let size = dest.metadata()?.len();
    if size < 200 * MIB {
        let _ = std::fs::remove_file(&dest);
        anyhow::bail!("标点恢复模型文件异常：仅 {:.1} MiB，预期约 281 MiB，已删除损坏文件", size as f64 / MIB as f64);
    }
    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_file(archive.with_extension("part"));
    log::info!("标点恢复模型安装完成: file={} size_mib={:.1}", dest.display(), size as f64 / MIB as f64);
    Ok(())
}

fn extract_punct_model(
    archive: &Path,
    dest: &Path,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<()> {
    let file = std::fs::File::open(archive)?;
    let decoder = bzip2::read::MultiBzDecoder::new(file);
    let mut bundle = tar::Archive::new(decoder);
    let staging = dest.with_extension("onnx.staging");
    for entry in bundle.entries().map_err(|e| anyhow::anyhow!("读取标点模型归档失败: {e}"))? {
        let mut entry = entry?;
        if entry.path()?.file_name().is_some_and(|name| name == "model.onnx") {
            let mut output = std::fs::File::create(&staging)?;
            let mut buffer = [0u8; 262144];
            loop {
                if cancelled(cancel) {
                    drop(output);
                    let _ = std::fs::remove_file(&staging);
                    return Err(anyhow::Error::new(DownloadCancelled));
                }
                let n = entry.read(&mut buffer)?;
                if n == 0 { break; }
                output.write_all(&buffer[..n])?;
            }
            output.flush()?;
            drop(output);
            std::fs::rename(&staging, dest)?;
            return Ok(());
        }
    }
    let _ = std::fs::remove_file(&staging);
    anyhow::bail!("标点模型官方归档中缺少 model.onnx")
}

/// Remove the punct model directory.
pub fn remove_punct_model(models_root: &Path) -> std::io::Result<()> {
    use crate::punct::PUNCT_MODEL_DIR;
    let dir = models_root.join(PUNCT_MODEL_DIR);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// 下载并安装引擎（同步阻塞；调用方应放入 spawn_blocking）。
/// 进度经 `progress` 回调（received/total 字节，total 未知为 0）。
/// `cancel` 为可选的取消标志：置位后尽快停止并清理临时文件，返回
/// [`DownloadCancelled`]。
pub fn download_engine(
    kind: EngineKind,
    models_root: &Path,
    progress: Option<&ProgressFn>,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<()> {
    if kind.is_available(models_root) {
        log::info!("模型已安装，跳过下载: engine={} dir={}", kind.display_name(), models_root.join(kind.model_dir_name()).display());
        return Ok(()); // 已安装
    }
    log::info!(
        "模型安装开始: engine={} expected_mb={} root={}",
        kind.display_name(),
        download_size_mb(kind),
        models_root.display()
    );
    std::fs::create_dir_all(models_root)?;
    ensure_download_space(kind, models_root)?;
    if kind == EngineKind::Qwen3Asr {
        let result = download_qwen3_asr(models_root, progress, cancel);
        if result.is_ok() {
            log::info!("模型安装完成: engine={} dir={}", kind.display_name(), models_root.join(kind.model_dir_name()).display());
        }
        return result;
    }
    let out_dir = models_root.join(kind.model_dir_name());
    std::fs::create_dir_all(&out_dir)?;
    let repo = hf_repo(kind);
    for (file, explicit) in sources(kind) {
        if file.is_empty() {
            continue;
        }
        let target = out_dir.join(&file);
        if kind != EngineKind::WhisperLargeV3TurboMetal
            && target.exists()
            && target.metadata().map(|m| m.len() > 0).unwrap_or(false)
        {
            continue;
        }
        let url = if explicit.is_empty() {
            format!("https://huggingface.co/{repo}/resolve/main/{file}")
        } else {
            explicit.to_string()
        };
        let expected_sha1 = (kind == EngineKind::WhisperLargeV3TurboMetal).then_some(METAL_MODEL_SHA1);
        download_file_checked(&url, &target, progress, cancel, expected_sha1)?;
    }
    log::info!("模型安装完成: engine={} dir={}", kind.display_name(), out_dir.display());
    Ok(())
}

/// 下载期间的峰值空间预算。Qwen 需要同时保存压缩包与 staging；单文件 Metal
/// 模型只需要 `.part`，另留 256 MiB 给日志、数据库和文件系统余量。
fn required_free_bytes(kind: EngineKind) -> u64 {
    let download = download_size_mb(kind) * MIB;
    let working = if kind == EngineKind::Qwen3Asr { download * 2 } else { download };
    working + 256 * MIB
}

fn ensure_download_space(kind: EngineKind, models_root: &Path) -> anyhow::Result<()> {
    let required = required_free_bytes(kind);
    let Some(available) = available_space(models_root)? else {
        log::warn!("当前平台暂不支持模型下载前的磁盘空间预检");
        return Ok(());
    };
    if available < required {
        anyhow::bail!(
            "磁盘空间不足：安装 {} 至少需要 {:.1} GiB 可用空间，当前仅 {:.1} GiB",
            kind.profile().label,
            required as f64 / 1024_f64.powi(3),
            available as f64 / 1024_f64.powi(3),
        );
    }
    log::info!(
        "模型磁盘空间检查通过: engine={} required_gib={:.2} available_gib={:.2} root={}",
        kind.display_name(),
        required as f64 / 1024_f64.powi(3),
        available as f64 / 1024_f64.powi(3),
        models_root.display()
    );
    Ok(())
}

#[cfg(unix)]
fn available_space(path: &Path) -> anyhow::Result<Option<u64>> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| anyhow::anyhow!("模型目录包含 NUL 字符"))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: c_path is NUL-terminated and stat points to writable, correctly sized storage.
    let result = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if result != 0 {
        return Err(anyhow::anyhow!("无法检查模型磁盘剩余空间 {}: {}", path.display(), std::io::Error::last_os_error()));
    }
    // SAFETY: statvfs returned success and initialized the structure.
    let stat = unsafe { stat.assume_init() };
    Ok(Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64)))
}

#[cfg(not(unix))]
fn available_space(_path: &Path) -> anyhow::Result<Option<u64>> {
    Ok(None)
}

/// Qwen3-ASR：下载官方 GitHub release 归档并解压到模型目录。
fn download_qwen3_asr(
    models_root: &Path,
    progress: Option<&ProgressFn>,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<()> {
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
    log::info!("Qwen3-ASR 归档下载: archive={}", archive.display());
    download_file(URL, &archive, progress, cancel)?;
    log::info!("Qwen3-ASR 开始解压: archive={} staging={}", archive.display(), staging.display());
    if let Err(e) = unpack_tar_bz2_strip_top(&archive, &staging, cancel) {
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_file(&archive);
        return Err(e);
    }
    // 校验关键文件存在且**非空**（0 字节 = 下载/解压失败，会触发 native 崩溃）
    let nonempty = |p: &Path| p.is_file() && p.metadata().map(|m| m.len() > 0).unwrap_or(false);
    if !nonempty(&staging.join("conv_frontend.onnx"))
        || !nonempty(&staging.join("encoder.int8.onnx"))
        || !nonempty(&staging.join("decoder.int8.onnx"))
    {
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_file(&archive);
        anyhow::bail!("Qwen3-ASR 归档解压后关键文件为空（下载损坏或磁盘问题），已清理，请重试");
    }
    if !staging.join("tokenizer").is_dir() {
        // 兼容旧约定：tokenizer.json 单文件
        if !nonempty(&staging.join("tokenizer.json")) {
            let _ = std::fs::remove_dir_all(&staging);
            let _ = std::fs::remove_file(&archive);
            anyhow::bail!("Qwen3-ASR 归档缺少 tokenizer");
        }
    }
    // staging → 正式目录（原子替换）
    if final_dir.exists() {
        std::fs::remove_dir_all(&final_dir)?;
    }
    std::fs::rename(&staging, &final_dir)?;
    let _ = std::fs::remove_file(&archive);
    log::info!("Qwen3-ASR 解压校验完成: dir={}", final_dir.display());
    Ok(())
}

/// 解压 tar.bz2 到 staging，剥掉归档顶层目录。
///
/// `tar` 条目共享同一顺序流：必须读完当前条目再 `next()`。先 collect 再拷贝
/// 会把未读 payload skip 掉，得到 0 字节文件（模型管理里 Qwen 下载「总是失败」）。
fn unpack_tar_bz2_strip_top(
    archive: &Path,
    staging: &Path,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<()> {
    let file = std::fs::File::open(archive)?;
    let decoder = bzip2::read::MultiBzDecoder::new(file);
    let mut ar = tar::Archive::new(decoder);
    // 不可 collect：Entries 共享同一顺序流，next() 会 skip 上一条未读 payload。
    for entry in ar.entries().map_err(|e| anyhow::anyhow!("读取归档失败: {e}"))? {
        if cancelled(cancel) {
            return Err(anyhow::Error::new(DownloadCancelled));
        }
        let mut entry = entry.map_err(|e| anyhow::anyhow!("读取归档失败: {e}"))?;
        let path = entry.path()?.into_owned();
        let mut parts = path.components();
        let _head = parts.next();
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
            // 分块拷贝：`std::io::copy` 内部阻塞且无法中途检查取消标志。
            let mut buf = [0u8; 262144];
            loop {
                if cancelled(cancel) {
                    return Err(anyhow::Error::new(DownloadCancelled));
                }
                let n = entry.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
            }
        }
    }
    Ok(())
}

/// 流式下载单个文件到目标路径（进度回调；不覆盖已存在的非空文件）。
/// `cancel` 置位时尽快停止：删除 `.part` 临时文件并返回 [`DownloadCancelled`]。
pub fn download_file(
    url: &str,
    target: &Path,
    progress: Option<&ProgressFn>,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<()> {
    download_file_checked(url, target, progress, cancel, None)
}

fn download_file_checked(
    url: &str,
    target: &Path,
    progress: Option<&ProgressFn>,
    cancel: Option<&AtomicBool>,
    expected_sha1: Option<&str>,
) -> anyhow::Result<()> {
    if target.exists() && target.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        if let Some(expected) = expected_sha1 {
            let actual = sha1_file(target)?;
            if actual == expected {
                write_verification_marker(target, expected)?;
                log::info!("已有模型完整性校验通过: file={} sha1={expected}", target.display());
                return Ok(());
            }
            std::fs::remove_file(target)?;
            log::warn!("已有模型 SHA-1 不匹配，已删除并准备重新下载: {}", target.display());
        } else {
            return Ok(());
        }
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let part = target.with_extension("part");
    // 下载开始前先检查取消（用户可能在下一次尝试前已取消）
    if cancelled(cancel) {
        let _ = std::fs::remove_file(&part);
        return Err(anyhow::Error::new(DownloadCancelled));
    }
    let existing = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    log::info!(
        "模型文件下载开始: file={} resumed_bytes={} source={}",
        target.display(),
        existing,
        url
    );
    // 短读超时：网络卡住时 `read()` 最多阻塞 2s 就醒来，让取消标志能及时生效；
    // 正常下载时数据持续到达，read 立即返回，不触发超时。
    // 注意：不能设整体超时（ureq 的 `.timeout()` 会覆盖 timeout_read，见 stream.rs
    // "deadline 优先"逻辑），停滞保护改由读取循环里的"无进展超时"承担。
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .timeout_read(std::time::Duration::from_secs(2))
        .build();
    let mut request = agent.get(url);
    if existing > 0 {
        request = request.set("Range", &format!("bytes={existing}-"));
    }
    let resp = request.call().map_err(|e| {
        // 连接/响应头阶段也可能卡住：用户已点取消则报"已取消"
        if cancelled(cancel) {
            return anyhow::Error::new(DownloadCancelled);
        }
        anyhow::anyhow!("下载失败 {url}: {e}")
    })?;
    let partial_response = resp.status() == 206 && existing > 0;
    let content_len: u64 = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let total = if partial_response { existing.saturating_add(content_len) } else { content_len };
    let mut reader = resp.into_reader();
    let mut out = if partial_response {
        std::fs::OpenOptions::new().create(true).append(true).open(&part)?
    } else {
        std::fs::File::create(&part)?
    };
    let mut buf = [0u8; 262144];
    let mut received = if partial_response { existing } else { 0 };
    if partial_response {
        log::info!("模型文件断点续传已接受: file={} offset={} total={}", target.display(), existing, total);
    } else if existing > 0 {
        log::warn!("下载源未接受 Range，将从头下载: file={} previous_bytes={}", target.display(), existing);
    }
    let mut last_logged_percent = if total > 0 { ((received * 100 / total) / 10) * 10 } else { 0 };
    let mut next_log_bytes = received.saturating_add(64 * MIB);
    // 停滞保护：超过该时长没有任何新字节到达则放弃（替代 ureq 整体超时，
    // 因为整体超时会覆盖 timeout_read，导致取消无法唤醒）。
    let stall_limit = std::time::Duration::from_secs(300);
    let mut last_data_at = std::time::Instant::now();
    loop {
        if cancelled(cancel) {
            drop(out);
            let _ = std::fs::remove_file(&part);
            log::info!("模型文件下载已取消: file={} received_mb={:.1}", target.display(), received as f64 / MIB as f64);
            return Err(anyhow::Error::new(DownloadCancelled));
        }
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.write_all(&buf[..n])?;
                received += n as u64;
                last_data_at = std::time::Instant::now();
                if let Some(cb) = progress {
                    cb(received, total);
                }
                if total > 0 {
                    let percent = received.saturating_mul(100) / total;
                    if percent >= last_logged_percent.saturating_add(10) || received == total {
                        last_logged_percent = (percent / 10) * 10;
                        log::info!(
                            "模型文件下载进度: file={} percent={} received_mb={:.1} total_mb={:.1}",
                            target.display(), percent.min(100), received as f64 / MIB as f64, total as f64 / MIB as f64
                        );
                    }
                } else if received >= next_log_bytes {
                    log::info!("模型文件下载进度: file={} received_mb={:.1} total=unknown", target.display(), received as f64 / MIB as f64);
                    next_log_bytes = received.saturating_add(64 * MIB);
                }
            }
            // 读超时（网络停滞/卡住）：醒来检查取消标志，未取消则继续读
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                if cancelled(cancel) {
                    drop(out);
                    let _ = std::fs::remove_file(&part);
                    log::info!("模型文件下载已取消(读超时): file={} received_mb={:.1}", target.display(), received as f64 / MIB as f64);
                    return Err(anyhow::Error::new(DownloadCancelled));
                }
                if last_data_at.elapsed() > stall_limit {
                    drop(out);
                    let _ = std::fs::remove_file(&part);
                    anyhow::bail!(
                        "下载停滞超过 {:.0}s 无数据（网络中断？），已清理临时文件: file={} received_mb={:.1}",
                        stall_limit.as_secs() as f64,
                        target.display(),
                        received as f64 / MIB as f64
                    );
                }
                continue;
            }
            Err(e) => return Err(anyhow::anyhow!("读取下载流失败: {e}")),
        }
    }
    out.flush()?;
    drop(out);
    // 下载完成校验非空：空文件（网络中断/服务端异常）不能当作成功
    let part_len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if part_len == 0 {
        let _ = std::fs::remove_file(&part);
        anyhow::bail!("下载内容为空（网络中断？），已清理临时文件，请重试");
    }
    if let Some(expected) = expected_sha1 {
        log::info!("模型文件开始完整性校验: file={} algorithm=sha1", target.display());
        let actual = sha1_file(&part)?;
        if actual != expected {
            let _ = std::fs::remove_file(&part);
            anyhow::bail!("模型完整性校验失败：SHA-1 不匹配（期望 {expected}，实际 {actual}），已删除损坏文件");
        }
        log::info!("模型文件完整性校验通过: file={} sha1={actual}", target.display());
    }
    std::fs::rename(&part, target)?;
    if let Some(expected) = expected_sha1 {
        write_verification_marker(target, expected)?;
    }
    log::info!("模型文件下载完成: file={} size_mb={:.1}", target.display(), target.metadata()?.len() as f64 / MIB as f64);
    Ok(())
}

fn write_verification_marker(target: &Path, sha1: &str) -> anyhow::Result<()> {
    let marker = target.with_extension("sha1");
    let size = target.metadata()?.len();
    std::fs::write(marker, format!("{sha1} {size}\n"))?;
    Ok(())
}

fn sha1_file(path: &Path) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha1::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 { break; }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    static NETWORK_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    fn sha1_and_marker_make_metal_model_available() {
        let tmp = std::env::temp_dir().join(format!("talksage-metal-marker-{}", std::process::id()));
        let dir = tmp.join(EngineKind::WhisperLargeV3TurboMetal.model_dir_name());
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("ggml-large-v3-turbo-q5_0.bin");
        std::fs::write(&model, b"abc").unwrap();
        assert_eq!(sha1_file(&model).unwrap(), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert!(!EngineKind::WhisperLargeV3TurboMetal.is_available(&tmp));
        write_verification_marker(&model, METAL_MODEL_SHA1).unwrap();
        assert!(EngineKind::WhisperLargeV3TurboMetal.is_available(&tmp));
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn qwen_space_budget_includes_archive_and_staging() {
        assert_eq!(required_free_bytes(EngineKind::Qwen3Asr), (878 * 2 + 256) * MIB);
        assert_eq!(required_free_bytes(EngineKind::WhisperLargeV3TurboMetal), (547 + 256) * MIB);
    }

    #[test]
    fn punct_archive_extracts_only_model_onnx() {
        let tmp = std::env::temp_dir().join(format!("talksage-punct-archive-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let archive = tmp.join("punct.tar.bz2");
        let file = std::fs::File::create(&archive).unwrap();
        let encoder = bzip2::write::BzEncoder::new(file, bzip2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let data = b"fake-onnx";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, "punct-model/model.onnx", &data[..]).unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();
        let dest = tmp.join("model.onnx");
        extract_punct_model(&archive, &dest, None).unwrap();
        assert_eq!(std::fs::read(dest).unwrap(), data);
        let _ = std::fs::remove_dir_all(tmp);
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
    fn product_sources_have_nonempty_names() {
        for kind in EngineKind::ALL {
            if kind == EngineKind::Qwen3Asr {
                continue;
            }
            let names = sources(kind);
            assert!(!names.is_empty());
            assert!(names.iter().all(|(f, _)| !f.is_empty()), "{} 文件名不应为空", kind.display_name());
        }
    }

    /// 取消标志置位后，下载应返回 DownloadCancelled 并清理 .part 临时文件。
    #[test]
    fn cancel_flag_stops_download_and_cleans_part() {
        use std::net::TcpListener;
        use std::sync::atomic::AtomicBool;
        use std::thread;

        let _env_guard = NETWORK_ENV_LOCK.lock().unwrap();
        // 起一个本地 HTTP server，持续返回数据（模拟大文件下载）
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(mut s) = stream {
                    let _ = s.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 100000000\r\n\r\n",
                    );
                    // 持续灌数据直到对端断开
                    let payload = vec![0x5Au8; 65536];
                    loop {
                        if s.write_all(&payload).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // 本地测试不能被环境代理劫持（HTTP_PROXY 会把 127.0.0.1 也转发出去）
        let saved_http = std::env::var_os("HTTP_PROXY");
        let saved_https = std::env::var_os("HTTPS_PROXY");
        let saved_http_l = std::env::var_os("http_proxy");
        let saved_https_l = std::env::var_os("https_proxy");
        std::env::remove_var("HTTP_PROXY");
        std::env::remove_var("HTTPS_PROXY");
        std::env::remove_var("http_proxy");
        std::env::remove_var("https_proxy");

        let result = (|| {
            let tmp = std::env::temp_dir().join(format!("talksage-cancel-{}", std::process::id()));
            std::fs::create_dir_all(&tmp).unwrap();
            let target = tmp.join("big.bin");
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            // 预先置位取消标志：下载线程首个循环就应检测到并清理 .part
            cancel.store(true, Ordering::Relaxed);
            let result = download_file(&format!("http://{addr}/big.bin"), &target, None, Some(cancel.as_ref()));
            let err = result.expect_err("取消标志已置位，应返回错误");
            assert!(
                err.downcast_ref::<DownloadCancelled>().is_some(),
                "应返回 DownloadCancelled，实际: {err}"
            );
            assert!(!target.with_extension("part").exists(), ".part 应被清理");
            assert!(!target.exists(), "目标文件不应残留");
            let _ = std::fs::remove_dir_all(&tmp);
        })();

        // 恢复代理环境变量
        for (k, v) in [("HTTP_PROXY", saved_http), ("HTTPS_PROXY", saved_https), ("http_proxy", saved_http_l), ("https_proxy", saved_https_l)] {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        let _ = result;
    }

    /// 下载**中途**取消：服务器发送部分数据后停顿（模拟网络卡住），
    /// 此时 read() 阻塞在 socket 上——必须靠读超时醒来检查取消标志，
    /// 而不是一直等整体 900s 超时。回归：此前 read 无短超时，取消永远不生效。
    #[test]
    fn cancel_mid_download_wakes_up_on_read_timeout() {
        use std::net::TcpListener;
        use std::sync::atomic::AtomicBool;
        use std::sync::mpsc;
        use std::thread;
        use std::time::{Duration, Instant};

        let _env_guard = NETWORK_ENV_LOCK.lock().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // 服务器：发响应头 + 前 1 MiB 数据，然后挂起连接（不再发数据 = 网络卡住）
        thread::spawn(move || {
            if let Ok(mut s) = listener.accept().map(|(s, _)| s) {
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000000\r\n\r\n");
                let payload = vec![0x5Au8; 65536];
                for _ in 0..16 {
                    if s.write_all(&payload).is_err() {
                        break;
                    }
                }
                // 之后不再发送数据，连接保持打开（阻塞 read 的场景）
                std::thread::sleep(Duration::from_secs(60));
            }
        });

        let saved_http = std::env::var_os("HTTP_PROXY");
        let saved_https = std::env::var_os("HTTPS_PROXY");
        let saved_http_l = std::env::var_os("http_proxy");
        let saved_https_l = std::env::var_os("https_proxy");
        std::env::remove_var("HTTP_PROXY");
        std::env::remove_var("HTTPS_PROXY");
        std::env::remove_var("http_proxy");
        std::env::remove_var("https_proxy");

        let (tx, rx) = mpsc::channel();
        let result = (|| {
            let tmp = std::env::temp_dir().join(format!("talksage-cancel-mid-{}", std::process::id()));
            std::fs::create_dir_all(&tmp).unwrap();
            let target = tmp.join("big.bin");
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            let cancel_clone = cancel.clone();
            let target_clone = target.clone();
            let url = format!("http://{addr}/big.bin");
            let dl = thread::spawn(move || {
                download_file(&url, &target_clone, None, Some(cancel_clone.as_ref()))
            });
            // 等下载线程把前 1 MiB 读掉、进入卡住状态，再置位取消
            thread::sleep(Duration::from_millis(500));
            let start = Instant::now();
            cancel.store(true, Ordering::Relaxed);
            let result = dl.join().expect("下载线程不应 panic");
            let elapsed = start.elapsed();
            let err = result.expect_err("取消标志已置位，应返回错误");
            assert!(
                err.downcast_ref::<DownloadCancelled>().is_some(),
                "应返回 DownloadCancelled，实际: {err}"
            );
            assert!(
                elapsed < Duration::from_secs(30),
                "取消应在读超时窗口内生效，实际耗时 {elapsed:?}"
            );
            assert!(!target.with_extension("part").exists(), ".part 应被清理");
            assert!(!target.exists(), "目标文件不应残留");
            let _ = std::fs::remove_dir_all(&tmp);
            tx.send(()).unwrap();
        })();

        // 等服务器线程退出（否则 stdout 挂起）——服务器 60s 后自然退出；
        // 测试主体完成后用 rx 同步，确保断言先于服务器清理。
        let _ = result;
        let _ = rx.recv_timeout(Duration::from_secs(35));

        // 恢复代理环境变量
        for (k, v) in [("HTTP_PROXY", saved_http), ("HTTPS_PROXY", saved_https), ("http_proxy", saved_http_l), ("https_proxy", saved_https_l)] {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }

    #[test]
    fn download_file_uses_http_proxy_from_env() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let _env_guard = NETWORK_ENV_LOCK.lock().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_addr = listener.local_addr().unwrap();
        let proxy = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("代理应收到连接");
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(
                req.contains("GET http://download.test.invalid/model.bin"),
                "HTTP 代理应收到绝对 URI，实际: {req}"
            );
            let body = b"onnx";
            let _ = stream.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                    .as_bytes(),
            );
            let _ = stream.write_all(body);
        });

        let keys = [
            "ALL_PROXY",
            "all_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
        ];
        let saved: Vec<_> = keys.iter().map(|k| (*k, std::env::var_os(k))).collect();
        for k in keys {
            std::env::remove_var(k);
        }
        let proxy_url = format!("http://{proxy_addr}");
        std::env::set_var("HTTP_PROXY", &proxy_url);
        std::env::set_var("http_proxy", &proxy_url);

        let result = (|| {
            let tmp = std::env::temp_dir().join(format!("talksage-proxy-{}", std::process::id()));
            std::fs::create_dir_all(&tmp).unwrap();
            let target = tmp.join("model.bin");
            download_file("http://download.test.invalid/model.bin", &target, None, None)
                .expect("走代理时应下载成功");
            assert_eq!(std::fs::read(&target).unwrap(), b"onnx");
            let _ = std::fs::remove_dir_all(&tmp);
        })();

        for (k, v) in saved {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        let _ = result;
        proxy.join().expect("代理线程异常");
    }

    #[test]
    fn download_file_resumes_existing_part_with_range() {
        use std::net::TcpListener;
        use std::thread;

        let _env_guard = NETWORK_ENV_LOCK.lock().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let n = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]);
            assert!(request.contains("Range: bytes=3-"), "应从已有 3 字节继续: {request}");
            stream.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nConnection: close\r\n\r\ndef").unwrap();
        });

        let tmp = std::env::temp_dir().join(format!("talksage-resume-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let target = tmp.join("model.bin");
        std::fs::write(target.with_extension("part"), b"abc").unwrap();
        download_file(&format!("http://{addr}/model.bin"), &target, None, None).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"abcdef");
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(tmp);
    }

    fn write_qwen_like_archive(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let encoder = bzip2::write::BzEncoder::new(file, bzip2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let top = "sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25";
        for (rel, data) in [
            ("conv_frontend.onnx", vec![0x11u8; 65536]),
            ("encoder.int8.onnx", vec![0x22u8; 65536]),
            ("decoder.int8.onnx", vec![0x33u8; 65536]),
            ("tokenizer/vocab.json", b"{\"a\":1}".to_vec()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{top}/{rel}"), data.as_slice())
                .unwrap();
        }
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();
    }

    #[test]
    fn unpack_qwen_archive_keeps_file_payloads() {
        let tmp = std::env::temp_dir().join(format!("talksage-qwen-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let archive = tmp.join("qwen.tar.bz2");
        let staging = tmp.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        write_qwen_like_archive(&archive);
        unpack_tar_bz2_strip_top(&archive, &staging, None).unwrap();
        let blob = |name: &str, byte: u8| {
            let data = std::fs::read(staging.join(name)).unwrap();
            assert_eq!(data.len(), 65536, "{name} 长度不对");
            assert!(
                data.iter().all(|&b| b == byte),
                "{name} 内容损坏（tar 条目必须边读边处理，不能先 collect）"
            );
        };
        blob("conv_frontend.onnx", 0x11);
        blob("encoder.int8.onnx", 0x22);
        blob("decoder.int8.onnx", 0x33);
        let vocab = staging.join("tokenizer").join("vocab.json");
        assert!(vocab.is_file() && vocab.metadata().unwrap().len() > 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
