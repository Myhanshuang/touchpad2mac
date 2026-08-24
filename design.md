

# Touchpad Runtime — Master Design Specification

## 1. 项目目标

开发一个跨桌面环境的用户态触控板输入系统。

Linux 为第一目标平台。

首个可用版本必须支持：

- Manjaro Linux
- Wayland
- X11
- 内置笔记本触控板

后续目标依次为：

- 跨 Linux 发行版
- 跨 Linux 桌面环境
- Windows
- macOS

项目目标不是增加几个手势，而是**完整接管触控板行为**。

项目负责：

- 指针移动
- 灵敏度
- 指针加速度
- 点击
- Tap-to-click
- Tap-and-drag
- Drag lock
- 双指滚动
- 滚动速度
- 滚动加速度
- 滚动惯性
- Palm / Thumb filtering
- 三指拖拽
- Pinch
- 三指、四指手势
- 后续连续手势

操作系统和桌面环境不得再次解释物理触控板输入。

------

# 2. 核心原则

## 2.1 Single Policy Owner

对被接管的物理触控板，只允许本项目作为 touchpad policy owner。

正确结构：

```text
Physical Touchpad
        ↓
Exclusive Raw Input
        ↓
Touchpad Runtime
        ↓
Resolved Output
        ↓
OS / Compositor / Application
```

禁止：

```text
Physical Touchpad
   ├─ System Touchpad Stack
   └─ Touchpad Runtime
```

也禁止默认使用：

```text
Physical Touchpad
        ↓
Touchpad Runtime
        ↓
Virtual Touchpad
        ↓
System libinput
        ↓
System Touchpad Policy
```

因为这会重新引入系统的 acceleration、tap、drag、scroll 和 gesture policy。

------

## 2.2 Raw Input, Resolved Output

输入端读取：

```text
raw multitouch contacts
```

输出端只产生已经解释完成的：

```text
pointer motion
button state
smooth scroll
keyboard event
desktop action
```

系统不应再获得原始 finger count 或 multitouch gesture 信息。

------

## 2.3 系统触控板设置不参与运行时行为

程序运行期间，以下系统设置不得改变实际行为：

```text
Pointer Speed
Acceleration
Tap-to-click
Tap-and-drag
Natural Scrolling
Scroll Speed
Three/Four-finger Gestures
```

不要求主动修改 KDE/GNOME 配置。

目标是通过接管输入链路，使这些设置不再控制物理触控板。

------

# 3. 总体架构

```text
Physical Touchpad
        │
        ▼
┌─────────────────────┐
│ Linux Input Backend │
│ evdev               │
│ EVIOCGRAB            │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ MT Decoder          │
│ Type-B slots        │
│ SYN_REPORT          │
│ resynchronization   │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ Contact Model       │
│ normalization       │
│ classification      │
│ filtering           │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ Interaction Arbiter │
└──────────┬──────────┘
           │
   ┌───────┼────────┬────────┐
   ▼       ▼        ▼        ▼
Pointer   Scroll   Tap/Drag  Gesture
Engine    Engine    Engine    Engine
   │       │        │        │
   └───────┴────────┴────────┘
                    │
                    ▼
           ┌─────────────────┐
           │ Semantic Output │
           └────────┬────────┘
                    │
          ┌─────────┴─────────┐
          ▼                   ▼
     Wayland Backend       X11 Backend
```

核心算法不得依赖 Wayland、X11、KDE 或 GNOME。

------

# 4. Linux 输入层

Linux 第一版直接读取 `/dev/input/event*`。

程序必须能够：

- 识别触控板
- 查询设备 capabilities
- 获得设备独占权
- 读取 raw evdev events
- 处理完整 frame
- 处理设备断开和重新连接
- 处理输入状态重新同步
- 程序退出时可靠释放设备

Linux kernel 的 evdev 是通用 input event 接口；Type-B multitouch 使用 slot 和 `ABS_MT_TRACKING_ID` 维护触点状态。([Linux内核档案馆](https://www.kernel.org/doc/html/next/input/input.html?utm_source=chatgpt.com))

设备接管使用 Linux input subsystem 的 grab 机制；内核文档记录了通过 `EVIOCGRAB` 持有设备 grab 的 input handle。([Linux内核档案馆](https://www.kernel.org/doc/html/v6.9/driver-api/input.html?utm_source=chatgpt.com))

------

# 5. Contact Model

Linux-specific event 必须在输入层结束。

核心统一使用：

```text
ContactFrame {
    timestamp

    contacts[] {
        id
        x_mm
        y_mm
        pressure
        major
        minor
        orientation
        state
    }

    physical_buttons
}
```

后续 Windows/macOS backend 也转换成相同结构。

------

# 6. Hardware Normalization

不同设备的：

```text
raw coordinate range
resolution
physical dimensions
pressure range
report rate
slot count
```

不得直接进入交互算法。

输入层负责转换为统一物理单位。

主要单位：

```text
millimeter
second
millimeter / second
logical pixel
```

必须预留：

```text
DeviceProfile
DeviceQuirk
```

用于处理具体硬件差异。

------

# 7. Interaction Arbiter

所有触点首先进入统一的 Interaction Arbiter。

禁止 Pointer、Scroll、Tap、Gesture 模块分别竞争同一批 raw contacts。

Arbiter 管理：

```text
Candidate
Committed
Cancelled
Finished
```

等 interaction 生命周期。

典型竞争关系：

```text
One Finger
├─ Tap
├─ Pointer
└─ Tap-and-drag

Two Fingers
├─ Scroll
├─ Pinch
└─ Multi-finger tap

Three Fingers
├─ Three-finger drag
├─ Swipe
└─ Tap
```

一旦一个 interaction committed，与之冲突的 candidate 必须取消。

------

# 8. Pointer Engine

Pointer Engine 独立负责：

```text
finger delta
→ physical normalization
→ jitter filtering
→ velocity estimation
→ acceleration
→ subpixel accumulation
→ logical pointer delta
```

指针停止后不得产生惯性运动。

第一目标 profile 为：

```text
macOS-like pointer profile
```

Apple 开源的 `IOHIDFamily` 包含 `IOHIDPointerScrollFilter` 和 `IOHIDAccelerationAlgorithm`。其中 pointer acceleration 支持参数化曲线和 table-based curve，并考虑设备 resolution、report rate 和 acceleration parameters。

因此第一版不重新发明 acceleration 模型，应优先研究并实现与 Apple 模型等价或接近的独立实现。

具体源码复用方式必须在许可证审查后决定。

------

# 9. Scroll Engine

Scroll Engine 与 Pointer Engine 独立。

负责：

```text
two-finger centroid
→ filtering
→ axis handling
→ velocity
→ scroll acceleration
→ pixel delta
→ release velocity
→ momentum
→ deceleration
→ stop
```

必须支持：

- horizontal
- vertical
- diagonal
- natural direction
- smooth scrolling
- kinetic scrolling
- interruption

Apple 的 `IOHIDPointerScrollFilter` 分别维护 pointer 和 scroll accelerator，并单独处理 scroll acceleration，因此可作为滚动算法的重要参考。

惯性生命周期由本项目控制，不交给桌面环境决定。

------

# 10. Tap / Drag Engine

项目自行实现：

```text
single tap
double tap
multi-finger tap
tap timeout
movement threshold
tap cancellation
tap-and-drag
drag lock
```

Tap-and-drag 必须在项目内部转换为：

```text
button down
pointer motion
button up
```

系统不得再看到需要解释为 tap-and-drag 的触摸序列。

libinput 已有完整 tap、tap-and-drag 和 drag-lock 行为，可作为状态机和边界条件参考。([Wayland](https://wayland.freedesktop.org/libinput/doc/latest/tapping.html?highlight=tap&utm_source=chatgpt.com))

------

# 11. Contact Classification

独立模块负责：

```text
Finger
Thumb
Palm
Unknown
Invalid
```

输入可以包括：

```text
position
pressure
contact size
velocity
device edges
previous state
current interaction
typing state
```

libinput 对不同硬件能力、pressure、touch size、palm/thumb detection 已有成熟处理，可作为主要参考实现之一。([Wayland](https://wayland.freedesktop.org/libinput/doc/latest/touchpads.html?utm_source=chatgpt.com))

------

# 12. Gesture Engine

基础 pointer、tap、drag、scroll 稳定后再实现。

第一批：

```text
Three-finger Drag
Three-finger Swipe
Four-finger Swipe
Pinch
```

长期接口必须支持连续手势：

```text
begin
update(progress, velocity)
commit
cancel
end
```

Gesture Engine 不直接读取 raw evdev。

它只接受已经经过 Contact Model 和 Interaction Arbiter 的数据。

------

# 13. 输出层

统一接口：

```text
move_pointer(dx, dy)

button_down(button)
button_up(button)

scroll_begin()
scroll_delta(dx, dy)
scroll_end()

key_down(key)
key_up(key)

desktop_action(action)
```

不同平台实现独立 backend。

### Wayland

优先研究 `libei`。

libei 是面向 Emulated Input 的协议/API，并提供 pointer 和 pixel-precise smooth scrolling event。([Libinput](https://libinput.pages.freedesktop.org/libei/?utm_source=chatgpt.com))

必须实际验证：

```text
Runtime calculated output
        ↓
Wayland Backend
        ↓
final application behavior
```

不存在不可控的二次 acceleration 或 scroll policy。

如果某输出方式无法满足该要求，不得作为默认 backend。

### X11

实现独立 X11 output backend。

必要时保留 uinput 作为兼容/fallback backend。

Linux `uinput` 允许 userspace 创建虚拟输入设备；内核文档同时建议新程序考虑使用 libevdev 封装以降低直接操作 uinput 的错误风险。([Linux内核文档](https://docs.kernel.org/6.2/input/uinput.html?utm_source=chatgpt.com))

------

# 14. Fail-Safe

由于程序需要独占物理触控板，输入接管必须采用 fail-open 原则。

以下情况必须释放设备或恢复系统输入：

```text
normal exit
SIGINT
SIGTERM
engine failure
backend failure
IPC failure
fatal error
```

开发版本默认不得在无法恢复输入的情况下自动开机接管设备。

------

# 15. Recording / Replay

必须早期实现真实输入录制。

```text
Physical Touchpad
        ↓
Raw Trace
```

以及：

```text
Raw Trace
        ↓
Offline Replay
        ↓
Trackpad Core
        ↓
Expected Output
```

每个实际 bug 应尽量保存为 regression trace。

libinput 自身的 `libinput record` 也是直接记录 kernel events，并且录制结果不依赖当前 X.Org 或 Wayland session，可作为设计参考。([Wayland](https://wayland.freedesktop.org/libinput/doc/latest/tools.html?utm_source=chatgpt.com))

------

# 16. 开源复用策略

开发原则：

```text
Reuse
→ Adapt
→ Port
→ Reimplement from reference
→ New implementation
```

按此顺序选择。

不得在已有成熟实现时无理由重新造轮子。

| 来源                        | 用途                                                         |
| --------------------------- | ------------------------------------------------------------ |
| `lmr97/linux-3-finger-drag` | Linux evdev、长期 grab、MT frame、resync、设备代理           |
| Linux Kernel                | evdev、Type-B MT、uinput、设备协议                           |
| Apple `IOHIDFamily`         | macOS pointer/scroll acceleration                            |
| `VoodooInput`               | Apple/Magic Trackpad contact representation 和普通触控板到 Apple MT 模型的转换参考 |
| `libinput`                  | tap、drag、palm、thumb、acceleration、device quirks          |
| `libei`                     | Wayland semantic input output                                |
| Linux `hid-magicmouse`      | Apple Magic Trackpad 输入协议参考                            |

`linux-3-finger-drag` 当前 `MtProxy` 已包含生命周期内持续 grab、raw frame 读取、`SYN_DROPPED` recovery、slot 查询和虚拟设备相关实现，非常适合作为 Linux input backend 的直接参考。

VoodooInput 已定义 Magic Trackpad 风格的 finger packet，包括位置、状态、touch major/minor、size、pressure、identifier 和 angle，并实现 generic contact 到该 representation 的转换。

libinput 本身就是完整 Linux input stack，包含 touchpad pointer generation、acceleration 等功能，因此适合作为算法和边界条件参考，而不作为本项目运行时的第二个 policy owner。([Wayland](https://wayland.freedesktop.org/libinput/doc/latest/index.html?utm_source=chatgpt.com))

任何直接复制或修改第三方代码之前必须检查对应文件和项目许可证。

------

# 17. 推荐代码边界

```text
trackpad-runtime/

core/
    contact
    classifier
    arbiter
    pointer
    scroll
    tap
    drag
    gesture

platform/
    linux/
        device
        evdev
        mt
        grab
        output/
            wayland
            x11
            uinput

profiles/
    macos

tools/
    device-info
    record
    replay
    debug

tests/
    traces
    regression
```

这些是逻辑边界。

初期可以编译成一个程序；是否拆成多个进程属于后续实现决定，不影响核心结构。

------

# 18. 开发阶段

### Phase 1 — Input Foundation

完成：

```text
device discovery
EVIOCGRAB
raw event reading
Type-B MT reconstruction
ContactFrame
record / replay
safe release
```

验收：

可以稳定独占设备并正确恢复。

------

### Phase 2 — Complete Basic Touchpad

完成：

```text
one-finger pointer
physical click
tap
tap-and-drag
two-finger scroll
```

验收：

在 Manjaro Wayland 和 X11 上，日常基本操作不需要系统 touchpad policy。

------

### Phase 3 — Motion Fidelity

完成：

```text
Mac-style pointer acceleration
scroll acceleration
smooth scrolling
scroll momentum
hardware normalization
```

验收：

同一 profile 在 X11 和 Wayland 上产生尽可能一致的 pointer 和 scroll 行为。

------

### Phase 4 — Contact Robustness

完成：

```text
jitter filtering
palm
thumb
typing suppression
device quirks
```

------

### Phase 5 — Gestures

完成：

```text
three-finger drag
pinch
three-finger swipe
four-finger swipe
continuous gestures
```

------

### Phase 6 — Linux Portability

将 Linux-specific assumptions 收敛到 `platform/linux` 和 `DeviceProfile`。

验证多个：

```text
distribution
desktop environment
touchpad vendor
```

------

### Phase 7 — Cross-platform

保留：

```text
Contact Model
Interaction Arbiter
Pointer Engine
Scroll Engine
Tap/Drag Engine
Gesture Engine
Profiles
```

替换：

```text
Input Backend
Output Backend
Platform integration
```

------

# 19. Linux 第一阶段最终验收标准

程序启动后必须满足：

```text
✓ 物理触控板由 Runtime 独占

✓ 系统原生多指手势不触发

✓ 系统 Pointer Speed 不控制实际触控板速度
✓ 系统 Touchpad Acceleration 不控制实际加速度

✓ 系统 Tap-to-click 不参与
✓ 系统 Tap-and-drag 不参与
✓ 系统 Natural Scrolling 不参与

✓ 指针由 Runtime 产生
✓ 点击由 Runtime 产生
✓ Tap-and-drag 由 Runtime 产生
✓ 双指滚动由 Runtime 产生
✓ 滚动惯性由 Runtime 产生

✓ Wayland 可用
✓ X11 可用

✓ Runtime 正常或异常退出后能够恢复物理触控板
```

如果这些条件未满足，则核心架构尚未完成，不进入 GUI、配置应用和高级 gesture 开发。

------

# 20. 项目定义

本项目最终定义为：

> **A cross-platform userspace touchpad runtime that exclusively owns touchpad policy and provides a consistent, macOS-oriented interaction model independent of the desktop environment.**

Linux 第一版的重点不是重新发明已有触控板技术，而是：

```text
成熟的 Linux input plumbing
+
成熟的 touch/contact processing 经验
+
Apple 已公开的 motion acceleration 模型
+
统一的 Interaction Arbiter
+
受控的 semantic output
=
独立的 Touchpad Runtime
```

