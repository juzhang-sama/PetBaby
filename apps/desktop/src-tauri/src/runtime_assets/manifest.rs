use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use super::cat_character::{
    parse_cat_character_manifest, parse_cat_spatial_manifest, RuntimeAssetManifestV4,
    RuntimeAssetManifestV5,
};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFileEntry {
    pub role: String,
    pub relative_path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestAnimation {
    pub idle_fps: u32,
    pub blink_ms_min: u32,
    pub blink_ms_max: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetManifestV1 {
    pub schema_version: u32,
    pub asset_type: String,
    pub pet_id: String,
    pub variant_id: String,
    pub style_id: String,
    pub view: String,
    pub pose: String,
    pub files: Vec<ManifestFileEntry>,
    pub animation: ManifestAnimation,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Live2DLicense {
    pub id: String,
    pub author: String,
    pub source: String,
    pub commercial_use: bool,
    pub redistributable: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetManifestV2 {
    pub schema_version: u32,
    pub renderer: String,
    pub pet_id: String,
    pub variant_id: String,
    pub model_entry: String,
    pub preview_image: String,
    pub files: Vec<ManifestFileEntry>,
    pub semantics: serde_json::Value,
    pub license: Live2DLicense,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetManifestV3 {
    pub schema_version: u32,
    pub renderer: String,
    pub pet_id: String,
    pub variant_id: String,
    pub image: String,
    pub motion_profile: String,
    pub files: Vec<ManifestFileEntry>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(untagged)]
pub enum RuntimeAssetManifest {
    V1(RuntimeAssetManifestV1),
    V2(RuntimeAssetManifestV2),
    V3(RuntimeAssetManifestV3),
    V4(RuntimeAssetManifestV4),
    V5(RuntimeAssetManifestV5),
}

pub fn manifest_files(manifest: &RuntimeAssetManifest) -> &[ManifestFileEntry] {
    match manifest {
        RuntimeAssetManifest::V1(value) => &value.files,
        RuntimeAssetManifest::V2(value) => &value.files,
        RuntimeAssetManifest::V3(value) => &value.files,
        RuntimeAssetManifest::V4(value) => &value.files,
        RuntimeAssetManifest::V5(value) => &value.files,
    }
}

pub fn manifest_identity(manifest: &RuntimeAssetManifest) -> (&str, &str) {
    match manifest {
        RuntimeAssetManifest::V1(value) => (&value.pet_id, &value.variant_id),
        RuntimeAssetManifest::V2(value) => (&value.pet_id, &value.variant_id),
        RuntimeAssetManifest::V3(value) => (&value.pet_id, &value.variant_id),
        RuntimeAssetManifest::V4(value) => (&value.pet_id, &value.variant_id),
        RuntimeAssetManifest::V5(value) => (&value.pet_id, &value.variant_id),
    }
}

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const ANIMATED_IMAGE_SCHEMA_VERSION: u32 = 3;
const SHA256_HEX: fn(&str) -> bool =
    |value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
const ALLOWED_EXTENSIONS: [&str; 8] = [
    ".json",
    ".moc3",
    ".png",
    ".motion3.json",
    ".exp3.json",
    ".physics3.json",
    ".pose3.json",
    ".userdata3.json",
];

pub fn normalize_relative_path(path: &str) -> Result<String, String> {
    if path.is_empty() || path.starts_with('/') || path.contains(':') {
        return Err(format!("unsafe asset path: {path}"));
    }
    let normalized = path.replace('\\', "/");
    if normalized
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("unsafe asset path: {path}"));
    }
    Ok(normalized)
}

pub fn validate_relative_path(path: &str) -> Result<(), String> {
    normalize_relative_path(path).map(|_| ())
}

pub(super) fn validate_files(files: &[ManifestFileEntry], v1: bool) -> Result<(), String> {
    if files.is_empty() {
        return Err("manifest must declare at least one file".into());
    }
    let mut seen_paths = HashSet::new();
    for file in files {
        if file.role.is_empty() {
            return Err("invalid file entry: role must not be empty".into());
        }
        if !SHA256_HEX(&file.sha256) {
            return Err("invalid file entry: sha256 must be 64 hex chars".into());
        }
        let normalized = normalize_relative_path(&file.relative_path)?;
        if !seen_paths.insert(normalized.clone()) {
            return Err(format!("duplicate asset path: {normalized}"));
        }
        let lower = normalized.to_ascii_lowercase();
        if v1 && !lower.ends_with(".png") {
            return Err("v1 manifests only support PNG fallback assets".into());
        }
        if !v1 && !ALLOWED_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
            return Err(format!(
                "unsupported asset extension: {}",
                file.relative_path
            ));
        }
    }
    Ok(())
}

pub fn parse_manifest_v1(json: &str) -> Result<RuntimeAssetManifestV1, String> {
    let mut manifest: RuntimeAssetManifestV1 =
        serde_json::from_str(json).map_err(|error| format!("invalid manifest json: {error}"))?;
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(format!(
            "unsupported schemaVersion: {}",
            manifest.schema_version
        ));
    }
    validate_files(&manifest.files, true)?;
    for file in &mut manifest.files {
        file.relative_path = normalize_relative_path(&file.relative_path)?;
        file.sha256.make_ascii_lowercase();
    }
    Ok(manifest)
}

pub fn parse_manifest(json: &str) -> Result<RuntimeAssetManifest, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| format!("invalid manifest json: {error}"))?;
    let version = value
        .get("schemaVersion")
        .and_then(|v| v.as_u64())
        .ok_or("missing schemaVersion")?;
    if version == 1 {
        return parse_manifest_v1(json).map(RuntimeAssetManifest::V1);
    }
    if version == 3 {
        return parse_manifest_v3(value).map(RuntimeAssetManifest::V3);
    }
    if version == 4 {
        return parse_cat_character_manifest(value).map(RuntimeAssetManifest::V4);
    }
    if version == 5 {
        return parse_cat_spatial_manifest(value).map(RuntimeAssetManifest::V5);
    }
    if version != 2 {
        return Err(format!("unsupported schemaVersion: {version}"));
    }
    let mut manifest: RuntimeAssetManifestV2 =
        serde_json::from_value(value).map_err(|error| format!("invalid v2 manifest: {error}"))?;
    if manifest.renderer != "live2d-v1" {
        return Err("unsupported renderer".into());
    }
    if manifest.pet_id.is_empty() || manifest.variant_id.is_empty() {
        return Err("missing petId or variantId".into());
    }
    validate_files(&manifest.files, false)?;
    manifest.model_entry = normalize_relative_path(&manifest.model_entry)?;
    manifest.preview_image = normalize_relative_path(&manifest.preview_image)?;
    for file in &mut manifest.files {
        file.relative_path = normalize_relative_path(&file.relative_path)?;
        file.sha256.make_ascii_lowercase();
    }
    for path in [&manifest.model_entry, &manifest.preview_image] {
        validate_relative_path(path)?;
        if !ALLOWED_EXTENSIONS
            .iter()
            .any(|ext| path.to_ascii_lowercase().ends_with(ext))
        {
            return Err(format!("unsupported asset extension: {path}"));
        }
    }
    if !manifest
        .files
        .iter()
        .any(|file| file.relative_path == manifest.model_entry)
    {
        return Err("modelEntry is not listed in files".into());
    }
    if !manifest
        .files
        .iter()
        .any(|file| file.relative_path == manifest.preview_image)
    {
        return Err("previewImage is not listed in files".into());
    }
    for (field, value) in [
        ("license.id", &manifest.license.id),
        ("license.author", &manifest.license.author),
        ("license.source", &manifest.license.source),
    ] {
        if value.is_empty() {
            return Err(format!("missing or invalid {field}"));
        }
    }
    validate_semantics(&manifest.semantics)?;
    Ok(RuntimeAssetManifest::V2(manifest))
}

fn parse_manifest_v3(value: serde_json::Value) -> Result<RuntimeAssetManifestV3, String> {
    let mut manifest: RuntimeAssetManifestV3 =
        serde_json::from_value(value).map_err(|error| format!("invalid v3 manifest: {error}"))?;
    if manifest.renderer != "animated-image-v1" {
        return Err("unsupported renderer".into());
    }
    if manifest.pet_id.is_empty() || manifest.variant_id.is_empty() {
        return Err("missing petId or variantId".into());
    }
    validate_files(&manifest.files, false)?;
    manifest.image =
        normalize_relative_path(&manifest.image).map_err(|_| "image must be a relative path")?;
    manifest.motion_profile = normalize_relative_path(&manifest.motion_profile)
        .map_err(|_| "motionProfile must be a relative path")?;
    if !manifest.image.to_ascii_lowercase().ends_with(".png") {
        return Err("image must be a PNG file".into());
    }
    if !manifest
        .motion_profile
        .to_ascii_lowercase()
        .ends_with(".json")
    {
        return Err("motionProfile must be a JSON file".into());
    }
    for file in &mut manifest.files {
        file.relative_path = normalize_relative_path(&file.relative_path)?;
        file.sha256.make_ascii_lowercase();
    }
    let has_file = |role: &str, path: &str| {
        manifest
            .files
            .iter()
            .any(|file| file.role == role && file.relative_path == path)
    };
    if !has_file("main", &manifest.image) {
        return Err("image is not listed as the main file".into());
    }
    if !has_file("motion-profile", &manifest.motion_profile) {
        return Err("motionProfile is not listed as the motion-profile file".into());
    }
    Ok(manifest)
}

fn validate_semantics(value: &serde_json::Value) -> Result<(), String> {
    let root = value.as_object().ok_or("invalid semantics")?;
    let groups = [
        (
            "motions",
            &[
                "idle",
                "look-left",
                "look-right",
                "react-happy",
                "react-curious",
                "sleep",
                "wake",
                "carried",
                "landed",
            ][..],
        ),
        (
            "expressions",
            &["neutral", "happy", "curious", "sleepy", "sad", "angry"][..],
        ),
        ("hitAreas", &["head", "body"][..]),
        (
            "parameters",
            &[
                "eyeOpen",
                "eyeBallX",
                "eyeBallY",
                "angleX",
                "angleY",
                "bodyBreath",
                "bodySway",
                "mouthOpen",
            ][..],
        ),
    ];
    for (group, allowed) in groups {
        let mappings = root
            .get(group)
            .and_then(|v| v.as_object())
            .ok_or_else(|| format!("invalid semantics.{group}"))?;
        for (key, mapping) in mappings {
            if !allowed.contains(&key.as_str()) {
                return Err(format!("unknown semantics.{group}.{key}"));
            }
            if group == "motions" {
                let motion_group = mapping.get("group").and_then(|v| v.as_str());
                let index_valid = mapping
                    .get("index")
                    .is_none_or(|index| index.as_u64().is_some());
                if motion_group.is_none_or(str::is_empty) || !index_valid {
                    return Err(format!("invalid semantics.{group}.{key}"));
                }
            } else if mapping.as_str().is_none_or(str::is_empty) {
                return Err(format!("invalid semantics.{group}.{key}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_v3_json() -> String {
        serde_json::json!({
            "schemaVersion": 3,
            "renderer": "animated-image-v1",
            "petId": "pet-a",
            "variantId": "variant-a",
            "image": "body.png",
            "motionProfile": "motion-profile.json",
            "files": [
                { "role": "main", "relativePath": "body.png", "sha256": "ab".repeat(32) },
                { "role": "motion-profile", "relativePath": "motion-profile.json", "sha256": "cd".repeat(32) }
            ]
        })
        .to_string()
    }

    fn valid_json() -> &'static str {
        r#"{"schemaVersion":1,"assetType":"single-image","petId":"pet-1","variantId":"variant-1","styleId":"signature-cartoon-v1","view":"front","pose":"sitting","files":[{"role":"main","relativePath":"pet.png","sha256":"abababababababababababababababababababababababababababababababab"}],"animation":{"idleFps":12,"blinkMsMin":3000,"blinkMsMax":8000}}"#
    }
    fn valid_v2_json() -> &'static str {
        r#"{"schemaVersion":2,"renderer":"live2d-v1","petId":"pet","variantId":"v","modelEntry":"model.model3.json","previewImage":"preview.png","files":[{"role":"model","relativePath":"model.model3.json","sha256":"abababababababababababababababababababababababababababababababab"},{"role":"preview","relativePath":"preview.png","sha256":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"}],"semantics":{"motions":{},"expressions":{},"hitAreas":{},"parameters":{}},"license":{"id":"test","author":"test","source":"test","commercialUse":true,"redistributable":false}}"#
    }
    #[test]
    fn parses_valid_manifest() {
        assert!(matches!(
            parse_manifest(valid_json()).unwrap(),
            RuntimeAssetManifest::V1(_)
        ));
    }
    #[test]
    fn rejects_unknown_schema_version() {
        assert!(parse_manifest(
            &valid_json().replace("\"schemaVersion\":1", "\"schemaVersion\":3")
        )
        .is_err());
    }
    #[test]
    fn rejects_invalid_sha256() {
        assert!(parse_manifest(&valid_json().replace(
            "abababababababababababababababababababababababababababababababab",
            "zz"
        ))
        .is_err());
    }
    #[test]
    fn rejects_empty_files() {
        assert!(parse_manifest(&valid_json().replace("\"files\":[{\"role\":\"main\",\"relativePath\":\"pet.png\",\"sha256\":\"abababababababababababababababababababababababababababababababab\"}]", "\"files\":[]")).is_err());
    }
    #[test]
    fn parses_valid_v2_manifest() {
        assert!(matches!(
            parse_manifest(valid_v2_json()).unwrap(),
            RuntimeAssetManifest::V2(_)
        ));
    }

    #[test]
    fn parses_an_animated_image_v3_manifest() {
        let manifest = parse_manifest(&valid_v3_json()).unwrap();
        let RuntimeAssetManifest::V3(value) = manifest else {
            panic!("expected v3")
        };
        assert_eq!(value.renderer, "animated-image-v1");
        assert_eq!(value.image, "body.png");
        assert_eq!(value.motion_profile, "motion-profile.json");
    }

    #[test]
    fn rejects_a_v3_motion_profile_traversal_path() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_v3_json()).unwrap();
        value["motionProfile"] = "../motion-profile.json".into();
        assert!(parse_manifest(&value.to_string())
            .unwrap_err()
            .contains("relative path"));
    }

    #[test]
    fn accepts_body_sway_parameter_semantic() {
        let json = valid_v2_json().replace(
            "\"parameters\":{}",
            "\"parameters\":{\"bodyBreath\":\"ParamBreath\",\"bodySway\":\"ParamBodyAngleX\"}",
        );
        assert!(matches!(
            parse_manifest(&json).unwrap(),
            RuntimeAssetManifest::V2(_)
        ));
    }
    #[test]
    fn rejects_v2_traversal() {
        let json = valid_v2_json().replace("model.model3.json", "../model.model3.json");
        assert!(parse_manifest(&json)
            .unwrap_err()
            .contains("unsafe asset path"));
    }

    #[test]
    fn normalizes_uppercase_sha256() {
        let uppercase = "AB".repeat(32);
        let json = valid_json().replace(
            "abababababababababababababababababababababababababababababababab",
            &uppercase,
        );
        let RuntimeAssetManifest::V1(manifest) = parse_manifest(&json).unwrap() else {
            panic!("expected v1 manifest");
        };
        assert_eq!(
            manifest.files[0].sha256,
            "abababababababababababababababababababababababababababababababab"
        );
    }

    #[test]
    fn rejects_empty_license_strings() {
        let json = valid_v2_json().replace("\"author\":\"test\"", "\"author\":\"\"");
        assert!(parse_manifest(&json)
            .unwrap_err()
            .contains("license.author"));
    }

    #[test]
    fn rejects_invalid_semantic_mapping() {
        let json = valid_v2_json().replace("\"motions\":{}", "\"motions\":{\"idle\":\"Idle\"}");
        assert!(parse_manifest(&json)
            .unwrap_err()
            .contains("semantics.motions.idle"));

        let bad_index = valid_v2_json().replace(
            "\"motions\":{}",
            "\"motions\":{\"idle\":{\"group\":\"Idle\",\"index\":\"0\"}}",
        );
        assert!(parse_manifest(&bad_index)
            .unwrap_err()
            .contains("semantics.motions.idle"));
    }

    #[test]
    fn rejects_empty_roles_and_duplicate_paths() {
        let empty_role = valid_v2_json().replace("\"role\":\"model\"", "\"role\":\"\"");
        assert!(parse_manifest(&empty_role).unwrap_err().contains("role"));

        let duplicate = valid_v2_json().replace("preview.png", "model.model3.json");
        assert!(parse_manifest(&duplicate)
            .unwrap_err()
            .contains("duplicate asset path"));
    }
}
