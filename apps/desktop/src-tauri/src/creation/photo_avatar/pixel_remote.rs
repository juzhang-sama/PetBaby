use super::domain::PhotoAvatarErrorCode;
use super::provider::{
    ControlledBackendProvider, PhotoAvatarProvider, PixelProviderStepRequest, ProviderSourceImage,
    RemoteJobState,
};
use super::store::{NormalizedPhoto, PhotoAvatarStore, RemoteJob};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::time::{Duration, Instant};

pub(super) fn provider_images(sources: &[NormalizedPhoto]) -> Vec<ProviderSourceImage> {
    sources
        .iter()
        .map(|source| ProviderSourceImage {
            source_id: source.source_id.clone(),
            png_base64: STANDARD.encode(&source.normalized_png),
            sha256: source.sha256.clone(),
            width: source.width,
            height: source.height,
        })
        .collect()
}

pub(super) fn run_remote_step(
    store: &PhotoAvatarStore,
    provider: &ControlledBackendProvider,
    session_id: &str,
    revision: u32,
    mut request: PixelProviderStepRequest,
) -> Result<(RemoteJob, RemoteJobState, u8), PixelRemoteFailure> {
    loop {
        let attempt = request.attempt;
        let job = match provider.submit_pixel_step(request.clone()) {
            Ok(job) => job,
            Err(error) if error.retryable && attempt < 3 => {
                request.attempt =
                    store.reserve_pixel_attempt(session_id, revision, request.step)?;
                continue;
            }
            Err(error) => {
                return Err(PixelRemoteFailure {
                    code: error.code,
                    retryable: error.retryable,
                    message: error.message,
                })
            }
        };
        store.set_pixel_provider_job(
            session_id,
            revision,
            job.provider_session_id.as_deref(),
            Some(&job.provider_job_id),
        )?;
        match poll(provider, &job.provider_job_id) {
            Ok(state) => return Ok((job, state, attempt)),
            Err(error) if error.retryable && attempt < 3 => {
                request.attempt =
                    store.reserve_pixel_attempt(session_id, revision, request.step)?;
            }
            Err(error) => return Err(error),
        }
    }
}

#[derive(Debug)]
pub(super) struct PixelRemoteFailure {
    pub code: PhotoAvatarErrorCode,
    pub retryable: bool,
    pub message: String,
}

impl From<PixelRemoteFailure> for String {
    fn from(failure: PixelRemoteFailure) -> Self {
        failure.message
    }
}

impl From<String> for PixelRemoteFailure {
    fn from(message: String) -> Self {
        Self {
            code: PhotoAvatarErrorCode::TemporaryUnavailable,
            retryable: false,
            message,
        }
    }
}

fn poll(
    provider: &ControlledBackendProvider,
    job_id: &str,
) -> Result<RemoteJobState, PixelRemoteFailure> {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let state = provider
            .poll_job(job_id)
            .map_err(|error| PixelRemoteFailure {
                code: error.code,
                retryable: error.retryable,
                message: error.message,
            })?;
        match state.state.as_str() {
            "succeeded" => return Ok(state),
            "failed" => {
                let (code, retryable, message) = state.error.as_ref().map_or_else(
                    || {
                        (
                            PhotoAvatarErrorCode::TemporaryUnavailable,
                            false,
                            "photo avatar provider failed".into(),
                        )
                    },
                    |error| {
                        (
                            remote_error_code(&error.code),
                            matches!(
                                error.code.as_str(),
                                "network" | "timeout" | "provider5xx" | "temporaryUnavailable"
                            ),
                            error.message.clone(),
                        )
                    },
                );
                return Err(PixelRemoteFailure {
                    code,
                    retryable,
                    message,
                });
            }
            "running" if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(250))
            }
            "running" => {
                return Err(PixelRemoteFailure {
                    code: PhotoAvatarErrorCode::Timeout,
                    retryable: true,
                    message: "photo avatar provider timed out".into(),
                })
            }
            _ => {
                return Err(PixelRemoteFailure {
                    code: PhotoAvatarErrorCode::InvalidInput,
                    retryable: false,
                    message: "photo avatar provider state is invalid".into(),
                })
            }
        }
    }
}

fn remote_error_code(code: &str) -> PhotoAvatarErrorCode {
    match code {
        "invalidInput" => PhotoAvatarErrorCode::InvalidInput,
        "auth" => PhotoAvatarErrorCode::Auth,
        "quota" => PhotoAvatarErrorCode::Quota,
        "contentPolicy" => PhotoAvatarErrorCode::ContentPolicy,
        "unsupported" => PhotoAvatarErrorCode::Unsupported,
        "network" => PhotoAvatarErrorCode::Network,
        "timeout" => PhotoAvatarErrorCode::Timeout,
        "provider5xx" => PhotoAvatarErrorCode::Provider5xx,
        "temporaryUnavailable" => PhotoAvatarErrorCode::TemporaryUnavailable,
        "localStorage" => PhotoAvatarErrorCode::LocalStorage,
        _ => PhotoAvatarErrorCode::InvalidInput,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::photo_avatar::domain::{
        PixelRemoteStep, PixelStyleProfileId, PHOTO_AVATAR_CONSENT_VERSION,
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
            .begin_pixel_revision("session-a", PixelStyleProfileId::V1, None, &[])
            .unwrap()
            .revision;
        (store, root, revision)
    }

    fn request(revision: u32, attempt: u8) -> PixelProviderStepRequest {
        PixelProviderStepRequest {
            route: "pixel-v1".into(),
            style_profile_id: PixelStyleProfileId::V1,
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
