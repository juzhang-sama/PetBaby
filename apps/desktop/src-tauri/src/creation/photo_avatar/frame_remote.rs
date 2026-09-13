//! 写实风（`frame-video-v1`）两个 step 的执行层。
//!
//! 重试策略与轮询都在 `remote_common`（**两条产线共用一处**）；这里只接上帧路线自己的
//! 三件事：提交哪个请求、找谁要下一个 attempt、把哪个 job 落库。
//!
//! 轮询节奏用 `RemotePolling::FRAME`（900s / 1s），理由见 `remote_common` 的模块注释 ——
//! 简单说：`packFrameSequence` 实测 136 秒，300s 会偶发超时，
//! 而 250ms 的间隔会为一次打包白打几百次状态请求。

use super::domain::FrameRemoteStep;
use super::provider::{ControlledBackendProvider, FrameProviderStepRequest, RemoteJobState};
use super::remote_common::{run_remote_step_with, RemotePolling, RemoteStepFailure};
use super::store::{PhotoAvatarStore, RemoteJob};

pub(super) type FrameRemoteFailure = RemoteStepFailure;

pub(super) fn run_frame_step(
    store: &PhotoAvatarStore,
    provider: &ControlledBackendProvider,
    session_id: &str,
    revision: u32,
    request: FrameProviderStepRequest,
) -> Result<(RemoteJob, RemoteJobState, u8), FrameRemoteFailure> {
    let attempt = request.attempt;
    let step = request.step;
    let mut outgoing = request;
    run_remote_step_with(
        provider,
        attempt,
        |attempt| {
            outgoing.attempt = attempt;
            provider.submit_frame_step(outgoing.clone())
        },
        || {
            store
                .reserve_frame_attempt(session_id, revision, step)
                .map_err(RemoteStepFailure::from)
        },
        |job| {
            store
                .set_frame_provider_job(
                    session_id,
                    revision,
                    job.provider_session_id.as_deref(),
                    Some(&job.provider_job_id),
                )
                .map_err(RemoteStepFailure::from)
        },
        RemotePolling::FRAME,
    )
}
