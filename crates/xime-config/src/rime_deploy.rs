use crate::metadata::app_metadata;
pub use librime::levers::SchemaInfo;
use librime::{create_session, initialize, setup, Traits};
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
        None => {
            let paths = default_rime_paths();
            (paths.shared_data_dir, paths.user_data_dir)
        }
    }
}

/// 解析默认 Rime 数据目录。
///
/// - Windows：单目录模型（对齐 Xime）——shared 与 user 都指向 `%APPDATA%/<name>/rime`，
///   安装目录自带的方案文件由宿主在启动时部署进去（首装全量、升级只强更非 custom 文件）。
/// - Unix（双目录模型）：shared 取首个存在 default.yaml 的候选目录
///   （`~/.local/share/<name>/rime-data` 或 `/usr/share/<name>/rime-data`），
///   user 为 `~/.config/<name>/rime`。
///
/// 宿主应用在启动时可直接调用 [`set_rime_paths`] 注入，或省略调用走本默认值。
pub fn default_rime_paths() -> RimePaths {
    let config_dir = app_metadata().config_dir_name;

    #[cfg(windows)]
    {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        let rime_dir = PathBuf::from(base).join(config_dir).join("rime");
        RimePaths {
            shared_data_dir: rime_dir.clone(),
            user_data_dir: rime_dir,
        }
    }

    #[cfg(not(windows))]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let shared_candidates = [
            PathBuf::from(&home).join(format!(".local/share/{config_dir}/rime-data")),
            PathBuf::from(format!("/usr/share/{config_dir}/rime-data")),
        ];
        let shared_data_dir = shared_candidates
            .iter()
            .find(|d| d.join("default.yaml").exists())
            .cloned()
            .unwrap_or_else(|| shared_candidates[0].clone());
        RimePaths {
            shared_data_dir,
            user_data_dir: PathBuf::from(&home).join(format!(".config/{config_dir}/rime")),
        }
    }
}

fn ensure_user_config_files(_shared_data_dir: &std::path::Path, user_data_dir: &std::path::Path) {
    if !user_data_dir.exists() {
        std::fs::create_dir_all(user_data_dir).ok();
    }
}

pub fn init_rime_deployer() -> Result<(), String> {
    // 注意：这里只做 setup + initialize（毫秒级），不做任何部署。
    // 全量维护（start_maintenance）在首次调用时可能耗时数秒（编译全部
    // 方案词典），本函数被 SchemaManager::new() 在设置启动的 UI 线程上
    // 调用——内置部署会卡死窗口；部署一律走显式 deploy_all()（调用方
    // 已在后台线程执行）。
    RIME_INIT.call_once(|| {
        let (shared_data_dir, user_data_dir) = get_data_dirs();
        ensure_user_config_files(&shared_data_dir, &user_data_dir);

        let mut traits = Traits::new();
        let meta = app_metadata();
        traits
            .set_shared_data_dir(shared_data_dir.to_str().unwrap_or(""))
            .set_user_data_dir(user_data_dir.to_str().unwrap_or(""))
            .set_distribution_name(meta.distribution_name)
            .set_distribution_code_name(meta.distribution_code_name)
            .set_distribution_version(meta.version)
            .set_app_name(meta.app_name)
            .set_min_log_level(2);

        setup(&mut traits);

        if initialize(&mut traits).is_err() {
            return;
        }

        if let Ok(session) = create_session() {
            drop(session);
        }
    });

    Ok(())
}

/// 部署全部方案（显式全量维护，对齐 weasel「重新部署」：start_maintenance(full) + join）。
///
/// 不用 `api->deploy`：它是 OnWorkspaceChange 语义——`installation_update` /
/// `detect_modifications` 判定无变化时返回 0（librime rime_api_impl.h），
/// 是「无需维护」不是失败，不能当错误处理。
pub fn deploy_all() -> Result<(), String> {
    init_rime_deployer()?;
    let config_file = format!("{}.yaml", app_metadata().config_file_base);
    unsafe {
        let api = librime::get_api();
        if api.is_null() {
            return Err("Rime API 未初始化".to_string());
        }
        let started = (*api).start_maintenance.ok_or("start_maintenance 不可用")?(1);
        if started != 0 {
            if let Some(join) = (*api).join_maintenance_thread {
                join();
            }
        }
        // 全量维护后补跑 xime.yaml 配置部署（幂等）。
        if let Some(deploy_config) = (*api).deploy_config_file {
            let version_key =
                std::ffi::CString::new("config_version").map_err(|e| e.to_string())?;
            let config_c = std::ffi::CString::new(config_file).map_err(|e| e.to_string())?;
            deploy_config(config_c.as_ptr(), version_key.as_ptr());
        }
    }
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
        let config_dir = app_metadata().config_dir_name;
        let paths = default_rime_paths();
        let (shared, user) = (paths.shared_data_dir, paths.user_data_dir);

        #[cfg(windows)]
        {
            // Windows 单目录模型：shared == user == %APPDATA%/<config_dir>/rime
            assert_eq!(shared, user, "Windows 必须使用单目录模型");
            assert!(
                user.ends_with(PathBuf::from(format!("{config_dir}/rime"))),
                "user dir: {}",
                user.display()
            );
        }

        #[cfg(not(windows))]
        {
            assert!(
                !shared.starts_with("/usr/share/rime-data"),
                "default shared dir must not use system librime-data: {}",
                shared.display()
            );
            assert!(
                shared.ends_with(format!(".local/share/{config_dir}/rime-data").as_str()),
                "shared dir must be read-only rime-wubi install dir: {}",
                shared.display()
            );
            assert!(
                user.ends_with(format!(".config/{config_dir}/rime").as_str()),
                "user dir: {}",
                user.display()
            );
            assert_ne!(
                shared, user,
                "shared/user 必须分离，默认文件不得落入用户目录"
            );
        }
    }
}
