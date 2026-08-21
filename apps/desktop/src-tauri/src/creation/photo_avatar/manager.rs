use super::domain::{
    parse_appearance_profile_v1, parse_photo_avatar_error_code, validate_revision_lock,
    AppearanceProfileV1, CanonicalTextureAuditV1, PhotoAvatarErrorCode, PhotoAvatarSnapshot,
    PhotoAvatarStep, PHOTO_AVATAR_CONSENT_VERSION,
};
use super::profile::{
    finalize_appearance_profile, revision_lock, validate_requested_traits, AppearanceCompletionV1,
};
use super::provider::{
    CleanupState, PhotoAvatarError, PhotoAvatarProvider, ProviderCleanupOutcome,
    ProviderSourceImage, ProviderStepRequest, ProviderStepResult, RemoteJobState,
    UpstreamCleanupState,
};
use super::source::{normalize_photo_sources, RawPhotoSource};
use super::store::{
    PhotoAvatarRunState, PhotoAvatarStore, ProviderArtifact, ProviderArtifactKind, RemoteJob,
    RemoteStep, ACTIVE_ATTEMPT_ERROR,
};
use crate::creation::finalization::{PhotoAvatarCleanupDisposition, PhotoAvatarFinalizationPort};
use crate::creation::service::PhotoAvatarAbandonPort;
use crate::runtime_assets::photo_avatar_builder::{BuildPhotoAvatarRequest, PhotoAvatarBuilder};
use base64::Engine as _;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BACKGROUND_INTERVAL: Duration = Duration::from_secs(2);

pub type SharedPhotoAvatarManager = Arc<PhotoAvatarManager>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoAvatarResumeFailure {
    pub session_id: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoAvatarResumeReport {
    pub resumed_session_ids: Vec<String>,
    pub failures: Vec<PhotoAvatarResumeFailure>,
}

pub struct UnconfiguredPhotoAvatarProvider;

impl PhotoAvatarProvider for UnconfiguredPhotoAvatarProvider {
    fn submit_step(&self, _request: ProviderStepRequest) -> Result<RemoteJob, PhotoAvatarError> {
        Err(unconfigured_provider_error())
    }

    fn poll_job(&self, _job_id: &str) -> Result<RemoteJobState, PhotoAvatarError> {
        Err(unconfigured_provider_error())
    }

    fn cancel_job(&self, _job_id: &str) -> Result<(), PhotoAvatarError> {
        Err(unconfigured_provider_error())
    }

    fn delete_session(
        &self,
        _provider_session_id: &str,
    ) -> Result<ProviderCleanupOutcome, PhotoAvatarError> {
        Err(unconfigured_provider_error())
    }

    fn download_artifact(
        &self,
        _url: &str,
        _expected_sha256: &str,
    ) -> Result<Vec<u8>, PhotoAvatarError> {
        Err(unconfigured_provider_error())
    }
}

fn unconfigured_provider_error() -> PhotoAvatarError {
    PhotoAvatarError {
        code: PhotoAvatarErrorCode::Unsupported,
        retryable: false,
        message: "photo avatar provider is not configured".into(),
    }
}

#[derive(Clone)]
pub struct PhotoAvatarManager {
    store: PhotoAvatarStore,
    provider: Arc<dyn PhotoAvatarProvider>,
    builder: Option<PhotoAvatarBuilder>,
    active_tokens: Arc<Mutex<HashMap<String, String>>>,
}

impl PhotoAvatarManager {
    pub fn new<P>(store: PhotoAvatarStore, provider: Arc<P>) -> Self
    where
        P: PhotoAvatarProvider + 'static,
    {
        Self::with_provider(store, provider)
    }

    pub fn with_provider(store: PhotoAvatarStore, provider: Arc<dyn PhotoAvatarProvider>) -> Self {
        Self {
            store,
            provider,
            builder: None,
            active_tokens: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_builder(mut self, builder: PhotoAvatarBuilder) -> Self {
        self.builder = Some(builder);
        self
    }

    pub fn begin(
        &self,
        session_id: &str,
        raw: Vec<RawPhotoSource>,
    ) -> Result<PhotoAvatarSnapshot, String> {
        self.begin_with_consent(session_id, PHOTO_AVATAR_CONSENT_VERSION, raw)
    }

    pub fn begin_with_consent(
        &self,
        session_id: &str,
        consent_version: &str,
        raw: Vec<RawPhotoSource>,
    ) -> Result<PhotoAvatarSnapshot, String> {
        if consent_version != PHOTO_AVATAR_CONSENT_VERSION {
            return Err("unsupported photo avatar consent version".into());
        }
        if !self.store.consent_accepted(consent_version)? {
            return Err("photo avatar consent is required before beginning".into());
        }
        let sources = normalize_photo_sources(raw).map_err(|error| error.to_string())?;
        self.store.replace_sources(session_id, &sources)?;
        self.store.begin_revision(session_id, None, &[])?;
        self.store.snapshot(session_id)
    }

    pub fn consent(&self, accept: bool) -> Result<bool, String> {
        if accept {
            self.store.save_consent(PHOTO_AVATAR_CONSENT_VERSION)?;
        }
        self.store.consent_accepted(PHOTO_AVATAR_CONSENT_VERSION)
    }

    pub fn status(&self, session_id: &str) -> Result<PhotoAvatarSnapshot, String> {
        self.store.snapshot(session_id)
    }

    pub fn status_if_exists(
        &self,
        session_id: &str,
    ) -> Result<Option<PhotoAvatarSnapshot>, String> {
        self.store.snapshot_if_exists(session_id)
    }

    pub fn tick(&self, session_id: &str) -> Result<PhotoAvatarSnapshot, String> {
        let snapshot = self.store.snapshot(session_id)?;
        if snapshot.step == PhotoAvatarStep::CleanupPending {
            self.retry_remote_cleanup(session_id)?;
            return self.store.snapshot(session_id);
        }
        if snapshot.step == PhotoAvatarStep::BuildV5 {
            self.build_preview(session_id)?;
            return self.store.snapshot(session_id);
        }
        let step = match snapshot.step {
            PhotoAvatarStep::AnalyzeIdentity => RemoteStep::AnalyzeIdentity,
            PhotoAvatarStep::CompleteAppearance => RemoteStep::CompleteAppearance,
            PhotoAvatarStep::RenderTextureAtlas => RemoteStep::RenderTextureAtlas,
            _ => return Ok(snapshot),
        };
        let run = self.store.current_run(session_id)?;
        if let Some(job_id) = run.provider_job_id.clone() {
            match self.provider.poll_job(&job_id) {
                Ok(state) => self.apply_polled_result(&run, &job_id, state)?,
                Err(error) if is_retryable(error.code) => {
                    self.store
                        .record_poll_error(&run, error.code, &error.message)?;
                }
                Err(error) => {
                    self.store
                        .fail_attempt(&run, error.code, &error.message, false)?;
                }
            }
            return self.store.snapshot(session_id);
        }

        let attempt = match self.store.reserve_attempt(session_id, run.revision, step) {
            Ok(attempt) => attempt,
            Err(error) if error == ACTIVE_ATTEMPT_ERROR => return Ok(snapshot),
            Err(error) => return Err(error),
        };
        let request = self.request_for(&run, step, attempt)?;
        match self.provider.submit_step(request) {
            Ok(job) => self
                .store
                .attach_job(&run.generation_token, step, attempt, &job)?,
            Err(error) => {
                let reserved = self.store.current_run(session_id)?;
                self.store.fail_attempt(
                    &reserved,
                    error.code,
                    &error.message,
                    is_retryable(error.code),
                )?;
            }
        }
        self.store.snapshot(session_id)
    }

    fn build_preview(&self, session_id: &str) -> Result<(), String> {
        let builder = self
            .builder
            .as_ref()
            .ok_or("photo avatar preview builder is not configured")?;
        let run = self.store.current_run(session_id)?;
        let profile = self
            .store
            .snapshot(session_id)?
            .profile
            .ok_or("photo avatar build requires a completed appearance profile")?;
        let artifact = self
            .store
            .texture_artifact(session_id, run.revision)?
            .ok_or("photo avatar texture artifact is missing")?;
        let texture_png = self
            .provider
            .download_artifact(&artifact.relative_path, &artifact.sha256)
            .map_err(format_provider_error)?;
        let texture_audit: CanonicalTextureAuditV1 = serde_json::from_str(
            artifact
                .audit_json
                .as_deref()
                .ok_or("photo avatar texture artifact audit is missing")?,
        )
        .map_err(|error| format!("invalid canonical texture audit: {error}"))?;
        texture_audit.validate_success()?;
        let pet_id = self.store.pet_id(session_id)?;
        let package = builder.build_preview(BuildPhotoAvatarRequest {
            session_id: session_id.into(),
            revision: run.revision,
            pet_id,
            variant_id: format!("photo-avatar-{session_id}-{}", run.revision),
            profile,
            texture_sha256: artifact.sha256,
            texture_png,
            texture_audit,
        })?;
        self.store.commit_preview_package(
            session_id,
            run.revision,
            &run.generation_token,
            &package.preview_dir,
            &package.manifest_sha256,
        )
    }

    pub fn runtime_check_passed(
        &self,
        session_id: &str,
        revision: u32,
        manifest_sha256: &str,
    ) -> Result<PhotoAvatarSnapshot, String> {
        self.store
            .runtime_check_passed(session_id, revision, manifest_sha256)?;
        self.store.snapshot(session_id)
    }

    pub fn preview_manifest(
        &self,
        session_id: &str,
        revision: u32,
    ) -> Result<serde_json::Value, String> {
        self.builder
            .as_ref()
            .ok_or("photo avatar preview builder is not configured")?
            .validate_preview(session_id, revision)?;
        self.store.preview_manifest(session_id, revision)
    }

    pub fn preview_file_b64(
        &self,
        session_id: &str,
        revision: u32,
        relative_path: &str,
    ) -> Result<String, String> {
        use base64::Engine as _;
        self.builder
            .as_ref()
            .ok_or("photo avatar preview builder is not configured")?
            .validate_preview(session_id, revision)?;
        let bytes = self
            .store
            .preview_file_bytes(session_id, revision, relative_path)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn cancel(&self, session_id: &str) -> Result<PhotoAvatarSnapshot, String> {
        let snapshot = self.store.snapshot(session_id)?;
        if snapshot.step == PhotoAvatarStep::Cancelled {
            return Ok(snapshot);
        }
        let run = self.store.current_run(session_id)?;
        if matches!(
            run.step,
            PhotoAvatarStep::Completed | PhotoAvatarStep::Failed
        ) {
            self.store.delete_sources_for_terminal_run(&run)?;
            return self.store.snapshot(session_id);
        }
        self.store
            .cancel(session_id, run.revision, &run.generation_token)?;
        self.remove_active_token(session_id, &run.generation_token);
        if let Some(job_id) = run.provider_job_id.as_deref() {
            let _ = self.provider.cancel_job(job_id);
        }
        if self.cleanup_remote(&run)?.is_some() {
            self.store
                .mark_cleanup_pending_for_run_and_delete_local_data(
                    session_id,
                    run.revision,
                    &run.generation_token,
                )?;
            return self.store.snapshot(session_id);
        }
        self.store
            .complete_remote_cleanup(session_id, run.revision, &run.generation_token)?;
        self.store.snapshot(session_id)
    }

    pub fn regenerate(&self, session_id: &str) -> Result<PhotoAvatarSnapshot, String> {
        let run = self.store.current_run(session_id)?;
        self.remove_active_token(session_id, &run.generation_token);
        self.store.begin_revision(session_id, None, &[])?;
        self.store.snapshot(session_id)
    }

    pub fn regenerate_and_start_background(
        &self,
        session_id: &str,
    ) -> Result<PhotoAvatarSnapshot, String> {
        let snapshot = self.regenerate(session_id)?;
        self.start_background(session_id)?;
        Ok(snapshot)
    }

    pub fn revise(
        &self,
        session_id: &str,
        instruction: &str,
    ) -> Result<PhotoAvatarSnapshot, String> {
        let instruction = instruction.trim();
        if instruction.is_empty() {
            return Err("instruction must be a non-empty string".into());
        }
        let run = self.store.current_run(session_id)?;
        self.remove_active_token(session_id, &run.generation_token);
        self.store
            .begin_revision(session_id, Some(instruction), &[])?;
        self.store.snapshot(session_id)
    }

    pub fn revise_and_start_background(
        &self,
        session_id: &str,
        instruction: &str,
    ) -> Result<PhotoAvatarSnapshot, String> {
        let snapshot = self.revise(session_id, instruction)?;
        self.start_background(session_id)?;
        Ok(snapshot)
    }

    pub fn resume_all(&self) -> Result<PhotoAvatarResumeReport, String> {
        let sessions = self.store.resumable_session_ids()?;
        let mut report = PhotoAvatarResumeReport {
            resumed_session_ids: Vec::new(),
            failures: Vec::new(),
        };
        for session_id in sessions {
            let resumed = (|| {
                let step = self.store.snapshot(&session_id)?.step;
                if matches!(
                    step,
                    PhotoAvatarStep::CleanupPending | PhotoAvatarStep::BuildV5
                ) {
                    self.tick(&session_id)?;
                } else {
                    #[cfg(not(test))]
                    self.start_background(&session_id)?;
                }
                Ok::<(), String>(())
            })();
            match resumed {
                Ok(()) => report.resumed_session_ids.push(session_id),
                Err(error) => report
                    .failures
                    .push(PhotoAvatarResumeFailure { session_id, error }),
            }
        }
        Ok(report)
    }

    pub fn cleanup_after_accept(
        &self,
        session_id: &str,
    ) -> Result<PhotoAvatarCleanupDisposition, String> {
        let run = self.store.current_run(session_id)?;
        if run.step == PhotoAvatarStep::Completed {
            return Ok(PhotoAvatarCleanupDisposition::Complete);
        }
        if run.step == PhotoAvatarStep::PreviewReady {
            self.store
                .mark_accept_cleanup_pending_and_delete_sources(session_id)?;
        } else if run.step != PhotoAvatarStep::CleanupPending {
            return Err("photo avatar preview is not ready for acceptance cleanup".into());
        }
        let pending = self.store.current_run(session_id)?;
        if let Some(error) = self.cleanup_remote(&pending)? {
            return Ok(PhotoAvatarCleanupDisposition::Pending(error));
        }
        self.store.complete_remote_cleanup(
            session_id,
            pending.revision,
            &pending.generation_token,
        )?;
        Ok(PhotoAvatarCleanupDisposition::Complete)
    }

    pub fn prepare_for_full_exit(&self) -> Result<Vec<String>, String> {
        let cancelled = self.store.prepare_for_full_exit()?;
        if let Ok(mut active) = self.active_tokens.lock() {
            for session_id in &cancelled {
                active.remove(session_id);
            }
        }
        Ok(cancelled)
    }

    pub fn start_background(&self, session_id: &str) -> Result<(), String> {
        let run = self.store.current_run(session_id)?;
        if !self.claim_active_token(session_id, &run.generation_token)? {
            return Ok(());
        }
        let manager = self.clone();
        let session_id = session_id.to_string();
        let token = run.generation_token;
        tauri::async_runtime::spawn_blocking(move || {
            loop {
                let still_active = manager
                    .active_tokens
                    .lock()
                    .map(|active| active.get(&session_id) == Some(&token))
                    .unwrap_or(false);
                if !still_active {
                    break;
                }
                match manager.tick(&session_id) {
                    Ok(snapshot) if is_terminal_or_local(snapshot.step) => break,
                    Err(error) => {
                        eprintln!("[desktop-pet] photo avatar background tick failed: {error}");
                        break;
                    }
                    _ => std::thread::sleep(BACKGROUND_INTERVAL),
                }
            }
            manager.remove_active_token(&session_id, &token);
        });
        Ok(())
    }

    fn claim_active_token(&self, session_id: &str, token: &str) -> Result<bool, String> {
        let mut active = self
            .active_tokens
            .lock()
            .map_err(|_| "photo avatar active token lock poisoned")?;
        if active
            .get(session_id)
            .is_some_and(|current| current == token)
        {
            return Ok(false);
        }
        active.insert(session_id.into(), token.into());
        Ok(true)
    }

    fn request_for(
        &self,
        run: &PhotoAvatarRunState,
        step: RemoteStep,
        attempt: u8,
    ) -> Result<ProviderStepRequest, String> {
        let source_images = if matches!(
            step,
            RemoteStep::AnalyzeIdentity | RemoteStep::RenderTextureAtlas
        ) {
            self.store
                .sources(&run.session_id)?
                .into_iter()
                .map(|source| ProviderSourceImage {
                    source_id: source.source_id,
                    png_base64: base64::engine::general_purpose::STANDARD
                        .encode(source.normalized_png),
                    sha256: source.sha256,
                    width: source.width,
                    height: source.height,
                })
                .collect()
        } else {
            Vec::new()
        };
        let profile = match step {
            RemoteStep::AnalyzeIdentity => None,
            RemoteStep::CompleteAppearance => {
                self.store.partial_profile(&run.session_id, run.revision)?
            }
            RemoteStep::RenderTextureAtlas => self.store.snapshot(&run.session_id)?.profile,
        };
        let body_module_contract_sha256 = if step == RemoteStep::RenderTextureAtlas {
            let profile = profile
                .as_ref()
                .ok_or("photo avatar render requires a completed profile")?;
            let builder = self
                .builder
                .as_ref()
                .ok_or("photo avatar preview builder is not configured")?;
            Some(builder.body_module_contract_sha256(&profile.body_module_id)?)
        } else {
            None
        };
        Ok(ProviderStepRequest {
            session_id: run.session_id.clone(),
            revision: run.revision,
            provider_session_id: run.provider_session_id.clone(),
            step,
            attempt,
            consent_version: PHOTO_AVATAR_CONSENT_VERSION.into(),
            source_images,
            profile,
            body_module_contract_sha256,
            modification: run.modification.clone(),
            locked_traits: Vec::new(),
        })
    }

    fn apply_polled_result(
        &self,
        run: &PhotoAvatarRunState,
        job_id: &str,
        state: RemoteJobState,
    ) -> Result<(), String> {
        if run.provider_job_id.as_deref() != Some(job_id) {
            return Err("superseded response".into());
        }
        let attempt = run.current_attempt.ok_or("attempt identity required")?;
        match state.state.as_str() {
            "queued" | "running" => Ok(()),
            "failed" => {
                let error = state.error.ok_or("failed provider job requires error")?;
                let code = parse_remote_error_code(&error.code)?;
                self.store
                    .fail_attempt(run, code, &error.message, is_retryable(code))
            }
            "succeeded" => match state
                .result
                .ok_or("successful provider job requires result")?
            {
                ProviderStepResult::Identity { partial_profile } => {
                    if run.step != PhotoAvatarStep::AnalyzeIdentity {
                        return Err("provider result does not match remote step".into());
                    }
                    self.commit_profile_result(run, job_id, attempt, partial_profile)
                }
                ProviderStepResult::Appearance { completion } => {
                    if run.step != PhotoAvatarStep::CompleteAppearance {
                        return Err("provider result does not match remote step".into());
                    }
                    self.commit_appearance_result(run, job_id, attempt, completion)
                }
                ProviderStepResult::LegacyAppearance { profile } => {
                    if run.step != PhotoAvatarStep::CompleteAppearance {
                        return Err("provider result does not match remote step".into());
                    }
                    self.commit_legacy_appearance_result(run, job_id, attempt, profile)
                }
                ProviderStepResult::TextureAtlas {
                    artifact_url,
                    sha256,
                    audit,
                    ..
                } => {
                    if run.step != PhotoAvatarStep::RenderTextureAtlas {
                        return Err("provider result does not match remote step".into());
                    }
                    let profile = self
                        .store
                        .snapshot(&run.session_id)?
                        .profile
                        .ok_or("photo avatar render requires a completed profile")?;
                    let builder = self
                        .builder
                        .as_ref()
                        .ok_or("photo avatar preview builder is not configured")?;
                    let module_contract_sha256 =
                        builder.body_module_contract_sha256(&profile.body_module_id)?;
                    if let Err(error) = validate_texture_audit_identity(
                        run,
                        attempt,
                        &profile,
                        &module_contract_sha256,
                        &audit,
                    ) {
                        self.store.fail_attempt(
                            run,
                            PhotoAvatarErrorCode::InvalidInput,
                            &error,
                            false,
                        )?;
                        return Ok(());
                    }
                    let audit_json = serde_json::to_string(&audit)
                        .map_err(|error| format!("serialize canonical texture audit: {error}"))?;
                    self.store.commit_artifact_for_attempt(
                        &run.generation_token,
                        RemoteStep::RenderTextureAtlas,
                        attempt,
                        job_id,
                        &ProviderArtifact {
                            kind: ProviderArtifactKind::TextureAtlas,
                            relative_path: artifact_url,
                            sha256,
                            local_path: None,
                            audit_json: Some(audit_json),
                        },
                    )
                }
                ProviderStepResult::PixelIdentity { .. }
                | ProviderStepResult::PixelAvatar { .. } => {
                    Err("pixel provider result reached legacy photo avatar manager".into())
                }
            },
            _ => Err("invalid provider job state".into()),
        }
    }

    fn commit_profile_result(
        &self,
        run: &PhotoAvatarRunState,
        job_id: &str,
        attempt: u8,
        profile: super::domain::AppearanceProfileV1,
    ) -> Result<(), String> {
        let profile_json = serde_json::to_string(&profile)
            .map_err(|error| format!("serialize provider appearance profile: {error}"))?;
        let profile = match parse_appearance_profile_v1(&profile_json) {
            Ok(profile) => profile,
            Err(error) => {
                self.store
                    .fail_attempt(run, PhotoAvatarErrorCode::InvalidInput, &error, false)?;
                return Ok(());
            }
        };
        let step = match run.step {
            PhotoAvatarStep::AnalyzeIdentity => RemoteStep::AnalyzeIdentity,
            PhotoAvatarStep::CompleteAppearance => RemoteStep::CompleteAppearance,
            _ => return Err("profile result does not match remote step".into()),
        };
        self.store.commit_profile_for_attempt(
            &run.generation_token,
            step,
            attempt,
            job_id,
            &profile,
        )
    }

    fn commit_appearance_result(
        &self,
        run: &PhotoAvatarRunState,
        job_id: &str,
        attempt: u8,
        completion: AppearanceCompletionV1,
    ) -> Result<(), String> {
        let requested = completion.requested_trait_keys.clone();
        let partial = self
            .store
            .partial_profile(&run.session_id, run.revision)?
            .ok_or("appearance completion requires a partial profile")?;
        let profile = match finalize_appearance_profile(&partial, completion) {
            Ok(profile) => profile,
            Err(error) => {
                self.store
                    .fail_attempt(run, PhotoAvatarErrorCode::InvalidInput, &error, false)?;
                return Ok(());
            }
        };
        if run.modification.is_some() {
            if let Err(error) = validate_requested_traits(&requested) {
                self.store
                    .fail_attempt(run, PhotoAvatarErrorCode::InvalidInput, &error, false)?;
                return Ok(());
            }
            let before = self
                .store
                .previous_profile(&run.session_id, run.revision)?
                .ok_or("revision requires a previous appearance profile")?;
            let locked = revision_lock(&before, &requested);
            if let Err(error) = validate_revision_lock(&before, &profile, &locked) {
                self.store
                    .fail_attempt(run, PhotoAvatarErrorCode::InvalidInput, &error, false)?;
                return Ok(());
            }
        }
        self.commit_profile_result(run, job_id, attempt, profile)
    }

    fn commit_legacy_appearance_result(
        &self,
        run: &PhotoAvatarRunState,
        job_id: &str,
        attempt: u8,
        profile: super::domain::AppearanceProfileV1,
    ) -> Result<(), String> {
        if run.modification.is_some() {
            self.store.fail_attempt(
                run,
                PhotoAvatarErrorCode::InvalidInput,
                "legacy appearance result cannot prove requestedTraitKeys for a modification revision",
                false,
            )?;
            return Ok(());
        }
        if self
            .store
            .legacy_partial_profile(&run.session_id, run.revision)?
            .is_none()
        {
            self.store.fail_attempt(
                run,
                PhotoAvatarErrorCode::InvalidInput,
                "legacy appearance result requires a migrated legacy partial profile",
                false,
            )?;
            return Ok(());
        }
        self.commit_profile_result(run, job_id, attempt, profile)
    }

    fn remove_active_token(&self, session_id: &str, token: &str) {
        if let Ok(mut active) = self.active_tokens.lock() {
            if active
                .get(session_id)
                .is_some_and(|current| current == token)
            {
                active.remove(session_id);
            }
        }
    }

    fn retry_remote_cleanup(&self, session_id: &str) -> Result<(), String> {
        let run = self.store.current_run(session_id)?;
        if run.step != PhotoAvatarStep::CleanupPending {
            return Ok(());
        }
        if self.cleanup_remote(&run)?.is_some() {
            return Ok(());
        }
        self.store
            .complete_remote_cleanup(session_id, run.revision, &run.generation_token)
    }

    fn cleanup_remote(&self, run: &PhotoAvatarRunState) -> Result<Option<String>, String> {
        let result = match run.provider_session_id.as_deref() {
            Some(provider_session_id) => self.provider.delete_session(provider_session_id),
            None => Ok(ProviderCleanupOutcome {
                backend_cleanup: CleanupState::Deleted,
                upstream_cleanup: UpstreamCleanupState::Unsupported,
                provider: "lk888".into(),
            }),
        };
        match result {
            Ok(outcome) => {
                self.store.record_cleanup_audit(
                    &run.session_id,
                    run.revision,
                    CleanupState::Deleted,
                    outcome.backend_cleanup.clone(),
                    outcome.upstream_cleanup.clone(),
                    &outcome.provider,
                )?;
                Ok(outcome
                    .has_retryable_cleanup()
                    .then(|| "photo avatar backend cleanup is pending".into()))
            }
            Err(error) => {
                self.store.record_cleanup_audit(
                    &run.session_id,
                    run.revision,
                    CleanupState::Deleted,
                    CleanupState::Pending,
                    UpstreamCleanupState::Unsupported,
                    "lk888",
                )?;
                Ok(Some(format_provider_error(error)))
            }
        }
    }
}

fn validate_texture_audit_identity(
    run: &PhotoAvatarRunState,
    attempt: u8,
    profile: &AppearanceProfileV1,
    module_contract_sha256: &str,
    audit: &CanonicalTextureAuditV1,
) -> Result<(), String> {
    audit.validate_success()?;
    if audit.session_id != run.session_id
        || audit.revision != run.revision
        || audit.attempt != attempt
        || audit.body_module_id != profile.body_module_id
        || audit.module_contract_sha256 != module_contract_sha256
    {
        return Err("canonical texture audit does not match current render attempt".into());
    }
    Ok(())
}

impl PhotoAvatarAbandonPort for PhotoAvatarManager {
    fn cancel_provider_job(&self, _session_id: &str, provider_job_id: &str) -> Result<(), String> {
        self.provider
            .cancel_job(provider_job_id)
            .map_err(format_provider_error)
    }

    fn delete_provider_session(
        &self,
        session_id: &str,
        provider_session_id: &str,
    ) -> Result<(), String> {
        let run = self.store.current_run(session_id)?;
        if run.provider_session_id.as_deref() != Some(provider_session_id) {
            return Err("photo avatar provider session does not match current run".into());
        }
        self.store
            .mark_cleanup_pending_and_delete_local_data(session_id)?;
        let pending = self.store.current_run(session_id)?;
        match self.cleanup_remote(&pending)? {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl PhotoAvatarFinalizationPort for PhotoAvatarManager {
    fn preview_ready(&self, session_id: &str) -> Result<bool, String> {
        Ok(matches!(
            self.store.snapshot(session_id)?.step,
            PhotoAvatarStep::PreviewReady
                | PhotoAvatarStep::CleanupPending
                | PhotoAvatarStep::Completed
        ))
    }

    fn install_preview(
        &self,
        session_id: &str,
        pet_id: &str,
        variant_id: &str,
        destination: &std::path::Path,
    ) -> Result<(), String> {
        let run = self.store.current_run(session_id)?;
        if run.step != PhotoAvatarStep::PreviewReady {
            return Err("photo avatar preview is not ready for installation".into());
        }
        let manifest = self.store.preview_manifest(session_id, run.revision)?;
        if manifest
            .get("schemaVersion")
            .and_then(serde_json::Value::as_u64)
            != Some(5)
            || manifest.get("petId").and_then(serde_json::Value::as_str) != Some(pet_id)
            || manifest
                .get("variantId")
                .and_then(serde_json::Value::as_str)
                != Some(variant_id)
        {
            return Err("photo avatar preview identity does not match finalization".into());
        }
        self.builder
            .as_ref()
            .ok_or("photo avatar preview builder is not configured")?
            .install_preview(session_id, run.revision, destination)
    }

    fn cleanup_after_accept(
        &self,
        session_id: &str,
    ) -> Result<PhotoAvatarCleanupDisposition, String> {
        PhotoAvatarManager::cleanup_after_accept(self, session_id)
    }

    fn restore_preview_after_abort(&self, session_id: &str) -> Result<(), String> {
        self.store
            .restore_preview_after_finalization_abort(session_id)
    }
}

fn parse_remote_error_code(value: &str) -> Result<PhotoAvatarErrorCode, String> {
    parse_photo_avatar_error_code(&serde_json::to_string(value).map_err(|error| error.to_string())?)
}

fn is_retryable(code: PhotoAvatarErrorCode) -> bool {
    matches!(
        code,
        PhotoAvatarErrorCode::Network
            | PhotoAvatarErrorCode::Timeout
            | PhotoAvatarErrorCode::Provider5xx
            | PhotoAvatarErrorCode::TemporaryUnavailable
    )
}

fn format_provider_error(error: PhotoAvatarError) -> String {
    format!("photo avatar provider {:?}: {}", error.code, error.message)
}

fn is_terminal_or_local(step: PhotoAvatarStep) -> bool {
    !matches!(
        step,
        PhotoAvatarStep::AnalyzeIdentity
            | PhotoAvatarStep::CompleteAppearance
            | PhotoAvatarStep::RenderTextureAtlas
            | PhotoAvatarStep::BuildV5
    )
}

#[cfg(test)]
mod tests {
    use super::{is_terminal_or_local, PhotoAvatarManager};
    use crate::creation::domain::new_entity_id;
    use crate::creation::photo_avatar::domain::{
        parse_appearance_profile_v1, CanonicalTextureAuditV1, IdentityTraitKey, IdentityTraitV1,
        PhotoAvatarAttemptStep, PhotoAvatarErrorCode, PhotoAvatarStep, TraitSource,
        PHOTO_AVATAR_CONSENT_VERSION,
    };
    use crate::creation::photo_avatar::profile::{AppearanceCompletionV1, ALL_IDENTITY_TRAIT_KEYS};
    use crate::creation::photo_avatar::provider::{
        CleanupState, FakeOutcome, FakePhotoAvatarProvider, PhotoAvatarError, PhotoAvatarProvider,
        ProviderCleanupOutcome, ProviderStepRequest, RemoteJobState, UpstreamCleanupState,
    };
    use crate::creation::photo_avatar::source::RawPhotoSource;
    use crate::creation::photo_avatar::store::{
        PhotoAvatarRunState, PhotoAvatarStore, RemoteJob, RemoteStep,
    };
    use crate::creation::service::PhotoAvatarAbandonPort;
    use crate::runtime_assets::photo_avatar_builder::{
        BuildPhotoAvatarRequest, PhotoAvatarBuilder,
    };
    use crate::storage::Storage;
    use image::{DynamicImage, ImageFormat, RgbaImage};
    use sha2::{Digest, Sha256};
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};

    struct SubmitErrorProvider {
        fake: FakePhotoAvatarProvider,
        errors: Mutex<VecDeque<PhotoAvatarError>>,
        submit_count: AtomicUsize,
    }

    impl SubmitErrorProvider {
        fn new(errors: Vec<PhotoAvatarError>) -> Self {
            Self {
                fake: FakePhotoAvatarProvider::new(vec![FakeOutcome::Running]),
                errors: Mutex::new(errors.into()),
                submit_count: AtomicUsize::new(0),
            }
        }
    }

    impl PhotoAvatarProvider for SubmitErrorProvider {
        fn submit_step(&self, request: ProviderStepRequest) -> Result<RemoteJob, PhotoAvatarError> {
            self.submit_count.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.errors.lock().unwrap().pop_front() {
                Err(error)
            } else {
                self.fake.submit_step(request)
            }
        }

        fn poll_job(&self, job_id: &str) -> Result<RemoteJobState, PhotoAvatarError> {
            self.fake.poll_job(job_id)
        }

        fn cancel_job(&self, job_id: &str) -> Result<(), PhotoAvatarError> {
            self.fake.cancel_job(job_id)
        }

        fn delete_session(
            &self,
            provider_session_id: &str,
        ) -> Result<ProviderCleanupOutcome, PhotoAvatarError> {
            self.fake.delete_session(provider_session_id)
        }

        fn download_artifact(
            &self,
            url: &str,
            expected_sha256: &str,
        ) -> Result<Vec<u8>, PhotoAvatarError> {
            self.fake.download_artifact(url, expected_sha256)
        }
    }

    struct DeleteSequenceProvider {
        fake: FakePhotoAvatarProvider,
        results: Mutex<VecDeque<Result<CleanupState, PhotoAvatarError>>>,
        deleted_sessions: Mutex<Vec<String>>,
    }

    impl DeleteSequenceProvider {
        fn new(results: Vec<Result<CleanupState, PhotoAvatarError>>) -> Self {
            Self {
                fake: FakePhotoAvatarProvider::new(vec![FakeOutcome::Running]),
                results: Mutex::new(results.into()),
                deleted_sessions: Mutex::new(Vec::new()),
            }
        }

        fn deleted_sessions(&self) -> Vec<String> {
            self.deleted_sessions.lock().unwrap().clone()
        }
    }

    impl PhotoAvatarProvider for DeleteSequenceProvider {
        fn submit_step(&self, request: ProviderStepRequest) -> Result<RemoteJob, PhotoAvatarError> {
            self.fake.submit_step(request)
        }

        fn poll_job(&self, job_id: &str) -> Result<RemoteJobState, PhotoAvatarError> {
            self.fake.poll_job(job_id)
        }

        fn cancel_job(&self, _job_id: &str) -> Result<(), PhotoAvatarError> {
            Err(PhotoAvatarError {
                code: PhotoAvatarErrorCode::Network,
                retryable: true,
                message: "cancel failed".into(),
            })
        }

        fn delete_session(
            &self,
            provider_session_id: &str,
        ) -> Result<ProviderCleanupOutcome, PhotoAvatarError> {
            self.deleted_sessions
                .lock()
                .unwrap()
                .push(provider_session_id.into());
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(CleanupState::Deleted))
                .map(|backend_cleanup| ProviderCleanupOutcome {
                    backend_cleanup,
                    upstream_cleanup: UpstreamCleanupState::Unsupported,
                    provider: "lk888".into(),
                })
        }

        fn download_artifact(
            &self,
            url: &str,
            expected_sha256: &str,
        ) -> Result<Vec<u8>, PhotoAvatarError> {
            self.fake.download_artifact(url, expected_sha256)
        }
    }

    struct BlockingSubmitProvider {
        fake: FakePhotoAvatarProvider,
        submit_count: AtomicUsize,
        gate: (Mutex<(bool, bool)>, Condvar),
    }

    impl BlockingSubmitProvider {
        fn new() -> Self {
            Self {
                fake: FakePhotoAvatarProvider::new(vec![FakeOutcome::Running]),
                submit_count: AtomicUsize::new(0),
                gate: (Mutex::new((false, false)), Condvar::new()),
            }
        }

        fn wait_until_first_submit_enters(&self) {
            let (lock, entered) = &self.gate;
            let mut state = lock.lock().unwrap();
            while !state.0 {
                state = entered.wait(state).unwrap();
            }
        }

        fn release_first_submit(&self) {
            let (lock, release) = &self.gate;
            let mut state = lock.lock().unwrap();
            state.1 = true;
            release.notify_all();
        }
    }

    impl PhotoAvatarProvider for BlockingSubmitProvider {
        fn submit_step(&self, request: ProviderStepRequest) -> Result<RemoteJob, PhotoAvatarError> {
            if self.submit_count.fetch_add(1, Ordering::SeqCst) == 0 {
                let (lock, signal) = &self.gate;
                let mut state = lock.lock().unwrap();
                state.0 = true;
                signal.notify_all();
                while !state.1 {
                    state = signal.wait(state).unwrap();
                }
            }
            self.fake.submit_step(request)
        }

        fn poll_job(&self, job_id: &str) -> Result<RemoteJobState, PhotoAvatarError> {
            self.fake.poll_job(job_id)
        }

        fn cancel_job(&self, job_id: &str) -> Result<(), PhotoAvatarError> {
            self.fake.cancel_job(job_id)
        }

        fn delete_session(
            &self,
            provider_session_id: &str,
        ) -> Result<ProviderCleanupOutcome, PhotoAvatarError> {
            self.fake.delete_session(provider_session_id)
        }

        fn download_artifact(
            &self,
            url: &str,
            expected_sha256: &str,
        ) -> Result<Vec<u8>, PhotoAvatarError> {
            self.fake.download_artifact(url, expected_sha256)
        }
    }

    struct BlockingDeleteProvider {
        fake: FakePhotoAvatarProvider,
        delete_succeeds: bool,
        gate: (Mutex<(bool, bool)>, Condvar),
    }

    impl BlockingDeleteProvider {
        fn new(delete_succeeds: bool) -> Self {
            Self {
                fake: FakePhotoAvatarProvider::new(vec![
                    FakeOutcome::Running,
                    FakeOutcome::Running,
                ]),
                delete_succeeds,
                gate: (Mutex::new((false, false)), Condvar::new()),
            }
        }

        fn wait_until_delete_enters(&self) {
            let (lock, entered) = &self.gate;
            let mut state = lock.lock().unwrap();
            while !state.0 {
                state = entered.wait(state).unwrap();
            }
        }

        fn release_delete(&self) {
            let (lock, release) = &self.gate;
            let mut state = lock.lock().unwrap();
            state.1 = true;
            release.notify_all();
        }
    }

    impl PhotoAvatarProvider for BlockingDeleteProvider {
        fn submit_step(&self, request: ProviderStepRequest) -> Result<RemoteJob, PhotoAvatarError> {
            self.fake.submit_step(request)
        }

        fn poll_job(&self, job_id: &str) -> Result<RemoteJobState, PhotoAvatarError> {
            self.fake.poll_job(job_id)
        }

        fn cancel_job(&self, job_id: &str) -> Result<(), PhotoAvatarError> {
            self.fake.cancel_job(job_id)
        }

        fn delete_session(
            &self,
            _provider_session_id: &str,
        ) -> Result<ProviderCleanupOutcome, PhotoAvatarError> {
            let (lock, signal) = &self.gate;
            let mut state = lock.lock().unwrap();
            state.0 = true;
            signal.notify_all();
            while !state.1 {
                state = signal.wait(state).unwrap();
            }
            if self.delete_succeeds {
                Ok(ProviderCleanupOutcome {
                    backend_cleanup: CleanupState::Deleted,
                    upstream_cleanup: UpstreamCleanupState::Unsupported,
                    provider: "lk888".into(),
                })
            } else {
                Err(PhotoAvatarError {
                    code: PhotoAvatarErrorCode::Network,
                    retryable: true,
                    message: "delete failed".into(),
                })
            }
        }

        fn download_artifact(
            &self,
            url: &str,
            expected_sha256: &str,
        ) -> Result<Vec<u8>, PhotoAvatarError> {
            self.fake.download_artifact(url, expected_sha256)
        }
    }

    struct Harness {
        root: std::path::PathBuf,
        session: String,
        storage: Arc<Mutex<Storage>>,
        store: PhotoAvatarStore,
        provider: Arc<FakePhotoAvatarProvider>,
        manager: PhotoAvatarManager,
    }

    impl Harness {
        fn new(outcomes: Vec<FakeOutcome>) -> Self {
            let suffix = new_entity_id("manager");
            let root = std::env::temp_dir().join(format!("desktop-pet-{suffix}"));
            let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
            let session = format!("session-{suffix}");
            {
                let storage = storage.lock().unwrap();
                storage.db.execute(
                    "INSERT INTO pets (pet_id, schema_version, species, identity_mode, creation_method, lifecycle, created_at, updated_at) VALUES (?1, 1, 'cat', 'realpet', 'upload', 'draft', '10', '10')",
                    [format!("pet-{suffix}")],
                ).unwrap();
                storage.db.execute(
                    "INSERT INTO creation_sessions (session_id, pet_id, method, status, last_stable_status, current_step, schema_version, created_at, updated_at) VALUES (?1, ?2, 'upload', 'draft', 'draft', 'upload', 1, '10', '10')",
                    rusqlite::params![session, format!("pet-{suffix}")],
                ).unwrap();
            }
            let store = PhotoAvatarStore::new(storage.clone());
            let provider = Arc::new(FakePhotoAvatarProvider::new(outcomes));
            let manager = PhotoAvatarManager::new(store.clone(), provider.clone());
            Self {
                root,
                session,
                storage,
                store,
                provider,
                manager,
            }
        }

        fn source(&self) -> RawPhotoSource {
            let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
                256,
                256,
                image::Rgba([91, 52, 31, 255]),
            ));
            let mut bytes = Vec::new();
            image
                .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
                .unwrap();
            RawPhotoSource {
                claimed_sha256: format!("{:x}", Sha256::digest(&bytes)),
                bytes,
            }
        }

        fn tick_n(&self, count: usize) {
            for _ in 0..count {
                self.manager.tick(&self.session).unwrap();
            }
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn profile(face: &str) -> crate::creation::photo_avatar::domain::AppearanceProfileV1 {
        parse_appearance_profile_v1(&format!(
            r#"{{"schemaVersion":1,"species":"cat","style":"animated-film-soft-v1","bodyModuleId":"body-balanced-v1","bodyModuleSource":"ai-completed","traits":[{{"key":"faceShape","value":"{face}","source":"user","evidencePhotoIds":["front"]}}],"completionSummary":[]}}"#
        )).unwrap()
    }

    fn completion(requested_trait_keys: Vec<IdentityTraitKey>) -> AppearanceCompletionV1 {
        AppearanceCompletionV1 {
            requested_trait_keys,
            completed_traits: ALL_IDENTITY_TRAIT_KEYS
                .iter()
                .copied()
                .filter(|key| *key != IdentityTraitKey::FaceShape)
                .map(|key| IdentityTraitV1 {
                    key,
                    value: "completed".into(),
                    source: TraitSource::AiCompleted,
                    evidence_photo_ids: Vec::new(),
                })
                .collect(),
            body_module_id: "body-balanced-v1".into(),
            body_module_source: TraitSource::AiCompleted,
        }
    }

    fn temporary(message: &str) -> FakeOutcome {
        FakeOutcome::Error(PhotoAvatarError {
            code: PhotoAvatarErrorCode::TemporaryUnavailable,
            retryable: true,
            message: message.into(),
        })
    }

    fn canonical_audit() -> CanonicalTextureAuditV1 {
        CanonicalTextureAuditV1 {
            schema_version: 1,
            session_id: "session-a".into(),
            revision: 7,
            attempt: 2,
            provider: "lk888".into(),
            provider_model: "gpt-image-2".into(),
            model_display_name: "GPT-image-2.0".into(),
            api_contract_version: "lk888-media-generate-v1".into(),
            privacy_policy_version: "unverified".into(),
            retention_policy: "unverified".into(),
            upstream_delete_api: "unsupported".into(),
            provider_task_id: "task-1".into(),
            provider_raw_sha256: "11".repeat(32),
            canonical_sha256: "00".repeat(32),
            body_module_id: "body-balanced-v1".into(),
            module_contract_sha256: "22".repeat(32),
            source_texture_sha256: "33".repeat(32),
            source_alpha_sha256: "44".repeat(32),
            work_canvas_sha256: "55".repeat(32),
            region_map_sha256: "66".repeat(32),
            composer_version: "deterministic-alpha-v1".into(),
            png_encoder_version: "pillow-png-v1".into(),
            coverage_report: serde_json::json!({"minimumChangeRatio": 0.95}),
            status: "succeeded".into(),
            error_code: None,
            created_at: "2026-08-17T00:00:00Z".into(),
            completed_at: "2026-08-17T00:00:01Z".into(),
        }
    }

    fn canonical_audit_for_texture(
        module_root: &std::path::Path,
        session_id: &str,
        revision: u32,
        module_id: &str,
        texture_png: &[u8],
    ) -> CanonicalTextureAuditV1 {
        let module_dir = module_root.join(module_id);
        let source_texture =
            std::fs::read(module_dir.join(format!("{module_id}.2048/texture_00.png"))).unwrap();
        let source_alpha = image::load_from_memory_with_format(&source_texture, ImageFormat::Png)
            .unwrap()
            .to_rgba8()
            .pixels()
            .map(|pixel| pixel[3])
            .collect::<Vec<_>>();
        let canonical_sha256 = format!("{:x}", Sha256::digest(texture_png));
        let identity_reference_sha256 = "77".repeat(32);
        let profile_sha256 = "88".repeat(32);
        let mask_sha256 = "ea0812149b2bb367eca38438b22a928e1148a5d348d4ad17f0a3c95cb182d404";
        let layer_ids = [
            "body-base",
            "face",
            "eyes-eyelids",
            "ears",
            "chest-forelegs",
            "tail",
            "occlusion-underlay",
        ];
        let layers = layer_ids
            .iter()
            .map(|layer_id| {
                serde_json::json!({
                    "layerId": layer_id,
                    "providerRawSha256": "99".repeat(32),
                    "canonicalLayerSha256": "aa".repeat(32),
                    "maskSha256": mask_sha256,
                    "attempt": 1,
                })
            })
            .collect::<Vec<_>>();
        let mut digest_fields = vec![identity_reference_sha256.clone(), profile_sha256.clone()];
        for layer_id in layer_ids {
            digest_fields.extend([
                layer_id.into(),
                "99".repeat(32),
                "aa".repeat(32),
                mask_sha256.into(),
                "1".into(),
            ]);
        }
        digest_fields.extend([canonical_sha256.clone(), module_id.into()]);
        let mut audit = canonical_audit();
        audit.provider_raw_sha256 = format!("{:x}", Sha256::digest(digest_fields.join("\n")));
        audit.coverage_report = serde_json::json!({
            "identityReferenceSha256": identity_reference_sha256,
            "profileSha256": profile_sha256,
            "layers": layers,
            "canonicalAtlasSha256": canonical_sha256,
            "bodyModuleId": module_id,
        });
        CanonicalTextureAuditV1 {
            session_id: session_id.into(),
            revision,
            canonical_sha256,
            body_module_id: module_id.into(),
            module_contract_sha256: format!(
                "{:x}",
                Sha256::digest(std::fs::read(module_dir.join("模块.json")).unwrap())
            ),
            source_texture_sha256: format!("{:x}", Sha256::digest(&source_texture)),
            source_alpha_sha256: format!("{:x}", Sha256::digest(&source_alpha)),
            ..audit
        }
    }

    #[test]
    fn canonical_audit_identity_must_match_current_render_attempt() {
        let run = PhotoAvatarRunState {
            session_id: "session-a".into(),
            revision: 7,
            step: PhotoAvatarStep::RenderTextureAtlas,
            generation_token: "token-a".into(),
            provider_session_id: Some("provider-session-a".into()),
            provider_job_id: Some("job-a".into()),
            modification: None,
            locked_traits: Vec::new(),
            current_attempt: Some(2),
        };
        let profile = profile("round");
        let contract = "22".repeat(32);
        let audit = canonical_audit();
        assert!(
            super::validate_texture_audit_identity(&run, 2, &profile, &contract, &audit).is_ok()
        );

        for invalid in [
            CanonicalTextureAuditV1 {
                session_id: "session-other".into(),
                ..audit.clone()
            },
            CanonicalTextureAuditV1 {
                revision: 8,
                ..audit.clone()
            },
            CanonicalTextureAuditV1 {
                attempt: 3,
                ..audit.clone()
            },
            CanonicalTextureAuditV1 {
                body_module_id: "body-rounded-v1".into(),
                ..audit.clone()
            },
            CanonicalTextureAuditV1 {
                module_contract_sha256: "77".repeat(32),
                ..audit.clone()
            },
        ] {
            assert!(
                super::validate_texture_audit_identity(&run, 2, &profile, &contract, &invalid)
                    .is_err()
            );
        }
    }

    #[test]
    fn optional_status_is_none_before_begin_and_some_after_begin() {
        let h = Harness::new(vec![FakeOutcome::Running]);

        assert_eq!(h.manager.status_if_exists(&h.session).unwrap(), None);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        assert!(h.manager.status_if_exists(&h.session).unwrap().is_some());
    }

    #[test]
    fn preview_manifest_revalidates_historical_texture_layout_before_serving() {
        let h = Harness::new(vec![]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        let run = h.store.current_run(&h.session).unwrap();
        let module_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../public/cat-character-modules/cat-a-live2d-v1");
        let preview_root = h.root.join("previews");
        let builder = PhotoAvatarBuilder::new(&module_root, &preview_root);
        let neutral = std::fs::read(
            module_root.join("body-balanced-v1/body-balanced-v1.2048/texture_00.png"),
        )
        .unwrap();
        let mut valid_image = image::load_from_memory_with_format(&neutral, ImageFormat::Png)
            .unwrap()
            .to_rgba8();
        for pixel in valid_image.pixels_mut() {
            let alpha = pixel[3];
            pixel.0[..3].copy_from_slice(if alpha == 0 {
                &[0, 0, 0]
            } else {
                &[67, 68, 69]
            });
        }
        let mut valid_atlas = Vec::new();
        DynamicImage::ImageRgba8(valid_image)
            .write_to(&mut Cursor::new(&mut valid_atlas), ImageFormat::Png)
            .unwrap();
        let built = builder
            .build_preview(BuildPhotoAvatarRequest {
                session_id: h.session.clone(),
                revision: run.revision,
                pet_id: h.store.pet_id(&h.session).unwrap(),
                variant_id: format!("photo-avatar-{}-{}", h.session, run.revision),
                profile: profile("round"),
                texture_sha256: format!("{:x}", Sha256::digest(&valid_atlas)),
                texture_audit: canonical_audit_for_texture(
                    &module_root,
                    &h.session,
                    run.revision,
                    "body-balanced-v1",
                    &valid_atlas,
                ),
                texture_png: valid_atlas,
            })
            .unwrap();
        let mut wrong_atlas = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            2048,
            2048,
            image::Rgba([67, 68, 69, 255]),
        ))
        .write_to(&mut Cursor::new(&mut wrong_atlas), ImageFormat::Png)
        .unwrap();
        std::fs::write(built.texture(), &wrong_atlas).unwrap();
        let wrong_atlas_sha = format!("{:x}", Sha256::digest(&wrong_atlas));
        let audit_path = built.preview_dir.join("canonical-texture-audit.json");
        let mut audit: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&audit_path).unwrap()).unwrap();
        audit["canonicalSha256"] = wrong_atlas_sha.clone().into();
        let audit_bytes = serde_json::to_vec_pretty(&audit).unwrap();
        std::fs::write(&audit_path, &audit_bytes).unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&built.manifest_path).unwrap()).unwrap();
        for file in manifest["files"].as_array_mut().unwrap() {
            match file["role"].as_str().unwrap() {
                "texture" => file["sha256"] = wrong_atlas_sha.clone().into(),
                "canonical-texture-audit" => {
                    file["sha256"] = format!("{:x}", Sha256::digest(&audit_bytes)).into()
                }
                _ => {}
            }
        }
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        std::fs::write(&built.manifest_path, &manifest_bytes).unwrap();
        {
            let storage = h.storage.lock().unwrap();
            storage
                .db
                .execute(
                    "UPDATE photo_avatar_runs SET step='buildV5' WHERE session_id=?1",
                    [&h.session],
                )
                .unwrap();
        }
        h.store
            .commit_preview_package(
                &h.session,
                run.revision,
                &run.generation_token,
                &built.preview_dir,
                &format!("{:x}", Sha256::digest(&manifest_bytes)),
            )
            .unwrap();
        let manager =
            PhotoAvatarManager::new(h.store.clone(), h.provider.clone()).with_builder(builder);

        let error = manager
            .preview_manifest(&h.session, run.revision)
            .unwrap_err();

        assert!(error.contains("UV alpha layout"), "{error}");
    }

    #[test]
    fn render_request_includes_body_module_contract_hash() {
        let h = Harness::new(vec![]);
        let provider = FakePhotoAvatarProvider::for_body_module("body-balanced-v1").unwrap();
        let provider = Arc::new(provider);
        let module_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../public/cat-character-modules/cat-a-live2d-v1");
        let manager = PhotoAvatarManager::new(h.store.clone(), provider.clone()).with_builder(
            crate::runtime_assets::photo_avatar_builder::PhotoAvatarBuilder::new(
                &module_root,
                &h.root.join("previews"),
            ),
        );

        manager.consent(true).unwrap();
        manager.begin(&h.session, vec![h.source()]).unwrap();
        for _ in 0..5 {
            manager.tick(&h.session).unwrap();
        }

        let expected = format!(
            "{:x}",
            Sha256::digest(std::fs::read(module_root.join("body-balanced-v1/模块.json")).unwrap(),)
        );
        let render = provider
            .requests()
            .into_iter()
            .find(|request| request.step == RemoteStep::RenderTextureAtlas)
            .expect("render request should be submitted");
        assert_eq!(
            render.body_module_contract_sha256.as_deref(),
            Some(expected.as_str())
        );
        manager.tick(&h.session).unwrap();
        let artifact = h
            .store
            .texture_artifact(&h.session, 1)
            .unwrap()
            .expect("canonical texture artifact should be persisted");
        let audit: CanonicalTextureAuditV1 =
            serde_json::from_str(artifact.audit_json.as_deref().unwrap()).unwrap();
        assert_eq!(audit.session_id, h.session);
        assert_eq!(audit.module_contract_sha256, expected);
        assert_eq!(audit.canonical_sha256, artifact.sha256);
    }

    #[test]
    fn retries_each_remote_step_three_total_attempts_without_counting_polls() {
        let h = Harness::new(vec![
            temporary("one"),
            temporary("two"),
            FakeOutcome::Success {
                profile: profile("round"),
            },
        ]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        h.tick_n(6);
        let snapshot = h.store.snapshot(&h.session).unwrap();
        assert_eq!(snapshot.step, PhotoAvatarStep::CompleteAppearance);
        assert_eq!(
            snapshot
                .attempts
                .get(&PhotoAvatarAttemptStep::AnalyzeIdentity),
            Some(&3)
        );

        let pending = Harness::new(vec![FakeOutcome::Running]);
        pending.manager.consent(true).unwrap();
        pending
            .manager
            .begin(&pending.session, vec![pending.source()])
            .unwrap();
        pending.tick_n(5);
        assert_eq!(pending.provider.requests().len(), 1);
        assert_eq!(
            pending
                .store
                .snapshot(&pending.session)
                .unwrap()
                .attempts
                .get(&PhotoAvatarAttemptStep::AnalyzeIdentity),
            Some(&1)
        );
    }

    #[test]
    fn non_retryable_and_third_retryable_failure_are_terminal() {
        let fatal = Harness::new(vec![FakeOutcome::Error(PhotoAvatarError {
            code: PhotoAvatarErrorCode::Auth,
            retryable: false,
            message: "denied".into(),
        })]);
        fatal.manager.consent(true).unwrap();
        fatal
            .manager
            .begin(&fatal.session, vec![fatal.source()])
            .unwrap();
        fatal.tick_n(2);
        assert_eq!(
            fatal.store.snapshot(&fatal.session).unwrap().step,
            PhotoAvatarStep::Failed
        );
        assert!(fatal.manager.tick(&fatal.session).is_ok());
        assert_eq!(fatal.provider.requests().len(), 1);

        let exhausted = Harness::new(vec![temporary("1"), temporary("2"), temporary("3")]);
        exhausted.manager.consent(true).unwrap();
        exhausted
            .manager
            .begin(&exhausted.session, vec![exhausted.source()])
            .unwrap();
        exhausted.tick_n(6);
        let snapshot = exhausted.store.snapshot(&exhausted.session).unwrap();
        assert_eq!(snapshot.step, PhotoAvatarStep::Failed);
        assert_eq!(
            snapshot
                .attempts
                .get(&PhotoAvatarAttemptStep::AnalyzeIdentity),
            Some(&3)
        );
    }

    #[test]
    fn submit_retryability_is_derived_only_from_error_code() {
        for code in [PhotoAvatarErrorCode::Auth, PhotoAvatarErrorCode::Quota] {
            let h = Harness::new(vec![]);
            let provider = Arc::new(SubmitErrorProvider::new(vec![PhotoAvatarError {
                code,
                retryable: true,
                message: "provider boolean conflicts with code".into(),
            }]));
            let manager = PhotoAvatarManager::new(h.store.clone(), provider.clone());
            manager.consent(true).unwrap();
            manager.begin(&h.session, vec![h.source()]).unwrap();

            let snapshot = manager.tick(&h.session).unwrap();

            assert_eq!(snapshot.step, PhotoAvatarStep::Failed, "{code:?}");
            assert_eq!(provider.submit_count.load(Ordering::SeqCst), 1);
        }

        let h = Harness::new(vec![]);
        let provider = Arc::new(SubmitErrorProvider::new(vec![PhotoAvatarError {
            code: PhotoAvatarErrorCode::Network,
            retryable: false,
            message: "provider boolean conflicts with code".into(),
        }]));
        let manager = PhotoAvatarManager::new(h.store.clone(), provider.clone());
        manager.consent(true).unwrap();
        manager.begin(&h.session, vec![h.source()]).unwrap();

        assert_eq!(
            manager.tick(&h.session).unwrap().step,
            PhotoAvatarStep::AnalyzeIdentity
        );
        manager.tick(&h.session).unwrap();
        assert_eq!(provider.submit_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn invalid_success_profile_fails_instead_of_remaining_running() {
        let mut invalid = profile("round");
        invalid.style = "unapproved-style".into();
        let h = Harness::new(vec![FakeOutcome::Success { profile: invalid }]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        h.manager.tick(&h.session).unwrap();
        let snapshot = h.manager.tick(&h.session).unwrap();
        assert_eq!(snapshot.step, PhotoAvatarStep::Failed);
        assert_eq!(
            snapshot.error_code,
            Some(PhotoAvatarErrorCode::InvalidInput)
        );
    }

    #[test]
    fn regenerate_and_revise_advance_revision_and_enforce_locked_traits() {
        let h = Harness::new(vec![
            FakeOutcome::Success {
                profile: profile("round"),
            },
            FakeOutcome::Appearance {
                completion: completion(Vec::new()),
            },
            FakeOutcome::Success {
                profile: profile("triangle"),
            },
            FakeOutcome::Appearance {
                completion: completion(vec![IdentityTraitKey::Tail]),
            },
        ]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        h.tick_n(4);
        assert_eq!(
            h.store.snapshot(&h.session).unwrap().step,
            PhotoAvatarStep::RenderTextureAtlas
        );
        assert_eq!(h.manager.regenerate(&h.session).unwrap().revision, 2);
        assert_eq!(
            h.manager
                .revise(&h.session, "fluffier tail")
                .unwrap()
                .revision,
            3
        );
        h.tick_n(4);
        let snapshot = h.store.snapshot(&h.session).unwrap();
        assert_eq!(snapshot.step, PhotoAvatarStep::Failed);
        assert!(snapshot
            .error_message
            .unwrap()
            .contains("locked trait changed"));
    }

    #[test]
    fn regenerated_analysis_does_not_send_the_previous_profile() {
        let h = Harness::new(vec![
            FakeOutcome::Success {
                profile: profile("round"),
            },
            FakeOutcome::Appearance {
                completion: completion(Vec::new()),
            },
            FakeOutcome::Running,
        ]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        h.tick_n(4);

        h.manager.regenerate(&h.session).unwrap();
        h.manager.tick(&h.session).unwrap();

        let requests = h.provider.requests();
        let regenerated = requests.last().expect("regenerated analysis request");
        assert_eq!(regenerated.step, RemoteStep::AnalyzeIdentity);
        assert_eq!(regenerated.profile, None);
    }

    #[test]
    fn background_worker_keeps_running_for_the_local_build_step() {
        assert!(!is_terminal_or_local(PhotoAvatarStep::BuildV5));
        assert!(is_terminal_or_local(PhotoAvatarStep::RuntimeCheckPending));
    }

    #[test]
    fn regenerate_and_start_background_launches_exactly_one_worker() {
        let h = Harness::new(vec![]);
        let provider = Arc::new(BlockingSubmitProvider::new());
        let manager = PhotoAvatarManager::new(h.store.clone(), provider.clone());
        manager.consent(true).unwrap();
        manager.begin(&h.session, vec![h.source()]).unwrap();

        let snapshot = manager.regenerate_and_start_background(&h.session).unwrap();
        provider.wait_until_first_submit_enters();
        manager.start_background(&h.session).unwrap();

        assert_eq!(snapshot.revision, 2);
        assert_eq!(provider.submit_count.load(Ordering::SeqCst), 1);

        let run = h.store.current_run(&h.session).unwrap();
        manager.remove_active_token(&h.session, &run.generation_token);
        provider.release_first_submit();
    }

    #[test]
    fn revise_and_start_background_launches_exactly_one_worker() {
        let h = Harness::new(vec![]);
        let provider = Arc::new(BlockingSubmitProvider::new());
        let manager = PhotoAvatarManager::new(h.store.clone(), provider.clone());
        manager.consent(true).unwrap();
        manager.begin(&h.session, vec![h.source()]).unwrap();

        let snapshot = manager
            .revise_and_start_background(&h.session, "fluffier tail")
            .unwrap();
        provider.wait_until_first_submit_enters();
        manager.start_background(&h.session).unwrap();

        assert_eq!(snapshot.revision, 2);
        assert_eq!(provider.submit_count.load(Ordering::SeqCst), 1);

        let run = h.store.current_run(&h.session).unwrap();
        manager.remove_active_token(&h.session, &run.generation_token);
        provider.release_first_submit();
    }

    #[test]
    fn tail_only_revision_rejects_a_changed_body_module() {
        let mut changed_body = completion(vec![IdentityTraitKey::Tail]);
        changed_body.body_module_id = "body-rounded-v1".into();
        let h = Harness::new(vec![
            FakeOutcome::Success {
                profile: profile("round"),
            },
            FakeOutcome::Appearance {
                completion: completion(Vec::new()),
            },
            FakeOutcome::Success {
                profile: profile("round"),
            },
            FakeOutcome::Appearance {
                completion: changed_body,
            },
        ]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        h.tick_n(4);
        h.manager.revise(&h.session, "fluffier tail").unwrap();

        h.tick_n(4);

        let snapshot = h.store.snapshot(&h.session).unwrap();
        assert_eq!(snapshot.step, PhotoAvatarStep::Failed);
        assert!(snapshot
            .error_message
            .unwrap()
            .contains("locked body module changed"));
    }

    #[test]
    fn initial_completion_accepts_empty_requested_traits_and_finalizes_all_identity_traits() {
        let h = Harness::new(vec![
            FakeOutcome::Success {
                profile: profile("round"),
            },
            FakeOutcome::Appearance {
                completion: completion(Vec::new()),
            },
        ]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();

        h.tick_n(4);

        let snapshot = h.store.snapshot(&h.session).unwrap();
        assert_eq!(snapshot.step, PhotoAvatarStep::RenderTextureAtlas);
        assert_eq!(snapshot.profile.unwrap().traits.len(), 11);
    }

    #[test]
    fn legacy_positive_analysis_profile_recovers_and_commits_an_initial_completion() {
        let legacy_final = profile("triangle");
        let h = Harness::new(vec![FakeOutcome::LegacyAppearance {
            profile: legacy_final.clone(),
        }]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        let run = h.store.current_run(&h.session).unwrap();
        let legacy_partial = profile("round");
        let legacy_json = serde_json::to_string(&legacy_partial).unwrap();
        let legacy_sha256 = format!("{:x}", Sha256::digest(&legacy_json));
        let storage = Storage::open(&h.root).unwrap();
        storage
            .db
            .execute(
                "INSERT INTO photo_avatar_profiles
                 (session_id, revision, schema_version, body_module_id, profile_json,
                  profile_sha256, created_at)
                 VALUES (?1, ?2, 1, ?3, ?4, ?5, '10')",
                rusqlite::params![
                    h.session,
                    run.revision,
                    legacy_partial.body_module_id,
                    legacy_json,
                    legacy_sha256,
                ],
            )
            .unwrap();
        storage
            .db
            .execute(
                "UPDATE photo_avatar_runs
                 SET step='completeAppearance', provider_session_id='legacy-session'
                 WHERE session_id=?1",
                [h.session.as_str()],
            )
            .unwrap();

        h.tick_n(2);

        assert_eq!(h.provider.requests()[0].profile, Some(legacy_partial));
        let snapshot = h.store.snapshot(&h.session).unwrap();
        assert_eq!(snapshot.step, PhotoAvatarStep::RenderTextureAtlas);
        assert_eq!(snapshot.profile, Some(legacy_final));
    }

    #[test]
    fn legacy_appearance_profile_cannot_bypass_a_modification_revision_lock() {
        let h = Harness::new(vec![
            FakeOutcome::Success {
                profile: profile("round"),
            },
            FakeOutcome::Appearance {
                completion: completion(Vec::new()),
            },
            FakeOutcome::Success {
                profile: profile("round"),
            },
            FakeOutcome::LegacyAppearance {
                profile: profile("triangle"),
            },
        ]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        h.tick_n(4);
        h.manager.revise(&h.session, "fluffier tail").unwrap();
        h.tick_n(4);

        let snapshot = h.store.snapshot(&h.session).unwrap();
        assert_eq!(snapshot.step, PhotoAvatarStep::Failed);
        assert_eq!(
            snapshot.error_code,
            Some(PhotoAvatarErrorCode::InvalidInput)
        );
        assert!(snapshot
            .error_message
            .unwrap()
            .contains("requestedTraitKeys"));
    }

    #[test]
    fn revision_instruction_is_plain_text_and_empty_provider_requests_are_rejected() {
        let h = Harness::new(vec![
            FakeOutcome::Success {
                profile: profile("round"),
            },
            FakeOutcome::Appearance {
                completion: completion(Vec::new()),
            },
            FakeOutcome::Success {
                profile: profile("round"),
            },
            FakeOutcome::Appearance {
                completion: completion(Vec::new()),
            },
        ]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        h.tick_n(4);

        h.manager.revise(&h.session, "  fluffier tail  ").unwrap();
        let run = h.store.current_run(&h.session).unwrap();
        assert_eq!(run.modification.as_deref(), Some("fluffier tail"));
        assert!(run.locked_traits.is_empty());
        h.tick_n(4);

        let snapshot = h.store.snapshot(&h.session).unwrap();
        assert_eq!(snapshot.step, PhotoAvatarStep::Failed);
        assert!(snapshot
            .error_message
            .unwrap()
            .contains("requestedTraitKeys must be non-empty"));
    }

    #[test]
    fn old_frontend_revision_json_is_not_parsed_as_trusted_locked_keys() {
        let h = Harness::new(Vec::new());
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        let old_json = r#"{"instruction":"fluffier tail","lockedTraitKeys":["faceShape"]}"#;

        h.manager.revise(&h.session, old_json).unwrap();

        let run = h.store.current_run(&h.session).unwrap();
        assert_eq!(run.modification.as_deref(), Some(old_json));
        assert!(run.locked_traits.is_empty());
    }

    #[test]
    fn render_texture_atlas_request_reuses_the_sessions_controlled_sources() {
        let h = Harness::new(vec![
            FakeOutcome::Success {
                profile: profile("round"),
            },
            FakeOutcome::Appearance {
                completion: completion(Vec::new()),
            },
            FakeOutcome::TextureAtlas { bytes: Vec::new() },
        ]);
        let module_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../public/cat-character-modules/cat-a-live2d-v1");
        let manager = PhotoAvatarManager::new(h.store.clone(), h.provider.clone()).with_builder(
            crate::runtime_assets::photo_avatar_builder::PhotoAvatarBuilder::new(
                &module_root,
                &h.root.join("previews"),
            ),
        );
        manager.consent(true).unwrap();
        manager.begin(&h.session, vec![h.source()]).unwrap();
        for _ in 0..5 {
            manager.tick(&h.session).unwrap();
        }

        let requests = h.provider.requests();
        let atlas = requests
            .iter()
            .find(|request| request.step == RemoteStep::RenderTextureAtlas)
            .expect("manager must submit renderTextureAtlas");
        assert_eq!(atlas.source_images.len(), 1);
        assert_eq!(
            atlas.source_images[0].source_id,
            requests[0].source_images[0].source_id
        );
        assert_eq!(
            atlas.source_images[0].sha256,
            requests[0].source_images[0].sha256
        );
    }

    #[test]
    fn legacy_v1_consent_cannot_begin_a_new_run() {
        let h = Harness::new(Vec::new());
        h.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO photo_avatar_consents(consent_version, accepted_at)
                 VALUES ('photo-avatar-third-party-ai-v1', '10')",
                [],
            )
            .unwrap();

        let error = h.manager.begin(&h.session, vec![h.source()]).unwrap_err();
        assert!(error.contains("consent is required"));
        assert!(h.store.sources(&h.session).unwrap().is_empty());
        assert!(h.store.current_run(&h.session).is_err());
        let consent_count: i64 = h
            .storage
            .lock()
            .unwrap()
            .db
            .query_row("SELECT COUNT(*) FROM photo_avatar_consents", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(consent_count, 1);
    }

    #[test]
    fn begin_without_explicit_v2_consent_does_not_create_sources_or_run() {
        let h = Harness::new(Vec::new());

        let error = h.manager.begin(&h.session, vec![h.source()]).unwrap_err();

        assert!(error.contains("consent is required"));
        assert!(h.store.sources(&h.session).unwrap().is_empty());
        assert!(h.store.current_run(&h.session).is_err());
        assert!(!h
            .store
            .consent_accepted(PHOTO_AVATAR_CONSENT_VERSION)
            .unwrap());
    }

    #[test]
    fn cancellation_rejects_late_result_and_stops_submits() {
        let h = Harness::new(vec![FakeOutcome::Running]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        h.manager.tick(&h.session).unwrap();
        let run = h.store.current_run(&h.session).unwrap();
        let cancelled = h.manager.cancel(&h.session).unwrap();
        assert_eq!(cancelled.step, PhotoAvatarStep::Cancelled);
        assert!(h.store.sources(&h.session).unwrap().is_empty());
        assert_eq!(h.provider.deleted_sessions(), vec!["fake-session-1"]);
        let cancelled_run = h.store.current_run(&h.session).unwrap();
        assert_eq!(cancelled_run.provider_session_id, None);
        assert_eq!(cancelled_run.provider_job_id, None);
        assert!(h
            .manager
            .apply_polled_result(
                &run,
                "fake-job-1",
                crate::creation::photo_avatar::provider::RemoteJobState {
                    state: "succeeded".into(),
                    result: Some(
                        crate::creation::photo_avatar::provider::ProviderStepResult::Identity {
                            partial_profile: profile("round")
                        }
                    ),
                    error: None,
                }
            )
            .unwrap_err()
            .contains("superseded"));
        h.manager.tick(&h.session).unwrap();
        assert_eq!(h.provider.requests().len(), 1);
    }

    #[test]
    fn pending_backend_delete_is_resumed_until_cleanup_completes() {
        let h = Harness::new(vec![]);
        let provider = Arc::new(DeleteSequenceProvider::new(vec![
            Ok(CleanupState::Pending),
            Ok(CleanupState::Deleted),
        ]));
        let manager = PhotoAvatarManager::new(h.store.clone(), provider.clone());
        manager.consent(true).unwrap();
        manager.begin(&h.session, vec![h.source()]).unwrap();
        manager.tick(&h.session).unwrap();

        let pending = manager.cancel(&h.session).unwrap();

        assert_eq!(pending.step, PhotoAvatarStep::CleanupPending);
        assert!(h.store.sources(&h.session).unwrap().is_empty());
        let pending_run = h.store.current_run(&h.session).unwrap();
        assert_eq!(
            pending_run.provider_session_id.as_deref(),
            Some("fake-session-1")
        );
        assert_eq!(pending_run.provider_job_id, None);
        assert_eq!(
            manager.resume_all().unwrap().resumed_session_ids,
            vec![h.session.clone()]
        );
        assert_eq!(
            h.store.snapshot(&h.session).unwrap().step,
            PhotoAvatarStep::Cancelled
        );
        let cleaned_run = h.store.current_run(&h.session).unwrap();
        assert_eq!(cleaned_run.provider_session_id, None);
        assert_eq!(cleaned_run.provider_job_id, None);
        let audit: (String, String, String) = h
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT local_cleanup, backend_cleanup, upstream_cleanup
                 FROM photo_avatar_cleanup_audit WHERE session_id=?1 AND revision=1",
                [&h.session],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            audit,
            ("deleted".into(), "deleted".into(), "unsupported".into())
        );
        assert_eq!(
            provider.deleted_sessions(),
            vec!["fake-session-1", "fake-session-1"]
        );
    }

    #[test]
    fn accepted_preview_deletes_local_sources_and_resumes_remote_cleanup_without_reinstall() {
        let h = Harness::new(vec![]);
        let provider = Arc::new(DeleteSequenceProvider::new(vec![
            Err(PhotoAvatarError {
                code: PhotoAvatarErrorCode::Network,
                retryable: true,
                message: "offline".into(),
            }),
            Ok(CleanupState::Deleted),
        ]));
        let manager = PhotoAvatarManager::new(h.store.clone(), provider.clone());
        manager.consent(true).unwrap();
        manager.begin(&h.session, vec![h.source()]).unwrap();
        manager.tick(&h.session).unwrap();
        {
            let storage = h.storage.lock().unwrap();
            storage
                .db
                .execute(
                    "UPDATE photo_avatar_runs SET step='previewReady' WHERE session_id=?1",
                    [&h.session],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO photo_avatar_artifacts
                     (session_id, revision, kind, relative_path, sha256, local_path, created_at)
                     VALUES (?1, 1, 'previewPackage', 'preview', ?2, 'preview', '10')",
                    rusqlite::params![h.session, "a".repeat(64)],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "UPDATE creation_sessions
                     SET status='finalizing', last_stable_status='candidateReady'
                     WHERE session_id=?1",
                    [&h.session],
                )
                .unwrap();
        }

        assert!(matches!(
            manager.cleanup_after_accept(&h.session).unwrap(),
            crate::creation::finalization::PhotoAvatarCleanupDisposition::Pending(_)
        ));
        assert!(h.store.sources(&h.session).unwrap().is_empty());
        assert_eq!(
            h.store.snapshot(&h.session).unwrap().step,
            PhotoAvatarStep::CleanupPending
        );

        manager.resume_all().unwrap();

        assert_eq!(
            h.store.snapshot(&h.session).unwrap().step,
            PhotoAvatarStep::Completed
        );
        assert_eq!(
            provider.deleted_sessions(),
            vec!["fake-session-1", "fake-session-1"]
        );
    }

    #[test]
    fn full_exit_cancels_collecting_sources_but_preserves_attached_provider_job() {
        let collecting = Harness::new(vec![]);
        collecting.manager.consent(true).unwrap();
        collecting
            .manager
            .begin(&collecting.session, vec![collecting.source()])
            .unwrap();
        collecting
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE photo_avatar_runs SET step='collecting' WHERE session_id=?1",
                [&collecting.session],
            )
            .unwrap();
        collecting
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO photo_avatar_artifacts
                 (session_id, revision, kind, relative_path, sha256, local_path, created_at)
                 VALUES (?1, 1, 'previewPackage', 'preview', ?2, 'preview', '10')",
                rusqlite::params![collecting.session, "a".repeat(64)],
            )
            .unwrap();

        collecting.manager.prepare_for_full_exit().unwrap();

        assert_eq!(
            collecting.store.snapshot(&collecting.session).unwrap().step,
            PhotoAvatarStep::Cancelled
        );
        assert!(collecting
            .store
            .sources(&collecting.session)
            .unwrap()
            .is_empty());
        let collecting_facts: (String, String, i64) = collecting
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT cs.status, p.lifecycle,
                        (SELECT COUNT(*) FROM photo_avatar_artifacts WHERE session_id=cs.session_id)
                 FROM creation_sessions cs JOIN pets p ON p.pet_id=cs.pet_id
                 WHERE cs.session_id=?1",
                [&collecting.session],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            collecting_facts,
            ("abandoned".into(), "abandoned".into(), 0)
        );

        let running = Harness::new(vec![]);
        running.manager.consent(true).unwrap();
        running
            .manager
            .begin(&running.session, vec![running.source()])
            .unwrap();
        running.manager.tick(&running.session).unwrap();
        let before = running.store.current_run(&running.session).unwrap();
        assert!(before.provider_job_id.is_some());
        let running_facts_before: (String, String) = running
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT cs.status, p.lifecycle
                 FROM creation_sessions cs JOIN pets p ON p.pet_id=cs.pet_id
                 WHERE cs.session_id=?1",
                [&running.session],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        running.manager.prepare_for_full_exit().unwrap();

        let after = running.store.current_run(&running.session).unwrap();
        assert_eq!(after.step, PhotoAvatarStep::AnalyzeIdentity);
        assert_eq!(after.provider_job_id, before.provider_job_id);
        assert!(!running.store.sources(&running.session).unwrap().is_empty());
        let running_facts_after: (String, String) = running
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT cs.status, p.lifecycle
                 FROM creation_sessions cs JOIN pets p ON p.pet_id=cs.pet_id
                 WHERE cs.session_id=?1",
                [&running.session],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(running_facts_after, running_facts_before);
    }

    #[test]
    fn resume_all_isolates_and_reports_a_failed_session_while_continuing_others() {
        let h = Harness::new(vec![]);
        let provider = Arc::new(DeleteSequenceProvider::new(vec![
            Err(PhotoAvatarError {
                code: PhotoAvatarErrorCode::Network,
                retryable: true,
                message: "offline".into(),
            }),
            Ok(CleanupState::Deleted),
        ]));
        let manager = PhotoAvatarManager::new(h.store.clone(), provider.clone());
        manager.consent(true).unwrap();
        manager.begin(&h.session, vec![h.source()]).unwrap();
        manager.tick(&h.session).unwrap();
        assert_eq!(
            manager.cancel(&h.session).unwrap().step,
            PhotoAvatarStep::CleanupPending
        );
        {
            let storage = h.storage.lock().unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO pets
                     (pet_id, schema_version, species, identity_mode, creation_method, lifecycle, created_at, updated_at)
                     VALUES ('a-broken-pet', 1, 'cat', 'realpet', 'upload', 'draft', '10', '10')",
                    [],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO creation_sessions
                     (session_id, pet_id, method, status, last_stable_status, current_step, schema_version, created_at, updated_at)
                     VALUES ('a-broken-session', 'a-broken-pet', 'upload', 'abandoned', 'draft', 'upload', 1, '10', '10')",
                    [],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO photo_avatar_runs
                     (session_id, revision, step, generation_token, locked_trait_keys_json, updated_at)
                     VALUES ('a-broken-session', 1, 'cleanupPending', 'broken', 'not-json', '10')",
                    [],
                )
                .unwrap();
        }

        let report = manager.resume_all().unwrap();

        assert_eq!(report.resumed_session_ids, vec![h.session.clone()]);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].session_id, "a-broken-session");
        assert!(!report.failures[0].error.is_empty());
        assert_eq!(
            h.store.snapshot(&h.session).unwrap().step,
            PhotoAvatarStep::Cancelled
        );
        assert_eq!(
            provider.deleted_sessions(),
            vec!["fake-session-1", "fake-session-1"]
        );
    }

    #[test]
    fn concurrent_ticks_share_one_active_submit_attempt() {
        let h = Harness::new(vec![]);
        let provider = Arc::new(BlockingSubmitProvider::new());
        let manager = PhotoAvatarManager::new(h.store.clone(), provider.clone());
        manager.consent(true).unwrap();
        manager.begin(&h.session, vec![h.source()]).unwrap();

        let first_manager = manager.clone();
        let first_session = h.session.clone();
        let first_tick = std::thread::spawn(move || first_manager.tick(&first_session));
        provider.wait_until_first_submit_enters();

        let second_snapshot = manager.tick(&h.session).unwrap();
        provider.release_first_submit();
        let first_snapshot = first_tick.join().unwrap().unwrap();

        assert_eq!(provider.submit_count.load(Ordering::SeqCst), 1);
        assert_eq!(second_snapshot.step, PhotoAvatarStep::AnalyzeIdentity);
        assert_eq!(
            first_snapshot.provider_job_id.as_deref(),
            Some("fake-job-1")
        );
        assert_eq!(
            h.store
                .snapshot(&h.session)
                .unwrap()
                .attempts
                .get(&PhotoAvatarAttemptStep::AnalyzeIdentity),
            Some(&1)
        );
    }

    fn assert_stale_cancel_cleanup_cannot_mutate_new_revision(delete_succeeds: bool) {
        let h = Harness::new(vec![]);
        let provider = Arc::new(BlockingDeleteProvider::new(delete_succeeds));
        let manager = PhotoAvatarManager::new(h.store.clone(), provider.clone());
        manager.consent(true).unwrap();
        manager.begin(&h.session, vec![h.source()]).unwrap();
        manager.tick(&h.session).unwrap();
        let replacement_sources = h.store.sources(&h.session).unwrap();

        let cancel_manager = manager.clone();
        let cancel_session = h.session.clone();
        let cancel = std::thread::spawn(move || cancel_manager.cancel(&cancel_session));
        provider.wait_until_delete_enters();

        let regenerated = manager.regenerate(&h.session).unwrap();
        assert_eq!(regenerated.revision, 2);
        h.store
            .replace_sources(&h.session, &replacement_sources)
            .unwrap();
        manager.tick(&h.session).unwrap();
        let revision_two = h.store.current_run(&h.session).unwrap();
        assert_eq!(revision_two.revision, 2);
        assert_eq!(revision_two.step, PhotoAvatarStep::AnalyzeIdentity);
        assert!(revision_two.provider_session_id.is_some());
        assert!(revision_two.provider_job_id.is_some());

        provider.release_delete();
        let error = cancel.join().unwrap().unwrap_err();

        assert!(error.contains("superseded response"), "{error}");
        let current = h.store.current_run(&h.session).unwrap();
        assert_eq!(current.revision, 2);
        assert_eq!(current.step, PhotoAvatarStep::AnalyzeIdentity);
        assert_eq!(current.generation_token, revision_two.generation_token);
        assert_eq!(
            current.provider_session_id,
            revision_two.provider_session_id
        );
        assert_eq!(current.provider_job_id, revision_two.provider_job_id);
        assert_eq!(h.store.sources(&h.session).unwrap(), replacement_sources);
    }

    #[test]
    fn stale_failed_delete_cannot_mark_new_revision_cleanup_pending() {
        assert_stale_cancel_cleanup_cannot_mutate_new_revision(false);
    }

    #[test]
    fn stale_successful_delete_cannot_complete_new_revision_cleanup() {
        assert_stale_cancel_cleanup_cannot_mutate_new_revision(true);
    }

    #[test]
    fn restart_with_attached_job_polls_without_resubmitting() {
        let h = Harness::new(vec![FakeOutcome::Running]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        h.manager.tick(&h.session).unwrap();
        let restarted = PhotoAvatarManager::new(h.store.clone(), h.provider.clone());
        restarted.tick(&h.session).unwrap();
        restarted.tick(&h.session).unwrap();
        assert_eq!(h.provider.requests().len(), 1);
        assert_eq!(
            restarted.resume_all().unwrap().resumed_session_ids,
            vec![h.session.clone()]
        );
    }

    #[test]
    fn manager_is_the_real_photo_avatar_abandon_port() {
        let h = Harness::new(vec![FakeOutcome::Running]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        h.manager.tick(&h.session).unwrap();
        let port: &dyn PhotoAvatarAbandonPort = &h.manager;
        port.cancel_provider_job(&h.session, "remote-job").unwrap();
        port.delete_provider_session(&h.session, "fake-session-1")
            .unwrap();
        assert_eq!(h.provider.cancellations(), vec!["remote-job"]);
        assert_eq!(h.provider.deleted_sessions(), vec!["fake-session-1"]);
    }

    #[test]
    fn one_session_keeps_only_one_active_generation_token() {
        let h = Harness::new(vec![]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        let run = h.store.current_run(&h.session).unwrap();
        assert!(h
            .manager
            .claim_active_token(&h.session, &run.generation_token)
            .unwrap());
        assert!(!h
            .manager
            .claim_active_token(&h.session, &run.generation_token)
            .unwrap());
        assert!(h
            .manager
            .claim_active_token(&h.session, "replacement-token")
            .unwrap());
        let active = h.manager.active_tokens.lock().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(
            active.get(&h.session).map(String::as_str),
            Some("replacement-token")
        );
    }

    #[test]
    fn reserve_attempt_rejects_terminal_runs() {
        let h = Harness::new(vec![FakeOutcome::Error(PhotoAvatarError {
            code: PhotoAvatarErrorCode::Auth,
            retryable: true,
            message: "fatal".into(),
        })]);
        h.manager.consent(true).unwrap();
        h.manager.begin(&h.session, vec![h.source()]).unwrap();
        h.tick_n(2);
        let run = h.store.current_run(&h.session).unwrap();
        assert_eq!(run.step, PhotoAvatarStep::Failed);
        assert!(h
            .store
            .reserve_attempt(&h.session, run.revision, RemoteStep::AnalyzeIdentity)
            .unwrap_err()
            .contains("not current"));
    }
}
