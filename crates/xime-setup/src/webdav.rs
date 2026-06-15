use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct WebdavConfig {
    pub url: String,
    pub username: String,
    pub password: String,
    pub remote_dir: String,
}

impl WebdavConfig {
    fn config_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        PathBuf::from(&home).join(".config/xime/webdav.yaml")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(config) = serde_yaml::from_str::<WebdavConfig>(&content) {
                    return config;
                }
            }
        }
        Self::default()
    }

    #[allow(dead_code)]
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {}", e))?;
        }
        let content =
            serde_yaml::to_string(self).map_err(|e| format!("Failed to serialize: {}", e))?;
        fs::write(&path, content).map_err(|e| format!("Failed to write: {}", e))?;
        Ok(())
    }
}
