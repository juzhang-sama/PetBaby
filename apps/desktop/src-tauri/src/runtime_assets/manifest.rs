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

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

pub fn parse_manifest(json: &str) -> Result<RuntimeAssetManifestV1, String> {
    let manifest: RuntimeAssetManifestV1 =
        serde_json::from_str(json).map_err(|error| format!("invalid manifest json: {error}"))?;
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(format!(
            "unsupported schemaVersion: {}",
            manifest.schema_version
        ));
    }
    if manifest.files.is_empty() {
        return Err("manifest must declare at least one file".into());
    }
    let valid_sha256 =
        |value: &str| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
    for file in &manifest.files {
        if !valid_sha256(&file.sha256) {
            return Err("invalid file entry: sha256 must be 64 hex chars".into());
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_json() -> &'static str {
        r#"{
            "schemaVersion": 1,
            "assetType": "single-image",
            "petId": "pet-1",
            "variantId": "variant-1",
            "styleId": "signature-cartoon-v1",
            "view": "front",
            "pose": "sitting",
            "files": [
                { "role": "main", "relativePath": "pet.png", "sha256": "abababababababababababababababababababababababababababababababab" }
            ],
            "animation": { "idleFps": 12, "blinkMsMin": 3000, "blinkMsMax": 8000 }
        }"#
    }

    #[test]
    fn parses_valid_manifest() {
        let manifest = parse_manifest(valid_json()).unwrap();
        assert_eq!(manifest.pet_id, "pet-1");
        assert_eq!(manifest.files[0].role, "main");
        assert_eq!(manifest.animation.idle_fps, 12);
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let json = valid_json().replace("\"schemaVersion\": 1", "\"schemaVersion\": 2");
        assert!(parse_manifest(&json).unwrap_err().contains("schemaVersion"));
    }

    #[test]
    fn rejects_invalid_sha256() {
        let json = valid_json().replace(
            "abababababababababababababababababababababababababababababababab",
            "zz",
        );
        assert!(parse_manifest(&json).unwrap_err().contains("sha256"));
    }

    #[test]
    fn rejects_empty_files() {
        let json = valid_json().replace(
            "\"files\": [\n                { \"role\": \"main\", \"relativePath\": \"pet.png\", \"sha256\": \"abababababababababababababababababababababababababababababababab\" }\n            ]",
            "\"files\": []",
        );
        assert!(parse_manifest(&json)
            .unwrap_err()
            .contains("at least one file"));
    }
}
