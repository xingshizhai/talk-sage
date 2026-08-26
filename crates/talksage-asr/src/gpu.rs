//! GPU 后端检测：运行时探测可用的硬件加速后端。
//!
//! 检测策略（按优先级）：
//!   1. Apple CoreML：只有运行时确实带 CoreML EP 才能选择。当前随
//!      `sherpa-onnx-sys` 1.13.5 分发的 macOS arm64 静态库会明确回退 CPU，
//!      因而不能仅凭 Apple Silicon 架构宣称 GPU 可用。
//!   2. NVIDIA CUDA（Windows/Linux）：尝试动态加载 CUDA runtime 库。
//!   3. 回退 CPU。

/// 可用的硬件加速后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBackend {
    /// NVIDIA CUDA GPU（Windows/Linux，sherpa-onnx CUDA EP）。
    Cuda,
    /// Apple Silicon Metal（whisper.cpp adapter）。
    Metal,
    /// Windows Vulkan GPU（whisper.cpp adapter；AMD/Intel/NVIDIA 通吃，同 Dictata）。
    Vulkan,
    /// 无受支持的 GPU，使用 CPU 推理。
    None,
}

impl GpuBackend {
    /// 运行时检测最优可用后端。
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            #[cfg(target_arch = "aarch64")]
            { Self::Metal }
            #[cfg(not(target_arch = "aarch64"))]
            { Self::None }
        }
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            // whisper.cpp Vulkan 优先：AMD/Intel/NVIDIA 通吃，只要系统有
            // Vulkan runtime（显卡驱动自带 loader）。注意：只有本 crate 以
            // `vulkan-gpu` feature 编译（whisper-rs vulkan）时才认为可用——
            // 否则运行时检测通过但引擎加载会失败。
            #[cfg(feature = "vulkan-gpu")]
            {
                if Self::vulkan_available() {
                    return Self::Vulkan;
                }
            }
            if Self::cuda_available() {
                Self::Cuda
            } else {
                Self::None
            }
        }
        #[cfg(all(not(target_os = "macos"), not(all(target_os = "windows", target_arch = "x86_64"))))]
        {
            if Self::cuda_available() {
                Self::Cuda
            } else {
                Self::None
            }
        }
    }

    /// 对应 sherpa-onnx `provider` 字段值。
    pub fn provider_str(self) -> &'static str {
        match self {
            Self::Cuda => "cuda",
            Self::Metal => "metal",
            Self::Vulkan => "vulkan",
            Self::None => "cpu",
        }
    }

    /// 是否比 CPU 更快（用于 UI 展示和自动路由）。
    pub fn is_accelerated(self) -> bool {
        !matches!(self, Self::None)
    }

    /// 人类可读名称（用于设置界面显示）。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Cuda => "NVIDIA CUDA",
            Self::Metal => "Apple Metal (whisper.cpp)",
            Self::Vulkan => "Vulkan GPU (whisper.cpp)",
            Self::None => "CPU",
        }
    }

    /// 物理平台说明；与真正可供 ASR 使用的 provider 分开，避免把“有 Apple
    /// GPU”误报为“当前推理运行时正在使用 GPU”。
    pub fn hardware_candidate() -> &'static str {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        { "Apple Silicon GPU（Metal）" }
        #[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
        { "Intel Mac（尚无本地 GPU 后端）" }
        #[cfg(not(target_os = "macos"))]
        { Self::detect().display_name() }
    }

    /// 解释物理 GPU 与当前 ASR 推理后端为何可能不一致，供 UI 和日志诊断。
    pub fn availability_note() -> &'static str {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            "已检测到 Apple Silicon GPU；本地 ASR 使用 whisper.cpp Metal adapter，不使用会回退 CPU 的 sherpa-onnx CoreML provider"
        }
        #[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
        {
            "已检测到 Intel Mac；当前尚未接入 Intel GPU ASR 后端"
        }
        #[cfg(not(target_os = "macos"))]
        {
            match Self::detect() {
                Self::Cuda => "已检测到可供 ASR 使用的 NVIDIA CUDA runtime",
                Self::Metal => "已检测到可供 ASR 使用的 Apple Metal runtime",
                Self::Vulkan => "已检测到可供 ASR 使用的 Vulkan GPU（whisper.cpp）",
                Self::None => "未检测到当前 ASR 运行时支持的 GPU 后端",
            }
        }
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64", feature = "vulkan-gpu"))]
    fn vulkan_available() -> bool {
        // Vulkan loader 由显卡驱动提供（vulkan-1.dll），不依赖 Vulkan SDK。
        // 注意：这是"运行时可用"检测；构建 whisper.cpp Vulkan 仍需 VULKAN_SDK。
        unsafe { libloading::Library::new("vulkan-1.dll").is_ok() }
    }

    #[cfg(not(target_os = "macos"))]
    fn cuda_available() -> bool {
        // 仅探测库是否可加载，不调用任何 CUDA API
        #[cfg(target_os = "windows")]
        { unsafe { libloading::Library::new("nvcuda.dll").is_ok() } }
        #[cfg(not(target_os = "windows"))]
        { unsafe { libloading::Library::new("libcuda.so.1").is_ok() } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_a_valid_variant() {
        let b = GpuBackend::detect();
        assert!(matches!(b, GpuBackend::Cuda | GpuBackend::Metal | GpuBackend::Vulkan | GpuBackend::None));
    }

    #[test]
    fn provider_str_matches_backend() {
        assert_eq!(GpuBackend::Cuda.provider_str(), "cuda");
        assert_eq!(GpuBackend::Metal.provider_str(), "metal");
        assert_eq!(GpuBackend::Vulkan.provider_str(), "vulkan");
        assert_eq!(GpuBackend::None.provider_str(), "cpu");
    }

    #[test]
    fn is_accelerated_only_for_gpu_backends() {
        assert!(GpuBackend::Cuda.is_accelerated());
        assert!(GpuBackend::Metal.is_accelerated());
        assert!(GpuBackend::Vulkan.is_accelerated());
        assert!(!GpuBackend::None.is_accelerated());
    }

    #[test]
    fn display_name_is_human_readable() {
        assert!(!GpuBackend::Cuda.display_name().is_empty());
        assert!(!GpuBackend::Metal.display_name().is_empty());
        assert!(!GpuBackend::Vulkan.display_name().is_empty());
        assert!(!GpuBackend::None.display_name().is_empty());
    }

    #[test]
    fn availability_note_is_not_empty() {
        assert!(!GpuBackend::availability_note().is_empty());
    }
}
