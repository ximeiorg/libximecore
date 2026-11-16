//! 笔画轨迹 → 模型输入特征。
//!
//! 与训练仓 `ochwpro/dataset.py::strokes_to_sequence` 逐行对齐：
//! 展平所有笔画的点，按**整字**包围盒归一化，5 维特征
//! `[x_norm, y_norm, dx, dy, pen_down]`，每笔最后一点 `pen_down = 0`。

use candle_core::{Device, Tensor};

use crate::InferError;

/// 把笔画序列转成扁平特征与序列长度。
///
/// 空输入返回一条全零特征（长度 1），形状仍是 `(1, 1, 5)`——是否拒绝空输入
/// 由上层决定（`StrokeRecognizer::predict` 会返回 [`InferError::EmptyInput`]）。
pub fn strokes_to_features(strokes: &[Vec<(f32, f32)>]) -> (Vec<f32>, usize) {
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    let mut pen_down = Vec::new();

    for stroke in strokes {
        for &(x, y) in stroke {
            xs.push(x);
            ys.push(y);
            pen_down.push(1.0f32);
        }
        if let Some(last) = pen_down.last_mut() {
            *last = 0.0; // 每笔最后一个点为抬笔
        }
    }

    let n = xs.len();
    if n == 0 {
        return (vec![0.0; 5], 1);
    }

    let min_x = xs.iter().copied().fold(f32::INFINITY, f32::min);
    let max_x = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let min_y = ys.iter().copied().fold(f32::INFINITY, f32::min);
    let max_y = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);

    // 单点/水平线/垂直线时避免除零；与训练代码一致用 max(…, 1.0)。
    let range_x = (max_x - min_x).max(1.0);
    let range_y = (max_y - min_y).max(1.0);

    let mut features = Vec::with_capacity(n * 5);
    let (mut prev_x, mut prev_y) = (xs[0], ys[0]);
    for i in 0..n {
        features.push((xs[i] - min_x) / range_x);
        features.push((ys[i] - min_y) / range_y);
        features.push((xs[i] - prev_x) / range_x);
        features.push((ys[i] - prev_y) / range_y);
        features.push(pen_down[i]);
        prev_x = xs[i];
        prev_y = ys[i];
    }

    (features, n)
}

/// 特征 → `(1, T, 5)` 张量，返回张量与序列长度。
pub fn strokes_to_tensor(
    strokes: &[Vec<(f32, f32)>],
    device: &Device,
) -> Result<(Tensor, usize), InferError> {
    let (features, seq_len) = strokes_to_features(strokes);
    let tensor = Tensor::from_vec(features, (1, seq_len, 5), device)?;
    Ok((tensor, seq_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_stroke_shape_and_pen_down() {
        let strokes = vec![vec![(0.0, 0.0), (100.0, 100.0), (200.0, 50.0)]];
        let (f, n) = strokes_to_features(&strokes);
        assert_eq!(n, 3);
        assert_eq!(f.len(), 15);
        // 每笔最后一点抬笔
        assert_eq!(f[4], 1.0);
        assert_eq!(f[9], 1.0);
        assert_eq!(f[14], 0.0);
    }

    #[test]
    fn multiple_strokes_pen_down_boundaries() {
        let strokes = vec![
            vec![(0.0, 0.0), (100.0, 0.0)],
            vec![(0.0, 100.0), (100.0, 100.0)],
        ];
        let (f, n) = strokes_to_features(&strokes);
        assert_eq!(n, 4);
        // 第 1 笔末点 = 抬笔，第 2 笔首点 = 落笔
        assert_eq!(f[9], 0.0);
        assert_eq!(f[14], 1.0);
        assert_eq!(f[19], 0.0);
    }

    #[test]
    fn normalization_is_within_unit_range() {
        let strokes = vec![vec![(10.0, 20.0), (30.0, 60.0)]];
        let (f, _) = strokes_to_features(&strokes);
        assert!((f[0] - 0.0).abs() < 1e-6); // min x
        assert!((f[5] - 1.0).abs() < 1e-6); // max x
        assert!((f[1] - 0.0).abs() < 1e-6); // min y
        assert!((f[6] - 1.0).abs() < 1e-6); // max y
        assert!((f[2] - 0.0).abs() < 1e-6); // 首点 dx = 0
        assert!((f[3] - 0.0).abs() < 1e-6); // 首点 dy = 0
    }

    #[test]
    fn single_point_does_not_divide_by_zero() {
        let strokes = vec![vec![(10.0, 20.0)]];
        let (f, n) = strokes_to_features(&strokes);
        assert_eq!(n, 1);
        assert!(f.iter().all(|v| v.is_finite()));
        assert_eq!(f[4], 0.0);
    }

    #[test]
    fn empty_input_yields_one_zero_step() {
        let (f, n) = strokes_to_features(&[]);
        assert_eq!(n, 1);
        assert_eq!(f, vec![0.0; 5]);
    }

    #[test]
    fn tensor_shape_matches_seq_len() {
        let dev = Device::Cpu;
        let strokes = vec![vec![(0.0, 0.0), (1.0, 1.0)]];
        let (t, n) = strokes_to_tensor(&strokes, &dev).unwrap();
        assert_eq!(n, 2);
        assert_eq!(t.dims(), &[1, 2, 5]);
    }
}
