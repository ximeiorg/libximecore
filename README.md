# libximecore

曦码（Xime）输入法的跨端核心库（Rust）。librime 引擎接入、配置与部署、剪贴板存储、JS 插件运行时、语音识别、剪贴板同步都由这里提供。

宿主：

| 平台 | 宿主 | 平台层 |
| --- | --- | --- |
| macOS | XimeYi（曦码·逸） | InputMethodKit |
| Windows | XimeYao（曦码·曜） | TSF |
| Linux | 设置界面、本地同步服务 | Wayland |
| 桌面全平台 | ximed（剪贴板同步） | iced + 托盘 |

Android 版不依赖本库，仅插件与语音的调用契约与此处保持一致。

```
XimeYi · XimeYao · Linux 桌面组件 · ximed
                      │
┌─────────────────────▼─────────────────────┐
│ libximecore                               │
│  librime-sys2 → librime → xime-rime       │
│  xime-config   xime-ipc   xime-setup-lib  │
│  xime-plugin   xime-infer   xime-lexicon  │
│  xime-sync-store → service → server       │
└─────────────────────┬─────────────────────┘
                      ▼
              librime (submodule) + rime-lua
```

## Crates

| crate | 职责 |
| --- | --- |
| `librime-sys2` | librime 原始 FFI 绑定与构建脚本（优先 pkg-config，否则 CMake 编译子模块） |
| `librime` | 安全封装：`Session` / `Context` / `Commit` / 按键码 / 用户词典 |
| `xime-rime` | 高层门面 `RimeEngine`：候选、组装串、状态 |
| `xime-config` | 配置模型、Rime 部署与路径、方案包清单、剪贴板存储、样式配色 |
| `xime-ipc` | 输入法进程与后台守护的消息类型、`DaemonClient` trait |
| `xime-setup-lib` | iced 跨平台设置界面（页面按 feature 门控） |
| `xime-plugin` | QuickJS 插件运行时：`.xipk` 包、manifest、能力白名单、超时熔断 |
| `xime-lexicon` | 外部词库解析：搜狗 `.scel` / 百度 `.bdict` / fcitx5 libime / 搜狗拼音备份等格式 |
| `xime-infer` | 基于 candle 的本地模型推理（笔画手写识别等） |
| `xime-infer` | 基于 candle 的本地模型推理：笔画手写识别（CPU / Metal / CUDA，按 feature 分包） |
| `xime-sync-store` | 同步存储后端（本地、历史） |
| `xime-sync-service` | 同步业务逻辑（幂等写入、事件序号） |
| `xime-sync-server` | HTTP / WS / SSE 同步服务端、TLS、鉴权 |

`xime-p2p` 与 `xime-sync-domain` 位于独立仓 [libximep2p](https://github.com/ximeiorg/libximep2p)，以 git 依赖（固定 rev）引用。

## 构建

需要 Rust stable、CMake 与 C++ 工具链、git。

```bash
git clone --recurse-submodules https://github.com/ximeiorg/libximecore.git
cd libximecore
cargo build
```

- `librime/` 为 submodule；系统存在 librime 时走 pkg-config，否则 CMake 现场编译，并自动拉取 rime-lua 插件。
- `xime-infer`（candle）不在 `default-members`（编译较重），单独构建/测试：
  `cargo test -p xime-infer`；macOS GPU 用 `--features metal`。
- 使用预编译库跑测试（macOS 用 `DYLD_LIBRARY_PATH`，Linux 用 `LD_LIBRARY_PATH`）：

  ```bash
  DYLD_LIBRARY_PATH=/path/to/librime/dist/lib cargo test
  ```

  ```bash
## 在宿主中引用

```toml
[dependencies]
xime-config = { git = "https://github.com/ximeiorg/libximecore.git", package = "xime-config" }
xime-plugin = { git = "https://github.com/ximeiorg/libximecore.git", package = "xime-plugin" }
```

本地联调在宿主仓的 `.cargo/config.toml` 里用 `[patch]` 指向本地路径，不改 `Cargo.toml`。

## 目录

```
crates/           各 crate 源码
docs/             协议与设计文档（clipboard-p2p-protocol.md）
resources/        默认配置 xime.yaml
librime/          git submodule
```

## 许可

GPL-3.0-or-later。