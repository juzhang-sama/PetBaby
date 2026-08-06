# Live2D 技术探针结论

- 自动化框架：已建立 `evaluateProbe` 单元测试及 Tauri 探针配置。
- 当前 SDK：未提供；`CUBISM_SDK_ROOT` 未设置时准备脚本会明确失败。
- 真实模型渲染：未执行，不能宣称 Live2D 可用。
- GPU、透明、置顶、穿透、多层 DPI、锁屏、睡眠、context lost 与性能：均未验收。
- 下一步：用户从官方渠道提供 Cubism SDK for Web 路径及获授权测试模型，再运行验证脚本。
