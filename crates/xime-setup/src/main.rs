mod components;
mod pages;
mod state;
mod theme;

#[cfg(target_os = "linux")]
mod webdav;

use gpui::*;
use pages::SettingsApp;
use rust_embed::{Embed, RustEmbed};
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/assets"]
#[include = "image/*.png"]
#[include = "icons/*.svg"]
struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }
        Ok(<Assets as Embed>::get(path).map(|x| x.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(<Assets as Embed>::iter()
            .filter_map(|p| {
                if p.starts_with(path) {
                    Some(p.into())
                } else {
                    None
                }
            })
            .collect())
    }
}

#[cfg(target_os = "linux")]
fn try_acquire_singleton_lock() -> bool {
    use std::fs::File;
    use std::path::PathBuf;

    let lock_path = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .map(|home| PathBuf::from(home).join(".local/share/xime"))
                .ok()
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("xime-setup.lock");

    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    match File::create(&lock_path) {
        Ok(f) => {
            use nix::fcntl::{Flock, FlockArg};
            use std::mem::forget;
            match Flock::lock(f, FlockArg::LockExclusiveNonblock) {
                Ok(flock) => {
                    forget(flock);
                    true
                }
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

#[cfg(target_os = "windows")]
fn try_acquire_singleton_lock() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Threading::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const MUTEX_NAME: &str = "XimeSetupSingleInstanceMutex";
    const WINDOW_CLASS: &str = "GPUI Window";
    const WINDOW_TITLE: &str = "Xime 设置";

    let mutex_name_wide: Vec<u16> = MUTEX_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let already_running = unsafe {
        let handle = CreateMutexW(None, false, PCWSTR(mutex_name_wide.as_ptr()));
        if handle.is_ok() {
            let last_error = GetLastError();
            if last_error == ERROR_ALREADY_EXISTS {
                let class_wide: Vec<u16> = WINDOW_CLASS
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                let title_wide: Vec<u16> = WINDOW_TITLE
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();

                if let Ok(hwnd) = FindWindowW(PCWSTR(class_wide.as_ptr()), PCWSTR(title_wide.as_ptr())) {
                    if !hwnd.0.is_null() {
                        if IsIconic(hwnd).as_bool() {
                            let _ = ShowWindow(hwnd, SW_RESTORE);
                        }
                        let _ = SetForegroundWindow(hwnd);
                    }
                }
                true
            } else {
                false
            }
        } else {
            false
        }
    };

    !already_running
}

fn main() {
    if !try_acquire_singleton_lock() {
        tracing::info!("xime-setup is already running, exiting...");
        return;
    }

    Application::new().run(|cx: &mut App| {
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(800.0), px(640.0)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Xime 设置".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                #[cfg(target_os = "linux")]
                window_decorations: Some(WindowDecorations::Client),
                ..Default::default()
            },
            |_window, cx| cx.new(SettingsApp::new),
        );
    });
}
