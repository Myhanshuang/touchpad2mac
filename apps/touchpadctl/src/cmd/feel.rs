//! M17 feel-overlay CLI and self-contained offline HTML editor generation.

#![forbid(unsafe_code)]

use std::path::Path;

use touchpad_core::{feel_parameter_specs, FeelConfig};

use crate::env::CommandEnv;
use crate::exit::CommandFailure;

pub(crate) fn read_config(path: &Path) -> Result<FeelConfig, CommandFailure> {
    let bytes = std::fs::read(path).map_err(|error| {
        CommandFailure::Config(format!(
            "could not read feel config {}: {error}",
            path.display()
        ))
    })?;
    let config: FeelConfig = serde_json::from_slice(&bytes).map_err(|error| {
        CommandFailure::Config(format!("invalid feel JSON in {}: {error}", path.display()))
    })?;
    config
        .validate()
        .map_err(|error| CommandFailure::Config(error.to_string()))?;
    Ok(config)
}

fn write_config(path: &Path, config: &FeelConfig) -> Result<(), CommandFailure> {
    config
        .validate()
        .map_err(|error| CommandFailure::Config(error.to_string()))?;
    let mut bytes = serde_json::to_vec_pretty(config).map_err(|error| {
        CommandFailure::Unexpected(format!("could not encode feel JSON: {error}"))
    })?;
    bytes.push(b'\n');
    std::fs::write(path, bytes).map_err(|error| {
        CommandFailure::Config(format!(
            "could not write feel config {}: {error}",
            path.display()
        ))
    })?;
    Ok(())
}

/// Writes the exact M17/M16-equivalent default tuning document.
pub fn run_default(env: &mut CommandEnv<'_>, output: &Path) -> Result<(), CommandFailure> {
    write_config(output, &FeelConfig::default())?;
    writeln!(
        env.out,
        "wrote M17 default feel config: {}",
        output.display()
    )
    .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))?;
    Ok(())
}

/// Strictly checks a feel document.
pub fn run_check(env: &mut CommandEnv<'_>, input: &Path) -> Result<(), CommandFailure> {
    let config = read_config(input)?;
    writeln!(
        env.out,
        "OK feel-version={} parameters={}",
        config.version,
        feel_parameter_specs().len()
    )
    .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))?;
    Ok(())
}

/// Prints a normalized validated feel document.
pub fn run_show(env: &mut CommandEnv<'_>, input: &Path) -> Result<(), CommandFailure> {
    let config = read_config(input)?;
    serde_json::to_writer_pretty(&mut *env.out, &config).map_err(|error| {
        CommandFailure::Unexpected(format!("could not write feel JSON: {error}"))
    })?;
    writeln!(env.out)
        .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))?;
    Ok(())
}

/// Applies `key=value` edits transactionally in memory and writes the output
/// only after the whole resulting document validates.
pub fn run_set(
    env: &mut CommandEnv<'_>,
    input: &Path,
    output: &Path,
    edits: &[String],
) -> Result<(), CommandFailure> {
    let mut config = read_config(input)?;
    for edit in edits {
        let (key, value) = edit.split_once('=').ok_or_else(|| {
            CommandFailure::Config(format!("feel edit must be key=value, got {edit:?}"))
        })?;
        config
            .set_key(key, value)
            .map_err(|error| CommandFailure::Config(error.to_string()))?;
    }
    write_config(output, &config)?;
    writeln!(env.out, "wrote tuned feel config: {}", output.display())
        .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))?;
    Ok(())
}

/// Generates one self-contained HTML editor. It has no external scripts,
/// styles, network calls, server, device access, or live-apply path.
pub fn run_gui(
    env: &mut CommandEnv<'_>,
    input: &Path,
    output: &Path,
) -> Result<(), CommandFailure> {
    let config = read_config(input)?;
    let config_json = serde_json::to_string(&config).map_err(|error| {
        CommandFailure::Unexpected(format!("could not encode feel JSON: {error}"))
    })?;
    let specs_json = serde_json::to_string(
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
    .map_err(|error| {
        CommandFailure::Unexpected(format!("could not encode GUI metadata: {error}"))
    })?;
    let html = html_document(&config_json, &specs_json);
    std::fs::write(output, html).map_err(|error| {
        CommandFailure::Config(format!("could not write GUI {}: {error}", output.display()))
    })?;
    writeln!(env.out, "wrote offline M17 feel GUI: {}", output.display())
        .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))?;
    Ok(())
}

fn html_document(config_json: &str, specs_json: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Touchpad M17 Feel Tuner</title><style>
body{{font-family:system-ui,sans-serif;max-width:980px;margin:32px auto;padding:0 18px;background:#111;color:#eee}}
h1{{margin-bottom:4px}} .note{{color:#bbb;margin-bottom:24px}} fieldset{{border:1px solid #444;border-radius:10px;margin:16px 0;padding:16px}}
legend{{font-weight:700;padding:0 8px}} .row{{display:grid;grid-template-columns:minmax(220px,1.2fr) 2fr 100px;gap:12px;align-items:center;margin:13px 0}}
input[type=range]{{width:100%}} input[type=number]{{width:92px;background:#222;color:#fff;border:1px solid #555;padding:6px;border-radius:6px}}
.effect{{font-size:.86rem;color:#aaa;grid-column:1/-1;margin-top:-8px}} button{{padding:9px 14px;margin-right:8px;border-radius:7px;border:1px solid #666;background:#222;color:#fff;cursor:pointer}}
pre{{background:#181818;padding:14px;border-radius:8px;overflow:auto}} .bad{{color:#ff8d8d}} .ok{{color:#8dffad}}
</style></head><body><h1>Touchpad M17 Feel Tuner</h1>
<div class="note">Offline editor only. It never accesses your touchpad, portal, KDE, network, or live session. Export JSON, validate with <code>touchpadctl feel-check</code>, then use the explicit bounded M17 takeover path.</div>
<div id="controls"></div><p id="status" class="ok">Loaded validated config.</p>
<button id="download">Export feel.json</button><button id="reset">Reset loaded values</button>
<pre id="json"></pre><script>
const initial={config_json}; const specs={specs_json}; let cfg=structuredClone(initial);
const get=(k)=>k.split('.').reduce((o,p)=>o[p],cfg); const set=(k,v)=>{{const p=k.split('.');let o=cfg;for(let i=0;i<p.length-1;i++)o=o[p[i]];o[p.at(-1)]=v;}};
function validate(){{const p=cfg.pointer,s=cfg.scroll,g=cfg.gesture,d=cfg.drag;if(p.max_gain<p.min_gain)return 'pointer max gain must be >= min gain';if(s.max_gain<s.min_gain)return 'scroll max gain must be >= min gain';if(s.axis_lock_release_ratio>=s.axis_lock_engage_ratio)return 'axis-lock release ratio must be below engage ratio';if(s.momentum_stop_speed_mm_per_s>=s.momentum_start_speed_mm_per_s)return 'momentum stop speed must be below start speed';if(d.commit_threshold_mm>=g.multi_swipe_commit_mm)return 'three-finger drag threshold must remain below multi-swipe threshold';return '';}}
function render(){{const root=document.getElementById('controls');root.innerHTML='';for(const group of [...new Set(specs.map(s=>s.group))]){{const fs=document.createElement('fieldset');const le=document.createElement('legend');le.textContent=group;fs.append(le);for(const sp of specs.filter(s=>s.group===group)){{const row=document.createElement('div');row.className='row';const lab=document.createElement('label');lab.textContent=sp.key+' ('+sp.unit+')';row.append(lab);if(sp.min===null){{const box=document.createElement('input');box.type='checkbox';box.checked=get(sp.key);box.onchange=()=>{{set(sp.key,box.checked);update();}};row.append(box);row.append(document.createElement('span'));}}else{{const range=document.createElement('input');range.type='range';range.min=sp.min;range.max=sp.max;range.step=sp.step;range.value=get(sp.key);const num=document.createElement('input');num.type='number';num.min=sp.min;num.max=sp.max;num.step=sp.step;num.value=get(sp.key);const change=v=>{{const n=sp.key.endsWith('momentum_tau_ms')?Math.round(Number(v)):Number(v);set(sp.key,n);range.value=n;num.value=n;update();}};range.oninput=()=>change(range.value);num.onchange=()=>change(num.value);row.append(range);row.append(num);}}const ef=document.createElement('div');ef.className='effect';ef.textContent=sp.effect;row.append(ef);fs.append(row);}}root.append(fs);}}update();}}
function update(){{const err=validate(),st=document.getElementById('status');st.textContent=err||'Config satisfies GUI cross-field constraints.';st.className=err?'bad':'ok';document.getElementById('json').textContent=JSON.stringify(cfg,null,2);document.getElementById('download').disabled=!!err;}}
document.getElementById('download').onclick=()=>{{const b=new Blob([JSON.stringify(cfg,null,2)+'\n'],{{type:'application/json'}}),u=URL.createObjectURL(b),a=document.createElement('a');a.href=u;a.download='feel.json';a.click();URL.revokeObjectURL(u);}};
document.getElementById('reset').onclick=()=>{{cfg=structuredClone(initial);render();}};render();
</script></body></html>"#
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
        std::env::temp_dir().join(format!(
            "touchpad-m17-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn write_read_and_set_share_the_same_schema() {
        let input = temp("in.json");
        let output = temp("out.json");
        write_config(&input, &FeelConfig::default()).unwrap();
        let mut cfg = read_config(&input).unwrap();
        cfg.set_key("pointer.tracking_speed", "1.25").unwrap();
        write_config(&output, &cfg).unwrap();
        assert_eq!(read_config(&output).unwrap().pointer.tracking_speed, 1.25);
        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn generated_gui_is_self_contained_and_has_no_live_or_network_path() {
        let cfg = serde_json::to_string(&FeelConfig::default()).unwrap();
        let specs = "[]";
        let html = html_document(&cfg, specs);
        assert!(html.contains("Offline editor only"));
        assert!(html.contains("Export feel.json"));
        assert!(!html.contains("https://"));
        assert!(!html.contains("fetch("));
        assert!(!html.contains("WebSocket"));
    }
}
