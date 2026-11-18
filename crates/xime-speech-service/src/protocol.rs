//! 客户端 ↔ 语音守护进程的 JSON-line 协议。
//!
//! 一行一个 JSON。请求用 `op` 标签区分，响应是固定结构（`ok` + 可选字段）。
//! 故意不引 RPC 框架：只有八条消息，`nc -U socket` 就能手测。

use serde::{Deserialize, Serialize};

/// 协议版本：字段只增不改，客户端按需取。
pub const PROTOCOL_VERSION: u32 = 1;

/// 守护进程的默认 socket 路径（宿主可以覆盖）。
pub const DEFAULT_SOCKET_NAME: &str = "speech.sock";

/// 请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// 当前状态（含最近一次识别文本）。
    Status,
    /// 模型列表。
    Models,
    /// 切换当前模型（会触发后台重新装载）。
    Select { model_id: String },
    /// 导入一个模型目录（复制到模型根目录）。
    Import { path: String },
    /// 删除一个模型目录。
    Remove { model_id: String },
    /// 开始录音（设置页试听与输入法共用）。
    ListenStart,
    /// 结束录音并识别；识别在后台线程进行，结果看 `status.text`。
    ListenStop,
    /// 退出守护进程。
    Shutdown,
}

/// 运行状态。`state` 取值：`idle` / `loading` / `listening` / `transcribing` / `error`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub state: String,
    /// 当前选中的模型 id。
    pub model_id: String,
    pub model_name: String,
    /// 引擎是否已装载完成（false = 还在 loading 或装载失败）。
    pub model_ready: bool,
    /// 推理后端说明，例如 `CPU · qwen3-asr-0.6b`。
    pub provider: String,
    /// 最近一次识别结果（整段识别用；流式听写中是定稿部分的全文）。
    pub text: String,
    /// 输入电平 0-100（只在 listening 时有意义；其余状态为 0）。
    pub level: u8,
    /// 本次听写中**已定稿**的文本：模型已经不会再改它，客户端应当立刻上屏。
    ///
    /// 流式听写按「稳定前缀」推进定稿，所以这个字段单调变长；松手后等于全文。
    pub committed: String,
    /// 未定稿的预览文本：随重解码变化，客户端显示为 preedit（会整体被替换）。
    pub partial: String,
    /// `partial` 的修订号。值没变就不必重画 preedit。
    pub partial_rev: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 模型集合变化计数（客户端据此决定要不要重拉列表）。
    pub models_rev: u64,
    /// 可管理的模型列表（随 status 一起给，省一次往返）。
    pub models: Vec<ModelInfo>,
}

/// 一个可管理的模型（模型根目录下的一个子目录）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub size: String,
    pub downloaded: bool,
    pub selected: bool,
    pub recommended: bool,
}

impl Status {
    pub fn state_idle() -> Self {
        Self {
            state: "idle".to_string(),
            ..Self::default()
        }
    }
}

/// 响应：`ok=false` 时看 `error`；其余字段按请求类型给。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ModelInfo>>,
    /// `select` / `import` / `remove` 之后的提示（可直接展示）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// `listen_stop` 触发的识别是否已排入后台（v1 恒为 None，结果看 status）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted: Option<bool>,
}

impl Response {
    pub fn with_status(status: Status) -> Self {
        Self {
            ok: true,
            status: Some(status),
            ..Self::default()
        }
    }

    pub fn with_models(models: Vec<ModelInfo>, status: Status) -> Self {
        Self {
            ok: true,
            models: Some(models),
            status: Some(status),
            ..Self::default()
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            ..Self::default()
        }
    }
}

/// 编码成一行（含换行）。
pub fn encode_line<T: Serialize>(value: &T) -> Result<String, String> {
    let mut line = serde_json::to_string(value).map_err(|e| e.to_string())?;
    line.push('\n');
    Ok(line)
}

/// 解码一行（容忍行尾换行/空白）。
pub fn decode_line<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, String> {
    serde_json::from_str(line.trim()).map_err(|e| format!("协议解析失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_uses_op_tag() {
        let line = encode_line(&Request::Select {
            model_id: "qwen3-asr-0.6b".into(),
        })
        .unwrap();
        assert!(line.contains("\"op\":\"select\""), "{line}");
        assert!(line.ends_with('\n'));
        let back: Request = decode_line(&line).unwrap();
        assert_eq!(
            back,
            Request::Select {
                model_id: "qwen3-asr-0.6b".into()
            }
        );
    }

    #[test]
    fn unit_variants_roundtrip() {
        for request in [
            Request::Status,
            Request::Models,
            Request::ListenStart,
            Request::ListenStop,
            Request::Shutdown,
        ] {
            let line = encode_line(&request).unwrap();
            assert_eq!(decode_line::<Request>(&line).unwrap(), request);
        }
    }

    #[test]
    fn response_omits_absent_fields() {
        let line = encode_line(&Response::with_status(Status::state_idle())).unwrap();
        assert!(line.contains("\"ok\":true"));
        assert!(line.contains("\"state\":\"idle\""));
        assert!(!line.contains("error"), "空字段不该出现在线上：{line}");
        assert!(!line.contains("\"error\""), "空错误不该出现：{line}");
    }

    #[test]
    fn error_response_carries_message() {
        let response: Response =
            decode_line(&encode_line(&Response::error("没有麦克风")).unwrap()).unwrap();
        assert!(!response.ok);
        assert_eq!(response.error.as_deref(), Some("没有麦克风"));
        assert!(response.status.is_none());
    }

    #[test]
    fn unknown_fields_are_ignored_for_forward_compat() {
        // 旧客户端遇到新字段要能继续工作（字段只增不改）。
        let request: Request = decode_line(r#"{"op":"status","future":1}"#).unwrap();
        assert_eq!(request, Request::Status);
    }
}
