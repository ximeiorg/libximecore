//! 与 ONNX Runtime 的对拍（需要真实模型，默认忽略）。
//!
//! 运行：
//! ```text
//! OCHWPRO_MODEL_DIR=/path/to/ochwpro cargo test -p xime-infer --test parity -- --ignored
//! ```
//! 目录需含 `model_fp16.safetensors`（或 `model.safetensors`）与 `char_index.json`。
//! 参考值来自 `ochwpro.onnx`（fp32）对同一「一」横的输出：
//! `一 0.4154 / - 0.3076 / 、 0.0222 / ` 0.0212 / 1 0.0207`，
//! 类别下标 `[2912, 132, 170, 85, 135]`。

use std::path::Path;

use xime_infer::{stroke::StrokeRecognizer, DeviceKind};

#[test]
#[ignore = "需要 OCHWPRO_MODEL_DIR 指向真实模型目录"]
fn horizontal_stroke_matches_onnx_reference() {
    let dir = std::env::var("OCHWPRO_MODEL_DIR").expect("设置 OCHWPRO_MODEL_DIR");
    let device = DeviceKind::Cpu.build().unwrap();
    let recognizer =
        StrokeRecognizer::from_dir(Path::new(&dir), &Default::default(), device).unwrap();

    let strokes = vec![vec![
        (0.0, 0.0),
        (10.0, 0.0),
        (20.0, 0.0),
        (30.0, 0.0),
        (40.0, 0.0),
    ]];
    let out = recognizer.predict(&strokes, 5).unwrap();

    assert_eq!(out.len(), 5);
    assert_eq!(out[0].ch, "一", "top1={:?}", out[0]);
    // fp16 权重相对 ONNX fp32 的偏差在 ~1% 量级；用宽容差只锁语义。
    assert!(
        (0.38..0.46).contains(&out[0].score),
        "score={}",
        out[0].score
    );
    assert_eq!(
        &out[..2].iter().map(|p| p.index).collect::<Vec<_>>(),
        &[2912, 132]
    );
}
