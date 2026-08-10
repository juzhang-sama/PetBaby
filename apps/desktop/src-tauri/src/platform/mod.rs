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

pub(crate) fn durable_replace_file(
    source: &std::path::Path,
    target: &std::path::Path,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        windows::durable_replace_file(source, target)
    }
    #[cfg(unix)]
    {
        std::fs::rename(source, target).map_err(|error| error.to_string())?;
        sync_directory(
            target
                .parent()
                .ok_or_else(|| "journal target has no parent".to_string())?,
        )?;
        if source.parent() != target.parent() {
            sync_directory(
                source
                    .parent()
                    .ok_or_else(|| "journal source has no parent".to_string())?,
            )?;
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "windows", unix)))]
    {
        std::fs::rename(source, target).map_err(|error| error.to_string())
    }
}

pub(crate) fn durable_directory_entry(path: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        sync_directory(
            path.parent()
                .ok_or_else(|| "durable directory has no parent".to_string())?,
        )
    }
    #[cfg(not(unix))]
    {
        // Windows orders the newly-created operation directory before resource isolation by
        // publishing journal.json inside it with MOVEFILE_WRITE_THROUGH. No isolation starts
        // until that child publication succeeds.
        let _ = path;
        Ok(())
    }
}

#[cfg(unix)]
fn sync_directory(path: &std::path::Path) -> Result<(), String> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
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
