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

**(已批准，2026-08-17；M8_REVIEW.md re-review 2 终审通过；2026-08-23 M19 实机安全修订)**：在 M7 已批准的单一 arbiter 上扩展可配置一指 tap-to-click、双 tap、tap-and-drag 与 sticky drag lock。当前 tap-and-drag 安全契约为：合格首 tap 打开 follow-up 窗口；deadline 内的新单指只进入 `TapDragCandidate`，**不立即合成 left-down**；只有 pointer motion 真正 commit 时才按下左键并在同帧按 `Down → Move` 顺序开始真实拖拽；未 commit 的合格第二 tap 在 release 帧输出完整 click pulse；tracking-id replacement/取消/缺坐标不产生 held-left。此修订来自 M19 KDE 实机发现的短 re-touch drag-through 风险，取代早期“follow-up Began 立即 down”的行为。sticky drag lock、源感知物理/合成左键聚合、checked timing、R1–R4 历史修复仍保留。实现事实见 `DESIGN_V2.md` §19.5/§19.11/§19.12；M19 实机修订评审见 `reviews/M19_REVIEW.md`。


## M9 — 双指二维滚动与右键（离线）（在 PHASE2_PLAN.md §5 定义）

**(已批准，2026-08-17；M9_REVIEW.md Re-review 2 终审通过，R1–R7 全部关闭)**：在 M8 已批准（`reviews/M8_REVIEW.md` re-review 2）后按 M9_TASK.md 实施，纯离线策略：在已批准的单一 arbiter 上扩展**恰两指二维 pixel scroll**（完整 `ScrollBegin → ScrollDelta* → ScrollEnd` 生命周期）、**显式 natural 方向**（`natural=true` 输出与双指质心运动同号、`false` 每轴取反）、**双指 secondary tap**（合格 release 边界恰好一对 `ButtonDown(Right), ButtonUp(Right)`，分帧抬起只发一次）、**buttonpad 双指物理二次点击**（physical left 在恰两 complete valid fingers 时 press 被 latch 到 Right，整个 press 锁定 owner，finger count 变化不 remap；第二指前开始的 press 保持 Left）。新增类型化 `TwoFingerConfig`（构造校验非正阈值/零时长/非正位移；`ArbiterConfig::new` 默认禁用双指家族，`with_two_finger` 显式启用）、可观测 `TwoFingerPhase`（`Idle/Candidate/CommittedScroll/PhysicalSecondaryClickHeld/Cancelled/Finished`，在 `FrameDecision.two_finger_phase_after` 与 `Arbiter::two_finger_phase()`）、6 个新 `DiagnosticCode` 与右键三源聚合仲裁（physical right / synthetic right / latched right，聚合 false→true 才 down、true→false 才 up，不静默 alias）。滚动按 tracking id 识别 pair（与 slot/vector 顺序无关）、每 contact 自 anchor 最大位移（对向 pinch 不能冒充 tap）、质心自 anchor 位移提交（相等接受）、逐轴 sub-pixel remainder（many-small == aggregate；finish/cancel/release/新 interaction 清零）、对角双轴一等公民。双指家族确定性取消/结束一指 ownership（sticky synthetic-left lock 按 M8 聚合规则释放恰好一个 left up）；第三指/缺坐标/tracking replacement/discontinuity/物理点击 → `ScrollEnd` 恰好一次且无 tap；replacement 同帧不 re-anchor；discontinuity 可 re-anchor 相对 scroll 但无 secondary tap。**accepted-prefix/fail-stop 扩展**：`ArbiterSink` 增 `delivered_held_right` 与 `delivered_scroll_open`（rejected `ScrollBegin` 不欠 `ScrollEnd`、accepted begin 后被拒 delta/end 保持 open 且 cleanup 关闭、rejected right down 不欠 up、accepted right down 后被拒 up 保持 held 且 cleanup release；wrapped `release_all` 仍为权威 ack）；`Arbiter::release_all` 发所需 `ScrollEnd` 与 right/left release 恰好一次后重置全部 M7–M9 状态；regression fail-closed 保持 open scroll / held right 对 cleanup 可见。只使用 `ContactFrame.monotonic_timestamp` 与 checked duration 运算；边界相等接受、严格更大取消。**M9_REVIEW R1–R6 修复（2026-08-17，详见 `DESIGN_V2.md` §20.10/§20.11）**：R1 `scroll_enabled=false` 全面门控（commit/candidate anchor/discontinuity re-anchor；三能力全关 config 完全惰性）；R2 同 cluster 内 physical-button（含一指时开始的 primary-left）与已 commit 一指 pointer ownership 永久 disqualify secondary tap（anchor 时 OR 继承 + release 边界防御检查）；R3 cluster-level disqualification 跨第三指/缺坐标/replacement/regression/discontinuity/物理点击取消存活，仅在 cluster 完全排空后清除（fresh pair 恢复 tap）；R4 同帧组装改为有序 intents——pre-handoff left up 先于新 right down（无瞬时 Left+Right chord）、old-owner `ScrollEnd` 先于新 physical-button down（press-while-scrolling 帧输出 `[ScrollEnd, ButtonDown(Right)]`），M8 within-owner 不变量保留，debug/release 均测；R5 `ArbiterSinkError::ReleaseFailed` 增 `others: Vec<OutputError>` 逐一报告每个失败的显式 release（retry 状态与 wrapped-cleanup 错误原样保留；公共模式匹配需 `..` 或显式 `others`）；R6 掉到 2 指以下的 tap 需 anchored pair **至少一个成员携带 clean complete `Ended` 记录**，否则 `Cancel` 无 click。**Re-review 1 R7（物理按键所有权排除 scroll 所有权，详见 `DESIGN_V2.md` §20.12）**：aggregate physical Left/Right（含 latched press）held 期间双指家族**不 anchor 候选、不 commit/发任何 `ScrollBegin`/`ScrollDelta`**，被 press 取消的 scroll 在 held 期间绝不在后续稳定帧 re-open（`handle_two_finger` candidate anchor 与 `handle_two_finger_discontinuity` re-anchor 均以 `physical_button_ownership_held()` gate，`update_two_finger_pair` commit 另加防御 gate）；按键干净 release 后同一仍在位的 pair 重新 anchor 相对 scroll（fresh anchor；secondary tap 仍 cluster-disqualified 直到 cluster 排空）；R4 同帧顺序保留（final delta → `ScrollEnd` → 新 down）；**无合法帧同时暴露 physical-button 与 scroll ownership**（每帧断言）。R5 双 owed 回归改用合法可达状态**同时按住 physical Left 与 physical Right**（`primary == Rejected(ButtonUp(Left))`、`others == [Rejected(ButtonUp(Right))]`、重试恰好重发各一次），scroll cleanup/retry 单独覆盖。实现事实见 `DESIGN_V2.md` §20（§20.7/§20.8/§20.9/§20.10/§20.11/§20.12）。workspace 共 **739** 个测试（M9_REVIEW R1–R6 修复后 +23：core 单元 +15 至 214、core 公共集成 +8 至 19；Re-review 1 R7 再 +8：core 单元 +4 至 218、core 公共集成 +4 至 23；linux replay 集成 3 不变；两个新 fixture `m9_scroll`/`m9_secondary_tap` 加入契约表）。M10（限时安全 Takeover）见下条。**M9 已批准（M9_REVIEW.md Re-review 2）。**

## M10 — 限时安全 Takeover 纵向切片（在 PHASE2_PLAN.md §5 定义）

**(已实现，2026-08-17；**M10_REVIEW.md R1–R6 修复（2026-08-17）：poll EINTR 停止再检查、poll revents 显式分类、release 前捕获服务器 interruption、全部五个 takeover flag 拒绝重复、真实 Left+Right 多显式 release 失败测试、真实工厂 create 惰性化**；静态/fake-backed 闸门全部通过；live-unqualified / pending user acceptance——用户按 `docs/M10_ACCEPTANCE.md` 完成 10/60/300 秒验收并记录 M6 校准表前不写 live-qualified）**：第一条有界纵向切片——`explicit evdev device (EVIOCGRAB) → Type-B decoder/resync → M7–M9 Arbiter/ArbiterSink（m10-linear-v1）→ prepared portal+libei streaming OutputSink → KDE Wayland`。新增：`touchpad-core::m10`（**`m10-linear-v1`** 命名版本化 bring-up profile：1.0 mm 指针阈值、10 px/mm、tap/tap-and-drag/drag-lock（180 ms/3.0 mm/350 ms）、双指 natural scroll（10 px/mm、1.0 mm commit）、secondary tap（300 ms/3.0 mm）、buttonpad click——typed/finite/validated，非 macOS 等价声明、非生产默认、运行时不读 KDE/libinput）；`touchpad-linux::bridge`（**`TakeoverBridge`**：infallible `FrameSink` → fallible `ArbiterSink` 窄桥，存储首 fault、sticky fail-stop、同批后续帧零输出、cleanup 恰好欠的 state）+ runtime 兼容扩展（`sink_mut`/`take_sink`/`fd`/`step_deferred`——deferred-cleanup 使 fatal 错误保留 output/recorder/grab/fd 给协调器统一 shutdown）+ `Sys::poll`（有界 loop 的 bounded-readiness seam；Linux 实现在既有 unsafe FFI 边界）；`touchpad-desktop::streaming`（**`StreamingOutput`/`StreamingOutputFactory`**、`PortalStreamingOutput` 包装 M6 sink、真实/伪造工厂）；`touchpadctl takeover`（`--takeover --confirm TAKEOVER --output-qualified --profile m10-linear-v1 --max-duration-seconds N`，1–300 秒；grab 是最后一步：parse→create→open→prepare（缺能力在 recorder/grab 前拒绝）→recorder create+header flush→attach→倒计时≥3s→重查→恰好一次 grab→有界 loop（100 ms quantum、注入 clock/readiness、deadline 到期即使 idle 设备）；统一有序 shutdown：output release→recorder finalize→ungrab→close，全部 cleanup 失败结构化保留，exit 优先级确定（deadline/signal clean=0 仅当全部 cleanup 成功；countdown cancel=8）；panic 由 `TakeoverCleanup` guard 同序兜底）。`docs/M10_ACCEPTANCE.md` 为人工验收程序（写而未执行；M6 校准表必须由用户填写，`--output-qualified` 是 attestation 而非测量证据）。**执行约束**：实现只运行 offline/fake-backed 测试；M10 保持 live-unqualified；M11 已 code-complete / review-approved，仍 live-unqualified，见下条。实现事实见 `DESIGN_V2.md` §21。workspace 共 **792** 个测试（M9 739 + M10 新增 53：core +3（m10 profile）、linux +8（bridge 4 + runtime step_deferred 2 + sys poll 分类 2）、desktop +4（streaming 2 + R3 生命周期 1 + R6 时间线 1）、touchpadctl +38（args 10 + cmd/takeover 26 + cli 集成 2））。

**M10 状态更新（2026-08-17）**：`reviews/M10_REVIEW.md` Re-review 1 已批准 M10 代码，R1–R6 全部关闭；仍为 `live-unqualified / pending user acceptance`——用户按 `docs/M10_ACCEPTANCE.md` 完成 M6 校准记录与有序 10/60/300 秒 `m10-linear-v1` 验收前保持 live-unqualified；M10 验收不构成 M11 live 资格。

## M11 — 实验性一指 Pointer Fidelity（`m11-fidelity-v1`）（在 M11_TASK.md 定义）

**(M11_REVIEW.md Re-review 1 已批准：code-complete / review-approved；仍 live-unqualified）**：在已批准 M7–M9/M10 交互策略上叠加实验性一指 pointer-fidelity；M10 路径保持兼容，M11 live 资格仍独立于 M10。实现事实见 `DESIGN_V2.md` §22。后续 M12–M16 已完成，见下列里程碑。

## M12 — Scroll Fidelity / Momentum

**已批准（`reviews/M12_REVIEW.md`）**：time-domain scroll velocity、smoothstep gain、axis lock、reversal 与 software momentum；增加可注入 monotonic `tick` 并贯通 Arbiter/Sink/Bridge/bounded runtime。仍 live-unqualified。

## M13 — Contact Robustness

**已批准（`reviews/M13_REVIEW.md`）**：palm/thumb、edge-start、typing suppression、jitter 与 feature-availability fallback；generic classifier 不依赖 CIRQ1080。仍 live-unqualified。

## M14 — Continuous Gestures

**已批准（`reviews/M14_REVIEW.md`）**：pinch/rotate/page/edge/3-4 finger/thumb+3 continuous recognizer 与单一 ownership；当前 M6 sink 对 native continuous output 明确 unavailable。仍 live-unqualified。

## M15 — Three-Finger Drag / KDE Actions

**已批准（`reviews/M15_REVIEW.md`）**：three-finger drag/drag-lock、semantic desktop actions、可配置 KDE action map/injected transport；真实 KDE transport 默认未启用。仍 live-unqualified。

## M16 — Productionization

**已批准（`reviews/M16_REVIEW.md`）**：strict runtime config v2 + v1 migration、device/output bounded reconnect、foreground service lifecycle、capability matrix、`config-check` / `service-preflight`、`m16-production-v1`。四个最终 workspace gates（fmt/clippy/debug/release）全部通过。persistent service/autostart、X11/uinput、pressure/haptics 均没有被静默启用或声称 qualified；M16 仍 live-unqualified。

## M17 — Tunable Feel Parameters

**已批准（`reviews/M17_REVIEW.md`）**：增加 strict `FeelConfig v1` tuning overlay 与 `m17-tunable-v1`，仅开放对手感影响显著的 pointer / scroll / continuous-gesture / three-finger-drag 参数。默认 overlay 与 M16 Arbiter config 完全相等；所有参数均有范围和 cross-field validation，three-finger drag commit 必须低于 multi-swipe commit 以保持单一 ownership 优先级。CLI 提供 `feel-default` / `feel-check` / `feel-show` / `feel-set` / `feel-gui`；GUI 为 self-contained offline HTML，无 network/device/live-apply。bounded takeover 仅在显式 `m17-tunable-v1 --feel-config FILE` 下读取 overlay，且在任何 output/device/recorder/grab 副作用之前完成 strict validation；M10–M16 禁止该 flag。最终 fmt、workspace clippy、debug tests、release tests 全部通过。M17 仍 live-unqualified，未来手感 A/B 见 `docs/M17_ACCEPTANCE.md`。

## M18 — Configurable Gesture Mapping

**已批准（`reviews/M18_REVIEW.md`）**：增加 strict `GestureMapConfig v1` 与统一 `UserSettings v1`，将 M17 feel 设置和 gesture→typed `DesktopAction` 映射合并为一个用户设置文件；支持 pinch/rotate、two-finger page、three/four-finger swipe、edge swipe、thumb+three、three-finger tap 的方向化映射，target 为 `passthrough` / `disabled` / 闭合集合 typed desktop action。mapped continuous gesture 只在 Begin 触发一次 action，后续 Update/End 被抑制。review 发现并关闭 R1：M15 three-finger drag 会抢先于 M14 three-finger swipe；`three_finger_drag_enabled` 默认 true 保持 M17，关闭后只禁止 drag commit，显式 three-finger tap mapping 仍可用。M19 KDE 接入后 `settings-macos` preset 收窄为真实 M19 可执行的 workspace/overview/present-windows/launcher/show-desktop 子集，其余 unsupported route 默认 disabled；仍不声称 macOS 等价。CLI/GUI 提供 `settings-default/macos/check/show/set/patch/gui`；`m18-remap-v1` 必须显式 `--settings FILE`。M18 profile 本身仍停在 typed action 边界；真实 KDE executor 由 M19 production backend 提供。M18 仍 live-unqualified。

## M19 — Safe Live Settings Hot Reload

**已批准（`reviews/M19_REVIEW.md`；真实 KDE integration 由 Re-review 1 收口）**：增加 `m19-live-v1` 与 foreground settings watcher；只有显式 `--settings FILE --watch-settings` 才启用。文件约按既有 bounded loop cadence 检查：合法变更构建完整新配置，非法/半写入文件只报告 `reload rejected` 并继续 last-good，后续合法保存自动恢复。配置绝不在 active pointer/scroll/momentum/gesture/three-finger-drag/button ownership 中途切换；busy 时仅保留最新 valid generation，neutral boundary 才原子应用并清 tunable filter/router residue。M19 继承 M18 user settings / gesture mapping；当前实机 tap-drag 语义是“一次 clean tap 后，在 180 ms 内再次落指并越过 pointer threshold 才 commit drag”，严格晚于 180 ms 的接触按普通 pointer 处理，同时关闭单指 sticky tap-drag lock，使真实 drag 在 clean `Ended` 帧立即释放 Left；M10–M18 历史 profile 不变。三指 drag 在 M19-only stable-reference 模式下仍用 centroid 判 commit，但 commit 位移只用于分类、不再补发；commit 时选择稳定 tracking-id reference，后续只按该 reference 增量移动，reference 抬起时零位移 re-baseline，第一次真实 PointerMove 才建立 synthetic Left。干净 `3→2→1→0` 始终由该 drag 独占并在 cluster empty 时唯一 release；M15–M18 保留历史 centroid/replay 行为。M19 同时为三指 drag 分离 fidelity，高速 `max_gain` 封顶 1.6，普通 pointer 继续使用用户配置。真实 portal/libei 输出把 `ButtonDown+first PointerMove` 与 `final PointerMove+ButtonUp` 各自保持在一个 EIS logical hardware frame，tap pulse 仍分两帧；`OutputSink::submit_frame` 保留 accepted-prefix/fail-stop。production M19 组合 M6/M10 portal+libei pointer/button/scroll 与 KDE Plasma 6 KGlobalAccel 离散 DesktopAction：workspace next/previous、Overview、Present Windows、Show Desktop、Application Launcher；unsupported mapping 在启动时 grab 前拒绝，hot reload 时拒绝该 generation 并保留 last-good。`settings-patch` 支持第二终端实时调参；没有 daemon、network listener、autostart 或 arbitrary shell execution。M19 仍 live-unqualified，用户实测见 `docs/M19_ACCEPTANCE.md`。

## Review 输出格式

每个里程碑完成后，执行代理必须报告：

1. 创建或修改的文件。
2. 实际实现的能力与明确未实现能力。
3. 三条验收命令的完整结论。
4. 设计偏差及理由。
5. reviewer 应重点检查的风险。
