use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentRoot(PathBuf);

impl ContentRoot {
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

fn checked_content_root(path: &Path, label: &str) -> Result<ContentRoot, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("{label} content root is unavailable: {error}"))?;
    if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(format!("{label} content root must be a real directory"));
    }
    path.canonicalize()
        .map_err(|error| format!("{label} content root cannot be canonicalized: {error}"))
        .map(ContentRoot)
}

pub fn resolve_content_root(
    resource_dir: &Path,
    dev_public_dir: &Path,
) -> Result<ContentRoot, String> {
    let production = resource_dir.join("creation-content");
    match std::fs::symlink_metadata(&production) {
        Ok(_) => checked_content_root(&production, "production"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            checked_content_root(&dev_public_dir.join("creation-content"), "development")
        }
        Err(error) => Err(format!("production content root is unavailable: {error}")),
    }
}

#[cfg(test)]
pub(crate) fn test_content_root(path: &Path) -> Result<ContentRoot, String> {
    checked_content_root(path, "test")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NONCE: AtomicU64 = AtomicU64::new(0);

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-content-{label}-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn production_content_root_wins_over_dev_fallback() {
        let resources = temp_root("resources");
        let dev = temp_root("dev");
        std::fs::create_dir(resources.join("creation-content")).unwrap();
        std::fs::create_dir(dev.join("creation-content")).unwrap();
        assert_eq!(
            resolve_content_root(&resources, &dev).unwrap().as_path(),
            resources.join("creation-content").canonicalize().unwrap()
        );
        std::fs::remove_dir_all(resources).unwrap();
        std::fs::remove_dir_all(dev).unwrap();
    }

    #[test]
    fn dev_content_root_is_used_only_when_production_root_is_missing() {
        let resources = temp_root("missing-resources");
        let dev = temp_root("fallback");
        std::fs::create_dir(dev.join("creation-content")).unwrap();
        assert_eq!(
            resolve_content_root(&resources, &dev).unwrap().as_path(),
            dev.join("creation-content").canonicalize().unwrap()
        );
        std::fs::remove_dir_all(resources).unwrap();
        std::fs::remove_dir_all(dev).unwrap();
    }

    #[test]
    fn invalid_or_missing_roots_are_rejected_instead_of_accepting_arbitrary_paths() {
        let resources = temp_root("invalid-resources");
        let dev = temp_root("invalid-dev");
        assert!(resolve_content_root(&resources, &dev).is_err());

        std::fs::write(resources.join("creation-content"), b"not a directory").unwrap();
        std::fs::create_dir(dev.join("creation-content")).unwrap();
        assert!(resolve_content_root(&resources, &dev).is_err());
        std::fs::remove_dir_all(resources).unwrap();
        std::fs::remove_dir_all(dev).unwrap();
    }

    #[test]
    fn linked_content_roots_are_rejected() {
        let resources = temp_root("link-resources");
        let dev = temp_root("link-dev");
        let outside = temp_root("link-outside");
        crate::platform::create_directory_link(&outside, &resources.join("creation-content"));
        std::fs::create_dir(dev.join("creation-content")).unwrap();
        assert!(resolve_content_root(&resources, &dev).is_err());
        std::fs::remove_dir_all(resources).unwrap();
        std::fs::remove_dir_all(dev).unwrap();
        std::fs::remove_dir_all(outside).unwrap();

        let resources = temp_root("missing-link-resources");
        let dev = temp_root("linked-dev");
        let outside = temp_root("linked-dev-outside");
        crate::platform::create_directory_link(&outside, &dev.join("creation-content"));
        assert!(resolve_content_root(&resources, &dev).is_err());
        std::fs::remove_dir_all(resources).unwrap();
        std::fs::remove_dir_all(dev).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }
}
