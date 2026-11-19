//! Fun-ASR-Nano 的前处理：Kaldi fbank（80 维）+ LFR 拼帧。
//!
//! 与 transformers 的 `FunAsrNanoFeatureExtractor` 对齐（它内部走
//! `torchaudio.compliance.kaldi.fbank` 的默认参数）：
//!
//! | 参数 | 值 |
//! | --- | --- |
//! | 采样率 | 16 kHz 单声道 |
//! | 窗长 / 帧移 | 25 ms（400 点）/ 10 ms（160 点），`snip_edges=True` |
//! | 窗函数 | hamming（周期式，分母 N 而非 N-1，同 torch.hamming_window） |
//! | 去直流 | 先减去本帧均值 |
//! | 预加重 | 0.97，在去直流**之后**做（同 Kaldi `ProcessWindow`） |
//! | FFT | 补零到 512 点，只取前 256 个频点（丢掉 Nyquist 那一格） |
//! | mel | 80 个三角滤波器，20 Hz ~ Nyquist，Kaldi 的 `1127·ln(1+f/700)` |
//! | 取对数 | `ln(max(e, f32::EPSILON))` |
//! | LFR | 叠 7 帧、跳 6 帧，首尾各复制 3 帧 → 每帧 560 维 |
//!
//! **幅度尺度**：FunASR 官方前端会先把波形乘以 32768（int16 量纲），这里不乘。
//! log-mel 会因此整体平移 `2·ln(32768) ≈ 20.8`，而编码器第一层就是沿 560 维做的
//! LayerNorm——整体平移在减均值时被消掉，所以两种尺度送进模型是等价的。省掉一次
//! 全量乘法，也避免浮点溢出。
//!
//! 与 Qwen3-ASR 的前端（Whisper 式 128 维 log-mel、`log10` + 动态范围压缩）没有任何
//! 共同点，所以这里不复用 [`crate::asr::qwen3_asr::frontend`] 的任何代码。

use std::f32::consts::PI;
use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::InferError;

/// 预加重系数（Kaldi / torchaudio 默认）。
const PREEMPH: f32 = 0.97;
/// mel 滤波器最低频率（Hz）。
const LOW_FREQ: f32 = 20.0;
/// 取对数时的下限：Kaldi 用 `numeric_limits<float>::epsilon()`。
const LOG_EPS: f32 = f32::EPSILON;

/// fbank + LFR 前端。
pub struct FbankFrontend {
    /// 窗长（采样点），25 ms → 400。
    frame_length: usize,
    /// 帧移（采样点），10 ms → 160。
    frame_shift: usize,
    /// mel 通道数（80）。
    mel_bins: usize,
    /// FFT 点数（窗长向上取到 2 的幂 → 512）。
    n_fft: usize,
    /// 参与 mel 加权的频点数（`n_fft / 2`，即丢 Nyquist → 256）。
    fft_bins: usize,
    /// hamming 窗，长度 `frame_length`。
    window: Vec<f32>,
    /// mel 滤波器组，`[mel_bins, fft_bins]` 行主序。
    banks: Vec<f32>,
    /// LFR 叠帧数（7）。
    lfr_m: usize,
    /// LFR 跳帧数（6）。
    lfr_n: usize,
    fft: Arc<dyn Fft<f32>>,
}

impl FbankFrontend {
    /// `frame_length_ms` / `frame_shift_ms` 来自 `processor_config.json`，
    /// 缺省即官方值 25 / 10。
    pub fn new(
        sample_rate: u32,
        mel_bins: usize,
        frame_length_ms: usize,
        frame_shift_ms: usize,
        lfr_m: usize,
        lfr_n: usize,
    ) -> Result<Self, InferError> {
        if sample_rate == 0 || mel_bins == 0 || lfr_m == 0 || lfr_n == 0 {
            return Err(InferError::Config(
                "fbank 参数出现 0（采样率 / mel 通道 / LFR）".into(),
            ));
        }
        let n_fft = (sample_rate as usize * frame_length_ms / 1000).next_power_of_two();
        let frame_length = n_fft.min(sample_rate as usize * frame_length_ms / 1000);
        let frame_shift = sample_rate as usize * frame_shift_ms / 1000;
        if frame_shift == 0 {
            return Err(InferError::Config("帧移为 0".into()));
        }
        let fft_bins = n_fft / 2;
        Ok(Self {
            frame_length,
            frame_shift,
            mel_bins,
            n_fft,
            fft_bins,
            window: hamming(frame_length),
            banks: mel_banks(
                mel_bins,
                fft_bins,
                sample_rate as f32,
                LOW_FREQ,
                n_fft as f32,
            ),
            lfr_m,
            lfr_n,
            fft: FftPlanner::<f32>::new().plan_fft_forward(n_fft),
        })
    }

    /// LFR 之后每帧的维度（`mel_bins * lfr_m`，官方 560）。
    pub fn feature_dim(&self) -> usize {
        self.mel_bins * self.lfr_m
    }

    /// 原始 mel 帧数（`snip_edges=True`）。
    pub fn mel_frames(&self, samples: usize) -> usize {
        if samples < self.frame_length {
            0
        } else {
            1 + (samples - self.frame_length) / self.frame_shift
        }
    }

    /// LFR 之后的帧数：`ceil(mel_frames / lfr_n)`。
    pub fn lfr_frames(&self, samples: usize) -> usize {
        self.mel_frames(samples).div_ceil(self.lfr_n)
    }

    /// PCM → 拼帧后的特征，`[lfr_frames, feature_dim]` 行主序。
    pub fn extract(&self, pcm: &[f32]) -> Result<(Vec<f32>, usize), InferError> {
        let frames = self.mel_frames(pcm.len());
        if frames == 0 {
            return Err(InferError::EmptyAudio);
        }
        let mel = self.log_mel(pcm, frames);
        Ok(self.apply_lfr(&mel, frames, self.mel_bins))
    }

    /// `[frames, mel_bins]` 的 log-mel。
    fn log_mel(&self, pcm: &[f32], frames: usize) -> Vec<f32> {
        let mut out = vec![0f32; frames * self.mel_bins];
        let mut buf = vec![Complex::new(0f32, 0f32); self.n_fft];
        let mut windowed = vec![0f32; self.frame_length];
        let mut power = vec![0f32; self.fft_bins];

        for f in 0..frames {
            let start = f * self.frame_shift;
            windowed.copy_from_slice(&pcm[start..start + self.frame_length]);

            // 1) 去直流
            let mean = windowed.iter().sum::<f32>() / self.frame_length as f32;
            for v in windowed.iter_mut() {
                *v -= mean;
            }
            // 2) 预加重（从后往前算，避免覆盖还没读到的样本）
            for i in (1..self.frame_length).rev() {
                windowed[i] -= PREEMPH * windowed[i - 1];
            }
            windowed[0] *= 1.0 - PREEMPH;
            // 3) 加窗 + 补零到 n_fft
            for (i, z) in buf.iter_mut().enumerate() {
                *z = if i < self.frame_length {
                    Complex::new(windowed[i] * self.window[i], 0.0)
                } else {
                    Complex::new(0.0, 0.0)
                };
            }

            self.fft.process(&mut buf);
            for (i, p) in power.iter_mut().enumerate() {
                *p = buf[i].re * buf[i].re + buf[i].im * buf[i].im;
            }

            // 4) mel 加权 + 取对数
            for bin in 0..self.mel_bins {
                let row = &self.banks[bin * self.fft_bins..(bin + 1) * self.fft_bins];
                let mut energy = 0f32;
                for (w, p) in row.iter().zip(power.iter()) {
                    energy += w * p;
                }
                out[f * self.mel_bins + bin] = energy.max(LOG_EPS).ln();
            }
        }
        out
    }

    /// LFR：`[frames, dim]` → `[ceil(frames/lfr_n), dim*lfr_m]`。
    ///
    /// 首尾各复制 `(lfr_m-1)/2` 帧，让每个窗口都以原帧为中心；窗口越界时继续用
    /// 最后一帧补齐（等价于 `torch.Tensor.unfold` + 尾部 expand）。
    fn apply_lfr(&self, mel: &[f32], frames: usize, dim: usize) -> (Vec<f32>, usize) {
        let left = (self.lfr_m - 1) / 2;
        let out_frames = frames.div_ceil(self.lfr_n);
        let mut out = Vec::with_capacity(out_frames * dim * self.lfr_m);

        for t in 0..out_frames {
            for k in 0..self.lfr_m {
                let p = t * self.lfr_n + k;
                // 把 padded 下标映射回原图：越界部分夹到首/尾帧。
                let src = if p < left {
                    0
                } else if p - left >= frames {
                    frames - 1
                } else {
                    p - left
                };
                out.extend_from_slice(&mel[src * dim..(src + 1) * dim]);
            }
        }
        (out, out_frames)
    }
}

/// 周期式 hamming 窗：`0.54 - 0.46·cos(2πi/N)`。
fn hamming(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.54 - 0.46 * (2.0 * PI * i as f32 / n as f32).cos())
        .collect()
}

/// Kaldi mel 三角滤波器组，`[num_bins, fft_bins]` 行主序。
///
/// 与 `torchaudio.compliance.kaldi._get_mel_banks` 一致：在 mel 轴上把
/// `[low_freq, high_freq]` 均分成 `num_bins + 2` 个点，第 i 个三角形的
/// 左/中/右顶点是第 i / i+1 / i+2 个点；只在 `left < mel < right` 的频点上取值。
fn mel_banks(
    num_bins: usize,
    fft_bins: usize,
    sample_freq: f32,
    low_freq: f32,
    n_fft: f32,
) -> Vec<f32> {
    let nyquist = sample_freq / 2.0;
    let fft_bin_width = sample_freq / n_fft;
    let mel_low = mel_scale(low_freq);
    let mel_high = mel_scale(nyquist);
    let delta = (mel_high - mel_low) / (num_bins + 1) as f32;

    let mut banks = vec![0f32; num_bins * fft_bins];
    for bin in 0..num_bins {
        let left = mel_low + bin as f32 * delta;
        let center = mel_low + (bin + 1) as f32 * delta;
        let right = mel_low + (bin + 2) as f32 * delta;
        for i in 0..fft_bins {
            let mel = mel_scale(fft_bin_width * i as f32);
            if mel > left && mel < right {
                let w = if mel <= center {
                    (mel - left) / (center - left)
                } else {
                    (right - mel) / (right - center)
                };
                banks[bin * fft_bins + i] = w;
            }
        }
    }
    banks
}

/// Kaldi 的 mel 刻度：`1127·ln(1 + f/700)`。
fn mel_scale(freq: f32) -> f32 {
    1127.0 * (1.0 + freq / 700.0).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front() -> FbankFrontend {
        FbankFrontend::new(16_000, 80, 25, 10, 7, 6).unwrap()
    }

    #[test]
    fn frame_geometry_matches_kaldi() {
        let f = front();
        assert_eq!(f.frame_length, 400);
        assert_eq!(f.frame_shift, 160);
        assert_eq!(f.n_fft, 512);
        assert_eq!(f.fft_bins, 256);
        // 不足一窗（400 点 = 25 ms）就没有帧；`snip_edges=True` 下不补零。
        assert_eq!(f.mel_frames(399), 0);
        assert_eq!(f.mel_frames(400), 1);
        assert_eq!(f.mel_frames(16_000), 98);
        // 3000 帧 / 6 → 500 个 LFR 帧
        assert_eq!(f.mel_frames(30 * 16_000), 2_998);
        assert_eq!(f.lfr_frames(30 * 16_000), 500);
        assert_eq!(f.feature_dim(), 560);
    }

    #[test]
    fn mel_banks_cover_bands_without_gaps() {
        let f = front();
        assert_eq!(f.banks.len(), 80 * 256);
        // 每个频点至少被一个滤波器覆盖（20 Hz~8 kHz 密布），且权重都为正。
        let mut covered = vec![0usize; 256];
        for bin in 0..80 {
            for (i, count) in covered.iter_mut().enumerate() {
                let w = f.banks[bin * 256 + i];
                assert!((0.0..=1.000_001).contains(&w), "权重越界：{w}");
                if w > 0.0 {
                    *count += 1;
                }
            }
        }
        // 低频（< 20 Hz 的第一格）不参与；其余频点都应被覆盖。
        assert_eq!(covered[0], 0, "0 Hz 不该被覆盖");
        assert!(covered[100..].iter().all(|c| *c > 0), "中高频有空洞");
    }

    #[test]
    fn hamming_window_is_symmetric_about_half() {
        let w = hamming(400);
        assert_eq!(w.len(), 400);
        assert!((w[0] - 0.08).abs() < 1e-6);
        assert!((w[200] - 1.0).abs() < 0.001);
        assert!((w[399] - 0.080_0).abs() < 0.001);
    }

    #[test]
    fn log_mel_shape_and_finiteness() {
        let f = front();
        let pcm = vec![0.1f32; 16_000];
        let (lfr, frames) = f.extract(&pcm).unwrap();
        assert_eq!(frames, 98usize.div_ceil(6));
        assert_eq!(lfr.len(), frames * 560);
        assert!(lfr.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn lfr_stacks_neighbours_and_pads_both_ends() {
        // 造 8 帧、每帧 1 维的假特征，直接看拼帧结果。
        let f = front();
        let mel: Vec<f32> = (0..8).map(|i| i as f32).collect();
        let (out, frames) = f.apply_lfr(&mel, 8, 1);
        assert_eq!(frames, 2); // ceil(8/6)
        assert_eq!(out.len(), 2 * 7);
        // 第 0 帧：左补 3 个 f0，再 f0..f3 → [0,0,0,0,1,2,3]
        assert_eq!(&out[..7], &[0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0]);
        // 第 1 帧：从 f3 起，尾部越界补 f7 → [3,4,5,6,7,7,7]
        assert_eq!(&out[7..], &[3.0, 4.0, 5.0, 6.0, 7.0, 7.0, 7.0]);
    }

    #[test]
    fn lfr_single_frame_repeats_itself() {
        let f = front();
        let (out, frames) = f.apply_lfr(&[9.0, 8.0], 1, 2);
        assert_eq!(frames, 1);
        assert_eq!(out.len(), 14);
        assert!(out.chunks(2).all(|c| c == [9.0, 8.0]));
    }

    #[test]
    fn rejects_empty_and_too_short_audio() {
        let f = front();
        assert!(matches!(f.extract(&[]), Err(InferError::EmptyAudio)));
        assert!(matches!(
            f.extract(&[0.0; 399]),
            Err(InferError::EmptyAudio)
        ));
        assert!(f.extract(&[0.0; 400]).is_ok());
    }
}
