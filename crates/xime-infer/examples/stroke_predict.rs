//! 冒烟/对拍示例：装载真实模型，识别一条「一」横，打印 top-k 与稳态延迟。
//!
//! 用法：
//! ```text
//! cargo run --release -p xime-infer --example stroke_predict -- <模型目录> [top_k] [points]
//! ```
//! 模型目录需含 `model_fp16.safetensors`（或 `model.safetensors`）与 `char_index.json`。
//! `points` 用于加长笔画、量长序列开销（默认 5）。

use std::path::Path;

use xime_infer::{stroke::StrokeRecognizer, DeviceKind};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args()
        .nth(1)
        .expect("用法：stroke_predict <模型目录> [top_k] [points]");
    let top_k = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(5);
    let points = std::env::args()
        .nth(3)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(5)
        .max(2);

    let kind = DeviceKind::detect();
    let recognizer =
        StrokeRecognizer::from_dir(Path::new(&dir), &Default::default(), kind.build()?)?;

    // 一条水平笔画；默认 5 点，期望 top-1 = "一"（ONNX 参考：0.4154）。
    let strokes = vec![(0..points)
        .map(|i| (i as f32 * 10.0, 0.0))
        .collect::<Vec<_>>()];

    let started = std::time::Instant::now();
    let out = recognizer.predict(&strokes, top_k)?;
    let first = started.elapsed();

    // Metal 首次调用可能编译 pipeline，必须预热后再量稳态。
    let mut times = Vec::new();
    for _ in 0..20 {
        let t = std::time::Instant::now();
        let _ = recognizer.predict(&strokes, top_k)?;
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!(
        "device={:?} classes={} points={} first={:?} p50={:.2}ms min={:.2}ms",
        recognizer.device(),
        recognizer.num_classes(),
        points,
        first,
        times[10],
        times[0]
    );
    for p in out {
        println!("{}\t{:.4}\t{}", p.ch, p.score, p.index);
    }
    Ok(())
}
