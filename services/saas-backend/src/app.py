# -*- coding: utf-8 -*-
"""FastAPI application for the desktop-pet SaaS generation relay."""
import asyncio
import hashlib
import json
import shutil
import uuid
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import Depends, FastAPI, File, Form, HTTPException, Request, UploadFile
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse

from src import config
from src.analyzer import PetAnalyzer
from src.lk888 import Lk888Provider
from src.prompt import STYLE_3D, STYLE_CARTOON, build_guided_prompt, build_prompt
from src.ratelimit import SlidingWindowRateLimiter
from src.storage import GenerationStorage
from src.worker import GenerationWorker

MAX_PHOTO_BYTES = 10 * 1024 * 1024
MAX_PHOTOS = 3
MAX_CANDIDATES = 3
ALLOWED_SPECIES = {"cat", "dog"}
ALLOWED_STYLES = {STYLE_CARTOON, STYLE_3D}


def detect_image_mime(data: bytes) -> str | None:
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image/png"
    if data.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    return None


def default_provider_factory():
    return Lk888Provider(
        key=config.lk888_api_key(),
        base=config.lk888_base_url(),
        model=config.lk888_model(),
    )


def default_analyzer_factory() -> PetAnalyzer:
    try:
        return PetAnalyzer(
            key=config.lk888_api_key(),
            base=config.lk888_base_url(),
            model=config.analyze_model(),
        )
    except RuntimeError:
        return PetAnalyzer(key="", base="", model="")


def create_app(
    storage: GenerationStorage | None = None,
    provider_factory=None,
    analyzer_factory=None,
    data_dir: Path | None = None,
    poll_interval: float | None = None,
    max_wait: float | None = None,
    access_token: str | None = None,
    rate_limit_per_minute: int | None = None,
) -> FastAPI:
    store = storage or GenerationStorage(config.database_path())
    factory = provider_factory or default_provider_factory
    analyzer_factory = analyzer_factory or default_analyzer_factory
    root = Path(data_dir) if data_dir is not None else config.data_dir()
    interval = poll_interval if poll_interval is not None else config.poll_interval()
    wait = max_wait if max_wait is not None else config.max_job_wait_seconds()
    token = access_token if access_token is not None else config.access_token()
    limit = (
        rate_limit_per_minute
        if rate_limit_per_minute is not None
        else config.rate_limit_per_minute()
    )

    @asynccontextmanager
    async def lifespan(_app: FastAPI):
        store.initialize()
        store.reset_stale_running()
        worker = GenerationWorker(
            store, factory, root, poll_interval=interval, max_wait=wait
        )
        _app.state.storage = store
        _app.state.data_dir = root
        _app.state.worker = worker
        _app.state.analyzer = analyzer_factory()
        _app.state.access_token = token
        _app.state.ratelimiter = (
            SlidingWindowRateLimiter(limit=limit, window_seconds=60) if limit > 0 else None
        )
        await worker.start()
        try:
            yield
        finally:
            await worker.stop()

    app = FastAPI(title="Desktop Pet SaaS Backend", lifespan=lifespan)
    # MVP: allow the Tauri webview (dev tauri://localhost / release
    # http://tauri.localhost) to call this service. Auth/allowlist comes
    # with the pre-release hardening task.
    app.add_middleware(
        CORSMiddleware,
        allow_origins=["*"],
        allow_methods=["*"],
        allow_headers=["*"],
    )

    def require_auth(request: Request) -> None:
        token = request.app.state.access_token
        if not token:
            return
        if request.headers.get("Authorization") != f"Bearer {token}":
            raise HTTPException(status_code=401, detail="unauthorized")

    def require_rate_limit(request: Request) -> None:
        limiter = request.app.state.ratelimiter
        if limiter is None:
            return
        key = request.client.host if request.client else "unknown"
        if not limiter.allow(key):
            raise HTTPException(status_code=429, detail="rate limit exceeded")

    @app.get("/healthz")
    def healthz() -> dict:
        return {"status": "ok"}

    @app.post(
        "/api/v1/generations",
        status_code=202,
        dependencies=[Depends(require_auth), Depends(require_rate_limit)],
    )
    async def create_generation(
        request: Request,
        photo: UploadFile | None = File(None),
        photos: list[UploadFile] = File(default=[]),
        species: str = Form(...),
        style: str = Form(STYLE_CARTOON),
        count: int = Form(1),
        traits: str | None = Form(None),
    ) -> dict:
        if species not in ALLOWED_SPECIES:
            raise HTTPException(status_code=422, detail="species must be cat or dog")
        if style not in ALLOWED_STYLES:
            raise HTTPException(
                status_code=422,
                detail=f"style must be one of {sorted(ALLOWED_STYLES)}",
            )
        if count < 1 or count > MAX_CANDIDATES:
            raise HTTPException(
                status_code=422,
                detail=f"count must be between 1 and {MAX_CANDIDATES}",
            )
        uploads = photos or ([photo] if photo is not None else [])
        if len(uploads) > MAX_PHOTOS:
            raise HTTPException(
                status_code=422, detail=f"at most {MAX_PHOTOS} photos are allowed"
            )
        parsed_traits: dict = {}
        if traits:
            try:
                parsed = json.loads(traits)
            except json.JSONDecodeError as error:
                raise HTTPException(status_code=422, detail="traits must be valid JSON") from error
            if not isinstance(parsed, dict):
                raise HTTPException(status_code=422, detail="traits must be a JSON object")
            parsed_traits = parsed
        if not uploads and not parsed_traits:
            raise HTTPException(status_code=422, detail="photo or traits is required")

        store = request.app.state.storage
        data_dir = request.app.state.data_dir
        analyzed_traits: dict | None = None
        uploaded: list[tuple[bytes, str, str]] = []
        for upload in uploads:
            data = await upload.read()
            if len(data) > MAX_PHOTO_BYTES:
                raise HTTPException(status_code=413, detail="photo too large (max 10MB)")
            mime = detect_image_mime(data)
            if mime is None:
                raise HTTPException(status_code=422, detail="photo must be PNG or JPEG")
            uploaded.append((data, mime, upload.filename or "photo"))
        if parsed_traits:
            prompt = build_guided_prompt(species, parsed_traits, style=style)
        else:
            if uploaded:
                analyzed_traits = await asyncio.to_thread(
                    request.app.state.analyzer.analyze,
                    [(data, mime) for data, mime, _ in uploaded],
                    species,
                )
            prompt = build_prompt(species, analyzed_traits, style=style)
        job_ids = [uuid.uuid4().hex for _ in range(count)]
        for job_id in job_ids:
            store.create_job(job_id, species, prompt)
            for index, (data, mime, original_name) in enumerate(uploaded):
                ext = "png" if mime == "image/png" else "jpg"
                photo_dir = data_dir / "photos" / job_id
                photo_dir.mkdir(parents=True, exist_ok=True)
                stored_path = photo_dir / f"source-{index + 1}.{ext}"
                stored_path.write_bytes(data)
                store.save_source_photo(
                    uuid.uuid4().hex,
                    job_id,
                    original_name or f"source-{index + 1}.{ext}",
                    str(stored_path),
                    hashlib.sha256(data).hexdigest(),
                    len(data),
                    mime,
                )
        return {
            "jobIds": job_ids,
            "jobId": job_ids[0],
            "status": "queued",
            "traits": parsed_traits if parsed_traits else analyzed_traits,
            "style": style,
        }

    @app.get(
        "/api/v1/generations/{job_id}",
        dependencies=[Depends(require_auth)],
    )
    def get_generation(request: Request, job_id: str) -> dict:
        job = request.app.state.storage.get_job(job_id)
        if job is None:
            raise HTTPException(status_code=404, detail="job not found")
        return {
            "jobId": job["job_id"],
            "species": job["species"],
            "status": job["status"],
            "error": job["error"],
            "resultAvailable": job["status"] == "completed",
            "createdAt": job["created_at"],
            "updatedAt": job["updated_at"],
        }

    @app.post(
        "/api/v1/landmarks",
        dependencies=[Depends(require_auth), Depends(require_rate_limit)],
    )
    async def analyze_landmarks(
        request: Request,
        photo: UploadFile = File(...),
        species: str = Form(...),
    ) -> dict:
        if species not in ALLOWED_SPECIES:
            raise HTTPException(status_code=422, detail="species must be cat or dog")
        data = await photo.read()
        if len(data) > MAX_PHOTO_BYTES:
            raise HTTPException(status_code=413, detail="photo too large (max 10MB)")
        mime = detect_image_mime(data)
        if mime is None:
            raise HTTPException(status_code=422, detail="photo must be PNG or JPEG")
        landmarks = await asyncio.to_thread(
            request.app.state.analyzer.analyze_landmarks,
            (data, mime),
            species,
        )
        return {"landmarks": landmarks}

    @app.get(
        "/api/v1/generations/{job_id}/result",
        dependencies=[Depends(require_auth)],
    )
    def get_result(request: Request, job_id: str):
        job = request.app.state.storage.get_job(job_id)
        if job is None:
            raise HTTPException(status_code=404, detail="job not found")
        if job["status"] != "completed" or not job["result_path"]:
            raise HTTPException(status_code=409, detail="result not ready")
        path = Path(job["result_path"])
        if not path.exists():
            raise HTTPException(status_code=404, detail="result file missing")
        return FileResponse(path, media_type="image/png", filename=f"{job_id}.png")

    @app.delete(
        "/api/v1/generations/{job_id}",
        status_code=204,
        dependencies=[Depends(require_auth)],
    )
    def delete_generation(request: Request, job_id: str) -> None:
        store = request.app.state.storage
        if store.get_job(job_id) is None:
            raise HTTPException(status_code=404, detail="job not found")
        store.delete_job(job_id)
        data_dir = request.app.state.data_dir
        for folder in ("photos", "results"):
            shutil.rmtree(data_dir / folder / job_id, ignore_errors=True)
        return None

    return app


app = create_app()
