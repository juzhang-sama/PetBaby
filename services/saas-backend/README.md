# SaaS Backend（桌面宠物生成中转服务）

桌面端照片上传代理 + 密钥托管 + 任务/照片生命周期管理。桌面端不再持有第三方生成平台 API Key，只与本服务通信。

## 安全

- API Key 只从环境变量读取：复制 `.env.example` 为 `.env` 并填写 `LK888_API_KEY`。
- `.env` 与 `data/`（照片、结果、数据库）已被 `.gitignore` 排除，绝不提交。
- 日志不记录照片内容、API Key 或完整模型请求。
- 可选访问令牌：`.env` 配置 `SAAS_ACCESS_TOKEN` 后，所有 `/api/v1/*` 请求必须带 `Authorization: Bearer <token>`（桌面端在设置里填写同一令牌）。
- 上传限流：`RATE_LIMIT_PER_MINUTE`（默认 10）限制每个来源 IP 每分钟创建任务数，设为 0 关闭。

## 使用

```powershell
pip install -r requirements.txt
python -m uvicorn src.app:app --host 127.0.0.1 --port 8787
```

端到端联调（起后端 + 冒烟 + 桌面端）：仓库根目录执行 `.\scripts\端到端联调.ps1`，指引见 `docs/验证记录/SaaS端到端联调指引.md`。

## API

- `POST /api/v1/generations`：multipart 上传 `photo` + `species`（cat|dog），返回 `202 {jobId, status}`
- `GET /api/v1/generations/{job_id}`：查询状态
- `GET /api/v1/generations/{job_id}/result`：下载结果图片
- `DELETE /api/v1/generations/{job_id}`：删除任务与照片/结果文件
- `GET /healthz`：健康检查
