//! 极简同步 WebDAV 客户端（ureq，阻塞式，供云备份页使用）。
//!
//! 语义与 `xime-sync-store::webdav`（异步 reqwest 版）对齐：
//! - `put`    → `PUT {base}/{key}`（自动 MKCOL 建父目录）
//! - `get`    → `GET {base}/{key}`（404 → None）
//! - `delete` → `DELETE {base}/{key}`（404 视为成功）
//! - `list`   → `PROPFIND {base}/{prefix} Depth:1`，解析 href + getcontentlength
//! - `test`   → `PROPFIND {base} Depth:0`（探测连通性与认证）
//!
//! 认证：Basic Auth。

use base64::Engine;

/// 单个远端文件条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    /// 服务器返回的资源路径（以 / 开头，用于 get/delete 操作）。
    pub path: String,
    /// 文件名（最后一段，用于展示）。
    pub name: String,
    /// 文件大小（字节；服务器未返回时为 None）。
    pub size: Option<u64>,
}

/// WebDAV 客户端。
pub struct WebDavClient {
    base_url: String,
    /// scheme://host 形式的源（用于把相对 href 还原成可请求的 URL）。
    origin: String,
    username: Option<String>,
    password: Option<String>,
}

impl WebDavClient {
    pub fn new(base_url: String, username: Option<String>, password: Option<String>) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        let origin = base_url
            .split_once("://")
            .map(|(scheme, rest)| {
                let host = rest.split('/').next().unwrap_or("");
                format!("{scheme}://{host}")
            })
            .unwrap_or_default();
        Self {
            base_url,
            origin,
            username,
            password,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn url(&self, key: &str) -> String {
        format!("{}/{}", self.base_url, key)
    }

    /// 发送任意方法请求（含认证头），非 2xx 转换为携带状态码的错误字符串。
    fn send(
        &self,
        method: &'static str,
        url: &str,
        body: Option<Vec<u8>>,
        headers: &[(&str, &str)],
    ) -> Result<ureq::http::Response<ureq::Body>, String> {
        let mut builder = ureq::http::Request::builder()
            .method(method)
            .uri(url)
            .header("User-Agent", "xime-setup");
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            let token =
                base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}").as_bytes());
            builder = builder.header("Authorization", format!("Basic {token}"));
        }
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let request = builder
            .body(body.unwrap_or_default())
            .map_err(|e| format!("构造请求失败: {e}"))?;
        match ureq::run(request) {
            Ok(res) => Ok(res),
            Err(ureq::Error::StatusCode(code)) => Err(format!("HTTP {code}")),
            Err(e) => Err(format!("{e}")),
        }
    }

    /// 递归创建 key 的父目录（MKCOL；405/409 视为已存在）。
    fn ensure_parents(&self, key: &str) -> Result<(), String> {
        let mut dirs: Vec<String> = Vec::new();
        let mut dir = match key.rfind('/') {
            Some(i) => key[..i].to_string(),
            None => return Ok(()),
        };
        while !dir.is_empty() {
            dirs.push(dir.clone());
            dir = match dir.rfind('/') {
                Some(i) => dir[..i].to_string(),
                None => break,
            };
        }
        for d in dirs.into_iter().rev() {
            let url = self.url(&d);
            match self.send("MKCOL", &url, None, &[]) {
                Ok(_) => {}
                // 405 = 资源已存在，409 = 中间目录已存在，均可继续
                Err(e) if e.contains("HTTP 405") || e.contains("HTTP 409") => {}
                Err(e) => return Err(format!("MKCOL {d} 失败: {e}")),
            }
        }
        Ok(())
    }

    /// 上传字节（自动建父目录）。
    pub fn put(&self, key: &str, data: &[u8]) -> Result<(), String> {
        self.ensure_parents(key)?;
        self.send("PUT", &self.url(key), Some(data.to_vec()), &[])
            .map(|_| ())
    }

    /// 下载（404 → None）。`path` 为 list 返回的资源路径。
    pub fn get(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
        match self.send("GET", &format!("{}{}", self.origin, path), None, &[]) {
            Ok(mut res) => {
                let bytes = res
                    .body_mut()
                    .read_to_vec()
                    .map_err(|e| format!("读取响应失败: {e}"))?;
                Ok(Some(bytes))
            }
            Err(e) if e.contains("HTTP 404") => Ok(None),
            Err(e) => Err(format!("GET 失败: {e}")),
        }
    }

    /// 删除（404 视为成功）。
    pub fn delete(&self, path: &str) -> Result<(), String> {
        match self.send("DELETE", &format!("{}{}", self.origin, path), None, &[]) {
            Ok(_) => Ok(()),
            Err(e) if e.contains("HTTP 404") => Ok(()),
            Err(e) => Err(format!("DELETE 失败: {e}")),
        }
    }

    /// 列出 `prefix`（目录）下的文件（Depth:1，404 → 空列表）。
    pub fn list(&self, prefix: &str) -> Result<Vec<RemoteFile>, String> {
        let prefix = prefix.trim_matches('/');
        let url = if prefix.is_empty() {
            self.base_url.clone()
        } else {
            self.url(prefix)
        };
        match self.send("PROPFIND", &url, None, &[("Depth", "1")]) {
            Ok(mut res) => {
                let body = res
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| format!("读取响应失败: {e}"))?;
                let base_path = self
                    .base_url
                    .strip_prefix(self.origin.as_str())
                    .unwrap_or("")
                    .trim_end_matches('/')
                    .to_string();
                let scope = if prefix.is_empty() {
                    base_path
                } else {
                    format!("{base_path}/{prefix}")
                };
                Ok(parse_propfind(&body, &self.origin, &scope))
            }
            Err(e) if e.contains("HTTP 404") => Ok(Vec::new()),
            Err(e) => Err(format!("PROPFIND 失败: {e}")),
        }
    }

    /// 连通性测试（PROPFIND 根目录 Depth:0）。
    pub fn test(&self) -> Result<(), String> {
        match self.send("PROPFIND", &self.base_url, None, &[("Depth", "0")]) {
            Ok(_) => Ok(()),
            Err(e) if e.contains("HTTP 401") => Err("认证失败：请检查用户名/密码".to_string()),
            Err(e) => Err(format!("连接失败: {e}")),
        }
    }
}

/// 解析 PROPFIND Depth:1 响应：每个 response 块取 href 与可选 getcontentlength，
/// 过滤目录（href 以 / 结尾）与 scope 路径之外的条目，按路径字典序返回。
fn parse_propfind(body: &str, origin: &str, scope: &str) -> Vec<RemoteFile> {
    let mut out: Vec<RemoteFile> = Vec::new();
    for block in split_responses(body) {
        let Some(href) = find_tag(block, "href") else {
            continue;
        };
        let decoded = percent_decode(&href);
        if decoded.ends_with('/') {
            continue; // 目录
        }
        // 归一化为绝对路径：绝对 href 直接用；相对 href 按 origin+path 语义还原。
        let path = if decoded.starts_with('/') {
            decoded
        } else if let Some(rest) = decoded.strip_prefix(origin) {
            rest.to_string()
        } else {
            format!("/{decoded}")
        };
        let name = path
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or("")
            .to_string();
        if !path.starts_with(scope) || path.len() <= scope.len() {
            continue; // 目录前缀之外的条目（服务器返回多余结果时兜底过滤）
        }
        let size = find_tag(block, "getcontentlength").and_then(|v| v.trim().parse::<u64>().ok());
        out.push(RemoteFile { path, name, size });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// 按 `<response>` 标签切分 PROPFIND 响应（大小写/命名空间前缀均兼容）。
fn split_responses(body: &str) -> Vec<&str> {
    let lower = body.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while let Some((open, content)) = find_tag_open(lb, cursor, b"response") {
        let Some(close) = find_tag_close(lb, content, b"response") else {
            break;
        };
        blocks.push(&body[open..close]);
        cursor = close;
    }
    blocks
}

/// 标签名的 local 部分（去掉 `d:` 之类命名空间前缀）。
fn tag_local_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(c) => &name[c + 1..],
        None => name,
    }
}

/// 从 `from` 起查找 local 名为 `tag` 的开标签，返回（'<' 位置, 标签体起点）。
fn find_tag_open(lb: &[u8], from: usize, tag: &[u8]) -> Option<(usize, usize)> {
    let mut i = from;
    while i < lb.len() {
        if lb[i] != b'<' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        if j < lb.len() && lb[j] == b'/' {
            // 闭标签：整体跳过
            while j < lb.len() && lb[j] != b'>' {
                j += 1;
            }
            i = j.min(lb.len());
            continue;
        }
        let name_start = j;
        while j < lb.len() && (lb[j].is_ascii_alphanumeric() || lb[j] == b':') {
            j += 1;
        }
        let matched = tag_local_name(&lb[name_start..j]) == tag;
        while j < lb.len() && lb[j] != b'>' {
            j += 1;
        }
        if j >= lb.len() {
            return None;
        }
        let content_start = j + 1;
        if matched {
            return Some((i, content_start));
        }
        i = content_start;
    }
    None
}

/// 从 `from` 起查找 local 名为 `tag` 的闭标签，返回 `</` 的位置。
fn find_tag_close(lb: &[u8], from: usize, tag: &[u8]) -> Option<usize> {
    let mut i = from;
    while i < lb.len() {
        if lb[i] != b'<' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        if j >= lb.len() || lb[j] != b'/' {
            // 开标签：跳过名字后按 '>' 推进
            while j < lb.len() && (lb[j].is_ascii_alphanumeric() || lb[j] == b':') {
                j += 1;
            }
            while j < lb.len() && lb[j] != b'>' {
                j += 1;
            }
            i = j.min(lb.len());
            continue;
        }
        j += 1;
        let name_start = j;
        while j < lb.len() && (lb[j].is_ascii_alphanumeric() || lb[j] == b':') {
            j += 1;
        }
        let name = &lb[name_start..j];
        while j < lb.len() && lb[j] != b'>' {
            j += 1;
        }
        if j >= lb.len() {
            return None;
        }
        if tag_local_name(name) == tag {
            return Some(i);
        }
        i = j + 1;
    }
    None
}

/// 在块内查找首个标签文本内容（大小写不敏感；兼容命名空间前缀如 `<d:href>`）。
fn find_tag(block: &str, tag: &str) -> Option<String> {
    let lower = block.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let (_, content_start) = find_tag_open(lb, 0, tag.as_bytes())?;
    let close_start = find_tag_close(lb, content_start, tag.as_bytes())?;
    Some(block[content_start..close_start].to_string())
}

/// 简单 percent-decode（%XX），与 xime-sync-store 的实现一致。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/dav/xime/</d:href>
    <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/dav/xime/backup/ximeyi-20260910-120000-full.tar.gz</d:href>
    <d:propstat><d:prop><d:getcontentlength>1048576</d:getcontentlength></d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/dav/xime/backup/ximeyi-20260909-080000-config.tar.gz</d:href>
  </d:response>
  <d:response>
    <d:href>/dav/other/file.txt</d:href>
  </d:response>
</d:multistatus>"#;

    #[test]
    fn propfind_parse_filters_dirs_and_foreign() {
        let files = parse_propfind(BODY, "https://dav.example.com", "/dav/xime/backup");
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[0].path,
            "/dav/xime/backup/ximeyi-20260909-080000-config.tar.gz"
        );
        assert_eq!(files[0].name, "ximeyi-20260909-080000-config.tar.gz");
        assert_eq!(files[0].size, None);
        assert_eq!(
            files[1].path,
            "/dav/xime/backup/ximeyi-20260910-120000-full.tar.gz"
        );
        assert_eq!(files[1].size, Some(1_048_576));
    }

    #[test]
    fn propfind_parse_handles_uppercase_ns() {
        let body = r#"<D:response><D:href>/x/a.tar.gz</D:href><D:propstat><D:prop><D:getcontentlength>7</D:getcontentlength></D:prop></D:propstat></D:response>"#;
        let files = parse_propfind(body, "https://dav.example.com", "/x");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "/x/a.tar.gz");
        assert_eq!(files[0].name, "a.tar.gz");
        assert_eq!(files[0].size, Some(7));
    }

    #[test]
    fn relative_href_normalized_to_absolute_path() {
        let files = parse_propfind(
            "<response><href>dav/xime/backup/b.tar.gz</href></response>",
            "https://dav.example.com",
            "/dav/xime/backup",
        );
        assert_eq!(files[0].path, "/dav/xime/backup/b.tar.gz");
        assert_eq!(files[0].name, "b.tar.gz");
    }

    #[test]
    fn percent_decode_matches_store_impl() {
        assert_eq!(percent_decode("a%20b.tar.gz"), "a b.tar.gz");
        assert_eq!(percent_decode("bad%zz"), "bad%zz");
    }
}
