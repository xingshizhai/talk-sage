//! GPU 后端检测：运行时探测可用的硬件加速后端。
//!
//! 检测策略（按优先级）：
//!   1. Apple CoreML（macOS only）：编译期已知，直接返回。
//!   2. NVIDIA CUDA（Windows/Linux）：尝试动态加载 CUDA runtime 库。
//!   3. 回退 CPU。

/// 可用的硬件加速后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBackend {
    /// NVIDIA CUDA GPU（Windows/Linux）。
    Cuda,
    /// Apple CoreML（macOS，Metal/ANE；M 系列最优）。
    CoreMl,
    /// 无受支持的 GPU，使用 CPU 推理。
    None,
}

impl GpuBackend {
    /// 运行时检测最优可用后端。
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            return Self::CoreMl;
        }
        #[cfg(not(target_os = "macos"))]
        if Self::cuda_available() {
            return Self::Cuda;
        }
        Self::None
    }

    /// 对应 sherpa-onnx `provider` 字段值。
    pub fn provider_str(self) -> &'static str {
        match self {
            Self::Cuda => "cuda",
            Self::CoreMl => "coreml",
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
            Self::CoreMl => "Apple CoreML (Metal)",
            Self::None => "CPU",
        }
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
        assert!(matches!(b, GpuBackend::Cuda | GpuBackend::CoreMl | GpuBackend::None));
    }

    #[test]
    fn provider_str_matches_backend() {
        assert_eq!(GpuBackend::Cuda.provider_str(), "cuda");
        assert_eq!(GpuBackend::CoreMl.provider_str(), "coreml");
        assert_eq!(GpuBackend::None.provider_str(), "cpu");
    }

    #[test]
    fn is_accelerated_only_for_gpu_backends() {
        assert!(GpuBackend::Cuda.is_accelerated());
        assert!(GpuBackend::CoreMl.is_accelerated());
        assert!(!GpuBackend::None.is_accelerated());
    }

    #[test]
    fn display_name_is_human_readable() {
        assert!(!GpuBackend::Cuda.display_name().is_empty());
        assert!(!GpuBackend::CoreMl.display_name().is_empty());
        assert!(!GpuBackend::None.display_name().is_empty());
    }
}
