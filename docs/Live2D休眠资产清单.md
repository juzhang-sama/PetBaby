# Live2D 休眠资产清单

> 归档日期：2026-08-20
> 背景：项目技术路线由 Live2D 变更为像素风照片分身（lk888 生成 + 像素化后处理）。
> Live2D 被确认为**今后迭代的技术方向**，因此相关代码、脚本、测试**全部保留**，
> 仅将依赖特殊环境（cv2 / WebGL）的失败测试标记为 skip 归档。
> 本文档是恢复时的唯一索引。

## 一、活跃引擎层（测试全绿，勿删，勿改）

这些是 Live2D 迭代的核心引擎，代码健康、测试全绿，随时可用：

| 位置 | 内容 |
|---|---|
| `apps/desktop/src/runtime-live2d/` | 渲染器 `live2d-renderer.ts`、Cubism 生命周期、动作状态机 `cat-motion-evidence.ts`、参数混合器 `parameter-mixer.ts`、动作控制器、微动作 `micro-motion.ts`、技术探针 `probe.ts`、模型加载器 |
| `apps/desktop/src/runtime-assets/` | 资产加载器 `live2d-asset-loader.ts`、v5 空间 manifest、`cat-motion-spatial-profile.ts`、`cat-character-manifest.ts`、`cat-body-module-contract` |

**依赖契约**：`.vendor/live2d-cubism-sdk/`（Core + Framework，CUBISM_SDK_ROOT 或 prepare:cubism 注入）。

## 二、休眠资产层（代码健康，环境缺依赖）

### A. Cubism 资产生产线（脚本，需要 cv2/OpenCV）

| 脚本 | 用途 |
|---|---|
| `scripts/构建标准猫角色包.ps1` | **核心打包管线**：v5 包构建、路径穿越防护、SHA256 校验、动作曲线校准（`Set-MotionCurveValues`）、staging/backup 原子发布。内含 12 个 Cubism 参数映射（ParamEyeLOpen/ParamEarL/ParamBreath/ParamBodyStretch 等）与命中区域约束（ArtMeshBody/ArtMeshTail） |
| `scripts/修复Cubism纹理透明孔.py` | **算法沉淀**：floodFill 一次洪泛区分连通外边界与封闭孔（避免逐轮膨胀上千次的性能优化）+ connectedComponentsWithStats + inpaint(TELEA) 修补 |
| `scripts/生成标准猫动作资源.ps1` | 标准猫动作资源生成 |
| `scripts/验证猫咪形体模块.ps1`（388 行）、`验证猫咪角色包.ps1` | 模块/角色包校验 |
| `scripts/准备CubismSDK.ps1` | SDK 准备（npm run prepare:cubism 调用） |
| `scripts/生成标准猫完整底图工作稿.py`（500 行）、`测试标准猫遮挡补画.py`（335 行） | 底图工作稿与遮挡补画（含纹理补画算法） |

### B. 照片分身 × Live2D 集成层

| 位置 | 内容 |
|---|---|
| `apps/desktop/src/settings/photo-avatar-live2d-preview.ts`（18KB） | 照片分身 Live2D 预览集成组件：ports 注入模式、动作证据审计、WebGL 帧读取。前端已不引用，是"照片分身 Live2D 化"的集成样板 |
| `apps/desktop/照片分身运行时验收.html` | 浏览器验收夹具 |
| `scripts/录制照片分身动作证据.ps1`（201 行）、`照片分身真实20样本验收.ps1`（286 行）、`照片分身动作证据驱动.mjs` | 照片分身动作证据录制与 20 样本验收 |
| `scripts/验证照片分身确定性纹理合成.ps1`（179 行） | 纹理确定性合成校验 |

## 三、归档测试层（已 skip，恢复时移除 .skip）

| 文件 | skip 的用例 | 原因 | 恢复前提 |
|---|---|---|---|
| `src/runtime-assets/Cubism纹理透明孔修复.test.ts` | 整个 describe（2 个用例） | 调 `修复Cubism纹理透明孔.py` 需要 cv2 | 使用装有 cv2 的解释器（如 `D:\DevTools\Python312`） |
| `src/runtime-assets/cat-standard-package-contract.test.ts` | 3 个走完整构建的用例 | 同上（管线内嵌 UV 修复） | 同上；2 个"拒绝路径"用例保留在跑 |
| `src/settings/photo-avatar-e2e.test.ts` | "mounts the browser fixture..." 1 个用例 | 验收夹具入口为 Live2D 预览 | Live2D 预览回归；API 转发用例保留在跑 |
| `src/settings/photo-avatar-live2d-preview.test.ts` | 9 个需真实渲染的用例（含 it.each × 3） | 无头测试环境无 WebGL/GPU，渲染空白帧 | 提供可渲染 WebGL 的测试环境；8 个"拒绝路径"用例保留在跑 |

## 四、恢复步骤（Live2D 迭代重启时）

1. 给测试环境装 OpenCV：`D:\DevTools\Python312 -m pip install opencv-python`，
   或把相关测试的 `execFileSync("python", ...)` 改为指向装有 cv2 的解释器。
2. 为 Live2D 渲染类测试提供 WebGL 上下文（如 puppeteer/headless-gl 夹具），
   或按需将像素级断言降级为"上下文存在"断言。
3. 移除上文表格中所有 `.skip` 标记。
4. 跑 `npm test`，确认 17 个用例恢复绿色。

## 五、关键知识索引（踩坑沉淀，恢复前必读）

- **Cubism 参数契约**：`构建标准猫角色包.ps1` 中 `$parameterMap`（12 参数）与 `$requiredEdges`/命中区域映射。
- **UV 修复性能优化**：`修复Cubism纹理透明孔.py` 用"加一圈透明边界 + 一次洪泛"代替逐轮膨胀。
- **动作证据审计**：`cat-motion-evidence.ts` 的 neutral/peak/fallback 三阶段 + 中断状态（interrupted-pet/interrupted-drag）。
- **渲染器生命周期**：`live2d-renderer.ts` 的 context-lost 处理、ready 状态 CAS、动作中断状态机。
- **v5 资产包结构**：`cat-spatial-manifest.ts` 的 schemaVersion=5、renderer=cat-spatial-live2d-v1、motion-spatial-profile 等字段。
