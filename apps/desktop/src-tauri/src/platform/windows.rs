use std::{
    ptr::null_mut,
    sync::{Mutex, OnceLock},
};

use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError, HWND},
    Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_OR},
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        WindowsAndMessaging::{
            GetClassNameW, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
            GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
            SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        },
    },
};

use crate::{
    platform::{FullscreenSnapshot, PlatformAdapter, PlatformError, ScreenRect},
    windowing::RegionSpan,
};

const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const EVENT_SYSTEM_DESKTOPSWITCH: u32 = 0x001E;
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;

struct ShowHookState {
    hook: usize,
    pet_hwnd: isize,
}

static SHOW_HOOK: OnceLock<Mutex<ShowHookState>> = OnceLock::new();

unsafe extern "system" fn show_event_cb(
    hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _id_thread: u32,
    _time: u32,
) {
    let Some(state) = SHOW_HOOK.get() else {
        return;
    };
    let Ok(state) = state.lock() else { return };
    if state.hook == 0 || state.pet_hwnd == 0 || hook as usize != state.hook {
        return;
    }

    let is_desktop = if event == EVENT_SYSTEM_DESKTOPSWITCH {
        true
    } else if event == EVENT_SYSTEM_FOREGROUND {
        let mut name = [0u16; 64];
        let len = GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32);
        if len <= 0 {
            return;
        }
        let class_name = String::from_utf16_lossy(&name[..len as usize]);
        class_name == "Progman" || class_name == "WorkerW" || class_name == "Shell_TrayWnd"
    } else {
        false
    };
    if !is_desktop {
        return;
    }

    let pet = state.pet_hwnd as HWND;
    // Win+D / show-desktop hides all non-desktop windows at the DWM layer.
    // Re-asserting TOPMOST + show forces DWM to re-composite the window.
    ShowWindow(pet, SW_SHOWNOACTIVATE);
    SetWindowPos(
        pet,
        HWND_TOPMOST,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
}

pub fn install_show_hook(hwnd: isize) -> Result<(), PlatformError> {
    let state = SHOW_HOOK.get_or_init(|| {
        Mutex::new(ShowHookState {
            hook: 0,
            pet_hwnd: 0,
        })
    });
    let mut state = state.lock().map_err(|_| PlatformError::WindowsApi {
        operation: "lock",
        code: 0,
    })?;
    if state.hook != 0 {
        state.pet_hwnd = hwnd;
        return Ok(());
    }
    unsafe {
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_DESKTOPSWITCH,
            null_mut(),
            Some(show_event_cb),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if hook.is_null() {
            return Err(last_error("SetWinEventHook"));
        }
        state.hook = hook as usize;
        state.pet_hwnd = hwnd;
    }
    Ok(())
}

pub struct WindowsPlatformAdapter;

impl PlatformAdapter for WindowsPlatformAdapter {
    fn configure_pet_window(&self, hwnd: isize) -> Result<(), PlatformError> {
        unsafe {
            let style = GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE);
            SetLastError(0);
            let previous = SetWindowLongPtrW(
                hwnd as HWND,
                GWL_EXSTYLE,
                style | WS_EX_NOACTIVATE as isize | WS_EX_TOOLWINDOW as isize,
            );
            if previous == 0 && GetLastError() != 0 {
                return Err(last_error("SetWindowLongPtrW"));
            }
            Ok(())
        }
    }

    fn apply_hit_region(&self, hwnd: isize, spans: &[RegionSpan]) -> Result<(), PlatformError> {
        unsafe {
            let aggregate = CreateRectRgn(0, 0, 0, 0);
            if aggregate.is_null() {
                return Err(last_error("CreateRectRgn"));
            }

            for span in spans {
                let row = CreateRectRgn(span.left, span.top, span.right, span.bottom);
                if row.is_null() {
                    let _ = DeleteObject(aggregate);
                    return Err(last_error("CreateRectRgn"));
                }
                let combined = CombineRgn(aggregate, aggregate, row, RGN_OR);
                let _ = DeleteObject(row);
                if combined == 0 {
                    let _ = DeleteObject(aggregate);
                    return Err(last_error("CombineRgn"));
                }
            }

            if SetWindowRgn(hwnd as HWND, aggregate, 1) == 0 {
                let _ = DeleteObject(aggregate);
                return Err(last_error("SetWindowRgn"));
            }
            Ok(())
        }
    }

    fn probe_fullscreen(&self, own_pid: u32) -> Result<FullscreenSnapshot, PlatformError> {
        use windows_sys::Win32::{
            Graphics::Gdi::{
                GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
            },
            UI::WindowsAndMessaging::{
                GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
            },
        };

        unsafe {
            let foreground = GetForegroundWindow();
            if foreground.is_null() {
                return Ok(FullscreenSnapshot {
                    is_fullscreen: false,
                    foreground_hwnd: None,
                    monitor_rect: None,
                    reason: "no-foreground",
                });
            }

            let mut foreground_pid = 0u32;
            GetWindowThreadProcessId(foreground, &mut foreground_pid);
            if foreground_pid == own_pid {
                return Ok(FullscreenSnapshot {
                    is_fullscreen: false,
                    foreground_hwnd: Some(foreground as isize),
                    monitor_rect: None,
                    reason: "own-window",
                });
            }

            let mut window_rect = std::mem::zeroed();
            if GetWindowRect(foreground, &mut window_rect) == 0 {
                return Err(last_error("GetWindowRect"));
            }
            let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST);
            let mut monitor_info: MONITORINFO = std::mem::zeroed();
            monitor_info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut monitor_info) == 0 {
                return Err(last_error("GetMonitorInfoW"));
            }

            let monitor_rect = ScreenRect {
                left: monitor_info.rcMonitor.left,
                top: monitor_info.rcMonitor.top,
                right: monitor_info.rcMonitor.right,
                bottom: monitor_info.rcMonitor.bottom,
            };
            let tolerance = 2;
            let is_fullscreen = (window_rect.left - monitor_rect.left).abs() <= tolerance
                && (window_rect.top - monitor_rect.top).abs() <= tolerance
                && (window_rect.right - monitor_rect.right).abs() <= tolerance
                && (window_rect.bottom - monitor_rect.bottom).abs() <= tolerance;

            Ok(FullscreenSnapshot {
                is_fullscreen,
                foreground_hwnd: Some(foreground as isize),
                monitor_rect: Some(monitor_rect),
                reason: if is_fullscreen {
                    "foreground-covers-monitor"
                } else {
                    "not-fullscreen"
                },
            })
        }
    }
}

fn last_error(operation: &'static str) -> PlatformError {
    PlatformError::WindowsApi {
        operation,
        code: unsafe { GetLastError() },
    }
}
