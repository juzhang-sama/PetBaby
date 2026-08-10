use crate::windowing::RegionSpan;

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("Windows API {operation} failed with code {code}")]
    WindowsApi { operation: &'static str, code: u32 },
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullscreenSnapshot {
    pub is_fullscreen: bool,
    pub foreground_hwnd: Option<isize>,
    pub monitor_rect: Option<ScreenRect>,
    pub reason: &'static str,
}

pub trait PlatformAdapter: Send + Sync {
    fn configure_pet_window(&self, hwnd: isize) -> Result<(), PlatformError>;
    fn apply_hit_region(&self, hwnd: isize, spans: &[RegionSpan]) -> Result<(), PlatformError>;
    fn probe_fullscreen(&self, own_pid: u32) -> Result<FullscreenSnapshot, PlatformError>;
}

pub(crate) fn is_link_or_reparse_point(metadata: &std::fs::Metadata) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::is_reparse_point(metadata)
    }
    #[cfg(not(target_os = "windows"))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(test)]
pub(crate) fn create_directory_link(target: &std::path::Path, link: &std::path::Path) {
    #[cfg(target_os = "windows")]
    windows::create_directory_junction(target, link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(target_os = "windows")]
pub(crate) mod windows;

#[cfg(target_os = "windows")]
pub use windows::WindowsPlatformAdapter;
