use crate::runtime_assets::manifest::{
    manifest_files, parse_manifest, validate_relative_path, RuntimeAssetManifest,
};
use crate::runtime_assets::motion_profile::parse_motion_profile;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetHealth {
    pub pet_id: String,
    pub status: &'static str,
    pub manifest_path: String,
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AssetReadError {
    Missing,
    Corrupt,
}

fn validate_asset_manifest(path: &Path) -> Result<(), AssetReadError> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AssetReadError::Missing)
        }
        Err(_) => return Err(AssetReadError::Corrupt),
    };
    let json = String::from_utf8(data).map_err(|_| AssetReadError::Corrupt)?;
    let manifest = parse_manifest(&json).map_err(|_| AssetReadError::Corrupt)?;
    let files = manifest_files(&manifest);
    let assets_dir = path.parent().ok_or(AssetReadError::Corrupt)?;
    for file in files {
        validate_relative_path(&file.relative_path).map_err(|_| AssetReadError::Corrupt)?;
        let bytes = std::fs::read(assets_dir.join(&file.relative_path))
            .map_err(|_| AssetReadError::Corrupt)?;
        if sha256_hex(&bytes) != file.sha256 {
            return Err(AssetReadError::Corrupt);
        }
    }
    if let RuntimeAssetManifest::V3(value) = manifest {
        let profile = std::fs::read_to_string(assets_dir.join(value.motion_profile))
            .map_err(|_| AssetReadError::Corrupt)?;
        parse_motion_profile(&profile).map_err(|_| AssetReadError::Corrupt)?;
    }
    Ok(())
}

pub fn inspect_pet_asset(pets_dir: &Path, pet_id: &str) -> AssetHealth {
    let manifest_path = pets_dir.join(pet_id).join("assets").join("manifest.json");
    let status = match validate_asset_manifest(&manifest_path) {
        Ok(()) => "healthy",
        Err(AssetReadError::Missing) => "missing",
        Err(AssetReadError::Corrupt) => "corrupt",
    };
    AssetHealth {
        pet_id: pet_id.to_owned(),
        status,
        manifest_path: manifest_path.to_string_lossy().into_owned(),
    }
}

pub fn scan_assets(pets_dir: &Path) -> Vec<AssetHealth> {
    let mut result = Vec::new();
    let Ok(entries) = std::fs::read_dir(pets_dir) else {
        return result;
    };
    for entry in entries.flatten() {
        let pet_dir = entry.path();
        if !pet_dir.is_dir() {
            continue;
        }
        let pet_id = entry.file_name().to_string_lossy().to_string();
        result.push(inspect_pet_asset(pets_dir, &pet_id));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_assets::importer::import_png_source;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn setup_v3() -> (std::path::PathBuf, std::path::PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-v3-loader-{}-{n}", std::process::id()));
        let pets_dir = root.join("pets");
        let assets = pets_dir.join("pet-a").join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        let body = b"body-bytes";
        let profile = serde_json::json!({
            "profileVersion": 1,
            "engineProfile": "life-v1",
            "alphaBounds": { "left": 0.1, "top": 0.05, "right": 0.9, "bottom": 0.96 },
            "breathZone": { "left": 0.2, "top": 0.5, "right": 0.8, "bottom": 0.84 },
            "swayPivot": { "x": 0.5, "y": 0.72 }
        })
        .to_string();
        std::fs::write(assets.join("body.png"), body).unwrap();
        std::fs::write(assets.join("motion-profile.json"), profile.as_bytes()).unwrap();
        let manifest = serde_json::json!({
            "schemaVersion": 3,
            "renderer": "animated-image-v1",
            "petId": "pet-a",
            "variantId": "variant-a",
            "image": "body.png",
            "motionProfile": "motion-profile.json",
            "files": [
                { "role": "main", "relativePath": "body.png", "sha256": sha256_hex(body) },
                { "role": "motion-profile", "relativePath": "motion-profile.json", "sha256": sha256_hex(profile.as_bytes()) }
            ]
        });
        std::fs::write(
            assets.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        (pets_dir, root)
    }

    fn write_png(path: &Path, width: u32, height: u32) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(b"IEND");
        bytes.extend_from_slice(&[0; 4]);
        std::fs::write(path, bytes).unwrap();
    }

    fn setup() -> (std::path::PathBuf, String) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-load-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let png = root.join("pet.png");
        write_png(&png, 64, 64);
        let pets_dir = root.join("pets");
        import_png_source("pet-a", &png, &pets_dir.join("pet-a").join("assets")).unwrap();
        (pets_dir, root.to_string_lossy().to_string())
    }

    #[test]
    fn reports_healthy_for_intact_asset() {
        let (pets_dir, root) = setup();
        let health = scan_assets(&pets_dir);
        assert_eq!(health.len(), 1);
        assert_eq!(health[0].pet_id, "pet-a");
        assert_eq!(health[0].status, "healthy");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reports_missing_when_manifest_absent() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-load-{}-{n}", std::process::id()));
        let pets_dir = root.join("pets");
        std::fs::create_dir_all(pets_dir.join("pet-x")).unwrap();
        let health = scan_assets(&pets_dir);
        assert_eq!(health.len(), 1);
        assert_eq!(health[0].status, "missing");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reports_corrupt_when_hash_mismatches() {
        let (pets_dir, root) = setup();
        // corrupt the copied image after import
        let img = pets_dir.join("pet-a").join("assets").join("pet.png");
        std::fs::write(&img, b"corrupted content").unwrap();
        let health = scan_assets(&pets_dir);
        assert_eq!(health[0].status, "corrupt");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reports_corrupt_when_the_motion_profile_hash_mismatches() {
        let (pets_dir, root) = setup_v3();
        std::fs::write(pets_dir.join("pet-a/assets/motion-profile.json"), b"{}").unwrap();
        assert_eq!(inspect_pet_asset(&pets_dir, "pet-a").status, "corrupt");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reports_corrupt_when_the_motion_profile_content_is_invalid() {
        let (pets_dir, root) = setup_v3();
        let assets = pets_dir.join("pet-a/assets");
        std::fs::write(assets.join("motion-profile.json"), b"{}").unwrap();
        let manifest_path = assets.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["files"][1]["sha256"] = sha256_hex(b"{}").into();
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert_eq!(inspect_pet_asset(&pets_dir, "pet-a").status, "corrupt");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reports_corrupt_when_the_motion_profile_is_missing() {
        let (pets_dir, root) = setup_v3();
        std::fs::remove_file(pets_dir.join("pet-a/assets/motion-profile.json")).unwrap();
        assert_eq!(inspect_pet_asset(&pets_dir, "pet-a").status, "corrupt");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn inspects_one_pet_without_scanning_siblings() {
        let (pets_dir, root) = setup();
        std::fs::create_dir_all(pets_dir.join("unrelated")).unwrap();
        let health = inspect_pet_asset(&pets_dir, "pet-a");
        assert_eq!(health.pet_id, "pet-a");
        assert_eq!(health.status, "healthy");
        let missing = inspect_pet_asset(&pets_dir, "pet-missing");
        assert_eq!(missing.status, "missing");
        let _ = std::fs::remove_dir_all(root);
    }
}
