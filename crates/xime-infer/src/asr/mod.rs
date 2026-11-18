//! Qwen3-ASR-0.6B / 1.7B 离线语音识别。
//!
//! 管线：
//!
//! ```text
//! PCM(16 kHz) → log-mel(128) → 音频塔 → 音频特征
//!                                          │
//!   prompt: <|im_start|>system…<|audio_start|>  ─┘
//!           <音频特征> <|audio_end|>…<|im_start|>assistant\n
//!                                          ↓
//!                                Qwen3 解码器（贪心）
//!                                          ↓
//!                        language Chinese<asr_text>文本
//! ```
//!
//! 使用方式：
//!
//! ```text
//! let mut asr = Qwen3Asr::from_dir(Path::new("/path/to/Qwen3-ASR-0.6B"))?;
//! let out = asr.transcribe_wav(Path::new("a.wav"))?;
//! println!("{}", out.text);
//! ```
//!
//! 注意：
//! - 识别器持有 KV cache，`transcribe` 需要 `&mut self`；宿主用单线程 worker 独占。
//! - 单次音频上限 30 秒（模型训练窗口）。更长的音频由宿主切段。
//! - 首次调用（尤其 Metal）会编译 kernel，耗时从几百毫秒到十几秒不等，
//!   必须用 [`Qwen3Asr::warmup`] 在后台预热。

pub mod audio;
pub mod config;
pub mod decoder;
pub mod encoder;
pub mod tokenizer;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;

pub use config::{AsrConfig, AudioTowerConfig, TextConfig, WeightLayout};
pub use decoder::Decoder;
pub use encoder::Encoder;
use tokenizer::{build_prompt, parse_output, AsrTokenizer};

use crate::asr::audio::{log_mel, AudioFrontend};
use crate::device::DeviceKind;
use crate::InferError;

/// 模型单次可处理的音频上限（秒）。Qwen3-ASR 按 30 秒窗口训练。
pub const MAX_AUDIO_SECS: f32 = 30.0;

/// 进程级单实例闸门。
///
/// 每个实例在 GPU 上要常驻 1.6 GB（f16）权重 + KV cache；两个实例并发装载会在
/// macOS 上触发 `IOGPUGroupMemory` 断言（内核 panic）。输入法本来就只需要一个
/// 识别器，这里直接挡死第二个。
static ASR_INSTANCE: AtomicBool = AtomicBool::new(false);

struct InstanceGuard;

impl InstanceGuard {
    fn acquire() -> Result<Self, InferError> {
        if ASR_INSTANCE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(InferError::AsrBusy(
                "同一进程只能装载一个 Qwen3Asr（GPU 显存与驱动限制）".into(),
            ));
        }
        Ok(Self)
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        ASR_INSTANCE.store(false, Ordering::SeqCst);
    }
}

/// 装载与解码选项。
#[derive(Debug, Clone)]
pub struct AsrOptions {
    /// 执行后端，默认 [`DeviceKind::detect`]。
    pub device: DeviceKind,
    /// 权重 dtype。默认 f32（CPU 上最稳）；Metal 可试 f16 省一半内存。
    pub dtype: DType,
    /// 单次最多生成多少 token。
    pub max_new_tokens: usize,
    /// 强制语种（如 `"Chinese"`）。`None` 表示让模型自己判。
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

/// 一次识别结果。
#[derive(Debug, Clone)]
pub struct Transcript {
    /// 识别文本（已去掉 `language X<asr_text>` 前缀）。
    pub text: String,
    /// 模型判定的语种（`None` 表示静音/未识别）。
    pub language: Option<String>,
    pub audio_ms: f64,
    /// 音频塔耗时。
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

/// 已装载的 Qwen3-ASR。
pub struct Qwen3Asr {
    cfg: AsrConfig,
    encoder: Encoder,
    decoder: Decoder,
    tokenizer: AsrTokenizer,
    front: AudioFrontend,
    device: Device,
    dtype: DType,
    max_new_tokens: usize,
    language: Option<String>,
    model_dir: PathBuf,
    _guard: InstanceGuard,
}

impl Qwen3Asr {
    /// 用默认选项装载（自动探测后端）。
    pub fn from_dir(dir: &Path) -> Result<Self, InferError> {
        Self::from_dir_with(dir, AsrOptions::default())
    }

    pub fn from_dir_with(dir: &Path, opts: AsrOptions) -> Result<Self, InferError> {
        let guard = InstanceGuard::acquire()?;
        let cfg = AsrConfig::load(dir)?;
        let device = opts.device.build()?;
        let shards = collect_shards(dir)?;

        // SAFETY: 模型文件在识别器存活期间不被修改（宿主的模型目录是只读的）。
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&shards, opts.dtype, &device)
                .map_err(InferError::from)?
        };

        let encoder = Encoder::load(&vb, &cfg.audio, cfg.layout, &device)?;
        let decoder = Decoder::load(&vb, &cfg.text, cfg.layout, &device, opts.dtype)?;
        let tokenizer = AsrTokenizer::from_dir(dir)?;
        tokenizer.validate_prompt_tokens()?;

        if cfg.audio.output_dim != cfg.text.hidden_size {
            return Err(InferError::Config(format!(
                "音频塔输出 {} 与文本模型 hidden {} 不一致",
                cfg.audio.output_dim, cfg.text.hidden_size
            )));
        }

        let front = AudioFrontend::new(16_000, 400, 160, cfg.audio.num_mel_bins);

        Ok(Self {
            cfg,
            encoder,
            decoder,
            tokenizer,
            front,
            device,
            dtype: opts.dtype,
            max_new_tokens: opts.max_new_tokens.max(1),
            language: opts.language,
            model_dir: dir.to_path_buf(),
            _guard: guard,
        })
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn dtype(&self) -> DType {
        self.dtype
    }

    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    pub fn config(&self) -> &AsrConfig {
        &self.cfg
    }

    /// 改强制语种（`None` = 自动判别）。
    pub fn set_language(&mut self, language: Option<String>) {
        self.language = language;
    }

    /// 识别 16 kHz 单声道 PCM。
    pub fn transcribe(&mut self, pcm: &[f32]) -> Result<Transcript, InferError> {
        self.transcribe_with_language(pcm, self.language.clone())
    }

    pub fn transcribe_with_language(
        &mut self,
        pcm: &[f32],
        language: Option<String>,
    ) -> Result<Transcript, InferError> {
        if pcm.is_empty() {
            return Err(InferError::EmptyAudio);
        }
        let secs = pcm.len() as f32 / self.front.sample_rate as f32;
        if secs > MAX_AUDIO_SECS {
            return Err(InferError::AudioTooLong {
                secs,
                max: MAX_AUDIO_SECS,
            });
        }

        let audio_ms = secs as f64 * 1000.0;
        let valid_frames = self.front.frames(pcm.len());
        if valid_frames == 0 {
            return Err(InferError::EmptyAudio);
        }

        // ---- 音频塔 ----
        let t_enc = Instant::now();
        let audio_embeds = self.encode_from(pcm, valid_frames)?;
        let (n_audio, _) = audio_embeds.dims2()?;
        let encode_ms = t_enc.elapsed().as_secs_f64() * 1000.0;

        // ---- 拼 prompt ----
        let t_dec = Instant::now();
        let prompt = build_prompt(
            &self.tokenizer,
            self.cfg.audio_start_token_id,
            self.cfg.audio_end_token_id,
            language.as_deref(),
        )?;
        let prefix = self.embed_ids(&prompt.prefix)?;
        let suffix = self.embed_ids(&prompt.suffix)?;
        let audio = audio_embeds.unsqueeze(0)?;
        let prompt_embeds = Tensor::cat(&[&prefix, &audio, &suffix], 1)?;
        let prompt_len = prompt_embeds.dims()[1];

        // ---- prefill + 贪心解码 ----
        let logits = self
            .decoder
            .prefill(&prompt_embeds, prompt_len + self.max_new_tokens)?;
        let mut token = logits.squeeze(0)?.argmax(0)?.to_scalar::<u32>()?;

        let mut output = Vec::with_capacity(64);
        let mut offset = prompt_len;
        while !self.cfg.eos_token_ids.contains(&token) && output.len() < self.max_new_tokens {
            output.push(token);
            token = self.decoder.step(token, offset)?;
            offset += 1;
        }
        let decode_ms = t_dec.elapsed().as_secs_f64() * 1000.0;

        let parsed = parse_output(&self.tokenizer, &output)?;
        // 强制语种时 prompt 已预填 `language X<asr_text>`，输出里没有语种前缀，
        // 但结果要如实反映调用方指定的语种。
        let language = parsed.language.or_else(|| language.clone());

        Ok(Transcript {
            text: parsed.text,
            language,
            audio_ms,
            encode_ms,
            decode_ms,
            audio_tokens: n_audio,
            text_tokens: output.len(),
            token_ids: output,
        })
    }

    /// 读 WAV（自动混单声道 / 重采样到 16 kHz）并识别。
    pub fn transcribe_wav(&mut self, path: &Path) -> Result<Transcript, InferError> {
        let pcm = audio::load_wav_16k(path, self.front.sample_rate)?;
        self.transcribe(&pcm)
    }

    /// 计算 log-mel（内部会把尾部补零到整秒，保证帧数是 100 的整数倍）。
    pub fn mel(&self, pcm: &[f32]) -> Result<audio::LogMel, InferError> {
        if pcm.is_empty() {
            return Err(InferError::EmptyAudio);
        }
        let padded = pad_to_second(pcm, self.front.sample_rate);
        let mel = log_mel(&padded, &self.front);
        if mel.frames == 0 {
            return Err(InferError::EmptyAudio);
        }
        Ok(mel)
    }

    /// 只跑音频塔：`[tokens, output_dim]`。用于对拍、流式预处理或强制对齐。
    pub fn encode(&self, pcm: &[f32]) -> Result<Tensor, InferError> {
        let valid_frames = self.front.frames(pcm.len());
        self.encode_from(pcm, valid_frames)
    }

    fn encode_from(&self, pcm: &[f32], valid_frames: usize) -> Result<Tensor, InferError> {
        let mel = self.mel(pcm)?;
        let mel_t = Tensor::from_vec(mel.data.clone(), (mel.mel_bins, mel.frames), &self.device)?
            .to_dtype(self.dtype)?;
        self.encoder
            .forward(&mel_t, self.valid_audio_tokens(valid_frames))
    }

    /// 预热：跑一遍 1 秒静音的编码 + prefill，触发 kernel 编译。
    ///
    /// 不生成文本（不进入自回归循环），所以不会在静音上产生幻觉。
    pub fn warmup(&mut self) -> Result<(), InferError> {
        let pcm = vec![0.0f32; self.front.sample_rate as usize];
        let audio_embeds = self.encode(&pcm)?;

        let prompt = build_prompt(
            &self.tokenizer,
            self.cfg.audio_start_token_id,
            self.cfg.audio_end_token_id,
            None,
        )?;
        let prefix = self.embed_ids(&prompt.prefix)?;
        let suffix = self.embed_ids(&prompt.suffix)?;
        let audio = audio_embeds.unsqueeze(0)?;
        let embeds = Tensor::cat(&[&prefix, &audio, &suffix], 1)?;
        let _ = self
            .decoder
            .prefill(&embeds, embeds.dims()[1] + self.max_new_tokens)?;
        self.decoder.reset();
        Ok(())
    }

    fn embed_ids(&self, ids: &[u32]) -> Result<Tensor, InferError> {
        let t = Tensor::from_vec(ids.to_vec(), (1, ids.len()), &self.device)?;
        self.decoder.embed(&t)
    }

    /// 有效 mel 帧数 → 音频 token 数。
    ///
    /// 卷积是**按 chunk 独立**做的，所以不能对总帧数连做三次 stride-2 取整：
    /// 100 帧一个 chunk 固定出 13 个 token，余数再单独过三次卷积。
    fn valid_audio_tokens(&self, valid_frames: usize) -> usize {
        let chunk = self.cfg.audio.chunk_frames();
        let full = valid_frames / chunk;
        let rem = valid_frames % chunk;
        full * self.cfg.audio.tokens_per_chunk() + conv3(rem)
    }
}

/// 三次 stride-2 卷积后的长度。
fn conv3(frames: usize) -> usize {
    config::conv_out_len(config::conv_out_len(config::conv_out_len(frames)))
}

/// 两侧补零到整秒，让 mel 帧数是 100 的整数倍。
fn pad_to_second(pcm: &[f32], sample_rate: u32) -> Vec<f32> {
    let frame = sample_rate as usize;
    let rem = pcm.len() % frame;
    if rem == 0 {
        return pcm.to_vec();
    }
    let mut out = Vec::with_capacity(pcm.len() + frame - rem);
    out.extend_from_slice(pcm);
    out.resize(pcm.len() + frame - rem, 0.0);
    out
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
    fn pads_to_whole_second() {
        assert_eq!(pad_to_second(&[0.0; 16_000], 16_000).len(), 16_000);
        assert_eq!(pad_to_second(&[0.0; 16_001], 16_000).len(), 32_000);
        let p = pad_to_second(&[1.0; 100], 16_000);
        assert_eq!(p.len(), 16_000);
        assert_eq!(p[99], 1.0);
        assert_eq!(p[100], 0.0);
    }

    #[test]
    fn valid_tokens_follow_conv_formula() {
        // 与 transformers `_get_feat_extract_output_lengths` 对齐：
        // 每满 100 帧出 13 个 token，余数单独过三次卷积。
        let tokens = |frames: usize| (frames / 100) * 13 + conv3(frames % 100);
        assert_eq!(tokens(100), 13);
        assert_eq!(tokens(1100), 143);
        assert_eq!(tokens(30), 4);
        assert_eq!(tokens(1053), 10 * 13 + 7);
        assert_eq!(conv3(0), 0);
    }

    #[test]
    fn missing_model_dir_is_reported() {
        let err = collect_shards(Path::new("/nonexistent/xime-asr")).unwrap_err();
        assert!(matches!(err, InferError::MissingFile(_)));
    }
}
