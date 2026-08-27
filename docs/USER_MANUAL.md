# Touchpad Runtime — 用户手册（M17–M19）

## 1. 你现在主要维护一个文件：`settings.json`

M18 起推荐使用 `UserSettings v1`：

```text
settings.json
├── version
├── feel       # M17 手感参数
│   ├── pointer
│   ├── scroll
│   ├── gesture
│   └── drag
└── gestures   # M18 手势 → 功能映射
```

严格 schema 的意义是：拼错字段、超范围值、破坏阈值优先级的组合会在进入
live path 前直接拒绝，不会静默猜一个值。

## 2. 手感参数

### Pointer

| 参数 | 作用 | 调大后的直觉 |
| --- | --- | --- |
| `feel.pointer.dead_zone_radius_mm` | 微抖死区 | 更稳，但极小位移更难触发 |
| `feel.pointer.tracking_speed` | 整体指针倍率 | 同样手指距离移动更远 |
| `feel.pointer.min_gain` | 低速 gain | 慢速微操更快 |
| `feel.pointer.max_gain` | 高速 gain | 快甩时移动更远 |

建议先只调 `tracking_speed`。微调顺序：整体速度 → 低速精度 → 高速甩动 →
死区。

### Scroll

| 参数 | 作用 |
| --- | --- |
| `feel.scroll.min_gain` / `max_gain` | 低速/高速滚动倍率 |
| `feel.scroll.axis_lock_engage_ratio` | 进入横/纵轴锁定需要的方向优势 |
| `feel.scroll.axis_lock_release_ratio` | 解除轴锁的 hysteresis |
| `feel.scroll.momentum_tau_ms` | 惯性衰减时间；越大滑得越久 |
| `feel.scroll.momentum_start_speed_mm_per_s` | 松手后启动惯性的最低速度 |
| `feel.scroll.momentum_stop_speed_mm_per_s` | 惯性衰减到此速度时结束 |

约束：`release_ratio < engage_ratio`，`stop_speed < start_speed`。

### Gesture / Three-finger drag

`feel.gesture.*` 决定 pinch/page/multi-swipe 需要多大位移才 commit；
`feel.drag.commit_threshold_mm` 决定三指拖动多快抢到 ownership。

M19 在三指拖拽期间使用独立 pointer-fidelity ceiling：低速 gain、tracking
speed、dead-zone 与普通 pointer 保持一致，但高速 `max_gain` 封顶为 **1.6**。
这只作用于三指拖拽；当前完整配置的普通 pointer 仍可以使用
`tracking_speed=1.25 / max_gain=2.9`。该限制用于缩短高频三指输入下硬件 cursor
相对 compositor 拖拽图标的领先距离。

M19 的三指拖拽 motion 采用 stable reference finger。三指 centroid 只负责判断
是否越过 commit threshold；越过阈值的那一帧只建立 reference baseline，**不会
把落指到 commit 之间的累计位移补发给 pointer**。随后固定跟踪一个 tracking-id
的相对位移；reference finger 先抬起时，从仍存活的原始接触中选新的 reference，
切换帧只重新 baseline、不产生位置 jump。第一次真正产生 PointerMove 的帧才
建立 synthetic Left ownership。

commit 后整组原始接触拥有 drag ownership，干净的 `3 → 2 → 1 → 0` staggered
lift 不会把剩余手指漏给普通 pointer/scroll；直到原始 contact cluster 为空才
发送唯一 ButtonUp。M15–M18 保留旧 centroid motion 行为，上述 reference 模式
只作用于 `m19-live-v1`。

真实 portal/libei 输出还会把 `ButtonDown + 首段 PointerMove` 以及
`末段 PointerMove + ButtonUp` 保持在同一个 EIS logical hardware frame 内；tap
的 `ButtonDown + ButtonUp` 仍分成两个 frame。这样 compositor 不会在拖拽边界
先看到按钮状态、下一 frame 才看到对应的相对位移。

必须保持：

```text
feel.drag.commit_threshold_mm
    < feel.gesture.multi_swipe_commit_mm
```

这保证三指 drag 的设计优先级不会被调参反转。

## 3. 手势功能映射

修改形式：

```bash
touchpadctl settings-patch settings.json \
  gesture.three-finger-swipe-up=open-overview
```

可选 target：

```text
passthrough
disabled
next-workspace
previous-workspace
show-desktop
open-overview
close-overview
present-windows
application-launcher
notification-center
page-next
page-previous
smart-zoom
lookup
```

`passthrough` 表示保留原来的 continuous gesture semantic stream；
`disabled` 表示识别并消费，但不产生动作；其余值产生一个 typed
`DesktopAction`。

三指 drag 与三指 swipe 会竞争同一组三指接触。统一设置里额外有：

```text
gestures.three_finger_drag_enabled
```

默认是 `true`，保持 M15/M17 的三指拖动行为；此时三指 drag 会在较小位移
先 commit，三指 swipe 映射不会抢占它。若要把三指 left/right/up/down 当作
工作区/概览等手势，应将它设为 `false`。这只禁止 **drag commit**，三指 tap
识别仍保留。

可映射 trigger：

```text
pinch-in / pinch-out
rotate-clockwise / rotate-counter-clockwise
two-finger-page-left/right/up/down
three-finger-swipe-left/right/up/down
four-finger-swipe-left/right/up/down
edge-swipe-left/right/up/down
thumb-three-pinch / thumb-three-spread
three-finger-tap
```

映射后的连续手势只在 Begin 触发一次动作，后续 Update/End 不重复触发。

## 4. macOS-inspired preset

`touchpadctl settings-macos settings.json` 当前预置：

| Gesture | Target |
| --- | --- |
| 三指 left/right | next-workspace / previous-workspace |
| 三指 up | open-overview |
| 三指 down | present-windows |
| thumb+three pinch | application-launcher |
| thumb+three spread | show-desktop |
| 其余 two-finger page / pinch / rotate / four-finger / edge / three-finger tap | disabled |

该 preset 同时设置 `gestures.three_finger_drag_enabled=false`，因此上表中的
三指 swipe 实际可达；如果之后重新启用三指 drag，三指 swipe 会再次让位给
drag ownership。

这个收窄是 M19 real-KDE revision：preset 默认只启用当前 production
KGlobalAccel transport 能执行的动作，避免一启动就携带已知 unsupported route。
你仍可以手动配置其他 target，但真实 M19 会在 grab 前或 hot reload 时明确
拒绝未支持的 target。这里的 “macOS-inspired” 只描述操作布局，不表示速度
曲线、动画或硬件体验已经与 macOS 等价。

## 5. M19 实时调参的工作方式

M19 不把浏览器 GUI 直接连到 input daemon，也不开 HTTP/WebSocket。它只
watch 你显式传入的本地 `settings.json`。

每次文件变化分三种：

- **valid + idle**：马上应用，终端显示 generation；
- **valid + busy**：只保留最新 generation，等抬手/滚动惯性结束后应用；
- **invalid**：拒绝本次 reload，继续 last-good；下一次合法保存可以恢复。

这避免最危险的一类调参问题：三指 swipe 做到一半，阈值/映射突然变成另一
套语义。

## 6. 推荐的调参 SOP

1. `settings-default` 或 `settings-macos` 建基线。
2. `settings-check`。
3. 先离线 GUI 粗调。
4. 进入 M19 bounded session。
5. 每次只改 1–2 个参数。
6. 抬手，等 `applied generation N`。
7. 重复同一动作 5–10 次形成感受。
8. 记录参数值，不满意立即回退。

推荐优先级：pointer tracking speed → pointer gain → scroll gain → axis lock →
gesture thresholds → gesture action mapping。momentum 三项当前仅为旧配置兼容字段。

### 单点 / 双击 / tap-and-drag 的安全边界

实机 M19 曾观察到极短 re-touch：一次接触结束后几十毫秒内又出现新 tracking
id。当前 M19 按 libinput 风格使用 deferred release：第一次 tap 在 release 帧
只发 `ButtonDown(Left)`，把 matching `ButtonUp` 延迟到 **180 ms** follow-up
窗口结束。窗口内的新单指接触继承这个 held press；它真正越过 pointer commit
threshold 时直接开始 `PointerMove`，不会再发第二个 `ButtonDown`。180 ms 内
没有 follow-up 时才发 `ButtonUp`，完成普通 click。

因此当前语义是：`tap release → 很快再次落指并滑动 = 复用原 press 的
tap-and-drag`；如果中间停顿超过 180 ms，第一次 tap 先完成 click，后续滑动
就是普通移动。tracking-id replacement、取消、缺坐标、多指竞争或物理按键
都会显式解决 pending press，不会把旧点击带到后续操作。

`m19-live-v1` 还额外关闭了单指 tap-and-drag 的 sticky drag lock。真实 drag
一旦发生，手指 clean lift 的 `Ended` 帧就立即输出 `ButtonUp(Left)`；不会再
在抬手后继续保持左键、等待下一次触碰来解锁。这项低延迟修订只作用于 M19，
M10–M18 的历史 profile 契约不回写。

## 7. 安全与故障处理

真实 takeover 时始终保留：

- 外接键盘和鼠标；
- 第二终端；
- `kill -TERM <pid>` 的独立退出路径；
- 有界 `--max-duration-seconds`。

正常停止用 Ctrl-C/SIGTERM。若出现 output fault、设备 unplug、settings reload
错误等，查看 stderr 的结构化状态；不要通过不断放宽权限/关闭校验来“让它
跑”。

## 8. KDE 桌面动作的当前状态

M18 已经完成：gesture recognition → user mapping → typed `DesktopAction`。
M19 的生产 backend 进一步把两条输出通道组合起来：pointer/button/scroll
继续通过 RemoteDesktop portal + libei；离散 `DesktopAction` 通过 KDE Plasma 6
的 `org.kde.kglobalaccel.Component.invokeShortcut` 执行。启动前会只读查询
`shortcutNames()`，确认当前会话确实注册了所需动作。

当前 real M19 的动作集合为：

- `next-workspace` / `previous-workspace`；
- `open-overview` / `close-overview`；
- `present-windows`；
- `show-desktop`；
- `application-launcher`。

以下目标仍没有真实 M19 transport：`notification-center`、`page-next`、
`page-previous`、`smart-zoom`、`lookup`，以及 continuous gesture 的
`passthrough`。它们不会再等到手势触发后把 takeover 弄 fault：初始配置会在
grab 前失败，hot reload 会保留 last-good 并报告 `reload rejected`。

`open-overview` / `close-overview` 不是盲目调用同一个 toggle。Plasma 当前公开
的 `Overview` shortcut 是 toggle，因此 M19 会先只读查询 KWin `/Effects` 的
`activeEffects`：open 只在 `overview` 尚未 active 时调用 shortcut，close 只在
它已经 active 时调用。这样四指下滑不会在 Overview 原本关闭时反向把它打开。

当前仓库的 `settings-full.json` 是实机调校基线：pointer dead-zone 0.06 mm、
tracking speed 1.15、min/max gain 1.05/2.60；三指 drag commit 0.8 mm；scroll
axis-lock engage/release 1.6/1.2。三指负责拖拽，四指左右切工作区，四指上/下
分别进入/退出 Overview。

新的 `settings-macos` preset 因此只启用当前真实可执行的六类 KDE action，
其他 route 默认 `disabled`。如果你的设置文件是在本次 M19 KDE 接入之前生成
的，请重新生成。该映射仍然只称为 macOS-inspired，不声称 macOS 等价。

代码已接入真实 KDE 环境不等于 live-qualified；第一次真实动作测试仍按
`docs/M19_ACCEPTANCE.md` 分阶段执行。
