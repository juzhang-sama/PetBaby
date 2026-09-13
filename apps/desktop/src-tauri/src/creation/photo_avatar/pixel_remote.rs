use super::domain::PixelRemoteStep;
use super::provider::{
    ControlledBackendProvider, PixelProviderStepRequest, ProviderSourceImage, RemoteJobState,
};
use super::remote_common::{run_remote_step_with, RemotePolling, RemoteStepFailure};
use super::store::{PhotoAvatarStore, RemoteJob};

/// 失败态的类型没变（就是 `RemoteStepFailure`），保留这个别名，
/// `pixel_manager.rs` 与测试一行都不用动。
pub(super) use super::remote_common::RemoteStepFailure as PixelRemoteFailure;
/// wire 形态转换也是两条产线共用的，放在 `remote_common`。
pub(super) use super::remote_common::provider_images;

pub(super) fn run_remote_step(
    store: &PhotoAvatarStore,
    provider: &ControlledBackendProvider,
    session_id: &str,
    revision: u32,
    request: PixelProviderStepRequest,
) -> Result<(RemoteJob, RemoteJobState, u8), PixelRemoteFailure> {
    let attempt = request.attempt;
    let step = request.step;
    let mut outgoing = request;
    run_remote_step_with(
        provider,
        attempt,
        |attempt| {
            outgoing.attempt = attempt;
            provider.submit_pixel_step(outgoing.clone())
        },
        || {
            store
                .reserve_pixel_attempt(session_id, revision, step)
                .map_err(RemoteStepFailure::from)
        },
        |job| {
            store
                .set_pixel_provider_job(
                    session_id,
                    revision,
                    job.provider_session_id.as_deref(),
                    Some(&job.provider_job_id),
                )
                .map_err(RemoteStepFailure::from)
        },
        RemotePolling::PIXEL,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::photo_avatar::domain::{
        PhotoAvatarErrorCode, PixelRemoteStep, PixelStyleProfileId, PHOTO_AVATAR_CONSENT_VERSION,
    };
    use crate::storage::Storage;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn test_store() -> (PhotoAvatarStore, std::path::PathBuf, u32) {
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-pixel-remote-{}",
            crate::creation::domain::new_entity_id("remote")
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
            .begin_pixel_revision("session-a", PixelStyleProfileId::V2AnimationReady, None, &[])
            .unwrap()
            .revision;
        (store, root, revision)
    }

    fn request(revision: u32, attempt: u8) -> PixelProviderStepRequest {
        PixelProviderStepRequest {
            route: "pixel-v1".into(),
            style_profile_id: PixelStyleProfileId::V2AnimationReady,
            session_id: "session-a".into(),
            revision,
            provider_session_id: None,
            step: PixelRemoteStep::AnalyzeIdentity,
            attempt,
            consent_version: PHOTO_AVATAR_CONSENT_VERSION.into(),
            source_images: vec![ProviderSourceImage {
                source_id: "source-0".into(),
                png_base64: "iVBORw0KGgo=".into(),
                sha256: "00".repeat(32),
                width: 256,
                height: 256,
            }],
            profile: None,
            modification: None,
            locked_traits: vec![],
        }
    }

    async fn mount_failed_job(server: &MockServer, code: &str, message: &str, count: u64) {
        Mock::given(method("POST"))
            .and(path("/v1/photo-avatar/steps"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jobId":"job1","providerSessionId":"remote1"})),
            )
            .expect(count)
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/photo-avatar/jobs/job1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": "failed",
                "result": null,
                "error": {"code": code, "message": message}
            })))
            .expect(count)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn invalid_input_failure_is_submitted_once_and_preserves_safe_message() {
        let server = MockServer::start().await;
        let message = "生成图片不符合像素素材要求，请重试。";
        mount_failed_job(&server, "invalidInput", message, 1).await;
        let (store, root, revision) = test_store();
        let attempt = store
            .reserve_pixel_attempt("session-a", revision, PixelRemoteStep::AnalyzeIdentity)
            .unwrap();
        let provider = ControlledBackendProvider::for_test(&server.uri());

        let error = tokio::task::spawn_blocking(move || {
            run_remote_step(
                &store,
                &provider,
                "session-a",
                revision,
                request(revision, attempt),
            )
            .unwrap_err()
        })
        .await
        .unwrap();

        assert_eq!(error.code, PhotoAvatarErrorCode::InvalidInput);
        assert_eq!(error.message, message);
        server.verify().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn network_failure_retries_three_total_attempts_and_preserves_safe_message() {
        let server = MockServer::start().await;
        let message = "网络连接失败，请稍后重试。";
        mount_failed_job(&server, "network", message, 3).await;
        let (store, root, revision) = test_store();
        let attempt = store
            .reserve_pixel_attempt("session-a", revision, PixelRemoteStep::AnalyzeIdentity)
            .unwrap();
        let provider = ControlledBackendProvider::for_test(&server.uri());

        let error = tokio::task::spawn_blocking(move || {
            run_remote_step(
                &store,
                &provider,
                "session-a",
                revision,
                request(revision, attempt),
            )
            .unwrap_err()
        })
        .await
        .unwrap();

        assert_eq!(error.code, PhotoAvatarErrorCode::Network);
        assert_eq!(error.message, message);
        server.verify().await;
        let _ = std::fs::remove_dir_all(root);
    }
}
