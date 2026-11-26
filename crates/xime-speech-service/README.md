# xime-speech-service / xime-speechd

本地语音识别服务：**独立进程**持有模型、麦克风和识别引擎，输入法与设置程序
只通过 Unix socket 说话。进程入口是 `xime-speechd`。

为什么不是把模型塞进输入法：f16 权重 1.6 GB、f32 3.1 GB，常驻一个 IMK 进程
等于永远不还内存；推理崩溃（Metal 驱动触发过内核 panic）不能连累打字；
麦克风 TCC 授权也必须落在真正录音的那个进程上。

## 目录与进程边界

```text
XimeYi（IME）────────┐
                     ├── Unix socket（JSON 行）──► xime-speechd
XimeYiSettings ──────┘                            ├─ 麦克风（cpal）
                                                   ├─ 模型目录
                                                   └─ xime-infer
                                                      （Qwen3-ASR /
                                                       Fun-ASR-Nano / Whisper）
```

- 本 crate 不含平台代码：设备、dtype、目录、空闲卸载时长都由参数传入。
- launchd / Windows 服务 / systemd 的托管归宿主，不在这里。
- 同一个 socket 同时服务输入法（听写）和设置页（状态、模型管理、试听）。

## 模型目录

```text
<models_root>/
├── .selected                      # 当前模型 id（纯文本）
└── qwen3-asr-0.6b/                # 一个子目录 = 一个模型
    ├── config.json
    ├── model.safetensors          # 或 model.safetensors.index.json + 分片
    ├── tokenizer.json             # 或 vocab.json + merges.txt
    └── model.json                 # 可选：{"name": "...", "description": "..."}
```

文件齐全才算 `downloaded`（缺文件的目录仍会列出，提示重新下载）。`import`
会把整个目录**复制**进 `<models_root>/<目录名>`，不记外部路径——否则「删除
模型」会变成删别人的文件。

## 协议

一行一个 JSON；请求用 `op` 区分，响应统一 `{"ok":bool, ...}`。

| 请求 | 作用 | 响应 |
| --- | --- | --- |
| `{"op":"status"}` | 状态 + 模型列表 | `status` |
| `{"op":"models"}` | 只拉模型列表 | `models` + `status` |
| `{"op":"select","model_id":"…"}` | 切换模型（后台重新装载） | `status` |
| `{"op":"import","path":"…"}` | 导入模型目录 | `status` + `message` |
| `{"op":"remove","model_id":"…"}` | 删除模型目录 | `status` + `message` |
| `{"op":"listen_start"}` | 开始录音 | `status` |
| `{"op":"listen_stop"}` | 结束录音；识别在后台线程 | `status`，结果看 `status.text` |
| `{"op":"shutdown"}` | 退出并删除 socket | `status` |

`status.state`：`idle` / `loading` / `listening` / `transcribing` / `error`。
`listen_stop` 立刻返回，客户端轮询 `status`（设置页本来就是 250ms 一次）。

## 构建与运行

```bash
# macOS（CPU + BLAS；GPU 版换 --features metal）
cargo build --release -p xime-speechd --features accelerate

# 启动
xime-speechd --socket ~/Library/Application\ Support/XimeYi/speech.sock \
             --models ~/Library/Application\ Support/XimeYi/models/asr \
             --device cpu --dtype f32 --idle-secs 300
```

GPU 版把 `--features metal`（macOS）或 `--features cuda`（Windows/Linux）传给
构建，并用 `--device metal|cuda` 指定；`--dtype f16` 在 Metal 上更快、显存减半。

手测（macOS 自带 `nc` 不行时用 python）：

```bash
python3 - <<'PY'
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('/tmp/xime-speech.sock'); f = s.makefile('rwb')
f.write(b'{"op":"status"}\n'); f.flush()
print(f.readline().decode())
PY
```

## 平台状态

| 平台 | IPC | endpoint 形态 | 说明 |
| --- | --- | --- | --- |
| macOS / Linux | Unix socket | 文件路径，如 `…/speech.sock` | 建好后 chmod 0600 |
| Windows | 命名管道 | `\\.\pipe\xime-speech`（可只传 `xime-speech`） | 默认 DACL（创建者 + SYSTEM） |
| 全部 | 麦克风 | — | cpal；macOS 需在宿主 Info.plist 声明麦克风用途；Linux 构建需 `libasound2-dev` |

两个平台走同一份代码（`interprocess` 把 Unix socket 与命名管道抹平），服务端/客户端/协议
完全一致。macOS / Linux 上已有 socket 往返测试；**Windows 分支只做了编译期代码，未在
Windows 上跑过**，接入时先跑一遍 README 的冒烟。

## 与设置页的关系

`xime-setup-lib` 的语音页不直接连 socket，而是调宿主注入的六个回调
（status / download / delete / select / test_start / test_stop）。宿主把这些
回调实现成上面的 `Client` 调用即可，页面代码不用改。

## 测试

```bash
cargo test -p xime-speech-service   # 20 项：协议、模型目录、状态机、dispatch、socket 往返
```

真实模型冒烟（不进 CI）：

```bash
xime-speechd --socket /tmp/speech.sock --models /path/to/models &
# status 从 loading → idle、model_ready=true；shutdown 后 socket 被删除
```
