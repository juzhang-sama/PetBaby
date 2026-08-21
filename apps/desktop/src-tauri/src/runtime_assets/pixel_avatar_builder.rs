use crate::creation::photo_avatar::domain::{PixelAppearanceProfileV1, PixelAvatarAudit};
use crate::runtime_assets::installer::{install_staged_assets, staging_directory_for};
use crate::runtime_assets::loader::validate_asset_directory;
use crate::runtime_assets::manifest::{parse_manifest, RuntimeAssetManifest};
use crate::runtime_assets::pixel_png::inspect_rgba_png;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct BuildPixelAvatarRequest {
    pub session_id: String,
    pub revision: u32,
    pub attempt: u8,
    pub pet_id: String,
    pub variant_id: String,
    pub profile: PixelAppearanceProfileV1,
    pub image_png: Vec<u8>,
    pub image_sha256: String,
    pub audit: PixelAvatarAudit,
}

#[derive(Debug, Clone)]
pub struct BuiltPixelAvatarPackage {
    pub preview_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
}

#[derive(Debug, Clone)]
pub struct PixelAvatarBuilder {
    preview_root: PathBuf,
}

impl PixelAvatarBuilder {
    pub fn new(preview_root: &Path) -> Self {
        Self {
            preview_root: preview_root.to_path_buf(),
        }
    }

    pub fn build_preview(
        &self,
        request: BuildPixelAvatarRequest,
    ) -> Result<BuiltPixelAvatarPackage, String> {
        validate_id(&request.session_id, "sessionId")?;
        validate_id(&request.pet_id, "petId")?;
        validate_id(&request.variant_id, "variantId")?;
        if sha256_hex(&request.image_png) != request.image_sha256 {
            return Err("pixel avatar hash mismatch".into());
        }
        let inspection = inspect_rgba_png(&request.image_png)?;
        let identity_profile_sha256 = profile_sha256(&request.profile)?;
        request.audit.bind_to(
            &request.session_id,
            request.revision,
            request.attempt,
            &request.image_sha256,
            &inspection.alpha_report,
        )?;
        if request.audit.identity_profile_sha256() != identity_profile_sha256
            || request.audit.width() != inspection.width
            || request.audit.height() != inspection.height
            || request.audit.style_profile_id() != request.profile.style_profile_id.as_str()
        {
            return Err("pixel avatar audit profile or dimensions mismatch".into());
        }
        if request.audit.visible_color_count().is_some_and(|count| {
            count != inspection.visible_color_count || inspection.visible_color_count > 24
        }) {
            return Err("pixel avatar visible color count does not match audit".into());
        }
        let preview_dir = self.preview_directory(&request.session_id, request.revision)?;
        let staging = staging_directory_for(&preview_dir)?;
        let result = write_preview(
            &staging,
            &request,
            &inspection.alpha_report,
            inspection.width,
            inspection.height,
        );
        if let Err(error) = result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
        validate_asset_directory(&staging)?;
        install_staged_assets(&staging, &preview_dir)?;
        self.validate_preview(&request.session_id, request.revision)?;
        let manifest_path = preview_dir.join("manifest.json");
        let manifest_bytes = std::fs::read(&manifest_path).map_err(|error| error.to_string())?;
        Ok(BuiltPixelAvatarPackage {
            preview_dir,
            manifest_path,
            manifest_sha256: sha256_hex(&manifest_bytes),
        })
    }

    pub fn validate_preview(&self, session_id: &str, revision: u32) -> Result<(), String> {
        let preview_dir = self.preview_directory(session_id, revision)?;
        validate_asset_directory(&preview_dir)?;
        let manifest_bytes =
            std::fs::read(preview_dir.join("manifest.json")).map_err(|error| error.to_string())?;
        let manifest_text = std::str::from_utf8(&manifest_bytes)
            .map_err(|_| "pixel avatar manifest must be UTF-8")?;
        let RuntimeAssetManifest::V3(manifest) = parse_manifest(manifest_text)? else {
            return Err("pixel avatar preview must use manifest schema v3".into());
        };
        if manifest.renderer != "animated-image-v1" {
            return Err("pixel avatar renderer must be animated-image-v1".into());
        }
        let image_entry = manifest
            .files
            .iter()
            .find(|entry| entry.role == "main")
            .ok_or("pixel avatar main image is missing")?;
        let png = std::fs::read(preview_dir.join(&image_entry.relative_path))
            .map_err(|error| error.to_string())?;
        if sha256_hex(&png) != image_entry.sha256 {
            return Err("pixel avatar preview hash mismatch".into());
        }
        inspect_rgba_png(&png).map(|_| ())
    }

    pub fn install_preview(
        &self,
        session_id: &str,
        revision: u32,
        destination: &Path,
    ) -> Result<(), String> {
        self.validate_preview(session_id, revision)?;
        let source = self.preview_directory(session_id, revision)?;
        let staging = staging_directory_for(destination)?;
        copy_directory(&source, &staging)?;
        install_staged_assets(&staging, destination)
    }

    fn preview_directory(&self, session_id: &str, revision: u32) -> Result<PathBuf, String> {
        validate_id(session_id, "sessionId")?;
        Ok(self
            .preview_root
            .join(session_id)
            .join(revision.to_string()))
    }
}

fn write_preview(
    staging: &Path,
    request: &BuildPixelAvatarRequest,
    alpha: &crate::creation::photo_avatar::domain::PixelAlphaReportV1,
    width: u32,
    height: u32,
) -> Result<(), String> {
    std::fs::create_dir_all(staging).map_err(|error| error.to_string())?;
    std::fs::write(staging.join("body.png"), &request.image_png)
        .map_err(|error| error.to_string())?;
    let bounds = [
        f64::from(alpha.bounds_left) / f64::from(width),
        f64::from(alpha.bounds_top) / f64::from(height),
        f64::from(alpha.bounds_right) / f64::from(width),
        f64::from(alpha.bounds_bottom) / f64::from(height),
    ];
    let face_safety = bounds[1] + (bounds[3] - bounds[1]) * 0.4;
    let motion = json!({
        "profileVersion": 1,
        "engineProfile": "life-v1",
        "alphaBounds": {"left": bounds[0], "top": bounds[1], "right": bounds[2], "bottom": bounds[3]},
        "breathZone": {"left": bounds[0], "top": face_safety, "right": bounds[2], "bottom": bounds[3]},
        "swayPivot": {"x": (bounds[0] + bounds[2]) / 2.0, "y": face_safety}
    });
    let motion_bytes = serde_json::to_vec_pretty(&motion).map_err(|error| error.to_string())?;
    std::fs::write(staging.join("motion-profile.json"), &motion_bytes)
        .map_err(|error| error.to_string())?;
    let manifest = json!({
        "schemaVersion": 3,
        "renderer": "animated-image-v1",
        "petId": request.pet_id,
        "variantId": request.variant_id,
        "image": "body.png",
        "motionProfile": "motion-profile.json",
        "files": [
            {"role": "main", "relativePath": "body.png", "sha256": request.image_sha256},
            {"role": "motion-profile", "relativePath": "motion-profile.json", "sha256": sha256_hex(&motion_bytes)}
        ]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    std::fs::write(staging.join("manifest.json"), manifest_bytes).map_err(|error| error.to_string())
}

fn validate_id(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return Err(format!("invalid {label}"));
    }
    Ok(())
}

fn profile_sha256(profile: &PixelAppearanceProfileV1) -> Result<String, String> {
    let value = serde_json::to_value(profile).map_err(|error| error.to_string())?;
    let canonical = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    Ok(sha256_hex(canonical.as_bytes()))
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            std::fs::copy(entry.path(), destination.join(entry.file_name()))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
