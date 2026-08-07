use crate::runtime_assets::manifest::{
    parse_manifest, validate_relative_path, RuntimeAssetManifest,
};
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
        let manifest_path = pet_dir.join("assets").join("manifest.json");
        if !manifest_path.exists() {
            result.push(AssetHealth {
                pet_id,
                status: "missing",
                manifest_path: manifest_path.to_string_lossy().to_string(),
            });
            continue;
        }
        let Ok(data) = std::fs::read(&manifest_path) else {
            result.push(AssetHealth {
                pet_id,
                status: "corrupt",
                manifest_path: manifest_path.to_string_lossy().to_string(),
            });
            continue;
        };
        let Ok(json) = String::from_utf8(data) else {
            result.push(AssetHealth {
                pet_id,
                status: "corrupt",
                manifest_path: manifest_path.to_string_lossy().to_string(),
            });
            continue;
        };
        let Ok(manifest) = parse_manifest(&json) else {
            result.push(AssetHealth {
                pet_id,
                status: "corrupt",
                manifest_path: manifest_path.to_string_lossy().to_string(),
            });
            continue;
        };
        let files = match &manifest {
            RuntimeAssetManifest::V1(value) => &value.files,
            RuntimeAssetManifest::V2(value) => &value.files,
        };
        let mut healthy = true;
        for file in files {
            if validate_relative_path(&file.relative_path).is_err() {
                healthy = false;
                break;
            }
            let file_path = pet_dir.join("assets").join(&file.relative_path);
            let Ok(bytes) = std::fs::read(&file_path) else {
                healthy = false;
                break;
            };
            if sha256_hex(&bytes) != file.sha256 {
                healthy = false;
                break;
            }
        }
        result.push(AssetHealth {
            pet_id,
            status: if healthy { "healthy" } else { "corrupt" },
            manifest_path: manifest_path.to_string_lossy().to_string(),
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_assets::importer::import_png_source;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

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
}
