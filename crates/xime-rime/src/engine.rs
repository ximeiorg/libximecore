use librime::{
    create_session, finalize, full_deploy_and_wait, get_api, initialize, is_maintenance_mode,
    join_maintenance_thread, setup, start_maintenance, DeployResult, Session, Traits,
};
use std::path::{Path, PathBuf};

pub struct RimeEngine {
    session: Session,
    initialized: bool,
    shared_data_dir: PathBuf,
    user_data_dir: PathBuf,
    distribution_name: String,
    /// 用户当前选中的方案：redeploy/deploy 重建会话后必须重新选择，
    /// 否则 librime 新会话回落到 schema_list 第一个（打字时"自动换方案"）。
    selected_schema: Option<String>,
}

unsafe impl Send for RimeEngine {}
unsafe impl Sync for RimeEngine {}

impl RimeEngine {
    pub fn new(
        shared_data_dir: &Path,
        user_data_dir: &Path,
        distribution_name: &str,
    ) -> Result<Self, RimeError> {
        let mut traits = Traits::new();
        traits
            .set_shared_data_dir(shared_data_dir.to_str().unwrap_or(""))
            .set_user_data_dir(user_data_dir.to_str().unwrap_or(""))
            .set_distribution_name(distribution_name)
            .set_distribution_code_name(distribution_name)
            .set_distribution_version("1.0")
            .set_app_name("rime.xime")
            .set_min_log_level(1);

        setup(&mut traits);
        initialize(&mut traits).map_err(|e| RimeError::InitFailed(e.to_string()))?;

        let need_maintenance = start_maintenance(false);
        if need_maintenance.is_ok() {
            join_maintenance_thread();
        }

        let session =
            create_session().map_err(|e| RimeError::SessionCreateFailed(e.to_string()))?;

        Ok(Self {
            session,
            initialized: true,
            shared_data_dir: shared_data_dir.to_path_buf(),
            user_data_dir: user_data_dir.to_path_buf(),
            distribution_name: distribution_name.to_string(),
            selected_schema: None,
        })
    }

    pub fn process_key(&mut self, keycode: i32, modifiers: i32) -> bool {
        self.session.process_key(keycode, modifiers)
    }

    pub fn commit_composition(&mut self) -> bool {
        unsafe {
            let api = get_api();
            if api.is_null() {
                return false;
            }
            if let Some(commit) = (*api).commit_composition {
                commit(self.session.session_id()) != 0
            } else {
                false
            }
        }
    }

    pub fn clear_composition(&mut self) {
        unsafe {
            let api = get_api();
            if !api.is_null() {
                if let Some(clear) = (*api).clear_composition {
                    clear(self.session.session_id());
                }
            }
        }
    }

    pub fn get_commit(&self) -> Option<String> {
        self.session.commit().map(|c| c.text().to_string())
    }

    pub fn get_composition(&self) -> Option<Composition> {
        self.session.context().map(|ctx| {
            let comp = ctx.composition();
            Composition {
                length: comp.length,
                cursor_pos: comp.cursor_pos,
                sel_start: comp.sel_start,
                sel_end: comp.sel_end,
                preedit: comp.preedit.map(|s: &str| s.to_string()),
            }
        })
    }

    pub fn get_candidates(&self) -> CandidateList {
        self.session
            .context()
            .map(|ctx| {
                let menu = ctx.menu();
                let candidates = menu
                    .candidates
                    .iter()
                    .map(|c| Candidate {
                        text: c.text.to_string(),
                        comment: c.comment.map(|s: &str| s.to_string()),
                    })
                    .collect();

                CandidateList {
                    candidates,
                    highlighted: menu.highlighted_candidate_index,
                    page_no: menu.page_no,
                    is_last_page: menu.is_last_page,
                }
            })
            .unwrap_or_else(|| CandidateList {
                candidates: Vec::new(),
                highlighted: 0,
                page_no: 0,
                is_last_page: true,
            })
    }

    pub fn is_composing(&self) -> bool {
        self.session
            .status()
            .map(|s| s.is_composing)
            .unwrap_or(false)
    }

    pub fn is_ascii_mode(&self) -> bool {
        self.session
            .status()
            .map(|s| s.is_ascii_mode)
            .unwrap_or(false)
    }

    pub fn get_status(&self) -> Option<RimeEngineStatus> {
        self.session.status().ok().map(|s| RimeEngineStatus {
            is_composing: s.is_composing,
            is_ascii_mode: s.is_ascii_mode,
            schema_id: s.schema_id().to_string(),
            schema_name: s.schema_name().to_string(),
        })
    }

    pub fn select_candidate(&mut self, index: usize) -> bool {
        unsafe {
            let api = get_api();
            if api.is_null() {
                return false;
            }
            if let Some(select) = (*api).select_candidate {
                select(self.session.session_id(), index) != 0
            } else if let Some(select_on_page) = (*api).select_candidate_on_current_page {
                select_on_page(self.session.session_id(), index) != 0
            } else {
                false
            }
        }
    }

    pub fn change_page(&mut self, backward: bool) -> bool {
        unsafe {
            let api = get_api();
            if api.is_null() {
                return false;
            }
            if let Some(change) = (*api).change_page {
                change(self.session.session_id(), if backward { 1 } else { 0 }) != 0
            } else {
                false
            }
        }
    }

    pub fn get_input(&self) -> Option<String> {
        self.session.get_input().map(|s: &str| s.to_string())
    }

    pub fn set_input(&mut self, input: &str) -> bool {
        unsafe {
            let api = get_api();
            if api.is_null() {
                return false;
            }
            if let Some(set_input) = (*api).set_input {
                let c_input = std::ffi::CString::new(input).unwrap();
                set_input(self.session.session_id(), c_input.as_ptr()) != 0
            } else {
                false
            }
        }
    }

    pub fn set_option(&mut self, option: &str, value: bool) {
        let _ = self.session.set_option(option, value);
    }

    pub fn get_option(&self, option: &str) -> Option<bool> {
        self.session.get_option(option).ok()
    }

    pub fn get_schema_list(&self) -> Vec<(String, String)> {
        unsafe {
            let api = get_api();
            if api.is_null() {
                return Vec::new();
            }

            let mut list = librime_sys2::RimeSchemaList {
                size: 0,
                list: std::ptr::null_mut(),
            };

            let mut result = Vec::new();
            if let Some(get_list) = (*api).get_schema_list {
                if get_list(&mut list) != 0 {
                    for i in 0..list.size {
                        let item = list.list.add(i);
                        let id = std::ffi::CStr::from_ptr((*item).schema_id)
                            .to_string_lossy()
                            .to_string();
                        let name = std::ffi::CStr::from_ptr((*item).name)
                            .to_string_lossy()
                            .to_string();
                        result.push((id, name));
                    }
                    if let Some(free) = (*api).free_schema_list {
                        free(&mut list);
                    }
                }
            }
            result
        }
    }

    /// 方案是否已部署（build/ 里有部署产物）。只存在 `.schema.yaml` 而未部署的
    /// 方案没有编译词典，select 进会话会得到一个「所有按键不组词」的死会话
    /// （librime 加载 Schema 配置成功，但翻译器无词典可用）。
    fn schema_deployed(&self, schema_id: &str) -> bool {
        self.user_data_dir
            .join("build")
            .join(format!("{schema_id}.schema.yaml"))
            .is_file()
    }

    pub fn select_schema(&mut self, schema_id: &str) -> bool {
        if !self.schema_deployed(schema_id) {
            return false;
        }
        if self.session.select_schema(schema_id).is_ok() {
            self.selected_schema = Some(schema_id.to_string());
            true
        } else {
            false
        }
    }

    pub fn get_current_schema(&self) -> Option<String> {
        self.session
            .status()
            .ok()
            .map(|s| s.schema_id().to_string())
    }

    pub fn redeploy(&mut self) -> bool {
        self.initialized = false;
        finalize();

        let mut traits = Traits::new();
        traits
            .set_shared_data_dir(self.shared_data_dir.to_str().unwrap_or(""))
            .set_user_data_dir(self.user_data_dir.to_str().unwrap_or(""))
            .set_distribution_name(&self.distribution_name)
            .set_distribution_code_name(&self.distribution_name)
            .set_distribution_version("1.0")
            .set_app_name("rime.xime")
            .set_min_log_level(1);

        setup(&mut traits);
        if initialize(&mut traits).is_err() {
            return false;
        }

        let result = full_deploy_and_wait();

        if is_maintenance_mode() {
            join_maintenance_thread();
        }

        if self.recreate_session() {
            self.initialized = true;
            result == DeployResult::Success
        } else {
            false
        }
    }

    pub fn deploy(&mut self) -> bool {
        if full_deploy_and_wait() != DeployResult::Success {
            return false;
        }
        // Deployment finalizes and reinitializes the Rime engine,
        // which invalidates the existing session. Create a new one.
        self.recreate_session()
    }

    /// 重建会话并恢复用户选中的方案（redeploy / deploy / with_user_dict_closed 共用）。
    ///
    /// 返回会话是否创建成功；方案恢复失败不算失败（只清掉记录走默认方案），
    /// 语义与原先两处内联写法完全一致。
    fn recreate_session(&mut self) -> bool {
        match create_session() {
            Ok(session) => {
                self.session = session;
                // 新会话默认回落 schema_list 第一个，必须恢复用户选中的方案。
                // 选中的方案若已不在部署产物里（被移出方案列表后重新部署），
                // 放弃恢复并清掉记录——重选一个未部署方案会固化死会话。
                if let Some(id) = self.selected_schema.clone() {
                    if self.schema_deployed(&id) {
                        let _ = self.session.select_schema(&id);
                    } else {
                        self.selected_schema = None;
                    }
                }
                true
            }
            Err(_) => false,
        }
    }

    /// 在「用户词典已关闭」的状态下执行一次操作。
    ///
    /// librime 的 `user_dict_manager.h` 有明确要求：Backup/Restore/Export/Import
    /// 之前用户词典必须处于关闭状态。本引擎只有当前这一个会话持有它，
    /// 所以「关闭」= 先销毁会话，操作完再重建（对齐安卓版 `withUserDictClosed`）。
    ///
    /// 代价：正在输入的句子（composition）会丢掉、`ascii_mode` 等会话级开关回落
    /// 默认——调用方要在界面上说明「写入会打断正在进行的输入」。方案选择照常恢复。
    /// 无论 op 成败都会重建会话；重建失败再退到完整的 [`Self::redeploy`] 自救，
    /// 仍失败才报错（此时引擎不可用，重启输入法服务才能恢复）。
    pub fn with_user_dict_closed<F, T>(&mut self, op: F) -> Result<T, String>
    where
        F: FnOnce() -> Result<T, String>,
    {
        // 先关掉当前会话（持有 user dict 的唯一入口）。失败也继续——
        // 下一步反正要重建，重建成功与否才是真正的成败。
        let _ = self.session.close();
        let result = op();
        if self.recreate_session() {
            return result;
        }
        if self.redeploy() {
            return result;
        }
        Err("重建 rime 会话失败，输入法服务需要重启".to_string())
    }

    pub fn get_version(&self) -> Option<String> {
        unsafe {
            let api = get_api();
            if api.is_null() {
                return None;
            }
            if let Some(get_ver) = (*api).get_version {
                let ptr = get_ver();
                if ptr.is_null() {
                    None
                } else {
                    std::ffi::CStr::from_ptr(ptr)
                        .to_str()
                        .ok()
                        .map(|s: &str| s.to_string())
                }
            } else {
                None
            }
        }
    }
}

impl Drop for RimeEngine {
    fn drop(&mut self) {
        if self.initialized {
            finalize();
        }
    }
}

#[derive(Debug, Clone)]
pub struct RimeEngineStatus {
    pub is_composing: bool,
    pub is_ascii_mode: bool,
    pub schema_id: String,
    pub schema_name: String,
}

#[derive(Debug, Clone)]
pub struct Composition {
    pub length: usize,
    pub cursor_pos: usize,
    pub sel_start: usize,
    pub sel_end: usize,
    pub preedit: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub text: String,
    pub comment: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CandidateList {
    pub candidates: Vec<Candidate>,
    pub highlighted: usize,
    pub page_no: usize,
    pub is_last_page: bool,
}

#[derive(Debug)]
pub enum RimeError {
    ApiNotFound,
    InitFailed(String),
    SessionCreateFailed(String),
    StatusError(String),
    LockFailed,
}

impl std::fmt::Display for RimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RimeError::ApiNotFound => write!(f, "Rime API not found (rime.dll missing)"),
            RimeError::InitFailed(msg) => write!(f, "Rime initialization failed: {}", msg),
            RimeError::SessionCreateFailed(msg) => {
                write!(f, "Failed to create Rime session: {}", msg)
            }
            RimeError::StatusError(msg) => write!(f, "Failed to get status: {}", msg),
            RimeError::LockFailed => write!(f, "Failed to acquire initialization lock"),
        }
    }
}

impl std::error::Error for RimeError {}
