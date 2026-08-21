use super::domain::{
    AppearanceProfileV1, CanonicalTextureAuditV1, IdentityTraitKey, PhotoAvatarErrorCode,
    PixelAppearanceProfileV1, PixelAvatarAudit, PixelIdentityTraitKey, PixelRemoteStep,
    PixelStyleProfileId, PHOTO_AVATAR_CONSENT_VERSION,
};
use super::profile::AppearanceCompletionV1;
use super::store::{RemoteJob, RemoteStep};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{de::Error as _, Deserialize, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoAvatarError {
    pub code: PhotoAvatarErrorCode,
    pub retryable: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CleanupState {
    Deleted,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpstreamCleanupState {
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCleanupOutcome {
    pub backend_cleanup: CleanupState,
    pub upstream_cleanup: UpstreamCleanupState,
    pub provider: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderCleanupOutcomeWire {
    backend_cleanup: CleanupState,
    upstream_cleanup: UpstreamCleanupState,
    provider: String,
}

impl<'de> Deserialize<'de> for ProviderCleanupOutcome {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ProviderCleanupOutcomeWire::deserialize(deserializer)?;
        if wire.provider != "lk888" {
            return Err(D::Error::custom("cleanup provider must be lk888"));
        }
        Ok(Self {
            backend_cleanup: wire.backend_cleanup,
            upstream_cleanup: wire.upstream_cleanup,
            provider: wire.provider,
        })
    }
}

impl ProviderCleanupOutcome {
    pub fn has_retryable_cleanup(&self) -> bool {
        self.backend_cleanup == CleanupState::Pending
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSourceImage {
    pub source_id: String,
    pub png_base64: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct ProviderStepRequest {
    pub session_id: String,
    pub revision: u32,
    pub provider_session_id: Option<String>,
    pub step: RemoteStep,
    pub attempt: u8,
    pub consent_version: String,
    pub source_images: Vec<ProviderSourceImage>,
    pub profile: Option<AppearanceProfileV1>,
    pub body_module_contract_sha256: Option<String>,
    pub modification: Option<String>,
    pub locked_traits: Vec<IdentityTraitKey>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PixelProviderStepRequest {
    pub route: String,
    pub style_profile_id: PixelStyleProfileId,
    pub session_id: String,
    pub revision: u32,
    pub provider_session_id: Option<String>,
    #[serde(serialize_with = "serialize_pixel_step")]
    pub step: PixelRemoteStep,
    pub attempt: u8,
    pub consent_version: String,
    pub source_images: Vec<ProviderSourceImage>,
    pub profile: Option<PixelAppearanceProfileV1>,
    pub modification: Option<String>,
    pub locked_traits: Vec<PixelIdentityTraitKey>,
}

fn serialize_pixel_step<S>(step: &PixelRemoteStep, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(match step {
        PixelRemoteStep::AnalyzeIdentity => "analyzeIdentity",
        PixelRemoteStep::GeneratePixelAvatar => "generatePixelAvatar",
    })
}

impl Serialize for ProviderStepRequest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("ProviderStepRequest", 11)?;
        out.serialize_field("sessionId", &self.session_id)?;
        out.serialize_field("revision", &self.revision)?;
        out.serialize_field("providerSessionId", &self.provider_session_id)?;
        out.serialize_field("step", self.step_name())?;
        out.serialize_field("attempt", &self.attempt)?;
        out.serialize_field("consentVersion", &self.consent_version)?;
        out.serialize_field("sourceImages", &self.source_images)?;
        out.serialize_field("profile", &self.profile)?;
        out.serialize_field(
            "bodyModuleContractSha256",
            &self.body_module_contract_sha256,
        )?;
        out.serialize_field("modification", &self.modification)?;
        out.serialize_field("lockedTraits", &self.locked_traits)?;
        out.end()
    }
}
impl ProviderStepRequest {
    fn step_name(&self) -> &'static str {
        match self.step {
            RemoteStep::AnalyzeIdentity => "analyzeIdentity",
            RemoteStep::CompleteAppearance => "completeAppearance",
            RemoteStep::RenderTextureAtlas => "renderTextureAtlas",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteJobWire {
    #[serde(rename = "providerSessionId")]
    provider_session_id: Option<String>,
    #[serde(rename = "jobId")]
    provider_job_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteJobState {
    pub state: String,
    pub result: Option<ProviderStepResult>,
    pub error: Option<RemoteError>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum ProviderStepResult {
    Identity {
        partial_profile: AppearanceProfileV1,
    },
    Appearance {
        completion: AppearanceCompletionV1,
    },
    LegacyAppearance {
        profile: AppearanceProfileV1,
    },
    TextureAtlas {
        artifact_url: String,
        sha256: String,
        width: u32,
        height: u32,
        audit: CanonicalTextureAuditV1,
    },
    PixelIdentity {
        partial_profile: PixelAppearanceProfileV1,
    },
    PixelAvatar {
        artifact_url: String,
        sha256: String,
        width: u32,
        height: u32,
        audit: PixelAvatarAudit,
    },
}

#[derive(Deserialize)]
#[serde(
    tag = "resultType",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ProviderStepResultWire {
    Identity {
        partial_profile: AppearanceProfileV1,
    },
    Appearance {
        #[serde(default)]
        completion: Option<AppearanceCompletionV1>,
        #[serde(default)]
        profile: Option<AppearanceProfileV1>,
    },
    TextureAtlas {
        artifact_url: String,
        sha256: String,
        width: u32,
        height: u32,
        audit: CanonicalTextureAuditV1,
    },
    PixelIdentity {
        partial_profile: PixelAppearanceProfileV1,
    },
    PixelAvatar {
        artifact_url: String,
        sha256: String,
        width: u32,
        height: u32,
        audit: PixelAvatarAudit,
    },
}

const SEMANTIC_LAYER_IDS: [&str; 7] = [
    "body-base",
    "face",
    "eyes-eyelids",
    "ears",
    "chest-forelegs",
    "tail",
    "occlusion-underlay",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SemanticLayerAuditV1 {
    layer_id: String,
    provider_raw_sha256: String,
    canonical_layer_sha256: String,
    mask_sha256: String,
    attempt: u8,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SemanticAtlasAuditV1 {
    identity_reference_sha256: String,
    profile_sha256: String,
    layers: Vec<SemanticLayerAuditV1>,
    canonical_atlas_sha256: String,
    body_module_id: String,
}

impl<'de> Deserialize<'de> for ProviderStepResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match ProviderStepResultWire::deserialize(deserializer)? {
            ProviderStepResultWire::Identity { partial_profile } => {
                Ok(Self::Identity { partial_profile })
            }
            ProviderStepResultWire::Appearance {
                completion: Some(completion),
                profile: None,
            } => Ok(Self::Appearance { completion }),
            ProviderStepResultWire::Appearance {
                completion: None,
                profile: Some(profile),
            } => Ok(Self::LegacyAppearance { profile }),
            ProviderStepResultWire::Appearance { .. } => Err(D::Error::custom(
                "appearance result requires exactly one of completion or profile",
            )),
            ProviderStepResultWire::TextureAtlas {
                artifact_url,
                sha256,
                width,
                height,
                audit,
            } => {
                audit.validate_success().map_err(D::Error::custom)?;
                if audit.canonical_sha256 != sha256 {
                    return Err(D::Error::custom(
                        "canonical texture audit hash does not match result",
                    ));
                }
                validate_semantic_audit(&audit).map_err(D::Error::custom)?;
                Ok(Self::TextureAtlas {
                    artifact_url,
                    sha256,
                    width,
                    height,
                    audit,
                })
            }
            ProviderStepResultWire::PixelIdentity { partial_profile } => {
                Ok(Self::PixelIdentity { partial_profile })
            }
            ProviderStepResultWire::PixelAvatar {
                artifact_url,
                sha256,
                width,
                height,
                audit,
            } => {
                audit.validate_success().map_err(D::Error::custom)?;
                if audit.normalized_sha256() != sha256
                    || audit.width() != width
                    || audit.height() != height
                {
                    return Err(D::Error::custom(
                        "pixel avatar audit does not match result metadata",
                    ));
                }
                Ok(Self::PixelAvatar {
                    artifact_url,
                    sha256,
                    width,
                    height,
                    audit,
                })
            }
        }
    }
}

fn validate_semantic_audit(audit: &CanonicalTextureAuditV1) -> Result<(), String> {
    let semantic: SemanticAtlasAuditV1 = serde_json::from_value(audit.coverage_report.clone())
        .map_err(|error| format!("invalid semantic layer audit: {error}"))?;
    if semantic.canonical_atlas_sha256 != audit.canonical_sha256
        || semantic.body_module_id != audit.body_module_id
        || !is_lower_sha256(&semantic.identity_reference_sha256)
        || !is_lower_sha256(&semantic.profile_sha256)
    {
        return Err("semantic atlas audit binding is invalid".into());
    }
    if semantic.layers.len() != SEMANTIC_LAYER_IDS.len() {
        return Err("semantic atlas audit layer set is invalid".into());
    }
    for (layer, expected_id) in semantic.layers.iter().zip(SEMANTIC_LAYER_IDS) {
        if layer.layer_id != expected_id
            || !(1..=3).contains(&layer.attempt)
            || !is_lower_sha256(&layer.provider_raw_sha256)
            || !is_lower_sha256(&layer.canonical_layer_sha256)
            || !is_lower_sha256(&layer.mask_sha256)
        {
            return Err("semantic atlas audit layer is invalid".into());
        }
    }
    if semantic_audit_digest(&semantic) != audit.provider_raw_sha256 {
        return Err("semantic atlas audit immutable digest mismatch".into());
    }
    Ok(())
}

fn semantic_audit_digest(audit: &SemanticAtlasAuditV1) -> String {
    let mut fields = vec![
        audit.identity_reference_sha256.clone(),
        audit.profile_sha256.clone(),
    ];
    for layer in &audit.layers {
        fields.extend([
            layer.layer_id.clone(),
            layer.provider_raw_sha256.clone(),
            layer.canonical_layer_sha256.clone(),
            layer.mask_sha256.clone(),
            layer.attempt.to_string(),
        ]);
    }
    fields.push(audit.canonical_atlas_sha256.clone());
    fields.push(audit.body_module_id.clone());
    format!("{:x}", Sha256::digest(fields.join("\n").as_bytes()))
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub trait PhotoAvatarProvider: Send + Sync {
    fn submit_step(&self, request: ProviderStepRequest) -> Result<RemoteJob, PhotoAvatarError>;
    fn poll_job(&self, job_id: &str) -> Result<RemoteJobState, PhotoAvatarError>;
    fn cancel_job(&self, job_id: &str) -> Result<(), PhotoAvatarError>;
    fn delete_session(
        &self,
        provider_session_id: &str,
    ) -> Result<ProviderCleanupOutcome, PhotoAvatarError>;
    fn download_artifact(
        &self,
        url: &str,
        expected_sha256: &str,
    ) -> Result<Vec<u8>, PhotoAvatarError>;
}

#[derive(Clone)]
pub struct ControlledBackendProvider {
    base_url: String,
    token: String,
    allow_insecure_loopback: bool,
}

impl ControlledBackendProvider {
    pub fn from_env() -> Result<Self, PhotoAvatarError> {
        let base_url = std::env::var("PHOTO_AVATAR_BACKEND_BASE_URL")
            .map_err(|_| cfg_error("missing backend base url"))?;
        let token = std::env::var("PHOTO_AVATAR_BACKEND_TOKEN")
            .map_err(|_| cfg_error("missing backend token"))?;
        let allow_insecure_loopback =
            std::env::var("PHOTO_AVATAR_ALLOW_INSECURE_LOOPBACK").is_ok_and(|value| value == "1");
        validate_backend_url(&base_url, cfg!(debug_assertions), allow_insecure_loopback)?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').into(),
            token,
            allow_insecure_loopback,
        })
    }
    #[cfg(test)]
    pub fn for_test(base_url: &str) -> Self {
        Self {
            base_url: base_url.into(),
            token: "test".into(),
            allow_insecure_loopback: true,
        }
    }
    pub fn delete_session_with_outcome(
        &self,
        id: &str,
    ) -> Result<ProviderCleanupOutcome, PhotoAvatarError> {
        let (status, bytes) = self.call(
            "DELETE",
            &format!("/v1/photo-avatar/sessions/{}", encode_path_segment(id)?),
            None,
        )?;
        classify_status(status, &bytes).and_then(|body| {
            serde_json::from_slice(&body).map_err(|error| protocol_error(error.to_string()))
        })
    }
    pub fn submit_pixel_step(
        &self,
        request: PixelProviderStepRequest,
    ) -> Result<RemoteJob, PhotoAvatarError> {
        if request.route != "pixel-v1"
            || request.session_id.trim().is_empty()
            || !(1..=3).contains(&request.attempt)
            || request.source_images.is_empty()
            || request.source_images.len() > 8
        {
            return Err(cfg_error("invalid pixel provider request"));
        }
        let body = serde_json::to_vec(&request).map_err(|error| cfg_error(error.to_string()))?;
        let (status, bytes) = self.call("POST", "/v1/photo-avatar/steps", Some(&body))?;
        let wire: RemoteJobWire = classify_status(status, &bytes).and_then(|response| {
            serde_json::from_slice(&response).map_err(|error| protocol_error(error.to_string()))
        })?;
        if wire.provider_job_id.trim().is_empty()
            || wire
                .provider_session_id
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(protocol_error(
                "pixel provider job response is invalid".into(),
            ));
        }
        Ok(RemoteJob {
            provider_session_id: wire.provider_session_id,
            provider_job_id: wire.provider_job_id,
        })
    }
    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>), PhotoAvatarError> {
        let base_url = validate_backend_url(
            &self.base_url,
            cfg!(debug_assertions),
            self.allow_insecure_loopback,
        )?;
        let payload = body.unwrap_or_default().to_vec();
        let url = base_url
            .join(path)
            .map_err(|error| cfg_error(format!("invalid backend path: {error}")))?
            .to_string();
        let method =
            reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| cfg_error(e.to_string()))?;
        let token = self.token.clone();
        std::thread::spawn(move || {
            tauri::async_runtime::block_on(async move {
                let client = reqwest::Client::new();
                let response = client
                    .request(method, url)
                    .bearer_auth(token)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(payload)
                    .send()
                    .await
                    .map_err(|e| net_error(e.to_string()))?;
                let status = response.status().as_u16();
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|e| net_error(e.to_string()))?
                    .to_vec();
                Ok((status, bytes))
            })
        })
        .join()
        .map_err(|_| net_error("provider HTTP worker panicked".into()))?
    }
}

impl PhotoAvatarProvider for ControlledBackendProvider {
    fn submit_step(&self, request: ProviderStepRequest) -> Result<RemoteJob, PhotoAvatarError> {
        validate_step_request(&request)?;
        let body = serde_json::to_vec(&request).map_err(|e| cfg_error(e.to_string()))?;
        let (status, bytes) = self.call("POST", "/v1/photo-avatar/steps", Some(&body))?;
        let job = classify_status(status, &bytes).and_then(|b| {
            serde_json::from_slice::<RemoteJobWire>(&b)
                .map(|wire| RemoteJob {
                    provider_session_id: wire.provider_session_id,
                    provider_job_id: wire.provider_job_id,
                })
                .map_err(|e| protocol_error(e.to_string()))
        })?;
        if job.provider_job_id.trim().is_empty()
            || job
                .provider_session_id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty())
        {
            return Err(protocol_error(
                "provider job response requires non-empty jobId and providerSessionId".into(),
            ));
        }
        Ok(job)
    }
    fn poll_job(&self, job_id: &str) -> Result<RemoteJobState, PhotoAvatarError> {
        let id = encode_path_segment(job_id)?;
        let (s, b) = self.call("GET", &format!("/v1/photo-avatar/jobs/{id}"), None)?;
        let state = classify_status(s, &b)
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| protocol_error(e.to_string())))?;
        validate_remote_job_state(&state)?;
        validate_remote_job_artifact_origin(&state, &self.base_url, self.allow_insecure_loopback)?;
        Ok(state)
    }
    fn cancel_job(&self, job_id: &str) -> Result<(), PhotoAvatarError> {
        let (s, b) = self.call(
            "POST",
            &format!(
                "/v1/photo-avatar/jobs/{}/cancel",
                encode_path_segment(job_id)?
            ),
            Some(b"{}"),
        )?;
        classify_status(s, &b).map(|_| ())
    }
    fn delete_session(&self, id: &str) -> Result<ProviderCleanupOutcome, PhotoAvatarError> {
        self.delete_session_with_outcome(id)
    }
    fn download_artifact(&self, url: &str, expected: &str) -> Result<Vec<u8>, PhotoAvatarError> {
        validate_artifact_url(
            &self.base_url,
            url,
            cfg!(debug_assertions),
            self.allow_insecure_loopback,
        )?;
        let url = url.to_string();
        let token = self.token.clone();
        let bytes = std::thread::spawn(move || {
            tauri::async_runtime::block_on(async move {
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(|e| net_error(e.to_string()))?;
                let mut response = client
                    .get(url)
                    .bearer_auth(token)
                    .send()
                    .await
                    .map_err(|e| net_error(e.to_string()))?;
                let status = response.status().as_u16();
                if response.headers().get(reqwest::header::LOCATION).is_some() {
                    return Err(cfg_error("artifact redirects are not allowed"));
                }
                validate_declared_artifact_size(response.content_length())?;
                let mut body = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|e| net_error(e.to_string()))?
                {
                    append_artifact_chunk(&mut body, &chunk)?;
                }
                classify_status(status, &body)
            })
        })
        .join()
        .map_err(|_| net_error("artifact HTTP worker panicked".into()))??;
        validate_artifact_bytes(&bytes, expected)?;
        Ok(bytes)
    }
}

#[derive(Clone)]
pub struct FakePhotoAvatarProvider {
    outcomes: Arc<Mutex<VecDeque<FakeOutcome>>>,
    jobs: Arc<Mutex<HashMap<String, FakeJobOutcome>>>,
    artifacts: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    next_job: Arc<Mutex<u64>>,
    requests: Arc<Mutex<Vec<ProviderStepRequest>>>,
    cancels: Arc<Mutex<Vec<String>>>,
    deletes: Arc<Mutex<Vec<String>>>,
}
#[derive(Clone)]
pub enum FakeOutcome {
    Running,
    Success { profile: AppearanceProfileV1 },
    Appearance { completion: AppearanceCompletionV1 },
    LegacyAppearance { profile: AppearanceProfileV1 },
    TextureAtlas { bytes: Vec<u8> },
    Error(PhotoAvatarError),
}

#[derive(Clone)]
struct FakeJobOutcome {
    request: ProviderStepRequest,
    outcome: FakeOutcome,
}
impl FakePhotoAvatarProvider {
    pub fn new(outcomes: Vec<FakeOutcome>) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(outcomes.into())),
            jobs: Default::default(),
            artifacts: Default::default(),
            next_job: Arc::new(Mutex::new(1)),
            requests: Default::default(),
            cancels: Default::default(),
            deletes: Default::default(),
        }
    }
    pub fn requests(&self) -> Vec<ProviderStepRequest> {
        self.requests.lock().unwrap().clone()
    }
    pub fn cancellations(&self) -> Vec<String> {
        self.cancels.lock().unwrap().clone()
    }
    pub fn deleted_sessions(&self) -> Vec<String> {
        self.deletes.lock().unwrap().clone()
    }

    pub fn for_body_module(body_module_id: &str) -> Result<Self, PhotoAvatarError> {
        if !matches!(body_module_id, "body-balanced-v1" | "body-rounded-v1") {
            return Err(cfg_error("unsupported fake body module fixture"));
        }
        let profile: AppearanceProfileV1 = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1,
            "species": "cat",
            "style": "animated-film-soft-v1",
            "bodyModuleId": body_module_id,
            "bodyModuleSource": "ai-completed",
            "traits": [],
            "completionSummary": []
        }))
        .map_err(|error| cfg_error(format!("invalid fake profile: {error}")))?;
        let completion: AppearanceCompletionV1 = serde_json::from_value(serde_json::json!({
            "requestedTraitKeys": [],
            "completedTraits": [
                {"key":"faceShape","value":"completed","source":"ai-completed","evidencePhotoIds":[]},
                {"key":"faceProportions","value":"completed","source":"ai-completed","evidencePhotoIds":[]},
                {"key":"furColors","value":"completed","source":"ai-completed","evidencePhotoIds":[]},
                {"key":"markings","value":"completed","source":"ai-completed","evidencePhotoIds":[]},
                {"key":"eyeShape","value":"completed","source":"ai-completed","evidencePhotoIds":[]},
                {"key":"eyeColor","value":"completed","source":"ai-completed","evidencePhotoIds":[]},
                {"key":"earShape","value":"completed","source":"ai-completed","evidencePhotoIds":[]},
                {"key":"bodyType","value":"completed","source":"ai-completed","evidencePhotoIds":[]},
                {"key":"tail","value":"completed","source":"ai-completed","evidencePhotoIds":[]},
                {"key":"signatureMarks","value":"completed","source":"ai-completed","evidencePhotoIds":[]},
                {"key":"temperament","value":"completed","source":"ai-completed","evidencePhotoIds":[]}
            ],
            "bodyModuleId": body_module_id,
            "bodyModuleSource": "ai-completed"
        }))
        .map_err(|error| cfg_error(format!("invalid fake completion: {error}")))?;
        let atlas = build_fake_atlas(body_module_id)?;
        Ok(Self::new(vec![
            FakeOutcome::Success { profile },
            FakeOutcome::Appearance { completion },
            FakeOutcome::TextureAtlas { bytes: atlas },
        ]))
    }
}

fn build_fake_atlas(body_module_id: &str) -> Result<Vec<u8>, PhotoAvatarError> {
    // This fixture is deliberately non-identity-bearing: it must not reuse
    // standard-cat fur, face, eye, or marking pixels in a photo-avatar test.
    // Only the body module's mechanical UV alpha layout is retained.
    let neutral_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../public/cat-character-modules/cat-a-live2d-v1")
        .join(body_module_id)
        .join(format!("{body_module_id}.2048/texture_00.png"));
    let neutral = std::fs::read(&neutral_path).map_err(|error| {
        cfg_error(format!(
            "failed to read fake atlas alpha guide {}: {error}",
            neutral_path.display()
        ))
    })?;
    let mut image = image::load_from_memory_with_format(&neutral, image::ImageFormat::Png)
        .map_err(|error| cfg_error(format!("failed to decode fake atlas alpha guide: {error}")))?
        .to_rgba8();
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let tile_x = (x / 96) % 2;
        let tile_y = (y / 96) % 2;
        let stripe = ((x / 24) + (y / 32)) % 3;
        let color = match (tile_x ^ tile_y, stripe) {
            (0, 0) => [38, 205, 190],
            (0, 1) => [78, 96, 180],
            (0, _) => [220, 92, 154],
            (1, 0) => [246, 184, 76],
            (1, 1) => [94, 188, 120],
            _ => [130, 74, 178],
        };
        let alpha = pixel[3];
        pixel.0[..3].copy_from_slice(if alpha == 0 { &[0, 0, 0] } else { &color });
    }
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|error| cfg_error(format!("failed to build fake atlas: {error}")))?;
    Ok(bytes)
}

fn fake_texture_audit(
    request: &ProviderStepRequest,
    provider_task_id: &str,
    canonical_png: &[u8],
) -> Result<CanonicalTextureAuditV1, PhotoAvatarError> {
    let profile = request
        .profile
        .as_ref()
        .ok_or_else(|| cfg_error("fake texture audit requires a profile"))?;
    let module_contract_sha256 = request
        .body_module_contract_sha256
        .clone()
        .ok_or_else(|| cfg_error("fake texture audit requires a module contract hash"))?;
    let neutral_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../public/cat-character-modules/cat-a-live2d-v1")
        .join(&profile.body_module_id)
        .join(format!("{}.2048/texture_00.png", profile.body_module_id));
    let neutral = std::fs::read(&neutral_path).map_err(|error| {
        cfg_error(format!(
            "failed to read fake audit source texture {}: {error}",
            neutral_path.display()
        ))
    })?;
    let source_alpha = image::load_from_memory_with_format(&neutral, image::ImageFormat::Png)
        .map_err(|error| cfg_error(format!("failed to decode fake audit source: {error}")))?
        .to_rgba8()
        .pixels()
        .map(|pixel| pixel[3])
        .collect::<Vec<_>>();
    let canonical_sha256 = format!("{:x}", Sha256::digest(canonical_png));
    let layers = SEMANTIC_LAYER_IDS
        .into_iter()
        .map(|layer_id| SemanticLayerAuditV1 {
            layer_id: layer_id.into(),
            provider_raw_sha256: format!(
                "{:x}",
                Sha256::digest(format!("fake-provider-{layer_id}").as_bytes())
            ),
            canonical_layer_sha256: format!(
                "{:x}",
                Sha256::digest([canonical_png, layer_id.as_bytes()].concat())
            ),
            mask_sha256: "ea0812149b2bb367eca38438b22a928e1148a5d348d4ad17f0a3c95cb182d404".into(),
            attempt: 1,
        })
        .collect();
    let semantic_audit = SemanticAtlasAuditV1 {
        identity_reference_sha256: request.source_images.first().map_or_else(
            || format!("{:x}", Sha256::digest(b"fake-identity")),
            |image| image.sha256.clone(),
        ),
        profile_sha256: format!("{:x}", Sha256::digest(b"fake-profile")),
        layers,
        canonical_atlas_sha256: canonical_sha256.clone(),
        body_module_id: profile.body_module_id.clone(),
    };
    let immutable_digest = semantic_audit_digest(&semantic_audit);
    let audit = CanonicalTextureAuditV1 {
        schema_version: 1,
        session_id: request.session_id.clone(),
        revision: request.revision,
        attempt: request.attempt,
        provider: "lk888".into(),
        provider_model: "gpt-image-2".into(),
        model_display_name: "GPT-image-2.0".into(),
        api_contract_version: "lk888-media-generate-v1".into(),
        privacy_policy_version: "unverified".into(),
        retention_policy: "unverified".into(),
        upstream_delete_api: "unsupported".into(),
        provider_task_id: provider_task_id.into(),
        provider_raw_sha256: immutable_digest,
        canonical_sha256,
        body_module_id: profile.body_module_id.clone(),
        module_contract_sha256,
        source_texture_sha256: format!("{:x}", Sha256::digest(&neutral)),
        source_alpha_sha256: format!("{:x}", Sha256::digest(&source_alpha)),
        work_canvas_sha256: format!("{:x}", Sha256::digest(b"fake-work-canvas-v1")),
        region_map_sha256: format!("{:x}", Sha256::digest(b"fake-region-map-v1")),
        composer_version: "deterministic-alpha-v1".into(),
        png_encoder_version: "pillow-png-v1".into(),
        coverage_report: serde_json::to_value(semantic_audit)
            .map_err(|error| cfg_error(format!("failed to serialize semantic audit: {error}")))?,
        status: "succeeded".into(),
        error_code: None,
        created_at: "2026-08-17T00:00:00Z".into(),
        completed_at: "2026-08-17T00:00:01Z".into(),
    };
    audit.validate_success().map_err(cfg_error)?;
    Ok(audit)
}

impl PhotoAvatarProvider for FakePhotoAvatarProvider {
    fn submit_step(&self, r: ProviderStepRequest) -> Result<RemoteJob, PhotoAvatarError> {
        validate_step_request(&r)?;
        self.requests.lock().unwrap().push(r.clone());
        let outcome = self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(FakeOutcome::Running);
        let mut next = self.next_job.lock().unwrap();
        let job_id = format!("fake-job-{next}");
        *next += 1;
        self.jobs.lock().unwrap().insert(
            job_id.clone(),
            FakeJobOutcome {
                request: r,
                outcome,
            },
        );
        Ok(RemoteJob {
            provider_session_id: Some("fake-session-1".into()),
            provider_job_id: job_id,
        })
    }
    fn poll_job(&self, job_id: &str) -> Result<RemoteJobState, PhotoAvatarError> {
        let job = self
            .jobs
            .lock()
            .unwrap()
            .get(job_id)
            .cloned()
            .ok_or_else(|| cfg_error("unknown fake job"))?;
        Ok(match job.outcome {
            FakeOutcome::Running => RemoteJobState {
                state: "running".into(),
                result: None,
                error: None,
            },
            FakeOutcome::Success { profile } => {
                let result = match job.request.step {
                    RemoteStep::AnalyzeIdentity => ProviderStepResult::Identity {
                        partial_profile: profile,
                    },
                    RemoteStep::CompleteAppearance => {
                        return Err(cfg_error(
                            "completeAppearance requires an appearance completion fixture",
                        ))
                    }
                    RemoteStep::RenderTextureAtlas => {
                        return Err(cfg_error("renderTextureAtlas requires a texture fixture"))
                    }
                };
                RemoteJobState {
                    state: "succeeded".into(),
                    result: Some(result),
                    error: None,
                }
            }
            FakeOutcome::Appearance { completion } => {
                if job.request.step != RemoteStep::CompleteAppearance {
                    return Err(cfg_error(
                        "appearance completion fixture is only valid for completeAppearance",
                    ));
                }
                RemoteJobState {
                    state: "succeeded".into(),
                    result: Some(ProviderStepResult::Appearance { completion }),
                    error: None,
                }
            }
            FakeOutcome::LegacyAppearance { profile } => {
                if job.request.step != RemoteStep::CompleteAppearance {
                    return Err(cfg_error(
                        "completeAppearance requires an appearance profile fixture",
                    ));
                }
                RemoteJobState {
                    state: "succeeded".into(),
                    result: Some(ProviderStepResult::LegacyAppearance { profile }),
                    error: None,
                }
            }
            FakeOutcome::TextureAtlas { bytes } => {
                if job.request.step != RemoteStep::RenderTextureAtlas {
                    return Err(cfg_error(
                        "texture fixture is only valid for renderTextureAtlas",
                    ));
                }
                let sha256 = format!("{:x}", Sha256::digest(&bytes));
                let url = format!("https://fake.photo-avatar.invalid/artifacts/{job_id}.png");
                let audit = fake_texture_audit(&job.request, job_id, &bytes)?;
                self.artifacts.lock().unwrap().insert(url.clone(), bytes);
                RemoteJobState {
                    state: "succeeded".into(),
                    result: Some(ProviderStepResult::TextureAtlas {
                        artifact_url: url,
                        sha256,
                        width: 2048,
                        height: 2048,
                        audit,
                    }),
                    error: None,
                }
            }
            FakeOutcome::Error(error) => RemoteJobState {
                state: "failed".into(),
                result: None,
                error: Some(RemoteError {
                    code: format_error_code(error.code),
                    message: error.message,
                }),
            },
        })
    }
    fn cancel_job(&self, id: &str) -> Result<(), PhotoAvatarError> {
        self.cancels.lock().unwrap().push(id.into());
        Ok(())
    }
    fn delete_session(&self, id: &str) -> Result<ProviderCleanupOutcome, PhotoAvatarError> {
        self.deletes.lock().unwrap().push(id.into());
        Ok(ProviderCleanupOutcome {
            backend_cleanup: CleanupState::Deleted,
            upstream_cleanup: UpstreamCleanupState::Unsupported,
            provider: "lk888".into(),
        })
    }
    fn download_artifact(&self, url: &str, expected: &str) -> Result<Vec<u8>, PhotoAvatarError> {
        let bytes = self
            .artifacts
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .ok_or_else(|| cfg_error("unknown fake artifact"))?;
        validate_artifact_bytes(&bytes, expected)?;
        Ok(bytes)
    }
}

fn classify_status(status: u16, body: &[u8]) -> Result<Vec<u8>, PhotoAvatarError> {
    if (200..300).contains(&status) {
        Ok(body.to_vec())
    } else {
        Err(provider_error_for_status(status))
    }
}
fn provider_error_for_status(status: u16) -> PhotoAvatarError {
    let (code, retryable) = match status {
        401 | 403 => (PhotoAvatarErrorCode::Auth, false),
        429 => (PhotoAvatarErrorCode::Quota, true),
        400 => (PhotoAvatarErrorCode::InvalidInput, false),
        409 => (PhotoAvatarErrorCode::TemporaryUnavailable, true),
        500..=599 => (PhotoAvatarErrorCode::Provider5xx, true),
        _ => (PhotoAvatarErrorCode::Network, true),
    };
    PhotoAvatarError {
        code,
        retryable,
        message: format!("provider HTTP {status}"),
    }
}
fn cfg_error(message: impl Into<String>) -> PhotoAvatarError {
    PhotoAvatarError {
        code: PhotoAvatarErrorCode::Unsupported,
        retryable: false,
        message: message.into(),
    }
}
fn protocol_error(message: String) -> PhotoAvatarError {
    PhotoAvatarError {
        code: PhotoAvatarErrorCode::InvalidInput,
        retryable: false,
        message,
    }
}
fn net_error(message: String) -> PhotoAvatarError {
    PhotoAvatarError {
        code: PhotoAvatarErrorCode::Network,
        retryable: true,
        message,
    }
}

const MAX_ARTIFACT_BYTES: usize = 20 * 1024 * 1024;

fn validate_declared_artifact_size(content_length: Option<u64>) -> Result<(), PhotoAvatarError> {
    if content_length.is_some_and(|size| size > MAX_ARTIFACT_BYTES as u64) {
        return Err(cfg_error("artifact exceeds 20 MiB"));
    }
    Ok(())
}

fn append_artifact_chunk(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), PhotoAvatarError> {
    if body.len().saturating_add(chunk.len()) > MAX_ARTIFACT_BYTES {
        return Err(cfg_error("artifact exceeds 20 MiB"));
    }
    body.extend_from_slice(chunk);
    Ok(())
}

fn validate_artifact_bytes(bytes: &[u8], expected_sha256: &str) -> Result<(), PhotoAvatarError> {
    if expected_sha256.len() != 64
        || !expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(cfg_error("artifact sha256 must be lowercase hex"));
    }
    if bytes.len() > MAX_ARTIFACT_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(cfg_error("artifact must be PNG <=20 MiB"));
    }
    let image = image::load_from_memory(bytes)
        .map_err(|e| cfg_error(format!("invalid PNG artifact: {e}")))?;
    if image.width() != 2048 || image.height() != 2048 {
        return Err(cfg_error("artifact must be exactly 2048x2048"));
    }
    if format!("{:x}", Sha256::digest(bytes)) != expected_sha256 {
        return Err(cfg_error("artifact sha256 mismatch"));
    }
    Ok(())
}

fn validate_step_request(request: &ProviderStepRequest) -> Result<(), PhotoAvatarError> {
    if request.consent_version != PHOTO_AVATAR_CONSENT_VERSION {
        return Err(cfg_error("explicit photo avatar v2 consent is required"));
    }
    if request.step != RemoteStep::AnalyzeIdentity
        && request
            .provider_session_id
            .as_deref()
            .is_none_or(|id| id.trim().is_empty())
    {
        return Err(cfg_error(
            "provider session is required for subsequent steps",
        ));
    }
    match request.step {
        RemoteStep::AnalyzeIdentity | RemoteStep::RenderTextureAtlas
            if !(1..=8).contains(&request.source_images.len()) =>
        {
            return Err(cfg_error(
                "analyzeIdentity and renderTextureAtlas require 1..8 source images",
            ));
        }
        RemoteStep::CompleteAppearance if !request.source_images.is_empty() => {
            return Err(cfg_error("completeAppearance must not carry source images"));
        }
        _ => {}
    }
    Ok(())
}

fn encode_path_segment(value: &str) -> Result<String, PhotoAvatarError> {
    if value.trim().is_empty() {
        return Err(cfg_error("remote id must be non-empty"));
    }
    Ok(utf8_percent_encode(value, NON_ALPHANUMERIC).to_string())
}

fn validate_backend_url(
    raw_url: &str,
    debug_build: bool,
    allow_insecure_loopback: bool,
) -> Result<reqwest::Url, PhotoAvatarError> {
    let url = reqwest::Url::parse(raw_url).map_err(|_| cfg_error("invalid backend URL"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(cfg_error(
            "backend URL must not contain credentials, query, or fragment",
        ));
    }
    match url.scheme() {
        "https" => Ok(url),
        "http"
            if debug_build
                && allow_insecure_loopback
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost")) =>
        {
            Ok(url)
        }
        "http" => Err(cfg_error(
            "HTTP backend URL requires a debug loopback opt-in",
        )),
        _ => Err(cfg_error("backend base URL must use HTTPS")),
    }
}

fn validate_artifact_url(
    base_url: &str,
    artifact_url: &str,
    debug_build: bool,
    allow_insecure_loopback: bool,
) -> Result<(), PhotoAvatarError> {
    let base = validate_backend_url(base_url, debug_build, allow_insecure_loopback)?;
    let artifact = validate_backend_url(artifact_url, debug_build, allow_insecure_loopback)?;
    if base.scheme() != artifact.scheme()
        || base.host_str() != artifact.host_str()
        || base.port_or_known_default() != artifact.port_or_known_default()
    {
        return Err(cfg_error("artifact URL must use the backend same origin"));
    }
    Ok(())
}

fn validate_remote_job_artifact_origin(
    state: &RemoteJobState,
    base_url: &str,
    allow_insecure_loopback: bool,
) -> Result<(), PhotoAvatarError> {
    if let Some(ProviderStepResult::TextureAtlas { artifact_url, .. }) = &state.result {
        validate_artifact_url(
            base_url,
            artifact_url,
            cfg!(debug_assertions),
            allow_insecure_loopback,
        )?;
    }
    if let Some(ProviderStepResult::PixelAvatar { artifact_url, .. }) = &state.result {
        validate_artifact_url(
            base_url,
            artifact_url,
            cfg!(debug_assertions),
            allow_insecure_loopback,
        )?;
    }
    Ok(())
}

fn validate_remote_job_state(state: &RemoteJobState) -> Result<(), PhotoAvatarError> {
    if let Some(ProviderStepResult::TextureAtlas {
        artifact_url: _,
        sha256,
        width,
        height,
        ..
    }) = &state.result
    {
        if *width != 2048 || *height != 2048 {
            return Err(protocol_error(
                "texture atlas metadata must be 2048x2048".into(),
            ));
        }
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(protocol_error(
                "texture atlas sha256 must be lowercase hex".into(),
            ));
        }
    }
    if let Some(ProviderStepResult::PixelAvatar { audit, .. }) = &state.result {
        audit
            .validate_success()
            .map_err(|error| protocol_error(error))?;
    }
    Ok(())
}

fn format_error_code(code: PhotoAvatarErrorCode) -> String {
    match code {
        PhotoAvatarErrorCode::InvalidInput => "invalidInput",
        PhotoAvatarErrorCode::Auth => "auth",
        PhotoAvatarErrorCode::Quota => "quota",
        PhotoAvatarErrorCode::ContentPolicy => "contentPolicy",
        PhotoAvatarErrorCode::Unsupported => "unsupported",
        PhotoAvatarErrorCode::Network => "network",
        PhotoAvatarErrorCode::Timeout => "timeout",
        PhotoAvatarErrorCode::Provider5xx => "provider5xx",
        PhotoAvatarErrorCode::TemporaryUnavailable => "temporaryUnavailable",
        PhotoAvatarErrorCode::LocalStorage => "localStorage",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        matchers::{header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn request() -> ProviderStepRequest {
        ProviderStepRequest {
            session_id: "s1".into(),
            revision: 1,
            provider_session_id: None,
            step: RemoteStep::AnalyzeIdentity,
            attempt: 1,
            consent_version: PHOTO_AVATAR_CONSENT_VERSION.into(),
            source_images: vec![ProviderSourceImage {
                source_id: "p1".into(),
                png_base64: "iVBORw0KGgo=".into(),
                sha256: "00".repeat(32),
                width: 256,
                height: 256,
            }],
            profile: None,
            body_module_contract_sha256: None,
            modification: None,
            locked_traits: vec![],
        }
    }

    #[tokio::test]
    async fn controlled_backend_rejects_unknown_fields_and_classifies_http_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/photo-avatar/steps"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jobId":"j1","providerSessionId":"r1","extra":true})),
            )
            .mount(&server)
            .await;
        let uri = server.uri();
        let error = std::thread::spawn(move || {
            ControlledBackendProvider::for_test(&uri)
                .submit_step(request())
                .unwrap_err()
        })
        .join()
        .unwrap();
        assert!(error.message.contains("unknown field") || error.message.contains("missing field"));
        assert_eq!(
            provider_error_for_status(401).code,
            PhotoAvatarErrorCode::Auth
        );
        assert_eq!(
            provider_error_for_status(429).code,
            PhotoAvatarErrorCode::Quota
        );
        assert!(provider_error_for_status(503).retryable);
    }

    #[tokio::test]
    async fn controlled_backend_requires_non_empty_remote_session() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/photo-avatar/steps"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jobId":"j1","providerSessionId":""})),
            )
            .mount(&server)
            .await;
        let uri = server.uri();
        let error = std::thread::spawn(move || {
            ControlledBackendProvider::for_test(&uri)
                .submit_step(request())
                .unwrap_err()
        })
        .join()
        .unwrap();
        assert!(error.message.contains("non-empty"));
    }

    #[test]
    fn fake_provider_records_requests_without_network() {
        let fake = FakePhotoAvatarProvider::new(vec![FakeOutcome::Running]);
        let job = fake.submit_step(request()).unwrap();
        fake.cancel_job(&job.provider_job_id).unwrap();
        fake.delete_session("fake-session-1").unwrap();
        assert_eq!(fake.requests().len(), 1);
        assert_eq!(fake.cancellations(), vec!["fake-job-1"]);
        assert_eq!(fake.deleted_sessions(), vec!["fake-session-1"]);
    }

    fn profile() -> AppearanceProfileV1 {
        serde_json::from_str(r#"{"schemaVersion":1,"species":"cat","style":"animated-film-soft-v1","bodyModuleId":"body-balanced-v1","bodyModuleSource":"ai-completed","traits":[],"completionSummary":[]}"#).unwrap()
    }

    fn valid_texture_audit(canonical_sha256: &str) -> serde_json::Value {
        let semantic_audit = SemanticAtlasAuditV1 {
            identity_reference_sha256: "77".repeat(32),
            profile_sha256: "88".repeat(32),
            layers: SEMANTIC_LAYER_IDS
                .into_iter()
                .map(|layer_id| SemanticLayerAuditV1 {
                    layer_id: layer_id.into(),
                    provider_raw_sha256: "99".repeat(32),
                    canonical_layer_sha256: "aa".repeat(32),
                    mask_sha256: "ea0812149b2bb367eca38438b22a928e1148a5d348d4ad17f0a3c95cb182d404"
                        .into(),
                    attempt: 1,
                })
                .collect(),
            canonical_atlas_sha256: canonical_sha256.into(),
            body_module_id: "body-balanced-v1".into(),
        };
        let provider_raw_sha256 = semantic_audit_digest(&semantic_audit);
        json!({
            "schemaVersion": 1,
            "sessionId": "s1",
            "revision": 1,
            "attempt": 1,
            "provider": "lk888",
            "providerModel": "gpt-image-2",
            "modelDisplayName": "GPT-image-2.0",
            "apiContractVersion": "lk888-media-generate-v1",
            "privacyPolicyVersion": "unverified",
            "retentionPolicy": "unverified",
            "upstreamDeleteApi": "unsupported",
            "providerTaskId": "task-1",
            "providerRawSha256": provider_raw_sha256,
            "canonicalSha256": canonical_sha256,
            "bodyModuleId": "body-balanced-v1",
            "moduleContractSha256": "22".repeat(32),
            "sourceTextureSha256": "33".repeat(32),
            "sourceAlphaSha256": "44".repeat(32),
            "workCanvasSha256": "55".repeat(32),
            "regionMapSha256": "66".repeat(32),
            "composerVersion": "deterministic-alpha-v1",
            "pngEncoderVersion": "pillow-11",
            "coverageReport": semantic_audit,
            "status": "succeeded",
            "errorCode": null,
            "createdAt": "2026-08-17T00:00:00Z",
            "completedAt": "2026-08-17T00:00:01Z"
        })
    }

    fn texture_job_with_audit(audit: serde_json::Value) -> serde_json::Value {
        json!({
            "state": "succeeded",
            "result": {
                "resultType": "textureAtlas",
                "artifactUrl": "https://backend.example/artifact.png",
                "sha256": "00".repeat(32),
                "width": 2048,
                "height": 2048,
                "audit": audit
            },
            "error": null
        })
    }

    #[test]
    fn texture_atlas_audit_accepts_canonical_success() {
        let state = serde_json::from_value::<RemoteJobState>(texture_job_with_audit(
            valid_texture_audit(&"00".repeat(32)),
        ));
        assert!(state.is_ok(), "{state:?}");
    }

    #[test]
    fn texture_atlas_audit_rejects_unknown_fields_and_uppercase_hashes() {
        let mut unknown = valid_texture_audit(&"00".repeat(32));
        unknown["unexpected"] = json!(true);
        assert!(serde_json::from_value::<RemoteJobState>(texture_job_with_audit(unknown)).is_err());

        let mut uppercase = valid_texture_audit(&"00".repeat(32));
        uppercase["providerRawSha256"] = json!("AA".repeat(32));
        assert!(
            serde_json::from_value::<RemoteJobState>(texture_job_with_audit(uppercase)).is_err()
        );
    }

    #[test]
    fn texture_atlas_audit_rejects_mismatched_canonical_hash() {
        assert!(
            serde_json::from_value::<RemoteJobState>(texture_job_with_audit(valid_texture_audit(
                &"99".repeat(32)
            ),))
            .is_err()
        );
    }

    #[test]
    fn texture_atlas_audit_rejects_tampered_semantic_layers() {
        for mutation in ["order", "hash", "mask", "attempt"] {
            let mut audit = valid_texture_audit(&"00".repeat(32));
            let layers = audit["coverageReport"]["layers"].as_array_mut().unwrap();
            match mutation {
                "order" => layers.swap(0, 1),
                "hash" => {
                    layers[0]["canonicalLayerSha256"] = json!("AA".repeat(32));
                }
                "mask" => layers[0]["maskSha256"] = json!("00".repeat(32)),
                "attempt" => layers[0]["attempt"] = json!(4),
                _ => unreachable!(),
            }
            assert!(
                serde_json::from_value::<RemoteJobState>(texture_job_with_audit(audit)).is_err(),
                "accepted semantic audit mutation: {mutation}"
            );
        }
    }

    #[test]
    fn texture_atlas_audit_rejects_wrong_fixed_metadata_and_module() {
        for (field, value) in [
            ("provider", json!("openai")),
            ("modelDisplayName", json!("GPT Image")),
            ("apiContractVersion", json!("other")),
            ("bodyModuleId", json!("body-unknown-v1")),
        ] {
            let mut audit = valid_texture_audit(&"00".repeat(32));
            audit[field] = value;
            assert!(
                serde_json::from_value::<RemoteJobState>(texture_job_with_audit(audit)).is_err(),
                "accepted invalid {field}"
            );
        }
    }

    #[test]
    fn appearance_result_requires_the_strict_completion_protocol() {
        let valid = json!({
            "resultType": "appearance",
            "completion": {
                "requestedTraitKeys": ["tail"],
                "completedTraits": [],
                "bodyModuleId": "body-rounded-v1",
                "bodyModuleSource": "ai-completed"
            }
        });
        let result: ProviderStepResult = serde_json::from_value(valid.clone()).unwrap();
        assert!(matches!(
            result,
            ProviderStepResult::Appearance { completion }
                if completion.requested_trait_keys == vec![IdentityTraitKey::Tail]
        ));

        let mut invalid = valid;
        invalid["completion"]["lockedTraitKeys"] = json!(["faceShape"]);
        assert!(serde_json::from_value::<ProviderStepResult>(invalid).is_err());
        let legacy: ProviderStepResult = serde_json::from_value(json!({
            "resultType": "appearance",
            "profile": profile()
        }))
        .unwrap();
        assert!(matches!(
            legacy,
            ProviderStepResult::LegacyAppearance { profile: legacy_profile }
                if legacy_profile == profile()
        ));
        assert!(serde_json::from_value::<ProviderStepResult>(json!({
            "resultType": "appearance",
            "completion": {
                "requestedTraitKeys": [],
                "completedTraits": [],
                "bodyModuleId": "body-rounded-v1",
                "bodyModuleSource": "ai-completed"
            },
            "profile": profile()
        }))
        .is_err());
    }

    #[test]
    fn python_wire_uses_camel_case_result_fields() {
        let identity: ProviderStepResult = serde_json::from_value(json!({
            "resultType": "identity",
            "partialProfile": profile()
        }))
        .unwrap();
        assert!(matches!(
            identity,
            ProviderStepResult::Identity { partial_profile }
                if partial_profile == profile()
        ));

        let texture: ProviderStepResult = serde_json::from_value(json!({
            "resultType": "textureAtlas",
            "artifactUrl": "https://backend.example/artifact.png",
            "sha256": "00".repeat(32),
            "width": 2048,
            "height": 2048,
            "audit": valid_texture_audit(&"00".repeat(32))
        }))
        .unwrap();
        assert!(matches!(
            texture,
            ProviderStepResult::TextureAtlas { artifact_url, .. }
                if artifact_url == "https://backend.example/artifact.png"
        ));
    }

    #[test]
    fn fake_provider_consumes_running_success_and_error_outcomes() {
        let fake = FakePhotoAvatarProvider::new(vec![
            FakeOutcome::Running,
            FakeOutcome::Success { profile: profile() },
            FakeOutcome::Error(PhotoAvatarError {
                code: PhotoAvatarErrorCode::Quota,
                retryable: true,
                message: "quota".into(),
            }),
        ]);
        let jobs = (0..3)
            .map(|_| fake.submit_step(request()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            fake.poll_job(&jobs[0].provider_job_id).unwrap().state,
            "running"
        );
        let success = fake.poll_job(&jobs[1].provider_job_id).unwrap();
        assert!(matches!(
            success.result,
            Some(ProviderStepResult::Identity { .. })
        ));
        let failed = fake.poll_job(&jobs[2].provider_job_id).unwrap();
        assert_eq!(failed.error.unwrap().code, "quota");
    }

    #[test]
    fn subsequent_steps_require_provider_session() {
        let provider = ControlledBackendProvider::for_test("http://127.0.0.1:1");
        let mut complete = request();
        complete.step = RemoteStep::CompleteAppearance;
        complete.source_images.clear();
        let error = provider.submit_step(complete).unwrap_err();
        assert!(error.message.contains("provider session"));

        let mut empty = request();
        empty.step = RemoteStep::CompleteAppearance;
        empty.source_images.clear();
        empty.provider_session_id = Some("  ".into());
        assert!(provider
            .submit_step(empty)
            .unwrap_err()
            .message
            .contains("provider session"));
    }

    #[test]
    fn provider_rejects_legacy_consent_for_a_new_remote_step() {
        let fake = FakePhotoAvatarProvider::new(vec![FakeOutcome::Running]);
        let mut legacy = request();
        legacy.consent_version = "photo-avatar-third-party-ai-v1".into();
        let error = fake.submit_step(legacy).unwrap_err();
        assert!(error.message.contains("consent"));
    }

    #[test]
    fn artifact_validation_rejects_non_2048_png() {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([0, 0, 0, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        assert!(validate_artifact_bytes(&bytes, &hash).is_err());
    }

    #[test]
    fn texture_result_metadata_requires_https_2048_and_lowercase_sha() {
        let invalid = RemoteJobState {
            state: "succeeded".into(),
            result: Some(ProviderStepResult::TextureAtlas {
                artifact_url: "http://example.test/atlas.png".into(),
                sha256: "AA".repeat(32),
                width: 1,
                height: 1,
                audit: serde_json::from_value(valid_texture_audit(&"00".repeat(32))).unwrap(),
            }),
            error: None,
        };
        assert!(validate_remote_job_state(&invalid).is_err());
    }

    #[test]
    fn fake_texture_fixture_is_downloadable_and_hash_checked() {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2048,
            2048,
            image::Rgba([1, 2, 3, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let fake = FakePhotoAvatarProvider::new(vec![FakeOutcome::TextureAtlas {
            bytes: bytes.clone(),
        }]);
        let mut render = request();
        render.step = RemoteStep::RenderTextureAtlas;
        render.provider_session_id = Some("remote-1".into());
        render.profile = Some(profile());
        render.body_module_contract_sha256 = Some("22".repeat(32));
        let job = fake.submit_step(render).unwrap();
        let state = fake.poll_job(&job.provider_job_id).unwrap();
        let ProviderStepResult::TextureAtlas {
            artifact_url,
            sha256,
            width,
            height,
            ..
        } = state.result.unwrap()
        else {
            panic!("expected texture atlas")
        };
        assert_eq!((width, height), (2048, 2048));
        assert_eq!(
            fake.download_artifact(&artifact_url, &sha256).unwrap(),
            bytes
        );
    }

    #[tokio::test]
    async fn artifact_download_accepts_valid_png_and_rejects_redirect() {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2048,
            2048,
            image::Rgba([4, 5, 6, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/atlas.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/redirect.png"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "http://example.test/insecure.png"),
            )
            .mount(&server)
            .await;
        let valid_url = format!("{}/atlas.png", server.uri());
        let redirect_url = format!("{}/redirect.png", server.uri());
        let provider = ControlledBackendProvider::for_test(&server.uri());
        let valid = std::thread::spawn({
            let provider = provider.clone();
            let sha256 = sha256.clone();
            move || provider.download_artifact(&valid_url, &sha256)
        })
        .join()
        .unwrap()
        .unwrap();
        assert_eq!(valid, bytes);
        let error = std::thread::spawn(move || provider.download_artifact(&redirect_url, &sha256))
            .join()
            .unwrap()
            .unwrap_err();
        assert!(error.message.contains("redirect"));
    }

    #[test]
    fn insecure_backend_requires_debug_loopback_and_explicit_opt_in() {
        assert!(validate_backend_url("http://127.0.0.1:8787", true, true).is_ok());
        assert!(validate_backend_url("http://192.168.1.2:8787", true, true).is_err());
        assert!(validate_backend_url("http://127.0.0.1:8787", false, true).is_err());
    }

    #[test]
    fn cleanup_response_keeps_upstream_unsupported_distinct_from_deleted() {
        let wire = r#"{
            "backendCleanup":"deleted",
            "upstreamCleanup":"unsupported",
            "provider":"lk888"
        }"#;
        let outcome: ProviderCleanupOutcome = serde_json::from_str(wire).unwrap();
        assert_eq!(outcome.backend_cleanup, CleanupState::Deleted);
        assert_eq!(outcome.upstream_cleanup, UpstreamCleanupState::Unsupported);
        assert_eq!(outcome.provider, "lk888");
        assert!(!outcome.has_retryable_cleanup());
    }

    #[test]
    fn cleanup_response_rejects_unknown_fields_states_and_provider() {
        for wire in [
            r#"{"backendCleanup":"deleted","upstreamCleanup":"unsupported","provider":"lk888","extra":true}"#,
            r#"{"backendCleanup":"unsupported","upstreamCleanup":"unsupported","provider":"lk888"}"#,
            r#"{"backendCleanup":"deleted","upstreamCleanup":"deleted","provider":"lk888"}"#,
            r#"{"backendCleanup":"deleted","upstreamCleanup":"unsupported","provider":"other"}"#,
        ] {
            assert!(serde_json::from_str::<ProviderCleanupOutcome>(wire).is_err());
        }
    }

    #[tokio::test]
    async fn loopback_artifact_download_is_same_origin_authenticated_and_no_redirect() {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2048,
            2048,
            image::Rgba([7, 8, 9, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/artifact.png"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/redirect.png"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/artifact.png", server.uri())),
            )
            .mount(&server)
            .await;

        let provider = ControlledBackendProvider {
            base_url: server.uri(),
            token: "test-token".into(),
            allow_insecure_loopback: true,
        };
        let artifact_url = format!("{}/artifact.png", provider.base_url);
        let downloaded = std::thread::spawn({
            let provider = provider.clone();
            let sha256 = sha256.clone();
            move || provider.download_artifact(&artifact_url, &sha256)
        })
        .join()
        .unwrap()
        .unwrap();
        assert_eq!(downloaded, bytes);

        let other_origin = MockServer::start().await;
        let cross_origin = format!("{}/artifact.png", other_origin.uri());
        let cross_origin_error = std::thread::spawn({
            let provider = provider.clone();
            let sha256 = sha256.clone();
            move || provider.download_artifact(&cross_origin, &sha256)
        })
        .join()
        .unwrap()
        .unwrap_err();
        assert!(cross_origin_error.message.contains("same origin"));

        let redirect_url = format!("{}/redirect.png", provider.base_url);
        let redirect_error =
            std::thread::spawn(move || provider.download_artifact(&redirect_url, &sha256))
                .join()
                .unwrap()
                .unwrap_err();
        assert!(redirect_error.message.contains("redirect"));
    }

    #[tokio::test]
    async fn delete_session_parses_the_strict_cleanup_outcome() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/v1/photo-avatar/sessions/session1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "backendCleanup": "pending",
                "upstreamCleanup": "unsupported",
                "provider": "lk888"
            })))
            .mount(&server)
            .await;
        let provider = ControlledBackendProvider::for_test(&server.uri());
        let outcome = std::thread::spawn(move || {
            <ControlledBackendProvider as PhotoAvatarProvider>::delete_session(
                &provider, "session1",
            )
        })
        .join()
        .unwrap()
        .unwrap();
        assert_eq!(outcome.backend_cleanup, CleanupState::Pending);
        assert!(outcome.has_retryable_cleanup());
    }

    #[test]
    fn path_segments_are_encoded_and_empty_ids_are_rejected() {
        assert_eq!(encode_path_segment("a/b?c").unwrap(), "a%2Fb%3Fc");
        assert!(encode_path_segment(" ").is_err());
    }

    #[test]
    fn declared_content_length_over_20_mib_is_rejected_before_reading() {
        assert!(validate_declared_artifact_size(Some((MAX_ARTIFACT_BYTES + 1) as u64)).is_err());
        assert!(validate_declared_artifact_size(Some(MAX_ARTIFACT_BYTES as u64)).is_ok());
    }

    #[test]
    fn chunked_body_is_rejected_when_accumulated_size_exceeds_20_mib() {
        let mut body = vec![0; MAX_ARTIFACT_BYTES];
        assert!(append_artifact_chunk(&mut body, &[1]).is_err());
        assert_eq!(body.len(), MAX_ARTIFACT_BYTES);
    }

    #[test]
    fn fixed_fake_fixtures_cover_balanced_and_rounded_modules() {
        let balanced = FakePhotoAvatarProvider::for_body_module("body-balanced-v1").unwrap();
        let rounded = FakePhotoAvatarProvider::for_body_module("body-rounded-v1").unwrap();
        for (fake, expected) in [(balanced, "body-balanced-v1"), (rounded, "body-rounded-v1")] {
            let identity_job = fake.submit_step(request()).unwrap();
            let identity = fake.poll_job(&identity_job.provider_job_id).unwrap();
            let Some(ProviderStepResult::Identity { partial_profile }) = identity.result else {
                panic!("expected identity fixture")
            };
            assert_eq!(partial_profile.body_module_id, expected);

            let mut appearance_request = request();
            appearance_request.step = RemoteStep::CompleteAppearance;
            appearance_request.provider_session_id = Some("fake-session-1".into());
            appearance_request.source_images.clear();
            let appearance_job = fake.submit_step(appearance_request).unwrap();
            let appearance = fake.poll_job(&appearance_job.provider_job_id).unwrap();
            assert!(matches!(
                appearance.result,
                Some(ProviderStepResult::Appearance { .. })
            ));

            let mut texture_request = request();
            texture_request.step = RemoteStep::RenderTextureAtlas;
            texture_request.provider_session_id = Some("fake-session-1".into());
            texture_request.profile = Some(partial_profile);
            texture_request.body_module_contract_sha256 = Some(format!(
                "{:x}",
                Sha256::digest(
                    std::fs::read(
                        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                            .join("../public/cat-character-modules/cat-a-live2d-v1")
                            .join(expected)
                            .join("模块.json"),
                    )
                    .unwrap()
                )
            ));
            let texture_job = fake.submit_step(texture_request).unwrap();
            let texture = fake.poll_job(&texture_job.provider_job_id).unwrap();
            let Some(ProviderStepResult::TextureAtlas {
                artifact_url,
                sha256,
                audit,
                ..
            }) = texture.result
            else {
                panic!("expected texture fixture")
            };
            assert_eq!(sha256, audit.canonical_sha256);
            assert_ne!(audit.provider_raw_sha256, audit.canonical_sha256);
            assert_eq!(audit.composer_version, "deterministic-alpha-v1");
            let bytes = fake.download_artifact(&artifact_url, &sha256).unwrap();
            let decoded = image::load_from_memory(&bytes).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (2048, 2048));
        }
    }

    #[test]
    fn fake_atlas_fixture_is_non_identity_bearing_and_motion_visible() {
        let bytes = build_fake_atlas("body-balanced-v1").unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
        let neutral = image::open(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../public/cat-character-modules/cat-a-live2d-v1")
                .join("body-balanced-v1/body-balanced-v1.2048/texture_00.png"),
        )
        .unwrap()
        .to_rgba8();
        assert_eq!((image.width(), image.height()), (2048, 2048));
        assert!(image.pixels().any(|pixel| pixel.0[0] != pixel.0[1]));
        assert!(image.pixels().any(|pixel| pixel.0[3] > 0));
        assert!(image
            .pixels()
            .zip(neutral.pixels())
            .all(|(atlas, guide)| atlas.0[3] == guide.0[3]));
        assert!(image
            .pixels()
            .filter(|pixel| pixel.0[3] == 0)
            .all(|pixel| pixel.0[..3] == [0, 0, 0]));
    }
}
