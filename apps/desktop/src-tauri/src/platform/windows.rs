use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError, HWND},
    Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_OR},
    UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    },
};

pub(crate) fn encode_windows_path(path: &std::path::Path) -> Result<Vec<u16>, String> {
    use std::os::windows::ffi::OsStrExt;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    let raw: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    if raw.contains(&0) {
        return Err("Windows path contains an embedded NUL".into());
    }
    let slash = b'\\' as u16;
    let question = b'?' as u16;
    let mut encoded = if raw.starts_with(&[slash, slash, question, slash]) {
        raw
    } else if raw.starts_with(&[slash, slash]) {
        let mut extended = "\\\\?\\UNC\\".encode_utf16().collect::<Vec<_>>();
        extended.extend_from_slice(&raw[2..]);
        extended
    } else {
        let mut extended = "\\\\?\\".encode_utf16().collect::<Vec<_>>();
        extended.extend_from_slice(&raw);
        extended
    };
    encoded.push(0);
    Ok(encoded)
}

fn durable_move(
    source: &std::path::Path,
    target: &std::path::Path,
    replace: bool,
) -> Result<(), String> {
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source = encode_windows_path(source)?;
    let target = encode_windows_path(target)?;
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    let moved = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), flags) };
    if moved == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

pub(crate) fn durable_replace_file(
    source: &std::path::Path,
    target: &std::path::Path,
) -> Result<(), String> {
    durable_move(source, target, true)
}

pub(crate) fn durable_move_file(
    source: &std::path::Path,
    target: &std::path::Path,
) -> Result<(), String> {
    durable_move(source, target, false)
}

pub(crate) fn durable_move_directory(
    source: &std::path::Path,
    target: &std::path::Path,
) -> Result<(), String> {
    durable_move(source, target, false)
}

use crate::{
    platform::{FullscreenSnapshot, PlatformAdapter, PlatformError, ScreenRect},
    windowing::RegionSpan,
};

pub struct WindowsPlatformAdapter;

pub(crate) fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(test)]
pub(crate) fn create_directory_junction(target: &std::path::Path, link: &std::path::Path) {
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "junction creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

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
                GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
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

            // The desktop shell windows (Progman/WorkerW/Shell_TrayWnd) cover the
            // monitor but are not fullscreen applications; Win+D or clicking the
            // desktop must not hide the pet.
            let mut class_name = [0u16; 64];
            let class_len =
                GetClassNameW(foreground, class_name.as_mut_ptr(), class_name.len() as i32);
            if class_len > 0 {
                let class_name = String::from_utf16_lossy(&class_name[..class_len as usize]);
                if class_name == "Progman"
                    || class_name == "WorkerW"
                    || class_name == "Shell_TrayWnd"
                {
                    return Ok(FullscreenSnapshot {
                        is_fullscreen: false,
                        foreground_hwnd: Some(foreground as isize),
                        monitor_rect: None,
                        reason: "desktop-foreground",
                    });
                }
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
