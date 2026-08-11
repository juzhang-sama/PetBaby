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
| `cat-dumpling` 团子 | ready；浅色猫脸眼清楚，胸腹与四肢连续 | `bfd7532d5a05c421e99e57fe51c8b1ca8340fd7f474c67aac90e69eebe358df0` | `fe2f2a17b5f191b30ab3577b56bcb4e0eae4d22ac865accda85e07bcdff1997b` | [帧 1](./adoption-cat-dumpling-帧1.jpg) / [帧 2](./adoption-cat-dumpling-帧2.jpg) |
| `cat-ink` 墨墨 | ready；黑猫轮廓、猫耳与眼部清楚，胸腹完整 | `2083b85a7c9eb087436887318ebe9a99d13d37babbfa6dccc3b9af5573285905` | `dbf851d7d4d90fb01b344caf37394c02d85c011f6f6f1e2a60ae8c2469862cb6` | [帧 1](./adoption-cat-ink-帧1.jpg) / [帧 2](./adoption-cat-ink-帧2.jpg) |
| `cat-cloud` 云朵 | ready；长毛猫脸、眼与胸毛连续，无透明缝隙 | `61d37ec7e7d00cd5c16a10a18fe5310b0b853096ff39e0da36ca4422bf41d922` | `dee1a36609776336a3846731cdb27798e0e2bda23f7d0f2cf4bd0c40e592f26f` | [帧 1](./adoption-cat-cloud-帧1.jpg) / [帧 2](./adoption-cat-cloud-帧2.jpg) |
| `cat-chestnut` 栗子 | ready；棕色猫耳、短吻与胸腹可辨，轮廓连续 | `9349825a42f061107b030cb741324a7c7c3ea2bf8be109dbff5048b329df56f4` | `4688617d13e9899268bbe233d51d8f4f999cd51f4c7e4932a2bd39b05f6fbfd0` | [帧 1](./adoption-cat-chestnut-帧1.jpg) / [帧 2](./adoption-cat-chestnut-帧2.jpg) |
| `cat-sesame` 芝麻 | ready；黑白猫耳、短猫吻与胸腹明确，不呈犬科轮廓 | `b72dde0e8107fc30a13e698a95d0158a8493595d07c49ae5c46265d34d101d5e` | `35cd0a9ab5244077247223c8871ba46fe3daf4bfaf38c7061adc5b45d22de5b9` | [帧 1](./adoption-cat-sesame-帧1.jpg) / [帧 2](./adoption-cat-sesame-帧2.jpg) |
| `cat-starlight` 星星 | ready；深色长毛猫，脸眼、胸毛和尾部完整 | `c9022bd168e4c767faac0934d822802d290f5504171bde3b47e3211b3706a197` | `4c5e448b18b989d94a6d3507f2b48b3604f4383a26521521cc2ab3dc77b62ec7` | [帧 1](./adoption-cat-starlight-帧1.jpg) / [帧 2](./adoption-cat-starlight-帧2.jpg) |

本节只打开预览，没有认领、删除或改变模板文件。

## 3. 三来源正式桌宠持续 idle

区域指标从两帧中按各宠物 `motion-profile.json` 的 `alphaBounds`、`faceSafetyLine` 与 `breathZone` 取样。先用脸部刚性区域估计并校正整数位移与小角度整体摇摆，再分别统计脸眼区域和胸腹区域的灰度绝对差均值，以及超过 6/255 的变化像素。这样避免把整体摇摆误记成呼吸。截图为窗口真实合成帧；未把候选预览当作正式桌宠。

| 来源 | 正式桌宠双帧 SHA256 | 刚性校正 | 脸眼区域 | 胸腹区域 | 结论与限制 |
|---|---|---|---|---|---|
| 引导组合 | `662bf7ea…f860c4b` / `751d2b37…25eb9f3` | `dx=7, dy=-1, rot=1.2°` | 均值 4.049；3589/19848（18.08%） | 均值 6.201；2936/9603（30.57%） | 胸腹均值为脸眼 1.53 倍；脸眼只随整体刚性摇摆，无局部呼吸形变；无缝隙 |
| 直接认领 | `f8841bf9…6c94217` / `43c37784…033cd2` | `dx=-1, dy=0, rot=-0.4°` | 均值 2.507；1218/11476（10.61%） | 均值 3.739；2835/14657（19.34%） | 胸腹均值为脸眼 1.49 倍；脸眼稳定、胸腹持续变化；无缝隙 |
| 上传创建 | `c7783e5a…7a2d436` / `7c26bb10…8b449ca` | `dx=1, dy=0, rot=-0.1°` | 均值 2.234/255；1810/17918（10.10%） | 均值 1.344/255；501/18957（2.64%） | 完整 2.8 秒周期内两帧不同，胸腹仍有 501 个显著变化像素；但素材胸腹近乎纯色，区域量化不足以证明“胸腹变化大于脸眼”。停止继续挑帧，保留该限制；人工 original 检查为持续微动、无缝隙和静态降级 |

正式桌宠证据：

- 引导组合：[帧 1](./composer-正式桌宠-帧1.jpg) / [帧 2](./composer-正式桌宠-帧2.jpg)
- 直接认领：[帧 1](./adoption-正式桌宠-帧1.jpg) / [帧 2](./adoption-正式桌宠-帧2.jpg)
- 上传创建完整周期：[帧 1](./upload-正式桌宠-周期帧1.jpg) / [帧 2](./upload-正式桌宠-周期帧2.jpg)

### 3.1 组合候选首帧与正式桌宠轮廓

[持久候选首帧复现](./composer-持久候选首帧.png) 与 [正式桌宠首帧](./composer-正式桌宠-帧1.jpg) 使用同一已落盘 body；源 body SHA256 均为 `096876497d42ea7a241abfdb70196da64663ada03a6e66e2d5f12b64bd3691d2`。original 视图对照确认猫耳、脸、胸腹、腿和尾部轮廓一致。正式帧因真实 motion renderer 的呼吸与摇摆产生尺度/位置变化，不存在部件缺失或画布缝隙。原始 GUI 候选截图未在首轮持久保存，因此这里明确使用同一持久候选的可复现首帧，不将其伪装为旧截图。

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

## 5. 证据边界

- 上传正式宠物已按用户确认删除，正式运行帧是在删除前从同一隔离实例捕获。
- 上传素材内部纹理很少，导致胸腹区域信号弱于脸部边缘的抗锯齿残差；本记录不把该项扩大为“胸腹相对脸眼量化通过”。
- 没有执行新的宠物删除或额外草稿放弃；没有对 API Key 做数据库查询、输出或记录。
