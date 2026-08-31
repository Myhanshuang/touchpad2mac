# Touchpad Runtime：Phase 0/1 深化设计与实现任务书

本文是 `design.md` 的工程化补充。原始目标与原则仍以 `design.md` 为准；本文把第一轮实现的范围、契约、失败语义和验收方式收敛到可以直接执行的程度。

## 1. 本轮目标

交付一个安全、可测试的 Phase 0/1 纵向切片：

```text
Linux raw evdev event
        ↓
versioned raw trace ───────→ offline replay
        ↓                         ↓
Type-B MT decoder ←───────────────┘
        ↓
normalized ContactFrame
```

本轮必须完成：

- 稳定 Rust workspace 与清晰模块边界。
- 平台无关的设备描述、轴描述、触点和帧模型。
- Linux touchpad 候选设备发现与 capabilities 检查。
- Type-B multitouch slot reconstruction。
- 仅在 `SYN_REPORT` 提交完整帧。
- `SYN_DROPPED` 检测、恢复状态机与可 mock 的内核快照接口。
- `EVIOCGRAB` 的 RAII 封装和幂等 shutdown。
- 版本化 raw trace、record 与 offline replay。
- 不依赖 root 或真实硬件的单元测试和集成测试。
- CLI：`devices`、`inspect`、`record`、`replay`。

本轮不实现：

- Wayland/X11 的实际事件注入。
- pointer acceleration、scroll、tap、drag、gesture 的完整算法。
- GUI、常驻服务、开机自启或系统设置修改。
- 在缺乏实机证据时宣称 Manjaro、Wayland、X11 已通过验收。

## 2. 技术与安全约束

- 使用稳定版 Rust。
- 核心 crate 不依赖 Linux、Wayland、X11、KDE 或 GNOME。
- `unsafe` 只允许位于最小化的 ioctl/FFI 边界，并在附近记录安全不变量。
- 不修改 `design.md`；如实现促成设计变化，更新本文。
- 不要求当前目录是 Git 仓库，不执行提交或推送。
- 不把 API key、token 或其他秘密写入源码、文档、日志、trace、测试夹具。
- 真实设备 grab 必须显式 opt-in；默认执行不得独占用户触控板。
- 所有真实设备权限错误必须变成可理解的诊断，不能 panic。
- 不复制 Apple、libinput 或其他项目源码，除非先核验许可证并保留归属。本轮以 clean-room 接口和自有测试为主。

## 3. 推荐 workspace 边界

```text
Cargo.toml
Cargo.lock
README.md
THIRD_PARTY.md
crates/
  touchpad-core/
  touchpad-linux/
  touchpad-trace/
apps/
  touchpadctl/
tests/
  fixtures/
```

### 3.1 `touchpad-core`

负责平台无关类型和契约：

- `ContactFrame`
- `Contact`
- `ContactState`
- `PhysicalButtons`
- `AxisInfo`
- `DeviceDescriptor`
- 毫米、原始轴值、单调时间等避免混用的类型
- descriptor/frame validation 和结构化诊断
- 后续 classifier、arbiter、pointer、scroll、tap、gesture 的模块或 trait 边界
- typed `OutputSink` 契约，只定义语义，不做真实桌面注入

不得以空的“成功实现”冒充手势、加速度或输出后端；未实现能力应明确表达。

### 3.2 `touchpad-linux`

负责所有 Linux-specific 工作：

- `/dev/input/event*` 枚举。
- 根据 `EV_KEY`、`EV_ABS`、`INPUT_PROP_POINTER`、`INPUT_PROP_BUTTONPAD` 等信息识别候选触控板并解释判定依据。
- 读取 `input_absinfo`、slot count、按键和设备标识。
- 将 kernel `input_event` 转为可注入测试的内部 `RawEvent`。
- Type-B slot decoder。
- `SYN_DROPPED` recovery。
- `EVIOCGRAB` guard。
- 阻塞输入循环和受控 shutdown。

### 3.3 `touchpad-trace`

负责 raw trace 的流式读写：

- 推荐 JSON Lines，首条必须是 header。
- header 包含 `schema_version`、clock 语义、设备标识、capabilities、轴 ranges、resolution、slot 数。
- 后续 event 保留原始 `sec/usec` 或等价精度的单调时间、`type/code/value`。
- reader 明确区分不支持的新 schema、损坏行、字段不合法和 I/O 错误。
- writer 定义 flush 和正常结束语义。
- replay 使用 trace descriptor 和与真实输入相同的 decoder，不建立第二套状态机。

### 3.4 `touchpadctl`

至少提供：

```text
touchpadctl devices
touchpadctl inspect DEVICE
touchpadctl record DEVICE OUTPUT [--grab]
touchpadctl replay INPUT
```

要求：

- `--grab` 默认关闭，帮助文本明确警告独占风险。
- 无设备、权限不足、设备不支持 Type-B MT、trace 损坏均返回非零状态和可操作诊断。
- `replay` 不访问真实输入设备，可在普通用户和 CI 环境运行。

## 4. 核心数据契约

统一帧至少表达：

```text
ContactFrame {
    monotonic_timestamp,
    sequence,
    discontinuity,
    contacts[],
    physical_buttons,
    diagnostics[]
}
```

触点至少表达：

```text
Contact {
    tracking_id,
    slot,
    x_mm,
    y_mm,
    pressure?,
    major_mm?,
    minor_mm?,
    orientation?,
    state
}
```

约束：

- 手势时序只使用 monotonic clock，wall clock 不得参与 timeout/velocity。
- 原始轴值与毫米必须是不同类型或必须经过显式转换 API。
- 坐标毫米化必须使用 `input_absinfo.resolution`。
- resolution 缺失时，只允许使用显式 `DeviceProfile` override；否则保留“未归一化”状态或返回明确诊断，不得伪装成精确毫米。
- 可选能力缺失不能导致整个帧不可用；必要坐标缺失必须有确定策略。

本轮确定采用以下必要坐标策略：新 tracking id 在 X/Y 未齐全之前保留为内部 incomplete slot，不发布为有效 `Contact`；后续事件补齐后才发布，并在该帧附带诊断。已存在 tracking id 的未更新字段沿用上一个 committed state。

## 5. Type-B decoder 状态机

decoder 同时维护：

- committed slot state：上一个已提交 kernel frame。
- pending slot state：当前 `SYN_REPORT` 周期中的增量修改。
- current slot：由 `ABS_MT_SLOT` 选择。
- sync state：`Normal | DroppedAwaitingBoundary | Recovering | Degraded`。

事件规则：

1. `ABS_MT_SLOT` 仅切换 current slot。
2. `ABS_MT_TRACKING_ID >= 0` 开始新触点；同一 slot 已有不同 id 时视为替换生命周期。
3. `ABS_MT_TRACKING_ID == -1` 结束当前 slot 触点。
4. 其他 `ABS_MT_*` 只更新 current slot 的 pending 字段。
5. physical button 事件进入同一 pending frame。
6. 只有 `SYN_REPORT` 才合并 pending、递增 sequence 并发布 frame。
7. 单个事件绝不直接发布半帧。

应测试：

- 单触点 begin/update/end。
- 多 slot 交错更新。
- slot 切换不改变触点数据。
- tracking id 替换。
- 新触点坐标不完整。
- 未变化字段继承。
- physical button 与触点在同一 frame 提交。
- 非法 slot 和异常事件顺序产生诊断而不是 panic。

## 6. `SYN_DROPPED` 恢复

恢复协议：

1. 收到 `SYN_DROPPED` 后进入 `DroppedAwaitingBoundary`。
2. 忽略普通增量事件，直到下一个 `SYN_REPORT` 边界。
3. 进入 `Recovering`，通过抽象 `KernelStateSnapshot`/`ResyncSource` 查询 slots、tracking ids、各 ABS 字段和按键状态。
4. 查询接口必须可 mock，使测试无需 `/dev/input`。
5. 恢复成功后原子替换 decoder 状态，并发布 `discontinuity = true` 的完整帧。
6. 恢复失败后进入 `Degraded`，向上返回致命错误；若持有 grab，运行时必须释放它，而不是继续依据不完整状态产生输出。

必须说明：无法拦截 `SIGKILL`、内核崩溃或硬断电，不能承诺这些情况下执行用户态 cleanup。

## 7. Grab 与生命周期

第一版采用单进程、单阻塞输入循环和受控 shutdown，不为简单 I/O 引入 async runtime。

启动顺序：

1. 打开并验证设备。
2. 准备 recorder/decoder，以及未来的输出后端。
3. 只有显式 `--grab` 时才申请 `EVIOCGRAB`。
4. 进入输入循环。

退出顺序：

1. 停止接收新工作。
2. 结束未来的 semantic output 生命周期并释放按键。
3. flush recorder。
4. 幂等 ungrab。
5. 关闭 fd 并返回结构化状态。

`GrabGuard::drop` 必须尽最大努力 ungrab；正常退出、错误路径、`SIGINT`、`SIGTERM` 都必须进入显式的幂等 shutdown。panic hook 只能作为补充，不能代替错误处理。

## 8. Recording / Replay

raw recorder 位于 decoder 之前，因此即使 decoder 有 bug，trace 仍保留用于复现的原始输入。

建议 schema：

```json
{"kind":"header","schema_version":1,"clock":"monotonic",...}
{"kind":"event","sec":0,"usec":1234,"type":3,"code":47,"value":0}
```

要求：

- 文件逐行可读，允许大型 trace 流式处理。
- schema version 不匹配必须显式报错。
- 数值范围、时间倒退和 header 缺失有明确诊断。
- fixture 至少覆盖单触点、多 slot、tracking id 结束、按钮、缺失 resolution 和 dropped/recovery。
- 集成测试必须证明 fixture replay 最终走到与实时输入相同的 `ContactFrame` 生成路径。

## 9. 未来输出边界

本轮只定义 typed semantic output，例如：

```text
PointerMove
ButtonDown / ButtonUp
ScrollBegin / ScrollDelta / ScrollEnd
KeyDown / KeyUp
DesktopAction
```

`OutputSink` 必须定义顺序、backpressure、部分失败和 shutdown 时释放状态的语义。

Wayland/libei、X11 和 uinput 均是待验证方案，不在本轮实现。进入下一阶段前必须分别验证：

- Runtime 产生的 pointer delta 是否被再次 acceleration。
- pixel scroll 是否被桌面再次加速或转成离散滚轮。
- backend 断开时能否可靠释放 button/key 状态。
- 是否会把原始 finger count 或 multitouch 重新暴露给系统 policy。

无法证明“resolved output 不再被二次解释”的 backend 不得成为默认实现。

## 10. 许可证与依赖

- 新增 `THIRD_PARTY.md` 或 `LICENSES.md`，记录直接依赖、许可证和用途。
- 依赖保持克制并锁定 `Cargo.lock`。
- 优先成熟 crates；系统库只是可选探测，不应让离线 replay 测试依赖桌面会话。
- Apple `IOHIDFamily`、libinput、linux-3-finger-drag 等在本轮只作为行为和接口参考，不直接复制源码。

## 11. 测试矩阵

普通用户、无真实触控板的 CI 必须能够运行：

```text
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

测试至少覆盖：

- core 类型校验与 raw/mm conversion。
- decoder 的完整状态机和错误事件。
- mocked resync success/failure。
- grab guard 的 syscall adapter 或 mock 上的幂等释放。
- trace header/event round-trip、schema error、损坏输入。
- fixture replay 到预期 `ContactFrame`。
- CLI help 和无需设备的 replay smoke test。

实机测试单独记录，不得由单元测试假装完成：

- 正确识别内置触控板。
- capabilities 和 axis resolution 与 `evtest`/内核信息一致。
- 显式 grab 后系统不再收到物理设备事件。
- `SIGINT`、`SIGTERM`、拔出、backend error 后恢复系统输入。
- `SYN_DROPPED` 的真实 ioctl resync。

## 12. 本轮验收物

- `DESIGN_V2.md` 或对本文的必要补充，但不修改原始 `design.md`。
- 可构建的 Rust workspace。
- 上述 core/linux/trace/CLI 纵向切片。
- `README.md`：构建、测试、普通用户 replay 示例、真实设备权限和 grab 风险。
- 第三方依赖说明。
- 全部无硬件测试通过。
- 最终报告必须准确列出：创建/修改文件、实际范围、命令结果、未验证硬件事项、下一阶段的最小任务。

## 13. 执行代理指令

执行代理必须：

1. 完整阅读 `design.md` 和本文。
2. 直接在工作区创建文件并实现，不只输出建议或代码片段。
3. 先检查本机已有 Rust 与相关系统库，但不得因没有真实设备权限而停工。
4. 逐步运行 format、clippy、test 并修复可修问题。
5. 不提交、不推送、不泄露秘密。
6. 不虚报 Wayland、X11、EVIOCGRAB 或实机行为已验证。

