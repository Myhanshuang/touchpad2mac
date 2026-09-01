# Touchpad Runtime — 运行指南

本指南面向“把项目跑起来并开始调手感”的用户。完整参数解释见
`doc/USER_MANUAL.md`。

> 当前所有 M10–M19 live profile 仍是 **live-unqualified**。代码通过自动化
> 测试不等于实机资格。真实 takeover 会独占触控板并向桌面发送输入。

## 1. 环境

当前真实输出纵向链针对 Linux + KDE Wayland：

- Rust toolchain，workspace MSRV 声明为 1.87；
- `/dev/input/event*` 可读权限；
- KDE Wayland session；
- XDG RemoteDesktop portal 可用；
- `libei.so.1` 可由运行时加载。

构建本身不要求 root，也不会打开触控板：

```bash
cd /home/acacia/touchpad
cargo build --release --locked
```

下文用：

```bash
TP=target/release/touchpadctl
```

## 2. 找到触控板

```bash
$TP devices
$TP inspect /dev/input/event12
```

把实际候选设备路径记为 `DEVICE`。如果权限不足，先解决设备读取权限；
不要为了“先跑起来”直接长期使用 root。

## 3. 检查桌面输出（默认不发输入）

```bash
$TP output-probe
```

这条命令是 non-emitting preflight。只有显式 `--emit` 才会产生真实桌面
输入；首次使用前按 `doc/old/acceptance/M6_ACCEPTANCE.md` 完成输出校准。

## 4. 创建用户设置

推荐直接从 M18/M19 的统一设置文件开始：

```bash
$TP settings-default settings.json
$TP settings-check settings.json
```

若希望先得到一套接近 macOS 习惯的“手势→功能”布局：

```bash
$TP settings-macos settings.json
$TP settings-check settings.json
```

这是 **macOS-inspired mapping preset**，不是 macOS 等价声明。
该 preset 会关闭三指 drag commit，让三指 swipe 能用于工作区/概览等映射；
三指 tap 仍可使用。若更需要三指拖动，可执行：

```bash
$TP settings-patch settings.json gesture.three-finger-drag-enabled=true
```

## 5. 修改设置

命令行：

```bash
$TP settings-set settings.json settings-new.json \
  feel.pointer.tracking_speed=1.20 \
  feel.scroll.momentum_tau_ms=450 \
  gesture.three-finger-swipe-up=open-overview

$TP settings-check settings-new.json
```

离线 GUI：

```bash
$TP settings-gui settings.json settings.html
```

用浏览器直接打开 `settings.html`。页面没有服务器、网络请求、设备访问或
live-apply；修改后导出新的 `settings.json`，再执行 `settings-check`。

## 6. M18：固定设置运行

只有在已经完成 M6/M10 的 live acceptance 后才建议进入真实 takeover。
必须准备外接键鼠和第二终端。

```bash
$TP takeover trace-m18.jsonl \
  --takeover \
  --confirm TAKEOVER \
  --output-qualified \
  --profile m18-remap-v1 \
  --settings settings.json \
  --max-duration-seconds 60
```

M18 启动后不会再读设置文件。要改变设置，停止本次 bounded takeover，修改
文件，再重新启动。M19 专门解决这个问题。

## 7. M19：边调边试

Terminal A：

```bash
$TP takeover trace-m19.jsonl \
  --takeover \
  --confirm TAKEOVER \
  --output-qualified \
  --profile m19-live-v1 \
  --settings settings.json \
  --watch-settings \
  --max-duration-seconds 300
```

默认会扫描 `/dev/input/event*`。如果只发现一个可用触控板，会直接选中并在
stderr 打印 `auto-selected touchpad: ...`。如果发现多个候选，takeover 会在
打开 portal、创建 recorder 或 grab 设备之前拒绝继续，并列出每个候选；此时按
提示重跑并加上例如 `--device /dev/input/event15`。旧的
`takeover DEVICE TRACE ...` 写法仍兼容，但新命令建议统一使用 `--device`。

M19 同时会为 DWT 自动寻找与内置触控板配对的键盘。键盘节点只以
`O_RDONLY | O_CLOEXEC` 打开，并设置为 `CLOCK_MONOTONIC`；**不会对键盘执行
EVIOCGRAB，也不会记录 keycode 到 trace/log**。默认首次有效打字按键后抑制
新的触摸 200 ms，连续输入时延长为 500 ms。单独的 Ctrl/Alt/Shift/Meta/Fn
等修饰键不触发 DWT；已经开始的 pointer、scroll、gesture、drag 不会因为随后
按键而被强行取消。

可实时调整：

```bash
$TP settings-patch settings.json dwt.enabled=true
$TP settings-patch settings.json dwt.short-timeout-ms=200
$TP settings-patch settings.json dwt.long-timeout-ms=500
```

若运行时无法读取配对键盘，会打印 `DWT unavailable` 并继续正常使用触控板，
而不是让 takeover 整体失败。此时检查对应 `/dev/input/event*` 的读取 ACL。

Terminal B：

```bash
$TP settings-patch settings.json feel.pointer.tracking_speed=1.10
$TP settings-patch settings.json feel.pointer.tracking_speed=1.25
$TP settings-patch settings.json feel.scroll.momentum_tau_ms=500
$TP settings-patch settings.json gesture.four-finger-swipe-up=open-overview
```

M19 的规则：

1. 合法文件变更通常在下一次 polling loop 被发现；正常 loop 约 100 ms。
2. 若当前没有 active pointer/scroll/gesture/button/momentum，立即原子切换。
3. 若当前手势还没结束，只排队最新一版；抬手回到 neutral boundary 后应用。
4. 临时写坏 JSON 不会杀掉 session：终端显示 `reload rejected`，继续使用
   last-good 设置；下一次合法保存自动恢复。

所以主观 A/B 调参建议始终采用：**抬手 → patch → 看 applied generation →
重新做动作**。

## 8. 停止与恢复

首选 Ctrl-C / SIGTERM，让程序走完整 ordered cleanup。不要把 SIGKILL 当正常
退出方式。

如果某个设置让体验明显变差：

```bash
$TP settings-default reset.json
$TP settings-check reset.json
```

M19 运行中可把 `reset.json` 替换/复制为正在 watch 的设置文件，或者用
`settings-patch` 恢复具体值。若行为异常，优先停止 takeover，再排查。

## 9. 当前功能边界

- pointer/click/scroll 的 portal+libei 输出已有真实纵向实现，但 live
  qualification 仍按各 acceptance 文档执行；
- M18 单独只负责 gesture recognition → typed `DesktopAction`；M19 的真实 KDE
  backend 现在把这些离散动作通过 Plasma 6 KGlobalAccel 执行，同时
  pointer/button/scroll 继续走 portal+libei；
- 当前真实 M19 支持：next/previous workspace、Overview、Present Windows、
  Show Desktop、Application Launcher；Notification Center、page next/previous、
  Smart Zoom、Lookup 与 native continuous-gesture passthrough 仍未实现；
- 不支持的 mapping 在真实 M19 启动时会在 grab 前拒绝，运行中的 hot reload
  则会 `reload rejected` 并继续 last-good；
- 如果 `settings.json` 是早期版本生成的 `settings-macos`，先重新运行
  `$TP settings-macos settings.json`，因为新的 preset 只启用上述真实可执行
  KDE 动作，其余 route 默认 disabled；
- KDE 动作代码已接入，但仍需按 `doc/old/acceptance/M19_ACCEPTANCE.md` 做用户实机验收，
  不能把 code-complete 等同于 live-qualified；
- M18/M19 不允许设置任意 shell command。

## 10. 推荐完整配置

仓库根目录的 `settings-full.json` 是当前 KDE/Wayland 实机路径的推荐完整配置，
并已同步到 `settings.json`。它采用：三指拖拽（抬手释放）、四指切工作区/
Overview、thumb+three launcher/show-desktop；四指上滑只进入 Overview，四指
下滑只退出 Overview。当前没有真实 KDE transport 的目标全部 disabled，避免
运行中 semantic output fault。

2026-08-23 实机手感校准后的关键值：

```text
pointer.dead_zone_radius_mm      = 0.06
pointer.tracking_speed           = 1.15
pointer.min_gain                 = 1.05
pointer.max_gain                 = 2.60
drag.commit_threshold_mm         = 0.80
scroll.axis_lock_engage_ratio    = 1.60
scroll.axis_lock_release_ratio   = 1.20
```

其中三指拖拽复用 pointer fidelity，因此 pointer 灵敏度调整会同步作用于三指拖动；
降低 drag commit threshold 只让拖拽更早进入 committed 状态。axis-lock ratio 越低
越允许纵向滚动带横向偏差，release ratio 提供已锁定后的 hysteresis。
