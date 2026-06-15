pub use librime::levers::deploy_all;
pub use librime::levers::SchemaInfo;
use librime::{
    create_session, get_api, initialize, join_maintenance_thread, setup, start_maintenance, Traits,
};
use std::ffi::CString;
use std::sync::Once;

static RIME_INIT: Once = Once::new();

fn get_shared_data_dir() -> std::path::PathBuf {
    if cfg!(windows) {
        let exe_path = std::env::current_exe()
            .ok()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        exe_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("data")
    } else {
        std::path::PathBuf::from("/usr/share/rime-data")
    }
}

fn get_user_data_dir() -> std::path::PathBuf {
    if cfg!(windows) {
        let exe_path = std::env::current_exe()
            .ok()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        exe_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("user-data")
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        std::path::PathBuf::from(&home).join(".config/xime/rime")
    }
}

pub fn get_data_dirs() -> (std::path::PathBuf, std::path::PathBuf) {
    (get_shared_data_dir(), get_user_data_dir())
}

fn ensure_user_config_files(shared_data_dir: &std::path::Path, _user_data_dir: &std::path::Path) {
    if !shared_data_dir.exists() {
        std::fs::create_dir_all(shared_data_dir).ok();
    }

    let xime_yaml = shared_data_dir.join("xime.yaml");
    if !xime_yaml.exists() {
        let default_yaml = r#"config_version: "1.0"
style:
  font_size: 14.0
  candidate_count: 5
  corner_radius: 8.0
  color_scheme: lavender_purple
"#;
        std::fs::write(&xime_yaml, default_yaml).ok();
    }
}

fn ensure_schemas_in_user_dir(_shared_data_dir: &std::path::Path, user_data_dir: &std::path::Path) {
    let default_custom = user_data_dir.join("default.custom.yaml");
    if !default_custom.exists() {
        let content = r#"customization:
  distribution_code_name: Xime
  distribution_version: "1.0"

patch:
  schema_list:
    - schema: wubi86_jidian
"#;
        std::fs::write(&default_custom, content).ok();
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
