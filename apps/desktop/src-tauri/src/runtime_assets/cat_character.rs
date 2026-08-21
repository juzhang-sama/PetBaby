use super::manifest::{normalize_relative_path, validate_files, Live2DLicense, ManifestFileEntry};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

const MOTIONS: [&str; 8] = [
    "breathing",
    "blink",
    "ear-twitch",
    "tail-idle",
    "pointer-focus",
    "pet-happy",
    "sleepy-yawn",
    "half-stand-stretch",
];
const PARAMETERS: [&str; 12] = [
    "eyeOpenLeft",
    "eyeOpenRight",
    "eyeBallX",
    "eyeBallY",
    "earLeft",
    "earRight",
    "tailAngle",
    "tailCurl",
    "tailTip",
    "bodyBreath",
    "bodyStretch",
    "mouthOpen",
];
const HIT_AREAS: [&str; 2] = ["body", "edgeTail"];
const EDGES: [&str; 4] = ["left", "right", "top", "bottom"];
const BODY_MODULE_IDS: [&str; 3] = ["body-slender-v1", "body-balanced-v1", "body-rounded-v1"];

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatMotionMappingV1 {
    pub group: String,
    pub index: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatEdgeTailMappingV1 {
    pub group: String,
    pub index: Option<u32>,
    pub tail_art_mesh: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetManifestV4 {
    pub schema_version: u32,
    pub renderer: String,
    pub pet_id: String,
    pub variant_id: String,
    pub skeleton_version: String,
    pub model_entry: String,
    pub preview_image: String,
    pub files: Vec<ManifestFileEntry>,
    pub motions: BTreeMap<String, CatMotionMappingV1>,
    pub parameters: BTreeMap<String, String>,
    pub hit_areas: BTreeMap<String, String>,
    pub edge_tail_states: BTreeMap<String, CatEdgeTailMappingV1>,
    pub license: Live2DLicense,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetManifestV5 {
    pub schema_version: u32,
    pub renderer: String,
    pub pet_id: String,
    pub variant_id: String,
    pub skeleton_version: String,
    pub body_module_id: String,
    pub model_entry: String,
    pub preview_image: String,
    pub motion_spatial_profile: String,
    pub files: Vec<ManifestFileEntry>,
    pub motions: BTreeMap<String, CatMotionMappingV1>,
    pub parameters: BTreeMap<String, String>,
    pub hit_areas: BTreeMap<String, String>,
    pub edge_tail_states: BTreeMap<String, CatEdgeTailMappingV1>,
    pub license: Live2DLicense,
}

fn validate_exact_keys<T>(
    field: &str,
    values: &BTreeMap<String, T>,
    allowed: &[&str],
) -> Result<(), String> {
    for key in values.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unknown {field}.{key}"));
        }
    }
    for key in allowed {
        if !values.contains_key(*key) {
            return Err(format!("missing {field}.{key}"));
        }
    }
    Ok(())
}

fn validate_unique_values(field: &str, values: impl Iterator<Item = String>) -> Result<(), String> {
    let values: Vec<_> = values.collect();
    let unique: HashSet<_> = values.iter().collect();
    if unique.len() != values.len() {
        return Err(format!("{field} IDs must be unique"));
    }
    Ok(())
}

pub fn parse_cat_character_manifest(
    value: serde_json::Value,
) -> Result<RuntimeAssetManifestV4, String> {
    let mut manifest: RuntimeAssetManifestV4 =
        serde_json::from_value(value).map_err(|error| format!("invalid v4 manifest: {error}"))?;
    if manifest.schema_version != 4 {
        return Err(format!(
            "unsupported schemaVersion: {}",
            manifest.schema_version
        ));
    }
    if manifest.renderer != "cat-live2d-v1" {
        return Err("unsupported renderer".into());
    }
    if manifest.skeleton_version != "cat-a-live2d-v1" {
        return Err("unsupported skeletonVersion".into());
    }
    if manifest.pet_id.is_empty() || manifest.variant_id.is_empty() {
        return Err("missing petId or variantId".into());
    }

    validate_files(&manifest.files, false)?;
    let mut windows_paths = HashSet::new();
    for file in &manifest.files {
        let normalized = normalize_relative_path(&file.relative_path)?;
        if !windows_paths.insert(normalized.to_ascii_lowercase()) {
            return Err(format!("duplicate asset path: {normalized}"));
        }
    }
    manifest.model_entry = normalize_relative_path(&manifest.model_entry)?;
    manifest.preview_image = normalize_relative_path(&manifest.preview_image)?;
    if !manifest
        .model_entry
        .to_ascii_lowercase()
        .ends_with(".model3.json")
    {
        return Err("modelEntry must be a .model3.json file".into());
    }
    if !manifest
        .preview_image
        .to_ascii_lowercase()
        .ends_with(".png")
    {
        return Err("previewImage must be a PNG file".into());
    }
    for file in &mut manifest.files {
        file.relative_path = normalize_relative_path(&file.relative_path)?;
        file.sha256.make_ascii_lowercase();
    }
    for path in [&manifest.model_entry, &manifest.preview_image] {
        if !manifest
            .files
            .iter()
            .any(|file| &file.relative_path == path)
        {
            return Err(format!("{path} is not listed in files"));
        }
    }

    validate_exact_keys("motions", &manifest.motions, &MOTIONS)?;
    validate_exact_keys("parameters", &manifest.parameters, &PARAMETERS)?;
    validate_exact_keys("hitAreas", &manifest.hit_areas, &HIT_AREAS)?;
    validate_exact_keys("edgeTailStates", &manifest.edge_tail_states, &EDGES)?;
    for (key, value) in &manifest.motions {
        if value.group.is_empty() {
            return Err(format!("invalid motions.{key}"));
        }
    }
    for (key, value) in &manifest.edge_tail_states {
        if value.group.is_empty() || value.tail_art_mesh.is_empty() {
            return Err(format!("invalid edgeTailStates.{key}"));
        }
    }
    for (field, values) in [
        ("parameters", manifest.parameters.values()),
        ("hitAreas", manifest.hit_areas.values()),
    ] {
        if values.clone().any(String::is_empty) {
            return Err(format!("invalid {field}"));
        }
        validate_unique_values(field, values.cloned())?;
    }
    let tail_meshes: HashSet<_> = manifest
        .edge_tail_states
        .values()
        .map(|value| value.tail_art_mesh.as_str())
        .collect();
    if tail_meshes.len() != 1 {
        return Err("all edgeTailStates must reuse the same tail ArtMesh".into());
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
    if !manifest.license.redistributable {
        return Err("license must be redistributable".into());
    }
    Ok(manifest)
}

pub fn parse_cat_spatial_manifest(
    value: serde_json::Value,
) -> Result<RuntimeAssetManifestV5, String> {
    let mut manifest: RuntimeAssetManifestV5 = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid v5 manifest: {error}"))?;
    if manifest.schema_version != 5 {
        return Err(format!(
            "unsupported schemaVersion: {}",
            manifest.schema_version
        ));
    }
    if manifest.renderer != "cat-spatial-live2d-v1" {
        return Err("unsupported renderer".into());
    }
    if !BODY_MODULE_IDS.contains(&manifest.body_module_id.as_str()) {
        return Err("unsupported bodyModuleId".into());
    }
    manifest.motion_spatial_profile = normalize_relative_path(&manifest.motion_spatial_profile)?;
    if !manifest
        .motion_spatial_profile
        .to_ascii_lowercase()
        .ends_with(".json")
    {
        return Err("motionSpatialProfile must be a JSON file".into());
    }

    let mut v4_value = value;
    let object = v4_value
        .as_object_mut()
        .ok_or("manifest must be an object")?;
    object.insert("schemaVersion".into(), 4.into());
    object.insert("renderer".into(), "cat-live2d-v1".into());
    let v4 = parse_cat_character_manifest(v4_value)?;
    if !v4.files.iter().any(|file| {
        file.role == "motion-spatial-profile"
            && file.relative_path == manifest.motion_spatial_profile
    }) {
        return Err(
            "motionSpatialProfile is not listed as the motion-spatial-profile file in files".into(),
        );
    }

    Ok(RuntimeAssetManifestV5 {
        schema_version: 5,
        renderer: "cat-spatial-live2d-v1".into(),
        pet_id: v4.pet_id,
        variant_id: v4.variant_id,
        skeleton_version: v4.skeleton_version,
        body_module_id: manifest.body_module_id,
        model_entry: v4.model_entry,
        preview_image: v4.preview_image,
        motion_spatial_profile: manifest.motion_spatial_profile,
        files: v4.files,
        motions: v4.motions,
        parameters: v4.parameters,
        hit_areas: v4.hit_areas,
        edge_tail_states: v4.edge_tail_states,
        license: v4.license,
    })
}

#[derive(Clone, Copy)]
struct SpatialPoint {
    x: f64,
    y: f64,
}

#[derive(Clone, Copy)]
struct SpatialRect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

pub fn parse_motion_spatial_profile_v1(
    json: &str,
    expected_body_module_id: &str,
) -> Result<(), String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| format!("invalid motion spatial profile: {error}"))?;
    let root = exact_object(
        &value,
        "profile",
        &[
            "schemaVersion",
            "bodyModuleId",
            "canvas",
            "alphaBounds",
            "faceSafeZone",
            "eyes",
            "earRoots",
            "breathZone",
            "stretchAxis",
            "swayPivot",
            "tailRoot",
            "edgeTailBounds",
            "amplitude",
        ],
    )?;
    if root
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err("invalid MotionSpatialProfileV1: schemaVersion must be 1".into());
    }
    let body_module_id = required_string(root, "bodyModuleId", "bodyModuleId")?;
    if !BODY_MODULE_IDS.contains(&body_module_id.as_str())
        || body_module_id != expected_body_module_id
    {
        return Err(
            "invalid MotionSpatialProfileV1: bodyModuleId is not a supported body module".into(),
        );
    }
    let canvas = exact_object(
        required(root, "canvas", "canvas")?,
        "canvas",
        &["width", "height"],
    )?;
    positive_integer(required(canvas, "width", "canvas.width")?, "canvas.width")?;
    positive_integer(
        required(canvas, "height", "canvas.height")?,
        "canvas.height",
    )?;
    let alpha_bounds =
        normalized_rect(required(root, "alphaBounds", "alphaBounds")?, "alphaBounds")?;
    let face_safe_zone = normalized_rect(
        required(root, "faceSafeZone", "faceSafeZone")?,
        "faceSafeZone",
    )?;
    require_rect_inside(face_safe_zone, alpha_bounds, "faceSafeZone", "alphaBounds")?;

    let eyes = exact_object(required(root, "eyes", "eyes")?, "eyes", &["left", "right"])?;
    let left_eye = spatial_eye(required(eyes, "left", "eyes.left")?, "eyes.left")?;
    let right_eye = spatial_eye(required(eyes, "right", "eyes.right")?, "eyes.right")?;
    for (point, bounds, name) in [
        (left_eye.0, left_eye.1, "eyes.left"),
        (right_eye.0, right_eye.1, "eyes.right"),
    ] {
        require_rect_inside(
            bounds,
            face_safe_zone,
            &format!("{name}.bounds"),
            "faceSafeZone",
        )?;
        require_point_inside(
            point,
            bounds,
            &format!("{name}.center"),
            &format!("{name}.bounds"),
        )?;
    }
    if left_eye.0.x >= right_eye.0.x || left_eye.1.left >= right_eye.1.left {
        return Err(
            "invalid MotionSpatialProfileV1: eyes must preserve left-to-right ordering".into(),
        );
    }

    let ear_roots = exact_object(
        required(root, "earRoots", "earRoots")?,
        "earRoots",
        &["left", "right"],
    )?;
    let left_ear = normalized_point(
        required(ear_roots, "left", "earRoots.left")?,
        "earRoots.left",
    )?;
    let right_ear = normalized_point(
        required(ear_roots, "right", "earRoots.right")?,
        "earRoots.right",
    )?;
    for (point, name) in [(left_ear, "earRoots.left"), (right_ear, "earRoots.right")] {
        require_point_inside(point, alpha_bounds, name, "alphaBounds")?;
        if point.y > face_safe_zone.bottom {
            return Err(format!(
                "invalid MotionSpatialProfileV1: {name} must remain in the subject upper region"
            ));
        }
    }
    if left_ear.x >= right_ear.x {
        return Err(
            "invalid MotionSpatialProfileV1: earRoots must preserve left-to-right ordering".into(),
        );
    }

    let breath_zone = normalized_rect(required(root, "breathZone", "breathZone")?, "breathZone")?;
    require_rect_inside(breath_zone, alpha_bounds, "breathZone", "alphaBounds")?;
    if positive_area_overlap(breath_zone, left_eye.1)
        || positive_area_overlap(breath_zone, right_eye.1)
    {
        return Err(
            "invalid MotionSpatialProfileV1: breathZone must not overlap eyes with positive area"
                .into(),
        );
    }
    let stretch_axis = exact_object(
        required(root, "stretchAxis", "stretchAxis")?,
        "stretchAxis",
        &["origin", "direction"],
    )?;
    let stretch_origin = normalized_point(
        required(stretch_axis, "origin", "stretchAxis.origin")?,
        "stretchAxis.origin",
    )?;
    let stretch_direction = normalized_point(
        required(stretch_axis, "direction", "stretchAxis.direction")?,
        "stretchAxis.direction",
    )?;
    require_point_inside(
        stretch_origin,
        alpha_bounds,
        "stretchAxis.origin",
        "alphaBounds",
    )?;
    if stretch_direction.x == 0.0 && stretch_direction.y == 0.0 {
        return Err(
            "invalid MotionSpatialProfileV1: stretchAxis.direction must be non-zero".into(),
        );
    }
    let sway_pivot = normalized_point(required(root, "swayPivot", "swayPivot")?, "swayPivot")?;
    require_point_inside(sway_pivot, alpha_bounds, "swayPivot", "alphaBounds")?;
    let tail_root = normalized_point(required(root, "tailRoot", "tailRoot")?, "tailRoot")?;
    let edge_tail_bounds = normalized_rect(
        required(root, "edgeTailBounds", "edgeTailBounds")?,
        "edgeTailBounds",
    )?;
    require_rect_inside(
        edge_tail_bounds,
        alpha_bounds,
        "edgeTailBounds",
        "alphaBounds",
    )?;
    require_point_inside(tail_root, alpha_bounds, "tailRoot", "alphaBounds")?;
    require_point_inside(tail_root, edge_tail_bounds, "tailRoot", "edgeTailBounds")?;

    let amplitude = exact_object(
        required(root, "amplitude", "amplitude")?,
        "amplitude",
        &[
            "breath",
            "blink",
            "ear",
            "tailAngle",
            "tailCurl",
            "tailTip",
            "bodyStretch",
        ],
    )?;
    for semantic in [
        "breath",
        "blink",
        "ear",
        "tailAngle",
        "tailCurl",
        "tailTip",
        "bodyStretch",
    ] {
        let range = exact_object(
            required(amplitude, semantic, &format!("amplitude.{semantic}"))?,
            &format!("amplitude.{semantic}"),
            &["min", "max"],
        )?;
        let min = finite_number(
            required(range, "min", &format!("amplitude.{semantic}.min"))?,
            &format!("amplitude.{semantic}.min"),
        )?;
        let max = finite_number(
            required(range, "max", &format!("amplitude.{semantic}.max"))?,
            &format!("amplitude.{semantic}.max"),
        )?;
        if min >= max {
            return Err(format!(
                "invalid MotionSpatialProfileV1: amplitude.{semantic}.min must be less than max"
            ));
        }
    }
    Ok(())
}

fn exact_object<'a>(
    value: &'a serde_json::Value,
    path: &str,
    keys: &[&str],
) -> Result<&'a serde_json::Map<String, serde_json::Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("invalid MotionSpatialProfileV1: {path} must be an object"))?;
    for key in object.keys() {
        if !keys.contains(&key.as_str()) {
            return Err(format!(
                "invalid MotionSpatialProfileV1: {path} has unknown field {key:?}"
            ));
        }
    }
    for key in keys {
        if !object.contains_key(*key) {
            return Err(format!(
                "invalid MotionSpatialProfileV1: missing {path}.{key}"
            ));
        }
    }
    Ok(object)
}

fn required<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &str,
) -> Result<&'a serde_json::Value, String> {
    object
        .get(key)
        .ok_or_else(|| format!("invalid MotionSpatialProfileV1: missing {path}"))
}

fn required_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &str,
) -> Result<String, String> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("invalid MotionSpatialProfileV1: {path} must be a non-empty string"))
}

fn finite_number(value: &serde_json::Value, path: &str) -> Result<f64, String> {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("invalid MotionSpatialProfileV1: {path} must be finite"))
}

fn positive_integer(value: &serde_json::Value, path: &str) -> Result<(), String> {
    let number = finite_number(value, path)?;
    if number <= 0.0 || number.fract() != 0.0 {
        return Err(format!(
            "invalid MotionSpatialProfileV1: {path} must be a positive integer"
        ));
    }
    Ok(())
}

fn normalized_point(value: &serde_json::Value, path: &str) -> Result<SpatialPoint, String> {
    let object = exact_object(value, path, &["x", "y"])?;
    Ok(SpatialPoint {
        x: normalized_number(
            required(object, "x", &format!("{path}.x"))?,
            &format!("{path}.x"),
        )?,
        y: normalized_number(
            required(object, "y", &format!("{path}.y"))?,
            &format!("{path}.y"),
        )?,
    })
}

fn normalized_rect(value: &serde_json::Value, path: &str) -> Result<SpatialRect, String> {
    let object = exact_object(value, path, &["left", "top", "right", "bottom"])?;
    let rect = SpatialRect {
        left: normalized_number(
            required(object, "left", &format!("{path}.left"))?,
            &format!("{path}.left"),
        )?,
        top: normalized_number(
            required(object, "top", &format!("{path}.top"))?,
            &format!("{path}.top"),
        )?,
        right: normalized_number(
            required(object, "right", &format!("{path}.right"))?,
            &format!("{path}.right"),
        )?,
        bottom: normalized_number(
            required(object, "bottom", &format!("{path}.bottom"))?,
            &format!("{path}.bottom"),
        )?,
    };
    if rect.left >= rect.right || rect.top >= rect.bottom {
        return Err(format!(
            "invalid MotionSpatialProfileV1: {path} must have positive area"
        ));
    }
    Ok(rect)
}

fn normalized_number(value: &serde_json::Value, path: &str) -> Result<f64, String> {
    let number = finite_number(value, path)?;
    if !(0.0..=1.0).contains(&number) {
        return Err(format!(
            "invalid MotionSpatialProfileV1: {path} must be within [0, 1]"
        ));
    }
    Ok(number)
}

fn spatial_eye(
    value: &serde_json::Value,
    path: &str,
) -> Result<(SpatialPoint, SpatialRect), String> {
    let object = exact_object(value, path, &["center", "bounds"])?;
    Ok((
        normalized_point(
            required(object, "center", &format!("{path}.center"))?,
            &format!("{path}.center"),
        )?,
        normalized_rect(
            required(object, "bounds", &format!("{path}.bounds"))?,
            &format!("{path}.bounds"),
        )?,
    ))
}

fn require_rect_inside(
    inner: SpatialRect,
    outer: SpatialRect,
    inner_path: &str,
    outer_path: &str,
) -> Result<(), String> {
    if inner.left < outer.left
        || inner.top < outer.top
        || inner.right > outer.right
        || inner.bottom > outer.bottom
    {
        return Err(format!(
            "invalid MotionSpatialProfileV1: {inner_path} must remain inside {outer_path}"
        ));
    }
    Ok(())
}

fn require_point_inside(
    point: SpatialPoint,
    rect: SpatialRect,
    point_path: &str,
    rect_path: &str,
) -> Result<(), String> {
    if point.x < rect.left || point.x > rect.right || point.y < rect.top || point.y > rect.bottom {
        return Err(format!(
            "invalid MotionSpatialProfileV1: {point_path} must remain inside {rect_path}"
        ));
    }
    Ok(())
}

fn positive_area_overlap(left: SpatialRect, right: SpatialRect) -> bool {
    left.right.min(right.right) > left.left.max(right.left)
        && left.bottom.min(right.bottom) > left.top.max(right.top)
}

#[cfg(test)]
mod tests {
    use super::{
        parse_cat_character_manifest, parse_cat_spatial_manifest, parse_motion_spatial_profile_v1,
    };
    use crate::runtime_assets::manifest::{parse_manifest, RuntimeAssetManifest};

    fn valid_manifest() -> serde_json::Value {
        let motions = [
            "breathing",
            "blink",
            "ear-twitch",
            "tail-idle",
            "pointer-focus",
            "pet-happy",
            "sleepy-yawn",
            "half-stand-stretch",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            (
                name.to_string(),
                serde_json::json!({ "group": "CatMotion", "index": index }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
        let parameters = [
            "eyeOpenLeft",
            "eyeOpenRight",
            "eyeBallX",
            "eyeBallY",
            "earLeft",
            "earRight",
            "tailAngle",
            "tailCurl",
            "tailTip",
            "bodyBreath",
            "bodyStretch",
            "mouthOpen",
        ]
        .into_iter()
        .map(|name| {
            (
                name.to_string(),
                serde_json::Value::String(format!("Param{name}")),
            )
        })
        .collect::<serde_json::Map<_, _>>();
        serde_json::json!({
            "schemaVersion": 4,
            "renderer": "cat-live2d-v1",
            "petId": "cat-a-standard-v1",
            "variantId": "standard-v1",
            "skeletonVersion": "cat-a-live2d-v1",
            "modelEntry": "model/cat.model3.json",
            "previewImage": "preview/cat.png",
            "files": [
                { "role": "model", "relativePath": "model/cat.model3.json", "sha256": "AB".repeat(32) },
                { "role": "preview", "relativePath": "preview/cat.png", "sha256": "CD".repeat(32) }
            ],
            "motions": motions,
            "parameters": parameters,
            "hitAreas": { "body": "HitAreaBody", "edgeTail": "HitAreaEdgeTail" },
            "edgeTailStates": {
                "left": { "group": "EdgeTail", "index": 0, "tailArtMesh": "ArtMeshTail" },
                "right": { "group": "EdgeTail", "index": 1, "tailArtMesh": "ArtMeshTail" },
                "top": { "group": "EdgeTail", "index": 2, "tailArtMesh": "ArtMeshTail" },
                "bottom": { "group": "EdgeTail", "index": 3, "tailArtMesh": "ArtMeshTail" }
            },
            "license": {
                "id": "project-owned", "author": "PetBaby", "source": "project",
                "commercialUse": true, "redistributable": true
            }
        })
    }

    #[test]
    fn parses_and_normalizes_a_valid_v4_manifest() {
        let value = valid_manifest();
        let manifest = parse_cat_character_manifest(value.clone()).unwrap();
        assert_eq!(manifest.skeleton_version, "cat-a-live2d-v1");
        assert_eq!(manifest.files[0].sha256, "ab".repeat(32));
        assert_eq!(manifest.motions.len(), 8);
        assert_eq!(manifest.edge_tail_states.len(), 4);
        assert!(matches!(
            parse_manifest(&value.to_string()).unwrap(),
            RuntimeAssetManifest::V4(_)
        ));
    }

    #[test]
    fn rejects_missing_required_semantics_and_split_tail_meshes() {
        for (group, key) in [
            ("motions", "blink"),
            ("parameters", "tailTip"),
            ("hitAreas", "edgeTail"),
            ("edgeTailStates", "left"),
        ] {
            let mut value = valid_manifest();
            value[group].as_object_mut().unwrap().remove(key);
            assert!(parse_cat_character_manifest(value)
                .unwrap_err()
                .contains(key));
        }
        let mut value = valid_manifest();
        value["edgeTailStates"]["right"]["tailArtMesh"] = "ArtMeshScreenshot".into();
        assert!(parse_cat_character_manifest(value)
            .unwrap_err()
            .contains("same tail ArtMesh"));
    }

    #[test]
    fn rejects_unknown_semantics_unsafe_files_and_non_redistributable_license() {
        let mut unknown = valid_manifest();
        unknown["motions"]["dance"] = serde_json::json!({ "group": "Dance" });
        assert!(parse_cat_character_manifest(unknown)
            .unwrap_err()
            .contains("unknown"));

        let mut traversal = valid_manifest();
        traversal["modelEntry"] = "../cat.model3.json".into();
        assert!(parse_cat_character_manifest(traversal)
            .unwrap_err()
            .contains("unsafe"));

        let mut duplicate = valid_manifest();
        duplicate["files"][1]["relativePath"] = "MODEL\\CAT.MODEL3.JSON".into();
        assert!(parse_cat_character_manifest(duplicate)
            .unwrap_err()
            .contains("duplicate"));

        let mut license = valid_manifest();
        license["license"]["redistributable"] = false.into();
        assert!(parse_cat_character_manifest(license)
            .unwrap_err()
            .contains("redistributable"));
    }

    #[test]
    fn parses_a_spatially_calibrated_v5_manifest() {
        let mut value = valid_manifest();
        value["schemaVersion"] = 5.into();
        value["renderer"] = "cat-spatial-live2d-v1".into();
        value["bodyModuleId"] = "body-balanced-v1".into();
        value["motionSpatialProfile"] = "profiles/body-balanced.json".into();
        value["files"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "motion-spatial-profile",
                "relativePath": "profiles/body-balanced.json",
                "sha256": "EF".repeat(32),
            }));

        let manifest = parse_cat_spatial_manifest(value.clone()).unwrap();
        assert_eq!(manifest.body_module_id, "body-balanced-v1");
        assert_eq!(
            manifest.motion_spatial_profile,
            "profiles/body-balanced.json"
        );
        let parsed = parse_manifest(&value.to_string()).unwrap();
        let RuntimeAssetManifest::V5(parsed) = parsed else {
            panic!("expected v5 manifest")
        };
        let serialized = serde_json::to_value(parsed).unwrap();
        assert_eq!(serialized["bodyModuleId"], "body-balanced-v1");
        assert_eq!(
            serialized["motionSpatialProfile"],
            "profiles/body-balanced.json"
        );
    }

    #[test]
    fn rejects_invalid_v5_module_profile_path_and_profile_file() {
        let mut unknown_module = valid_manifest();
        unknown_module["schemaVersion"] = 5.into();
        unknown_module["renderer"] = "cat-spatial-live2d-v1".into();
        unknown_module["bodyModuleId"] = "body-unknown-v1".into();
        unknown_module["motionSpatialProfile"] = "profiles/body-balanced.json".into();
        unknown_module["files"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "motion-spatial-profile",
                "relativePath": "profiles/body-balanced.json",
                "sha256": "EF".repeat(32),
            }));
        assert!(parse_manifest(&unknown_module.to_string())
            .unwrap_err()
            .contains("bodyModuleId"));

        let mut traversal = valid_manifest();
        traversal["schemaVersion"] = 5.into();
        traversal["renderer"] = "cat-spatial-live2d-v1".into();
        traversal["bodyModuleId"] = "body-balanced-v1".into();
        traversal["motionSpatialProfile"] = "../profiles/body-balanced.json".into();
        assert!(parse_manifest(&traversal.to_string())
            .unwrap_err()
            .contains("unsafe"));

        let mut unlisted = valid_manifest();
        unlisted["schemaVersion"] = 5.into();
        unlisted["renderer"] = "cat-spatial-live2d-v1".into();
        unlisted["bodyModuleId"] = "body-balanced-v1".into();
        unlisted["motionSpatialProfile"] = "profiles/body-balanced.json".into();
        assert!(parse_manifest(&unlisted.to_string())
            .unwrap_err()
            .contains("motionSpatialProfile"));

        let mut bad_hash = valid_manifest();
        bad_hash["schemaVersion"] = 5.into();
        bad_hash["renderer"] = "cat-spatial-live2d-v1".into();
        bad_hash["bodyModuleId"] = "body-balanced-v1".into();
        bad_hash["motionSpatialProfile"] = "profiles/body-balanced.json".into();
        bad_hash["files"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "motion-spatial-profile",
                "relativePath": "profiles/body-balanced.json",
                "sha256": "invalid",
            }));
        assert!(parse_manifest(&bad_hash.to_string())
            .unwrap_err()
            .contains("sha256"));
    }

    #[test]
    fn rejects_invalid_spatial_profile_geometry() {
        let profile = serde_json::json!({
            "schemaVersion": 1,
            "bodyModuleId": "body-balanced-v1",
            "canvas": { "width": 1000, "height": 1200 },
            "alphaBounds": { "left": 0.1, "top": 0.05, "right": 0.1, "bottom": 0.95 },
            "faceSafeZone": { "left": 0.25, "top": 0.1, "right": 0.75, "bottom": 0.4 },
            "eyes": {
                "left": { "center": { "x": 0.38, "y": 0.25 }, "bounds": { "left": 0.32, "top": 0.18, "right": 0.44, "bottom": 0.32 } },
                "right": { "center": { "x": 0.62, "y": 0.25 }, "bounds": { "left": 0.56, "top": 0.18, "right": 0.68, "bottom": 0.32 } }
            },
            "earRoots": { "left": { "x": 0.24, "y": 0.13 }, "right": { "x": 0.76, "y": 0.13 } },
            "breathZone": { "left": 0.3, "top": 0.45, "right": 0.7, "bottom": 0.75 },
            "stretchAxis": { "origin": { "x": 0.5, "y": 0.65 }, "direction": { "x": 0.0, "y": 1.0 } },
            "swayPivot": { "x": 0.5, "y": 0.7 },
            "tailRoot": { "x": 0.78, "y": 0.72 },
            "edgeTailBounds": { "left": 0.74, "top": 0.55, "right": 0.9, "bottom": 0.9 },
            "amplitude": {
                "breath": { "min": 0.0, "max": 1.0 }, "blink": { "min": 0.0, "max": 1.0 },
                "ear": { "min": -0.35, "max": 0.35 }, "tailAngle": { "min": -20.0, "max": 20.0 },
                "tailCurl": { "min": -0.6, "max": 0.6 }, "tailTip": { "min": -0.7, "max": 0.7 },
                "bodyStretch": { "min": 0.0, "max": 1.0 }
            }
        });

        assert!(
            parse_motion_spatial_profile_v1(&profile.to_string(), "body-balanced-v1")
                .unwrap_err()
                .contains("alphaBounds")
        );
    }
}
