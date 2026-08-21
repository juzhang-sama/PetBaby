use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

pub use super::pixel_contract::{
    PixelAlphaReportV1, PixelAvatarAudit, PixelAvatarAuditV1, PixelAvatarAuditV2,
};

pub const PHOTO_AVATAR_CONSENT_VERSION: &str = "photo-avatar-third-party-ai-lk888-no-delete-v2";
pub const DEFAULT_PIXEL_STYLE_ID: PixelStyleProfileId = PixelStyleProfileId::V2AnimationReady;
pub const PHOTO_AVATAR_DISCLOSURE_TEXT: &str = concat!(
    "provider=lk888.ai\n",
    "identity_analysis=gpt-4o\n",
    "appearance_completion=gpt-4o\n",
    "texture_generation=gpt-image-2\n",
    "upstream_public_delete_api=unsupported\n",
    "provider_retention_policy=unverified\n",
    "provider_privacy_policy_version=unverified\n",
    "owned_domain_terminal_deletion=required\n",
);
// 授权披露文本（上方 PHOTO_AVATAR_DISCLOSURE_TEXT）的 sha256，用于固定披露版本。
// 若改动披露文本内容，必须同步此常量（有测试 v2_disclosure_hash_is_derived... 兜底）。
pub const PHOTO_AVATAR_DISCLOSURE_SHA256: &str =
    "fa6ad319cea369bb51349b9b16d11544ecab71ba0bbb027c32b624f72c86a3be";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PhotoAvatarStep {
    Collecting,
    AnalyzeIdentity,
    CompleteAppearance,
    RenderTextureAtlas,
    BuildV5,
    RuntimeCheckPending,
    PreviewReady,
    CleanupPending,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PhotoAvatarErrorCode {
    InvalidInput,
    Auth,
    Quota,
    ContentPolicy,
    Unsupported,
    Network,
    Timeout,
    Provider5xx,
    TemporaryUnavailable,
    LocalStorage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CanonicalTextureAuditV1 {
    pub schema_version: u32,
    pub session_id: String,
    pub revision: u32,
    pub attempt: u8,
    pub provider: String,
    pub provider_model: String,
    pub model_display_name: String,
    pub api_contract_version: String,
    pub privacy_policy_version: String,
    pub retention_policy: String,
    pub upstream_delete_api: String,
    pub provider_task_id: String,
    pub provider_raw_sha256: String,
    pub canonical_sha256: String,
    pub body_module_id: String,
    pub module_contract_sha256: String,
    pub source_texture_sha256: String,
    pub source_alpha_sha256: String,
    pub work_canvas_sha256: String,
    pub region_map_sha256: String,
    pub composer_version: String,
    pub png_encoder_version: String,
    pub coverage_report: serde_json::Value,
    pub status: String,
    pub error_code: Option<String>,
    pub created_at: String,
    pub completed_at: String,
}

impl CanonicalTextureAuditV1 {
    pub fn validate_success(&self) -> Result<(), String> {
        if self.schema_version != 1
            || self.provider != "lk888"
            || self.provider_model != "gpt-image-2"
            || self.model_display_name != "GPT-image-2.0"
            || self.api_contract_version != "lk888-media-generate-v1"
            || self.privacy_policy_version != "unverified"
            || self.retention_policy != "unverified"
            || self.upstream_delete_api != "unsupported"
            || self.composer_version != "deterministic-alpha-v1"
            || self.status != "succeeded"
            || self.error_code.is_some()
        {
            return Err("canonical texture audit fixed metadata is invalid".into());
        }
        if self.session_id.trim().is_empty()
            || self.attempt == 0
            || self.attempt > 3
            || self.provider_task_id.trim().is_empty()
            || self.png_encoder_version.trim().is_empty()
            || self.created_at.trim().is_empty()
            || self.completed_at.trim().is_empty()
            || !self.coverage_report.is_object()
        {
            return Err("canonical texture audit fields are invalid".into());
        }
        if !matches!(
            self.body_module_id.as_str(),
            "body-slender-v1" | "body-balanced-v1" | "body-rounded-v1"
        ) {
            return Err("canonical texture audit body module is invalid".into());
        }
        for value in [
            &self.provider_raw_sha256,
            &self.canonical_sha256,
            &self.module_contract_sha256,
            &self.source_texture_sha256,
            &self.source_alpha_sha256,
            &self.work_canvas_sha256,
            &self.region_map_sha256,
        ] {
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err("canonical texture audit sha256 is invalid".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TraitSource {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "ai-completed")]
    AiCompleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PixelIdentityTraitKey {
    FaceShape,
    FaceProportions,
    EyeShape,
    EyeColor,
    EarShape,
    PrimaryFurColor,
    SecondaryFurColor,
    FaceMarkings,
    ChestMarkings,
    PawMarkings,
    BodyMarkings,
    TailShape,
    TailMarkings,
    SignatureMarks,
    Temperament,
}

impl PixelIdentityTraitKey {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::FaceShape => "faceShape",
            Self::FaceProportions => "faceProportions",
            Self::EyeShape => "eyeShape",
            Self::EyeColor => "eyeColor",
            Self::EarShape => "earShape",
            Self::PrimaryFurColor => "primaryFurColor",
            Self::SecondaryFurColor => "secondaryFurColor",
            Self::FaceMarkings => "faceMarkings",
            Self::ChestMarkings => "chestMarkings",
            Self::PawMarkings => "pawMarkings",
            Self::BodyMarkings => "bodyMarkings",
            Self::TailShape => "tailShape",
            Self::TailMarkings => "tailMarkings",
            Self::SignatureMarks => "signatureMarks",
            Self::Temperament => "temperament",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PixelIdentityTraitV1 {
    pub key: PixelIdentityTraitKey,
    pub value: String,
    pub source: TraitSource,
    pub evidence_photo_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PixelStyleProfileId {
    #[serde(rename = "pixel-style-v1")]
    V1,
    #[serde(rename = "pixel-style-v2-animation-ready")]
    V2AnimationReady,
}

impl PixelStyleProfileId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V1 => "pixel-style-v1",
            Self::V2AnimationReady => "pixel-style-v2-animation-ready",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pixel-style-v1" => Ok(Self::V1),
            "pixel-style-v2-animation-ready" => Ok(Self::V2AnimationReady),
            _ => Err("styleProfileId is not supported".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PixelAppearanceProfileV1 {
    pub schema_version: u32,
    pub species: String,
    pub style_profile_id: PixelStyleProfileId,
    pub traits: Vec<PixelIdentityTraitV1>,
    pub completion_summary: Vec<PixelIdentityTraitKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PixelPhotoAvatarStep {
    Collecting,
    AnalyzeIdentity,
    GeneratePixelAvatar,
    QualityCheckPending,
    RuntimeCheckPending,
    PreviewReady,
    CleanupPending,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PixelRemoteStep {
    AnalyzeIdentity,
    GeneratePixelAvatar,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PixelPhotoAvatarSnapshot {
    pub route: String,
    pub style_profile_id: PixelStyleProfileId,
    pub session_id: String,
    pub revision: u32,
    pub step: PixelPhotoAvatarStep,
    pub provider_job_id: Option<String>,
    pub profile: Option<PixelAppearanceProfileV1>,
    pub attempts: BTreeMap<PixelRemoteStep, u32>,
    pub error_code: Option<PhotoAvatarErrorCode>,
    pub error_message: Option<String>,
}

pub fn parse_pixel_appearance_profile_v1(json: &str) -> Result<PixelAppearanceProfileV1, String> {
    let mut profile: PixelAppearanceProfileV1 = serde_json::from_str(json)
        .map_err(|error| format!("invalid pixel appearance profile: {error}"))?;
    if profile.schema_version != 1 {
        return Err("schemaVersion must be 1".into());
    }
    if profile.species != "cat" {
        return Err("species must be cat".into());
    }
    let mut summary = HashSet::new();
    for (index, key) in profile.completion_summary.iter().enumerate() {
        if !summary.insert(*key) {
            return Err(format!("duplicate completionSummary key at index {index}"));
        }
    }
    let mut seen = HashSet::new();
    for (index, identity_trait) in profile.traits.iter_mut().enumerate() {
        if !seen.insert(identity_trait.key) {
            return Err(format!(
                "duplicate trait key: {}",
                identity_trait.key.as_str()
            ));
        }
        normalize_non_empty(&mut identity_trait.value, &format!("traits[{index}].value"))?;
        for (photo_index, photo_id) in identity_trait.evidence_photo_ids.iter_mut().enumerate() {
            normalize_non_empty(
                photo_id,
                &format!("traits[{index}].evidencePhotoIds[{photo_index}]"),
            )?;
        }
        if identity_trait.source == TraitSource::User
            && identity_trait.evidence_photo_ids.is_empty()
        {
            return Err(format!(
                "traits[{index}].evidencePhotoIds must contain at least one photo id"
            ));
        }
        if identity_trait.source == TraitSource::AiCompleted
            && !summary.contains(&identity_trait.key)
        {
            return Err(format!(
                "completionSummary must include ai-completed trait: {}",
                identity_trait.key.as_str()
            ));
        }
    }
    profile.species = profile.species.trim().to_string();
    Ok(profile)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IdentityTraitKey {
    FaceShape,
    FaceProportions,
    FurColors,
    Markings,
    EyeShape,
    EyeColor,
    EarShape,
    BodyType,
    Tail,
    SignatureMarks,
    Temperament,
}

impl IdentityTraitKey {
    fn as_str(self) -> &'static str {
        match self {
            Self::FaceShape => "faceShape",
            Self::FaceProportions => "faceProportions",
            Self::FurColors => "furColors",
            Self::Markings => "markings",
            Self::EyeShape => "eyeShape",
            Self::EyeColor => "eyeColor",
            Self::EarShape => "earShape",
            Self::BodyType => "bodyType",
            Self::Tail => "tail",
            Self::SignatureMarks => "signatureMarks",
            Self::Temperament => "temperament",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentityTraitV1 {
    pub key: IdentityTraitKey,
    pub value: String,
    pub source: TraitSource,
    pub evidence_photo_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppearanceProfileV1 {
    pub schema_version: u32,
    pub species: String,
    pub style: String,
    pub body_module_id: String,
    pub body_module_source: TraitSource,
    pub traits: Vec<IdentityTraitV1>,
    pub completion_summary: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PhotoAvatarAttemptStep {
    AnalyzeIdentity,
    CompleteAppearance,
    RenderTextureAtlas,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhotoAvatarSnapshot {
    pub session_id: String,
    pub revision: u32,
    pub step: PhotoAvatarStep,
    pub provider_job_id: Option<String>,
    pub profile: Option<AppearanceProfileV1>,
    pub attempts: BTreeMap<PhotoAvatarAttemptStep, u32>,
    pub error_code: Option<PhotoAvatarErrorCode>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhotoAvatarRevisionRequest {
    pub instruction: String,
    pub locked_trait_keys: Vec<IdentityTraitKey>,
}

pub fn parse_appearance_profile_v1(json: &str) -> Result<AppearanceProfileV1, String> {
    let mut profile: AppearanceProfileV1 = serde_json::from_str(json)
        .map_err(|error| format!("invalid appearance profile: {error}"))?;
    if profile.schema_version != 1 {
        return Err("schemaVersion must be 1".into());
    }
    if profile.species != "cat" {
        return Err("species must be cat".into());
    }
    if profile.style != "animated-film-soft-v1" {
        return Err("style must be animated-film-soft-v1".into());
    }
    if !matches!(
        profile.body_module_id.as_str(),
        "body-slender-v1" | "body-balanced-v1" | "body-rounded-v1"
    ) {
        return Err("bodyModuleId is not supported".into());
    }

    normalize_non_empty(&mut profile.species, "species")?;
    normalize_non_empty(&mut profile.style, "style")?;
    normalize_non_empty(&mut profile.body_module_id, "bodyModuleId")?;
    for (index, summary) in profile.completion_summary.iter_mut().enumerate() {
        normalize_non_empty(summary, &format!("completionSummary[{index}]"))?;
    }

    let mut seen = HashSet::new();
    for (index, identity_trait) in profile.traits.iter_mut().enumerate() {
        if !seen.insert(identity_trait.key) {
            return Err(format!(
                "duplicate trait key: {}",
                identity_trait.key.as_str()
            ));
        }
        normalize_non_empty(&mut identity_trait.value, &format!("traits[{index}].value"))?;
        for (photo_index, photo_id) in identity_trait.evidence_photo_ids.iter_mut().enumerate() {
            normalize_non_empty(
                photo_id,
                &format!("traits[{index}].evidencePhotoIds[{photo_index}]"),
            )?;
        }
        if identity_trait.source == TraitSource::User
            && identity_trait.evidence_photo_ids.is_empty()
        {
            return Err(format!(
                "traits[{index}].evidencePhotoIds must contain at least one photo id"
            ));
        }
        if identity_trait.source == TraitSource::AiCompleted
            && !profile
                .completion_summary
                .iter()
                .any(|entry| entry == identity_trait.key.as_str())
        {
            return Err(format!(
                "completionSummary must include ai-completed trait: {}",
                identity_trait.key.as_str()
            ));
        }
    }
    Ok(profile)
}

pub fn parse_photo_avatar_error_code(json: &str) -> Result<PhotoAvatarErrorCode, String> {
    serde_json::from_str(json).map_err(|error| format!("invalid photo avatar error code: {error}"))
}

pub fn parse_photo_avatar_revision_request(
    json: &str,
) -> Result<PhotoAvatarRevisionRequest, String> {
    let mut request: PhotoAvatarRevisionRequest = serde_json::from_str(json)
        .map_err(|error| format!("invalid photo avatar revision request: {error}"))?;
    normalize_non_empty(&mut request.instruction, "instruction")?;
    request.locked_trait_keys.sort_by_key(|key| key.as_str());
    request.locked_trait_keys.dedup();
    Ok(request)
}

pub fn validate_revision_lock(
    before: &AppearanceProfileV1,
    after: &AppearanceProfileV1,
    locked: &[IdentityTraitKey],
) -> Result<(), String> {
    if locked.contains(&IdentityTraitKey::BodyType)
        && (before.body_module_id != after.body_module_id
            || before.body_module_source != after.body_module_source)
    {
        return Err("locked body module changed".into());
    }
    for key in locked {
        let before_trait = before
            .traits
            .iter()
            .find(|identity_trait| identity_trait.key == *key);
        let after_trait = after
            .traits
            .iter()
            .find(|identity_trait| identity_trait.key == *key);
        let unchanged = match (before_trait, after_trait) {
            (Some(before_trait), Some(after_trait)) => {
                before_trait.value == after_trait.value
                    && before_trait.source == after_trait.source
                    && before_trait.evidence_photo_ids == after_trait.evidence_photo_ids
            }
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            return Err(format!("locked trait changed: {}", key.as_str()));
        }
    }
    Ok(())
}

fn normalize_non_empty(value: &mut String, path: &str) -> Result<(), String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(format!("{path} must be a non-empty string"));
    }
    if normalized.len() != value.len() {
        *value = normalized.to_string();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        parse_appearance_profile_v1, parse_photo_avatar_error_code,
        parse_photo_avatar_revision_request, parse_pixel_appearance_profile_v1,
        validate_revision_lock, IdentityTraitKey, PixelStyleProfileId,
        PHOTO_AVATAR_DISCLOSURE_SHA256, PHOTO_AVATAR_DISCLOSURE_TEXT,
    };
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    fn valid_profile() -> Value {
        json!({
            "schemaVersion": 1,
            "species": "cat",
            "style": "animated-film-soft-v1",
            "bodyModuleId": "body-balanced-v1",
            "bodyModuleSource": "ai-completed",
            "traits": [
                {
                    "key": "faceShape",
                    "value": "round",
                    "source": "user",
                    "evidencePhotoIds": ["photo-front"]
                },
                {
                    "key": "bodyType",
                    "value": "balanced",
                    "source": "ai-completed",
                    "evidencePhotoIds": []
                }
            ],
            "completionSummary": ["bodyType"]
        })
    }

    #[test]
    fn appearance_profile_requires_explicit_body_provenance_and_unique_traits() {
        let mut without_source = valid_profile();
        without_source
            .as_object_mut()
            .unwrap()
            .remove("bodyModuleSource");
        let error = parse_appearance_profile_v1(&without_source.to_string()).unwrap_err();
        assert!(error.contains("bodyModuleSource"));

        let mut duplicate = valid_profile();
        duplicate["traits"] = json!([
            {
                "key": "eyeColor",
                "value": "green",
                "source": "user",
                "evidencePhotoIds": ["photo-front"]
            },
            {
                "key": "eyeColor",
                "value": "amber",
                "source": "user",
                "evidencePhotoIds": ["photo-side"]
            }
        ]);
        duplicate["completionSummary"] = json!([]);
        assert!(parse_appearance_profile_v1(&duplicate.to_string()).is_err());
    }

    #[test]
    fn revision_request_is_strict_and_normalizes_locked_keys() {
        let request = parse_photo_avatar_revision_request(
            r#"{"instruction":"  make the tail fluffier  ","lockedTraitKeys":["markings","faceShape","markings"]}"#,
        )
        .unwrap();
        assert_eq!(request.instruction, "make the tail fluffier");
        assert_eq!(
            request.locked_trait_keys,
            vec![IdentityTraitKey::FaceShape, IdentityTraitKey::Markings]
        );
        assert!(parse_photo_avatar_revision_request(
            r#"{"instruction":"change eyes","lockedTraitKeys":[],"sessionId":"session-1"}"#
        )
        .is_err());
    }

    #[test]
    fn revision_lock_preserves_value_source_and_evidence() {
        let before = parse_appearance_profile_v1(&valid_profile().to_string()).unwrap();
        let mut changed = valid_profile();
        changed["traits"][0]["value"] = json!("triangular");
        let after = parse_appearance_profile_v1(&changed.to_string()).unwrap();
        assert_eq!(
            validate_revision_lock(&before, &after, &[IdentityTraitKey::FaceShape]),
            Err("locked trait changed: faceShape".to_string())
        );
    }

    #[test]
    fn error_code_parser_accepts_only_the_frozen_union() {
        assert!(parse_photo_avatar_error_code(r#""temporaryUnavailable""#).is_ok());
        assert!(parse_photo_avatar_error_code(r#""rateLimited""#).is_err());
    }

    #[test]
    fn v2_disclosure_hash_is_derived_from_the_canonical_auditable_text() {
        assert!(PHOTO_AVATAR_DISCLOSURE_TEXT.contains("provider=lk888.ai"));
        assert!(PHOTO_AVATAR_DISCLOSURE_TEXT.contains("identity_analysis=gpt-4o"));
        assert!(PHOTO_AVATAR_DISCLOSURE_TEXT.contains("appearance_completion=gpt-4o"));
        assert!(PHOTO_AVATAR_DISCLOSURE_TEXT.contains("texture_generation=gpt-image-2"));
        assert!(PHOTO_AVATAR_DISCLOSURE_TEXT.contains("upstream_public_delete_api=unsupported"));
        assert!(PHOTO_AVATAR_DISCLOSURE_TEXT.contains("provider_retention_policy=unverified"));
        assert!(PHOTO_AVATAR_DISCLOSURE_TEXT.contains("provider_privacy_policy_version=unverified"));
        assert!(PHOTO_AVATAR_DISCLOSURE_TEXT.contains("owned_domain_terminal_deletion=required"));
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(PHOTO_AVATAR_DISCLOSURE_TEXT.as_bytes())
            ),
            PHOTO_AVATAR_DISCLOSURE_SHA256
        );
    }

    #[test]
    fn pixel_profile_parser_accepts_both_supported_style_ids() {
        for (style, expected) in [
            ("pixel-style-v1", PixelStyleProfileId::V1),
            (
                "pixel-style-v2-animation-ready",
                PixelStyleProfileId::V2AnimationReady,
            ),
        ] {
            let profile = json!({
                "schemaVersion": 1,
                "species": "cat",
                "styleProfileId": style,
                "traits": [],
                "completionSummary": []
            });
            assert_eq!(
                parse_pixel_appearance_profile_v1(&profile.to_string())
                    .unwrap()
                    .style_profile_id,
                expected
            );
        }
    }

    #[test]
    fn new_pixel_revision_default_is_v2_after_cutover() {
        assert_eq!(
            super::DEFAULT_PIXEL_STYLE_ID,
            PixelStyleProfileId::V2AnimationReady
        );
    }
}
