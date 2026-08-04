use crate::runtime_assets::manifest::{
    parse_manifest, ManifestAnimation, ManifestFileEntry, RuntimeAssetManifestV1,
};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileResult {
    pub manifest_path: String,
    pub degraded: bool,
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

/// Compile a candidate into runtime assets: body.png + manifest.json.
/// `cutout_path` is the transparent RGBA image; if it is missing or the image
/// is not transparent, the raw image is used and the result is marked degraded.
pub fn compile_single_image(
    pet_id: &str,
    variant_id: &str,
    cutout_path: &Path,
    dest_dir: &Path,
) -> Result<CompileResult, String> {
    if pet_id.is_empty() || variant_id.is_empty() {
        return Err("pet_id and variant_id must not be empty".into());
    }
    std::fs::create_dir_all(dest_dir).map_err(|error| error.to_string())?;

    let (body_path, degraded) = if cutout_path.exists() {
        let image = image::open(cutout_path).map_err(|error| error.to_string())?;
        let is_transparent = image.color().has_alpha();
        if is_transparent {
            (cutout_path.to_path_buf(), false)
        } else {
            (cutout_path.to_path_buf(), true)
        }
    } else {
        return Err(format!("cutout not found: {}", cutout_path.display()));
    };

    let body_bytes = std::fs::read(&body_path).map_err(|error| error.to_string())?;
    let dest_body = dest_dir.join("body.png");
    std::fs::copy(&body_path, &dest_body).map_err(|error| error.to_string())?;

    let manifest = RuntimeAssetManifestV1 {
        schema_version: 1,
        asset_type: "single-image".into(),
        pet_id: pet_id.into(),
        variant_id: variant_id.into(),
        style_id: "signature-cartoon-v1".into(),
        view: "front".into(),
        pose: "sitting".into(),
        files: vec![ManifestFileEntry {
            role: "main".into(),
            relative_path: "body.png".into(),
            sha256: sha256_hex(&body_bytes),
        }],
        animation: ManifestAnimation {
            idle_fps: 12,
            blink_ms_min: 3000,
            blink_ms_max: 8000,
        },
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

    Ok(CompileResult {
        manifest_path: manifest_path.to_string_lossy().to_string(),
        degraded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn write_rgba_png(path: &Path) {
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                img.put_pixel(x, y, image::Rgba([72, 94, 86, 255]));
            }
        }
        img.save(path).unwrap();
    }

    #[test]
    fn compiles_transparent_cutout_to_assets() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-compile-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let cutout = root.join("cutout.png");
        write_rgba_png(&cutout);
        let dest = root.join("assets");

        let result = compile_single_image("pet-1", "variant-1", &cutout, &dest).unwrap();
        assert!(!result.degraded);
        assert!(dest.join("body.png").exists());
        let manifest = std::fs::read_to_string(dest.join("manifest.json")).unwrap();
        let parsed = parse_manifest(&manifest).unwrap();
        assert_eq!(parsed.pet_id, "pet-1");
        assert_eq!(parsed.files[0].role, "main");
        assert_eq!(parsed.files[0].sha256.len(), 64);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn marks_opaque_input_as_degraded() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-compile-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        // RGB (no alpha) input
        let cutout = root.join("opaque.png");
        let mut img = image::RgbImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                img.put_pixel(x, y, image::Rgb([226, 226, 226]));
            }
        }
        img.save(&cutout).unwrap();
        let dest = root.join("assets");
        let result = compile_single_image("pet-1", "variant-1", &cutout, &dest).unwrap();
        assert!(result.degraded);
        assert!(dest.join("body.png").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_cutout_fails() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-compile-{}-{n}", std::process::id()));
        let dest = root.join("assets");
        let result = compile_single_image("pet-1", "variant-1", &root.join("nope.png"), &dest);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
