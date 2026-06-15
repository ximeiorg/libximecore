use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateInfo {
    pub current_page: u32,
    pub total_pages: u32,
    pub highlighted: usize,
    pub is_last_page: bool,
    pub candies: Vec<String>,
    pub comments: Vec<String>,
    pub labels: Vec<String>,
}

impl Default for CandidateInfo {
    fn default() -> Self {
        Self {
            current_page: 0,
            total_pages: 0,
            highlighted: 0,
            is_last_page: false,
            candies: Vec::new(),
            comments: Vec::new(),
            labels: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputContext {
    pub preedit: String,
    pub commit: Option<String>,
    pub candidates: CandidateInfo,
}

impl Default for InputContext {
    fn default() -> Self {
        Self {
            preedit: String::new(),
            commit: None,
            candidates: CandidateInfo::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputMethodStatus {
    pub schema_name: String,
    pub schema_id: String,
    pub ascii_mode: bool,
    pub composing: bool,
}

impl Default for InputMethodStatus {
    fn default() -> Self {
        Self {
            schema_name: String::new(),
            schema_id: String::new(),
            ascii_mode: false,
            composing: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcCommand {
    Echo,
    ProcessKeyEvent,
    UpdateInputPosition,
    FocusIn,
    FocusOut,
    SelectCandidate,
    ChangePage,
    CommitComposition,
    ClearComposition,
    Shutdown,
    ToggleAsciiMode,
    ReloadConfig,
    GetSchemaList,
    SelectSchema,
    Deploy,
    GetStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEventData {
    pub keycode: i32,
    pub modifiers: i32,
}
