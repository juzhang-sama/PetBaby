use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

use crate::windowing::WindowMode;

pub const PREFERENCES_SCHEMA_VERSION: u32 = 2;
pub const BASE_WIDTH: u32 = 420;
pub const BASE_HEIGHT: u32 = 520;
pub const MIN_DISPLAY_SCALE: f64 = 0.5;
pub const MAX_DISPLAY_SCALE: f64 = 1.5;
const MAX_TRUSTED_LEGACY_SCALE: f64 = 100.0;

fn is_trusted_dimension(value: u32, base: u32) -> bool {
    value > 0 && f64::from(value) / f64::from(base) <= MAX_TRUSTED_LEGACY_SCALE
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProbePreferences {
    pub schema_version: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub display_scale: f64,
    pub flipped: bool,
    pub mode: String,
    pub user_visible: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPreferences {
    #[serde(default)]
    schema_version: RawSchemaVersion,
    x: Option<i32>,
    y: Option<i32>,
    width: Option<u32>,
    height: Option<u32>,
    display_scale: Option<f64>,
    #[allow(dead_code)]
    scale: Option<serde_json::Value>,
    flipped: Option<bool>,
    #[serde(rename = "mode")]
    _mode: Option<String>,
    user_visible: Option<bool>,
}

#[derive(Debug, Default)]
enum RawSchemaVersion {
    #[default]
    Missing,
    Present(serde_json::Value),
}

impl<'de> Deserialize<'de> for RawSchemaVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        serde_json::Value::deserialize(deserializer).map(Self::Present)
    }
}

impl From<RawPreferences> for ProbePreferences {
    fn from(raw: RawPreferences) -> Self {
        let mut value = Self::default();
        if matches!(
            &raw.schema_version,
            RawSchemaVersion::Present(schema_version)
                if schema_version.as_u64() != Some(u64::from(PREFERENCES_SCHEMA_VERSION))
        ) {
            return value;
        }
        value.x = raw.x.unwrap_or(value.x);
        value.y = raw.y.unwrap_or(value.y);
        value.width = raw
            .width
            .filter(|width| is_trusted_dimension(*width, BASE_WIDTH))
            .unwrap_or(value.width);
        value.height = raw
            .height
            .filter(|height| is_trusted_dimension(*height, BASE_HEIGHT))
            .unwrap_or(value.height);
        value.display_scale = raw
            .display_scale
            .filter(|scale| scale.is_finite())
            .map(|scale| scale.clamp(MIN_DISPLAY_SCALE, MAX_DISPLAY_SCALE))
            .unwrap_or_else(|| match (raw.width, raw.height) {
                (Some(width), Some(height))
                    if is_trusted_dimension(width, BASE_WIDTH)
                        && is_trusted_dimension(height, BASE_HEIGHT) =>
                {
                    let width_scale = f64::from(width) / f64::from(BASE_WIDTH);
                    let height_scale = f64::from(height) / f64::from(BASE_HEIGHT);
                    width_scale
                        .min(height_scale)
                        .clamp(MIN_DISPLAY_SCALE, MAX_DISPLAY_SCALE)
                }
                _ => 1.0,
            });
        value.flipped = raw.flipped.unwrap_or(value.flipped);
        value.mode = "companion".into();
        value.user_visible = raw.user_visible.unwrap_or(value.user_visible);
        value
    }
}

impl<'de> Deserialize<'de> for ProbePreferences {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        RawPreferences::deserialize(deserializer).map(Into::into)
    }
}

impl Default for ProbePreferences {
    fn default() -> Self {
        Self {
            schema_version: PREFERENCES_SCHEMA_VERSION,
            x: 1200,
            y: 500,
            width: BASE_WIDTH,
            height: BASE_HEIGHT,
            display_scale: 1.0,
            flipped: false,
            mode: "companion".into(),
            user_visible: true,
        }
    }
}

fn decode(bytes: &[u8]) -> serde_json::Result<ProbePreferences> {
    serde_json::from_slice(bytes)
}

pub fn load(path: &Path) -> io::Result<ProbePreferences> {
    if !path.exists() {
        return Ok(ProbePreferences::default());
    }
    match decode(&fs::read(path)?) {
        Ok(value) => Ok(value),
        // corrupt preferences fall back to defaults instead of blocking startup
        Err(_) => Ok(ProbePreferences::default()),
    }
}

pub fn save(path: &Path, value: &ProbePreferences) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(value).map_err(io::Error::other)?,
    )?;
    fs::rename(temporary, path)
}

pub fn update_window_mode(path: &Path, mode: WindowMode, user_visible: bool) -> io::Result<()> {
    let mut value = load(path)?;
    let _ = mode;
    value.mode = "companion".to_owned();
    value.user_visible = user_visible;
    save(path, &value)
}

pub fn save_preserving_window_intent(
    path: &Path,
    mut frontend_value: ProbePreferences,
) -> io::Result<()> {
    let current = load(path)?;
    frontend_value.mode = current.mode;
    frontend_value.user_visible = current.user_visible;
    save(path, &frontend_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_dormant_scale_is_ignored_and_invalid_display_scale_is_clamped() {
        let legacy = br#"{"x":1,"y":2,"width":420,"height":520,"scale":1.25,"flipped":false,"mode":"companion"}"#;
        assert_eq!(decode(legacy).unwrap().display_scale, 1.0);
        let scaled_legacy = br#"{"width":630,"height":780,"scale":0.75}"#;
        assert_eq!(decode(scaled_legacy).unwrap().display_scale, 1.5);
        let invalid = br#"{"schemaVersion":2,"x":1,"y":2,"displayScale":9,"flipped":false,"mode":"companion"}"#;
        assert_eq!(decode(invalid).unwrap().display_scale, 1.5);
    }

    #[test]
    fn explicit_display_scale_takes_priority_and_preserves_legacy_fields() {
        let encoded = br#"{"schemaVersion":2,"x":-1,"y":2,"width":210,"height":260,"displayScale":1.25,"scale":0.5,"flipped":true,"mode":"desktop"}"#;
        let value = decode(encoded).unwrap();
        assert_eq!(value.schema_version, PREFERENCES_SCHEMA_VERSION);
        assert_eq!((value.x, value.y), (-1, 2));
        assert_eq!((value.width, value.height), (210, 260));
        assert_eq!(value.display_scale, 1.25);
        assert!(value.flipped);
        assert_eq!(value.mode, "companion");

        let below_minimum = br#"{"schemaVersion":2,"displayScale":-1}"#;
        assert_eq!(decode(below_minimum).unwrap().display_scale, 0.5);
    }

    #[test]
    fn legacy_dimensions_infer_the_smaller_scale_and_clamp_it() {
        let inconsistent = br#"{"width":840,"height":650}"#;
        assert_eq!(decode(inconsistent).unwrap().display_scale, 1.25);

        let oversized = br#"{"width":840,"height":1040}"#;
        assert_eq!(decode(oversized).unwrap().display_scale, 1.5);
    }

    #[test]
    fn extreme_legacy_dimensions_do_not_infer_a_scale() {
        let extreme = br#"{"width":4294967295,"height":4294967295}"#;
        let value = decode(extreme).unwrap();
        assert_eq!(value.display_scale, 1.0);
        assert_eq!((value.width, value.height), (BASE_WIDTH, BASE_HEIGHT));
    }

    #[test]
    fn zero_legacy_dimension_is_replaced_without_partial_scale_inference() {
        let zero = br#"{"width":0,"height":650}"#;
        let value = decode(zero).unwrap();
        assert_eq!(value.display_scale, 1.0);
        assert_eq!((value.width, value.height), (BASE_WIDTH, 650));
    }

    #[test]
    fn future_schema_fails_closed_to_current_defaults() {
        let future = br#"{"schemaVersion":3,"x":1,"y":2,"width":630,"height":780,"displayScale":1.5,"flipped":true,"mode":"desktop"}"#;
        assert_eq!(decode(future).unwrap(), ProbePreferences::default());
    }

    #[test]
    fn corrupt_schema_value_fails_closed_to_current_defaults() {
        let corrupt = br#"{"schemaVersion":null,"x":1,"y":2}"#;
        assert_eq!(decode(corrupt).unwrap(), ProbePreferences::default());
    }

    #[test]
    fn unknown_window_mode_falls_back_to_companion() {
        let invalid = br#"{"mode":"floating"}"#;
        assert_eq!(decode(invalid).unwrap().mode, "companion");
    }

    #[test]
    fn round_trips_preferences_atomically() {
        let root = std::env::temp_dir().join(format!("desktop-pet-{}", std::process::id()));
        let path = root.join("preferences.json");
        let _ = fs::remove_dir_all(&root);
        let first = ProbePreferences {
            x: 1,
            y: 2,
            ..ProbePreferences::default()
        };
        save(&path, &first).unwrap();

        let value = ProbePreferences {
            x: 3,
            y: 4,
            width: 630,
            height: 780,
            display_scale: 1.5,
            flipped: true,
            mode: "companion".into(),
            ..ProbePreferences::default()
        };
        save(&path, &value).unwrap();

        assert_eq!(load(&path).unwrap(), value);
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["schemaVersion"], PREFERENCES_SCHEMA_VERSION);
        assert_eq!(json["displayScale"], 1.5);
        assert_eq!(json["width"], 630);
        assert_eq!(json["height"], 780);
        assert!(json.get("scale").is_none());
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_preferences_fall_back_to_defaults() {
        let root = std::env::temp_dir().join(format!("desktop-pet-pref-{}", std::process::id()));
        let path = root.join("preferences.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, b"{ this is not valid json !").unwrap();
        assert_eq!(load(&path).unwrap(), ProbePreferences::default());

        for corrupt in [
            br#"{"displayScale":null}"#.as_slice(),
            br#"{"displayScale":"1.25"}"#.as_slice(),
            br#"{"displayScale":NaN}"#.as_slice(),
            br#"{"width":"420","height":520}"#.as_slice(),
        ] {
            fs::write(&path, corrupt).unwrap();
            assert_eq!(load(&path).unwrap(), ProbePreferences::default());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_preferences_file_uses_v2_defaults() {
        let root =
            std::env::temp_dir().join(format!("desktop-pet-missing-pref-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let value = load(&root.join("preferences.json")).unwrap();
        assert_eq!(value, ProbePreferences::default());
        assert_eq!(value.schema_version, PREFERENCES_SCHEMA_VERSION);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn window_mode_update_preserves_geometry_and_persists_visibility_intent() {
        let root = crate::test_support::TestStorageRoot::claim("window-mode-preferences").unwrap();
        let path = root.path().join("preferences.json");
        let original = ProbePreferences {
            x: 17,
            y: 23,
            display_scale: 1.25,
            ..ProbePreferences::default()
        };
        save(&path, &original).unwrap();

        update_window_mode(&path, WindowMode::Desktop, false).unwrap();

        let updated = load(&path).unwrap();
        assert_eq!((updated.x, updated.y), (17, 23));
        assert_eq!(updated.display_scale, 1.25);
        assert_eq!(updated.mode, "companion");
        assert!(!updated.user_visible);
    }

    #[test]
    fn frontend_geometry_save_cannot_overwrite_controller_window_intent() {
        let root = crate::test_support::TestStorageRoot::claim("window-mode-stale-save").unwrap();
        let path = root.path().join("preferences.json");
        update_window_mode(&path, WindowMode::Desktop, false).unwrap();
        let stale_frontend = ProbePreferences {
            x: 44,
            y: 55,
            mode: "companion".into(),
            user_visible: true,
            ..ProbePreferences::default()
        };

        save_preserving_window_intent(&path, stale_frontend).unwrap();

        let saved = load(&path).unwrap();
        assert_eq!((saved.x, saved.y), (44, 55));
        assert_eq!(saved.mode, "companion");
        assert!(!saved.user_visible);
    }
}
