//! 把两条产线的 port impl 合成一个「按 route 分派」的 port。
//!
//! `CreationFinalizationService` 与 `CreationService` 各自只认**一个** port
//! （`Arc<dyn PhotoAvatarFinalizationPort>` / `Arc<dyn PhotoAvatarAbandonPort>`），
//! 而两条产线的预览目录与状态机是分开的 —— 所以「分派」这件事只能落在这一层：
//! 上游只给一个 `session_id`，这里问库「它在哪条路上」，再转给对应的 manager。
//!
//! ⚠️ 这一层**只做分派，不做判断**：没有 run（`Ok(None)`）时按**默认产线**（像素风）走，
//! 与 `begin` 没给 route 时的默认保持同一个口径。
//!
//! `live2d-v5` 是已冻结的历史路线，它有自己的 manager（`manager.rs`），
//! 从来没挂到这两个 port 上 —— 这里给它一条明确的错误，而不是让像素 manager
//! 回一句 `pixel avatar run does not exist` 让人以为数据坏了。

use super::domain::PhotoAvatarRoute;
use super::frame_manager::SharedFramePhotoAvatarManager;
use super::pixel_manager::SharedPixelPhotoAvatarManager;
use super::store::PhotoAvatarStore;
use crate::creation::finalization::{PhotoAvatarCleanupDisposition, PhotoAvatarFinalizationPort};
use crate::creation::service::PhotoAvatarAbandonPort;
use std::path::Path;

pub struct RoutePhotoAvatarPorts {
    store: PhotoAvatarStore,
    pixel: SharedPixelPhotoAvatarManager,
    frame: SharedFramePhotoAvatarManager,
}

impl RoutePhotoAvatarPorts {
    pub fn new(
        store: PhotoAvatarStore,
        pixel: SharedPixelPhotoAvatarManager,
        frame: SharedFramePhotoAvatarManager,
    ) -> Self {
        Self {
            store,
            pixel,
            frame,
        }
    }

    fn route(&self, session_id: &str) -> Result<PhotoAvatarRoute, String> {
        Ok(self
            .store
            .session_route(session_id)?
            .unwrap_or(PhotoAvatarRoute::Pixel))
    }

    fn legacy_route_error() -> String {
        "legacy photo avatar route has no shared preview port".into()
    }
}

impl PhotoAvatarFinalizationPort for RoutePhotoAvatarPorts {
    fn preview_ready(&self, session_id: &str) -> Result<bool, String> {
        match self.route(session_id)? {
            PhotoAvatarRoute::Live2d => Err(Self::legacy_route_error()),
            PhotoAvatarRoute::Pixel => self.pixel.preview_ready(session_id),
            PhotoAvatarRoute::Frame => self.frame.preview_ready(session_id),
        }
    }

    fn install_preview(
        &self,
        session_id: &str,
        pet_id: &str,
        variant_id: &str,
        destination: &Path,
    ) -> Result<(), String> {
        match self.route(session_id)? {
            PhotoAvatarRoute::Live2d => Err(Self::legacy_route_error()),
            PhotoAvatarRoute::Pixel => {
                self.pixel
                    .install_preview(session_id, pet_id, variant_id, destination)
            }
            PhotoAvatarRoute::Frame => {
                self.frame
                    .install_preview(session_id, pet_id, variant_id, destination)
            }
        }
    }

    fn cleanup_after_accept(
        &self,
        session_id: &str,
    ) -> Result<PhotoAvatarCleanupDisposition, String> {
        match self.route(session_id)? {
            PhotoAvatarRoute::Live2d => Err(Self::legacy_route_error()),
            PhotoAvatarRoute::Pixel => self.pixel.cleanup_after_accept(session_id),
            PhotoAvatarRoute::Frame => self.frame.cleanup_after_accept(session_id),
        }
    }

    fn restore_preview_after_abort(&self, session_id: &str) -> Result<(), String> {
        match self.route(session_id)? {
            PhotoAvatarRoute::Live2d => Err(Self::legacy_route_error()),
            PhotoAvatarRoute::Pixel => self.pixel.restore_preview_after_abort(session_id),
            PhotoAvatarRoute::Frame => self.frame.restore_preview_after_abort(session_id),
        }
    }
}

impl PhotoAvatarAbandonPort for RoutePhotoAvatarPorts {
    fn cancel_provider_job(&self, session_id: &str, provider_job_id: &str) -> Result<(), String> {
        match self.route(session_id)? {
            PhotoAvatarRoute::Live2d => Err(Self::legacy_route_error()),
            PhotoAvatarRoute::Pixel => self.pixel.cancel_provider_job(session_id, provider_job_id),
            PhotoAvatarRoute::Frame => self.frame.cancel_provider_job(session_id, provider_job_id),
        }
    }

    fn delete_provider_session(
        &self,
        session_id: &str,
        provider_session_id: &str,
    ) -> Result<(), String> {
        match self.route(session_id)? {
            PhotoAvatarRoute::Live2d => Err(Self::legacy_route_error()),
            PhotoAvatarRoute::Pixel => self
                .pixel
                .delete_provider_session(session_id, provider_session_id),
            PhotoAvatarRoute::Frame => self
                .frame
                .delete_provider_session(session_id, provider_session_id),
        }
    }
}
