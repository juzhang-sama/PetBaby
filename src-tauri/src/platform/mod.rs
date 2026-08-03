use crate::windowing::{RegionSpan, WindowMode};

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("Windows API {operation} failed with code {code}")]
    WindowsApi { operation: &'static str, code: u32 },
    #[error("platform capability unavailable: {0}")]
    Unavailable(&'static str),
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowModeEvidence {
    pub requested: WindowMode,
    pub applied: bool,
    pub strategy: &'static str,
    pub parent_hwnd: Option<isize>,
}

pub trait PlatformAdapter: Send + Sync {
    fn configure_pet_window(&self, hwnd: isize) -> Result<(), PlatformError>;
    fn apply_hit_region(&self, hwnd: isize, spans: &[RegionSpan]) -> Result<(), PlatformError>;
    fn set_window_mode(
        &self,
        hwnd: isize,
        mode: WindowMode,
    ) -> Result<WindowModeEvidence, PlatformError>;
}

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::WindowsPlatformAdapter;
