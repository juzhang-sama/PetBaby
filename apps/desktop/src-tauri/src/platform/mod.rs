use crate::windowing::RegionSpan;

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("Windows API {operation} failed with code {code}")]
    WindowsApi { operation: &'static str, code: u32 },
    #[error("window host {operation} failed: {detail}")]
    WindowHost {
        operation: &'static str,
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowHostSnapshot {
    pub parent: isize,
    pub style: isize,
    pub ex_style: isize,
    pub rect: ScreenRect,
    pub topmost: bool,
    /// Window that immediately preceded the pet in its original Z order.
    /// Zero means the pet was first in that Z-order band.
    pub z_order_after: isize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DesktopAttachOutcome {
    WorkerW { parent: isize },
    BottomFallback,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullscreenSnapshot {
    pub is_fullscreen: bool,
    pub foreground_hwnd: Option<isize>,
    pub monitor_rect: Option<ScreenRect>,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowVisibilityFacts {
    pub visible: bool,
    pub shell_cloaked: bool,
    pub topmost: bool,
}

pub trait PlatformAdapter: Send + Sync {
    fn configure_pet_window(&self, hwnd: isize) -> Result<(), PlatformError>;
    fn apply_hit_region(&self, hwnd: isize, spans: &[RegionSpan]) -> Result<(), PlatformError>;
    fn probe_fullscreen(
        &self,
        own_pid: u32,
        pet_hwnd: isize,
    ) -> Result<FullscreenSnapshot, PlatformError>;
    fn capture_window_host(&self, hwnd: isize) -> Result<WindowHostSnapshot, PlatformError>;
    fn attach_desktop_host(
        &self,
        hwnd: isize,
        snapshot: &WindowHostSnapshot,
    ) -> Result<DesktopAttachOutcome, PlatformError>;
    fn restore_window_host(
        &self,
        hwnd: isize,
        snapshot: &WindowHostSnapshot,
    ) -> Result<(), PlatformError>;
    fn desktop_host_alive(
        &self,
        hwnd: isize,
        host: DesktopAttachOutcome,
    ) -> Result<bool, PlatformError>;
    fn probe_window_visibility(&self, hwnd: isize) -> Result<WindowVisibilityFacts, PlatformError>;
    fn ensure_companion_window(&self, hwnd: isize) -> Result<(), PlatformError>;
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

pub(crate) fn durable_move_file(
    source: &std::path::Path,
    target: &std::path::Path,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        windows::durable_move_file(source, target)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (source, target);
        Err("durable no-replace file move currently requires Windows".into())
    }
}

pub(crate) fn durable_move_directory(
    source: &std::path::Path,
    target: &std::path::Path,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        windows::durable_move_directory(source, target)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (source, target);
        Err("durable composer directory publication currently requires Windows".into())
    }
}

pub(crate) fn sync_existing_directory_entry(path: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        sync_directory(
            path.parent()
                .ok_or_else(|| "existing directory has no parent".to_string())?,
        )
    }
    #[cfg(not(unix))]
    {
        // A production Windows publish is only reported successful after MoveFileExW with
        // MOVEFILE_WRITE_THROUGH returns success. If the prior call returned an error, seeing
        // the directory after restart means the filesystem recovered a durable directory entry.
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

pub(crate) fn read_regular_file_no_reparse(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Result<Vec<u8>, String> {
    with_regular_file_no_reparse(root, path, |bytes| Ok(bytes.to_vec()))
}

pub(crate) fn with_regular_file_no_reparse<T>(
    root: &std::path::Path,
    path: &std::path::Path,
    callback: impl FnOnce(&[u8]) -> Result<T, String>,
) -> Result<T, String> {
    #[cfg(target_os = "windows")]
    {
        windows::with_regular_file_no_reparse(root, path, callback)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let bytes = read_regular_file_no_reparse_portable(root, path)?;
        callback(&bytes)
    }
}

#[cfg(all(test, target_os = "windows"))]
pub(crate) fn read_regular_file_no_reparse_with_hook(
    root: &std::path::Path,
    path: &std::path::Path,
    hook: impl FnOnce(),
) -> Result<Vec<u8>, String> {
    windows::read_regular_file_no_reparse_with_hook(root, path, hook)
}

#[cfg(not(target_os = "windows"))]
fn read_regular_file_no_reparse_portable(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Result<Vec<u8>, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "preview file escapes package root")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err("preview path contains an unsafe component".into());
        };
        current.push(name);
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("preview file path contains a link or reparse point".into());
        }
        if current != path && !metadata.is_dir() {
            return Err("preview file path contains a non-directory parent".into());
        }
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("preview path is not a regular file".into());
    }
    std::fs::read(path).map_err(|error| error.to_string())
}

#[cfg(all(test, target_os = "windows"))]
mod secure_file_tests {
    use std::sync::{Arc, Barrier};

    #[test]
    fn secure_preview_read_keeps_the_opened_file_identity_during_replacement() {
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-secure-read-{}",
            crate::creation::domain::new_entity_id("file")
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("manifest.json");
        std::fs::write(&path, b"trusted").unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = barrier.clone();
        let replacement = root.join("replacement.json");
        std::fs::write(&replacement, b"untrusted").unwrap();
        let replace_path = path.clone();
        let worker = std::thread::spawn(move || {
            worker_barrier.wait();
            std::fs::rename(replacement, replace_path)
        });

        let bytes = super::read_regular_file_no_reparse_with_hook(&root, &path, || {
            barrier.wait();
            let result = worker.join().unwrap();
            assert!(
                result.is_err(),
                "opened file must deny replacement until read completes"
            );
        })
        .unwrap();

        assert_eq!(bytes, b"trusted");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn secure_preview_read_rejects_an_intermediate_directory_junction() {
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-secure-read-{}",
            crate::creation::domain::new_entity_id("junction")
        ));
        let outside = std::env::temp_dir().join(format!(
            "desktop-pet-secure-read-{}",
            crate::creation::domain::new_entity_id("outside")
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
        super::create_directory_link(&outside, &root.join("linked"));

        let error = super::read_regular_file_no_reparse(&root, &root.join("linked/secret.txt"))
            .unwrap_err();

        assert!(error.contains("reparse point"), "{error}");

        let terminal_error =
            super::read_regular_file_no_reparse(&root, &root.join("linked")).unwrap_err();
        assert!(terminal_error.contains("reparse point"), "{terminal_error}");

        let _ = std::fs::remove_dir_all(root.join("linked"));
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }
}

#[cfg(target_os = "windows")]
pub(crate) mod windows;

#[cfg(target_os = "windows")]
pub use windows::WindowsPlatformAdapter;
