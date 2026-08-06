//! The `AppsUseLightTheme` registry value, watched with
//! `RegNotifyChangeKeyValue`.
//!
//! `RegNotifyChangeKeyValue` with `bAsynchronous = FALSE` blocks the calling
//! thread inside the kernel until the key changes. That is the whole reason
//! there is a thread here and no timer: the thread costs one stack and zero
//! wakeups until the user actually changes appearance, whereas the obvious
//! implementation of "watch a registry value" is a one-second poll.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, KEY_READ, REG_DWORD, REG_NOTIFY_CHANGE_LAST_SET,
    REG_VALUE_TYPE, RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, RegQueryValueExW,
};
use windows::core::PCWSTR;

use crate::capability::{Support, Unavailable};
use crate::theme::{Theme, ThemeHandler, ThemeWatcher, deduplicate, theme_from_apps_use_light_theme};

const KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const VALUE_NAME: &str = "AppsUseLightTheme";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Read the DWORD, or say precisely why it could not be read.
fn read_apps_use_light_theme() -> Result<u32, Unavailable> {
    let path = wide(KEY_PATH);
    let mut hkey = HKEY::default();
    // SAFETY: `path` is NUL-terminated and outlives the call.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(open_error(status));
    }

    let name = wide(VALUE_NAME);
    let mut kind = REG_VALUE_TYPE::default();
    let mut data: u32 = 0;
    let mut size: u32 = core::mem::size_of::<u32>() as u32;
    // SAFETY: `data` is a live u32 and `size` states its exact byte length.
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some((&raw mut data).cast::<u8>()),
            Some(&mut size),
        )
    };
    // SAFETY: `hkey` came from a successful open and is not used again.
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if status != ERROR_SUCCESS {
        return Err(Unavailable::service_missing(format!(
            "{VALUE_NAME} is absent from HKCU\\{KEY_PATH} (error {}). Windows only writes it once \
             the user has visited Settings > Personalisation > Colours; treat its absence as the \
             light default.",
            status.0
        )));
    }
    if kind != REG_DWORD {
        return Err(Unavailable::runtime_error(format!(
            "{VALUE_NAME} has registry type {}, expected REG_DWORD",
            kind.0
        )));
    }
    Ok(data)
}

fn open_error(status: WIN32_ERROR) -> Unavailable {
    Unavailable::service_missing(format!(
        "cannot open HKCU\\{KEY_PATH} (error {}); this key exists on Windows 10 1809 and later",
        status.0
    ))
}

pub(crate) struct RegistryThemeWatcher {
    handler: Arc<Mutex<Option<ThemeHandler>>>,
    listener_started: AtomicBool,
}

impl RegistryThemeWatcher {
    pub fn connect() -> Result<Self, Unavailable> {
        read_apps_use_light_theme()?;
        Ok(Self { handler: Arc::new(Mutex::new(None)), listener_started: AtomicBool::new(false) })
    }
}

impl ThemeWatcher for RegistryThemeWatcher {
    fn capability(&self) -> Support {
        Support::from_result(read_apps_use_light_theme())
    }

    fn current(&self) -> Result<Theme, Unavailable> {
        Ok(theme_from_apps_use_light_theme(read_apps_use_light_theme()?))
    }

    fn preference(&self) -> Result<Option<Theme>, Unavailable> {
        // Windows always has a concrete value once the key exists.
        self.current().map(Some)
    }

    fn subscribe(&self, handler: ThemeHandler) -> Result<(), Unavailable> {
        let handler = deduplicate(self.current().ok(), handler);
        *self.handler.lock().expect("handler slot is never held across a panic") = Some(handler);
        if self.listener_started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let slot = Arc::clone(&self.handler);
        std::thread::Builder::new()
            .name("vitrum-theme-watch".to_string())
            .spawn(move || {
                let path = wide(KEY_PATH);
                let mut hkey = HKEY::default();
                // SAFETY: `path` is NUL-terminated and outlives the call.
                let status = unsafe {
                    RegOpenKeyExW(
                        HKEY_CURRENT_USER,
                        PCWSTR(path.as_ptr()),
                        None,
                        KEY_READ | KEY_NOTIFY,
                        &mut hkey,
                    )
                };
                if status != ERROR_SUCCESS {
                    return;
                }
                loop {
                    // SAFETY: synchronous wait on a key we hold open with
                    // KEY_NOTIFY. Returns only when the key changes.
                    let status = unsafe {
                        RegNotifyChangeKeyValue(
                            hkey,
                            false,
                            REG_NOTIFY_CHANGE_LAST_SET,
                            None,
                            false,
                        )
                    };
                    if status != ERROR_SUCCESS {
                        break;
                    }
                    let Ok(raw) = read_apps_use_light_theme() else { break };
                    let handler = slot
                        .lock()
                        .expect("handler slot is never held across a panic")
                        .clone();
                    match handler {
                        Some(handler) => handler(theme_from_apps_use_light_theme(raw)),
                        // Nobody is listening any more.
                        None => break,
                    }
                }
                // SAFETY: `hkey` came from a successful open and the loop has
                // finished with it.
                unsafe {
                    let _ = RegCloseKey(hkey);
                }
            })
            .map_err(|e| {
                Unavailable::runtime_error(format!("cannot spawn the theme listener: {e}"))
            })?;
        Ok(())
    }
}
