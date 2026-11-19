//! Fun-ASR-Nano-2512 离线语音识别（通义实验室，800M）。
//!
//! 与 [`crate::asr::Qwen3Asr`] 并列的第二个 ASR 家族，管线完全不同：
//!
//! ```text
//! PCM(16 kHz) → Kaldi fbank(80) → LFR(7/6) → 560 维
//!                                             │
//!                              SAN-M 编码器 70 层（整段双向注意力）
//!                                             │
//!                          linear_1 + relu + linear_2 → 2 层 adaptor
//!                                             │
//!   prompt: <|im_start|>system\nYou are a helpful assistant.<|im_end|>
//!           <|im_start|>user\n语音转写成中文：<音频 embedding><|im_end|>
//!           <|im_start|>assistant\n
//!                                             ↓
//!                                 Qwen3-0.6B 解码器（贪心）
//!                                             ↓
//!                                          纯文本
//! ```
//!
//! 与 Qwen3-ASR 的差别（选型时要看这几条）：
//!
//! | | Qwen3-ASR | Fun-ASR-Nano |
//! | --- | --- | --- |
//! | 前处理 | Whisper 式 128 维 log-mel | Kaldi fbank 80 维 + LFR 拼帧 |
//! | 音频编码器 | 18 层窗口注意力（104 token 窗口） | 70 层整段双向注意力 |
//! | 输出 | `language X<asr_text>文本` | 纯文本（语种写进 prompt） |
//! | 流式 | 支持（按 1 秒分块编码） | **不支持**（见下） |
//! | 强项 | 中英日 + 语种判定 | 中文方言/口音覆盖广，热词与上下文提示 |
//!
//! **为什么不支持流式**：LFR 拼帧要跨帧（叠 7 跳 6），分块边界会截断窗口；注意力
//! 又是整段双向的，改一段就全变。要流式只能每个窗口重新编码整段，CPU 上追不上。
//! 所以 [`FunAsrNano`] 只做整段识别，宿主按「录完再识别」处理即可。
//!
//! 使用方式：
//!
//! ```text
//! let mut asr = FunAsrNano::from_dir(Path::new("/path/to/Fun-ASR-Nano-2512-hf"))?;
//! let out = asr.transcribe_wav(Path::new("a.wav"))?;
//! println!("{}", out.text);
//! ```
//!
//! 权重必须是 transformers 布局的 safetensors（`model.audio_tower.*` /
//! `model.multi_modal_projector.*` / `model.language_model.*`）。ModelScope 原始仓库
//! 的 `model.pt` 请先用 `tools/convert_fun_asr_nano.py` 转换。

pub mod config;
pub mod decoder;
pub mod encoder;
pub mod frontend;
pub mod tokenizer;

use std::path::{Path, PathBuf};
use std::time::Instant;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;

pub use config::{AdaptorConfig, AudioEncoderConfig, FrontendConfig, FunAsrConfig};
pub use decoder::Decoder;
pub use tokenizer::{build_prompt, language_name, AsrTokenizer, PromptIds};

use super::{acquire_instance, collect_shards, AsrOptions, InstanceGuard, Transcript};
use crate::InferError;

/// 单次可处理的音频上限（秒）。
///
/// 官方 VAD 切段上限是 30 s，模型也是按这个量级训练的；再长就由宿主切段。
pub const MAX_AUDIO_SECS: f32 = 30.0;

/// 已装载的 Fun-ASR-Nano。
pub struct FunAsrNano {
    cfg: FunAsrConfig,
    encoder: encoder::Encoder,
    decoder: Decoder,
    tokenizer: AsrTokenizer,
    front: frontend::FbankFrontend,
    device: Device,
    dtype: DType,
    max_new_tokens: usize,
    language: Option<String>,
    model_dir: PathBuf,
    _guard: InstanceGuard,
}

impl FunAsrNano {
    /// 用默认选项装载（自动探测后端）。
    pub fn from_dir(dir: &Path) -> Result<Self, InferError> {
        Self::from_dir_with(dir, AsrOptions::default())
    }

    pub fn from_dir_with(dir: &Path, opts: AsrOptions) -> Result<Self, InferError> {
        let guard = acquire_instance()?;
        let cfg = FunAsrConfig::load(dir)?;
        let device = opts.device.build()?;
        let shards = collect_shards(dir)?;

        // SAFETY: 模型文件在识别器存活期间不被修改（宿主的模型目录是只读的）。
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&shards, opts.dtype, &device)
                .map_err(InferError::from)?
        };

        let encoder = encoder::Encoder::load(&vb, &cfg.audio, &cfg.adaptor, &device)?;
        // 文本侧固定是 Qwen3-0.6B，权重前缀与 Qwen3-ASR 不同，所以解码器在自己
        // 目录里带一份固定前缀的实现（见 `decoder.rs` 的模块文档）。
        let decoder = Decoder::load(&vb, &cfg.text, &device, opts.dtype)?;
        let tokenizer = AsrTokenizer::from_dir(dir)?;
        tokenizer.validate_prompt_tokens()?;

        // LFR 叠帧数同时出现在 config.json（audio_config）与 processor_config.json，
        // 不一致会让特征维度对不上权重，装载时就挡下。
        if cfg.frontend.lfr_m != cfg.audio.num_stacked_frames {
            return Err(InferError::Config(format!(
                "LFR 叠帧数不一致：processor_config.json 里是 {}，audio_config 里是 {}",
                cfg.frontend.lfr_m, cfg.audio.num_stacked_frames
            )));
        }

        let front = frontend::FbankFrontend::new(
            cfg.frontend.sample_rate,
            cfg.audio.num_mel_bins,
            cfg.frontend.frame_length_ms,
            cfg.frontend.frame_shift_ms,
            cfg.frontend.lfr_m,
            cfg.frontend.lfr_n,
        )?;
        if front.feature_dim() != encoder.in_dim() {
            return Err(InferError::Config(format!(
                "前处理输出 {} 维，权重期望 {} 维",
                front.feature_dim(),
                encoder.in_dim()
            )));
        }

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

    pub fn config(&self) -> &FunAsrConfig {
        &self.cfg
    }

    /// 改强制语种。接受 `"zh"` / `"Chinese"` / `"中文"` 等写法；`None` = 自动。
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
        let secs = pcm.len() as f32 / self.cfg.frontend.sample_rate as f32;
        if secs > MAX_AUDIO_SECS {
            return Err(InferError::AudioTooLong {
                secs,
                max: MAX_AUDIO_SECS,
            });
        }

        // ---- 前处理 + 编码器 ----
        let t_enc = Instant::now();
        let (feats, frames) = self.front.extract(pcm)?;
        let audio = self.encode_features(feats, frames)?;
        let encode_ms = t_enc.elapsed().as_secs_f64() * 1000.0;

        let mut out = self.decode_audio(&audio, secs as f64 * 1000.0, language)?;
        out.encode_ms = encode_ms;
        Ok(out)
    }

    /// 读 WAV（自动混单声道 / 重采样到模型采样率）并识别。
    pub fn transcribe_wav(&mut self, path: &Path) -> Result<Transcript, InferError> {
        let pcm = crate::asr::audio::load_wav_16k(path, self.cfg.frontend.sample_rate)?;
        self.transcribe(&pcm)
    }

    /// 只跑前处理，返回 `(特征, 帧数, 每帧维度)`。
    ///
    /// 对拍用：与 transformers 的 `FunAsrNanoFeatureExtractor` 输出逐元素比较，
    /// 就能把「前处理错」与「编码器错」分开。
    pub fn fbank(&self, pcm: &[f32]) -> Result<(Vec<f32>, usize, usize), InferError> {
        let (feats, frames) = self.front.extract(pcm)?;
        Ok((feats, frames, self.front.feature_dim()))
    }

    /// 只跑前处理 + 编码器 + 投影，得到可直接拼进文本 embedding 的音频向量。
    ///
    /// 对拍用：形状 `(帧数, 1024)`。
    pub fn encode(&self, pcm: &[f32]) -> Result<Tensor, InferError> {
        let (feats, frames) = self.front.extract(pcm)?;
        self.encode_features(feats, frames)
    }

    /// 当前 prompt 的 token id（`prefix` 之后接音频，再接 `suffix`）。
    pub fn prompt_tokens(
        &self,
        language: Option<&str>,
    ) -> Result<tokenizer::PromptIds, InferError> {
        build_prompt(&self.tokenizer, language)
    }

    /// 前处理 + 编码器 + 投影，得到可直接拼进文本 embedding 的音频向量。
    fn encode_features(&self, feats: Vec<f32>, frames: usize) -> Result<Tensor, InferError> {
        let dim = self.front.feature_dim();
        let t = Tensor::from_vec(feats, (frames, dim), &self.device)?.to_dtype(self.dtype)?;
        self.encoder.forward(&t)
    }

    /// 音频向量 + prompt → 贪心解码。
    fn decode_audio(
        &mut self,
        audio: &Tensor,
        audio_ms: f64,
        language: Option<String>,
    ) -> Result<Transcript, InferError> {
        let n_audio = audio.dims()[0];
        if n_audio == 0 {
            return Err(InferError::EmptyAudio);
        }

        let t_dec = Instant::now();
        let prompt = build_prompt(&self.tokenizer, language.as_deref())?;
        let prefix = self.embed_ids(&prompt.prefix)?;
        let suffix = self.embed_ids(&prompt.suffix)?;
        let audio = audio.unsqueeze(0)?;
        let prompt = Tensor::cat(&[&prefix, &audio, &suffix], 1)?;
        let prompt_len = prompt.dims()[1];

        let logits = self
            .decoder
            .prefill(&prompt, prompt_len + self.max_new_tokens)?;
        let mut token = logits.squeeze(0)?.argmax(0)?.to_scalar::<u32>()?;

        let mut output = Vec::with_capacity(64);
        let mut offset = prompt_len;
        while !self.cfg.eos_token_ids.contains(&token) && output.len() < self.max_new_tokens {
            output.push(token);
            token = self.decoder.step(token, offset)?;
            offset += 1;
        }
        let decode_ms = t_dec.elapsed().as_secs_f64() * 1000.0;

        let text = clean_text(&self.tokenizer.decode(&output)?);
        Ok(Transcript {
            text,
            language: language.map(|l| language_name(&l).to_string()),
            audio_ms,
            encode_ms: 0.0,
            decode_ms,
            audio_tokens: n_audio,
            text_tokens: output.len(),
            token_ids: output,
        })
    }

    /// 预热：跑一遍 1 秒静音的前处理 + 编码 + prefill，触发 kernel 编译。
    ///
    /// 不进入自回归循环，所以不会在静音上产生幻觉。
    pub fn warmup(&mut self) -> Result<(), InferError> {
        let pcm = vec![0.0f32; self.cfg.frontend.sample_rate as usize];
        let (feats, frames) = self.front.extract(&pcm)?;
        let audio = self.encode_features(feats, frames)?;

        let prompt = build_prompt(&self.tokenizer, self.language.as_deref())?;
        let prefix = self.embed_ids(&prompt.prefix)?;
        let suffix = self.embed_ids(&prompt.suffix)?;
        let embeds = Tensor::cat(&[&prefix, &audio.unsqueeze(0)?, &suffix], 1)?;
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
}

/// 输出清理：去空白、还原 `/sil`、剥掉模型偶尔加的英文前缀与引号。
///
/// prompt 与指令句的构造在 [`tokenizer`] 里，这里只管解码出来的原始文本。
fn clean_text(text: &str) -> String {
    let mut s = text.replace("/sil", " ").trim().to_string();
    for prefix in [
        "The spoken content of the audio is",
        "The transcription of the audio is",
        "The content of the input audio is",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim_start().to_string();
            break;
        }
    }
    // 整段被一对引号包住时去掉引号（只处理完整闭合的情况）
    let quoted = s.len() >= 2
        && (s.starts_with('"') && s.ends_with('"') || s.starts_with('\'') && s.ends_with('\''));
    if quoted {
        s = s[1..s.len() - 1].trim().to_string();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_strips_prefix_and_quotes() {
        assert_eq!(clean_text(" 你好 "), "你好");
        assert_eq!(clean_text("a/silb"), "a b");
        assert_eq!(
            clean_text("The transcription of the audio is \"你好。\""),
            "你好。"
        );
        // 只有一侧有引号时不动
        assert_eq!(clean_text("\"你好"), "\"你好");
        assert_eq!(clean_text(""), "");
    }
}
