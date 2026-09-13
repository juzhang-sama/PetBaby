//! 写实风（`frame-video-v1`）的 manager：两个 step 串起来跑，跑完出一个预览目录。
//!
//! 形状照 `pixel_manager.rs` —— **能照抄的都照抄**，包括「后台线程 + 失败落库」这套。
//! 不同的只有三处，都是这条产线本身决定的：
//!
//! 1. **两个 step，顺序固定**：`generateMotionSource`（花钱，出视频）→
//!    `packFrameSequence`（0 算力，出 zip）。第二个 step **不带照片**，
//!    靠第一个 step 拿到的 `providerSessionId` 去找服务侧 scratch 里的 mp4。
//! 2. **没有 profile**：写实风不走 trait 档案，所以没有 `completeAppearance` 那一步，
//!    也没有「改指令」（`revise`）这条路。
//! 3. **跑完停在 `RuntimeCheckPending`**：等用户人工确认（老王三定第 1 条
//!    「跑完弹预览，用户确认才安装」）。与像素风同一个闸口。

use super::domain::{FramePhotoAvatarSnapshot, FramePhotoAvatarStep, FrameRemoteStep, FRAME_ROUTE};
use super::frame_remote::{run_frame_step, FrameRemoteFailure};
use super::provider::{
    ControlledBackendProvider, FrameProviderStepRequest, PhotoAvatarProvider, ProviderStepResult,
};
use super::remote_common::provider_images;
use super::source::{normalize_photo_sources, RawPhotoSource};
use super::store::{NormalizedPhoto, PhotoAvatarStore};
use crate::runtime_assets::frame_sequence_builder::{
    BuildFrameSequenceRequest, FrameSequenceBuilder, FRAME_SEQUENCE_VARIANT_ID,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// zip 在库里的 `relative_path`。一个 job 只有一个 artifact，
/// 名字固定即可（真正的位置由 artifact 表记录）。
const FRAME_SEQUENCE_ARTIFACT: &str = "frame-sequence.zip";

pub type SharedFramePhotoAvatarManager = Arc<FramePhotoAvatarManager>;

pub struct FramePhotoAvatarManager {
    pub(super) store: PhotoAvatarStore,
    pub(super) provider: Option<Arc<ControlledBackendProvider>>,
    pub(super) builder: FrameSequenceBuilder,
    pub(super) preview_root: PathBuf,
}

impl FramePhotoAvatarManager {
    pub fn new(
        store: PhotoAvatarStore,
        provider: Option<Arc<ControlledBackendProvider>>,
        preview_root: &Path,
    ) -> Self {
        Self {
            store,
            provider,
            builder: FrameSequenceBuilder::new(preview_root),
            preview_root: preview_root.to_path_buf(),
        }
    }

    pub fn begin(
        self: &Arc<Self>,
        session_id: &str,
        consent_version: &str,
        sources: Vec<RawPhotoSource>,
    ) -> Result<FramePhotoAvatarSnapshot, String> {
        if consent_version != super::domain::PHOTO_AVATAR_CONSENT_VERSION
            || !self.store.consent_accepted(consent_version)?
        {
            return Err("photo avatar consent is required".into());
        }
        let normalized = normalize_photo_sources(sources).map_err(|error| error.to_string())?;
        self.store.replace_sources(session_id, &normalized)?;
        self.start_revision(session_id, normalized)
    }

    pub fn status(&self, session_id: &str) -> Result<Option<FramePhotoAvatarSnapshot>, String> {
        match self.store.frame_snapshot(session_id) {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(error) if error == "frame video run does not exist" => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// 重新生成 = **换一次 revision 重跑两个 step**。
    ///
    /// 注意这与「重试」不是一回事：`generateMotionSource` 会复用服务侧 scratch 里
    /// 同一 `providerSessionId` 的 mp4，而换 revision 会拿到新的 `providerSessionId`
    /// → **真的会重付一次视频钱**（5.69 算力）。用户点「重新生成」就是要这个效果。
    pub fn regenerate(
        self: &Arc<Self>,
        session_id: &str,
    ) -> Result<FramePhotoAvatarSnapshot, String> {
        self.start_revision(session_id, self.store.sources(session_id)?)
    }

    pub fn cancel(&self, session_id: &str) -> Result<FramePhotoAvatarSnapshot, String> {
        let snapshot = self.store.frame_snapshot(session_id)?;
        self.store.set_frame_step(
            session_id,
            snapshot.revision,
            FramePhotoAvatarStep::Cancelled,
        )?;
        self.store.frame_snapshot(session_id)
    }

    fn start_revision(
        self: &Arc<Self>,
        session_id: &str,
        sources: Vec<NormalizedPhoto>,
    ) -> Result<FramePhotoAvatarSnapshot, String> {
        let run = self.store.begin_frame_revision(session_id)?;
        let manager = Arc::clone(self);
        let session = session_id.to_string();
        std::thread::spawn(move || {
            match manager.run_revision(&session, run.revision, sources) {
                Ok(()) => {}
                Err(RunRevisionFailure::Remote(error)) => {
                    persist_remote_failure(&manager.store, &session, run.revision, error);
                }
                Err(RunRevisionFailure::Local(message)) => {
                    eprintln!("[frame-video] local failure for {session}: {message}");
                    let _ = manager
                        .store
                        .fail_frame_revision_if_active(&session, run.revision);
                }
            }
        });
        self.store.frame_snapshot(session_id)
    }

    fn run_revision(
        &self,
        session_id: &str,
        revision: u32,
        sources: Vec<NormalizedPhoto>,
    ) -> Result<(), RunRevisionFailure> {
        let provider = self
            .provider
            .as_ref()
            .ok_or("photo avatar backend is not configured")?;
        // 身份三件套从库里读：`petId` 会被写进 manifest，安装时要逐字比。
        let identity = self.store.pet_identity(session_id)?;
        let images = provider_images(&sources);

        // ---- 1/2 generateMotionSource：花钱那一步 ----
        let motion_attempt = self.store.reserve_frame_attempt(
            session_id,
            revision,
            FrameRemoteStep::GenerateMotionSource,
        )?;
        let (motion_job, motion_state, _) = run_frame_step(
            &self.store,
            provider,
            session_id,
            revision,
            self.step_request(
                session_id,
                revision,
                FrameRemoteStep::GenerateMotionSource,
                motion_attempt,
                None,
                images,
                &identity.pet_id,
                &identity.display_name,
                &identity.species,
            ),
        )?;
        let Some(ProviderStepResult::MotionSource { .. }) = motion_state.result else {
            return Err("frame motion source result is invalid".into());
        };

        // ---- 2/2 packFrameSequence：0 算力，但慢（实测 136s）----
        self.store.set_frame_step(
            session_id,
            revision,
            FramePhotoAvatarStep::PackFrameSequence,
        )?;
        let pack_attempt = self.store.reserve_frame_attempt(
            session_id,
            revision,
            FrameRemoteStep::PackFrameSequence,
        )?;
        let (_pack_job, pack_state, _) = run_frame_step(
            &self.store,
            provider,
            session_id,
            revision,
            self.step_request(
                session_id,
                revision,
                FrameRemoteStep::PackFrameSequence,
                pack_attempt,
                // 这一步吃的是服务侧 scratch 里的 mp4 —— **不带照片**，
                // 但必须带上第一个 step 给的 providerSessionId。
                motion_job.provider_session_id,
                Vec::new(),
                &identity.pet_id,
                &identity.display_name,
                &identity.species,
            ),
        )?;
        let Some(ProviderStepResult::FrameSequence {
            artifact_url,
            sha256,
            ..
        }) = pack_state.result
        else {
            return Err("frame sequence result is invalid".into());
        };

        let zip = provider
            .download_frame_sequence(&artifact_url, &sha256)
            .map_err(|error| FrameRemoteFailure {
                code: error.code,
                retryable: error.retryable,
                message: error.message,
            })?;
        self.store
            .commit_frame_artifact(session_id, revision, FRAME_SEQUENCE_ARTIFACT, &sha256)?;

        // 装出预览目录。**任何一步失败都不许留下半成品** —— builder 自己会清 staging。
        self.builder.build_preview(BuildFrameSequenceRequest {
            session_id: session_id.into(),
            revision,
            pet_id: identity.pet_id,
            variant_id: FRAME_SEQUENCE_VARIANT_ID.into(),
            zip_bytes: zip,
            zip_sha256: sha256,
        })?;

        self.store.set_frame_step(
            session_id,
            revision,
            FramePhotoAvatarStep::RuntimeCheckPending,
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn step_request(
        &self,
        session_id: &str,
        revision: u32,
        step: FrameRemoteStep,
        attempt: u8,
        provider_session_id: Option<String>,
        source_images: Vec<super::provider::ProviderSourceImage>,
        pet_id: &str,
        display_name: &str,
        species: &str,
    ) -> FrameProviderStepRequest {
        FrameProviderStepRequest {
            route: FRAME_ROUTE.into(),
            session_id: session_id.into(),
            revision,
            provider_session_id,
            step,
            attempt,
            consent_version: super::domain::PHOTO_AVATAR_CONSENT_VERSION.into(),
            source_images,
            pet_id: pet_id.into(),
            display_name: display_name.into(),
            species: species.into(),
        }
    }
}

enum RunRevisionFailure {
    Remote(FrameRemoteFailure),
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

impl From<FrameRemoteFailure> for RunRevisionFailure {
    fn from(failure: FrameRemoteFailure) -> Self {
        Self::Remote(failure)
    }
}

fn persist_remote_failure(
    store: &PhotoAvatarStore,
    session_id: &str,
    revision: u32,
    failure: FrameRemoteFailure,
) -> String {
    let message = failure.message;
    if let Err(error) =
        store.fail_frame_revision_with_error_if_active(session_id, revision, failure.code, &message)
    {
        eprintln!("[frame-video] failed to persist remote failure for {session_id}: {error}");
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::finalization::PhotoAvatarFinalizationPort;
    use crate::creation::photo_avatar::domain::PHOTO_AVATAR_CONSENT_VERSION;
    use crate::creation::photo_avatar::source::RawPhotoSource;
    use crate::storage::Storage;
    use image::{DynamicImage, ImageFormat, RgbaImage};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const MOTIONS: [&str; 9] = [
        "idle",
        "look-left",
        "look-right",
        "react-happy",
        "react-curious",
        "carried",
        "landed",
        "sleep",
        "wake",
    ];
    const FRAME_NAMES: [&str; 2] =
        ["frames/idle-combo/f0000.webp", "frames/idle-combo/f0001.webp"];
    const FRAME_BYTES: [&[u8]; 2] = [b"RIFF0000WEBPfirst", b"RIFF0000WEBPsecond"];

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-frame-manager-{label}-{}",
            crate::creation::domain::new_entity_id("frame")
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// 一来源照片。`RawPhotoSource` 要求真 PNG/JPEG 且 sha256 对得上。
    fn photo() -> RawPhotoSource {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            512,
            512,
            image::Rgba([10, 20, 30, 255]),
        ));
        let mut bytes = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        RawPhotoSource {
            claimed_sha256: sha256_hex(&bytes),
            bytes,
        }
    }

    /// 造一支**能过 `validate_asset_directory`** 的 schema 7 包。
    ///
    /// 与 `frame_sequence_builder` 的测试夹具**有意重复**：测试夹具不跨模块借 ——
    /// 一边改了夹具，另一边失败起来会让人莫名其妙（这条是本仓库既有约定）。
    fn frame_pack(pet_id: &str) -> Vec<u8> {
        let files: Vec<serde_json::Value> = FRAME_NAMES
            .iter()
            .zip(FRAME_BYTES.iter())
            .map(|(name, data)| {
                json!({
                    "role": "frame",
                    "relativePath": name,
                    "sha256": sha256_hex(data),
                })
            })
            .collect();
        let semantics: serde_json::Map<String, serde_json::Value> = MOTIONS
            .iter()
            .map(|motion| {
                (
                    (*motion).to_string(),
                    serde_json::Value::String("idle-combo".into()),
                )
            })
            .collect();
        let manifest = serde_json::to_vec_pretty(&json!({
            "schemaVersion": 7,
            "renderer": "frame-sequence-v1",
            "petId": pet_id,
            "variantId": FRAME_SEQUENCE_VARIANT_ID,
            "displayName": "我的猫",
            "species": "cat",
            "baseImage": FRAME_NAMES[0],
            "defaultAction": "idle-combo",
            "anchorPolicy": "fixed",
            "actions": [{
                "actionId": "idle-combo",
                "loop": true,
                "frameDurationMs": 42,
                "frames": FRAME_NAMES.to_vec(),
            }],
            "semantics": serde_json::Value::Object(semantics),
            "files": files,
        }))
        .unwrap();

        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in FRAME_NAMES.iter().zip(FRAME_BYTES.iter()) {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.start_file("manifest.json", options).unwrap();
        writer.write_all(&manifest).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn seeded_store(root: &Path) -> (PhotoAvatarStore, Arc<Mutex<Storage>>) {
        let storage = Arc::new(Mutex::new(Storage::open(root).unwrap()));
        {
            let storage = storage.lock().unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO pets
                     (pet_id, schema_version, species, identity_mode, creation_method,
                      display_name, lifecycle, created_at, updated_at)
                     VALUES ('pet-a', 1, 'cat', 'realpet', 'upload', '我的猫', 'draft', '10', '10')",
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
        (PhotoAvatarStore::new(Arc::clone(&storage)), storage)
    }

    /// ⚠️ 夹具里的 job id **只用字母数字**：`encode_path_segment` 用的是
    /// `NON_ALPHANUMERIC`，连 `-` 都会编成 `%2D`（轮询路径会变成 `/jobs/job%2Dx`），
    /// mock 按字面路径匹配就对不上。真后端会解码，所以这不是产品 bug，是夹具要注意。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_frame_revision_runs_both_steps_and_lands_on_runtime_check() {
        let server = MockServer::start().await;
        let pack = frame_pack("pet-a");
        let pack_sha = sha256_hex(&pack);
        let artifact_url = format!("{}/artifact.zip", server.uri());

        // 两个 step 的提交**按请求体里的 step 区分**（不靠注册顺序，读起来也更清楚）。
        Mock::given(method("POST"))
            .and(path("/v1/photo-avatar/steps"))
            .and(body_partial_json(json!({"step": "generateMotionSource"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"jobId": "jobmotion", "providerSessionId": "remote-1"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/photo-avatar/steps"))
            .and(body_partial_json(json!({"step": "packFrameSequence"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"jobId": "jobpack", "providerSessionId": "remote-1"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/photo-avatar/jobs/jobmotion"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": "succeeded",
                "error": null,
                "result": {
                    "resultType": "motionSource",
                    "videoBytes": 3145728,
                    "reused": false,
                    "masterTaskId": null,
                    "videoTaskId": "task-video-1",
                    "firstFrameScale": 0.85,
                    "firstFrameLeftMargin": 0.06,
                    "firstFrameRightMargin": null,
                },
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/photo-avatar/jobs/jobpack"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": "succeeded",
                "error": null,
                "result": {
                    "resultType": "frameSequence",
                    "artifactUrl": artifact_url,
                    "sha256": pack_sha,
                    "frameCount": 2,
                    "frameDurationMs": 42,
                    "frameFormat": "webp",
                    "overallPassed": true,
                    "failedCriteria": [],
                },
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/artifact.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(pack))
            .mount(&server)
            .await;

        let root = temp_root("ok");
        let (store, _storage) = seeded_store(&root);
        let manager = Arc::new(FramePhotoAvatarManager::new(
            store,
            Some(Arc::new(ControlledBackendProvider::for_test(&server.uri()))),
            &root.join("previews"),
        ));
        manager.save_consent(true).unwrap();

        let started = manager
            .begin("session-a", PHOTO_AVATAR_CONSENT_VERSION, vec![photo()])
            .unwrap();
        assert_eq!(started.step, FramePhotoAvatarStep::GenerateMotionSource);
        assert_eq!(started.revision, 1);

        let deadline = Instant::now() + Duration::from_secs(30);
        let settled = loop {
            let snapshot = manager.status("session-a").unwrap().unwrap();
            if matches!(
                snapshot.step,
                FramePhotoAvatarStep::RuntimeCheckPending | FramePhotoAvatarStep::Failed
            ) {
                break snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "帧路线没跑到人工确认那一步：step={:?} attempts={:?}",
                snapshot.step,
                snapshot.attempts
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            settled.step,
            FramePhotoAvatarStep::RuntimeCheckPending,
            "失败信息：{:?}；收到的请求：{:?}",
            settled.error_message,
            server
                .received_requests()
                .await
                .unwrap_or_default()
                .iter()
                .map(|request| format!("{} {}", request.method, request.url.path()))
                .collect::<Vec<_>>()
        );
        // 两个 step 各占了一次 attempt。
        assert_eq!(
            settled.attempts.get(&FrameRemoteStep::GenerateMotionSource),
            Some(&1)
        );
        assert_eq!(
            settled.attempts.get(&FrameRemoteStep::PackFrameSequence),
            Some(&1)
        );

        // 预览目录真的躺在磁盘上，而且是 zip 里那份内容。
        let preview = root.join("previews").join("session-a").join("1");
        assert!(preview.join("manifest.json").is_file());
        assert!(preview.join(FRAME_NAMES[0]).is_file());
        assert!(preview.join(FRAME_NAMES[1]).is_file());
        assert_eq!(
            std::fs::read(preview.join(FRAME_NAMES[0])).unwrap(),
            FRAME_BYTES[0]
        );

        // 人工确认 → PreviewReady → 安装到别处。
        let manifest_bytes = std::fs::read(preview.join("manifest.json")).unwrap();
        let confirmed = manager
            .runtime_check_passed("session-a", 1, &sha256_hex(&manifest_bytes))
            .unwrap();
        assert_eq!(confirmed.step, FramePhotoAvatarStep::PreviewReady);

        let destination = root.join("installed");
        PhotoAvatarFinalizationPort::install_preview(
            manager.as_ref(),
            "session-a",
            "pet-a",
            FRAME_SEQUENCE_VARIANT_ID,
            &destination,
        )
        .unwrap();
        assert!(destination.join("manifest.json").is_file());
        assert!(destination.join(FRAME_NAMES[1]).is_file());

        server.verify().await;
        let _ = std::fs::remove_dir_all(root);
    }

    /// 预览路径的遍历防护 —— 这条**必须与像素风不同**（帧路径天生带子目录），
    /// 所以两边都得有自己的测试。
    #[test]
    fn frame_preview_paths_allow_subdirectories_but_never_escape() {
        let root = temp_root("paths");
        let (store, _storage) = seeded_store(&root);
        let manager = FramePhotoAvatarManager::new(store, None, &root.join("previews"));
        let preview = root.join("previews").join("session-a").join("1");
        std::fs::create_dir_all(&preview).unwrap();
        std::fs::write(preview.join("manifest.json"), b"{}").unwrap();
        std::fs::create_dir_all(preview.join("frames").join("idle-combo")).unwrap();
        std::fs::write(preview.join(FRAME_NAMES[0]), FRAME_BYTES[0]).unwrap();

        // 合法：带子目录的相对路径要能读到。
        assert_eq!(
            manager.preview_file("session-a", 1, "manifest.json").unwrap(),
            b"{}"
        );
        assert_eq!(
            manager
                .preview_file("session-a", 1, FRAME_NAMES[0])
                .unwrap(),
            FRAME_BYTES[0].to_vec()
        );

        // 非法：一律拒，且错误文案统一（不泄漏探测结果）。
        for unsafe_path in [
            "",
            "/etc/passwd",
            "../manifest.json",
            "frames/../../manifest.json",
            "frames/idle-combo/../../../manifest.json",
            "frames//f0000.webp",
            "frames/./f0000.webp",
            "frames\\idle-combo\\f0000.webp",
            "C:/Windows/win.ini",
        ] {
            assert_eq!(
                manager.preview_file("session-a", 1, unsafe_path).unwrap_err(),
                "invalid frame sequence preview path",
                "该拒的没拒: {unsafe_path:?}"
            );
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn status_is_none_before_the_first_revision() {
        let root = temp_root("status");
        let (store, _storage) = seeded_store(&root);
        let manager = FramePhotoAvatarManager::new(store, None, &root.join("previews"));

        assert!(manager.status("session-a").unwrap().is_none());
        // 没有预览目录时读 manifest 要报错，不能装成成功（内存里没缓存可骗）。
        assert!(manager.preview_manifest("session-a", 1).is_err());

        let _ = std::fs::remove_dir_all(root);
    }
}
