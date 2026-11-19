//! Qwen3-ASR 的 log-mel 前处理（Whisper 式）。
//!
//! 参数与 transformers 的 `WhisperFeatureExtractor` 一致：
//! `n_fft = 400`、`hop_length = 160`、128 个 mel、Slaney 归一化滤波器组、
//! `log10` 后按全局最大值钳到 -8 dB、再 `(x + 4) / 4` 缩放。
//!
//! 与官方实现唯一的差别是**不补到 30 秒**：官方把音频补零到 30 s 再算 mel，
//! 然后按有效帧数截取 token。这里补到下一个整秒（整 100 帧），卷积在边界
//! 看到的同样是零帧，因此保留下来的 token 与官方一致，但省掉 3 倍卷积。
//!
//! 这是 Qwen3-ASR 专属的前处理；Fun-ASR-Nano 走 Kaldi fbank，见
//! [`crate::asr::fun_asr::frontend`]。

use std::f32::consts::PI;

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

/// mel 频谱参数（固定值来自 `preprocessor_config.json`）。
#[derive(Debug, Clone)]
pub struct AudioFrontend {
    pub sample_rate: u32,
    pub n_fft: usize,
    pub hop_length: usize,
    pub mel_bins: usize,
}

impl AudioFrontend {
    pub fn new(sample_rate: u32, n_fft: usize, hop_length: usize, mel_bins: usize) -> Self {
        Self {
            sample_rate,
            n_fft,
            hop_length,
            mel_bins,
        }
    }

    /// 帧数：与 `torch.stft(center=True)` 丢掉最后一帧后一致。
    pub fn frames(&self, samples: usize) -> usize {
        samples / self.hop_length
    }
}

/// log-mel 结果，布局为 `[mel_bins, frames]` 行主序（mel 在外层）。
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

/// 计算 log-mel 频谱。
///
/// 输入是 16 kHz 单声道 f32；`samples` 长度应为 `hop_length` 的整数倍
/// （调用方补到整秒，见 [`crate::asr::qwen3_asr::Qwen3Asr::transcribe`]）。
pub fn log_mel(samples: &[f32], front: &AudioFrontend) -> LogMel {
    let n_fft = front.n_fft;
    let hop = front.hop_length;
    let n_freq = n_fft / 2 + 1;
    let mel_bins = front.mel_bins;
    let pad = n_fft / 2;
    let n = samples.len();

    // reflect 填充，与 torch.stft(center=True) 一致
    let mut padded = Vec::with_capacity(n + 2 * pad);
    for i in 0..pad {
        let src = pad as isize - i as isize;
        padded.push(if src >= 0 && (src as usize) < n {
            samples[src as usize]
        } else {
            0.0
        });
    }
    padded.extend_from_slice(samples);
    for i in 0..pad {
        let src = n as isize - 2 - i as isize;
        padded.push(if src >= 0 && (src as usize) < n {
            samples[src as usize]
        } else {
            0.0
        });
    }

    let n_frames_total = (padded.len().saturating_sub(n_fft)) / hop + 1;
    let n_frames = n_frames_total.saturating_sub(1);
    if n_frames == 0 {
        return LogMel {
            data: vec![],
            frames: 0,
            mel_bins,
        };
    }

    // 周期 Hann 窗
    let window: Vec<f32> = (0..n_fft)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / n_fft as f32).cos()))
        .collect();
    let filters = build_mel_filters(front);

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n_fft);
    let mut scratch = vec![Complex::new(0.0f32, 0.0); fft.get_inplace_scratch_len()];
    let mut buf = vec![Complex::new(0.0f32, 0.0); n_fft];
    let mut power = vec![0.0f32; n_freq];

    let mut mel = vec![0.0f32; n_frames * mel_bins];
    let mut global_max = f32::NEG_INFINITY;

    for t in 0..n_frames {
        let start = t * hop;
        for i in 0..n_fft {
            buf[i] = Complex::new(padded[start + i] * window[i], 0.0);
        }
        fft.process_with_scratch(&mut buf, &mut scratch);
        for f in 0..n_freq {
            let c = buf[f];
            power[f] = c.re * c.re + c.im * c.im;
        }
        for m in 0..mel_bins {
            let filt = &filters[m * n_freq..(m + 1) * n_freq];
            let sum: f32 = filt.iter().zip(power.iter()).map(|(f, p)| f * p).sum();
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

/// Slaney 归一化的 mel 滤波器组，`[mel_bins, n_freq]` 行主序。
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

    fn front() -> AudioFrontend {
        AudioFrontend::new(16_000, 400, 160, 128)
    }

    fn silence(secs: f32) -> Vec<f32> {
        vec![0.0; (16_000.0 * secs) as usize]
    }

    #[test]
    fn mel_shape_matches_hop() {
        let mel = log_mel(&silence(2.0), &front());
        assert_eq!(mel.frames, 200);
        assert_eq!(mel.mel_bins, 128);
        assert_eq!(mel.data.len(), 200 * 128);
    }

    #[test]
    fn mel_of_silence_is_finite_and_scaled() {
        let mel = log_mel(&silence(1.0), &front());
        // 全静音：log10(1e-10) = -10，全局最大值也是 -10，
        // 所以不会被 max-8 钳到，最后 (x+4)/4 = -1.5
        assert!((mel.get(0, 0) + 1.5).abs() < 1e-5, "{}", mel.get(0, 0));
        assert!(mel.data.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn mel_of_tone_peaks_in_expected_band() {
        // 1 kHz 正弦：能量应集中在低频段（mel 0..40）
        let samples: Vec<f32> = (0..16_000)
            .map(|i| (2.0 * PI * 1000.0 * i as f32 / 16_000.0).sin() * 0.5)
            .collect();
        let mel = log_mel(&samples, &front());
        let energies: Vec<f32> = (0..128)
            .map(|m| (0..mel.frames).map(|t| mel.get(m, t)).sum::<f32>())
            .collect();
        let (best, _) = energies
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert!((20..60).contains(&best), "峰值落在 mel bin {best}");
    }

    #[test]
    fn mel_frame_count_is_floor() {
        // 非整秒输入也不 panic；帧数按 hop 取整
        let mel = log_mel(&vec![0.1; 12_345], &front());
        assert_eq!(mel.frames, 12_345 / 160);
    }
}
