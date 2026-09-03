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

/// Color scheme config that supports both string and object formats.
/// - String: `"lavender_purple"` (same scheme for light/dark)
/// - Object: `{ light: "lavender_purple", dark: "slate_gray" }`
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum ColorSchemeConfig {
    /// Simple string (same scheme for light/dark)
    Simple(String),
    /// Object with separate light/dark schemes
    Named {
        light: String,
        dark: String,
    },
}

impl Default for ColorSchemeConfig {
    fn default() -> Self {
        ColorSchemeConfig::Simple("lavender_purple".to_string())
    }
}

impl ColorSchemeConfig {
    /// Get the scheme name for the given dark mode state.
    pub fn scheme_name(&self, is_dark: bool) -> String {
        match self {
            ColorSchemeConfig::Simple(name) => name.clone(),
            ColorSchemeConfig::Named { light, dark } => {
                if is_dark {
                    dark.clone()
                } else {
                    light.clone()
                }
            }
        }
    }
}

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
    #[serde(default = "default_color_scheme")]
    pub color_scheme: ColorSchemeConfig,
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
fn default_color_scheme() -> ColorSchemeConfig {
    ColorSchemeConfig::Simple("lavender_purple".to_string())
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
    #[serde(default)]
    pub keyboard_background: Option<KeyboardBackground>,
    // Key colors
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub key_bg_color: Option<u32>,
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub key_bg_color_dark: Option<u32>,
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub key_text_color: Option<u32>,
    #[serde(
        deserialize_with = "deserialize_hex_color_optional",
        serialize_with = "serialize_hex_color_optional",
        default
    )]
    pub key_text_color_dark: Option<u32>,
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
    // Dynamic color (Material You)
    #[serde(default)]
    pub dynamic_color: Option<bool>,
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self {
            name: String::new(),
            primary_color: default_primary_color(),
            keyboard_background: None,
            key_bg_color: None,
            key_bg_color_dark: None,
            key_text_color: None,
            key_text_color_dark: None,
            candidate_text_color: None,
            candidate_text_color_dark: None,
            candidate_selected_text_color: None,
            candidate_selected_text_color_dark: None,
            dynamic_color: None,
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
