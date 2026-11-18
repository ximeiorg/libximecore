# xime-infer

基于 [candle](https://github.com/huggingface/candle) 的本地模型推理，当前有两个模型族：

| 模块 | 模型 | 用途 |
| --- | --- | --- |
| [`stroke`] | StrokeTransformer（1.72M） | 笔画手写识别 |
| [`asr`] | Qwen3-ASR-0.6B / 1.7B | 离线语音识别 |

[`stroke`]: src/stroke
[`asr`]: src/asr

设计边界：本 crate 只做**推理**（装载、前处理、解码），模型下载/校验/解压归宿主；
不依赖 librime；识别器由调用方 worker 线程独占（`transcribe` 需要 `&mut self`，内部有 KV cache）。

## 后端：按平台用 feature 区分

三个平台对应三个 flavor，构建时选 feature，运行期不需要猜：

| 平台 | 构建命令 | `DeviceKind::platform_default()` |
| --- | --- | --- |
| macOS | `--features metal`（Apple Silicon / Intel Mac GPU） | `Metal` |
| Windows / Linux（NVIDIA） | `--features cuda` | `Cuda` |
| 任意 | 不带 feature（CPU） | `Cpu` |
| macOS CPU 加速 | `--features accelerate` | `Cpu` |

```bash
# macOS
cargo build --release -p xime-infer --features metal
# Windows / Linux + NVIDIA（需 CUDA Toolkit，nvcc 在 PATH）
cargo build --release -p xime-infer --features cuda
# 纯 CPU
cargo build --release -p xime-infer
```

约束与语义：

- `metal` 与 `cuda` **互斥**；非 macOS 开 `metal`、macOS 开 `cuda`、非 macOS 开
  `accelerate` 都是**编译期错误**，不会拖到运行期。
- `DeviceKind::platform_default()`：编译期决定，不探测、不初始化；适合打包时就定
  flavor 的宿主。`DeviceKind::detect()`：运行期探测 GPU，失败**静默回退 CPU**
  （输入法不能因为驱动坏了不可用）。
- ASR 的默认后端跟着 feature 走：`cuda` 包用 `detect()`（成功即 CUDA，失败回退
  CPU），其余用 CPU（Metal 解码是 dispatch-bound，实测不比 CPU 快，却占 1.6 GB）。
- CUDA flavor 的**运行期依赖**（cudart / cuBLAS）要随安装包分发；与 CPU flavor
  不能共用同一份产物。
- 本仓库只在 Apple M1 上验证过 CPU 与 Metal；**CUDA 路径按 candle 的后端能力接入，
  未在本机编译/跑过**（macOS 无 nvcc）。CUDA 机器上先跑：

  ```bash
  XIME_ASR_MODEL_DIR=/path/to/Qwen3-ASR-0.6B XIME_ASR_DEVICE=cuda \
    cargo test --release -p xime-infer --features cuda \
      --test asr_parity -- --ignored --test-threads=1
  ```

### 打包两个 flavor（纯 CPU / GPU）

同一份源码，只差 feature；**不需要运行期开关**，也不会把 GPU 依赖带进 CPU 包。
实际只有三种组合：`CPU`、`CPU + metal`（macOS）、`CPU + cuda`（Windows/Linux）。

| 产物 | 命令 |
| --- | --- |
| 纯 CPU（macOS 建议带 BLAS） | `cargo build --release -p xime-infer --features accelerate` |
| CPU + Metal（macOS） | `cargo build --release -p xime-infer --features metal` |
| CPU + CUDA（Windows/Linux） | `cargo build --release -p xime-infer --features cuda` |

- CPU flavor 的依赖树里**没有** `cudarc` / `objc2-metal`：
  `cargo tree -p xime-infer | grep -Ei 'cudarc|metal'` 为空。
- GPU flavor 的 CUDA 产物要自带 cudart / cuBLAS，且不能与 CPU 产物混用。
- `DeviceKind::GPU_COMPILED` 是编译期常量，宿主可以据此显示「GPU 加速已启用」。

**宿主接入**：依赖写 `default-features = false`，打包脚本按平台从命令行打开依赖的
feature，**不用改宿主 Cargo.toml**：

```toml
[dependencies]
xime-infer = { path = "../libximecore/crates/xime-infer", default-features = false }
```

```bash
cargo build --release -p ximeyi                                  # 纯 CPU 包
cargo build --release -p ximeyi --features xime-infer/metal      # macOS GPU 包
cargo build --release -p ximeyi --features xime-infer/cuda       # Windows/Linux GPU 包
```

## 笔画手写识别

以 [`ochwpro`](https://github.com/ximeiorg/ochwpro) 训练仓为准：

| 文件 | 说明 |
| --- | --- |
| `model_fp16.safetensors`（或 `model.safetensors`） | 权重，PyTorch 命名；fp16 会在装载时转 f32 计算 |
| `char_index.json` | `{"chars": [...]}`，下标即类别 id，必须与权重同版本 |

```rust
use xime_infer::{stroke::StrokeRecognizer, DeviceKind};

let device = DeviceKind::detect().build()?;
let rec = StrokeRecognizer::from_dir(
    std::path::Path::new("/path/to/model"),
    &Default::default(),                              // StrokeConfig：7356 类 / d=192 / 3 层
    device,
)?;
let top = rec.predict(&[vec![(0.0, 0.0), (10.0, 0.0), (40.0, 0.0)]], 10)?;
```

```bash
cargo run --release -p xime-infer --example stroke_predict -- /path/to/model 5 5
# 参数：模型目录 top_k 笔画点数
```

实测（Apple M1，release，T=笔画点数）：

| T | CPU p50 | Metal p50 | Metal 首次 |
| ---: | ---: | ---: | ---: |
| 5 | 1.44 ms | 1.86 ms | ~30 ms |
| 50 | 3.95 ms | 3.07 ms | ~17 ms |
| 200 | 10.14 ms | 6.66 ms | ~22 ms |

短笔画 CPU 更快（Metal 的 command buffer 开销占主导）；长笔画 Metal 约 1.5×。
Metal 首次调用要建 pipeline，宿主必须装载后**后台预热**。

## 语音识别（Qwen3-ASR）

支持两种发布格式，按 `config.json` 自动识别：

| 格式 | 目录内容 | 权重前缀 |
| --- | --- | --- |
| `-hf`（transformers） | `config.json` + `tokenizer.json` + `model.safetensors` | `model.audio_tower` / `model.language_model` |
| ModelScope 原始 | 同上 + `vocab.json` + `merges.txt` | `thinker.audio_tower` / `thinker.model` |

```rust
use std::path::Path;
use xime_infer::asr::Qwen3Asr;

let mut asr = Qwen3Asr::from_dir(Path::new("/path/to/Qwen3-ASR-0.6B"))?;
asr.warmup()?;                                   // 后台线程里做，别放按键路径
let out = asr.transcribe_wav(Path::new("a.wav"))?;
println!("{:?} {}", out.language, out.text);     // Some("Chinese") 今天天气不错……
```

> **一个进程只允许一个实例**。每个实例常驻 1.6 GB（f16）/ 3.1 GB（f32）权重；
> 并发装载两个实例会在 macOS 上触发 `IOGPUGroupMemory` 内核断言（直接 panic 重启）。
> `from_dir_with` 用进程级闸门挡下第二个实例，返回 `InferError::AsrBusy`。
>
> **默认后端**：CPU 包 / Metal 包用 CPU（Metal 解码是逐 token 的 dispatch-bound，
> 并不比 CPU 快，却要占 1.6 GB 显存）；CUDA 包用 `detect()`，探测成功即 CUDA。
> 显式指定 GPU 时**只跑一个实例**。

也接受任意采样率的 WAV（内部混单声道 + rubato 重采样到 16 kHz），
或直接喂 16 kHz 单声道 PCM：`asr.transcribe(&pcm)`。

```bash
# 参数：模型目录 wav [语种]；XIME_ASR_DTYPE=f16|bf16|f32 可指定 dtype
cargo run --release -p xime-infer --example asr_transcribe -- /path/to/model a.wav
```

### 实测（Apple M1 16GB，release，CPU + accelerate，贪心解码）

| 音频 | 编码 | 解码 | 总计 | RT |
| --- | ---: | ---: | ---: | ---: |
| 11 s（jfk，英语） | 0.40 s | 3.8 s | 4.2 s | 0.38× |
| 3.3 s（TTS 中文） | 0.16 s | 1.2 s | 1.35 s | 0.41× |

- **编码**：早期单实例测量里 Metal 比 CPU 快 10–15 倍（40 ms vs 600 ms）。
- **解码**：逐 token 的 operator dispatch 是瓶颈，Metal 与 CPU 相当；开 `accelerate`
  是关键（不开时 CPU 解码 ~9 s，慢 3 倍）。
- 上表只列 CPU：Metal 路径在 2026-11-18 触发过 macOS 驱动 panic（见下），
  修复后未在本机复测，保留为可选能力。
- **内存**：f32 约 3.1 GB，f16 约 1.6 GB（权重 1.56 GB 全量常驻），且**单进程单实例**。
- 首次调用（尤其 Metal）要编译 kernel，`warmup()` 是 1 秒静音的编码 + prefill，
  不进入生成循环，不会在静音上产生幻觉。

### 边界

- **单次 ≤ 30 秒**（模型训练窗口），更长的音频由宿主切段；超限返回 `AudioTooLong`。
- **只有离线**：不做流式增量输出、不做时间戳（forced aligner 是另一个模型）。
- **1.7B** 走同一套代码（配置/分片自动识别），未实测。

### 与官方对拍

同一份权重、同一段音频，输出 token 级一致：
jfk（英语）`And so, my fellow Americans, ask not what your country can do for you; ask what you can do for your country.`；
TTS 中文「今天天气不错，我们出去走走吧。」。

中间量：mel 除最后一帧（补零边界）外最大误差 6e-6；音频塔输出 [143,1024]
最大误差 2.3e-3、均值 3.8e-5（bf16 权重转 f32 的累加顺序差异）。

```bash
XIME_ASR_MODEL_DIR=/path/to/model \
  cargo test --release -p xime-infer --features accelerate \
    --test asr_parity -- --ignored --test-threads=1
```

`target/asr-test/` 下放测试音频（`jfk.wav` 与 `say -v Tingting` 生成的 `zh.wav`/`zh48.wav`，
该目录被根 `.gitignore` 的 `/target` 覆盖，不入库）。

## 移植踩过的坑

1. **fp16 权重统一按 f32 装载**（笔画模型）：CPU 的 f16 算子支持不完整。ASR 则相反——
   Metal 上用 f16 更快且输出一致，CPU 上 f16 不可用（accelerate 不支持）。
2. **Metal 的 matmul 不接受非连续输入**：`permute` 后的 Q/K/V 必须 `.contiguous()`。
3. **Pre-Norm 残差加的是 norm 之前的输入**（笔画模型）：写错后模型仍能跑、形状也对，
   但结果全错，必须用真实权重对拍。
4. **不要给单字补零到固定长度**（笔画模型）：没有 mask 时注意力会看到零向量。
5. **ASR 的卷积按 100 帧一个 chunk 独立做**：等价于官方 `view(batch, chunks, ...)`，
   三次 stride-2 后每个 chunk 固定 13 个 token；对总帧数连做三次取整会算成 138 而不是 143。
6. **每个 chunk 复用同一张 13 行的正弦位置编码**，不是跨 chunk 递增。
7. **注意力窗口 104 token**（13 × 8），窗口内双向、窗口间互不可见。
8. **mel 补到整秒而不是 30 秒**：卷积在尾部看到的同样是零帧，保留下来的 token 与官方
   补零到 30 s 一致，但省掉 3 倍卷积。
9. **Qwen3 的 `head_dim` 不等于 `hidden/heads`**：0.6B 是 128 vs 64，`o_proj` 前 reshape
   必须用 `heads*head_dim`，用 `hidden_size` 会在 decode 第一步报 shape mismatch。
10. **mask / 位置编码的 dtype 要跟随激活**：f16 推理时混入 f32 mask 会直接报错。
11. **不要并发装载多个实例**：`cargo test` 默认多线程，每测试各装一份 1.6 GB 模型，
    4 个测试并行就是 6 GB GPU 内存，macOS 会在 `IOGPUGroupMemory.cpp` 断言 panic。
    测试必须 `--test-threads=1` 且共享同一实例；库内用进程级闸门兜底。

## 对拍（笔画模型）

```bash
OCHWPRO_MODEL_DIR=/path/to/model \
  cargo test -p xime-infer --test parity -- --ignored
```

参考值来自 `ochwpro.onnx`（fp32）对同一「一」横的输出：`一 0.4154 / - 0.3076 / …`。
fp16 权重下 top-1 稳定为「一」（~0.42），前二类别下标一致。
