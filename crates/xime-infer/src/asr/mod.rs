//! 本地语音识别：按模型族分开，一族一个目录。
//!
//! ```text
//! asr/
//!   mod.rs        家族识别、进程级单实例闸门、权重分片发现（与模型无关的部分）
//!   audio.rs      PCM 输入输出：WAV 解码 + 重采样（两族共用）
//!   qwen3_asr/    Qwen3-ASR：Whisper 式 log-mel + 18 层音频塔 + 输出「语种 + 文本」
//!   fun_asr/      Fun-ASR-Nano：Kaldi fbank + LFR + 70 层 SAN-M + 纯文本输出
//! ```
//!
//! **每族目录自成一体**：config / 前处理 / 编码器 / 文本解码器 / 分词器 / prompt
//! 都在自己的目录里各写一份，即使两族都用 Qwen3 当文本侧也不例外。前处理与 prompt
//! 约定差异太大（mel vs fbank、`<|audio_pad|>` vs 纯文本指令），共享实现只会让两边
//! 都被对方的约束牵制，改 A 时得验证 B。这里只有「与模型无关」的三件事放在外层。
//!
//! 宿主按目录结构选引擎：
//!
//! ```text
//! let family = AsrFamily::detect(Path::new("/path/to/model"))?;
//! match family {
//!     AsrFamily::Qwen3Asr => { let mut asr = qwen3_asr::Qwen3Asr::from_dir(dir)?; /* … */ }
//!     AsrFamily::FunAsrNano => { let mut asr = fun_asr::FunAsrNano::from_dir(dir)?; /* … */ }
//! }
//! ```
//!
//! 两族的公共约束：
//!
//! - 权重都是 `model.safetensors`（可分片 + index），目录里另有 `config.json` 与分词器；
//! - 识别器持有 KV cache，`transcribe` 需要 `&mut self`，由宿主单线程 worker 独占；
//! - 单次音频上限 30 秒；
//! - **同一进程只允许一个识别器**（GPU 显存与驱动限制），见 [`AsrFamily`] 下面的闸门。

pub mod audio;
pub mod fun_asr;
pub mod qwen3_asr;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use candle_core::DType;

use crate::device::DeviceKind;
use crate::InferError;

/// 便于既有调用方继续写 `asr::Qwen3Asr`；新代码建议直接用 [`qwen3_asr::Qwen3Asr`]。
pub use qwen3_asr::Qwen3Asr;

/// 装载与解码选项（与模型族无关，两族共用同一份）。
///
/// 字段语义两族都适用，只是 `language` 的落点不同：Qwen3-ASR 用它预填
/// `language X<asr_text>`，Fun-ASR 用它拼进「语音转写成 X：」这句指令。
#[derive(Debug, Clone)]
pub struct AsrOptions {
    /// 执行后端，默认 [`DeviceKind::detect`]。
    pub device: DeviceKind,
    /// 权重 dtype。默认 f32（CPU 上最稳）；GPU 上可试 bf16/f16 省一半内存。
    pub dtype: DType,
    /// 单次最多生成多少 token。
    pub max_new_tokens: usize,
    /// 强制语种（如 `"Chinese"` / `"zh"`）。`None` 表示让模型自己判。
    pub language: Option<String>,
}

impl Default for AsrOptions {
    fn default() -> Self {
        Self {
            device: default_device(),
            dtype: DType::F32,
            max_new_tokens: 512,
            language: None,
        }
    }
}

/// ASR 的默认后端。
///
/// - 构建带 `cuda`：用 [`DeviceKind::detect`]（探测成功就用 CUDA，驱动缺失回退 CPU）。
///   CUDA 上逐 token 解码比 CPU 快，显存常驻 1.6–3 GB，可接受。
/// - 其余（CPU / Metal）：固定 CPU。Metal 上解码是 dispatch-bound，实测不比 CPU 快，
///   却要占 1.6 GB 显存；需要 Metal 时由调用方显式传 `DeviceKind::Metal`。
fn default_device() -> DeviceKind {
    #[cfg(feature = "cuda")]
    {
        DeviceKind::detect()
    }
    #[cfg(not(feature = "cuda"))]
    {
        DeviceKind::Cpu
    }
}

/// 一次识别结果（两族共用）。
#[derive(Debug, Clone)]
pub struct Transcript {
    /// 识别文本（不带任何头部标记）。
    pub text: String,
    /// 模型判定/调用方指定的语种（`None` 表示静音或未判定）。
    pub language: Option<String>,
    pub audio_ms: f64,
    /// 音频编码器耗时。
    pub encode_ms: f64,
    /// 解码器耗时（prefill + 自回归）。
    pub decode_ms: f64,
    pub audio_tokens: usize,
    pub text_tokens: usize,
    /// 原始生成 token（解码前的 id，便于对拍与调试）。
    pub token_ids: Vec<u32>,
}

impl Transcript {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// 模型目录属于哪个 ASR 家族（看 `config.json`）。
///
/// 两族目录里都有 `config.json` / `model.safetensors` / `tokenizer.json`，光看文件名
/// 分不出来；判据是 `model_type`，老配置没有该字段时再看有没有 `adaptor_config`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrFamily {
    Qwen3Asr,
    FunAsrNano,
}

impl AsrFamily {
    /// 读模型目录的 `config.json` 判断家族。
    pub fn detect(dir: &Path) -> Result<Self, InferError> {
        let path = dir.join("config.json");
        let text = std::fs::read_to_string(&path).map_err(|source| InferError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_json(&serde_json::from_str(&text)?)
    }

    pub fn from_json(root: &serde_json::Value) -> Result<Self, InferError> {
        if fun_asr::FunAsrConfig::is_fun_asr(root) {
            return Ok(Self::FunAsrNano);
        }
        // 剩余情况按 Qwen3-ASR 处理，但要求它有 audio_config，否则连报错都看不懂。
        if root.get("audio_config").is_some() {
            return Ok(Self::Qwen3Asr);
        }
        Err(InferError::Config(
            "config.json 既不是 Qwen3-ASR 也不是 Fun-ASR-Nano（缺 audio_config）".into(),
        ))
    }
}

/// 进程级单实例闸门。
///
/// 每个实例在 GPU 上要常驻 1.2–1.6 GB 权重 + KV cache；两个实例并发装载会在
/// macOS 上触发 `IOGPUGroupMemory` 断言（内核 panic）。输入法本来就只需要一个
/// 识别器，这里直接挡死第二个——两族共用同一道闸门，切换模型必须先卸载。
static ASR_INSTANCE: AtomicBool = AtomicBool::new(false);

/// 单实例闸门的持有凭证，随识别器一起释放。
pub(crate) struct InstanceGuard;

impl InstanceGuard {
    fn acquire() -> Result<Self, InferError> {
        if ASR_INSTANCE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(InferError::AsrBusy(
                "同一进程只能装载一个 ASR 模型（GPU 显存与驱动限制）".into(),
            ));
        }
        Ok(Self)
    }
}

/// 取得单实例闸门；第二个实例会返回 [`InferError::AsrBusy`]。
pub(crate) fn acquire_instance() -> Result<InstanceGuard, InferError> {
    InstanceGuard::acquire()
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        ASR_INSTANCE.store(false, Ordering::SeqCst);
    }
}

/// 收集权重分片：优先 `model.safetensors.index.json`，否则单文件。
pub fn collect_shards(dir: &Path) -> Result<Vec<PathBuf>, InferError> {
    let index = dir.join("model.safetensors.index.json");
    if index.exists() {
        let text = std::fs::read_to_string(&index).map_err(|source| InferError::Io {
            path: index.display().to_string(),
            source,
        })?;
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let map = v
            .get("weight_map")
            .and_then(|m| m.as_object())
            .ok_or_else(|| InferError::Config("index.json 缺少 weight_map".into()))?;
        let mut names: Vec<String> = map
            .values()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        names.sort_unstable();
        names.dedup();
        let paths: Vec<PathBuf> = names.into_iter().map(|n| dir.join(n)).collect();
        for p in &paths {
            if !p.exists() {
                return Err(InferError::MissingFile(p.display().to_string()));
            }
        }
        Ok(paths)
    } else {
        let single = dir.join("model.safetensors");
        if !single.exists() {
            return Err(InferError::MissingFile(single.display().to_string()));
        }
        Ok(vec![single])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_model_dir_is_reported() {
        let err = collect_shards(Path::new("/nonexistent/xime-asr")).unwrap_err();
        assert!(matches!(err, InferError::MissingFile(_)));
    }

    #[test]
    fn family_detection_prefers_model_type() {
        let qwen = serde_json::json!({ "model_type": "qwen3_asr", "audio_config": {} });
        assert_eq!(AsrFamily::from_json(&qwen).unwrap(), AsrFamily::Qwen3Asr);

        let fun = serde_json::json!({ "model_type": "fun_asr_nano", "audio_config": {} });
        assert_eq!(AsrFamily::from_json(&fun).unwrap(), AsrFamily::FunAsrNano);

        // 缺 model_type 的老配置靠 adaptor_config 区分
        let legacy = serde_json::json!({ "audio_config": {}, "adaptor_config": {} });
        assert_eq!(
            AsrFamily::from_json(&legacy).unwrap(),
            AsrFamily::FunAsrNano
        );

        let none = serde_json::json!({ "model_type": "foo" });
        assert!(AsrFamily::from_json(&none).is_err());
    }
}
