use windows_sys::Win32::{
    Foundation::{GetLastError, HWND},
    Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_OR},
    UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    },
};

use crate::{
    platform::{PlatformAdapter, PlatformError},
    windowing::RegionSpan,
};

pub struct WindowsPlatformAdapter;

impl PlatformAdapter for WindowsPlatformAdapter {
    fn configure_pet_window(&self, hwnd: isize) -> Result<(), PlatformError> {
        unsafe {
            let style = GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE);
            windows_sys::Win32::Foundation::SetLastError(0);
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
}

fn last_error(operation: &'static str) -> PlatformError {
    PlatformError::WindowsApi {
        operation,
        code: unsafe { GetLastError() },
    }
}
