//! 音频前处理：WAV 解码、重采样、log-mel 频谱。
//!
//! mel 参数与 transformers 的 `WhisperFeatureExtractor` 一致：
//! `n_fft = 400`、`hop_length = 160`、128 个 mel、Slaney 归一化滤波器组、
//! `log10` 后按全局最大值钳到 -8 dB、再 `(x + 4) / 4` 缩放。
//!
//! 与官方实现唯一的差别是**不补到 30 秒**：官方把音频补零到 30 s 再算 mel，
//! 然后按有效帧数截取 token。这里补到下一个整秒（整 100 帧），卷积在边界
//! 看到的同样是零帧，因此保留下来的 token 与官方一致，但省掉 3 倍卷积。

use std::f32::consts::PI;
use std::path::Path;

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

use crate::InferError;

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

/// 读 WAV，混成单声道 f32，并按需重采样到 16 kHz。
pub fn load_wav_16k(path: &Path, target_rate: u32) -> Result<Vec<f32>, InferError> {
    let reader = hound::WavReader::open(path)
        .map_err(|e| InferError::Audio(format!("打不开 WAV {}：{e}", path.display())))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    if channels == 0 {
        return Err(InferError::Audio("WAV 声道数为 0".into()));
    }

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample;
            if bits == 0 || bits > 32 {
                return Err(InferError::Audio(format!("不支持的位深：{bits}")));
            }
            // 8-bit WAV 是无符号，其余是有符号。
            let scale = 1.0 / (1i64 << (bits - 1)) as f32;
            if bits == 8 {
                reader
                    .into_samples::<i32>()
                    .map(|s| {
                        s.map(|v| (v as f32 - 128.0) / 128.0)
                            .map_err(|e| InferError::Audio(e.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                reader
                    .into_samples::<i32>()
                    .map(|s| {
                        s.map(|v| v as f32 * scale)
                            .map_err(|e| InferError::Audio(e.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| s.map_err(|e| InferError::Audio(e.to_string())))
            .collect::<Result<Vec<_>, _>>()?,
    };

    let mono = if channels == 1 {
        samples
    } else {
        samples
            .chunks_exact(channels)
            .map(|ch| ch.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    if spec.sample_rate == target_rate {
        Ok(mono)
    } else {
        resample(&mono, spec.sample_rate, target_rate)
    }
}

/// 带限重采样（rubato sinc）。声道数固定为 1。
pub fn resample(samples: &[f32], from_hz: u32, to_hz: u32) -> Result<Vec<f32>, InferError> {
    use rubato::{
        Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
    };

    if from_hz == 0 || to_hz == 0 {
        return Err(InferError::Audio("采样率为 0".into()));
    }
    if samples.is_empty() || from_hz == to_hz {
        return Ok(samples.to_vec());
    }

    let ratio = to_hz as f64 / from_hz as f64;
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let chunk = 4096usize;
    let mut resampler = SincFixedIn::<f32>::new(ratio, 2.0, params, chunk, 1)
        .map_err(|e| InferError::Audio(format!("重采样器初始化失败（{from_hz}→{to_hz}）：{e}")))?;

    let expected = (samples.len() as f64 * ratio).round() as usize;
    let mut out = Vec::with_capacity(expected + 64);
    for block in samples.chunks(chunk) {
        let mut frame = vec![0.0f32; chunk];
        frame[..block.len()].copy_from_slice(block);
        let waves = resampler
            .process(&[frame], None)
            .map_err(|e| InferError::Audio(format!("重采样失败：{e}")))?;
        out.extend_from_slice(&waves[0]);
    }
    // 冲刷滤波器尾部
    let tail = resampler
        .process_partial(Some(&[vec![0.0f32; chunk]]), None)
        .map_err(|e| InferError::Audio(format!("重采样收尾失败：{e}")))?;
    if let Some(ch) = tail.first() {
        out.extend_from_slice(ch);
    }
    // rubato 的 chunk 输出会向上取整，且滤波器尾部会多出延迟样本；
    // 按理论长度裁剪/补零，保证 44.1k/48k 输入的时长与 16k 一致。
    out.truncate(expected);
    out.resize(expected, 0.0);
    Ok(out)
}

/// 计算 log-mel 频谱。
///
/// 输入是 16 kHz 单声道 f32；`samples` 长度应为 `hop_length` 的整数倍
/// （调用方补到整秒，见 [`crate::asr::Qwen3Asr::transcribe`]）。
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

    #[test]
    fn resample_changes_rate_and_keeps_length_ratio() {
        let samples: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / 48_000.0).sin())
            .collect();
        let out = resample(&samples, 48_000, 16_000).unwrap();
        let expected = 16_000.0;
        assert!(
            (out.len() as f32 - expected).abs() < expected * 0.02,
            "重采样后 {} 点，期望约 {expected}",
            out.len()
        );
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn resample_same_rate_is_identity() {
        let samples = vec![0.1f32, -0.2, 0.3];
        assert_eq!(resample(&samples, 16_000, 16_000).unwrap(), samples);
    }

    #[test]
    fn wav_roundtrip_mono() {
        let path = std::env::temp_dir().join("xime_infer_wav_roundtrip.wav");
        {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(&path, spec).unwrap();
            for i in 0..1600 {
                w.write_sample(((i % 100) as i16 - 50) * 100).unwrap();
            }
            w.finalize().unwrap();
        }
        let pcm = load_wav_16k(&path, 16_000).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(pcm.len(), 1600);
        assert!(pcm.iter().all(|v| (-1.0..=1.0).contains(v)));
    }

    #[test]
    fn stereo_is_averaged() {
        let path = std::env::temp_dir().join("xime_infer_wav_stereo.wav");
        {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(&path, spec).unwrap();
            for _ in 0..100 {
                w.write_sample(16_000i16).unwrap();
                w.write_sample(-16_000i16).unwrap();
            }
            w.finalize().unwrap();
        }
        let pcm = load_wav_16k(&path, 16_000).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(pcm.len(), 100);
        assert!(pcm.iter().all(|v| v.abs() < 1e-6));
    }
}
