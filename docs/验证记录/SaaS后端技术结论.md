# SaaS 后端技术结论（2026-08-05 MVP）

## 阶段判定

- 服务骨架：通过（`services/saas-backend`，FastAPI + SQLite + httpx）
- 存储层：通过（`generation_jobs`/`source_photos`，claim 状态迁移、重启恢复、删除级联，7 项测试）
- 提示词：通过（纯白背景 + 高保真特征，3 项测试）
- 供应商适配：通过（lk888 submit/poll/download/generate、数字 task_id、重试，9 项测试）
- 下载超时加固（2026-08-05）：lk888 结果下载超时 120s→300s 并加重试（默认 3 次），避免平台偶发不可达直接判失败；新增 2 项测试
- Worker：通过（认领队列、结果落盘、失败落库，4 项测试）
- API：通过（创建/查询/下载/删除/健康检查、输入校验、CORS 预检，9 项测试）
- 联调准备：通过（`scripts/端到端联调.ps1 -SkipDesktop` 实测：后端启动、healthz、CORS 预检、非法物种 422、缺失任务 404 全部通过）
- 发布加固（2026-08-05）：访问令牌鉴权（`SAAS_ACCESS_TOKEN`，Bearer）、上传限流（`RATE_LIMIT_PER_MINUTE`，滑动窗口）、过期任务自动清理（completed/failed 超过 24 小时删除 DB 与照片/结果文件，worker 每小时检查）；新增 7 项测试
- 引导式创造（2026-08-05）：`POST /api/v1/generations` 支持可选 photo + `traits` JSON，无照片时按特征生成；生成提示词随任务落库（`generation_jobs.prompt`），worker 使用存储的提示词；新增 5 项测试
- 合计：45 项 pytest 全绿，已并入 `执行M4检查.ps1`

## 架构

- 桌面端 → `POST /api/v1/generations`（照片 + species）→ SQLite `queued` → worker 持钥调用 lk888 → 结果落盘 → 客户端轮询状态/下载/删除。
- API Key 只从环境变量读取，不落库、不入日志。
- 重启恢复：`running → queued`，中断任务继续处理。
- 桌面端接线：设置向导改为填写服务地址（不再持有 API Key）；满意后把结果字节交给 `asset_compile_from_raw`（本地质量门控抠图 + 编译），放弃时调用 `DELETE` 清理云端任务。
- 修复（2026-08-05）：`SaasClient` 原先以方法形式调用 `window.fetch`，WebView2 报 `Illegal invocation`；构造时 `fetchImpl.bind(globalThis)` 修复，并新增回归测试。
- 修复（2026-08-05）：manifest `parts[].boneId` 为 `null` 时前端解析器抛错，导致宠物窗口解析失败并回退默认测试形象；解析器改为接受 `null` 并归一化为 `undefined`，新增回归测试。
- 修复（2026-08-05）：后端未启动时 `fetch` 抛 `Failed to fetch`，现在统一转为“无法连接生成服务，请确认后端已启动（地址）”的可读错误，新增回归测试。
- 跨域：服务端 CORS 允许 WebView 来源（开发期 `*`）；Tauri CSP `connect-src` 放开 `http://127.0.0.1:*`、`http://localhost:*` 与 `https:`，release 下可访问本地/远程 SaaS。

## 端点

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/healthz` | 健康检查 |
| POST | `/api/v1/generations` | 上传照片创建生成任务（202） |
| GET | `/api/v1/generations/{job_id}` | 查询状态 |
| GET | `/api/v1/generations/{job_id}/result` | 下载结果（未就绪 409） |
| DELETE | `/api/v1/generations/{job_id}` | 删除任务与照片/结果文件（204） |

## 待办

- 供应商任务超时后的远端素材生命周期策略。
