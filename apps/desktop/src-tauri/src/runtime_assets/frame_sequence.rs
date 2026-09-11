//! `frame-sequence-v1`（schemaVersion 6/7）资产契约。
//!
//! 校验规则与前端 `src/runtime/frame-sequence-manifest.ts` 逐条对齐。
//! 内置宠物资产由前端解析并直接播放，用户上传资产则在安装前由本模块解析，
//! 两侧对同一份 manifest 必须给出同样的接受/拒绝结论 —— 否则会出现
//! "内置宠物能跑、用户宠物装不上"的静默分裂。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

use super::manifest::{normalize_relative_path, ManifestFileEntry};

/// 归一化后的目标版本：schemaVersion 6 与 7 都解析成 V7 结构。
pub const FRAME_SEQUENCE_SCHEMA_VERSION: u32 = 7;
pub const FRAME_SEQUENCE_RENDERER: &str = "frame-sequence-v1";

/// 产品会发出的全部动作（唯一真源：前端 `pet-presentation-controller.ts`）。
///
/// `semantics` 必须显式声明这里的每一个键 —— 没有专属动作的就显式指向
/// `defaultAction`。原因是运行时的取法是 `semantics[motion] ?? defaultAction`，
/// 键缺失与"故意不响应"在数据上不可区分，静默回落无法被验收。
/// 显式声明后，"此处回落待机"变成一个看得见的决定。
pub const PRODUCT_MOTIONS: [&str; 9] = [
    "idle",
    "look-left",
    "look-right",
    "react-happy",
    "react-curious",
    "carried",
    "landed",
    "sleep",
    "wake",
];

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameSequenceRectV7 {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameSequenceActionV7 {
    pub action_id: String,
    /// JSON 字段名是 `loop`，在 Rust 里是关键字，因此改名为 `looping`。
    #[serde(rename = "loop")]
    pub looping: bool,
    pub frame_duration_ms: f64,
    pub frames: Vec<String>,
    /// 交互保持区间（帧下标闭区间）：拎起类动作的"悬空保持"段。
    #[serde(default)]
    pub hold_range: Option<[i64; 2]>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameSequenceBlinkV7 {
    pub enabled: bool,
    pub min_interval_ms: f64,
    pub max_interval_ms: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameSequenceIdleScheduleEntryV7 {
    pub action_id: String,
    pub weight: f64,
    pub min_interval_ms: f64,
    #[serde(default)]
    pub max_interval_ms: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameSequenceIdleScheduleV7 {
    pub entries: Vec<FrameSequenceIdleScheduleEntryV7>,
    #[serde(default)]
    pub min_interval_ms: Option<f64>,
    #[serde(default)]
    pub max_interval_ms: Option<f64>,
    #[serde(default)]
    pub align_to_default_loop: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetManifestV7 {
    pub schema_version: u32,
    pub renderer: String,
    pub pet_id: String,
    pub variant_id: String,
    pub display_name: String,
    pub species: String,
    pub base_image: String,
    pub default_action: String,
    pub anchor_policy: String,
    pub actions: Vec<FrameSequenceActionV7>,
    pub semantics: BTreeMap<String, String>,
    #[serde(default)]
    pub idle_schedule: Option<FrameSequenceIdleScheduleV7>,
    #[serde(default)]
    pub blink: Option<FrameSequenceBlinkV7>,
    #[serde(default)]
    pub hit_bounds: Option<FrameSequenceRectV7>,
    pub files: Vec<ManifestFileEntry>,
}

/// 解析并校验 `frame-sequence-v1` manifest。
///
/// 校验顺序与错误文案刻意与前端 `parseFrameSequenceManifest` 保持一致，
/// 这样同一份 manifest 在两侧要么都接受、要么都以同样的理由拒绝。
/// 注意：这里**刻意不做**比前端更严格的检查（例如 idleSchedule 里的 actionId
/// 是否真的被声明），保持两侧同构；要加严必须两侧一起加。
pub fn parse_frame_sequence_manifest(
    value: serde_json::Value,
) -> Result<RuntimeAssetManifestV7, String> {
    validate_frame_sequence(&value)?;
    let mut manifest: RuntimeAssetManifestV7 = serde_json::from_value(value)
        .map_err(|error| format!("invalid frame-sequence manifest: {error}"))?;
    manifest.schema_version = FRAME_SEQUENCE_SCHEMA_VERSION;
    manifest.base_image = normalize_relative_path(&manifest.base_image)
        .map_err(|_| "baseImage must be a relative path".to_string())?;
    for action in &mut manifest.actions {
        for frame in &mut action.frames {
            *frame = normalize_relative_path(frame)
                .map_err(|_| "action frame must be a relative path".to_string())?;
        }
    }
    for file in &mut manifest.files {
        file.relative_path = normalize_relative_path(&file.relative_path)?;
        file.sha256.make_ascii_lowercase();
    }
    // 旧版（schema 6）用 blink 描述眨眼节奏，归一化成 idleSchedule 条目。
    if manifest.idle_schedule.is_none() {
        manifest.idle_schedule = blink_to_idle_schedule(manifest.blink.as_ref());
    }
    Ok(manifest)
}

fn blink_to_idle_schedule(
    blink: Option<&FrameSequenceBlinkV7>,
) -> Option<FrameSequenceIdleScheduleV7> {
    let blink = blink?;
    if !blink.enabled {
        return None;
    }
    Some(FrameSequenceIdleScheduleV7 {
        entries: vec![FrameSequenceIdleScheduleEntryV7 {
            action_id: "blink".to_string(),
            weight: 1.0,
            min_interval_ms: blink.min_interval_ms,
            max_interval_ms: Some(blink.max_interval_ms),
        }],
        min_interval_ms: None,
        max_interval_ms: None,
        align_to_default_loop: None,
    })
}

fn validate_frame_sequence(value: &serde_json::Value) -> Result<(), String> {
    let manifest = as_object(value, "manifest")?;

    let schema_version = manifest
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing or invalid schemaVersion")?;
    if schema_version != 6 && schema_version != u64::from(FRAME_SEQUENCE_SCHEMA_VERSION) {
        return Err(format!("unsupported schemaVersion: {schema_version}"));
    }

    let renderer = required_string(manifest, "renderer")?;
    if renderer != FRAME_SEQUENCE_RENDERER {
        return Err(format!("unsupported renderer: {renderer}"));
    }

    for field in ["petId", "variantId", "displayName"] {
        required_string(manifest, field)?;
    }

    let species = required_string(manifest, "species")?;
    if species != "cat" && species != "dog" {
        return Err("species must be cat or dog".into());
    }

    required_image_path(manifest, "baseImage")?;
    required_string(manifest, "defaultAction")?;

    let anchor_policy = required_string(manifest, "anchorPolicy")?;
    if anchor_policy != "fixed" {
        return Err("anchorPolicy must be fixed".into());
    }

    let actions = match manifest.get("actions") {
        Some(serde_json::Value::Array(actions)) if !actions.is_empty() => actions,
        _ => return Err("manifest must declare at least one action".into()),
    };
    let mut declared_actions = HashSet::new();
    for (index, action) in actions.iter().enumerate() {
        let label = format!("actions[{index}]");
        let action = as_object(action, &label)?;

        let action_id = required_string(action, "actionId")?;
        if !declared_actions.insert(action_id.clone()) {
            return Err(format!("duplicate actionId: {action_id}"));
        }

        match action.get("loop") {
            Some(serde_json::Value::Bool(_)) => {}
            _ => return Err(format!("{label}.loop must be a boolean")),
        }

        if required_number(action, "frameDurationMs")? <= 0.0 {
            return Err(format!("{label}.frameDurationMs must be positive"));
        }

        let frames = match action.get("frames") {
            Some(serde_json::Value::Array(frames)) if !frames.is_empty() => frames,
            _ => return Err(format!("{label}.frames must declare at least one frame")),
        };
        let mut seen_frames = HashSet::new();
        for (frame_index, frame) in frames.iter().enumerate() {
            let frame_label = format!("{label}.frames[{frame_index}]");
            let frame = match frame {
                serde_json::Value::String(frame) if !frame.is_empty() => frame,
                _ => return Err(format!("{frame_label} must be a string")),
            };
            let path = normalize_relative_path(frame)
                .map_err(|_| format!("{frame_label} must be a relative path"))?;
            if !is_supported_image_path(&path) {
                return Err(format!("{frame_label} must be a PNG or WebP file"));
            }
            if !seen_frames.insert(path.clone()) {
                return Err(format!("duplicate frame path: {path}"));
            }
        }

        if let Some(raw) = action.get("holdRange") {
            let held = raw
                .as_array()
                .filter(|held| held.len() == 2)
                .ok_or_else(|| {
                    format!("{label}.holdRange must be a [lo, hi] frame-index pair")
                })?;
            let lo = held[0].as_i64().ok_or_else(|| {
                format!("{label}.holdRange must contain integer frame indices")
            })?;
            let hi = held[1].as_i64().ok_or_else(|| {
                format!("{label}.holdRange must contain integer frame indices")
            })?;
            if lo < 0 || hi < lo || (hi as usize) >= frames.len() {
                return Err(format!(
                    "{label}.holdRange must satisfy 0 <= lo <= hi < {}",
                    frames.len()
                ));
            }
        }
    }

    let default_action = required_string(manifest, "defaultAction")?;
    if !declared_actions.contains(&default_action) {
        return Err("defaultAction must reference a declared action".into());
    }

    let semantics = as_object(
        manifest
            .get("semantics")
            .ok_or("semantics must be an object")?,
        "semantics",
    )?;
    for (motion, target) in semantics {
        let target = match target {
            serde_json::Value::String(target) if !target.is_empty() => target,
            _ => return Err(format!("semantics.{motion} must be a non-empty string")),
        };
        if !declared_actions.contains(target.as_str()) {
            return Err(format!(
                "semantics.{motion} references unknown action: {target}"
            ));
        }
    }

    let missing: Vec<&str> = PRODUCT_MOTIONS
        .iter()
        .copied()
        .filter(|motion| !semantics.contains_key(*motion))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "semantics must declare every product motion: missing {}",
            missing.join(", ")
        ));
    }

    if let Some(blink) = manifest.get("blink") {
        let blink = as_object(blink, "blink")?;
        match blink.get("enabled") {
            Some(serde_json::Value::Bool(_)) => {}
            _ => return Err("blink.enabled must be a boolean".into()),
        }
        let min_interval_ms = required_number(blink, "minIntervalMs")?;
        let max_interval_ms = required_number(blink, "maxIntervalMs")?;
        if min_interval_ms <= 0.0 {
            return Err("blink.minIntervalMs must be positive".into());
        }
        if max_interval_ms < min_interval_ms {
            return Err("blink.maxIntervalMs must be >= minIntervalMs".into());
        }
    }

    if let Some(schedule) = manifest.get("idleSchedule") {
        validate_idle_schedule(schedule)?;
    }

    if let Some(bounds) = manifest.get("hitBounds") {
        let bounds = as_object(bounds, "hitBounds")?;
        let mut values = [0.0_f64; 4];
        for (slot, field) in values.iter_mut().zip(["left", "top", "right", "bottom"]) {
            *slot = bounds
                .get(field)
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| format!("hitBounds.{field} must be a number"))?;
            if !(0.0..=1.0).contains(slot) {
                return Err("hitBounds is out of range".into());
            }
        }
        let [left, top, right, bottom] = values;
        if left >= right || top >= bottom {
            return Err("hitBounds is an inverted rect".into());
        }
    }

    let files = match manifest.get("files") {
        Some(serde_json::Value::Array(files)) if !files.is_empty() => files,
        _ => return Err("manifest must declare files".into()),
    };
    let mut seen_paths = HashSet::new();
    for entry in files {
        let entry = as_object(entry, "file entry")?;
        required_string(entry, "role")?;
        let raw_path = required_string(entry, "relativePath")?;
        let path = normalize_relative_path(&raw_path)?;
        let sha256 = required_string(entry, "sha256")?;
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("invalid file entry: sha256 must be 64 hex chars".into());
        }
        if !seen_paths.insert(path.clone()) {
            return Err(format!("duplicate asset path: {path}"));
        }
        if !is_supported_image_path(&path) && !path.to_ascii_lowercase().ends_with(".json") {
            return Err(format!("unsupported asset extension: {raw_path}"));
        }
    }

    Ok(())
}

fn validate_idle_schedule(value: &serde_json::Value) -> Result<(), String> {
    let schedule = as_object(value, "idleSchedule")?;
    let entries = match schedule.get("entries") {
        Some(serde_json::Value::Array(entries)) if !entries.is_empty() => entries,
        _ => return Err("idleSchedule.entries must be a non-empty array".into()),
    };
    let mut seen_action_ids = HashSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let label = format!("idleSchedule.entries[{index}]");
        let entry = as_object(entry, &label)?;
        let action_id = required_string(entry, "actionId")?;
        if !seen_action_ids.insert(action_id.clone()) {
            return Err(format!("idleSchedule.entries has duplicate actionId: {action_id}"));
        }
        if required_number(entry, "weight")? < 0.0 {
            return Err(format!("{label}.weight must be >= 0"));
        }
        let min_interval_ms = required_number(entry, "minIntervalMs")?;
        if min_interval_ms <= 0.0 {
            return Err(format!("{label}.minIntervalMs must be positive"));
        }
        let max_interval_ms = match entry.get("maxIntervalMs") {
            None => min_interval_ms,
            Some(value) => value
                .as_f64()
                .ok_or_else(|| format!("{label}.maxIntervalMs must be a number"))?,
        };
        if max_interval_ms < min_interval_ms {
            return Err(format!("{label}.maxIntervalMs must be >= minIntervalMs"));
        }
    }
    if let Some(align) = schedule.get("alignToDefaultLoop") {
        if !align.is_boolean() {
            return Err("idleSchedule.alignToDefaultLoop must be a boolean".into());
        }
    }
    if let Some(value) = schedule.get("minIntervalMs") {
        let min_interval_ms = value
            .as_f64()
            .ok_or("idleSchedule.minIntervalMs must be a number")?;
        if min_interval_ms <= 0.0 {
            return Err("idleSchedule.minIntervalMs must be positive".into());
        }
    }
    if let Some(value) = schedule.get("maxIntervalMs") {
        let max_interval_ms = value
            .as_f64()
            .ok_or("idleSchedule.maxIntervalMs must be a number")?;
        let min_interval_ms = schedule
            .get("minIntervalMs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(max_interval_ms);
        if max_interval_ms < min_interval_ms {
            return Err("idleSchedule.maxIntervalMs must be >= minIntervalMs".into());
        }
    }
    Ok(())
}

fn as_object<'a>(
    value: &'a serde_json::Value,
    name: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{name} must be an object"))
}

fn required_string(
    map: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<String, String> {
    match map.get(field) {
        Some(serde_json::Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        _ => Err(format!("missing or invalid {field}")),
    }
}

fn required_number(
    map: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<f64, String> {
    map.get(field)
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| format!("missing or invalid {field}"))
}

fn required_image_path(
    map: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<String, String> {
    let raw = required_string(map, field)?;
    let path = normalize_relative_path(&raw)
        .map_err(|_| format!("{field} must be a relative path"))?;
    if !is_supported_image_path(&path) {
        return Err(format!("{field} must be a PNG or WebP file"));
    }
    Ok(path)
}

/// 帧序列的位图帧支持 PNG（历史）与 WebP（体积瘦身）两种编码。
fn is_supported_image_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".png") || lower.ends_with(".webp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_assets::manifest::{parse_manifest, RuntimeAssetManifest};
    use std::path::PathBuf;

    const REAL_PETS: [&str; 5] = [
        "01-longhair-black-white",
        "02-round-tabby",
        "03-sleek-black",
        "04-warm-brown-tabby",
        "05-silver-tabby",
    ];

    fn builtin_pets_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/builtin-pets")
    }

    fn real_manifest(pet_id: &str) -> serde_json::Value {
        let path = builtin_pets_root().join(pet_id).join("manifest.json");
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        serde_json::from_slice(&bytes).expect("builtin manifest must be valid JSON")
    }

    /// 最小合法 manifest：测试按需覆盖单个字段。
    fn minimal_manifest() -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": 7,
            "renderer": "frame-sequence-v1",
            "petId": "pet-a",
            "variantId": "variant-a",
            "displayName": "测试宠物",
            "species": "cat",
            "baseImage": "frames/idle/f0000.webp",
            "defaultAction": "idle",
            "anchorPolicy": "fixed",
            "actions": [
                {
                    "actionId": "idle",
                    "loop": true,
                    "frameDurationMs": 42,
                    "frames": ["frames/idle/f0000.webp", "frames/idle/f0001.webp"]
                }
            ],
            "semantics": {
                "idle": "idle",
                "look-left": "idle",
                "look-right": "idle",
                "react-happy": "idle",
                "react-curious": "idle",
                "carried": "idle",
                "landed": "idle",
                "sleep": "idle",
                "wake": "idle",
            },
            "files": [
                { "role": "base", "relativePath": "frames/idle/f0000.webp", "sha256": "a".repeat(64) },
                { "role": "frame", "relativePath": "frames/idle/f0001.webp", "sha256": "b".repeat(64) }
            ]
        })
    }

    fn parse(value: serde_json::Value) -> Result<RuntimeAssetManifestV7, String> {
        parse_frame_sequence_manifest(value)
    }

    #[test]
    fn parses_the_minimal_manifest() {
        let manifest = parse(minimal_manifest()).expect("minimal manifest must parse");
        assert_eq!(manifest.schema_version, FRAME_SEQUENCE_SCHEMA_VERSION);
        assert_eq!(manifest.renderer, FRAME_SEQUENCE_RENDERER);
        assert_eq!(manifest.species, "cat");
        assert_eq!(manifest.default_action, "idle");
        assert_eq!(manifest.actions.len(), 1);
        assert_eq!(manifest.actions[0].frames.len(), 2);
    }

    #[test]
    fn parses_real_builtin_combo_loop_manifest() {
        let manifest = parse(real_manifest("04-warm-brown-tabby")).expect("04 must parse");
        assert_eq!(manifest.schema_version, 7);
        assert_eq!(manifest.pet_id, "04-warm-brown-tabby");
        assert_eq!(manifest.species, "cat");
        assert_eq!(manifest.default_action, "idle-combo");
        assert_eq!(manifest.base_image, "frames/idle-combo/f0000.webp");

        let action_ids: Vec<&str> = manifest
            .actions
            .iter()
            .map(|action| action.action_id.as_str())
            .collect();
        assert_eq!(action_ids, vec!["idle-combo", "yawn", "lick"]);

        let idle = &manifest.actions[0];
        assert!(idle.looping);
        assert_eq!(idle.frame_duration_ms, 42.0);
        assert_eq!(idle.frames.len(), 287);
        assert!(idle.hold_range.is_none());

        let schedule = manifest.idle_schedule.expect("04 must declare idleSchedule");
        assert_eq!(schedule.align_to_default_loop, Some(true));
        assert_eq!(schedule.entries.len(), 2);
        assert_eq!(schedule.entries[0].action_id, "yawn");
        assert_eq!(schedule.entries[0].min_interval_ms, 30000.0);
        assert_eq!(schedule.entries[0].max_interval_ms, Some(60000.0));
    }

    #[test]
    fn parses_real_builtin_grab_release_manifest() {
        let manifest = parse(real_manifest("05-silver-tabby")).expect("05 must parse");
        let grab = manifest
            .actions
            .iter()
            .find(|action| action.action_id == "grab-release")
            .expect("05 must declare grab-release");
        assert!(!grab.looping);
        assert_eq!(grab.frames.len(), 81);
        assert_eq!(grab.hold_range, Some([30, 59]));
        // holdRange 必须落在帧区间内，渲染器据此做"悬空保持"循环。
        let [lo, hi] = grab.hold_range.unwrap();
        assert!(lo >= 0);
        assert!(hi > lo);
        assert!((hi as usize) < grab.frames.len());

        for motion in ["grab-release", "carried", "landed"] {
            assert_eq!(
                manifest.semantics.get(motion).map(String::as_str),
                Some("grab-release"),
                "{motion} 必须映射到 grab-release"
            );
        }
    }

    #[test]
    fn every_real_builtin_frame_is_declared_in_files() {
        for pet_id in REAL_PETS {
            let manifest = parse(real_manifest(pet_id))
                .unwrap_or_else(|error| panic!("{pet_id} must parse: {error}"));
            assert_eq!(manifest.schema_version, 7, "{pet_id}");
            let declared: HashSet<&str> = manifest
                .files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect();
            assert!(
                declared.contains(manifest.base_image.as_str()),
                "{pet_id} 的 baseImage 必须出现在 files 中"
            );
            for action in &manifest.actions {
                for frame in &action.frames {
                    assert!(
                        declared.contains(frame.as_str()),
                        "{pet_id}/{} 的帧 {frame} 没有出现在 files 中",
                        action.action_id
                    );
                }
            }
        }
    }

    #[test]
    fn is_reachable_through_parse_manifest() {
        // 用户上传资产的安装前校验走 parse_manifest，V7 必须在这里可达，
        // 否则 schema7 资产会被当成 Corrupt 拒收。
        let json = real_manifest("04-warm-brown-tabby").to_string();
        match parse_manifest(&json).expect("parse_manifest must accept schema 7") {
            RuntimeAssetManifest::V7(manifest) => {
                assert_eq!(manifest.pet_id, "04-warm-brown-tabby");
            }
            other => panic!("expected V7, got {other:?}"),
        }
    }

    #[test]
    fn normalizes_schema_version_6_to_7() {
        let mut value = minimal_manifest();
        value["schemaVersion"] = serde_json::json!(6);
        let manifest = parse(value).expect("schema 6 must still parse");
        assert_eq!(manifest.schema_version, 7);
    }

    #[test]
    fn rejects_unsupported_schema_versions() {
        for version in [5, 8] {
            let mut value = minimal_manifest();
            value["schemaVersion"] = serde_json::json!(version);
            let error = parse(value).unwrap_err();
            assert!(
                error.contains("unsupported schemaVersion"),
                "schema {version} 必须以 schemaVersion 错误被拒，实际：{error}"
            );
        }
    }

    #[test]
    fn rejects_wrong_renderer() {
        let mut value = minimal_manifest();
        value["renderer"] = serde_json::json!("animated-image-v1");
        assert!(parse(value).unwrap_err().contains("renderer"));
    }

    #[test]
    fn rejects_unknown_species() {
        let mut value = minimal_manifest();
        value["species"] = serde_json::json!("bird");
        assert!(parse(value).unwrap_err().contains("species"));
    }

    #[test]
    fn accepts_dog_species() {
        let mut value = minimal_manifest();
        value["species"] = serde_json::json!("dog");
        assert_eq!(parse(value).unwrap().species, "dog");
    }

    #[test]
    fn rejects_non_fixed_anchor_policy() {
        let mut value = minimal_manifest();
        value["anchorPolicy"] = serde_json::json!("floating");
        assert!(parse(value).unwrap_err().contains("anchorPolicy"));
    }

    #[test]
    fn rejects_default_action_that_is_not_declared() {
        let mut value = minimal_manifest();
        value["defaultAction"] = serde_json::json!("sleep");
        assert!(parse(value).unwrap_err().contains("defaultAction"));
    }

    #[test]
    fn rejects_duplicate_action_ids() {
        let mut value = minimal_manifest();
        let actions = value["actions"].as_array_mut().unwrap();
        let duplicate = actions[0].clone();
        actions.push(duplicate);
        assert!(parse(value).unwrap_err().contains("duplicate actionId"));
    }

    #[test]
    fn rejects_non_positive_frame_duration() {
        let mut value = minimal_manifest();
        value["actions"][0]["frameDurationMs"] = serde_json::json!(0);
        assert!(parse(value).unwrap_err().contains("frameDurationMs"));
    }

    #[test]
    fn rejects_action_without_frames() {
        let mut value = minimal_manifest();
        value["actions"][0]["frames"] = serde_json::json!([]);
        assert!(parse(value).unwrap_err().contains("frames"));
    }

    #[test]
    fn rejects_duplicate_frame_paths_within_an_action() {
        let mut value = minimal_manifest();
        value["actions"][0]["frames"] = serde_json::json!([
            "frames/idle/f0000.webp",
            "frames/idle/f0000.webp"
        ]);
        assert!(parse(value).unwrap_err().contains("duplicate frame path"));
    }

    #[test]
    fn rejects_unsupported_frame_extension() {
        let mut value = minimal_manifest();
        value["actions"][0]["frames"] = serde_json::json!(["frames/idle/f0000.jpg"]);
        assert!(parse(value).unwrap_err().contains("PNG or WebP"));
    }

    #[test]
    fn rejects_unsupported_base_image_extension() {
        let mut value = minimal_manifest();
        value["baseImage"] = serde_json::json!("frames/idle/f0000.jpg");
        assert!(parse(value).unwrap_err().contains("PNG or WebP"));
    }

    #[test]
    fn rejects_frame_path_traversal() {
        let mut value = minimal_manifest();
        value["actions"][0]["frames"] = serde_json::json!(["../f0000.webp"]);
        assert!(parse(value).unwrap_err().contains("relative path"));
    }

    #[test]
    fn rejects_hold_range_out_of_bounds() {
        let mut value = minimal_manifest();
        value["actions"][0]["holdRange"] = serde_json::json!([0, 5]);
        let error = parse(value).unwrap_err();
        assert!(error.contains("holdRange"), "实际：{error}");
    }

    #[test]
    fn rejects_inverted_hold_range() {
        let mut value = minimal_manifest();
        value["actions"][0]["holdRange"] = serde_json::json!([1, 0]);
        assert!(parse(value).unwrap_err().contains("holdRange"));
    }

    #[test]
    fn rejects_hold_range_without_two_entries() {
        let mut value = minimal_manifest();
        value["actions"][0]["holdRange"] = serde_json::json!([0]);
        assert!(parse(value).unwrap_err().contains("holdRange"));
    }

    #[test]
    fn accepts_a_valid_hold_range() {
        let mut value = minimal_manifest();
        value["actions"][0]["holdRange"] = serde_json::json!([0, 1]);
        assert_eq!(parse(value).unwrap().actions[0].hold_range, Some([0, 1]));
    }

    #[test]
    fn rejects_semantics_pointing_at_unknown_action() {
        let mut value = minimal_manifest();
        value["semantics"] = serde_json::json!({ "idle": "sleep" });
        assert!(parse(value).unwrap_err().contains("unknown action"));
    }

    #[test]
    fn rejects_empty_semantics_value() {
        let mut value = minimal_manifest();
        value["semantics"] = serde_json::json!({ "idle": "" });
        assert!(parse(value).unwrap_err().contains("semantics"));
    }

    #[test]
    fn rejects_semantics_missing_product_motions() {
        let mut value = minimal_manifest();
        value["semantics"] = serde_json::json!({ "idle": "idle" });
        let error = parse(value).unwrap_err();
        assert!(error.contains("every product motion"), "{error}");
        assert!(error.contains("look-left"), "{error}");
        assert!(error.contains("wake"), "{error}");
    }

    #[test]
    fn accepts_semantics_declaring_every_product_motion() {
        let manifest = parse(minimal_manifest()).expect("9 键 semantics 必须解析通过");
        for motion in PRODUCT_MOTIONS {
            assert_eq!(
                manifest.semantics.get(motion).map(String::as_str),
                Some("idle"),
                "{motion} 必须被显式声明"
            );
        }
    }

    #[test]
    fn rejects_invalid_file_sha256() {
        let mut value = minimal_manifest();
        value["files"][0]["sha256"] = serde_json::json!("nope");
        assert!(parse(value).unwrap_err().contains("sha256"));
    }

    #[test]
    fn rejects_duplicate_asset_paths() {
        let mut value = minimal_manifest();
        value["files"][1]["relativePath"] = serde_json::json!("frames/idle/f0000.webp");
        assert!(parse(value).unwrap_err().contains("duplicate asset path"));
    }

    #[test]
    fn rejects_empty_files() {
        let mut value = minimal_manifest();
        value["files"] = serde_json::json!([]);
        assert!(parse(value).unwrap_err().contains("file"));
    }

    #[test]
    fn rejects_files_with_unsupported_extension() {
        let mut value = minimal_manifest();
        value["files"][0]["relativePath"] = serde_json::json!("frames/idle/f0000.mp4");
        assert!(parse(value).unwrap_err().contains("extension"));
    }

    #[test]
    fn normalizes_uppercase_sha256_and_backslash_paths() {
        let mut value = minimal_manifest();
        value["files"][0]["sha256"] = serde_json::json!("A".repeat(64));
        value["files"][0]["relativePath"] = serde_json::json!("frames\\idle\\f0000.webp");
        let manifest = parse(value).unwrap();
        assert_eq!(manifest.files[0].sha256, "a".repeat(64));
        assert_eq!(manifest.files[0].relative_path, "frames/idle/f0000.webp");
    }

    #[test]
    fn rejects_empty_identity_fields() {
        for field in ["petId", "variantId", "displayName"] {
            let mut value = minimal_manifest();
            value[field] = serde_json::json!("");
            let error = parse(value).unwrap_err();
            assert!(error.contains(field), "{field} 必须被拒，实际：{error}");
        }
    }

    #[test]
    fn rejects_invalid_hit_bounds() {
        let mut value = minimal_manifest();
        value["hitBounds"] =
            serde_json::json!({ "left": 0.5, "top": 0.1, "right": 0.5, "bottom": 0.9 });
        assert!(parse(value).unwrap_err().contains("hitBounds"));
    }

    #[test]
    fn accepts_valid_hit_bounds() {
        let mut value = minimal_manifest();
        value["hitBounds"] =
            serde_json::json!({ "left": 0.1, "top": 0.1, "right": 0.9, "bottom": 0.9 });
        let manifest = parse(value).unwrap();
        assert_eq!(manifest.hit_bounds.unwrap().right, 0.9);
    }

    #[test]
    fn fills_idle_schedule_from_legacy_blink() {
        let mut value = minimal_manifest();
        value["blink"] =
            serde_json::json!({ "enabled": true, "minIntervalMs": 3000, "maxIntervalMs": 8000 });
        let manifest = parse(value).unwrap();
        let schedule = manifest.idle_schedule.expect("blink must produce an idleSchedule");
        assert_eq!(schedule.entries.len(), 1);
        assert_eq!(schedule.entries[0].action_id, "blink");
        assert_eq!(schedule.entries[0].min_interval_ms, 3000.0);
    }

    #[test]
    fn rejects_invalid_blink() {
        let mut value = minimal_manifest();
        value["blink"] =
            serde_json::json!({ "enabled": true, "minIntervalMs": 8000, "maxIntervalMs": 3000 });
        assert!(parse(value).unwrap_err().contains("blink"));

        let mut value = minimal_manifest();
        value["blink"] = serde_json::json!({ "enabled": true, "minIntervalMs": 0, "maxIntervalMs": 0 });
        assert!(parse(value).unwrap_err().contains("blink"));
    }

    #[test]
    fn rejects_idle_schedule_with_duplicate_entries() {
        let mut value = minimal_manifest();
        value["idleSchedule"] = serde_json::json!({
            "entries": [
                { "actionId": "idle", "weight": 1, "minIntervalMs": 30000, "maxIntervalMs": 60000 },
                { "actionId": "idle", "weight": 1, "minIntervalMs": 30000, "maxIntervalMs": 60000 }
            ]
        });
        assert!(parse(value).unwrap_err().contains("duplicate actionId"));
    }
}
