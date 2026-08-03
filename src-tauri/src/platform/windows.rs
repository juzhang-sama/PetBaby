use std::ffi::c_void;

use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError, HWND},
    Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_OR},
    UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_BOTTOM, HWND_TOPMOST,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST,
    },
};

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
                unsafe {
                    let exstyle = GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE);
                    if SetWindowPos(
                        hwnd as *mut c_void,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
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
                unsafe {
                    let exstyle = GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE);
                    if exstyle != 0 {
                        SetWindowLongPtrW(
                            hwnd as HWND,
                            GWL_EXSTYLE,
                            exstyle & !(WS_EX_TOPMOST as isize),
                        );
                    }
                    if SetWindowPos(
                        hwnd as *mut c_void,
                        HWND_BOTTOM,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    ) == 0
                    {
                        return Err(last_error("SetWindowPos"));
                    }
                }
                Ok(WindowModeEvidence {
                    requested: mode,
                    applied: true,
                    strategy: "bottom-no-topmost",
                    parent_hwnd: None,
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

fn last_error(operation: &'static str) -> PlatformError {
    PlatformError::WindowsApi {
        operation,
        code: unsafe { GetLastError() },
    }
}
