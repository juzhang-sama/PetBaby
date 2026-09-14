//! 写实风 manager 的**对外接口层**：命令用到的读写 + 两个共享 port 的 impl。
//!
//! 照 `pixel_manager_ports.rs`（只有 155 行）抄。只有一处必须不一样，见 `preview_file`。

use super::domain::{FramePhotoAvatarSnapshot, FramePhotoAvatarStep};
use super::frame_manager::FramePhotoAvatarManager;
use super::provider::PhotoAvatarProvider;
use crate::creation::finalization::{PhotoAvatarCleanupDisposition, PhotoAvatarFinalizationPort};
use crate::creation::service::PhotoAvatarAbandonPort;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use std::path::Path;

impl FramePhotoAvatarManager {
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
    ) -> Result<FramePhotoAvatarSnapshot, String> {
        self.builder.validate_preview(session_id, revision)?;
        let manifest = self.preview_file(session_id, revision, "manifest.json")?;
        if format!("{:x}", Sha256::digest(&manifest)) != manifest_sha256 {
            return Err("frame sequence preview manifest hash mismatch".into());
        }
        self.store
            .set_frame_step(session_id, revision, FramePhotoAvatarStep::PreviewReady)?;
        // 与像素风/composer 对齐：预览就绪就是「有产物」，会话必须从 `draft` 前进到
        // `candidateReady`。少了这一步，重启后前端会把这份快照当成空草稿 `abandon` 掉
        // （真删预览目录），白费一次生成。详见 `mark_frame_candidate_ready`。
        self.store.mark_frame_candidate_ready(session_id)?;
        self.store.frame_snapshot(session_id)
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

    /// 读预览目录里的一个文件。
    ///
    /// ⚠️ **与像素风那条刻意不同**：像素风的预览是平的（`body.png`），所以它直接
    /// 「不许出现斜杠」；**帧序列的路径天生带子目录**（`frames/idle-combo/f0000.webp`），
    /// 照抄那条会把整条链路堵死。
    ///
    /// 但遍历防护一点都不能少，改成**逐段判**：拒绝空段、`.`、`..`、
    /// 开头的 `/`、反斜杠、以及冒号（Windows 盘符 / NTFS 数据流）。
    pub(super) fn preview_file(
        &self,
        session_id: &str,
        revision: u32,
        relative_path: &str,
    ) -> Result<Vec<u8>, String> {
        let unsafe_path = relative_path.is_empty()
            || relative_path.starts_with('/')
            || relative_path.contains('\\')
            || relative_path.contains(':')
            || relative_path
                .split('/')
                .any(|segment| segment.is_empty() || segment == "." || segment == "..");
        if unsafe_path {
            return Err("invalid frame sequence preview path".into());
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

impl PhotoAvatarFinalizationPort for FramePhotoAvatarManager {
    fn preview_ready(&self, session_id: &str) -> Result<bool, String> {
        Ok(matches!(
            self.store.frame_snapshot(session_id)?.step,
            FramePhotoAvatarStep::PreviewReady
                | FramePhotoAvatarStep::CleanupPending
                | FramePhotoAvatarStep::Completed
        ))
    }

    fn install_preview(
        &self,
        session_id: &str,
        pet_id: &str,
        variant_id: &str,
        destination: &Path,
    ) -> Result<(), String> {
        let snapshot = self.store.frame_snapshot(session_id)?;
        if snapshot.step != FramePhotoAvatarStep::PreviewReady {
            return Err("frame sequence preview is not ready for installation".into());
        }
        let manifest = self.preview_manifest(session_id, snapshot.revision)?;
        if manifest.get("petId").and_then(serde_json::Value::as_str) != Some(pet_id)
            || manifest
                .get("variantId")
                .and_then(serde_json::Value::as_str)
                != Some(variant_id)
        {
            return Err("frame sequence preview identity does not match finalization".into());
        }
        self.builder
            .install_preview(session_id, snapshot.revision, destination)
    }

    fn cleanup_after_accept(
        &self,
        session_id: &str,
    ) -> Result<PhotoAvatarCleanupDisposition, String> {
        let snapshot = self.store.frame_snapshot(session_id)?;
        self.store.delete_sources(session_id)?;
        self.store.set_frame_step(
            session_id,
            snapshot.revision,
            FramePhotoAvatarStep::Completed,
        )?;
        Ok(PhotoAvatarCleanupDisposition::Complete)
    }

    fn restore_preview_after_abort(&self, session_id: &str) -> Result<(), String> {
        let snapshot = self.store.frame_snapshot(session_id)?;
        self.store.set_frame_step(
            session_id,
            snapshot.revision,
            FramePhotoAvatarStep::PreviewReady,
        )
    }
}

impl PhotoAvatarAbandonPort for FramePhotoAvatarManager {
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
