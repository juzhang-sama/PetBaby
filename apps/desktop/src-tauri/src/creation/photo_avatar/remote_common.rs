//! 两条产线**共用**的远端 step 执行机制：提交 → 轮询 → 按需换 attempt 重来。
//!
//! `pixel_remote.rs` 与 `frame_remote.rs` 只差三件事：提交哪个请求、找谁要下一个 attempt、
//! 以及轮询节奏（deadline / interval）。**重试策略本身必须只有一处** ——
//! 「可重试就换 attempt 重来、不可重试立刻上报、attempt 上限 3」这条规则要是各写一份，
//! 哪天给一条产线加了抖动或改了退避，另一条不会有，而两段代码长得一模一样、
//! review 时根本看不出来。
//!
//! 轮询节奏**按产线给**：像素风一个 step 几十秒，帧路线的 `packFrameSequence` 实测
//! **135.7 / 136.6 / 145.8 秒**。拿同一套 250ms / 300s 套两条线，
//! 前者是打几千次状态请求（纯浪费），后者会偶发超时（代价是白等两分钟再重来）。

use super::domain::PhotoAvatarErrorCode;
use super::provider::{
    ControlledBackendProvider, PhotoAvatarError, PhotoAvatarProvider, ProviderSourceImage,
    RemoteJobState,
};
use super::store::{NormalizedPhoto, RemoteJob};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::time::{Duration, Instant};

/// 归一化照片 → wire 形态。两条产线的请求体里这部分的形状是一样的
/// （`sourceId` / `pngBase64` / `sha256` / `width` / `height`）。
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

/// 一条产线的轮询节奏。
#[derive(Debug, Clone, Copy)]
pub(super) struct RemotePolling {
    pub deadline: Duration,
    pub interval: Duration,
}

impl RemotePolling {
    /// 像素风：一个 step 几十秒量级，跟着旧行为走。
    pub(super) const PIXEL: Self = Self {
        deadline: Duration::from_secs(300),
        interval: Duration::from_millis(250),
    };

    /// 写实风：`packFrameSequence` 实测 136s 上下，900s 留足余量；
    /// 间隔 1s —— 这条链每个 step 都是分钟级，250ms 只会白打几千次请求。
    pub(super) const FRAME: Self = Self {
        deadline: Duration::from_secs(900),
        interval: Duration::from_millis(1000),
    };
}

/// 远端 step 的失败态。`code` / `retryable` 决定上层要不要重试、给用户看什么。
#[derive(Debug)]
pub(super) struct RemoteStepFailure {
    pub code: PhotoAvatarErrorCode,
    pub retryable: bool,
    pub message: String,
}

impl From<RemoteStepFailure> for String {
    fn from(failure: RemoteStepFailure) -> Self {
        failure.message
    }
}

impl From<String> for RemoteStepFailure {
    fn from(message: String) -> Self {
        Self {
            code: PhotoAvatarErrorCode::TemporaryUnavailable,
            retryable: false,
            message,
        }
    }
}

impl From<PhotoAvatarError> for RemoteStepFailure {
    fn from(error: PhotoAvatarError) -> Self {
        Self {
            code: error.code,
            retryable: error.retryable,
            message: error.message,
        }
    }
}

/// 提交一步并轮询到终态；可重试的失败就换一个 attempt 重来（上限 3）。
///
/// 四个回调是为了让两条产线共用这一段控制流：
/// - `submit(attempt)`：发请求（调用方自己把 `attempt` 写进请求体）；
/// - `reserve_next()`：要下一个 attempt（同时是「这个 step 还允许重试吗」的闸口）；
/// - `record_job(job)`：把 provider 给的 job/session id 落库。
pub(super) fn run_remote_step_with<S, R, J>(
    provider: &ControlledBackendProvider,
    mut attempt: u8,
    mut submit: S,
    mut reserve_next: R,
    mut record_job: J,
    polling: RemotePolling,
) -> Result<(RemoteJob, RemoteJobState, u8), RemoteStepFailure>
where
    S: FnMut(u8) -> Result<RemoteJob, PhotoAvatarError>,
    R: FnMut() -> Result<u8, RemoteStepFailure>,
    J: FnMut(&RemoteJob) -> Result<(), RemoteStepFailure>,
{
    loop {
        let job = match submit(attempt) {
            Ok(job) => job,
            Err(error) if error.retryable && attempt < 3 => {
                attempt = reserve_next()?;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        record_job(&job)?;
        match poll_remote_job(provider, &job.provider_job_id, polling) {
            Ok(state) => return Ok((job, state, attempt)),
            Err(error) if error.retryable && attempt < 3 => {
                attempt = reserve_next()?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn poll_remote_job(
    provider: &ControlledBackendProvider,
    job_id: &str,
    polling: RemotePolling,
) -> Result<RemoteJobState, RemoteStepFailure> {
    let deadline = Instant::now() + polling.deadline;
    loop {
        let state = provider.poll_job(job_id).map_err(RemoteStepFailure::from)?;
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
                return Err(RemoteStepFailure {
                    code,
                    retryable,
                    message,
                });
            }
            "running" if Instant::now() < deadline => std::thread::sleep(polling.interval),
            "running" => {
                return Err(RemoteStepFailure {
                    code: PhotoAvatarErrorCode::Timeout,
                    retryable: true,
                    message: "photo avatar provider timed out".into(),
                })
            }
            _ => {
                return Err(RemoteStepFailure {
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
