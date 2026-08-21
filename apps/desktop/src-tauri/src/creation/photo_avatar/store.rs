use super::domain::{
    parse_appearance_profile_v1, parse_pixel_appearance_profile_v1, AppearanceProfileV1,
    IdentityTraitKey, PhotoAvatarAttemptStep, PhotoAvatarErrorCode, PhotoAvatarSnapshot,
    PhotoAvatarStep, PixelAppearanceProfileV1, PixelIdentityTraitKey, PixelPhotoAvatarSnapshot,
    PixelPhotoAvatarStep, PixelRemoteStep, PixelStyleProfileId, PHOTO_AVATAR_CONSENT_VERSION,
    PHOTO_AVATAR_DISCLOSURE_SHA256,
};
use super::provider::{CleanupState, UpstreamCleanupState};
use crate::runtime_assets::manifest::{
    manifest_files, normalize_relative_path, parse_manifest, RuntimeAssetManifest,
};
use crate::storage::Storage;
use rusqlite::{params, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

pub type SharedPhotoAvatarStore = Arc<Mutex<PhotoAvatarStore>>;
pub(crate) const ACTIVE_ATTEMPT_ERROR: &str = "photo avatar attempt already active";
const LEGACY_PARTIAL_CREATED_AT_PREFIX: &str = "legacy-partial-migration:";

#[cfg(test)]
struct AfterPreviewManifestReadHook {
    manifest_path: std::path::PathBuf,
    callback: Box<dyn FnOnce() + Send>,
}

#[cfg(test)]
static AFTER_PREVIEW_MANIFEST_READ_HOOK: Mutex<Option<AfterPreviewManifestReadHook>> =
    Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedPhoto {
    pub source_id: String,
    pub ordinal: u32,
    pub normalized_png: Vec<u8>,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteStep {
    AnalyzeIdentity,
    CompleteAppearance,
    RenderTextureAtlas,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteJob {
    pub provider_session_id: Option<String>,
    pub provider_job_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderArtifactKind {
    TextureAtlas,
    PreviewPackage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderArtifact {
    pub kind: ProviderArtifactKind,
    pub relative_path: String,
    pub sha256: String,
    pub local_path: Option<String>,
    pub audit_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoAvatarRun {
    pub session_id: String,
    pub revision: u32,
    pub step: PhotoAvatarStep,
    pub generation_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelPhotoAvatarRun {
    pub session_id: String,
    pub revision: u32,
    pub step: PixelPhotoAvatarStep,
    pub generation_token: String,
    pub style_profile_id: PixelStyleProfileId,
}

#[derive(Clone)]
pub struct PhotoAvatarStore {
    storage: Arc<Mutex<Storage>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoAvatarRunState {
    pub session_id: String,
    pub revision: u32,
    pub step: PhotoAvatarStep,
    pub generation_token: String,
    pub provider_session_id: Option<String>,
    pub provider_job_id: Option<String>,
    pub modification: Option<String>,
    pub locked_traits: Vec<IdentityTraitKey>,
    pub current_attempt: Option<u8>,
}

impl PhotoAvatarStore {
    pub fn new(storage: Arc<Mutex<Storage>>) -> Self {
        Self { storage }
    }

    pub fn save_consent(&self, version: &str) -> Result<(), String> {
        if version != PHOTO_AVATAR_CONSENT_VERSION {
            return Err("unsupported photo avatar consent version".into());
        }
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage
            .db
            .execute(
                "INSERT INTO photo_avatar_consents
                 (consent_version, provider_id, disclosure_sha256, accepted_at)
                 VALUES (?1, 'lk888', ?2, ?3)
                 ON CONFLICT(consent_version) DO NOTHING",
                params![version, PHOTO_AVATAR_DISCLOSURE_SHA256, now_iso()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn consent_accepted(&self, version: &str) -> Result<bool, String> {
        if version != PHOTO_AVATAR_CONSENT_VERSION {
            return Err("unsupported photo avatar consent version".into());
        }
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM photo_avatar_consents
                 WHERE consent_version=?1 AND accepted_at IS NOT NULL)",
                [version],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
    }

    pub fn pet_id(&self, session_id: &str) -> Result<String, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage
            .db
            .query_row(
                "SELECT pet_id FROM creation_sessions WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "photo avatar session does not exist".into())
    }

    pub fn record_cleanup_audit(
        &self,
        session_id: &str,
        revision: u32,
        local: CleanupState,
        backend: CleanupState,
        upstream: UpstreamCleanupState,
        provider: &str,
    ) -> Result<(), String> {
        if provider != "lk888" {
            return Err("unsupported photo avatar cleanup provider".into());
        }
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage
            .db
            .execute(
                "INSERT INTO photo_avatar_cleanup_audit
                 (session_id, revision, local_cleanup, backend_cleanup, upstream_cleanup,
                  provider_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(session_id, revision) DO UPDATE SET
                   local_cleanup=CASE
                     WHEN photo_avatar_cleanup_audit.local_cleanup='pending'
                     THEN excluded.local_cleanup
                     ELSE photo_avatar_cleanup_audit.local_cleanup
                   END,
                   backend_cleanup=CASE
                     WHEN photo_avatar_cleanup_audit.backend_cleanup='pending'
                     THEN excluded.backend_cleanup
                     ELSE photo_avatar_cleanup_audit.backend_cleanup
                   END,
                   updated_at=CASE
                     WHEN photo_avatar_cleanup_audit.local_cleanup='pending'
                       OR photo_avatar_cleanup_audit.backend_cleanup='pending'
                     THEN excluded.updated_at
                     ELSE photo_avatar_cleanup_audit.updated_at
                   END",
                params![
                    session_id,
                    revision,
                    cleanup_state_as_str(local),
                    cleanup_state_as_str(backend),
                    upstream_cleanup_state_as_str(upstream),
                    provider,
                    now_iso()
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn replace_sources(
        &self,
        session_id: &str,
        sources: &[NormalizedPhoto],
    ) -> Result<(), String> {
        validate_sources(sources)?;
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM creation_sessions WHERE session_id=?1)",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !exists {
            return Err("photo avatar session does not exist".into());
        }
        tx.execute(
            "DELETE FROM photo_avatar_sources WHERE session_id=?1",
            [session_id],
        )
        .map_err(|error| error.to_string())?;
        let now = now_iso();
        for source in sources {
            tx.execute(
                "INSERT INTO photo_avatar_sources
                 (session_id, source_id, ordinal, normalized_png, sha256, width, height,
                  byte_size, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    session_id,
                    &source.source_id,
                    source.ordinal,
                    &source.normalized_png,
                    &source.sha256,
                    source.width,
                    source.height,
                    source.normalized_png.len() as i64,
                    now,
                ],
            )
            .map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn begin_revision(
        &self,
        session_id: &str,
        modification: Option<&str>,
        locked: &[IdentityTraitKey],
    ) -> Result<PhotoAvatarRun, String> {
        let modification = modification
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let locked_json = serde_json::to_string(locked)
            .map_err(|error| format!("serialize locked trait keys: {error}"))?;
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM creation_sessions WHERE session_id=?1)",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !exists {
            return Err("photo avatar session does not exist".into());
        }
        let previous: Option<i64> = tx
            .query_row(
                "SELECT revision FROM photo_avatar_runs WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let revision = previous.unwrap_or(0) + 1;
        let token: String = tx
            .query_row("SELECT lower(hex(randomblob(32)))", [], |row| row.get(0))
            .map_err(|error| format!("generate photo avatar token: {error}"))?;
        tx.execute(
            "INSERT INTO photo_avatar_runs
             (session_id, revision, step, generation_token, modification_instruction,
              locked_trait_keys_json, updated_at)
             VALUES (?1, ?2, 'analyzeIdentity', ?3, ?4, ?5, ?6)
             ON CONFLICT(session_id) DO UPDATE SET
               revision=excluded.revision,
               step=excluded.step,
               provider_session_id=NULL,
               provider_job_id=NULL,
               generation_token=excluded.generation_token,
               modification_instruction=excluded.modification_instruction,
               locked_trait_keys_json=excluded.locked_trait_keys_json,
               error_code=NULL,
               error_message=NULL,
               updated_at=excluded.updated_at",
            params![
                session_id,
                revision,
                token,
                modification,
                locked_json,
                now_iso()
            ],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(PhotoAvatarRun {
            session_id: session_id.into(),
            revision: revision as u32,
            step: PhotoAvatarStep::AnalyzeIdentity,
            generation_token: token,
        })
    }

    pub fn begin_pixel_revision(
        &self,
        session_id: &str,
        style_profile_id: PixelStyleProfileId,
        modification: Option<&str>,
        locked: &[PixelIdentityTraitKey],
    ) -> Result<PixelPhotoAvatarRun, String> {
        let modification = modification
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let locked_json = serde_json::to_string(locked)
            .map_err(|error| format!("serialize pixel locked trait keys: {error}"))?;
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM creation_sessions WHERE session_id=?1)",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !exists {
            return Err("photo avatar session does not exist".into());
        }
        let previous: Option<i64> = tx
            .query_row(
                "SELECT MAX(revision) FROM photo_avatar_runs WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let revision = previous.unwrap_or(0) + 1;
        let token: String = tx
            .query_row("SELECT lower(hex(randomblob(32)))", [], |row| row.get(0))
            .map_err(|error| format!("generate pixel avatar token: {error}"))?;
        tx.execute(
            "INSERT INTO photo_avatar_runs
             (session_id, revision, route, style_profile_id, step, generation_token,
              modification_instruction, locked_trait_keys_json, updated_at)
             VALUES (?1, ?2, 'pixel-v1', ?3, 'analyzeIdentity', ?4, ?5, ?6, ?7)
             ON CONFLICT(session_id) DO UPDATE SET
               revision=excluded.revision, route=excluded.route,
               style_profile_id=excluded.style_profile_id, step=excluded.step,
               provider_session_id=NULL, provider_job_id=NULL,
               generation_token=excluded.generation_token,
               modification_instruction=excluded.modification_instruction,
               locked_trait_keys_json=excluded.locked_trait_keys_json,
               error_code=NULL, error_message=NULL, updated_at=excluded.updated_at",
            params![
                session_id,
                revision,
                style_profile_id.as_str(),
                token,
                modification,
                locked_json,
                now_iso()
            ],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(PixelPhotoAvatarRun {
            session_id: session_id.into(),
            revision: revision as u32,
            step: PixelPhotoAvatarStep::AnalyzeIdentity,
            generation_token: token,
            style_profile_id,
        })
    }

    pub fn reserve_pixel_attempt(
        &self,
        session_id: &str,
        revision: u32,
        step: PixelRemoteStep,
    ) -> Result<u8, String> {
        let step_name = match step {
            PixelRemoteStep::AnalyzeIdentity => "analyzeIdentity",
            PixelRemoteStep::GeneratePixelAvatar => "generatePixelAvatar",
        };
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let active: Option<String> = tx
            .query_row(
                "SELECT step FROM photo_avatar_runs WHERE session_id=?1 AND revision=?2
                 AND route='pixel-v1'",
                params![session_id, revision],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if active.as_deref() != Some(step_name) {
            return Err("pixel avatar run is not current".into());
        }
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM photo_avatar_step_attempts
                 WHERE session_id=?1 AND revision=?2 AND route='pixel-v1' AND step=?3",
                params![session_id, revision, step_name],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if count >= 3 {
            return Err("pixel avatar remote step already has three attempts".into());
        }
        let attempt = count as u8 + 1;
        tx.execute(
            "INSERT INTO photo_avatar_step_attempts
             (session_id, revision, route, step, attempt_no, status, retryable, started_at)
             VALUES (?1, ?2, 'pixel-v1', ?3, ?4, 'submitted', 1, ?5)",
            params![session_id, revision, step_name, attempt, now_iso()],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(attempt)
    }

    pub fn commit_pixel_profile(
        &self,
        session_id: &str,
        revision: u32,
        profile: &PixelAppearanceProfileV1,
    ) -> Result<(), String> {
        let profile_json = serde_json::to_string(profile)
            .map_err(|error| format!("serialize pixel profile: {error}"))?;
        let normalized = parse_pixel_appearance_profile_v1(&profile_json)?;
        let canonical_json = serde_json::to_string(&normalized)
            .map_err(|error| format!("serialize normalized pixel profile: {error}"))?;
        let profile_sha256 = sha256_hex(canonical_json.as_bytes());
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage
            .db
            .execute(
                "INSERT INTO photo_avatar_profiles
                 (session_id, revision, route, profile_kind, schema_version, body_module_id,
                  profile_json, profile_sha256, created_at)
                 VALUES (?1, ?2, 'pixel-v1', 'pixel-v1', 1, NULL, ?3, ?4, ?5)
                 ON CONFLICT(session_id, revision, route) DO UPDATE SET
                   profile_json=excluded.profile_json, profile_sha256=excluded.profile_sha256,
                   created_at=excluded.created_at",
                params![
                    session_id,
                    revision,
                    canonical_json,
                    profile_sha256,
                    now_iso()
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn commit_pixel_artifact(
        &self,
        session_id: &str,
        revision: u32,
        relative_path: &str,
        sha256: &str,
    ) -> Result<(), String> {
        if relative_path.trim().is_empty() || !is_lower_hex(sha256) {
            return Err("invalid pixel avatar artifact".into());
        }
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage
            .db
            .execute(
                "INSERT INTO photo_avatar_artifacts
                 (session_id, revision, route, kind, relative_path, sha256, created_at)
                 VALUES (?1, ?2, 'pixel-v1', 'pixelAvatar', ?3, ?4, ?5)
                 ON CONFLICT(session_id, revision, route, kind) DO UPDATE SET
                   relative_path=excluded.relative_path, sha256=excluded.sha256,
                   created_at=excluded.created_at",
                params![session_id, revision, relative_path, sha256, now_iso()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn set_pixel_provider_job(
        &self,
        session_id: &str,
        revision: u32,
        provider_session_id: Option<&str>,
        provider_job_id: Option<&str>,
    ) -> Result<(), String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let updated = storage
            .db
            .execute(
                "UPDATE photo_avatar_runs
                 SET provider_session_id=COALESCE(?3, provider_session_id), provider_job_id=?4,
                     updated_at=?5
                 WHERE session_id=?1 AND revision=?2 AND route='pixel-v1'",
                params![
                    session_id,
                    revision,
                    provider_session_id,
                    provider_job_id,
                    now_iso()
                ],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("pixel avatar run is not current".into());
        }
        Ok(())
    }

    pub fn set_pixel_step(
        &self,
        session_id: &str,
        revision: u32,
        step: PixelPhotoAvatarStep,
    ) -> Result<(), String> {
        let step_name = pixel_step_as_str(step);
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let updated = storage
            .db
            .execute(
                "UPDATE photo_avatar_runs SET step=?3, provider_job_id=NULL, updated_at=?4
                 WHERE session_id=?1 AND revision=?2 AND route='pixel-v1'",
                params![session_id, revision, step_name, now_iso()],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("pixel avatar run is not current".into());
        }
        Ok(())
    }

    pub fn fail_pixel_revision_if_active(
        &self,
        session_id: &str,
        revision: u32,
    ) -> Result<bool, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let updated = storage
            .db
            .execute(
                "UPDATE photo_avatar_runs SET step='failed', provider_job_id=NULL, updated_at=?3
                 WHERE session_id=?1 AND revision=?2 AND route='pixel-v1'
                   AND step NOT IN ('cancelled','completed','failed')",
                params![session_id, revision, now_iso()],
            )
            .map_err(|error| error.to_string())?;
        Ok(updated == 1)
    }

    pub fn fail_pixel_revision_with_error_if_active(
        &self,
        session_id: &str,
        revision: u32,
        code: PhotoAvatarErrorCode,
        message: &str,
    ) -> Result<bool, String> {
        let message = message.trim();
        if message.is_empty() || message.len() > 256 || message.contains(['\r', '\n']) {
            return Err("pixel avatar failure message is invalid".into());
        }
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let updated = storage
            .db
            .execute(
                "UPDATE photo_avatar_runs
                 SET step='failed', provider_job_id=NULL, error_code=?3, error_message=?4,
                     updated_at=?5
                 WHERE session_id=?1 AND revision=?2 AND route='pixel-v1'
                   AND step NOT IN ('cancelled','completed','failed')",
                params![
                    session_id,
                    revision,
                    error_code_as_str(code),
                    message,
                    now_iso()
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(updated == 1)
    }

    pub fn pixel_snapshot(&self, session_id: &str) -> Result<PixelPhotoAvatarSnapshot, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let row: (u32, String, String, Option<String>, Option<String>, Option<String>) = storage
            .db
            .query_row(
                "SELECT revision, step, style_profile_id, provider_job_id, error_code, error_message
                 FROM photo_avatar_runs WHERE session_id=?1 AND route='pixel-v1'",
                [session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or("pixel avatar run does not exist")?;
        let profile_json: Option<String> = storage
            .db
            .query_row(
                "SELECT profile_json FROM photo_avatar_profiles
                 WHERE session_id=?1 AND revision=?2 AND route='pixel-v1'",
                params![session_id, row.0],
                |profile| profile.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let profile = profile_json
            .as_deref()
            .map(parse_pixel_appearance_profile_v1)
            .transpose()?;
        let mut attempts = std::collections::BTreeMap::new();
        let mut statement = storage
            .db
            .prepare(
                "SELECT step, MAX(attempt_no) FROM photo_avatar_step_attempts
                 WHERE session_id=?1 AND revision=?2 AND route='pixel-v1' GROUP BY step",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![session_id, row.0], |attempt| {
                Ok((attempt.get::<_, String>(0)?, attempt.get::<_, u32>(1)?))
            })
            .map_err(|error| error.to_string())?;
        for attempt in rows {
            let (step, count) = attempt.map_err(|error| error.to_string())?;
            attempts.insert(pixel_remote_step_from_db(&step)?, count);
        }
        Ok(PixelPhotoAvatarSnapshot {
            route: "pixel-v1".into(),
            style_profile_id: PixelStyleProfileId::parse(&row.2)?,
            session_id: session_id.into(),
            revision: row.0,
            step: pixel_step_from_db(&row.1)?,
            provider_job_id: row.3,
            profile,
            attempts,
            error_code: row
                .4
                .as_deref()
                .map(photo_avatar_error_code_from_db)
                .transpose()?,
            error_message: row.5,
        })
    }

    pub fn reserve_attempt(
        &self,
        session_id: &str,
        revision: u32,
        step: RemoteStep,
    ) -> Result<u8, String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let active: Option<String> = tx
            .query_row(
                "SELECT step FROM photo_avatar_runs WHERE session_id=?1 AND revision=?2",
                params![session_id, revision],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if active.as_deref() != Some(step.as_str()) {
            return Err("photo avatar run is not current".into());
        }
        let has_active_attempt: bool = tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM photo_avatar_step_attempts
                    WHERE session_id=?1 AND revision=?2 AND step=?3
                      AND status IN ('submitted', 'running')
                 )",
                params![session_id, revision, step.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if has_active_attempt {
            return Err(ACTIVE_ATTEMPT_ERROR.into());
        }
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM photo_avatar_step_attempts
                 WHERE session_id=?1 AND revision=?2 AND step=?3",
                params![session_id, revision, step.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if count >= 3 {
            return Err("photo avatar remote step already has three attempts".into());
        }
        let attempt = count as u8 + 1;
        tx.execute(
            "INSERT INTO photo_avatar_step_attempts
             (session_id, revision, route, step, attempt_no, status, retryable, started_at)
             VALUES (?1, ?2, 'live2d-v5', ?3, ?4, 'submitted', 1, ?5)",
            params![session_id, revision, step.as_str(), attempt, now_iso()],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "UPDATE photo_avatar_runs SET step=?3, updated_at=?4
             WHERE session_id=?1 AND revision=?2 AND step!='cancelled'",
            params![session_id, revision, step.as_str(), now_iso()],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(attempt)
    }

    pub fn attach_job(
        &self,
        token: &str,
        step: RemoteStep,
        attempt: u8,
        job: &RemoteJob,
    ) -> Result<(), String> {
        if attempt == 0 || attempt > 3 || job.provider_job_id.trim().is_empty() {
            return Err("invalid provider job attachment".into());
        }
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let (session_id, revision) = current_run_for_token(&tx, token)?;
        let attached = tx
            .execute(
                "UPDATE photo_avatar_step_attempts
                 SET provider_job_id=?5, status='running'
                 WHERE session_id=?1 AND revision=?2 AND step=?3 AND attempt_no=?4
                   AND status='submitted'
                   AND EXISTS(
                     SELECT 1 FROM photo_avatar_runs
                     WHERE session_id=?1 AND revision=?2 AND generation_token=?6
                       AND step!='cancelled'
                   )",
                params![
                    session_id,
                    revision,
                    step.as_str(),
                    attempt,
                    job.provider_job_id,
                    token,
                ],
            )
            .map_err(|error| error.to_string())?;
        if attached != 1 {
            return Err("superseded response".into());
        }
        let updated = tx
            .execute(
                "UPDATE photo_avatar_runs
                 SET provider_session_id=?2, provider_job_id=?3, updated_at=?4
                 WHERE session_id=?1 AND revision=?5 AND generation_token=?6
                   AND step!='cancelled'",
                params![
                    session_id,
                    job.provider_session_id,
                    job.provider_job_id,
                    now_iso(),
                    revision,
                    token,
                ],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("superseded response".into());
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn commit_profile(&self, token: &str, profile: &AppearanceProfileV1) -> Result<(), String> {
        let (step, attempt, provider_job_id) = self.legacy_running_attempt(token)?;
        self.commit_profile_for_attempt(token, step, attempt, &provider_job_id, profile)
    }

    pub fn commit_profile_for_attempt(
        &self,
        token: &str,
        step: RemoteStep,
        attempt: u8,
        provider_job_id: &str,
        profile: &AppearanceProfileV1,
    ) -> Result<(), String> {
        if attempt == 0 || attempt > 3 || provider_job_id.trim().is_empty() {
            return Err("invalid provider attempt result".into());
        }
        let profile_json = serde_json::to_string(profile)
            .map_err(|error| format!("serialize appearance profile: {error}"))?;
        let profile = parse_appearance_profile_v1(&profile_json)?;
        let profile = canonical_profile(profile);
        let profile_json = serde_json::to_string(&profile)
            .map_err(|error| format!("serialize normalized appearance profile: {error}"))?;
        let profile_sha256 = sha256_hex(profile_json.as_bytes());
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let (session_id, revision) = current_run_for_token(&tx, token)?;
        ensure_attempt_is_current(
            &tx,
            token,
            &session_id,
            revision,
            step,
            attempt,
            provider_job_id,
        )?;
        let stored_revision = match step {
            RemoteStep::AnalyzeIdentity => -i64::from(revision),
            RemoteStep::CompleteAppearance => i64::from(revision),
            RemoteStep::RenderTextureAtlas => {
                return Err("profile result is not valid for renderTextureAtlas".into())
            }
        };
        let existing_hash: Option<String> = tx
            .query_row(
                "SELECT profile_sha256 FROM photo_avatar_profiles
                 WHERE session_id=?1 AND revision=?2",
                params![session_id, stored_revision],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let stored = match existing_hash.as_deref() {
            Some(existing) if existing == profile_sha256 => 1,
            Some(_) => return Err("profile revision conflict".into()),
            None => tx
                .execute(
                    "INSERT INTO photo_avatar_profiles
                     (session_id, revision, route, profile_kind, schema_version, body_module_id,
                      profile_json, profile_sha256, created_at)
                     VALUES (?1, ?2, 'live2d-v5', 'live2d-v5', ?3, ?4, ?5, ?6, ?7)",
                    params![
                        session_id,
                        stored_revision,
                        profile.schema_version,
                        profile.body_module_id,
                        profile_json,
                        profile_sha256,
                        now_iso(),
                    ],
                )
                .map_err(|error| error.to_string())?,
        };
        if stored != 1 {
            return Err("superseded response".into());
        }
        let next_step = match step {
            RemoteStep::AnalyzeIdentity => "completeAppearance",
            RemoteStep::CompleteAppearance => "renderTextureAtlas",
            RemoteStep::RenderTextureAtlas => unreachable!(),
        };
        ensure_current_update(
            &tx,
            "UPDATE photo_avatar_runs
             SET step=?1, provider_job_id=NULL, error_code=NULL, error_message=NULL, updated_at=?2
             WHERE session_id=?3 AND revision=?4 AND generation_token=?5 AND step=?6",
            params![
                next_step,
                now_iso(),
                session_id,
                revision,
                token,
                step.as_str()
            ],
        )?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn commit_artifact(&self, token: &str, artifact: &ProviderArtifact) -> Result<(), String> {
        let (step, attempt, provider_job_id) = self.legacy_running_attempt(token)?;
        self.commit_artifact_for_attempt(token, step, attempt, &provider_job_id, artifact)
    }

    pub fn commit_artifact_for_attempt(
        &self,
        token: &str,
        step: RemoteStep,
        attempt: u8,
        provider_job_id: &str,
        artifact: &ProviderArtifact,
    ) -> Result<(), String> {
        if artifact.relative_path.trim().is_empty() || !is_lower_hex(&artifact.sha256) {
            return Err("invalid provider artifact".into());
        }
        if artifact.kind == ProviderArtifactKind::TextureAtlas
            && artifact
                .audit_json
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err("texture artifact requires canonical audit".into());
        }
        if attempt == 0 || attempt > 3 || provider_job_id.trim().is_empty() {
            return Err("invalid provider attempt result".into());
        }
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let (session_id, revision) = current_run_for_token(&tx, token)?;
        ensure_attempt_is_current(
            &tx,
            token,
            &session_id,
            revision,
            step,
            attempt,
            provider_job_id,
        )?;
        let stored = tx
            .execute(
                "INSERT INTO photo_avatar_artifacts
             (session_id, revision, route, kind, relative_path, sha256, local_path, audit_json, created_at)
             SELECT ?1, ?2, 'live2d-v5', ?3, ?4, ?5, ?6, ?7, ?8
             WHERE EXISTS(
               SELECT 1 FROM photo_avatar_runs
               WHERE session_id=?1 AND revision=?2 AND generation_token=?9 AND step=?10
             )
             ON CONFLICT(session_id, revision, route, kind) DO UPDATE SET
               relative_path=excluded.relative_path,
               sha256=excluded.sha256,
               local_path=excluded.local_path,
               audit_json=excluded.audit_json,
               created_at=excluded.created_at",
                params![
                    session_id,
                    revision,
                    artifact.kind.as_str(),
                    artifact.relative_path,
                    artifact.sha256,
                    artifact.local_path,
                    artifact.audit_json,
                    now_iso(),
                    token,
                    step.as_str(),
                ],
            )
            .map_err(|error| error.to_string())?;
        if stored != 1 {
            return Err("superseded response".into());
        }
        let next_step = match artifact.kind {
            ProviderArtifactKind::TextureAtlas => "buildV5",
            ProviderArtifactKind::PreviewPackage => "previewReady",
        };
        ensure_current_update(
            &tx,
            "UPDATE photo_avatar_runs
             SET step=?1, provider_job_id=NULL, error_code=NULL, error_message=NULL, updated_at=?2
             WHERE session_id=?3 AND revision=?4 AND generation_token=?5 AND step=?6",
            params![
                next_step,
                now_iso(),
                session_id,
                revision,
                token,
                step.as_str()
            ],
        )?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn texture_artifact(
        &self,
        session_id: &str,
        revision: u32,
    ) -> Result<Option<ProviderArtifact>, String> {
        self.artifact(session_id, revision, ProviderArtifactKind::TextureAtlas)
    }

    fn artifact(
        &self,
        session_id: &str,
        revision: u32,
        kind: ProviderArtifactKind,
    ) -> Result<Option<ProviderArtifact>, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage
            .db
            .query_row(
                "SELECT relative_path, sha256, local_path, audit_json FROM photo_avatar_artifacts
                 WHERE session_id=?1 AND revision=?2 AND kind=?3",
                params![session_id, revision, kind.as_str()],
                |row| {
                    Ok(ProviderArtifact {
                        kind,
                        relative_path: row.get(0)?,
                        sha256: row.get(1)?,
                        local_path: row.get(2)?,
                        audit_json: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn commit_preview_package(
        &self,
        session_id: &str,
        revision: u32,
        generation_token: &str,
        preview_dir: &std::path::Path,
        manifest_sha256: &str,
    ) -> Result<(), String> {
        if !is_lower_hex(manifest_sha256) || !preview_dir.is_absolute() {
            return Err("invalid preview package".into());
        }
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let inserted = tx
            .execute(
                "INSERT INTO photo_avatar_artifacts
                 (session_id, revision, route, kind, relative_path, sha256, local_path, created_at)
                 SELECT ?1, ?2, 'live2d-v5', 'previewPackage', 'manifest.json', ?3, ?4, ?5
                 WHERE EXISTS(
                   SELECT 1 FROM photo_avatar_runs
                   WHERE session_id=?1 AND revision=?2 AND route='live2d-v5'
                     AND generation_token=?6 AND step='buildV5'
                 )
                 ON CONFLICT(session_id, revision, route, kind) DO UPDATE SET
                   relative_path=excluded.relative_path, sha256=excluded.sha256,
                   local_path=excluded.local_path, created_at=excluded.created_at",
                params![
                    session_id,
                    revision,
                    manifest_sha256,
                    preview_dir.to_string_lossy(),
                    now_iso(),
                    generation_token
                ],
            )
            .map_err(|error| error.to_string())?;
        if inserted != 1 {
            return Err("superseded response".into());
        }
        let updated = tx
            .execute(
                "UPDATE photo_avatar_runs SET step='runtimeCheckPending', updated_at=?1
                 WHERE session_id=?2 AND revision=?3 AND generation_token=?4 AND step='buildV5'",
                params![now_iso(), session_id, revision, generation_token],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("superseded response".into());
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn runtime_check_passed(
        &self,
        session_id: &str,
        revision: u32,
        manifest_sha256: &str,
    ) -> Result<(), String> {
        if !is_lower_hex(manifest_sha256) {
            return Err("manifestSha256 must be a lowercase SHA-256".into());
        }
        let artifact = self
            .artifact(session_id, revision, ProviderArtifactKind::PreviewPackage)?
            .ok_or("photo avatar preview package is not available")?;
        if artifact.sha256 != manifest_sha256 {
            return Err("runtime check CAS did not match the current preview".into());
        }
        let root = preview_root(&artifact)?;
        crate::platform::with_regular_file_no_reparse(
            &root,
            &root.join("manifest.json"),
            |manifest_bytes| {
                if sha256_hex(manifest_bytes) != artifact.sha256 {
                    return Err("preview manifest hash does not match the recorded package".into());
                }
                let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
                let updated = storage
                    .db
                    .execute(
                        "UPDATE photo_avatar_runs SET step='previewReady', updated_at=?1
                         WHERE session_id=?2 AND revision=?3 AND step='runtimeCheckPending'
                           AND EXISTS(
                             SELECT 1 FROM photo_avatar_artifacts
                             WHERE session_id=?2 AND revision=?3 AND kind='previewPackage'
                               AND sha256=?4
                           )",
                        params![now_iso(), session_id, revision, manifest_sha256],
                    )
                    .map_err(|error| error.to_string())?;
                if updated == 1 {
                    Ok(())
                } else {
                    Err("runtime check CAS did not match the current preview".into())
                }
            },
        )
    }

    pub fn preview_manifest(
        &self,
        session_id: &str,
        revision: u32,
    ) -> Result<serde_json::Value, String> {
        let artifact = self
            .artifact(session_id, revision, ProviderArtifactKind::PreviewPackage)?
            .ok_or("photo avatar preview package is not available")?;
        let bytes = read_preview_manifest(&artifact)?;
        let manifest = parse_manifest(
            std::str::from_utf8(&bytes).map_err(|_| "preview manifest is not UTF-8")?,
        )?;
        if !matches!(manifest, RuntimeAssetManifest::V5(_)) {
            return Err("photo avatar preview must use RuntimeAssetManifestV5".into());
        }
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }

    pub fn preview_file_bytes(
        &self,
        session_id: &str,
        revision: u32,
        relative_path: &str,
    ) -> Result<Vec<u8>, String> {
        self.preview_file(session_id, revision, relative_path, true)
    }

    fn preview_file(
        &self,
        session_id: &str,
        revision: u32,
        relative_path: &str,
        require_manifest_entry: bool,
    ) -> Result<Vec<u8>, String> {
        let artifact = self
            .artifact(session_id, revision, ProviderArtifactKind::PreviewPackage)?
            .ok_or("photo avatar preview package is not available")?;
        let root = preview_root(&artifact)?;
        let normalized = normalize_relative_path(relative_path)?;
        let manifest_bytes = read_preview_manifest(&artifact)?;
        let manifest = parse_manifest(
            std::str::from_utf8(&manifest_bytes).map_err(|_| "preview manifest is not UTF-8")?,
        )?;
        let RuntimeAssetManifest::V5(manifest) = manifest else {
            return Err("photo avatar preview must use RuntimeAssetManifestV5".into());
        };
        let expected = if normalized == "manifest.json" && !require_manifest_entry {
            None
        } else {
            Some(
                manifest_files(&RuntimeAssetManifest::V5(manifest.clone()))
                    .iter()
                    .find(|entry| entry.relative_path == normalized)
                    .ok_or("preview file is not declared by manifest")?
                    .sha256
                    .clone(),
            )
        };
        let path = normalized
            .split('/')
            .fold(root.clone(), |mut path, component| {
                path.push(component);
                path
            });
        let bytes = crate::platform::read_regular_file_no_reparse(&root, &path)?;
        if let Some(expected) = expected {
            if sha256_hex(&bytes) != expected.to_ascii_lowercase() {
                return Err("preview file hash does not match manifest".into());
            }
        }
        Ok(bytes)
    }

    fn legacy_running_attempt(&self, token: &str) -> Result<(RemoteStep, u8, String), String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let (session_id, revision, step): (String, u32, String) = storage
            .db
            .query_row(
                "SELECT session_id, revision, step FROM photo_avatar_runs
                 WHERE generation_token=?1 AND step NOT IN ('cancelled', 'completed', 'failed')",
                [token],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or("superseded response")?;
        let step = remote_step_from_db(&step)?;
        let count: i64 = storage
            .db
            .query_row(
                "SELECT COUNT(*) FROM photo_avatar_step_attempts
                 WHERE session_id=?1 AND revision=?2 AND step=?3",
                params![session_id, revision, step.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if count != 1 {
            return Err("attempt identity required".into());
        }
        storage
            .db
            .query_row(
                "SELECT attempt_no, provider_job_id FROM photo_avatar_step_attempts
                 WHERE session_id=?1 AND revision=?2 AND step=?3 AND status='running'
                   AND provider_job_id IS NOT NULL",
                params![session_id, revision, step.as_str()],
                |row| Ok((step, row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "attempt identity required".into())
    }

    pub fn cancel(
        &self,
        session_id: &str,
        revision: u32,
        generation_token: &str,
    ) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let cancelled = tx
            .execute(
                "UPDATE photo_avatar_runs SET step='cancelled', updated_at=?4
                 WHERE session_id=?1 AND revision=?2 AND generation_token=?3 AND step!='cancelled'",
                params![session_id, revision, generation_token, now_iso()],
            )
            .map_err(|error| error.to_string())?;
        if cancelled != 1 {
            return Err("photo avatar run is not current".into());
        }
        tx.execute(
            "UPDATE photo_avatar_step_attempts
             SET status='cancelled', finished_at=?3
             WHERE session_id=?1 AND revision=?2 AND status IN ('submitted', 'running')",
            params![session_id, revision, now_iso()],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "DELETE FROM photo_avatar_sources
             WHERE session_id=?1 AND EXISTS(
                 SELECT 1 FROM photo_avatar_runs
                 WHERE session_id=?1 AND revision=?2 AND generation_token=?3 AND step='cancelled'
             )",
            params![session_id, revision, generation_token],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn snapshot(&self, session_id: &str) -> Result<PhotoAvatarSnapshot, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        snapshot_with_storage(&storage, session_id)?
            .ok_or_else(|| "photo avatar run does not exist".into())
    }

    pub fn snapshot_if_exists(
        &self,
        session_id: &str,
    ) -> Result<Option<PhotoAvatarSnapshot>, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        snapshot_with_storage(&storage, session_id)
    }

    pub fn run_exists(&self, session_id: &str) -> Result<bool, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM photo_avatar_runs WHERE session_id=?1)",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
    }

    pub fn current_run(&self, session_id: &str) -> Result<PhotoAvatarRunState, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let row: (
            u32,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = storage
            .db
            .query_row(
                "SELECT revision, step, generation_token, provider_session_id, provider_job_id,
                        modification_instruction, locked_trait_keys_json
                 FROM photo_avatar_runs WHERE session_id=?1",
                [session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or("photo avatar run does not exist")?;
        let current_attempt = if matches!(
            row.1.as_str(),
            "analyzeIdentity" | "completeAppearance" | "renderTextureAtlas"
        ) {
            storage.db.query_row(
                "SELECT attempt_no FROM photo_avatar_step_attempts
                 WHERE session_id=?1 AND revision=?2 AND step=?3 AND status IN ('submitted','running')
                 ORDER BY attempt_no DESC LIMIT 1",
                params![session_id, row.0, row.1],
                |attempt| attempt.get(0),
            ).optional().map_err(|error| error.to_string())?
        } else {
            None
        };
        let locked_traits = row
            .6
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| format!("invalid stored locked trait keys: {error}"))?
            .unwrap_or_default();
        Ok(PhotoAvatarRunState {
            session_id: session_id.into(),
            revision: row.0,
            step: photo_avatar_step_from_db(&row.1)?,
            generation_token: row.2,
            provider_session_id: row.3,
            provider_job_id: row.4,
            modification: row.5,
            locked_traits,
            current_attempt,
        })
    }

    pub fn sources(&self, session_id: &str) -> Result<Vec<NormalizedPhoto>, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let mut statement = storage
            .db
            .prepare(
                "SELECT source_id, ordinal, normalized_png, sha256, width, height
             FROM photo_avatar_sources WHERE session_id=?1 ORDER BY ordinal",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([session_id], |row| {
                Ok(NormalizedPhoto {
                    source_id: row.get(0)?,
                    ordinal: row.get(1)?,
                    normalized_png: row.get(2)?,
                    sha256: row.get(3)?,
                    width: row.get(4)?,
                    height: row.get(5)?,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(rows)
    }

    pub fn previous_profile(
        &self,
        session_id: &str,
        revision: u32,
    ) -> Result<Option<AppearanceProfileV1>, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let json: Option<String> = storage
            .db
            .query_row(
                "SELECT profile_json FROM photo_avatar_profiles
             WHERE session_id=?1 AND revision >= 1 AND revision < ?2
             ORDER BY revision DESC LIMIT 1",
                params![session_id, revision],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        json.as_deref().map(parse_appearance_profile_v1).transpose()
    }

    pub fn partial_profile(
        &self,
        session_id: &str,
        revision: u32,
    ) -> Result<Option<AppearanceProfileV1>, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let json: Option<String> = storage
            .db
            .query_row(
                "SELECT profile_json FROM photo_avatar_profiles
                 WHERE session_id=?1 AND revision=?2",
                params![session_id, -i64::from(revision)],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some(profile) = json
            .as_deref()
            .map(parse_appearance_profile_v1)
            .transpose()?
        {
            return Ok(Some(profile));
        }
        let json: Option<String> = storage
            .db
            .query_row(
                "SELECT p.profile_json
                 FROM photo_avatar_profiles p
                 JOIN photo_avatar_runs r ON r.session_id=p.session_id AND r.revision=p.revision
                 WHERE p.session_id=?1 AND p.revision=?2 AND r.step='completeAppearance'
                   AND NOT EXISTS (
                       SELECT 1 FROM photo_avatar_profiles
                       WHERE session_id=?1 AND revision=?3
                   )",
                params![session_id, revision, -i64::from(revision)],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        json.as_deref().map(parse_appearance_profile_v1).transpose()
    }

    pub fn legacy_partial_profile(
        &self,
        session_id: &str,
        revision: u32,
    ) -> Result<Option<AppearanceProfileV1>, String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let negative_revision = -i64::from(revision);
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT profile_json, created_at FROM photo_avatar_profiles
                 WHERE session_id=?1 AND revision=?2",
                params![session_id, negative_revision],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some((json, created_at)) = existing {
            tx.commit().map_err(|error| error.to_string())?;
            return if created_at.starts_with(LEGACY_PARTIAL_CREATED_AT_PREFIX) {
                parse_appearance_profile_v1(&json).map(Some)
            } else {
                Ok(None)
            };
        }

        let legacy: Option<String> = tx
            .query_row(
                "SELECT p.profile_json
                 FROM photo_avatar_profiles p
                 JOIN photo_avatar_runs r ON r.session_id=p.session_id AND r.revision=p.revision
                WHERE p.session_id=?1 AND p.revision=?2 AND r.step='completeAppearance'",
                params![session_id, revision],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some(json) = legacy else {
            tx.commit().map_err(|error| error.to_string())?;
            return Ok(None);
        };
        let moved = tx
            .execute(
                "UPDATE photo_avatar_profiles
                 SET revision=?1, created_at=?2
                 WHERE session_id=?3 AND revision=?4
                   AND NOT EXISTS (
                       SELECT 1 FROM photo_avatar_profiles
                       WHERE session_id=?3 AND revision=?1
                   )
                   AND EXISTS (
                       SELECT 1 FROM photo_avatar_runs
                       WHERE session_id=?3 AND revision=?4 AND step='completeAppearance'
                   )",
                params![
                    negative_revision,
                    format!("{LEGACY_PARTIAL_CREATED_AT_PREFIX}{}", now_iso()),
                    session_id,
                    revision,
                ],
            )
            .map_err(|error| error.to_string())?;
        if moved != 1 {
            return Err("superseded response".into());
        }
        tx.commit().map_err(|error| error.to_string())?;
        parse_appearance_profile_v1(&json).map(Some)
    }

    pub fn record_poll_error(
        &self,
        run: &PhotoAvatarRunState,
        code: PhotoAvatarErrorCode,
        message: &str,
    ) -> Result<(), String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let changed = storage
            .db
            .execute(
                "UPDATE photo_avatar_runs SET error_code=?1, error_message=?2, updated_at=?3
             WHERE session_id=?4 AND revision=?5 AND generation_token=?6 AND provider_job_id=?7",
                params![
                    error_code_as_str(code),
                    message,
                    now_iso(),
                    run.session_id,
                    run.revision,
                    run.generation_token,
                    run.provider_job_id
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed == 1 {
            Ok(())
        } else {
            Err("superseded response".into())
        }
    }

    pub fn fail_attempt(
        &self,
        run: &PhotoAvatarRunState,
        code: PhotoAvatarErrorCode,
        message: &str,
        retryable: bool,
    ) -> Result<(), String> {
        let attempt = run.current_attempt.ok_or("attempt identity required")?;
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let changed = tx.execute(
            "UPDATE photo_avatar_step_attempts SET status='failed', retryable=?1, error_code=?2, finished_at=?3
             WHERE session_id=?4 AND revision=?5 AND step=?6 AND attempt_no=?7 AND status IN ('submitted','running')",
            params![retryable, error_code_as_str(code), now_iso(), run.session_id, run.revision, remote_step_for_avatar_step(run.step)?.as_str(), attempt],
        ).map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err("superseded response".into());
        }
        let next_step = if retryable && attempt < 3 {
            remote_step_for_avatar_step(run.step)?.as_str()
        } else {
            "failed"
        };
        ensure_current_update(&tx,
            "UPDATE photo_avatar_runs SET step=?1, provider_job_id=NULL, error_code=?2, error_message=?3, updated_at=?4
             WHERE session_id=?5 AND revision=?6 AND generation_token=?7 AND step=?8",
            params![next_step, error_code_as_str(code), message, now_iso(), run.session_id, run.revision, run.generation_token, remote_step_for_avatar_step(run.step)?.as_str()])?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn resumable_session_ids(&self) -> Result<Vec<String>, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let mut statement = storage.db.prepare(
            "SELECT session_id FROM photo_avatar_runs
             WHERE route='live2d-v5'
               AND step IN ('analyzeIdentity','completeAppearance','renderTextureAtlas','buildV5','cleanupPending')
             ORDER BY session_id",
        ).map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| row.get(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(rows)
    }

    pub fn delete_sources(&self, session_id: &str) -> Result<usize, String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let deleted = tx
            .execute(
                "DELETE FROM photo_avatar_sources WHERE session_id=?1",
                [session_id],
            )
            .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(deleted)
    }

    pub fn delete_sources_for_terminal_run(
        &self,
        run: &PhotoAvatarRunState,
    ) -> Result<usize, String> {
        let terminal_step = match run.step {
            PhotoAvatarStep::Completed => "completed",
            PhotoAvatarStep::Failed => "failed",
            _ => return Err("photo avatar run is not terminal".into()),
        };
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let current: bool = tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM photo_avatar_runs
                    WHERE session_id=?1 AND revision=?2 AND generation_token=?3
                      AND step=?4 AND step IN ('completed', 'failed')
                 )",
                params![
                    run.session_id,
                    run.revision,
                    run.generation_token,
                    terminal_step
                ],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !current {
            return Err("superseded response".into());
        }
        let deleted = tx
            .execute(
                "DELETE FROM photo_avatar_sources
                 WHERE session_id=?1 AND EXISTS(
                     SELECT 1 FROM photo_avatar_runs
                     WHERE session_id=?1 AND revision=?2 AND generation_token=?3
                       AND step=?4 AND step IN ('completed', 'failed')
                 )",
                params![
                    run.session_id,
                    run.revision,
                    run.generation_token,
                    terminal_step
                ],
            )
            .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(deleted)
    }

    pub fn cleanup_terminal_photo_avatar_sources(&self) -> Result<Vec<String>, String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let mut statement = tx
            .prepare(
                "SELECT DISTINCT source.session_id
                 FROM photo_avatar_sources source
                 JOIN creation_sessions session ON session.session_id=source.session_id
                 LEFT JOIN photo_avatar_runs run ON run.session_id=source.session_id
                 WHERE session.status IN ('completed', 'abandoned')
                    OR run.step IN ('completed', 'cancelled')
                 ORDER BY source.session_id",
            )
            .map_err(|error| error.to_string())?;
        let session_ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        drop(statement);
        for session_id in &session_ids {
            tx.execute(
                "DELETE FROM photo_avatar_sources WHERE session_id=?1",
                [session_id],
            )
            .map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(session_ids)
    }

    pub fn mark_cleanup_pending_and_delete_local_data(
        &self,
        session_id: &str,
    ) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let updated = tx
            .execute(
                "UPDATE photo_avatar_runs
                 SET step='cleanupPending', provider_job_id=NULL, updated_at=?2
                 WHERE session_id=?1",
                params![session_id, now_iso()],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("photo avatar run does not exist".into());
        }
        tx.execute(
            "DELETE FROM photo_avatar_sources WHERE session_id=?1",
            [session_id],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "DELETE FROM photo_avatar_artifacts WHERE session_id=?1",
            [session_id],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn mark_accept_cleanup_pending_and_delete_sources(
        &self,
        session_id: &str,
    ) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let updated = tx
            .execute(
                "UPDATE photo_avatar_runs
                 SET step='cleanupPending', provider_job_id=NULL, updated_at=?2
                 WHERE session_id=?1 AND step='previewReady'
                   AND EXISTS (
                     SELECT 1 FROM photo_avatar_artifacts
                     WHERE session_id=?1 AND revision=photo_avatar_runs.revision
                       AND kind='previewPackage' AND local_path IS NOT NULL
                   )",
                params![session_id, now_iso()],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("photo avatar preview package is unavailable for acceptance".into());
        }
        tx.execute(
            "DELETE FROM photo_avatar_sources WHERE session_id=?1",
            [session_id],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn restore_preview_after_finalization_abort(&self, session_id: &str) -> Result<(), String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let updated = storage
            .db
            .execute(
                "UPDATE photo_avatar_runs
                 SET step='previewReady', provider_job_id=NULL, updated_at=?2
                 WHERE session_id=?1 AND step IN ('cleanupPending','completed')
                   AND EXISTS (
                     SELECT 1 FROM photo_avatar_artifacts
                     WHERE session_id=?1 AND revision=photo_avatar_runs.revision
                       AND kind='previewPackage' AND local_path IS NOT NULL
                   )
                   AND EXISTS (
                     SELECT 1 FROM creation_sessions
                     WHERE session_id=?1 AND status='finalizing'
                   )",
                params![session_id, now_iso()],
            )
            .map_err(|error| error.to_string())?;
        if updated == 1 {
            Ok(())
        } else {
            Err("photo avatar preview cannot be restored after finalization abort".into())
        }
    }

    pub fn mark_cleanup_pending_for_run_and_delete_local_data(
        &self,
        session_id: &str,
        revision: u32,
        generation_token: &str,
    ) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let updated = tx
            .execute(
                "UPDATE photo_avatar_runs
                 SET step='cleanupPending', provider_job_id=NULL, updated_at=?4
                 WHERE session_id=?1 AND revision=?2 AND generation_token=?3 AND step='cancelled'",
                params![session_id, revision, generation_token, now_iso()],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("superseded response".into());
        }
        tx.execute(
            "DELETE FROM photo_avatar_sources
             WHERE session_id=?1 AND EXISTS(
                 SELECT 1 FROM photo_avatar_runs
                 WHERE session_id=?1 AND revision=?2 AND generation_token=?3 AND step='cleanupPending'
             )",
            params![session_id, revision, generation_token],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "DELETE FROM photo_avatar_artifacts
             WHERE session_id=?1 AND revision=?2 AND EXISTS(
                 SELECT 1 FROM photo_avatar_runs
                 WHERE session_id=?1 AND revision=?2 AND generation_token=?3 AND step='cleanupPending'
             )",
            params![session_id, revision, generation_token],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn complete_remote_cleanup(
        &self,
        session_id: &str,
        revision: u32,
        generation_token: &str,
    ) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let updated = tx
            .execute(
                "UPDATE photo_avatar_runs
                  SET step=CASE WHEN EXISTS(
                        SELECT 1 FROM creation_sessions cs
                        WHERE cs.session_id=?1 AND cs.status IN ('finalizing','completed')
                      ) THEN 'completed' ELSE 'cancelled' END,
                      provider_session_id=NULL, provider_job_id=NULL,
                     error_code=NULL, error_message=NULL, updated_at=?4
                 WHERE session_id=?1 AND revision=?2 AND generation_token=?3
                   AND step IN ('cancelled', 'cleanupPending')",
                params![session_id, revision, generation_token, now_iso()],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("superseded response".into());
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn prepare_for_full_exit(&self) -> Result<Vec<String>, String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let sessions = {
            let mut statement = tx
                .prepare(
                    "SELECT run.session_id, cs.pet_id
                     FROM photo_avatar_runs run
                     JOIN creation_sessions cs ON cs.session_id=run.session_id
                     JOIN pets p ON p.pet_id=cs.pet_id
                     WHERE run.step='collecting' AND cs.status!='completed'
                       AND cs.status!='abandoned' AND p.lifecycle='draft'
                       AND p.completed_at IS NULL
                     ORDER BY run.session_id",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            rows
        };
        for (session_id, pet_id) in &sessions {
            let now = now_iso();
            tx.execute(
                "UPDATE photo_avatar_runs
                 SET step='cancelled', provider_job_id=NULL, updated_at=?2
                 WHERE session_id=?1 AND step='collecting'",
                params![session_id, now],
            )
            .map_err(|error| error.to_string())?;
            tx.execute(
                "UPDATE creation_sessions
                 SET status='abandoned', current_step='abandoned', error=NULL, updated_at=?2
                 WHERE session_id=?1 AND pet_id=?3 AND status!='completed' AND status!='abandoned'",
                params![session_id, now, pet_id],
            )
            .map_err(|error| error.to_string())?;
            tx.execute(
                "UPDATE pets SET lifecycle='abandoned', updated_at=?2
                 WHERE pet_id=?1 AND lifecycle='draft' AND completed_at IS NULL",
                params![pet_id, now],
            )
            .map_err(|error| error.to_string())?;
            tx.execute(
                "DELETE FROM photo_avatar_sources WHERE session_id=?1",
                [session_id],
            )
            .map_err(|error| error.to_string())?;
            tx.execute(
                "DELETE FROM photo_avatar_artifacts WHERE session_id=?1",
                [session_id],
            )
            .map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(sessions
            .into_iter()
            .map(|(session_id, _)| session_id)
            .collect())
    }

    pub fn abandon_request(
        &self,
        session_id: &str,
    ) -> Result<Option<PhotoAvatarAbandonRequest>, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage.db.query_row(
            "SELECT provider_session_id, provider_job_id FROM photo_avatar_runs WHERE session_id=?1",
            [session_id],
            |row| Ok(PhotoAvatarAbandonRequest {
                provider_session_id: row.get(0)?,
                provider_job_id: row.get(1)?,
            }),
        ).optional().map_err(|error| error.to_string())
    }
}

fn snapshot_with_storage(
    storage: &Storage,
    session_id: &str,
) -> Result<Option<PhotoAvatarSnapshot>, String> {
    let row: Option<(u32, String, Option<String>, Option<String>, Option<String>)> = storage
        .db
        .query_row(
            "SELECT revision, step, provider_job_id, error_code, error_message
             FROM photo_avatar_runs WHERE session_id=?1",
            [session_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((revision, step, provider_job_id, error_code, error_message)) = row else {
        return Ok(None);
    };
    let profile_json: Option<String> = storage
        .db
        .query_row(
            "SELECT profile_json FROM photo_avatar_profiles
             WHERE session_id=?1 AND route='live2d-v5' AND revision IN (?2, -?2)
             ORDER BY revision DESC LIMIT 1",
            params![session_id, revision],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let profile = profile_json
        .as_deref()
        .map(parse_appearance_profile_v1)
        .transpose()?;
    let mut attempts = std::collections::BTreeMap::new();
    let mut statement = storage
        .db
        .prepare(
            "SELECT step, MAX(attempt_no) FROM photo_avatar_step_attempts
             WHERE session_id=?1 AND revision=?2 AND route='live2d-v5' GROUP BY step",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![session_id, revision], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (step, count) = row.map_err(|error| error.to_string())?;
        attempts.insert(attempt_step_from_db(&step)?, count);
    }
    Ok(Some(PhotoAvatarSnapshot {
        session_id: session_id.into(),
        revision,
        step: photo_avatar_step_from_db(&step)?,
        provider_job_id,
        profile,
        attempts,
        error_code: error_code
            .as_deref()
            .map(photo_avatar_error_code_from_db)
            .transpose()?,
        error_message,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoAvatarAbandonRequest {
    pub provider_session_id: Option<String>,
    pub provider_job_id: Option<String>,
}

impl RemoteStep {
    fn as_str(self) -> &'static str {
        match self {
            Self::AnalyzeIdentity => "analyzeIdentity",
            Self::CompleteAppearance => "completeAppearance",
            Self::RenderTextureAtlas => "renderTextureAtlas",
        }
    }
}

fn pixel_step_as_str(step: PixelPhotoAvatarStep) -> &'static str {
    match step {
        PixelPhotoAvatarStep::Collecting => "collecting",
        PixelPhotoAvatarStep::AnalyzeIdentity => "analyzeIdentity",
        PixelPhotoAvatarStep::GeneratePixelAvatar => "generatePixelAvatar",
        PixelPhotoAvatarStep::QualityCheckPending => "qualityCheckPending",
        PixelPhotoAvatarStep::RuntimeCheckPending => "runtimeCheckPending",
        PixelPhotoAvatarStep::PreviewReady => "previewReady",
        PixelPhotoAvatarStep::CleanupPending => "cleanupPending",
        PixelPhotoAvatarStep::Completed => "completed",
        PixelPhotoAvatarStep::Failed => "failed",
        PixelPhotoAvatarStep::Cancelled => "cancelled",
    }
}

fn pixel_step_from_db(value: &str) -> Result<PixelPhotoAvatarStep, String> {
    match value {
        "collecting" => Ok(PixelPhotoAvatarStep::Collecting),
        "analyzeIdentity" => Ok(PixelPhotoAvatarStep::AnalyzeIdentity),
        "generatePixelAvatar" => Ok(PixelPhotoAvatarStep::GeneratePixelAvatar),
        "qualityCheckPending" => Ok(PixelPhotoAvatarStep::QualityCheckPending),
        "runtimeCheckPending" => Ok(PixelPhotoAvatarStep::RuntimeCheckPending),
        "previewReady" => Ok(PixelPhotoAvatarStep::PreviewReady),
        "cleanupPending" => Ok(PixelPhotoAvatarStep::CleanupPending),
        "completed" => Ok(PixelPhotoAvatarStep::Completed),
        "failed" => Ok(PixelPhotoAvatarStep::Failed),
        "cancelled" => Ok(PixelPhotoAvatarStep::Cancelled),
        _ => Err(format!("invalid pixel avatar step: {value}")),
    }
}

fn pixel_remote_step_from_db(value: &str) -> Result<PixelRemoteStep, String> {
    match value {
        "analyzeIdentity" => Ok(PixelRemoteStep::AnalyzeIdentity),
        "generatePixelAvatar" => Ok(PixelRemoteStep::GeneratePixelAvatar),
        _ => Err(format!("invalid pixel remote step: {value}")),
    }
}

fn cleanup_state_as_str(state: CleanupState) -> &'static str {
    match state {
        CleanupState::Deleted => "deleted",
        CleanupState::Pending => "pending",
    }
}

fn upstream_cleanup_state_as_str(state: UpstreamCleanupState) -> &'static str {
    match state {
        UpstreamCleanupState::Unsupported => "unsupported",
    }
}

impl ProviderArtifactKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::TextureAtlas => "textureAtlas",
            Self::PreviewPackage => "previewPackage",
        }
    }
}

fn now_iso() -> String {
    crate::creation::profiles::now_iso()
}

fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn canonical_profile(mut profile: AppearanceProfileV1) -> AppearanceProfileV1 {
    profile
        .traits
        .sort_by_key(|value| trait_key_rank(value.key));
    for identity_trait in &mut profile.traits {
        identity_trait.evidence_photo_ids.sort();
        identity_trait.evidence_photo_ids.dedup();
    }
    profile.completion_summary.sort();
    profile.completion_summary.dedup();
    profile
}

fn trait_key_rank(key: IdentityTraitKey) -> u8 {
    match key {
        IdentityTraitKey::FaceShape => 0,
        IdentityTraitKey::FaceProportions => 1,
        IdentityTraitKey::FurColors => 2,
        IdentityTraitKey::Markings => 3,
        IdentityTraitKey::EyeShape => 4,
        IdentityTraitKey::EyeColor => 5,
        IdentityTraitKey::EarShape => 6,
        IdentityTraitKey::BodyType => 7,
        IdentityTraitKey::Tail => 8,
        IdentityTraitKey::SignatureMarks => 9,
        IdentityTraitKey::Temperament => 10,
    }
}

fn is_lower_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn preview_root(artifact: &ProviderArtifact) -> Result<std::path::PathBuf, String> {
    let root = std::path::PathBuf::from(
        artifact
            .local_path
            .as_deref()
            .ok_or("photo avatar preview package path is unavailable")?,
    );
    if !root.is_absolute() {
        return Err("preview package root is not an absolute directory".into());
    }
    Ok(root)
}

fn read_preview_manifest(artifact: &ProviderArtifact) -> Result<Vec<u8>, String> {
    let root = preview_root(artifact)?;
    let manifest_path = root.join("manifest.json");
    let bytes = crate::platform::read_regular_file_no_reparse(&root, &manifest_path)?;
    if sha256_hex(&bytes) != artifact.sha256 {
        return Err("preview manifest hash does not match the recorded package".into());
    }
    #[cfg(test)]
    {
        let hook = {
            let mut slot = AFTER_PREVIEW_MANIFEST_READ_HOOK
                .lock()
                .map_err(|_| "preview manifest test hook lock poisoned")?;
            match slot.take() {
                Some(hook) if hook.manifest_path == manifest_path => Some(hook.callback),
                Some(hook) => {
                    *slot = Some(hook);
                    None
                }
                None => None,
            }
        };
        if let Some(hook) = hook {
            hook();
        }
    }
    Ok(bytes)
}

fn validate_sources(sources: &[NormalizedPhoto]) -> Result<(), String> {
    if !(1..=8).contains(&sources.len()) {
        return Err("photo avatar requires between one and eight normalized sources".into());
    }
    let mut source_ids = std::collections::HashSet::new();
    let mut ordinals = std::collections::HashSet::new();
    let mut hashes = std::collections::HashSet::new();
    let mut total_bytes = 0_usize;
    for (expected_ordinal, source) in sources.iter().enumerate() {
        if source.source_id.trim().is_empty()
            || !source_ids.insert(&source.source_id)
            || !ordinals.insert(source.ordinal)
        {
            return Err("photo avatar source identifiers must be unique and non-empty".into());
        }
        if source.ordinal != expected_ordinal as u32 {
            return Err("photo avatar source ordinals must start at zero and be contiguous".into());
        }
        if source.normalized_png.is_empty() || source.normalized_png.len() > 10 * 1024 * 1024 {
            return Err("photo avatar source bytes must be between 1 byte and 10 MiB".into());
        }
        if !source.normalized_png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("photo avatar source bytes must be a PNG".into());
        }
        if !(256..=4096).contains(&source.width) || !(256..=4096).contains(&source.height) {
            return Err("photo avatar source dimensions must be between 256 and 4096".into());
        }
        let pixels = u64::from(source.width)
            .checked_mul(u64::from(source.height))
            .ok_or("photo avatar source dimensions overflow")?;
        if pixels > 16_000_000 {
            return Err("photo avatar source pixels exceed 16,000,000".into());
        }
        if !is_lower_hex(&source.sha256) || sha256_hex(&source.normalized_png) != source.sha256 {
            return Err("photo avatar source hash does not match normalized bytes".into());
        }
        if source.source_id != format!("source-{}-{}", source.ordinal, &source.sha256[..12]) {
            return Err("photo avatar source id does not match ordinal and hash".into());
        }
        if !hashes.insert(&source.sha256) {
            return Err("photo avatar normalized source hashes must be distinct".into());
        }
        total_bytes = total_bytes
            .checked_add(source.normalized_png.len())
            .ok_or("photo avatar normalized source total overflows")?;
        if total_bytes > 40 * 1024 * 1024 {
            return Err("photo avatar normalized sources exceed 40 MiB".into());
        }
    }
    Ok(())
}

fn current_run_for_token(tx: &Transaction<'_>, token: &str) -> Result<(String, u32), String> {
    tx.query_row(
        "SELECT session_id, revision FROM photo_avatar_runs
         WHERE generation_token=?1 AND step!='cancelled'",
        [token],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "superseded response".into())
}

fn ensure_current_update(
    tx: &Transaction<'_>,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<(), String> {
    let affected = tx.execute(sql, params).map_err(|error| error.to_string())?;
    if affected == 1 {
        Ok(())
    } else {
        Err("superseded response".into())
    }
}

fn ensure_attempt_is_current(
    tx: &Transaction<'_>,
    token: &str,
    session_id: &str,
    revision: u32,
    step: RemoteStep,
    attempt: u8,
    provider_job_id: &str,
) -> Result<(), String> {
    let affected = tx
        .execute(
            "UPDATE photo_avatar_step_attempts
             SET status='succeeded', finished_at=?7
             WHERE session_id=?1 AND revision=?2 AND step=?3 AND attempt_no=?4
               AND provider_job_id=?5 AND status='running'
               AND EXISTS(
                 SELECT 1 FROM photo_avatar_runs
                 WHERE session_id=?1 AND revision=?2 AND generation_token=?6 AND step=?3
               )",
            params![
                session_id,
                revision,
                step.as_str(),
                attempt,
                provider_job_id,
                token,
                now_iso(),
            ],
        )
        .map_err(|error| error.to_string())?;
    if affected == 1 {
        Ok(())
    } else {
        Err("superseded response".into())
    }
}

fn photo_avatar_step_from_db(value: &str) -> Result<PhotoAvatarStep, String> {
    serde_json::from_value(serde_json::Value::String(value.into()))
        .map_err(|error| format!("invalid stored photo avatar step: {error}"))
}

fn attempt_step_from_db(value: &str) -> Result<PhotoAvatarAttemptStep, String> {
    serde_json::from_value(serde_json::Value::String(value.into()))
        .map_err(|error| format!("invalid stored photo avatar attempt step: {error}"))
}

fn remote_step_from_db(value: &str) -> Result<RemoteStep, String> {
    match value {
        "analyzeIdentity" => Ok(RemoteStep::AnalyzeIdentity),
        "completeAppearance" => Ok(RemoteStep::CompleteAppearance),
        "renderTextureAtlas" => Ok(RemoteStep::RenderTextureAtlas),
        _ => Err("attempt identity required".into()),
    }
}

fn photo_avatar_error_code_from_db(value: &str) -> Result<PhotoAvatarErrorCode, String> {
    serde_json::from_value(serde_json::Value::String(value.into()))
        .map_err(|error| format!("invalid stored photo avatar error code: {error}"))
}

fn error_code_as_str(code: PhotoAvatarErrorCode) -> &'static str {
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
}

fn remote_step_for_avatar_step(step: PhotoAvatarStep) -> Result<RemoteStep, String> {
    match step {
        PhotoAvatarStep::AnalyzeIdentity => Ok(RemoteStep::AnalyzeIdentity),
        PhotoAvatarStep::CompleteAppearance => Ok(RemoteStep::CompleteAppearance),
        PhotoAvatarStep::RenderTextureAtlas => Ok(RemoteStep::RenderTextureAtlas),
        _ => Err("photo avatar run is not on a remote step".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::photo_avatar::domain::{
        parse_appearance_profile_v1, PhotoAvatarAttemptStep,
    };
    use image::{DynamicImage, ImageFormat, RgbaImage};
    use sha2::{Digest, Sha256};
    use std::io::Cursor;

    fn test_store() -> (PhotoAvatarStore, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-photo-avatar-{}",
            crate::creation::domain::new_entity_id("store")
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
        (PhotoAvatarStore::new(storage), root)
    }

    fn source(ordinal: u32, color: u8) -> NormalizedPhoto {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            256,
            256,
            image::Rgba([color, 0, 0, 255]),
        ));
        let mut normalized_png = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut normalized_png), ImageFormat::Png)
            .unwrap();
        let sha256 = format!("{:x}", Sha256::digest(&normalized_png));
        NormalizedPhoto {
            source_id: format!("source-{ordinal}-{}", &sha256[..12]),
            ordinal,
            normalized_png,
            sha256,
            width: 256,
            height: 256,
        }
    }

    fn valid_profile() -> AppearanceProfileV1 {
        parse_appearance_profile_v1(
            r#"{"schemaVersion":1,"species":"cat","style":"animated-film-soft-v1","bodyModuleId":"body-balanced-v1","bodyModuleSource":"ai-completed","traits":[{"key":"faceShape","value":"round","source":"user","evidencePhotoIds":["front"]}],"completionSummary":[]}"#,
        )
        .unwrap()
    }

    #[test]
    fn run_exists_distinguishes_pre_begin_draft_from_existing_run() {
        let (store, root) = test_store();

        assert!(!store.run_exists("session-a").unwrap());
        store.begin_revision("session-a", None, &[]).unwrap();
        assert!(store.run_exists("session-a").unwrap());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancelled_pixel_run_rejects_late_failure() {
        let (store, root) = test_store();
        let run = store
            .begin_pixel_revision("session-a", PixelStyleProfileId::V1, None, &[])
            .unwrap();
        store
            .set_pixel_step("session-a", run.revision, PixelPhotoAvatarStep::Cancelled)
            .unwrap();

        assert!(!store
            .fail_pixel_revision_if_active("session-a", run.revision)
            .unwrap());
        assert_eq!(
            store.pixel_snapshot("session-a").unwrap().step,
            PixelPhotoAvatarStep::Cancelled
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pixel_revision_persists_explicit_v2_style() {
        let (store, root) = test_store();
        let run = store
            .begin_pixel_revision(
                "session-a",
                PixelStyleProfileId::V2AnimationReady,
                None,
                &[],
            )
            .unwrap();

        assert_eq!(run.style_profile_id, PixelStyleProfileId::V2AnimationReady);
        assert_eq!(
            store.pixel_snapshot("session-a").unwrap().style_profile_id,
            PixelStyleProfileId::V2AnimationReady
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn superseded_pixel_revision_rejects_late_failure() {
        let (store, root) = test_store();
        let old = store
            .begin_pixel_revision("session-a", PixelStyleProfileId::V1, None, &[])
            .unwrap();
        let current = store
            .begin_pixel_revision("session-a", PixelStyleProfileId::V1, None, &[])
            .unwrap();

        assert!(!store
            .fail_pixel_revision_if_active("session-a", old.revision)
            .unwrap());
        let snapshot = store.pixel_snapshot("session-a").unwrap();
        assert_eq!(snapshot.revision, current.revision);
        assert_eq!(snapshot.step, PixelPhotoAvatarStep::AnalyzeIdentity);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn active_pixel_failure_records_safe_error_details() {
        let (store, root) = test_store();
        let run = store
            .begin_pixel_revision("session-a", PixelStyleProfileId::V1, None, &[])
            .unwrap();

        assert!(store
            .fail_pixel_revision_with_error_if_active(
                "session-a",
                run.revision,
                PhotoAvatarErrorCode::InvalidInput,
                "生成图片不符合像素素材要求，请重试。",
            )
            .unwrap());
        let snapshot = store.pixel_snapshot("session-a").unwrap();
        assert_eq!(snapshot.step, PixelPhotoAvatarStep::Failed);
        assert_eq!(
            snapshot.error_code,
            Some(PhotoAvatarErrorCode::InvalidInput)
        );
        assert_eq!(
            snapshot.error_message.as_deref(),
            Some("生成图片不符合像素素材要求，请重试。")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cleanup_audit_advances_pending_but_never_overwrites_terminal_outcome() {
        use crate::creation::photo_avatar::provider::{CleanupState, UpstreamCleanupState};
        let (store, root) = test_store();
        let run = store.begin_revision("session-a", None, &[]).unwrap();

        store
            .record_cleanup_audit(
                "session-a",
                run.revision,
                CleanupState::Deleted,
                CleanupState::Pending,
                UpstreamCleanupState::Unsupported,
                "lk888",
            )
            .unwrap();
        store
            .record_cleanup_audit(
                "session-a",
                run.revision,
                CleanupState::Deleted,
                CleanupState::Deleted,
                UpstreamCleanupState::Unsupported,
                "lk888",
            )
            .unwrap();
        store
            .record_cleanup_audit(
                "session-a",
                run.revision,
                CleanupState::Pending,
                CleanupState::Pending,
                UpstreamCleanupState::Unsupported,
                "lk888",
            )
            .unwrap();

        let audit: (String, String, String, String) = store
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT local_cleanup, backend_cleanup, upstream_cleanup, provider_id
                 FROM photo_avatar_cleanup_audit WHERE session_id='session-a' AND revision=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            audit,
            (
                "deleted".into(),
                "deleted".into(),
                "unsupported".into(),
                "lk888".into()
            )
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn attached_attempt(
        store: &PhotoAvatarStore,
        run: &PhotoAvatarRun,
        step: RemoteStep,
        job_id: &str,
    ) -> u8 {
        let attempt = store
            .reserve_attempt("session-a", run.revision, step)
            .unwrap();
        store
            .attach_job(
                &run.generation_token,
                step,
                attempt,
                &RemoteJob {
                    provider_session_id: Some("provider-session".into()),
                    provider_job_id: job_id.into(),
                },
            )
            .unwrap();
        attempt
    }

    fn preinsert_final_profile(
        store: &PhotoAvatarStore,
        revision: u32,
        profile: &AppearanceProfileV1,
    ) -> String {
        let profile = canonical_profile(profile.clone());
        let json = serde_json::to_string(&profile).unwrap();
        let hash = sha256_hex(json.as_bytes());
        store
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO photo_avatar_profiles
                 (session_id, revision, schema_version, body_module_id, profile_json,
                  profile_sha256, created_at)
                 VALUES ('session-a', ?1, 1, ?2, ?3, ?4, '10')",
                params![revision, profile.body_module_id, json, hash],
            )
            .unwrap();
        hash
    }

    fn preview_fixture(store: &PhotoAvatarStore) -> (PhotoAvatarRun, std::path::PathBuf, Vec<u8>) {
        let run = store.begin_revision("session-a", None, &[]).unwrap();
        store
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE photo_avatar_runs SET step='buildV5' WHERE session_id='session-a'",
                [],
            )
            .unwrap();
        let package = std::env::temp_dir().join(format!(
            "desktop-pet-photo-avatar-preview-{}",
            crate::creation::domain::new_entity_id("package")
        ));
        std::fs::create_dir_all(&package).unwrap();
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
        .map(|name| {
            (
                name.to_string(),
                serde_json::json!({ "group": name, "index": 0 }),
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
        .map(|name| (name.to_string(), serde_json::json!(format!("Param{name}"))))
        .collect::<serde_json::Map<_, _>>();
        let manifest = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 5,
            "renderer": "cat-spatial-live2d-v1",
            "petId": "pet-a",
            "variantId": "preview-v1",
            "skeletonVersion": "cat-a-live2d-v1",
            "bodyModuleId": "body-balanced-v1",
            "modelEntry": "cat.model3.json",
            "previewImage": "preview.png",
            "motionSpatialProfile": "profiles/body-balanced.json",
            "files": [
                { "role": "model", "relativePath": "cat.model3.json", "sha256": "ab".repeat(32) },
                { "role": "preview", "relativePath": "preview.png", "sha256": "cd".repeat(32) },
                { "role": "motion-spatial-profile", "relativePath": "profiles/body-balanced.json", "sha256": "ef".repeat(32) }
            ],
            "motions": motions,
            "parameters": parameters,
            "hitAreas": { "body": "ArtMeshBody", "edgeTail": "ArtMeshTail" },
            "edgeTailStates": {
                "left": { "group": "EdgeTail", "index": 0, "tailArtMesh": "ArtMeshTail" },
                "right": { "group": "EdgeTail", "index": 0, "tailArtMesh": "ArtMeshTail" },
                "top": { "group": "EdgeTail", "index": 0, "tailArtMesh": "ArtMeshTail" },
                "bottom": { "group": "EdgeTail", "index": 0, "tailArtMesh": "ArtMeshTail" }
            },
            "license": { "id": "project", "author": "PetBaby", "source": "project", "commercialUse": true, "redistributable": true }
        }))
        .unwrap();
        std::fs::write(package.join("manifest.json"), &manifest).unwrap();
        let hash = sha256_hex(&manifest);
        store
            .commit_preview_package(
                "session-a",
                run.revision,
                &run.generation_token,
                &package,
                &hash,
            )
            .unwrap();
        (run, package, manifest)
    }

    #[test]
    fn runtime_check_and_manifest_serving_bind_the_recorded_hash_to_disk_bytes() {
        let (store, root) = test_store();
        let (run, package, manifest) = preview_fixture(&store);
        let recorded_hash = sha256_hex(&manifest);

        assert_eq!(
            store.preview_manifest("session-a", run.revision).unwrap()["schemaVersion"],
            5
        );
        std::fs::write(package.join("manifest.json"), b"{}").unwrap();

        let runtime_error = store
            .runtime_check_passed("session-a", run.revision, &recorded_hash)
            .unwrap_err();
        assert!(runtime_error.contains("manifest hash"), "{runtime_error}");
        let serving_error = store
            .preview_manifest("session-a", run.revision)
            .unwrap_err();
        assert!(serving_error.contains("manifest hash"), "{serving_error}");
        assert_eq!(
            store.snapshot("session-a").unwrap().step,
            PhotoAvatarStep::RuntimeCheckPending
        );

        std::fs::write(package.join("manifest.json"), &manifest).unwrap();
        store
            .runtime_check_passed("session-a", run.revision, &recorded_hash)
            .unwrap();
        assert_eq!(
            store.snapshot("session-a").unwrap().step,
            PhotoAvatarStep::PreviewReady
        );
        let _ = std::fs::remove_dir_all(package);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preview_manifest_returns_the_same_verified_bytes_when_the_file_changes_after_read() {
        let (store, root) = test_store();
        let (run, package, manifest) = preview_fixture(&store);
        let manifest_path = package.join("manifest.json");
        let mut replacement: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        replacement["petId"] = serde_json::Value::String("replaced-pet".into());
        let replacement = serde_json::to_vec(&replacement).unwrap();

        *AFTER_PREVIEW_MANIFEST_READ_HOOK.lock().unwrap() = Some(AfterPreviewManifestReadHook {
            manifest_path,
            callback: Box::new(move || {
                std::fs::write(package.join("manifest.json"), replacement).unwrap()
            }),
        });

        let served = store.preview_manifest("session-a", run.revision).unwrap();

        assert_eq!(served["petId"], "pet-a");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn canonical_profile_hash_is_idempotent_and_conflicting_revision_is_rejected() {
        for conflict in [false, true] {
            let (store, root) = test_store();
            let run = store.begin_revision("session-a", None, &[]).unwrap();
            let analyze_attempt =
                attached_attempt(&store, &run, RemoteStep::AnalyzeIdentity, "identity-job");
            let mut partial = valid_profile();
            partial.traits[0].evidence_photo_ids = vec!["side".into(), "front".into()];
            store
                .commit_profile_for_attempt(
                    &run.generation_token,
                    RemoteStep::AnalyzeIdentity,
                    analyze_attempt,
                    "identity-job",
                    &partial,
                )
                .unwrap();
            assert_eq!(
                store.snapshot("session-a").unwrap().profile,
                Some(canonical_profile(partial.clone()))
            );
            let complete_attempt = attached_attempt(
                &store,
                &run,
                RemoteStep::CompleteAppearance,
                "appearance-job",
            );
            let stored_hash = preinsert_final_profile(&store, run.revision, &partial);
            let mut response = partial.clone();
            response.traits[0].evidence_photo_ids.reverse();
            if conflict {
                response.traits[0].value = "triangle".into();
                let error = store
                    .commit_profile_for_attempt(
                        &run.generation_token,
                        RemoteStep::CompleteAppearance,
                        complete_attempt,
                        "appearance-job",
                        &response,
                    )
                    .unwrap_err();
                assert_eq!(error, "profile revision conflict");
                assert_eq!(
                    store.current_run("session-a").unwrap().step,
                    PhotoAvatarStep::CompleteAppearance
                );
            } else {
                store
                    .commit_profile_for_attempt(
                        &run.generation_token,
                        RemoteStep::CompleteAppearance,
                        complete_attempt,
                        "appearance-job",
                        &response,
                    )
                    .unwrap();
                let actual_hash: String = store
                    .storage
                    .lock()
                    .unwrap()
                    .db
                    .query_row(
                        "SELECT profile_sha256 FROM photo_avatar_profiles
                         WHERE session_id='session-a' AND revision=?1",
                        [run.revision],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(actual_hash, stored_hash);
            }
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn partial_profile_recovers_a_legacy_positive_revision_only_while_completing() {
        let (store, root) = test_store();
        let run = store.begin_revision("session-a", None, &[]).unwrap();
        let legacy_partial = valid_profile();
        preinsert_final_profile(&store, run.revision, &legacy_partial);

        store
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE photo_avatar_runs SET step='renderTextureAtlas' WHERE session_id='session-a'",
                [],
            )
            .unwrap();
        assert_eq!(
            store.partial_profile("session-a", run.revision).unwrap(),
            None
        );

        store
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE photo_avatar_runs SET step='completeAppearance' WHERE session_id='session-a'",
                [],
            )
            .unwrap();
        assert_eq!(
            store.partial_profile("session-a", run.revision).unwrap(),
            Some(legacy_partial.clone())
        );
        assert_eq!(
            store
                .legacy_partial_profile("session-a", run.revision)
                .unwrap(),
            Some(legacy_partial)
        );
        let profile_count: i64 = store
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM photo_avatar_profiles
                 WHERE session_id='session-a' AND revision=?1",
                [-i64::from(run.revision)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(profile_count, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_partial_profile_recovers_a_migrated_row_after_restart() {
        let (store, root) = test_store();
        let run = store.begin_revision("session-a", None, &[]).unwrap();
        let legacy_partial = valid_profile();
        preinsert_final_profile(&store, run.revision, &legacy_partial);
        store
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE photo_avatar_runs SET step='completeAppearance' WHERE session_id='session-a'",
                [],
            )
            .unwrap();

        assert_eq!(
            store
                .legacy_partial_profile("session-a", run.revision)
                .unwrap(),
            Some(legacy_partial.clone())
        );

        let restarted = PhotoAvatarStore::new(Arc::new(Mutex::new(Storage::open(&root).unwrap())));
        assert_eq!(
            restarted
                .legacy_partial_profile("session-a", run.revision)
                .unwrap(),
            Some(legacy_partial)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_partial_profile_rejects_an_unmarked_negative_revision() {
        let (store, root) = test_store();
        let run = store.begin_revision("session-a", None, &[]).unwrap();
        let attempt = attached_attempt(&store, &run, RemoteStep::AnalyzeIdentity, "identity-job");
        store
            .commit_profile_for_attempt(
                &run.generation_token,
                RemoteStep::AnalyzeIdentity,
                attempt,
                "identity-job",
                &valid_profile(),
            )
            .unwrap();

        assert_eq!(
            store
                .legacy_partial_profile("session-a", run.revision)
                .unwrap(),
            None
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_never_serializes_source_photo_bytes() {
        let (store, root) = test_store();
        store
            .replace_sources("session-a", &[source(0, 42)])
            .unwrap();
        store.begin_revision("session-a", None, &[]).unwrap();

        let json = serde_json::to_string(&store.snapshot("session-a").unwrap()).unwrap();

        assert!(!json.contains("normalizedPng"));
        assert!(!json.contains("pngBase64"));
        assert!(!json.contains("iVBOR"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn optional_snapshot_is_none_without_a_run_and_some_with_a_run() {
        let (store, root) = test_store();

        assert_eq!(store.snapshot_if_exists("session-a").unwrap(), None);
        store.begin_revision("session-a", None, &[]).unwrap();

        let snapshot = store.snapshot_if_exists("session-a").unwrap();
        assert_eq!(
            snapshot.map(|value| value.step),
            Some(PhotoAvatarStep::AnalyzeIdentity)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn source_replacement_is_atomic_and_delete_reports_removed_rows() {
        let (store, root) = test_store();
        store
            .replace_sources("session-a", &[source(0, 1), source(1, 2)])
            .unwrap();
        let mut invalid = source(0, 3);
        invalid.normalized_png = vec![1];
        invalid.sha256 = format!("{:x}", Sha256::digest(&invalid.normalized_png));
        let error = store.replace_sources("session-a", &[invalid]).unwrap_err();
        assert!(error.contains("source"));
        let retained: i64 = store
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM photo_avatar_sources WHERE session_id='session-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 2);
        let snapshot = store.snapshot("session-a").unwrap_err();
        assert!(snapshot.contains("run"));
        assert_eq!(store.delete_sources("session-a").unwrap(), 2);
        assert_eq!(store.delete_sources("session-a").unwrap(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stale_terminal_cancel_identity_cannot_delete_new_revision_sources() {
        for (terminal_step, terminal_step_db) in [
            (PhotoAvatarStep::Completed, "completed"),
            (PhotoAvatarStep::Failed, "failed"),
        ] {
            let (store, root) = test_store();
            let first = store.begin_revision("session-a", None, &[]).unwrap();
            store
                .storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "UPDATE photo_avatar_runs SET step=?1 WHERE session_id=?2",
                    params![terminal_step_db, first.session_id],
                )
                .unwrap();
            let stale_terminal_run = store.current_run("session-a").unwrap();
            assert_eq!(stale_terminal_run.step, terminal_step);

            let second = store.begin_revision("session-a", None, &[]).unwrap();
            let replacement = source(0, 77);
            store
                .replace_sources("session-a", std::slice::from_ref(&replacement))
                .unwrap();

            let error = store
                .delete_sources_for_terminal_run(&stale_terminal_run)
                .unwrap_err();

            assert!(error.contains("superseded response"), "{error}");
            let current = store.current_run("session-a").unwrap();
            assert_eq!(current.revision, second.revision);
            assert_eq!(current.generation_token, second.generation_token);
            assert_eq!(current.step, PhotoAvatarStep::AnalyzeIdentity);
            assert_eq!(store.sources("session-a").unwrap(), vec![replacement]);
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn replace_sources_enforces_normalized_photo_contract_before_mutating_rows() {
        let (store, root) = test_store();
        store.replace_sources("session-a", &[source(0, 1)]).unwrap();
        let mut non_png = source(0, 2);
        non_png.normalized_png = vec![1, 2, 3];
        non_png.sha256 = format!("{:x}", Sha256::digest(&non_png.normalized_png));
        let mut discontinuous = source(1, 3);
        discontinuous.ordinal = 2;
        let mut wrong_id = source(0, 4);
        wrong_id.source_id = "source-0-not-the-hash".into();
        let mut oversized_total = Vec::new();
        for ordinal in 0..5 {
            let mut photo = source(ordinal, ordinal as u8 + 10);
            photo.normalized_png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
            photo.normalized_png.resize(9 * 1024 * 1024, ordinal as u8);
            photo.sha256 = format!("{:x}", Sha256::digest(&photo.normalized_png));
            photo.source_id = format!("source-{ordinal}-{}", &photo.sha256[..12]);
            oversized_total.push(photo);
        }

        for invalid in [
            vec![non_png],
            vec![discontinuous],
            vec![wrong_id],
            oversized_total,
        ] {
            assert!(store.replace_sources("session-a", &invalid).is_err());
        }
        let retained: i64 = store
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM photo_avatar_sources WHERE session_id='session-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn terminal_cleanup_removes_only_finished_session_sources() {
        let (store, root) = test_store();
        let storage = store.storage.clone();
        {
            let storage = storage.lock().unwrap();
            storage.db.execute_batch(
                "INSERT INTO pets (pet_id, schema_version, species, identity_mode, creation_method, lifecycle, created_at, updated_at)
                 VALUES ('pet-completed', 1, 'cat', 'realpet', 'upload', 'ready', '10', '10'),
                        ('pet-cancelled', 1, 'cat', 'realpet', 'upload', 'draft', '10', '10'),
                        ('pet-pending', 1, 'cat', 'realpet', 'upload', 'draft', '10', '10');
                 INSERT INTO creation_sessions (session_id, pet_id, method, status, last_stable_status, current_step, schema_version, created_at, updated_at)
                 VALUES ('session-completed', 'pet-completed', 'upload', 'completed', 'completed', 'completed', 1, '10', '10'),
                        ('session-cancelled', 'pet-cancelled', 'adoption', 'draft', 'draft', 'upload', 1, '10', '10'),
                        ('session-pending', 'pet-pending', 'adoption', 'draft', 'draft', 'upload', 1, '10', '10');
                 INSERT INTO photo_avatar_runs (session_id, revision, step, generation_token, updated_at)
                 VALUES ('session-cancelled', 1, 'cancelled', 'token-cancelled', '10'),
                        ('session-pending', 1, 'cleanupPending', 'token-pending', '10');",
            ).unwrap();
        }
        for session_id in [
            "session-a",
            "session-completed",
            "session-cancelled",
            "session-pending",
        ] {
            store.replace_sources(session_id, &[source(0, 42)]).unwrap();
        }

        assert_eq!(
            store.cleanup_terminal_photo_avatar_sources().unwrap(),
            vec!["session-cancelled", "session-completed"]
        );
        for session_id in ["session-a", "session-pending"] {
            let count: i64 = store
                .storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT COUNT(*) FROM photo_avatar_sources WHERE session_id=?1",
                    [session_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{session_id} must remain recoverable");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reserves_at_most_three_attempts_for_each_remote_step() {
        let (store, root) = test_store();
        let run = store.begin_revision("session-a", None, &[]).unwrap();
        assert_eq!(run.revision, 1);
        for expected in 1..=3 {
            assert_eq!(
                store
                    .reserve_attempt("session-a", run.revision, RemoteStep::AnalyzeIdentity)
                    .unwrap(),
                expected
            );
            let reserved = store.current_run("session-a").unwrap();
            store
                .fail_attempt(&reserved, PhotoAvatarErrorCode::Network, "temporary", true)
                .unwrap();
        }
        assert_eq!(
            store.snapshot("session-a").unwrap().step,
            PhotoAvatarStep::Failed
        );
        assert!(store
            .reserve_attempt("session-a", run.revision, RemoteStep::AnalyzeIdentity)
            .unwrap_err()
            .contains("not current"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stale_or_cancelled_tokens_cannot_commit_provider_results() {
        let (store, root) = test_store();
        let first = store.begin_revision("session-a", None, &[]).unwrap();
        store
            .reserve_attempt("session-a", first.revision, RemoteStep::AnalyzeIdentity)
            .unwrap();
        let second = store
            .begin_revision("session-a", Some("more amber eyes"), &[])
            .unwrap();
        assert_eq!(first.generation_token.len(), 64);
        assert!(first
            .generation_token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first.generation_token, second.generation_token);
        let job = RemoteJob {
            provider_session_id: Some("provider-session".into()),
            provider_job_id: "provider-job".into(),
        };
        assert!(store
            .attach_job(
                &first.generation_token,
                RemoteStep::AnalyzeIdentity,
                1,
                &job
            )
            .unwrap_err()
            .contains("superseded"));
        store
            .cancel("session-a", second.revision, &second.generation_token)
            .unwrap();
        assert!(store
            .commit_profile(&second.generation_token, &valid_profile())
            .unwrap_err()
            .contains("superseded"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn retry_requires_attempt_specific_commit_to_reject_an_earlier_result() {
        let (store, root) = test_store();
        let run = store.begin_revision("session-a", None, &[]).unwrap();
        let first = store
            .reserve_attempt("session-a", run.revision, RemoteStep::AnalyzeIdentity)
            .unwrap();
        let job = |provider_job_id: &str| RemoteJob {
            provider_session_id: None,
            provider_job_id: provider_job_id.into(),
        };
        store
            .attach_job(
                &run.generation_token,
                RemoteStep::AnalyzeIdentity,
                first,
                &job("job-1"),
            )
            .unwrap();
        let failed_first = store.current_run("session-a").unwrap();
        store
            .fail_attempt(
                &failed_first,
                PhotoAvatarErrorCode::Network,
                "temporary",
                true,
            )
            .unwrap();
        let second = store
            .reserve_attempt("session-a", run.revision, RemoteStep::AnalyzeIdentity)
            .unwrap();
        store
            .attach_job(
                &run.generation_token,
                RemoteStep::AnalyzeIdentity,
                second,
                &job("job-2"),
            )
            .unwrap();

        assert!(store
            .commit_profile(&run.generation_token, &valid_profile())
            .unwrap_err()
            .contains("attempt identity required"));
        let profile = valid_profile();
        store
            .commit_profile_for_attempt(
                &run.generation_token,
                RemoteStep::AnalyzeIdentity,
                second,
                "job-2",
                &profile,
            )
            .unwrap();
        assert!(store
            .commit_profile_for_attempt(
                &run.generation_token,
                RemoteStep::AnalyzeIdentity,
                first,
                "job-1",
                &valid_profile(),
            )
            .unwrap_err()
            .contains("superseded"));

        let complete_attempt = store
            .reserve_attempt("session-a", run.revision, RemoteStep::CompleteAppearance)
            .unwrap();
        store
            .attach_job(
                &run.generation_token,
                RemoteStep::CompleteAppearance,
                complete_attempt,
                &job("complete-job-1"),
            )
            .unwrap();
        store
            .commit_profile_for_attempt(
                &run.generation_token,
                RemoteStep::CompleteAppearance,
                complete_attempt,
                "complete-job-1",
                &profile,
            )
            .unwrap();

        let render_first = store
            .reserve_attempt("session-a", run.revision, RemoteStep::RenderTextureAtlas)
            .unwrap();
        store
            .attach_job(
                &run.generation_token,
                RemoteStep::RenderTextureAtlas,
                render_first,
                &job("render-job-1"),
            )
            .unwrap();
        let failed_render = store.current_run("session-a").unwrap();
        store
            .fail_attempt(
                &failed_render,
                PhotoAvatarErrorCode::Network,
                "temporary",
                true,
            )
            .unwrap();
        let render_second = store
            .reserve_attempt("session-a", run.revision, RemoteStep::RenderTextureAtlas)
            .unwrap();
        store
            .attach_job(
                &run.generation_token,
                RemoteStep::RenderTextureAtlas,
                render_second,
                &job("render-job-2"),
            )
            .unwrap();
        let artifact = ProviderArtifact {
            kind: ProviderArtifactKind::TextureAtlas,
            relative_path: "atlas/current.png".into(),
            sha256: "a".repeat(64),
            local_path: None,
            audit_json: Some("{\"schemaVersion\":1}".into()),
        };
        store
            .commit_artifact_for_attempt(
                &run.generation_token,
                RemoteStep::RenderTextureAtlas,
                render_second,
                "render-job-2",
                &artifact,
            )
            .unwrap();
        assert!(store
            .commit_artifact_for_attempt(
                &run.generation_token,
                RemoteStep::RenderTextureAtlas,
                render_first,
                "render-job-1",
                &ProviderArtifact {
                    kind: ProviderArtifactKind::TextureAtlas,
                    relative_path: "atlas/stale.png".into(),
                    sha256: "b".repeat(64),
                    local_path: None,
                    audit_json: Some("{\"schemaVersion\":1}".into()),
                },
            )
            .unwrap_err()
            .contains("superseded"));
        let snapshot = store.snapshot("session-a").unwrap();
        assert_eq!(snapshot.step, PhotoAvatarStep::BuildV5);
        assert_eq!(snapshot.profile, Some(profile));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_projects_current_revision_profile_and_attempts() {
        let (store, root) = test_store();
        let run = store
            .begin_revision("session-a", Some("make the tail fluffier"), &[])
            .unwrap();
        let attempt = store
            .reserve_attempt("session-a", run.revision, RemoteStep::AnalyzeIdentity)
            .unwrap();
        store
            .attach_job(
                &run.generation_token,
                RemoteStep::AnalyzeIdentity,
                attempt,
                &RemoteJob {
                    provider_session_id: None,
                    provider_job_id: "job-1".into(),
                },
            )
            .unwrap();
        let profile = valid_profile();
        store
            .commit_profile(&run.generation_token, &profile)
            .unwrap();
        let complete_attempt = store
            .reserve_attempt("session-a", run.revision, RemoteStep::CompleteAppearance)
            .unwrap();
        store
            .attach_job(
                &run.generation_token,
                RemoteStep::CompleteAppearance,
                complete_attempt,
                &RemoteJob {
                    provider_session_id: None,
                    provider_job_id: "complete-job-1".into(),
                },
            )
            .unwrap();
        store
            .commit_profile_for_attempt(
                &run.generation_token,
                RemoteStep::CompleteAppearance,
                complete_attempt,
                "complete-job-1",
                &profile,
            )
            .unwrap();
        let render_attempt = store
            .reserve_attempt("session-a", run.revision, RemoteStep::RenderTextureAtlas)
            .unwrap();
        store
            .attach_job(
                &run.generation_token,
                RemoteStep::RenderTextureAtlas,
                render_attempt,
                &RemoteJob {
                    provider_session_id: None,
                    provider_job_id: "render-job-1".into(),
                },
            )
            .unwrap();
        store
            .commit_artifact_for_attempt(
                &run.generation_token,
                RemoteStep::RenderTextureAtlas,
                render_attempt,
                "render-job-1",
                &ProviderArtifact {
                    kind: ProviderArtifactKind::TextureAtlas,
                    relative_path: "atlas/texture.png".into(),
                    sha256: "a".repeat(64),
                    local_path: None,
                    audit_json: Some("{\"schemaVersion\":1}".into()),
                },
            )
            .unwrap();
        assert_eq!(
            store
                .texture_artifact("session-a", run.revision)
                .unwrap()
                .unwrap()
                .audit_json
                .as_deref(),
            Some("{\"schemaVersion\":1}")
        );
        let snapshot = store.snapshot("session-a").unwrap();
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.provider_job_id, None);
        assert_eq!(snapshot.profile, Some(profile));
        assert_eq!(
            snapshot
                .attempts
                .get(&PhotoAvatarAttemptStep::AnalyzeIdentity),
            Some(&1)
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
