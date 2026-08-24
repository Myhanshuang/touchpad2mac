//! M18 unified user settings CLI and offline editor.

#![forbid(unsafe_code)]

use std::path::Path;

use touchpad_core::{
    feel_parameter_specs, UserSettings, ALL_GESTURE_TARGETS, ALL_GESTURE_TRIGGERS,
};

use crate::env::CommandEnv;
use crate::exit::CommandFailure;

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
                serde_json::json!({
                    "key": spec.key,
                    "group": spec.group,
                    "unit": spec.unit,
                    "min": spec.min,
                    "max": spec.max,
                    "step": spec.step,
                    "effect": spec.effect,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| CommandFailure::Unexpected(error.to_string()))?;
    let triggers = serde_json::to_string(
        &ALL_GESTURE_TRIGGERS
            .iter()
            .map(|trigger| trigger.name())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| CommandFailure::Unexpected(error.to_string()))?;
    let targets = serde_json::to_string(
        &ALL_GESTURE_TARGETS
            .iter()
            .map(|target| target.name())
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
<title>Touchpad Settings</title><style>body{{font-family:system-ui;max-width:1050px;margin:28px auto;padding:0 18px;background:#111;color:#eee}}fieldset{{border:1px solid #444;border-radius:10px;margin:14px 0;padding:14px}}legend{{font-weight:700}}.row{{display:grid;grid-template-columns:minmax(250px,1.2fr) 2fr 130px;gap:10px;align-items:center;margin:10px 0}}input,select{{background:#222;color:#fff;border:1px solid #555;border-radius:6px;padding:6px}}input[type=range]{{width:100%}}.effect{{grid-column:1/-1;color:#aaa;font-size:.86rem}}button{{padding:9px 14px;margin:8px 8px 8px 0;background:#222;color:#fff;border:1px solid #666;border-radius:7px}}pre{{background:#181818;padding:12px;overflow:auto;border-radius:8px}}.note{{color:#bbb}}.bad{{color:#ff8d8d}}.ok{{color:#8dffad}}</style></head><body>
<h1>Touchpad M18 Settings</h1><p class="note">Offline editor. No touchpad, portal, KDE, network, or live session access. Export JSON and validate with <code>touchpadctl settings-check</code>.</p>
<div id="feel"></div><fieldset><legend>Gesture actions</legend><div id="gestures"></div></fieldset><p id="status" class="ok">Loaded validated settings.</p><button id="download">Export settings.json</button><button id="reset">Reset</button><pre id="json"></pre>
<script>const initial={settings};const specs={feel_specs};const triggers={triggers};const targets={targets};let cfg=structuredClone(initial);const get=(k)=>k.split('.').reduce((o,p)=>o[p],cfg.feel);const set=(k,v)=>{{const p=k.split('.');let o=cfg.feel;for(let i=0;i<p.length-1;i++)o=o[p[i]];o[p.at(-1)]=v;}};
function feelUI(){{const root=document.getElementById('feel');root.innerHTML='';for(const group of [...new Set(specs.map(s=>s.group))]){{const fs=document.createElement('fieldset'),lg=document.createElement('legend');lg.textContent=group;fs.append(lg);for(const sp of specs.filter(s=>s.group===group)){{const r=document.createElement('div');r.className='row';const l=document.createElement('label');l.textContent=sp.key+' ('+sp.unit+')';r.append(l);if(sp.min===null){{const b=document.createElement('input');b.type='checkbox';b.checked=get(sp.key);b.onchange=()=>{{set(sp.key,b.checked);update();}};r.append(b);r.append(document.createElement('span'));}}else{{const q=document.createElement('input'),n=document.createElement('input');q.type='range';q.min=sp.min;q.max=sp.max;q.step=sp.step;q.value=get(sp.key);n.type='number';n.min=sp.min;n.max=sp.max;n.step=sp.step;n.value=get(sp.key);const c=v=>{{let x=Number(v);if(sp.key.endsWith('momentum_tau_ms'))x=Math.round(x);set(sp.key,x);q.value=x;n.value=x;update();}};q.oninput=()=>c(q.value);n.onchange=()=>c(n.value);r.append(q);r.append(n);}}const e=document.createElement('div');e.className='effect';e.textContent=sp.effect;r.append(e);fs.append(r);}}root.append(fs);}}}}
function gestureUI(){{const root=document.getElementById('gestures');root.innerHTML='';const drag=document.createElement('div');drag.className='row';const dl=document.createElement('label');dl.textContent='three-finger-drag-enabled';const db=document.createElement('input');db.type='checkbox';db.checked=cfg.gestures.three_finger_drag_enabled;db.onchange=()=>{{cfg.gestures.three_finger_drag_enabled=db.checked;update();}};const dn=document.createElement('span');dn.textContent='Disable this when assigning three-finger swipes; tap recognition remains available.';drag.append(dl,db,dn);root.append(drag);for(const t of triggers){{const r=document.createElement('div');r.className='row';const l=document.createElement('label');l.textContent=t;const s=document.createElement('select');for(const x of targets){{const o=document.createElement('option');o.value=x;o.textContent=x;s.append(o);}}s.value=cfg.gestures.bindings[t];s.onchange=()=>{{cfg.gestures.bindings[t]=s.value;update();}};r.append(l);r.append(s);root.append(r);}}}}
function validate(){{const p=cfg.feel.pointer,s=cfg.feel.scroll,g=cfg.feel.gesture,d=cfg.feel.drag;if(p.max_gain<p.min_gain)return'pointer max_gain < min_gain';if(s.max_gain<s.min_gain)return'scroll max_gain < min_gain';if(s.axis_lock_release_ratio>=s.axis_lock_engage_ratio)return'axis lock release must be below engage';if(s.momentum_stop_speed_mm_per_s>=s.momentum_start_speed_mm_per_s)return'momentum stop must be below start';if(d.commit_threshold_mm>=g.multi_swipe_commit_mm)return'drag threshold must stay below multi-swipe threshold';return'';}}
function update(){{const e=validate(),st=document.getElementById('status');st.textContent=e||'Settings satisfy editor constraints.';st.className=e?'bad':'ok';document.getElementById('download').disabled=!!e;document.getElementById('json').textContent=JSON.stringify(cfg,null,2);}}function render(){{feelUI();gestureUI();update();}}document.getElementById('download').onclick=()=>{{const b=new Blob([JSON.stringify(cfg,null,2)+'\n'],{{type:'application/json'}}),u=URL.createObjectURL(b),a=document.createElement('a');a.href=u;a.download='settings.json';a.click();URL.revokeObjectURL(u);}};document.getElementById('reset').onclick=()=>{{cfg=structuredClone(initial);render();}};render();</script></body></html>"#
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
        assert!(html.contains("Gesture actions"));
        assert!(html.contains("Offline editor"));
        assert!(!html.contains("fetch("));
        assert!(!html.contains("WebSocket"));
        assert!(!html.contains("https://"));
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
