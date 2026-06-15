use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SmartSuggestionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default = "default_suggestion_count")]
    pub suggestion_count: i32,
    #[serde(default)]
    pub record_user_frequency: bool,
    #[serde(default)]
    pub auto_adjust_frequency: bool,
    #[serde(default = "default_learning_threshold")]
    pub learning_threshold: i32,
    #[serde(default)]
    pub model: SmartSuggestionModelConfig,
}

impl Default for SmartSuggestionConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            suggestion_count: default_suggestion_count(),
            record_user_frequency: false,
            auto_adjust_frequency: false,
            learning_threshold: default_learning_threshold(),
            model: SmartSuggestionModelConfig::default(),
        }
    }
}

fn default_suggestion_count() -> i32 {
    5
}
fn default_learning_threshold() -> i32 {
    3
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SmartSuggestionModelConfig {
    #[serde(default = "default_model_provider")]
    pub provider: String,
    #[serde(default = "default_model_name")]
    pub name: String,
    #[serde(default)]
    pub auto_download: bool,
    #[serde(default)]
    pub files: Vec<SmartSuggestionModelFile>,
}

impl Default for SmartSuggestionModelConfig {
    fn default() -> Self {
        Self {
            provider: default_model_provider(),
            name: default_model_name(),
            auto_download: false,
            files: Vec::new(),
        }
    }
}

fn default_model_provider() -> String {
    "modelscope".to_string()
}
fn default_model_name() -> String {
    "predictive-text-small".to_string()
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct SmartSuggestionModelFile {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub filename: String,
}
