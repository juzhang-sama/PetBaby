# Task 1 实施报告

## 状态

仓库侧 Cubism 许可证清单、WebGL 探针接口、Tauri 独立探针配置和官方 SDK 本地准备脚本已完成。SDK 与真实模型未提供，真实 Live2D 渲染和 GPU/窗口验收保持未验证。

## TDD 记录

RED 命令：`npm test -- src/runtime-live2d/probe.test.ts`

预期失败：Vitest 无法导入尚不存在的 `./probe`（`Cannot find module './probe'`）。

GREEN 命令：`npm test -- src/runtime-live2d/probe.test.ts`

输出：`1 passed`，`4 passed`。

全量验证：`npm test` 输出 `13 passed` / `55 passed`；`npm run typecheck` 通过且无警告。

SDK 准备验证：`npm run prepare:cubism` 在未设置 `CUBISM_SDK_ROOT` 时按设计以非零状态退出，并明确提示缺少环境变量。

## 文件

- `apps/desktop/src/runtime-live2d/probe.ts`
- `apps/desktop/src/runtime-live2d/probe.test.ts`
- `apps/desktop/src/runtime-live2d/cubism-adapter.ts`
- `apps/desktop/src/runtime-live2d/许可证说明.md`
- `apps/desktop/src-tauri/tauri.live2d-probe.conf.json`
- `scripts/准备CubismSDK.ps1`
- `scripts/验证Live2D技术探针.ps1`
- `docs/验证记录/Live2D技术探针结论.md`
- 修改 `apps/desktop/src/main.ts`、`apps/desktop/package.json`、`apps/desktop/vite.config.ts`、`.gitignore`

## 自评与风险

- 探针只报告 WebGL 可用性、context lost 和非透明像素，不伪造 SDK/model 成功。
- `UnavailableCubismAdapter` 是明确失败的仓库侧适配缝隙；官方 Framework/Core 不在仓库内。
- 尚未执行 Tauri 窗口实测、真实模型绘制、透明/穿透/多 DPI/锁屏睡眠/性能验收。
- 提供 SDK 路径和授权测试模型后，需运行 `scripts/验证Live2D技术探针.ps1` 完成后续人工验收。
