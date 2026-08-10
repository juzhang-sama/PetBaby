use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static DIRECTORY_NONCE: AtomicU64 = AtomicU64::new(0);

fn current_epoch_nanos() -> Result<u128, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())
        .map(|duration| duration.as_nanos())
}

pub fn staging_directory_for(dest_dir: &Path) -> Result<PathBuf, String> {
    let parent = dest_dir
        .parent()
        .ok_or("asset destination must have a parent directory")?;
    let name = dest_dir
        .file_name()
        .ok_or("asset destination must have a directory name")?
        .to_string_lossy();
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let epoch = current_epoch_nanos()?;
    loop {
        let nonce = DIRECTORY_NONCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{name}.{}-{epoch}-{nonce}.staging",
            std::process::id()
        ));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn move_destination_to_unique_backup(dest_dir: &Path) -> Result<PathBuf, String> {
    let parent = dest_dir
        .parent()
        .ok_or("asset destination must have a parent directory")?;
    let name = dest_dir
        .file_name()
        .ok_or("asset destination must have a directory name")?
        .to_string_lossy();
    let epoch = current_epoch_nanos()?;
    loop {
        let nonce = DIRECTORY_NONCE.fetch_add(1, Ordering::Relaxed);
        let backup = parent.join(format!(
            ".{name}.{}-{epoch}-{nonce}.backup",
            std::process::id()
        ));
        if backup.exists() {
            continue;
        }
        match std::fs::rename(dest_dir, &backup) {
            Ok(()) => return Ok(backup),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
}

pub fn install_staged_assets(staging_dir: &Path, dest_dir: &Path) -> Result<(), String> {
    install_staged_assets_with_cleanup(staging_dir, dest_dir, |backup| {
        std::fs::remove_dir_all(backup).map_err(|error| error.to_string())
    })
}

fn install_staged_assets_with_cleanup<F>(
    staging_dir: &Path,
    dest_dir: &Path,
    cleanup_backup: F,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let parent = dest_dir
        .parent()
        .ok_or("asset destination must have a parent directory")?;
    if staging_dir.parent() != Some(parent) {
        return Err("staging and destination must be sibling directories".into());
    }
    crate::runtime_assets::loader::validate_asset_directory(staging_dir)?;
    let had_destination = dest_dir.exists();
    let backup = had_destination
        .then(|| move_destination_to_unique_backup(dest_dir))
        .transpose()?;
    if let Err(error) = std::fs::rename(staging_dir, dest_dir) {
        if let Some(backup) = backup {
            return match std::fs::rename(backup, dest_dir) {
                Ok(()) => Err(format!("install staged assets: {error}")),
                Err(restore_error) => Err(format!(
                    "install staged assets: {error}; restore backup: {restore_error}"
                )),
            };
        }
        return Err(format!("install staged assets: {error}"));
    }

    // The staging -> destination rename is the commit point. From here on,
    // callers must observe success because the new assets are authoritative.
    if let Some(backup) = backup {
        if let Err(error) = cleanup_backup(&backup) {
            let warning = format!(
                "asset backup cleanup pending at {}: {error}",
                backup.display()
            );
            let marker = backup.join(".cleanup-pending");
            if let Err(marker_error) = std::fs::write(&marker, warning.as_bytes()) {
                eprintln!(
                    "warning: {warning}; failed to write {}: {marker_error}",
                    marker.display()
                );
            } else {
                eprintln!("warning: {warning}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn write_valid_assets(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        let body = b"body";
        let profile = serde_json::json!({
            "profileVersion": 1,
            "engineProfile": "life-v1",
            "alphaBounds": { "left": 0.1, "top": 0.05, "right": 0.9, "bottom": 0.96 },
            "breathZone": { "left": 0.2, "top": 0.5, "right": 0.8, "bottom": 0.84 },
            "swayPivot": { "x": 0.5, "y": 0.72 }
        })
        .to_string();
        std::fs::write(path.join("body.png"), body).unwrap();
        std::fs::write(path.join("motion-profile.json"), profile.as_bytes()).unwrap();
        let manifest = serde_json::json!({
            "schemaVersion": 3,
            "renderer": "animated-image-v1",
            "petId": "pet-1",
            "variantId": "variant-1",
            "image": "body.png",
            "motionProfile": "motion-profile.json",
            "files": [
                { "role": "main", "relativePath": "body.png", "sha256": sha256_hex(body) },
                { "role": "motion-profile", "relativePath": "motion-profile.json", "sha256": sha256_hex(profile.as_bytes()) }
            ]
        });
        std::fs::write(
            path.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn backup_directories(root: &Path) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".backup"))
            })
            .collect()
    }

    #[test]
    fn atomically_replaces_existing_assets_with_complete_staging() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-installer-{}-{n}", std::process::id()));
        let dest = root.join("assets");
        let staging = root.join("assets.staging");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("old.txt"), "old").unwrap();
        write_valid_assets(&staging);

        install_staged_assets(&staging, &dest).unwrap();

        assert!(dest.join("body.png").exists());
        assert!(dest.join("motion-profile.json").exists());
        assert!(dest.join("manifest.json").exists());
        assert!(!dest.join("old.txt").exists());
        assert!(!staging.exists());
        assert!(backup_directories(&root).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_staging_does_not_replace_existing_assets() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-installer-invalid-{}-{n}",
            std::process::id()
        ));
        let dest = root.join("assets");
        let staging = root.join("assets.staging");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("old.txt"), "old").unwrap();
        write_valid_assets(&staging);
        std::fs::remove_file(staging.join("motion-profile.json")).unwrap();

        assert!(install_staged_assets(&staging, &dest).is_err());

        assert_eq!(
            std::fs::read_to_string(dest.join("old.txt")).unwrap(),
            "old"
        );
        assert!(!dest.join("manifest.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn staging_directory_is_reserved_without_reusing_a_stale_directory() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-installer-staging-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let dest = root.join("assets");

        let stale = staging_directory_for(&dest).unwrap();
        std::fs::write(stale.join("owner.txt"), "stale").unwrap();
        let fresh = staging_directory_for(&dest).unwrap();

        assert_ne!(fresh, stale);
        assert_eq!(fresh.parent(), dest.parent());
        assert_eq!(
            std::fs::read_to_string(stale.join("owner.txt")).unwrap(),
            "stale"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stale_backup_is_preserved_while_assets_are_replaced() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-installer-backup-{}-{n}",
            std::process::id()
        ));
        let dest = root.join("assets");
        let staging = root.join("assets.staging");
        let stale_backup = root.join("assets.backup");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("old.txt"), "old").unwrap();
        std::fs::create_dir_all(&stale_backup).unwrap();
        std::fs::write(stale_backup.join("owner.txt"), "stale").unwrap();
        write_valid_assets(&staging);

        install_staged_assets(&staging, &dest).unwrap();

        assert!(dest.join("manifest.json").exists());
        assert_eq!(
            std::fs::read_to_string(stale_backup.join("owner.txt")).unwrap(),
            "stale"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn second_rename_failure_restores_the_previous_assets() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-installer-rollback-{}-{n}",
            std::process::id()
        ));
        let dest = root.join("assets");
        write_valid_assets(&dest);
        std::fs::write(dest.join("old.txt"), "old").unwrap();

        assert!(install_staged_assets(&dest, &dest).is_err());

        assert_eq!(
            std::fs::read_to_string(dest.join("old.txt")).unwrap(),
            "old"
        );
        crate::runtime_assets::loader::validate_asset_directory(&dest).unwrap();
        assert!(backup_directories(&root).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cleanup_failure_after_commit_returns_success_and_leaves_a_retryable_marker() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-installer-cleanup-{}-{n}",
            std::process::id()
        ));
        let dest = root.join("assets");
        let staging = root.join("assets.staging");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("old.txt"), "old").unwrap();
        write_valid_assets(&staging);

        let result = install_staged_assets_with_cleanup(&staging, &dest, |_| {
            Err("simulated backup cleanup failure".into())
        });

        assert!(
            result.is_ok(),
            "committed install must return Ok, got {result:?}"
        );
        assert!(dest.join("manifest.json").exists());
        assert!(!dest.join("old.txt").exists());
        let pending = backup_directories(&root);
        assert_eq!(pending.len(), 1);
        assert!(pending[0].join(".cleanup-pending").exists());

        let next_staging = root.join("assets.next.staging");
        write_valid_assets(&next_staging);
        install_staged_assets(&next_staging, &dest).unwrap();
        assert!(dest.join("manifest.json").exists());
        assert_eq!(backup_directories(&root), pending);
        let _ = std::fs::remove_dir_all(root);
    }
}
