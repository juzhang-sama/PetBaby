use super::domain::PixelPhotoAvatarStep;
use super::pixel_manager::PixelPhotoAvatarManager;
use super::provider::PhotoAvatarProvider;
use crate::creation::finalization::{PhotoAvatarCleanupDisposition, PhotoAvatarFinalizationPort};
use crate::creation::service::PhotoAvatarAbandonPort;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use std::path::Path;

impl PixelPhotoAvatarManager {
    pub fn save_consent(&self, accepted: bool) -> Result<bool, String> {
        if accepted {
            self.store
                .save_consent(super::domain::PHOTO_AVATAR_CONSENT_VERSION)?;
        }
        Ok(accepted)
    }

    pub fn runtime_check_passed(
        &self,
        session_id: &str,
        revision: u32,
        manifest_sha256: &str,
    ) -> Result<super::domain::PixelPhotoAvatarSnapshot, String> {
        self.builder.validate_preview(session_id, revision)?;
        let manifest = self.preview_file(session_id, revision, "manifest.json")?;
        if format!("{:x}", Sha256::digest(&manifest)) != manifest_sha256 {
            return Err("pixel preview manifest hash mismatch".into());
        }
        self.store
            .set_pixel_step(session_id, revision, PixelPhotoAvatarStep::PreviewReady)?;
        self.store.pixel_snapshot(session_id)
    }

    pub fn preview_manifest(
        &self,
        session_id: &str,
        revision: u32,
    ) -> Result<serde_json::Value, String> {
        let bytes = self.preview_file(session_id, revision, "manifest.json")?;
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }

    pub fn preview_file_b64(
        &self,
        session_id: &str,
        revision: u32,
        relative_path: &str,
    ) -> Result<String, String> {
        self.preview_file(session_id, revision, relative_path)
            .map(|bytes| STANDARD.encode(bytes))
    }

    fn preview_file(
        &self,
        session_id: &str,
        revision: u32,
        relative_path: &str,
    ) -> Result<Vec<u8>, String> {
        if relative_path.is_empty()
            || relative_path.contains("..")
            || relative_path.contains(['/', '\\'])
        {
            return Err("invalid pixel preview path".into());
        }
        std::fs::read(
            self.preview_root
                .join(session_id)
                .join(revision.to_string())
                .join(relative_path),
        )
        .map_err(|error| error.to_string())
    }
}

impl PhotoAvatarFinalizationPort for PixelPhotoAvatarManager {
    fn preview_ready(&self, session_id: &str) -> Result<bool, String> {
        Ok(matches!(
            self.store.pixel_snapshot(session_id)?.step,
            PixelPhotoAvatarStep::PreviewReady
                | PixelPhotoAvatarStep::CleanupPending
                | PixelPhotoAvatarStep::Completed
        ))
    }

    fn install_preview(
        &self,
        session_id: &str,
        pet_id: &str,
        variant_id: &str,
        destination: &Path,
    ) -> Result<(), String> {
        let snapshot = self.store.pixel_snapshot(session_id)?;
        if snapshot.step != PixelPhotoAvatarStep::PreviewReady {
            return Err("pixel avatar preview is not ready for installation".into());
        }
        let manifest = self.preview_manifest(session_id, snapshot.revision)?;
        if manifest.get("petId").and_then(serde_json::Value::as_str) != Some(pet_id)
            || manifest
                .get("variantId")
                .and_then(serde_json::Value::as_str)
                != Some(variant_id)
        {
            return Err("pixel avatar preview identity does not match finalization".into());
        }
        self.builder
            .install_preview(session_id, snapshot.revision, destination)
    }

    fn cleanup_after_accept(
        &self,
        session_id: &str,
    ) -> Result<PhotoAvatarCleanupDisposition, String> {
        let snapshot = self.store.pixel_snapshot(session_id)?;
        self.store.delete_sources(session_id)?;
        self.store.set_pixel_step(
            session_id,
            snapshot.revision,
            PixelPhotoAvatarStep::Completed,
        )?;
        Ok(PhotoAvatarCleanupDisposition::Complete)
    }

    fn restore_preview_after_abort(&self, session_id: &str) -> Result<(), String> {
        let snapshot = self.store.pixel_snapshot(session_id)?;
        self.store.set_pixel_step(
            session_id,
            snapshot.revision,
            PixelPhotoAvatarStep::PreviewReady,
        )
    }
}

impl PhotoAvatarAbandonPort for PixelPhotoAvatarManager {
    fn cancel_provider_job(&self, _session_id: &str, provider_job_id: &str) -> Result<(), String> {
        self.provider
            .as_ref()
            .ok_or("photo avatar backend is not configured")?
            .cancel_job(provider_job_id)
            .map_err(|error| error.message)
    }

    fn delete_provider_session(
        &self,
        _session_id: &str,
        provider_session_id: &str,
    ) -> Result<(), String> {
        self.provider
            .as_ref()
            .ok_or("photo avatar backend is not configured")?
            .delete_session(provider_session_id)
            .map(|_| ())
            .map_err(|error| error.message)
    }
}
