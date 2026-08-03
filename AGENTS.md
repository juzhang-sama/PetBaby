# 项目协作约束

- Codex 创建的 Markdown 默认使用中文文件名，工具强制文件名除外。
- Windows 专属代码只放在 `src-tauri/src/platform/windows.rs`。
- M0 不引入 React、SQLite、云端模型、声音或 Agent。
- 修改纯逻辑前先写失败测试；系统能力必须补充人工验证记录。
- 不把尚未实测的窗口、性能或生成能力描述为已完成。
