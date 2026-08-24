# Touchpad Runtime — DESIGN V2（M1/M2/M3/M4 决策记录 + M5 已实现记录）

状态：**M1–M9 已批准**；**M10–M11 code-complete / review-approved、live-unqualified**（见 §21–§22）；**M12–M16 code-complete / review-approved、live-unqualified**（见 §23 与 `reviews/M12_REVIEW.md`–`reviews/M16_REVIEW.md`）；**M17（Tunable Feel Parameters）code-complete / review-approved、live-unqualified**（见 §24）；**M18（Configurable Gesture Mapping）与 M19（Safe Live Settings Hot Reload）均 code-complete / review-approved、live-unqualified**（见 §25–§26 与 `reviews/M18_REVIEW.md`、`reviews/M19_REVIEW.md`）。M10 及其后续 live 资格仍依赖各自独立的用户受控验收；M17–M19 的 settings/control plane 不改变 takeover safety、cleanup、output qualification 或 service lifecycle 规则。
本文是 `design.md`（未修改）与 `IMPLEMENTATION_BRIEF.md` 的工程化补充，只记录**当前已确定且已实现**的模块边界、数据契约、失败语义和后续闸门；不宣称任何尚未实现的能力。

## 1. 当前 workspace 布局

```text
Cargo.toml            # workspace 根：成员 crates/touchpad-core、crates/touchpad-trace、crates/touchpad-linux、crates/touchpad-desktop 与 apps/touchpadctl
crates/
  touchpad-core/      # 平台无关类型与契约（M1）
                      # + arbiter：Interaction Arbiter、一指指针/物理点击（M7）、
                      #   tap/tap-and-drag/drag lock（M8）、双指二维滚动/二次 tap/
                      #   buttonpad 双指物理二次点击（M9）
  touchpad-trace/     # 版本化 JSON Lines raw trace 读写 + 平台中立 replay 边界（M2）
  touchpad-linux/     # Linux RawEvent 边界 + Type-B slot decoder + mocked resync（M3）
                      # + 设备枚举/探测、syscall seam、EVIOCGRAB guard、真实 snapshot adapter、输入 runtime（M4）
                      # + raw-event recorder（decoder 之前）与 SIGINT/SIGTERM 信号接缝（M5，§16）
  touchpad-desktop/   # KDE Wayland 输出 adapter：RemoteDesktop portal（zbus）+ 运行时加载 libei sender、
                      # 会话生命周期、held-state/release_all、固定有界 --emit pattern、环境 probe（M6，§17）
apps/
  touchpadctl/        # CLI：devices / inspect / record [--grab] / replay（M5，§16）
                      # + output-probe [--emit]（M6，§17）
README.md             # 构建、测试、四个命令示例、权限诊断、--grab 风险、信号退出、x86_64 限制、
                      # 自动测试/环境探测/实机验证三者严格区分（M5 新增）
docs/
  M5_ACCEPTANCE.md    # M5 验收矩阵（自动测试、环境观察、未执行的实机清单）（M5 新增）
DESIGN_V2.md          # 本文
THIRD_PARTY.md        # 直接依赖与许可证（M4 起；M5 补全锁定版本）
```

`IMPLEMENTATION_BRIEF.md §3` 中的 `apps/touchpadctl` 已在本里程碑（M5）创建并接入。

## 2. `touchpad-core` 模块图（M1，未变）

| 模块 | 关键类型 | 职责 |
| --- | --- | --- |
| `units` | `Millimeters`、`LogicalPixels`、`RawAxis`、`NonFiniteError` | 类型化单位；raw/mm/逻辑像素不可混用；无 panic 构造器、无算术 operator trait，finite 不变量只由 fallible API 保证 |
| `time` | `Monotonic` | 边界层提供的纳秒精度单调时间戳；core 自身不读取任何时钟 |
| `axis` | `AxisInfo`、`AxisConversionError`、`raw_axis_position_to_mm`、`raw_axis_position_to_mm_with_resolution`、`raw_axis_delta_to_mm` | 轴描述与 raw→mm 转换；绝对 position 与相对 delta 为独立 API，原点语义统一 |
| `device` | `DeviceDescriptor`、`AxisId` | 平台无关设备描述 |
| `contact` | `Contact`、`ContactState`、`ContactFrame`、`PhysicalButtons` | 触点与帧模型 |
| `diagnostic` | `Diagnostic`、`DiagnosticCode`、`DiagnosticLevel` | 结构化诊断 |
| `output` | `OutputEvent`、`OutputSink`、`OutputError`、`RecordingSink`、`MouseButton`、`KeyId`、`DesktopAction` | 语义输出契约（无真实桌面注入） |
| `profile` | `DeviceProfile`、`DeviceQuirk` | 显式硬件调整（resolution override、quirks） |
| `validation` | 有限值/归一化检查与 serde 辅助 | 构造器与反序列化的统一校验 |

M1 的核心不变量（raw/mm 类型隔离、毫米/像素 finite 且无 panic 路径、position 原点语义、缺失 resolution 的唯一 override 路径、手势时序只用边界层 monotonic 时间、可选能力缺失不使整帧不可用、结构化诊断而非 panic）保持不变，见 M1 已批准记录。

## 3. `touchpad-trace` 模块图（M2 已实现）

| 模块 | 关键类型 | 职责 |
| --- | --- | --- |
| `time` | `TraceTime`（`sec: u64`、`usec: u32`） | trace 时间戳：`(sec, usec)` 对；**checked** 转换 `to_monotonic() -> Option<Monotonic>`（`usec` 越界或 `u64` 纳秒溢出返回 `None`）；`from_monotonic` 截断亚微秒残差 |
| `event` | `TraceEvent`、`TraceFieldError` | 原始 kernel 风格事件 `type/code/value` + 时间；`event_type` 为 `u16`（JSON 字段名 `"type"`）、`code` 为 `u16`、`value` 为 `i32`；`validate_fields()` 只校验字段范围（不校验时序） |
| `header` | `TraceHeader`、`TraceClock` | schema 版本 + clock 语义 + **嵌套** `device: DeviceDescriptor`（设备标识、capabilities、轴 ranges、resolution、slot 数） |
| `reader` | `TraceReader`、`Events` | 逐行流式读取；header 首行且仅一次；schema/clock/范围/时间倒退校验；区分错误类别 |
| `writer` | `TraceWriter` | 逐行流式写入；header 在 `new` 中立即写入且仅一次；flush/finish 语义；忠实记录原始时间戳 |
| `replay` | `ReplaySink`、`ReplayDriver`、`ReplayError`、`ReplayStats`、`RecordingSink`、`SinkError` | 平台中立 replay 事件流边界；驱动 header→events→finish；**不实现 decoder、不产生 ContactFrame** |
| `error` | `TraceError` | 错误分类（见 §5） |

## 4. Trace schema v1 契约（M2 已实现）

文件为 JSON Lines：每行一个 JSON 对象；**首行必须是 header，且 header 只能出现一次**。

```json
{"kind":"header","schema_version":1,"clock":"monotonic","device":{"name":"...","vendor_id":0,"product_id":0,"axes":{"0":{"min":0,"max":1000,"fuzz":0,"flat":0,"resolution":100}},"slot_count":10,"supports_type_b_mt":true,"has_physical_buttons":true,"profile":{"name":"default","axis_resolutions":{},"quirks":[]}}}
{"kind":"event","sec":0,"usec":1234,"type":3,"code":47,"value":0}
```

- **Header**：`schema_version`（`u32`，当前只支持 `1`）、`clock`（`"monotonic"`，v1 唯一 clock）、`device` 为**嵌套对象**（刻意不 flatten）。嵌套理由：`#[serde(flatten)]` 的 Content 缓冲无法往返整数型 map key（`BTreeMap<AxisId, _>`），嵌套后设备字段经 serde_json 的 MapKeyDeserializer 正常解析；且 schema 级字段与设备字段自然分离，不修改 core 的 `AxisId` 序列化格式。
- **Event**：`sec`（非负整秒，`u64`）、`usec`（`[0, 999_999]`）、`type`（`u16`，kernel `EV_*`）、`code`（`u16`，kernel `ABS_*`/`KEY_*`/`SYN_*`）、`value`（`i32`）。event 保留原始 kernel 事件的 sec/usec 精度（微秒），不存解码后的数据。
- **范围策略**：`sec ∈ [0, u64::MAX]`；`usec ∈ [0, 999_999]`（`1_000_000` 及以上为非法字段，不静默进位到下一秒）；`type/code` 由 `u16` 类型约束；`value` 由 `i32` 类型约束；`sec * 1_000_000_000 + usec * 1_000` 必须能放进 `u64` 纳秒（checked conversion，溢出为明确错误，绝不截断）。
- **数值字段分类（reader，R2 修订）**：所有整数型字段（`sec`、`usec`、`type`、`code`、`value`、`schema_version`）**不经 serde 预收窄**直接反序列化，而是先按原始 `serde_json::Number` 读取，再做显式符号/整性/范围分类：缺失或非数字值（字符串、bool、`null`、对象、数组）→ `CorruptedLine`（形状错误）；小数或指数形式数字（`1.5`、`1e3`）→ `InvalidField`（非整数，绝不静默截断）；符号/范围越界（负数 `sec`、`usec ≥ 1_000_000`、`type`/`code ∉ [0, 65535]`、`value ∉ i32`）→ `InvalidField`；大于 `i64::MAX` 但可表示为 `u64` 的正整数 `schema_version` → `SchemaTooNew`。因此 `sec == i64::MAX + 1`、`sec == u64::MAX` 等合法 `u64` 语法值一律进入字段校验（时间戳溢出 → `InvalidField`），绝不误报为 `CorruptedLine`。
- **无 wall-clock 语义**：header 声明 clock domain；`TraceTime::to_monotonic()` 是 trace 时间进入 core `Monotonic` 的唯一路径；replay 不做 wall-clock 节奏（pacing 不在 M2 范围）。

## 5. 错误分类与时间策略（M2 已实现）

`TraceError` 按 IMPLEMENTATION_BRIEF §3.3 区分类别：

- 不支持的 schema：`SchemaTooNew { found, supported }` / `SchemaTooOld { found, supported }`（新 schema 显式失败）。
- 损坏行：`CorruptedLine { line_number, message }`（非合法 JSON、或不是 trace 行形状、缺字段、空行）。
- 字段不合法：`InvalidField { line_number, message }`（`usec` 越界、`sec` 为负、clock 不支持、时间戳不可转换等）。
- header 问题：`EmptyTrace`（空文件）、`MissingHeader { kind }`（首行不是 header）、`DuplicateHeader { line_number }`（header 重复）。
- 时间倒退：`TimeRegression { line_number, previous, current }`。
- I/O：`Io(#[from] io::Error)`。
- 中毒流：`Poisoned(&'static str)`（R1/R3 修订）——reader/writer 在部分读/写失败后的终态错误：此后所有 header/event（reader）或 `write_event`/`flush`/`finish`（writer）操作确定性拒绝，绝不恢复继续。
- API 误用：`InvalidState(&'static str)`（如 `read_event` 先于 `read_header`、`finish` 后写、重复 `finish`）。

**事件时间非递减策略**（显式区分读写两侧）：

- **Reader（replay）**：event 时间必须非递减；倒退即 `TimeRegression` 错误。理由：replay 的时序语义（timeout、velocity）依赖单调时间线，静默继续会产出错误手势。
- **Writer（record）**：忠实记录原始时间戳，**不**因倒退拒绝事件——recorder 位于 decoder 之前，trace 必须保留真实（即使异常）的 kernel 时间戳用于回归复现；writer 只校验字段范围。

**成功 writer 输出并不总是能被 replay 接受（R4 修订，明确例外）**：含时间倒退的捕获是「忠实记录但 replay 无效」的诊断产物——writer 原样保留倒退的时间戳（不归一化、不丢弃），reader/replay 在**确切的那一行**报 `TimeRegression`。「writer 输出必被 reader 接受」只对无倒退的事件流成立；该例外有端到端测试覆盖（writer 接受并保留倒退 → reader/replay 在正确行号报错）。header 本身总是可被 reader 接受（写出前校验 schema/clock）。

**未知可选字段的向前兼容策略**：

- 同一 schema 版本内，reader 忽略 header/event/device 上的未知可选字段（serde 默认行为，不用 `deny_unknown_fields`）。未来 writer 只允许在**不升版本**的情况下新增可选字段。
- **未知 line kind 是错误**（`UnknownLineKind`），绝不跳过：新 kind 代表 reader 无法复现的语义，静默跳过可能错误回放。任何结构性变更（新 kind、新必填字段、新 clock）必须升 `schema_version`。

## 6. Writer flush/finish 语义（M2 已实现，R1 修订）

- `TraceWriter::new(inner, header)`：先校验 header（schema 版本、clock），再**立即**把 header 写成第一行；构造后不存在"没有 header 的 trace"。header 写出失败发生在 `new` 内部，返回错误且不产出 writer 对象。
- `write_event`：逐行提交完整 JSON 行（`Write::write_all`，本 crate 绝不交错行）；校验字段范围（不校验时序，见 §5）。**失败语义精确二分**：
  - **写出前失败（可恢复）**：字段校验/序列化失败发生在任何字节到达底层 writer 之前，writer 保持可用，调用者可重试、flush 或 finish。
  - **事件行 I/O 失败（中毒，不可恢复）**：泛型 `Write::write_all` 可能写出前缀后才报错；此后流中可能存在**部分行**，writer 进入终态 **poisoned**（`TraceError::Poisoned`），`write_event`/`flush`/`finish` 一律确定性拒绝——绝不暗示重试安全（重试会把新的 JSON 对象追加到残缺前缀上，静默损坏 trace）。
- `flush()`：把缓冲字节推到内层 writer 并上报 I/O 错误；flush 失败不产生部分**行**，因此不中毒，但中毒后的 flush 被拒绝。
- `finish()`：正常结束——flush 并标记 finished；重复 `finish` 或 `finish` 后写入返回 `InvalidState`；中毒后的 `finish` 返回 `Poisoned`。
- Drop 未 `finish` 视为**异常结束**：`Drop` 尽力 flush，已写行保持可读，但不承诺干净结束。
- 正常路径逐行写出/读入 ⇒ 大 trace 内存占用为一行，不整体载入内存（200k 事件 streaming 测试覆盖）；I/O 失败后流可能含部分行，不再是干净 JSON Lines——这正是中毒语义要保证「不再假装干净」的原因。

## 7. Replay 边界（M2 已实现；M3 已接入 decoder）

`ReplaySink` 是平台中立的原始事件消费契约：

```text
ReplayDriver::replay(input, sink)
  → TraceReader（header 首行/仅一次、schema、时间策略）
  → sink.on_header(&TraceHeader)      # 一次
  → sink.on_event(&TraceEvent)        # 每个原始事件，按 trace 顺序
  → sink.finish()                     # 仅当整条 trace 干净读完
```

- M3 的 Type-B decoder 实现 `ReplaySink`，且使用与实时输入**相同**的状态机（`IMPLEMENTATION_BRIEF §8`：不建立第二套状态机）；trace header 的 `DeviceDescriptor` 就是 replay 时使用的设备描述。**（M3 已实现：见 §12.4/§12.5）**
- **M2 明确不实现 decoder、不产生 `ContactFrame` 输出**：`RecordingSink` 只是原样记录原始事件的观察者（测试/验证用），不是解码器。
- trace 损坏时 `finish` 不被调用；sink 拒绝/失败通过 `ReplayError::Trace` / `ReplayError::Sink` 区分。
- **`finish` 的同步状态要求（M3 R5 修订，见 §12.7）**：普通 trace 在帧与帧之间结束（decoder 处于 `Normal`）是干净完成；以未解决的同步丢失结束（`SYN_DROPPED` 后未恢复，decoder 处于 `DroppedAwaitingBoundary`/`Recovering`/`Degraded`）则 `finish` 返回结构化错误且不产生帧。
- **Reader 是 fail-stop（R3 修订）**：任何消费/解析/校验 trace 行的失败——包括底层 I/O 失败——都把 reader 置为终态 failed，此后所有 header/event 操作返回 `TraceError::Poisoned`，原始错误只报一次。由此杜绝两类绕过：首行 header 失败后接受第二行 header（违反「首行必须是 header」）；时间倒退/损坏行之后继续消费后续事件。API 误用（`InvalidState`，如先 `read_event` 后 `read_header`）不消费行、不中毒。

## 8. M2 测试（全部无硬件；R1–R4 修订后共 134 个 workspace 测试）

- 110 个单元测试（core 36、trace 74）。trace 单元测试除原有类别外，R1–R4 修订新增：
  - **writer 中毒（R1）**：故障注入 `Write`（指定字节数后失败）证明部分行确实可达底层 writer（header+换行+事件行严格前缀），且部分行后 `write_event`/`flush`/`finish` 一律 `Poisoned`；行终止符写失败同样中毒；header 写出失败不产出 writer；校验失败（写出前）后 writer 仍可用。
  - **数值边界分类（R2）**：`schema_version > i64::MAX` → `SchemaTooNew`；`sec == i64::MAX + 1` 与 `sec == u64::MAX` → `InvalidField`（时间戳溢出，非 `CorruptedLine`）；负/越界 `type`、`code`、`value`；小数（`1.5`）与指数形式（`1e3`）→ `InvalidField`；非数字（字符串、bool）与缺失 → `CorruptedLine`。
  - **reader 终态失败（R3）**：首行 header 失败后拒绝第二行 header；损坏行/时间倒退/I/O 失败后拒绝继续消费事件（后续调用均 `Poisoned`）；API 误用不中毒。
- 5 个人工 fixture（`crates/touchpad-trace/tests/fixtures/`）：`single_contact`（单触点 begin/update/end）、`multi_slot`（多 slot 交错）、`buttons`（物理按钮 + 触点）、`missing_resolution`（轴无 resolution）、`dropped_recovery`（`SYN_DROPPED` 在中间）。均为原始 kernel 事件序列，是 M3 decoder 的输入语料。**M3 修订：fixture header 的轴键从占位的 `0`/`1` 更新为 Linux 层轴约定 `53`/`54`（`AxisId::new(ABS_MT_POSITION_X/Y)`），使 fixture 与 M4 设备探测将产出的 descriptor 一致（见 §12.3）。**
- 集成测试（22 个）：fixture 读取契约验证（4）；`ReplayDriver` 对每个 fixture 原样转发事件并给出 `ReplayStats`（4）；writer→reader 全量 round-trip、200k 事件 streaming、**回归端到端例外**（R4：writer 接受并原样保留倒退时间戳，reader/replay 在正确行号报 `TimeRegression` 且不调用 sink `finish`）（6）；core 契约（8）。

## 9. M2 明确未实现（不得虚报）

以下为 M2 结束时的未实现清单；标注「(M3 已实现)」的条目已由 M3 完成（见 §12）：

- Type-B slot decoder、`SYN_REPORT` 帧提交、`SYN_DROPPED` 恢复状态机（M3；`DiagnosticCode::DecodeRecovered/DecodeDegraded` 仍只是预留代码）**(M3 已实现；`DecodeRecovered` 已实际使用，`DecodeDegraded` 仍预留，见 §12.6)**。
- 设备枚举、capabilities/axis/slot 查询、`EVIOCGRAB`、RAII grab guard、真实 ioctl（M4）。
- `touchpadctl` CLI、record/replay 命令（M5）。
- replay 到 `ContactFrame` 的集成证明（M3 验收：fixture replay 必须复用与实时输入相同的 decoder 路径）**(M3 已实现，见 §12.8)**。
- Wayland/libei、X11、uinput 输出后端（后续阶段）。
- pointer/scroll/tap/drag/gesture 算法、GUI、常驻服务、开机自启、系统设置修改。
- 任何实机行为（Manjaro/Wayland/X11 触控板）验证。

## 10. 后续闸门（来自 MILESTONES.md）

- M3 — Type-B Decoder and Mocked Resynchronization **(已实现，见 §12；外部 review 终审通过，见 M3_REVIEW.md)**。
- M4 — Linux Device Boundary and Fail-Open Grab **(已批准，R1–R7/RR1–RR3 全部关闭，见 §14 与 M4_REVIEW.md)**。
- M5 — CLI Vertical Slice and Phase 1 Handoff **(已实现，待外部复核，见 §16；未经外部 review 批准前不得视为 approved)**。

每阶段结束执行：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace`，并由外部 reviewer 验收后才进入下一阶段。

## 11. 依赖与第三方

- `touchpad-core` 直接依赖仅 `serde`（derive）与 `thiserror`；dev-dependency `serde_json` 仅供测试。
- `touchpad-trace` 直接依赖 `touchpad-core`（path）、`serde`（derive）、`thiserror`、`serde_json`。无 Linux/Wayland/X11/KDE/GNOME 依赖，无 `/dev/input` 访问。
- `touchpad-linux` 直接依赖 `touchpad-core`（path）、`touchpad-trace`（path，用于实现 `ReplaySink` 边界与 `KernelEvent::to_trace_event`）、`thiserror`；M4 起新增 `libc`（**仅** `cfg(target_os = "linux")` 目标依赖，版本锁定 `=0.2.186`），只被 Linux-only 的 `sys::ffi::LinuxSys` 使用（C `input_*` 结构布局与 `open`/`read`/`ioctl`/`close` 原始 syscall）；M5 起 `libc` 还被 `sys::ffi` 的 `sigaction` 信号处理使用（同一 FFI 边界）。其余模块无 serde、无 Linux 系统库依赖、无 `/dev/input` 访问。**测试说明（M5 review 文档修正）**：绝大多数测试走 `sys::mock::MockSys`，但 `sys::ffi` 的 Linux-only 测试故意使用**真实、无副作用**的 OS 表面来验证 seam 本身——真实 `sigaction` 安装/恢复与真实 `raise(SIGINT)` 交付、对不存在路径的真实 `read_dir`（ENOENT）、对不存在设备节点的真实 `open(2)` 尝试（ENOENT/EACCES）——它们从不成功打开或 grab 真实设备。
- `touchpadctl`（M5）直接依赖三个 workspace crate（path）与已锁定的 `serde_json`（replay 的 JSON ContactFrame 输出）、`thiserror`（结构化错误）。**M5 未向 `Cargo.lock` 新增任何第三方 crate**；CLI 参数解析与帮助文本为手写，刻意不引入 CLI 框架。`THIRD_PARTY.md` 列出全部直接依赖的锁定版本与传递依赖（含 serde_json 引入的 `zmij` 等）。
- `touchpad-desktop`（M6）直接依赖 `touchpad-core`（path）、`serde`（body bound）、`thiserror`、`zbus`（**纯 Rust** D-Bus 客户端，`default-features = false` + `blocking-api`/`async-io`，不链接系统 D-Bus 库）、`libloading`（运行时加载系统 `libei.so.1`；**构建期不链接 libei**，缺库是结构化运行期结果而非构建失败）与 `futures-lite`/`async-io`（portal Response 等待的 deadline race，均已在锁图内）。libei FFI（`ffi`，Linux-only）是唯一含 `unsafe` 的新模块且为 **crate 私有**（`pub(crate) mod ffi`），句柄是 **non-`Copy` RAII owner**（M6 re-review R1）；其余所有模块 `#![forbid(unsafe_code)]`。
- **MSRV（M6 re-review R6）**：工作区 manifest 声明 `rust-version = 1.87`——锁图（zbus 5.19 与 zvariant 族）的真实最小声明；不再出现「manifest 声明 1.85 而锁图拒绝」的矛盾。声明 MSRV 尚未在 1.87 工具链上独立验证，本里程碑所有闸门在 rustc/cargo 1.97.1 上运行。
- `Cargo.lock` 已由 cargo 生成于工作区根目录。

## 12. M3 — Type-B Decoder and Mocked Resynchronization（已实现）

### 12.1 范围与边界

M3 实现 `touchpad-linux`：Linux 侧的**内部 `RawEvent` 边界**、**Type-B slot decoder**、**`SYN_REPORT` 帧提交**与 **`SYN_DROPPED` 恢复状态机**。M3 明确不触碰：`/dev/input` 访问、设备枚举、ioctl/FFI、`EVIOCGRAB`、syscall adapter、信号处理、CLI、输出后端。`touchpad-linux` 是 `unsafe`-free 的（`#![forbid(unsafe_code)]`），无任何平台依赖，普通用户/CI 可完整运行全部测试。

### 12.2 `touchpad-linux` 模块图

| 模块 | 关键类型 | 职责 |
| --- | --- | --- |
| `codes` | `EV_*`、`SYN_*`、`ABS_MT_*`、`BTN_*` 常量与 `axis_id_for_code` | 内核 input 事件码常量（kernel ABI，无 FFI）与「ABS code → `AxisId`」的 Linux 层轴约定 |
| `rawevent` | `RawEvent` | 解码器唯一输入边界：`timestamp: Monotonic` + `type/code/value`；`from_trace_event` 是 trace 进入解码器的唯一路径 |
| `sink` | `FrameSink`、`RecordingFrameSink` | 已提交 `ContactFrame` 的输出契约与测试观察 sink |
| `resync` | `ResyncSource`、`KernelStateSnapshot`、`SlotSnapshot` | 可 mock 的内核状态查询边界（M4 提供真实 ioctl adapter） |
| `decode` | `TypeBDecoder`、`SyncState`、`DecodeError` | slot 状态机、帧提交、恢复协议 |
| `replay` | `ReplayDecodeError` + `ReplaySink` impl | 离线 replay 驱动与实时输入**同一个** decoder |

### 12.3 RawEvent 边界与 Linux 层轴约定

- `RawEvent { timestamp: Monotonic, event_type: u16, code: u16, value: i32 }`。实时输入（M4）从 kernel `struct input_event` 构造它；replay 通过 `RawEvent::from_trace_event`（唯一转换路径，使用 `TraceTime::to_monotonic` 的 checked 转换）构造它。**两条路径调用同一个 `TypeBDecoder::feed`，不存在第二套 decoder/状态机**（§12.8 有直接证明测试）。
- **轴约定**：Linux 层以 kernel ABS code 作为 `AxisId`，即 `ABS_MT_POSITION_X`(53) → `AxisId::new(53)`、`ABS_MT_POSITION_Y`(54) → `AxisId::new(54)`。M4 的设备探测将按此约定构建 descriptor。
- **fixture 轴键修订**：M2 的 5 个 fixture header 原先用占位键 `0`/`1`；M3 将其更新为 `53`/`54`，使 fixture 与真实 Linux descriptor 一致（`touchpad-trace/tests/fixtures.rs` 中 X 轴断言同步更新）。这样「resolution 缺失 → 未归一化」与「resolution 存在 → 毫米化」两条路径都能被 fixture 真实覆盖。

### 12.4 状态机与帧提交

`SyncState` 为 `Normal | DroppedAwaitingBoundary | Recovering | Degraded` 四个显式状态。事件规则（`IMPLEMENTATION_BRIEF §5`；R1–R3 修订后）：

1. `ABS_MT_SLOT` 仅切换 current slot。**越界值 fail-closed（R1）**：产生 `SlotOutOfRange`（Error）诊断并**吊销 slot 选择**——此后所有 slot 作用域 `ABS_MT_*` 事件（tracking id、坐标、压力、长度、方向）一律被忽略并附 `InvalidEventOrder`（Warning）诊断，**绝不重定向到旧 slot**；只有收到合法的 `ABS_MT_SLOT` 才恢复。协议默认初始选择 slot 0（合法）。
2. `ABS_MT_TRACKING_ID >= 0` 开始新触点；同一 slot 出现不同 id 视为替换生命周期（旧触点隐式结束，帧附 `TrackingIdReplaced` Info 诊断；帧内同 slot 只允许一个触点，所以被替换的旧 id 不出现在帧中）。**tracking 转换在事件到达时增量地应用到每个 slot 的「有效生命周期」状态**（`PendingLifecycle: Active{id, fresh} | Empty | Ended`，R2 Re-review 1 修订，取代原先的 `Vec<TrackingTransition>` 有序列表——per-slot pending 状态为常数内存，replay 控制的流无法在没有 `SYN_REPORT` 的情况下无界增长）：`end(-1) -> begin(new)` 在边界留下**新**生命周期（坐标齐全则以 `Began` 发布）；`begin(new) -> end(-1)` 在边界**不**留下任何生命周期（附 `InvalidEventOrder` Warning，不发布——生命周期从未越过帧边界）；重复 begin 同一 id 是 no-op（触点继续，不替换、不结束，字段不重置）；多个替换只保留最终生命周期，每个替换步骤各附 `TrackingIdReplaced` 诊断（每 slot 每 cycle 最多保留 `MAX_TRACKING_REPLACEMENTS = 16` 步，超出部分计数并附一条汇总诊断，保证诊断缓冲同样有界）。新生命周期**绝不继承**旧触点字段：`Begin(new)` 清空字段桶，旧生命周期的字段不会泄漏进新生命周期。
3. `ABS_MT_TRACKING_ID == -1`（**恰好 -1**，R2）结束触点：已发布的触点以 `Ended` 状态（带最终坐标）出现在该帧；从未发布的触点不产生 `Ended`（消费者从未见过它）。**任何 `< -1` 的值都被诊断（`InvalidEventOrder` Warning）并忽略**，既不能结束也不能替换触点。
4. 其他 `ABS_MT_*` 更新 current slot 的 pending 字段，**字段关联到事件到达时刻的有效生命周期**（R2 Re-review 1 修订）：字段属于当时活动的生命周期，并在生命周期转换（替换/新 begin）时随旧生命周期一并丢弃；**有效生命周期为 ended/empty（无活动生命周期）时到达的字段被诊断（`InvalidEventOrder` Warning）并忽略**——既不改变先前 `Ended` 触点的最终状态，也不泄漏进稍后 `Began` 的触点。生命周期在边界结束（`end(-1)`，且边界无新生命周期）时，其**结束前**到达的字段仍应用到 `Ended` 触点（那是该生命周期自己的最终状态）。
5. **`ABS_MT_TOUCH_MAJOR`/`ABS_MT_TOUCH_MINOR` 是触点长度（R3）**：用 core 的**相对 delta/length 转换**（`raw / resolution`，`raw_axis_delta_to_mm`，无原点），**绝不**用绝对 position 转换（那会错误减去 `AxisInfo.min`，对非零 min 轴产生错误物理尺寸）；X/Y 仍用绝对 position 转换。两条路径（pending 与 resync snapshot）一致，都保留 resolution/profile override 与缺失 resolution 诊断（值保持未归一化 + `MissingAxisResolution`）。
6. 物理按钮事件（`BTN_LEFT/RIGHT/MIDDLE`）进入同一 pending frame。
7. **只有 `SYN_REPORT` 才合并 pending、递增 sequence 并发布帧**；单个事件绝不发布半帧。空 `SYN_REPORT` 也发布空帧（帧边界语义）。
8. 触点生命周期状态：新 tracking id 且坐标齐全 → `Began`；已发布且存活 → `Active`；结束 → `Ended`。未更新字段**继承**上一个 committed 状态（`IMPLEMENTATION_BRIEF §4`）。

**不完整新触点策略**（`IMPLEMENTATION_BRIEF §4`）：新 tracking id 在 X/Y 未齐全前保留为内部 incomplete slot，不发布为 `Contact`，帧附 `IncompleteNewContact`（Warning，每个生命周期一次）；补齐后发布，并在该帧附 `DelayedNewContact`（Info）。分辨率缺失/无 profile override 时坐标保持未归一化（`x_mm`/`y_mm` 为 `None`），帧附 `MissingAxisResolution`（Warning，每帧每轴去重一次），绝不伪造毫米。

### 12.5 恢复协议（`SYN_DROPPED`；R4 修订）

1. 收到 `SYN_DROPPED` → `DroppedAwaitingBoundary`；所有增量事件（ABS/KEY）被忽略，直到下一个 `SYN_REPORT`。
2. 该 `SYN_REPORT` → `Recovering`（瞬态，单个 `feed` 调用内进出），通过 `ResyncSource::snapshot()` 查询 `KernelStateSnapshot`（slots、tracking ids、各 ABS 字段、按键状态；可 mock，无需 `/dev/input`）。
3. **快照先完整校验、构建完整草稿状态，全部成功后才原子换入（R4）**：任何「越界 slot、重复 slot 条目、非法 tracking id（`< -1`）、活动触点缺失原始 X 或 Y」都使恢复失败（`apply_snapshot` 返回错误，不动任何 live 状态）。校验通过后把草稿换入 committed/pending/buttons/current-slot（`current_slot` 回到 0、slot 选择重新有效），发布**完整** `discontinuity = true` 帧（所有活动 slot 以 `Began` 出现，附 `DecodeRecovered` Info 诊断；快照按钮状态进入该帧），回到 `Normal`。
4. 失败（查询错误、未配置 resync source、或快照内容非法）→ `Degraded`（终态），`feed` 返回致命 `DecodeError::ResyncFailed`，**不发布任何 discontinuity 帧**；退化后不再产生任何可信帧，后续 `feed` 一律返回 `DecodeError::Degraded`（对应 M4 的「若持有 grab 必须释放」的失败语义；M3 不触碰 grab）。

### 12.6 诊断

- 新使用：`DecodeRecovered`（成功恢复的 discontinuity 帧）、`TrackingIdReplaced`（slot tracking id 替换，含同 cycle 多步替换的每一步，超过 16 步时其余步骤汇总为一条诊断）、`DelayedNewContact`（补齐后发布）；沿用 `IncompleteNewContact`、`InvalidEventOrder`、`SlotOutOfRange`、`MissingAxisResolution`。R1 修订后，非法 slot 选择吊销后每个被忽略的 slot 作用域事件都会附 `InvalidEventOrder`（Warning）；R2 修订后，`ABS_MT_TRACKING_ID < -1`、「同帧 begin→end」以及**无活动生命周期时到达的字段**（end 之前/之后的字段）也附 `InvalidEventOrder`（Warning）。
- `DecodeDegraded` **仍为预留**：恢复失败路径不发布任何帧（「不再产生可信输出」），失败以 `DecodeError::ResyncFailed` 结构化上报；该 code 留给 M4 及以后的失败/grab-release 通知。

### 12.7 Replay finish 语义与 slot_count 安全界（R5/R6 修订）

- **`ReplaySink::finish` 要求可信的终态同步状态（R5）**：decoder 处于 `Normal`（普通 trace 在帧与帧之间结束）时 finish 成功；处于 `DroppedAwaitingBoundary`/`Recovering`/`Degraded`（trace 在 `SYN_DROPPED` 之后、恢复 `SYN_REPORT` 之前结束，或已退化）时 finish 返回结构化 `ReplayDecodeError::UnresolvedSynchronizationLoss(state)`，**不产生任何帧**。端到端测试证明：以 `SYN_DROPPED` 结尾的 trace 回放失败而不是报告干净完成；以普通 `SYN_REPORT` 结尾的 trace 干净完成。
- **`slot_count` 安全界（R6）**：`configure` 只接受 `[1, MAX_SLOT_COUNT]` 的 slot 数，`MAX_SLOT_COUNT = 256`（文档化常量 `touchpad_linux::MAX_SLOT_COUNT`）。理由：Linux 输入子系统无硬性全局上限，但已知 Type-B 触控板至多报告几十个 slot；256 是宽松的安全上限，使每 slot 状态有界，并阻止 replay 控制的 header 请求数十亿 slot 造成分配失败/进程中止。超出上限的 descriptor 在构造任何 decoder 状态**之前**以 `DecodeError::InvalidDevice` 拒绝（含端到端回放测试：slot_count=10⁹ 的 header 干净失败、零帧）。

### 12.8 M3 测试（全部无硬件；workspace 共 190 个测试）

- `touchpad-linux` 单元测试（44 个）：单触点 begin/update/end、多 slot 交错、slot 切换不影响其他 slot pending、tracking id 替换/结束、字段继承（含 pressure）、不完整新触点（held→补齐发布+双诊断）、物理按钮与帧原子性、**非法 slot 选择 fail-closed（旧 slot 不被修改、被忽略事件不泄漏、合法选择恢复）**、无活动触点时的 end/字段事件顺序诊断、空 `SYN_REPORT` 空帧、`SYN_DROPPED` 进入 DroppedAwaitingBoundary 且忽略增量事件、**tracking 语义（`< -1` 诊断并忽略、end→begin 留下新生命周期、begin→end 不留生命周期、重复 id no-op、多步替换只留最终生命周期、新旧生命周期字段不泄漏）**、**R2 Re-review 1 字段-生命周期关联回归（字段在替换前到达不完成新触点——`X(old) -> tid(new) -> Y(new)` 后新触点 held，随后以新 X 补齐而非旧 X；end 后到达的字段被诊断并忽略、不改变 `Ended` 触点、不泄漏进稍后 `Began` 触点；跨多次替换的交错字段各归其生命周期，最终触点只带自己 begin 后到达的字段；单 cycle 超过 16 步替换时诊断有界并附汇总）**、**touch major/minor 用 delta/length 转换（非零 min 轴区分 position/delta，pending 与 snapshot 两条路径）**、resync 成功（discontinuity 帧 + 回到 Normal + 快照按钮）、**resync 快照非法（越界 slot、重复 slot、活动触点缺 X/Y、非法 tracking id）→ Degraded 终态 + 无帧 + 后续 feed 一律拒绝**、resync 失败（查询失败）、未配置 resync source、快照查询恰好一次、未配置/重复 configure 错误、非 Type-B/缺 slot_count/`slot_count > MAX_SLOT_COUNT` 拒绝（上限值 256 接受、257 拒绝）、`RawEvent::from_trace_event` 转换、**replay finish 拒绝未解决的同步丢失（`UnresolvedSynchronizationLoss`）且 Normal 状态干净完成**。
- `touchpad-linux` 集成测试（12 个）：5 个 fixture 逐一 replay 到预期 `ContactFrame`（含 `missing_resolution` 未归一化与 `dropped_recovery` 的 discontinuity 帧）；**每个 fixture 的 replay 路径与直接 `feed`（模拟实时输入）路径产出逐帧相等**——直接证明 fixture replay 与实时输入共用同一 decoder；resync 失败时 replay 返回 Sink 致命错误且失败后无可信帧；**以 `SYN_DROPPED` 结尾的端到端 trace 回放失败（`UnresolvedSynchronizationLoss`，零帧）**；**以普通 `SYN_REPORT` 结尾的 trace 干净完成**；**slot_count=10⁹ 的 replay header 以 `InvalidDevice` 干净失败（零帧）**。
- 全 workspace：core 36 单元 + 8 集成；trace 74 单元 + 14 集成（fixtures 4、replay 4、roundtrip 6，fixture 断言已按 53/54 轴键更新）；linux 44 单元 + 12 集成；doc-tests 2（trace）。共 **190**。

## 13. 剩余范围（M5 以后，不得虚报）

- **M5** — CLI Vertical Slice：`touchpadctl devices`/`inspect`/`record [--grab]`/`replay`；recorder 位于 decoder 之前；受控 `SIGINT`/`SIGTERM` shutdown、flush 与 ungrab 顺序；README/第三方依赖说明。**(已实现，待外部复核，见 §16)**。
- 尚未实现：**M6 已实现 KDE Wayland 输出资格切片（portal + libei，见 §17），但其真实桌面 `--emit` 测量尚未由 reviewer 执行，backend 状态保持 `experimental/unqualified`**；X11、uinput 输出后端；pointer/scroll/tap/drag/gesture 算法；GUI、常驻服务、开机自启、系统设置修改；任何实机行为验证（M5/M6 均未触碰真实设备——没有任何测试成功打开或 grab 真实设备或发射真实桌面输入；绝大多数测试经 mock seam，仅 `sys::ffi` 的 Linux-only 测试使用真实但无副作用的 `sigaction`/`raise`/不存在路径文件系统检查，M6 另有无副作用的 `libei.so.1` dlopen probe 与 session bus 可达性 probe）。
- 已知限制：`SIGKILL`、内核崩溃、硬断电无法保证用户态 cleanup（见 §14.9 与 §16.6；内核会在进程退出关闭 fd 时自动释放 evdev grab，但有序的 `ungrab`/`close` 序列只在可运行用户态代码的路径上有保证）。

## 14. M4 — Linux Device Boundary and Fail-Open Grab（已批准；R1–R7/RR1–RR3 全部关闭）

### 14.1 范围与边界

M4 实现 `touchpad-linux` 的 Linux 设备边界：`/dev/input/event*` 枚举与候选判定、capability/axis/slot 查询、kernel `input_event` → `RawEvent` 的受检转换、`EVIOCGRAB` RAII guard、真实 `SYN_DROPPED` snapshot adapter（对接 M3 `ResyncSource`）、受控 shutdown 输入 runtime，以及**所有真实文件系统/syscall 行为背后的可 mock adapter seam**。M4 明确不实现：signal handler、CLI（M5）、输出后端、任何真实设备打开/grab 的测试。

M4 review R1–R7 修订要点（详见本节各小节）：

- **R1**：会话 fd 在 grab/读取前执行 `EVIOCSCLOCKID(CLOCK_MONOTONIC)`；失败可行动并关闭 fd；修正「默认即 monotonic」的错误说法。
- **R2**：`EVIOCGMTSLOTS` 按内核真实语义建模——成功返回 0、缓冲为 leading code + slot_count 个 i32；mock 与真实 FFI 语义一致（**RR2**：缓冲装不下 `num_slots` 时**截断**而非 `-EINVAL`，mock 不伪造错误；完整性依据同一 fd 的 `num_slots == ABS_MT_SLOT.max+1` 内核不变量）。
- **R3**：live Linux FFI 仅实现并验证 **x86_64**（24 字节 `struct input_event`、两个 8 字节 `timeval` 字段）；其他 Linux ABI（32 位 `timeval`/time64、sparc64 `usec`+padding 等）未实现，`target_os=linux` 非 x86_64 编译期报错（**RR3**，不再宣称所有 32/64 位 Linux 正确）；非 Linux 的离线 replay/mock 保持可移植。
- **R4**：runtime 一次 open，capabilities/axes/slot 在同一 fd 上验证；clock/descriptor/decoder/snapshot 准备完毕后才可选 grab，grab 最后发生（**M5 review R2 修订**：grab 移出 open，成为 runtime 显式方法，在 recorder 准备完成、header flush 成功后、首次 read 前调用，见 §16.3）。
- **R5**：ungrab 即使失败也只发一次 `EVIOCGRAB(0)`，随后必然 close（fail-open）；shutdown 报告保留首个 ungrab 错误。
- **R6**：恢复成功后 drain 当前 read 批中剩余事件（它们先于快照观察），绝不回放被快照覆盖的旧事件；有多个 tracking lifecycle 的回归测试。
- **R7**：所有实际消费的 ioctl 响应都校验完整性（尤其 `EVIOCGKEY` 至少覆盖 `BTN_LEFT..BTN_MIDDLE`）；短响应 fail-closed、无帧；probe 也不把截断的必需 capability 响应静默当成完整。

`unsafe` 收敛策略：crate 级 `#![forbid(unsafe_code)]` 已移除，改为**逐模块** `#![forbid(unsafe_code)]`；唯一例外是 Linux-only 的 `sys::ffi`（见 §14.6 的 unsafe 清单与安全不变量）。

### 14.2 `sys` 模块：可 mock 的 OS seam

| 子模块 | 关键类型 | 职责 |
| --- | --- | --- |
| `sys`（mod） | `Sys` trait、`Fd`、`SysError`、`InputId`、`AbsInfo`、`CLOCK_MONOTONIC` | 可 mock seam：`read_dir`/`open`/`close`/`read` + `ioctl_grab`/`ioctl_set_clock_id`（`EVIOCSCLOCKID`，R1）/`ioctl_name`/`ioctl_id`/`ioctl_ev_bits`/`ioctl_prop_bits`/`ioctl_key_state`（**返回实际复制字节数**——内核 `evdev_handle_get_val`→`bits_to_user` 返回字节数而非 0，**RR1**）/`ioctl_absinfo`/`ioctl_mt_slots`（**成功即 `Ok(())`**——内核 `evdev_handle_mt_request` 返回 0，R2/RR2）；`Fd` 为不透明整数 token（`Copy`），真实路径上索引 `LinuxSys` 内部的 fd registry |
| `sys::requests` | `eviocgname`/`eviocgid`/`eviocgbit`/`eviocgprop`/`eviocgkey`/`eviocgabs`/`eviocgmt_slots`/`eviocgrab`/`eviocsc_lockid` 编码器 | 纯整数 `_IOC(dir,type,nr,size)` 编码（`'E'=0x45`），单元测试对照内核规范值（`EVIOCGRAB==0x40044590`、`EVIOCGID==0x80084502`、`EVIOCGABS(ABS_X)==0x80184540`、`EVIOCSCLOCKID==0x400445a0` 等） |
| `sys::ffi` | `LinuxSys`（`cfg(target_os="linux")`） | 唯一含 `unsafe` 的模块：`RefCell<Vec<Option<OwnedFd>>>` registry 持有 fd；`close` 通过 `OwnedFd` 恰好关闭一次、天然幂等；`read`/`ioctl` 直连 libc |
| `sys::mock` | `MockSys`、`MockDevice`、`MockFailure`、`ReadChunk`、`MockCall` | 可编程测试替身：目录列表、设备节点（名称/id/capability 位图/absinfo/MT slot 值/按键态/grab 态/脚本化 read 流）、失败注入、完整 syscall 日志（`MockCall`） |

- **共享方式**：`Sys` 方法全部 `&self`（真实实现无状态，只有 registry；mock 用内部可变性），运行时/grab guard/snapshot adapter 通过 `Rc<dyn Sys>` 共享同一个实例，无生命周期耦合。
- **错误分类**（`SysError`，均可行动、不 panic）：`Io(io::Error)`、`NotFound{path}`（ENOENT，含无 `/dev/input`）、`PermissionDenied{path,source}`（EACCES/EPERM）、`Interrupted`（EINTR——M4 显式上报而非静默重试，M5 的 signal 处理将其映射为优雅退出）、`Closed(Fd)`（对已关闭句柄的操作）、`InvalidArgument(String)`、`TruncatedResponse{operation,returned,required}`（**R7**：必需 ioctl 响应被截断，调用方 fail-closed，绝不把截断当完整）。
- **EINTR 策略**：`LinuxSys::read`/`ioctl` 遇到 EINTR 返回 `SysError::Interrupted`，不自动重试；runtime 把该错误按致命错误 fail-open 处理（停止、释放 grab、关闭 fd、返回可行动错误）。这是 M5 信号处理的接缝。

### 14.3 设备枚举与候选判定（`device`）

- `enumerate(sys) -> Result<Vec<ProbeReport>, ProbeError>`：读取 `/dev/input`，过滤 `event<digits>` 节点（`is_event_node`），按路径排序后逐一 `probe`。只有目录本身不可读才是错误（`ProbeError::ReadDir`）；单个节点任何失败都落在 `ProbeReport.verdict` 里。
- `probe(sys, path) -> ProbeReport`（绝不 fail，每个结果都可解释）：
  - 打开节点后调用**共享的 opened-fd probe**（`probe_open_fd`，**R4 修订**）：对**已打开**的 fd 读 `EVIOCGNAME`/`EVIOCGID`/`EVIOCGBIT(0|EV_KEY|EV_ABS)`/`EVIOCGPROP`，并对 `absbit` 中每个 axis 读 `EVIOCGABS`；runtime 会话对**同一个会话 fd** 复用同一逻辑，规则不漂移；探测句柄在返回前总是关闭（`MockCall` 日志证明）。**R7 修订**：每个必需 capability 响应都校验完整性（`EVIOCGBIT`/`EVIOCGPROP` 的返回长度必须覆盖完整内核位数组，短响应 → `Inaccessible`/`TruncatedResponse`），绝不把截断当成完整。
  - `DeviceCapabilities` 提供 `has_ev/has_key/has_abs/has_prop/is_type_b/is_pointer_like/is_direct/has_physical_buttons` 查询。
  - `ProbeVerdict`：
    - `Candidate{descriptor}`：Type-B 四轴齐备（`ABS_MT_SLOT`/`ABS_MT_TRACKING_ID`/`ABS_MT_POSITION_X/Y`）、`INPUT_PROP_POINTER` 或 `INPUT_PROP_BUTTONPAD` 之一、无 `INPUT_PROP_DIRECT`、slot 数 ∈ `[1, MAX_SLOT_COUNT]`；descriptor 轴键按 Linux 约定为 ABS code（`axis_id_for_code`），分辨率 `input_absinfo.resolution <= 0` 视为缺失（`AxisInfo.resolution = None`），`has_physical_buttons` 由 `BTN_LEFT/RIGHT/MIDDLE` 决定。
    - `Rejected{reasons}`：每条 reason 对应一个失败检查（非 Type-B 缺哪些轴、DIRECT、无 POINTER/BUTTONPAD、slot 数越界），可解释拒绝。
    - `Inaccessible{error}`：打开或任一必需 ioctl 失败（权限、设备消失等），error 可行动。
  - `evidence: Vec<String>`：正面观测（报 EV_KEY/EV_ABS、Type-B 确认、POINTER/BUTTONPAD/DIRECT、slot 数、物理按键列表），与 verdict 一起构成完整解释。
- `pick_candidate(reports) -> Option<usize>`：确定性取第一个候选（列表已按路径排序）。
- 判定规则刻意保守且完整记录在模块文档；`EV_KEY`/`EV_ABS` 的使用：EV_ABS 是绝对轴前提、EV_KEY 用于物理按键检测；按钮缺失不 disqualify（buttonpad 可能经触点送点击）。

### 14.4 kernel `input_event` → `RawEvent` 受检转换（`event`）

- `decode_input_events(buf)`：把一次 `read` 的字节安全解码为 `Vec<KernelEvent>`（原生端序 `from_ne_bytes`，无 `unsafe`）；长度必须是 `INPUT_EVENT_SIZE` 的整数倍，撕裂缓冲返回 `EventDecodeError::BadLength`（内核从不产生撕裂事件，mock 可注入）。**R3/RR3 修订**：live Linux FFI 只实现并验证 **x86_64** 布局——`INPUT_EVENT_SIZE` 恒为 24（两个 8 字节 `long` time 字段），`KernelEvent::from_bytes` 固定按该布局读 time 字段；`target_os=linux` 且非 x86_64 时 `compile_error!`（其余 Linux ABI——32 位 `timeval`/time64 的无符号 `__kernel_ulong_t`、sparc64 的 32 位 `usec`+padding 等——未实现，size 相等也捕捉不到字段解释错误，故不虚报）；并有 `size_of::<libc::input_event>() == 24` 的 Linux ABI 编译期断言与运行时测试；`encode_input_event` 提供同一 x86_64 布局编码供 mock/测试使用。非 Linux 目标（离线 replay、mock 测试）不受影响、保持可移植。
- `KernelEvent::to_raw_event()` / `to_monotonic()`（**单调时间域，M4 需求 2；R1 修订**）：evdev 客户端时钟零初始化到 `INPUT_CLK_REAL`（`CLOCK_REALTIME`，0），**并非构造上即 monotonic**；runtime 在会话 fd 上先执行 `EVIOCSCLOCKID(CLOCK_MONOTONIC)` 成功后才读事件，此时 `timeval` 按**内核单调时间域**处理，绝不当作 wall clock：
  - `tv_sec < 0` → `TimevalError::NegativeSeconds`（单调时钟永不为负）；
  - `tv_usec < 0` → `NegativeMicroseconds`；`tv_usec >= 1_000_000` → `MicrosecondsOutOfRange`（绝不进位到下一秒）；
  - `sec * 1e9 + usec * 1e3` 溢出 u64 纳秒 → `NanosecondOverflow`（绝不截断/回绕）；
  - 合法值 → `Monotonic::from_nanos`。与 `TraceTime::to_monotonic` 的 checked 数学逐对一致（有测试证明 live/trace 同一 `(sec,usec)` 转换相同）。
- runtime 另做**单调性检查**：时间戳倒退即 `RuntimeError::TimestampRegression`（fail-closed，同 trace reader 的 fail-stop 语义）。

### 14.5 Grab guard 与受控 shutdown（`grab`、`runtime`）

- `DeviceHandle`（RAII grab guard，`grab` 模块）：持有 `Rc<dyn Sys>` + fd + `grabbed`/`release_attempted`/`closed` 状态。
  - **显式 opt-in**：`open` 不 grab；只有 `DeviceHandle::grab()` 才发 `EVIOCGRAB(1)`。runtime 层自 **M5 review R2 起不再有 `OpenOptions.grab`**（`OpenOptions` 已删除）：grab 是 `EvdevRuntime::grab()` 显式方法，由 record 命令在 recorder 准备完成、header flush 成功之后、首次 read 之前调用（见 §16.3）。
  - `grab`/`ungrab`/`close` 全部**幂等**（重复调用是 no-op）；`close` 先 ungrab 再关 fd；`fd()` 在关闭后返回 `None`（无 panic 路径）。
  - **release 至多尝试一次（R5 修订）**：`release_attempted` 与 `grabbed` 分开跟踪——`EVIOCGRAB(0)` 无论成败都只发一次；失败时 `grabbed` 保持 true（内核仍持有 grab），但 `close`/`Drop` 不再重试该 ioctl，而是直接关闭 fd（内核 fd teardown 释放 grab，fail-open）。成功释放后再 `grab()` 会重新武装 release（`release_attempted = false`）。
  - **`Drop` 仅 best-effort 兜底**：未显式关闭时在 Drop 中 ungrab + close，忽略错误；显式路径必须用 fallible 方法以便上报。
  - **fail-open**：关闭 fd 时内核自动释放 grab，因此即使 `EVIOCGRAB(0)` 失败，fd 关闭后系统仍恢复对设备的控制。
- `EvdevRuntime<S: FrameSink>`（`runtime` 模块）：
  - `open(sys, path, sink)`（**R4 修订，一次 open；grab 移出 open 见 M5 review R2 / §16.3**）：`DeviceHandle::open`（会话唯一一次 open）→ 对**同一 fd** 执行共享 `probe_open_fd` 完成 capabilities/axes/slot 验证 → `decide_verdict` 得到 descriptor（非候选则 close 并报 `NotCandidate`）→ `EVIOCSCLOCKID(CLOCK_MONOTONIC)`（R1；失败 close 并报 `Clock`）→ `decoder.configure(descriptor)` → 安装真实 snapshot adapter（同一 fd）→ 进入 `Running`。**`open` 绝不 grab**。失败分类 `OpenError::{NotCandidate, Probe, Access, Configure, SnapshotSource, Clock}`；**任何准备步骤失败都关闭 fd，且 grab 绝不在 open 失败路径上发生**。
  - `step() -> Result<usize, RuntimeError>`：一次阻塞读（64 事件缓冲）→ 长度校验 → 逐事件受检转换 → 单调性检查 → `decoder.feed`；返回喂入的事件数。帧在 `SYN_REPORT` 处经 sink 发布。
  - **resync drain 边界（R6 修订）**：若一次 `read` 批内含 `SYN_DROPPED`、恢复 `SYN_REPORT` 与**之后**的事件，快照 ioctl 观察到的是已包含这些后续事件的内核状态；回放它们会把「先于快照」的增量（tracking-id 生命周期、按键变化）叠加到更新的快照上，产生伪转换。`step()` 在 `decoder.just_resynced()`（快照安装成功的那次 feed）之后**立即停止喂入当前批**，剩余事件视为 dropped 窗口的一部分被丢弃；下一次 read 从与快照一致的状态开始。这是文档化的 fail-closed 同步边界：evdev 队列/快照顺序限制（队列中可能存在先于快照 ioctl 的事件）无法在用户态消除，丢弃是「绝不产生陈旧 lifecycle/frame」的唯一保证。
  - **fail-open 清理**：任何致命错误（EOF/拔出、撕裂读、EINTR、时间戳倒退、非法 timeval、decoder 失败含 resync 失败）都先释放 grab（至多一次，best-effort）、关闭 fd、置 `Stopped`，再返回可行动 `RuntimeError`；degraded decoder 不再产帧（M3 语义）。
  - **受控 shutdown 生命周期**（`shutdown() -> ShutdownReport`，顺序明确）：
    1. **停止工作**：phase → `Stopping`，此后 `step()` 返回 `NotRunning`；
    2. **结束输出/flush**：M4 的 `FrameSink` 无 flush 契约（recorder flush 属 M5），该步是**显式文档化的抽象边界、M4 为 no-op**——不发明 M5 功能；
    3. **幂等 ungrab**：`EVIOCGRAB(0)` 至多一次（**即使失败也不重试，R5**）；report 保留首个 ungrab 错误；
    4. **关闭 fd**：在 ungrab 之后，至多一次；report 同时上报 close 状态。
    重复 `shutdown` 是安全 no-op（report 中 `ungrab`/`close` 为 `None`）。
  - `RuntimePhase::{Running, Stopping, Stopped}`；无 signal handler、无 CLI（M5 范围）。
- `RuntimeError` 分类：`Open`、`NotOpen`、`NotRunning`、`Read(SysError)`、`DeviceGone`（EOF=拔出）、`PartialRead{actual,event_size}`、`TimestampRegression`、`Event(EventDecodeError)`、`Timeval(TimevalError)`、`Decode(DecodeError)`（含 `ResyncFailed`）、`Grab(GrabError)`、`Interrupted`（M5 信号停止）、`GrabAfterStep`（M5 review R2：step 后禁止 grab）、`Recorder(RecorderError)`（M5）。

### 14.6 unsafe 清单与安全不变量（全部在 `sys::ffi`，Linux-only）

| 位置 | 操作 | 安全不变量 |
| --- | --- | --- |
| `LinuxSys::open` | `libc::open` | `CString::new` 已拒绝内部 NUL，指针指向合法 NUL 结尾路径；flags 为合法 `O_RDONLY|O_CLOEXEC` |
| `LinuxSys::open` | `OwnedFd::from_raw_fd(raw)` | `open(2)` 刚返回 `>= 0` 且进程从未别名/复制该 fd，唯一所有权转移合法 |
| `LinuxSys::read` | `libc::read(raw, buf, len)` | `raw` 来自 registry 中合法打开的 `OwnedFd`；`buf` 为长度 `len` 的可写切片，内核写至多 `len` 字节 |
| `LinuxSys::ioctl_grab` | `libc::ioctl(raw, EVIOCGRAB, arg)` | grab 时 `arg` 指向合法 `c_int`（内核只判指针非空，不解引用）；release 时为 NULL |
| `LinuxSys::ioctl_set_clock_id` | `libc::ioctl(raw, EVIOCSCLOCKID, arg)` | `arg` 指向合法 `c_uint`（4 字节，与 `_IOW('E',0xa0,__u32)` 编码一致）；内核只读一个 `__u32`（`get_user`），不写 |
| `ioctl_name`/`ioctl_ev_bits`/`ioctl_prop_bits`/`ioctl_key_state` | `libc::ioctl` | 请求编码的 size 与 `buf.len()` 一致，`buf` 为可写切片；`EVIOCGKEY` 成功时内核恰好拷贝 `BITS_TO_BYTES(KEY_MAX)` 字节并**返回该字节数**（`evdev_handle_get_val`→`bits_to_user` 返回复制字节数，不是 0——只有 `EVIOCGMTSLOTS` 返回 0，**RR1**；R7 由调用方校验返回长度） |
| `ioctl_id` | `libc::ioctl` | `arg` 指向 `libc::input_id`（8 字节，与请求编码一致），内核填满四字段 |
| `ioctl_absinfo` | `libc::ioctl` | `arg` 指向 `libc::input_absinfo`（24 字节，与 `EVIOCGABS` 编码一致），内核填满六字段 |
| `ioctl_mt_slots` | `libc::ioctl` | 请求编码 `len == buf.len()*4`；`buf[0]` 进入前是 ABS_MT code；内核在 code 后每 slot 写一个值并**返回 0**（`evdev_handle_mt_request`，R2）——snapshot adapter 传 `slot_count+1` 个 i32（leading code + slot_count 个值），slot 数来自同一 fd 的 `ABS_MT_SLOT` 读并受 `MAX_SLOT_COUNT` 约束，成功即全部写入，无需也不可用返回值验证 slot 数 |
| `ioctl_call`（共享 helper） | `libc::ioctl` | 调用方保证 `arg` 对请求方向/编码 size 有效；`raw` 为合法打开 fd；EINTR 映射为 `SysError::Interrupted` |

模块级：除 `sys::ffi` 外，所有模块 `#![forbid(unsafe_code)]`（rustc 强制，clippy 一并验证）。

### 14.7 真实 `SYN_DROPPED` snapshot adapter（`snapshot`）

- `EvdevSnapshotSource` 实现 M3 `ResyncSource`：`snapshot()` 经 `EVIOCGMTSLOTS` 逐轴读取全部 slot 当前值（`ABS_MT_TRACKING_ID` + 设备上报的 X/Y/PRESSURE/TOUCH_MAJOR/MINOR/ORIENTATION），经 `EVIOCGKEY` 读按键态，构建**完整** `KernelStateSnapshot`（列出每个 slot，活动/空均有）。
- **slot 数受 M3 上限约束（R2 修订）**：构造时校验 `[1, MAX_SLOT_COUNT]`，越界直接 `SnapshotError::SlotCountOutOfRange`；slot 数来自**同一会话 fd** 的 `ABS_MT_SLOT.max + 1`（R4），不依赖 ioctl 返回值。
- **`EVIOCGMTSLOTS` 缓冲协议（R2/RR2 修订）**：`buf[0]=code`、`buf[1..]=各 slot 值`，adapter 传 **`slot_count+1` 个 i32**；内核 `evdev_handle_mt_request` 计算 `max_slots = (size-4)/4`，循环写 **`min(num_slots, max_slots)`** 个值并**返回 0**——缓冲装不下时**截断而非 `-EINVAL`**（RR2：mock 同样截断，不伪造错误）。生产完整性的依据是同一 fd 的内核不变量 `num_slots == ABS_MT_SLOT.max+1 == slot_count`（`input_mt_init_slots` 由 slot 数设定轴最大值），因此 `slot_count+1` 个 i32 的缓冲恰好被写满；**不再用返回字节数或虚构错误验证 slot 数**（内核不提供该信息）。
- **fail-closed（R2/R7 修订）**：任一 ioctl 失败（`MtSlots`/`KeyState`）、`EVIOCGKEY` 返回长度不足以覆盖 `BTN_LEFT..BTN_MIDDLE`（`KeyStateTruncated`，短响应会被零填充误读为「无按键」，对持键 resync 不安全）、tracking id `< -1`（`InvalidTrackingId`）都使 `snapshot()` 返回错误 → decoder 进入 `Degraded`，**不发布任何帧**（M3 R4 语义保持）；M3 的 `apply_snapshot` 仍做二次完整校验（重复 slot、活动触点缺 X/Y 等）后才原子换入。`SlotMismatch` 错误类型已随返回字节数语义删除。

### 14.8 M4 测试与 mock 覆盖矩阵（全部无硬件、完全依赖 mock）

M4（R1–R7/RR1–RR3 修订后）：`touchpad-linux` 单元测试 135 个（`sys` 4、`sys::requests` 3、`sys::mock` 7、`sys::ffi` Linux-only 4、`event` 10、`device` 18、`grab` 14、`snapshot` 9、`runtime` 21、`decode` 45——含 M3 单元测试）与集成测试 17 个（`tests/decoder.rs` 12 个 M3 fixture replay/端到端 + `tests/m4_device.rs` 5 个：enumerate→pick→open→step→shutdown 端到端、无设备空结果、无 `/dev/input` 可行动错误、打开被拒设备给出 reasons、**R6 端到端 drain 回归**）。workspace 共 **286** 个测试（core 36 单元 + 8 集成；trace 74 单元 + 14 集成；linux 135 单元 + 17 集成；doc-tests 2）。

| 真实行为 | mock 场景 | 覆盖测试 |
| --- | --- | --- |
| `read_dir(/dev/input)` | 条目列表 / `NotFound` | `enumerate` 过滤排序、`enumerate_missing_input_dir` |
| `open` 设备节点 | 成功 / `NotFound` / `PermissionDenied` / 注入 `open_error` | probe Inaccessible、runtime `Access` 分类、`open_permission_denied_is_actionable` |
| `close` fd | 日志记录 + 幂等 | `probe_closes_its_handle`、grab/close 幂等、shutdown 重复调用 |
| `read` 事件流 | 整批事件 / EOF（空队列）/ 撕裂字节 / `EINTR` | `step_decodes_events_into_frames`、`eof_releases_grab_closes_and_stops`、`partial_read_is_fatal`、`einterrupt_is_actionable` |
| `EVIOCSCLOCKID` | 成功（记录 clock id）/ 失败（`clock_id_error`） | `open_sets_monotonic_clock_before_grab_and_read`（R1 顺序）、`clock_failure_closes_fd_and_never_grabs_or_reads`（R1 清理）、`clock_id_is_recorded_in_the_log` |
| `EVIOCGNAME`/`EVIOCGID`/`EVIOCGBIT`/`EVIOCGPROP` | 设备位图/名称/id / **截断位图**（R7） | probe 判定、`candidate_touchpad_is_accepted...`、`physical_buttons...`、`truncated_{ev,key,prop}_bits_response_is_inaccessible` |
| `EVIOCGABS` | absinfo（含 SLOT max→slot 数） | `oversized_slot_count_is_rejected`、descriptor 轴 |
| `EVIOCGMTSLOTS` | 每轴每 slot 值 / 失败 / num_slots 超出缓冲（**截断而非 -EINVAL**，RR2） | snapshot 全量读取、`mt_slots_ioctl_failure`、`mt_slots_success_at_the_ffi_boundary_is_zero`（R2 回归：旧 byte-count seam 下必失败）、`mt_slots_truncates_values_that_do_not_fit_like_the_kernel`（RR2：截断 + 成功，完整性靠同一 fd 不变量）、runtime resync 成功/失败 |
| `EVIOCGKEY` | 按键态（BTN_LEFT...）/ **短响应**（R7）/ **byte-count 语义**（RR1） | `snapshot_reads_all_slots_axes_and_buttons`、`snapshot_without_buttons_skips_key_state`、`short_key_state_response_fails_the_snapshot`、`short_key_state_during_resync_fails_closed_with_no_frame`（runtime 级：resync fail-closed、无帧）、`key_state_returns_copied_bytes_while_mt_slots_returns_unit`（RR1 contract：KEY 返回字节数 vs MTSLOTS 返回 `Ok(())`） |
| `EVIOCGRAB` | grab 状态 + 日志 + `grab_error`/`release_error` 注入 | `grab_is_explicit_opt_in`、`ungrab_is_idempotent`、`failed_ungrab_is_attempted_at_most_once`（R5）、`close_with_failed_ungrab_releases_once_then_closes`（R5）、`drop_with_failed_ungrab_releases_once_and_closes`（R5）、`regrab_after_release_rearms_the_release`、`shutdown_with_failed_ungrab_reports_error_and_releases_once`（R5）、`fatal_resync_cleanup_with_failed_ungrab_releases_once_and_closes`（R5）、runtime 各 fatal 路径的 ungrab 断言 |
| R4 会话 | 一次 open / 同 fd 查询 / grab 最后（M5 R2 后为 open 后显式 `runtime.grab()`） | `open_validates_and_runs_the_same_fd`、`grab_is_issued_after_preparation_and_failure_cleans_up`（R2 修订）、`grab_is_checked_idempotent_and_rejected_after_step_or_shutdown`（R2） |
| R6 drain | 一批内 SYN_DROPPED + 恢复边界 + 多个 tracking lifecycle | `resync_drains_the_rest_of_the_read_batch`（runtime 单元）、`resync_drains_post_boundary_lifecycles_end_to_end`（m4_device 集成）、`just_resynced_flags_the_feed_that_installed_the_snapshot`（decoder） |
| resync 失败 | `mt_slots_error` 注入 | `resync_failure_degrades_releases_grab_and_stops_frames`、M3 decoder 单元测试 |

### 14.9 文档化限制（M4 需求 9，不虚报）

- **无法保证的清理**：`SIGKILL`、内核崩溃、硬断电无法运行用户态 cleanup；内核会在进程退出/关闭 fd 时自动释放 evdev grab，但有序的 `ungrab`→`close` 序列只在可执行用户态代码的路径上有保证。lib.rs、runtime 模块与本文均明确说明。
- **未做真实硬件验证**：M4 未打开或 grab 真实设备；未验证真实触控板识别、`evtest`/内核信息一致性、真实 grab 独占、真实 `SYN_DROPPED` ioctl resync、真实拔出/EINTR 行为——这些全部列入 M5 之后的实机测试清单。
- **EINTR 语义**：M4 把 EINTR 作为可行动错误上报并 fail-open；M5 的 signal handler 将把它映射为优雅 shutdown。
- **timeval 域（R1 修订）**：evdev 默认 `INPUT_CLK_REAL`；runtime 在会话 fd 上 `EVIOCSCLOCKID(CLOCK_MONOTONIC)` 成功后才按**内核单调时间域**处理 timeval，绝不当 wall clock（§14.4）。
- **resync drain 边界（R6 修订）**：evdev 队列/快照顺序限制（同一 read 批中快照之后的旧事件）无法在用户态消除；runtime 在恢复成功后丢弃当前批剩余事件（见 §14.5），保证绝不回放先于快照的事件。
- **live Linux 目标集（RR3 修订）**：live Linux FFI（`sys::ffi` + `event` 解码）仅实现并验证 **x86_64 Linux**；`target_os=linux` 的非 x86_64 架构在编译期 `compile_error!`（`event.rs`），不虚报 32/64 位全支持。非 Linux 的离线 replay 与全部 mock 测试保持可移植、不受影响。

### 14.10 本机 KDE/libinput 触控体验基线（仅记录，供未来校准/A-B，不实现）

M4 review 现场只读观察（未打开或 grab 真实设备）记录本机当前 KDE/libinput 配置，作为后续 pointer/scroll/tap 里程碑的**校准/A-B 基线**：

- 设备：`CIRQ1080:00 0488:1054 Touchpad`（KDE 配置组亦记录十进制 vendor/product `1160/4180`）。
- `pointerAcceleration=0.8`、`naturalScroll=true`、`scrollTwoFinger=true`、`scrollEdge=false`、`TapDragLock=true`。

明确边界（**M4 不实现任何桌面配置读取**）：以上数值**不得**作为本项目算法的实时依赖或被当作与项目单位等价的数值参数直接复制；主设计要求 grab 后由本 runtime 拥有策略。这些参数只用于设计可量化的行为画像与对比 trace（基线仅文档化，见 review「Local good touch experience reference」）。

## 15. M5 及以后的闸门

- M5 — CLI Vertical Slice and Phase 1 Handoff（`touchpadctl`；`--grab` 默认关闭并带风险警告；recorder 位于 decoder 之前；受控 `SIGINT`/`SIGTERM` shutdown 顺序；README/第三方许可说明）。**(已批准，见 M5_REVIEW.md 终审)**。
- M6 — KDE Wayland Output Backend Qualification（portal + libei 输出 adapter；`output-probe` 默认非发射 dry-run、`--emit` 显式有界发射；fake transport/session 全覆盖测试；人工 A/B 验收程序）。**(已实现，待外部复核，见 §17)**

## 16. M5 — CLI Vertical Slice and Phase 1 Handoff（已实现，待外部复核）

状态：**已实现**。M5 尚未经外部 review，本节所有内容均为「实现事实 + 待复核」，**不写 approved**；外部 reviewer 按 `docs/M5_ACCEPTANCE.md` 与 MILESTONES.md 验收后才进入下一阶段。

### 16.1 范围与边界

M5 实现 `apps/touchpadctl`（CLI）与 `touchpad-linux` 的两个 M5 附加件：**raw-event recorder**（decoder 之前）与 **SIGINT/SIGTERM 信号接缝**。四个命令：

```text
touchpadctl devices
touchpadctl inspect DEVICE
touchpadctl record DEVICE OUTPUT [--grab]
touchpadctl replay INPUT
```

M5 明确不实现：Wayland/X11/uinput/任何真实输出后端；pointer/scroll/tap/drag/gesture 算法；GUI、常驻服务、开机自启、系统设置修改；读取 KDE 配置作为运行时依赖；任何真实 `/dev/input` 打开/grab 的测试或实机验证（没有任何测试成功打开或 grab 真实设备；绝大多数测试经 `sys::mock`，仅 `sys::ffi` 的 Linux-only 测试使用真实但无副作用的 `sigaction`/`raise`/不存在路径文件系统检查；本会话环境探测显示无 `/dev/input`）。**所有新模块 `#![forbid(unsafe_code)]`**；`unsafe` 仍只存在于既有最小 Linux FFI 边界 `sys::ffi`（M5 在此边界内新增 `sigaction` 信号处理）。

### 16.2 `touchpadctl` 结构（库层 command runner + 薄 binary）

| 模块 | 职责 |
| --- | --- |
| `args` | 手写参数解析（无 CLI 框架）与帮助文本；`--grab` 默认 false、**record-only（R5：`devices`/`inspect`/`replay` 传 `--grab` 为 usage exit 1，重复 `--grab` 也拒绝）**，帮助文本显式警告独占风险（含 `EVIOCGRAB(1)`、独占、SIGKILL/断电不保证 cleanup、默认 OFF） |
| `env` | `CommandEnv`：`sys`（可 mock 的 `Sys` seam）+ `out`/`err` 写入器 + `stop_flag`（**可注入停止源**：真实信号 handler 不再写调用方 flag，而是写进程生命周期静态（M5 re-review R1，经 `touchpad_linux::termination_requested` 读取）；`stop_flag` 供测试确定性模拟信号）+ `recorder_factory`（可选 recorder 工厂，R2/R3 fault-injection 与共享时间线测试用）；全部命令依赖它 ⇒ 库层 command runner 可进程内测试 |
| `exit` | `ExitCode`（0–9，稳定契约，见 README）与 `CommandFailure`（结构化失败，携带 exit code；R3 新增 `RecorderFinalize`（→7）与 `DeviceRelease`（→6），保证 cleanup 失败结构可见且不冒充 exit 8） |
| `output` | `FramePrinterSink`（replay：每帧一行 JSON `ContactFrame`）、`CountingSink`（record 状态统计） |
| `cmd::devices` | 枚举 + 逐节点 verdict/evidence 输出；无候选/目录缺失/权限不足均为可行动结果 + 合理退出码，不 panic |
| `cmd::inspect` | 先 open 分类访问性（2/3），再 `probe` 全量报告；非候选打印全部细节并以 4 退出（含 reasons） |
| `cmd::record` | 打开 runtime（**不 grab**，R2）→ 用 runtime 自身 descriptor 建 `TraceHeader` → `TraceRecorder::create` → **flush header 证明输出可写** → `set_recorder` → **显式 `runtime.grab()`（受检/幂等/step 后拒绝）** → 读循环（stop flag + `termination_requested()` 轮询 + `RuntimeError::Interrupted`）→ 统一有序 finalization（**由 runtime 执行**：recorder finish + 销毁 → ungrab → close，R3 re-review）→ 结构化状态 + 复合退出码 |
| `cmd::replay` | `File::open` + `ReplayDriver` + 同一个 `TypeBDecoder`（无第二套状态机）；stdout 为 JSON Lines 帧，stderr 为 summary；纯离线 |
| `main` | 薄 shell：真实 `LinuxSys`（非 Linux 用空 mock，replay 仍可用）、安装信号 handler（**无 flag 参数**：handler 只写进程生命周期静态，guard 保活，M5 re-review R1）、`run_command` → exit code |

退出码契约（README 与帮助文本一致）：`0` 成功、`1` 用法、`2` 无 `/dev/input`/节点缺失、`3` 权限、`4` 无候选/非候选、`5` trace 错误、`6` 设备流/decoder 错误或清理时 ungrab/close 失败（R3）、`7` recorder 错误（输出无法写入或 finalization 失败，R3）、`8` 受控信号停止（**仅当 recorder finalization 与设备释放都成功**，否则返回 6/7 并保留全部诊断，R3）、`9` 内部错误。

### 16.3 Recorder 位于 decoder 之前（流水线顺序）与准备顺序（M5 review R2）

- `touchpad-linux::recorder`（新模块，`forbid(unsafe_code)`）定义 `RawEventRecorder { record, flush, finish, events_recorded }` 与 `TraceRecorder`（包装 `touchpad_trace::TraceWriter`；`KernelEvent::to_trace_event` 用与 live 相同的受检 timeval 规则）。
- **BufWriter 诚实契约（M5 review 文档修正）**：`TraceRecorder::create` 把 header 写入 `BufWriter`——`create` 成功**不代表 header 已落盘**；显式 `flush`（或 `finish`、或 Drop 的 best-effort flush）成功才证明输出可写。record 命令在 `create` 后立即 `flush`，把 flush 失败当作「输出不可写」（exit 7，零 grab 调用）。
- **准备顺序（R2，共享时间线证明）**：`open`（不 grab）→ 用 runtime 自身 descriptor 建 header → `create` recorder → **flush header（证明输出可写）** → `set_recorder` → 显式可选 `runtime.grab()`（受检：仅 `Running`、设备打开、幂等、**step 后拒绝** `RuntimeError::GrabAfterStep`）→ 读循环。测试 `header_flush_precedes_grab_in_the_shared_timeline` 在同一时间线上证明 flush < `EVIOCGRAB(1)` < 首次 read；输出创建或 header flush 失败时 grab 调用数为 **0**。
- `EvdevRuntime::step` 对**每个**已解码的 kernel event **先** `recorder.record` **再** `decoder.feed`（`record_all` 暂取 recorder 避免自借用冲突）。**decoder 失败不会丢失已读 raw event**：测试证明 resync 失败时批内全部事件（含触发失败的 recovery `SYN_REPORT`）都已在 trace 中。
- recorder 失败对会话是致命的（fail-open），但已记录事件保留。
- resync drain（M4 R6）只影响 decoder **输出**；recorder 仍记录整批（trace 是内核投递的 ground truth），有测试断言 drained 事件也被记录。
- `ShutdownReport` 携带**整个有序 finalization 的结果**（M5 re-review R3）：`recorder_finish: Option<Result<(), RecorderError>>`（shutdown 步骤 2，位于 ungrab 之前）、`events_recorded: u64`（finalization 前捕获的已记录事件数，供状态如实上报）、`ungrab`、`close`；`fail_open` 也把同一组结果记录到 `EvdevRuntime::take_fail_open_report()`（R3：致命路径的 cleanup 失败不再被丢弃、不再误报为「n/a (already closed)」）。

### 16.4 受控 SIGINT/SIGTERM 停止（R1 修订，re-review 再修订）

- `touchpad-linux::signals`（新模块，`forbid(unsafe_code)`）提供 `install_termination_handler() -> TerminationHandlerGuard`（**无 flag 参数**）与 `termination_requested() -> bool`；Linux 实现位于 `sys::ffi`（既有 unsafe 边界）：`sigaction` 注册无 `SA_RESTART` 的 handler，handler 体仅为一次 relaxed atomic store。非 Linux 为 no-op（便携 replay/mock 不受影响），`termination_requested()` 恒为 `false`。
- **R1 re-review 内存安全（进程生命周期静态，无调用方内存）**：
  - **async handler 路径上没有任何调用方分配**：handler 的唯一副作用是对进程生命周期静态 `TERMINATION_REQUESTED`（`'static`，永不回收）的 store。它不 load、不 dereference 任何调用方指针，因此**任何** teardown 交错都不可能让 in-flight handler 触碰已释放内存——恢复 disposition 不会等待已启动的 handler，但这与内存安全无关：该 handler 剩余的工作只是对永不回收的静态存储的 store。旧设计的竞态（in-flight handler 在 guard teardown 释放最后一个 `Arc` 后 dereference 已释放的 flag）**构造性消除**。
  - **单一 active install 由代码强制（保留）**：进程级 `INSTALLED` 标记，第二次安装返回结构化错误 `SignalError::AlreadyInstalled`/`TerminationInstallError::AlreadyInstalled`（不叠加 handler）；首个 guard drop 后可再次安装（从干净的 stop 状态开始）。测试 `second_install_is_rejected_with_structured_error`、`fresh_install_succeeds_after_the_first_guard_is_dropped`（FFI 与 signals 两层）。
  - **恢复/清除顺序（现在是整洁性而非内存安全同步）**：guard Drop 先恢复 SIGINT/SIGTERM 先前 disposition（此后我们的 handler 不可能再运行），再复位静态 stop 状态（恢复前到达的信号已由 handler 记录；恢复后到达的信号由恢复的 disposition 处理），最后释放 install 标记。**并发边界如实记录**：安装/移除单线程（CLI 主线程）只为确定性的 disposition 恢复顺序——不是内存安全前提；即使 guard drop 与 in-flight handler 竞态，handler 的目标是永不回收的静态存储，不可能 UB。
  - **确定性模型测试（不触发 UB、非时序依赖）**：`in_flight_handler_resuming_after_teardown_touches_only_static_memory`（FFI 层）与 `in_flight_handler_resuming_after_teardown_is_safe_by_construction`（signals 层）确定性建模旧竞态——先 fire handler（其「目标加载」就是对静态的 store），再 drop guard（teardown），再 fire 一次（建模 teardown 后恢复执行的 in-flight 调用）——两次 store 都落在永不回收的 `'static` 存储上。
- 阻塞 read 被信号唤醒：`read(2)` 返回 `EINTR` → `SysError::Interrupted`（M4 seam）→ runtime 检查「stop 已请求」（= 附着的 stop flag 置位 **或** `termination_requested()` 静态置位）：
  - **stop 已请求** → `RuntimeError::Interrupted`（新变体），phase → `Stopping`，**不 fail-open**（设备保持打开），调用者随后执行有序 shutdown —— **不把 EINTR 一律误报为普通致命错误**；
  - **stop 未请求** → 保持 M4 语义：普通致命错误，fail-open。
- CLI 同时在每次 step 前轮询 `stop_flag` 与 `termination_requested()`（覆盖两次 step 之间到达的信号）。
- 测试证明链：FFI 测试用真实 `raise(SIGINT)` 证明 OS 交付 → handler → 静态 stop 状态（`real_sigint_records_the_stop_request`）与 guard drop 恢复 disposition 并复位静态；signals 测试直接调用 handler 证明置位；runtime 单元测试证明「EINTR + stop → graceful（设备保持打开）」与「EINTR 无 stop → fatal」；CLI 集成测试证明 exit 8 与有序清理。信号相关测试共用 `signals::SIGNAL_TEST_LOCK` 串行化（runtime 的 EINTR 测试也会读静态，必须与写静态的信号测试互斥）。

### 16.5 有序、幂等清理（所有退出路径；M5 review R3/R4 修订）

**统一 finalization 顺序（M5 re-review R3：整条序列由 runtime 在一个地方执行）**——信号、grab 失败、EOF/拔出、decoder/recorder 错误全部一致：

1. 停止接收新工作（phase 离开 `Running`）；
2. 结束语义输出生命周期 —— **显式 no-op**（本阶段无真实 backend；文档化抽象边界）；
3. **recorder 完整可失败 finalization —— 在设备释放之前**：调用 recorder `finish()`（含 flush），然后**销毁 recorder**（其 best-effort `Drop` flush 是 `finish` 失败时把缓冲字节推到底层 sink 的最后机会）——两者都在 ungrab/close 之前；**fallible `finish` 绝不在设备释放之后调用**（命令不再自己调用 `finish`；signal/grab 路径由 `EvdevRuntime::shutdown()` 执行，fatal 路径由 `fail_open()` 在 `step` 返回前执行）；
4. ungrab 至多一次（`EVIOCGRAB(0)`，失败也不重试，M4 R5 保持）；
5. 即使 ungrab 失败仍 close（fail-open：内核在 close 时释放 grab）；
6. 打印结构化状态并返回**复合退出码**。

**一份结构化报告（R3 re-review，不丢诊断、不冒充成功）**：`ShutdownReport` 携带 recorder `finish` 结果、`events_recorded`、ungrab、close。命令从**恰好一个来源**取实际结果——fatal 路径用 `take_fail_open_report()`（此时 `shutdown()` 是 no-op），signal/grab 路径用 `shutdown()` 的报告——同一份报告同时驱动 cleanup 状态行与 exit 决定，状态文本与实际结果不可能不一致（fatal 路径不再误印「n/a (already closed)」）。`final_failure` 的优先级为——recorder finalization 失败（`CommandFailure::RecorderFinalize` → **7**）> 设备释放失败 ungrab/close（`CommandFailure::DeviceRelease` → **6**）> 主因（信号 → **8**，仅当 recorder finalization 与设备释放都成功；流错误 → 原退出码）。消息同时保留主因与全部 cleanup 诊断。

**R4 有序 fallback Drop（R3 扩展）**：`EvdevRuntime` 实现有序 best-effort `Drop`——未显式 shutdown 的销毁路径（recorder attach 后早期 `?` 返回，如状态输出写入失败；以及意外 unwind）先完成 recorder 的 fallible finalization（`finish`，best-effort）**并销毁 recorder**（其 `Drop` 的 best-effort flush 也在设备释放前），再释放设备（`DeviceHandle` Drop 的 ungrab/close 各至多一次），recorder finalization 保证先于设备释放。显式 `shutdown()`/`fail_open` 仍是主路径（置 `Stopped` 后 Drop 无操作）。测试：runtime `drop_finalizes_recorder_before_releasing_the_device`（共享时间线 finish < drop < ungrab < close）、CLI `status_writer_failure_after_recorder_attachment_uses_ordered_fallback`（失败状态 writer + 共享时间线 + 每设备操作至多一次）。

`fail_open`（致命路径，保留 M4 立即 fail-open）按同一顺序 finish recorder（+ 销毁）→ ungrab → close，结果记录进 `take_fail_open_report()`。**不用 panic hook 代替主路径**；重复 shutdown 是安全 no-op（有测试断言无新增 syscall、不重复 finish）。`SIGKILL`、内核崩溃、硬断电明确不保证 cleanup（§14.9 保持）。

### 16.6 `touchpadctl replay` 输出契约（稳定、可测试）

- stdout：每个已提交 `ContactFrame` 一行 JSON（`serde_json` 序列化 core 类型），无其他内容；
- stderr：`replay summary: device=… schema_version=… events_forwarded=… frames=…`；
- 退出码：0 干净完成；5 trace 错误（文件缺失、损坏行、schema 不匹配、时间倒退、结尾同步丢失、非法 header）；6 decoder 失败（如离线 `SYN_DROPPED` 无法 resync —— 离线无内核快照源，失败于 drop 处并打印 drop 前的帧，诚实上报）。
- replay 纯离线：只打开 trace 文件，不访问 `/dev/input`，普通用户/CI 可运行。

### 16.7 M5 测试矩阵（全部无硬件；R1/R3 再修复后 workspace 共 368 个测试）

- `touchpad-linux` 单元测试 **160** 个（M4 的 135 + M5 新增 25）：`KernelEvent::to_trace_event`（2）、recorder（4：写入忠实、finish、非法 timeval 拒绝、create 失败）、signals（2：handler 记录 stop 请求、第二次安装拒绝且 drop 后可重装）、**R1 再修订** signals（1：**in-flight handler 在 teardown 后恢复只碰静态内存的确定性模型测试**）与 ffi 信号（2+3：**真实 raise(SIGINT)** 记录 stop 请求、guard Drop 恢复 disposition 并复位静态、**in-flight handler 恢复的确定性模型测试**、第二次安装结构化拒绝、drop 后重装成功且状态干净）、runtime（8：EINTR+stop graceful 且设备保持打开、**signal stop 的 finish→drop→ungrab→close 共享时间线顺序证明**（`TimelineSys`+`MarkerRecorder`）、recorder 记录整批含 drained 事件、**decoder 失败不丢已读事件**、recorder 失败 fatal 且释放 grab、shutdown finalize recorder 且幂等、descriptor 暴露、**fatal 路径 finish<drop<ungrab<close 共享时间线**（R3 re-review））+ **R2/R3/R4 新增**（3：grab 受检/幂等/step 后拒绝、**fallback Drop 的 finish<drop<ungrab<close 共享时间线**、shutdown finish 失败仍按序释放并上报）。集成 12 + 5 不变。
- `touchpadctl` 单元测试 **46** 个（args 12：含 **help 文本对 --grab 风险的显式警告**、**R5：`--grab` 对 devices/inspect/replay 全部拒绝**、**重复 `--grab` 拒绝**；devices 4：无设备/目录缺失/权限/混合 verdict；inspect 4：候选/非候选/缺失/权限；record 18：正常流水线+EOF 清理（**cleanup 行打印实际 fail-open 的 ungrab/close 结果**）、**--grab 显式 opt-in**、**decoder 失败不丢 7 个已读事件**、stop flag exit 8 有序清理、**EINTR+flag graceful**、EINTR 无 flag fatal、非候选/缺失设备、**R2：header flush 先于 grab 的共享时间线**、**R2：输出不可写/header flush 失败零 grab**、**R3：finish 失败 → 7 且设备仍按序释放**、**R3：ungrab 失败+close 成功 → 6 双诊断**、**R3：close 失败 → 6**、**R3：主 decoder 错误+cleanup 失败双保留**、**R3 re-review：fatal 主失败 + recorder finish 失败 → 7，双诊断与退出优先级**、**R4：状态 writer 失败后的有序 fallback 清理**、record→replay 同 decoder 往返；replay 8：**fixture replay smoke（single_contact → 3 帧 JSON）**、其余干净 fixture、dropped_recovery 离线失败、损坏行/schema/时间倒退/缺失文件 exit 5、header-only 零帧）与集成 **11** 个（`tests/cli.rs` 经公开 API：help exit 0 含警告、fixture replay smoke、无设备 exit 4、损坏 trace exit 5、**record 流水线顺序**、**signal stop exit 8 + ungrab<close 顺序**、缺失设备、inspect 非候选、header-only fixture、exit code 契约、**R5：非 record 命令 `--grab` 为 usage exit 1**）。
- 全 workspace：core 36+8；trace 74+14；linux 160+12+5；touchpadctl 46+11；doc-tests 2。共 **368**，0 失败。

### 16.8 M5 明确未实现（不得虚报）

- 实机验证全部未执行：正确识别内置触控板、capabilities/axis resolution 与 `evtest` 一致、真实 grab 独占、真实 `SYN_DROPPED` ioctl resync、真实拔出/EINTR、真实信号驱动清理（README 明确区分自动测试/环境探测/实机验证；本会话环境探测：无 `/dev/input`）。
- Wayland/libei、X11、uinput 输出后端；pointer/scroll/tap/drag/gesture 算法；GUI、常驻服务、开机自启、系统设置修改；读取 KDE 配置作为运行时依赖（§14.10 基线不得被复制为算法参数）。
- 离线 replay 无法对含 `SYN_DROPPED` 的 trace 做真实 resync（无内核快照源），诚实失败。


## 17. M6 — KDE Wayland Output Backend Qualification（已实现，待外部复核）

状态：**已实现**（2026-08-16 M6 re-review 的 R1–R6 与 cleanup 项，以及 re-review 1–3 的 R7–R12 已全部修复并补回归测试，待复审）。M6 尚未经外部 review 批准，本节内容均为「实现事实 + 待复核」，**不写 approved**。backend 状态为 **`experimental/unqualified`**，直到 reviewer 按 `docs/M6_ACCEPTANCE.md` §3 实测 `--emit`（相对 delta 位移 A/B、pixel scroll、按键释放、取消/拒绝清理）并记录测量结果；**未测量前不得成为 takeover 默认**（PHASE2_PLAN.md §5 M6 验收闸门）。

### 17.1 范围与边界

M6 实现 `crates/touchpad-desktop`（新 crate，平台输出 adapter）与 `apps/touchpadctl output-probe [--emit]`。它把 `touchpad-core::OutputSink` 契约（相对指针、主/次键、pixel-precise smooth scroll lifecycle）映射到本机已安装的 **XDG RemoteDesktop portal v2 + libei/liboeffis 1.6.0** 栈，带显式生命周期、诚实结构化失败与幂等 `release_all`。

**M6 明确不做**：读取/记录/grab 任何物理 `/dev/input` 设备；takeover、指针/滚动算法、tap、drag、手势、daemon、autostart、系统设置修改；测试或普通 probe 中自动移动指针/点击/滚动；创建虚拟触控板或向 compositor 暴露原始触点/finger count（TOUCH capability 永不 bind）；声称相对移动免于 compositor 二次加速（未测量前）。

**ABI 选择（依据本机环境，已文档化）**：

- Portal：`org.freedesktop.portal.RemoteDesktop` **interface version 2**（本机 D-Bus introspection 实测：version=2、`AvailableDeviceTypes=7`、`ConnectToEIS` 存在）。v2 流程：`CreateSession → SelectDevices(pointer) → Start（授权对话框）→ ConnectToEIS → EIS fd`；`Start` 的 Response 信号（0 ok / 1 cancelled / 2 refused）是授权结果。
- libei：soname **`libei.so.1`**（1.6.0），**运行时 `libloading` 加载**——构建期不链接 libei，缺库是结构化 `LibraryMissing`（dry-run 报告、`--emit` pre-flight 拦截，exit 4），CI 无库也可构建/测试。
- D-Bus：纯 Rust `zbus`（blocking API，不链接系统 D-Bus 库）；请求用 handle_token 预先订阅 `Response` 信号（request path = `/org/freedesktop/portal/desktop/request/<sender_component>/<token>`，sender component = unique name 去掉前导 `:` 且 `.` → `_`，与 xdg-desktop-portal 的 `xdp_request_init_invocation` 一致），消除信号竞态。**token 必须是合法 D-Bus object-path element**：`handle_token` 与 `session_handle_token` 都从 `[A-Za-z0-9_]` 字母表生成（`m6_<pid>_<counter>`，唯一且可区分；旧 `m6-<pid>-<counter>` 含 `-`，导致真实 `--emit` 在授权前 exit 9 `Invalid object path`——M6 re-review R12），预测的完整 request/session path 在注册 match rule **之前**用 zvariant 校验，失败返回结构化 `InvalidPortalPath`（错误信息点名 kind、构造出的 path、sender component、token）。`CreateSession` 携带**两个不同** token（request `handle_token` + `session_handle_token`，spec 仅文档化这两个 key）；同步方法 `ConnectToEIS` 不携带 `handle_token`（其 options 契约无任何 key）。**`CreateSession` 响应的 `session_handle` 线型是 `s` 而非 `o`**（本机 `/usr/share/dbus-1/interfaces/org.freedesktop.portal.RemoteDesktop.xml` 明示：session handle 是 object path 但“erroneously implemented as `s`”，向后兼容保持 `s`——M6 re-review R13）；客户端**先按 `s` 解码字符串，再校验其内容为合法 object path** 存入 `PortalSession`，绝不声称响应线型为 `o`（旧代码直接 `OwnedObjectPath::try_from(OwnedValue)`，真实 `--emit` 因此 exit 2 `incorrect type`）。

### 17.2 模块图（`touchpad-desktop`）

| 模块 | 关键类型 | 职责 |
| --- | --- | --- |
| `error` | `DesktopOutputError`（单一结构化错误分类：NoSessionBus / PortalUnavailable / ProtocolUnsupported / AuthorizationCancelled / AuthorizationRefused / LibraryMissing / TransportDisconnected / DevicePaused / SendFailed / ReleaseFailed / PrepareFailed / CapabilityMissing / Timeout / Cancelled / **InvalidPortalPath**（预测的 request/session path 非合法 D-Bus object path，错误点名 path 构造——M6 re-review R12）/ UnsupportedPlatform / Internal） | 全 adapter 统一错误；CLI 映射到稳定退出码；`PrepareFailed{primary, cleanup}` 复合保留主错误类别（M6 re-review R4） |
| `capabilities` | `OutputCapabilities`、`Capability`、`libei_capability_bits` | 只在实际协商到的 libei capability 存在时报告支持；TOUCH 永不映射 |
| `held` | `HeldState`（buttons/keys/scroll_open/scroll_x_active/scroll_y_active） | 已成功提交的按键/滚动生命周期跟踪；`validate`（纯校验）与 `record`（提交）分离；**逐轴**跟踪非零 delta（M6 re-review R5），`scroll_stop_axes()` 只对活动轴 stop；`release_events()` 确定性合成释放序列（无非零 delta 的 begin 不产生裸 ScrollStop——libei 文档视为 client bug） |
| `transport` | `Transport` trait、`TransportEvent`（Connected/SeatAdded/DeviceAdded（含 `device_type`）/DeviceResumed/…/Disconnected/Timeout）、`SeatId`/`DeviceId`（不透明）、`DeviceType`（Virtual/Physical/Other） | libei sender 传输 seam；`pump()` 非阻塞排空服务器事件（M6 re-review R3）；fake 与 native 同一契约；物理设备（毫米）与虚拟设备（逻辑像素）区分 |
| `portal` | `Portal` trait、`PortalSession`、`EisFd`、`device_types` | RemoteDesktop portal seam（CreateSession/SelectDevices/Start/ConnectToEIS/Close） |
| `portal_zbus` | `ZbusPortal`（含 `TokenGenerator`：从 D-Bus object-path-safe 字母表 `[A-Za-z0-9_]` 生成唯一 token `m6_<pid>_<counter>`） | 真实 portal client（zbus blocking；handle_token 预先订阅；**预测的 request/session path 在注册 match rule 前用 zvariant 校验**，失败为结构化 `InvalidPortalPath` 且点名 path 构造——M6 re-review R12；`CreateSession` 携带不同的 `handle_token` + `session_handle_token`；同步 `ConnectToEIS` 无 `handle_token`；**`CreateSession` 响应的 `session_handle` 按线型 `s` 解码后校验内容为 object path——M6 re-review R13**；Response 等待在 helper 线程上把异步 stream 与 deadline 竞速，**超时线程也会退出**——M6 cleanup：不再遗留阻塞线程；probe 只读属性） |
| `ffi` | `Libei`（libloading 符号集）、**non-`Copy` RAII owner** `EiContext/EiSeat/EiDevice/EiEvent`（Drop 恰好一次 unref）、lifetime-bound 借用视图 `EiSeatRef/EiDeviceRef`、`poll_fd` | **唯一含 `unsafe` 的模块且 crate 私有**（Linux-only 运行时加载）；句柄不可复制、不可重复释放、释放后不可用（借用检查器）；`ei_device_get_type` 查询真实设备类型（M6 cleanup） |
| `native_transport` | `NativeTransport`（`#![forbid(unsafe_code)]`） | 真实 libei sender：own context/ref-counted seats/devices（RAII 释放顺序 devices → seats → context）、`poll` 事件循环、motion/button/scroll/frame 发射、`pump()`（dispatch 同时 flush 出站数据；写侧错误以 DISCONNECT 事件浮出）、幂等 disconnect；**测试永不构造它** |
| `fake` | `FakePortal`、`FakeTransport`、`FakeWireCall`、`FakeDesktopOutput`、`FakeProbeSource` | 确定性测试 seam：脚本化事件、故障注入（含 `connect_error`/`close_error`，M6 re-review R4）、完整 wire call 日志；**任何 fake 都不发射真实桌面输入** |
| `sink` | `PortalOutputSink<P, T>`、`SessionState`（Disconnected/Authorizing/Ready/Emulating/**Interrupted**/Stopping/Stopped/Fatal） | 会话生命周期 + `OutputSink` 实现：`prepare()`/`prepare_cancellable()`（portal 授权 → ConnectToEIS → libei 握手 → Emulating；握手可取消）、`submit()`（仅 Emulating、按 capability 门控、validate→pump→send→pump→commit；握手后每帧 pump 服务器事件，pause/removal/disconnect → **Interrupted** 拒绝后续输出并结构化失败）、`release_all_detailed()`（幂等；release sends → disconnect → close session；disconnect 是 compositor 侧状态复位 backstop）、fallback `Drop` |
| `emit` | `pattern()`、`run_pattern`、`EmitOutcome`、`MAX_PATTERN_EVENTS = 16` | 固定有界 `--emit` pattern（+10/+50/+200 px 相对移动、主键 click、smooth scroll begin/-120/-240/end、次键 click）；缺 capability 的步骤跳过并报告 |
| `probe` | `ProbeReport`、`ProbeSource`、`EnvProbeSource`、`render_report`、`preflight_error` | 只读环境探测（session bus、portal version/device types、libei dlopen、平台观察）；backend 状态恒为 `experimental/unqualified` |
| `desktop` | `DesktopOutput` trait、`EmitDriver`（sleeper/progress/cancelled 注入）、`PortalDesktopOutput`（Linux）、`UnsupportedDesktopOutput` | CLI seam：`probe()` 非发射；`emit_pattern()` pre-flight → prepare → run_pattern → **任何路径都 release**，保留主错误与 cleanup 诊断 |

### 17.3 生命周期与失败语义（outcome 2/3）

- `prepare()`/`prepare_cancellable()`：CreateSession → SelectDevices(pointer) → Start（授权；取消=AuthorizationCancelled、拒绝=AuthorizationRefused，均结构化）→ ConnectToEIS → libei 握手（bind pointer/button/scroll；等有用 **虚拟** device added+resumed；15s 超时；握手每 500ms 检查取消钩子，M6 re-review R2）。**任何失败都走 release_all_detailed()**，不遗留活动 session；cleanup 也失败时返回 `PrepareFailed{primary, cleanup}` 复合，**保留主错误类别/退出码**并把 cleanup 诊断带给调用方（M6 re-review R4）。物理设备（毫米）不会被选为候选：只有虚拟设备（逻辑像素）才进入单位映射；仅有物理设备时握手以 `CapabilityMissing` 结构化失败（M6 cleanup：真实设备类型经 `ei_device_get_type` 查询）。本步骤是 M10 在 EVIOCGRAB 之前必须完成的「输出准备与授权」。
- `submit()`：非 Emulating → `Unavailable`；能力缺失 → `Unavailable`；生命周期滥用（ButtonUp 未持有、ScrollEnd 无 begin 等）→ `Rejected`（validate 纯校验，先于 send）；send 失败 → `Io` 且 **不记入 held**（部分发送失败诚实上报，绝不虚报成功）。每帧前后非阻塞 **pump** 服务器事件（M6 re-review R3）：活动 device 被 pause/removed、seat 被 removed、或 DISCONNECT → 状态转 **Interrupted**，后续输出被拒，结构化失败经 `take_server_interruption()` 保留给调用方。
- `release_all`/`release_all_detailed`：幂等；顺序 = 释放已跟踪 button/scroll → `transport.disconnect()`（compositor 复位 backstop）→ `close_session()`；Interrupted 时跳过 release sends（设备已不在，disconnect 即 backstop）；disconnect 成功 ⇒ Stopped，失败 ⇒ Fatal；失败聚合为 `ReleaseFailed` 并保留在 `take_cleanup_error()`。`Drop` 为 best-effort 兜底（早期 `?` 与 unwind 均不遗留逻辑持有状态）。
- `OutputSink` 的 `ScrollBegin` 无 wire 事件（首条非零 delta 在 server 侧开始滚动）；`ScrollEnd` → **只对收到非零 delta 的轴** `scroll_stop(stop_x, stop_y)` + frame（逐轴生命周期，M6 re-review R5；固定 probe 只发 Y delta → 只 stop Y）；每个逻辑事件后 frame（`ei_now` 单调 µs）。

### 17.4 `output-probe`（outcome 4）

- 默认 **dry-run 非发射**：打印 probe 报告（backend state、平台观察、session bus、portal version/device types、libei、requested capabilities、`--emit` 八步、unqualified 提示），exit 0（完成即成功；结论在报告里）；绝不移动指针/点击/滚动，不触碰 `/dev/input`。
- `--emit`：显式 opt-in（其他命令拒绝、重复拒绝）；可见警告 + **3 秒倒计时**（每 100ms 轮询取消，Ctrl-C → exit 8「nothing was emitted」）；然后固定有界 pattern（≤16 wire events），每步按协商能力门控；**任何路径**（成功/部分发送失败/断连/取消/服务器中断）都执行有序 release（held → disconnect → close session）；退出码映射：0 成功、1 用法、2 无 bus/portal、3 授权取消/拒绝、4 缺库/协议/能力、5 断连/超时/设备暂停、6 发送失败、7 release 失败、8 用户中止、9 内部。**真实 Ctrl-C/SIGTERM 由 binary 为 `--emit`（与 `record`）安装受控信号处理**（M6 re-review R2：`command_needs_termination_handler` 分类；dry-run 不安装）；授权/握手等待有界（Start 120s、握手 15s），信号在握手期即时中止（exit 8），在 portal 阻塞期最迟于该有界等待结束后清理。
- **测试永不发射真实桌面输入**：所有测试走 FakePortal/FakeTransport/FakeDesktopOutput；真实 zbus portal 与 libei transport 在测试中从不构造；真实信号回归（SIGINT/SIGTERM → 停止静态 → exit 8 → guard 恢复）在 touchpadctl 集成测试中用 `libc::raise` 端到端验证（M6 re-review R2）。

### 17.5 unsafe 边界（M6）

`unsafe` 只存在于 `touchpad-desktop::ffi`（Linux-only，运行时加载）：

| 位置 | 操作 | 安全不变量 |
| --- | --- | --- |
| `load_libei` | `Library::new` + 符号解析 | 库对象被 `Libei` 持有整个生命周期；fn 指针复制出来只在库加载期间有效（模块不变量 1）；缺失库/符号 → `LibraryMissing` |
| `Libei` safe wrappers（内部 unsafe） | 调用 libei fn 指针 | 所有指针经 **non-`Copy` RAII owner**（`EiContext/EiSeat/EiDevice/EiEvent`，`Drop` 恰好一次 unref；无 `Clone`/`Copy`）与 lifetime-bound 借用视图（`EiSeatRef/EiDeviceRef`，不得长于所属 event）传递；`ffi` 模块 **crate 私有**，安全代码无法命名/构造句柄（M6 re-review R1）；句柄来自 libei 且在使用期间存活（不变量 2）；fd 所有权在 `setup_backend_fd` 转移给 libei（调用方不再 close）；`seat_bind_capabilities` 走**可变参数 fn 指针类型**传固定 `(seat, caps..., NULL)` 序列（不变量 4）；单线程（CLI 主线程，不变量 3）；唯一长生命周期持有者 `NativeTransport` 在 `Libei` 字段析构前按 devices → seats → context 顺序释放全部 owner（不变量 1 的构造性保证） |
| `poll_fd` | `libc::poll` | `pollfd` 已初始化、单 fd、超时有界 |

`native_transport`、`sink`、`emit`、`probe`、`portal_zbus`、`fake`、`held`、`capabilities`、`desktop`、`transport`、`portal`、`error` 全部 `#![forbid(unsafe_code)]`（rustc 强制）。

### 17.6 测试矩阵（M6；全部无硬件、无桌面、无 session bus 依赖）

workspace 共 **496** 个测试（M5 368 + M6 新增 128，0 失败）。覆盖：held 生命周期与幂等 release（含**逐轴** scroll：x-only/y-only/两轴/零 delta/重复生命周期/部分发送/强制 release，M6 re-review R5）；能力协商（pointer-only / scroll-only / TOUCH 永不映射）；会话生命周期（happy handshake、取消授权、拒绝授权、握手期断连、握手超时、未 prepare 拒绝 submit、wire 顺序、ScrollBegin 无 wire、能力缺失拒绝、keyboard/desktop action 拒绝、部分发送失败诚实且不记 held、release_all 幂等、release 先于 disconnect 的顺序、fallback Drop、release 失败保留）；**prepare 各阶段（SelectDevices/Start/ConnectToEIS/transport connect/handshake）× close_session/transport-disconnect 故障注入 → `PrepareFailed` 复合且主类别保留**（M6 re-review R4）；**握手后服务器事件**（pause/removed/seat-removed/disconnect → Interrupted、拒绝后续输出、无后续 wire 事件、仍执行 cleanup；他设备 pause 忽略；pump 排空语义）（M6 re-review R3）；**可取消握手**（prepare 期/授权后/握手期取消 → Cancelled + 释放会话，M6 re-review R2）；**物理设备拒绝/虚拟设备选用**（M6 cleanup）；固定 pattern（内容固定、≤16 事件、覆盖四能力、跳过缺能力步骤、尊重取消）；fake seam 的调用记录与故障注入；probe 渲染与 pre-flight 拦截；libei loader 结构化 probe（dlopen 成功或 LibraryMissing，不 panic）、常量对照 `libei.h` 1.6、**RAII owner Drop 恰好一次 unref / 空句柄 no-op / 可移动不可复制**（M6 re-review R1）；zbus request path 约定与 Response code 映射、**Response 竞速 deadline：超时退出线程**（M6 cleanup）；**portal token/path 回归**（每个生成 token 都是合法 object-path element 且唯一（单 generator 1 万 + 跨 pid 不碰撞）、预测的 request/session path 全部合法且符合 portal 命名约定、非法 token 产生点名 path 构造的 `InvalidPortalPath` 诊断、`CreateSession` options 恰含两个不同且安全的 token、每方法 options 契约符合 spec：request 方法不带 session token、`ConnectToEIS` options 为空——M6 re-review R12）；**`CreateSession` 响应解码（纯函数，无 live portal）**（`session_handle` 线型 `s`：合法路径字符串成功；缺失 key / 非字符串（点名实际值与 D-Bus signature，含“本应为 `o`”的 `ObjectPath` 值也被 string-first 解码拒绝）/ 字符串内容非法路径 各自给出不同诊断——M6 re-review R13）；CLI 解析（`--emit` opt-in/重复拒绝/`--grab` 拒绝）、dry-run exit 0 且不发射、`--emit` fake 成功路径与全部退出码映射（2/3/4/5/6/7/8/9）、倒计时取消、**真实 SIGINT/SIGTERM → 停止静态 → exit 8 → guard 恢复**（M6 re-review R2，touchpadctl 集成测试）。

**测试真实性声明**：没有任何测试成功打开或 grab 真实设备，也没有任何测试发射真实桌面输入；唯一的真实 OS 表面为无副作用检查（sigaction/raise、不存在路径、`libei.so.1` dlopen probe、session bus 可达性 probe）。

### 17.7 文档化限制（不虚报）

- **未执行 reviewer 实测**：`--emit` 的桌面位移测量、pixel scroll 观察、按键释放确认、取消/拒绝清理——backend 保持 `experimental/unqualified`（`docs/M6_ACCEPTANCE.md` §3 是书面人工验收程序）。
- **不保证的清理**：`SIGKILL`、内核崩溃、断电不能运行用户态 cleanup；portal 在客户端退出时会关闭 session，但有序序列只在可运行用户态代码的路径上有保证。
- **相对移动免二次加速未声称**：libei/portal 由 compositor 最终处理，是否再加速/再解释必须实测（PHASE2_PLAN.md §2）。
- **MSRV（M6 re-review R6）**：manifest 声明 `rust-version = 1.87`（锁图 zbus 5.19/zvariant 族的真实最小声明；不再声明 1.85）。尚未在 1.87 工具链独立验证；闸门在 1.97.1 运行。
- 单设备模型：M6 使用第一个有用的 EIS device（KWin 提供单一虚拟 pointer）；能力分散在多个 device 时只报告所选用 device 的能力，不静默合并。仅物理设备（毫米）出现时以 `CapabilityMissing` 拒绝，绝不把毫米 delta 当作逻辑像素（M6 cleanup）。

## 18. M7 — Arbiter 骨架、单指线性指针与物理点击（离线，已实现）

状态：**已实现**（2026-08-16）。M6 已由外部 review 终审通过（M6_REVIEW.md re-review 5），M7 按 M7_TASK.md 在离线约束下实施。M7 是纯离线策略里程碑：**不连接 M6 真实后端、不读取/记录/grab 任何物理 `/dev/input`、不调用 portal/libei、不实例化 `PortalDesktopOutput`、不运行任何 live 输入/输出命令**。

### 18.1 范围与边界

M7 全部实现位于平台无关的 `touchpad-core`（`arbiter` 模块 + `units::LogicalPixelsPerMm` + 4 个新 `DiagnosticCode`），以及两个离线集成测试（`touchpad-core/tests/m7_arbiter.rs` 公共 API 契约、`touchpad-linux/tests/m7_arbiter.rs` trace/replay 与合成帧同路径证明）和一个新 trace fixture（`touchpad-trace/tests/fixtures/m7_motion.jsonl`）。`touchpad-core` 无任何 Linux/Wayland/KDE/桌面依赖；`arbiter` 模块位于 `#![forbid(unsafe_code)]` crate 内，**无 `unsafe`**。

**明确不做（M8/M9 及以后）**：tap、tap-and-drag、drag lock、双指滚动/右键、momentum、手势、加速度曲线、palm/thumb 分类、haptics；物理右键/中键映射、Force Click、压力行为；任何 CLI/daemon/autostart/环境配置改动。M7 不读取 KDE/libinput 设置，不声称 macOS 加速度曲线。

### 18.2 模块图（`touchpad-core`）

| 模块 | 关键类型 | 职责 |
| --- | --- | --- |
| `arbiter` | `Arbiter`、`ArbiterConfig`、`FrameDecision`、`Lifecycle`、`LifecycleTransition`、`ArbiterError`/`ArbiterConfigError`/`TransitionError`/`ArbiterSinkError`、`ArbiterSink<S>` | 统一 Interaction Arbiter：单一策略 owner；`Candidate/Committed/Cancelled/Finished` 生命周期；一指线性指针（毫米输入 → 逻辑像素输出，逐轴 sub-pixel remainder）；物理左键 down/up 生命周期与按住拖动；`release_all` 幂等释放；原子帧决策（draft state，失败不产生半应用状态）；**`ArbiterSink` 交付感知 fail-stop 适配**（逐事件确认、接受前缀跟踪、部分失败 fault、重试式 cleanup，见 §18.4） |
| `units` | `LogicalPixelsPerMm`（`ScaleError`） | 显式线性 mm→逻辑像素映射；构造时校验有限且严格为正 |
| `diagnostic` | `DiagnosticCode::{InteractionBegun, InteractionCommitted, InteractionCancelled, InteractionFinished}` | 生命周期可观测诊断（Info/Warning） |

### 18.3 生命周期与失败语义

- **一个 arbiter 裁决全部竞争**：`ContactFrame` 只进一个 `Arbiter`；不存在独立 pointer/tap/scroll recognizer 各自对同一帧 commit。生命周期转移由纯函数 `Arbiter::validate_transition` 校验（合法：`Idle|Cancelled|Finished → Candidate`、`Candidate → Committed|Cancelled|Finished`、`Committed → Cancelled|Finished`；其余一律 `TransitionError::Illegal`，绝不 panic；5×5 全表测试）。
- **候选期零泄漏**：阈值前不发射任何 `PointerMove` 或合成按键事件；首次 commit 把自 anchor 起累计的位移**恰好一次**量化输出（不丢失、不重复）；低于阈值开始并结束的接触在 M7 产出零事件（M8 加 tap 语义）；零位移不产生 `PointerMove`。
- **确定性取消**：第二 live contact、discontinuity、活动接触缺必需坐标 → 决策级取消（按钮事件仍处理）；timestamp/sequence 回归 → 结构化错误 + 取消 + 保留回归基线（fail-closed 直到 `release_all`）。
- **帧级模型校验（M7_REVIEW R2）**：`Arbiter::frame` 直接消费 `ContactFrame::validate()`（核心模型的既有校验，不重复实现子集）。任何 **Error/Fatal** 诊断（负 live tracking id、非有限/越界 pressure、非有限 orientation、负 ellipse 轴、重复 slot）→ `InvalidFrame { sequence, codes, reason }`，整帧原子拒绝：**零状态/按钮/回归基线变更**（即使同帧还携带物理按钮边沿，边沿也不应用、不产生 `ButtonDown`）。**Warning-only**（不完整 `Began` 接触缺坐标）不触发拒绝：保留 M7 策略——不产生 candidate/输出并附 Warning 诊断；活动接触缺坐标仍取消并处理物理释放。
- **原子性**：`frame()` 对 draft state 计算完整决策（按钮边沿、运动、生命周期、新状态），全部校验与算术成功后才原子换入；拒绝的帧（`InvalidFrame`/`NonFinite`）不留下半应用 contact/button/scale/remainder/lifecycle 状态；`NonFinite` 路径无部分事件批次。
- **remainder 不变量**：逐轴像素空间 remainder；`total = remainder + d_mm·s`（f64 精确）、`emitted = trunc(total)`、`remainder' = total - emitted ∈ (-1,1)`；`Σ emitted + remainder == Σ scaled` 恒成立；取消/结束/`release_all` 清零 remainder，新接触绝不继承旧 remainder（tracking-id 替换与 slot 复用有回归测试；many-small-deltas 与等价 aggregate 总量相等有测试）。

### 18.4 物理左键与拖动

- 与帧原子消费 `physical_buttons.left` 边沿：false→true 恰好一个 `ButtonDown(Left)`，true→false 恰好一个 `ButtonUp(Left)`，稳定状态零事件；重复 down/up 对按帧序直通（无人工延迟、无发明事件；两个有效对即物理双击表示）。
- 同帧确定性顺序：**按压先于属于拖动的运动，最后一段运动先于释放**。
- 释放永不因接触取消/新增手指/缺坐标/discontinuity 被抑制；`release_all`（M10 shutdown 路径）对逻辑持有左键恰好释放一次且幂等，清除 candidate/remainder/回归基线，即使此前出错。
- **`ArbiterSink` 交付感知、fail-stop（M7_REVIEW R1）**：`Arbiter::frame` 仍先在纯状态机上原子提交决策（纯 arbiter 行为不变）；`ArbiterSink::frame` 随后**逐事件提交并确认**，跟踪接受前缀：
  - 被拒绝的 `ButtonDown` **不算已交付**——arbiter 的 held 标志在部分失败边界被对账回实际接受前缀，cleanup 绝不因此产生 unmatched `ButtonUp`；
  - 已接受的 down 之后运动/up 失败 → 仍记为 delivered-held，直到 cleanup 成功；
  - 任何部分提交 → 适配器进入 **faulted**，后续普通帧一律 `ArbiterSinkError::Faulted` 拒绝，直到 cleanup 成功复位（输出状态可能与决策状态分叉时不得静默继续）；
  - `release_all` 只释放 sink 实际接受的按键：显式 `ButtonUp` 提交 + 调用被包装 `OutputSink::release_all`（其自身 cleanup 契约）；只有两者都确认（acknowledgement boundary）才 `arbiter.release_all()` 复位并清 fault；任一步失败都保留 owed release、不复位 arbiter，下一次调用重试；
  - 结构化错误保留 failed event/index、accepted prefix、primary failure，以及 primary 与 cleanup 同时存在时的 cleanup failure，不假装整批决策已交付。
- **错误面**：`ArbiterSinkError::{PartialSubmit{index, accepted_prefix, decision_len, failed_event, primary}, Faulted, ReleaseFailed{primary, others, cleanup}}`（M9_REVIEW R5 起 `others: Vec<OutputError>` 逐条报告首个之后的显式 release 失败；M9 可同时 owed `ButtonUp(Right)` 与 `ScrollEnd`）；`ArbiterError` 直接透传且不 fault 适配器（arbiter 状态未变，后续合法帧可继续）。

### 18.5 测试矩阵（全部离线；workspace 共 570 个测试）

- `touchpad-core` 单元测试 92（M1 36 + M7 新增 56）：生命周期合法/非法全表、候选期零输出、阈值边界（恰好/刚低于/刚高于/结束帧跨越）、首次累计位移恰好一次+增量、水平/垂直/对角/负向/零位移、many-small vs aggregate（整数与分数 ppm）、remainder 累计与负向对称、tracking-id 替换/slot 复用无锚点与 remainder 泄漏、取消后 Active 不恢复、第二接触（commit 前/后）、第二指单独抬起不取消、缺坐标/重复 slot/discontinuity/时间回归/序列回归、回归基线保留至 release、非有限算术原子拒绝、物理 down/up/稳定去重/click/双 click 对/无接触按压/取消与 discontinuity 下释放、同帧 press-move-release 顺序、按住拖动、结束帧 final-move-before-release、候选期物理按压不 commit、`release_all` 幂等与错误后复位、config 校验、decision serde 往返；**R1 故障注入（6 例）**：被拒 down 不 held 且无 unmatched up、已接受 down 后运动失败保持 delivered-held、已接受 down 后 up 失败保持 held 并重试、cleanup 首次/重复失败重试、wrapped sink `release_all` 失败上报与重试、fault 复位后新 interaction 无重复/丢失；**R2 帧级校验（4 例）**：负 live tracking id、pressure/orientation/ellipse 非法、Warning-only 不完整 Began、无效帧携带按钮边沿的原子性。
- `touchpad-core` 集成测试 8 + 14（`tests/m7_arbiter.rs`）：公共 API 全流程、回归/无效帧结构化错误、`release_all` 公共幂等、ArbiterSink 事件顺序 + R1 故障注入（被拒 down/运动失败/up 失败/cleanup 重试/wrapped sink cleanup/fresh interaction）、R2 公共校验（负 id、pressure/orientation/ellipse、Warning-only 不完整 Began、按钮边沿原子性）、decision serde。
- `touchpad-linux` 集成测试 + 4（`tests/m7_arbiter.rs`）：**replay 派生帧与手工合成帧逐帧决策相等**（single_contact/buttons/m7_motion 三个 fixture；证明 trace/replay 与直接合成走同一 arbiter 路径）、m7_motion fixture 端到端（阈值跨越恰好一次 + 拖动 + down/up + 顺序 + Finish）、低于阈值 fixture 零输出、buttons fixture 恰好一次 click。
- `touchpad-trace`：新 fixture `m7_motion.jsonl`（20 事件）加入 fixtures 契约表。
- 全部测试无硬件、无桌面、无 `/dev/input`、无 portal/libei、无真实输出。

### 18.6 文档化限制（不虚报）

- M7 是离线策略层：未与 M6 输出后端串接（M10 的纵向切片范围）；未做任何实机行为验证；不声称桌面相对位移免二次加速。
- 阈值/scale 是显式配置；未做 A/B 校准（M11 拥有加速度/抖动/速度估计；KDE 体验基线 §14.10 只作参考，不复制为参数）。
- 取消后同指（Active）不恢复候选：只有新 Began 接触开启新 interaction（确定性规则，有测试）。
- `SIGKILL`/内核崩溃/断电不保证用户态 cleanup（保持 §14.9/§17.7 限制）。

## 19. M8 — Tap、Tap-and-Drag 与 Sticky Drag Lock（离线，已实现）

状态：**已实现**（2026-08-17）。M8 在 M7 已批准（M7_REVIEW.md re-review 2）后按 M8_TASK.md 实施，是纯离线策略里程碑：**不连接 M6 真实后端、不读取/记录/grab 任何物理 `/dev/input`、不调用 portal/libei、不实例化 `PortalDesktopOutput`、不运行任何 live 输入/输出命令**。

### 19.1 范围与边界

M8 全部实现位于平台无关的 `touchpad-core`（`arbiter` 模块 + 4 个新 `DiagnosticCode`），以及离线测试（`touchpad-core` 单元/集成、`touchpad-linux` replay/synthetic 同路径、新 trace fixture `m8_tap.jsonl`）。`touchpad-core` 无任何 Linux/Wayland/KDE/桌面依赖；`arbiter` 模块位于 `#![forbid(unsafe_code)]` crate 内，**无 `unsafe`**，**无新依赖**。

**明确不做（M9 及以后）**：双指滚动/右键、momentum、pinch/rotate/swipes、加速度曲线、palm/thumb 分类、Force Click、压力、haptics；任何 CLI/daemon/autostart/环境配置改动。M8 不读取 KDE/libinput 设置（KDE/libinput 值只是 A/B 参考），不声称 macOS 行为。

### 19.2 配置与默认

- **默认禁用**：`ArbiterConfig::new(...)` 的 tap 配置为 `None`，全部 M7 低于阈值序列保持零输出；显式 `ArbiterConfig::with_tap(TapConfig)` 才启用。
- `TapConfig::new(tap_enabled, tap_and_drag_enabled, drag_lock_enabled, max_tap_duration, max_tap_movement_mm, max_tap_drag_gap)` 构造时校验：duration 严格为正（零/非法拒绝，不静默修正）、movement 严格为正（`Millimeters` 类型保证有限）、特征组合必须可能（tap-and-drag 要求 tap；drag lock 要求 tap-and-drag）。
- 边界策略（文档化 + 测试）：配置的 duration/distance/gap **相等即接受**，**严格更大**即过期/取消。
- 只使用 `ContactFrame.monotonic_timestamp` 与 checked duration 运算；不读墙钟/进程本地时钟。超时只在**到达帧边界**求值。Sticky drag lock 在 M8 无自主超时；`release_all` 是无条件逃生路径。

### 19.3 单一 arbiter 与可观测 tap/drag 阶段

- 仍是**单一 arbiter 作为唯一策略 owner**；无独立 recognizer 可对同一帧各自 commit。
- 新增 `TapDragPhase`（`Idle | FirstTapCandidate | FollowUpWindow | TapDragContact | LockedWithoutContact | LockedContact | Cancelled | Finished`），可观测于 `FrameDecision.tap_drag_phase_after` 与 `Arbiter::tap_drag_phase()`。
- M7 `Candidate/Committed/Cancelled/Finished` 指针生命周期、首次累计位移恰好一次与逐轴 remainder 不变量完整保留；M8 只在归属需要的边界重构。
- Tap 候选跟踪**自其 anchor 起的最大位移**（非仅最后一帧 delta）；超过 tap 阈值后即使回到 anchor 也永久失去 tap 资格；指针一旦越过 M7 motion 阈值即提交，同一接触绝不同时产生普通指针输出与 tap click。

### 19.4 Tap 语义

- 一指 Began→Ended 只有同时满足：tap 启用、坐标有效（Ended 携带坐标）、时长 ≤ 上限、最大位移 ≤ 上限、无额外 live 接触、无物理点击竞争、无 discontinuity/取消/错误时才算 tap。
- 合格 tap 在 release 帧输出**恰好 `ButtonDown(Left), ButtonUp(Left)`**（同帧下/上，聚合 OR 由 false 起 false 终仍产生下+上）。两个合格 tap 自然输出两对 click pair（不延迟第一击、不发明双击事件）。过长/过远/不完整/取消/多接触/discontinuity 序列零合成 click。tap 禁用时全部 M7 行为不变。

### 19.5 Tap-and-drag 语义

- 合格首 tap 打开 follow-up 窗口；**恰好一个**新有效手指在 deadline 内（相等接受）开始且 tap-and-drag 启用 → 进入 `TapDragCandidate`，**此时不合成左键 down**。这条安全边界用于阻止短暂 re-touch / tracking-id bounce 在窗口焦点切换附近制造 held-left。
- follow-up 接触沿用 M7 的 typed 线性映射、阈值、首次累计 delta 恰好一次与 remainder 规则；只有指针 motion 真正 commit 时，才在第一段 committed motion 之前输出逻辑左 down，并进入 `TapDragContact`。因此真实 drag 的线序仍严格为 `ButtonDown(Left) → PointerMove...`。
- follow-up 未 commit 且以合格第二 tap 干净结束 → 在 release 帧一次性输出第二对 `ButtonDown(Left), ButtonUp(Left)`；不合格/替换/取消则零合成按钮事件，**绝不进入 drag lock**。已 commit 且 drag lock 禁用 → 手指释放时最后一段 motion 先于一个合成 up。已 commit 且 drag lock 启用 → 手指释放保持合成左键 held 并进入 locked-without-contact。
- 严格晚于窗口的 follow-up 是普通新指针/tap 候选，**绝不合成提前 down**。

### 19.6 Sticky drag-lock 语义

- locked-without-contact 中，一个新有效手指开始 locked-contact 候选（**不再下按**）；越过 M7 指针阈值 → 累计首 delta 恰好一次输出、继续正常拖动、抬起后回到 locked-without-contact（仍无 up）。
- 该接触以合格 tap（无已 commit 位移）结束 → 输出**恰好一个逻辑左 up** 并离开 drag lock。
- 不合格（过长/过远）且从未 commit 的接触 → 不伪造 click，lock 保持 held 供下一次续拖尝试；`release_all` 始终终结。
- 额外 live 接触、discontinuity、无效活动坐标、确定性取消 → 合成 drag/lock fail-closed 以一个逻辑 up 结束（除非物理源仍 held）。

### 19.7 物理/合成左键仲裁

- 重构单一 `held_left` 为**源感知**：分别跟踪 `physical_left` 与 `synthetic_left`，只把逻辑 OR 暴露给输出 sink；聚合 false→true 才发 `ButtonDown(Left)`、true→false 才发 `ButtonUp(Left)`。
- 物理按压取消 pending tap/follow-up 策略并胜过合成 click 生成；合成 drag/lock 期间物理按下不重复 down；合成源结束时若物理仍 held 不发 up；合成 drag 在物理 held 时开始不重复 down。
- 确定性顺序：任何聚合 down 先于拖动 motion；最后 motion 先于聚合 up。物理释放不因 tap 取消/额外接触/缺坐标/discontinuity 被抑制。同帧合成 tap pulse 仍产生 down 后 up。按钮复用集中在 frame 内的单一 multiplexer，物理与合成路径不可能各自发出矛盾事件。
- 帧内顺序是**连贯的源转换序列**（M8_REVIEW R2 修复）：`Arbiter::frame` 在任何策略变更之前捕获真正的 pre-frame 源状态（`physical_prev`/`synthetic_prev`），并**先应用物理边沿**（`draft.physical_left = frame.physical_buttons.left`），再执行 discontinuity/取消前奏与接触策略——因此任何合成转换（`begin_synthetic`/`end_synthetic`）都观察到**当前**物理状态，不再读取陈旧快照。discontinuity + 物理释放同帧时恰好产生一个聚合 up；debug 构建的 `simulate_wire` 断言恒等（发射 wire 状态 == post-frame 聚合），另有表驱动测试覆盖 discontinuity/额外接触/缺坐标三类取消 × 物理 held/释放组合。
- 聚合 OR 真值表、同帧顺序（tap pulse、press+move、move+release）均有测试。

### 19.8 失败、清理与兼容

- 帧校验与原子 draft commit 保留：被拒 Error/Fatal 帧不改任何指针/tap/计时/按钮源/基线/输出状态。
- 序列/时间戳回归仍 fail-closed；合成 held 状态对 `release_all` 保持可见，`release_all` 输出所需聚合 up 恰好一次并重置所有指针/tap/drag/lock/计时/源状态。
- `ArbiterSink` 接受前缀/fail-stop 契约对合成 click 与 drag 事件保留；新增状态化 sink 故障测试：被拒 tap down、已接受 down 后被拒 tap up、已接受合成 down 后 motion 失败、drag-locked 下 cleanup/恢复。无 unmatched/duplicate up、无 lost release。
- M1–M7 API 保持（`FrameDecision` 增加一个字段 `tap_drag_phase_after`）；574 个既有测试全部通过；serde/finite-unit 保证保留；`#![forbid(unsafe_code)]` 保留。

### 19.9 测试矩阵（全部离线；workspace 共 647 个测试）

- `touchpad-core` 单元测试 152（M7 95 + M8 57，含 M8_REVIEW R1–R4 回归 9）：tap config 校验（零时长/非正位移/不可能组合/默认禁用与 with_tap）、单 tap click pair、双 tap 两对 click（窗口内与窗口外）、时长/位移边界相等接受严格更大取消、anchor-return 不能复成 tap、取消（第二指/物理按压/discontinuity）、tap-and-drag 首累计 delta 恰好一次、最终 motion 先于合成 up、gap 相等/过期、恰好一指 follow-up、第二指于 tap-drag/于 lock、缺坐标于 tap-drag、drag lock 进入/续拖/重复 reposition/合格 tap 解锁/不合格保锁/`release_all`、序列/时间戳回归保 held、tracking replacement（tap 候选无 click / drag 续锁）、物理/合成仲裁（不重复 down、物理 held 时合成结束不发 up、物理释放后恰好一个 up、聚合 OR 真值表）、指针 commit 胜过 tap、无效帧原子性、新 DiagnosticCode（TapFired/TapAndDragBegan/DragLocked/DragUnlocked）、ArbiterSink 合成事件故障（被拒 tap down / 被拒 tap up / 合成 down 后 motion 失败 / drag-locked cleanup）、**R1** final-Ended 指针 commit 无合成 click（阈值相等 + tap 上限宽于阈值）、final-Ended tap-drag commit 进入 lock 无 up、final-Ended locked continuation 保持 locked、**R2** discontinuity+物理释放同帧恰好一个聚合 up（状态化回归 + release_all 幂等）与三取消原因 × 物理转换的 wire 不变量表驱动、**R3** discontinuity+Began 不能 seeding tap 候选/不能即时 tap-and-drag down/M7 re-anchor 保留/后续新 Began 正常、**R4** follow-up near-`u64::MAX` 边界（相等接受、严格更大过期、deadline 溢出不转态）。
- `touchpad-core` 集成测试 + 14（`tests/m8_arbiter.rs`）：公共 TapConfig 校验、公共 tap 序列与 phase 可观测、公共 lock 续拖/解锁、物理竞争、decision serde 带新字段、ArbiterSink 被拒 tap up 重试、drag-locked cleanup 恰好一次、**R1–R4 公共契约回归 6**（final-Ended 指针 commit 无 click / tap-drag 进入 lock / locked continuation 保锁、discontinuity+物理释放单 up、discontinuity+Began 无 tap/无 tap-and-drag down 且后续新 Began 正常、follow-up near-`u64::MAX` 边界）。
- `touchpad-linux` 集成测试 + 2（`tests/m8_arbiter.rs`）：**replay 派生帧与手工合成帧逐帧决策相等**（m8_tap fixture；证明 trace/replay 与直接合成对 tap 策略走同一 arbiter 路径）、m8_tap fixture 端到端（合格 tap 恰好一对 click + FollowUpWindow）。
- `touchpad-trace`：新 fixture `m8_tap.jsonl`（10 事件）加入 fixtures 契约表。
- 全部测试无硬件、无桌面、无 `/dev/input`、无 portal/libei、无真实输出；确定性合成时间戳，测试不 sleep。

### 19.10 文档化限制（不虚报）

- M8 是离线策略层：未与 M6 输出后端串接（M10 纵向切片范围）；未做任何实机行为验证；不声称桌面 tap 免二次解释。
- 阈值/时长/位移/gap 是显式配置；未做 A/B 校准；KDE/libinput 体验值只作参考，不作为运行时配置依赖。
- 取消后同指（Active）不恢复候选；只有新 Began 接触开启新 interaction（确定性规则，有测试）。
- 回归后合成 held 保持到 `release_all`（fail-closed）；回归后未 `release_all` 前的新接触行为是降级但确定性的，文档要求以 `release_all` 复位。
- `SIGKILL`/内核崩溃/断电不保证用户态 cleanup（保持 §14.9/§17.7/§18.6 限制）。

### 19.11 M8_REVIEW R1–R4 修复事实（2026-08-17）

M8 评审（`reviews/M8_REVIEW.md`）拒绝并要求修复 R1–R4；本小节记录修复后的确定性事实。修复只改 `touchpad-core`（`arbiter` 模块 + 测试），未触碰 M1–M7、公共配置 API、accepted-prefix/cleanup 契约；无新依赖、无 `unsafe`，全部离线。

- **R1（指针 commit 副作用统一）**：新增单一 `ArbiterState::commit_pointer`（commit + `Commit` transition + commit diagnostic + `note_pointer_commit`），active 帧越阈与**final-`Ended` 帧越阈**两个 commit 路径共用它，副作用不可能再分叉。修复后：final-Ended 首 tap 候选越阈只发指针 move（无合成 click pair）；final-Ended tap-and-drag 越阈置 `drag_committed` → 启用 drag lock 时进入 locked-without-contact 无 up；final-Ended locked continuation 越阈保持 locked（tap 上限宽于指针阈值也不会被误判为解锁 tap）。三个精确事件/phase 回归测试均在 motion 阈值**相等**处断言。
- **R2（multiplexer/取消顺序）**：`Arbiter::frame` 改为先捕获真正 pre-frame 源状态并**先应用物理边沿**，再执行 discontinuity/取消前奏与接触策略，使合成转换始终观察到当前物理状态（不再读陈旧快照）。discontinuity + 物理释放同帧 → 恰好一个聚合 up、双源 false、lock 取消、重复 cleanup/release 无 unmatched up；表驱动测试覆盖 discontinuity/额外接触/缺坐标 × 物理 held/释放，断言发射 wire 状态恒等于 post-frame 聚合（debug 与 release 语义一致，`simulate_wire` debug 断言在 debug 构建同样验证）。
- **R3（discontinuity 接触不能 seeding tap）**：新增 `tap_disqualified` 状态：在 `discontinuity=true` 帧上 Began 的接触（含 tracking-replacement 路径）被标记，`begin_tap_family` 对 tap 家族整体拒绝（无 FirstTapCandidate、无 follow-up 即时 tap-and-drag down、无 locked continuation），`tap_candidate_qualifies` 亦防御性排除；M7 指针 re-anchor 完全保留（候选可正常 commit 指针 move）。接触结束/取消（`clear_interaction`）清除标记，之后真正的新 Began 恢复正常 tap 策略。
- **R4（follow-up 过期改为 checked 语义）**：follow-up 窗口过期不再用 `saturating_add` 计算 deadline，改为 `frame_ts.duration_since(completed)` 的 checked elapsed 与配置 gap 比较：相等接受、严格更大过期；`duration_since` 为 `None`（时钟倒退，回归检查已在上游拒绝、实际不可达）时确定性 fail-closed 关闭窗口。near-`u64::MAX` 测试证明相等接受、严格更大过期、以及 nominal deadline 溢出 `u64::MAX` 时窗口仍按 checked 语义保持（不 panic、不转成其它状态转换）。时间戳回归处理未改动。

### 19.12 M19 实机 re-touch / drag-through 修复（2026-08-23）

M19 实机验收暴露：在 KDE 上点击前景窗口时，偶发出现底层桌面图标被拖动。对 `tuning-r3.jsonl` 的只读检查发现真实设备存在非常短的 follow-up contact，例如 tracking id 832 仅持续约 48.5 ms，坐标从 `(1902,817)` 到 `(1900,812)`；按 24 units/mm 分辨率计算位移约 0.22 mm。旧 M8 契约会在 follow-up `Began` 当帧立即合成左 down，即使这段接触远未达到任何 drag motion 阈值，也会短暂暴露 held-left，给桌面焦点/层级切换留下 drag-through 竞态窗口。

当前修复将 follow-up 分成 `TapDragCandidate` 与 `TapDragContact` 两阶段：`Began` 只建立候选；真实 pointer commit 才合成 down，且 down 严格先于首个 committed move。候选期 tracking replacement、取消、缺坐标、竞争 ownership 均零合成 down；候选期干净第二 tap 在 release 帧才输出完整 down/up pulse。新增 `follow_up_tracking_bounce_cannot_turn_a_single_tap_into_drag_through` 回归覆盖“首 tap → follow-up → tracking-id replacement → 后续普通 pointer commit”，断言按钮序列始终只有首 tap 的 click pair、无 synthetic-held 泄漏。该安全修复取代 §19.5 与早期 M8 评审中“follow-up Began 立即 down”的历史行为描述。

## 20. M9 — 双指二维滚动与右键（离线，已实现）

状态：**已批准**（2026-08-17；M9_REVIEW.md Re-review 2 终审通过，R1–R7 全部关闭）。M9 在 M8 已批准（M8_REVIEW.md re-review 2）后按 M9_TASK.md 实施，是纯离线策略里程碑：**不连接 M6 真实后端、不读取/记录/grab 任何物理 `/dev/input`、不调用 portal/libei、不实例化 `PortalDesktopOutput`、不运行任何 live 输入/输出命令**。M9_REVIEW R1–R6 修复事实见 §20.10，Re-review 1 R7 修复事实见 §20.12。

### 20.1 范围与边界

M9 全部实现位于平台无关的 `touchpad-core`（`arbiter` 模块 + 6 个新 `DiagnosticCode`），以及离线测试（`touchpad-core` 单元/集成、`touchpad-linux` replay/synthetic 同路径、两个新 trace fixture `m9_scroll.jsonl` / `m9_secondary_tap.jsonl`）。`touchpad-core` 无任何 Linux/Wayland/KDE/桌面依赖；`arbiter` 模块位于 `#![forbid(unsafe_code)]` crate 内，**无 `unsafe`**，**无新依赖**。

**明确不做（M10/M12/M14+ 及以后）**：momentum/惯性、scroll acceleration/filtering/axis lock、pinch/rotate/swipes/Smart Zoom/翻页/边缘手势、palm/thumb 分类、Force Click、压力、haptics；任何 CLI/daemon/autostart/环境配置改动或 live calibration。M9 不读取 KDE/libinput 设置（其值只是 A/B 参考），不声称 macOS 行为。

### 20.2 配置与默认

- **默认禁用**：`ArbiterConfig::new(...)` 的 two-finger 配置为 `None`，全部 M7/M8 行为（恰两个 live contact 只取消一指 interaction）保持不变；显式 `ArbiterConfig::with_two_finger(TwoFingerConfig)` 才启用双指家族。
- `TwoFingerConfig::new(scroll_enabled, natural, scroll_logical_pixels_per_mm, scroll_commit_threshold_mm, secondary_tap_enabled, two_finger_physical_click_enabled, max_secondary_tap_duration, max_secondary_tap_movement_mm)` 构造时校验：scroll 阈值严格为正（`Millimeters` 类型保证有限）、`max_secondary_tap_duration` 严格为正（零拒绝，不静默修正）、`max_secondary_tap_movement_mm` 严格为正、scale 由 `LogicalPixelsPerMm::try_new` 校验（有限且严格为正）。**M9 范围内 scroll/secondary-tap/physical-click 三个能力互相独立，无结构性不可能的 flag 组合**（与 M8 不同）；所有数值限制仍无条件校验（不静默修正），已在文档与测试中如实说明。
- **能力门控（M9_REVIEW R1）**：每个能力只在对应 flag 启用时活动——`scroll_enabled=false` 时质心运动越过 commit 阈值**绝不打开/发出任何 scroll 生命周期**（candidate 仍可作 secondary tap 候选；`handle_two_finger` 的 candidate anchor 只在 scroll 或 tap 启用时建立；discontinuity 的 relative-scroll re-anchor 只在 scroll 启用时进行）；secondary tap 只在 `secondary_tap_enabled` 时发；buttonpad click 只在 `two_finger_physical_click_enabled` 时 latch。三个能力全关的 `TwoFingerConfig` 完全惰性（无 candidate、无输出），不会仅仅因为存在 `Option<TwoFingerConfig>` 而激活任何能力。**所有权门控（M9_REVIEW Re-review 1 R7）**：candidate anchor 与 scroll commit 另以 `physical_button_ownership_held()`（`physical_left || physical_right || latched_right_owned`）gate——物理按键 held 期间 scroll 绝不被打开/重开（见 §20.9/§20.12）。
- **natural 符号显式**：`natural=true` 输出 scroll delta 与双指质心运动同号（内容跟随手指）；`false` 每轴取反；两轴符号均有测试。M10/M12 可校准 backend 约定，M9 不隐式留号。
- 边界策略（文档化 + 测试）：配置的 duration/distance/threshold **相等即接受**，**严格更大**即过期/取消。只使用 `ContactFrame.monotonic_timestamp` 与 checked duration 运算；不读墙钟/进程本地时钟。超时只在**到达帧边界**求值。

### 20.3 单一 arbiter 与可观测双指阶段

- 仍是**单一 arbiter 作为唯一策略 owner**；无独立 recognizer 可对同一帧各自 commit。
- 新增 `TwoFingerPhase`（`Idle | Candidate | CommittedScroll | PhysicalSecondaryClickHeld | Cancelled | Finished`），可观测于 `FrameDecision.two_finger_phase_after` 与 `Arbiter::two_finger_phase()`。
- **恰两个 complete live contacts** 形成双指候选；第二个有效 contact 出现的帧即 anchor 帧；候选期**零输出**（无 pointer/button/scroll 泄漏）。
- 进入双指家族确定性取消/结束不兼容的一指 interaction（含 tap family）；sticky synthetic-left drag lock 按 M8 aggregate-source 规则释放（恰好一个聚合 left up）后才归双指家族所有，无 double commit。
- 双指 interaction 结束时，剩余 `Active` contact 不得静默变成一指 pointer/tap 候选——只有真正的新 `Began` 边界才开启（确定性规则，有测试）。

### 20.4 几何与滚动语义

- 按 tracking id 识别两个 contact（sorted ids，与 slot/vector 顺序无关；有向量顺序交换回归测试）。**tracking replacement / duplicate identity / unknown Active 不复用旧 anchor/remainder**：replacement 结束 interaction（无 tap；scroll open 时发 `ScrollEnd`），且**同帧不 re-anchor**（后续稳定帧重新 anchor，有测试）。
- 每个 contact 跟踪**自其 anchor 的最大位移**（非仅质心运动），对向 pinch/rotate 运动不能回程冒充 secondary tap（有回归测试）。
- Scroll commit 基于质心自 candidate centroid anchor 的位移：**相等提交、严格低于保持候选**。提交帧发 `ScrollBegin` + 接受累计质心位移**恰好一次**（量化非零轴才发 `ScrollDelta`）；之后每帧增量。逐轴 sub-pixel remainder 与 pointer 同一不变量（`total = rem + d·s`、`emitted = trunc`、`rem ∈ (-1,1)`；`Σ emitted + rem == Σ scaled`）；finish/cancel/release/新 interaction 时清零（many-small vs aggregate 与 remainder reset 有测试）。
- **对角运动双轴一等公民**：不塌缩主轴、不加轴锁；x/y/对角/负向/零位移均有测试。`ScrollDelta` 类型 `LogicalPixels`；zero/zero 不产生事件。
- `ScrollEnd` 恰好一次，条件：掉到 1/0 指、升到 3+ 指、缺必需坐标、tracking replacement、discontinuity、物理点击竞争、`release_all`。`ScrollBegin` 前 / `ScrollEnd` 后无 scroll 事件（有测试）。

### 20.5 双指 secondary tap

- 仅当全部满足：secondary tap 启用、两个初始 contact 有效、未 commit scroll、时长 ≤ 上限、**每 contact 最大位移 ≤ 上限**、无第三 contact/物理点击/discontinuity/error 竞争、interaction 以掉到 2 指以下结束。
- **释放证据（M9_REVIEW R6）**：掉到 2 指以下的第一个边界要按 `Release` 处理（可能发 tap），**至少一个 anchored pair member 必须携带 clean、complete 的 `Ended` 记录**（其最终坐标计入最大位移）；成员只是从帧中消失、无 `Ended` 记录时按确定性 `Cancel("release without Ended record")` 处理——不合成 click。分帧抬起 / 双指同帧 `Ended` 仍正常工作；一个成员干净 `Ended`、另一个消失也仍合格（“至少一个”语义，有测试）。
- 在结束 exactly-two interaction 的**第一个边界至多一次**发恰好 `ButtonDown(Right), ButtonUp(Right)`（分帧抬起也只发一次；剩余 Active/Ended contact 不产生 primary pointer/tap 输出）。
- 过长/过远/对向运动/不完整/取消/discontinuity/scroll-committed 均无 secondary click；两个合格 secondary tap = 两对普通 right click（无发明双击/Smart Zoom）。
- **continuing-cluster 永久 disqualification（M9_REVIEW R2/R3）**：physical button ownership（一指时开始的 primary physical-left press、物理 right、latched press）与**已 commit 的一指 pointer ownership**（发出过 `PointerMove` 后第二指出现）在该 cluster 内**永久** disqualify secondary tap——`begin_two_finger_candidate` 在 anchor 时以 OR 继承（`physical_left || physical_right || latched_right_owned`），`secondary_tap_qualifies` 另在 release 边界防御性要求 `!physical_left && !physical_right && !latched_right_owned`；`handle_contacts` 在 (2+ live, Committed) 取消路径设置 disqualification（仍为 candidate 的一指 interaction 已发零输出，不 disqualify）。第三指/缺坐标/tracking replacement/regression/discontinuity/物理点击等**确定性取消本身**也设置 cluster-level disqualification（`end_two_finger(Cancel)`、`fail_closed_cancel_two_finger`、`handle_two_finger_discontinuity`、`cancel_two_finger_for_physical_press` 的 `_` 分支）。`clear_two_finger_interaction` **不再清除**该 flag；只有 contact cluster **完全排空**（无 live contact 且无 active interaction）才清除——之后真正 fresh 的 cluster 恢复 tap 资格（有逐项回归：第三指→回原两 Active、缺坐标→有效恢复、replacement→稳定 pair、regression→后续单调帧、物理 press→release 后同指、全部排空→fresh pair 正常 tap）。

### 20.6 Buttonpad 双指物理点击（latched）

- physical left 在**恰两个 complete valid fingers** 时 up→down 且策略启用 → **latch 到 Right**：发 `ButtonDown(Right)` 而非 `ButtonDown(Left)`；整个 press 锁定 owner（finger count/contact 变化绝不 remap 回 Left，有测试）；匹配的 physical release 恰好一次 `ButtonUp(Right)`。
- 第二指出现**之前**开始的 press 保持 primary-left（有测试）。
- 双指物理点击取消 secondary-tap/scroll candidate，release 时无合成 secondary tap；被点击打断后的**同指 re-anchor 候选 tap-disqualified**（点击竞争，真实 down 时间/运动未知；相对 scroll 在按键 release 后可继续——held 期间按钮所有权排除 scroll，见 §20.9/§20.12）。re-anchor 由 `begin_two_finger_candidate` 以 OR 语义继承 disqualification，interaction 结束时清除。
- `physical_buttons.right` 作为独立 right source **显式处理**（不静默 alias）：右键聚合 = `physical_right || synthetic_right || latched_right_owned`，聚合 false→true 才 down、true→false 才 up；与 left 复用同一 multiplexer 顺序（physical 边沿先、synthetic/latched 后）与 debug `simulate_wire` 双按钮断言；三源真值表有测试。
- **跨家族同帧顺序（M9_REVIEW R4）**：最终组装不再是“所有 button down 全局在前、所有 up 全局在后”的全局分桶，而是有序 intents：(1) **pre-handoff release**——同一帧内若有一个非 pulse 的 left up（sticky drag lock / drag 结束释放）与 right down（latched 或 physical）同时发生，left up 先发（旧 owner 先关闭，绝不瞬时 Left+Right chord；tap pulse 的 down+up 不是 handoff release，保持 down 后 up）；(2) **old-owner `ScrollEnd` 先于新 physical-button down**（“final delta, `ScrollEnd`, 新 down”）；(3) 其余 button down；(4) owned motion/lifecycle events（pointer move、`ScrollBegin` 先于首 delta）；(5) 其余 up。M8 的 within-owner 不变量（down 先于 drag movement、final movement 先于 matching up）全部保留；同帧精确顺序在 debug 与 release profile 均有回归测试。

### 20.7 失败、清理与 accepted-prefix 交付

- `ContactFrame::validate()` 与原子 draft commit 保留：Error/Fatal 帧不改任何 pointer/tap/scroll/button/timing/baseline 状态且零输出。
- sequence/timestamp regression 仍 fail-closed：open scroll 与 held right/latched **保持对 `release_all` 可见**（有测试）。
- `Arbiter::release_all` 确定性发所需 `ScrollEnd` 与 right/left release **恰好一次**，然后重置全部 M7–M9 phase/anchor/remainder/disqualification/button owner/regression baseline；重复调用为空。
- `ArbiterSink` 扩展为 left/right/scroll 三路 delivered 知识（`delivered_held_left` / `delivered_held_right` / `delivered_scroll_open`）：**rejected `ScrollBegin` 不欠 `ScrollEnd`；accepted begin 后被拒 delta/end 保持 open 且 cleanup 必须关闭；rejected right down 不欠 up；accepted right down 后被拒 up 保持 held 且 cleanup 必须 release**；wrapped `OutputSink::release_all` 仍是权威 ack（成功即对账全部 released/open）；fault/fail-stop/重试语义与 M7 R1–R3 完全保留。
- **cleanup 结构性多失败报告（M9_REVIEW R5）**：`ArbiterSinkError::ReleaseFailed` 新增 `others: Vec<OutputError>` 字段（`primary` 保留为首个失败，其余显式 release 失败按提交顺序进 `others`；wrapped-cleanup 错误仍为 `cleanup`）。`release_all` 现在**逐一**报告每个失败的显式 release（不再 `primary.or(...)` 丢弃后续失败），retry 状态（delivered-held/open）与 wrapped-cleanup 错误原样保留，重试恰好一次重发仍欠的 release。**多失败回归使用合法可达状态（M9_REVIEW Re-review 1 R7 修订）**：M9 可同时欠两个显式 release 的合法路径是**同时按住 physical Left 与 physical Right**——两个独立的 held button source（`primary == Rejected(ButtonUp(Left))`、`others == [Rejected(ButtonUp(Right))]`）；R7 起「held 物理键 + open scroll」不再是合法状态（物理按键所有权排除 scroll 所有权，见 §20.9/§20.12），scroll 的 cleanup/retry 由单独测试覆盖（rejected `ScrollBegin` 不欠 end / accepted begin 后被拒 release 保持 open / rejected `ScrollEnd`）。

### 20.8 测试矩阵（全部离线；workspace 共 739 个测试）

- `touchpad-core` 单元测试 **218**（M8 152 + M9 66）：config 默认禁用/`with_two_finger`/非法值（非正阈值/零时长/非正位移）；候选零泄漏与阈值边界（相等/低于/高于）；累计 delta 恰好一次 + 增量；natural sign 两轴；x/y/对角/负向/零；many-small vs aggregate 与 remainder reset；pair identity 与 vector order 无关；staggered/同帧抬起 secondary tap 恰好一次；剩余 Active 不转 pointer；第三指/缺坐标/tracking replacement 结束（ScrollEnd / 无 tap / 同帧不 re-anchor 且后续稳定帧新 anchor）；tap 时长/位移边界（相等接受、严格更大取消）；对向 pinch 不能假 tap；tap 禁用零输出；两次 secondary tap 两对 right click；scroll 胜过 tap；双指家族胜过一指 pointer（无 double commit）；进入双指时 sticky lock 释放恰好一个 left up；latched right down/up；一指 click 保持 Left；第二指前 press 保持 Left；finger count 变化不 remap；click 取消 candidate 且 release 无 tap；**click 发生在第一个双指帧（候选尚未 anchor）时同样 latch 且 re-anchor 候选 tap-disqualified**；**latch release 后同指 re-anchor 候选保持 tap-disqualified（点击 disqualification 跨 release 存活）**；physical right passthrough 与候选取消；右键三源 truth table；click 结束 committed scroll（ScrollEnd）；discontinuity re-anchor scroll 但无 tap；无效帧原子性；序列/时间戳回归保 scroll open/latch 可见；`release_all` 幂等与 fresh interaction；phase 可观测；decision serde 带新字段；**ArbiterSink fault（rejected begin / rejected first delta after accepted begin / rejected end / rejected right down / rejected right up / Left+Right 双 held cleanup 失败重试与精确 accepted 日志）**。
- **M9_REVIEW R1–R6 新增单元回归（+15，`arbiter.rs`）**：R1 `scroll_enabled=false` 三态（tap 开→无 scroll 且 quick lift 仍 tap；tap 关 click 开→无 scroll 且 latch 仍工作、无候选；全关→完全惰性）；R2 一指 physical-left press + 第二指且 release 边界 Left 仍 held → 无 Right tap、无 Left/Right chord；Left 释放后同指 quick lift 仍无 tap（cluster 级 disqualification 存活）且排空后 fresh pair 正常 tap；committed pointer 后 quick 双指 release 无 tap；未 commit 的 candidate 一指不 disqualify（tap 仍发）；R3 第三指→回原两 Active 稳定→无 tap→排空→fresh pair tap；缺坐标→有效恢复无 tap；tracking replacement→稳定 pair 无 tap；sequence regression→后续单调帧无 tap；R4 物理 right press while scrolling 同帧 [ScrollEnd, RightDown] 精确顺序；sticky drag lock + 双指物理 click 同帧 [LeftUp, RightDown] 精确顺序；R6 无 `Ended` 记录消失→Cancel 无 click；一个成员干净 `Ended` 另一消失→tap 仍发。全部 deterministic 时间戳、无 sleep；同帧顺序测试在 debug 与 release profile 都运行。**M9_REVIEW Re-review 1 R7 新增单元回归（+4，`arbiter.rs`）**：physical Right / physical Left 各两组——pair 形成前 held（候选不 anchor、motion while held 零 scroll、release 帧 [Up]+同指 re-anchor、后续 fresh anchor scroll 正常）与 committed scroll 期间 press（同帧 [ScrollEnd, 新 down] 顺序保留、press 帧 phase 为 Cancelled 不再 re-anchor、held 期间连续 motion 零 scroll、release 后 re-anchor 并 scroll）；每组经 `run_r7` 在**每一帧后**断言 `is_scroll_open()` 与 physical-button ownership（`is_physical_left_held/is_physical_right_held/is_latched_right_held`）不同时成立。
- `touchpad-core` 集成测试 + 23（`tests/m9_arbiter.rs`）：公共 `TwoFingerConfig` 校验与默认禁用；公共 scroll+phase+natural；公共 non-natural 两轴取反；公共 secondary tap；公共 latched click 与 finger count 变化不 remap；一指 click 保持 Left；公共 `release_all`；公共 ArbiterSink fault（rejected end / rejected right up / rejected right down）；decision serde；**M9_REVIEW R1–R6 新增公共回归（+8）**：`scroll_enabled=false` 永不打开 scroll 生命周期且 tap 仍发；physical Left held 于 release 边界无 Right tap；committed pointer 后 quick release 无 tap；第三指取消 disqualify cluster 直到 fresh cluster 才 tap；physical right press while scrolling [ScrollEnd, RightDown]；消失无 `Ended` → Cancel；一个成员干净 `Ended` → tap；`release_all` 双 owed（同时按住 physical Left+Right）时 `primary`+`others` 同时报告两个显式失败且重试恰好一次；**M9_REVIEW Re-review 1 R7 新增公共回归（+4）**：physical Left / physical Right 各两组——pair 形成前 held（motion while held 零 scroll、release 后同指 re-anchor 并 scroll）与 committed scroll 期间 press（同帧 [ScrollEnd, 新 down]、held 期间零 scroll、release 后 re-anchor 并 scroll），每帧断言无同时的 physical-button 与 scroll ownership。
- `touchpad-linux` 集成测试 + 3（`tests/m9_arbiter.rs`）：**replay 派生帧与手工合成帧逐帧决策相等**（m9_scroll / m9_secondary_tap；证明 trace/replay 与直接合成对双指策略走同一 arbiter 路径）、m9_scroll 端到端对角 scroll 生命周期（commit 恰好一次、双轴保留、ScrollEnd 恰好一次、无按钮输出）、m9_secondary_tap 端到端恰好一对 right click（staggered lift 只发一次、剩余 Active 零输出）。
- `touchpad-trace`：新 fixture `m9_scroll.jsonl`（40 事件）、`m9_secondary_tap.jsonl`（16 事件）加入 fixtures 契约表。
- 全部测试无硬件、无桌面、无 `/dev/input`、无 portal/libei、无真实输出；确定性合成时间戳，测试不 sleep。

### 20.9 文档化限制（不虚报）

- M9 是离线策略层：未与 M6 输出后端串接（M10 纵向切片范围）；未做任何实机行为验证；不声称桌面 pixel scroll 免二次解释。
- threshold/scale/duration 是显式配置；未做 A/B 校准；KDE/libinput 体验值只作参考，不作为运行时配置依赖。
- natural 符号是 core 语义约定（`natural=true` 输出与质心运动同号）；M10/M12 可校准 backend 约定，M9 不隐式留号。
- 双指物理点击/物理点击竞争、一指已 commit pointer ownership、以及任何确定性取消（第三指/缺坐标/replacement/regression/discontinuity）后的**同 cluster 的 contact 全部 tap-disqualified**（真实 down 时间/运动未知或已发生竞争所有权）；只有 cluster 完全排空后真正 fresh 的 cluster 才恢复 tap 资格。
- 双指物理点击 release 帧不立即 re-seed 候选（`latched_right_up` 守卫），避免 click 后快速抬起产生伪 secondary tap；下一帧才可 re-anchor（tap-disqualified）。
- **物理按键所有权排除 scroll 所有权（M9_REVIEW Re-review 1 R7）**：aggregate physical Left 或 Right（含 latched physical-left-as-right press）held 期间，双指家族**既不 anchor candidate、也不 commit/发任何 `ScrollBegin`/`ScrollDelta`**，被物理 press 取消的 scroll 在按键仍 held 时绝不在后续稳定帧 re-open（`handle_two_finger` 的 candidate anchor 与 `handle_two_finger_discontinuity` 的 relative-scroll re-anchor 都以 `physical_button_ownership_held()`（`physical_left || physical_right || latched_right_owned`）gate；`update_two_finger_pair` 的 commit 另加同一条件的防御 gate）。**没有合法帧同时暴露 physical-button 与 scroll ownership**（`run_r7` 每帧断言）。按键**干净 release 后**，同一仍在位的 pair 可重新 anchor 相对 scroll（fresh anchor；secondary tap 仍 cluster-disqualified 直到 cluster 排空）；release 帧即 re-anchor 允许（R4 同帧顺序不变：`ScrollEnd` 先于新 physical-button down）。因此 M9 不再存在「held 物理键 + 双指滚动」状态，`ArbiterSink::release_all` 的多失败路径由同时 held 的 physical Left+Right 覆盖（§20.7/§20.12）。
- `SIGKILL`/内核崩溃/断电不保证用户态 cleanup（保持 §14.9/§17.7/§18.6/§19.10 限制）。

### 20.10 M9_REVIEW R1–R6 修复事实（2026-08-17）

M9_REVIEW 判定 REJECTED（708 测试通过但漏掉 ownership/feature-gating 路径），以下为修复事实，全部有精确回归：

- **R1（Critical）`scroll_enabled=false` 被 scroll commit 路径忽略**：`update_two_finger_pair` 的 Candidate 分支在 commit 前检查 `two_cfg.scroll_enabled()`，禁用时绝不开 `ScrollBegin`/发 `ScrollDelta`（candidate 保持、per-contact 位移仍跟踪供 tap）；`handle_two_finger` 的 candidate anchor 只在 `scroll_enabled || secondary_tap_enabled` 时建立；三能力全关的 config 完全惰性；`handle_two_finger_discontinuity` 的 relative-scroll re-anchor 只在 scroll 启用时进行（签名改为接收 `&ArbiterConfig`）。回归：单元 3 个 + 公共 1 个（tap 开 / click 开 / 全关 三态）。
- **R2（Critical）同 cluster 内已有一指/物理 ownership 后仍合成 Right tap**：`secondary_tap_qualifies` 增加 release 边界 `!physical_left && !physical_right && !latched_right_owned` 防御；`begin_two_finger_candidate` 在 anchor 时把 `physical_left || physical_right || latched_right_owned` OR 进 cluster-level `two_tap_disqualified`（物理 Left 在一指时开始的 press 即 primary-left ownership）；`handle_contacts` 的 (2+ live, Committed) 取消路径在 **was_committed** 时设置 disqualification（已发 `PointerMove` 的一指 interaction；仍为 candidate 的不 disqualify）。回归：单元 3 个 + 公共 2 个（Left held 于 release 边界、committed pointer 后 quick release；另加 candidate 一指不 disqualify 的对照）。
- **R3（High）取消过早清除 tap disqualification**：`clear_two_finger_interaction` **不再**重置 `two_tap_disqualified`；`end_two_finger(TwoEnd::Cancel)`（Candidate 与 CommittedScroll 两分支）、`fail_closed_cancel_two_finger`（regression）、`handle_two_finger_discontinuity` 的取消、`cancel_two_finger_for_physical_press` 的 `_` 分支（物理 press 与同帧新 anchor 竞争）都设置 cluster-level flag；清除只发生在 **cluster 排空**（`handle_two_finger` 顶部：无 live contact 且无 active interaction；无 config / 全关 config 的早退路径同样排空）。回归：单元 4 个 + 公共 1 个（第三指→回原两 Active、缺坐标→有效恢复、replacement→稳定 pair、regression→后续单调帧、排空→fresh pair 正常 tap）。
- **R4（High）跨家族 handoff 同帧顺序短暂重叠不兼容 ownership**：最终组装从“全部 down 在前、全部 up 在后”的全局分桶改为有序 intents——(1) pre-handoff left up（非 pulse，且同帧有 right down 时先发，绝不瞬时 Left+Right chord；tap pulse 的 down+up 保持 down 后 up）；(2) old-owner `ScrollEnd` 先于新 physical-button down（“final delta, `ScrollEnd`, 新 down”，press-while-scrolling 帧输出 `[ScrollEnd, ButtonDown(Right)]`）；(3) 其余 down；(4) owned motion/lifecycle events；(5) 其余 up。M8 within-owner 不变量（down 先于 drag movement、final movement 先于 matching up）保留。回归：单元 2 个 + 公共 1 个，均为**精确同帧事件序列**，debug 与 release profile 都运行（`cargo test` 与 `cargo test --release`）。**Re-review 1 R7 起**，press-while-scrolling 帧不再 re-anchor 候选（phase 为 `Cancelled` 而非 `Candidate`，见 §20.12），且「held 物理键 + open scroll」整体不可达——R5 多失败回归改用同时 held physical Left+Right 的合法状态（见 R5/§20.12）。
- **R5（Medium）多个显式 cleanup 失败被结构性折叠为一个错误**：`ArbiterSinkError::ReleaseFailed` 新增 `others: Vec<OutputError>`（`primary` 保留为首个失败、`cleanup` 保留 wrapped 错误）；`ArbiterSink::release_all` 用 `record_failure` 收集**每个**失败的显式 release（left up → right up → scroll end 提交顺序），不再 `primary.or(...)` 丢弃后续失败；retry 状态与 wrapped-cleanup 错误原样保留。公共 API 影响：`ReleaseFailed` 模式匹配需加 `..` 或显式 `others`（见 §20.11）。回归：单元 1 个重写 + 公共 1 个——**Re-review 1 R7 起改用合法可达状态：同时按住 physical Left 与 physical Right**（`primary == Rejected(ButtonUp(Left))`、`others == [Rejected(ButtonUp(Right))]`、`cleanup.is_some()`；重试恰好重发 LeftUp 与 RightUp 一次）；scroll 的 cleanup/retry 覆盖保留在单独测试（rejected begin / accepted begin 后被拒 release / rejected end）。
- **R6（Medium）secondary tap 不要求 anchored pair 的干净 `Ended` 记录**：`handle_two_finger` 的 `0 | 1` live 分支先检查**至少一个 anchored pair member 携带 complete `Ended` 记录**（`two_finger_ids` 匹配 + `ContactState::Ended` + `is_complete()`），有则 `TwoEnd::Release`（`end_two_finger` 已把 Ended 最终坐标计入位移），无则 `TwoEnd::Cancel("release without Ended record")`。回归：单元 2 个 + 公共 2 个（消失无 `Ended` → Cancel 无 click；一个成员干净 `Ended` 另一消失 → tap 仍发；既有 staggered/both-Ended 测试保持通过）。
- 全部修复只改 `touchpad-core`（`arbiter.rs` 单元测试与公共集成测试），无新依赖、无 `unsafe`、无 live 输入/输出；`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo test --workspace --locked` 全部通过。R1–R6 修复后 workspace 共 **731** 个测试（+23：core 单元 +15、core 公共集成 +8）；**Re-review 1 R7 修复后共 739 个（再 +8：core 单元 +4、core 公共集成 +4，见 §20.12）**。**M9_REVIEW.md Re-review 2 终审通过：M9 APPROVED（2026-08-17），M10 可以开始。**

### 20.11 公共 API 影响（M9_REVIEW 修复）

- `ArbiterSinkError::ReleaseFailed` 增加字段 `others: Vec<OutputError>`：现有 `matches!(..., ReleaseFailed { primary, cleanup })` 模式需加 `..` 或显式列出 `others`；`primary`（首个失败）与 `cleanup`（wrapped 错误）语义不变。其余公共 API（`TwoFingerConfig`、`TwoFingerPhase`、`FrameDecision`、`Arbiter`/`ArbiterSink` 方法、`OutputEvent`/`OutputError`）不变；serde 兼容性不受影响（该错误类型不参与 serde）。

### 20.12 M9_REVIEW Re-review 1 R7 修复事实（2026-08-17）

M9_REVIEW Re-review 1 判定 REJECTED——R1–R6 关闭，但 **R7（Critical）** 未修复：物理 Left/Right down 取消双指 candidate/scroll 后，下一个稳定帧仍可 `begin_two_finger_candidate`，且 `update_two_finger_pair` 不阻止 scroll commit，导致 held 物理键与新建 scroll 生命周期共存；Right 情形被 §20.9（旧文）与 R5 双 owed 测试当作合法状态。以下为修复事实，全部有精确回归：

- **物理按键所有权排除 scroll 所有权**：新增 `ArbiterState::physical_button_ownership_held()`（`physical_left || physical_right || latched_right_owned`），用于三处 gate——
  1. `handle_two_finger` 的 candidate anchor（`!interaction_active` 分支）：held 期间**不 anchor 候选**（pair 形成前 held、press 取消后的同帧/后续稳定帧均不 re-anchor）；
  2. `handle_two_finger_discontinuity` 的 relative-scroll re-anchor：同样以 `!physical_button_ownership_held()` gate；
  3. `update_two_finger_pair` 的 Candidate commit 分支：防御性 gate（与 `scroll_enabled=false` 同路径）——held 期间即使存在候选也绝不开 `ScrollBegin`/发 `ScrollDelta`。
  任一 held 期间双指 motion 零 scroll 输出；被 press 取消的 scroll 在按键仍 held 时绝不在后续稳定帧 re-open。**不变式：没有合法帧同时暴露 physical-button 与 scroll ownership**（新 `run_r7` 测试 helper 在每一帧后断言 `is_scroll_open()` 与 `is_physical_left_held/is_physical_right_held/is_latched_right_held` 不同时成立）。
- **确定性的 release 恢复策略（文档化并测试）**：按键**干净 release 后**，同一仍在位的 pair 可**重新 anchor 相对 scroll**（fresh anchor；release 帧即可 re-anchor），secondary tap 仍 cluster-disqualified 直到 cluster 完全排空（R2/R3 语义不变）。R4 的跨家族同帧顺序原样保留：final delta（如有）→ `ScrollEnd` → 新 physical-button down。
- **R5 多失败回归改用合法可达状态**：旧的「physical right held + 双指滚动」双 owed 测试（单元 `sink_cleanup_failure_with_right_and_scroll_retries_exact_logs` 与公共 `public_release_all_reports_both_cleanup_failures`）已重写为**同时按住 physical Left 与 physical Right** 产生两个独立 held button source（fault 来自继续 motion 帧的 rejected `PointerMove`；`primary == Rejected(ButtonUp(Left))`、`others == [Rejected(ButtonUp(Right))]`、`cleanup.is_some()`；重试恰好重发 LeftUp/RightUp 各一次）。scroll 的 cleanup/retry 覆盖保留在单独测试（rejected `ScrollBegin` 不欠 end、accepted begin 后被拒 release 保持 open、rejected `ScrollEnd`）。
- **回归矩阵（R7）**：单元 +4（`arbiter.rs`）：physical Right/Left 各两组——pair 形成前 held（候选不 anchor、motion while held 零 scroll、release 帧 [Up]+同指 re-anchor、后续 fresh anchor scroll 正常；Left 情形另断言 quick lift 仍无 secondary tap）与 committed scroll 期间 press（同帧 [ScrollEnd, 新 down] 顺序保留、press 帧 phase 为 `Cancelled` 不再 re-anchor、held 期间连续 motion 零 scroll、release 后 re-anchor 并 scroll）；公共 +4（`tests/m9_arbiter.rs`）：镜像同一矩阵（Left 期间 press 用 `two_finger_physical_click_enabled=false` 的 config，使 physical-left press 为普通 left press 而非 latch——latch 路径本就由 `PhysicalSecondaryClickHeld` 守卫）。既有 R4 单元回归 `physical_right_press_while_scrolling_orders_scroll_end_before_down` 的 press 帧 phase 断言从 `Candidate` 改为 `Cancelled`（held 期间不再 re-anchor）。
- 全部修复只改 `touchpad-core`（`arbiter.rs` 与 `tests/m9_arbiter.rs`），无新依赖、无 `unsafe`、无公共 API 变更、无 live 输入/输出；workspace 共 **739** 个测试（R1–R6 后 731 + R7 再 +8：core 单元 +4 至 218、core 公共集成 +4 至 23）。`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo test --workspace --locked` 全部通过；同帧顺序回归在 debug 与 release profile 都运行。**M9_REVIEW.md Re-review 2 终审通过：M9 APPROVED（2026-08-17），M10 可以开始。**

## 21. M10 — 限时安全 Takeover 纵向切片（代码已批准，live-unqualified）

状态：**代码已批准（2026-08-17，见 `M10_REVIEW.md` Re-review 1）**；R1–R6 已关闭：poll EINTR 停止再检查（R1）、poll revents 显式分类（R2）、release 前捕获服务器 interruption（R3）、全部五个 takeover flag 拒绝重复（R4）、真实 Left+Right 多显式 release 失败测试（R5）、真实工厂 create 惰性化/零外部副作用（R6）。静态/fake-backed 闸门全部通过。**M10 代码已批准但 live-unqualified**：live 验收 pending 用户记录的 M6 输出校准证据（相对 delta A/B、pixel scroll）与按 `docs/M10_ACCEPTANCE.md` 完成的有序 10 秒 → 60 秒 → 300 秒 `m10-linear-v1` 人工验收；`--output-qualified` 只是 operator attestation。M11 状态见 §22（code-complete / review-approved，仍 live-unqualified）。

### 21.1 范围与边界

M10 串起第一条有界纵向切片：

```text
explicit evdev device (exclusive grab)
  → existing Type-B decoder/resync
  → approved M7–M9 Arbiter/ArbiterSink（m10-linear-v1）
  → prepared portal + libei streaming OutputSink
  → current KDE Wayland desktop
```

新增组件（M10_TASK.md）：

- `touchpad-core::m10`：**`m10-linear-v1`** 命名版本化 bring-up profile（§21.2）。
- `touchpad-linux::bridge`：**`TakeoverBridge`**（§21.5）——infallible `FrameSink` → fallible `ArbiterSink` 的窄桥。
- `touchpad-linux::EvdevRuntime` 兼容扩展：`sink_mut`/`take_sink`/`fd`/`step_deferred`；`Sys` trait 新增 `poll`（M10 有界 loop 的 bounded-readiness seam；Linux 实现位于既有 unsafe FFI 边界，`libc::poll`，含文档化安全不变量；mock 与 TimelineSys 同步实现）。
- `touchpad-desktop::streaming`：**`StreamingOutput`/`StreamingOutputFactory`**（§21.4），`PortalStreamingOutput`（包装 M6 `PortalOutputSink`）与 `RealStreamingOutputFactory`；`fake::FakeStreamingOutput`/`FakeStreamingOutputFactory`/`FakeStreamingState`。
- `touchpadctl takeover`（`cmd::takeover`）：CLI 契约、有界事件循环、统一有序 shutdown（§21.3/§21.6/§21.7）。
- `docs/M10_ACCEPTANCE.md`：人工验收程序（写而未执行）。

M10 明确不做：daemon/fork/后台/autostart/service/持久化/配置写入；acceleration、momentum、palm/thumb、pinch/rotate/swipe、Force Click、pressure、haptics；任何实机输入/输出验证（所有自动测试 fake-backed：不打开/grab 真实设备、不构造真实 portal/libei session、不发射桌面输入、不 sleep、不修改系统设置）。

### 21.2 `m10-linear-v1` profile（`touchpad-core::m10`）

`M10Profile`（`M10Profile::NAME == "m10-linear-v1"`）把每个 M7–M9 参数做成 typed/finite/validated（经 `ArbiterConfig::new`/`TapConfig::new`/`TwoFingerConfig::new`/`LogicalPixelsPerMm::try_new`）并文档化：

| 参数 | 值 | 说明 |
| --- | --- | --- |
| one-finger commit threshold | 1.0 mm | M7 线性指针（相等接受、严格更大提交） |
| one-finger scale | 10 px/mm | 线性映射，无加速度（M11 拥有加速度） |
| tap / tap-and-drag / drag-lock | 全启用 | M8：tap 180 ms、movement 3.0 mm、follow-up gap 350 ms |
| two-finger 2D natural scroll | 启用，natural=true | M9：10 px/mm、centroid commit 1.0 mm |
| secondary tap | 启用 | 300 ms、per-contact movement 3.0 mm |
| buttonpad two-finger click | 启用 | M9 latch 策略 |

这是保守 bring-up profile，**不是 macOS 等价声明、不是生产默认**；运行时不读取/复制 KDE/libinput 值（系统行为仅是人工 A/B 基线）。

### 21.3 CLI 契约（`takeover`）

```text
touchpadctl takeover DEVICE TRACE --takeover --confirm TAKEOVER
  --output-qualified --profile m10-linear-v1 --max-duration-seconds N
```

- `DEVICE`/`TRACE` 为必填显式路径；每个 opt-in 必填且独立校验：`--takeover`、精确确认文本 `TAKEOVER`、`--output-qualified`（**operator attestation，非测量证据**；未记录 M6 校准表前不得诚实通过）、`--profile`（仅 `m10-linear-v1`）、`--max-duration-seconds N`（整数 `1..=300`；零/溢出/缺失/重复/无限形式全部拒绝）。
- 未知/重复 flag、其他命令使用 takeover-only flag → usage error（任何设备/输出副作用之前）。
- 无 daemon/fork/后台/autostart/service/持久化/系统设置写入。
- help 必须声明：grab 物理触控板、发射真实桌面输入、打开 portal 授权提示、记录 raw 输入、experimental、需要外部键盘/鼠标与第二终端 `SIGTERM` 逃生、`SIGKILL`/内核崩溃/断电不保证 cleanup。
- `record --grab` 保持独立显式；既有命令行为不变。
- `command_needs_termination_handler` 纳入 `takeover`（安装既有受控 SIGINT/SIGTERM handler）；dry-run/非 live 命令不扩大信号行为。

### 21.4 Streaming output boundary（`touchpad-desktop::streaming`）

- `StreamingOutput: OutputSink`：`prepare(cancelled)`（与 M6 完全一致的 cancellable/bounded）、`capabilities()`、`state()`、`take_server_interruption()`、`take_cleanup_error()`。`Box<dyn StreamingOutput>: OutputSink` 桥接实现使 `ArbiterSink` 可持有 trait object。
- `StreamingOutputFactory::create()`：**纯对象分配、零外部副作用（M10 review R6）**——真实工厂把 session bus 连接（`ZbusPortal`）与 libei dlopen（`Libei::load`）**惰性化**到 `prepare()`（经 `LazyPortal`/`LazyTransport` 包装，首次使用/首次 connect 才执行外部工作）；真正的输出准备——portal session/授权/EIS 握手——发生在 `prepare()`，设备 open 失败时**零 D-Bus/libei/output 访问**且保留 device-error 退出优先级。对象分配与外部准备在文档/测试中显式分离（可观测的 factory/preparation 时间线测试）。
- `PortalStreamingOutput` 包装已 review 的 M6 `PortalOutputSink`，`submit` 保留同步 accepted/rejected 语义；`release_all` 幂等（显式语义 release → transport disconnect → portal session close）；服务器 pause/removal/disconnect 是终态输出 fault（首个被拒语义事件后不再有 wire 输出，结构化 interruption 保留）。**不构造虚拟 touchpad、不转发 raw contacts/finger count**。
- 生产用真实工厂；测试注入 fake session/factory，绝不连接 D-Bus/Wayland/portal/libei。M6 的固定 `--emit` pattern 不是 streaming API，绝不被 takeover 重放。

### 21.5 Fallible frame bridge（`touchpad-linux::bridge`）

`TakeoverBridge<S: OutputSink>` 实现 `FrameSink`（infallible `on_frame`），内部驱动 `ArbiterSink<S>`：

- 存储**首个** arbiter/output failure（`fault: Option<ArbiterSinkError>`），立即停止接收语义工作；同批已读 evdev 事件中的后续帧全部忽略并计数（`frames_ignored_after_fault`）——no-late-output 规则。
- `stopped` 为 sticky fail-stop 标志：`take_fault()` 取出 fault 供协调器上报后，bridge 仍拒绝后续帧。
- `release_all()` 委托 `ArbiterSink::release_all`（accepted-prefix/faulted 语义保留：cleanup 恰好提交仍欠的 release）；`sink_mut()` 供准备期访问；`into_parts()` 返回 (Arbiter, S)。
- 不静默 log-and-continue；主 fault 不被通用 decoder error 替换。

### 21.6 准备顺序与有界事件循环（`cmd::takeover`）

**grab 是最后一步**：parse（零副作用）→ create session 对象（构造）→ open/validate device（不 read、不 grab）→ `prepare()` streaming session（cancellable；要求 relative pointer、primary/secondary button、pixel-precise scroll，缺能力在 recorder/grab 之前拒绝）→ m10-linear-v1 arbiter pipeline（bridge 即 decoder sink）→ recorder create + header flush（证明输出可写）→ attach → 打印 device/trace/profile/capabilities/duration/cleanup 顺序/逃生说明 + 可见可取消 ≥3 秒倒计时 → 重查 stop/readiness → 恰好一次 `EVIOCGRAB(1)` → 有界循环。step 7 前任何失败/取消：**零 grab、零语义桌面事件**；已准备 session 显式 release、已 open device/recorder 按有序路径 finalize/close，诊断全保留。

**有界循环**（§7）：`POLL_QUANTUM = 100ms` 固定量子——loop 经可注入 readiness seam 唤醒，检查注入 monotonic clock（deadline）、signal stop、bridge fault，然后仅在 ready 时 `step_deferred()`。即使触控板完全无输入，最大时长也会到期；grab 超时最多超出配置时长一个 poll quantum（deadline 在每个 poll 前检查）。测试用 fake clock/sys、零 sleep。`step_deferred`（runtime 兼容扩展）在 fatal stream/decoder/recorder 错误时停止新工作但**保留 output/recorder/grab/fd** 给协调器统一 shutdown（M4/M5 的即时 fail-open 会先于虚拟输出 cleanup 释放设备，M10 不得如此）；`Drop` 仍只是 best-effort 兜底。

**Readiness 分类（M10 review R1/R2）**：`Sys::poll` 显式分类 revents——`POLLIN`/`POLLHUP`/`POLLERR` → ready（read 会推进：数据，或 unplug/failed fd 的真实 EOF/error，loop 立即 read 而非等到 deadline）；`POLLNVAL` → 立即结构化错误（fd 无效，绝不 idle）；纯 timeout → idle。`poll(2)` 返回 `EINTR` 时 loop **再检查两个 stop 源**：已请求 stop（非 `SA_RESTART` handler 下正常的 Ctrl-C/SIGTERM 打断 idle poll）→ 受控 `Signal` 停止（clean，exit 0）；未请求的 EINTR → 结构化 poll/stream 失败（exit 6），绝不误分类。fake/mock 与 FFI 分类函数均有逐 flag/组合单元测试。

### 21.7 统一有序 shutdown（§8）与退出码

每个 post-preparation 退出（deadline、SIGINT/SIGTERM、output/arbiter fault、portal revocation、EOF/unplug、poll/read error、decoder degraded/resync failure、recorder failure、grab failure、status-writer failure、panic fallback）收敛到幂等协调器 `finalize`：

```text
1. 停止接收 raw/semantic work
2. ArbiterSink::release_all：释放欠的虚拟 Left/Right 与 scroll lifecycle，
   然后 wrapped portal sink disconnect + close session
3. finalize/destroy recorder（finish 结果保留）
4. EVIOCGRAB(0) 至多一次
5. 即使 ungrab 失败也恰好 close 一次 fd
```

pre-grab 失败对存在的资源按同序执行，未 grab 则零 ungrab ioctl；重复 shutdown 全 no-op。`TakeoverCleanup` guard 的 `Drop` 对 early-return/unwind 执行同序 best-effort 兜底（output release → recorder → ungrab → close）。结构化 outcome 保留主停止原因与**全部** cleanup 失败（每个显式虚拟 release、wrapped output cleanup、recorder finish、ungrab、close、status-output failure）。

**服务器 interruption 捕获顺序（M10 review R3）**：真实 `PortalOutputSink::release_all_detailed` 会在 release 期间**清空**其 interruption，因此协调器必须在 `release_all` **之前**经 `sink_mut().take_server_interruption()` 捕获结构化 interruption（device pause/removal、seat removal、disconnect），否则真实 interruption 会丢失并被展平成通用 semantic-output 失败；fake 生命周期与真实 adapter 对齐（release 清 interruption），并有 `PortalStreamingOutput<FakePortal, FakeTransport>` 测试证明类别在真实 release 行为下存活。`take_cleanup_error()` 由协调器消费（非死访问器），但只在 arbiter 级 release 成功时才单独上报——`ArbiterSinkError::ReleaseFailed.cleanup` 已携带 wrapped cleanup 错误时绝不重复（类别不丢失、不重复）。

退出码优先级（确定且文档化，见 help/README/M10_ACCEPTANCE）：recorder finalize（7）> output release（7）> device release（6）> status-output（9）> 主因——deadline/signal 且全部 cleanup 成功 → **0（clean）**；countdown 取消（grab 前）→ 8；stream/grab/output fault → 6；output prepare → 按类别 2/3/4/5（服务器 interruption 经 `take_server_interruption` 保留 M6 一致的 transport 类别 5）；recorder preflight → 7。绝不宣称 SIGKILL cleanup。

### 21.8 测试矩阵（M10；全部 fake-backed，workspace 共 792 个测试，M10 新增 53：core +3（m10 profile）、linux +8（bridge 4 + runtime step_deferred 2 + sys poll 分类 2）、desktop +4（streaming 2 + R3 生命周期 1 + R6 时间线 1）、touchpadctl +38（args 10 + cmd/takeover 26 + cli 集成 2））

- `touchpad-core`：`m10` profile 单元（构造/参数/`arbiter_config`/NAME）；`ArbiterSink::sink_mut`。
- `touchpad-linux`：`bridge` 单元（顺序转发、首 fault 存储 + sticky stopped、partial fault 后 release 恰好欠的 state、`sink_mut`）；runtime `step_deferred`（fatal 后 Stopping + 资源保留 + `shutdown` 恰好一次 ungrab/close + 重复 shutdown no-op；decoder failure 保留 recorder 给 finalize）；`Sys::poll`（mock 语义 + FFI 实现 + **revents 分类逐 flag/组合单元测试：POLLIN/HUP/ERR ready、NVAL 结构化错误、组合、timeout idle，R2**）。
- `touchpad-desktop`：`streaming` 单元（`PortalStreamingOutput` 委托 M6 sink：prepare/submit/release/能力/interruption；prepare 失败释放；**R3：服务器 interruption 必须在 release 前捕获（真实 release 行为清空 interruption）**；**R6：惰性 factory 可观测时间线——create 零外部工作、prepare 恰好一次外部工作**）；`FakeStreamingOutput/Factory/State`（**release 生命周期与真实 adapter 对齐：清 interruption、失败才留 cleanup_error**）。
- `touchpadctl`：args 单元（每个缺失/重复/非法 opt-in；duration 0/1/300/301/malformed/overflow；takeover-only flags 到处拒绝；**R4：全部五个 flag 重复拒绝，含相同/冲突值**；help 全部强制警告）；`cmd::takeover` 集成（成功启动时间线 open→prepare→recorder flush→countdown→grab→read→deadline→ordered cleanup；idle device 1s/300s 边界；signal during loop；**R1：poll EINTR + 已请求 stop → clean signal stop；未请求 EINTR → 结构化 poll 失败**；**R2：unplug/HUP 立即唤醒 loop 并有序 cleanup 不等 deadline；POLLNVAL 立即结构化失败**；countdown cancel；device-open failure 零 output-prepare/零 grab（**R6：create 分配一次、prepare 外部工作零次、device-error 优先级保留**）；output prepare failure 释放 session 关 device 零 grab；capability missing 在 recorder/grab 前拒绝；recorder create/flush failure 输出先于 device close 零 grab；status-writer failure 有序 cleanup；EOF/unplug、readiness error、timestamp regression、SYN_DROPPED resync failure、recorder event failure、grab failure；首 output rejection 后同批零后续输出 + cleanup 恰好欠的 state；server interruption 结构化 fault（**fake 清 interruption 后仍经捕获顺序存活**）；**R5：合法同时按住 physical Left+Right——两个 down 接受、两个显式 cleanup up 分别拒绝（primary+others）、wrapped cleanup 失败、recorder finish/ungrab/close 失败，全部诊断与优先级（exit 7）**；**R5 单独 success/retry 幂等用例：欠的 up 恰好释放、重复 finalize 全 no-op**；fallback Drop 有序兜底；trace/replay parity；七手势全管线 decoder→arbiter→output 精确语义流、零 raw 泄漏）；cli 集成（takeover parse failure 零副作用；**R4 公共路径：全部五个 flag 重复为 usage error（exit 1）**）。
- **测试真实性**：没有任何测试打开/grab 真实设备、创建真实 portal/libei session、发射桌面输入、sleep、或修改系统设置；唯一真实 OS 表面为既有无副作用检查（sigaction/raise、不存在路径、libei dlopen probe、session bus 可达性）。

### 21.9 文档化限制（不虚报）

- **live-unqualified**：M6 输出校准（相对 delta A/B、pixel scroll）未由用户记录前，`--output-qualified` 只是 operator attestation 而非测量证据；backend 保持 `experimental/unqualified`；M10 代码已批准但 live 验收 pending，且 M10 验收**不**构成 M11 的 live 资格（M11 见 §22）。
- **不保证的清理**：`SIGKILL`、内核崩溃、断电不能运行用户态 cleanup；内核在 fd 关闭时释放 evdev grab，但有序序列只在可运行用户态代码的路径上有保证。
- 单设备、foreground、有界（1–300 秒）；无后台/自启。
- **真实工厂惰性化（M10 review R6）**：`RealStreamingOutputFactory::create()` 零外部副作用；session bus 连接与 libei dlopen 在 `prepare()`（设备 open 成功之后）经 `LazyPortal`/`LazyTransport` 发生。对象分配与外部准备分离，可观测时间线测试证明；设备 open 失败零 D-Bus/libei/output 访问且保留 device-error 优先级。
- `Sys::poll` 的 Linux 实现位于既有 `sys::ffi` unsafe 边界（libc::poll，文档化安全不变量）；其余新模块 `#![forbid(unsafe_code)]`。

## 22. M11 — 实验性一指 Pointer Fidelity（`m11-fidelity-v1`；code-complete，live-unqualified）

状态：**M11 已由 `M11_REVIEW.md` Re-review 1 批准为 code-complete**。原 R1–R4 已关闭，最终全量闸门 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo test --workspace --locked`、`cargo test --release --workspace --locked` 均通过。该批准严格属于离线/fake-backed 代码层；**M11 仍 live-unqualified**，未来独立 M11 专用 live acceptance 尚未执行。

M11 是 **experimental、opt-in only、永非默认**，且**不声称 macOS 等价**（M11_TASK.md）。它在已归一化毫米输入上增加一指 pointer-fidelity 阶段（signed radial jitter dead-zone、单调时域速度估计、有界连续 smoothstep gain、tracking multiplier），叠加在已批准 M7–M9 策略与 `m10-linear-v1` 之上；候选 commit/tap/scroll 所有权仍基于 pre-fidelity 原始毫米行为。

### 22.1 已就位的实现事实（全部离线/fake-backed）

- **`touchpad-core::fidelity`（新模块）**：typed/finite/validated `FidelityConfig`（dead_zone_radius 0.09 mm、velocity_tau 20 ms、long_gap 150 ms 含边界、gain_x0/x1 50/600 mm/s、min/max gain 1.0/2.0、base_px_per_mm 10.0、tracking_speed 1.0；`FidelityConfigError`）、`FidelityState`（last_sample_timestamp、signed P/V_pending/t_acc、filtered_velocity）、`FidelityOutcome::{Hold, EmitScaledPixels, Reanchored}` 与纯 `process(config, state, delta_mm, timestamp)` 阶段 API：首调用锚定时间戳、整段位移折入 P、以 min_gain 保持；`dt == 0` 折叠进 P/V_pending、不除不估、不评估 dead-zone；正 dt 更新速度并评估 dead-zone；`dt >= long_gap`（含边界）丢弃跨隙位移、清状态、re-anchor 并发射零。`FidelityError` 只覆盖非有限/溢出，Arbiter fail-closed 映射为 `ArbiterError::NonFinite`。
- **`M11Profile`（`touchpad-core::m11`）**：`M11Profile::new()` 从 `M10Profile::new()?.arbiter_config()` 继承**全部** M7–M9 配置（不复制常量、不改 `m10.rs`），仅 `.with_fidelity(...)` 添加 fidelity；R4 修复后 `FidelityConfig::new(...)` 以 `M11ProfileError::Fidelity` 传播，**无 panic 路径**。
- **原子状态**：`FidelityState` 存于 `ArbiterState` 内（非 Arbiter 独立可变字段）；`Arbiter::frame` 把它复制进既有 frame draft，随整份 `ArbiterState` 原子提交，被拒帧回滚全部 fidelity 状态；`Arbiter::remainder_px()` 暴露 committed fidelity remainder，无第二/隐藏 remainder 累加器；生命周期 reset 经既有状态 reset 清空。
- **M10-disabled 路径不变**：fidelity 默认 `None`（`ArbiterConfig` 的 `Option<FidelityConfig>`），禁用时走既有线性量化分支、不经过 M11 fidelity 逻辑（`emit_pointer_delta` 为窄开关）；M10 decisions 保持输出兼容（fidelity-disabled 回归 fixture 逐帧相等）。
- **CLI**：`--profile` accepted set **恰为** `{m10-linear-v1, m11-fidelity-v1}`（`m10-linear-v1` mention-first，缺省不推断）；M11 不加 flag；`select_profile` 纯 helper（不进入 `takeover::run` 副作用即可测）与显式实验 banner（experimental/uncalibrated、非默认、无 macOS 等价、无 live M11 验证、M10 安全 opt-in 与 1..=300 时长界仍适用），banner 在任何设备/输出/recorder/倒计时/grab 副作用之前打印。
- **trace/replay 覆盖**：确定性 fixture `crates/touchpad-trace/tests/fixtures/m11_fidelity.jsonl`（first commit、低/高速运动、duplicate timestamp、reversal、对角、exact/over-long-gap、clean end、fresh interaction），`touchpad-linux/tests/m11_arbiter.rs` 证明 replay 派生帧与手工合成帧逐帧产生**相同** M11 decisions（direct-vs-replay equality）。
- **`docs/M11_ACCEPTANCE.md`**：已书写（future user-run procedure：M6 校准证据前置、M10 有序验收前置、8 个有界 staged live 阶段、fail-open/stop 标准、non-claims），**尚未执行**——无任何 live 运行。

### 22.2 资格边界（精确）

- **M10**：代码已批准（M10_REVIEW.md Re-review 1），但 **live-unqualified**——pending 用户记录的 M6 校准证据（相对 delta A/B、pixel scroll）与有序 **10 → 60 → 300 秒 `m10-linear-v1`** 验收（`docs/M10_ACCEPTANCE.md`）；`--output-qualified` 仍是 operator attestation，非测量证据。
- **M11（代码）**：`M11_REVIEW.md` Re-review 1 已批准，四项最终全量闸门均通过，**code-complete / review-approved**。M10 验收**不**赋予 M11 live 资格。
- **M11（live）**：保持 **live-unqualified**，直到独立的、将来的 M11 专属用户验收被书写并通过（`docs/M11_ACCEPTANCE.md` 即该程序，未执行）。
- **M12–M16（代码）**：scroll fidelity/momentum、contact robustness、连续手势、三指拖动/KDE action adapter 边界、versioned runtime config/reconnect/service lifecycle/capability matrix 已实现并通过最终 workspace debug/release gates；见 `reviews/M12_REVIEW.md`、`reviews/M13_REVIEW.md`、`reviews/M14_REVIEW.md`、`reviews/M15_REVIEW.md`、`reviews/M16_REVIEW.md`。
- **M12–M16（live）**：全部保持 **live-unqualified**。M16 的 `m16-production-v1` 只表示配置/运行维护代码完整，不表示生产资格、跨设备资格或 macOS 等价；`config-check`/`service-preflight` 不启动服务，X11/uinput 仍需独立实现与资格测试，pressure/haptics 仍 unsupported。

## 23. M12–M16 Phase-2 收口记录

- **M12**：两指 scroll time-domain velocity、smoothstep gain、axis-lock hysteresis、reversal handling、software momentum；`Arbiter::tick` → `ArbiterSink::tick` → `TakeoverBridge::tick`，momentum 活跃时 bounded loop 使用 16 ms quantum。新 contact/button/discontinuity 先取消 momentum，clean release 才可保持 scroll lifecycle 进入 momentum。
- **M13**：feature-aware palm/thumb/edge/typing/jitter robustness。classifier 只使用实际 Contact 特征，缺特征明确 fallback；sticky 状态按 tracking-id 存储于 atomic Arbiter draft。CIRQ1080 profile 只记录已观察到的 buttonpad quirk，不把设备常量写进 generic algorithm。
- **M14**：platform-neutral continuous gesture Begin/Update/End 与 pinch/rotate/page swipe/3-4 finger swipe/edge/thumb+3 recognizer；与 M12 scroll 使用单一 ownership 竞争。当前 M6 sink 对 native continuous event 明确 `Unavailable`。
- **M15**：three-finger drag/drag-lock/three-finger tap semantic action；drag motion 复用 pointer fidelity/remainder。KDE action mapping 位于 desktop adapter，可发现/remap/disable，真实 KDE transport 默认未启用。
- **M16**：strict config v2 + v1 migration、独立 device/output bounded reconnect controller、foreground service lifecycle、capability matrix、`config-check`、`service-preflight`、`m16-production-v1`。persistent/autostart 配置被明确拒绝，X11/uinput 无 silent fallback。

## 24. M17 手感参数层

- **定位**：M17 不扩张 safety/service 配置面，而是在 M16 之上增加独立、strict、versioned 的 `FeelConfig v1` overlay。CLI 与 GUI 使用完全相同的 JSON schema；默认 overlay 经测试构造出的 Arbiter config 与 `m16-production-v1` 完全相等。
- **可调范围**：pointer dead-zone / tracking speed / low-high gain；scroll low-high gain / axis-lock hysteresis / momentum decay-start-stop；pinch/page/multi-swipe commit threshold；three-finger drag commit threshold 与 drag-lock。每个数值都有显式安全编辑范围和 cross-field validation。
- **ownership 约束**：`drag.commit_threshold_mm < gesture.multi_swipe_commit_mm` 是 schema invariant，防止调参破坏 M15 three-finger drag 先于 M14 multi-swipe 的裁决优先级。
- **明确不开放**：M10 takeover confirmation/grab/duration/cleanup，M6 output qualification，device quirks/normalization，M8/M9 tap timing，M16 reconnect/service/autostart，X11/uinput，pressure/haptics 均不进入 feel editor。
- **CLI**：`feel-default`、`feel-check`、`feel-show`、`feel-set`、`feel-gui`。`feel-set` 先在内存中完成全部修改并通过整体校验后才写输出文件。
- **GUI**：`feel-gui` 生成单文件 offline HTML，包含 slider/number input、constraint check、JSON preview/export；无外部资源、network、server、device access 或 live-apply。
- **live 路由**：只有显式 `--profile m17-tunable-v1 --feel-config FILE` 可读取 tuning overlay；M17 缺文件直接 parse error，M10–M16 携带该 flag 也直接 parse error。文件在 output/device/recorder/grab 副作用之前 strict load/validate。
- **资格**：`reviews/M17_REVIEW.md` 已批准 code-complete；最终 fmt/clippy/debug/release workspace gates 全通过。手感本身仍 **live-unqualified**，需按 `docs/M17_ACCEPTANCE.md` 做独立 10/60/300 秒 A/B 验收，不声称 macOS 等价。

## 25. M18 可配置 Gesture → Action 映射

- **统一用户设置**：新增 strict `UserSettings v1`，组合 M17 `FeelConfig` 与
  `GestureMapConfig v1`。CLI/GUI 不再要求用户手工维护彼此无关的 tuning 与
  gesture 文件；`settings-default` / `settings-macos` / `settings-check` /
  `settings-show` / `settings-set` / `settings-patch` / `settings-gui` 均读写同一
  schema。
- **closed typed mapping**：gesture target 只能是 `passthrough`、`disabled` 或
  既有 `DesktopAction` 闭合集合；没有 command string / shell execution。方向化
  trigger 包含 pinch、rotate、two-finger page、three/four-finger swipe、edge、
  thumb+three 与 three-finger tap。
- **single-fire semantics**：mapped continuous gesture 在 Begin 时发恰好一个
  `DesktopAction`，同一 gesture 的 Update/End 被 route state 抑制；passthrough
  则保留完整 M14 continuous stream，disabled 消费且无 action。
- **M17 默认兼容**：默认 gesture map 对 continuous gesture 全部 passthrough，
  three-finger tap 保持 M15 的 Lookup；默认
  `three_finger_drag_enabled=true`，因此未选择新 preset 的用户保持 M17/M15
  ownership 行为。
- **review R1 — three-finger drag/swipe 冲突**：第一版 mapping 虽能配置三指
  swipe，但 M15 drag commit 阈值小于 M14 multi-swipe，drag 会先赢 ownership。
  最终设计把 `three_finger_drag_enabled` 作为显式 gesture setting：macOS-inspired
  preset 设为 false，仅禁止 drag commit，仍保留 three-finger tap candidate；
  integration test 证明 three-finger swipe-up 实际到达 `OpenOverview` 且无合成
  left-down；另外的显式 remap 测试证明 drag commit 关闭后 three-finger tap
  candidate 仍可正常映射。
- **macOS-inspired preset（M19 KDE revision）**：three-finger horizontal →
  workspace；three-finger vertical → overview/present windows；thumb+three →
  launcher/show desktop。M19 真实 KDE transport 尚未支持的 page、notification、
  lookup 与 native continuous passthrough 默认 disabled。该 preset 只描述布局，
  不声称 macOS 等价。
- **takeover gate**：`m18-remap-v1` 必须显式 `--settings FILE`；M17-only
  `--feel-config` 和 M19-only `--watch-settings` 被拒绝。settings 在任何真实
  output/device/recorder/grab 副作用之前 strict load/validate。
- **desktop transport 边界**：M18 profile 本身仍只完成 recognition → mapping →
  typed `DesktopAction`；M19 后续在 production backend 中增加真实 KDE action
  transport，不回写 M18 的 live 资格。
- **资格**：`reviews/M18_REVIEW.md` 已批准 code-complete；review R1 修复后四项
  workspace gates 重新从头全部通过。M18 仍 **live-unqualified**。

## 26. M19 安全实时 Settings Hot Reload

- **profile/opt-in**：`m19-live-v1` 继承同一份 M18 `UserSettings` / gesture
  policy，并增加 runtime reload；同时针对实机交互修订单指 tap-and-drag：
  一次 tap clean release 后在 **180 ms** 内 arm 下一次单指接触；该 follow-up
  真正越过 pointer threshold 时按 `ButtonDown(Left) → PointerMove` 开始拖拽。
  严格晚于 180 ms 的接触按普通 pointer 处理。该窗口与当前 libinput
  single-finger tap-and-drag timeout 对齐。M19 同时关闭 M8 遗留的**单指 sticky
  tap-drag lock**，所以 clean `Ended` 帧立即发 `ButtonUp(Left)`，不再进入
  `LockedWithoutContact`。tap duration / movement 保持原值，
  M10–M18 历史 profile 保持原行为。CLI 除全部既有
  M10 opt-in 外，必须显式提供
  `--settings FILE --watch-settings`；watch 不会由 profile 名或文件存在自动推断。
- **三指拖拽 fidelity 分离**：M19 为 committed three-finger drag 安装独立的
  pointer-fidelity profile。dead-zone、velocity curve 的低速端、base scale 与
  tracking speed 继承普通 pointer，但 drag-only `max_gain` 封顶为 **1.6**；普通
  一指 pointer 仍使用用户配置（当前完整配置为 `tracking_speed=1.25`、
  `max_gain=2.9`）。`tuning-r6.jsonl` 的三指输入约 165 Hz，稳定三指段单帧
  centroid 位移最高约 1.43 mm，而当前显示为 120 Hz；旧共享高速曲线理论上
  可在一次输入帧产生约 52 logical px，形成硬件 cursor 相对 compositor drag
  item 的明显上一方向领先。drag-only ceiling 将同一极端帧降到约 29 px，且不
  降低普通 pointer 的高速手感。
- **M19 stable-reference motion**：`tuning-r8.jsonl` 证明每轮新拖拽的 commit
  前 centroid 位移会沿当前拖拽方向累积；当用户来回拖同一个图标时，该向量
  自然与上一轮总拖拽向量反向。旧 M15 `BeginDrag` 会在 synthetic press 后把
  这段 classification displacement 一次性补发，直接解释“新拖拽起点沿上一
  拖拽反方向偏移”。对照 `linux-3-finger-drag` 后，M19-only config 启用 stable
  reference 模式：centroid 仍只负责 commit 判定；commit 帧变成 `ArmDrag`，
  丢弃 pre-commit displacement 并选定一个 tracking-id reference baseline；后续
  只从该 reference 的增量生成 `Move`。reference 先抬起时从仍存活的原始接触
  重新选择并 baseline，切换帧零 motion，避免 finger-count/绝对坐标 jump。
  synthetic Left 也推迟到第一次真正产生 PointerMove 的帧才建立。M15–M18 的
  legacy centroid/replay path 保持不变。
- **M19 three-finger release boundary**：stable-reference drag 一旦 commit，干净
  的 `3→2→1→0` 都继续由该 drag 独占；reference 存活时可以继续按 reference
  增量移动，reference 被抬起则零位移 re-baseline，任何剩余原始 contact 都不
  暴露给 one/two-finger policy。原始 cluster 完全为空时才唯一 `EndDrag` /
  `ButtonUp`。新增/replacement tracking id 仍 fail-closed release。
- **libei logical-frame 对齐**：core 的一个 `ContactFrame` 可产生多个 semantic
  `OutputEvent`，但真实 portal/libei backend 不再把 drag ownership 边界逐事件
  各自 `ei_device.frame`。`ButtonDown + first PointerMove` 与
  `final PointerMove + ButtonUp` 在同一个 EIS logical hardware frame 提交；tap
  pulse 的 `ButtonDown + ButtonUp` 仍拆成两个 frame。`OutputSink::submit_frame`
  保留 accepted-prefix/fail-stop 报告，默认 fake/旧 sink 仍逐事件提交，因此该
  改动不弱化 M10/M9 cleanup 与故障语义。
- **真实 KDE Plasma 6 输出**：production M19 使用 composite session。pointer /
  button / pixel-scroll 继续走 M6/M10 RemoteDesktop portal + libei；离散
  `DesktopAction` 走 session-bus `org.kde.kglobalaccel.Component`。真实 transport
  对 component 的 `shortcutNames()` 做只读 preflight，执行时只调用
  `invokeShortcut(action_id)`；无 shell/任意命令执行。当前 closed support set 为
  next/previous workspace、Overview、Present Windows (`Expose`)、Show Desktop、
  Application Launcher。
- **capability-before-grab**：真实 M19 初始 settings 若含 notification center、
  page next/previous、smart zoom、lookup 或 native continuous passthrough，会在
  grab 前被明确拒绝；KGlobalAccel required shortcut 也在 portal prepare/grab 前
  只读验证。watch reload 复用相同静态 capability 校验，unsupported edit 只
  `reload rejected` 并保留 last-good。
- **foreground polling**：没有 daemon、autostart、HTTP/WebSocket 或其他网络
  control plane。watcher 运行在既有 bounded takeover loop，正常 cadence 约
  100 ms；momentum 已经需要更短 loop 时复用该 cadence。
- **last-good**：文件 bytes 未变不做工作；变化后完整 decode + strict validate +
  `M19Profile` build。read/JSON/schema/构造失败只输出 `reload rejected` 并保留
  当前 last-good config；后续合法 save 可恢复。watcher 对 busy 状态只保留最新
  valid generation。
- **neutral-boundary apply**：`Arbiter::is_settings_quiescent()` 要求无 one-finger /
  two-finger ownership、open scroll、momentum、continuous gesture、three-finger
  drag/lock、physical/synthetic button ownership。否则 `try_replace_config` 返回
  false，caller queue 最新配置。neutral 后原子替换完整 config，并只清理 tunable
  pointer/scroll/gesture/router/drag residue 与 sub-pixel remainders；device/output/
  recorder/cleanup ownership 不被替换。faulted/stopped sink/bridge 拒绝 reconfigure。
- **用户快速调参**：`settings-patch FILE KEY=VALUE...` 使用和离线编辑相同的
  strict `UserSettings::set_key`，适合在第二终端执行；watcher 会看到文件变化。
  推荐主观 A/B 节奏是“抬手 → patch → 等 generation applied → 重做动作”。
- **测试事实**：dedicated tests 覆盖 invalid→last-good→valid recovery、pending
  latest-wins、active interaction 拒绝替换、neutral boundary 后替换、unsupported
  KDE reload reject/recover、DesktopAction 与 libei 分流、KDE preflight 失败先于
  inner portal prepare；M19 单指覆盖“一次 tap 后 180 ms 内 follow-up motion 进入
  drag”“181 ms 后只作为普通 pointer”“clean Ended 同帧 release”。三指覆盖
  `M18 stable_reference_motion=false / M19=true`、commit 丢弃 classifier motion、
  第一次真实 reference move 才 `ButtonDown → PointerMove`、reference replacement
  零 jump、`3→2→1→0` ownership 保持至 cluster empty；同时断言普通 pointer
  `max_gain=2.9` 时 drag-only ceiling 为 1.6。M15 legacy drag 集成测试仍全部通过，
  证明该修订没有回写旧 profile。全部 M10 cleanup/fault 回归仍通过。
- **资格**：真实 KDE 接入实现期间只执行 Plasma/KWin/KGlobalAccel 的只读
  introspection/preflight，没有调用 `invokeShortcut`，也没有执行真实 grab、
  portal/libei 发射或 live reload。M19 仍 **live-unqualified**，用户程序见
  `docs/M19_ACCEPTANCE.md`。
