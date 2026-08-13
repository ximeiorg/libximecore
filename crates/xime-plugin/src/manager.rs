use crate::manifest::PluginManifest;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;

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
    /// - 校验包内 manifest.yaml 与入口脚本存在
    /// - 已安装同版本时报错；不同版本时覆盖（保持 enabled 状态）
    pub fn install_from_zip(&self, xipk: &Path, force: bool) -> Result<PluginRecord, ManagerError> {
        let file = std::fs::File::open(xipk)?;
        let mut archive = zip::ZipArchive::new(file)?;

        let manifest_yaml =
            read_zip_entry(&mut archive, "manifest.yaml").ok_or(ManagerError::MissingManifest)?;
        let manifest = PluginManifest::parse(&manifest_yaml)?;
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

fn read_zip_entry<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<String> {
    let mut entry = archive.by_name(name).ok()?;
    let mut content = String::new();
    entry.read_to_string(&mut content).ok()?;
    Some(content)
}

/// 安全解压：跳过路径穿越条目，所有路径限定在目标目录内。
fn extract_zip_safe<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    target: &Path,
) -> Result<(), ManagerError> {
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
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
        zip.start_file("manifest.yaml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(
            b"id: com.example.test\nname: Test\nversion: 1.0.0\ntype: emoji\nentry: main.lua\n",
        )
        .unwrap();
        zip.start_file("main.lua", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"return { getCategories = function() return { \"A\" } end }\n")
            .unwrap();
        zip.start_file("libs/util.lua", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"return { version = 1 }\n").unwrap();
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
            .join("main.lua")
            .exists());
        assert!(manager
            .plugin_dir("com.example.test")
            .join("libs/util.lua")
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
        zip.start_file("manifest.yaml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(
            b"id: com.example.bad\nname: Bad\nversion: 1\ntype: emoji\nentry: nope.lua\n",
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
}
