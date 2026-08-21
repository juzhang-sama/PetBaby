# 第一轮测试：Windows 双窗口模式验收

## 验收结论

截至 2026-08-12，Windows 双窗口模式的自动化验收门禁通过：Rust `all-targets` 共 674 项通过、0 failed、0 ignored，前端共 921 项通过、4 项按既有环境条件跳过，TypeScript 类型检查和生产构建均通过；窗口模式 focused 矩阵也全部通过，串行 `window_mode` 组没有再出现等待 ACK 导致的挂起。

本结论只覆盖纯 reducer、Fake Win32 adapter、FakeIo/controller、Tauri 接线源码契约、Vitest 假 DOM/mock port 和生产静态构建。真实桌宠进程没有在本轮启动，真实 HWND 没有发生挂载或切换；陪伴置顶、Win+D、前台全屏、锁屏、双屏、点击/拖动、非 100% 尺寸、切换中退出、真实 bottom fallback 提示和 Explorer 重启均未执行。因此当前状态为：**双窗口模式自动化验收通过，Windows/Tauri 真人矩阵未执行，不能声明双窗口模式整体验收或发布验收完成。**

## 基线、环境与安全边界

- 日期：2026-08-12（Asia/Shanghai）。
- 验收基线：`9f4cd4745981f82a4f9a7a8d9eef7a513b757baf`（`fix: make window mode cancellation and revisions atomic`）。
- 系统：Microsoft Windows 11 Pro 10.0.26200，build 26200。
- 当前只读显示器枚举：1 台显示器，`DISPLAY1`，1920×1080，工作区 1920×1032；因此本轮没有真实双屏证据。
- Rust 命令目录：`apps/desktop/src-tauri`；npm 命令目录：`apps/desktop`。
- 全量与 focused 命令严格串行执行，避免 Cargo target、SQLite 夹具或 CPU 竞争干扰结果。
- 自动化使用测试专用临时文件/数据库、Fake Win32 API、FakeIo、假 DOM 和 mock port；未读取或写入真实用户偏好、真实用户数据库或真实 app-data。
- 未启动或强制启动真实 `desktop-pet`，未重启 Explorer，未改变桌面 Shell、窗口父子关系、Z 序或系统偏好。
- 未使用 Spy++，也没有把未取得的 Spy++ 结果写入本文。

## 全量门禁结果

| 顺序 | 工作目录 | 命令 | 结果 |
| --- | --- | --- | --- |
| 1 | `apps/desktop/src-tauri` | `cargo test --all-targets --no-fail-fast` | exit 0；674 passed、0 failed、0 ignored、0 measured；binary target 另为 0 tests；92.48 秒 |
| 2 | `apps/desktop/src-tauri` | `cargo check --all-targets` | exit 0；dev/test targets 检查通过 |
| 3 | `apps/desktop` | `npm test -- --run` | exit 0；80 个测试文件通过、1 个条件跳过；921 passed、4 skipped、0 failed；13.11 秒 |
| 4 | `apps/desktop` | `npm run typecheck` | exit 0；`tsc --noEmit` 通过 |
| 5 | `apps/desktop` | `npm run build` | exit 0；`tsc --noEmit && vite build` 通过；Vite 8.2.0 转换 80 个模块，141 ms |

构建产物现场核对：

| 入口 | 大小 | 结果 |
| --- | ---: | --- |
| `apps/desktop/dist/index.html` | 650 bytes | 存在 |
| `apps/desktop/dist/settings.html` | 22,276 bytes | 存在 |
| `apps/desktop/dist/assets/` | 11 个文件 | 存在 |

### Warning 与 skip

- `cargo test --all-targets` 的 lib target 报告 10 条 warning：1 条 unused import、多条既有 unused/dead-code，以及 1 条 Windows linker stdout 信息；lib test 另报告 `src/generation/lk888.rs:53` 的既有 `unfulfilled_lint_expectations`。
- `cargo check --all-targets` 的 lib target 报告 9 条 unused/dead-code warning，lib test 同样报告 `src/generation/lk888.rs:53`。warning 没有造成测试或检查失败；本 Task 不修改产品代码，也没有用自动修复命令处理它们。
- Vitest 的 4 项 skip 全部位于 `src/runtime-live2d/cubism-runtime-lifecycle.test.ts`，由本机缺少本地 Cubism SDK 时的 `it.skipIf(!hasLocalCubismSdk)` 触发：adapter 销毁后再次初始化、单 adapter WebGL shader context 释放、共享 context 最终释放，以及 context loss 时 shader/listener 清理。它们与窗口模式逻辑无关，但仍是当前环境的覆盖缺口。
- focused Cargo 命令重复显示同一 `lk888.rs:53` warning；没有新的 focused warning 或 skip。

## 窗口模式 focused 跨层矩阵

### Rust

| 命令 | 结果 | 覆盖层级 |
| --- | --- | --- |
| `cargo test --lib window_mode::tests:: -- --test-threads=1` | 47 passed、0 failed、0 ignored；0.33 秒 | controller 事务、pause/resume ACK、持久化补偿、显隐、启动恢复、Explorer 恢复、五次重试、取消、shutdown、revision；串行无挂起 |
| `cargo test --lib platform::windows::tests:: -- --test-threads=1` | 22 passed、0 failed、0 ignored | 快照、WorkerW、BottomFallback、2px readback、DPI 预检、完整 restore、存活探测与失败矩阵；均为 Fake API |
| `cargo test --lib windowing::mode_tests:: -- --test-threads=1` | 20 passed、0 failed、0 ignored | companion/desktop 纯 reducer、组合 suppression、WorkerW→fallback→回滚、Explorer lost/recovered 与幂等 |
| `cargo test --lib lib_tests:: -- --test-threads=1` | 31 passed、0 failed、0 ignored；0.69 秒 | Tauri command registry、调用窗口限制、托盘 CAS/revision、设置与托盘竞争、后台启动恢复接线；其中也包含其他 lib boundary 回归 |

`window_mode` 组已包含本计划的 startup/recovery/shutdown 证据：启动 desktop 等待 runtime-ready、ready 超时回 companion、显式请求取消 startup、Explorer 丢失后立即重挂及 1/2/4/8 秒 fake backoff、五次失败终止、恢复取消、退出恢复快照与 restore 失败诊断。所有等待和退避均由测试 ACK/fake wait 驱动，不是真实等待 Explorer 或真实操作 HWND。

### TypeScript

命令：

```powershell
npm test -- --run src/runtime/window-mode-client.test.ts src/settings/window-mode-control.test.ts src/runtime/window-mode-runtime.test.ts src/runtime/fullscreen.test.ts src/runtime/pet-stage.test.ts src/main.test.ts src/settings.test.ts
```

结果：7 个测试文件、102 项测试全部通过，0 failed、0 skipped，exit 0。覆盖 strict snapshot/revision、设置页 radio 状态、失败复位和 BottomFallback 提示、runtime cycle/ACK 与 fail-closed、全屏 single-flight、PetStage pause/resume/effective visibility、主入口接线和设置页装配。该组使用 mock invoke/listen、假 DOM 与可注入 stage，不等于真实 WebView2/Tauri 窗口交互。

## 自动化状态组合与故障证据

下表中的“最终物理可见性”只表示 FakeIo/Fake API 或 reducer 期望动作，不是本机真实窗口观测。

| 场景 | `desiredMode` | `actualMode` | `strategy` | `suppressions` | 最终物理可见性（自动化） | 证据性质 |
| --- | --- | --- | --- | --- | --- | --- |
| companion 遇到同屏全屏 | `companion` | `companion` | `null` | `[fullscreen]` | FakeIo 收到 hide；用户 `userVisible` 仍为 true | controller + reducer 自动测试 |
| manual hide + fullscreen，再退出全屏 | `companion` | `companion` | `null` | 全屏时 `[fullscreen]`，退出后 `[]` | 始终隐藏，因为 `userVisible=false` | controller + reducer 自动测试 |
| desktop + fullscreen | `desktop` | `desktop` | 成功 fixture 为 `workerW` | `[]`；desktop 忽略 fullscreen | reducer 期望 show | reducer/controller 自动组合；非真实 HWND |
| hidden 时切到 desktop，随后 Explorer host 丢失/恢复 | `desktop` | `desktop` | 恢复成功 fixture 为 `workerW` | 恢复完成后不含 `explorerLost`/`transition` | 保持隐藏，恢复不得覆盖 `userVisible=false` | FakeIo recovery 测试 |
| WorkerW 与 BottomFallback 双失败 | 回滚为 `companion` | `companion` | `null` | transition 完成后清除 | 恢复原 companion 可见性；不持久化 desktop | reducer + controller + Fake Win32 adapter |
| Explorer host 丢失后重挂成功 | `desktop` | `desktop` | fixture 返回 `workerW` 或 `bottomFallback` | 丢失期含 `explorerLost`/`transition`，成功后清除 | 按 `userVisible` 恢复 | FakeIo/fake wait，不是真实 Explorer |
| Explorer 重挂连续五次失败 | 终止意图为 `companion` | 安全收尾成功时 `companion` | `null` | 安全收尾成功后清除 | 回 companion；持久化 companion 并报告降级 | FakeIo/fake wait，不是真实 Shell |
| 终止收尾 restore/visibility/runtime/persist 任一步再失败 | `companion` | `null` | `null` | 保留 `transition`，并按事实保留 degraded 标记 | fail closed；可见性未知或强制隐藏 | terminal failure 自动矩阵 |
| 切换中收到 fullscreen | 请求目标决定最终模式 | 自动化得到的最终 actual | 随成功宿主或回滚结果 | 提交前事实被排空后再恢复 runtime | 先保持 transition 隐藏，再按最终事实恢复 | controller 并发/ACK 自动测试 |
| 切换中退出 / shutdown | 保存的用户意图不因退出清理被改写 | 退出前按快照做 best-effort restore | 清理宿主 | 取消 recovery/transition 租约 | restore 失败只记录，不阻止退出 | 自动 shutdown 测试；未真实关闭应用 |

“桌宠尺寸非 100% 时切模式”没有单一自动化用例把真实 OS resize、真实 hit-region 和真实 HWND reparent 串成端到端证据；本轮虽执行并通过 `pet-stage.test.ts` 及整仓尺寸回归，仍不把它推断为该组合已验收，保留到下文人工矩阵。

## 当前 Shell 只读诊断

运行前已逐行审查 `scripts/诊断窗口状态.ps1`。脚本只调用 `Get-Process` 以及 `GetClassName`、`GetWindowLongPtr`、`GetParent`、`IsWindowVisible`、`GetWindowRect`、`FindWindowW`、`FindWindowExW`；没有 `SetParent`、`SetWindowPos`、`ShowWindow`、消息发送、Explorer 重启或文件/偏好/数据库写入。随后在仓库根目录只读执行，exit 0：

- `desktop-pet not running`，因此没有真实宠物 HWND、parent class、actual mode 或 desktop strategy 可记录。
- `Progman : NULL`。
- `SHELLDLL_DefView : NULL`。
- 脚本枚举上限内取得 10 个 `WorkerW`；均为不可见的 136×39 顶层窗口，`WS_CHILD=false`、parent=0；第一个带 TOPMOST，其余不带。

该结果只描述诊断时刻的当前 Shell 拓扑。由于应用未运行、没有执行 attach，也没有取得应用 canonical snapshot，它既不能证明 WorkerW 策略成功，也不能证明本次实现已真实走到 BottomFallback；不计入真人窗口模式通过项。

## 历史 M0 证据的使用边界

`docs/验证记录/M0技术结论.md` 与 `docs/验证记录/M0手工验证清单.md` 记录了 2026-08-03 的历史现场：当前设备桌面层曾被第三方工具深度修改，无 `SHELLDLL_DefView`，标准 WorkerW/Progman 路径不可行，并曾使用旧的置底模拟方案。本文只把它作为“为什么当前机器必须做兼容矩阵”的历史背景；它不是本次新控制器、可恢复 adapter、BottomFallback 提示或 Explorer 自动重挂的成功证据，也不替代当前实现的真人复验。

## 五项最终统一人工验收（本轮未执行）

以下五项必须使用真实 Tauri 应用、测试专用 app-data 和可回滚的测试偏好统一执行。当前全部为“未执行”，不得从自动化模拟或历史 M0 结果推断为通过。

| 项目 | 真人执行要求 | 当前状态 |
| --- | --- | --- |
| 1. 陪伴模式与输入交互 | 启动 companion，确认不抢焦点、保持置顶；点击宠物和透明区分别确认接收/穿透；拖动后位置、点击区域和置顶保持正确；手动隐藏后不得被全屏退出或其他 suppression 清除误显示 | 未执行 |
| 2. 桌面模式标准 Shell 行为 | 在标准 Explorer 上切到 desktop，记录 canonical `desiredMode/actualMode/strategy/suppressions` 与真实 parent class；验证显示桌面/Win+D、切应用、前台全屏时仍位于桌面层且不错误隐藏；确认 companion/desktop 往返没有双窗口、任务栏按钮或失焦激活 | 未执行 |
| 3. 锁屏、Explorer 与失败恢复 | 在 desktop 下锁屏/解锁、睡眠/唤醒并重启 Explorer，验证自动隐藏、重挂和恢复；注入连续五次重挂失败，确认自动回 companion；记录真实日志、parent、strategy 与最终可见性，不得通过篡改真实用户偏好或数据库造结果 | 未执行 |
| 4. 双屏、DPI、尺寸与退出组合 | 在双屏及 100%/125%/150% 等缩放下，跨屏拖动并使用非 100% 桌宠尺寸切换模式；验证位置、尺寸、脚底锚点、显示器归属和 hit-region；分别覆盖 hidden 时切模式、切换中退出及重启恢复 | 未执行 |
| 5. 真实兼容与回滚提示 | 在可验证 WorkerW 的标准 Shell 和当前兼容 Shell 分别观察真实 WorkerW 成功或明确失败、真实 BottomFallback 非阻塞提示；注入 WorkerW+BottomFallback 双失败，确认 companion 的位置、尺寸、置顶、点击区域与可见性完整恢复；保存脱敏截图/录屏与日志 | 未执行 |

真人验收不得使用真实用户数据库；应记录操作者、时间、应用提交、Windows build、Shell/第三方桌面软件、显示器拓扑和 DPI、每一步 canonical snapshot、实际 HWND parent class/Z 序/矩形、应用日志以及证据路径。Explorer 重启和故障注入只应在明确可回滚的测试会话中执行。

## 最终判定

- 自动化全量门禁：**通过**。
- 窗口模式 Rust/TypeScript focused 跨层矩阵：**通过**。
- 构建产物实体核对：**通过**。
- 当前 Shell 只读拓扑诊断：**已执行，仅作环境记录，不构成功能通过**。
- Windows/Tauri 五项真实窗口矩阵：**未执行**。
- 可发布结论：当前只可声明“Windows 双窗口模式自动化验收通过”；在五项真人矩阵完成前，不可声明双窗口模式整体或发布验收完成。
