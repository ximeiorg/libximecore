//! 笔画手写识别（StrokeTransformer）。
//!
//! 用法：
//! ```text
//! use xime_infer::{stroke::StrokeRecognizer, DeviceKind};
//!
//! # fn main() -> Result<(), xime_infer::InferError> {
//! let device = DeviceKind::detect().build()?;
//! let recognizer = StrokeRecognizer::from_dir(
//!     std::path::Path::new("/path/to/model"),
//!     &Default::default(),
//!     device,
//! )?;
//! let top = recognizer.predict(
//!     &[vec![(0.0, 0.0), (10.0, 10.0), (20.0, 5.0)]],
//!     10,
//! )?;
//! # let _ = top;
//! # Ok(())
//! # }
//! ```

mod labels;
mod model;
mod preprocess;

pub use model::StrokeConfig;
pub use preprocess::{strokes_to_features, strokes_to_tensor};

use std::path::Path;

use candle_core::Device;
use serde::Serialize;

use crate::InferError;

/// 一个候选字。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Prediction {
    /// 识别出的字符。
    pub ch: String,
    /// softmax 概率。
    pub score: f32,
    /// 类别下标（与 `char_index.json` 顺序一致，便于调试/对齐）。
    pub index: usize,
}

/// 笔画识别器：装载一次，predict 可反复调用。
///
/// 非 `Sync` 语义使用：与 `xime-speech` 的识别器一致，由调用方 worker 线程独占。
pub struct StrokeRecognizer {
    model: model::StrokeTransformer,
    labels: Vec<String>,
    device: Device,
}

impl StrokeRecognizer {
    /// 从 safetensors 字节 + 字符表 JSON 装载。
    pub fn from_bytes(
        weights: &[u8],
        char_index_json: &str,
        config: &StrokeConfig,
        device: Device,
    ) -> Result<Self, InferError> {
        let labels = labels::parse_char_index(char_index_json)?;
        let model = model::load(weights, config, &device)?;
        assemble(model, labels, device)
    }

    /// 从模型目录装载：优先 `model.safetensors`，回退 `model_fp16.safetensors`；
    /// 字符表固定为 `char_index.json`。
    pub fn from_dir(dir: &Path, config: &StrokeConfig, device: Device) -> Result<Self, InferError> {
        let weights = read_first(dir, &["model.safetensors", "model_fp16.safetensors"])?;
        let json = read_first(dir, &["char_index.json"])?;
        let json = String::from_utf8(json)
            .map_err(|e| InferError::Text(format!("char_index.json：{e}")))?;
        Self::from_bytes(&weights, &json, config, device)
    }

    /// 识别一组笔画，返回 top-k 候选（按概率降序）。
    ///
    /// `strokes` 是每笔的点坐标；坐标单位任意（内部按整字包围盒归一化）。
    pub fn predict(
        &self,
        strokes: &[Vec<(f32, f32)>],
        top_k: usize,
    ) -> Result<Vec<Prediction>, InferError> {
        if strokes.iter().all(|s| s.is_empty()) {
            return Err(InferError::EmptyInput);
        }
        if top_k == 0 {
            return Ok(Vec::new());
        }

        let (input, _seq_len) = preprocess::strokes_to_tensor(strokes, &self.device)?;
        let logits = self.model.forward(&input)?;
        let probs = candle_nn::ops::softmax(&logits, 1)?;

        let mut ranked: Vec<(usize, f32)> = probs
            .squeeze(0)?
            .to_vec1::<f32>()?
            .into_iter()
            .enumerate()
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(ranked
            .into_iter()
            .take(top_k.min(self.labels.len()))
            .map(|(index, score)| Prediction {
                ch: self.labels[index].clone(),
                score,
                index,
            })
            .collect())
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn num_classes(&self) -> usize {
        self.model.num_classes()
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }
}

/// 校验标签与模型类别数一致后组装。
fn assemble(
    model: model::StrokeTransformer,
    labels: Vec<String>,
    device: Device,
) -> Result<StrokeRecognizer, InferError> {
    if labels.len() != model.num_classes() {
        return Err(InferError::LabelCount {
            actual: labels.len(),
            expected: model.num_classes(),
        });
    }
    Ok(StrokeRecognizer {
        model,
        labels,
        device,
    })
}

fn read_first(dir: &Path, names: &[&str]) -> Result<Vec<u8>, InferError> {
    for name in names {
        let path = dir.join(name);
        if path.is_file() {
            return std::fs::read(&path).map_err(|source| InferError::Io {
                path: path.display().to_string(),
                source,
            });
        }
    }
    Err(InferError::MissingFile(
        names
            .iter()
            .map(|n| dir.join(n).display().to_string())
            .collect::<Vec<_>>()
            .join(" / "),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::DType;
    use candle_nn::VarBuilder;

    fn zero_recognizer(n: usize) -> StrokeRecognizer {
        let dev = Device::Cpu;
        let vb = VarBuilder::zeros(DType::F32, &dev);
        let cfg = StrokeConfig {
            num_classes: n,
            ..Default::default()
        };
        let model = model::StrokeTransformer::load_with_vb(vb, &cfg).unwrap();
        let labels = (0..n).map(|i| format!("字{i}")).collect();
        assemble(model, labels, Device::Cpu).unwrap()
    }

    #[test]
    fn predict_returns_top_k() {
        let r = zero_recognizer(7356);
        let strokes = vec![vec![(0.0, 0.0), (10.0, 10.0), (20.0, 5.0)]];
        let out = r.predict(&strokes, 5).unwrap();
        assert_eq!(out.len(), 5);
        assert!(out.iter().all(|p| (0.0..=1.0).contains(&p.score)));
        // 零权重下 softmax 均匀，概率和≈1
        let sum: f32 = r
            .predict(&strokes, 7356)
            .unwrap()
            .iter()
            .map(|p| p.score)
            .sum();
        assert!((sum - 1.0).abs() < 1e-3);
    }

    #[test]
    fn predict_top_k_zero_and_oversized() {
        let r = zero_recognizer(16);
        let strokes = vec![vec![(0.0, 0.0), (1.0, 1.0)]];
        assert!(r.predict(&strokes, 0).unwrap().is_empty());
        assert_eq!(r.predict(&strokes, 999).unwrap().len(), 16);
    }

    #[test]
    fn predict_rejects_empty_strokes() {
        let r = zero_recognizer(16);
        assert!(matches!(r.predict(&[], 5), Err(InferError::EmptyInput)));
        assert!(matches!(
            r.predict(&[vec![]], 5),
            Err(InferError::EmptyInput)
        ));
    }

    #[test]
    fn label_count_mismatch_is_rejected() {
        // 标签数与类别数不一致时，必须在装载阶段失败，而不是运行期越界。
        let labels = vec!["一".to_string(), "乙".to_string()];
        let cfg = StrokeConfig {
            num_classes: 3,
            ..Default::default()
        };
        let vb = VarBuilder::zeros(DType::F32, &Device::Cpu);
        let model = model::StrokeTransformer::load_with_vb(vb, &cfg).unwrap();
        assert!(matches!(
            assemble(model, labels, Device::Cpu),
            Err(InferError::LabelCount {
                actual: 2,
                expected: 3
            })
        ));
    }
}
