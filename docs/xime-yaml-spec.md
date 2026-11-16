# xime.yaml 规范（桌面端 / 移动端共用）

同一个文件名、同一套字段命名，两端各自只读自己用得上的部分，其余原样保留（用户改写时不会丢）。

- 移动端：`app/src/main/assets/xime.yaml` → 用户 `rime/xime.custom.yaml`
- 桌面端：`libximecore/resources/xime.yaml` → 系统级（`/usr/share/xime/xime.yaml`、exe 旁 `data/`）→ 用户 `xime.custom.yaml`

## 合并规则

- 映射（map）按 **键深合并**：用户只写 `style.font_size`，其它字段仍取默认值。
- 列表（`xime_index.base_urls` 等）**整段覆盖**，需完整写出。
- 未知节一律保留：移动端的 `keyboard:`、桌面端的 `wubi_radicals:` 互不干扰。

## 颜色写法

| 写法 | 含义 | 桌面端 | 移动端 |
| --- | --- | --- | --- |
| `0xRRGGBB` | 不透明色 | 原样使用 | 原样使用 |
| `0xAARRGGBB` | 带透明度 | 只取低 24 位（忽略 alpha） | 完整使用 |

反序列化同时接受 `0x` 十六进制、`#RRGGBB` 与十进制整数；序列化统一写 `0x` 十六进制。

## 节与字段

| 节 | 桌面端 | 移动端 | 说明 |
| --- | --- | --- | --- |
| `metadata` | 不读 | 读 | `app_name` / `app_version` / `platform` / `config_version` / `generator` / `modified_time`；`platform` 写适用的平台，支持单个字符串或字符串数组（桌面端 `[windows, linux, macos]`，移动端 `android`） |
| `xime_index.base_urls` | 读 | 读 | 市场索引镜像列表，按顺序尝试，末尾需 `/` |
| `style` | 读 | 部分 | 见下 |
| `color_schemes` | 读 | 读 | 见下 |
| `wubi_radicals` | 读 | — | 五笔字根提示（`hotkeys.show_key`、`schema_radicals.<schema>.<key>`） |
| `calculator` | 读 | — | 数学计算开关 |
| `pair_secret` | 读 | — | 局域网配对密钥 |
| `log_level` | 读 | — | 桌面端日志级别，`RUST_LOG` 优先 |
| `correction.schemas` | — | 读 | 邻键误触纠错适用的方案 id |
| `fuzzy_pinyin.schemas` | — | 读 | 按全拼处理的方案 id（模糊音） |
| `keyboard` | — | 读 | 软键盘布局、按键、阴影、字体 |

### style

| 字段 | 桌面端 | 移动端 | 说明 |
| --- | --- | --- | --- |
| `color_scheme` | 读 | 读 | `{ light, dark }` 或单个字符串 |
| `dark_mode` | 读 | 读 | `0` 浅色 / `1` 深色 / `2` 跟随系统 |
| `font_family` / `font_size` | 读 | — | 候选窗字体 |
| `candidate_count` | 读 | — | 落成 rime `menu/page_size`，改动需重新部署 |
| `horizontal` | 读 | — | 候选横排 / 竖排 |
| `corner_radius` | 读 | — | 候选窗圆角 |

### color_schemes.\<id\>

桌面端只有候选窗、没有软键盘，因此只读下表标「读」的字段；其余字段属于软键盘，
桌面端不解析，用户文件里写了也原样保留。

| 字段 | 桌面端 | 移动端 | 说明 |
| --- | --- | --- | --- |
| `name` | 读 | 读 | 显示名 |
| `primary_color` | 读 | 读 | 主题强调色（选中/高亮） |
| `keyboard_background` | 读 | 读 | 候选窗背景。`type`: `solid` / `gradient` / `image`；纯色 `color`、`color_dark`；渐变 `colors`、`colors_dark`、`angle`；图片 `src`、`fit`、`overlay_alpha` |
| `candidate_text_color` / `_dark` | 读 | 读 | 候选文字色 |
| `candidate_selected_text_color` / `_dark` | 读 | 读 | 选中候选文字色 |
| `candidate_selected_bg_color` / `_dark` | 读 | 读 | 选中候选底色（可带透明度） |
| `keyboard_bg_color` | — | 读 | 软键盘背景后备色 |
| `candidate_bar_bg_color` / `candidate_bar_background` | — | 读 | 候选条底色（移动端候选栏，桌面端候选窗就是整个窗口，用 `keyboard_background`） |
| `key_bg_color` / `_dark` | — | 读 | 按键底色 |
| `special_key_bg_color` / `_dark` | — | 读 | 功能键（shift / 退格 / 回车）底色 |
| `key_text_color` / `_dark` | — | 读 | 按键文字色 |
| `key_background` | — | 读 | 单个按键背景 |
| `dynamic_color` | — | 读 | Material You 动态配色（Android 12+） |

新增皮肤时的暗色约束：渐变皮肤的 `colors_dark` 暗端压到 OKLCH L ≤ 0.32 / C ≤ 0.10；
候选文字与背景对比度不低于 4.5:1；选中底色与普通底色要能分辨。

