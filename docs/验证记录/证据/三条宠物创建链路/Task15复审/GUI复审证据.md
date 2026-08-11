# Task 15 独立复审 GUI 证据

## 1. 环境与取证方式

- 日期：2026-08-11；复审基线：`eb28ac6815be9bc35a57723ccd4ee1a0e0cfd0d5` 加本次待提交修复。
- 隔离应用标识：`com.desktop-pet.task15-8e5480bf`；隔离端口：14321。
- 设置窗口标题：`桌面宠物设置 Task15 8e5480bf`；桌宠窗口标题：`Desktop Pet Task15 8e5480bf`。
- 所有输入均用 `@oai/sky` 在每次重新枚举唯一自有窗口后执行，并遵守“观察 → 单动作 → 刷新”。透明桌宠窗口不能稳定出现在普通窗口枚举中，因此先只读取得自有桌宠 HWND，再由 Sky 捕获；未操作其他窗口。
- 截图不含 API Key 值、个人照片或用户文件路径。上传素材是仓库内非个人合成 fixture。

## 2. 七个剩余认领模板的真实动态预览

`cat-misty` 已在首轮完成认领、释放和重领；本次逐个打开其余七个模板。每个预览均由无障碍状态确认“呼吸与微动预览已准备好”，再间隔约 1.6 秒取两帧。两帧 SHA256 均不同；original 视图人工检查均为明确猫科轮廓，脸眼保持稳定，胸腹有轻微呼吸，没有不透明底色、绿边或部件缝隙。

| 模板 | 预览状态与人工观察 | 帧 1 SHA256 | 帧 2 SHA256 | 证据 |
|---|---|---|---|---|
| `cat-tangerine` 橘子 | ready；橘色短毛猫，猫耳、短吻与胸腹完整 | `68c40665aca2ad7e9baca1782dc05002ccb7f359991c62825de71a80c096de81` | `9eb2ab22026c8d55bbca7e922a7d3312a9d155f8d9fd582d89e072b485021e00` | [帧 1](./adoption-cat-tangerine-帧1.jpg) / [帧 2](./adoption-cat-tangerine-帧2.jpg) |
| `cat-dumpling` 团子 | ready；明确折耳长毛猫，短猫吻、圆脸、脸眼与胸腹连续 | `0f9cadfbb4d92a577309091505fd55dd8b2c162224dff8949c7b550a794fd776` | `6f4edbfe915868dac8054c589328d9c3f598aca743ca27216393d8fc339a372b` | [帧 1](./adoption-cat-dumpling-帧1.jpg) / [帧 2](./adoption-cat-dumpling-帧2.jpg) |
| `cat-ink` 墨墨 | ready；黑猫轮廓、猫耳与眼部清楚，胸腹完整 | `2083b85a7c9eb087436887318ebe9a99d13d37babbfa6dccc3b9af5573285905` | `dbf851d7d4d90fb01b344caf37394c02d85c011f6f6f1e2a60ae8c2469862cb6` | [帧 1](./adoption-cat-ink-帧1.jpg) / [帧 2](./adoption-cat-ink-帧2.jpg) |
| `cat-cloud` 云朵 | ready；长毛猫脸、眼与胸毛连续，无透明缝隙 | `61d37ec7e7d00cd5c16a10a18fe5310b0b853096ff39e0da36ca4422bf41d922` | `dee1a36609776336a3846731cdb27798e0e2bda23f7d0f2cf4bd0c40e592f26f` | [帧 1](./adoption-cat-cloud-帧1.jpg) / [帧 2](./adoption-cat-cloud-帧2.jpg) |
| `cat-chestnut` 栗子 | ready；棕色猫耳、短吻与胸腹可辨，轮廓连续 | `9349825a42f061107b030cb741324a7c7c3ea2bf8be109dbff5048b329df56f4` | `4688617d13e9899268bbe233d51d8f4f999cd51f4c7e4932a2bd39b05f6fbfd0` | [帧 1](./adoption-cat-chestnut-帧1.jpg) / [帧 2](./adoption-cat-chestnut-帧2.jpg) |
| `cat-sesame` 芝麻 | ready；明确立耳黑白猫，猫科头骨、短猫吻与胸腹清楚 | `e35565e8c48f3fb2803b7c0bf8ac50ad099ec3f0eeddde235bd44ce7d9818e66` | `f818ea9786dcddc779f3b21d0e5eecd76b17df32f2b0df95da12b2ef9e5367a6` | [帧 1](./adoption-cat-sesame-帧1.jpg) / [帧 2](./adoption-cat-sesame-帧2.jpg) |
| `cat-starlight` 星星 | ready；深色长毛猫，脸眼、胸毛和尾部完整 | `c9022bd168e4c767faac0934d822802d290f5504171bde3b47e3211b3706a197` | `4c5e448b18b989d94a6d3507f2b48b3604f4383a26521521cc2ab3dc77b62ec7` | [帧 1](./adoption-cat-starlight-帧1.jpg) / [帧 2](./adoption-cat-starlight-帧2.jpg) |

本节只打开预览，没有认领、删除或改变模板文件。

复审指出旧版团子/芝麻四张证据不是目标设置窗口。旧文件已先从工作树移除，再启动唯一隔离实例同名重拍；本次提交快照不保留旧图。四次正式保存前均重新枚举 HWND，并核对 PID `18780`、可执行文件 `D:\petBaby\desktop-pet\.worktrees\three-creation-paths-design\apps\desktop\src-tauri\target\debug\desktop-pet.exe`、窗口标题 `桌面宠物设置 Task15 8e5480bf`，以及 accessibility 中的目标模板名和“呼吸与微动预览已准备好”。每张截图尺寸均为 722×552，只包含该自有设置窗口；四张均以 `original` 视图人工检查，无浏览器、人物或其他应用内容。

## 3. 三来源正式桌宠持续 idle

区域指标从两帧中按各宠物 `motion-profile.json` 的 `alphaBounds`、`faceSafetyLine` 与 `breathZone` 取样。先用脸部刚性区域估计并校正整数位移与小角度整体摇摆，再分别统计脸眼区域和胸腹区域的灰度绝对差均值，以及超过 6/255 的变化像素。这样避免把整体摇摆误记成呼吸。截图为窗口真实合成帧；未把候选预览当作正式桌宠。

### 3.0 最终复审固定序列协议（执行前冻结）

正式 renderer 的 `life-v1` 呼吸周期为 2800 ms，整体 sway 周期为 5200 ms，最小共同周期为 36400 ms。最终复审在看到新结果前固定以下规则，禁止按输出挑帧：

1. 新 upload 正式宠物成为 active 且 runtime ready 后，从单调时钟 `t=0` 开始，按 400 ms 目标间隔连续采集至 `t=36400`，共 92 帧；每帧记录目标/实际时间、窗口 HWND/PID/path/title 和 SHA256。任一帧缺失、目标错误或采样抖动超过 150 ms，整组作废并从 `t=0` 重跑。
2. 使用 motion profile 和 renderer 的 contain 布局映射 `alphaBounds`、`faceSafetyLine = alphaTop + 0.4 × alphaHeight`、`breathZone`。白底合成截图以固定 RGB 距离阈值重建主体 mask 和 alpha 轮廓边界，不逐帧调阈值。
3. `life-v1` 的整体 sway 是绕 `swayPivot` 旋转加水平平移，因此只在 faceSafety alpha 轮廓上拟合刚体参数：`dx/dy = -4…4 px`，rotation `-0.8°…0.8°`、步长 `0.1°`；不拟合全局缩放，避免把局部呼吸吃进整体变换。随后对每一帧做逆变换，再分别计算 faceSafety 与 breathZone 的边界 Dice 残差和区域亮度残差。
4. 预先固定判据：faceSafety 边界残差 P95 不高于 0.03；breathZone 边界残差中位数至少 0.01 且至少为 faceSafety 中位数的 1.5 倍。人工 `original` 检查固定帧号 0/23/46/68/91，只检查脸眼静止、胸腹局部运动与缝隙，不替换量化判据。
5. 输出全部 92 帧、采样 manifest、可复算脚本和 JSON 指标。若固定序列未达到判据，则记为真实产品缺陷，先写 renderer/profile RED 再最小修复；不得只改文档或换一对“好看”的帧。

| 来源 | 正式桌宠双帧 SHA256 | 刚性校正 | 脸眼区域 | 胸腹区域 | 结论与限制 |
|---|---|---|---|---|---|
| 引导组合 | `662bf7ea…f860c4b` / `751d2b37…25eb9f3` | `dx=7, dy=-1, rot=1.2°` | 均值 4.049；3589/19848（18.08%） | 均值 6.201；2936/9603（30.57%） | 胸腹均值为脸眼 1.53 倍；脸眼只随整体刚性摇摆，无局部呼吸形变；无缝隙 |
| 直接认领 | `f8841bf9…6c94217` / `43c37784…033cd2` | `dx=-1, dy=0, rot=-0.4°` | 均值 2.507；1218/11476（10.61%） | 均值 3.739；2835/14657（19.34%） | 胸腹均值为脸眼 1.49 倍；脸眼稳定、胸腹持续变化；无缝隙 |
| 上传创建 | `c7783e5a…7a2d436` / `7c26bb10…8b449ca` | `dx=1, dy=0, rot=-0.1°` | 均值 2.234/255；1810/17918（10.10%） | 均值 1.344/255；501/18957（2.64%） | 这是旧两帧初步观察；未据此声称区域量化通过。最终复审改用下述预冻结 92 帧全周期协议，结果已通过 |

正式桌宠证据：

- 引导组合：[帧 1](./composer-正式桌宠-帧1.jpg) / [帧 2](./composer-正式桌宠-帧2.jpg)
- 直接认领：[帧 1](./adoption-正式桌宠-帧1.jpg) / [帧 2](./adoption-正式桌宠-帧2.jpg)
- 上传创建完整周期：[帧 1](./upload-正式桌宠-周期帧1.jpg) / [帧 2](./upload-正式桌宠-周期帧2.jpg)

### 3.1 组合候选首帧与正式桌宠轮廓

[持久候选首帧复现](./composer-持久候选首帧.png) 与 [正式桌宠首帧](./composer-正式桌宠-帧1.jpg) 使用同一已落盘 body；源 body SHA256 均为 `096876497d42ea7a241abfdb70196da64663ada03a6e66e2d5f12b64bd3691d2`。original 视图对照确认猫耳、脸、胸腹、腿和尾部轮廓一致。正式帧因真实 motion renderer 的呼吸与摇摆产生尺度/位置变化，不存在部件缺失或画布缝隙。原始 GUI 候选截图未在首轮持久保存，因此这里明确使用同一持久候选的可复现首帧，不将其伪装为旧截图。

### 3.2 upload 正式运行时固定序列结果

最终复审使用同一隔离 identifier `com.desktop-pet.task15-8e5480bf`，通过 UI 仅提交一次仓库合成 fixture。正式对象为 pet `pet-61dc-18caba5f44e19394-3`、session `session-61dc-18caba5f44e15578-2`、job `job-61dc-18cabcddc56e3cf8-4`，展示名 `Task15上传运动证据8e5480bf`。取证期间它保持 active，未删除、未切换、未再次调用 provider。

debug capture 仅在 `debug_assertions` 且精确环境变量 `DESKTOP_PET_TASK15_CAPTURE=1` 时省略 `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`；同一 `pet` label、正式 SQLite active pet、正式 manifest/body/profile 和 renderer 均未改变。窗口先由正常产品拖动从 `(1573,460)` 分五个 15 px 小步移动到 `(1491,460)`；首次 140 px 尝试在 Sky ownership preflight 被拒绝，未产生输入。重启后持久位置恢复为 `(1491,460)`，client/outer 均为 420×520，DPI 96、DPR 1、monitor 不变，Sky 正常返回完整 420×520 WGC。序列期间 PID `18584`、HWND `70059754`、path/title/origin/尺寸保持不变，未 reload、switch、rebuild、drag、resize 或 focus。

- 采样：`2026-08-11T11:54:54.931Z` 至 `11:55:31.449Z`，92/92 帧，目标间隔 400 ms，最大抖动 65.391 ms（上限 150 ms）。
- 正式资产 SHA256：manifest `6bb76d09475821b9c8403d00f7f7f828dc19eb1fc4582a4793e353944c31a7de`；body `1975c8b5f5234efdde207dd0a6b34d87981120567241854cff78e9157168f7f0`；motion profile `36361e27af3c4d28dd0d01ef123a478668ef476c8315c9078b4445766e511ddd`。
- 固定 RGB 距离 18 对纯自有 settings 背景做白底重建；所有帧使用同一阈值。formal body neutral reference 通过相同 contain 布局生成，SHA256 `cbf7db3e15c366cb7972626a36aba714ce4daf47e269c8c3a0eb3e716555a24f`。
- 首次分析 RED 的根因在证据管线：仅替换完全相等的浅蓝背景会保留 JPEG 边缘；此外随机相位首帧作为 neutral 时，相对 sway 可达 1.4°，超出预冻结 ±0.8°拟合范围。契约测试先 RED，再改为显式 formal neutral reference；renderer、motion profile、判据和阈值均未修改。
- 最终刚体拟合 `dx=-3…1`、`dy=0`、rotation `-0.6°…0.7°`。faceSafety 边界残差中位数 `0.01281`、P95 `0.02671 ≤ 0.03`；breathZone 中位数 `0.04587 ≥ max(0.01, 1.5 × 0.01281) = 0.01921`，两项均通过。
- 固定帧号 0/23/46/68/91 已用 `original` 人工检查：猫耳、脸、眼、鼻口保持同一局部几何，只随整体刚性 sway；胸腹轮廓有克制周期变化；未见透明缝隙、静态降级或外部窗口/人物内容。

可复核材料：[采样清单](./upload-固定序列/capture-manifest.json)、[neutral reference](./upload-固定序列/neutral-body-reference.jpg)、[指标 JSON](./upload-固定序列/固定序列指标.json)、[复算脚本](./固定序列运动分析.ps1)、[契约测试](./固定序列运动分析契约测试.ps1)，以及 [帧 0](./upload-固定序列/frame-000.jpg)、[帧 23](./upload-固定序列/frame-023.jpg)、[帧 46](./upload-固定序列/frame-046.jpg)、[帧 68](./upload-固定序列/frame-068.jpg)、[帧 91](./upload-固定序列/frame-091.jpg)。`raw/` 保留 Sky 返回的全部原始 420×520 JPEG，便于独立重建白底并核对 SHA256。

## 4. 用户确认后的精确删除与放弃

### 4.1 上传测试宠物删除

在用户对精确对象进行 action-time 确认后，仅通过 UI 删除：

- display：`Task15上传猫8e5480bf`
- pet：`pet-4504-18caae6c9290c618-3`
- session：`session-4504-18caae6c92907be0-2`
- job：`job-507c-18cab058435b6ec0-2`

删除前生产 UI 执行 `window.confirm("确定删除这只宠物吗？……")`；删除后 active 安全回退默认 Live2D，组合与认领宠物保留，正式 25 文件聚合 SHA256 仍为 `e2d0af9658e7d00dd93e6688c993dc4b5e4b086843261e1eafd6a103c7424d62`，`cat-misty` 的释放/重领闭环不受影响。

### 4.2 初始 upload 空草稿放弃

首轮发现 upload 初始步骤没有直接可见的“放弃创建”，已按 TDD 增加入口，并复用原有确认与 `creation_abandon`。用户确认后，仅放弃：

- session：`session-15ac-18cab457144739e0-3`
- placeholder pet：`pet-15ac-18cab45714477d10-4`

真实 UI 弹出“确定放弃这次创建吗？本地草稿和生成任务会被清理。”，确认后返回创建方式页并显示“已放弃创建并清理本地草稿”。精确只读核对：目标 session/pet 的数据库行和目录均不存在，`generation_jobs=0`，`creation_upload_sources=0`；没有选择文件、创建 job 或调用 provider，也没有新建 composer 草稿。

### 4.3 固定序列 upload 宠物删除

用户再次对精确对象进行 action-time 确认后，在唯一 `com.desktop-pet.task15-8e5480bf` 设置窗口中删除：

- display：`Task15上传运动证据8e5480bf`
- pet：`pet-61dc-18caba5f44e19394-3`
- session：`session-61dc-18caba5f44e15578-2`
- job：`job-61dc-18cabcddc56e3cf8-4`

删除前该卡片同时显示“当前使用”和“上传创建”；生产确认文案为“确定删除这只宠物吗？此操作会移除它的本地资料和生成任务。”。确认后 UI 显示“宠物已删除。”，目标卡消失，默认 Live2D 显示“当前使用”，组合与重领卡片保留。只读核对：精确 pet/session/job/upload source 数据库计数均为 0，对应三个私有目录均不存在；composer 与 adoption 各保留 1 条。正式 adoption 25 个 tracked blob 与 HEAD 比较为 0 mismatch，既有聚合 SHA256 仍对应 `e2d0af9658e7d00dd93e6688c993dc4b5e4b086843261e1eafd6a103c7424d62`。

### 4.4 竞态 identifier 空草稿放弃

按用户对精确对象的 action-time 确认，顺序停止第一实例后启动唯一 `com.desktop-pet.task15-i3-upload-8e5480bf` 设置窗口。恢复 upload 入口前只读核对：session `session-235c-18cab9586249b234-1` 与 placeholder pet `pet-235c-18cab958624a0054-2` 各 1 条，session 为 `draft`，job/source 均为 0，未选择文件且没有私有目录。

恢复页直接显示“放弃创建”；确认文案为“确定放弃这次创建吗？本地草稿和生成任务会被清理。”。确认后返回三路径页并显示“已放弃创建并清理本地草稿。”。只读核对目标及该隔离库全部 pet/session/job/source 均为 0，目标目录不存在；未选择文件、调用 provider 或创建其他草稿，也未删除 app-data 根。

## 5. 证据边界

- 第 4.1 与 4.3 节的两个 upload 测试宠物均只在用户分别确认精确对象后删除；第 4.2 与 4.4 节的两个空 upload 草稿均只在用户分别确认后放弃。除这四项外没有执行其他删除或放弃。
- 第 3.2 节的 92 帧、neutral reference、指标与 original 检查证据已在删除前完整固化；后续清理不改变已提交证据。
- 两次最终清理均通过唯一自有 settings 窗口完成；各自 PID 树随后精确停止，14321/14322 均释放。没有递归删除任何 app-data 根，也没有对 API Key 做数据库查询、输出或记录。
