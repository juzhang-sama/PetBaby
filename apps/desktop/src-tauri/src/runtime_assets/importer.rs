use crate::runtime_assets::manifest::{
    main_part, parse_manifest, ManifestAnimation, ManifestFileEntry, RuntimeAssetManifestV1,
};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const MAX_DIMENSION: u32 = 4096;

const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedAsset {
    pub manifest_path: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
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

/// Parse the PNG IHDR chunk to extract width/height without decoding pixels.
fn png_dimensions(data: &[u8]) -> Result<(u32, u32), String> {
    if data.len() < 8 + 8 + 8 {
        return Err("file too small to be a PNG".into());
    }
    if data[..8] != PNG_SIGNATURE {
        return Err("not a PNG file (bad signature)".into());
    }
    // IHDR chunk: length(4) + "IHDR"(4) + width(4) + height(4)
    let ihdr_type = &data[12..16];
    if ihdr_type != b"IHDR" {
        return Err("PNG is missing IHDR chunk".into());
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    if width == 0 || height == 0 {
        return Err("PNG dimensions must be positive".into());
    }
    Ok((width, height))
}

pub fn import_png_source(
    pet_id: &str,
    source_path: &Path,
    dest_dir: &Path,
) -> Result<ImportedAsset, String> {
    if pet_id.is_empty() {
        return Err("pet_id must not be empty".into());
    }
    let data = std::fs::read(source_path).map_err(|error| format!("read failed: {error}"))?;
    let (width, height) = png_dimensions(&data)?;
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(format!(
            "image too large: {width}x{height} exceeds {MAX_DIMENSION}"
        ));
    }

    std::fs::create_dir_all(dest_dir).map_err(|error| error.to_string())?;
    let file_name = source_path
        .file_name()
        .ok_or("source path has no file name")?
        .to_string_lossy()
        .to_string();
    let dest_path = dest_dir.join(&file_name);
    std::fs::copy(source_path, &dest_path).map_err(|error| error.to_string())?;

    let sha256 = sha256_hex(&data);
    let manifest = RuntimeAssetManifestV1 {
        schema_version: 1,
        asset_type: "single-image".into(),
        pet_id: pet_id.into(),
        variant_id: format!("variant-{pet_id}"),
        style_id: "signature-cartoon-v1".into(),
        view: "front".into(),
        pose: "sitting".into(),
        files: vec![ManifestFileEntry {
            role: "main".into(),
            relative_path: file_name.clone(),
            sha256: sha256.clone(),
        }],
        animation: ManifestAnimation {
            idle_fps: 12,
            blink_ms_min: 3000,
            blink_ms_max: 8000,
        },
        parts: Some(vec![main_part(&file_name)]),
        mesh_features: None,
    };
    let manifest_path = dest_dir.join("manifest.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    // validate the round trip
    let written = std::fs::read(&manifest_path).map_err(|error| error.to_string())?;
    parse_manifest(&String::from_utf8_lossy(&written)).map_err(|error| error.to_string())?;

    Ok(ImportedAsset {
        manifest_path: manifest_path.to_string_lossy().to_string(),
        sha256,
        width,
        height,
        bytes: data.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn write_png(path: &Path, width: u32, height: u32) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&PNG_SIGNATURE);
        bytes.extend_from_slice(&13u32.to_be_bytes()); // IHDR length
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]); // bit depth, color type, etc.
        bytes.extend_from_slice(&[0; 4]); // fake CRC
        bytes.extend_from_slice(&0u32.to_be_bytes()); // IEND length
        bytes.extend_from_slice(b"IEND");
        bytes.extend_from_slice(&[0; 4]); // fake CRC
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn rejects_non_png() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("desktop-pet-imp-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let not_png = root.join("fake.png");
        std::fs::write(
            &not_png,
            b"this is definitely not a png file content at all",
        )
        .unwrap();
        let result = import_png_source("pet-x", &not_png, &root.join("out"));
        assert!(result.unwrap_err().contains("not a PNG"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_oversized_dimensions() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("desktop-pet-imp-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let big = root.join("big.png");
        write_png(&big, MAX_DIMENSION + 1, 100);
        let result = import_png_source("pet-x", &big, &root.join("out"));
        assert!(result.unwrap_err().contains("too large"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn imports_valid_png_with_manifest_and_hash() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("desktop-pet-imp-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let png = root.join("pet.png");
        write_png(&png, 512, 512);
        let out = root.join("out");
        let imported = import_png_source("pet-x", &png, &out).unwrap();
        assert_eq!(imported.width, 512);
        assert_eq!(imported.height, 512);
        assert_eq!(imported.sha256.len(), 64);

        let manifest = std::fs::read(out.join("manifest.json")).unwrap();
        let parsed = parse_manifest(&String::from_utf8_lossy(&manifest)).unwrap();
        assert_eq!(parsed.pet_id, "pet-x");
        assert_eq!(parsed.files[0].role, "main");
        assert_eq!(parsed.files[0].sha256, imported.sha256);
        assert_eq!(parsed.animation.idle_fps, 12);
        let parts = parsed.parts.expect("imported manifest must declare parts");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].role, "main");
        assert_eq!(parts[0].pivot.x, 0.5);
        assert_eq!(parts[0].anchor.y, 1.0);
        let _ = std::fs::remove_dir_all(root);
    }
}
