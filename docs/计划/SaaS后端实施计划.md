# SaaS 后端（照片上传代理 + 密钥托管 + 生命周期）实施计划

> **For agentic workers:** 本计划在本会话内按 TDD 以 inline 方式执行（本项目不使用子代理）。步骤用 `- [ ]` 跟踪。

**Goal:** 交付一个独立部署的 SaaS 后端 MVP：桌面端上传宠物照片，后端持钥调用 lk888 生成平台，并托管任务状态与照片生命周期。

**Architecture:** FastAPI 单进程 + SQLite + 后台 worker。API 层只负责接收/查询/下载/删除；worker 循环认领 `queued` 任务、调用同步生成供应商（线程池）并落盘结果；进程重启时把中断的 `running` 任务恢复为 `queued`。

**Tech Stack:** Python 3.12、FastAPI、uvicorn、httpx、python-dotenv、pytest、SQLite（stdlib）。

## Global Constraints

- API Key 只从环境变量读取（`LK888_API_KEY`），绝不写入数据库、普通配置或日志。
- 任务状态机：`queued → running → completed | failed`；重启时 `running → queued`。
- 源照片与生成结果只保存在服务本地 `data/` 目录；`DELETE` 接口可彻底清理。
- 所有时间戳为 UTC ISO-8601 字符串。
- 测试使用 `tmp_path` 隔离数据目录；供应商在测试中用 FakeProvider 或 httpx 打桩，不访问真实网络。

---

## Task 1: 服务脚手架与配置

**Files:**
- Create: `services/saas-backend/requirements.txt`
- Create: `services/saas-backend/pytest.ini`
- Create: `services/saas-backend/.env.example`
- Create: `services/saas-backend/.gitignore`
- Create: `services/saas-backend/README.md`
- Create: `services/saas-backend/src/__init__.py`
- Create: `services/saas-backend/src/config.py`
- Test: 无（配置与脚手架属环境搭建，不写单测）

**Interfaces:**
- Produces: `config.data_dir() -> Path`、`config.database_path() -> Path`、`config.lk888_api_key() -> str`、`config.lk888_base_url() -> str`、`config.lk888_model() -> str`、`config.poll_interval() -> float`、`config.max_job_wait_seconds() -> float`、`config.host() -> str`、`config.port() -> int`

- [x] 创建目录与文件，依赖固定为：`fastapi>=0.115`、`uvicorn>=0.30`、`httpx>=0.28`、`python-dotenv>=1.0`、`python-multipart>=0.0.9`、`pytest>=8`
- [x] `config.py` 从环境变量读取（缺省：`LK888_BASE_URL=https://api.lk888.ai`、`LK888_MODEL=gpt-image-2`、`SAAS_DATA_DIR=./data`、`POLL_INTERVAL=2.0`、`MAX_JOB_WAIT_SECONDS=300.0`、`HOST=127.0.0.1`、`PORT=8787`）；`lk888_api_key()` 为空时抛 `RuntimeError`
- [x] `.env.example` 与 `.gitignore`（忽略 `.env`、`data/`、`__pycache__/`、`.pytest_cache/`）

## Task 2: SQLite 存储层

**Files:**
- Create: `services/saas-backend/src/storage.py`
- Test: `services/saas-backend/src/test_storage.py`

**Interfaces:**
- Produces: `GenerationStorage(db_path: Path)`；`initialize()`、`create_job(job_id, species)`、`get_job(job_id) -> dict | None`、`claim_next_queued() -> dict | None`、`mark_running(job_id, provider_task_id)`、`mark_completed(job_id, result_path)`、`mark_failed(job_id, error)`、`reset_stale_running() -> int`、`save_source_photo(photo_id, job_id, original_name, stored_path, sha256, size)`、`list_photos(job_id) -> list[dict]`、`delete_job(job_id) -> None`

**行为：**
- 表 `generation_jobs(job_id TEXT PK, species TEXT, status TEXT, provider_task_id TEXT, error TEXT, result_path TEXT, created_at TEXT, updated_at TEXT)`
- 表 `source_photos(photo_id TEXT PK, job_id TEXT, original_name TEXT, stored_path TEXT, sha256 TEXT, size INTEGER, created_at TEXT)`
- `claim_next_queued()` 用 `BEGIN IMMEDIATE` 取最早 `queued` 并置为 `running`，返回整行
- `reset_stale_running()` 把全部 `running` 置回 `queued`，返回数量
- `delete_job()` 同时删除该任务的照片记录

- [x] 先写 `test_storage.py` 覆盖：建任务/查任务、claim 状态迁移、完成/失败落库、重启恢复、照片保存、删除级联；运行 `pytest src/test_storage.py` 确认 RED
- [x] 实现 `storage.py`，运行确认 GREEN

## Task 3: 提示词模块

**Files:**
- Create: `services/saas-backend/src/prompt.py`
- Test: `services/saas-backend/src/test_prompt.py`

**Interfaces:**
- Produces: `build_prompt(species: str) -> str`（纯白背景 + 高保真特征，与桌面端 `creation-flow.ts` 的 `buildPrompt` 保持一致）

- [x] 先写失败测试（断言包含 `pure white background`、species 文本、`no watermark`），再实现

## Task 4: 生成供应商适配层

**Files:**
- Create: `services/saas-backend/src/provider.py`
- Create: `services/saas-backend/src/lk888.py`
- Test: `services/saas-backend/src/test_lk888.py`

**Interfaces:**
- Produces: `TaskState`、`GenerationResult`、`GenerationError`、`GenerationProvider`（Protocol）
- Produces: `Lk888Provider(key, base, model)`：`submit(prompt, ref_images, mime, size, retries, retry_delay) -> task_id`、`poll(task_id) -> TaskState`、`download(result_url) -> bytes`、`generate(prompt, ref_images, mime, size, poll_interval, max_wait) -> GenerationResult`
- 提交载荷与桌面 Rust 适配层一致：`model/prompt/params{size,quality,n,response_format,images[data-url]}`
- `task_id` 兼容数字/字符串

- [x] 先写失败测试（monkeypatch httpx：提交载荷、数字 task_id、轮询成功/运行中、下载、整链路），再实现

## Task 5: 后台任务 Worker

**Files:**
- Create: `services/saas-backend/src/worker.py`
- Test: `services/saas-backend/src/test_worker.py`

**Interfaces:**
- Produces: `GenerationWorker(storage, provider_factory, data_dir, poll_interval, max_wait)`；`start()`、`stop()`、`process_available() -> int`

**行为：**
- `process_available()` 循环 `claim_next_queued()`，读照片字节，`asyncio.to_thread(provider.generate, ...)`，成功写 `data/results/<job_id>/result.png` 并 `mark_completed`，失败 `mark_failed`
- `start()` 启动后台循环（间隔 `poll_interval`），`stop()` 取消

- [x] 先写失败测试（FakeProvider 成功/失败、结果落盘、失败写 error、循环认领），再实现

## Task 6: FastAPI 应用与端点

**Files:**
- Create: `services/saas-backend/src/app.py`
- Test: `services/saas-backend/src/test_app.py`

**Interfaces:**
- Produces: `create_app(storage=None, provider_factory=None, data_dir=None, poll_interval=None, max_wait=None) -> FastAPI`

**端点：**
- `GET /healthz` → `{"status":"ok"}`
- `POST /api/v1/generations`：multipart `photo` + `species`（cat|dog）；校验照片为 PNG/JPEG 且 ≤10MB；存照片 → `create_job` → 返回 `202 {"jobId","status":"queued"}`
- `GET /api/v1/generations/{job_id}` → 状态/错误/结果可用标记
- `GET /api/v1/generations/{job_id}/result` → completed 时返回 PNG/JPEG 文件，否则 409/404
- `DELETE /api/v1/generations/{job_id}` → 删除 DB 行与 `data/photos|results/<job_id>`，返回 204
- 启动时 `storage.initialize()` + `reset_stale_running()` + `worker.start()`；关闭时 `worker.stop()`

- [x] 先写失败测试（TestClient + FakeProvider：创建→worker 处理→状态/结果下载；非法物种/超大文件 422/413；删除清理），再实现

## Task 7: 验证与文档收尾

- [x] `pip install -r requirements.txt`
- [x] `python -m pytest src -q` 全绿（28 项）
- [x] 更新 `docs/开发进度与计划.md`（SaaS 后端状态、下一步桌面端接线）
- [x] 新增 `docs/验证记录/SaaS后端技术结论.md` 并纳入全量闸门
