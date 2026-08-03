# 桌面宠物 M2-P1A 生命感实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或 `executing-plans` 执行本计划；不得跨过 M1 闸门直接实现 M2 范围之外的功能。

**Goal:** 让桌面宠物摆脱"贴纸感"：引入领域事件与行为决策、无惩罚状态（energy/mood/bond）、分层运行资产、呼吸/眨眼/点击反馈动画、拖拽中断恢复，并验证渲染档位与长时间稳定性。

**Architecture:** 前端 `apps/desktop/src/behavior/`（事件、状态、决策、意图，纯逻辑可单测）+ `src/assets/`（分层资产渲染适配）；Rust 侧状态持久化复用 M1 `storage`（`state` 表）。动画完全本地运行，不依赖云端。

**Tech Stack:** 沿用 M1 锁定版本。动画用 PixiJS 8 的 `Ticker`/`Tween`（`pixi.js` 内置 tween）或轻量自研插值，不引入新框架。

## Global Constraints

- 无惩罚陪伴模式是唯一行为模式：不掉亲近度、不生病、不离家、不死亡；长时间未启动只表现为"好久不见"。
- `energy`、`mood`、`bond` 是隐藏状态，不显示数值，不制造压力。
- 互动不高频打扰：同类反馈带冷却（最小间隔）。
- P1 只支持单宠激活；行为状态按 `pet_id` 隔离。
- 所有领域数据带 `schemaVersion`；状态跨重启温和恢复（不做剧烈惩罚或丢失）。
- TDD：纯逻辑先写失败测试；动画和视觉效果用人工验收。

---

## 预期文件结构（M2 新增部分）

```text
apps/desktop/
├─ src/
│  ├─ behavior/
│  │  ├─ events.ts  events.test.ts          # 领域事件定义与事件总线
│  │  ├─ state.ts  state.test.ts            # PetState（energy/mood/bond）演化
│  │  ├─ policy.ts  policy.test.ts          # CompanionPolicy（无惩罚规则+冷却）
│  │  ├─ decision.ts  decision.test.ts      # 行为决策器：事件+状态 -> 意图
│  │  └─ intents.ts                         # BehaviorIntent 类型
│  ├─ assets/
│  │  └─ layered-sprite.ts  layered-sprite.test.ts  # 分层资产（身体/闭眼/前景）渲染
│  └─ runtime/
│     ├─ pet-stage.ts（修改：接入行为系统与动画）
│     └─ pet-animator.ts  pet-animator.test.ts  # 呼吸/眨眼/反馈动画调度
├─ public/test-assets/
│  ├─ pet-probe.png（已有）
│  └─ layered/（生成：body.png, eye-open.png, eye-closed.png, accent.png）
└─ src-tauri/src/
   └─ pets/state.rs  state.test.rs          # 状态持久化命令（Rust）
```

### Task 1: 领域事件与行为决策器

**Files:**
- Create: `apps/desktop/src/behavior/events.ts`、`events.test.ts`
- Create: `apps/desktop/src/behavior/intents.ts`
- Create: `apps/desktop/src/behavior/decision.ts`、`decision.test.ts`

**Interfaces:**
```ts
export type PetEvent =
  | { type: "head-clicked" } | { type: "body-clicked" }
  | { type: "double-clicked" } | { type: "drag-start" } | { type: "drag-end" }
  | { type: "pet-shown" } | { type: "pet-hidden" }
  | { type: "idle-tick"; elapsedMs: number };

export type BehaviorIntent =
  | { type: "blink"; intensity: 1 } | { type: "look"; target: "front" | "left" | "right" }
  | { type: "react-happy" } | { type: "react-curious" }
  | { type: "carried" } | { type: "landed" } | { type: "sleep" } | { type: "awake" };

export interface DecisionInput { event: PetEvent; state: PetStateSnapshot; policy: PolicySnapshot; }
export function decide(input: DecisionInput): BehaviorIntent[];
```

- [ ] **Step 1: 失败测试**：点击事件产生对应意图；`double-clicked` 触发 `react-happy`；`drag-start` 触发 `carried`；`idle-tick` 在超时后触发 `look`（随机方向）；连续同类事件受冷却限制（policy）。
- [ ] **Step 2: 确认失败**：`npm test -- src/behavior/decision.test.ts`。
- [ ] **Step 3: 实现**：事件类型、意图类型、`decide` 纯函数（依赖 state/policy，见 Task 2）。
- [ ] **Step 4: 提交** `feat: domain events and behavior decision`。

### Task 2: 无惩罚状态与 CompanionPolicy

**Files:**
- Create: `apps/desktop/src/behavior/state.ts`、`state.test.ts`
- Create: `apps/desktop/src/behavior/policy.ts`、`policy.test.ts`
- Create: `apps/desktop/src-tauri/src/pets/state.rs`、`state.test.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`

**Interfaces:**
```ts
export interface PetStateSnapshot {
  schemaVersion: 1; petId: string;
  energy: number;   // 0..1，隐藏状态
  mood: number;     // 0..1
  bond: number;     // 0..1
  lastSeenAt: string; lastInteractionAt: string;
}
export function evolveState(snapshot: PetStateSnapshot, now: Date, elapsedMs: number): PetStateSnapshot;
// 规则：空闲时 energy 缓慢下降；互动时 mood 上升、bond 缓慢上升；无惩罚（不惩罚长时间未上线）
export interface PolicySnapshot { cooldowns: Record<string, number>; }
export function cooldownRemaining(policy: PolicySnapshot, key: string, now: number): number;
```
Rust：`pets/state.rs` 提供 `state_load(pet_id)` / `state_save(pet_id, json)`（存 `state` 表，key=`pet:{id}:behavior`），Tauri 命令 `pet_state_load`、`pet_state_save`。

- [ ] **Step 1: 失败测试（TS）**：energy 随时间下降且不归零突变；互动提升 mood 有上限；bond 缓慢增长；长时间未启动不产生负面效果（"好久不见"由 decision 层处理）。
- [ ] **Step 2: 失败测试（Rust）**：state 表 round-trip 保存/加载。
- [ ] **Step 3: 实现 TS 状态演化与冷却**。
- [ ] **Step 4: 实现 Rust 持久化 + 命令**。
- [ ] **Step 5: 提交** `feat: no-punishment pet state and policy`。

### Task 3: 分层宠物资产与渲染适配

**Files:**
- Modify: `apps/desktop/scripts/创建探针宠物素材.ps1`（新增分层输出）
- Create: `apps/desktop/public/test-assets/layered/`（生成 body/eye-open/eye-closed/accent 四图）
- Create: `apps/desktop/src/assets/layered-sprite.ts`、`layered-sprite.test.ts`
- Modify: `apps/desktop/src/runtime/pet-stage.ts`

**Interfaces:**
```ts
export interface LayeredAsset { bodyUrl: string; eyeOpenUrl: string; eyeClosedUrl: string; accentUrl?: string; }
export class LayeredSprite {
  constructor(assets: LayeredAsset);
  async mount(stage: Container, width: number, height: number): Promise<void>;
  setEyesOpen(open: boolean): void;
  setBreathPhase(phase: number): void;   // 0..1
  setFlip(flipped: boolean): void;
  setCarried(carried: boolean): void;    // 拖拽时轻微上移+缩小
}
```

- [ ] **Step 1: 扩展素材脚本**：生成四张 512x512 透明 PNG（身体/睁眼/闭眼/装饰），脚本参数化输出目录。
- [ ] **Step 2: 失败测试**：LayeredSprite 状态切换（eyes 切换、flip 翻转锚点）纯逻辑。
- [ ] **Step 3: 实现 LayeredSprite**（PixiJS Container + 4 个 Sprite，锚点与 z 序固定：body 底、accent 顶）。
- [ ] **Step 4: PetStage 默认改用分层资产**（测试素材路径切换，健康检查逻辑保留）。
- [ ] **Step 5: 提交** `feat: layered pet assets with rendering adapter`。

### Task 4: 呼吸、眨眼与点击反馈动画

**Files:**
- Create: `apps/desktop/src/runtime/pet-animator.ts`、`pet-animator.test.ts`
- Modify: `apps/desktop/src/runtime/pet-stage.ts`、`apps/desktop/src/behavior/decision.ts`

**Interfaces:**
```ts
export class PetAnimator {
  constructor(driver: { setEyesOpen(b: boolean): void; setBreathPhase(p: number): void; scaleSquash(f: number): void; shift(dx: number, dy: number): void; });
  start(): void; stop(): void;
  setMode(mode: "idle" | "interact" | "carried"): void;
  setIntent(intent: BehaviorIntent): void;   // react-happy -> 弹跳；react-curious -> 歪头
}
// 呼吸：sin 周期 4 秒，幅度 2%；眨眼：随机间隔 blinkMsMin..blinkMsMax，闭眼 150ms；点击反馈：位移+缩放弹性恢复
```

- [ ] **Step 1: 失败测试**：呼吸相位连续；眨眼调度器在 min/max 窗口内触发且每次闭眼有限时长；intent 切换重置动作状态。
- [ ] **Step 2: 确认失败**。
- [ ] **Step 3: 实现 PetAnimator**（requestAnimationFrame 或 PixiJS Ticker 驱动，停止时释放）。
- [ ] **Step 4: 接入 PetStage**：`active`（点击时 60fps 档）→ `companion`（24fps 呼吸眨眼）→ `still`（静止）档位联动。
- [ ] **Step 5: 提交** `feat: breathing, blinking and click feedback animation`。

### Task 5: 拖拽中断、抱起与落地

**Files:**
- Modify: `apps/desktop/src/runtime/pet-stage.ts`、`apps/desktop/src/runtime/pet-animator.ts`
- Modify: `apps/desktop/src/behavior/decision.ts`（已有 drag-start/drag-end 事件）

**Interfaces:**
- `drag-start` -> `carried` 意图：动画暂停呼吸、宠物轻微上移；窗口跟随鼠标（M0 手动拖动已有）
- `drag-end` -> `landed` 意图：缩放弹性恢复（squash & stretch 一次），1 秒后回到 idle
- 拖拽中命中区域不重算（性能），拖拽结束重算一次

- [ ] **Step 1: 失败测试**：carried 时动画模式切换；landed 后 1 秒回 idle（用假定时器）。
- [ ] **Step 2: 实现并接入**。
- [ ] **Step 3: 提交** `feat: carried and landed behavior during drag`。

### Task 6: 渲染档位接入运行时

**Files:**
- Modify: `apps/desktop/src/runtime/pet-stage.ts`、`apps/desktop/src/runtime/render-scheduler.ts`（无接口变化）

**Interfaces:**
- `active`：60fps，交互反馈 + 点击动作
- `companion`：24fps，呼吸 + 随机眨眼
- `still`：渲染一帧后停止，仅保留最后画面
- `paused`：窗口隐藏或全屏时完全停止
- 档位切换来源：指针按下/释放（active/still）、空闲定时（companion）、全屏/隐藏（paused）

- [ ] **Step 1: 失败测试**：调度器档位联动动画模式映射（companion -> 动画运行，still -> 动画停止）。
- [ ] **Step 2: 接入 PetStage 生命周期**。
- [ ] **Step 3: 提交** `feat: render tier integration with behavior loop`。

### Task 7: 按需校准窗口

**Files:**
- Create: `apps/desktop/src/calibration.html`、`apps/desktop/src/calibration.ts`
- Modify: `apps/desktop/src-tauri/tauri.conf.json`（注册 calibration 窗口）、`lib.rs`（托盘菜单加"校准"）

**Interfaces:**
- 简化校准：三滑杆（呼吸幅度 0..5%、眨眼间隔缩放 0.5x..2x、点击反馈强度 0..1）+ 实时预览（调用 PetAnimator）
- 校准参数保存到 `state` 表（key=`pet:{id}:calibration`），JSON schema 带版本

- [ ] **Step 1: 创建校准页 + 窗口注册 + 托盘项**。
- [ ] **Step 2: 校准参数读取/保存命令（Rust state.rs 扩展）**。
- [ ] **Step 3: 实时预览接线**（校准窗口内嵌预览画布）。
- [ ] **Step 4: 提交** `feat: on-demand calibration window`。

### Task 8: 体验测试、稳定性与 M2 结论

**Files:**
- Create: `apps/desktop/docs/验证记录/M2手工验证清单.md`
- Create: `apps/desktop/docs/验证记录/M2技术结论.md`
- Modify: `apps/desktop/docs/验证记录/性能基线.md`（companion 档位采样）

**Interfaces:**
- 人工清单：呼吸可感知、眨眼自然、点击反馈不烦人、拖拽顺滑、档位切换无闪烁、状态跨重启温和恢复
- 性能：`companion` 档位 300 秒采样（复用测量脚本）
- 长时间：挂机 ≥2 小时分段采样（完整 8 小时作为 M2 后置人工项）

- [ ] **Step 1: 创建 M2 检查脚本**（`scripts/执行M2检查.ps1`，结构同 M1）。
- [ ] **Step 2: 创建人工清单并引导用户补测**。
- [ ] **Step 3: companion 档位性能采样**。
- [ ] **Step 4: 写 M2 技术结论**（"贴纸感"闸门评估：测试者能感知生命感、互动不高频打扰、状态跨重启温和恢复、性能预算通过）。
- [ ] **Step 5: 最终闸门 + 提交** `docs: record M2 life-feel verdict`。

## M2 完成定义

1. 领域事件 -> 行为决策 -> 动画意图链路有单测覆盖。
2. energy/mood/bond 无惩罚演化与跨重启恢复有测试与人工证据。
3. 分层资产渲染（身体/眼/装饰）可用。
4. 呼吸、眨眼、点击反馈、拖拽抱起/落地均有实现与人工验收。
5. `active/companion/still/paused` 档位与行为循环联动。
6. 校准窗口可按需打开并保存参数。
7. companion 档位性能采样与稳定性记录。
8. M2 结论冻结 M3 输入（生成链路实验所需的资产分层规范与身份约束）。
