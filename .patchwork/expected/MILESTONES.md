# Touchpad Runtime Phase 0/1：增量实施与 Review 闸门

本文件把 `IMPLEMENTATION_BRIEF.md` 拆成可独立构建、测试和审查的里程碑。执行代理一次只能实施当前里程碑；不得提前铺开后续里程碑。每一阶段完成后由外部 reviewer 运行验收命令并检查设计不变量，通过后才允许进入下一阶段。

## 通用规则

- 完整遵守 `design.md` 与 `IMPLEMENTATION_BRIEF.md`。
- 不修改 `design.md`，不提交，不推送，不把凭据写入任何文件或输出。
- 每个里程碑结束时，整个 workspace 必须可构建；不能通过引用不存在的 crate 或模块来预告未来结构。
- 未实现能力必须明确标记，不能使用会返回虚假成功的 stub。
- 普通用户、无 `/dev/input`、无桌面会话的环境必须能运行全部自动测试。
- 每个里程碑结束时执行：

```text
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## M1 — Buildable Core Foundation

范围：

- 修复当前被中断的半成品，使根 workspace 可构建。
- 完成 `touchpad-core` 的平台无关类型：units、monotonic time、axis conversion、device descriptor、contact/frame、diagnostics、profile override、validation、typed `OutputSink`。
- 强制核心不变量：raw/mm 类型隔离；毫米和逻辑像素浮点值不得携带 NaN/Infinity；缺失 resolution 时只能显式 profile override 或返回诊断。
- 删除或替换 `example.invalid` 等占位元数据。
- 创建 `DESIGN_V2.md`，记录当前已确定的模块、状态机、失败语义和后续闸门；不宣称尚未实现的功能。
- 只加入本里程碑所需的 workspace member。若创建未来 crate，只能提供诚实的 unavailable API，且必须有测试证明不会虚假成功。

验收：

- 三条通用命令全部通过。
- core 不依赖 Linux、Wayland、X11 或桌面库。
- public constructors 和 serde 反序列化不能绕过 finite/unit validation。
- 不存在 API key 或其他秘密。

明确不做：trace、Type-B decoder、ioctl、设备发现、grab、CLI。

## M2 — Versioned Trace and Offline Replay Boundary

范围：

- 实现 `touchpad-trace` 的版本化 JSON Lines header/event reader/writer。
- 定义 schema mismatch、损坏行、非法范围、header 缺失、时间倒退和 I/O 错误。
- 加入 streaming round-trip 测试和人工 fixture。
- 建立 replay driver 的平台中立边界，但尚不实现 Linux MT decoder。

验收：

- 三条通用命令全部通过。
- 大文件可逐行处理，不要求整体载入内存。
- 不支持的新 schema 明确失败；未知可选字段按写入设计的兼容策略处理。
- trace 不包含 wall-clock 手势语义，也不访问真实设备。

## M3 — Type-B Decoder and Mocked Resynchronization

范围：

- 实现 `touchpad-linux` 的内部 `RawEvent`、Type-B slot decoder 和 frame commit。
- 只在 `SYN_REPORT` 发布帧。
- 实现 `Normal | DroppedAwaitingBoundary | Recovering | Degraded`。
- 定义可 mock 的 `ResyncSource`，覆盖恢复成功、失败和 discontinuity frame。
- fixture replay 必须复用与实时输入相同的 decoder 路径。

验收：

- 三条通用命令全部通过。
- 覆盖单触点、多 slot、tracking id 替换/结束、字段继承、不完整新触点、按钮、非法 slot、dropped/resync。
- resync 失败返回 fatal/degraded，不能继续产生可信输出。
- 本阶段不触碰真实 ioctl 或 grab。

## M4 — Linux Device Boundary and Fail-Open Grab

范围：

- 实现设备枚举、候选触控板判定、capability/axis/slot 查询。
- 实现最小、文档化安全不变量的 ioctl/FFI 边界。
- 实现 `EVIOCGRAB` RAII guard、显式 opt-in、幂等 ungrab 与 shutdown。
- 真实 `SYN_DROPPED` snapshot adapter 对接 M3 的 `ResyncSource`。
- 所有 syscall 通过 adapter seam 可 mock。

验收：

- 三条通用命令全部通过。
- 无 `/dev/input`、权限不足、设备拔出均返回可操作错误且不 panic。
- mock 测试覆盖正常退出、错误退出、重复 shutdown、resync failure 后 ungrab。
- 文档明确不能保证 `SIGKILL`、断电或内核崩溃时执行 cleanup。
- 不声称已完成实机验证。

## M5 — CLI Vertical Slice and Phase 1 Handoff

范围：

- 实现 `touchpadctl devices`、`inspect`、`record [--grab]`、`replay`。
- recorder 位于 decoder 之前；replay 使用相同 decoder。
- 加入受控 `SIGINT`/`SIGTERM` shutdown、flush 与 ungrab 顺序。
- 完成 README、第三方依赖/许可证说明、fixture 与验收文档。

验收：

- 三条通用命令全部通过。
- CLI help 和 fixture replay smoke test 通过。
- `--grab` 默认关闭且帮助文本有明确风险警告。
- 无硬件时命令有清晰结果，不 panic。
- 最终报告区分自动测试、环境探测和未执行的实机验证。

## M6 — KDE Wayland Output Backend Qualification（在 PHASE2_PLAN.md §5 定义）

**(已批准，2026-08-16 M6_REVIEW.md re-review 5：M6 APPROVED FOR DEVELOPMENT；真实 bounded `--emit` 协议路径通过，backend 保持 experimental/unqualified，takeover 校准是 M10 前置门)**：`crates/touchpad-desktop`（RemoteDesktop portal v2 + 运行时加载 libei sender 的输出 adapter）+ `touchpadctl output-probe [--emit]`（默认非发射 dry-run；`--emit` 显式 opt-in、警告+倒计时、固定有界 pattern）。实现事实见 `DESIGN_V2.md` §17；验收矩阵与 reviewer 人工 `--emit` A/B 测量程序见 `docs/M6_ACCEPTANCE.md`。

## M7 — Arbiter 骨架、单指线性指针与物理点击（离线）（在 PHASE2_PLAN.md §5 定义）

**(已实现，2026-08-16；M7_REVIEW R1–R2 修复；M7_REVIEW re-review 2 批准，2026-08-17)**：平台无关统一 Interaction Arbiter 落在 `touchpad-core`（`arbiter` 模块 + `units::LogicalPixelsPerMm` + 4 个新 `DiagnosticCode`），提供 `Candidate/Committed/Cancelled/Finished` 生命周期、一指线性毫米→逻辑像素指针（逐轴 sub-pixel remainder、恰好一次首次累计输出、候选期零泄漏）、物理左键 down/up/click/双击对/按住拖动与确定性同帧顺序、幂等 `release_all`（M10 shutdown 路径）与原子帧决策；**R1**：`ArbiterSink` 改为交付感知 fail-stop（接受前缀跟踪、被拒 down 不 held、部分提交 fault、cleanup 只释放已接受状态并调用 wrapped sink 的 `release_all`、失败可重试、acknowledgement boundary 才复位）；**R2**：`Arbiter::frame` 消费 `ContactFrame::validate()`，Error/Fatal 诊断整帧原子拒绝（结构化 code/reason），Warning-only 策略保留。全部由合成/trace 派生 `ContactFrame` 驱动；不连接 M6 后端、不触碰 live 输入。实现事实见 `DESIGN_V2.md` §18。M8（tap/tap-drag/drag lock）与 M9（双指滚动/右键）未实现。

## M8 — Tap、Tap-and-Drag 与 Sticky Drag Lock（离线）（在 PHASE2_PLAN.md §5 定义）

**(已批准，2026-08-17；M8_REVIEW.md re-review 2 终审通过，R1–R5 全部关闭；M9 按此基线实施)**：在 M7 已批准的单一 arbiter 上扩展可配置一指 tap-to-click、双 tap（两对正确计时的 click pair，无发明双击事件）、tap-and-drag（合格首 tap 打开 follow-up 窗口；恰好一个新有效手指在 deadline 内开始 → 立即合成第二按；motion 用 M7 线性映射/阈值/首次累计 delta/remainder；未 commit 的第二次触摸是普通第二击）、sticky drag lock（真实 drag 后抬起保持合成左键 held；locked-without-contact 中新手指续拖不重复下按、首累计 delta 恰好一次；合格 tap 解锁输出恰好一个 up；不合格保锁；`release_all` 无条件逃生）。新增类型化 `TapConfig`（构造校验零时长/非正位移/不可能组合；`ArbiterConfig::new` 默认禁用 tap，`with_tap` 显式启用）与可观测 `TapDragPhase`（`FrameDecision.tap_drag_phase_after` + `Arbiter::tap_drag_phase()`）；物理/合成左键分离为源感知聚合 OR 仲裁（聚合 false→true 才 down、true→false 才 up，同帧合成 tap pulse 仍 down 后 up）。只使用 `ContactFrame.monotonic_timestamp` 与 checked duration 运算；边界策略为相等接受、严格更大过期/取消；超时只在帧边界求值；drag lock 无自主超时。`ArbiterSink` 交付感知 fail-stop 契约保留并新增合成事件故障测试。**M8_REVIEW R1–R4 修复**：R1 指针 commit 副作用统一（active 与 final-`Ended` 帧共用 `commit_pointer`：final-Ended 越阈无合成 click / tap-drag 进入 lock / locked continuation 保锁）；R2 multiplexer 改为连贯源转换序列（先捕获 pre-frame 源状态并应用物理边沿，再执行 discontinuity/取消：discontinuity+物理释放同帧恰好一个聚合 up，wire 恒等于 post-frame 聚合）；R3 discontinuity 帧 Began 的接触被标记 `tap_disqualified` 而 tap 家族不可用（无 tap click、无即时 tap-and-drag down），M7 指针 re-anchor 保留，接触结束后的新 Began 恢复正常；R4 follow-up 过期改 checked elapsed（`duration_since` 与 gap 比较，不用 `saturating_add`），near-`u64::MAX` 边界有测试。实现事实见 `DESIGN_V2.md` §19（含 §19.11 修复事实）。workspace 共 647 个测试（+15）。M9（双指滚动/右键）见下条。


## M9 — 双指二维滚动与右键（离线）（在 PHASE2_PLAN.md §5 定义）

**(已批准，2026-08-17；M9_REVIEW.md Re-review 2 终审通过，R1–R7 全部关闭)**：在 M8 已批准（`reviews/M8_REVIEW.md` re-review 2）后按 M9_TASK.md 实施，纯离线策略：在已批准的单一 arbiter 上扩展**恰两指二维 pixel scroll**（完整 `ScrollBegin → ScrollDelta* → ScrollEnd` 生命周期）、**显式 natural 方向**（`natural=true` 输出与双指质心运动同号、`false` 每轴取反）、**双指 secondary tap**（合格 release 边界恰好一对 `ButtonDown(Right), ButtonUp(Right)`，分帧抬起只发一次）、**buttonpad 双指物理二次点击**（physical left 在恰两 complete valid fingers 时 press 被 latch 到 Right，整个 press 锁定 owner，finger count 变化不 remap；第二指前开始的 press 保持 Left）。新增类型化 `TwoFingerConfig`（构造校验非正阈值/零时长/非正位移；`ArbiterConfig::new` 默认禁用双指家族，`with_two_finger` 显式启用）、可观测 `TwoFingerPhase`（`Idle/Candidate/CommittedScroll/PhysicalSecondaryClickHeld/Cancelled/Finished`，在 `FrameDecision.two_finger_phase_after` 与 `Arbiter::two_finger_phase()`）、6 个新 `DiagnosticCode` 与右键三源聚合仲裁（physical right / synthetic right / latched right，聚合 false→true 才 down、true→false 才 up，不静默 alias）。滚动按 tracking id 识别 pair（与 slot/vector 顺序无关）、每 contact 自 anchor 最大位移（对向 pinch 不能冒充 tap）、质心自 anchor 位移提交（相等接受）、逐轴 sub-pixel remainder（many-small == aggregate；finish/cancel/release/新 interaction 清零）、对角双轴一等公民。双指家族确定性取消/结束一指 ownership（sticky synthetic-left lock 按 M8 聚合规则释放恰好一个 left up）；第三指/缺坐标/tracking replacement/discontinuity/物理点击 → `ScrollEnd` 恰好一次且无 tap；replacement 同帧不 re-anchor；discontinuity 可 re-anchor 相对 scroll 但无 secondary tap。**accepted-prefix/fail-stop 扩展**：`ArbiterSink` 增 `delivered_held_right` 与 `delivered_scroll_open`（rejected `ScrollBegin` 不欠 `ScrollEnd`、accepted begin 后被拒 delta/end 保持 open 且 cleanup 关闭、rejected right down 不欠 up、accepted right down 后被拒 up 保持 held 且 cleanup release；wrapped `release_all` 仍为权威 ack）；`Arbiter::release_all` 发所需 `ScrollEnd` 与 right/left release 恰好一次后重置全部 M7–M9 状态；regression fail-closed 保持 open scroll / held right 对 cleanup 可见。只使用 `ContactFrame.monotonic_timestamp` 与 checked duration 运算；边界相等接受、严格更大取消。**M9_REVIEW R1–R6 修复（2026-08-17，详见 `DESIGN_V2.md` §20.10/§20.11）**：R1 `scroll_enabled=false` 全面门控（commit/candidate anchor/discontinuity re-anchor；三能力全关 config 完全惰性）；R2 同 cluster 内 physical-button（含一指时开始的 primary-left）与已 commit 一指 pointer ownership 永久 disqualify secondary tap（anchor 时 OR 继承 + release 边界防御检查）；R3 cluster-level disqualification 跨第三指/缺坐标/replacement/regression/discontinuity/物理点击取消存活，仅在 cluster 完全排空后清除（fresh pair 恢复 tap）；R4 同帧组装改为有序 intents——pre-handoff left up 先于新 right down（无瞬时 Left+Right chord）、old-owner `ScrollEnd` 先于新 physical-button down（press-while-scrolling 帧输出 `[ScrollEnd, ButtonDown(Right)]`），M8 within-owner 不变量保留，debug/release 均测；R5 `ArbiterSinkError::ReleaseFailed` 增 `others: Vec<OutputError>` 逐一报告每个失败的显式 release（retry 状态与 wrapped-cleanup 错误原样保留；公共模式匹配需 `..` 或显式 `others`）；R6 掉到 2 指以下的 tap 需 anchored pair **至少一个成员携带 clean complete `Ended` 记录**，否则 `Cancel` 无 click。**Re-review 1 R7（物理按键所有权排除 scroll 所有权，详见 `DESIGN_V2.md` §20.12）**：aggregate physical Left/Right（含 latched press）held 期间双指家族**不 anchor 候选、不 commit/发任何 `ScrollBegin`/`ScrollDelta`**，被 press 取消的 scroll 在 held 期间绝不在后续稳定帧 re-open（`handle_two_finger` candidate anchor 与 `handle_two_finger_discontinuity` re-anchor 均以 `physical_button_ownership_held()` gate，`update_two_finger_pair` commit 另加防御 gate）；按键干净 release 后同一仍在位的 pair 重新 anchor 相对 scroll（fresh anchor；secondary tap 仍 cluster-disqualified 直到 cluster 排空）；R4 同帧顺序保留（final delta → `ScrollEnd` → 新 down）；**无合法帧同时暴露 physical-button 与 scroll ownership**（每帧断言）。R5 双 owed 回归改用合法可达状态**同时按住 physical Left 与 physical Right**（`primary == Rejected(ButtonUp(Left))`、`others == [Rejected(ButtonUp(Right))]`、重试恰好重发各一次），scroll cleanup/retry 单独覆盖。实现事实见 `DESIGN_V2.md` §20（§20.7/§20.8/§20.9/§20.10/§20.11/§20.12）。workspace 共 **739** 个测试（M9_REVIEW R1–R6 修复后 +23：core 单元 +15 至 214、core 公共集成 +8 至 19；Re-review 1 R7 再 +8：core 单元 +4 至 218、core 公共集成 +4 至 23；linux replay 集成 3 不变；两个新 fixture `m9_scroll`/`m9_secondary_tap` 加入契约表）。M10（限时安全 Takeover）见下条。**M9 已批准（M9_REVIEW.md Re-review 2）。**

## M10 — 限时安全 Takeover 纵向切片（在 PHASE2_PLAN.md §5 定义）

**(已实现，2026-08-17；**M10_REVIEW.md R1–R6 修复（2026-08-17）：poll EINTR 停止再检查、poll revents 显式分类、release 前捕获服务器 interruption、全部五个 takeover flag 拒绝重复、真实 Left+Right 多显式 release 失败测试、真实工厂 create 惰性化**；静态/fake-backed 闸门全部通过，待外部 re-review；live-unqualified / pending user acceptance——用户按 `docs/M10_ACCEPTANCE.md` 完成 10/60/300 秒验收并记录 M6 校准表前不写 approved）**：第一条有界纵向切片——`explicit evdev device (EVIOCGRAB) → Type-B decoder/resync → M7–M9 Arbiter/ArbiterSink（m10-linear-v1）→ prepared portal+libei streaming OutputSink → KDE Wayland`。新增：`touchpad-core::m10`（**`m10-linear-v1`** 命名版本化 bring-up profile：1.0 mm 指针阈值、10 px/mm、tap/tap-and-drag/drag-lock（180 ms/3.0 mm/350 ms）、双指 natural scroll（10 px/mm、1.0 mm commit）、secondary tap（300 ms/3.0 mm）、buttonpad click——typed/finite/validated，非 macOS 等价声明、非生产默认、运行时不读 KDE/libinput）；`touchpad-linux::bridge`（**`TakeoverBridge`**：infallible `FrameSink` → fallible `ArbiterSink` 窄桥，存储首 fault、sticky fail-stop、同批后续帧零输出、cleanup 恰好欠的 state）+ runtime 兼容扩展（`sink_mut`/`take_sink`/`fd`/`step_deferred`——deferred-cleanup 使 fatal 错误保留 output/recorder/grab/fd 给协调器统一 shutdown）+ `Sys::poll`（有界 loop 的 bounded-readiness seam；Linux 实现在既有 unsafe FFI 边界）；`touchpad-desktop::streaming`（**`StreamingOutput`/`StreamingOutputFactory`**、`PortalStreamingOutput` 包装 M6 sink、真实/伪造工厂）；`touchpadctl takeover`（`--takeover --confirm TAKEOVER --output-qualified --profile m10-linear-v1 --max-duration-seconds N`，1–300 秒；grab 是最后一步：parse→create→open→prepare（缺能力在 recorder/grab 前拒绝）→recorder create+header flush→attach→倒计时≥3s→重查→恰好一次 grab→有界 loop（100 ms quantum、注入 clock/readiness、deadline 到期即使 idle 设备）；统一有序 shutdown：output release→recorder finalize→ungrab→close，全部 cleanup 失败结构化保留，exit 优先级确定（deadline/signal clean=0 仅当全部 cleanup 成功；countdown cancel=8）；panic 由 `TakeoverCleanup` guard 同序兜底）。`docs/M10_ACCEPTANCE.md` 为人工验收程序（写而未执行；M6 校准表必须由用户填写，`--output-qualified` 是 attestation 而非测量证据）。**执行约束**：实现只运行 offline/fake-backed 测试；reviewer 批准后用户才执行 live 测试；M10 保持 live-unqualified；M11 已按离线约束实施（见下条）。实现事实见 `DESIGN_V2.md` §21。workspace 共 **792** 个测试（M9 739 + M10 新增 53：core +3（m10 profile）、linux +8（bridge 4 + runtime step_deferred 2 + sys poll 分类 2）、desktop +4（streaming 2 + R3 生命周期 1 + R6 时间线 1）、touchpadctl +38（args 10 + cmd/takeover 26 + cli 集成 2））。

**M10 状态更新（2026-08-17）**：`reviews/M10_REVIEW.md` Re-review 1 已批准 M10 代码，R1–R6 全部关闭；仍为 `live-unqualified / pending user acceptance`——用户完成 M6 校准记录与 10/60/300 秒真机序列前不得宣称 M10 live 合格。M11 已按离线约束实施（见下条），其 live 验收独立于 M10。

## M11 — 实验性一指指针保真（`m11-fidelity-v1`）（在 PHASE2_PLAN.md §5 定义）

**(已实现，2026-08-22；`M11_REVIEW.md` R1–R4 修复完成；独立 re-review 中——未批准、未 code-complete；live-unqualified / 待独立的 M11 专用用户验收——代码完成不意味着 live 合格，M10 验收不使 M11 合格，M12 未开始）**：在已批准的 M7–M9/M10 交互策略之上增加平台无关的一指指针保真阶段（signed radial dead-zone 0.09 mm、20 ms tau 单调时域速度 EMA、150 ms 含边界 long-gap、50–600 mm/s → gain 1.0–2.0 smoothstep、tracking 倍率 1.0、base 10 px/mm 继承 M10）。`M11Profile` 从 `M10Profile` 继承全部 M7–M9 值（不复制常量、不改 `m10.rs`）；`FidelityState` 存入 `ArbiterState` 参与既有 draft/commit 原子性（拒绝帧回滚），fidelity 关闭时 M10 分支不变；`touchpadctl --profile` 接受集恰为 `{m10-linear-v1, m11-fidelity-v1}`（M10 mention-first），纯 `select_profile` + 实验 banner（experimental/uncalibrated、非默认、无 macOS 等价声明、无 live 验证、M10 五个 opt-in 与 1..=300 秒上界仍适用）在任何副作用之前写出；确定性 trace fixture `m11_fidelity.jsonl`（25 帧：first commit/低高速/重复时间戳/reversal/对角/恰等于与超过 long-gap/clean end/fresh interaction）+ 直接-重放决策一致性测试；`docs/M11_ACCEPTANCE.md` 为未来用户人工验收程序（写而未执行；要求 M10/M6 前置，分离 M11 code-complete 与 live-qualified）。实现事实见 `DESIGN_V2.md` §22；workspace 共 **872** 个测试（debug/release 门禁实测，M10 792 + M11 新增 80，0 失败）。
## Review 输出格式

每个里程碑完成后，执行代理必须报告：

1. 创建或修改的文件。
2. 实际实现的能力与明确未实现能力。
3. 三条验收命令的完整结论。
4. 设计偏差及理由。
5. reviewer 应重点检查的风险。
