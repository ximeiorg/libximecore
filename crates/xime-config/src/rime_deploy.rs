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

// ------------------------------------------------------------------
// 单目录模型：随包方案数据部署（对齐 XimeYao `ensure_rime_data`，见其
// DECISIONS 2026-09-11——旧 shared/user 分离导致方案来源混乱）
// ------------------------------------------------------------------

/// 把随包方案数据源部署进 rime 用户目录（单目录模型；宿主在启动时、
/// `set_rime_paths` 之前调用，随后 shared == user == rime_dir）。
///
/// - **首装**（rime 目录下无任何 `*.schema.yaml`）：把 `source_dirs` 依次
///   全量复制进 rime 目录（递归，含 lua/ 子目录）；
/// - **升级**：仅覆盖"内容有变化且文件名不含 custom"的文件（保护用户定制）；
///   用户启用列表（default.custom.yaml 的 `- schema:` 行）非空时，未启用的
///   builtin 方案文件（`<id>.schema.yaml` / `<id>.dict.yaml`）不强更——
///   不覆盖用户已弃用的方案；
/// - `source_dirs` 按序依次复制（后者同名覆盖前者；实际场景 dev 与系统
///   rime-data 互斥存在，冲突仅理论）。
pub fn ensure_bundled_rime_data(source_dirs: &[std::path::PathBuf], rime_dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(rime_dir);
    let has_schema = std::fs::read_dir(rime_dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".schema.yaml"))
        })
        .unwrap_or(false);

    let mut failures: Vec<String> = Vec::new();
    for source in source_dirs {
        if !source.exists() {
            continue;
        }
        if has_schema {
            // 升级路径：用户已弃用的 builtin 方案文件不强更。
            let enabled = read_enabled_schemas(rime_dir);
            let skip_builtin = !enabled.is_empty();
            failures.extend(copy_changed_files(
                source,
                rime_dir,
                skip_builtin.then_some(&enabled),
            ));
        } else {
            failures.extend(copy_dir_contents(source, rime_dir));
        }
    }
    // 复制失败不能静默：磁盘满/权限问题会产出半更新的方案数据（schema 更新
    // 而词典没落），用户侧表现为部署出"死会话"。daemon 启动时调用本函数，
    // 此时日志已初始化，warn 即可见。
    for f in &failures {
        tracing::warn!("rime data deploy copy failed: {f}");
    }
    if !failures.is_empty() {
        tracing::warn!(
            "rime data deploy finished with {} failed file(s); check disk space/permissions",
            failures.len()
        );
    }
}

/// 读取用户启用的方案 id 列表（default.custom.yaml 里的 `- schema: xxx` 行）。
pub fn read_enabled_schemas(rime_dir: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(rime_dir.join("default.custom.yaml"))
        .map(|content| {
            content
                .lines()
                .filter_map(|line| {
                    let line = line.trim();
                    let rest = line.strip_prefix('-')?.trim();
                    let rest = rest.strip_prefix("schema:")?.trim();
                    let id = rest.trim_matches('"').trim_matches('\'');
                    (!id.is_empty()).then(|| id.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 判断文件是否属于用户未启用的 builtin 方案（`<id>.schema.yaml` /
/// `<id>.dict.yaml`，id 在 builtin 集合内但不在用户启用列表中）。
fn is_unused_builtin_file(
    name: &str,
    builtin_ids: &std::collections::HashSet<String>,
    enabled: &[String],
) -> bool {
    let stem = name
        .strip_suffix(".schema.yaml")
        .or_else(|| name.strip_suffix(".dict.yaml"));
    let Some(id) = stem else {
        return false;
    };
    builtin_ids.contains(id) && !enabled.iter().any(|e| e == id)
}

/// (len, mtime 纳秒) 签名：stat 即得，用于跳过"自上次复制后没变过"的文件。
type FileSignature = Option<(u64, i128)>;

fn file_signature(path: &std::path::Path) -> FileSignature {
    use std::time::UNIX_EPOCH;
    std::fs::metadata(path).ok().map(|m| {
        let mtime = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        (m.len(), mtime as i128)
    })
}

/// 复制并把目标文件的 mtime 改成源文件的：fs::copy 不保留时间戳，不回写的话
/// 下次启动 (len, mtime) 永远对不上，stat 短路就失效了。
fn copy_preserving_mtime(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::copy(src, dst)?;
    // filetime 支持 unix/windows；失败不致命（只是下次多一次内容比对）
    if let Ok(mtime) = std::fs::metadata(src).and_then(|m| m.modified()) {
        let _ = filetime::set_file_mtime(dst, filetime::FileTime::from_system_time(mtime));
    }
    Ok(())
}

/// 升级复制：仅当目标缺失或内容不同，且文件名不含 "custom"（保护用户定制）。
/// `skip` 传入用户启用列表时，未启用的 builtin 方案文件不再强更。
/// 返回失败的文件描述（不中断整体复制，逐个上报）。
fn copy_changed_files(
    src: &std::path::Path,
    dst: &std::path::Path,
    skip: Option<&[String]>,
) -> Vec<String> {
    let mut failures = Vec::new();
    let builtin_ids: std::collections::HashSet<String> = match skip {
        Some(_) => std::fs::read_dir(src)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| {
                        let n = e.file_name().to_string_lossy().to_string();
                        n.strip_suffix(".schema.yaml")
                            .filter(|_| e.path().is_file())
                            .map(String::from)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        None => Default::default(),
    };
    let Ok(entries) = std::fs::read_dir(src) else {
        failures.push(format!("read_dir {}: {}", src.display(), "failed"));
        return failures;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let dest = dst.join(entry.file_name());
        if ft.is_dir() {
            let _ = std::fs::create_dir_all(&dest);
            failures.extend(copy_changed_files(&entry.path(), &dest, skip));
        } else {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.contains("custom") {
                continue;
            }
            if let Some(enabled) = skip {
                if is_unused_builtin_file(&name, &builtin_ids, enabled) {
                    continue;
                }
            }
            // (len, mtime) 一致 = 自上次复制后源文件没变过：stat 短路，不读盘。
            // 签名不一致（含首次）才读内容比对。
            let needs_copy = match (file_signature(&entry.path()), file_signature(&dest)) {
                (Some(s), Some(d)) if s == d => false,
                (Some(s), Some(d)) if s.0 != d.0 => true,
                _ => match std::fs::read(&dest) {
                    Ok(existing) => existing != std::fs::read(entry.path()).unwrap_or_default(),
                    Err(_) => true,
                },
            };
            if needs_copy {
                if let Err(e) = copy_preserving_mtime(&entry.path(), &dest) {
                    failures.push(format!(
                        "{} -> {}: {e}",
                        entry.path().display(),
                        dest.display()
                    ));
                }
            }
        }
    }
    failures
}

/// 首装全量复制（递归）。返回失败的文件描述。
fn copy_dir_contents(src: &std::path::Path, dst: &std::path::Path) -> Vec<String> {
    let mut failures = Vec::new();
    let Ok(entries) = std::fs::read_dir(src) else {
        failures.push(format!("read_dir {}: failed", src.display()));
        return failures;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let dest = dst.join(entry.file_name());
        if ft.is_dir() {
            let _ = std::fs::create_dir_all(&dest);
            failures.extend(copy_dir_contents(&entry.path(), &dest));
        } else if let Err(e) = copy_preserving_mtime(&entry.path(), &dest) {
            failures.push(format!(
                "{} -> {}: {e}",
                entry.path().display(),
                dest.display()
            ));
        }
    }
    failures
}

#[cfg(test)]
mod bundled_deploy_tests {
    use super::*;

    /// 造一个 rime-wubi 风格的数据源目录（default.yaml + 方案 + lua/ 子目录）。
    fn make_source(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join("lua")).unwrap();
        std::fs::write(dir.join("default.yaml"), "config:\n  version: \"1\"\n").unwrap();
        std::fs::write(dir.join("wubi86.schema.yaml"), "name: 五笔\n").unwrap();
        std::fs::write(dir.join("wubi86.dict.yaml"), "---\n...\n工\ta\n").unwrap();
        std::fs::write(dir.join("lua").join("uuid.lua"), "return 1\n").unwrap();
    }

    #[test]
    fn first_install_copies_everything_recursively() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let rime = tmp.path().join("rime");
        make_source(&src);

        ensure_bundled_rime_data(std::slice::from_ref(&src), &rime);

        for f in [
            "default.yaml",
            "wubi86.schema.yaml",
            "wubi86.dict.yaml",
            "lua/uuid.lua",
        ] {
            assert!(rime.join(f).exists(), "首装缺 {f}");
        }
        // 二次调用幂等（都存在 → 走升级路径 → 内容相同不复制）。
        ensure_bundled_rime_data(std::slice::from_ref(&src), &rime);
        assert!(rime.join("wubi86.schema.yaml").exists());
    }

    #[test]
    fn upgrade_overwrites_changed_but_never_custom() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let rime = tmp.path().join("rime");
        make_source(&src);
        ensure_bundled_rime_data(std::slice::from_ref(&src), &rime);

        // 用户定制：改写 rime 目录里的 custom 文件 + 更新源的普通文件。
        std::fs::write(rime.join("wubi86.custom.yaml"), "patch:\n  我的定制: 1\n").unwrap();
        std::fs::write(src.join("wubi86.custom.yaml"), "patch:\n  源里的定制: 2\n").unwrap();
        std::fs::write(src.join("wubi86.dict.yaml"), "---\n...\n工\ta\n新词\tbb\n").unwrap();

        ensure_bundled_rime_data(std::slice::from_ref(&src), &rime);

        assert_eq!(
            std::fs::read_to_string(rime.join("wubi86.custom.yaml")).unwrap(),
            "patch:\n  我的定制: 1\n",
            "custom 文件绝不被覆盖"
        );
        assert!(
            std::fs::read_to_string(rime.join("wubi86.dict.yaml"))
                .unwrap()
                .contains("新词"),
            "内容变化的非 custom 文件应强更"
        );
    }

    #[test]
    fn upgrade_skips_unused_builtin_schemas() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let rime = tmp.path().join("rime");
        make_source(&src);
        std::fs::write(src.join("stroke.schema.yaml"), "name: 笔画\n").unwrap();
        std::fs::write(src.join("stroke.dict.yaml"), "---\n...\n一\ta\n").unwrap();
        ensure_bundled_rime_data(std::slice::from_ref(&src), &rime);

        // 用户弃用 stroke（启用列表只有 wubi86）并删除了它的文件；
        // 升级时不应把弃用方案强塞回来。
        std::fs::write(
            rime.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: wubi86\n",
        )
        .unwrap();
        std::fs::remove_file(rime.join("stroke.schema.yaml")).unwrap();
        std::fs::remove_file(rime.join("stroke.dict.yaml")).unwrap();

        ensure_bundled_rime_data(std::slice::from_ref(&src), &rime);

        assert!(!rime.join("stroke.schema.yaml").exists(), "弃用方案不强更");
        // 仍启用的 wubi86 缺了文件则照常补回。
        std::fs::remove_file(rime.join("wubi86.schema.yaml")).unwrap();
        ensure_bundled_rime_data(std::slice::from_ref(&src), &rime);
        assert!(rime.join("wubi86.schema.yaml").exists());
    }

    #[test]
    fn later_source_overwrites_same_name_on_first_install() {
        // source_dirs 按序依次复制（XimeYao 同款）：后一个 source 的同名文件
        // 覆盖前一个。实际场景 dev 与系统 rime-data 互斥存在，冲突仅理论。
        let tmp = tempfile::tempdir().unwrap();
        let dev = tmp.path().join("dev");
        let sys = tmp.path().join("sys");
        let rime = tmp.path().join("rime");
        make_source(&dev);
        make_source(&sys);
        std::fs::write(sys.join("wubi86.dict.yaml"), "---\n...\n系统版\ta\n").unwrap();

        ensure_bundled_rime_data(&[dev, sys], &rime);
        assert!(std::fs::read_to_string(rime.join("wubi86.dict.yaml"))
            .unwrap()
            .contains("系统版"));
    }
}
