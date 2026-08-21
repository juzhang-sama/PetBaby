use crate::runtime_assets::{compiler, installer, loader, manifest, motion_profile};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    Migrated,
    AlreadyCurrent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationFailure {
    pub pet_id: String,
    pub error: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub migrated: usize,
    pub already_current: usize,
    pub failures: Vec<MigrationFailure>,
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub fn migrate_v1_pet_assets(assets_dir: &Path) -> Result<MigrationOutcome, String> {
    let manifest_path = assets_dir.join("manifest.json");
    let manifest_json = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("read asset manifest: {error}"))?;
    let parsed = manifest::parse_manifest(&manifest_json)?;
    loader::validate_asset_directory(assets_dir)?;

    let manifest = match parsed {
        manifest::RuntimeAssetManifest::V1(manifest) => manifest,
        manifest::RuntimeAssetManifest::V2(_)
        | manifest::RuntimeAssetManifest::V3(_)
        | manifest::RuntimeAssetManifest::V4(_)
        | manifest::RuntimeAssetManifest::V5(_) => return Ok(MigrationOutcome::AlreadyCurrent),
    };
    let image = manifest
        .files
        .iter()
        .find(|file| file.role == "main")
        .or_else(|| manifest.files.iter().find(|file| file.role == "body"))
        .ok_or("v1 manifest does not declare a main/body image")?;
    let image_path = assets_dir.join(&image.relative_path);
    let image_bytes = std::fs::read(&image_path)
        .map_err(|error| format!("read v1 image {}: {error}", image.relative_path))?;
    let rgba = image::load_from_memory(&image_bytes)
        .map_err(|error| format!("decode v1 image {}: {error}", image.relative_path))?
        .to_rgba8();
    let profile = motion_profile::generate_motion_profile(&rgba)?;

    let temporary = installer::staging_directory_for(assets_dir)?;
    let temporary = TemporaryDirectory::new(temporary);
    let profile_path = temporary.path.join("motion-profile.json");
    motion_profile::write_motion_profile_atomic(&profile_path, &profile)?;
    compiler::compile_animated_image(
        &manifest.pet_id,
        &manifest.variant_id,
        &image_path,
        &profile_path,
        assets_dir,
    )?;
    Ok(MigrationOutcome::Migrated)
}

pub fn migrate_all_v1_assets(pets_dir: &Path) -> MigrationReport {
    let entries = match std::fs::read_dir(pets_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return MigrationReport::default()
        }
        Err(error) => {
            return MigrationReport {
                failures: vec![MigrationFailure {
                    pet_id: "<pets-dir>".into(),
                    error: format!("scan {}: {error}", pets_dir.display()),
                }],
                ..MigrationReport::default()
            }
        }
    };
    let mut report = MigrationReport::default();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report.failures.push(MigrationFailure {
                    pet_id: "<unknown>".into(),
                    error: format!("read pet directory entry: {error}"),
                });
                continue;
            }
        };
        let pet_dir = entry.path();
        if !pet_dir.is_dir() {
            continue;
        }
        let pet_id = entry.file_name().to_string_lossy().into_owned();
        match migrate_v1_pet_assets(&pet_dir.join("assets")) {
            Ok(MigrationOutcome::Migrated) => report.migrated += 1,
            Ok(MigrationOutcome::AlreadyCurrent) => report.already_current += 1,
            Err(error) => report.failures.push(MigrationFailure { pet_id, error }),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_assets::manifest::RuntimeAssetManifest;
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct MigrationFixture {
        root: std::path::PathBuf,
        assets: std::path::PathBuf,
        image: std::path::PathBuf,
    }

    impl Drop for MigrationFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn write_source(path: &std::path::Path) {
        let mut image = image::RgbaImage::new(64, 64);
        for y in 4..60 {
            for x in 8..56 {
                image.put_pixel(x, y, image::Rgba([80, 90, 100, 255]));
            }
        }
        image.save(path).unwrap();
    }

    fn v1_asset_fixture() -> MigrationFixture {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-v1-migration-{}-{n}",
            std::process::id()
        ));
        let source = root.join("source.png");
        let assets = root.join("assets");
        std::fs::create_dir_all(&root).unwrap();
        write_source(&source);
        crate::runtime_assets::compiler::compile_single_image(
            "pet-a",
            "variant-a",
            &source,
            &assets,
        )
        .unwrap();

        let image = assets.join("portrait.png");
        std::fs::rename(assets.join("body.png"), &image).unwrap();
        let manifest_path = assets.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["files"][0]["relativePath"] = "portrait.png".into();
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        MigrationFixture {
            root,
            assets,
            image,
        }
    }

    fn read_manifest(assets: &std::path::Path) -> RuntimeAssetManifest {
        let json = std::fs::read_to_string(assets.join("manifest.json")).unwrap();
        crate::runtime_assets::manifest::parse_manifest(&json).unwrap()
    }

    #[test]
    fn migrates_a_v1_asset_to_v3_and_is_idempotent() {
        let fixture = v1_asset_fixture();
        assert_eq!(
            migrate_v1_pet_assets(&fixture.assets).unwrap(),
            MigrationOutcome::Migrated
        );
        let RuntimeAssetManifest::V3(manifest) = read_manifest(&fixture.assets) else {
            panic!("expected v3 manifest")
        };
        assert_eq!(manifest.pet_id, "pet-a");
        assert_eq!(manifest.variant_id, "variant-a");
        assert_eq!(
            migrate_v1_pet_assets(&fixture.assets).unwrap(),
            MigrationOutcome::AlreadyCurrent
        );
    }

    #[test]
    fn invalid_png_preserves_the_original_manifest_and_image() {
        let fixture = v1_asset_fixture();
        let invalid_png = b"invalid png";
        std::fs::write(&fixture.image, invalid_png).unwrap();
        let manifest_path = fixture.assets.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["files"][0]["sha256"] = sha256_hex(invalid_png).into();
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let before_manifest = std::fs::read(&manifest_path).unwrap();
        let before_image = std::fs::read(&fixture.image).unwrap();

        assert!(migrate_v1_pet_assets(&fixture.assets).is_err());
        assert_eq!(std::fs::read(&manifest_path).unwrap(), before_manifest);
        assert_eq!(std::fs::read(&fixture.image).unwrap(), before_image);
    }

    #[test]
    fn hash_mismatch_preserves_the_original_manifest_and_image() {
        let fixture = v1_asset_fixture();
        std::fs::write(&fixture.image, b"changed after manifest was written").unwrap();
        let manifest_path = fixture.assets.join("manifest.json");
        let before_manifest = std::fs::read(&manifest_path).unwrap();
        let before_image = std::fs::read(&fixture.image).unwrap();

        assert!(migrate_v1_pet_assets(&fixture.assets).is_err());
        assert_eq!(std::fs::read(&manifest_path).unwrap(), before_manifest);
        assert_eq!(std::fs::read(&fixture.image).unwrap(), before_image);
    }

    #[test]
    fn intact_v2_assets_are_already_current() {
        let fixture = v1_asset_fixture();
        std::fs::remove_dir_all(&fixture.assets).unwrap();
        std::fs::create_dir_all(&fixture.assets).unwrap();
        let model = b"model";
        let preview = b"preview";
        std::fs::write(fixture.assets.join("model.model3.json"), model).unwrap();
        std::fs::write(fixture.assets.join("preview.png"), preview).unwrap();
        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "renderer": "live2d-v1",
            "petId": "pet-a",
            "variantId": "variant-a",
            "modelEntry": "model.model3.json",
            "previewImage": "preview.png",
            "files": [
                { "role": "model", "relativePath": "model.model3.json", "sha256": sha256_hex(model) },
                { "role": "preview", "relativePath": "preview.png", "sha256": sha256_hex(preview) }
            ],
            "semantics": { "motions": {}, "expressions": {}, "hitAreas": {}, "parameters": {} },
            "license": {
                "id": "test", "author": "test", "source": "test",
                "commercialUse": true, "redistributable": false
            }
        });
        std::fs::write(
            fixture.assets.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        assert_eq!(
            migrate_v1_pet_assets(&fixture.assets).unwrap(),
            MigrationOutcome::AlreadyCurrent
        );
    }

    #[test]
    fn missing_manifest_is_an_error_without_creating_assets() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let assets =
            std::env::temp_dir().join(format!("desktop-pet-v1-missing-{}-{n}", std::process::id()));
        assert!(migrate_v1_pet_assets(&assets).is_err());
        assert!(!assets.exists());
    }

    #[test]
    fn migrate_all_continues_after_one_pet_fails() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-v1-all-{}-{n}", std::process::id()));
        let pets_dir = root.join("pets");
        std::fs::create_dir_all(&root).unwrap();
        for pet_id in ["pet-good", "pet-bad"] {
            let source = root.join(format!("{pet_id}.png"));
            write_source(&source);
            crate::runtime_assets::compiler::compile_single_image(
                pet_id,
                "variant-a",
                &source,
                &pets_dir.join(pet_id).join("assets"),
            )
            .unwrap();
        }
        std::fs::write(pets_dir.join("pet-bad/assets/body.png"), b"corrupt").unwrap();

        let report = migrate_all_v1_assets(&pets_dir);

        assert_eq!(report.migrated, 1);
        assert_eq!(report.already_current, 0);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].pet_id, "pet-bad");
        assert!(matches!(
            read_manifest(&pets_dir.join("pet-good/assets")),
            RuntimeAssetManifest::V3(_)
        ));
        assert!(matches!(
            read_manifest(&pets_dir.join("pet-bad/assets")),
            RuntimeAssetManifest::V1(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn migrate_all_does_not_create_a_missing_pets_directory() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pets_dir = std::env::temp_dir().join(format!(
            "desktop-pet-v1-all-missing-{}-{n}",
            std::process::id()
        ));

        let report = migrate_all_v1_assets(&pets_dir);

        assert_eq!(report.migrated, 0);
        assert_eq!(report.already_current, 0);
        assert!(report.failures.is_empty());
        assert!(!pets_dir.exists());
    }
}
