pub use librime::levers::deploy_all;
pub use librime::levers::SchemaInfo;
use librime::{
    create_session, get_api, initialize, join_maintenance_thread, setup, start_maintenance, Traits,
};
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::{Once, OnceLock};

static RIME_INIT: Once = Once::new();

/// Rime 数据目录，由宿主应用在启动时通过 [`set_rime_paths`] 提供。
#[derive(Clone, Debug)]
pub struct RimePaths {
    pub shared_data_dir: PathBuf,
    pub user_data_dir: PathBuf,
}

static RIME_PATHS: OnceLock<RimePaths> = OnceLock::new();

/// 设置 Rime 数据目录。必须在首次调用 Rime 相关函数之前调用。
pub fn set_rime_paths(paths: RimePaths) -> Result<(), String> {
    RIME_PATHS
        .set(paths)
        .map_err(|_| "rime paths already set".to_string())
}

pub fn get_data_dirs() -> (PathBuf, PathBuf) {
    match RIME_PATHS.get() {
        Some(paths) => (paths.shared_data_dir.clone(), paths.user_data_dir.clone()),
        None => default_data_dirs(),
    }
}

/// 未显式配置时的兜底默认（统一 single dir，不含系统 librime 目录）。
fn default_data_dirs() -> (PathBuf, PathBuf) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let rime_dir = PathBuf::from(&home).join(".config/xime/rime");
    (rime_dir.clone(), rime_dir)
}

fn ensure_user_config_files(_shared_data_dir: &std::path::Path, user_data_dir: &std::path::Path) {
    if !user_data_dir.exists() {
        std::fs::create_dir_all(user_data_dir).ok();
    }
}

fn ensure_schemas_in_user_dir(shared_data_dir: &std::path::Path, user_data_dir: &std::path::Path) {
    let default_custom = user_data_dir.join("default.custom.yaml");
    if !default_custom.exists() {
        // 优先复制 rime-wubi 自带的 default.custom.yaml（含默认 schema_list）
        let template = shared_data_dir.join("default.custom.yaml");
        if template.exists() {
            std::fs::copy(&template, &default_custom).ok();
        } else {
            let content = r#"customization:
  distribution_code_name: Xime
  distribution_version: "1.0"

patch:
  schema_list:
    - schema: wubi86_pinyin
"#;
            std::fs::write(&default_custom, content).ok();
        }
    }
}

pub fn init_rime_deployer() -> Result<(), String> {
    RIME_INIT.call_once(|| {
        let (shared_data_dir, user_data_dir) = get_data_dirs();
        ensure_user_config_files(&shared_data_dir, &user_data_dir);
        ensure_schemas_in_user_dir(&shared_data_dir, &user_data_dir);

        let mut traits = Traits::new();
        traits
            .set_shared_data_dir(shared_data_dir.to_str().unwrap_or(""))
            .set_user_data_dir(user_data_dir.to_str().unwrap_or(""))
            .set_distribution_name("Xime")
            .set_distribution_code_name("Xime")
            .set_distribution_version("1.0")
            .set_app_name("rime.xime.setup")
            .set_min_log_level(2);

        setup(&mut traits);

        if initialize(&mut traits).is_err() {
            return;
        }

        if start_maintenance(true).is_ok() {
            join_maintenance_thread();
        }

        if let Ok(session) = create_session() {
            drop(session);
        }

        unsafe {
            let api = get_api();
            if !api.is_null() {
                if let Some(deploy_config) = (*api).deploy_config_file {
                    let config_file = CString::new("xime.yaml").unwrap_or_default();
                    let version_key = CString::new("config_version").unwrap_or_default();
                    deploy_config(config_file.as_ptr(), version_key.as_ptr());
                }
            }
        }
    });

    Ok(())
}

pub fn deploy_all_schemas() -> Result<(), String> {
    init_rime_deployer()?;
    deploy_all().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_data_dirs_no_system_librime() {
        let (shared, user) = default_data_dirs();
        assert!(
            !shared.starts_with("/usr/share/rime-data"),
            "default shared dir must not use system librime-data: {}",
            shared.display()
        );
        assert!(
            user.ends_with(".config/xime/rime"),
            "user dir: {}",
            user.display()
        );
    }
}
