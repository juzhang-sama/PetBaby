# Desktop Pet

Windows 优先的轻量桌面宠物：上传猫/狗照片 → 云端生成形象 → 桌面陪伴（多宠、离线可用）。

## 开发

**唯一入口：双击仓库根目录的 `一键启动桌宠开发环境.cmd`。**

它依次做：检查 Node.js / Rust 工具链 → 补全前端依赖 → 拉起生成后端（`127.0.0.1:8787`，已在跑就复用）→ 启动桌宠窗口。

前置条件：

- 已安装 Node.js（含 npm）和 Rust 工具链（cargo）。
- 需要「照片生成」能力：把 `services/appearance-generation/.env.example` 复制成 `.env` 并填好里面的必填项。没有这个文件也能正常开发和跑桌宠，只是不能生成新宠物。

只做环境校验、不启动窗口：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\start-desktop-pet-dev.ps1 -ValidateOnly
```

## M0 状态

M0 的当前结论见 [M0技术结论](docs/验证记录/M0技术结论.md)。该文档区分已通过、实验性、未通过和未测试能力；README 不单独重复可能过期的兼容性结论。
