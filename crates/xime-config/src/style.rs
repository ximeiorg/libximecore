use serde::{Deserialize, Serialize};

/// Color scheme selection mode.
/// - `0`: Light mode
/// - `1`: Dark mode
/// - `2`: Follow system (default)
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(untagged)]
pub enum DarkMode {
    /// Simple mode: 0=light, 1=dark, 2=follow system
    Simple(u8),
}

impl Default for DarkMode {
    fn default() -> Self {
        DarkMode::Simple(2)
    }
}

impl DarkMode {
    pub fn value(&self) -> u8 {
        match self {
            DarkMode::Simple(v) => *v,
        }
    }

    /// Returns true if dark mode should be used given the system is in dark theme.
    pub fn is_dark(&self, system_is_dark: bool) -> bool {
        match self.value() {
            0 => false,
            1 => true,
            _ => system_is_dark,
        }
    }
}

/// 默认配色方案 id。
pub const DEFAULT_COLOR_SCHEME: &str = "lavender_purple";

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StyleConfig {
    #[serde(default)]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_candidate_count")]
    pub candidate_count: i32,
    #[serde(default = "default_horizontal")]
    pub horizontal: bool,
    #[serde(default = "default_corner_radius")]
    pub corner_radius: f32,
    /// 配色方案 id（`color_schemes` 的键）。浅色与深色是这个方案自己的
    /// `color` / `color_dark`、`candidate_text_color` / `_dark` 等成对字段，
    /// 由 `dark_mode` 决定用哪一套，不再分别为明暗挑两套方案。
    #[serde(
        default = "default_color_scheme",
        deserialize_with = "deserialize_color_scheme"
    )]
    pub color_scheme: String,
    #[serde(default = "default_dark_mode")]
    pub dark_mode: DarkMode,
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self {
            font_family: String::new(),
            font_size: default_font_size(),
            candidate_count: default_candidate_count(),
            horizontal: default_horizontal(),
            corner_radius: default_corner_radius(),
            color_scheme: default_color_scheme(),
            dark_mode: default_dark_mode(),
        }
    }
}

fn default_font_size() -> f32 {
    14.0
}
fn default_candidate_count() -> i32 {
    5
}
fn default_horizontal() -> bool {
    true
}
fn default_corner_radius() -> f32 {
    8.0
}
fn default_color_scheme() -> String {
    DEFAULT_COLOR_SCHEME.to_string()
}

/// 兼容旧写法 `color_scheme: { light: a, dark: b }`：早期方案没有 `*_dark`
/// 变体，只能拆成两套方案区分明暗；现在方案自身成对，读取时归并成一个 id，
/// 保存时一律写回单值。
fn deserialize_color_scheme<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Compat {
        Single(String),
        Pair {
            light: Option<String>,
            dark: Option<String>,
        },
    }

    Ok(match Compat::deserialize(deserializer)? {
        Compat::Single(name) => name,
        // 成对配置取 light（方案的"身份"）；只写了 dark 就取 dark。
        Compat::Pair { light, dark } => light
            .or(dark)
            .unwrap_or_else(|| DEFAULT_COLOR_SCHEME.to_string()),
    })
}
fn default_dark_mode() -> DarkMode {
    DarkMode::Simple(2)
}

/// Keyboard background configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct KeyboardBackground {
    #[serde(default = "default_bg_type")]
    pub r#type: String, // "solid", "gradient", "image"
    // Solid colors
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub color: Option<u32>,
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub color_dark: Option<u32>,
    // Gradient colors
    #[serde(default)]
    pub colors: Option<Vec<u32>>,
    #[serde(default)]
    pub colors_dark: Option<Vec<u32>>,
    #[serde(default)]
    pub angle: Option<u32>,
    // Image
    #[serde(default)]
    pub src: Option<String>,
    #[serde(default)]
    pub fit: Option<String>,
    #[serde(default)]
    pub overlay_alpha: Option<f32>,
    #[serde(default)]
    pub overlay_alpha_dark: Option<f32>,
}

fn default_bg_type() -> String {
    "solid".to_string()
}

impl Default for KeyboardBackground {
    fn default() -> Self {
        Self {
            r#type: default_bg_type(),
            color: None,
            color_dark: None,
            colors: None,
            colors_dark: None,
            angle: None,
            src: None,
            fit: None,
            overlay_alpha: None,
            overlay_alpha_dark: None,
        }
    }
}

/// 一套配色。
///
/// 桌面端只有候选窗、没有软键盘，所以这里只保留候选窗能画出来的颜色：
/// 主色（强调/选中）、窗背景、候选文字、选中文字、选中底色。
/// 移动端的 `key_*` / `special_key_*`（按键、功能键底色）**不进**这个结构体——
/// 那属于软键盘；配置里写了也不会被桌面端读取，用户文件中的这些键原样保留。
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ColorScheme {
    #[serde(default)]
    pub name: String,
    #[serde(
        deserialize_with = "deserialize_hex_color",
        serialize_with = "serialize_hex_color",
        default = "default_primary_color"
    )]
    pub primary_color: u32,
    /// 候选窗背景（solid / gradient / image）。
    #[serde(default)]
    pub keyboard_background: Option<KeyboardBackground>,
    // Candidate colors
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub candidate_text_color: Option<u32>,
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub candidate_text_color_dark: Option<u32>,
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub candidate_selected_text_color: Option<u32>,
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub candidate_selected_text_color_dark: Option<u32>,
    /// 候选选中背景色；未配置时由宿主按主色派生。
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub candidate_selected_bg_color: Option<u32>,
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub candidate_selected_bg_color_dark: Option<u32>,
}

impl ColorScheme {
    /// 候选窗背景色（亮/暗）。
    pub fn keyboard_bg(&self, dark: bool) -> Option<u32> {
        let bg = self.keyboard_background.as_ref()?;
        if dark {
            bg.color_dark
        } else {
            bg.color
        }
    }

    pub fn candidate_text(&self, dark: bool) -> Option<u32> {
        if dark {
            self.candidate_text_color_dark.or(self.candidate_text_color)
        } else {
            self.candidate_text_color
        }
    }

    pub fn candidate_selected_text(&self, dark: bool) -> Option<u32> {
        if dark {
            self.candidate_selected_text_color_dark
                .or(self.candidate_selected_text_color)
        } else {
            self.candidate_selected_text_color
        }
    }

    pub fn candidate_selected_bg(&self, dark: bool) -> Option<u32> {
        if dark {
            self.candidate_selected_bg_color_dark
                .or(self.candidate_selected_bg_color)
        } else {
            self.candidate_selected_bg_color
        }
    }
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self {
            name: String::new(),
            primary_color: default_primary_color(),
            keyboard_background: None,
            candidate_text_color: None,
            candidate_text_color_dark: None,
            candidate_selected_text_color: None,
            candidate_selected_text_color_dark: None,
            candidate_selected_bg_color: None,
            candidate_selected_bg_color_dark: None,
        }
    }
}

fn default_primary_color() -> u32 {
    0x8F73E2
}

fn deserialize_hex_color<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: serde_yaml::Value = serde::Deserialize::deserialize(deserializer)?;
    match value {
        serde_yaml::Value::Number(n) => Ok(n.as_u64().unwrap_or(0x8F73E2) as u32),
        serde_yaml::Value::String(s) => {
            let s = s.trim();
            if s.starts_with("0x") || s.starts_with("0X") {
                u32::from_str_radix(&s[2..], 16)
                    .map_err(|_| serde::de::Error::custom("Invalid hex color"))
            } else if let Some(stripped) = s.strip_prefix('#') {
                u32::from_str_radix(stripped, 16)
                    .map_err(|_| serde::de::Error::custom("Invalid hex color"))
            } else {
                s.parse::<u32>()
                    .map_err(|_| serde::de::Error::custom("Invalid color number"))
            }
        }
        _ => Ok(0x8F73E2),
    }
}

fn serialize_hex_color<S>(value: &u32, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&format!("0x{:06X}", value))
}

fn deserialize_hex_color_optional<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<serde_yaml::Value> = serde::Deserialize::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(serde_yaml::Value::Number(n)) => Ok(n.as_u64().map(|v| v as u32)),
        Some(serde_yaml::Value::String(s)) => {
            let s = s.trim();
            if s.starts_with("0x") || s.starts_with("0X") {
                u32::from_str_radix(&s[2..], 16)
                    .map(Some)
                    .map_err(|_| serde::de::Error::custom("Invalid hex color"))
            } else if let Some(stripped) = s.strip_prefix('#') {
                u32::from_str_radix(stripped, 16)
                    .map(Some)
                    .map_err(|_| serde::de::Error::custom("Invalid hex color"))
            } else {
                s.parse::<u32>()
                    .map(Some)
                    .map_err(|_| serde::de::Error::custom("Invalid color number"))
            }
        }
        _ => Ok(None),
    }
}

fn serialize_hex_color_optional<S>(value: &Option<u32>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(v) => serializer.serialize_str(&format!("0x{:06X}", v)),
        None => serializer.serialize_none(),
    }
}
