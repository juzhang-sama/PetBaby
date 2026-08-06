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
pub struct ManifestVec2 {
    pub x: f64,
    pub y: f64,
}

/// Part-level rig contract (foundation for parts-based / skeleton runtime).
/// `anchor` and `pivot` are normalized 0..1 coordinates inside the part
/// texture; `z_index` is the draw order; `bone_id` links the part to a
/// skeleton bone defined at runtime (optional for root-attached parts).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestPart {
    pub role: String,
    pub relative_path: String,
    pub anchor: ManifestVec2,
    pub pivot: ManifestVec2,
    pub z_index: i32,
    pub deformable: bool,
    #[serde(default)]
    pub bone_id: Option<String>,
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
pub struct ManifestFeatureBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Normalized (0..1 relative to the body image) feature regions used by the
/// single-image mesh rig. Produced by vision landmark analysis so the runtime
/// does not have to guess where eyes/ears/tail are.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestMeshFeatures {
    pub left_eye: ManifestFeatureBox,
    pub right_eye: ManifestFeatureBox,
    pub left_ear: ManifestFeatureBox,
    pub right_ear: ManifestFeatureBox,
    pub tail: ManifestFeatureBox,
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
    #[serde(default)]
    pub parts: Option<Vec<ManifestPart>>,
    #[serde(default)]
    pub mesh_features: Option<ManifestMeshFeatures>,
}

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Default rig metadata for a single-image asset: one root-attached part
/// anchored at the bottom-center so future idle motion keeps the pet planted.
/// No `bone_id` yet; generation tooling can refine the geometry later.
pub fn main_part(relative_path: &str) -> ManifestPart {
    ManifestPart {
        role: "main".into(),
        relative_path: relative_path.into(),
        anchor: ManifestVec2 { x: 0.5, y: 1.0 },
        pivot: ManifestVec2 { x: 0.5, y: 1.0 },
        z_index: 0,
        deformable: true,
        bone_id: None,
    }
}

/// Default rig metadata for layered assets: body + the two eye states.
/// Eye geometry is a center placeholder until generation tooling provides
/// real part bounds.
pub fn layered_parts() -> Vec<ManifestPart> {
    vec![
        ManifestPart {
            role: "body".into(),
            relative_path: "body.png".into(),
            anchor: ManifestVec2 { x: 0.5, y: 1.0 },
            pivot: ManifestVec2 { x: 0.5, y: 1.0 },
            z_index: 0,
            deformable: true,
            bone_id: None,
        },
        ManifestPart {
            role: "eye-open".into(),
            relative_path: "eye-open.png".into(),
            anchor: ManifestVec2 { x: 0.5, y: 0.5 },
            pivot: ManifestVec2 { x: 0.5, y: 0.5 },
            z_index: 1,
            deformable: false,
            bone_id: None,
        },
        ManifestPart {
            role: "eye-closed".into(),
            relative_path: "eye-closed.png".into(),
            anchor: ManifestVec2 { x: 0.5, y: 0.5 },
            pivot: ManifestVec2 { x: 0.5, y: 0.5 },
            z_index: 1,
            deformable: false,
            bone_id: None,
        },
    ]
}

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
    if let Some(parts) = &manifest.parts {
        if parts.is_empty() {
            return Err("parts must declare at least one part".into());
        }
        let mut seen_roles = std::collections::HashSet::new();
        for part in parts {
            let valid_vec2 = |value: &ManifestVec2| {
                value.x.is_finite()
                    && (0.0..=1.0).contains(&value.x)
                    && value.y.is_finite()
                    && (0.0..=1.0).contains(&value.y)
            };
            if part.role.is_empty()
                || part.relative_path.is_empty()
                || !valid_vec2(&part.anchor)
                || !valid_vec2(&part.pivot)
                || part.bone_id.as_deref().is_some_and(str::is_empty)
            {
                return Err(
                    "invalid part entry: role/relativePath/anchor/pivot/zIndex/deformable/boneId"
                        .into(),
                );
            }
            if !seen_roles.insert(&part.role) {
                return Err(format!("duplicate part role: {}", part.role));
            }
        }
    }
    if let Some(mesh) = &manifest.mesh_features {
        let valid_box = |value: &ManifestFeatureBox| {
            value.x.is_finite()
                && (0.0..=1.0).contains(&value.x)
                && value.y.is_finite()
                && (0.0..=1.0).contains(&value.y)
                && value.width.is_finite()
                && (0.0..=1.0).contains(&value.width)
                && value.height.is_finite()
                && (0.0..=1.0).contains(&value.height)
                && value.x + value.width <= 1.001
                && value.y + value.height <= 1.001
        };
        for (name, value) in [
            ("leftEye", &mesh.left_eye),
            ("rightEye", &mesh.right_eye),
            ("leftEar", &mesh.left_ear),
            ("rightEar", &mesh.right_ear),
            ("tail", &mesh.tail),
        ] {
            if !valid_box(value) {
                return Err(format!(
                    "invalid meshFeatures.{name}: box must be normalized 0..1"
                ));
            }
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

    fn valid_json_with_parts() -> String {
        r#"{
            "schemaVersion": 1,
            "assetType": "layered-v1",
            "petId": "pet-1",
            "variantId": "variant-1",
            "styleId": "signature-cartoon-v1",
            "view": "front",
            "pose": "sitting",
            "files": [
                { "role": "body", "relativePath": "body.png", "sha256": "abababababababababababababababababababababababababababababababab" }
            ],
            "animation": { "idleFps": 12, "blinkMsMin": 3000, "blinkMsMax": 8000 },
            "parts": [
                {
                    "role": "body",
                    "relativePath": "body.png",
                    "anchor": { "x": 0.5, "y": 1 },
                    "pivot": { "x": 0.5, "y": 0.5 },
                    "zIndex": 0,
                    "deformable": true,
                    "boneId": "spine"
                }
            ]
        }"#
        .to_string()
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

    #[test]
    fn parses_manifest_with_parts() {
        let manifest = parse_manifest(&valid_json_with_parts()).unwrap();
        let parts = manifest.parts.expect("parts must be present");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].role, "body");
        assert_eq!(parts[0].anchor.x, 0.5);
        assert_eq!(parts[0].bone_id.as_deref(), Some("spine"));
    }

    #[test]
    fn rejects_anchor_outside_normalized_range() {
        let json = valid_json_with_parts().replace("\"x\": 0.5, \"y\": 1", "\"x\": 1.5, \"y\": 1");
        assert!(parse_manifest(&json).unwrap_err().contains("anchor"));
    }

    #[test]
    fn rejects_duplicate_part_roles() {
        let json = valid_json_with_parts().replace(
            "\"role\": \"body\",\n                    \"relativePath\": \"body.png\",\n                    \"anchor\": { \"x\": 0.5, \"y\": 1 },\n                    \"pivot\": { \"x\": 0.5, \"y\": 0.5 },\n                    \"zIndex\": 0,\n                    \"deformable\": true,\n                    \"boneId\": \"spine\"\n                }\n            ]",
            "\"role\": \"body\",\n                    \"relativePath\": \"body.png\",\n                    \"anchor\": { \"x\": 0.5, \"y\": 1 },\n                    \"pivot\": { \"x\": 0.5, \"y\": 0.5 },\n                    \"zIndex\": 0,\n                    \"deformable\": true,\n                    \"boneId\": \"spine\"\n                },\n                {\n                    \"role\": \"body\",\n                    \"relativePath\": \"other.png\",\n                    \"anchor\": { \"x\": 0.5, \"y\": 0.5 },\n                    \"pivot\": { \"x\": 0.5, \"y\": 0.5 },\n                    \"zIndex\": 1,\n                    \"deformable\": false\n                }\n            ]",
        );
        assert!(parse_manifest(&json).unwrap_err().contains("duplicate"));
    }

    #[test]
    fn rejects_empty_parts() {
        let json = valid_json_with_parts().replace(
            "\"parts\": [\n                {\n                    \"role\": \"body\",\n                    \"relativePath\": \"body.png\",\n                    \"anchor\": { \"x\": 0.5, \"y\": 1 },\n                    \"pivot\": { \"x\": 0.5, \"y\": 0.5 },\n                    \"zIndex\": 0,\n                    \"deformable\": true,\n                    \"boneId\": \"spine\"\n                }\n            ]",
            "\"parts\": []",
        );
        assert!(parse_manifest(&json)
            .unwrap_err()
            .contains("at least one part"));
    }

    #[test]
    fn rejects_empty_bone_id() {
        let json = valid_json_with_parts().replace("\"boneId\": \"spine\"", "\"boneId\": \"\"");
        assert!(parse_manifest(&json).unwrap_err().contains("boneId"));
    }

    fn valid_json_with_mesh() -> String {
        let box_json = r#"{"x": 0.2, "y": 0.3, "width": 0.1, "height": 0.08}"#;
        format!(
            r#"{{
                "schemaVersion": 1,
                "assetType": "single-image",
                "petId": "pet-1",
                "variantId": "variant-1",
                "styleId": "signature-cartoon-v1",
                "view": "front",
                "pose": "sitting",
                "files": [
                    {{ "role": "main", "relativePath": "pet.png", "sha256": "abababababababababababababababababababababababababababababababab" }}
                ],
                "animation": {{ "idleFps": 12, "blinkMsMin": 3000, "blinkMsMax": 8000 }},
                "meshFeatures": {{
                    "leftEye": {box_json},
                    "rightEye": {box_json},
                    "leftEar": {box_json},
                    "rightEar": {box_json},
                    "tail": {box_json}
                }}
            }}"#
        )
    }

    #[test]
    fn parses_manifest_with_mesh_features() {
        let manifest = parse_manifest(&valid_json_with_mesh()).unwrap();
        let mesh = manifest.mesh_features.expect("mesh features must be present");
        assert_eq!(mesh.left_eye.x, 0.2);
        assert_eq!(mesh.tail.width, 0.1);
    }

    #[test]
    fn rejects_mesh_box_outside_normalized_range() {
        let json = valid_json_with_mesh().replace(
            r#""leftEye": {"x": 0.2, "y": 0.3, "width": 0.1, "height": 0.08}"#,
            r#""leftEye": {"x": 1.2, "y": 0.3, "width": 0.1, "height": 0.08}"#,
        );
        assert!(parse_manifest(&json).unwrap_err().contains("meshFeatures"));
    }
}
