//! 宿主只读 API（按 manifest capabilities 门禁注入）。
//!
//! 对齐 Android 版 `ClipboardHostApiImpl` / `QuickSendHostApiImpl` 的只读语义：
//! 插件声明 `clipboard_read: true` / `quick_send_read: true` 后，宿主在构建
//! host 表时注入 `host.clipboard` / `host.quickSend`；未声明或宿主未提供
//! 实现时不注入（Lua 侧不可见）。

use std::sync::Arc;

/// 剪贴板条目（只读快照）。
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardEntryInfo {
    pub text: String,
    /// Unix 毫秒时间戳。
    pub timestamp: i64,
    pub is_pinned: bool,
}

/// 快捷发送条目（只读快照）。
#[derive(Debug, Clone, PartialEq)]
pub struct QuickSendItemInfo {
    pub text: String,
    /// 触发编码（如 "dh"），可为空串。
    pub code: String,
}

/// 宿主剪贴板只读 API。
pub trait ClipboardReadApi: Send + Sync {
    /// 最近条目（timestamp 倒序，最多 `limit` 条）。
    fn recent(&self, limit: usize) -> Vec<ClipboardEntryInfo>;
}

/// 宿主快捷发送只读 API。
pub trait QuickSendReadApi: Send + Sync {
    /// 全部快捷发送条目（timestamp 倒序）。
    fn list(&self) -> Vec<QuickSendItemInfo>;
}

/// 宿主 API 集合（None = 宿主未提供，即使插件声明了能力也不注入）。
#[derive(Clone, Default)]
pub struct HostApis {
    pub clipboard: Option<Arc<dyn ClipboardReadApi>>,
    pub quick_send: Option<Arc<dyn QuickSendReadApi>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeClipboard;
    impl ClipboardReadApi for FakeClipboard {
        fn recent(&self, limit: usize) -> Vec<ClipboardEntryInfo> {
            (0..limit)
                .map(|i| ClipboardEntryInfo {
                    text: format!("t{i}"),
                    timestamp: 100 + i as i64,
                    is_pinned: false,
                })
                .collect()
        }
    }

    #[test]
    fn test_host_apis_default_empty() {
        let apis = HostApis::default();
        assert!(apis.clipboard.is_none());
        assert!(apis.quick_send.is_none());
    }

    #[test]
    fn test_fake_clipboard_recent() {
        let apis: Arc<dyn ClipboardReadApi> = Arc::new(FakeClipboard);
        assert_eq!(apis.recent(2).len(), 2);
        assert_eq!(apis.recent(0).len(), 0);
    }
}
