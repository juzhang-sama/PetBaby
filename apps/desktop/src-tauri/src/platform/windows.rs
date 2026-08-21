use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError, HWND},
    Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_OR},
    UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_CHILD, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
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
    platform::{
        DesktopAttachOutcome, FullscreenSnapshot, PlatformAdapter, PlatformError, ScreenRect,
        WindowHostSnapshot, WindowVisibilityFacts,
    },
    windowing::RegionSpan,
};

use windows_sys::Win32::{
    Foundation::{POINT, RECT},
    Graphics::Gdi::ScreenToClient,
    UI::HiDpi::{AreDpiAwarenessContextsEqual, GetWindowDpiAwarenessContext},
    UI::WindowsAndMessaging::{
        EnumWindows, FindWindowExW, FindWindowW, GetParent, GetWindow, GetWindowRect, IsWindow,
        SendMessageTimeoutW, SetParent, SetWindowPos, GWL_STYLE, GW_HWNDPREV, HWND_BOTTOM,
        HWND_TOP, HWND_TOPMOST, SMTO_ABORTIFHUNG, SMTO_ERRORONEXIT, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    },
};

pub struct WindowsPlatformAdapter;

pub(crate) fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

pub(crate) fn with_regular_file_no_reparse<T>(
    root: &std::path::Path,
    path: &std::path::Path,
    callback: impl FnOnce(&[u8]) -> Result<T, String>,
) -> Result<T, String> {
    read_regular_file_no_reparse_inner(root, path, || {}, callback)
}

#[cfg(test)]
pub(crate) fn read_regular_file_no_reparse_with_hook(
    root: &std::path::Path,
    path: &std::path::Path,
    hook: impl FnOnce(),
) -> Result<Vec<u8>, String> {
    read_regular_file_no_reparse_inner(root, path, hook, |bytes| Ok(bytes.to_vec()))
}

fn read_regular_file_no_reparse_inner<T>(
    root: &std::path::Path,
    path: &std::path::Path,
    hook: impl FnOnce(),
    callback: impl FnOnce(&[u8]) -> Result<T, String>,
) -> Result<T, String> {
    use std::io::Read;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandle, GetFinalPathNameByHandleW,
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_SEQUENTIAL_SCAN,
        FILE_NAME_NORMALIZED, FILE_SHARE_READ, OPEN_EXISTING, VOLUME_NAME_DOS,
    };

    fn directory_handle(path: &std::path::Path) -> Result<OwnedHandle, String> {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ, OPEN_EXISTING,
        };
        let wide = encode_windows_path(path)?;
        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(format!(
                "open preview directory: {}",
                std::io::Error::last_os_error()
            ));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) } == 0 {
            return Err(format!(
                "inspect preview directory: {}",
                std::io::Error::last_os_error()
            ));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err("preview file path contains a link or reparse point".into());
        }
        Ok(handle)
    }

    fn final_path(handle: RawHandle) -> Result<String, String> {
        let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS;
        let required = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, flags) };
        if required == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut buffer = vec![0_u16; required as usize + 1];
        let written = unsafe {
            GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, flags)
        };
        if written == 0 || written as usize >= buffer.len() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(String::from_utf16_lossy(&buffer[..written as usize]).to_ascii_lowercase())
    }

    let relative = path
        .strip_prefix(root)
        .map_err(|_| "preview file escapes package root")?;
    let components = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(name) => Ok(name.to_os_string()),
            _ => Err("preview path contains an unsafe component".to_string()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if components.is_empty() {
        return Err("preview path is not a regular file".into());
    }

    let root_handle = directory_handle(root)?;
    let trusted_root = final_path(root_handle.as_raw_handle())?;
    let mut directory_handles = vec![root_handle];
    let mut current = root.to_path_buf();
    for component in &components[..components.len() - 1] {
        current.push(component);
        directory_handles.push(directory_handle(&current)?);
    }

    let wide = encode_windows_path(path)?;
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(format!(
            "open preview file: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut file = unsafe { std::fs::File::from_raw_handle(raw) };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(format!(
            "inspect preview file: {}",
            std::io::Error::last_os_error()
        ));
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err("preview path is not a regular file or contains a reparse point".into());
    }
    let resolved_file = final_path(file.as_raw_handle())?;
    let root_prefix = format!("{}\\", trusted_root.trim_end_matches('\\'));
    if !resolved_file.starts_with(&root_prefix) {
        return Err("preview file escapes package root".into());
    }

    hook();
    let mut bytes = Vec::with_capacity(info.nFileSizeLow as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("read preview file: {error}"))?;
    let result = callback(&bytes)?;
    drop(directory_handles);
    Ok(result)
}

fn should_apply_pet_stealth_styles(debug_assertions: bool, capture_env: Option<&str>) -> bool {
    !(debug_assertions && capture_env == Some("1"))
}

const SPAWN_WORKERW_MESSAGE: u32 = 0x052C;
const WORKERW_TIMEOUT_MS: u32 = 1_000;
const RECT_TOLERANCE_PX: i32 = 2;

trait WindowApi {
    fn note_phase(&self, _phase: &'static str) {}
    fn is_window(&self, hwnd: isize) -> Result<bool, PlatformError>;
    fn parent(&self, hwnd: isize) -> Result<isize, PlatformError>;
    fn window_long(&self, hwnd: isize, index: i32) -> Result<isize, PlatformError>;
    fn window_rect(&self, hwnd: isize) -> Result<ScreenRect, PlatformError>;
    fn previous_window(&self, hwnd: isize) -> Result<isize, PlatformError>;
    fn find_window(&self, class_name: &str) -> Result<Option<isize>, PlatformError>;
    fn spawn_workerw(&self, progman: isize) -> Result<(), PlatformError>;
    fn top_level_windows(&self) -> Result<Vec<isize>, PlatformError>;
    fn find_child(
        &self,
        parent: isize,
        after: isize,
        class_name: &str,
    ) -> Result<Option<isize>, PlatformError>;
    fn dpi_contexts_compatible(&self, child: isize, parent: isize) -> Result<bool, PlatformError>;
    fn set_parent(&self, hwnd: isize, parent: isize) -> Result<(), PlatformError>;
    fn set_window_long(&self, hwnd: isize, index: i32, value: isize) -> Result<(), PlatformError>;
    fn screen_to_client(
        &self,
        parent: isize,
        point: (i32, i32),
    ) -> Result<(i32, i32), PlatformError>;
    fn set_window_pos(
        &self,
        hwnd: isize,
        insert_after: isize,
        rect: Option<ScreenRect>,
        flags: u32,
    ) -> Result<(), PlatformError>;
}

struct NativeWindowApi;

impl WindowApi for NativeWindowApi {
    fn is_window(&self, hwnd: isize) -> Result<bool, PlatformError> {
        Ok(unsafe { IsWindow(hwnd as HWND) } != 0)
    }

    fn parent(&self, hwnd: isize) -> Result<isize, PlatformError> {
        unsafe {
            SetLastError(0);
            let parent = GetParent(hwnd as HWND);
            let code = GetLastError();
            if parent.is_null() && code != 0 {
                Err(PlatformError::WindowsApi {
                    operation: "GetParent",
                    code,
                })
            } else {
                Ok(parent as isize)
            }
        }
    }

    fn window_long(&self, hwnd: isize, index: i32) -> Result<isize, PlatformError> {
        unsafe {
            SetLastError(0);
            let value = GetWindowLongPtrW(hwnd as HWND, index);
            let code = GetLastError();
            if value == 0 && code != 0 {
                Err(PlatformError::WindowsApi {
                    operation: "GetWindowLongPtrW",
                    code,
                })
            } else {
                Ok(value)
            }
        }
    }

    fn window_rect(&self, hwnd: isize) -> Result<ScreenRect, PlatformError> {
        unsafe {
            let mut rect: RECT = std::mem::zeroed();
            SetLastError(0);
            if GetWindowRect(hwnd as HWND, &mut rect) == 0 {
                return Err(last_error("GetWindowRect"));
            }
            Ok(ScreenRect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            })
        }
    }

    fn previous_window(&self, hwnd: isize) -> Result<isize, PlatformError> {
        unsafe {
            SetLastError(0);
            let previous = GetWindow(hwnd as HWND, GW_HWNDPREV);
            let code = GetLastError();
            if previous.is_null() && code != 0 {
                Err(PlatformError::WindowsApi {
                    operation: "GetWindow(GW_HWNDPREV)",
                    code,
                })
            } else {
                Ok(previous as isize)
            }
        }
    }

    fn find_window(&self, class_name: &str) -> Result<Option<isize>, PlatformError> {
        let class_name = wide(class_name);
        unsafe {
            SetLastError(0);
            let hwnd = FindWindowW(class_name.as_ptr(), std::ptr::null());
            let code = GetLastError();
            if hwnd.is_null() {
                if code == 0 {
                    Ok(None)
                } else {
                    Err(PlatformError::WindowsApi {
                        operation: "FindWindowW",
                        code,
                    })
                }
            } else {
                Ok(Some(hwnd as isize))
            }
        }
    }

    fn spawn_workerw(&self, progman: isize) -> Result<(), PlatformError> {
        unsafe {
            let mut response = 0usize;
            SetLastError(0);
            let sent = SendMessageTimeoutW(
                progman as HWND,
                SPAWN_WORKERW_MESSAGE,
                0,
                0,
                SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
                WORKERW_TIMEOUT_MS,
                &mut response,
            );
            if sent == 0 {
                return Err(last_error("SendMessageTimeoutW(0x052C)"));
            }
            Ok(())
        }
    }

    fn top_level_windows(&self) -> Result<Vec<isize>, PlatformError> {
        unsafe extern "system" fn collect(hwnd: HWND, lparam: isize) -> i32 {
            let windows = &mut *(lparam as *mut Vec<isize>);
            windows.push(hwnd as isize);
            1
        }

        let mut windows = Vec::new();
        unsafe {
            SetLastError(0);
            if EnumWindows(Some(collect), (&mut windows as *mut Vec<isize>) as isize) == 0 {
                return Err(last_error("EnumWindows"));
            }
        }
        Ok(windows)
    }

    fn find_child(
        &self,
        parent: isize,
        after: isize,
        class_name: &str,
    ) -> Result<Option<isize>, PlatformError> {
        let class_name = wide(class_name);
        unsafe {
            SetLastError(0);
            let hwnd = FindWindowExW(
                parent as HWND,
                after as HWND,
                class_name.as_ptr(),
                std::ptr::null(),
            );
            let code = GetLastError();
            if hwnd.is_null() {
                if code == 0 {
                    Ok(None)
                } else {
                    Err(PlatformError::WindowsApi {
                        operation: "FindWindowExW",
                        code,
                    })
                }
            } else {
                Ok(Some(hwnd as isize))
            }
        }
    }

    fn dpi_contexts_compatible(&self, child: isize, parent: isize) -> Result<bool, PlatformError> {
        unsafe {
            SetLastError(0);
            let child_context = GetWindowDpiAwarenessContext(child as HWND);
            if child_context.is_null() {
                return Err(last_error("GetWindowDpiAwarenessContext(child)"));
            }
            SetLastError(0);
            let parent_context = GetWindowDpiAwarenessContext(parent as HWND);
            if parent_context.is_null() {
                return Err(last_error("GetWindowDpiAwarenessContext(parent)"));
            }
            Ok(AreDpiAwarenessContextsEqual(child_context, parent_context) != 0)
        }
    }

    fn set_parent(&self, hwnd: isize, parent: isize) -> Result<(), PlatformError> {
        unsafe {
            SetLastError(0);
            let previous = SetParent(hwnd as HWND, parent as HWND);
            let code = GetLastError();
            if previous.is_null() && code != 0 {
                Err(PlatformError::WindowsApi {
                    operation: "SetParent",
                    code,
                })
            } else {
                Ok(())
            }
        }
    }

    fn set_window_long(&self, hwnd: isize, index: i32, value: isize) -> Result<(), PlatformError> {
        unsafe {
            SetLastError(0);
            let previous = SetWindowLongPtrW(hwnd as HWND, index, value);
            let code = GetLastError();
            if previous == 0 && code != 0 {
                Err(PlatformError::WindowsApi {
                    operation: "SetWindowLongPtrW",
                    code,
                })
            } else {
                Ok(())
            }
        }
    }

    fn screen_to_client(
        &self,
        parent: isize,
        point: (i32, i32),
    ) -> Result<(i32, i32), PlatformError> {
        if parent == 0 {
            return Ok(point);
        }
        unsafe {
            let mut point = POINT {
                x: point.0,
                y: point.1,
            };
            SetLastError(0);
            if ScreenToClient(parent as HWND, &mut point) == 0 {
                return Err(last_error("ScreenToClient"));
            }
            Ok((point.x, point.y))
        }
    }

    fn set_window_pos(
        &self,
        hwnd: isize,
        insert_after: isize,
        rect: Option<ScreenRect>,
        flags: u32,
    ) -> Result<(), PlatformError> {
        let (x, y, width, height) = rect
            .map(|rect| {
                (
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                )
            })
            .unwrap_or((0, 0, 0, 0));
        unsafe {
            SetLastError(0);
            if SetWindowPos(
                hwnd as HWND,
                insert_after as HWND,
                x,
                y,
                width,
                height,
                flags,
            ) == 0
            {
                return Err(last_error("SetWindowPos"));
            }
        }
        Ok(())
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn host_error(operation: &'static str, detail: impl Into<String>) -> PlatformError {
    PlatformError::WindowHost {
        operation,
        detail: detail.into(),
    }
}

fn capture_window_host_with_api(
    api: &impl WindowApi,
    hwnd: isize,
) -> Result<WindowHostSnapshot, PlatformError> {
    api.note_phase("capture");
    if hwnd == 0 || !api.is_window(hwnd)? {
        return Err(host_error("capture", "pet HWND is not valid"));
    }
    let parent = api.parent(hwnd)?;
    let style = api.window_long(hwnd, GWL_STYLE)?;
    let ex_style = api.window_long(hwnd, GWL_EXSTYLE)?;
    Ok(WindowHostSnapshot {
        parent,
        style,
        ex_style,
        rect: api.window_rect(hwnd)?,
        topmost: ex_style & WS_EX_TOPMOST as isize != 0,
        z_order_after: api.previous_window(hwnd)?,
    })
}

fn find_workerw_host(api: &impl WindowApi) -> Result<isize, PlatformError> {
    let progman = api
        .find_window("Progman")?
        .ok_or_else(|| host_error("WorkerW discovery", "Progman was not found"))?;
    if !api.is_window(progman)? {
        return Err(host_error(
            "WorkerW discovery",
            "Progman is not a valid HWND",
        ));
    }
    api.spawn_workerw(progman)?;
    let mut found_def_view = false;
    for top in api.top_level_windows()? {
        if !api.is_window(top)? {
            continue;
        }
        if api.find_child(top, 0, "SHELLDLL_DefView")?.is_some() {
            found_def_view = true;
            if let Some(worker) = api.find_child(0, top, "WorkerW")? {
                if worker != 0 && api.is_window(worker)? {
                    return Ok(worker);
                }
            }
        }
    }
    Err(host_error(
        "WorkerW discovery",
        if found_def_view {
            "SHELLDLL_DefView was found but no valid adjacent WorkerW host was available"
        } else {
            "SHELLDLL_DefView host was not found"
        },
    ))
}

fn fullscreen_covers_pet_monitor(
    foreground_rect: ScreenRect,
    foreground_monitor: ScreenRect,
    pet_monitor: ScreenRect,
) -> bool {
    let tolerance = RECT_TOLERANCE_PX;
    let covers_foreground_monitor = (foreground_rect.left - foreground_monitor.left).abs()
        <= tolerance
        && (foreground_rect.top - foreground_monitor.top).abs() <= tolerance
        && (foreground_rect.right - foreground_monitor.right).abs() <= tolerance
        && (foreground_rect.bottom - foreground_monitor.bottom).abs() <= tolerance;
    covers_foreground_monitor && foreground_monitor == pet_monitor
}

fn client_rect(
    api: &impl WindowApi,
    parent: isize,
    screen_rect: ScreenRect,
) -> Result<ScreenRect, PlatformError> {
    let top_left = api.screen_to_client(parent, (screen_rect.left, screen_rect.top))?;
    let bottom_right = api.screen_to_client(parent, (screen_rect.right, screen_rect.bottom))?;
    Ok(ScreenRect {
        left: top_left.0,
        top: top_left.1,
        right: bottom_right.0,
        bottom: bottom_right.1,
    })
}

fn rect_matches(actual: ScreenRect, expected: ScreenRect) -> bool {
    (actual.left - expected.left).abs() <= RECT_TOLERANCE_PX
        && (actual.top - expected.top).abs() <= RECT_TOLERANCE_PX
        && (actual.right - expected.right).abs() <= RECT_TOLERANCE_PX
        && (actual.bottom - expected.bottom).abs() <= RECT_TOLERANCE_PX
}

fn verify_hosted_window(
    api: &impl WindowApi,
    hwnd: isize,
    parent: isize,
    expected_style: isize,
    expected_ex_style: isize,
    expected_rect: ScreenRect,
) -> Result<(), PlatformError> {
    if api.parent(hwnd)? != parent {
        return Err(host_error("verify", "window parent did not match"));
    }
    if api.window_long(hwnd, GWL_STYLE)? != expected_style {
        return Err(host_error("verify", "window style did not match"));
    }
    let ex_style = api.window_long(hwnd, GWL_EXSTYLE)?;
    if ex_style != expected_ex_style || ex_style & WS_EX_TOPMOST as isize != 0 {
        return Err(host_error("verify", "window extended style did not match"));
    }
    if !rect_matches(api.window_rect(hwnd)?, expected_rect) {
        return Err(host_error(
            "verify",
            "window screen rectangle drifted by more than 2px",
        ));
    }
    Ok(())
}

fn try_workerw(
    api: &impl WindowApi,
    hwnd: isize,
    snapshot: &WindowHostSnapshot,
) -> Result<DesktopAttachOutcome, PlatformError> {
    api.note_phase("workerw");
    let parent = find_workerw_host(api)?;
    if !api.dpi_contexts_compatible(hwnd, parent)? {
        return Err(host_error(
            "WorkerW DPI preflight",
            "child and prospective parent use incompatible DPI awareness contexts",
        ));
    }
    let style = (snapshot.style | WS_CHILD as isize) & !(WS_POPUP as isize);
    let ex_style = snapshot.ex_style & !(WS_EX_TOPMOST as isize);
    api.set_window_long(hwnd, GWL_STYLE, style)?;
    api.set_parent(hwnd, parent)?;
    api.set_window_long(hwnd, GWL_EXSTYLE, ex_style)?;
    let rect = client_rect(api, parent, snapshot.rect)?;
    api.set_window_pos(
        hwnd,
        HWND_BOTTOM as isize,
        Some(rect),
        SWP_NOACTIVATE | SWP_FRAMECHANGED,
    )?;
    verify_hosted_window(api, hwnd, parent, style, ex_style, snapshot.rect)?;
    Ok(DesktopAttachOutcome::WorkerW { parent })
}

fn try_bottom_fallback(
    api: &impl WindowApi,
    hwnd: isize,
    snapshot: &WindowHostSnapshot,
) -> Result<DesktopAttachOutcome, PlatformError> {
    api.note_phase("bottom");
    let ex_style = snapshot.ex_style & !(WS_EX_TOPMOST as isize);
    api.set_parent(hwnd, snapshot.parent)?;
    api.set_window_long(hwnd, GWL_STYLE, snapshot.style)?;
    api.set_window_long(hwnd, GWL_EXSTYLE, ex_style)?;
    api.set_window_pos(
        hwnd,
        HWND_BOTTOM as isize,
        None,
        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED,
    )?;
    verify_hosted_window(
        api,
        hwnd,
        snapshot.parent,
        snapshot.style,
        ex_style,
        snapshot.rect,
    )?;
    Ok(DesktopAttachOutcome::BottomFallback)
}

fn restore_window_host_with_api(
    api: &impl WindowApi,
    hwnd: isize,
    snapshot: &WindowHostSnapshot,
) -> Result<(), PlatformError> {
    api.note_phase("restore");
    let mut errors = Vec::new();
    if let Err(error) = api.set_parent(hwnd, snapshot.parent) {
        errors.push(format!("parent: {error}"));
    }
    if let Err(error) = api.set_window_long(hwnd, GWL_STYLE, snapshot.style) {
        errors.push(format!("style: {error}"));
    }
    if let Err(error) = api.set_window_long(hwnd, GWL_EXSTYLE, snapshot.ex_style) {
        errors.push(format!("ex-style: {error}"));
    }
    match client_rect(api, snapshot.parent, snapshot.rect) {
        Ok(rect) => {
            if let Err(error) = api.set_window_pos(
                hwnd,
                HWND_TOP as isize,
                Some(rect),
                SWP_NOACTIVATE | SWP_NOZORDER,
            ) {
                errors.push(format!("rect: {error}"));
            }
        }
        Err(error) => errors.push(format!("rect-coordinates: {error}")),
    }
    let z_order_after = if snapshot.z_order_after != 0 {
        snapshot.z_order_after
    } else if snapshot.topmost {
        HWND_TOPMOST as isize
    } else {
        HWND_TOP as isize
    };
    if let Err(error) = api.set_window_pos(
        hwnd,
        z_order_after,
        None,
        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED,
    ) {
        errors.push(format!("z-order: {error}"));
    }
    match api.parent(hwnd) {
        Ok(parent) if parent == snapshot.parent => {}
        Ok(parent) => errors.push(format!(
            "verification: parent is {parent}, expected {}",
            snapshot.parent
        )),
        Err(error) => errors.push(format!("verification parent: {error}")),
    }
    match api.window_long(hwnd, GWL_STYLE) {
        Ok(style) if style == snapshot.style => {}
        Ok(style) => errors.push(format!(
            "verification: style is {style:#x}, expected {:#x}",
            snapshot.style
        )),
        Err(error) => errors.push(format!("verification style: {error}")),
    }
    match api.window_long(hwnd, GWL_EXSTYLE) {
        Ok(ex_style) if ex_style == snapshot.ex_style => {}
        Ok(ex_style) => errors.push(format!(
            "verification: ex-style is {ex_style:#x}, expected {:#x}",
            snapshot.ex_style
        )),
        Err(error) => errors.push(format!("verification ex-style: {error}")),
    }
    match api.window_rect(hwnd) {
        Ok(rect) if rect_matches(rect, snapshot.rect) => {}
        Ok(_) => errors.push("verification: screen rectangle was not restored".into()),
        Err(error) => errors.push(format!("verification rectangle: {error}")),
    }
    match api.previous_window(hwnd) {
        Ok(previous) if previous == snapshot.z_order_after => {}
        Ok(previous) => errors.push(format!(
            "verification: z-order predecessor is {previous}, expected {}",
            snapshot.z_order_after
        )),
        Err(error) => errors.push(format!("verification z-order: {error}")),
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(host_error("restore", errors.join("; ")))
    }
}

fn attach_from_snapshot_with_api(
    api: &impl WindowApi,
    hwnd: isize,
    snapshot: &WindowHostSnapshot,
) -> Result<DesktopAttachOutcome, PlatformError> {
    match try_workerw(api, hwnd, snapshot) {
        Ok(outcome) => Ok(outcome),
        Err(worker_error) => {
            if let Err(restore_error) = restore_window_host_with_api(api, hwnd, snapshot) {
                let final_restore_error = restore_window_host_with_api(api, hwnd, snapshot).err();
                return Err(host_error(
                    "attach desktop",
                    match final_restore_error {
                        Some(final_error) => format!(
                            "WorkerW failed: {worker_error}; pre-fallback restore failed: {restore_error}; final restore failed: {final_error}"
                        ),
                        None => format!(
                            "WorkerW failed: {worker_error}; pre-fallback restore failed: {restore_error}; final restore succeeded"
                        ),
                    },
                ));
            }
            match try_bottom_fallback(api, hwnd, snapshot) {
                Ok(outcome) => Ok(outcome),
                Err(bottom_error) => {
                    let restore_error = restore_window_host_with_api(api, hwnd, snapshot).err();
                    Err(host_error(
                        "attach desktop",
                        match restore_error {
                            Some(error) => format!(
                                "WorkerW failed: {worker_error}; bottom fallback failed: {bottom_error}; restore failed: {error}"
                            ),
                            None => format!(
                                "WorkerW failed: {worker_error}; bottom fallback failed: {bottom_error}"
                            ),
                        },
                    ))
                }
            }
        }
    }
}

fn desktop_host_alive_with_api(
    api: &impl WindowApi,
    hwnd: isize,
    host: DesktopAttachOutcome,
) -> Result<bool, PlatformError> {
    match host {
        DesktopAttachOutcome::WorkerW { parent } => Ok(parent != 0 && api.is_window(parent)?),
        DesktopAttachOutcome::BottomFallback => api.is_window(hwnd),
    }
}

#[cfg(test)]
fn attach_desktop_with_api(
    api: &impl WindowApi,
    hwnd: isize,
) -> Result<(WindowHostSnapshot, DesktopAttachOutcome), PlatformError> {
    let snapshot = capture_window_host_with_api(api, hwnd)?;
    let outcome = attach_from_snapshot_with_api(api, hwnd, &snapshot)?;
    Ok((snapshot, outcome))
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
            let capture_env = std::env::var("DESKTOP_PET_TASK15_CAPTURE").ok();
            let stealth_styles = if should_apply_pet_stealth_styles(
                cfg!(debug_assertions),
                capture_env.as_deref(),
            ) {
                WS_EX_NOACTIVATE as isize | WS_EX_TOOLWINDOW as isize
            } else {
                0
            };
            SetLastError(0);
            let previous = SetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE, style | stealth_styles);
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

    fn probe_fullscreen(
        &self,
        own_pid: u32,
        pet_hwnd: isize,
    ) -> Result<FullscreenSnapshot, PlatformError> {
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
            if monitor.is_null() {
                return Err(last_error("MonitorFromWindow(foreground)"));
            }
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
            let pet_monitor = MonitorFromWindow(pet_hwnd as HWND, MONITOR_DEFAULTTONEAREST);
            if pet_monitor.is_null() {
                return Err(last_error("MonitorFromWindow(pet)"));
            }
            let mut pet_monitor_info: MONITORINFO = std::mem::zeroed();
            pet_monitor_info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(pet_monitor, &mut pet_monitor_info) == 0 {
                return Err(last_error("GetMonitorInfoW(pet)"));
            }
            let pet_monitor_rect = ScreenRect {
                left: pet_monitor_info.rcMonitor.left,
                top: pet_monitor_info.rcMonitor.top,
                right: pet_monitor_info.rcMonitor.right,
                bottom: pet_monitor_info.rcMonitor.bottom,
            };
            let is_fullscreen = fullscreen_covers_pet_monitor(
                ScreenRect {
                    left: window_rect.left,
                    top: window_rect.top,
                    right: window_rect.right,
                    bottom: window_rect.bottom,
                },
                monitor_rect,
                pet_monitor_rect,
            );

            Ok(FullscreenSnapshot {
                is_fullscreen,
                foreground_hwnd: Some(foreground as isize),
                monitor_rect: Some(monitor_rect),
                reason: if is_fullscreen {
                    "foreground-covers-monitor"
                } else {
                    if monitor_rect != pet_monitor_rect {
                        "fullscreen-on-other-monitor"
                    } else {
                        "not-fullscreen"
                    }
                },
            })
        }
    }

    fn capture_window_host(&self, hwnd: isize) -> Result<WindowHostSnapshot, PlatformError> {
        capture_window_host_with_api(&NativeWindowApi, hwnd)
    }

    fn attach_desktop_host(
        &self,
        hwnd: isize,
        snapshot: &WindowHostSnapshot,
    ) -> Result<DesktopAttachOutcome, PlatformError> {
        attach_from_snapshot_with_api(&NativeWindowApi, hwnd, snapshot)
    }

    fn restore_window_host(
        &self,
        hwnd: isize,
        snapshot: &WindowHostSnapshot,
    ) -> Result<(), PlatformError> {
        restore_window_host_with_api(&NativeWindowApi, hwnd, snapshot)
    }

    fn desktop_host_alive(
        &self,
        hwnd: isize,
        host: DesktopAttachOutcome,
    ) -> Result<bool, PlatformError> {
        desktop_host_alive_with_api(&NativeWindowApi, hwnd, host)
    }

    fn probe_window_visibility(&self, hwnd: isize) -> Result<WindowVisibilityFacts, PlatformError> {
        use windows_sys::Win32::{
            Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWM_CLOAKED_SHELL},
            UI::WindowsAndMessaging::IsWindowVisible,
        };
        unsafe {
            if IsWindow(hwnd as HWND) == 0 {
                return Err(host_error("visibility probe", "pet HWND is not valid"));
            }
            let mut cloaked = 0u32;
            let result = DwmGetWindowAttribute(
                hwnd as HWND,
                DWMWA_CLOAKED as u32,
                (&mut cloaked as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
            if result < 0 {
                return Err(PlatformError::WindowHost {
                    operation: "DwmGetWindowAttribute(DWMWA_CLOAKED)",
                    detail: format!("HRESULT {result:#x}"),
                });
            }
            let ex_style = GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE);
            Ok(WindowVisibilityFacts {
                visible: IsWindowVisible(hwnd as HWND) != 0,
                shell_cloaked: cloaked & DWM_CLOAKED_SHELL != 0,
                topmost: ex_style & WS_EX_TOPMOST as isize != 0,
            })
        }
    }

    fn ensure_companion_window(&self, hwnd: isize) -> Result<(), PlatformError> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNA};
        unsafe {
            if IsWindow(hwnd as HWND) == 0 {
                return Err(host_error("companion restore", "pet HWND is not valid"));
            }
            ShowWindow(hwnd as HWND, SW_SHOWNA);
            if SetWindowPos(
                hwnd as HWND,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            ) == 0
            {
                return Err(last_error("SetWindowPos companion restore"));
            }
            Ok(())
        }
    }
}

fn last_error(operation: &'static str) -> PlatformError {
    PlatformError::WindowsApi {
        operation,
        code: unsafe { GetLastError() },
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::BTreeSet};

    use super::{
        attach_desktop_with_api, capture_window_host_with_api, desktop_host_alive_with_api,
        find_workerw_host, fullscreen_covers_pet_monitor, host_error, restore_window_host_with_api,
        should_apply_pet_stealth_styles, DesktopAttachOutcome, PlatformError, ScreenRect,
        WindowApi, WindowHostSnapshot, GWL_EXSTYLE, GWL_STYLE, SWP_NOMOVE, SWP_NOSIZE, WS_CHILD,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    const HWND_TEST: isize = 42;
    const WORKERW: isize = 900;

    #[test]
    fn companion_visibility_reassertion_is_no_activate_and_non_invasive() {
        let source = include_str!("windows.rs");
        let start = source
            .find("fn ensure_companion_window")
            .expect("companion restore implementation");
        let end = source[start..]
            .find("fn last_error")
            .map(|offset| start + offset)
            .expect("last error helper after platform adapter");
        let implementation = &source[start..end];

        assert!(implementation.contains("SW_SHOWNA"), "{implementation}");
        assert!(implementation.contains("HWND_TOPMOST"), "{implementation}");
        assert!(
            implementation.contains("SWP_NOACTIVATE"),
            "{implementation}"
        );
        assert!(!implementation.contains("SetParent("), "{implementation}");
        assert!(!implementation.contains("FindWindow"), "{implementation}");
        assert!(!implementation.contains("SendMessage"), "{implementation}");
    }

    #[derive(Clone)]
    struct FakeWindowApi {
        state: std::rc::Rc<RefCell<FakeState>>,
    }

    struct FakeState {
        phases: Vec<&'static str>,
        operations: Vec<&'static str>,
        phase: &'static str,
        parent: isize,
        style: isize,
        ex_style: isize,
        rect: ScreenRect,
        previous: isize,
        worker_error: bool,
        bottom_error: bool,
        worker_alive: bool,
        pet_alive: bool,
        bottom_rect_drift: bool,
        ignore_restore_ex_style: bool,
        restore_failures: BTreeSet<&'static str>,
        client_offset: (i32, i32),
        dpi_compatible: bool,
        invalid_first_workerw_candidate: bool,
        defview_present: bool,
        workerw_present: bool,
        failures: BTreeSet<&'static str>,
        restore_failure_sequence: Vec<&'static str>,
        current_restore_failure: Option<&'static str>,
    }

    impl Default for FakeWindowApi {
        fn default() -> Self {
            Self {
                state: std::rc::Rc::new(RefCell::new(FakeState {
                    phases: Vec::new(),
                    operations: Vec::new(),
                    phase: "idle",
                    parent: 0,
                    style: 0x1234,
                    ex_style: WS_EX_TOPMOST as isize
                        | WS_EX_NOACTIVATE as isize
                        | WS_EX_TOOLWINDOW as isize,
                    rect: ScreenRect {
                        left: 100,
                        top: 200,
                        right: 300,
                        bottom: 500,
                    },
                    previous: 77,
                    worker_error: false,
                    bottom_error: false,
                    worker_alive: true,
                    pet_alive: true,
                    bottom_rect_drift: false,
                    ignore_restore_ex_style: false,
                    restore_failures: BTreeSet::new(),
                    client_offset: (10, 20),
                    dpi_compatible: true,
                    invalid_first_workerw_candidate: false,
                    defview_present: true,
                    workerw_present: true,
                    failures: BTreeSet::new(),
                    restore_failure_sequence: Vec::new(),
                    current_restore_failure: None,
                })),
            }
        }
    }

    impl FakeWindowApi {
        fn workerw_error() -> Self {
            let api = Self::default();
            api.state.borrow_mut().worker_error = true;
            api
        }

        fn bottom_error(self) -> Self {
            self.state.borrow_mut().bottom_error = true;
            self
        }

        fn with_parent(self, parent: isize) -> Self {
            self.state.borrow_mut().parent = parent;
            self
        }

        fn with_existing_child_parent(self, parent: isize) -> Self {
            let mut state = self.state.borrow_mut();
            state.parent = parent;
            state.style = WS_CHILD as isize | 0x34;
            drop(state);
            self
        }

        fn with_restore_failures(self, failures: &[&'static str]) -> Self {
            self.state
                .borrow_mut()
                .restore_failures
                .extend(failures.iter().copied());
            self
        }

        fn with_incompatible_dpi(self) -> Self {
            self.state.borrow_mut().dpi_compatible = false;
            self
        }

        fn with_invalid_first_workerw_candidate(self) -> Self {
            self.state.borrow_mut().invalid_first_workerw_candidate = true;
            self
        }

        fn without_defview(self) -> Self {
            self.state.borrow_mut().defview_present = false;
            self
        }

        fn without_workerw(self) -> Self {
            self.state.borrow_mut().workerw_present = false;
            self
        }

        fn with_failure(self, failure: &'static str) -> Self {
            self.state.borrow_mut().failures.insert(failure);
            self
        }

        fn with_restore_failure_sequence(self, failures: &[&'static str]) -> Self {
            self.state.borrow_mut().restore_failure_sequence =
                failures.iter().rev().copied().collect();
            self
        }

        fn calls(&self) -> Vec<&'static str> {
            self.state.borrow().phases.clone()
        }

        fn operations(&self) -> Vec<&'static str> {
            self.state.borrow().operations.clone()
        }

        fn snapshot(&self) -> WindowHostSnapshot {
            capture_window_host_with_api(self, HWND_TEST).unwrap()
        }
    }

    impl WindowApi for FakeWindowApi {
        fn note_phase(&self, phase: &'static str) {
            let mut state = self.state.borrow_mut();
            state.phase = phase;
            state.phases.push(phase);
            if phase == "restore" {
                state.current_restore_failure = state.restore_failure_sequence.pop();
            }
        }

        fn is_window(&self, hwnd: isize) -> Result<bool, PlatformError> {
            let state = self.state.borrow();
            if state.phase == "capture" && state.failures.contains("capture-is-window") {
                return Err(host_error("fake capture IsWindow", "capture-is-window"));
            }
            Ok(match hwnd {
                HWND_TEST => state.pet_alive,
                WORKERW => state.worker_alive,
                901 => false,
                100 | 200 | 300 => true,
                _ => hwnd != 0,
            })
        }

        fn parent(&self, _hwnd: isize) -> Result<isize, PlatformError> {
            let state = self.state.borrow();
            let failure = match state.phase {
                "capture" => "capture-parent",
                "workerw" => "worker-readback-parent",
                "bottom" => "bottom-readback-parent",
                "restore" => "restore-readback-parent",
                _ => "",
            };
            if state.failures.contains(failure) {
                Err(host_error("fake parent read", failure))
            } else {
                Ok(state.parent)
            }
        }

        fn window_long(&self, _hwnd: isize, index: i32) -> Result<isize, PlatformError> {
            let state = self.state.borrow();
            let failure = match (state.phase, index) {
                ("capture", GWL_STYLE) => "capture-style",
                ("capture", GWL_EXSTYLE) => "capture-ex-style",
                ("workerw", GWL_STYLE) => "worker-readback-style",
                ("workerw", GWL_EXSTYLE) => "worker-readback-ex-style",
                ("bottom", GWL_STYLE) => "bottom-readback-style",
                ("bottom", GWL_EXSTYLE) => "bottom-readback-ex-style",
                ("restore", GWL_STYLE) => "restore-readback-style",
                ("restore", GWL_EXSTYLE) => "restore-readback-ex-style",
                _ => "",
            };
            if state.failures.contains(failure) {
                return Err(host_error("fake window-long read", failure));
            }
            match index {
                GWL_STYLE => Ok(state.style),
                GWL_EXSTYLE => Ok(state.ex_style),
                _ => Err(host_error("fake", "unsupported index")),
            }
        }

        fn window_rect(&self, _hwnd: isize) -> Result<ScreenRect, PlatformError> {
            let state = self.state.borrow();
            let failure = match state.phase {
                "capture" => "capture-rect",
                "workerw" => "worker-readback-rect",
                "bottom" => "bottom-readback-rect",
                "restore" => "restore-readback-rect",
                _ => "",
            };
            if state.failures.contains(failure) {
                return Err(host_error("fake rect read", failure));
            }
            let mut rect = state.rect;
            if state.phase == "bottom" && state.bottom_rect_drift {
                rect.left += 3;
            }
            Ok(rect)
        }

        fn previous_window(&self, _hwnd: isize) -> Result<isize, PlatformError> {
            let state = self.state.borrow();
            if state.phase == "capture" && state.failures.contains("capture-z-order") {
                Err(host_error("fake z-order read", "capture-z-order"))
            } else {
                Ok(state.previous)
            }
        }

        fn find_window(&self, class_name: &str) -> Result<Option<isize>, PlatformError> {
            if self.state.borrow().failures.contains("discover-progman") {
                return Err(host_error("fake Progman discovery", "discover-progman"));
            }
            Ok((class_name == "Progman").then_some(100))
        }

        fn spawn_workerw(&self, _progman: isize) -> Result<(), PlatformError> {
            if self.state.borrow().worker_error
                || self.state.borrow().failures.contains("spawn-workerw")
            {
                Err(host_error("fake WorkerW", "injected failure"))
            } else {
                Ok(())
            }
        }

        fn top_level_windows(&self) -> Result<Vec<isize>, PlatformError> {
            if self.state.borrow().failures.contains("enumerate-windows") {
                return Err(host_error("fake EnumWindows", "enumerate-windows"));
            }
            Ok(if self.state.borrow().invalid_first_workerw_candidate {
                vec![200, 201]
            } else {
                vec![200]
            })
        }

        fn find_child(
            &self,
            parent: isize,
            after: isize,
            class_name: &str,
        ) -> Result<Option<isize>, PlatformError> {
            if self.state.borrow().failures.contains("discover-child") {
                return Err(host_error("fake child discovery", "discover-child"));
            }
            match (parent, after, class_name) {
                (200, 0, "SHELLDLL_DefView") if !self.state.borrow().defview_present => Ok(None),
                (200, 0, "SHELLDLL_DefView") => Ok(Some(300)),
                (0, 200, "WorkerW") if !self.state.borrow().workerw_present => Ok(None),
                (0, 200, "WorkerW") if self.state.borrow().invalid_first_workerw_candidate => {
                    Ok(Some(901))
                }
                (0, 200, "WorkerW") => Ok(Some(WORKERW)),
                (201, 0, "SHELLDLL_DefView") => Ok(Some(301)),
                (0, 201, "WorkerW") => Ok(Some(WORKERW)),
                _ => Ok(None),
            }
        }

        fn dpi_contexts_compatible(
            &self,
            _child: isize,
            _parent: isize,
        ) -> Result<bool, PlatformError> {
            if self.state.borrow().failures.contains("worker-dpi-query") {
                Err(host_error("fake DPI query", "worker-dpi-query"))
            } else {
                Ok(self.state.borrow().dpi_compatible)
            }
        }

        fn set_parent(&self, _hwnd: isize, parent: isize) -> Result<(), PlatformError> {
            let mut state = self.state.borrow_mut();
            if state.phase == "workerw" {
                state.operations.push("worker-parent");
                if state.style & WS_CHILD as isize == 0 || state.style & WS_POPUP as isize != 0 {
                    return Err(host_error("fake order", "SetParent before child style"));
                }
                if state.failures.contains("worker-parent") {
                    return Err(host_error("SetParent", "injected failure"));
                }
            }
            if state.phase == "bottom" {
                state.operations.push("bottom-parent");
                if state.failures.contains("bottom-parent") {
                    return Err(host_error("fake bottom parent", "injected failure"));
                }
            }
            if state.phase == "restore" {
                state.operations.push("parent");
                if let Some(error) = state.current_restore_failure.take() {
                    return Err(host_error("fake restore parent", error));
                }
                if state.restore_failures.contains("parent") {
                    return Err(host_error("fake restore parent", "injected failure"));
                }
            }
            state.parent = parent;
            Ok(())
        }

        fn set_window_long(
            &self,
            _hwnd: isize,
            index: i32,
            value: isize,
        ) -> Result<(), PlatformError> {
            let mut state = self.state.borrow_mut();
            if index == GWL_STYLE {
                if state.phase == "workerw" {
                    state.operations.push("worker-style");
                    if state.failures.contains("worker-style") {
                        return Err(host_error("fake worker style", "injected failure"));
                    }
                }
                if state.phase == "bottom" {
                    state.operations.push("bottom-style");
                    if state.failures.contains("bottom-style") {
                        return Err(host_error("fake bottom style", "injected failure"));
                    }
                }
                if state.phase == "restore" {
                    state.operations.push("style");
                    if state.restore_failures.contains("style") {
                        return Err(host_error("fake restore style", "injected failure"));
                    }
                }
                state.style = value;
            } else if index == GWL_EXSTYLE {
                if state.phase == "workerw" {
                    state.operations.push("worker-ex-style");
                    if state.parent != WORKERW {
                        return Err(host_error("fake order", "ex-style before SetParent"));
                    }
                    if state.failures.contains("worker-ex-style") {
                        return Err(host_error("fake worker ex-style", "injected failure"));
                    }
                }
                if state.phase == "bottom" {
                    state.operations.push("bottom-ex-style");
                    if state.failures.contains("bottom-ex-style") {
                        return Err(host_error("fake bottom ex-style", "injected failure"));
                    }
                }
                if state.phase == "restore" {
                    state.operations.push("ex-style");
                    if state.restore_failures.contains("ex-style") {
                        return Err(host_error("fake restore ex-style", "injected failure"));
                    }
                }
                if !(state.phase == "restore" && state.ignore_restore_ex_style) {
                    state.ex_style = value;
                }
            }
            Ok(())
        }

        fn screen_to_client(
            &self,
            parent: isize,
            point: (i32, i32),
        ) -> Result<(i32, i32), PlatformError> {
            let state = self.state.borrow();
            let failure = match state.phase {
                "workerw" => "worker-screen-to-client",
                "bottom" => "bottom-screen-to-client",
                _ => "",
            };
            if state.failures.contains(failure) {
                return Err(host_error("fake ScreenToClient", failure));
            }
            if state.phase == "restore" && state.restore_failures.contains("coordinates") {
                return Err(host_error("fake coordinates", "injected failure"));
            }
            if parent == 0 {
                Ok(point)
            } else {
                Ok((
                    point.0 - state.client_offset.0,
                    point.1 - state.client_offset.1,
                ))
            }
        }

        fn set_window_pos(
            &self,
            _hwnd: isize,
            insert_after: isize,
            rect: Option<ScreenRect>,
            flags: u32,
        ) -> Result<(), PlatformError> {
            let mut state = self.state.borrow_mut();
            if state.phase == "workerw" {
                state.operations.push("worker-position");
                if state.failures.contains("worker-position") {
                    return Err(host_error("fake worker position", "injected failure"));
                }
                if state.parent != WORKERW {
                    return Err(host_error("fake order", "position before SetParent"));
                }
            }
            if state.phase == "bottom" {
                state.operations.push("bottom-position");
                if state.failures.contains("bottom-position") {
                    return Err(host_error("fake bottom position", "injected failure"));
                }
            }
            if state.phase == "bottom" && state.bottom_error {
                return Err(host_error("fake bottom", "injected failure"));
            }
            if state.phase == "restore" {
                let operation = if flags & (SWP_NOMOVE | SWP_NOSIZE) == 0 {
                    "rect"
                } else {
                    "z-order"
                };
                state.operations.push(operation);
                if state.restore_failures.contains(operation) {
                    return Err(host_error("fake restore position", "injected failure"));
                }
            }
            if let Some(mut rect) = rect {
                if state.parent != 0 {
                    rect.left += state.client_offset.0;
                    rect.right += state.client_offset.0;
                    rect.top += state.client_offset.1;
                    rect.bottom += state.client_offset.1;
                }
                state.rect = rect;
            }
            if flags & super::SWP_NOZORDER == 0 {
                state.previous = if insert_after <= 1 { 0 } else { insert_after };
            }
            Ok(())
        }
    }

    #[test]
    fn workerw_discovery_distinguishes_missing_defview_from_missing_valid_workerw() {
        let no_defview = find_workerw_host(&FakeWindowApi::default().without_defview())
            .unwrap_err()
            .to_string();
        assert!(no_defview.contains("SHELLDLL_DefView host was not found"));

        let no_workerw = find_workerw_host(&FakeWindowApi::default().without_workerw())
            .unwrap_err()
            .to_string();
        assert!(no_workerw.contains("DefView was found but no valid adjacent WorkerW"));
        assert_ne!(no_defview, no_workerw);
    }

    #[test]
    fn fullscreen_only_suppresses_a_pet_on_the_same_monitor() {
        let primary = ScreenRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let secondary = ScreenRect {
            left: 1920,
            top: 0,
            right: 3840,
            bottom: 1080,
        };
        assert!(fullscreen_covers_pet_monitor(primary, primary, primary));
        assert!(!fullscreen_covers_pet_monitor(
            secondary, secondary, primary
        ));
        assert!(!fullscreen_covers_pet_monitor(
            ScreenRect {
                left: 0,
                top: 0,
                right: 1280,
                bottom: 720
            },
            primary,
            primary,
        ));
    }

    #[test]
    fn desktop_attach_tries_workerw_then_bottom_and_restores_on_double_failure() {
        let api = FakeWindowApi::workerw_error().bottom_error();
        let original = api.snapshot();
        api.state.borrow_mut().phases.clear();
        let result = attach_desktop_with_api(&api, HWND_TEST);
        assert!(result.is_err());
        assert_eq!(
            api.calls(),
            ["capture", "workerw", "restore", "bottom", "restore"]
        );
        let state = api.state.borrow();
        assert_eq!(state.parent, original.parent);
        assert_eq!(state.style, original.style);
        assert_eq!(state.ex_style, original.ex_style);
        assert_eq!(state.rect, original.rect);
    }

    #[test]
    fn desktop_attach_uses_verified_workerw_and_preserves_screen_rect() {
        let api = FakeWindowApi::default();
        let (snapshot, outcome) = attach_desktop_with_api(&api, HWND_TEST).unwrap();
        assert_eq!(outcome, DesktopAttachOutcome::WorkerW { parent: WORKERW });
        assert_eq!(api.calls(), ["capture", "workerw"]);
        let state = api.state.borrow();
        assert_eq!(state.parent, WORKERW);
        assert_eq!(state.rect, snapshot.rect);
        assert_eq!(
            state.style,
            (snapshot.style | WS_CHILD as isize) & !(WS_POPUP as isize)
        );
        assert_eq!(
            state.ex_style,
            snapshot.ex_style & !(WS_EX_TOPMOST as isize)
        );
        assert_ne!(state.ex_style & WS_EX_NOACTIVATE as isize, 0);
        assert_ne!(state.ex_style & WS_EX_TOOLWINDOW as isize, 0);
    }

    #[test]
    fn desktop_attach_restores_before_verified_bottom_fallback() {
        let api = FakeWindowApi::workerw_error();
        let (snapshot, outcome) = attach_desktop_with_api(&api, HWND_TEST).unwrap();
        assert_eq!(outcome, DesktopAttachOutcome::BottomFallback);
        assert_eq!(api.calls(), ["capture", "workerw", "restore", "bottom"]);
        let state = api.state.borrow();
        assert_eq!(state.parent, snapshot.parent);
        assert_eq!(state.style, snapshot.style);
        assert_eq!(
            state.ex_style,
            snapshot.ex_style & !(WS_EX_TOPMOST as isize)
        );
        assert_eq!(state.rect, snapshot.rect);
    }

    #[test]
    fn bottom_verification_rejects_more_than_two_pixel_drift_and_restores() {
        let api = FakeWindowApi::workerw_error();
        api.state.borrow_mut().bottom_rect_drift = true;
        let original = api.snapshot();
        api.state.borrow_mut().phases.clear();
        let error = attach_desktop_with_api(&api, HWND_TEST).unwrap_err();
        assert!(error.to_string().contains("more than 2px"));
        assert_eq!(
            api.calls(),
            ["capture", "workerw", "restore", "bottom", "restore"]
        );
        let state = api.state.borrow();
        assert_eq!(state.parent, original.parent);
        assert_eq!(state.ex_style, original.ex_style);
        assert_eq!(state.rect, original.rect);
    }

    #[test]
    fn restore_is_ordered_best_effort_and_aggregates_partial_failures() {
        let api = FakeWindowApi::default().with_restore_failures(&["style", "z-order"]);
        let snapshot = api.snapshot();
        api.state.borrow_mut().operations.clear();
        let error = restore_window_host_with_api(&api, HWND_TEST, &snapshot).unwrap_err();
        assert_eq!(
            api.operations(),
            ["parent", "style", "ex-style", "rect", "z-order"]
        );
        let message = error.to_string();
        assert!(message.contains("style:"));
        assert!(message.contains("z-order:"));
    }

    #[test]
    fn restore_is_idempotent_and_converts_screen_rect_for_existing_parent() {
        let api = FakeWindowApi::default().with_parent(55);
        let snapshot = api.snapshot();
        api.state.borrow_mut().parent = WORKERW;
        api.state.borrow_mut().rect = ScreenRect {
            left: 1,
            top: 2,
            right: 3,
            bottom: 4,
        };
        restore_window_host_with_api(&api, HWND_TEST, &snapshot).unwrap();
        restore_window_host_with_api(&api, HWND_TEST, &snapshot).unwrap();
        let state = api.state.borrow();
        assert_eq!(state.parent, 55);
        assert_eq!(state.rect, snapshot.rect);
        assert_eq!(state.ex_style, snapshot.ex_style);
    }

    #[test]
    fn restore_rejects_a_silent_partial_restore() {
        let api = FakeWindowApi::default();
        let snapshot = api.snapshot();
        {
            let mut state = api.state.borrow_mut();
            state.ex_style &= !(WS_EX_TOPMOST as isize);
            state.ignore_restore_ex_style = true;
        }
        let error = restore_window_host_with_api(&api, HWND_TEST, &snapshot).unwrap_err();
        assert!(error.to_string().contains("verification"));
    }

    #[test]
    fn invalid_workerw_is_rejected_before_set_parent_and_falls_back() {
        let api = FakeWindowApi::default();
        api.state.borrow_mut().worker_alive = false;
        let (_, outcome) = attach_desktop_with_api(&api, HWND_TEST).unwrap();
        assert_eq!(outcome, DesktopAttachOutcome::BottomFallback);
        assert_eq!(api.calls(), ["capture", "workerw", "restore", "bottom"]);
    }

    #[test]
    fn desktop_host_liveness_uses_worker_parent_or_pet_for_fallback() {
        let api = FakeWindowApi::default();
        assert!(desktop_host_alive_with_api(
            &api,
            HWND_TEST,
            DesktopAttachOutcome::WorkerW { parent: WORKERW }
        )
        .unwrap());
        api.state.borrow_mut().worker_alive = false;
        assert!(!desktop_host_alive_with_api(
            &api,
            HWND_TEST,
            DesktopAttachOutcome::WorkerW { parent: WORKERW }
        )
        .unwrap());
        assert!(
            desktop_host_alive_with_api(&api, HWND_TEST, DesktopAttachOutcome::BottomFallback)
                .unwrap()
        );
        api.state.borrow_mut().pet_alive = false;
        assert!(!desktop_host_alive_with_api(
            &api,
            HWND_TEST,
            DesktopAttachOutcome::BottomFallback
        )
        .unwrap());
    }

    #[test]
    fn workerw_style_is_changed_before_set_parent() {
        let api = FakeWindowApi::default();
        attach_desktop_with_api(&api, HWND_TEST).unwrap();
        let operations = api.operations();
        let style = operations
            .iter()
            .position(|call| *call == "worker-style")
            .unwrap();
        let parent = operations
            .iter()
            .position(|call| *call == "worker-parent")
            .unwrap();
        assert!(style < parent, "operations: {operations:?}");
    }

    #[test]
    fn incompatible_dpi_is_rejected_before_workerw_mutation_then_falls_back() {
        let api = FakeWindowApi::default().with_incompatible_dpi();
        let (_, outcome) = attach_desktop_with_api(&api, HWND_TEST).unwrap();
        assert_eq!(outcome, DesktopAttachOutcome::BottomFallback);
        assert!(!api
            .operations()
            .iter()
            .any(|call| call.starts_with("worker-")));
    }

    #[test]
    fn workerw_discovery_skips_invalid_first_candidate() {
        let api = FakeWindowApi::default().with_invalid_first_workerw_candidate();
        let (_, outcome) = attach_desktop_with_api(&api, HWND_TEST).unwrap();
        assert_eq!(outcome, DesktopAttachOutcome::WorkerW { parent: WORKERW });
    }

    #[test]
    fn set_parent_failure_restores_the_original_snapshot() {
        let api = FakeWindowApi::default()
            .with_failure("worker-parent")
            .bottom_error();
        let snapshot = api.snapshot();
        api.state.borrow_mut().phases.clear();
        let error = attach_desktop_with_api(&api, HWND_TEST).unwrap_err();
        assert!(error.to_string().contains("SetParent"));
        assert_eq!(
            api.calls(),
            ["capture", "workerw", "restore", "bottom", "restore"]
        );
        assert_eq!(
            &api.operations()[..7],
            [
                "worker-style",
                "worker-parent",
                "parent",
                "style",
                "ex-style",
                "rect",
                "z-order"
            ]
        );
        let state = api.state.borrow();
        assert_eq!(state.parent, snapshot.parent);
        assert_eq!(state.style, snapshot.style);
        assert_eq!(state.ex_style, snapshot.ex_style);
        assert_eq!(state.rect, snapshot.rect);
        assert_eq!(state.previous, snapshot.z_order_after);
    }

    #[test]
    fn pre_fallback_and_final_restore_errors_are_both_reported() {
        let api = FakeWindowApi::workerw_error()
            .with_restore_failure_sequence(&["first restore", "final restore"]);
        let error = attach_desktop_with_api(&api, HWND_TEST)
            .unwrap_err()
            .to_string();
        assert!(error.contains("first restore"), "{error}");
        assert!(error.contains("final restore"), "{error}");
    }

    #[test]
    fn capture_failure_matrix_never_mutates_the_window() {
        for failure in [
            "capture-is-window",
            "capture-parent",
            "capture-style",
            "capture-ex-style",
            "capture-rect",
            "capture-z-order",
        ] {
            let api = FakeWindowApi::default().with_failure(failure);
            let error = attach_desktop_with_api(&api, HWND_TEST)
                .unwrap_err()
                .to_string();
            assert!(error.contains(failure), "{failure}: {error}");
            assert!(
                api.operations().is_empty(),
                "{failure}: {:?}",
                api.operations()
            );
        }
    }

    #[test]
    fn workerw_step_failure_matrix_restores_before_a_verified_fallback() {
        for failure in [
            "discover-progman",
            "spawn-workerw",
            "enumerate-windows",
            "discover-child",
            "worker-dpi-query",
            "worker-style",
            "worker-parent",
            "worker-ex-style",
            "worker-screen-to-client",
            "worker-position",
            "worker-readback-parent",
            "worker-readback-style",
            "worker-readback-ex-style",
            "worker-readback-rect",
        ] {
            let api = FakeWindowApi::default().with_failure(failure);
            let snapshot = api.snapshot();
            api.state.borrow_mut().phases.clear();
            let (_, outcome) = attach_desktop_with_api(&api, HWND_TEST)
                .unwrap_or_else(|error| panic!("{failure}: {error}"));
            assert_eq!(outcome, DesktopAttachOutcome::BottomFallback, "{failure}");
            let state = api.state.borrow();
            assert_eq!(state.parent, snapshot.parent, "{failure}");
            assert_eq!(state.style, snapshot.style, "{failure}");
            assert_eq!(
                state.ex_style,
                snapshot.ex_style & !(WS_EX_TOPMOST as isize),
                "{failure}"
            );
            assert_eq!(state.rect, snapshot.rect, "{failure}");
        }
    }

    #[test]
    fn fallback_step_failure_matrix_finishes_with_the_original_snapshot() {
        for failure in [
            "bottom-parent",
            "bottom-style",
            "bottom-ex-style",
            "bottom-position",
            "bottom-readback-parent",
            "bottom-readback-style",
            "bottom-readback-ex-style",
            "bottom-readback-rect",
        ] {
            let api = FakeWindowApi::workerw_error().with_failure(failure);
            let snapshot = api.snapshot();
            api.state.borrow_mut().phases.clear();
            let error = attach_desktop_with_api(&api, HWND_TEST)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("bottom fallback failed"),
                "{failure}: {error}"
            );
            let state = api.state.borrow();
            assert_eq!(state.parent, snapshot.parent, "{failure}");
            assert_eq!(state.style, snapshot.style, "{failure}");
            assert_eq!(state.ex_style, snapshot.ex_style, "{failure}");
            assert_eq!(state.rect, snapshot.rect, "{failure}");
            assert_eq!(state.previous, snapshot.z_order_after, "{failure}");
        }
    }

    #[test]
    fn every_restore_mutation_failure_is_reported_after_best_effort_completion() {
        for failure in [
            "parent",
            "style",
            "ex-style",
            "coordinates",
            "rect",
            "z-order",
        ] {
            let api = FakeWindowApi::default().with_restore_failures(&[failure]);
            let snapshot = api.snapshot();
            {
                let mut state = api.state.borrow_mut();
                state.parent = WORKERW;
                state.style = WS_CHILD as isize;
                state.ex_style = 0;
                state.rect.left += 20;
                state.previous = 0;
                state.operations.clear();
            }
            let error = restore_window_host_with_api(&api, HWND_TEST, &snapshot)
                .unwrap_err()
                .to_string();
            assert!(error.contains(failure), "{failure}: {error}");
            let expected = if failure == "coordinates" {
                vec!["parent", "style", "ex-style", "z-order"]
            } else {
                vec!["parent", "style", "ex-style", "rect", "z-order"]
            };
            assert_eq!(api.operations(), expected, "{failure}");
        }
    }

    #[test]
    fn an_existing_child_parent_round_trips_without_top_level_assumptions() {
        let api = FakeWindowApi::default().with_existing_child_parent(55);
        let snapshot = api.snapshot();
        let (_, outcome) = attach_desktop_with_api(&api, HWND_TEST).unwrap();
        assert_eq!(outcome, DesktopAttachOutcome::WorkerW { parent: WORKERW });
        restore_window_host_with_api(&api, HWND_TEST, &snapshot).unwrap();
        let state = api.state.borrow();
        assert_eq!(state.parent, 55);
        assert_eq!(state.style, snapshot.style);
        assert_eq!(state.rect, snapshot.rect);
    }

    #[test]
    fn pet_window_stealth_style_policy_requires_exact_debug_capture_opt_in() {
        assert!(should_apply_pet_stealth_styles(true, None));
        assert!(!should_apply_pet_stealth_styles(true, Some("1")));
        assert!(should_apply_pet_stealth_styles(true, Some("true")));
        assert!(should_apply_pet_stealth_styles(false, Some("1")));
    }
}
