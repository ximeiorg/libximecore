# xime-infer

基于 [candle](https://github.com/huggingface/candle) 的本地模型推理。当前实现笔画手写识别
（StrokeTransformer），作为后续 LLM / 小模型的移植模板。

## 模型

以 [`ochwpro`](https://github.com/ximeiorg/ochwpro) 训练仓为准：

| 文件 | 说明 |
| --- | --- |
| `model_fp16.safetensors`（或 `model.safetensors`） | 权重，PyTorch 命名；fp16 会在装载时转 f32 计算 |
| `char_index.json` | `{"chars": [...]}`，下标即类别 id，必须与权重同版本 |

模型目录布局：

```text
<model_dir>/
├── model_fp16.safetensors
└── char_index.json
```

`model_fp16.safetensors` 是训练仓 `ochwpro/model_fp16.safetensors` 的原样拷贝；
`char_index.json` 在仓库根目录与 `ochw-wasm/` 各有一份（内容相同）。

## 用法

```rust
use xime_infer::{stroke::StrokeRecognizer, DeviceKind};

let device = DeviceKind::detect().build()?;           // Metal/CUDA 失败自动回退 CPU
let rec = StrokeRecognizer::from_dir(
    std::path::Path::new("/path/to/model"),
    &Default::default(),                              // StrokeConfig：7356 类 / d=192 / 3 层
    device,
)?;
let top = rec.predict(&[vec![(0.0, 0.0), (10.0, 0.0), (40.0, 0.0)]], 10)?;
for p in top { println!("{} {:.4}", p.ch, p.score); }
```

冒烟/对拍：

```bash
cargo run --release -p xime-infer --example stroke_predict -- /path/to/model 5 5
# 参数：模型目录 top_k 笔画点数
```

## 后端

| feature | 后端 | 说明 |
| --- | --- | --- |
| 默认 | CPU | 任何平台；手写模型足够 |
| `metal` | Apple Metal | 仅 macOS；需 Xcode CLT 编译 Metal kernel |
| `cuda` | NVIDIA CUDA | 需 nvcc；CPU/CUDA 两 flavor 用独立 target 目录 |
| `accelerate` | macOS CPU BLAS | 可选，小模型收益有限 |

## 实测（Apple M1，release，T=笔画点数）

| T | CPU p50 | Metal p50 | Metal 首次 |
| ---: | ---: | ---: | ---: |
| 5 | 1.44 ms | 1.86 ms | ~30 ms |
| 50 | 3.95 ms | 3.07 ms | ~17 ms |
| 200 | 10.14 ms | 6.66 ms | ~22 ms |

结论：短笔画 CPU 更快（Metal 的 command buffer 开销占主导）；长笔画 Metal 约 1.5×。
Metal 首次调用要建 pipeline，宿主必须在装载后**后台预热一次**，不能放在按键路径上。

## 移植注意（踩过的坑）

1. **权重是 fp16**：CPU 的 f16 算子支持不完整，统一用 `VarBuilder` 的 `DType::F32` 读入，
   由 candle 在装载时转换。直接按 f16 计算会在 CPU 上失败或精度异常。
2. **Metal 的 matmul 不接受非连续输入**：`permute` 后的 Q/K/V 必须 `.contiguous()`，
   否则报 `Invalid matmul arguments`。CPU 不报错，只有 Metal 暴露。
3. **Pre-Norm 残差加的是 norm 之前的输入**：`x + attn(norm1(x))`，不是
   `norm1(x) + attn(norm1(x))`。写错后模型仍能跑、输出形状也对，但结果全错——
   必须用真实权重和 ONNX 对拍才能发现。
4. **不要给单字补零到固定长度**：训练/导出用的是 padding mask；补零而不加 mask 会让
   注意力看到一堆零向量，top-1 直接从「一」变成「÷」。本 crate 的 `predict` 按真实
   笔画长度构图，不做补齐。

## 对拍

```bash
OCHWPRO_MODEL_DIR=/path/to/model \
  cargo test -p xime-infer --test parity -- --ignored
```

参考值来自 `ochwpro.onnx`（fp32）对同一「一」横的输出：`一 0.4154 / - 0.3076 / …`。
fp16 权重下 top-1 稳定为「一」（~0.42），前二类别下标一致。
