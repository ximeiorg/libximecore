//! 本地模型目录：扫描、导入、删除、记住选中的那个。
//!
//! 约定：模型根目录下的**每个子目录就是一个模型**。识别所需文件齐全才算
//! `ready`（缺文件的目录仍然列出来，界面上可以提示「重新下载」而不是消失）。
//!
//! 选中项记在 `<root>/.selected`（纯文本 id）：守护进程重启后仍用同一个模型。

use std::path::{Path, PathBuf};

/// 模型根目录扫描结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    /// 识别所需文件是否齐全。
    pub ready: bool,
    /// 目录占用字节数（展示用）。
    pub bytes: u64,
}

impl Entry {
    pub fn size_label(&self) -> String {
        format_size(self.bytes)
    }
}

/// 模型目录。
#[derive(Debug, Clone)]
pub struct Registry {
    root: PathBuf,
}

impl Registry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 扫描全部子目录（按 id 排序，稳定）。
    pub fn scan(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return out;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            let Some(id) = dir.file_name().map(|n| n.to_string_lossy().to_string()) else {
                continue;
            };
            if id.starts_with('.') {
                continue;
            }
            let meta = read_model_meta(&dir);
            out.push(Entry {
                name: meta.0.unwrap_or_else(|| id.clone()),
                description: meta.1.unwrap_or_default(),
                ready: is_ready(&dir),
                bytes: dir_size(&dir),
                path: dir,
                id,
            });
        }
        out
    }

    /// 当前选中的模型 id（文件不存在或指向已删除目录时返回 None）。
    pub fn selected(&self) -> Option<String> {
        let id = std::fs::read_to_string(self.root.join(".selected"))
            .ok()?
            .trim()
            .to_string();
        if id.is_empty() {
            return None;
        }
        self.scan().into_iter().find(|e| e.id == id).map(|e| e.id)
    }

    /// 记住选中的模型（不校验是否存在，调用方负责）。
    pub fn set_selected(&self, id: &str) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| format!("创建模型目录失败：{e}"))?;
        let path = self.root.join(".selected");
        std::fs::write(&path, id).map_err(|e| format!("写入选中模型失败：{e}"))
    }

    /// 导入一个模型目录：把 `src` 整个复制到 `<root>/<目录名>`。
    ///
    /// 为什么是复制而不是记路径：模型目录要能被守护进程独立管理（删除、迁移、
    /// 单实例），指向用户桌面上的临时目录会让「删了模型」变成删别人的文件。
    pub fn import(&self, src: &Path) -> Result<String, String> {
        let src = src
            .canonicalize()
            .map_err(|e| format!("模型目录不存在：{}（{e}）", src.display()))?;
        if !src.is_dir() {
            return Err(format!("不是目录：{}", src.display()));
        }
        let id = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| "无法从目录名推断模型 id".to_string())?;
        if !is_ready(&src) {
            return Err(format!(
                "目录里缺少识别所需文件（config.json / model.safetensors / 分词器）：{}",
                src.display()
            ));
        }
        let dst = self.root.join(&id);
        if dst.exists() {
            return Err(format!("模型「{id}」已存在，先删除再导入"));
        }
        std::fs::create_dir_all(&self.root).map_err(|e| format!("创建模型目录失败：{e}"))?;
        copy_dir(&src, &dst).map_err(|e| format!("复制模型失败：{e}"))?;
        Ok(id)
    }

    /// 删除一个模型目录；删的是选中项时顺带清掉 `.selected`。
    pub fn remove(&self, id: &str) -> Result<(), String> {
        if id.is_empty() || id.contains('/') || id.contains('\\') || id == "." || id == ".." {
            return Err(format!("非法的模型 id：{id}"));
        }
        let dir = self.root.join(id);
        if !dir.is_dir() {
            return Err(format!("模型「{id}」不存在"));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("删除模型失败：{e}"))?;
        if self.selected().as_deref() == Some(id) {
            let _ = std::fs::remove_file(self.root.join(".selected"));
        }
        Ok(())
    }
}

/// 识别所需文件是否齐全。两种发布格式都认。
pub fn is_ready(dir: &Path) -> bool {
    let weights = dir.join("model.safetensors").is_file()
        || dir.join("model.safetensors.index.json").is_file();
    let tokenizer = dir.join("tokenizer.json").is_file()
        || (dir.join("vocab.json").is_file() && dir.join("merges.txt").is_file());
    dir.join("config.json").is_file() && weights && tokenizer
}

/// 目录里的可选展示信息（`model.json`，缺失就用目录名）。
fn read_model_meta(dir: &Path) -> (Option<String>, Option<String>) {
    let Ok(content) = std::fs::read_to_string(dir.join("model.json")) else {
        return (None, None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return (None, None);
    };
    let get = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };
    (get("name"), get("description"))
}

/// 目录占用字节数（递归；模型目录文件很少，扫描不贵）。
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size(&path);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 字节数 → 人看的单位。
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.2} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes / KB)
    } else {
        format!("{bytes:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个文件齐全的模型目录（内容无所谓，只看存在性）。
    fn make_model(root: &Path, id: &str, ready: bool) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        std::fs::write(dir.join("tokenizer.json"), "{}").unwrap();
        if ready {
            std::fs::write(dir.join("model.safetensors"), vec![0u8; 2048]).unwrap();
        }
    }

    #[test]
    fn scan_lists_dirs_and_marks_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_model(root, "b-model", true);
        make_model(root, "a-model", false);
        std::fs::create_dir_all(root.join(".hidden")).unwrap();

        let registry = Registry::new(root);
        let entries = registry.scan();
        assert_eq!(
            entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["a-model", "b-model"],
            "按目录名排序，跳过点开头的目录"
        );
        assert!(!entries[0].ready, "缺权重文件的目录不算 ready");
        assert!(entries[1].ready);
        assert_eq!(entries[1].size_label(), "2 KB");
    }

    #[test]
    fn selection_is_remembered_and_cleared_with_model() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_model(root, "m1", true);
        make_model(root, "m2", true);
        let registry = Registry::new(root);

        assert_eq!(registry.selected(), None);
        registry.set_selected("m2").unwrap();
        assert_eq!(registry.selected().as_deref(), Some("m2"));

        registry.remove("m2").unwrap();
        assert_eq!(registry.selected(), None, "删掉选中模型后不该再指向它");
        assert_eq!(registry.scan().len(), 1);
    }

    #[test]
    fn import_copies_and_refuses_duplicates_or_incomplete_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("models");
        let src = tmp.path().join("incoming").join("qwen3-asr");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("config.json"), "{}").unwrap();
        std::fs::write(src.join("tokenizer.json"), "{}").unwrap();
        std::fs::write(src.join("model.safetensors"), vec![0u8; 16]).unwrap();

        let registry = Registry::new(&root);
        let id = registry.import(&src).unwrap();
        assert_eq!(id, "qwen3-asr");
        assert!(root.join("qwen3-asr").join("model.safetensors").is_file());
        assert!(registry.import(&src).is_err(), "同名模型不能重复导入");

        // 缺权重的目录直接拒绝，别把半个模型复制进去。
        let bad = tmp.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("config.json"), "{}").unwrap();
        assert!(registry.import(&bad).is_err());
    }

    #[test]
    fn remove_rejects_path_traversal_and_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = Registry::new(tmp.path());
        assert!(registry.remove("../evil").is_err());
        assert!(registry.remove("nope").is_err());
    }

    #[test]
    fn model_json_overrides_display_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_model(root, "qwen3-asr-0.6b", true);
        std::fs::write(
            root.join("qwen3-asr-0.6b").join("model.json"),
            r#"{"name":"Qwen3-ASR 0.6B","description":"中英混输"}"#,
        )
        .unwrap();
        let entries = Registry::new(root).scan();
        assert_eq!(entries[0].name, "Qwen3-ASR 0.6B");
        assert_eq!(entries[0].description, "中英混输");
    }
}
