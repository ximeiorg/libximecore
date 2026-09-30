pub mod commit;
pub mod context;
pub mod error;
pub mod key;
pub mod levers;
pub mod session;
pub mod status;
pub mod traits;

pub use key::{
    get_key_modifiers, vk_to_xk, VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT,
    VK_NEXT, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA,
    VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB,
    VK_UP, XK_SEMICOLON,
};
pub use key::{
    KeyCode, Modifier, K_ALT_MASK, K_CONTROL_MASK, K_LOCK_MASK, K_RELEASE_MASK, K_SHIFT_MASK,
    XK_SHIFT_L,
};
pub use levers::{
    backup_user_dict, deploy_all, export_user_dict, import_user_dict, list_user_dicts,
    restore_user_dict, CustomSettings, SchemaInfo, SwitcherSettings,
};
pub use librime_sys2::rime_struct;
pub use session::Session;
pub use traits::Traits;

use once_cell::sync::Lazy;
use std::ffi::CStr;
use std::ffi::CString;
use std::sync::Mutex;

use librime_sys2::rime_get_api;

struct RimeApiWrapper(*mut librime_sys2::RimeApi);
unsafe impl Send for RimeApiWrapper {}
unsafe impl Sync for RimeApiWrapper {}

static RIME_API: Lazy<RimeApiWrapper> = Lazy::new(|| RimeApiWrapper(rime_get_api()));

static DEPLOY_RESULT: Lazy<Mutex<Option<DeployResult>>> = Lazy::new(|| Mutex::new(None));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployResult {
    Success,
    Failure,
}

macro_rules! rime_api_call {
    ($f:tt, $($arg:tt)*) => {
        unsafe {
            let api = $crate::get_api();
            if api.is_null() {
                return Err($crate::error::Error::ApiNotInitialized);
            }
            let func = (*api).$f;
            if func.is_none() {
                return Err($crate::error::Error::FunctionNotAvailable(stringify!($f)));
            }
            func.unwrap()($($arg)*)
        }
    };
    ($f:tt) => {
        unsafe {
            let api = $crate::get_api();
            if api.is_null() {
                return Err($crate::error::Error::ApiNotInitialized);
            }
            let func = (*api).$f;
            if func.is_none() {
                return Err($crate::error::Error::FunctionNotAvailable(stringify!($f)));
            }
            func.unwrap()()
        }
    };
}

pub fn get_api() -> *mut librime_sys2::RimeApi {
    RIME_API.0
}

pub fn setup(traits: &mut traits::Traits) {
    unsafe {
        let api = get_api();
        if !api.is_null() {
            if let Some(setup) = (*api).setup {
                setup(&mut traits.inner);
            }
        }
    }
}

pub fn initialize(traits: &mut traits::Traits) -> error::Result<()> {
    unsafe {
        let api = get_api();
        if api.is_null() {
            return Err(error::Error::ApiNotInitialized);
        }
        if let Some(initialize) = (*api).initialize {
            initialize(&mut traits.inner);
        }
        if let Some(set_notification_handler) = (*api).set_notification_handler {
            set_notification_handler(Some(notification_handler), std::ptr::null_mut());
        }
    }
    Ok(())
}

pub fn finalize() {
    unsafe {
        let api = get_api();
        if !api.is_null() {
            if let Some(finalize) = (*api).finalize {
                finalize();
            }
        }
    }
}

pub fn start_maintenance(full_check: bool) -> error::Result<()> {
    let result = rime_api_call!(start_maintenance, full_check as i32);
    if result == 0 {
        Err(error::Error::StartMaintenance)
    } else {
        Ok(())
    }
}

pub fn join_maintenance_thread() {
    unsafe {
        let api = get_api();
        if !api.is_null() {
            if let Some(join) = (*api).join_maintenance_thread {
                join();
            }
        }
    }
}

pub fn is_maintenance_mode() -> bool {
    unsafe {
        let api = get_api();
        if api.is_null() {
            return false;
        }
        if let Some(is_maintenance) = (*api).is_maintenance_mode {
            is_maintenance() != 0
        } else {
            false
        }
    }
}

pub fn full_deploy_and_wait() -> DeployResult {
    *DEPLOY_RESULT.lock().unwrap() = None;
    if start_maintenance(true).is_err() {
        return DeployResult::Failure;
    }
    join_maintenance_thread();
    if let Some(result) = *DEPLOY_RESULT.lock().unwrap() {
        result
    } else {
        DeployResult::Failure
    }
}

pub fn sync_user_data() -> error::Result<()> {
    let result = rime_api_call!(sync_user_data);
    if result == 0 {
        Err(error::Error::SyncUserData)
    } else {
        Ok(())
    }
}

// ------------------------------------------------------------------
// 用户词典管理（对齐 weasel DictManagementDialog 的四个操作 + 列表）
// ------------------------------------------------------------------

/// 用户数据同步目录（快照存放位置；rime 由 installation.yaml 的 sync_dir 决定）。
pub fn get_user_data_sync_dir() -> String {
    unsafe {
        let api = get_api();
        if api.is_null() {
            return String::new();
        }
        let Some(func) = (*api).get_user_data_sync_dir else {
            return String::new();
        };
        let mut buf: [std::ffi::c_char; 512] = [0 as std::ffi::c_char; 512];
        (func)(buf.as_mut_ptr(), buf.len());
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

extern "C" fn notification_handler(
    _context: *mut std::ffi::c_void,
    _session_id: librime_sys2::RimeSessionId,
    message_type: *const std::ffi::c_char,
    message_value: *const std::ffi::c_char,
) {
    use std::ffi::CStr;
    unsafe {
        let msg_type = CStr::from_ptr(message_type).to_string_lossy();
        let msg_value = CStr::from_ptr(message_value).to_string_lossy();

        if msg_type == "deploy" {
            let mut result = DEPLOY_RESULT.lock().unwrap();
            match msg_value.as_ref() {
                "success" => *result = Some(DeployResult::Success),
                "failure" => *result = Some(DeployResult::Failure),
                _ => {}
            }
        }
    }
}

pub fn create_session() -> error::Result<session::Session> {
    let session_id = rime_api_call!(create_session);
    let session = session::Session::new(session_id);
    if !session.find_session() {
        Err(error::Error::CreateSession)
    } else {
        Ok(session)
    }
}
