use crate::error::{Error, Result};
use crate::get_api;
use std::ffi::{CStr, CString};
use std::ptr;

fn get_levers_api() -> Option<*const librime_sys2::RimeLeversApi> {
    librime_sys2::rime_get_levers_api()
}

pub struct CustomSettings {
    settings: *mut librime_sys2::RimeCustomSettings,
    api: *const librime_sys2::RimeLeversApi,
}

impl CustomSettings {
    pub fn new(config_id: &str, generator_id: &str) -> Result<Self> {
        let api = get_levers_api().ok_or(Error::FunctionNotAvailable("rime_get_levers_api"))?;

        let config_id_c = CString::new(config_id)?;
        let generator_id_c = CString::new(generator_id)?;

        unsafe {
            let init_func = (*api)
                .custom_settings_init
                .ok_or(Error::FunctionNotAvailable("custom_settings_init"))?;
            let settings = init_func(config_id_c.as_ptr(), generator_id_c.as_ptr());
            if settings.is_null() {
                return Err(Error::FunctionNotAvailable(
                    "custom_settings_init returned null",
                ));
            }

            let load_func = (*api)
                .load_settings
                .ok_or(Error::FunctionNotAvailable("load_settings"))?;
            load_func(settings);

            Ok(Self { settings, api })
        }
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        unsafe {
            let key_c = CString::new(key).ok()?;
            let rime_api = get_api();
            if rime_api.is_null() {
                return None;
            }

            let get_config = (*self.api).settings_get_config?;
            let mut config = librime_sys2::RimeConfig {
                ptr: ptr::null_mut(),
            };
            if get_config(self.settings, &mut config) == 0 {
                return None;
            }

            let get_cstring = (*rime_api).config_get_cstring?;
            let value_ptr = get_cstring(&mut config, key_c.as_ptr());
            if value_ptr.is_null() {
                return None;
            }

            CStr::from_ptr(value_ptr)
                .to_str()
                .ok()
                .map(|s| s.to_owned())
        }
    }

    pub fn get_int(&self, key: &str) -> Option<i32> {
        unsafe {
            let key_c = CString::new(key).ok()?;
            let rime_api = get_api();
            if rime_api.is_null() {
                return None;
            }

            let get_config = (*self.api).settings_get_config?;
            let mut config = librime_sys2::RimeConfig {
                ptr: ptr::null_mut(),
            };
            if get_config(self.settings, &mut config) == 0 {
                return None;
            }

            let get_int = (*rime_api).config_get_int?;
            let mut value: std::os::raw::c_int = 0;
            if get_int(&mut config, key_c.as_ptr(), &mut value) == 0 {
                return None;
            }

            Some(value)
        }
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        unsafe {
            let key_c = CString::new(key).ok()?;
            let rime_api = get_api();
            if rime_api.is_null() {
                return None;
            }

            let get_config = (*self.api).settings_get_config?;
            let mut config = librime_sys2::RimeConfig {
                ptr: ptr::null_mut(),
            };
            if get_config(self.settings, &mut config) == 0 {
                return None;
            }

            let get_bool = (*rime_api).config_get_bool?;
            let mut value: std::os::raw::c_int = 0;
            if get_bool(&mut config, key_c.as_ptr(), &mut value) == 0 {
                return None;
            }

            Some(value != 0)
        }
    }

    pub fn set_int(&self, key: &str, value: i32) -> Result<()> {
        unsafe {
            let key_c = CString::new(key)?;
            let func = (*self.api)
                .customize_int
                .ok_or(Error::FunctionNotAvailable("customize_int"))?;
            if func(self.settings, key_c.as_ptr(), value) == 0 {
                return Err(Error::FunctionNotAvailable("customize_int returned 0"));
            }
        }
        Ok(())
    }

    pub fn set_bool(&self, key: &str, value: bool) -> Result<()> {
        unsafe {
            let key_c = CString::new(key)?;
            let func = (*self.api)
                .customize_bool
                .ok_or(Error::FunctionNotAvailable("customize_bool"))?;
            if func(self.settings, key_c.as_ptr(), if value { 1 } else { 0 }) == 0 {
                return Err(Error::FunctionNotAvailable("customize_bool returned 0"));
            }
        }
        Ok(())
    }

    pub fn set_string(&self, key: &str, value: &str) -> Result<()> {
        unsafe {
            let key_c = CString::new(key)?;
            let value_c = CString::new(value)?;
            let func = (*self.api)
                .customize_string
                .ok_or(Error::FunctionNotAvailable("customize_string"))?;
            if func(self.settings, key_c.as_ptr(), value_c.as_ptr()) == 0 {
                return Err(Error::FunctionNotAvailable(
                    "customize_string returned 0",
                ));
            }
        }
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        unsafe {
            let func = (*self.api)
                .save_settings
                .ok_or(Error::FunctionNotAvailable("save_settings"))?;
            if func(self.settings) == 0 {
                return Err(Error::FunctionNotAvailable("save_settings returned 0"));
            }
        }
        Ok(())
    }
}

impl Drop for CustomSettings {
    fn drop(&mut self) {
        unsafe {
            if let Some(destroy) = (*self.api).custom_settings_destroy {
                destroy(self.settings);
            }
        }
    }
}

pub fn deploy_all() -> Result<()> {
    use std::ffi::CString;

    unsafe {
        let api = get_api();
        if api.is_null() {
            return Err(Error::ApiNotInitialized);
        }

        if let Some(deploy) = (*api).deploy {
            if deploy() == 0 {
                return Err(Error::FunctionNotAvailable("deploy returned 0"));
            }
        }

        let config_name = CString::new("xime.yaml").unwrap_or_default();
        let version_key = CString::new("config_version").unwrap_or_default();
        if let Some(deploy_config) = (*api).deploy_config_file {
            deploy_config(config_name.as_ptr(), version_key.as_ptr());
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct SchemaInfo {
    pub schema_id: String,
    pub name: String,
}

pub struct SwitcherSettings {
    settings: *mut librime_sys2::RimeSwitcherSettings,
    api: *const librime_sys2::RimeLeversApi,
}

impl SwitcherSettings {
    pub fn new() -> Result<Self> {
        let api =
            get_levers_api().ok_or(Error::FunctionNotAvailable("rime_get_levers_api"))?;

        unsafe {
            let init = (*api)
                .switcher_settings_init
                .ok_or(Error::FunctionNotAvailable("switcher_settings_init"))?;
            let settings = init();
            if settings.is_null() {
                return Err(Error::FunctionNotAvailable(
                    "switcher_settings_init returned null",
                ));
            }
            Ok(Self { settings, api })
        }
    }

    pub fn get_available_schema_list(&self) -> Result<Vec<String>> {
        let mut list = librime_sys2::RimeSchemaList {
            size: 0,
            list: ptr::null_mut(),
        };

        unsafe {
            let func = (*self.api)
                .get_available_schema_list
                .ok_or(Error::FunctionNotAvailable("get_available_schema_list"))?;

            if func(self.settings, &mut list) == 0 {
                return Err(Error::FunctionNotAvailable(
                    "get_available_schema_list returned 0",
                ));
            }

            let mut schemas = Vec::new();
            for i in 0..list.size {
                let item = &*list.list.add(i);
                let id = CStr::from_ptr(item.schema_id)
                    .to_string_lossy()
                    .to_string();
                schemas.push(id);
            }

            if let Some(destroy) = (*self.api).schema_list_destroy {
                destroy(&mut list);
            }

            Ok(schemas)
        }
    }

    pub fn select_schemas(&self, schema_ids: &[&str]) -> Result<()> {
        let c_strings: Vec<CString> = schema_ids
            .iter()
            .map(|id| CString::new(*id).unwrap())
            .collect();
        let mut ptrs: Vec<*const std::ffi::c_char> =
            c_strings.iter().map(|c| c.as_ptr()).collect();

        unsafe {
            let func = (*self.api)
                .select_schemas
                .ok_or(Error::FunctionNotAvailable("select_schemas"))?;
            if func(self.settings, ptrs.as_mut_ptr(), schema_ids.len() as i32) == 0 {
                return Err(Error::FunctionNotAvailable("select_schemas returned 0"));
            }
        }
        Ok(())
    }
}

impl Drop for SwitcherSettings {
    fn drop(&mut self) {
        // The levers API doesn't expose a destroy for switcher settings
    }
}
