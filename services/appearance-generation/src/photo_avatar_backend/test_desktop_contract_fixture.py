"""真实 FastAPI 与 Rust 桌面端之间的跨进程合同夹具。"""

from __future__ import annotations

from contextlib import contextmanager
from io import BytesIO
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time
from typing import Iterator

import httpx
from PIL import Image

from .app import PipelineRunner, create_app
from .config import BackendConfig
from .job_store import JobStore
from .lk888_client import Lk888Client


REPO_ROOT = Path(__file__).resolve().parents[4]
RUST_CRATE = REPO_ROOT / "apps" / "desktop" / "src-tauri"
MODULE_ROOT = REPO_ROOT / "apps" / "desktop" / "public" / "cat-character-modules" / "cat-a-live2d-v1"
EVIDENCE_ROOT = REPO_ROOT / ".omo" / "evidence" / "task-10"
BACKEND_TOKEN = "desktop-contract-fixture-token"
TRAIT_KEYS = (
    "faceShape", "faceProportions", "furColors", "markings", "eyeShape", "eyeColor",
    "earShape", "bodyType", "tail", "signatureMarks", "temperament",
)


def _fixture_profile() -> dict[str, object]:
    return {
        "schemaVersion": 1,
        "species": "cat",
        "style": "animated-film-soft-v1",
        "bodyModuleId": "body-balanced-v1",
        "bodyModuleSource": "user",
        "traits": [
            {
                "key": key,
                "value": f"fixture {key}",
                "source": "user",
                "evidencePhotoIds": ["photo-fixture"],
            }
            for key in TRAIT_KEYS
        ],
        "completionSummary": [],
    }


def _fixture_atlas() -> bytes:
    image = Image.new("RGB", (2048, 2048), (42, 91, 137))
    output = BytesIO()
    image.save(output, format="PNG")
    return output.getvalue()


class _FakeLk888Transport:
    """只替换上游传输层，保留真实 Lk888Client。"""

    def __init__(self, metrics_path: Path):
        self.metrics_path = metrics_path
        self.calls: list[dict[str, str]] = []
        self.atlas = _fixture_atlas()
        self._persist()

    def __call__(self, request: httpx.Request) -> httpx.Response:
        model = ""
        prompt = ""
        if request.content:
            body = json.loads(request.content)
            model = str(body.get("model", ""))
            prompt = str(body.get("prompt", ""))
            if request.url.path == "/v1/chat/completions":
                content = body["messages"][0]["content"]
                prompt = content if isinstance(content, str) else str(content[0]["text"])
        self.calls.append({"method": request.method, "path": request.url.path, "model": model})
        self._persist()

        if request.url.path == "/v1/chat/completions":
            result = _fixture_profile() if "Do not infer" in prompt else {
                "requestedTraitKeys": [],
                "completedTraits": [],
                "bodyModuleId": "body-balanced-v1",
                "bodyModuleSource": "user",
            }
            return httpx.Response(
                200,
                json={"choices": [{"message": {"content": json.dumps(result)}}]},
                request=request,
            )
        if request.url.path == "/v1/media/generate":
            return httpx.Response(
                200,
                json={"code": 200, "msg": "created", "data": {"task_id": 12345}},
                request=request,
            )
        if request.url.path == "/v1/skills/task-status":
            return httpx.Response(200, json={
                "task_id": 12345, "state": "success", "is_final": True,
                "result_url": "https://cdn.lk888.ai/fake-atlas.png", "error": None,
                "status": "completed", "progress": "100%",
            }, request=request)
        if request.url.host == "cdn.lk888.ai":
            return httpx.Response(
                200, content=self.atlas, headers={"content-type": "image/png"}, request=request
            )
        return httpx.Response(404, request=request)

    def _persist(self) -> None:
        generation = [
            call for call in self.calls
            if call["path"] in {"/v1/chat/completions", "/v1/media/generate"}
        ]
        self.metrics_path.write_text(json.dumps({
            "generationCalls": len(generation),
            "calls": self.calls,
            "models": [call["model"] for call in generation],
        }, ensure_ascii=False, indent=2), encoding="utf-8")


def _serve(port: int, state_dir: Path, metrics_path: Path) -> None:
    import uvicorn

    config = BackendConfig(
        lk888_api_key="fake-provider-token", backend_token=BACKEND_TOKEN,
        host="127.0.0.1", port=port, state_dir=state_dir,
    )
    transport = _FakeLk888Transport(metrics_path)
    runner = PipelineRunner(config)
    runner.client = Lk888Client(config, httpx.Client(transport=httpx.MockTransport(transport)))
    uvicorn.run(
        create_app(config, JobStore(state_dir, runner=runner)),
        host=config.host, port=config.port, log_level="info",
    )


def _available_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


@contextmanager
def _backend_process(root: Path) -> Iterator[tuple[subprocess.Popen[str], str, Path, Path]]:
    state_dir = root / "state"
    metrics_path = root / "fake-lk888-metrics.json"
    address = f"http://127.0.0.1:{_available_port()}"
    port = int(address.rsplit(":", 1)[1])
    log_path = root / "backend.log"
    environment = os.environ.copy()
    environment["PYTHONPATH"] = str(Path(__file__).resolve().parents[1])
    log = log_path.open("w", encoding="utf-8")
    process = subprocess.Popen(
        [sys.executable, "-m", "photo_avatar_backend.test_desktop_contract_fixture",
         "--serve", str(port), str(state_dir), str(metrics_path)],
        cwd=REPO_ROOT, env=environment, stdout=log, stderr=subprocess.STDOUT, text=True,
    )
    try:
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise AssertionError(f"FastAPI fixture exited early; see {log_path}")
            try:
                if httpx.get(f"{address}/healthz", timeout=0.5).status_code == 200:
                    break
            except httpx.HTTPError:
                time.sleep(0.05)
        else:
            raise AssertionError(f"FastAPI fixture did not become healthy; see {log_path}")
        yield process, address, state_dir, metrics_path
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        log.close()


def _copy_rust_crate(destination: Path) -> Path:
    copied = destination / "src-tauri"
    shutil.copytree(RUST_CRATE, copied, ignore=shutil.ignore_patterns("target", ".git", "*.log"))
    # tauri-build validates configured resource roots before compiling tests.
    (destination / "public" / "creation-content").mkdir(parents=True)
    (destination / "public" / "cat-character-modules").mkdir(parents=True)
    provider = copied / "src" / "creation" / "photo_avatar" / "provider.rs"
    with provider.open("a", encoding="utf-8", newline="\n") as output:
        output.write(_RUST_FIXTURE)
    return copied


def _run_rust_fixture(address: str, root: Path) -> subprocess.CompletedProcess[str]:
    crate = _copy_rust_crate(root)
    environment = os.environ.copy()
    environment.update({
        "PHOTO_AVATAR_BACKEND_BASE_URL": address,
        "PHOTO_AVATAR_BACKEND_TOKEN": BACKEND_TOKEN,
        "PHOTO_AVATAR_ALLOW_INSECURE_LOOPBACK": "1",
        "PHOTO_AVATAR_FIXTURE_MODULE_ROOT": str(MODULE_ROOT),
        "PHOTO_AVATAR_FIXTURE_PREVIEW_ROOT": str(root / "previews"),
        "CARGO_TARGET_DIR": str(RUST_CRATE / "target"),
    })
    return subprocess.run(
        ["cargo", "test", "desktop_contract_fixture::controlled_backend_drives_three_steps_artifact_builder_and_delete",
         "--manifest-path", str(crate / "Cargo.toml"), "--lib", "--", "--nocapture"],
        cwd=crate, env=environment, capture_output=True, text=True, timeout=110, check=False,
    )


def test_real_fastapi_and_rust_controlled_backend_contract(tmp_path: Path) -> None:
    EVIDENCE_ROOT.mkdir(parents=True, exist_ok=True)
    with _backend_process(tmp_path) as (process, address, state_dir, metrics_path):
        health = httpx.get(f"{address}/healthz", timeout=2)
        unauthorized = httpx.post(f"{address}/v1/photo-avatar/steps", json={}, timeout=2)
        empty_delete = httpx.delete(
            f"{address}/v1/photo-avatar/sessions/probe-empty-session",
            headers={"Authorization": f"Bearer {BACKEND_TOKEN}"}, timeout=2,
        )
        metrics_before = json.loads(metrics_path.read_text(encoding="utf-8"))
        images_before = [
            str(path.relative_to(state_dir)) for path in state_dir.rglob("*")
            if path.is_file() and path.suffix.lower() in {".png", ".jpg", ".jpeg", ".webp"}
        ]
        probe = {
            "pid": process.pid, "loopbackAddress": address,
            "health": {"status": health.status_code, "body": health.json()},
            "unauthorized": {"status": unauthorized.status_code, "body": unauthorized.json()},
            "emptySessionDelete": {"status": empty_delete.status_code, "body": empty_delete.json()},
            "generationCalls": metrics_before["generationCalls"],
            "stateDir": str(state_dir), "stateDirImages": images_before,
        }
        (EVIDENCE_ROOT / "03-no-photo-probe.json").write_text(
            json.dumps(probe, ensure_ascii=False, indent=2), encoding="utf-8"
        )
        assert probe["health"] == {"status": 200, "body": {"status": "ok"}}
        assert probe["unauthorized"]["status"] == 401
        assert probe["emptySessionDelete"] == {"status": 200, "body": {
            "backendCleanup": "deleted", "upstreamCleanup": "unsupported", "provider": "lk888",
        }}
        assert metrics_before["generationCalls"] == 0
        assert images_before == []

        rust = _run_rust_fixture(address, tmp_path)
        (EVIDENCE_ROOT / "02-rust-cross-process.txt").write_text(
            rust.stdout + rust.stderr, encoding="utf-8"
        )
        assert rust.returncode == 0, rust.stdout + rust.stderr
        assert "controlled_backend_drives_three_steps_artifact_builder_and_delete ... ok" in rust.stdout
        metrics_after = json.loads(metrics_path.read_text(encoding="utf-8"))
        assert metrics_after["generationCalls"] == 3
        assert metrics_after["models"] == ["gpt-4o", "gpt-4o", "gpt-image-2"]
        assert list((state_dir / "artifacts").glob("*.png")) == []
        shutil.copy2(metrics_path, EVIDENCE_ROOT / "04-fake-lk888-metrics.json")


_RUST_FIXTURE = r'''

#[cfg(test)]
mod desktop_contract_fixture {
    use super::*;
    use crate::creation::photo_avatar::domain::{IdentityTraitKey, PHOTO_AVATAR_CONSENT_VERSION};
    use crate::creation::photo_avatar::profile::finalize_appearance_profile;
    use crate::runtime_assets::photo_avatar_builder::{BuildPhotoAvatarRequest, PhotoAvatarBuilder};
    use base64::Engine as _;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn source_image() -> ProviderSourceImage {
        let image = image::RgbaImage::from_pixel(256, 256, image::Rgba([91, 52, 33, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        ProviderSourceImage {
            source_id: "photo-fixture".into(),
            png_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            sha256: format!("{:x}", Sha256::digest(&bytes)), width: 256, height: 256,
        }
    }

    fn poll(provider: &ControlledBackendProvider, job_id: &str) -> RemoteJobState {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let state = provider.poll_job(job_id).unwrap();
            if state.state != "running" { return state; }
            assert!(Instant::now() < deadline, "backend job timed out");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn controlled_backend_drives_three_steps_artifact_builder_and_delete() {
        let provider = ControlledBackendProvider::from_env().unwrap();
        let source = source_image();
        let identity_job = provider.submit_step(ProviderStepRequest {
            session_id: "desktop-contract-session".into(), revision: 0, provider_session_id: None,
            step: RemoteStep::AnalyzeIdentity, attempt: 1,
            consent_version: PHOTO_AVATAR_CONSENT_VERSION.into(), source_images: vec![source.clone()],
            profile: None, body_module_contract_sha256: None, modification: None, locked_traits: vec![],
        }).unwrap();
        let provider_session_id = identity_job.provider_session_id.clone().unwrap();
        let identity = poll(&provider, &identity_job.provider_job_id);
        let Some(ProviderStepResult::Identity { partial_profile }) = identity.result else {
            panic!("expected identity profile")
        };
        assert_eq!(partial_profile.body_module_id, "body-balanced-v1");
        assert!(partial_profile.traits.iter().all(|value| !value.evidence_photo_ids.is_empty()));

        let completion_job = provider.submit_step(ProviderStepRequest {
            session_id: "desktop-contract-session".into(), revision: 0,
            provider_session_id: Some(provider_session_id.clone()), step: RemoteStep::CompleteAppearance,
            attempt: 1, consent_version: PHOTO_AVATAR_CONSENT_VERSION.into(), source_images: vec![],
            profile: Some(partial_profile.clone()), body_module_contract_sha256: None,
            modification: None, locked_traits: vec![],
        }).unwrap();
        let completion = poll(&provider, &completion_job.provider_job_id);
        let Some(ProviderStepResult::Appearance { completion }) = completion.result else {
            panic!("expected strict appearance completion")
        };
        let profile = finalize_appearance_profile(&partial_profile, completion).unwrap();
        assert_eq!(profile.traits.len(), 11);
        assert_eq!(profile.body_module_id, "body-balanced-v1");

        let module_root = PathBuf::from(std::env::var("PHOTO_AVATAR_FIXTURE_MODULE_ROOT").unwrap());
        let contract = std::fs::read(module_root.join("body-balanced-v1").join("模块.json")).unwrap();
        let contract_sha256 = format!("{:x}", Sha256::digest(&contract));
        let texture_job = provider.submit_step(ProviderStepRequest {
            session_id: "desktop-contract-session".into(), revision: 0,
            provider_session_id: Some(provider_session_id.clone()), step: RemoteStep::RenderTextureAtlas,
            attempt: 1, consent_version: PHOTO_AVATAR_CONSENT_VERSION.into(), source_images: vec![source],
            profile: Some(profile.clone()), body_module_contract_sha256: Some(contract_sha256),
            modification: None, locked_traits: Vec::<IdentityTraitKey>::new(),
        }).unwrap();
        let texture = poll(&provider, &texture_job.provider_job_id);
        let Some(ProviderStepResult::TextureAtlas {
            artifact_url, sha256, width, height, audit,
        }) = texture.result else {
            panic!("expected texture atlas")
        };
        assert_eq!((width, height), (2048, 2048));
        assert_eq!(sha256.as_str(), audit.canonical_sha256.as_str());
        assert_ne!(audit.provider_raw_sha256, audit.canonical_sha256);
        assert_eq!(audit.composer_version, "deterministic-alpha-v1");
        let bytes = provider.download_artifact(&artifact_url, &sha256).unwrap();
        let neutral = std::fs::read(module_root.join("body-balanced-v1")
            .join("body-balanced-v1.2048").join("texture_00.png")).unwrap();
        assert_ne!(bytes, neutral, "standard-cat fallback is forbidden");

        let preview_root = PathBuf::from(std::env::var("PHOTO_AVATAR_FIXTURE_PREVIEW_ROOT").unwrap());
        let built = PhotoAvatarBuilder::new(&module_root, &preview_root).build_preview(BuildPhotoAvatarRequest {
            session_id: "desktop-contract-session".into(), revision: 0,
            pet_id: "photo-avatar-fixture".into(), variant_id: "revision-0".into(),
            profile, texture_png: bytes.clone(), texture_sha256: sha256,
            texture_audit: audit,
        }).unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&built.manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["schemaVersion"], 5);
        assert_eq!(manifest["renderer"], "cat-spatial-live2d-v1");
        assert_eq!(std::fs::read(built.texture()).unwrap(), bytes);

        let cleanup = provider.delete_session_with_outcome(&provider_session_id).unwrap();
        assert_eq!(cleanup.backend_cleanup, CleanupState::Deleted);
        assert_eq!(cleanup.upstream_cleanup, UpstreamCleanupState::Unsupported);
        assert_eq!(cleanup.provider, "lk888");
    }
}
'''


if __name__ == "__main__" and len(sys.argv) == 5 and sys.argv[1] == "--serve":
    _serve(int(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4]))
