# 桌面宠物 M4-P1B 创建闭环实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或 `executing-plans` 执行本计划；本阶段把 M3 验证的生成链路接入桌宠主流程。

**Goal:** 把"上传宠物照片 → 生成招牌画风候选 → 主人确认 → 资产编译 → 桌面可用"做成完整闭环，支持多宠保存/切换与离线可用。

**Architecture:** 在 `apps/desktop` 主应用内实现生成链路：
- Rust 侧：`generation/`（lk888 客户端：submit/poll/download/重试/幂等，移植 M3 验证的协议）、`creation/`（创建会话与持久化）、`assets/`（色键抠图 + 质量检查 + 资产编译）
- 前端：`src/creation/` 创建流程页面（settings 窗口扩展为多页：上传/特征 → 生成进度 → 候选选择 → 确认）
- 抠图决策：生成强制浅灰背景 → **Rust 色键抠图 + 质量检查**；质量不合格标记"需校准"进入降级；rembg 增强列为迭代项（M3 结论记录色键在浅色毛的过度抠图风险，通过质量检查拦截）

**Tech Stack:** 沿用 M1-M2；新增 Rust 侧 `reqwest`（HTTP）、`image`（解码/抠图）、`sha2`（已有）。

## Global Constraints

- 生成只允许"真实宠物照片"入口（本阶段）+ 内置领养（预置资产）；参考图/引导创建入口做结构预留（UI 禁用态）。
- 照片默认仅用于生成任务；创建 UI 明示"照片会上传到第三方生成平台，平台侧结果默认保留"（M3 结论）。
- 生成任务支持进度显示、取消、断点恢复（tasks 落盘）。
- 资产编译失败降级：单图资产（带背景不透明）或进入校准流程；不阻塞应用。
- 多宠保存/切换/单宠激活（M1 PetRepository 已有基础）。
- TDD：纯逻辑（任务状态机、抠图质量、资产编译）先写失败测试。

---

## 预期文件结构（M4 新增部分）

```text
apps/desktop/src-tauri/src/
├─ generation/
│  ├─ mod.rs
│  ├─ lk888.rs  lk888.test.rs        # 供应商客户端（reqwest）
│  ├─ tasks.rs  tasks.test.rs        # 任务状态机 + 幂等 + 断点
│  └─ cutout.rs  cutout.test.rs      # 色键抠图 + 质量检查
├─ creation/
│  ├─ mod.rs
│  ├─ session.rs  session.test.rs    # 创建会话（上传→生成→确认→编译）
│  └─ profiles.rs  profiles.test.rs  # IdentityProfile/AppearanceRecipe 持久化
apps/desktop/src/
├─ creation/
│  ├─ creation-flow.ts               # 创建流程控制器
│  └─ creation-flow.test.ts
└─ settings.ts（扩展：创建入口 + 候选选择 UI）
```

### Task 1: Rust 生成客户端（移植 M3 协议）

**Files:**
- Create: `src-tauri/src/generation/mod.rs`、`lk888.rs`、`lk888.test.rs`
- Modify: `Cargo.toml`（`reqwest = "=0.12"`、`image = "=0.25"`）
- Modify: `src-tauri/src/lib.rs`（注册模块）

**Interfaces:**
```rust
pub struct Lk888Client { key: String, base: String, model: String, client: reqwest::Client }
pub struct TaskState { pub task_id: String, pub state: String, pub is_final: bool, pub result_url: Option<String>, pub error: Option<String> }
impl Lk888Client {
    pub fn submit(&self, prompt: &str, ref_image_png: &[u8], size: &str) -> Result<String, GenError>;
    pub fn poll(&self, task_id: &str) -> Result<TaskState, GenError>;
    pub fn download(&self, url: &str) -> Result<Vec<u8>, GenError>;
}
pub enum GenError { Network(String), Auth, RateLimit, Generation(String), Timeout }
```
- submit 重试 3 次（M3 验证的容错）。
- 测试：mock reqwest（用 `wiremock` 或自建 test server）校验请求体（model/prompt/params/images data url）。

- [ ] **Step 1: 失败测试**：submit 请求体、poll 状态映射、重试逻辑。
- [ ] **Step 2: 实现 + 接入 lib.rs（命令 `gen_submit`/`gen_poll` 或封装在任务层）。**
- [ ] **Step 3: 提交** `feat: rust generation client`

### Task 2: 创建域持久化（Profile/Job/Variant）

**Files:**
- Modify: `src-tauri/src/storage/migrate.rs`（迁移 v2）
- Create: `src-tauri/src/creation/profiles.rs`、`profiles.test.rs`

**Interfaces（迁移 v2 表）：**
```sql
CREATE TABLE identity_profiles (
  profile_id TEXT PRIMARY KEY,
  pet_id TEXT NOT NULL REFERENCES pets(pet_id) ON DELETE CASCADE,
  schema_version INTEGER NOT NULL,
  species TEXT NOT NULL,
  identity_mode TEXT NOT NULL,
  locked_traits TEXT NOT NULL,        -- JSON
  ref_asset_id TEXT,
  created_at TEXT NOT NULL
);
CREATE TABLE generation_jobs (
  job_id TEXT PRIMARY KEY,
  pet_id TEXT NOT NULL REFERENCES pets(pet_id) ON DELETE CASCADE,
  prompt TEXT NOT NULL,
  ref_sha256 TEXT NOT NULL,
  task_id TEXT,
  status TEXT NOT NULL,               -- pending/running/success/failed/cancelled
  result_url TEXT,
  error TEXT,
  created_at TEXT NOT NULL
);
CREATE TABLE appearance_variants (
  variant_id TEXT PRIMARY KEY,
  pet_id TEXT NOT NULL REFERENCES pets(pet_id) ON DELETE CASCADE,
  job_id TEXT REFERENCES generation_jobs(job_id),
  image_path TEXT NOT NULL,
  cutout_path TEXT,
  quality TEXT NOT NULL,              -- clean/needs-calibration
  accepted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);
```
Rust 类型 + CRUD（`GenerationRepository` 扩展或新 `CreationStore`）。

- [ ] **Step 1: 失败测试**：迁移 v2 幂等；Job 状态流转记录。
- [ ] **Step 2: 实现 + 提交** `feat: creation domain persistence`

### Task 3: 色键抠图与质量检查

**Files:**
- Create: `src-tauri/src/generation/cutout.rs`、`cutout.test.rs`

**Interfaces:**
```rust
pub fn estimate_background(rgb: &[u8], w: u32, h: u32) -> [u8; 3];
pub fn is_uniform_background(rgb: &[u8], w: u32, h: u32, bg: [u8; 3], tol: u8) -> bool;
pub fn chroma_remove(rgb: &[u8], w: u32, h: u32) -> Result<Vec<u8>, CutoutError>;  // RGBA
pub fn quality_check(rgba: &[u8], w: u32, h: u32) -> QualityReport;
// QualityReport { opaque_ratio, transparent_ratio, edge_holes: bool }
```
- 质量检查拦截"过度抠图"：透明占比异常（>0.9 或与预期不符）→ `needs-calibration`。
- 测试：构造纯色背景图验证；构造"浅色主体+浅背景"场景验证质量检查能拦截。

- [ ] **Step 1: 失败测试。**
- [ ] **Step 2: 实现 + 提交** `feat: chroma cutout with quality gate`

### Task 4: 生成任务管理（进度/取消/恢复）

**Files:**
- Create: `src-tauri/src/generation/tasks.rs`、`tasks.test.rs`
- Modify: `src-tauri/src/lib.rs`（命令：`gen_start`、`gen_status`、`gen_cancel`、`gen_list`）

**Interfaces:**
```rust
pub struct GenerationManager { store: Arc<Mutex<Storage>>, client: Lk888Client }
impl GenerationManager {
    pub fn start(&self, pet_id, prompt, ref_png, ref_sha) -> Result<String, String>; // 创建 job + submit + 落盘
    pub fn poll_all(&self) -> Vec<JobUpdate>;   // 轮询 running jobs，完成后下载+抠图
    pub fn cancel(&self, job_id) -> Result<(), String>;  // 标记 cancelled（平台无取消 API，停止轮询）
    pub fn resume(&self) -> Result<(), String>; // 启动时恢复未完成 jobs
}
```
- 后台任务：Tauri `std::thread` + 定时 poll_all（5 秒），状态经命令查询。
- 断点：jobs 表 task_id 落盘，重启 resume 续查（M3 验证模式）。

- [ ] **Step 1: 失败测试**：状态机（pending→running→success/failed/cancelled）、resume 不重复。
- [ ] **Step 2: 实现 + 接入 setup（spawn 轮询线程）。**
- [ ] **Step 3: 提交** `feat: generation job manager`

### Task 5: 创建流程 UI（settings 扩展）

**Files:**
- Modify: `apps/desktop/settings.html`、`src/settings.ts`（多页：列表 / 创建向导）
- Create: `src/creation/creation-flow.ts`、`creation-flow.test.ts`

**Interfaces:**
```ts
export interface CreationStep { id: "upload" | "traits" | "generating" | "review" | "confirm"; }
export class CreationFlow {
  start(species: "cat" | "dog"): void;
  setPhoto(file: File): Promise<void>;      // 转 PNG + 缩放到 ≤1024 + 计算 sha
  submitBatch(count: number): Promise<void>; // 生成 4 候选（调用 gen_start ×4）
  poll(): Promise<JobUpdate[]>;
  accept(variantId: string): Promise<void>; // 标记 accepted
  compile(): Promise<CompileResult>;        // 编译为运行资产
}
```
- UI：上传照片 → 可选特征（默认留空参考图模式）→ 生成进度（4 卡片进度）→ 候选网格（去背景预览）→ 选择确认 → "制作完成，出现在桌面"。
- 内置领养：预置 2 个合成宠物（复用 pet-probe 资产），跳过生成。
- 隐私提示：上传前明示"照片将上传第三方生成平台，平台侧结果默认保留"。

- [ ] **Step 1: 失败测试**：CreationFlow 状态机。
- [ ] **Step 2: 实现 settings 多页 UI + flow 控制器。**
- [ ] **Step 3: 提交** `feat: creation flow ui`

### Task 6: 资产编译（manifest + 动画参数 + 单图降级）

**Files:**
- Modify: `src-tauri/src/runtime_assets/`（新增 compiler.rs）
- Create: `src-tauri/src/runtime_assets/compiler.rs`、`compiler.test.rs`

**Interfaces:**
```rust
pub struct CompileInput { variant_id, cutout_path: PathBuf, pet_id }
pub struct CompileResult { manifest_path: PathBuf, degraded: bool }
pub fn compile_single_image(input: CompileInput, dest: PathBuf) -> Result<CompileResult, String>;
// 输出：<pet>/assets/body.png（RGBA 单层）+ manifest.json（assetType: single-image）
// 降级：cutout 质量为 needs-calibration 时，输出原图（带浅灰背景）并标记 degraded
// 闭眼层：M4 生成闭眼候选（可选开关），编译为 eye-closed.png；失败则跳过（动画层用默认闭眼）
```
- 复用 M1 manifest v1 结构（files/sha256/animation）。
- 测试：输入 RGBA → 输出 manifest 可被 parse_manifest 解析；哈希正确。

- [ ] **Step 1: 失败测试。**
- [ ] **Step 2: 实现 + 接入 CreationFlow.compile。**
- [ ] **Step 3: 提交** `feat: runtime asset compilation`

### Task 7: 多宠切换与运行时接入

**Files:**
- Modify: `apps/desktop/src/main.ts`、`src/runtime/pet-stage.ts`、`src/settings.ts`

**Interfaces:**
- `pet_get_active` → 加载 active pet 的编译资产（manifest 读取 body.png/eye-closed.png）
- PetStage 支持"从宠物资产加载"（替换测试素材路径）：`loadFromManifest(manifestPath)`
- 切换：设置页选择宠物 → `pet_set_active` → 前端重载资产
- 资产缺失/损坏 → M1 的 asset_scan 占位逻辑复用

- [ ] **Step 1: 失败测试**：manifest 资产加载路径解析。
- [ ] **Step 2: 实现 PetStage 资产化 + 切换接线。**
- [ ] **Step 3: 提交** `feat: multi-pet switch and asset loading`

### Task 8: 验收与 M4 结论

**Files:**
- Create: `docs/验证记录/M4手工验证清单.md`、`docs/验证记录/M4技术结论.md`
- Create: `scripts/执行M4检查.ps1`

**人工清单：**
| 编号 | 场景 | 预期 |
|---|---|---|
| C-01 | 创建闭环 | 上传照片→生成→选择→桌面出现宠物 |
| C-02 | 抠图质量 | 无过度抠图；质量不合格进入降级/校准 |
| C-03 | 任务进度/取消 | 进度显示、可取消、重启恢复 |
| C-04 | 多宠切换 | 保存/切换/单宠激活 |
| C-05 | 断网 | 已有宠物完全离线可用；生成提示失败 |
| C-06 | 隐私提示 | 上传前明示照片生命周期 |
| C-07 | 资产损坏 | 不阻塞启动（复用占位） |

**验收闸门：** C-01~C-07 通过；创建失败不破坏已有宠物；生成完成后完全离线可用；性能无回归（companion 采样）。

- [ ] **Step 1: 创建 M4 检查脚本与人工清单。**
- [ ] **Step 2: 全量闸门 + companion 性能回归。**
- [ ] **Step 3: 用户实测 C-01~C-07，回填清单。**
- [ ] **Step 4: 写 M4 技术结论（总体通过 → 小范围测试版就绪）。**
- [ ] **Step 5: 提交** `docs: record M4 creation loop verdict`

## M4 完成定义

1. 创建闭环：上传 → 生成 → 候选 → 确认 → 编译 → 桌面可用。
2. 生成任务可进度/取消/恢复，失败不破坏已有宠物。
3. 抠图质量闸门拦截过度抠图，降级路径可用。
4. 多宠保存/切换/单宠激活。
5. 断网完全离线可用。
6. 隐私提示与照片生命周期明示。
7. M4 技术结论 → 小范围测试版可交付（Agent/声音/主动养成等仍为后续）。
