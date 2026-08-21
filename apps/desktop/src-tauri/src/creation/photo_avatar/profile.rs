use super::domain::{
    parse_appearance_profile_v1, AppearanceProfileV1, IdentityTraitKey, IdentityTraitV1,
    TraitSource,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const ALL_IDENTITY_TRAIT_KEYS: [IdentityTraitKey; 11] = [
    IdentityTraitKey::FaceShape,
    IdentityTraitKey::FaceProportions,
    IdentityTraitKey::FurColors,
    IdentityTraitKey::Markings,
    IdentityTraitKey::EyeShape,
    IdentityTraitKey::EyeColor,
    IdentityTraitKey::EarShape,
    IdentityTraitKey::BodyType,
    IdentityTraitKey::Tail,
    IdentityTraitKey::SignatureMarks,
    IdentityTraitKey::Temperament,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppearanceCompletionV1 {
    pub requested_trait_keys: Vec<IdentityTraitKey>,
    pub completed_traits: Vec<IdentityTraitV1>,
    pub body_module_id: String,
    pub body_module_source: TraitSource,
}

pub fn finalize_appearance_profile(
    partial: &AppearanceProfileV1,
    completion: AppearanceCompletionV1,
) -> Result<AppearanceProfileV1, String> {
    validate_unique_keys(&completion.requested_trait_keys, "requestedTraitKeys")?;
    validate_body_module(&completion.body_module_id)?;

    let mut seen_partial = HashSet::new();
    for identity_trait in &partial.traits {
        if !seen_partial.insert(identity_trait.key) {
            return Err(format!(
                "duplicate partial trait key: {}",
                trait_key_name(identity_trait.key)
            ));
        }
    }

    let mut seen_completed = HashSet::new();
    for identity_trait in &completion.completed_traits {
        if !seen_completed.insert(identity_trait.key) {
            return Err(format!(
                "duplicate completed trait key: {}",
                trait_key_name(identity_trait.key)
            ));
        }
        if identity_trait.source != TraitSource::AiCompleted {
            return Err("completedTraits entries must use ai-completed source".into());
        }
    }

    let mut traits = Vec::with_capacity(ALL_IDENTITY_TRAIT_KEYS.len());
    for key in ALL_IDENTITY_TRAIT_KEYS {
        let partial_trait = partial.traits.iter().find(|value| value.key == key);
        let completed_trait = completion
            .completed_traits
            .iter()
            .find(|value| value.key == key);
        let selected = match (partial_trait, completed_trait) {
            (Some(value), _) if value.source == TraitSource::User => value.clone(),
            (_, Some(value)) => value.clone(),
            (Some(value), None) => value.clone(),
            (None, None) => return Err(format!("missing identity trait: {}", trait_key_name(key))),
        };
        if partial_trait.is_none() && selected.source != TraitSource::AiCompleted {
            return Err(format!(
                "missing trait must be ai-completed: {}",
                trait_key_name(key)
            ));
        }
        traits.push(selected);
    }

    let user_body = partial
        .traits
        .iter()
        .find(|value| value.key == IdentityTraitKey::BodyType)
        .filter(|value| value.source == TraitSource::User);
    let (body_module_id, body_module_source) = if partial.body_module_source == TraitSource::User {
        let body = user_body.ok_or("user body module requires user bodyType evidence")?;
        if body.evidence_photo_ids.is_empty() {
            return Err("user body module requires user bodyType photo evidence".into());
        }
        validate_body_module(&partial.body_module_id)?;
        (partial.body_module_id.clone(), TraitSource::User)
    } else {
        if completion.body_module_source == TraitSource::User {
            let body = user_body.ok_or("user body module requires user bodyType evidence")?;
            if body.evidence_photo_ids.is_empty() {
                return Err("user body module requires user bodyType photo evidence".into());
            }
        }
        (completion.body_module_id, completion.body_module_source)
    };

    let mut completion_summary: Vec<String> = traits
        .iter()
        .filter(|value| value.source == TraitSource::AiCompleted)
        .map(|value| trait_key_name(value.key).to_string())
        .collect();
    if body_module_source == TraitSource::AiCompleted {
        completion_summary.push(format!("体型: {body_module_id}"));
    }
    completion_summary.sort();
    completion_summary.dedup();

    let profile = AppearanceProfileV1 {
        schema_version: partial.schema_version,
        species: partial.species.clone(),
        style: partial.style.clone(),
        body_module_id,
        body_module_source,
        traits,
        completion_summary,
    };
    let json = serde_json::to_string(&profile)
        .map_err(|error| format!("serialize finalized appearance profile: {error}"))?;
    parse_appearance_profile_v1(&json)
}

pub fn revision_lock(
    _current: &AppearanceProfileV1,
    requested: &[IdentityTraitKey],
) -> Vec<IdentityTraitKey> {
    ALL_IDENTITY_TRAIT_KEYS
        .iter()
        .copied()
        .filter(|key| !requested.contains(key))
        .collect()
}

pub fn validate_requested_traits(requested: &[IdentityTraitKey]) -> Result<(), String> {
    if requested.is_empty() {
        return Err("requestedTraitKeys must be non-empty for a modification revision".into());
    }
    validate_unique_keys(requested, "requestedTraitKeys")
}

fn validate_unique_keys(keys: &[IdentityTraitKey], field: &str) -> Result<(), String> {
    let mut seen = HashSet::new();
    for key in keys {
        if !seen.insert(*key) {
            return Err(format!(
                "{field} contains duplicate trait: {}",
                trait_key_name(*key)
            ));
        }
    }
    Ok(())
}

fn validate_body_module(value: &str) -> Result<(), String> {
    if matches!(
        value,
        "body-slender-v1" | "body-balanced-v1" | "body-rounded-v1"
    ) {
        Ok(())
    } else {
        Err("bodyModuleId is not supported".into())
    }
}

fn trait_key_name(key: IdentityTraitKey) -> &'static str {
    match key {
        IdentityTraitKey::FaceShape => "faceShape",
        IdentityTraitKey::FaceProportions => "faceProportions",
        IdentityTraitKey::FurColors => "furColors",
        IdentityTraitKey::Markings => "markings",
        IdentityTraitKey::EyeShape => "eyeShape",
        IdentityTraitKey::EyeColor => "eyeColor",
        IdentityTraitKey::EarShape => "earShape",
        IdentityTraitKey::BodyType => "bodyType",
        IdentityTraitKey::Tail => "tail",
        IdentityTraitKey::SignatureMarks => "signatureMarks",
        IdentityTraitKey::Temperament => "temperament",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        finalize_appearance_profile, revision_lock, validate_requested_traits,
        AppearanceCompletionV1,
    };
    use crate::creation::photo_avatar::domain::{
        AppearanceProfileV1, IdentityTraitKey, IdentityTraitV1, TraitSource,
    };

    fn identity_trait(key: IdentityTraitKey, value: &str, source: TraitSource) -> IdentityTraitV1 {
        IdentityTraitV1 {
            key,
            value: value.into(),
            source,
            evidence_photo_ids: if source == TraitSource::User {
                vec!["front".into()]
            } else {
                Vec::new()
            },
        }
    }

    fn face_only_partial() -> AppearanceProfileV1 {
        AppearanceProfileV1 {
            schema_version: 1,
            species: "cat".into(),
            style: "animated-film-soft-v1".into(),
            body_module_id: "body-balanced-v1".into(),
            body_module_source: TraitSource::AiCompleted,
            traits: vec![identity_trait(
                IdentityTraitKey::FaceShape,
                "round",
                TraitSource::User,
            )],
            completion_summary: Vec::new(),
        }
    }

    fn completion() -> AppearanceCompletionV1 {
        AppearanceCompletionV1 {
            requested_trait_keys: Vec::new(),
            completed_traits: super::ALL_IDENTITY_TRAIT_KEYS
                .iter()
                .copied()
                .filter(|key| *key != IdentityTraitKey::FaceShape)
                .map(|key| identity_trait(key, "completed", TraitSource::AiCompleted))
                .collect(),
            body_module_id: "body-rounded-v1".into(),
            body_module_source: TraitSource::AiCompleted,
        }
    }

    #[test]
    fn body_without_photo_evidence_is_explicitly_ai_completed_not_balanced_by_default() {
        let profile = finalize_appearance_profile(&face_only_partial(), completion()).unwrap();

        assert_eq!(profile.body_module_id, "body-rounded-v1");
        assert_eq!(profile.body_module_source, TraitSource::AiCompleted);
        assert!(profile
            .completion_summary
            .iter()
            .any(|value| value.contains("体型")));
    }

    #[test]
    fn revision_lock_is_the_complement_of_provider_requested_traits() {
        let lock = revision_lock(&face_only_partial(), &[IdentityTraitKey::Tail]);

        assert!(lock.contains(&IdentityTraitKey::FaceShape));
        assert!(!lock.contains(&IdentityTraitKey::Tail));
        assert_eq!(lock.len(), 10);
    }

    #[test]
    fn completion_is_strict_camel_case_and_rejects_unknown_fields() {
        let value = serde_json::to_value(completion()).unwrap();
        assert!(value.get("requestedTraitKeys").is_some());
        assert!(value.get("completedTraits").is_some());
        let mut unknown = value.as_object().unwrap().clone();
        unknown.insert("lockedTraitKeys".into(), serde_json::json!([]));

        assert!(serde_json::from_value::<AppearanceCompletionV1>(unknown.into()).is_err());
    }

    #[test]
    fn user_traits_are_preserved_and_output_is_complete_unique_and_stably_sorted() {
        let mut completed = completion();
        completed.completed_traits.push(identity_trait(
            IdentityTraitKey::FaceShape,
            "provider override",
            TraitSource::AiCompleted,
        ));

        let profile = finalize_appearance_profile(&face_only_partial(), completed).unwrap();

        assert_eq!(profile.traits.len(), 11);
        assert_eq!(profile.traits[0].key, IdentityTraitKey::FaceShape);
        assert_eq!(profile.traits[0].value, "round");
        assert_eq!(profile.traits[0].source, TraitSource::User);
        let mut summary = profile.completion_summary.clone();
        summary.sort();
        assert_eq!(profile.completion_summary, summary);
    }

    #[test]
    fn missing_trait_cannot_be_silently_defaulted() {
        let mut completed = completion();
        completed
            .completed_traits
            .retain(|value| value.key != IdentityTraitKey::Tail);

        let error = finalize_appearance_profile(&face_only_partial(), completed).unwrap_err();

        assert!(error.contains("missing identity trait: tail"), "{error}");
    }

    #[test]
    fn user_body_module_requires_body_type_photo_evidence() {
        let mut partial = face_only_partial();
        partial.body_module_source = TraitSource::User;

        let error = finalize_appearance_profile(&partial, completion()).unwrap_err();

        assert!(error.contains("user body module requires user bodyType evidence"));
    }

    #[test]
    fn modification_requested_traits_must_be_non_empty_and_unique() {
        assert!(validate_requested_traits(&[]).is_err());
        assert!(validate_requested_traits(&[IdentityTraitKey::Tail]).is_ok());
        assert!(
            validate_requested_traits(&[IdentityTraitKey::Tail, IdentityTraitKey::Tail,])
                .unwrap_err()
                .contains("duplicate")
        );
    }
}
