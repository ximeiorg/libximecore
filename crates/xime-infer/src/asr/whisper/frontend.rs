//! Whisper 的 log-mel 前处理（与 `WhisperFeatureExtractor` 对齐）。
//!
//! | 参数 | 值 |
//! | --- | --- |
//! | 采样率 | 16 kHz 单声道 |
//! | 窗长 / 帧移 | 400 点（25 ms）/ 160 点（10 ms），`center=True` 反射补边 |
//! | 窗函数 | 周期 Hann（分母 `n_fft`，同 `torch.hann_window`） |
//! | mel | Slaney 归一化三角滤波器，80 或 128 个（跟随 `num_mel_bins`） |
//! | 取对数 | `log10(max(e, 1e-10))`，再按**全窗**最大值钳到 -8 dB |
//! | 缩放 | `(x + 4) / 4` |
//!
//! **必须补到 30 秒**（480000 点 → 3000 帧），这与 Qwen3-ASR 的「补到整秒」不同。
//! 原因不是省算力而是语义：whisper 编码器是整段双向注意力，补出来的静音帧会作为
//! key/value 参与**真实帧**的注意力，少喂帧会让结果与官方不一致；位置表也固定
//! 1500 格（每格 20 ms），网络结构里没有「变长」这一说。官方实现、whisper.cpp、
//! faster-whisper 都是补满 30 s，这里照做。
//!
//! mel 滤波器组不读 `preprocessor_config.json`：新版 transformers（4.46+）已经把
//! `mel_filters` 从配置里删掉，改成装载时用 librosa 现场算；老版本（whisper-small
//! 这份）仍然带着 80×201 的表。两条路算的是同一个 Slaney 滤波器组，本模块按公式
//! 生成，`mel_filters_match_transformers_reference` 用老版本存的表逐点校验。

use std::f32::consts::PI;

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

/// 单次音频窗口（秒）。whisper 全系按 30 s 训练，编码器位置表也只有 1500 格。
pub const WINDOW_SECS: usize = 30;
/// 采样率。
pub const SAMPLE_RATE: usize = 16_000;
/// FFT 点数（25 ms）。
pub const N_FFT: usize = 400;
/// 帧移（10 ms）。
pub const HOP_LENGTH: usize = 160;

/// 一窗音频的采样点数（480000）。
pub const WINDOW_SAMPLES: usize = WINDOW_SECS * SAMPLE_RATE;
/// 一窗的 mel 帧数（3000）。
pub const WINDOW_FRAMES: usize = WINDOW_SAMPLES / HOP_LENGTH;

/// mel 前端参数。采样率/窗长/帧移是 whisper 的固定值，只有 mel 维度随代际变。
#[derive(Debug, Clone)]
pub struct AudioFrontend {
    pub sample_rate: usize,
    pub n_fft: usize,
    pub hop_length: usize,
    pub mel_bins: usize,
    /// 一窗的采样点数（480000）。
    pub window_samples: usize,
}

impl AudioFrontend {
    pub fn new(mel_bins: usize) -> Self {
        Self {
            sample_rate: SAMPLE_RATE,
            n_fft: N_FFT,
            hop_length: HOP_LENGTH,
            mel_bins,
            window_samples: WINDOW_SAMPLES,
        }
    }

    /// 一窗的 mel 帧数（3000）。
    pub fn frames(&self) -> usize {
        self.window_samples / self.hop_length
    }

    /// mel 滤波器组 `[mel_bins, n_freq]`（行主序）。对拍/测试用；正常运行不需要
    /// 把它拿出来，`log_mel` 内部自己算。
    pub fn mel_filters(&self) -> Vec<f32> {
        build_mel_filters(self)
    }

    /// 计算 log-mel，返回 `[mel_bins, 3000]`。
    ///
    /// 输入短于 30 s 时**右补零**，长于 30 s 的音频由调用方切段（返回
    /// [`crate::InferError::AudioTooLong`]）。
    pub fn log_mel(&self, samples: &[f32]) -> LogMel {
        let mut pcm = samples;
        // 超窗只不该发生（调用方已挡），这里截断保证形状不变，避免越界。
        if pcm.len() > self.window_samples {
            pcm = &pcm[..self.window_samples];
        }
        log_mel_padded(pcm, self)
    }
}

/// log-mel 结果，布局为 `[mel_bins, frames]` 行主序（mel 在外层），与
/// transformers 的 `input_features` 一致。
#[derive(Debug, Clone)]
pub struct LogMel {
    pub data: Vec<f32>,
    pub frames: usize,
    pub mel_bins: usize,
}

impl LogMel {
    pub fn get(&self, mel: usize, frame: usize) -> f32 {
        self.data[mel * self.frames + frame]
    }
}

/// 计算 log-mel：内部把 `pcm` 补零到 30 s。
fn log_mel_padded(pcm: &[f32], front: &AudioFrontend) -> LogMel {
    let n = pcm.len().min(front.window_samples);
    let n_fft = front.n_fft;
    let hop = front.hop_length;
    let n_freq = n_fft / 2 + 1;
    let mel_bins = front.mel_bins;
    let pad = n_fft / 2;
    let total = front.window_samples;

    // 先把有效音频补零到整窗，再做 center=True 的反射填充。反射填充的边界是
    // 「整窗」而不是「有效音频」——官方同样先 `padding_value=0.0` 补到 30 s。
    let mut window = vec![0.0f32; total];
    window[..n].copy_from_slice(&pcm[..n]);

    let mut padded = Vec::with_capacity(total + 2 * pad);
    for i in 0..pad {
        let src = pad as isize - i as isize;
        padded.push(if src >= 0 && (src as usize) < total {
            window[src as usize]
        } else {
            0.0
        });
    }
    padded.extend_from_slice(&window);
    for i in 0..pad {
        let src = total as isize - 2 - i as isize;
        padded.push(if src >= 0 && (src as usize) < total {
            window[src as usize]
        } else {
            0.0
        });
    }

    // 与 torch.stft(center=True) 一致：丢掉最后一帧。
    let n_frames_total = (padded.len().saturating_sub(n_fft)) / hop + 1;
    let n_frames = n_frames_total.saturating_sub(1).min(front.frames());

    let window_fn: Vec<f32> = (0..n_fft)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / n_fft as f32).cos()))
        .collect();
    let filters = build_mel_filters(front);

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n_fft);
    let mut scratch = vec![Complex::new(0.0f32, 0.0); fft.get_inplace_scratch_len()];
    let mut buf = vec![Complex::new(0.0f32, 0.0); n_fft];
    let mut power = vec![0.0f32; n_freq];

    // 临时按 [frames, mel] 存，最后转置；全局最大值要等所有帧算完才知道。
    let mut mel = vec![0.0f32; n_frames * mel_bins];
    let mut global_max = f32::NEG_INFINITY;

    for t in 0..n_frames {
        let start = t * hop;
        for i in 0..n_fft {
            buf[i] = Complex::new(padded[start + i] * window_fn[i], 0.0);
        }
        fft.process_with_scratch(&mut buf, &mut scratch);
        for f in 0..n_freq {
            let c = buf[f];
            power[f] = c.re * c.re + c.im * c.im;
        }
        for m in 0..mel_bins {
            let filt = &filters[m * n_freq..(m + 1) * n_freq];
            let sum: f32 = filt.iter().zip(power.iter()).map(|(f, p)| f * p).sum();
            // 下限 1e-10 与官方一致：静音帧 log10 → -10，而不是 -inf。
            let val = sum.max(1e-10).log10();
            mel[t * mel_bins + m] = val;
            if val > global_max {
                global_max = val;
            }
        }
    }

    let min_val = global_max - 8.0;
    let mut out = vec![0.0f32; mel_bins * n_frames];
    for t in 0..n_frames {
        for m in 0..mel_bins {
            let v = mel[t * mel_bins + m].max(min_val);
            out[m * n_frames + t] = (v + 4.0) / 4.0;
        }
    }

    LogMel {
        data: out,
        frames: n_frames,
        mel_bins,
    }
}

/// Slaney 归一化的 mel 三角滤波器组，`[mel_bins, n_freq]` 行主序。
///
/// 与 librosa `filters.mel(..., htk=False, norm='slaney')` 逐点一致：
/// 线性区（< 1000 Hz）`mel = 3f/200`，对数区在上面接 `15 + ln(f/1000)·27/ln 6.4`；
/// 每个滤波器按带宽 `2/(f[m+2]-f[m])` 归一化。
fn build_mel_filters(front: &AudioFrontend) -> Vec<f32> {
    let n_fft = front.n_fft;
    let n_freq = n_fft / 2 + 1;
    let n_mel = front.mel_bins;
    let sr = front.sample_rate as f32;

    let hz_to_mel = |freq: f32| -> f32 {
        let min_log_hz = 1000.0f32;
        let min_log_mel = 15.0f32;
        let logstep = 27.0f32 / 6.4f32.ln();
        if freq >= min_log_hz {
            min_log_mel + (freq / min_log_hz).ln() * logstep
        } else {
            3.0 * freq / 200.0
        }
    };
    let mel_to_hz = |mels: f32| -> f32 {
        let min_log_hz = 1000.0f32;
        let min_log_mel = 15.0f32;
        let logstep = 6.4f32.ln() / 27.0f32;
        if mels >= min_log_mel {
            min_log_hz * ((mels - min_log_mel) * logstep).exp()
        } else {
            200.0 * mels / 3.0
        }
    };

    let fft_freqs: Vec<f32> = (0..n_freq).map(|i| i as f32 * sr / n_fft as f32).collect();

    let mel_min = hz_to_mel(0.0);
    let mel_max = hz_to_mel(sr / 2.0);
    let filter_freqs: Vec<f32> = (0..=n_mel + 1)
        .map(|i| mel_to_hz(mel_min + (mel_max - mel_min) * i as f32 / (n_mel + 1) as f32))
        .collect();

    let filter_diff: Vec<f32> = (0..=n_mel)
        .map(|i| {
            let d = filter_freqs[i + 1] - filter_freqs[i];
            if d == 0.0 {
                1e-6
            } else {
                d
            }
        })
        .collect();

    let mut filters = vec![0.0f32; n_mel * n_freq];
    for m in 0..n_mel {
        let enorm = 2.0 / (filter_freqs[m + 2] - filter_freqs[m]);
        for f in 0..n_freq {
            let down = (fft_freqs[f] - filter_freqs[m]) / filter_diff[m];
            let up = (filter_freqs[m + 2] - fft_freqs[f]) / filter_diff[m + 1];
            filters[m * n_freq + f] = down.min(up).max(0.0) * enorm;
        }
    }
    filters
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `whisper-small/preprocessor_config.json` 里存的 librosa 滤波器组，
    /// 抽查 5 行（80 mel / 201 频点）的非零点。老版本 transformers 把这个表
    /// 原样写进配置，正好拿来当独立参考。
    #[test]
    fn mel_filters_match_transformers_reference() {
        let front = AudioFrontend::new(80);
        let filters = build_mel_filters(&front);
        let n_freq = front.n_fft / 2 + 1;
        let get = |m: usize, f: usize| filters[m * n_freq + f];

        // (mel 行, 频点, 参考值)
        let cases = [
            (0, 1, 0.024862_595f32),
            (1, 1, 0.001_990_822),
            (1, 2, 0.022_871_772),
            (20, 19, 0.010_972_2),
            (20, 20, 0.013_890_394),
            (40, 42, 0.005_411_105),
            (40, 43, 0.014_735_566),
            (40, 44, 0.006_518_19),
            (79, 186, 0.000_366_742),
            (79, 199, 0.000_448_759),
        ];
        for (m, f, want) in cases {
            let got = get(m, f);
            assert!(
                (got - want).abs() < 2e-6,
                "mel[{m}][{f}] = {got}，参考 {want}"
            );
        }

        // 稀疏性：第 0 行只有频点 1 非零
        let nz: Vec<usize> = (0..n_freq).filter(|f| get(0, *f) != 0.0).collect();
        assert_eq!(nz, vec![1]);
        // 高 mel 行的非零宽度更大（对数区带宽更宽）
        let width = |m: usize| (0..n_freq).filter(|f| get(m, *f) != 0.0).count();
        assert!(width(79) > width(20));
    }

    #[test]
    fn log_mel_has_fixed_30s_shape() {
        let front = AudioFrontend::new(128);
        for secs in [0.5f32, 1.0, 5.0, 30.0] {
            let pcm = vec![0.1f32; (16_000.0 * secs) as usize];
            let mel = front.log_mel(&pcm);
            assert_eq!(mel.frames, WINDOW_FRAMES, "{secs} s 输入");
            assert_eq!(mel.mel_bins, 128);
            assert_eq!(mel.data.len(), 3000 * 128);
            assert!(mel.data.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn silence_mel_is_constant_and_scaled() {
        let front = AudioFrontend::new(80);
        let mel = front.log_mel(&vec![0.0f32; 16_000]);
        // log10(1e-10) = -10，全局最大值也是 -10（不被 max-8 钳），(x+4)/4 = -1.5
        for m in [0usize, 40, 79] {
            let v = mel.get(m, 0);
            assert!((v + 1.5).abs() < 1e-5, "mel[{m}][0] = {v}");
        }
    }

    #[test]
    fn tone_energy_peaks_in_low_mel_band() {
        let front = AudioFrontend::new(80);
        let pcm: Vec<f32> = (0..16_000)
            .map(|i| (2.0 * PI * 1000.0 * i as f32 / 16_000.0).sin() * 0.5)
            .collect();
        let mel = front.log_mel(&pcm);
        let energy: Vec<f32> = (0..80)
            .map(|m| (0..mel.frames).map(|t| mel.get(m, t)).sum::<f32>())
            .collect();
        let (best, _) = energy
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert!((10..40).contains(&best), "1 kHz 峰值落在 mel bin {best}");
    }

    #[test]
    fn truncates_overlong_input_instead_of_panicking() {
        let front = AudioFrontend::new(80);
        let mel = front.log_mel(&vec![0.2f32; WINDOW_SAMPLES + 12_345]);
        assert_eq!(mel.frames, WINDOW_FRAMES);
    }
}
