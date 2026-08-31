# Touchpad Runtime：Phase 2–5 接管与 macOS 风格能力实施计划

状态：规划基线（2026-08-16）  
前置条件：M1–M5 已通过 review；任何实现里程碑仍须逐项 review 后才能推进下一项。

本文把“完整接管物理触控板并向当前桌面提交已解析语义事件”拆成安全、可独立验收的增量。`design.md` 仍是架构原则来源；本文覆盖 M5 之后的执行顺序、硬件能力边界和当前 KDE Wayland 落地闸门。

## 1. 最终数据路径与所有权

```text
physical evdev touchpad
        ↓  (explicit EVIOCGRAB; only after output is ready)
Type-B decoder / resync
        ↓
contact classifier
        ↓
single Interaction Arbiter
        ↓
pointer / tap-drag / scroll / gesture engines
        ↓
typed semantic OutputEvent
        ↓
qualified desktop output backend
        ↓
current KDE Wayland desktop
```

运行时是唯一 touchpad policy owner。桌面只能收到已解析的相对指针、按键、平滑滚动、快捷键或桌面动作，不得重新收到原始触点、finger count 或虚拟 touchpad。因此系统现有触控板设置只作为体验 A/B 基线，不作为运行时配置依赖，也不直接复制为算法参数。

## 2. 当前机器的事实边界

- 目标环境：Manjaro Linux、KDE Plasma Wayland。
- 物理设备：`CIRQ1080:00 0488:1054 Touchpad`，Type-B buttonpad，5 slots，约 131 × 77 mm，物理 `BTN_LEFT` 可用。
- 当前桌面已提供 XDG RemoteDesktop portal v2，设备类型包含 keyboard、pointer、touchscreen；系统安装 libei/liboeffis 1.6.0。
- 已有 KDE/libinput 体验参数可用于盲测基线，尤其 tracking speed、natural scrolling、two-finger scrolling、tap-drag-lock；它们不得静默变成项目默认值。
- 当前 descriptor 没有可信压力轴，也没有可用的触觉输出接口。因此 genuine Force Click、连续压力控制、click pressure、click/alignment haptic 在这台硬件上必须报告 `unsupported`。不得用停留时长、接触面积或软件振动冒充 Force Touch。
- 首选输出候选是 portal + libei。它仍由 compositor/EIS 做最终处理，所以“相对移动是否再次加速、平滑滚动是否被再次解释”必须实测，不能仅凭 API 名称宣称成立。

## 3. 安全不变量

1. 输出 session、能力和 `release_all` 路径必须在申请物理 grab 前准备完成。
2. 默认命令永不 grab、永不发射真实桌面输入；真实发射和 takeover 都要独立、显式 opt-in。
3. 初次 takeover 必须有倒计时、最大持续时间、清晰状态输出和外部键盘/鼠标紧急退出办法。
4. 输出失败、portal 撤销、decoder degraded、`SYN_DROPPED` 恢复失败、设备拔出或受控信号都必须 fail-open：先停止产生新语义事件，释放虚拟 button/key/scroll 生命周期，再结束 recorder，最后 ungrab/close。
5. 不能保证 `SIGKILL`、内核崩溃或断电时执行用户态 cleanup；不得在文档中作此承诺。
6. core 保持平台无关；portal/libei/KDE 依赖停留在平台 adapter。
7. 无桌面、无 portal、无 `/dev/input` 的 CI 必须继续运行全部自动测试。
8. 每个里程碑只实现自己的范围；未实现能力返回明确 unavailable/unsupported，不使用假成功 stub。

## 4. 功能覆盖策略

| macOS 风格能力 | 项目语义 | 计划里程碑 | 当前硬件/桌面策略 |
|---|---|---:|---|
| 单指移动、tracking speed | `PointerMove` + pointer curve | M7、M11 | 支持；先线性，再经 A/B 调曲线 |
| 单指按压、按住移动、双击 | physical button lifecycle | M7、M8 | `BTN_LEFT` 支持 |
| Tap to Click、tap-and-drag、drag lock | tap/drag state machine | M8 | 可配置；覆盖取消和超时边界 |
| 双指二维滚动、natural scrolling | pixel scroll lifecycle | M9 | 支持；方向可配置 |
| 惯性滚动 | momentum state machine | M12 | 软件实现；新接触/点击立即取消 |
| 双指右键 | secondary click | M9 | 支持 tap 与 click-zone 策略配置 |
| 双指 pinch、rotate | continuous gesture | M14 | 输出为应用级手势或可配置降级动作，取决于后端能力 |
| 双指双击 Smart Zoom | configurable semantic action | M14 | 不假设所有应用有统一动作 |
| 双指左右翻页 | configurable page action | M14 | 与普通二维滚动通过 arbiter 消歧 |
| 右边缘双指 Notification Center | KDE desktop action | M14、M15 | KDE adapter 映射；不是 core 常量 |
| 三指拖动 | drag mode | M15 | 独立可配置模式；与三指手势互斥 |
| 三指轻触 Look Up/Data Detectors | configurable semantic action | M15 | KDE/Linux 无统一 Apple 语义；由 profile 映射 |
| 三/四指上下左右滑 | workspace/window desktop action | M14、M15 | arity 可配置；映射 Overview、Present Windows、切换桌面等 |
| 拇指 + 三指张开 Show Desktop | desktop action | M14、M15 | classifier 明确 thumb 后才提交 |
| 拇指 + 三指捏合 Apps | desktop action | M14、M15 | KDE adapter 映射应用启动器/Overview |
| Force Click、连续压力 | pressure semantic | M16+ capability gate | 本机 unsupported；只为未来有压力轴硬件保留接口 |
| click/alignment haptic、click pressure | haptic output | M16+ capability gate | 本机 unsupported；不模拟 |

## 5. 里程碑

### M6 — KDE Wayland 输出后端资格验证

状态：**已实现，待外部复核（2026-08-16）**。实现事实见 `DESIGN_V2.md` §17；验收矩阵与 reviewer 人工 `--emit` 测量程序见 `docs/M6_ACCEPTANCE.md`。以下为原计划基线（保持为验收参照）。

目标：实现一个受控的 portal/libei 输出 adapter 和诊断工具，证明桌面输出生命周期可用；不读取或 grab 物理触控板。

范围：

- 保持 `touchpad-core::OutputSink` 契约不变或只做向后兼容的必要增强。
- 在 Linux 平台边界实现 portal/libei session 生命周期：未连接、授权中、ready、emulating、stopping、stopped/fatal。
- 映射相对指针、主/次键、pixel-precise scroll begin/delta/stop；只有能力存在时才报告支持。
- 所有正常、错误和 Drop 路径释放已按下的 button/key 及开放的 scroll 生命周期；部分写入失败不能虚报成功。
- 提供 `touchpadctl output-probe`：默认仅打印环境/能力和将要执行的步骤，不发射事件；真实发射必须额外显式 `--emit`，有提示和倒计时，发射量有严格上限。
- portal 拒绝、取消、无会话总线、缺库、协议版本/能力不足均给出结构化、可操作错误。
- 新依赖记录进 `THIRD_PARTY.md`；若使用 FFI，unsafe 只位于最小且有安全不变量说明的边界。
- 建立 fake transport/session，使无 Wayland、无 portal 的 CI 覆盖顺序、backpressure、断连和 `release_all`。
- 写出人工 A/B 验证步骤：小/中/大相对 delta 的桌面实际位移、重复采样、pixel scroll、按键释放。没有测量证据前，backend 状态只能是 `experimental/unqualified`，不能成为 takeover 默认值。

明确不做：物理设备 grab、常驻服务、指针/滚动算法、tap 或 gesture；测试不得自动移动真实鼠标或点击。

验收闸门：

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace`
- dry-run 在当前 KDE Wayland 上能准确报告 portal/libei 能力，取消授权不 panic。
- reviewer 手工执行 `--emit` 后确认受限 pointer/button/scroll 序列和 cleanup；记录是否存在 compositor 二次加速/转换。
- reviewer 将后端标记为 `qualified` 前，不允许进入真实 takeover。

### M7 — Arbiter 骨架、单指指针与物理点击（离线）

- 实现统一 Interaction Arbiter 的 `Candidate / Committed / Cancelled / Finished` 生命周期。
- 单指移动输出相对 pointer delta；毫米输入、逻辑像素输出和余数累计保持类型隔离。
- 物理 `BTN_LEFT` 实现主键 down/up、click、double-click 和按住移动。
- 全部由 trace/合成 `ContactFrame` 驱动；不得 grab 实机。
- 手势候选期不得泄漏不可撤销 pointer/button 事件。

### M8 — Tap、Tap-and-Drag 与 Drag Lock（离线）

- 可配置 tap-to-click、双 tap、tap 后再次触摸拖动、drag lock。
- 明确定义时间/位移阈值、物理点击竞争、额外触点、取消、断连和 release_all。
- 利用现有 KDE 体验作为 A/B 对照数据，不读取 KDE 配置作为运行时依赖。

### M9 — 双指基础滚动与右键（离线）

- 二维 pixel scroll、natural direction、begin/delta/end；双指斜向必须保留两个轴。
- 双指 tap/click 映射 secondary click。
- arbiter 区分 pointer、scroll、tap、click，不允许同一 interaction 重复提交。
- 本阶段无惯性、pinch 或 rotate。

### M10 — 限时安全 Takeover 纵向切片

- 串起真实 evdev → decoder → M7–M9 → 已 qualified 输出后端。
- 输出 ready 和 recorder header flush 后才允许 `EVIOCGRAB`。
- 新命令必须显式指定设备、`--takeover`、确认和最大持续时间；首版禁止后台/开机自启。
- 状态机和 shutdown 统一管理输出释放、recorder finalization、ungrab、close，保留主错误和全部 cleanup 诊断。
- 依次做 10 秒、60 秒、5 分钟人工验收，再决定是否允许无时限运行。

### M11 — 指针保真度

- hardware normalization、subpixel accumulation、jitter dead-zone、速度估计、tracking speed。
- 实现有界、连续、可测试的 macOS 风格速度相关增益曲线；不复制 Apple 私有实现或参数。
- 用可重复轨迹和 KDE 当前体验做盲 A/B；默认参数来自测量，不从系统配置静默复制。

### M12 — 滚动保真度与 Momentum

- 平滑滚动增益、速度估计、惯性衰减、方向反转和轴锁定策略。
- 新接触、按钮、反向滚动、设备断连和输出失败必须立即取消 momentum。
- 验证桌面没有把 pixel scroll 二次转换为离散 wheel。

### M13 — 触点鲁棒性

- palm/thumb 分类、typing suppression、边缘起始、jitter filtering、contact replacement。
- classifier 只使用设备真实提供的特征；缺压力/major/minor 时采用显式降级策略。
- 为当前 CIRQ1080 建立 `DeviceProfile` quirks，但通用算法不能依赖单一设备。

### M14 — 连续手势识别

- pinch、zoom、rotate、双指翻页、Smart Zoom、边缘手势、三/四指 swipe、thumb+3 pinch/spread。
- 每个手势都有 candidate/commit/cancel、方向反转、触点增减、阈值迟滞和连续 progress。
- 后端不支持原生连续手势时，只能使用显式配置的 semantic/shortcut 降级，不能声称等价。

### M15 — 三指拖动与 KDE 桌面动作

- 三指拖动模式及 drag lock；和三指 tap/swipe 的优先级由 arbiter 唯一裁决。
- 独立 KDE adapter 映射 Overview/Mission Control、Present Windows/App Exposé、Desktop Spaces、Show Desktop、Apps、Notification Center 等。
- 所有映射可发现、可配置、可禁用；core 不依赖 KDE 名称或 D-Bus API。

### M16 — 生产化与扩展能力

- 设备重连、portal session 重连、服务生命周期、配置版本化和升级/回滚。
- 在多台设备、桌面和发行版上验证；X11/uinput 只能作为经过同等资格测试的 adapter。
- 压力与 haptic 仅在未来硬件和输出接口真实提供能力时实现；当前机器继续明确报告 unsupported。

## 6. 每个里程碑的 review 输出

dsh 每轮必须报告：

1. 创建/修改的文件。
2. 实际实现、未实现和降级能力。
3. 三条 workspace 质量命令的完整结论与测试数量。
4. 自动测试、环境探测和实机验证分别列出，不能混写。
5. 设计偏差、依赖/许可证和 unsafe 边界。
6. reviewer 应重点检查的风险。

Reviewer 每轮只作批准、要求修复或阻断决定。未批准时 dsh 只修当前里程碑；批准后才启动下一项。
