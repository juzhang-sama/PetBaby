"""Loopback-only FastAPI adapter for the frozen desktop photo-avatar contract."""

from __future__ import annotations

from contextlib import asynccontextmanager
from concurrent.futures import ThreadPoolExecutor
import hashlib
import os
from pathlib import Path
import secrets
from typing import Any
import json

import httpx
from dotenv import load_dotenv
from fastapi import Depends, FastAPI, Header, HTTPException, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse, Response
from starlette.exceptions import HTTPException as StarletteHTTPException
import uvicorn

from .config import BackendConfig, ConfigError
from .contracts import ContractError, PixelStepRequest, StepRequest, parse_step_request
from .job_store import JobState, JobStore, JobStoreError
from .lk888_client import Lk888Client
from .pixel_avatar import analyze_pixel_identity, generate_pixel_avatar
from .pixel_style import SUPPORTED_PIXEL_STYLE_IDS, load_pixel_style_pack
from .audit import AuditContextV1
from .pipelines import (
    COMPOSER_VERSION,
    PNG_ENCODER_VERSION,
    _MODULE_ROOT,
    _decode_rgba_png,
    _guide_entry,
    analyze_identity,
    build_work_canvas,
    complete_appearance,
    render_texture_atlas,
    resolve_module_file,
)


_WIRE_CODES = frozenset(
    {
        "invalidInput",
        "auth",
        "quota",
        "contentPolicy",
        "unsupported",
        "network",
        "timeout",
        "provider5xx",
        "temporaryUnavailable",
    }
)
_PIXEL_CONTRACT_MESSAGE = "生成图片不符合像素素材要求，请重试。"
_SAFE_WIRE_MESSAGES = frozenset({_PIXEL_CONTRACT_MESSAGE})
_DEFAULT_WIRE_MESSAGES = {
    "invalidInput": "生成请求无效，请检查照片后重试。",
    "auth": "图片生成服务认证失败。",
    "quota": "图片生成服务额度不足。",
    "contentPolicy": "图片生成请求未通过内容审核。",
    "unsupported": "当前图片生成请求不受支持。",
    "network": "网络连接失败，请稍后重试。",
    "timeout": "图片生成超时，请稍后重试。",
    "provider5xx": "图片生成服务暂时不可用，请稍后重试。",
    "temporaryUnavailable": "图片生成服务暂时不可用，请稍后重试。",
}


class PipelineRunner:
    """Dispatch one request step to the only permitted lk888.ai model route."""

    def __init__(self, config: BackendConfig):
        self.config = config
        self.client = Lk888Client(config, httpx.Client())
        guide_path = Path(__file__).parent / "assets" / "uv-guides" / "索引.json"
        self.guide_index = json.loads(guide_path.read_text(encoding="utf-8"))

    def audit_context(self, request: StepRequest | PixelStepRequest) -> AuditContextV1:
        if isinstance(request, PixelStepRequest):
            return AuditContextV1(provider_model=self.config.image_model if request.step == "generatePixelAvatar" else self.config.analysis_model)
        model = (
            self.config.image_model
            if request.step == "renderTextureAtlas"
            else self.config.analysis_model
        )
        if request.step != "renderTextureAtlas" or request.profile is None:
            return AuditContextV1(provider_model=model)
        module_id = request.profile["bodyModuleId"]
        entry = _guide_entry(self.guide_index, module_id)
        module_dir = _MODULE_ROOT / module_id
        contract_bytes = (module_dir / "模块.json").read_bytes()
        contract = json.loads(contract_bytes.decode("utf-8"))
        neutral_path = resolve_module_file(module_dir, contract["files"]["neutralTexture"])
        neutral_bytes = neutral_path.read_bytes()
        _, neutral_alpha = _decode_rgba_png(neutral_bytes, "body module neutral texture")
        bundle = build_work_canvas(neutral_bytes)
        return AuditContextV1(
            provider_model=model,
            body_module_id=module_id,
            module_contract_sha256=hashlib.sha256(contract_bytes).hexdigest(),
            source_texture_sha256=hashlib.sha256(neutral_bytes).hexdigest(),
            source_alpha_sha256=hashlib.sha256(neutral_alpha).hexdigest(),
            work_canvas_sha256=hashlib.sha256(bundle.work_canvas_png).hexdigest(),
            region_map_sha256=hashlib.sha256(bundle.region_map_png).hexdigest(),
            composer_version=COMPOSER_VERSION,
            png_encoder_version=PNG_ENCODER_VERSION,
        )

    def run(self, request: StepRequest | PixelStepRequest) -> dict[str, object] | object:
        return self.run_with_task_reporter(request, lambda _: None)

    def run_with_task_reporter(
        self, request: StepRequest | PixelStepRequest, report_task_id: Any
    ) -> dict[str, object] | object:
        if isinstance(request, PixelStepRequest):
            style = load_pixel_style_pack(request.style_profile_id)
            if request.step == "analyzeIdentity":
                return analyze_pixel_identity(request, client=self.client)
            if request.step == "generatePixelAvatar":
                return generate_pixel_avatar(
                    request,
                    client=self.client,
                    style=style,
                    report_task_id=report_task_id,
                )
            raise ContractError("unsupported pixel step")
        if request.step == "analyzeIdentity":
            return analyze_identity(request, client=self.client)
        if request.step == "completeAppearance":
            after = complete_appearance(request, client=self.client)
            return _completion_wire(request.profile, after)
        if request.step == "renderTextureAtlas":
            return render_texture_atlas(
                request,
                self.client,
                self.guide_index,
                report_task_id=report_task_id,
            )
        raise ContractError("unsupported step")


def create_app(config: BackendConfig, store: JobStore) -> FastAPI:
    @asynccontextmanager
    async def lifespan(app: FastAPI):
        executor = ThreadPoolExecutor(max_workers=2, thread_name_prefix="photo-avatar-worker")
        app.state.executor = executor
        try:
            yield
        finally:
            executor.shutdown(wait=True, cancel_futures=True)

    app = FastAPI(lifespan=lifespan)

    def authorize(authorization: str = Header(default="")) -> None:
        expected = f"Bearer {config.backend_token}"
        if not secrets.compare_digest(authorization, expected):
            raise HTTPException(status_code=401, detail="unauthorized")

    @app.exception_handler(ContractError)
    async def invalid_contract(_: Any, __: ContractError) -> JSONResponse:
        return _error(400, "invalidInput", "invalid request")

    @app.exception_handler(RequestValidationError)
    async def invalid_shape(_: Any, __: RequestValidationError) -> JSONResponse:
        return _error(400, "invalidInput", "invalid request")

    @app.exception_handler(JobStoreError)
    async def invalid_store(_: Any, __: JobStoreError) -> JSONResponse:
        return _error(404, "invalidInput", "resource not found")

    @app.exception_handler(StarletteHTTPException)
    async def http_error(_: Any, error: StarletteHTTPException) -> JSONResponse:
        if error.status_code == 401:
            return _error(401, "auth", "unauthorized")
        return _error(error.status_code, "invalidInput", "request rejected")

    @app.get("/healthz")
    def health() -> dict[str, str]:
        return {"status": "ok"}

    @app.post("/v1/photo-avatar/steps", dependencies=[Depends(authorize)])
    def submit(payload: dict[str, object]) -> dict[str, str]:
        job = store.reserve(parse_step_request(payload))
        app.state.executor.submit(store.run_reserved, job.job_id)
        return {"providerSessionId": job.provider_session_id, "jobId": job.job_id}

    @app.get("/v1/photo-avatar/jobs/{job_id}", dependencies=[Depends(authorize)])
    def status(job_id: str, request: Request) -> dict[str, object]:
        return _job_wire(store.status(job_id), str(request.base_url))

    @app.post("/v1/photo-avatar/jobs/{job_id}/cancel", dependencies=[Depends(authorize)])
    def cancel(job_id: str) -> dict[str, str]:
        return store.cancel(job_id).to_wire()

    @app.delete("/v1/photo-avatar/sessions/{session_id}", dependencies=[Depends(authorize)])
    def delete_session(session_id: str) -> dict[str, str]:
        return store.delete_session(session_id).to_wire()

    @app.get("/v1/photo-avatar/artifacts/{artifact_id}", dependencies=[Depends(authorize)])
    def artifact(artifact_id: str) -> Response:
        return Response(store.read_artifact(artifact_id), media_type="image/png")

    return app


def _job_wire(state: JobState, origin: str) -> dict[str, object]:
    if state.status == "running":
        return {"state": "running", "result": None, "error": None}
    if state.status == "succeeded":
        if state.step == "analyzeIdentity" and state.result is not None:
            result: dict[str, object] = {
                "resultType": (
                    "pixelIdentity"
                    if state.result.get("styleProfileId") in SUPPORTED_PIXEL_STYLE_IDS
                    else "identity"
                ),
                "partialProfile": state.result,
            }
        elif state.step == "completeAppearance" and state.result is not None:
            result = {"resultType": "appearance", "completion": state.result}
        elif (
            state.step == "renderTextureAtlas"
            and state.artifact_id is not None
            and state.artifact_sha256 is not None
            and state.audit is not None
        ):
            result = {
                "resultType": "textureAtlas",
                "artifactUrl": _artifact_url(origin, state.artifact_id),
                "sha256": state.artifact_sha256,
                "width": 2048,
                "height": 2048,
                "audit": state.audit,
            }
        elif (
            state.step == "generatePixelAvatar"
            and state.artifact_id is not None
            and state.artifact_sha256 is not None
            and state.audit is not None
        ):
            width = state.audit.get("width")
            height = state.audit.get("height")
            if not isinstance(width, int) or not isinstance(height, int):
                return _failed_wire("invalidInput", "pixel audit dimensions unavailable")
            result = {
                "resultType": "pixelAvatar",
                "artifactUrl": _artifact_url(origin, state.artifact_id),
                "sha256": state.artifact_sha256,
                "width": width,
                "height": height,
                "audit": state.audit,
            }
        else:
            return _failed_wire("invalidInput", "job result unavailable")
        return {"state": "succeeded", "result": result, "error": None}
    if state.status == "failed":
        error = state.error or {"code": "temporaryUnavailable", "message": "job failed"}
        return _failed_wire(error["code"], error["message"])
    return _failed_wire("temporaryUnavailable", "job no longer available")


def _failed_wire(code: str, message: str) -> dict[str, object]:
    safe_code = code if code in _WIRE_CODES else "invalidInput"
    safe_message = (
        message
        if message in _SAFE_WIRE_MESSAGES
        else _DEFAULT_WIRE_MESSAGES[safe_code]
    )
    return {
        "state": "failed",
        "result": None,
        "error": {"code": safe_code, "message": safe_message},
    }


def _completion_wire(before: object, after: object) -> dict[str, object]:
    if not isinstance(before, dict) or not isinstance(after, dict):
        raise ContractError("completeAppearance profile is invalid")
    before_traits = {
        value.get("key"): value
        for value in before.get("traits", [])
        if isinstance(value, dict) and isinstance(value.get("key"), str)
    }
    completed: list[dict[str, object]] = []
    requested: list[str] = []
    traits = after.get("traits")
    if not isinstance(traits, list):
        raise ContractError("completeAppearance result is invalid")
    for value in traits:
        if not isinstance(value, dict) or not isinstance(value.get("key"), str):
            raise ContractError("completeAppearance result is invalid")
        key = value["key"]
        if before_traits.get(key) == value:
            continue
        if value.get("source") != "ai-completed":
            raise ContractError("completion result changed an observed trait")
        completed.append(value)
        if key in before_traits:
            requested.append(key)
    body_module_id = after.get("bodyModuleId")
    body_module_source = after.get("bodyModuleSource")
    if not isinstance(body_module_id, str) or not isinstance(body_module_source, str):
        raise ContractError("completeAppearance result is invalid")
    return {
        "requestedTraitKeys": requested,
        "completedTraits": completed,
        "bodyModuleId": body_module_id,
        "bodyModuleSource": body_module_source,
    }


def _artifact_url(origin: str, artifact_id: str) -> str:
    return f"{origin}v1/photo-avatar/artifacts/{artifact_id}"


def _error(status: int, code: str, message: str) -> JSONResponse:
    return JSONResponse(status_code=status, content={"code": code, "message": message})


def main() -> None:
    load_dotenv()
    try:
        config = BackendConfig.from_env(os.environ)
    except ConfigError as exc:
        raise SystemExit(f"photo-avatar backend configuration invalid: {exc}") from exc
    store = JobStore(config.state_dir, runner=PipelineRunner(config))
    uvicorn.run(create_app(config, store), host=config.host, port=config.port)


if __name__ == "__main__":
    main()
