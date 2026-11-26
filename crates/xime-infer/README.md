# xime-infer

基于 [candle](https://github.com/huggingface/candle) 的本地模型推理，当前有三个模型族：

| 模块 | 模型 | 用途 |
| --- | --- | --- |
| [`stroke`] | StrokeTransformer（1.72M） | 笔画手写识别 |
| [`asr::qwen3_asr`] | Qwen3-ASR-0.6B / 1.7B | 离线语音识别（支持按秒分块编码） |
| [`asr::fun_asr`] | Fun-ASR-Nano-2512（800M） | 离线语音识别（中文方言更强，只能整段） |
| [`asr::whisper`] | Whisper small（244M）/ large-v3-turbo（809M） | 离线语音识别 + 翻译（80+ 语种，只能整段） |

[`stroke`]: src/stroke
[`asr::qwen3_asr`]: src/asr/qwen3_asr
[`asr::fun_asr`]: src/asr/fun_asr
[`asr::whisper`]: src/asr/whisper

三个语音模型族**各自一套代码**（config / 前处理 / 编码器 / 文本解码器 / 分词器都在自己的
目录里），只在 `asr::AsrOptions`、`asr::Transcript`、家族识别、单实例闸门、WAV I/O 这些
与模型无关的部分共用。前处理与 prompt 约定差异太大（log-mel vs fbank、
`<|audio_pad|>` vs 纯文本指令 vs `<|startoftranscript|>`），共享实现只会让改一边必须
验证另一边。

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
- 本仓库的 CPU 与 Metal 在 Apple M1 上验证；**CUDA 路径已在 Linux + RTX 4060 Ti
  （CUDA 13.4 / cudarc 0.19）上编译并跑过真实权重**，输出 token 与 CPU 逐位相同。
  CUDA 机器上自检：

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

> **一个进程只允许一个 ASR 实例**（两个族共用同一道闸门）。每个实例常驻 1.6 GB（f16）
> / 3.1 GB（f32）权重；并发装载两个实例会在 macOS 上触发 `IOGPUGroupMemory` 内核断言
> （直接 panic 重启）。`from_dir_with` 用进程级闸门挡下第二个实例，返回
> `InferError::AsrBusy`。换模型要走「先卸载、再装载」，服务层的 `select()` 就是这么做的。
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

## 语音识别（Fun-ASR-Nano）

第二族，代码在 `src/asr/fun_asr/`，与 Qwen3-ASR **零共享**。上游只有 FunASR 格式的
`model.pt`，本 crate 吃 transformers 布局的 safetensors，所以模型目录要先转换：

```bash
# 依赖 torch + safetensors；--self-test 只验键名映射，不读权重
python3 tools/convert_fun_asr_nano.py \
  --model_path /path/to/Fun-ASR-Nano-2512 \
  --output_path  ~/.config/xime/models/fun-asr-nano
```

源目录按 ModelScope 原始仓库（`FunAudioLLM/Fun-ASR-Nano-2512`）摆放：`config.yaml` +
`model.pt` + `Qwen3-0.6B/`（后者只有分词器与 config，**没有**权重——LLM 权重就在
`model.pt` 的 `llm.*` 里）。脚本做的事：

| 产物 | 来源 |
| --- | --- |
| `model.safetensors` | `model.pt` 的 1627 个张量（fused QKV 按行切三份，CTC 旁支 86 个丢弃） |
| `config.json` | 由 `config.yaml` 生成（`audio_config` / `adaptor_config` / `text_config`） |
| `processor_config.json` | 由 `frontend_conf` 生成（25/10 ms、80 mel、LFR 7/6） |
| `tokenizer.json` | 复制自 `Qwen3-0.6B/` |

```rust
use std::path::Path;
use xime_infer::asr::fun_asr::FunAsrNano;

let mut asr = FunAsrNano::from_dir(Path::new("/path/to/fun-asr-nano"))?;
asr.warmup()?;                                    // 同样别放按键路径
let out = asr.transcribe_wav(Path::new("a.wav"))?;
println!("{}", out.text);                         // 纯文本，语种由 prompt 指定
```

与 Qwen3-ASR 的差别（选型看这张表）：

| | Qwen3-ASR | Fun-ASR-Nano |
| --- | --- | --- |
| 前处理 | Whisper 式 128 维 log-mel | Kaldi fbank 80 维 + LFR(7/6) → 560 维 |
| 音频编码 | 18 层窗口注意力（104 token 窗口） | 70 层整段双向注意力（SAN-M + FSMN） |
| 输出 | `language X<asr_text>文本` | 纯文本（语种写进「语音转写成中文：」） |
| 流式 | 支持（按 1 秒分块编码） | **不支持** |
| 音频 token | 13 / 秒 | 16.7 / 秒（60 ms 一帧，不做降采样） |

**为什么不支持流式**：LFR 要跨帧拼（叠 7 跳 6），分块边界会截断窗口；注意力又是整段
双向的，改一段就全变。要流式请继续用 Qwen3-ASR——服务层 `engine::load` 按
`config.json` 自动派发，`FunAsrRecognizer` 的 `supports_streaming()` 返回 `false`，
流式路径会直接跳过。

边界：单次 ≤ 30 秒；位置编码表 2049 帧（约 123 秒）；输出不做时间戳。

### 与官方对拍

**已用真实权重验证过**：ModelScope `FunAudioLLM/Fun-ASR-Nano-2512` 的 `model.pt`
（2,127,426,538 字节）转出 1541 个张量 / 985M 参数，再用 transformers 的
`FunAsrNanoForConditionalGeneration` 吃**同一份 safetensors** 逐层比：

```bash
# 1) Rust 侧导出中间量（前处理特征 / 编码器输出 / prompt id / 生成 id）
XIME_ASR_DUMP=/tmp/dump XIME_ASR_DUMP_IDS=/tmp/dump/ids.txt \
  cargo run --release -p xime-infer --example fun_asr_transcribe \
  -- ~/.config/xime/models/fun-asr-nano-2512 example/zh16k.wav 中文

# 2) 参考实现三段对拍（需要 pip install torch safetensors transformers）
python3 tools/fun_asr_reference.py ~/.config/xime/models/fun-asr-nano-2512 \
  example/zh16k.wav --dump /tmp/dump
```

官方 `example/zh.mp3`（5.6 s）与 `en.mp3`（7.2 s）的结果：

| 层 | 指标 | 中文 | 英文 |
| --- | --- | --- | --- |
| 前处理（fbank+LFR） | 相对 RMS | 1.0e-3 | 1.2e-3 |
| 编码器 + 投影 | 相对 RMS / 余弦 | 3.0e-4 / 1.000000 | 7.1e-4 / 1.000000 |
| 贪心解码 | token id | 11/11 逐位相同 | 20/20 逐位相同 |
| 文本 | | `开饭时间早上九点至下午五点。` | `The tribal chieftain called for the boy, and presented him with fifty pieces of gold.` |

- 前处理的残差集中在能量贴近 `f32::EPSILON` 的静音 bin（占 0.2%，log 域把 1e-8 级
  舍入放大成 0.5），不是公式差异；编码器输出与 token 序列都不受影响。
- 「开饭」不是错字：官方 README 的示例传了热词 `开放时间`，不给提示时模型自己选的
  就是「开饭」，参考实现逐位相同。本实现暂无热词通道，走的是同一条 prompt。
- 编码器输出相对 RMS 3e-4 来自 bf16 权重（8 bit 尾数）；CPU 上按 f32 计算不影响。

回归用 `fun_asr_parity`（5 条，`#[ignore]`，同一份权重跑过全部通过）：音频 token 数
必须等于 `ceil(mel帧数 / 6)`、输出不含 `<asr_text>` 标记、30 秒上限与空音频按预期报错、
第二个实例被闸门挡下。服务层的派发另有 `xime-speech-service/tests/engine_smoke.rs`
（同样 `#[ignore]`）验证「模型目录 → `engine::load` → 文本」。

### 实测（release，f32，贪心解码）

| 后端 | 音频 | 装载 | 预热 | 编码 | 解码 | 合计 / 实时率 |
| --- | --- | --- | --- | --- | --- | --- |
| CPU（Ryzen 5 7600X，6 核） | 中文 5.6 s | 1.5 s | 7.4 s | 7.3 s | 2.0 s | 9.3 s / **1.66×** |
| CPU | 英文 7.2 s | — | — | 7.3 s | 3.2 s | 10.5 s / **1.47×** |
| CUDA（RTX 4060 Ti 16G，CUDA 13.4） | 中文 5.6 s | 0.45 s | 0.8 s | 0.66 s | 0.15 s | 0.81 s / **0.14×** |

编码是大头（70 层 × 16.7 token/秒、整段双向注意力，CPU 上约 42 GFLOP；GPU 上同一个
序列只要 0.66 s）。整段识别的使用姿势是「松手后上屏」：CPU 上 1.5–1.7 倍实时勉强可用，
GPU 上 7 倍实时很宽裕。CUDA 路径与 CPU 路径输出的 token 序列逐位相同。

实现上的对齐要点（都已在代码注释里写明）：

- 编码器第一层输入 560 维 ≠ hidden 512，所以**两处跳连都不加**；只有第 50、70 层末尾
  接 `final_layernorm`。
- 位置编码宽度是 **560**（不是 512），`sin` 在前半、`cos` 在后半，位置从 1 起算；
  特征先乘 `sqrt(512)` 再加位置。
- FSMN 跑在**没按头切开**的 `v` 上：`depthwise_conv(pad(v)) + v`，核 11、左右各补 5。
- fbank 不乘 32768（官方 FunASR 前端乘、HF 前端不乘）：log 域整体平移会在第一层
  LayerNorm 减均值时消掉。

## 语音识别（Whisper）

第三族，代码在 `src/asr/whisper/`，与前两族**零共享**。模型目录用 transformers 布局
（ModelScope 的 `openai-mirror/whisper-small`、`openai-mirror/whisper-large-v3-turbo`
就是这种）：

```text
config.json              ← 结构超参（d_model / 层数 / num_mel_bins）
generation_config.json   ← 语种表、task、<|notimestamps|>、抑制表
tokenizer.json           ← byte-level BPE + 全部特殊 token
model.safetensors        ← transformers 权重（proj_out 与 embed_tokens 绑定，不单独存）
```

```rust
use std::path::Path;
use xime_infer::asr::whisper::Whisper;

let mut asr = Whisper::from_dir(Path::new("/path/to/whisper-small"))?;
asr.set_language(Some("zh".into()));      // None / "auto" = 自动判语种
asr.set_task("transcribe")?;              // 或 "translate"（译成英文）
asr.warmup()?;                            // 固定 30 s 编码器，别放按键路径
let out = asr.transcribe_wav(Path::new("a.wav"))?;
println!("[{:?}] {}", out.language, out.text);
```

```bash
# 参数：模型目录 wav [语种]；XIME_ASR_TASK=translate 走翻译
cargo run --release -p xime-infer --example whisper_transcribe -- /path/to/model a.wav zh
```

同一套代码吃整个 Whisper 家族：`d_model` / 层数 / mel 维度 / 位置表长度全部读
`config.json`，特殊 token 全部从 `generation_config.json` + `tokenizer.json` 解析。

| | small | large-v3-turbo |
| --- | --- | --- |
| 参数 / 权重 | 244M / 967 MB f32 | 809M / 1.6 GB **f16** |
| mel / 编码器 / 解码器层 | 80 / 12 / 12 | 128 / 32 / 4 |
| 语种数 | 99 | 100（多 `<|yue|>`） |
| `<|transcribe|>` / `<|notimestamps|>` | 50359 / 50363 | 50360 / 50364 |

> 最后一行是重点：large-v3 因为多一个语种，`<|translate|>` / `<|transcribe|>` /
> `<|notimestamps|>` 全部 +1。**特殊 token 的 id 不能写死**，否则换一代模型「能跑但
> 输出全错」。

### 管线与关键取舍

```text
PCM(16 kHz) → log-mel(80/128) 补满 30 s → [mel, 3000]
  → conv1(k3) + GELU → conv2(k3,s2) + GELU → [d, 1500]
  → + 正弦位置表（权重里存着，不重算）→ N 层双向 transformer → 交叉注意力 K/V

prompt: <|startoftranscript|> <|语种|> <|transcribe|> <|notimestamps|>
  → 学习位置表 + 因果自注意力 + 交叉注意力（贪心，带抑制表）
  → 纯文本，遇 <|endoftext|> 停
```

- **必须补满 30 s**，不能像 Qwen3-ASR 那样「补到整秒」：编码器是整段双向注意力，补出来
  的静音帧会作为 key/value 参与真实帧的注意力；位置表也固定 1500 格。官方实现、
  whisper.cpp、faster-whisper 都是补满 30 s。
- **输出投影与 `embed_tokens` 共享权重**。HF 的 `proj_out` 是绑定权重，safetensors 里
  没有独立张量；自己实现时漏了就等于随机初始化 66M 参数。
- **key 投影没有 bias**（`k_proj` 只有 weight，q/v/out 都有 bias）——这是 OpenAI 原版
  `MultiHeadAttention` 的约定，按「所有 Linear 都有 bias」装载会直接报缺张量。
- **交叉注意力的 K/V 只算一次**：同一个 30 s 窗口内编码器输出是常量，prefill 时投影好
  缓存，逐 token 解码不再重算（官方实现每步都重算 1500 个位置，纯浪费）。
- 语种自动判定：只用 `[<|startoftranscript|>]` 预填一次，在语种 token 上取 argmax
  （与 transformers `detect_language` 同做法），然后再拿判定的语种拼正式 prompt。
- 抑制表：`suppress_tokens` 每步压、`begin_suppress_tokens` 只在第一步压；另外把
  `<|notimestamps|>` 之后的**全部时间戳 token** 压掉（对齐官方
  `without_timestamps=True`）。
- **不支持流式**：交叉注意力要看整段音频，改一个窗口就得重跑编码器（CPU 上 small 1.4 s /
  turbo 7 s），做不了逐字预览。`WhisperRecognizer::supports_streaming()` 返回 `false`，
  服务层录音期间不出预览、松手后一次上屏。

### 边界

- 单次 ≤ 30 秒（与另两族一致），更长的音频由宿主切段。
- 不做时间戳、不做说话人分离；要流式请用 Qwen3-ASR。
- **turbo 的翻译能力明显弱于 small**（OpenAI 的已知取舍）：中文语音 + `translate` 时
  turbo 会原样输出中文，small 能译成英文。这不是实现问题，transformers 参考实现同样如此。

### 与官方对拍

同一份权重、同一段音频，与 transformers `WhisperForConditionalGeneration` 逐层比：

```bash
# 1) Rust 侧导出 log-mel / 编码器输出 / prompt id / 生成 id
XIME_ASR_DUMP=/tmp/wd XIME_ASR_DUMP_IDS=/tmp/wd/ids.txt \
  cargo run --release -p xime-infer --example whisper_transcribe -- <模型目录> a.wav en

# 2) 参考实现（需要 pip install torch transformers）
python3 tools/whisper_reference.py <模型目录> a.wav --dump /tmp/wd --language en
```

| 层 | small（jfk 11 s / 中文 3.3 s） | large-v3-turbo（jfk 11 s） |
| --- | --- | --- |
| 前处理 log-mel | 相对 RMS 6.3e-7 / 3.7e-7 | 1.8e-6（滤波器现场算） |
| 编码器输出 | 6.9e-6 / 6.9e-6（余弦 1.000000） | 6.9e-4（f16 权重的累加顺序差） |
| 贪心 token | 25/25、11/11 逐位相同 | 26/26 逐位相同 |
| 文本 | 与参考逐字相同 | 与参考逐字相同 |

- small 的 mel 还能与 `preprocessor_config.json` 里存着的 librosa 滤波器组逐点对：
  80×201 最大误差 < 2e-6（`mel_filters_match_stored_reference`）。turbo 这份配置
  （transformers 4.46+）已经删掉 `mel_filters`，改成现场算，所以只能比 mel 输出。
- 回归用 `whisper_parity`（7 条，`#[ignore]`，small 与 turbo 都跑过）：
  语种检测、强制语种、非法语种报错、30 s 上限、空音频、第二实例被闸门挡下、
  mel 滤波器对拍。

```bash
XIME_WHISPER_MODEL_DIR=/path/to/whisper-small \
  cargo test --release -p xime-infer --test whisper_parity -- --ignored --test-threads=1
```

### 实测（release，f32，贪心解码，Ryzen 5 7600X 6 核 / 12 线程）

| 模型 | 音频 | 装载 | 预热 | 编码 | 解码 | 合计 / 实时率 |
| --- | --- | --- | --- | --- | --- | --- |
| small | 英文 11 s | 0.5 s（缓存热） | 1.5 s | 1.33 s | 1.63 s | 2.96 s / **0.27×** |
| small | 中文 3.3 s | — | — | 1.37 s | 0.76 s | 2.13 s / **0.65×** |
| large-v3-turbo | 英文 11 s | 1.9 s（缓存热） | 7.2 s | 7.1 s | 1.04 s | 8.1 s / **0.74×** |
| large-v3-turbo | 中文 3.3 s | — | — | 7.15 s | 0.44 s | 7.6 s / **2.33×** |

- **编码是大头**：turbo 32 层 × 1500 位置 × 20 头的整段注意力约 190 GFLOP，CPU 上
  7 s；GPU（`--features cuda`）会快一个量级，路径与另两族共用 `DeviceKind`。
- 编码固定 30 s：3 秒音频也要付 7 s 的编码成本，所以「松手后上屏」才划算，别做逐秒预览。
- 解码那一列是整句 25/26（或 10/11）个 token 的总和（turbo 只有 4 层，但 lm_head 是
  51866×1280）。自动判语种会多一次 `[sot]` 预填，随后的正式 prompt 靠
  `continue_prefill` 续写，**不重算** 1500 个位置的交叉注意力 K/V（turbo 上省约 2 s）。
- 装载耗时受磁盘影响很大：冷缓存读 967 MB / 1.6 GB 会到 20–50 s，热缓存 0.5–2 s
  （small 的 f32 权重不用转换，比 turbo 的 f16→f32 快一个量级）。
- 内存：small f32 约 1 GB，turbo f32 约 3.2 GB（f16 权重会展开）。显存紧时
  `AsrOptions::dtype = F16` + GPU 后端。

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
12. **Whisper 的 `k_proj` 没有 bias**（q/v/out 都有），输出投影与 `embed_tokens` 绑定；
    特殊 token 的 id 每代差 1（large-v3 起 `<|transcribe|>` 50359→50360）。三件事都只能
    靠真实权重/文件发现，写死任何一个都会「能跑但全错」。
13. **Whisper 的 mel 必须补满 30 s**：它的编码器是整段双向注意力，补出来的帧会参与真实帧
    的注意力；「按有效帧截断」那种在 Qwen3-ASR 上成立的省算力技巧在这里会改变结果。
14. **f16 权重（turbo）按 f32 装载会翻倍**：809M 参数 × 4 B ≈ 3.2 GB，再加编码器中间量。
    显存/内存紧就显式 `DType::F16`（GPU）。

## 对拍（笔画模型）

```bash
OCHWPRO_MODEL_DIR=/path/to/model \
  cargo test -p xime-infer --test parity -- --ignored
```

参考值来自 `ochwpro.onnx`（fp32）对同一「一」横的输出：`一 0.4154 / - 0.3076 / …`。
fp16 权重下 top-1 稳定为「一」（~0.42），前二类别下标一致。
