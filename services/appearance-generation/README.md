# Appearance Generation Experiment (M3)

独立实验工具：单张宠物照片 → 招牌画风卡通候选。不进入桌宠主流程。

## 安全

- API Key 只从环境变量读取：复制 `.env.example` 为 `.env` 并填写 `LK888_API_KEY`。
- `.env` 与 `output/`（含实验图片）已被 `.gitignore` 排除，绝不提交。

## 使用

```powershell
pip install -r requirements.txt
python -m src.lk888 --smoke      # 冒烟：生成一张测试图到 output/smoke/
python -m src.run_experiment --photo samples/xxx.jpg --traits traits.json
```

## 目录

- `src/`：适配层、提示词、后处理、过滤、评估
- `samples/`：真实测试照片（用户同意后放入，不进 git）
- `output/`：运行产物（不进 git）

## 照片分身受控后端

桌面端通过本机回环地址调用 FastAPI 合同。后端只使用 `lk888.ai`；
`analyzeIdentity` 与 `completeAppearance` 固定使用 `gpt-4o`，
`renderTextureAtlas` 固定使用 `gpt-image-2`。只有带 Bearer 鉴权的
`POST /v1/photo-avatar/steps` 会启动生成，其他路由不会上传照片或发起 lk888 请求。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/启动照片分身受控后端.ps1 -EnvFile services/appearance-generation/.env
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/探测照片分身受控后端.ps1 -BaseUrl http://127.0.0.1:8787 -EnvFile services/appearance-generation/.env
```

探针只验证 `/healthz`、未授权响应和假 session 的本地删除。删除响应中的
`upstreamCleanup` 固定为 `unsupported`：自有后端只删除本地状态和产物，
不会宣称 lk888 上游资源已删除。日志、HTTP 响应和状态文件不会包含 token、
base64、完整 prompt 或本机照片路径。
