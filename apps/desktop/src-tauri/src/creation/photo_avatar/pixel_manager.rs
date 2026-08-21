use super::domain::{
    PixelIdentityTraitKey, PixelPhotoAvatarSnapshot, PixelPhotoAvatarStep, PixelRemoteStep,
    PixelStyleProfileId, DEFAULT_PIXEL_STYLE_ID,
};
use super::pixel_remote::{provider_images, run_remote_step, PixelRemoteFailure};
use super::provider::{
    ControlledBackendProvider, PhotoAvatarProvider, PixelProviderStepRequest, ProviderStepResult,
};
use super::source::{normalize_photo_sources, RawPhotoSource};
use super::store::{NormalizedPhoto, PhotoAvatarStore};
use crate::runtime_assets::pixel_avatar_builder::{BuildPixelAvatarRequest, PixelAvatarBuilder};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type SharedPixelPhotoAvatarManager = Arc<PixelPhotoAvatarManager>;

pub struct PixelPhotoAvatarManager {
    pub(super) store: PhotoAvatarStore,
    pub(super) provider: Option<Arc<ControlledBackendProvider>>,
    pub(super) builder: PixelAvatarBuilder,
    pub(super) preview_root: PathBuf,
}

impl PixelPhotoAvatarManager {
    pub fn new(
        store: PhotoAvatarStore,
        provider: Option<Arc<ControlledBackendProvider>>,
        preview_root: &Path,
    ) -> Self {
        Self {
            store,
            provider,
            builder: PixelAvatarBuilder::new(preview_root),
            preview_root: preview_root.to_path_buf(),
        }
    }

    pub fn begin(
        self: &Arc<Self>,
        session_id: &str,
        consent_version: &str,
        sources: Vec<RawPhotoSource>,
    ) -> Result<PixelPhotoAvatarSnapshot, String> {
        if consent_version != super::domain::PHOTO_AVATAR_CONSENT_VERSION
            || !self.store.consent_accepted(consent_version)?
        {
            return Err("photo avatar consent is required".into());
        }
        let normalized = normalize_photo_sources(sources).map_err(|error| error.to_string())?;
        self.store.replace_sources(session_id, &normalized)?;
        self.start_revision(
            session_id,
            DEFAULT_PIXEL_STYLE_ID,
            None,
            Vec::new(),
            normalized,
        )
    }

    pub fn status(&self, session_id: &str) -> Result<Option<PixelPhotoAvatarSnapshot>, String> {
        match self.store.pixel_snapshot(session_id) {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(error) if error == "pixel avatar run does not exist" => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn regenerate(
        self: &Arc<Self>,
        session_id: &str,
    ) -> Result<PixelPhotoAvatarSnapshot, String> {
        let style_profile_id = self.store.pixel_snapshot(session_id)?.style_profile_id;
        self.start_revision(
            session_id,
            style_profile_id,
            None,
            Vec::new(),
            self.store.sources(session_id)?,
        )
    }

    pub fn revise(
        self: &Arc<Self>,
        session_id: &str,
        instruction: &str,
    ) -> Result<PixelPhotoAvatarSnapshot, String> {
        let instruction = instruction.trim();
        if instruction.is_empty() {
            return Err("revision instruction must be non-empty".into());
        }
        let snapshot = self.store.pixel_snapshot(session_id)?;
        let style_profile_id = snapshot.style_profile_id;
        let locked = snapshot
            .profile
            .map(|profile| {
                profile
                    .traits
                    .into_iter()
                    .map(|trait_| trait_.key)
                    .collect()
            })
            .unwrap_or_default();
        self.start_revision(
            session_id,
            style_profile_id,
            Some(instruction),
            locked,
            self.store.sources(session_id)?,
        )
    }

    pub fn cancel(&self, session_id: &str) -> Result<PixelPhotoAvatarSnapshot, String> {
        let snapshot = self.store.pixel_snapshot(session_id)?;
        self.store.set_pixel_step(
            session_id,
            snapshot.revision,
            PixelPhotoAvatarStep::Cancelled,
        )?;
        self.store.pixel_snapshot(session_id)
    }

    fn start_revision(
        self: &Arc<Self>,
        session_id: &str,
        style_profile_id: PixelStyleProfileId,
        modification: Option<&str>,
        locked_traits: Vec<PixelIdentityTraitKey>,
        sources: Vec<NormalizedPhoto>,
    ) -> Result<PixelPhotoAvatarSnapshot, String> {
        let run = self.store.begin_pixel_revision(
            session_id,
            style_profile_id,
            modification,
            &locked_traits,
        )?;
        let manager = Arc::clone(self);
        let session = session_id.to_string();
        let modification = modification.map(str::to_owned);
        std::thread::spawn(move || {
            match manager.run_revision(
                &session,
                run.revision,
                run.style_profile_id,
                modification,
                locked_traits,
                sources,
            ) {
                Ok(()) => {}
                Err(RunRevisionFailure::Remote(error)) => {
                    persist_remote_failure(&manager.store, &session, run.revision, error);
                }
                Err(RunRevisionFailure::Local(message)) => {
                    eprintln!("[pixel-avatar] local failure for {session}: {message}");
                    let _ = manager
                        .store
                        .fail_pixel_revision_if_active(&session, run.revision);
                }
            }
        });
        self.store.pixel_snapshot(session_id)
    }

    fn run_revision(
        &self,
        session_id: &str,
        revision: u32,
        style_profile_id: PixelStyleProfileId,
        modification: Option<String>,
        locked_traits: Vec<PixelIdentityTraitKey>,
        sources: Vec<NormalizedPhoto>,
    ) -> Result<(), RunRevisionFailure> {
        let provider = self
            .provider
            .as_ref()
            .ok_or("photo avatar backend is not configured")?;
        let images = provider_images(&sources);
        let identity_attempt = self.store.reserve_pixel_attempt(
            session_id,
            revision,
            PixelRemoteStep::AnalyzeIdentity,
        )?;
        let (identity_job, identity_state, _identity_attempt) = run_remote_step(
            &self.store,
            provider,
            session_id,
            revision,
            PixelProviderStepRequest {
                route: "pixel-v1".into(),
                style_profile_id,
                session_id: session_id.into(),
                revision,
                provider_session_id: None,
                step: PixelRemoteStep::AnalyzeIdentity,
                attempt: identity_attempt,
                consent_version: super::domain::PHOTO_AVATAR_CONSENT_VERSION.into(),
                source_images: images.clone(),
                profile: None,
                modification: modification.clone(),
                locked_traits: locked_traits.clone(),
            },
        )?;
        let Some(ProviderStepResult::PixelIdentity { partial_profile }) = identity_state.result
        else {
            return Err("pixel identity result is invalid".into());
        };
        self.store
            .commit_pixel_profile(session_id, revision, &partial_profile)?;
        self.store.set_pixel_step(
            session_id,
            revision,
            PixelPhotoAvatarStep::GeneratePixelAvatar,
        )?;
        let generate_initial_attempt = self.store.reserve_pixel_attempt(
            session_id,
            revision,
            PixelRemoteStep::GeneratePixelAvatar,
        )?;
        let (_generate_job, generated, generate_attempt) = run_remote_step(
            &self.store,
            provider,
            session_id,
            revision,
            PixelProviderStepRequest {
                route: "pixel-v1".into(),
                style_profile_id,
                session_id: session_id.into(),
                revision,
                provider_session_id: identity_job.provider_session_id,
                step: PixelRemoteStep::GeneratePixelAvatar,
                attempt: generate_initial_attempt,
                consent_version: super::domain::PHOTO_AVATAR_CONSENT_VERSION.into(),
                source_images: images,
                profile: Some(partial_profile),
                modification,
                locked_traits,
            },
        )?;
        let Some(ProviderStepResult::PixelAvatar {
            artifact_url,
            sha256,
            audit,
            ..
        }) = generated.result
        else {
            return Err("pixel avatar result is invalid".into());
        };
        let png = provider
            .download_artifact(&artifact_url, &sha256)
            .map_err(|error| PixelRemoteFailure {
                code: error.code,
                retryable: error.retryable,
                message: error.message,
            })?;
        self.store
            .commit_pixel_artifact(session_id, revision, "avatar.png", &sha256)?;
        self.store.set_pixel_step(
            session_id,
            revision,
            PixelPhotoAvatarStep::QualityCheckPending,
        )?;
        let pet_id = self.store.pet_id(session_id)?;
        self.builder.build_preview(BuildPixelAvatarRequest {
            session_id: session_id.into(),
            revision,
            attempt: generate_attempt,
            pet_id,
            variant_id: format!("photo-avatar-{session_id}-{revision}"),
            profile: self
                .store
                .pixel_snapshot(session_id)?
                .profile
                .ok_or("pixel avatar profile is missing")?,
            image_png: png,
            image_sha256: sha256,
            audit,
        })?;
        self.store.set_pixel_step(
            session_id,
            revision,
            PixelPhotoAvatarStep::RuntimeCheckPending,
        )?;
        Ok(())
    }
}

enum RunRevisionFailure {
    Remote(PixelRemoteFailure),
    Local(String),
}

impl From<String> for RunRevisionFailure {
    fn from(message: String) -> Self {
        Self::Local(message)
    }
}

impl From<&str> for RunRevisionFailure {
    fn from(message: &str) -> Self {
        Self::Local(message.to_owned())
    }
}

impl From<PixelRemoteFailure> for RunRevisionFailure {
    fn from(failure: PixelRemoteFailure) -> Self {
        Self::Remote(failure)
    }
}

fn persist_remote_failure(
    store: &PhotoAvatarStore,
    session_id: &str,
    revision: u32,
    failure: PixelRemoteFailure,
) -> String {
    let message = failure.message;
    if let Err(error) =
        store.fail_pixel_revision_with_error_if_active(session_id, revision, failure.code, &message)
    {
        eprintln!("[pixel-avatar] failed to persist remote failure for {session_id}: {error}");
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::photo_avatar::domain::PhotoAvatarErrorCode;
    use crate::creation::photo_avatar::pixel_remote::PixelRemoteFailure;
    use crate::storage::Storage;
    use std::sync::Mutex;

    #[test]
    fn remote_failure_is_persisted_for_the_active_pixel_revision() {
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-pixel-manager-{}",
            crate::creation::domain::new_entity_id("failure")
        ));
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        {
            let storage = storage.lock().unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO pets
                     (pet_id, schema_version, species, identity_mode, creation_method,
                      lifecycle, created_at, updated_at)
                     VALUES ('pet-a', 1, 'cat', 'realpet', 'upload', 'draft', '10', '10')",
                    [],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO creation_sessions
                     (session_id, pet_id, method, status, last_stable_status, current_step,
                      schema_version, created_at, updated_at)
                     VALUES ('session-a', 'pet-a', 'upload', 'draft', 'draft', 'upload',
                             1, '10', '10')",
                    [],
                )
                .unwrap();
        }
        let store = PhotoAvatarStore::new(storage);
        let revision = store
            .begin_pixel_revision("session-a", PixelStyleProfileId::V1, None, &[])
            .unwrap()
            .revision;

        let message = persist_remote_failure(
            &store,
            "session-a",
            revision,
            PixelRemoteFailure {
                code: PhotoAvatarErrorCode::InvalidInput,
                retryable: false,
                message: "生成图片不符合像素素材要求，请重试。".into(),
            },
        );

        assert_eq!(message, "生成图片不符合像素素材要求，请重试。");
        let snapshot = store.pixel_snapshot("session-a").unwrap();
        assert_eq!(snapshot.step, PixelPhotoAvatarStep::Failed);
        assert_eq!(
            snapshot.error_code,
            Some(PhotoAvatarErrorCode::InvalidInput)
        );
        assert_eq!(snapshot.error_message.as_deref(), Some(message.as_str()));
        let _ = std::fs::remove_dir_all(root);
    }
}
