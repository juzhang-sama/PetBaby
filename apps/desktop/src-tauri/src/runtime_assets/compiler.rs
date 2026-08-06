use crate::runtime_assets::manifest::{
    layered_parts, main_part, parse_manifest, ManifestAnimation, ManifestFileEntry,
    ManifestMeshFeatures, RuntimeAssetManifestV1,
};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

static RAW_CUTOUT_COUNTER: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileResult {
    pub manifest_path: String,
    pub degraded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutout_png_b64: Option<String>,
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

fn write_manifest(dest_dir: &Path, manifest: &RuntimeAssetManifestV1) -> Result<(), String> {
    let manifest_path = dest_dir.join("manifest.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let written = std::fs::read(&manifest_path).map_err(|error| error.to_string())?;
    parse_manifest(&String::from_utf8_lossy(&written)).map_err(|error| error.to_string())?;
    Ok(())
}

fn cleanup_job_dir(cutout_path: &Path) {
    if let Some(job_dir) = cutout_path.parent() {
        if job_dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("job-"))
        {
            let _ = std::fs::remove_dir_all(job_dir);
        }
    }
}

/// Cutout a raw generated image (quality-gated) and compile it into runtime
/// assets in one step. Used by the SaaS flow where the raw result is
/// downloaded from our own backend instead of a local generation job.
pub fn compile_from_raw(
    pet_id: &str,
    variant_id: &str,
    raw_bytes: &[u8],
    dest_dir: &Path,
    mesh_features: Option<ManifestMeshFeatures>,
) -> Result<CompileResult, String> {
    if pet_id.is_empty() || variant_id.is_empty() {
        return Err("pet_id and variant_id must not be empty".into());
    }
    let image = image::load_from_memory(raw_bytes)
        .map_err(|error| format!("invalid raw image: {error}"))?;
    let cutout = crate::generation::cutout::remove_background_guarded(&image);
    let n = RAW_CUTOUT_COUNTER.fetch_add(1, Ordering::SeqCst);
    let temp_dir = std::env::temp_dir().join(format!(
        "desktop-pet-raw-cutout-{}-{}-{n}",
        std::process::id(),
        variant_id
    ));
    std::fs::create_dir_all(&temp_dir).map_err(|error| error.to_string())?;
    let cutout_path = temp_dir.join("cutout.png");
    cutout
        .save(&cutout_path)
        .map_err(|error| error.to_string())?;
    let result = compile_single_image(pet_id, variant_id, &cutout_path, dest_dir, mesh_features);
    let _ = std::fs::remove_dir_all(&temp_dir);
    let mut result = result?;
    if let Ok(body) = std::fs::read(dest_dir.join("body.png")) {
        result.cutout_png_b64 = Some(base64::engine::general_purpose::STANDARD.encode(body));
    }
    Ok(result)
}

/// Import an already-cutout PNG (e.g. a built-in adopted pet) directly as
/// runtime assets without re-running the generation cutout.
pub fn compile_png_bytes(
    pet_id: &str,
    variant_id: &str,
    png_bytes: &[u8],
    dest_dir: &Path,
    mesh_features: Option<ManifestMeshFeatures>,
) -> Result<CompileResult, String> {
    if pet_id.is_empty() || variant_id.is_empty() {
        return Err("pet_id and variant_id must not be empty".into());
    }
    let image =
        image::load_from_memory(png_bytes).map_err(|error| format!("invalid png: {error}"))?;
    let degraded = !image.color().has_alpha();
    std::fs::create_dir_all(dest_dir).map_err(|error| error.to_string())?;
    std::fs::write(dest_dir.join("body.png"), png_bytes).map_err(|error| error.to_string())?;

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
            sha256: sha256_hex(png_bytes),
        }],
        animation: ManifestAnimation {
            idle_fps: 12,
            blink_ms_min: 3000,
            blink_ms_max: 8000,
        },
        parts: Some(vec![main_part("body.png")]),
        mesh_features,
    };
    write_manifest(dest_dir, &manifest)?;

    Ok(CompileResult {
        manifest_path: dest_dir.join("manifest.json").to_string_lossy().to_string(),
        degraded,
        cutout_png_b64: None,
    })
}

/// Compile a candidate into runtime assets: body.png + manifest.json.
/// `cutout_path` is the transparent RGBA image; if it is missing or the image
/// is not transparent, the raw image is used and the result is marked degraded.
pub fn compile_single_image(
    pet_id: &str,
    variant_id: &str,
    cutout_path: &Path,
    dest_dir: &Path,
    mesh_features: Option<ManifestMeshFeatures>,
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
        parts: Some(vec![main_part("body.png")]),
        mesh_features,
    };
    write_manifest(dest_dir, &manifest)?;

    // intermediate cutout no longer needed: remove its job directory
    cleanup_job_dir(cutout_path);

    Ok(CompileResult {
        manifest_path: dest_dir.join("manifest.json").to_string_lossy().to_string(),
        degraded,
        cutout_png_b64: None,
    })
}

/// Compile a candidate plus an optional eye-closed edit into layered runtime
/// assets: body.png + eye-open.png + eye-closed.png + a `layered-v1` manifest.
/// When the eye-closed cutout is missing or opaque, falls back to the
/// single-image compile so blinking degrades gracefully.
///
/// RESERVED for future local-edit capability: the wizard no longer
/// auto-generates the eye-closed layer because the current generation
/// platform redraws the whole image instead of editing locally. Keep this
/// path (together with the `generation_jobs.kind` column) for a provider
/// with mask/inpaint support.
pub fn compile_layered(
    pet_id: &str,
    variant_id: &str,
    body_cutout: &Path,
    eye_closed_cutout: Option<&Path>,
    dest_dir: &Path,
) -> Result<CompileResult, String> {
    if pet_id.is_empty() || variant_id.is_empty() {
        return Err("pet_id and variant_id must not be empty".into());
    }
    if !body_cutout.exists() {
        return Err(format!("cutout not found: {}", body_cutout.display()));
    }

    let Some(eye_closed) = eye_closed_cutout else {
        return compile_single_image(pet_id, variant_id, body_cutout, dest_dir, None);
    };
    if !eye_closed.exists() {
        return compile_single_image(pet_id, variant_id, body_cutout, dest_dir, None);
    }
    let eye_closed_image = image::open(eye_closed).map_err(|error| error.to_string())?;
    if !eye_closed_image.color().has_alpha() {
        return compile_single_image(pet_id, variant_id, body_cutout, dest_dir, None);
    }

    std::fs::create_dir_all(dest_dir).map_err(|error| error.to_string())?;
    let body_image = image::open(body_cutout).map_err(|error| error.to_string())?;
    let degraded = !body_image.color().has_alpha();

    let body_bytes = std::fs::read(body_cutout).map_err(|error| error.to_string())?;
    let eye_closed_bytes = std::fs::read(eye_closed).map_err(|error| error.to_string())?;
    // body.png may already exist from a previous single-image compile and the
    // pet window may still hold it open; never self-copy it (Windows would
    // reject copying a file onto itself with a sharing violation)
    if !dest_dir.join("body.png").exists() {
        std::fs::copy(body_cutout, dest_dir.join("body.png")).map_err(|error| error.to_string())?;
    }
    std::fs::copy(body_cutout, dest_dir.join("eye-open.png")).map_err(|error| error.to_string())?;
    std::fs::copy(eye_closed, dest_dir.join("eye-closed.png"))
        .map_err(|error| error.to_string())?;

    let manifest = RuntimeAssetManifestV1 {
        schema_version: 1,
        asset_type: "layered-v1".into(),
        pet_id: pet_id.into(),
        variant_id: variant_id.into(),
        style_id: "signature-cartoon-v1".into(),
        view: "front".into(),
        pose: "sitting".into(),
        files: vec![
            ManifestFileEntry {
                role: "body".into(),
                relative_path: "body.png".into(),
                sha256: sha256_hex(&body_bytes),
            },
            ManifestFileEntry {
                role: "eye-open".into(),
                relative_path: "eye-open.png".into(),
                sha256: sha256_hex(&body_bytes),
            },
            ManifestFileEntry {
                role: "eye-closed".into(),
                relative_path: "eye-closed.png".into(),
                sha256: sha256_hex(&eye_closed_bytes),
            },
        ],
        animation: ManifestAnimation {
            idle_fps: 12,
            blink_ms_min: 3000,
            blink_ms_max: 8000,
        },
        parts: Some(layered_parts()),
        mesh_features: None,
    };
    write_manifest(dest_dir, &manifest)?;

    cleanup_job_dir(body_cutout);
    cleanup_job_dir(eye_closed);

    Ok(CompileResult {
        manifest_path: dest_dir.join("manifest.json").to_string_lossy().to_string(),
        degraded,
        cutout_png_b64: None,
    })
}

/// Attach mesh feature landmarks to an existing compiled asset. The boxes must
/// be normalized to the final body.png (cutout) coordinates.
pub fn set_mesh_features(
    dest_dir: &Path,
    mesh_features: ManifestMeshFeatures,
) -> Result<(), String> {
    let manifest_path = dest_dir.join("manifest.json");
    let json = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("read manifest failed: {error}"))?;
    let mut manifest = parse_manifest(&json)?;
    manifest.mesh_features = Some(mesh_features);
    write_manifest(dest_dir, &manifest)
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

        let result =
            compile_single_image("pet-1", "variant-1", &cutout, &dest, None).unwrap();
        assert!(!result.degraded);
        assert!(dest.join("body.png").exists());
        let manifest = std::fs::read_to_string(dest.join("manifest.json")).unwrap();
        let parsed = parse_manifest(&manifest).unwrap();
        assert_eq!(parsed.pet_id, "pet-1");
        assert_eq!(parsed.files[0].role, "main");
        assert_eq!(parsed.files[0].sha256.len(), 64);
        let parts = parsed
            .parts
            .expect("single-image manifest must declare parts");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].role, "main");
        assert_eq!(parts[0].z_index, 0);
        assert!(parts[0].deformable);
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
        let result =
            compile_single_image("pet-1", "variant-1", &cutout, &dest, None).unwrap();
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
        let result =
            compile_single_image("pet-1", "variant-1", &root.join("nope.png"), &dest, None);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compiles_from_raw_bytes_with_quality_gated_cutout() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("desktop-pet-raw-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut img = image::RgbaImage::new(128, 128);
        for y in 0..128 {
            for x in 0..128 {
                let in_subject = (40..88).contains(&x) && (40..88).contains(&y);
                img.put_pixel(
                    x,
                    y,
                    if in_subject {
                        image::Rgba([120, 90, 60, 255])
                    } else {
                        image::Rgba([255, 255, 255, 255])
                    },
                );
            }
        }
        let mut raw = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut raw), image::ImageFormat::Png)
            .unwrap();
        let dest = root.join("assets");

        let result = compile_from_raw("pet-1", "variant-1", &raw, &dest, None).unwrap();

        assert!(!result.degraded);
        assert!(result.cutout_png_b64.is_some());
        assert!(dest.join("body.png").exists());
        let manifest = std::fs::read_to_string(dest.join("manifest.json")).unwrap();
        let parsed = parse_manifest(&manifest).unwrap();
        assert_eq!(parsed.pet_id, "pet-1");
        assert_eq!(parsed.files[0].role, "main");
        let parts = parsed.parts.expect("parts must be present");
        assert_eq!(parts[0].role, "main");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn set_mesh_features_attaches_landmarks_to_manifest() {
        use crate::runtime_assets::manifest::ManifestFeatureBox;
        use crate::runtime_assets::manifest::ManifestMeshFeatures;

        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-mesh-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut img = image::RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                img.put_pixel(x, y, image::Rgba([200, 120, 80, 255]));
            }
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let dest = root.join("assets");
        compile_png_bytes("pet-1", "variant-1", &bytes, &dest, None).unwrap();

        let box_ = ManifestFeatureBox {
            x: 0.2,
            y: 0.3,
            width: 0.1,
            height: 0.08,
        };
        let features = ManifestMeshFeatures {
            left_eye: box_.clone(),
            right_eye: box_.clone(),
            left_ear: box_.clone(),
            right_ear: box_.clone(),
            tail: box_.clone(),
        };
        set_mesh_features(&dest, features).unwrap();

        let manifest = std::fs::read_to_string(dest.join("manifest.json")).unwrap();
        let parsed = parse_manifest(&manifest).unwrap();
        let mesh = parsed.mesh_features.expect("mesh features must be present");
        assert_eq!(mesh.left_eye.x, 0.2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compiles_png_bytes_into_single_image_assets() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-pngbytes-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut img = image::RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                img.put_pixel(x, y, image::Rgba([200, 120, 80, 255]));
            }
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let dest = root.join("assets");

        let result = compile_png_bytes("pet-1", "builtin-1", &bytes, &dest, None).unwrap();

        assert!(!result.degraded);
        assert!(dest.join("body.png").exists());
        let manifest = std::fs::read_to_string(dest.join("manifest.json")).unwrap();
        let parsed = parse_manifest(&manifest).unwrap();
        assert_eq!(parsed.pet_id, "pet-1");
        assert_eq!(parsed.variant_id, "builtin-1");
        assert_eq!(parsed.files[0].role, "main");
        let parts = parsed.parts.expect("parts must be present");
        assert_eq!(parts[0].role, "main");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compiles_layered_assets_with_eye_closed() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-layered-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let body = root.join("body-cutout.png");
        let eye_closed = root.join("eye-closed-cutout.png");
        write_rgba_png(&body);
        write_rgba_png(&eye_closed);
        let dest = root.join("assets");

        let result =
            compile_layered("pet-1", "variant-1", &body, Some(&eye_closed), &dest).unwrap();
        assert!(!result.degraded);
        for file in ["body.png", "eye-open.png", "eye-closed.png"] {
            assert!(dest.join(file).exists(), "{file} must exist");
        }
        let manifest = std::fs::read_to_string(dest.join("manifest.json")).unwrap();
        let parsed = parse_manifest(&manifest).unwrap();
        assert_eq!(parsed.asset_type, "layered-v1");
        let roles: Vec<&str> = parsed.files.iter().map(|f| f.role.as_str()).collect();
        assert_eq!(roles, vec!["body", "eye-open", "eye-closed"]);
        let parts = parsed.parts.expect("layered manifest must declare parts");
        let part_roles: Vec<&str> = parts.iter().map(|p| p.role.as_str()).collect();
        assert_eq!(part_roles, vec!["body", "eye-open", "eye-closed"]);
        assert!(parts[0].deformable);
        assert!(!parts[1].deformable);
        assert!(!parts[2].deformable);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn layered_without_eye_closed_falls_back_to_single_image() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-layered-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let body = root.join("body-cutout.png");
        write_rgba_png(&body);
        let dest = root.join("assets");

        let result = compile_layered("pet-1", "variant-1", &body, None, &dest).unwrap();
        assert!(!result.degraded);
        let manifest = std::fs::read_to_string(dest.join("manifest.json")).unwrap();
        let parsed = parse_manifest(&manifest).unwrap();
        assert_eq!(parsed.asset_type, "single-image");
        assert_eq!(parsed.files.len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn layered_upgrade_keeps_existing_body_png() {
        // the pet already has a compiled single-image body.png; the layered
        // upgrade must not self-copy body.png (Windows sharing violation)
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-upgrade-{}-{n}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let body = root.join("body-cutout.png");
        let eye_closed = root.join("eye-closed-cutout.png");
        write_rgba_png(&body);
        write_rgba_png(&eye_closed);
        let dest = root.join("assets");
        std::fs::copy(&body, dest.join("body.png")).unwrap();

        let result =
            compile_layered("pet-1", "variant-1", &body, Some(&eye_closed), &dest).unwrap();
        assert!(!result.degraded);
        assert!(dest.join("body.png").exists());
        assert!(dest.join("eye-open.png").exists());
        assert!(dest.join("eye-closed.png").exists());
        let manifest = std::fs::read_to_string(dest.join("manifest.json")).unwrap();
        let parsed = parse_manifest(&manifest).unwrap();
        assert_eq!(parsed.asset_type, "layered-v1");
        let _ = std::fs::remove_dir_all(root);
    }
}
