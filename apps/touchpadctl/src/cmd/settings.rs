//! M18 unified user settings CLI and offline editor.

#![forbid(unsafe_code)]

use std::path::Path;

use touchpad_core::{
    feel_parameter_specs, UserSettings, ALL_GESTURE_TARGETS, ALL_GESTURE_TRIGGERS,
};

use crate::env::CommandEnv;
use crate::exit::CommandFailure;

#[derive(Clone, Copy)]
struct FeelUiText {
    label: &'static str,
    controls: &'static str,
    lower: &'static str,
    higher: &'static str,
    tuning: &'static str,
}

fn feel_ui_text(key: &str) -> Option<FeelUiText> {
    Some(match key {
        "pointer.dead_zone_radius_mm" => FeelUiText {
            label: "指针微动死区",
            controls: "过滤手指轻微颤动和触控板噪声。位移累计没有跨过这个半径前，不把它当成有效指针运动。",
            lower: "更灵敏，极小的手指移动也更容易让光标动；过低可能显得抖。",
            higher: "更稳，细小抖动更容易被吃掉；过高会让起步发黏、微调困难。",
            tuning: "先用很慢的一指微调观察。想减少光标抖动就逐步加大；觉得起步迟钝就减小。",
        },
        "pointer.tracking_speed" => FeelUiText {
            label: "指针整体速度",
            controls: "所有一指指针位移的全局倍率。它作用在速度增益曲线之外，相当于整体放大或缩小光标行程。",
            lower: "同样的手指位移让光标走得更短，整体更慢。",
            higher: "同样的手指位移让光标走得更远，整体更快。",
            tuning: "先用它确定整体行程，再用低速/高速增益分别修慢速精细操作和快速甩动。",
        },
        "pointer.min_gain" => FeelUiText {
            label: "指针低速增益",
            controls: "手指低速移动时的增益下限，主要影响小范围、精细定位时光标跟手程度。",
            lower: "慢速移动更克制，适合精细定位，但可能显得拖。",
            higher: "慢速移动更积极，细小动作带来更多光标位移。",
            tuning: "用慢速追一个小目标来调。它不应该高到让精细定位变得难控制。",
        },
        "pointer.max_gain" => FeelUiText {
            label: "指针高速增益上限",
            controls: "手指快速移动时加速曲线允许达到的最高增益，主要决定快速横跨屏幕时能甩多远。",
            lower: "高速甩动更受控，快速移动和慢速移动差异较小。",
            higher: "高速甩动更远，加速感更明显。",
            tuning: "用一次快速横向滑动测试是否能舒服地跨过常用屏幕距离。三指拖拽在 M19 还有独立的高速增益限制，不完全等于此值。",
        },
        "scroll.min_gain" => FeelUiText {
            label: "滚动低速增益",
            controls: "双指慢速滚动时的灵敏度下限，主要影响逐行阅读和小距离滚动。",
            lower: "慢滚更细、更克制。",
            higher: "轻微双指移动就会产生更多滚动距离。",
            tuning: "在网页或代码中做很慢的双指滚动，以是否容易停在目标行附近为准。",
        },
        "scroll.max_gain" => FeelUiText {
            label: "滚动高速增益上限",
            controls: "双指快速滚动时的最大增益，决定快速扫页面时单次手势能推进多远。",
            lower: "快速滚动更可控，但长页面需要更多次手势。",
            higher: "快速滚动推进更远，长页面浏览更快。",
            tuning: "和惯性一起调。若快速滚动本身已经很远，再把惯性拖得很长通常会显得失控。",
        },
        "scroll.axis_lock_engage_ratio" => FeelUiText {
            label: "滚动方向锁定门槛",
            controls: "决定双指滚动何时锁成纯水平或纯垂直。主方向位移必须比副方向明显到一定比例才会进入轴锁。",
            lower: "更容易锁轴，斜着一点也会较快变成纯水平/垂直。",
            higher: "只有方向非常明确时才锁轴，更多保留真实的二维滚动。",
            tuning: "如果竖向滚动经常带出横向漂移就减小；如果斜向滚动经常被强行掰直就增大。",
        },
        "scroll.axis_lock_release_ratio" => FeelUiText {
            label: "滚动方向解锁门槛",
            controls: "已经锁定某一轴后，另一方向要增长到什么程度才解除轴锁。该值必须小于“方向锁定门槛”。",
            lower: "锁住后更难被副方向打断，方向保持更牢。",
            higher: "副方向稍微增强就更容易解除当前轴锁。",
            tuning: "如果锁轴后很难自然转成斜向滚动就提高；如果锁轴经常被小抖动打破就降低。",
        },
        "scroll.momentum_tau_ms" => FeelUiText {
            label: "滚动惯性衰减时间",
            controls: "松手后惯性速度指数衰减的时间尺度。它主要控制惯性滚动能拖多久、速度下降得多快。",
            lower: "惯性更快衰减，松手后更快停。",
            higher: "惯性保持更久，页面会滑行更长时间。",
            tuning: "觉得滚动“刹不住”就减小；觉得惯性刚开始就没了就增大。",
        },
        "scroll.momentum_start_speed_mm_per_s" => FeelUiText {
            label: "惯性启动速度门槛",
            controls: "双指离开触控板时，估计滚动速度至少达到这个值才会进入软件惯性。",
            lower: "较慢的松手也会触发惯性，惯性更常出现。",
            higher: "只有明显快速甩动才会触发惯性，普通滚动更容易立即停止。",
            tuning: "如果轻轻滚一下也总在滑就提高；如果快速甩动也没有惯性就降低。",
        },
        "scroll.momentum_stop_speed_mm_per_s" => FeelUiText {
            label: "惯性停止速度门槛",
            controls: "惯性衰减过程中，速度降到这个值以下就直接结束滚动生命周期。必须低于惯性启动门槛。",
            lower: "允许惯性拖到更低的速度，尾巴更长。",
            higher: "更早截断低速尾巴，停止更干脆。",
            tuning: "如果最后有很长一段几乎不动但还在滑的尾巴就提高；想保留更柔和的收尾就降低。",
        },
        "gesture.pinch_commit_mm" => FeelUiText {
            label: "捏合手势确认距离",
            controls: "双指捏合/张开时，指间距离变化累计到多少毫米后才正式确认 pinch 手势。确认前仍处于候选阶段。",
            lower: "捏合更快被识别，但误触概率会上升。",
            higher: "需要更明显的捏合动作才确认，误触更少但响应更迟。",
            tuning: "如果正常缩放要做很大动作才触发就降低；滚动时容易误判成 pinch 就提高。",
        },
        "gesture.page_swipe_commit_mm" => FeelUiText {
            label: "双指翻页手势确认距离",
            controls: "双指 page-swipe 候选累计移动到多少毫米后才确认成翻页类连续手势。它和普通双指滚动存在 ownership 竞争。",
            lower: "翻页类手势更早确认。",
            higher: "需要更明显的双指扫动才确认，更不容易从普通滚动误入翻页。",
            tuning: "当前真实后端是否支持 passthrough/映射由桌面适配器决定；若主要使用普通滚动，可保持较保守的阈值。",
        },
        "gesture.multi_swipe_commit_mm" => FeelUiText {
            label: "三/四指滑动确认距离",
            controls: "三指或四指 swipe 累计移动到多少毫米后才正式确认。三指拖拽启用时，三指 ownership 会优先由拖拽策略处理。",
            lower: "多指桌面手势更快确认。",
            higher: "要移动更远才确认，误触更少但启动更迟。",
            tuning: "该值必须高于三指拖拽确认距离，保证拖拽在允许时先取得 ownership。",
        },
        "drag.commit_threshold_mm" => FeelUiText {
            label: "三指拖拽确认距离",
            controls: "普通/分阶段三指进入时，候选位移累计到多少毫米后确认拖拽。M19 的快速三指同时落下路径在 50 ms entry debounce 后会直接 arm，不再经过第二次该距离门槛。",
            lower: "普通三指候选更快进入拖拽，启动动作更轻。",
            higher: "需要更明显移动才进入拖拽，可减少普通候选误触。",
            tuning: "不要用它解决“猛地三指划拉”的 50 ms 快速入口问题；那条路径目前由固定 entry debounce 控制。该值必须小于三/四指 swipe 确认距离。",
        },
        "drag.drag_lock" => FeelUiText {
            label: "三指拖拽锁定",
            controls: "启用后，三指拖拽结束时可以继续保持拖拽 ownership，直到后续释放动作。M19 当前实机 profile 的三指 release 语义还会受 M19 专用策略约束。",
            lower: "关闭：更接近抬手即结束拖拽。",
            higher: "开启：允许抬手后继续保持拖拽，适合触控板边缘重新摆手。",
            tuning: "当前追求 macOS 三指拖拽手感时建议关闭 sticky lock；如果你明确需要抬手续拖再开启。",
        },
        _ => return None,
    })
}

#[derive(Clone, Copy)]
struct GestureUiText {
    label: &'static str,
    description: &'static str,
}

fn gesture_trigger_ui(name: &str) -> Option<GestureUiText> {
    Some(match name {
        "pinch-in" => GestureUiText {
            label: "双指捏合",
            description: "两指距离缩小达到 pinch 确认门槛后触发。",
        },
        "pinch-out" => GestureUiText {
            label: "双指张开",
            description: "两指距离增大达到 pinch 确认门槛后触发。",
        },
        "rotate-clockwise" => GestureUiText {
            label: "双指顺时针旋转",
            description: "双指相对角度产生顺时针旋转时触发。",
        },
        "rotate-counter-clockwise" => GestureUiText {
            label: "双指逆时针旋转",
            description: "双指相对角度产生逆时针旋转时触发。",
        },
        "two-finger-page-left" => GestureUiText {
            label: "双指向左翻页",
            description: "双指 page-swipe 向左确认后触发；与普通双指滚动区分。",
        },
        "two-finger-page-right" => GestureUiText {
            label: "双指向右翻页",
            description: "双指 page-swipe 向右确认后触发；与普通双指滚动区分。",
        },
        "two-finger-page-up" => GestureUiText {
            label: "双指向上翻页",
            description: "双指 page-swipe 向上确认后触发。",
        },
        "two-finger-page-down" => GestureUiText {
            label: "双指向下翻页",
            description: "双指 page-swipe 向下确认后触发。",
        },
        "three-finger-swipe-left" => GestureUiText {
            label: "三指左滑",
            description: "三指 swipe 向左确认后触发；启用三指拖拽时通常不会获得 swipe ownership。",
        },
        "three-finger-swipe-right" => GestureUiText {
            label: "三指右滑",
            description: "三指 swipe 向右确认后触发；启用三指拖拽时通常不会获得 swipe ownership。",
        },
        "three-finger-swipe-up" => GestureUiText {
            label: "三指上滑",
            description: "三指 swipe 向上确认后触发；启用三指拖拽时通常不会获得 swipe ownership。",
        },
        "three-finger-swipe-down" => GestureUiText {
            label: "三指下滑",
            description: "三指 swipe 向下确认后触发；启用三指拖拽时通常不会获得 swipe ownership。",
        },
        "four-finger-swipe-left" => GestureUiText {
            label: "四指左滑",
            description: "四指 swipe 向左确认后触发；适合绑定下一个桌面。",
        },
        "four-finger-swipe-right" => GestureUiText {
            label: "四指右滑",
            description: "四指 swipe 向右确认后触发；适合绑定上一个桌面。",
        },
        "four-finger-swipe-up" => GestureUiText {
            label: "四指上滑",
            description: "四指 swipe 向上确认后触发；适合绑定 Overview。",
        },
        "four-finger-swipe-down" => GestureUiText {
            label: "四指下滑",
            description: "四指 swipe 向下确认后触发；可绑定关闭 Overview 或窗口展示。",
        },
        "edge-swipe-left" => GestureUiText {
            label: "边缘左滑",
            description: "从触控板边缘进入并向左移动的 edge-swipe。",
        },
        "edge-swipe-right" => GestureUiText {
            label: "边缘右滑",
            description: "从触控板边缘进入并向右移动的 edge-swipe。",
        },
        "edge-swipe-up" => GestureUiText {
            label: "边缘上滑",
            description: "从触控板边缘进入并向上移动的 edge-swipe。",
        },
        "edge-swipe-down" => GestureUiText {
            label: "边缘下滑",
            description: "从触控板边缘进入并向下移动的 edge-swipe。",
        },
        "thumb-three-pinch" => GestureUiText {
            label: "拇指 + 三指捏合",
            description: "拇指与另外三指向内收拢的四指组合手势。",
        },
        "thumb-three-spread" => GestureUiText {
            label: "拇指 + 三指张开",
            description: "拇指与另外三指向外张开的四指组合手势。",
        },
        "three-finger-tap" => GestureUiText {
            label: "三指轻点",
            description: "三指形成 tap 候选并在未进入拖拽/swipe 的情况下完成轻点。",
        },
        _ => return None,
    })
}

fn gesture_target_ui(name: &str) -> Option<GestureUiText> {
    Some(match name {
        "passthrough" => GestureUiText {
            label: "原样传递",
            description: "保留连续手势事件，让支持该事件的桌面后端继续处理。当前后端不支持时会在 preflight/reload 被拒绝。",
        },
        "disabled" => GestureUiText {
            label: "禁用",
            description: "识别到该手势后不执行桌面动作。",
        },
        "next-workspace" => GestureUiText {
            label: "下一个桌面",
            description: "切换到下一个虚拟桌面 / workspace。",
        },
        "previous-workspace" => GestureUiText {
            label: "上一个桌面",
            description: "切换到上一个虚拟桌面 / workspace。",
        },
        "show-desktop" => GestureUiText {
            label: "显示桌面",
            description: "触发桌面环境的 Show Desktop 动作。",
        },
        "open-overview" => GestureUiText {
            label: "打开概览",
            description: "打开桌面环境的 Overview / Mission-Control 类界面。",
        },
        "close-overview" => GestureUiText {
            label: "关闭概览",
            description: "仅在概览已打开时关闭 Overview；当前 KDE 适配器会先读取状态避免错误 toggle。",
        },
        "present-windows" => GestureUiText {
            label: "展示窗口",
            description: "触发 Present Windows / Exposé 类窗口总览。",
        },
        "application-launcher" => GestureUiText {
            label: "应用启动器",
            description: "打开桌面环境的应用启动器。",
        },
        "notification-center" => GestureUiText {
            label: "通知中心",
            description: "请求打开通知中心；是否可执行取决于当前桌面后端。",
        },
        "page-next" => GestureUiText {
            label: "下一页",
            description: "发送语义上的 Page Next 动作；需要桌面后端提供对应实现。",
        },
        "page-previous" => GestureUiText {
            label: "上一页",
            description: "发送语义上的 Page Previous 动作；需要桌面后端提供对应实现。",
        },
        "smart-zoom" => GestureUiText {
            label: "智能缩放",
            description: "发送 Smart Zoom 语义动作；需要桌面后端提供对应实现。",
        },
        "lookup" => GestureUiText {
            label: "查询 / Lookup",
            description: "发送 Lookup 语义动作；需要桌面后端提供对应实现。",
        },
        _ => return None,
    })
}

pub(crate) fn read_settings(path: &Path) -> Result<UserSettings, CommandFailure> {
    let bytes = std::fs::read(path).map_err(|error| {
        CommandFailure::Config(format!(
            "could not read user settings {}: {error}",
            path.display()
        ))
    })?;
    let settings: UserSettings = serde_json::from_slice(&bytes).map_err(|error| {
        CommandFailure::Config(format!(
            "invalid user-settings JSON in {}: {error}",
            path.display()
        ))
    })?;
    settings
        .validate()
        .map_err(|error| CommandFailure::Config(error.to_string()))?;
    Ok(settings)
}

fn write_settings(path: &Path, settings: &UserSettings) -> Result<(), CommandFailure> {
    settings
        .validate()
        .map_err(|error| CommandFailure::Config(error.to_string()))?;
    let mut bytes = serde_json::to_vec_pretty(settings).map_err(|error| {
        CommandFailure::Unexpected(format!("could not encode user settings: {error}"))
    })?;
    bytes.push(b'\n');
    std::fs::write(path, bytes).map_err(|error| {
        CommandFailure::Config(format!(
            "could not write user settings {}: {error}",
            path.display()
        ))
    })
}

/// Writes the M17-compatible M18 default settings document.
pub fn run_default(env: &mut CommandEnv<'_>, output: &Path) -> Result<(), CommandFailure> {
    write_settings(output, &UserSettings::default())?;
    writeln!(env.out, "wrote default M18 settings: {}", output.display())
        .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))
}

/// Writes the documented macOS-inspired gesture preset.
pub fn run_macos(env: &mut CommandEnv<'_>, output: &Path) -> Result<(), CommandFailure> {
    write_settings(output, &UserSettings::macos_inspired())?;
    writeln!(
        env.out,
        "wrote macos-inspired M18 settings: {}",
        output.display()
    )
    .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))
}

/// Strictly validates one unified settings file.
pub fn run_check(env: &mut CommandEnv<'_>, input: &Path) -> Result<(), CommandFailure> {
    let settings = read_settings(input)?;
    writeln!(
        env.out,
        "OK settings-version={} feel-parameters={} gesture-bindings={}",
        settings.version,
        feel_parameter_specs().len(),
        settings.gestures.bindings.len()
    )
    .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))
}

/// Prints one normalized validated settings file.
pub fn run_show(env: &mut CommandEnv<'_>, input: &Path) -> Result<(), CommandFailure> {
    let settings = read_settings(input)?;
    serde_json::to_writer_pretty(&mut *env.out, &settings).map_err(|error| {
        CommandFailure::Unexpected(format!("could not write settings JSON: {error}"))
    })?;
    writeln!(env.out)
        .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))
}

/// Applies transactional `key=value` edits to one settings file.
pub fn run_set(
    env: &mut CommandEnv<'_>,
    input: &Path,
    output: &Path,
    edits: &[String],
) -> Result<(), CommandFailure> {
    let mut settings = read_settings(input)?;
    for edit in edits {
        let (key, value) = edit.split_once('=').ok_or_else(|| {
            CommandFailure::Config(format!("settings edit must be key=value, got {edit:?}"))
        })?;
        settings
            .set_key(key, value)
            .map_err(|error| CommandFailure::Config(error.to_string()))?;
    }
    write_settings(output, &settings)?;
    writeln!(env.out, "wrote updated settings: {}", output.display())
        .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))
}

/// Applies one or more validated edits in-place. This is the convenient M19
/// companion for a running `--watch-settings` session.
pub fn run_patch(
    env: &mut CommandEnv<'_>,
    input: &Path,
    edits: &[String],
) -> Result<(), CommandFailure> {
    let mut settings = read_settings(input)?;
    for edit in edits {
        let (key, value) = edit.split_once('=').ok_or_else(|| {
            CommandFailure::Config(format!("settings edit must be key=value, got {edit:?}"))
        })?;
        settings
            .set_key(key, value)
            .map_err(|error| CommandFailure::Config(error.to_string()))?;
    }
    write_settings(input, &settings)?;
    writeln!(env.out, "patched settings in-place: {}", input.display())
        .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))
}

/// Generates the self-contained offline M18 settings editor.
pub fn run_gui(
    env: &mut CommandEnv<'_>,
    input: &Path,
    output: &Path,
) -> Result<(), CommandFailure> {
    let settings = read_settings(input)?;
    let settings_json = serde_json::to_string(&settings).map_err(|error| {
        CommandFailure::Unexpected(format!("could not encode settings: {error}"))
    })?;
    let feel_specs = serde_json::to_string(
        &feel_parameter_specs()
            .iter()
            .map(|spec| {
                let ui = feel_ui_text(spec.key).expect("every feel parameter has GUI help text");
                serde_json::json!({
                    "key": spec.key,
                    "group": spec.group,
                    "unit": spec.unit,
                    "min": spec.min,
                    "max": spec.max,
                    "step": spec.step,
                    "effect": spec.effect,
                    "label": ui.label,
                    "controls": ui.controls,
                    "lower": ui.lower,
                    "higher": ui.higher,
                    "tuning": ui.tuning,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| CommandFailure::Unexpected(error.to_string()))?;
    let triggers = serde_json::to_string(
        &ALL_GESTURE_TRIGGERS
            .iter()
            .map(|trigger| {
                let name = trigger.name();
                let ui = gesture_trigger_ui(name).expect("every gesture trigger has GUI help text");
                serde_json::json!({
                    "name": name,
                    "label": ui.label,
                    "description": ui.description,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| CommandFailure::Unexpected(error.to_string()))?;
    let targets = serde_json::to_string(
        &ALL_GESTURE_TARGETS
            .iter()
            .map(|target| {
                let name = target.name();
                let ui = gesture_target_ui(name).expect("every gesture target has GUI help text");
                serde_json::json!({
                    "name": name,
                    "label": ui.label,
                    "description": ui.description,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| CommandFailure::Unexpected(error.to_string()))?;
    std::fs::write(
        output,
        html_document(&settings_json, &feel_specs, &triggers, &targets),
    )
    .map_err(|error| {
        CommandFailure::Config(format!(
            "could not write settings GUI {}: {error}",
            output.display()
        ))
    })?;
    writeln!(
        env.out,
        "wrote offline M18 settings GUI: {}",
        output.display()
    )
    .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))
}

fn html_document(settings: &str, feel_specs: &str, triggers: &str, targets: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Touchpad Settings</title><style>body{{font-family:system-ui,-apple-system,sans-serif;max-width:1180px;margin:28px auto;padding:0 18px;background:#111;color:#eee;line-height:1.55}}h1,h2,h3,p{{margin-top:.6em}}fieldset{{border:1px solid #444;border-radius:12px;margin:18px 0;padding:16px}}legend{{font-weight:750;font-size:1.1rem}}.param{{border-top:1px solid #303030;padding:16px 0}}.param:first-of-type{{border-top:0}}.param-head{{display:flex;justify-content:space-between;gap:16px;align-items:baseline;flex-wrap:wrap}}.param-title{{font-weight:700;font-size:1rem}}code.key{{color:#9ecbff;font-size:.82rem}}.range-note{{color:#999;font-size:.82rem}}.control{{display:grid;grid-template-columns:minmax(240px,1fr) 130px;gap:12px;align-items:center;margin:10px 0}}input,select{{background:#222;color:#fff;border:1px solid #555;border-radius:7px;padding:7px}}input[type=range]{{width:100%;padding:0}}.explain{{display:grid;grid-template-columns:1fr 1fr;gap:9px 16px;margin-top:10px}}.explain p{{margin:0;color:#ccc}}.explain strong{{color:#eee}}.controls{{grid-column:1/-1}}.tuning{{grid-column:1/-1;background:#191919;border-radius:7px;padding:9px 11px}}button{{padding:9px 14px;margin:8px 8px 8px 0;background:#222;color:#fff;border:1px solid #666;border-radius:7px}}pre{{background:#181818;padding:12px;overflow:auto;border-radius:8px}}.note{{color:#bbb}}.warning{{background:#2a2215;border:1px solid #6c5423;padding:10px 12px;border-radius:8px;color:#e7d2a2}}.bad{{color:#ff8d8d}}.ok{{color:#8dffad}}.gesture{{display:grid;grid-template-columns:minmax(220px,1fr) minmax(220px,1fr);gap:8px 18px;border-top:1px solid #303030;padding:13px 0}}.gesture:first-child{{border-top:0}}.gesture-name{{font-weight:700}}.gesture-desc,.target-desc{{color:#aaa;font-size:.86rem}}.gesture select{{width:100%}}.drag-toggle{{background:#191919;border-radius:9px;padding:12px;margin-bottom:14px}}@media(max-width:720px){{.control,.explain,.gesture{{grid-template-columns:1fr}}}}</style></head><body>
<h1>Touchpad 设置</h1><p class="note">离线编辑器。页面不会访问触控板、portal、KDE、网络或正在运行的 session。导出 JSON 后可用 <code>touchpadctl settings-check</code> 验证；M19 <code>--watch-settings</code> 可在 neutral boundary 热加载。</p>
<p class="warning"><strong>参数说明按当前 M19 实现编写。</strong> 三指快速同时落下的路径使用固定 50 ms entry debounce，并在窗口结束后直接 arm；因此“三指拖拽确认距离”并不控制这条快速入口的第二次位移门槛。</p>
<div id="feel"></div><fieldset><legend>手势绑定</legend><div id="gestures"></div></fieldset><p id="status" class="ok">已加载并验证当前设置。</p><button id="download">导出 settings.json</button><button id="reset">恢复载入时设置</button><details><summary>查看 JSON</summary><pre id="json"></pre></details>
<script>const initial={settings};const specs={feel_specs};const triggers={triggers};const targets={targets};let cfg=structuredClone(initial);const get=(k)=>k.split('.').reduce((o,p)=>o[p],cfg.feel);const set=(k,v)=>{{const p=k.split('.');let o=cfg.feel;for(let i=0;i<p.length-1;i++)o=o[p[i]];o[p.at(-1)]=v;}};
const groupNames={{Pointer:'指针',Scroll:'双指滚动',Gestures:'手势识别','Three-finger drag':'三指拖拽'}};
function feelUI(){{const root=document.getElementById('feel');root.innerHTML='';for(const group of [...new Set(specs.map(s=>s.group))]){{const fs=document.createElement('fieldset'),lg=document.createElement('legend');lg.textContent=groupNames[group]||group;fs.append(lg);for(const sp of specs.filter(s=>s.group===group)){{const card=document.createElement('div');card.className='param';const head=document.createElement('div');head.className='param-head';const title=document.createElement('div');title.innerHTML='<span class="param-title">'+sp.label+'</span> &nbsp;<code class="key">feel.'+sp.key+'</code>';const range=document.createElement('div');range.className='range-note';range.textContent=sp.min===null?'布尔开关':'范围 '+sp.min+' ～ '+sp.max+' '+sp.unit+'，步进 '+sp.step;head.append(title,range);card.append(head);const ctl=document.createElement('div');ctl.className='control';if(sp.min===null){{const b=document.createElement('input');b.type='checkbox';b.checked=get(sp.key);b.onchange=()=>{{set(sp.key,b.checked);update();}};const state=document.createElement('span');state.textContent=b.checked?'开启':'关闭';b.onchange=()=>{{set(sp.key,b.checked);state.textContent=b.checked?'开启':'关闭';update();}};ctl.append(b,state);}}else{{const q=document.createElement('input'),n=document.createElement('input');q.type='range';q.min=sp.min;q.max=sp.max;q.step=sp.step;q.value=get(sp.key);n.type='number';n.min=sp.min;n.max=sp.max;n.step=sp.step;n.value=get(sp.key);const c=v=>{{let x=Number(v);if(sp.key.endsWith('momentum_tau_ms'))x=Math.round(x);set(sp.key,x);q.value=x;n.value=x;update();}};q.oninput=()=>c(q.value);n.onchange=()=>c(n.value);ctl.append(q,n);}}card.append(ctl);const ex=document.createElement('div');ex.className='explain';ex.innerHTML='<p class="controls"><strong>控制什么：</strong>'+sp.controls+'</p><p><strong>调小：</strong>'+sp.lower+'</p><p><strong>调大：</strong>'+sp.higher+'</p><p class="tuning"><strong>建议怎么测：</strong>'+sp.tuning+'</p>';card.append(ex);fs.append(card);}}root.append(fs);}}}}
function gestureUI(){{const root=document.getElementById('gestures');root.innerHTML='';const drag=document.createElement('div');drag.className='drag-toggle';const db=document.createElement('input');db.type='checkbox';db.checked=cfg.gestures.three_finger_drag_enabled;db.onchange=()=>{{cfg.gestures.three_finger_drag_enabled=db.checked;update();}};const dl=document.createElement('label');dl.append(db,document.createTextNode(' 启用三指拖拽'));const dk=document.createElement('div');dk.innerHTML='<code class="key">gesture.three-finger-drag-enabled</code>';const dn=document.createElement('div');dn.className='gesture-desc';dn.textContent='开启后，三指平移优先由拖拽策略取得 ownership，三指 swipe 通常不可达；四指 swipe 不受影响。关闭后可重新把三指 swipe 用作桌面切换等动作。三指 tap candidate 仍可保留。';drag.append(dl,dk,dn);root.append(drag);for(const t of triggers){{const r=document.createElement('div');r.className='gesture';const left=document.createElement('div');const name=document.createElement('div');name.className='gesture-name';name.textContent=t.label;const key=document.createElement('code');key.className='key';key.textContent='gesture.'+t.name;const desc=document.createElement('div');desc.className='gesture-desc';desc.textContent=t.description;left.append(name,key,desc);const right=document.createElement('div');const s=document.createElement('select');for(const x of targets){{const o=document.createElement('option');o.value=x.name;o.textContent=x.label+'  ('+x.name+')';s.append(o);}}s.value=cfg.gestures.bindings[t.name];const td=document.createElement('div');td.className='target-desc';const refresh=()=>{{const x=targets.find(x=>x.name===s.value);td.textContent=x?x.description:'';}};s.onchange=()=>{{cfg.gestures.bindings[t.name]=s.value;refresh();update();}};refresh();right.append(s,td);r.append(left,right);root.append(r);}}}}
function validate(){{const p=cfg.feel.pointer,s=cfg.feel.scroll,g=cfg.feel.gesture,d=cfg.feel.drag;if(p.max_gain<p.min_gain)return'pointer max_gain < min_gain';if(s.max_gain<s.min_gain)return'scroll max_gain < min_gain';if(s.axis_lock_release_ratio>=s.axis_lock_engage_ratio)return'axis lock release must be below engage';if(s.momentum_stop_speed_mm_per_s>=s.momentum_start_speed_mm_per_s)return'momentum stop must be below start';if(d.commit_threshold_mm>=g.multi_swipe_commit_mm)return'drag threshold must stay below multi-swipe threshold';return'';}}
function update(){{const e=validate(),st=document.getElementById('status');st.textContent=e||'当前参数满足编辑器约束。';st.className=e?'bad':'ok';document.getElementById('download').disabled=!!e;document.getElementById('json').textContent=JSON.stringify(cfg,null,2);}}function render(){{feelUI();gestureUI();update();}}document.getElementById('download').onclick=()=>{{const b=new Blob([JSON.stringify(cfg,null,2)+'\n'],{{type:'application/json'}}),u=URL.createObjectURL(b),a=document.createElement('a');a.href=u;a.download='settings.json';a.click();URL.revokeObjectURL(u);}};document.getElementById('reset').onclick=()=>{{cfg=structuredClone(initial);render();}};render();</script></body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("touchpad-m18-{name}-{nonce}"))
    }

    #[test]
    fn unified_settings_set_edits_feel_and_gesture() {
        let path = temp("settings.json");
        let mut settings = UserSettings::default();
        settings
            .set_key("feel.pointer.tracking_speed", "1.25")
            .unwrap();
        settings
            .set_key("gesture.three-finger-swipe-up", "open-overview")
            .unwrap();
        write_settings(&path, &settings).unwrap();
        let read = read_settings(&path).unwrap();
        assert_eq!(read.feel.pointer.tracking_speed, 1.25);
        assert_eq!(
            read.gestures
                .target(touchpad_core::GestureTrigger::ThreeFingerSwipeUp)
                .name(),
            "open-overview"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn gui_is_offline_and_contains_gesture_controls() {
        let html = html_document("{}", "[]", "[]", "[]");
        assert!(html.contains("手势绑定"));
        assert!(html.contains("控制什么"));
        assert!(html.contains("调小"));
        assert!(html.contains("调大"));
        assert!(html.contains("50 ms entry debounce"));
        assert!(html.contains("离线编辑器"));
        assert!(!html.contains("fetch("));
        assert!(!html.contains("WebSocket"));
        assert!(!html.contains("https://"));
    }

    #[test]
    fn gui_help_covers_every_exposed_parameter_trigger_and_target() {
        for spec in feel_parameter_specs() {
            assert!(
                feel_ui_text(spec.key).is_some(),
                "missing GUI help for feel parameter {}",
                spec.key
            );
        }
        for trigger in ALL_GESTURE_TRIGGERS {
            assert!(
                gesture_trigger_ui(trigger.name()).is_some(),
                "missing GUI help for gesture trigger {}",
                trigger.name()
            );
        }
        for target in ALL_GESTURE_TARGETS {
            assert!(
                gesture_target_ui(target.name()).is_some(),
                "missing GUI help for gesture target {}",
                target.name()
            );
        }
    }

    #[test]
    fn in_place_patch_uses_the_same_strict_settings_schema() {
        let path = temp("patch.json");
        write_settings(&path, &UserSettings::default()).unwrap();
        let mut settings = read_settings(&path).unwrap();
        settings
            .set_key("feel.pointer.tracking_speed", "1.3")
            .unwrap();
        settings
            .set_key("gesture.three-finger-swipe-up", "open-overview")
            .unwrap();
        write_settings(&path, &settings).unwrap();
        let patched = read_settings(&path).unwrap();
        assert_eq!(patched.feel.pointer.tracking_speed, 1.3);
        assert_eq!(
            patched
                .gestures
                .target(touchpad_core::GestureTrigger::ThreeFingerSwipeUp)
                .name(),
            "open-overview"
        );
        let _ = std::fs::remove_file(path);
    }
}
