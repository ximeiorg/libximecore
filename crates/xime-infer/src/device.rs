//! 推理后端选择。
//!
//! feature 决定「这个包能表达什么」：CPU 包没有 `Metal`/`Cuda` 变体，误用在编译期
//! 被挡下；GPU 包在运行期仍可显式选 `Cpu`，初始化失败也能退回 CPU。

use candle_core::Device;

use crate::InferError;

/// 执行后端。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeviceKind {
    /// CPU（默认；小模型与手写识别足够）。
    Cpu,
    /// Apple Metal（仅 macOS + `metal` feature）。
    #[cfg(all(target_os = "macos", feature = "metal"))]
    Metal,
    /// NVIDIA CUDA（仅 `cuda` feature）。
    #[cfg(feature = "cuda")]
    Cuda,
}

impl DeviceKind {
    /// 探测可用后端：GPU 初始化失败时**静默回退 CPU**。
    ///
    /// 输入法不能因为显卡驱动异常而不可用，所以这里不返回错误。
    pub fn detect() -> Self {
        #[cfg(all(target_os = "macos", feature = "metal"))]
        {
            if Device::new_metal(0).is_ok() {
                return Self::Metal;
            }
        }
        #[cfg(feature = "cuda")]
        {
            if Device::new_cuda(0).is_ok() {
                return Self::Cuda;
            }
        }
        Self::Cpu
    }

    /// 构建具体设备；显式选择 GPU 且初始化失败时返回错误（由调用方决定是否回退）。
    pub fn build(self) -> Result<Device, InferError> {
        match self {
            Self::Cpu => Ok(Device::Cpu),
            #[cfg(all(target_os = "macos", feature = "metal"))]
            Self::Metal => {
                Device::new_metal(0).map_err(|e| InferError::Device(format!("Metal: {e}")))
            }
            #[cfg(feature = "cuda")]
            Self::Cuda => Device::new_cuda(0).map_err(|e| InferError::Device(format!("CUDA: {e}"))),
        }
    }

    /// 日志/诊断用的稳定名称。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            #[cfg(all(target_os = "macos", feature = "metal"))]
            Self::Metal => "metal",
            #[cfg(feature = "cuda")]
            Self::Cuda => "cuda",
        }
    }
}

impl std::fmt::Display for DeviceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_builds() {
        assert!(DeviceKind::Cpu.build().is_ok());
        assert_eq!(DeviceKind::Cpu.as_str(), "cpu");
    }

    #[test]
    fn detect_without_gpu_features_is_cpu() {
        #[cfg(not(any(all(target_os = "macos", feature = "metal"), feature = "cuda")))]
        assert_eq!(DeviceKind::detect(), DeviceKind::Cpu);
        // 开了 GPU feature 时结果取决于机器，不做强断言。
        let _ = DeviceKind::detect().as_str();
    }
}
