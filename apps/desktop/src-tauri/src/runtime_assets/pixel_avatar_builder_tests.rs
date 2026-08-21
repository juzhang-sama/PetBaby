use super::pixel_avatar_builder::{BuildPixelAvatarRequest, PixelAvatarBuilder};
use super::pixel_png::inspect_rgba_png;
use crate::creation::photo_avatar::domain::{
    PixelAppearanceProfileV1, PixelAvatarAudit, PixelAvatarAuditV1, PixelAvatarAuditV2,
    PixelIdentityTraitKey, PixelIdentityTraitV1, PixelStyleProfileId, TraitSource,
};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use sha2::{Digest, Sha256};
use std::io::Cursor;

fn profile() -> PixelAppearanceProfileV1 {
    let keys = [
        PixelIdentityTraitKey::FaceShape,
        PixelIdentityTraitKey::FaceProportions,
        PixelIdentityTraitKey::EyeShape,
        PixelIdentityTraitKey::EyeColor,
        PixelIdentityTraitKey::EarShape,
        PixelIdentityTraitKey::PrimaryFurColor,
        PixelIdentityTraitKey::SecondaryFurColor,
        PixelIdentityTraitKey::FaceMarkings,
        PixelIdentityTraitKey::ChestMarkings,
        PixelIdentityTraitKey::PawMarkings,
        PixelIdentityTraitKey::BodyMarkings,
        PixelIdentityTraitKey::TailShape,
        PixelIdentityTraitKey::TailMarkings,
        PixelIdentityTraitKey::SignatureMarks,
        PixelIdentityTraitKey::Temperament,
    ];
    PixelAppearanceProfileV1 {
        schema_version: 1,
        species: "cat".into(),
        style_profile_id: PixelStyleProfileId::V1,
        traits: keys
            .into_iter()
            .map(|key| PixelIdentityTraitV1 {
                key,
                value: "fixture".into(),
                source: TraitSource::User,
                evidence_photo_ids: vec!["source-0".into()],
            })
            .collect(),
        completion_summary: vec![],
    }
}

fn png_with_subject(left: u32) -> Vec<u8> {
    let mut image = RgbaImage::new(1024, 1024);
    for y in 100..924 {
        for x in left..(1024 - left) {
            image.put_pixel(x, y, Rgba([30, 60, 90, 255]));
        }
    }
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("fixture PNG encoding must work");
    bytes
}

fn request(png: Vec<u8>) -> BuildPixelAvatarRequest {
    let inspection = inspect_rgba_png(&png).expect("fixture geometry must pass");
    let profile = profile();
    let profile_value = serde_json::to_value(&profile).expect("profile JSON must work");
    let profile_json = serde_json::to_string(&profile_value).expect("profile JSON must serialize");
    let image_sha256 = format!("{:x}", Sha256::digest(&png));
    BuildPixelAvatarRequest {
        session_id: "session-a".into(),
        revision: 1,
        attempt: 1,
        pet_id: "pet-a".into(),
        variant_id: "photo-avatar-session-a-1".into(),
        profile,
        image_png: png,
        image_sha256: image_sha256.clone(),
        audit: PixelAvatarAudit::V1(PixelAvatarAuditV1 {
            schema_version: 1,
            session_id: "session-a".into(),
            revision: 1,
            attempt: 1,
            provider: "lk888".into(),
            provider_model: "gpt-image-2".into(),
            provider_task_id: "108652999".into(),
            style_profile_id: "pixel-style-v1".into(),
            style_profile_sha256:
                super::super::creation::photo_avatar::pixel_contract::PIXEL_V1_STYLE_PROFILE_SHA256
                    .into(),
            reference_sha256:
                super::super::creation::photo_avatar::pixel_contract::PIXEL_V1_REFERENCE_SHA256.into(),
            prompt_template_version:
                super::super::creation::photo_avatar::pixel_contract::PIXEL_V1_PROMPT_TEMPLATE_VERSION
                    .into(),
            identity_profile_sha256: format!("{:x}", Sha256::digest(profile_json.as_bytes())),
            provider_raw_sha256: "11".repeat(32),
            normalized_sha256: image_sha256,
            width: inspection.width,
            height: inspection.height,
            alpha_report: inspection.alpha_report,
            privacy_policy_version: "unverified".into(),
            retention_policy: "unverified".into(),
            upstream_delete_api: "unsupported".into(),
            status: "succeeded".into(),
            error_code: None,
            created_at: "2026-08-18T00:00:00Z".into(),
            completed_at: "2026-08-18T00:00:01Z".into(),
        }),
    }
}

fn v2_request(png: Vec<u8>) -> BuildPixelAvatarRequest {
    let inspection = inspect_rgba_png(&png).expect("fixture geometry must pass");
    let mut profile = profile();
    profile.style_profile_id = PixelStyleProfileId::V2AnimationReady;
    let profile_value = serde_json::to_value(&profile).expect("profile JSON must work");
    let profile_json = serde_json::to_string(&profile_value).expect("profile JSON must serialize");
    let image_sha256 = format!("{:x}", Sha256::digest(&png));
    BuildPixelAvatarRequest {
        session_id: "session-a".into(),
        revision: 1,
        attempt: 1,
        pet_id: "pet-a".into(),
        variant_id: "photo-avatar-session-a-1".into(),
        profile,
        image_png: png,
        image_sha256: image_sha256.clone(),
        audit: PixelAvatarAudit::V2(PixelAvatarAuditV2 {
            schema_version: 2,
            session_id: "session-a".into(),
            revision: 1,
            attempt: 1,
            provider: "lk888".into(),
            provider_model: "gpt-image-2".into(),
            provider_task_id: "108652999".into(),
            style_profile_id: "pixel-style-v2-animation-ready".into(),
            style_profile_sha256:
                crate::creation::photo_avatar::pixel_contract::PIXEL_V2_STYLE_PROFILE_SHA256.into(),
            reference_sha256:
                crate::creation::photo_avatar::pixel_contract::PIXEL_V2_REFERENCE_SHA256.into(),
            prompt_template_version: "pixel-style-v2-animation-ready-prompt-v2".into(),
            identity_profile_sha256: format!("{:x}", Sha256::digest(profile_json.as_bytes())),
            provider_raw_sha256: "11".repeat(32),
            normalized_sha256: image_sha256,
            width: inspection.width,
            height: inspection.height,
            alpha_report: inspection.alpha_report,
            privacy_policy_version: "unverified".into(),
            retention_policy: "unverified".into(),
            upstream_delete_api: "unsupported".into(),
            status: "succeeded".into(),
            error_code: None,
            created_at: "2026-08-21T00:00:00Z".into(),
            completed_at: "2026-08-21T00:00:01Z".into(),
            logical_grid_size: 160,
            palette_color_limit: 24,
            visible_color_count: 2,
            quantize_method: "maxcoverage".into(),
            dither: "none".into(),
            protected_accent_slots: 4,
            protected_accent_count: 1,
            downsample: "box".into(),
            upsample: "nearest".into(),
        }),
    }
}

#[test]
fn v2_builder_recounts_visible_colors() {
    let root = std::env::temp_dir().join("desktop-pet-v2-color-count-test");
    let builder = PixelAvatarBuilder::new(&root);
    let request = v2_request(png_with_subject(100));

    let error = builder.build_preview(request).unwrap_err();

    assert!(error.contains("visible color count"), "{error}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn pixel_builder_writes_schema_v3_body_and_motion_assets() {
    let root =
        std::env::temp_dir().join(format!("desktop-pet-pixel-builder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let builder = PixelAvatarBuilder::new(&root);
    let package = builder
        .build_preview(request(png_with_subject(100)))
        .expect("valid pixel candidate must build");
    assert!(package.preview_dir.join("body.png").is_file());
    assert!(package.preview_dir.join("motion-profile.json").is_file());
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(package.preview_dir.join("manifest.json")).expect("manifest exists"),
    )
    .expect("manifest JSON is valid");
    assert_eq!(manifest["schemaVersion"], 3);
    assert_eq!(manifest["renderer"], "animated-image-v1");
    builder
        .validate_preview("session-a", 1)
        .expect("built candidate validates");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn pixel_png_gate_rejects_subject_touching_canvas_edge() {
    let mut image = RgbaImage::new(1024, 1024);
    for y in 0..1024 {
        for x in 0..1024 {
            image.put_pixel(x, y, Rgba([30, 60, 90, 255]));
        }
    }
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("fixture PNG encoding must work");
    assert!(inspect_rgba_png(&bytes).is_err());
}
