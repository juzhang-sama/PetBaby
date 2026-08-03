use std::{
    ffi::c_void,
    ptr::null_mut,
    sync::atomic::{AtomicIsize, Ordering},
    sync::{Mutex, OnceLock},
};

use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError, HWND},
    Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_OR},
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        WindowsAndMessaging::{
            EnumWindows, GetClassNameW, GetWindowLongPtrW, GetWindowRect, IsWindowVisible,
            SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE,
            HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
            SW_SHOWNOACTIVATE, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        },
    },
};

const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;

use crate::{
    platform::{
        FullscreenSnapshot, PlatformAdapter, PlatformError, ScreenRect, WindowModeEvidence,
    },
    windowing::{RegionSpan, WindowMode},
};

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

    fn set_window_mode(
        &self,
        hwnd: isize,
        mode: WindowMode,
    ) -> Result<WindowModeEvidence, PlatformError> {
        match mode {
            WindowMode::Companion => {
                uninstall_desktop_hook();
                unsafe {
                    let style = GetWindowLongPtrW(hwnd as HWND, GWL_STYLE);
                    let exstyle = GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE);
                    if style != 0 {
                        SetWindowLongPtrW(hwnd as HWND, GWL_STYLE, style & !(WS_CHILD as isize));
                    }
                    let _ = SetParent(hwnd as *mut c_void, null_mut());
                    if SetWindowPos(
                        hwnd as *mut c_void,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE
                            | SWP_NOSIZE
                            | SWP_NOACTIVATE
                            | SWP_FRAMECHANGED
                            | SWP_SHOWWINDOW,
                    ) == 0
                    {
                        return Err(last_error("SetWindowPos"));
                    }
                    if exstyle != 0 {
                        SetWindowLongPtrW(
                            hwnd as HWND,
                            GWL_EXSTYLE,
                            exstyle | WS_EX_TOPMOST as isize,
                        );
                    }
                }
                Ok(WindowModeEvidence {
                    requested: mode,
                    applied: true,
                    strategy: "topmost-no-activate",
                    parent_hwnd: None,
                })
            }
            WindowMode::Desktop => {
                let progman = find_progman()?;
                install_desktop_hook(hwnd)?;
                unsafe {
                    let mut rect = std::mem::zeroed();
                    if GetWindowRect(hwnd as HWND, &mut rect) == 0 {
                        return Err(last_error("GetWindowRect"));
                    }
                    let style = GetWindowLongPtrW(hwnd as HWND, GWL_STYLE);
                    let exstyle = GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE);
                    SetWindowLongPtrW(hwnd as HWND, GWL_STYLE, style | WS_CHILD as isize);
                    if exstyle != 0 {
                        SetWindowLongPtrW(
                            hwnd as HWND,
                            GWL_EXSTYLE,
                            exstyle & !(WS_EX_TOPMOST as isize),
                        );
                    }
                    SetLastError(0);
                    let previous = SetParent(hwnd as *mut c_void, progman as *mut c_void);
                    if previous.is_null() && GetLastError() != 0 {
                        return Err(last_error("SetParent"));
                    }
                    if SetWindowPos(
                        hwnd as *mut c_void,
                        null_mut(),
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
                    ) == 0
                    {
                        return Err(last_error("SetWindowPos"));
                    }
                }
                Ok(WindowModeEvidence {
                    requested: mode,
                    applied: true,
                    strategy: "progman-child",
                    parent_hwnd: Some(progman),
                })
            }
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

const EVENT_SYSTEM_DESKTOPSWITCH: u32 = 0x001E;
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;

static PROGMAN_FOUND: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn find_progman_cb(hwnd: HWND, _lparam: isize) -> i32 {
    let mut name = [0u16; 64];
    let len = GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32);
    if len > 0 && String::from_utf16_lossy(&name[..len as usize]) == "Progman" {
        PROGMAN_FOUND.store(hwnd as isize, Ordering::SeqCst);
        return 0;
    }
    1
}

fn find_progman() -> Result<isize, PlatformError> {
    PROGMAN_FOUND.store(0, Ordering::SeqCst);
    unsafe {
        let _ = EnumWindows(Some(find_progman_cb), 0);
    }
    let found = PROGMAN_FOUND.load(Ordering::SeqCst);
    if found == 0 {
        return Err(PlatformError::WindowsApi {
            operation: "EnumWindows(Progman)",
            code: 0,
        });
    }
    Ok(found)
}

struct DesktopHookState {
    hook: usize,
    pet_hwnd: isize,
}

static DESKTOP_HOOK: OnceLock<Mutex<DesktopHookState>> = OnceLock::new();

unsafe extern "system" fn desktop_event_cb(
    hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _id_thread: u32,
    _time: u32,
) {
    let Some(state) = DESKTOP_HOOK.get() else {
        return;
    };
    let Ok(state) = state.lock() else { return };
    if state.hook == 0 || state.pet_hwnd == 0 || hook as usize != state.hook {
        return;
    }

    let mut class_name = String::new();
    if event == EVENT_SYSTEM_FOREGROUND {
        let mut name = [0u16; 64];
        let len = GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32);
        if len <= 0 {
            return;
        }
        class_name = String::from_utf16_lossy(&name[..len as usize]);
    }
    let is_desktop = if event == EVENT_SYSTEM_DESKTOPSWITCH {
        true
    } else if event == EVENT_SYSTEM_FOREGROUND {
        class_name == "Progman" || class_name == "WorkerW" || class_name == "Shell_TrayWnd"
    } else {
        false
    };
    if !is_desktop {
        return;
    }

    let pet = state.pet_hwnd as HWND;
    let was_visible = IsWindowVisible(pet);
    ShowWindow(pet, SW_SHOWNOACTIVATE);
    let now_visible = IsWindowVisible(pet);
    println!(
        "[desktop-pet] win-event event=0x{event:X} class={class_name} pet_visible_before={was_visible} after={now_visible}"
    );
}

fn install_desktop_hook(hwnd: isize) -> Result<(), PlatformError> {
    let state = DESKTOP_HOOK.get_or_init(|| {
        Mutex::new(DesktopHookState {
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
            Some(desktop_event_cb),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if hook.is_null() {
            println!("[desktop-pet] SetWinEventHook FAILED");
            return Err(last_error("SetWinEventHook"));
        }
        state.hook = hook as usize;
        state.pet_hwnd = hwnd;
        println!("[desktop-pet] desktop hook installed 0x{hook:?}");
    }
    Ok(())
}

fn uninstall_desktop_hook() {
    if let Some(state) = DESKTOP_HOOK.get() {
        if let Ok(mut state) = state.lock() {
            if state.hook != 0 {
                unsafe { UnhookWinEvent(state.hook as HWINEVENTHOOK) };
                state.hook = 0;
                state.pet_hwnd = 0;
            }
        }
    }
}

fn last_error(operation: &'static str) -> PlatformError {
    PlatformError::WindowsApi {
        operation,
        code: unsafe { GetLastError() },
    }
}
