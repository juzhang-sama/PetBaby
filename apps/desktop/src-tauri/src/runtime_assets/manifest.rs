use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(untagged)]
pub enum RuntimeAssetManifest {
    V1(RuntimeAssetManifestV1),
    V2(RuntimeAssetManifestV2),
}

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
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

fn validate_files(files: &[ManifestFileEntry], v1: bool) -> Result<(), String> {
    if files.is_empty() {
        return Err("manifest must declare at least one file".into());
    }
    for file in files {
        if !SHA256_HEX(&file.sha256) {
            return Err("invalid file entry: sha256 must be 64 hex chars".into());
        }
        validate_relative_path(&file.relative_path)?;
        let lower = file.relative_path.to_ascii_lowercase();
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
    validate_semantics(&manifest.semantics)?;
    Ok(RuntimeAssetManifest::V2(manifest))
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
                if mapping.get("group").and_then(|v| v.as_str()).is_none() {
                    return Err(format!("invalid semantics.{group}.{key}"));
                }
            } else if !mapping.is_string() {
                return Err(format!("invalid semantics.{group}.{key}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid_json() -> &'static str {
        r#"{"schemaVersion":1,"assetType":"single-image","petId":"pet-1","variantId":"variant-1","styleId":"signature-cartoon-v1","view":"front","pose":"sitting","files":[{"role":"main","relativePath":"pet.png","sha256":"abababababababababababababababababababababababababababababababab"}],"animation":{"idleFps":12,"blinkMsMin":3000,"blinkMsMax":8000}}"#
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
        let json = r#"{"schemaVersion":2,"renderer":"live2d-v1","petId":"pet","variantId":"v","modelEntry":"model.model3.json","previewImage":"preview.png","files":[{"role":"model","relativePath":"model.model3.json","sha256":"abababababababababababababababababababababababababababababababab"},{"role":"preview","relativePath":"preview.png","sha256":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"}],"semantics":{"motions":{},"expressions":{},"hitAreas":{},"parameters":{}},"license":{"id":"test","author":"test","source":"test","commercialUse":true,"redistributable":false}}"#;
        assert!(matches!(
            parse_manifest(json).unwrap(),
            RuntimeAssetManifest::V2(_)
        ));
    }
    #[test]
    fn rejects_v2_traversal() {
        let json = r#"{"schemaVersion":2,"renderer":"live2d-v1","petId":"pet","variantId":"v","modelEntry":"../model.model3.json","previewImage":"preview.png","files":[{"role":"model","relativePath":"../model.model3.json","sha256":"abababababababababababababababababababababababababababababababab"},{"role":"preview","relativePath":"preview.png","sha256":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"}],"semantics":{"motions":{},"expressions":{},"hitAreas":{},"parameters":{}},"license":{"id":"test","author":"test","source":"test","commercialUse":true,"redistributable":false}}"#;
        assert!(parse_manifest(json)
            .unwrap_err()
            .contains("unsafe asset path"));
    }
}
