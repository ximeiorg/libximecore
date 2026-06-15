use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct WubiRadicalsConfig {
    #[serde(default)]
    pub hotkeys: HotkeyConfig,
    #[serde(default)]
    pub schema_radicals: HashMap<String, KeyRadicalsConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HotkeyConfig {
    #[serde(default = "default_show_key")]
    pub show_key: String,
    #[serde(default)]
    pub show_all_keys: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            show_key: default_show_key(),
            show_all_keys: String::new(),
        }
    }
}

fn default_show_key() -> String {
    "Ctrl".to_string()
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct KeyRadicalsConfig {
    #[serde(default)]
    pub g: String,
    #[serde(default)]
    pub f: String,
    #[serde(default)]
    pub d: String,
    #[serde(default)]
    pub s: String,
    #[serde(default)]
    pub a: String,
    #[serde(default)]
    pub h: String,
    #[serde(default)]
    pub j: String,
    #[serde(default)]
    pub k: String,
    #[serde(default)]
    pub l: String,
    #[serde(default)]
    pub m: String,
    #[serde(default)]
    pub t: String,
    #[serde(default)]
    pub r: String,
    #[serde(default)]
    pub e: String,
    #[serde(default)]
    pub w: String,
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub y: String,
    #[serde(default)]
    pub u: String,
    #[serde(default)]
    pub i: String,
    #[serde(default)]
    pub o: String,
    #[serde(default)]
    pub p: String,
    #[serde(default)]
    pub n: String,
    #[serde(default)]
    pub b: String,
    #[serde(default)]
    pub v: String,
    #[serde(default)]
    pub c: String,
    #[serde(default)]
    pub x: String,
}

impl WubiRadicalsConfig {
    pub fn get_root_for_key(&self, schema: &str, key: char) -> Option<String> {
        let radicals = self.schema_radicals.get(schema)?;
        let root = match key.to_ascii_lowercase() {
            'g' => &radicals.g,
            'f' => &radicals.f,
            'd' => &radicals.d,
            's' => &radicals.s,
            'a' => &radicals.a,
            'h' => &radicals.h,
            'j' => &radicals.j,
            'k' => &radicals.k,
            'l' => &radicals.l,
            'm' => &radicals.m,
            't' => &radicals.t,
            'r' => &radicals.r,
            'e' => &radicals.e,
            'w' => &radicals.w,
            'q' => &radicals.q,
            'y' => &radicals.y,
            'u' => &radicals.u,
            'i' => &radicals.i,
            'o' => &radicals.o,
            'p' => &radicals.p,
            'n' => &radicals.n,
            'b' => &radicals.b,
            'v' => &radicals.v,
            'c' => &radicals.c,
            'x' => &radicals.x,
            _ => return None,
        };
        if root.is_empty() {
            None
        } else {
            Some(root.clone())
        }
    }

    pub fn is_schema_enabled_for_radicals(&self, current_schema: &str) -> bool {
        self.schema_radicals.contains_key(current_schema)
    }
}
