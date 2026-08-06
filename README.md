# Desktop Pet

Windows 优先的轻量桌面宠物项目（Tauri 2 + Rust + TypeScript + PixiJS 8 + SQLite）：上传宠物照片 → 云端生成卡通形象 → 桌面陪伴。

## 当前状态

M0~M4 已通过，处于 P2“体验精进与发布准备”阶段。最新进度与计划见 [docs/开发进度与计划.md](docs/开发进度与计划.md)，技术结论见 [docs/验证记录/](docs/验证记录/)。

## 开发

```powershell
cd apps/desktop
npm install
npm test
npm run tauri dev
```

全量检查：`scripts\执行M4检查.ps1`（前端 + Rust + Python 测试、clippy、构建）。
