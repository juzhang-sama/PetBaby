use crate::windowing::RegionSpan;

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("Windows API {operation} failed with code {code}")]
    WindowsApi { operation: &'static str, code: u32 },
    #[allow(dead_code)] // used by the Task 6 WorkerW desktop-mode probe
    #[error("platform capability unavailable: {0}")]
    Unavailable(&'static str),
}

pub trait PlatformAdapter: Send + Sync {
    fn configure_pet_window(&self, hwnd: isize) -> Result<(), PlatformError>;
    fn apply_hit_region(&self, hwnd: isize, spans: &[RegionSpan]) -> Result<(), PlatformError>;
}

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::WindowsPlatformAdapter;
