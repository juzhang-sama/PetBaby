# SaaS 端到端联调指引

目标：本地跑通“桌面端上传照片 → SaaS 后端持钥生成 → 结果下载 → 本地抠图编译 → 宠物上桌面”的完整链路。

## 前置准备

1. 安装 SaaS 后端依赖：

```powershell
cd services\saas-backend
pip install -r requirements.txt
```

2. 配置密钥：复制 `.env.example` 为 `.env`，填写 `LK888_API_KEY`。密钥只留在服务端，桌面端不再需要。

## 一键联调

在仓库根目录（`D:\petBaby\desktop-pet`）下执行：

```powershell
.\scripts\端到端联调.ps1
```

脚本会依次：启动 SaaS 后端（后台隐藏窗口）→ 等待 `/healthz` 就绪 → 跑冒烟检查 → 启动 `npm run tauri dev`。退出桌面应用后自动停掉后端。

如果在其他目录执行，请使用绝对路径：

```powershell
powershell -ExecutionPolicy Bypass -File D:\petBaby\desktop-pet\scripts\端到端联调.ps1
```

只验证后端（不启动桌面）：

```powershell
.\scripts\端到端联调.ps1 -SkipDesktop
```

单独前台启动后端：

```powershell
.\scripts\启动SaaS后端.ps1
```

## 桌面端操作步骤

1. 打开设置窗口 →「创建宠物」。
2. 生成服务地址填 `http://127.0.0.1:8787`，点「保存地址」。
3. 选择宠物照片（PNG/JPEG）→ 选择种类 →「下一步」。
4. 等待“排队中 → 生成中…”，完成后预览候选。
5. 满意 →「满意，出现在桌面」；不满意 →「重新生成」；放弃会删除云端任务和本地宠物记录。

## 预期结果

- 创建后宠物立即出现在桌面，本地不再有 API Key 输入框。
- 后端 `data/photos/<job_id>/source.*` 保存上传原图，`data/results/<job_id>/result.png` 保存生成结果。
- 放弃任务后，`DELETE /api/v1/generations/{job_id}` 会清理云端照片/结果文件。

## 排错

- 后端日志：`%TEMP%\desktop-pet-saas-backend.out.log` / `%TEMP%\desktop-pet-saas-backend.err.log`。
- 后端能起但任务一直失败：检查 `.env` 的 `LK888_API_KEY` 是否填写；密钥缺失时任务会落 `failed`。
- 桌面端访问不到后端：确认服务地址以 `http://` 开头；CSP/CORS 已放开 `127.0.0.1:*`、`localhost:*` 与 `https:`。
- 端口被占用：先结束占用 8787 的进程，再重跑联调脚本。
