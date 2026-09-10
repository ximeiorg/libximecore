//! 云备份打包/恢复（对齐 Android `BackupManager` 语义）。
//!
//! - 备份包：rime 用户目录的 tar.gz，归档条目统一带 `rime/` 前缀，
//!   文件名 `ximeyi-<UTC时间戳>-<config|full>.tar.gz`；
//! - 备份模式（对齐 Android `ExportMode`）：仅配置排除用户词典（`*.userdb/`）
//!   与模型（`*.bin` / `*.gram`），全量包含全部；
//! - 恢复：覆盖 rime 用户目录同名文件（Android 同款"重启后生效"语义）。

use std::path::Path;

/// 备份模式（对齐 Android `ExportMode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupMode {
    /// 仅配置：不含用户词典与模型。
    ConfigOnly,
    /// 全量：配置 + 用户词典 + 模型。
    Full,
}

impl BackupMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ConfigOnly => "仅配置",
            Self::Full => "全量",
        }
    }

    pub fn detail(&self) -> &'static str {
        match self {
            Self::ConfigOnly => "不含用户词典（*.userdb）与模型（*.bin/*.gram）",
            Self::Full => "包含用户词典与模型，包体较大",
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Self::ConfigOnly => "config",
            Self::Full => "full",
        }
    }

    pub fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::ConfigOnly),
            1 => Some(Self::Full),
            _ => None,
        }
    }
}

/// ConfigOnly 模式跳过的归档条目（相对路径，/ 分隔）。
fn is_user_data(rel: &str) -> bool {
    rel.split('/').any(|seg| seg.ends_with(".userdb"))
        || rel.ends_with(".bin")
        || rel.ends_with(".gram")
}

/// 生成备份包文件名：`ximeyi-YYYYmmdd-HHMMSS-<tag>.tar.gz`（UTC 时间）。
pub fn archive_name(mode: BackupMode, unix_secs: u64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_unix(unix_secs as i64);
    format!(
        "ximeyi-{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-{}.tar.gz",
        mode.tag()
    )
}

/// 将 `rime_dir` 打包为 tar.gz 字节（条目按路径排序，ConfigOnly 跳过用户数据）。
pub fn pack_rime(rime_dir: &Path, mode: BackupMode) -> Result<Vec<u8>, String> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    collect_files(rime_dir, &mut files)?;
    files.sort();
    files.retain(|p| {
        mode != BackupMode::ConfigOnly
            || !is_user_data(
                &p.strip_prefix(rime_dir)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
            )
    });

    let buf = std::io::Cursor::new(Vec::new());
    let gz = flate2::write::GzEncoder::new(buf, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    for path in &files {
        let rel = path
            .strip_prefix(rime_dir)
            .map_err(|e| format!("相对路径计算失败: {e}"))?
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        tar.append_path_with_name(path, format!("rime/{rel}"))
            .map_err(|e| format!("打包 {} 失败: {e}", rel))?;
    }
    let gz = tar
        .into_inner()
        .map_err(|e| format!("收尾 tar 失败: {e}"))?;
    Ok(gz
        .finish()
        .map_err(|e| format!("收尾 gzip 失败: {e}"))?
        .into_inner())
}

/// 把备份包解包到 `rime_dir`（覆盖同名文件）。返回恢复的文件数。
///
/// 安全校验：只接受 `rime/` 前缀条目，拒绝包含 `..` 的路径与非普通文件。
pub fn unpack_rime(archive: &[u8], rime_dir: &Path) -> Result<usize, String> {
    let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
    let mut tar = tar::Archive::new(gz);
    let mut restored = 0usize;
    for entry in tar.entries().map_err(|e| format!("读取备份包失败: {e}"))? {
        let mut entry = entry.map_err(|e| format!("读取条目失败: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("读取条目路径失败: {e}"))?
            .to_path_buf();
        let Some(rel) = path.strip_prefix("rime").ok().map(|p| p.to_path_buf()) else {
            continue; // 非 rime/ 前缀条目：忽略
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        if rel
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return Err(format!("备份包含非法路径: {}", rel.display()));
        }
        let target = rime_dir.join(&rel);
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("建目录失败: {e}"))?;
            continue;
        }
        if !entry_type.is_file() {
            return Err(format!("备份包含不支持的条目类型: {}", rel.display()));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败: {e}"))?;
        }
        let mut out = std::fs::File::create(&target)
            .map_err(|e| format!("写 {} 失败: {e}", rel.display()))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("写 {} 失败: {e}", rel.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.header().mode().ok().map(|m| m & 0o777) {
                let _ = out.set_permissions(std::fs::Permissions::from_mode(mode));
            }
        }
        restored += 1;
    }
    Ok(restored)
}

/// 判断备份包文件名中的模式标签（远端列表展示用）。
pub fn mode_tag_of(name: &str) -> Option<&'static str> {
    if name.ends_with("-config.tar.gz") {
        Some("config")
    } else if name.ends_with("-full.tar.gz") {
        Some("full")
    } else {
        None
    }
}

fn collect_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("读取目录 {} 失败: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("遍历条目失败: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// Unix 秒 → (年, 月, 日, 时, 分, 秒)（UTC，Howard Hinnant 民用历算法）。
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    // days since 1970-01-01 → civil date
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, h, mi, s)
}

/// 备份包字节 → 可读内容校验（测试/调试用）：读出全部条目相对路径。
#[cfg(test)]
fn list_entries(archive: &[u8]) -> Vec<String> {
    let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
    let mut tar = tar::Archive::new(gz);
    let mut names = Vec::new();
    for entry in tar.entries().unwrap() {
        let entry = entry.unwrap();
        names.push(
            entry
                .path()
                .unwrap()
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/"),
        );
    }
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ximeyi-backup-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, content: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn pack_config_only_skips_user_data_and_roundtrips() {
        let src = temp_dir("src");
        write(&src.join("default.yaml"), b"config: true\n");
        write(&src.join("wubi86.schema.yaml"), b"schema\n");
        write(&src.join("wubi86.userdb/launch"), b"db1");
        write(&src.join("zh_lts.bin"), b"model");
        let archive = pack_rime(&src, BackupMode::ConfigOnly).unwrap();

        let names = list_entries(&archive);
        assert!(names.contains(&"rime/default.yaml".to_string()));
        assert!(names.contains(&"rime/wubi86.schema.yaml".to_string()));
        assert!(!names.iter().any(|n| n.contains("userdb")));
        assert!(!names.iter().any(|n| n.ends_with(".bin")));

        // 全量应包含用户数据。
        let full = pack_rime(&src, BackupMode::Full).unwrap();
        let full_names = list_entries(&full);
        assert!(full_names.contains(&"rime/wubi86.userdb/launch".to_string()));
        assert!(full_names.contains(&"rime/zh_lts.bin".to_string()));

        // 恢复到新目录，内容一致。
        let dst = temp_dir("dst");
        let n = unpack_rime(&archive, &dst).unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            std::fs::read(dst.join("default.yaml")).unwrap(),
            b"config: true\n"
        );

        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(dst);
    }

    #[test]
    fn unpack_rejects_path_traversal() {
        // tar 库在写入端会拒绝 `..` 路径，这里手工构造 ustar 字节绕过写入校验，
        // 验证恢复端对恶意包的防护。
        let archive = raw_ustar_gz("rime/../evil.txt", b"xx");

        let dst = temp_dir("evil");
        assert!(unpack_rime(&archive, &dst).is_err());
        assert!(!dst.parent().unwrap().join("evil.txt").exists());
        let _ = std::fs::remove_dir_all(dst);
    }

    /// 手工构造含单条目的 gzip ustar 归档（绕过 tar crate 的写入端路径校验）。
    fn raw_ustar_gz(path: &str, content: &[u8]) -> Vec<u8> {
        let mut header = [0u8; 512];
        header[..path.len()].copy_from_slice(path.as_bytes());
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", content.len()).as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].fill(b' ');
        let sum: u32 = header.iter().map(|&b| b as u32).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());

        let mut raw = header.to_vec();
        raw.extend_from_slice(content);
        let pad = (512 - content.len() % 512) % 512;
        raw.extend(std::iter::repeat_n(0u8, pad));
        raw.extend_from_slice(&[0u8; 1024]);

        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&raw).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn archive_name_format() {
        // 2026-09-10 12:00:00 UTC = 1789041600
        let name = archive_name(BackupMode::Full, 1_789_041_600);
        assert_eq!(name, "ximeyi-20260910-120000-full.tar.gz");
        assert_eq!(mode_tag_of(&name), Some("full"));
        assert_eq!(
            mode_tag_of("ximeyi-20260910-120000-config.tar.gz"),
            Some("config")
        );
        assert_eq!(mode_tag_of("random.tar.gz"), None);
    }
}
