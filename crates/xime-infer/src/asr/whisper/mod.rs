//! Whisper 离线语音识别（OpenAI 原始模型，现已由社区接管维护）。
//!
//! 这是本 crate 的第三个 ASR 家族，与 [`crate::asr::qwen3_asr`] /
//! [`crate::asr::fun_asr`] 零共享实现——Whisper 是**编码器-解码器**结构，
//! 前处理、prompt、解码循环都与前两族不同：
//!
//! ```text
//! PCM(16 kHz) → log-mel(80 或 128) 补满 30 s → [mel, 3000]
//!   → 编码器（2 层卷积 stem + 1500 位置；12/32 层双向 transformer）
//!   → 交叉注意力的 K/V
//!
//! prompt: <|startoftranscript|> <|语种|> <|transcribe|> <|notimestamps|>
//!   → 解码器（学习位置 + 因果自注意力 + 交叉注意力；4/12 层，贪心解码）
//!   → 纯文本，遇 <|endoftext|> 停
//! ```
//!
//! 三代模型走同一份代码，超参全部来自 `config.json` + `generation_config.json`
//! （本机实测过 small 与 large-v3-turbo）：
//!
//! | | small | large-v3-turbo |
//! | --- | --- | --- |
//! | 参数 | 244M | 809M |
//! | mel / 编码器 / 解码器层 | 80 / 12 / 12 | 128 / 32 / 4 |
//! | 语种 | 99 | 100（多 `<|yue|>`） |
//!
//! 使用方式：
//!
//! ```text
//! let mut asr = Whisper::from_dir(Path::new("/path/to/whisper-small"))?;
//! asr.set_language(Some("zh".into()));       // None = 自动判语种
//! let out = asr.transcribe_wav(Path::new("a.wav"))?;
//! println!("[{}] {}", out.language.unwrap_or_default(), out.text);
//! ```
//!
//! 与另两族不同，[`Transcript::language`] 这里是 whisper 的 **ISO 代码**（`zh` /
//! `en` / `yue` …），因为 prompt 里用的就是 `<|zh|>` 这种 token；Qwen3-ASR 给的是
//! 模型自己写出的英文名（`Chinese`）。
//!
//! 与另两族的共同约束：
//!
//! - 单次 ≤ 30 秒，更长的音频由宿主切段；
//! - 识别器持有 KV cache，`transcribe` 需要 `&mut self`，宿主用单线程 worker 独占；
//! - 同一进程只能装载一个 ASR 实例（[`crate::asr::AsrFamily`] 那道闸门）；
//! - 首次调用要预热：whisper 的编码器固定吃满 30 s，`warmup()` 跑一遍静音编码
//!   加一次 prefill，别放在按键路径上。
//!
//! 不做的事：流式增量输出（交叉注意力要整段音频，改一个窗口就得全量重算）、
//! 时间戳（`<|notimestamps|>` 压掉了）、说话人分离。要流式请用 Qwen3-ASR。

pub mod config;
pub mod decoder;
pub mod encoder;
pub mod frontend;
pub mod tokenizer;

use std::path::{Path, PathBuf};
use std::time::Instant;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;

pub use config::{Activation, GenerationConfig, WhisperConfig};
use encoder::Encoder;
use frontend::AudioFrontend;
use tokenizer::{parse_text, SpecialTokens, WhisperTokenizer};

use super::{acquire_instance, collect_shards, AsrOptions, InstanceGuard, Transcript};
use crate::InferError;

/// 单次可处理的音频上限（秒）。whisper 训练窗口就是 30 s。
pub const MAX_AUDIO_SECS: f32 = 30.0;

/// 默认解码 task。
pub const TASK_TRANSCRIBE: &str = "transcribe";
/// 翻译成英文的 task。
pub const TASK_TRANSLATE: &str = "translate";

/// 已装载的 Whisper。
///
/// 注意：`large-v3-turbo` 的权重是 f16（1.6 GB），按默认 `AsrOptions::dtype = f32`
/// 装载会展开成约 3.2 GB；显存/内存紧的话显式传 `DType::F16` + GPU 后端。
pub struct Whisper {
    cfg: WhisperConfig,
    special: SpecialTokens,
    encoder: Encoder,
    decoder: decoder::Decoder,
    tokenizer: WhisperTokenizer,
    front: AudioFrontend,
    device: Device,
    dtype: DType,
    max_new_tokens: usize,
    language: Option<String>,
    task: String,
    /// 上一次 `detect_language_from` 是否把 `[sot]` 留在了 decoder cache 里。
    sot_prefilled: bool,
    model_dir: PathBuf,
    _guard: InstanceGuard,
}

impl Whisper {
    /// 用默认选项装载（自动探测后端）。
    pub fn from_dir(dir: &Path) -> Result<Self, InferError> {
        Self::from_dir_with(dir, AsrOptions::default())
    }

    pub fn from_dir_with(dir: &Path, opts: AsrOptions) -> Result<Self, InferError> {
        let guard = acquire_instance()?;
        let cfg = WhisperConfig::load(dir)?;
        let device = opts.device.build()?;
        let shards = collect_shards(dir)?;

        // SAFETY: 模型文件在识别器存活期间不被修改（宿主的模型目录是只读的）。
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&shards, opts.dtype, &device)
                .map_err(InferError::from)?
        };

        let encoder = Encoder::load(&vb, &cfg)?;
        let decoder = decoder::Decoder::load(&vb, &cfg)?;
        let tokenizer = WhisperTokenizer::from_dir(dir)?;
        let special = SpecialTokens::resolve(&tokenizer, &cfg)?;

        // 多语种模型必须有语种表，否则解码 prompt 少了语种段，输出会串语言。
        if cfg.is_multilingual() && !special.is_multilingual() {
            return Err(InferError::Config(
                "config 说是多语种模型，但分词器里找不到 <|xx|> 语种 token".into(),
            ));
        }
        if cfg.generation.suppress_tokens.contains(&special.eot) {
            return Err(InferError::Config(
                "suppress_tokens 里含 <|endoftext|>，模型将永远停不下来".into(),
            ));
        }

        let front = AudioFrontend::new(cfg.num_mel_bins);

        Ok(Self {
            cfg,
            special,
            encoder,
            decoder,
            tokenizer,
            front,
            device,
            dtype: opts.dtype,
            max_new_tokens: opts.max_new_tokens.max(1),
            language: opts.language,
            task: TASK_TRANSCRIBE.to_string(),
            sot_prefilled: false,
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

    pub fn config(&self) -> &WhisperConfig {
        &self.cfg
    }

    pub fn special_tokens(&self) -> &SpecialTokens {
        &self.special
    }

    /// 前处理参数（mel 滤波器组、帧长等，对拍/测试用）。
    pub fn mel_frontend(&self) -> &AudioFrontend {
        &self.front
    }

    /// 改强制语种。接受 `zh` / `<|zh|>` / `Chinese` / `中文`；`None` 或 `auto`
    /// 表示让模型自己判（多语种模型先跑一次语种检测）。
    pub fn set_language(&mut self, language: Option<String>) {
        self.language = language;
    }

    /// 当前设置的语种（`None` = 自动）。
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// 当前 task（`transcribe` / `translate`）。
    pub fn task(&self) -> &str {
        &self.task
    }

    /// 改 task：`transcribe`（默认）或 `translate`（译成英文）。
    pub fn set_task(&mut self, task: &str) -> Result<(), InferError> {
        match task {
            TASK_TRANSCRIBE | TASK_TRANSLATE => {
                self.task = task.to_string();
                Ok(())
            }
            other => Err(InferError::Config(format!(
                "未知 task `{other}`（可用 transcribe / translate）"
            ))),
        }
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
        let secs = validate_audio(pcm)?;

        // ---- 编码（固定 30 s 窗口）----
        let t_enc = Instant::now();
        let enc = self.encode(pcm)?;
        let encode_ms = t_enc.elapsed().as_secs_f64() * 1000.0;

        // ---- 语种 ----
        let language = self.resolve_language(language.as_deref(), &enc)?;
        let lang_token = language
            .as_deref()
            .and_then(|code| self.special.language_token(code));

        // ---- prompt + 贪心解码 ----
        let t_dec = Instant::now();
        let task_token = self.special.task_token(&self.task);
        let prompt = self.special.prompt(lang_token, task_token, true);
        // 自动判语种时 decoder 里已经预填了 `[<|startoftranscript|>]`（见
        // `detect_language_from`），正式 prompt 只续上剩下的三段，省掉一遍
        // 1500 个位置的交叉注意力 K/V 投影。
        let logits = if self.sot_prefilled && prompt.len() > 1 {
            self.decoder.continue_prefill(&prompt[1..])?
        } else {
            self.decoder.prefill(&enc, &prompt)?
        };
        self.sot_prefilled = false;
        let ids = self.generate(logits, prompt.len())?;
        let decode_ms = t_dec.elapsed().as_secs_f64() * 1000.0;

        let text = parse_text(&self.tokenizer, &ids)?;
        let audio_tokens = enc.dims()[1];

        Ok(Transcript {
            text,
            language,
            audio_ms: secs as f64 * 1000.0,
            encode_ms,
            decode_ms,
            audio_tokens,
            text_tokens: ids.len(),
            token_ids: ids,
        })
    }

    /// 读 WAV（自动混单声道 / 重采样到 16 kHz）并识别。
    pub fn transcribe_wav(&mut self, path: &Path) -> Result<Transcript, InferError> {
        let pcm = crate::asr::audio::load_wav_16k(path, frontend::SAMPLE_RATE as u32)?;
        self.transcribe(&pcm)
    }

    /// 只判语种（多语种模型）。单语模型返回 `None`。
    pub fn detect_language(&mut self, pcm: &[f32]) -> Result<Option<String>, InferError> {
        validate_audio(pcm)?;
        let enc = self.encode(pcm)?;
        // 公开入口是一次独立查询，用完就把 cache 清掉，避免影响后续 transcribe。
        let lang = self.detect_language_from(&enc, false)?;
        Ok(lang)
    }

    /// 计算 log-mel（`[mel_bins, 3000]`，自动补满 30 s）。
    pub fn mel(&self, pcm: &[f32]) -> Result<frontend::LogMel, InferError> {
        validate_audio(pcm)?;
        let mel = self.front.log_mel(pcm);
        if mel.frames == 0 {
            return Err(InferError::EmptyAudio);
        }
        Ok(mel)
    }

    /// 只跑编码器：`[1, 1500, d_model]`。用于对拍、特征复用或强制对齐。
    pub fn encode(&self, pcm: &[f32]) -> Result<Tensor, InferError> {
        validate_audio(pcm)?;
        let mel = self.mel(pcm)?;
        let mel_t = Tensor::from_vec(mel.data, (mel.mel_bins, mel.frames), &self.device)?
            .to_dtype(self.dtype)?;
        self.encoder.forward(&mel_t)
    }

    /// 预热：跑一遍 1 秒静音的编码 + prompt prefill，触发 kernel 编译。
    ///
    /// 不进入生成循环（不会在静音上产生幻觉），但**编码器仍然吃满 30 s**：
    /// 这就是 whisper 的固定成本，短音频也一样。
    pub fn warmup(&mut self) -> Result<(), InferError> {
        self.sot_prefilled = false;
        let pcm = vec![0.0f32; frontend::SAMPLE_RATE];
        let enc = self.encode(&pcm)?;
        let prompt = self.special.prompt(None, None, true);
        let _ = self.decoder.prefill(&enc, &prompt)?;
        self.decoder.reset();
        Ok(())
    }

    /// 构造本次识别会用的 prompt token（对拍/调试用；不影响识别流程）。
    ///
    /// `language` 的写法与 [`Self::set_language`] 一致；`None` 表示单语模型不写
    /// 语种段。多语种模型要拿真实 prompt 请先 [`Self::detect_language`]。
    pub fn prompt_tokens(
        &self,
        language: Option<&str>,
        task: &str,
    ) -> Result<Vec<u32>, InferError> {
        let lang_token = match language {
            Some(raw) if self.special.is_multilingual() => {
                let code = self.cfg.generation.resolve_language(raw).ok_or_else(|| {
                    InferError::Config(format!("语种 `{raw}` 不在模型的语种表里"))
                })?;
                self.special.language_token(&code)
            }
            _ => None,
        };
        let task_token = self.special.task_token(task);
        Ok(self.special.prompt(lang_token, task_token, true))
    }

    /// 解析并校验语种：`Some("auto")` / `None` → 自动检测；非法语种直接报错。
    ///
    /// 副作用：自动检测会预填 decoder 并把 `sot_prefilled` 置真；强制语种路径
    /// 保证它是假（不留下上一次的残留）。
    fn resolve_language(
        &mut self,
        language: Option<&str>,
        enc: &Tensor,
    ) -> Result<Option<String>, InferError> {
        self.sot_prefilled = false;
        let Some(raw) = language.map(str::trim).filter(|s| !s.is_empty()) else {
            return self.detect_language_from(enc, true);
        };
        if matches!(
            raw.to_ascii_lowercase().as_str(),
            "auto" | "none" | "自动" | "自动检测"
        ) {
            return self.detect_language_from(enc, true);
        }

        // 单语模型没有语种段，指定语种没有意义；按「模型本来就只会这一种」处理。
        if !self.special.is_multilingual() {
            return Ok(None);
        }
        let code = self.cfg.generation.resolve_language(raw).ok_or_else(|| {
            InferError::Config(format!(
                "语种 `{raw}` 不在模型的语种表里（支持 {} 种，例如 zh / en / ja）",
                self.cfg.generation.lang_to_id.len()
            ))
        })?;
        Ok(Some(code))
    }

    /// 用**已编码**的音频判语种：`<|startoftranscript|>` 位置在语种 token 上的
    /// argmax（与 transformers `detect_language` 同一做法）。
    /// `keep_cache` 为真时把 `[sot]` 的预填状态留在 decoder 里，供正式 prompt 续写；
    /// 为假时用完即清（公开的 [`Self::detect_language`] 走这条）。
    fn detect_language_from(
        &mut self,
        enc: &Tensor,
        keep_cache: bool,
    ) -> Result<Option<String>, InferError> {
        if !self.special.is_multilingual() {
            return Ok(None);
        }
        let prompt = [self.special.sot];
        let logits = self.decoder.prefill(enc, &prompt)?;
        let vals = logits.squeeze(0)?.to_vec1::<f32>()?;
        if keep_cache {
            self.sot_prefilled = true;
        } else {
            self.decoder.reset();
        }

        let best = self
            .special
            .languages
            .iter()
            .filter_map(|(code, id)| vals.get(*id as usize).map(|v| (code, *v)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(best.map(|(code, _)| code.clone()))
    }

    /// 贪心解码（带 whisper 的抑制表）。`logits` 是 prompt 预填后的第一格分布，
    /// `prompt_len` 用来算硬上限与 KV cache 的位置。
    fn generate(&mut self, mut logits: Tensor, prompt_len: usize) -> Result<Vec<u32>, InferError> {
        let vocab = self.cfg.vocab_size;
        let hard_limit = self
            .cfg
            .max_target_positions
            .saturating_sub(prompt_len)
            .saturating_sub(1);
        let limit = self.max_new_tokens.min(hard_limit);
        if limit == 0 {
            return Ok(Vec::new());
        }

        // 每步都压：generation_config 的 suppress_tokens + 全部时间戳 token。
        let timestamps = self.special.timestamp_range(vocab);
        let mut always: Vec<u32> = self
            .cfg
            .generation
            .suppress_tokens
            .iter()
            .copied()
            .filter(|id| (*id as usize) < vocab)
            .collect();
        always.extend(timestamps.clone().filter(|id| (*id as usize) < vocab));
        // `<|notimestamps|>` 本身也压掉：它在 prompt 里已经出现过一次，官方
        // `ApplyTimestampRules` 在 `without_timestamps` 时同样把它设为 -inf。
        if let Some(nots) = self.special.notimestamps {
            always.push(nots);
        }
        always.sort_unstable();
        always.dedup();
        // 只有第一步压（whisper 的 begin_suppress：空白 token 与 eot 的形态）。
        let begin: Vec<u32> = self
            .cfg
            .generation
            .begin_suppress_tokens
            .iter()
            .copied()
            .filter(|id| (*id as usize) < vocab)
            .collect();
        let eot = self.special.eot;

        let mut offset = prompt_len;
        let mut out = Vec::new();
        let mut first = true;

        loop {
            let mut vals = logits.squeeze(0)?.to_vec1::<f32>()?;
            for id in &always {
                vals[*id as usize] = f32::NEG_INFINITY;
            }
            if first {
                // 与 transformers 的 `SuppressTokensAtBeginLogitsProcessor` 一致：
                // 原样压掉，不特殊照顾 eot（begin 表里本来就可能含 eot）。
                for id in &begin {
                    vals[*id as usize] = f32::NEG_INFINITY;
                }
            }
            let token = argmax(&vals);
            if token == eot || out.len() >= limit {
                break;
            }
            out.push(token);
            if out.len() >= limit {
                break;
            }
            logits = self.decoder.step(token, offset)?;
            offset += 1;
            first = false;
        }
        Ok(out)
    }
}

/// 音频合法性检查，返回时长（秒）。30 秒是 whisper 的训练窗口，超了必须由宿主切段；
/// 这里**不静默截断**——截断会让宿主以为整段识别完了。
fn validate_audio(pcm: &[f32]) -> Result<f32, InferError> {
    if pcm.is_empty() {
        return Err(InferError::EmptyAudio);
    }
    let secs = pcm.len() as f32 / frontend::SAMPLE_RATE as f32;
    if secs > MAX_AUDIO_SECS {
        return Err(InferError::AudioTooLong {
            secs,
            max: MAX_AUDIO_SECS,
        });
    }
    Ok(secs)
}

/// 取最大值的下标；`NaN` 视为最小。全 `-inf` 时返回 0（不会发生，兜底用）。
fn argmax(values: &[f32]) -> u32 {
    let mut best = 0usize;
    let mut best_v = f32::NEG_INFINITY;
    for (i, v) in values.iter().enumerate() {
        if *v > best_v {
            best_v = *v;
            best = i;
        }
    }
    best as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_audio_bounds() {
        assert!(matches!(
            validate_audio(&[]).unwrap_err(),
            InferError::EmptyAudio
        ));
        assert!(validate_audio(&[0.0; 16_000]).is_ok());
        let over = vec![0.0f32; (MAX_AUDIO_SECS as usize) * 16_000 + 1];
        assert!(matches!(
            validate_audio(&over).unwrap_err(),
            InferError::AudioTooLong { .. }
        ));
        let exact = vec![0.0f32; (MAX_AUDIO_SECS as usize) * 16_000];
        assert!(validate_audio(&exact).is_ok());
    }

    #[test]
    fn argmax_picks_max_and_ignores_nan() {
        assert_eq!(argmax(&[0.1, 0.5, 0.2]), 1);
        assert_eq!(argmax(&[f32::NAN, 0.1, 0.2]), 2);
        assert_eq!(argmax(&[f32::NEG_INFINITY, 0.0, 0.1]), 2);
        assert_eq!(argmax(&[f32::NEG_INFINITY; 3]), 0);
    }
}
