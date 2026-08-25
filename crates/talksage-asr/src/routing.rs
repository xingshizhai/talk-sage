//! ASR execution routing. This module is deliberately free of model/network side effects so
//! adapters and the pipeline can validate a session before opening audio devices.

use crate::GpuBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrRoute {
    Local { backend: GpuBackend },
    AliyunCloud,
}

impl AsrRoute {
    pub fn provider(self) -> Option<&'static str> {
        match self {
            Self::Local { backend } => Some(backend.provider_str()),
            Self::AliyunCloud => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Local { backend } => backend.display_name(),
            Self::AliyunCloud => "阿里云实时语音识别",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CloudCredentials<'a> {
    pub access_key_id: &'a str,
    pub access_key_secret: &'a str,
    pub app_key: &'a str,
}

impl CloudCredentials<'_> {
    pub fn is_complete(self) -> bool {
        !self.access_key_id.trim().is_empty()
            && !self.access_key_secret.trim().is_empty()
            && !self.app_key.trim().is_empty()
    }
}

/// Resolve the effective ASR route.
///
/// Product policy for `auto` is intentionally strict: supported GPU means local inference;
/// otherwise Aliyun is required. CPU inference remains available only through explicit
/// `asr_mode = "local"` plus `backend = "cpu"` (or auto on a CPU-only machine) for offline,
/// private, and diagnostic use.
pub fn resolve_asr_route(
    mode: &str,
    backend_preference: &str,
    detected: GpuBackend,
    cloud: CloudCredentials<'_>,
) -> anyhow::Result<AsrRoute> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "auto" | "" => {
            if detected.is_accelerated() {
                Ok(AsrRoute::Local { backend: detected })
            } else if cloud.is_complete() {
                Ok(AsrRoute::AliyunCloud)
            } else {
                anyhow::bail!(
                    "未检测到受支持的 GPU，自动模式需要完整的阿里云 AccessKey ID、AccessKey Secret 和 AppKey"
                )
            }
        }
        "cloud" => {
            if cloud.is_complete() {
                Ok(AsrRoute::AliyunCloud)
            } else {
                anyhow::bail!(
                    "云端 ASR 配置不完整：需要 AccessKey ID、AccessKey Secret 和 AppKey"
                )
            }
        }
        "local" => resolve_local_backend(backend_preference, detected),
        other => anyhow::bail!("未知 ASR 模式 `{other}`，可选值：auto、local、cloud"),
    }
}

fn resolve_local_backend(preference: &str, detected: GpuBackend) -> anyhow::Result<AsrRoute> {
    let backend = match preference.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => detected,
        "cpu" => GpuBackend::None,
        "cuda" if detected == GpuBackend::Cuda => GpuBackend::Cuda,
        "coreml" | "metal" if detected == GpuBackend::CoreMl => GpuBackend::CoreMl,
        "cuda" => anyhow::bail!("已强制选择 CUDA，但当前机器未检测到 NVIDIA CUDA"),
        "coreml" | "metal" => {
            anyhow::bail!("已强制选择 CoreML/Metal，但当前 ASR 运行时未提供可用的 Apple GPU 后端")
        }
        "intel" | "openvino" | "directml" => {
            anyhow::bail!("Intel GPU 后端尚未实现；请使用 auto、cpu、cuda 或 coreml")
        }
        other => anyhow::bail!("未知 ASR backend `{other}`，可选值：auto、cpu、cuda、coreml"),
    };
    Ok(AsrRoute::Local { backend })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: CloudCredentials<'static> = CloudCredentials {
        access_key_id: "",
        access_key_secret: "",
        app_key: "",
    };
    const CLOUD: CloudCredentials<'static> = CloudCredentials {
        access_key_id: "id",
        access_key_secret: "secret",
        app_key: "app",
    };

    #[test]
    fn auto_prefers_supported_gpu() {
        assert_eq!(
            resolve_asr_route("auto", "auto", GpuBackend::Cuda, CLOUD).unwrap(),
            AsrRoute::Local { backend: GpuBackend::Cuda }
        );
        assert_eq!(
            resolve_asr_route("auto", "auto", GpuBackend::CoreMl, CLOUD).unwrap(),
            AsrRoute::Local { backend: GpuBackend::CoreMl }
        );
    }

    #[test]
    fn auto_uses_cloud_without_supported_gpu() {
        assert_eq!(
            resolve_asr_route("auto", "auto", GpuBackend::None, CLOUD).unwrap(),
            AsrRoute::AliyunCloud
        );
    }

    #[test]
    fn auto_without_gpu_requires_all_cloud_credentials() {
        assert!(resolve_asr_route("auto", "auto", GpuBackend::None, EMPTY).is_err());
        let missing_secret = CloudCredentials {
            access_key_id: "id",
            access_key_secret: "",
            app_key: "app",
        };
        assert!(resolve_asr_route("auto", "auto", GpuBackend::None, missing_secret).is_err());
    }

    #[test]
    fn explicit_local_cpu_remains_available() {
        assert_eq!(
            resolve_asr_route("local", "cpu", GpuBackend::None, EMPTY).unwrap(),
            AsrRoute::Local { backend: GpuBackend::None }
        );
    }

    #[test]
    fn unavailable_or_future_gpu_backend_is_rejected() {
        assert!(resolve_asr_route("local", "cuda", GpuBackend::None, EMPTY).is_err());
        assert!(resolve_asr_route("local", "intel", GpuBackend::None, EMPTY).is_err());
    }
}
