//! 流式识别器封装（sherpa-onnx `OnlineRecognizer`，transducer 系模型）。
//!
//! 语义对齐 Android xime speech 的 `AsrNative`：
//! `create(encoder, decoder, joiner, tokens) → acceptPcm → getPartial → finalize`。
//! 识别器与流对象归调用方 worker 线程独占（本类型非 `Send` 语义使用，不跨线程共享）。

use std::path::Path;

use sherpa_onnx::{
    OnlineModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineStream,
    OnlineTransducerModelConfig,
};

use crate::registry::AsrModelProfile;

/// 执行后端。`Cuda` 变体只在启用 `cuda` feature 的分包里存在——CPU 包编译期
/// 就无法表达 GPU（误用被编译器挡下）；CUDA 包里仍可显式选 `Cpu`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpeechProvider {
    /// CPU EP（默认；int8 流式 zipformer 单线程即可实时）。
    #[default]
    Cpu,
    /// NVIDIA GPU（CUDA EP）。
    #[cfg(feature = "cuda")]
    Cuda,
}

impl SpeechProvider {
    /// sherpa-onnx C API 的 provider 字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            #[cfg(feature = "cuda")]
            Self::Cuda => "cuda",
        }
    }
}

/// 推理配置（不含模型路径——模型目录由宿主解析）。
#[derive(Clone, Debug)]
pub struct SpeechConfig {
    /// 推理线程数（CPU EP 的 intra-op 并行）。
    pub num_threads: i32,
    /// 执行后端。
    pub provider: SpeechProvider,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        // 桌面端 2 线程：int8 zipformer 单线程已实时，2 线程留突发余量
        Self {
            num_threads: 2,
            provider: SpeechProvider::Cpu,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    /// 模型目录里缺角色文件（多半是没下载/解压不完整）。
    #[error("模型文件缺失：{path}（请先在设置中下载语音模型）")]
    MissingModel { path: String },
    /// sherpa 创建识别器失败：文件损坏，或 CUDA 依赖缺失。
    #[error("识别器创建失败（provider = {provider}）：模型文件损坏或 GPU 运行库缺失")]
    CreateFailed { provider: &'static str },
}

/// 一路流式识别会话：装载模型后喂 pcm16 音频块，边说边出 partial，
/// `finalize` 取最终文本；一次会话可跨多句（端点检测 + `reset` 分句）。
pub struct StreamingRecognizer {
    recognizer: OnlineRecognizer,
    stream: OnlineStream,
    /// pcm16 → f32 的复用缓冲（每帧 ~1600 样本，避免逐帧分配）。
    buf: Vec<f32>,
}

impl StreamingRecognizer {
    /// 从模型目录装载识别器。文件角色由 `profile` 给出；`model_dir` 是宿主
    /// 解析出的绝对目录（如 `%APPDATA%\Xime\models\<id>`）。
    pub fn open(
        profile: &AsrModelProfile,
        model_dir: &Path,
        config: &SpeechConfig,
    ) -> Result<Self, SpeechError> {
        for file in [
            &profile.encoder_file,
            &profile.decoder_file,
            &profile.joiner_file,
            &profile.tokens_file,
        ] {
            let path = model_dir.join(file);
            if !path.is_file() {
                return Err(SpeechError::MissingModel {
                    path: path.to_string_lossy().into_owned(),
                });
            }
        }

        let c = build_recognizer_config(profile, model_dir, config);
        let recognizer = OnlineRecognizer::create(&c).ok_or(SpeechError::CreateFailed {
            provider: config.provider.as_str(),
        })?;
        let stream = recognizer.create_stream();
        Ok(Self {
            recognizer,
            stream,
            buf: Vec::new(),
        })
    }

    /// 喂入一帧 PCM（16bit 单声道）。`sample_rate` 原样上交——sherpa 内部
    /// 会重采样到模型要求的 16k（c-api.h：sample_rate differs → resamples
    /// internally），宿主不必预重采样。
    pub fn accept_pcm16(&mut self, sample_rate: i32, samples: &[i16]) {
        self.buf.clear();
        self.buf.extend(samples.iter().map(|&s| s as f32 / 32768.0));
        self.stream.accept_waveform(sample_rate, &self.buf);
        self.drain();
    }

    /// 喂入一帧 PCM（32bit 浮点 [-1, 1]，单声道）。WASAPI 采集的原生格式
    /// 就是 f32，走这条路径省一次量化、也不丢精度。
    pub fn accept_pcm32f(&mut self, sample_rate: i32, samples: &[f32]) {
        self.stream.accept_waveform(sample_rate, samples);
        self.drain();
    }

    /// 当前（部分）识别文本；还没有输出时为空串。
    pub fn partial_text(&self) -> String {
        self.recognizer
            .get_result(&self.stream)
            .map(|r| r.text)
            .unwrap_or_default()
    }

    /// 结束一句：冲刷解码取最终文本，并换新流（之后可继续喂下一句）。
    /// Android `nativeFinalize` 的对应物。
    pub fn finalize(&mut self) -> String {
        self.stream.input_finished();
        self.drain();
        let text = self.partial_text();
        self.stream = self.recognizer.create_stream();
        text
    }

    /// 丢弃未定文本、换新流（Android `nativeReset` 对应物）。
    pub fn reset(&mut self) {
        self.stream = self.recognizer.create_stream();
    }

    /// 端点检测：尾部静音到达阈值时为真（宿主可据此把 partial 作为一句
    /// 结果发出并 `reset`）。仅在 `accept_pcm16` 之后调用才有意义。
    pub fn is_endpoint(&self) -> bool {
        self.recognizer.is_endpoint(&self.stream)
    }

    /// 冲刷可解码帧（喂音频后调用一次，结果随即可查）。
    fn drain(&mut self) {
        while self.recognizer.is_ready(&self.stream) {
            self.recognizer.decode(&self.stream);
        }
    }
}

/// 组装 sherpa 的识别器配置（独立出来便于单测钉住映射关系）。
fn build_recognizer_config(
    profile: &AsrModelProfile,
    model_dir: &Path,
    config: &SpeechConfig,
) -> OnlineRecognizerConfig {
    let abs = |file: &str| -> String { model_dir.join(file).to_string_lossy().into_owned() };
    let mut c = OnlineRecognizerConfig::default();
    c.model_config = OnlineModelConfig {
        transducer: OnlineTransducerModelConfig {
            encoder: Some(abs(&profile.encoder_file)),
            decoder: Some(abs(&profile.decoder_file)),
            joiner: Some(abs(&profile.joiner_file)),
        },
        tokens: Some(abs(&profile.tokens_file)),
        num_threads: config.num_threads,
        provider: Some(config.provider.as_str().to_string()),
        ..OnlineModelConfig::default()
    };
    // 贪心解码：延迟最低；热词/beam search 留给后续按需升级
    c.decoding_method = Some("greedy_search".to_string());
    // 端点检测（静音断句）：rule1 长句停顿、rule2 快速断句、rule3 语句时长上限。
    // 取 sherpa 官方默认值，显式写死便于理解与调参。
    c.enable_endpoint = true;
    c.rule1_min_trailing_silence = 2.4;
    c.rule2_min_trailing_silence = 1.2;
    c.rule3_min_utterance_length = 20.0;
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::AsrModelRegistry;

    #[test]
    fn sherpa_links_and_reports_version() {
        // 链接验证：shared 模式下 DLL 应已被 build 脚本放到测试二进制旁
        let version = sherpa_onnx::version();
        assert!(!version.is_empty(), "sherpa-onnx 版本号不应为空");
    }

    #[test]
    fn provider_strings_match_c_api() {
        assert_eq!(SpeechProvider::Cpu.as_str(), "cpu");
        #[cfg(feature = "cuda")]
        assert_eq!(SpeechProvider::Cuda.as_str(), "cuda");
        // 与包型自述一致：CPU 包不带 cuda 变体
        assert_eq!(crate::cuda_supported(), cfg!(feature = "cuda"));
    }

    #[test]
    fn recognizer_config_maps_profile_files_and_provider() {
        // Windows 上 Path::join 用反斜杠，断言前归一成正斜杠
        let slash = |s: Option<String>| s.map(|x| x.replace('\\', "/"));
        let profile = AsrModelRegistry::default_profile();
        let dir = Path::new("D:/models/x");
        let config = SpeechConfig {
            num_threads: 3,
            provider: SpeechProvider::Cpu,
        };
        let c = build_recognizer_config(&profile, dir, &config);
        assert_eq!(
            slash(c.model_config.transducer.encoder),
            Some("D:/models/x/encoder.int8.onnx".into())
        );
        assert_eq!(
            slash(c.model_config.transducer.decoder),
            Some("D:/models/x/decoder.onnx".into())
        );
        assert_eq!(
            slash(c.model_config.transducer.joiner),
            Some("D:/models/x/joiner.int8.onnx".into())
        );
        assert_eq!(
            slash(c.model_config.tokens),
            Some("D:/models/x/tokens.txt".into())
        );
        assert_eq!(c.model_config.num_threads, 3);
        assert_eq!(c.model_config.provider, Some("cpu".into()));
        assert_eq!(c.decoding_method, Some("greedy_search".into()));
        assert!(c.enable_endpoint);
        assert!((c.rule2_min_trailing_silence - 1.2).abs() < f32::EPSILON);
    }

    #[cfg(feature = "cuda")]
    #[test]
    fn recognizer_config_passes_cuda_provider() {
        let profile = AsrModelRegistry::default_profile();
        let config = SpeechConfig {
            num_threads: 2,
            provider: SpeechProvider::Cuda,
        };
        let c = build_recognizer_config(&profile, Path::new("D:/models/x"), &config);
        assert_eq!(c.model_config.provider, Some("cuda".into()));
    }

    #[test]
    fn open_fails_cleanly_when_model_files_missing() {
        let profile = AsrModelRegistry::default_profile();
        let empty = std::env::temp_dir().join("xime-speech-missing-model");
        let err = StreamingRecognizer::open(&profile, &empty, &SpeechConfig::default());
        match err {
            Err(SpeechError::MissingModel { path }) => {
                assert!(path.contains("encoder.int8.onnx"), "应点名缺失文件：{path}");
            }
            Err(other) => panic!("应报 MissingModel，实际 {other}"),
            Ok(_) => panic!("空目录不应装载成功"),
        }
    }

    #[test]
    fn open_real_model_when_present() {
        // 端到端装载冒烟：模型已下载到用户数据目录才执行（否则静默跳过）。
        // 覆盖：真实模型装载 + 喂 300ms 静音 + finalize 出空文本不炸。
        let root = std::env::var("APPDATA")
            .map(|appdata| Path::new(&appdata).join("Xime").join("models"))
            .ok();
        let model_dir = root.map(|r| r.join("x-asr-480ms-zh-en-punct-int8"));
        let Some(model_dir) = model_dir else { return };
        if !model_dir.is_dir() {
            return; // 模型尚未下载
        }
        let profile = AsrModelRegistry::default_profile();
        let mut recognizer =
            match StreamingRecognizer::open(&profile, &model_dir, &SpeechConfig::default()) {
                Ok(r) => r,
                Err(e) => panic!("真实模型装载失败：{e}"),
            };
        recognizer.accept_pcm16(16000, &vec![0i16; 4800]);
        let _ = recognizer.finalize();
        // 48k 浮点输入（WASAPI 混音格式的典型采样率）：验证 sherpa 内部重采样路径
        recognizer.accept_pcm32f(48000, &vec![0.0f32; 14400]);
        let _ = recognizer.finalize();
    }

    /// CUDA 端到端冒烟（仅 CUDA 分包）：真实模型 + `SpeechProvider::Cuda`
    /// 在 onnxruntime 上建 CUDA EP 会话并跑一帧。会话创建失败（无 NVIDIA
    /// GPU / 驱动不匹配）时打印原因并跳过——本机有卡时它必须真跑过。
    #[cfg(feature = "cuda")]
    #[test]
    fn open_real_model_cuda_when_present() {
        let root = std::env::var("APPDATA")
            .map(|appdata| Path::new(&appdata).join("Xime").join("models"))
            .ok();
        let model_dir = root.map(|r| r.join("x-asr-480ms-zh-en-punct-int8"));
        let Some(model_dir) = model_dir else { return };
        if !model_dir.is_dir() {
            return; // 模型尚未下载
        }
        let profile = AsrModelRegistry::default_profile();
        let config = SpeechConfig {
            num_threads: 2,
            provider: SpeechProvider::Cuda,
        };
        let mut recognizer = match StreamingRecognizer::open(&profile, &model_dir, &config) {
            Ok(r) => r,
            Err(SpeechError::CreateFailed { .. }) => {
                eprintln!("跳过：CUDA 会话创建失败（本机无 NVIDIA GPU 或驱动不匹配）");
                return;
            }
            Err(e) => panic!("CUDA 装载失败：{e}"),
        };
        recognizer.accept_pcm16(16000, &vec![0i16; 4800]);
        let _ = recognizer.finalize();
    }
}
