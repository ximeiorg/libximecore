use crate::manifest::{self, PluginManifest};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// 插件包大小上限（同 Android InstallerManager）。
const MAX_PACKAGE_BYTES: u64 = 10 * 1024 * 1024;
/// zip 条目数上限。
const MAX_ZIP_ENTRIES: usize = 512;
/// 解压后总大小上限。
const MAX_TOTAL_UNCOMPRESSED: u64 = 64 * 1024 * 1024;

/// 插件管理错误。
#[derive(Debug, Error)]
pub enum ManagerError {
    #[error("io 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip 读取失败: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("manifest 错误: {0}")]
    Manifest(#[from] crate::manifest::ManifestError),
    #[error("registry 序列化失败: {0}")]
    RegistrySerialize(#[from] serde_yaml::Error),
    #[error("manifest.yaml 缺失或不可解析")]
    MissingManifest,
    #[error("入口脚本缺失: {0}")]
    MissingEntry(String),
    #[error("插件已安装: {0}")]
    AlreadyInstalled(String),
    #[error("宿主版本不满足插件要求: {0}")]
    IncompatibleHost(String),
    #[error("插件包超出限制: {0}")]
    PackageLimit(String),
}

/// 已安装插件记录（registry.yaml 条目）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRecord {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(rename = "type", default)]
    pub plugin_type: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(rename = "installedAt", default)]
    pub installed_at: String,
    #[serde(skip)]
    pub state: PluginRecordState,
}

fn default_true() -> bool {
    true
}

/// 目录存在性派生的运行期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PluginRecordState {
    #[default]
    Ready,
    /// registry 有记录但目录缺失（可重装）。
    MissingDir,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    plugins: Vec<PluginRecord>,
}

/// 插件安装目录管理：安装 / 卸载 / 启停 / 列表。
///
/// 目录布局：
/// - `<root>/<id>/`        解压后的插件包
/// - `<root>/registry.yaml` 已安装插件元数据
/// - `<root>/config/<id>.yaml` 插件配置（host.config 读写）
pub struct PluginManager {
    root: PathBuf,
}

impl PluginManager {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn plugin_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn config_path(&self, id: &str) -> PathBuf {
        self.root.join("config").join(format!("{id}.yaml"))
    }

    /// 读取 registry.yaml；不存在或损坏时返回空列表。
    pub fn list(&self) -> Vec<PluginRecord> {
        let path = self.root.join("registry.yaml");
        let Ok(content) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let Ok(registry) = serde_yaml::from_str::<Registry>(&content) else {
            return Vec::new();
        };
        let mut records = registry.plugins;
        for record in &mut records {
            record.state = if self.plugin_dir(&record.id).exists() {
                PluginRecordState::Ready
            } else {
                PluginRecordState::MissingDir
            };
        }
        records
    }

    pub fn get(&self, id: &str) -> Option<PluginRecord> {
        self.list().into_iter().find(|p| p.id == id)
    }

    /// 安装 .xipk 压缩包到插件目录。
    ///
    /// - 校验包内 manifest（manifest.json 优先，兼容 manifest.yaml）与入口脚本存在
    /// - 宿主版本门禁（minHostVersion/maxHostVersion，同 Android InstallerManager）
    /// - 限额：包 ≤10MB、条目 ≤512、解压总量 ≤64MB；`lib/` 前缀条目跳过
    /// - 已安装同版本时报错；不同版本时覆盖（保持 enabled 状态）
    pub fn install_from_zip(&self, xipk: &Path, force: bool) -> Result<PluginRecord, ManagerError> {
        let package_size = std::fs::metadata(xipk)?.len();
        if package_size > MAX_PACKAGE_BYTES {
            return Err(ManagerError::PackageLimit(format!(
                "包大小 {package_size} 超过 {}MB 上限",
                MAX_PACKAGE_BYTES / 1024 / 1024
            )));
        }

        let file = std::fs::File::open(xipk)?;
        let mut archive = zip::ZipArchive::new(file)?;

        let entry_count = archive.len();
        if entry_count > MAX_ZIP_ENTRIES {
            return Err(ManagerError::PackageLimit(format!(
                "条目数 {entry_count} 超过 {MAX_ZIP_ENTRIES} 上限"
            )));
        }
        let mut total_uncompressed = 0u64;
        for i in 0..entry_count {
            let entry = archive.by_index(i)?;
            total_uncompressed += entry.size();
        }
        if total_uncompressed > MAX_TOTAL_UNCOMPRESSED {
            return Err(ManagerError::PackageLimit(format!(
                "解压总量 {total_uncompressed} 超过 {}MB 上限",
                MAX_TOTAL_UNCOMPRESSED / 1024 / 1024
            )));
        }

        let manifest = if let Some(json) = read_zip_entry(&mut archive, "manifest.json") {
            PluginManifest::parse_json(&json)?
        } else {
            let yaml = read_zip_entry(&mut archive, "manifest.yaml")
                .ok_or(ManagerError::MissingManifest)?;
            PluginManifest::parse(&yaml)?
        };
        check_host_compatibility(&manifest)?;
        let id = manifest.id.clone();

        // 入口脚本必须在包内
        let entry_in_zip = archive
            .file_names()
            .any(|n| n.trim_start_matches("./") == manifest.entry);
        if !entry_in_zip {
            return Err(ManagerError::MissingEntry(manifest.entry.clone()));
        }

        let target = self.plugin_dir(&id);
        let existing = self.get(&id);

        if existing.is_some() && !force {
            return Err(ManagerError::AlreadyInstalled(id));
        }

        // 覆盖安装时先清空旧目录
        if target.exists() {
            std::fs::remove_dir_all(&target)?;
        }
        std::fs::create_dir_all(&target)?;

        extract_zip_safe(&mut archive, &target)?;

        let record = PluginRecord {
            id: id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            plugin_type: manifest.plugin_type.clone(),
            enabled: existing.map(|p| p.enabled).unwrap_or(true),
            installed_at: now_string(),
            state: PluginRecordState::Ready,
        };

        // 写 registry
        let mut registry = self.read_registry();
        registry.retain(|p| p.id != id);
        registry.push(record.clone());
        self.write_registry(&registry)?;

        Ok(record)
    }

    /// 从已解压的插件目录安装（用于随宿主分发的内置插件，等价 install_from_zip）。
    pub fn install_from_dir(
        &self,
        source: &Path,
        force: bool,
    ) -> Result<PluginRecord, ManagerError> {
        let manifest = PluginManifest::from_dir(source)?;
        check_host_compatibility(&manifest)?;
        let id = manifest.id.clone();

        if !source.join(&manifest.entry).exists() {
            return Err(ManagerError::MissingEntry(manifest.entry.clone()));
        }
        let existing = self.get(&id);
        if existing.is_some() && !force {
            return Err(ManagerError::AlreadyInstalled(id));
        }

        let target = self.plugin_dir(&id);
        if target.exists() {
            std::fs::remove_dir_all(&target)?;
        }
        std::fs::create_dir_all(&target)?;
        copy_dir_recursive(source, &target)?;

        let record = PluginRecord {
            id: id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            plugin_type: manifest.plugin_type.clone(),
            enabled: existing.map(|p| p.enabled).unwrap_or(true),
            installed_at: now_string(),
            state: PluginRecordState::Ready,
        };

        let mut registry = self.read_registry();
        registry.retain(|p| p.id != id);
        registry.push(record.clone());
        self.write_registry(&registry)?;

        Ok(record)
    }

    /// 卸载插件（删除目录与配置，更新 registry）。
    pub fn uninstall(&self, id: &str) -> Result<(), ManagerError> {
        let dir = self.plugin_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        let config = self.config_path(id);
        if config.exists() {
            std::fs::remove_file(&config).ok();
        }

        let mut registry = self.read_registry();
        registry.retain(|p| p.id != id);
        self.write_registry(&registry)
    }

    /// 启用 / 禁用插件。
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), ManagerError> {
        let mut registry = self.read_registry();
        let Some(record) = registry.iter_mut().find(|p| p.id == id) else {
            return Err(ManagerError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("插件未安装: {id}"),
            )));
        };
        record.enabled = enabled;
        self.write_registry(&registry)
    }

    /// 读取已安装插件的 manifest。
    pub fn load_manifest(&self, id: &str) -> Result<PluginManifest, ManagerError> {
        Ok(PluginManifest::from_dir(&self.plugin_dir(id))?)
    }

    // ---- 私有 ----

    fn read_registry(&self) -> Vec<PluginRecord> {
        self.list()
    }

    fn write_registry(&self, plugins: &[PluginRecord]) -> Result<(), ManagerError> {
        let registry = Registry {
            plugins: plugins.to_vec(),
        };
        let yaml = serde_yaml::to_string(&registry)?;
        std::fs::create_dir_all(&self.root)?;
        std::fs::write(self.root.join("registry.yaml"), yaml)?;
        Ok(())
    }
}

/// 宿主版本门禁：JS 插件宿主代次以 SDK_VERSION 表达（插件声明的
/// minHostVersion 3.0.0 指向 Android 3.0 / 桌面对齐代次）。
fn check_host_compatibility(manifest: &PluginManifest) -> Result<(), ManagerError> {
    let host = crate::runtime::SDK_VERSION;
    if !manifest::version_compatible(host, &manifest.min_host_version, &manifest.max_host_version) {
        return Err(ManagerError::IncompatibleHost(format!(
            "宿主 {host} 不在 [{}, {}] 区间内",
            manifest.min_host_version, manifest.max_host_version
        )));
    }
    Ok(())
}

fn now_string() -> String {
    // 近似 RFC3339（无外部时间依赖），用于 installedAt。
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let years = days / 365;
    format!(
        "{}-{:02}-{:02}",
        1970 + years,
        (days % 365) / 28 + 1,
        days % 28 + 1
    )
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&dest)?;
            copy_dir_recursive(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

fn read_zip_entry<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<String> {
    let mut entry = archive.by_name(name).ok()?;
    let mut content = String::new();
    entry.read_to_string(&mut content).ok()?;
    Some(content)
}

/// 安全解压：跳过路径穿越条目与 `lib/` 前缀原生库，所有路径限定在目标目录内。
fn extract_zip_safe<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    target: &Path,
) -> Result<(), ManagerError> {
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        // Android 安装器约定：跳过平台原生库前缀（JS 插件不应携带）
        if entry.name().trim_start_matches("./").starts_with("lib/") {
            continue;
        }
        let Some(path) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            continue;
        };
        let dest = target.join(&path);
        if entry.is_dir() {
            std::fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = std::fs::File::create(&dest)?;
        std::io::copy(&mut entry, &mut output)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn test_xipk(dir: &Path) -> PathBuf {
        let xipk = dir.join("test.xipk");
        let file = std::fs::File::create(&xipk).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(
            br#"{"id":"com.example.test","name":"Test","version":"1.0.0","type":"emoji","entry":"main.js"}"#,
        )
        .unwrap();
        zip.start_file("main.js", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(
            b"globalThis.plugin = { emoji: { listCategories: function () { return [\"A\"]; } } };\n",
        )
        .unwrap();
        zip.start_file("libs/util.js", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"module.exports = { version: 1 };\n")
            .unwrap();
        zip.start_file(
            "resources/icon.txt",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"icon").unwrap();
        zip.finish().unwrap();
        xipk
    }

    #[test]
    fn install_list_uninstall_roundtrip() {
        let dir = std::env::temp_dir().join(format!("xime_plugin_mgr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let xipk = test_xipk(&dir);
        let manager = PluginManager::new(dir.join("root"));

        let record = manager.install_from_zip(&xipk, false).unwrap();
        assert_eq!(record.id, "com.example.test");
        assert_eq!(record.version, "1.0.0");
        assert!(record.enabled);
        assert!(manager
            .plugin_dir("com.example.test")
            .join("main.js")
            .exists());
        assert!(manager
            .plugin_dir("com.example.test")
            .join("libs/util.js")
            .exists());

        // 同版本重复安装报错
        assert!(matches!(
            manager.install_from_zip(&xipk, false),
            Err(ManagerError::AlreadyInstalled(_))
        ));

        // 启停
        manager.set_enabled("com.example.test", false).unwrap();
        assert!(!manager.get("com.example.test").unwrap().enabled);

        // 卸载后 registry 清空
        manager.uninstall("com.example.test").unwrap();
        assert!(manager.list().is_empty());
        assert!(manager.get("com.example.test").is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_missing_entry_rejected() {
        let dir = std::env::temp_dir().join(format!("xime_plugin_bad_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let xipk = dir.join("bad.xipk");
        let file = std::fs::File::create(&xipk).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(
            br#"{"id":"com.example.bad","name":"Bad","version":"1","type":"emoji","entry":"nope.js"}"#,
        )
        .unwrap();
        zip.finish().unwrap();

        let manager = PluginManager::new(dir.join("root"));
        assert!(matches!(
            manager.install_from_zip(&xipk, false),
            Err(ManagerError::MissingEntry(_))
        ));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_rejects_incompatible_host_version() {
        let dir = std::env::temp_dir().join(format!("xime_plugin_ver_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let xipk = dir.join("future.xipk");
        let file = std::fs::File::create(&xipk).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(
            br#"{"id":"com.example.future","name":"Future","version":"9.0.0","type":"emoji",
                "entry":"main.js","minHostVersion":"9.0.0"}"#,
        )
        .unwrap();
        zip.start_file("main.js", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"globalThis.plugin = {};\n").unwrap();
        zip.finish().unwrap();

        let manager = PluginManager::new(dir.join("root"));
        assert!(matches!(
            manager.install_from_zip(&xipk, false),
            Err(ManagerError::IncompatibleHost(_))
        ));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_skips_lib_prefix_entries() {
        let dir = std::env::temp_dir().join(format!("xime_plugin_lib_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let xipk = test_xipk(&dir);
        // 追加 lib/ 前缀条目
        {
            let file = std::fs::File::options()
                .read(true)
                .write(true)
                .open(&xipk)
                .unwrap();
            let mut zip = zip::ZipWriter::new_append(file).unwrap();
            zip.start_file(
                "lib/arm64-v8a/libnative.so",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"\x7fELF").unwrap();
            zip.finish().unwrap();
        }

        let manager = PluginManager::new(dir.join("root"));
        let record = manager.install_from_zip(&xipk, false).unwrap();
        let plugin_dir = manager.plugin_dir(&record.id);
        assert!(plugin_dir.join("main.js").exists());
        assert!(plugin_dir.join("libs/util.js").exists(), "libs/ 不应被误伤");
        assert!(!plugin_dir.join("lib").exists(), "lib/ 前缀条目应被跳过");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
