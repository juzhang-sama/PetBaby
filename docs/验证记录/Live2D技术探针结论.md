# Live2D 技术探针结论

## 本轮已验证

- 官方 SDK 路径：`D:\Live2D\CubismSdkForWeb-5-r.5`。
- `npm run prepare:cubism` 成功；Core、Framework shader、Wanko model3/moc3、纹理和 Motion 复制到 Git 忽略目录 `apps/desktop/.vendor/` 与 `apps/desktop/public/live2d/`。
- 聚焦测试：`npm test -- src/runtime-live2d`，`2` 个测试文件、`11/11` 通过。
- `npm run typecheck`：通过。
- `npm run build`：通过，生成独立 `cubism-runtime` bundle。
- 独立 Tauri 窗口 PID `20760` 正常响应；窗口标题为 `Desktop Pet Live2D Probe`，Win32 窗口矩形为 `420×520`。
- 窗口区域：`GetWindowRgn` 返回 `code=3`，有效区域框为 `117,228–317,442`，说明 WebGL alpha 已转换为 CSS 命中行并应用到 Win32 窗口区域。
- 透明区域点击：在窗口外透明点 `(36,36)` 点击前后，前台窗口句柄均为 `10031262`，点击未被宠物窗口截获。
- 探针运行时持续提交 `requestAnimationFrame`；idle Motion 的连续运行和 context lost 停止路径均有自动化覆盖。

## 结论状态

- **已通过**：官方 Cubism Core/Framework 初始化、Wanko 模型加载路径、至少一帧非透明绘制、透明窗口、置顶窗口、窗口尺寸适配、DPR 命中区域计算、透明区域点击穿透、动画帧调度的代码与现场探针验证。
- **未测**：多屏跨 DPI 拖动、Windows 锁屏恢复、睡眠/唤醒恢复、真实 WebGL context lost/recovery、GPU/CPU/内存长期采样、Windows 10 兼容性。
- **限制**：`skipTaskbar`/`WS_EX_TOOLWINDOW` 探针窗口不出现在 CUA 的可枚举窗口列表中，因此本轮没有把合成桌面截图当作模型渲染证据；渲染结果以 WebGL alpha 采样、Tauri 页面结果和 Win32 区域证据为准。

Task 1 到此具备进入代码审查和提交的证据；正式产品 Renderer 接线仍需等后续任务，不在本探针中提前实现。
