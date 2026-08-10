use crate::creation::content::ContentRoot;
use crate::creation::domain::ComposerRecipe;
use crate::runtime_assets::manifest::normalize_relative_path;
use image::{ColorType, GenericImageView, ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};

fn deserialize_present_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

fn deserialize_required_nullable_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerRect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerPart {
    pub id: String,
    pub image: String,
    #[serde(
        default,
        deserialize_with = "deserialize_present_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub color_mask: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub pattern_mask: Option<String>,
    pub compatible_body_ids: Vec<String>,
    pub anchor: ComposerPoint,
    pub z_index: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerEyePart {
    pub id: String,
    pub open_image: String,
    pub closed_image: String,
    #[serde(
        default,
        deserialize_with = "deserialize_present_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub color_mask: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub pattern_mask: Option<String>,
    pub compatible_body_ids: Vec<String>,
    pub anchor: ComposerPoint,
    pub z_index: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerDefaults {
    pub ears_id: String,
    pub eyes_id: String,
    pub muzzle_id: String,
    pub tail_id: String,
    pub color_id: String,
    pub pattern_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerBodyPart {
    pub id: String,
    pub image: String,
    #[serde(
        default,
        deserialize_with = "deserialize_present_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub color_mask: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub pattern_mask: Option<String>,
    pub compatible_body_ids: Vec<String>,
    pub anchor: ComposerPoint,
    pub z_index: i32,
    pub defaults: ComposerDefaults,
    pub alpha_bounds: ComposerRect,
    pub face_safe_zone: ComposerRect,
    pub breath_zone: ComposerRect,
    pub sway_pivot: ComposerPoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerColor {
    pub id: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerPattern {
    pub id: String,
    #[serde(deserialize_with = "deserialize_required_nullable_string")]
    pub image: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerCanvas {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerPackManifest {
    pub schema_version: u32,
    pub pack_id: String,
    pub pack_version: u32,
    pub species: String,
    pub canvas: ComposerCanvas,
    pub layer_contract_version: u32,
    pub bodies: Vec<ComposerBodyPart>,
    pub ears: Vec<ComposerPart>,
    pub eyes: Vec<ComposerEyePart>,
    pub muzzles: Vec<ComposerPart>,
    pub tails: Vec<ComposerPart>,
    pub colors: Vec<ComposerColor>,
    pub patterns: Vec<ComposerPattern>,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn validate_id(value: &str, label: &str) -> Result<(), String> {
    if valid_id(value) {
        Ok(())
    } else {
        Err(format!("{label} is an invalid ID"))
    }
}

fn validate_point(value: &ComposerPoint, label: &str) -> Result<(), String> {
    if value.x.is_finite()
        && value.y.is_finite()
        && (0.0..=1024.0).contains(&value.x)
        && (0.0..=1024.0).contains(&value.y)
    {
        Ok(())
    } else {
        Err(format!("{label} must be a finite canvas point"))
    }
}

fn validate_rect(value: &ComposerRect, label: &str) -> Result<(), String> {
    let coordinates = [value.left, value.top, value.right, value.bottom];
    if coordinates
        .iter()
        .all(|coordinate| coordinate.is_finite() && (0.0..=1024.0).contains(coordinate))
        && value.left < value.right
        && value.top < value.bottom
    {
        Ok(())
    } else {
        Err(format!(
            "{label} must have finite canvas bounds and positive area"
        ))
    }
}

fn ensure_unique(values: &[String], label: &str) -> Result<(), String> {
    if values.is_empty() {
        return Err(format!("{label} must be non-empty"));
    }
    let mut seen = HashSet::new();
    for value in values {
        validate_id(value, label)?;
        if !seen.insert(value.as_str()) {
            return Err(format!("{label} contains duplicate ID: {value}"));
        }
    }
    Ok(())
}

trait CompatiblePart {
    fn id(&self) -> &str;
    fn compatible_body_ids(&self) -> &[String];
}

impl CompatiblePart for ComposerPart {
    fn id(&self) -> &str {
        &self.id
    }
    fn compatible_body_ids(&self) -> &[String] {
        &self.compatible_body_ids
    }
}

impl CompatiblePart for ComposerEyePart {
    fn id(&self) -> &str {
        &self.id
    }
    fn compatible_body_ids(&self) -> &[String] {
        &self.compatible_body_ids
    }
}

fn validate_compatible_parts<T: CompatiblePart>(
    items: &[T],
    label: &str,
    body_ids: &HashSet<&str>,
) -> Result<(), String> {
    if items.is_empty() {
        return Err(format!("{label} must be non-empty"));
    }
    for item in items {
        validate_id(item.id(), &format!("{label} ID"))?;
        ensure_unique(
            item.compatible_body_ids(),
            &format!("{}.compatibleBodyIds", item.id()),
        )?;
        for body_id in item.compatible_body_ids() {
            if !body_ids.contains(body_id.as_str()) {
                return Err(format!("{} references unknown body: {body_id}", item.id()));
            }
        }
    }
    Ok(())
}

fn item_is_compatible<T: CompatiblePart>(item: &T, body_id: &str) -> bool {
    item.compatible_body_ids()
        .iter()
        .any(|candidate| candidate == body_id)
}

fn validate_declared_image_path(relative_path: &str) -> Result<(), String> {
    let normalized = normalize_relative_path(relative_path)?;
    if normalized != relative_path || relative_path.contains('%') {
        return Err(format!(
            "asset path must be a canonical relative path: {relative_path}"
        ));
    }
    if !relative_path.to_ascii_lowercase().ends_with(".png") {
        return Err(format!("asset path must reference PNG: {relative_path}"));
    }
    Ok(())
}

fn validate_manifest(pack: &ComposerPackManifest) -> Result<(), String> {
    if pack.schema_version != 1 {
        return Err("schemaVersion must be 1".into());
    }
    validate_id(&pack.pack_id, "packId")?;
    if pack.pack_version == 0 {
        return Err("packVersion must be positive".into());
    }
    if pack.species != "cat" {
        return Err("species must be cat".into());
    }
    if pack.canvas.width != 1024 || pack.canvas.height != 1024 {
        return Err("canvas must be 1024x1024".into());
    }
    if pack.layer_contract_version != 1 {
        return Err("layerContractVersion must be 1".into());
    }
    if pack.bodies.is_empty()
        || pack.ears.is_empty()
        || pack.eyes.is_empty()
        || pack.muzzles.is_empty()
        || pack.tails.is_empty()
        || pack.colors.is_empty()
        || pack.patterns.is_empty()
    {
        return Err("composer pack arrays must be non-empty".into());
    }

    let mut all_ids = HashSet::new();
    for id in pack
        .bodies
        .iter()
        .map(|item| item.id.as_str())
        .chain(pack.ears.iter().map(|item| item.id.as_str()))
        .chain(pack.eyes.iter().map(|item| item.id.as_str()))
        .chain(pack.muzzles.iter().map(|item| item.id.as_str()))
        .chain(pack.tails.iter().map(|item| item.id.as_str()))
        .chain(pack.colors.iter().map(|item| item.id.as_str()))
        .chain(pack.patterns.iter().map(|item| item.id.as_str()))
    {
        validate_id(id, "item ID")?;
        if !all_ids.insert(id) {
            return Err(format!("duplicate ID: {id}"));
        }
    }
    let body_ids = pack
        .bodies
        .iter()
        .map(|item| item.id.as_str())
        .collect::<HashSet<_>>();
    validate_compatible_parts(&pack.ears, "ears", &body_ids)?;
    validate_compatible_parts(&pack.eyes, "eyes", &body_ids)?;
    validate_compatible_parts(&pack.muzzles, "muzzles", &body_ids)?;
    validate_compatible_parts(&pack.tails, "tails", &body_ids)?;

    for body in &pack.bodies {
        ensure_unique(
            &body.compatible_body_ids,
            &format!("{}.compatibleBodyIds", body.id),
        )?;
        if !body.compatible_body_ids.iter().any(|id| id == &body.id) {
            return Err(format!("{} is not compatible with itself", body.id));
        }
        for compatible in &body.compatible_body_ids {
            if !body_ids.contains(compatible.as_str()) {
                return Err(format!("{} references unknown body: {compatible}", body.id));
            }
        }
        validate_point(&body.anchor, &format!("{}.anchor", body.id))?;
        validate_point(&body.sway_pivot, &format!("{}.swayPivot", body.id))?;
        validate_rect(&body.alpha_bounds, &format!("{}.alphaBounds", body.id))?;
        validate_rect(&body.face_safe_zone, &format!("{}.faceSafeZone", body.id))?;
        validate_rect(&body.breath_zone, &format!("{}.breathZone", body.id))?;
    }
    for item in &pack.ears {
        validate_point(&item.anchor, &format!("{}.anchor", item.id))?;
    }
    for item in &pack.eyes {
        validate_point(&item.anchor, &format!("{}.anchor", item.id))?;
    }
    for item in &pack.muzzles {
        validate_point(&item.anchor, &format!("{}.anchor", item.id))?;
    }
    for item in &pack.tails {
        validate_point(&item.anchor, &format!("{}.anchor", item.id))?;
    }
    for color in &pack.colors {
        if color.value.len() != 7
            || !color.value.starts_with('#')
            || !color.value[1..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!("{} value must be #RRGGBB", color.id));
        }
    }
    for pattern in &pack.patterns {
        if (pattern.id == "pattern-none") != pattern.image.is_none() {
            return Err(format!("{} has invalid image null semantics", pattern.id));
        }
    }
    if !pack
        .patterns
        .iter()
        .any(|pattern| pattern.id == "pattern-none")
    {
        return Err("patterns must declare pattern-none with a null image".into());
    }
    for relative_path in asset_paths(pack) {
        validate_declared_image_path(relative_path)?;
    }

    let ears = pack
        .ears
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    let eyes = pack
        .eyes
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    let muzzles = pack
        .muzzles
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    let tails = pack
        .tails
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    let colors = pack
        .colors
        .iter()
        .map(|item| item.id.as_str())
        .collect::<HashSet<_>>();
    let patterns = pack
        .patterns
        .iter()
        .map(|item| item.id.as_str())
        .collect::<HashSet<_>>();
    for body in &pack.bodies {
        let defaults = &body.defaults;
        let ears_item = ears
            .get(defaults.ears_id.as_str())
            .ok_or_else(|| format!("{} defaults reference unknown earsId", body.id))?;
        let eyes_item = eyes
            .get(defaults.eyes_id.as_str())
            .ok_or_else(|| format!("{} defaults reference unknown eyesId", body.id))?;
        let muzzle_item = muzzles
            .get(defaults.muzzle_id.as_str())
            .ok_or_else(|| format!("{} defaults reference unknown muzzleId", body.id))?;
        let tail_item = tails
            .get(defaults.tail_id.as_str())
            .ok_or_else(|| format!("{} defaults reference unknown tailId", body.id))?;
        for (label, compatible) in [
            ("earsId", item_is_compatible(*ears_item, &body.id)),
            ("eyesId", item_is_compatible(*eyes_item, &body.id)),
            ("muzzleId", item_is_compatible(*muzzle_item, &body.id)),
            ("tailId", item_is_compatible(*tail_item, &body.id)),
        ] {
            if !compatible {
                return Err(format!("{} defaults select incompatible {label}", body.id));
            }
        }
        if !colors.contains(defaults.color_id.as_str()) {
            return Err(format!("{} defaults reference unknown colorId", body.id));
        }
        if !patterns.contains(defaults.pattern_id.as_str()) {
            return Err(format!("{} defaults reference unknown patternId", body.id));
        }
    }
    Ok(())
}

pub fn parse_pack(json: &str) -> Result<ComposerPackManifest, String> {
    let pack = serde_json::from_str::<ComposerPackManifest>(json)
        .map_err(|error| format!("invalid composer pack JSON: {error}"))?;
    validate_manifest(&pack)?;
    Ok(pack)
}

fn asset_paths(pack: &ComposerPackManifest) -> Vec<&str> {
    let mut paths = Vec::new();
    for body in &pack.bodies {
        paths.push(body.image.as_str());
        paths.extend(body.color_mask.iter().map(String::as_str));
        paths.extend(body.pattern_mask.iter().map(String::as_str));
    }
    for item in pack
        .ears
        .iter()
        .chain(pack.muzzles.iter())
        .chain(pack.tails.iter())
    {
        paths.push(item.image.as_str());
        paths.extend(item.color_mask.iter().map(String::as_str));
        paths.extend(item.pattern_mask.iter().map(String::as_str));
    }
    for eye in &pack.eyes {
        paths.push(eye.open_image.as_str());
        paths.push(eye.closed_image.as_str());
        paths.extend(eye.color_mask.iter().map(String::as_str));
        paths.extend(eye.pattern_mask.iter().map(String::as_str));
    }
    paths.extend(
        pack.patterns
            .iter()
            .filter_map(|pattern| pattern.image.as_deref()),
    );
    paths.sort_unstable();
    paths.dedup();
    paths
}

fn checked_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("{label} is unavailable: {error}"))?;
    if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(format!("{label} cannot be a link or reparse point"));
    }
    path.canonicalize()
        .map_err(|error| format!("{label} cannot be canonicalized: {error}"))
}

fn checked_asset_path(pack_root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    validate_declared_image_path(relative_path)?;
    let mut cursor = pack_root.to_path_buf();
    let components = Path::new(relative_path).components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(format!("asset path must be relative: {relative_path}"));
        };
        cursor.push(component);
        let metadata = std::fs::symlink_metadata(&cursor)
            .map_err(|error| format!("asset {relative_path} is unavailable: {error}"))?;
        if crate::platform::is_link_or_reparse_point(&metadata) {
            return Err(format!(
                "asset {relative_path} cannot contain a link or reparse point"
            ));
        }
        if index + 1 == components.len() {
            if !metadata.is_file() {
                return Err(format!("asset {relative_path} must be a regular file"));
            }
        } else if !metadata.is_dir() {
            return Err(format!("asset {relative_path} has a non-directory parent"));
        }
    }
    let canonical = cursor
        .canonicalize()
        .map_err(|error| format!("asset {relative_path} cannot be canonicalized: {error}"))?;
    if !canonical.starts_with(pack_root) {
        return Err(format!("asset {relative_path} escapes the composer pack"));
    }
    Ok(canonical)
}

fn validate_png_asset(pack_root: &Path, relative_path: &str) -> Result<(), String> {
    let canonical_before = checked_asset_path(pack_root, relative_path)?;
    let file = std::fs::File::open(&canonical_before)
        .map_err(|error| format!("asset {relative_path} cannot be opened: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("asset {relative_path} metadata failed: {error}"))?;
    if !metadata.is_file() || metadata.len() > 32 * 1024 * 1024 {
        return Err(format!(
            "asset {relative_path} is not a bounded regular file"
        ));
    }
    let canonical_after_open = checked_asset_path(pack_root, relative_path)?;
    if canonical_after_open != canonical_before {
        return Err(format!("asset {relative_path} changed while opening"));
    }

    let mut reader = ImageReader::with_format(BufReader::new(file), ImageFormat::Png);
    let mut limits = Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    limits.max_alloc = Some(8 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| format!("asset {relative_path} is not a complete PNG: {error}"))?;
    if image.dimensions() != (1024, 1024) || image.color() != ColorType::Rgba8 {
        return Err(format!("asset {relative_path} must be 1024x1024 RGBA"));
    }
    let canonical_after_decode = checked_asset_path(pack_root, relative_path)?;
    if canonical_after_decode != canonical_before {
        return Err(format!("asset {relative_path} changed while decoding"));
    }
    Ok(())
}

pub fn validate_pack(
    pack: &ComposerPackManifest,
    content_root: &ContentRoot,
) -> Result<(), String> {
    validate_manifest(pack)?;
    let content_root = content_root.as_path();
    let canonical_content = checked_directory(content_root, "content root")?;
    let canonical_composer = checked_directory(&content_root.join("composer"), "composer root")?;
    if canonical_composer.parent() != Some(canonical_content.as_path()) {
        return Err("composer root escapes the content root".into());
    }
    let pack_path = content_root.join("composer").join(&pack.pack_id);
    let canonical_pack = checked_directory(&pack_path, "composer pack root")?;
    if canonical_pack.parent() != Some(canonical_composer.as_path()) {
        return Err("composer pack root escapes the content root".into());
    }
    for relative_path in asset_paths(pack) {
        validate_png_asset(&canonical_pack, relative_path)?;
    }
    Ok(())
}

pub fn validate_recipe(pack: &ComposerPackManifest, recipe: &ComposerRecipe) -> Result<(), String> {
    validate_manifest(pack)?;
    let mut errors = Vec::new();
    if recipe.recipe_version != 1 {
        errors.push("recipeVersion must be 1".to_string());
    }
    if recipe.pack_id != pack.pack_id {
        errors.push(format!("packId must match {}", pack.pack_id));
    }
    if recipe.pack_version != pack.pack_version {
        errors.push(format!("packVersion must match {}", pack.pack_version));
    }
    if recipe.layer_contract_version != pack.layer_contract_version {
        errors.push(format!(
            "layerContractVersion must match {}",
            pack.layer_contract_version
        ));
    }
    let body = pack.bodies.iter().find(|item| item.id == recipe.body_id);
    if body.is_none() {
        errors.push(format!("bodyId does not exist: {}", recipe.body_id));
    }
    macro_rules! check_part {
        ($field:literal, $selected:expr, $items:expr) => {{
            match $items.iter().find(|item| item.id == *$selected) {
                None => errors.push(format!("{} does not exist: {}", $field, $selected)),
                Some(item) => {
                    if let Some(body) = body {
                        if !item.compatible_body_ids.iter().any(|id| id == &body.id) {
                            errors.push(format!(
                                "{} is incompatible with bodyId {}: {}",
                                $field, body.id, $selected
                            ));
                        }
                    }
                }
            }
        }};
    }
    check_part!("earsId", &recipe.ears_id, pack.ears);
    check_part!("eyesId", &recipe.eyes_id, pack.eyes);
    check_part!("muzzleId", &recipe.muzzle_id, pack.muzzles);
    check_part!("tailId", &recipe.tail_id, pack.tails);
    if !pack.colors.iter().any(|item| item.id == recipe.color_id) {
        errors.push(format!("colorId does not exist: {}", recipe.color_id));
    }
    if !pack
        .patterns
        .iter()
        .any(|item| item.id == recipe.pattern_id)
    {
        errors.push(format!("patternId does not exist: {}", recipe.pattern_id));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::domain::ComposerRecipe;
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NONCE: AtomicU64 = AtomicU64::new(0);

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-composer-{label}-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn valid_pack_value() -> Value {
        json!({
            "schemaVersion": 1,
            "packId": "cat-cute-v1",
            "packVersion": 1,
            "species": "cat",
            "canvas": { "width": 1024, "height": 1024 },
            "layerContractVersion": 1,
            "bodies": [{
                "id": "body-round", "image": "parts/body.png",
                "colorMask": "parts/body-color.png", "patternMask": "parts/body-pattern.png",
                "compatibleBodyIds": ["body-round"], "anchor": { "x": 512.0, "y": 512.0 },
                "zIndex": 10,
                "defaults": { "earsId": "ears-round", "eyesId": "eyes-amber", "muzzleId": "muzzle-soft", "tailId": "tail-curl", "colorId": "color-cream", "patternId": "pattern-none" },
                "alphaBounds": { "left": 100.0, "top": 50.0, "right": 900.0, "bottom": 1000.0 },
                "faceSafeZone": { "left": 300.0, "top": 160.0, "right": 720.0, "bottom": 500.0 },
                "breathZone": { "left": 260.0, "top": 500.0, "right": 760.0, "bottom": 900.0 },
                "swayPivot": { "x": 512.0, "y": 780.0 }
            }, {
                "id": "body-other", "image": "parts/body-other.png",
                "compatibleBodyIds": ["body-other"], "anchor": { "x": 512.0, "y": 512.0 },
                "zIndex": 10,
                "defaults": { "earsId": "ears-other", "eyesId": "eyes-amber", "muzzleId": "muzzle-soft", "tailId": "tail-curl", "colorId": "color-cream", "patternId": "pattern-none" },
                "alphaBounds": { "left": 100.0, "top": 50.0, "right": 900.0, "bottom": 1000.0 },
                "faceSafeZone": { "left": 300.0, "top": 160.0, "right": 720.0, "bottom": 500.0 },
                "breathZone": { "left": 260.0, "top": 500.0, "right": 760.0, "bottom": 900.0 },
                "swayPivot": { "x": 512.0, "y": 780.0 }
            }],
            "ears": [
                { "id": "ears-round", "image": "parts/ears-round.png", "compatibleBodyIds": ["body-round"], "anchor": { "x": 512.0, "y": 230.0 }, "zIndex": 20 },
                { "id": "ears-other", "image": "parts/ears-other.png", "compatibleBodyIds": ["body-other"], "anchor": { "x": 512.0, "y": 230.0 }, "zIndex": 20 }
            ],
            "eyes": [{ "id": "eyes-amber", "openImage": "parts/eyes-open.png", "closedImage": "parts/eyes-closed.png", "compatibleBodyIds": ["body-round", "body-other"], "anchor": { "x": 512.0, "y": 340.0 }, "zIndex": 30 }],
            "muzzles": [{ "id": "muzzle-soft", "image": "parts/muzzle.png", "compatibleBodyIds": ["body-round", "body-other"], "anchor": { "x": 512.0, "y": 430.0 }, "zIndex": 40 }],
            "tails": [{ "id": "tail-curl", "image": "parts/tail.png", "colorMask": "parts/tail-color.png", "patternMask": "parts/tail-pattern.png", "compatibleBodyIds": ["body-round", "body-other"], "anchor": { "x": 700.0, "y": 650.0 }, "zIndex": 0 }],
            "colors": [{ "id": "color-cream", "value": "#F4D6A0" }],
            "patterns": [
                { "id": "pattern-none", "image": null },
                { "id": "pattern-tabby", "image": "patterns/tabby.png" }
            ]
        })
    }

    fn valid_recipe() -> ComposerRecipe {
        ComposerRecipe {
            recipe_version: 1,
            pack_id: "cat-cute-v1".into(),
            pack_version: 1,
            layer_contract_version: 1,
            body_id: "body-round".into(),
            ears_id: "ears-round".into(),
            eyes_id: "eyes-amber".into(),
            muzzle_id: "muzzle-soft".into(),
            tail_id: "tail-curl".into(),
            color_id: "color-cream".into(),
            pattern_id: "pattern-none".into(),
        }
    }

    fn asset_paths(value: &Value) -> Vec<String> {
        let mut paths = Vec::new();
        for category in ["bodies", "ears", "muzzles", "tails"] {
            for item in value[category].as_array().unwrap() {
                for key in ["image", "colorMask", "patternMask"] {
                    if let Some(path) = item.get(key).and_then(Value::as_str) {
                        paths.push(path.to_string());
                    }
                }
            }
        }
        for item in value["eyes"].as_array().unwrap() {
            paths.push(item["openImage"].as_str().unwrap().to_string());
            paths.push(item["closedImage"].as_str().unwrap().to_string());
            for key in ["colorMask", "patternMask"] {
                if let Some(path) = item.get(key).and_then(Value::as_str) {
                    paths.push(path.to_string());
                }
            }
        }
        for item in value["patterns"].as_array().unwrap() {
            if let Some(path) = item["image"].as_str() {
                paths.push(path.to_string());
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }

    fn write_rgba_png(path: &Path, width: u32, height: u32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbaImage::new(width, height).save(path).unwrap();
    }

    fn fixture(value: &Value) -> (PathBuf, PathBuf) {
        let content = temp_root("fixture");
        let pack_root = content.join("composer").join("cat-cute-v1");
        let paths = asset_paths(value);
        let source = pack_root.join(&paths[0]);
        write_rgba_png(&source, 1024, 1024);
        for path in paths.into_iter().skip(1) {
            let destination = pack_root.join(path);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::hard_link(&source, destination).unwrap();
        }
        (content, pack_root)
    }

    fn validate_fixture_pack(pack: &ComposerPackManifest, content: &Path) -> Result<(), String> {
        let root = crate::creation::content::test_content_root(content)?;
        validate_pack(pack, &root)
    }

    #[test]
    fn accepts_a_complete_cat_pack_and_recipe() {
        let value = valid_pack_value();
        let (content, _) = fixture(&value);
        let pack = parse_pack(&value.to_string()).unwrap();
        validate_fixture_pack(&pack, &content).unwrap();
        validate_recipe(&pack, &valid_recipe()).unwrap();
        std::fs::remove_dir_all(content).unwrap();
    }

    #[test]
    fn rejects_unknown_fields_and_invalid_fixed_contract_values() {
        for (path, invalid) in [
            ("extra", json!(true)),
            ("schemaVersion", json!(2)),
            ("species", json!("dog")),
            ("packVersion", json!(0)),
            ("layerContractVersion", json!(2)),
        ] {
            let mut value = valid_pack_value();
            value[path] = invalid;
            assert!(parse_pack(&value.to_string()).is_err(), "accepted {path}");
        }
        let mut value = valid_pack_value();
        value["canvas"]["width"] = json!(512);
        assert!(parse_pack(&value.to_string()).is_err());
        let mut value = valid_pack_value();
        value["bodies"][0]["anchor"]["unexpected"] = json!(1);
        assert!(parse_pack(&value.to_string()).is_err());
    }

    #[test]
    fn rejects_empty_invalid_duplicate_and_unknown_ids() {
        let mutations: Vec<Box<dyn Fn(&mut Value)>> = vec![
            Box::new(|v| v["ears"] = json!([])),
            Box::new(|v| v["ears"][0]["id"] = json!("")),
            Box::new(|v| v["ears"][0]["id"] = json!("Bad ID")),
            Box::new(|v| v["ears"][0]["id"] = v["bodies"][0]["id"].clone()),
            Box::new(|v| v["ears"][0]["compatibleBodyIds"] = json!(["missing-body"])),
            Box::new(|v| v["bodies"][0]["defaults"]["earsId"] = json!("missing-ears")),
        ];
        for mutate in mutations {
            let mut value = valid_pack_value();
            mutate(&mut value);
            assert!(parse_pack(&value.to_string()).is_err());
        }
    }

    #[test]
    fn rejects_invalid_geometry_color_pattern_and_default_compatibility() {
        let mutations: Vec<Box<dyn Fn(&mut Value)>> = vec![
            Box::new(|v| v["bodies"][0]["alphaBounds"]["right"] = json!(99)),
            Box::new(|v| v["bodies"][0]["faceSafeZone"]["left"] = json!(-1)),
            Box::new(|v| v["bodies"][0]["breathZone"]["bottom"] = json!(1025)),
            Box::new(|v| v["bodies"][0]["swayPivot"]["x"] = json!("NaN")),
            Box::new(|v| v["ears"][0]["anchor"]["y"] = json!(-0.5)),
            Box::new(|v| v["colors"][0]["value"] = json!("cream")),
            Box::new(|v| v["patterns"][0]["image"] = json!("patterns/none.png")),
            Box::new(|v| v["patterns"][1]["image"] = Value::Null),
            Box::new(|v| v["bodies"][0]["colorMask"] = Value::Null),
            Box::new(|v| v["ears"][0]["compatibleBodyIds"] = json!(["body-other"])),
        ];
        for mutate in mutations {
            let mut value = valid_pack_value();
            mutate(&mut value);
            assert!(parse_pack(&value.to_string()).is_err());
        }
    }

    #[test]
    fn pattern_image_is_required_but_explicit_null_means_no_pattern() {
        let mut missing = valid_pack_value();
        missing["patterns"][0]
            .as_object_mut()
            .unwrap()
            .remove("image");
        assert!(parse_pack(&missing.to_string()).is_err());

        let pack = parse_pack(&valid_pack_value().to_string()).unwrap();
        assert_eq!(pack.patterns[0].id, "pattern-none");
        assert_eq!(pack.patterns[0].image, None);
        assert_eq!(
            pack.patterns[1].image.as_deref(),
            Some("patterns/tabby.png")
        );
    }

    #[test]
    fn rejects_missing_eye_assets_and_unsafe_or_missing_files() {
        let mut missing_closed = valid_pack_value();
        missing_closed["eyes"][0]
            .as_object_mut()
            .unwrap()
            .remove("closedImage");
        assert!(parse_pack(&missing_closed.to_string()).is_err());

        for path in [
            "../secret.png",
            "/secret.png",
            "C:/secret.png",
            "parts/%2e%2e/body.png",
            "parts\\body.png",
            "parts//body.png",
            "",
        ] {
            let mut value = valid_pack_value();
            value["bodies"][0]["image"] = json!(path);
            assert!(parse_pack(&value.to_string()).is_err(), "accepted {path}");
        }

        let value = valid_pack_value();
        let (content, pack_root) = fixture(&value);
        std::fs::remove_file(pack_root.join("parts/body.png")).unwrap();
        let pack = parse_pack(&value.to_string()).unwrap();
        assert!(validate_fixture_pack(&pack, &content)
            .unwrap_err()
            .contains("body.png"));
        std::fs::remove_dir_all(content).unwrap();
    }

    #[test]
    fn rejects_rgb_wrong_size_truncated_and_oversized_pngs() {
        let value = valid_pack_value();
        let pack = parse_pack(&value.to_string()).unwrap();
        for kind in ["rgb", "size", "truncated", "oversized"] {
            let (content, pack_root) = fixture(&value);
            let body = pack_root.join("parts/body.png");
            match kind {
                "rgb" => image::RgbImage::new(1024, 1024).save(&body).unwrap(),
                "size" => write_rgba_png(&body, 512, 1024),
                "truncated" => std::fs::write(&body, b"\x89PNG\r\n\x1a\n").unwrap(),
                "oversized" => write_rgba_png(&body, 4096, 4096),
                _ => unreachable!(),
            }
            assert!(
                validate_fixture_pack(&pack, &content).is_err(),
                "accepted {kind}"
            );
            std::fs::remove_dir_all(content).unwrap();
        }
    }

    #[test]
    fn rejects_pack_root_and_intermediate_directory_links_without_touching_sentinel() {
        let value = valid_pack_value();
        let pack = parse_pack(&value.to_string()).unwrap();

        let content = temp_root("pack-link");
        std::fs::create_dir_all(content.join("composer")).unwrap();
        let outside = temp_root("outside-pack");
        std::fs::write(outside.join("sentinel.txt"), b"unchanged").unwrap();
        crate::platform::create_directory_link(
            &outside,
            &content.join("composer").join("cat-cute-v1"),
        );
        assert!(validate_fixture_pack(&pack, &content).is_err());
        assert_eq!(
            std::fs::read(outside.join("sentinel.txt")).unwrap(),
            b"unchanged"
        );
        std::fs::remove_dir_all(content).unwrap();
        std::fs::remove_dir_all(outside).unwrap();

        let (content, pack_root) = fixture(&value);
        let outside = temp_root("outside-parts");
        std::fs::write(outside.join("sentinel.txt"), b"unchanged").unwrap();
        std::fs::remove_dir_all(pack_root.join("parts")).unwrap();
        crate::platform::create_directory_link(&outside, &pack_root.join("parts"));
        assert!(validate_fixture_pack(&pack, &content).is_err());
        assert_eq!(
            std::fs::read(outside.join("sentinel.txt")).unwrap(),
            b"unchanged"
        );
        std::fs::remove_dir_all(content).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn rejects_a_reparse_point_at_the_asset_file_path_without_touching_its_target() {
        let value = valid_pack_value();
        let pack = parse_pack(&value.to_string()).unwrap();
        let (content, pack_root) = fixture(&value);
        let outside = temp_root("outside-file-link");
        let target = outside.join("sentinel.png");
        let sentinel = b"external sentinel must remain unchanged";
        std::fs::write(&target, sentinel).unwrap();
        let linked_asset = pack_root.join("parts").join("body.png");
        std::fs::remove_file(&linked_asset).unwrap();
        crate::platform::create_directory_link(&outside, &linked_asset);

        assert!(validate_fixture_pack(&pack, &content).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), sentinel);
        std::fs::remove_dir_all(content).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn validates_recipe_identity_existence_and_body_whitelists() {
        let pack = parse_pack(&valid_pack_value().to_string()).unwrap();
        for mutate in [
            |r: &mut ComposerRecipe| r.recipe_version = 2,
            |r: &mut ComposerRecipe| r.pack_id = "other".into(),
            |r: &mut ComposerRecipe| r.pack_version = 2,
            |r: &mut ComposerRecipe| r.layer_contract_version = 2,
            |r: &mut ComposerRecipe| r.ears_id = "missing".into(),
            |r: &mut ComposerRecipe| r.ears_id = "ears-other".into(),
        ] {
            let mut recipe = valid_recipe();
            mutate(&mut recipe);
            assert!(validate_recipe(&pack, &recipe).is_err());
        }
    }
}
