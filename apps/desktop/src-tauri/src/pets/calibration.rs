use crate::pets::state::StateStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PetCalibrationV1 {
    pub schema_version: u32,
    pub breath_amplitude_percent: f64,
    pub blink_interval_scale: f64,
    pub feedback_strength: f64,
}

impl Default for PetCalibrationV1 {
    fn default() -> Self {
        Self {
            schema_version: 1,
            breath_amplitude_percent: 2.0,
            blink_interval_scale: 1.0,
            feedback_strength: 0.6,
        }
    }
}

impl PetCalibrationV1 {
    pub fn validate(self) -> Result<Self, String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported calibration schemaVersion: {}",
                self.schema_version
            ));
        }
        validate_finite_range(
            "breathAmplitudePercent",
            self.breath_amplitude_percent,
            0.0,
            5.0,
        )?;
        validate_finite_range("blinkIntervalScale", self.blink_interval_scale, 0.5, 2.0)?;
        validate_finite_range("feedbackStrength", self.feedback_strength, 0.0, 1.0)?;
        Ok(self)
    }
}

fn validate_finite_range(name: &str, value: f64, min: f64, max: f64) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!("{name} must be finite"));
    }
    if !(min..=max).contains(&value) {
        return Err(format!("{name} must be between {min} and {max}"));
    }
    Ok(())
}

pub fn state_key(pet_id: &str) -> Result<String, String> {
    crate::validate_pet_asset_id(pet_id)?;
    Ok(format!("pet:{pet_id}:calibration:v1"))
}

pub fn load(store: &StateStore, pet_id: &str) -> Result<PetCalibrationV1, String> {
    let key = state_key(pet_id)?;
    let Some(json) = store.load(&key)? else {
        return Ok(PetCalibrationV1::default());
    };
    let value = serde_json::from_str::<PetCalibrationV1>(&json)
        .map_err(|error| format!("invalid persisted calibration for {pet_id}: {error}"))?;
    value
        .validate()
        .map_err(|error| format!("invalid persisted calibration for {pet_id}: {error}"))
}

pub fn save(
    store: &StateStore,
    pet_id: &str,
    value: PetCalibrationV1,
) -> Result<PetCalibrationV1, String> {
    let key = state_key(pet_id)?;
    let value = value.validate()?;
    let json = serde_json::to_string(&value)
        .map_err(|error| format!("failed to serialize calibration: {error}"))?;
    store.save(&key, &json)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pets::state::StateStore;
    use crate::storage::Storage;
    use std::sync::{Arc, Mutex};

    fn temp_store() -> (StateStore, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-calibration-{}-{}",
            std::process::id(),
            crate::creation::domain::new_entity_id("test")
        ));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        (StateStore::new(storage), root)
    }

    fn calibration(
        breath_amplitude_percent: f64,
        blink_interval_scale: f64,
        feedback_strength: f64,
    ) -> PetCalibrationV1 {
        PetCalibrationV1 {
            schema_version: 1,
            breath_amplitude_percent,
            blink_interval_scale,
            feedback_strength,
        }
    }

    fn close_store(store: StateStore, root: std::path::PathBuf) {
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn calibration_is_typed_validated_and_scoped_by_pet() {
        let (store, root) = temp_store();
        assert_eq!(load(&store, "pet-a").unwrap(), PetCalibrationV1::default());
        save(
            &store,
            "pet-a",
            PetCalibrationV1 {
                schema_version: 1,
                breath_amplitude_percent: 3.5,
                blink_interval_scale: 1.2,
                feedback_strength: 0.8,
            },
        )
        .unwrap();
        assert_eq!(load(&store, "pet-a").unwrap().feedback_strength, 0.8);
        assert_eq!(load(&store, "pet-b").unwrap(), PetCalibrationV1::default());
        close_store(store, root);
    }

    #[test]
    fn default_and_inclusive_range_boundaries_are_exact() {
        assert_eq!(PetCalibrationV1::default(), calibration(2.0, 1.0, 0.6));
        assert_eq!(
            calibration(0.0, 0.5, 0.0).validate().unwrap(),
            calibration(0.0, 0.5, 0.0)
        );
        assert_eq!(
            calibration(5.0, 2.0, 1.0).validate().unwrap(),
            calibration(5.0, 2.0, 1.0)
        );
    }

    #[test]
    fn each_out_of_range_value_is_rejected_with_its_field_name() {
        for (value, field) in [
            (calibration(-0.01, 1.0, 0.6), "breathAmplitudePercent"),
            (calibration(5.01, 1.0, 0.6), "breathAmplitudePercent"),
            (calibration(2.0, 0.49, 0.6), "blinkIntervalScale"),
            (calibration(2.0, 2.01, 0.6), "blinkIntervalScale"),
            (calibration(2.0, 1.0, -0.01), "feedbackStrength"),
            (calibration(2.0, 1.0, 1.01), "feedbackStrength"),
        ] {
            assert!(value.validate().unwrap_err().contains(field));
        }
    }

    #[test]
    fn every_non_finite_value_is_rejected_before_serialization() {
        for (value, field) in [
            (calibration(f64::NAN, 1.0, 0.6), "breathAmplitudePercent"),
            (
                calibration(f64::INFINITY, 1.0, 0.6),
                "breathAmplitudePercent",
            ),
            (
                calibration(2.0, f64::NEG_INFINITY, 0.6),
                "blinkIntervalScale",
            ),
            (calibration(2.0, 1.0, f64::NAN), "feedbackStrength"),
        ] {
            let error = value.validate().unwrap_err();
            assert!(error.contains(field));
            assert!(error.contains("finite"));
        }
    }

    #[test]
    fn schema_version_must_be_one() {
        let mut value = PetCalibrationV1::default();
        value.schema_version = 2;
        assert!(value.validate().unwrap_err().contains("schemaVersion"));
    }

    #[test]
    fn serde_uses_camel_case_and_denies_unknown_or_wrongly_typed_fields() {
        let json = serde_json::to_value(PetCalibrationV1::default()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "schemaVersion": 1,
                "breathAmplitudePercent": 2.0,
                "blinkIntervalScale": 1.0,
                "feedbackStrength": 0.6
            })
        );

        let unknown = r#"{"schemaVersion":1,"breathAmplitudePercent":2.0,"blinkIntervalScale":1.0,"feedbackStrength":0.6,"extra":true}"#;
        assert!(serde_json::from_str::<PetCalibrationV1>(unknown)
            .unwrap_err()
            .to_string()
            .contains("unknown field"));

        let wrong_type = r#"{"schemaVersion":1,"breathAmplitudePercent":"2","blinkIntervalScale":1.0,"feedbackStrength":0.6}"#;
        assert!(serde_json::from_str::<PetCalibrationV1>(wrong_type)
            .unwrap_err()
            .to_string()
            .contains("invalid type"));
    }

    #[test]
    fn corrupt_unknown_schema_and_out_of_range_persisted_values_are_errors_and_preserved() {
        let (store, root) = temp_store();
        let cases = [
            ("pet-corrupt", "{not-json", "invalid persisted calibration"),
            (
                "pet-unknown",
                r#"{"schemaVersion":1,"breathAmplitudePercent":2.0,"blinkIntervalScale":1.0,"feedbackStrength":0.6,"extra":true}"#,
                "unknown field",
            ),
            (
                "pet-schema",
                r#"{"schemaVersion":2,"breathAmplitudePercent":2.0,"blinkIntervalScale":1.0,"feedbackStrength":0.6}"#,
                "schemaVersion",
            ),
            (
                "pet-range",
                r#"{"schemaVersion":1,"breathAmplitudePercent":5.1,"blinkIntervalScale":1.0,"feedbackStrength":0.6}"#,
                "breathAmplitudePercent",
            ),
        ];
        for (pet_id, raw, expected) in cases {
            let key = state_key(pet_id).unwrap();
            store.save(&key, raw).unwrap();
            assert!(load(&store, pet_id).unwrap_err().contains(expected));
            assert_eq!(store.load(&key).unwrap().as_deref(), Some(raw));
        }
        close_store(store, root);
    }

    #[test]
    fn invalid_save_does_not_overwrite_the_previous_canonical_json() {
        let (store, root) = temp_store();
        let saved = calibration(3.5, 1.2, 0.8);
        assert_eq!(save(&store, "pet-a", saved.clone()).unwrap(), saved);
        let key = state_key("pet-a").unwrap();
        let before = store.load(&key).unwrap().unwrap();

        assert!(save(&store, "pet-a", calibration(f64::NAN, 1.2, 0.8)).is_err());
        assert_eq!(store.load(&key).unwrap().as_deref(), Some(before.as_str()));
        assert_eq!(
            serde_json::from_str::<PetCalibrationV1>(&before).unwrap(),
            saved
        );
        close_store(store, root);
    }

    #[test]
    fn state_key_rejects_empty_path_like_colon_and_overlong_pet_ids() {
        for pet_id in ["", "pet:a", "../pet", "pet/name", "pet\\name"] {
            assert_eq!(state_key(pet_id), Err("invalid petId".into()));
        }
        assert_eq!(state_key(&"a".repeat(81)), Err("invalid petId".into()));
        assert_eq!(state_key("pet_A-1").unwrap(), "pet:pet_A-1:calibration:v1");
    }
}
