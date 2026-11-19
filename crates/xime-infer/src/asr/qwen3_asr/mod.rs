//! Qwen3-ASR-0.6B / 1.7B 离线语音识别（与 [`crate::asr::fun_asr`] 并列的另一族）。
//!
//! 本目录自成一体：配置、音频塔、文本解码器、分词器、mel 前处理都在这里，
//! 不与其他模型族共享实现——两族的前处理与 prompt 约定差异太大，共享只会互相牵制。
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

pub mod config;
pub mod decoder;
pub mod encoder;
pub mod frontend;
pub mod tokenizer;

use std::path::{Path, PathBuf};
use std::time::Instant;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;

pub use config::{AsrConfig, AudioTowerConfig, TextConfig, WeightLayout};
pub use decoder::Decoder;
pub use encoder::Encoder;
use frontend::{log_mel, AudioFrontend};
use tokenizer::{build_prompt, build_prompt_with_context, parse_output, AsrTokenizer};

use super::{acquire_instance, collect_shards, AsrOptions, InstanceGuard, Transcript};
use crate::InferError;

/// 单次可处理的音频上限（秒）。
///
/// 音频塔按 1 秒分块、注意力窗口 8 秒，30 秒是训练时的主力窗口；再长由宿主切段。
pub const MAX_AUDIO_SECS: f32 = 30.0;

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
        let guard = acquire_instance()?;
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
        let encode_ms = t_enc.elapsed().as_secs_f64() * 1000.0;

        let mut out = self.decode_audio_embeds(&audio_embeds, None, audio_ms, language)?;
        out.encode_ms = encode_ms;
        Ok(out)
    }

    /// 用**已有的**音频特征解码，不碰音频塔。
    ///
    /// 流式识别的核心入口：录音中每秒把新音频过一遍音频塔，把特征累积起来，
    /// 然后反复调用本方法重解码。音频塔按 chunk（1 秒）独立做卷积、位置编码
    /// 逐 chunk 复用同一张表，所以「逐秒编码再拼接」与「整段一次编码」等价；
    /// 重解码只需要付解码器的钱，编码是增量付的。
    ///
    /// - `audio`：`[tokens, d_model]`，来自 [`Self::encode`] 或分块拼接；
    /// - `context`：上一段已定稿文本，注入 prompt 的 system 段（见
    ///   [`build_prompt_with_context`]）；流式窗口滑动的接缝靠它衔接；
    /// - `audio_ms`：本次窗口对应的音频时长，仅用于回填 [`Transcript`]。
    ///
    /// 返回值的 `encode_ms` 恒为 0（本方法不编码），耗时看 `decode_ms`。
    pub fn decode_embeds(
        &mut self,
        audio: &Tensor,
        context: Option<&str>,
        audio_ms: f64,
    ) -> Result<Transcript, InferError> {
        let language = self.language.clone();
        self.decode_audio_embeds(audio, context, audio_ms, language)
    }

    fn decode_audio_embeds(
        &mut self,
        audio: &Tensor,
        context: Option<&str>,
        audio_ms: f64,
        language: Option<String>,
    ) -> Result<Transcript, InferError> {
        let (n_audio, _) = audio.dims2().map_err(|_| {
            InferError::Config(format!(
                "音频特征应为 [tokens, d_model]，实际 {:?}",
                audio.dims()
            ))
        })?;
        if n_audio == 0 {
            return Err(InferError::EmptyAudio);
        }

        // ---- 拼 prompt ----
        let t_dec = Instant::now();
        let prompt = build_prompt_with_context(
            &self.tokenizer,
            self.cfg.audio_start_token_id,
            self.cfg.audio_end_token_id,
            language.as_deref(),
            context,
        )?;
        let prefix = self.embed_ids(&prompt.prefix)?;
        let suffix = self.embed_ids(&prompt.suffix)?;
        let audio = audio.unsqueeze(0)?;
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
            encode_ms: 0.0,
            decode_ms,
            audio_tokens: n_audio,
            text_tokens: output.len(),
            token_ids: output,
        })
    }

    /// 读 WAV（自动混单声道 / 重采样到 16 kHz）并识别。
    pub fn transcribe_wav(&mut self, path: &Path) -> Result<Transcript, InferError> {
        let pcm = crate::asr::audio::load_wav_16k(path, self.front.sample_rate)?;
        self.transcribe(&pcm)
    }

    /// 计算 log-mel（内部会把尾部补零到整秒，保证帧数是 100 的整数倍）。
    pub fn mel(&self, pcm: &[f32]) -> Result<frontend::LogMel, InferError> {
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
}
