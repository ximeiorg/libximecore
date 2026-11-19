//! PCM 输入输出：WAV 解码与重采样（两族 ASR 共用）。
//!
//! 只放与模型无关的音频 I/O。各族的频谱前处理在各自的 `frontend` 模块里：
//! Qwen3-ASR 是 Whisper 式 log-mel，Fun-ASR-Nano 是 Kaldi fbank。

use std::path::Path;

use crate::InferError;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

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
