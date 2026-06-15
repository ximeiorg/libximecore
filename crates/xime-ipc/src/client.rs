use crate::messages::InputContext;

#[derive(Debug, Clone)]
pub struct DaemonStatus {
    pub version: String,
    pub running: bool,
    pub ascii_mode: bool,
    pub current_schema: String,
}

#[async_trait::async_trait]
pub trait DaemonClient: Send {
    /// Reload daemon config (style, etc.)
    async fn reload_config(&self) -> Result<(), IpcError>;

    /// Trigger full Rime deploy + reload
    async fn deploy(&self) -> Result<(), IpcError>;

    /// Get daemon version string
    async fn get_version(&self) -> Result<String, IpcError>;

    /// Get current input context (preedit, candidates, etc.)
    async fn get_context(&self) -> Result<InputContext, IpcError>;

    /// Get current daemon/input status
    async fn get_status(&self) -> Result<DaemonStatus, IpcError>;

    /// Toggle ascii mode
    async fn toggle_ascii_mode(&self) -> Result<bool, IpcError>;

    /// Gracefully shut down the daemon
    async fn shutdown(&self) -> Result<(), IpcError>;
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Request failed: {0}")]
    RequestFailed(String),
    #[error("Daemon not running")]
    NotRunning,
    #[error("Timeout")]
    Timeout,
}
