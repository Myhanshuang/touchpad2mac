# M17 Feel Tuning

M17 adds an explicit standalone `FeelConfig` v1 overlay. It contains only
parameters that strongly change perceived pointer/scroll/gesture feel. Device
selection, takeover opt-ins, output qualification, cleanup, reconnect, tap
timing and service policy stay outside this file.

## CLI workflow

```text
touchpadctl feel-default feel.json
touchpadctl feel-check feel.json
touchpadctl feel-show feel.json
touchpadctl feel-set feel.json feel-fast.json pointer.tracking_speed=1.25 scroll.momentum_tau_ms=450
touchpadctl feel-gui feel.json feel-tuner.html
```

The generated HTML is self-contained and offline. Open it in a browser, edit
with sliders/numeric fields, then export `feel.json`. The page has no network,
server, device access or live-apply path.

To use a tuned file in the existing bounded live path, M17 requires both the
explicit profile and the file:

```text
touchpadctl takeover DEVICE TRACE \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m17-tunable-v1 --feel-config feel.json \
  --max-duration-seconds N
```

`--feel-config` is rejected for M10–M16. `m17-tunable-v1` without it is also
rejected. Validation finishes before output/device/recorder/grab side effects.

## Exposed parameters

| Key | Range | Effect |
|---|---:|---|
| `pointer.dead_zone_radius_mm` | 0.01–0.30 mm | Higher suppresses more micro-jitter; excessive values feel sticky |
| `pointer.tracking_speed` | 0.25–4.0× | Global pointer travel multiplier |
| `pointer.min_gain` | 0.5–2.0× | Slow precision-motion gain |
| `pointer.max_gain` | 0.5–4.0× | Fast-motion acceleration ceiling; must be ≥ min gain |
| `scroll.min_gain` | 0.5–2.0× | Slow scroll sensitivity |
| `scroll.max_gain` | 0.5–4.0× | Fast scroll sensitivity; must be ≥ min gain |
| `scroll.axis_lock_engage_ratio` | 1.2–6.0 | Higher requires cleaner directional dominance before locking |
| `scroll.axis_lock_release_ratio` | 1.05–4.0 | Existing lock release sensitivity; must stay below engage ratio |
| `scroll.momentum_tau_ms` | 50–1200 ms | Higher coasts longer |
| `scroll.momentum_start_speed_mm_per_s` | 10–200 mm/s | Higher requires faster release to start inertia |
| `scroll.momentum_stop_speed_mm_per_s` | 1–50 mm/s | Higher ends inertia earlier; must stay below start speed |
| `gesture.pinch_commit_mm` | 0.2–3.0 mm | Lower commits pinch/zoom sooner |
| `gesture.page_swipe_commit_mm` | 0.3–5.0 mm | Lower commits two-finger page swipe sooner |
| `gesture.multi_swipe_commit_mm` | 1.0–8.0 mm | Lower commits 3/4-finger swipe sooner |
| `drag.commit_threshold_mm` | 0.6–4.0 mm | Lower grabs three-finger drag sooner; must remain below multi-swipe threshold |
| `drag.drag_lock` | boolean | Keeps synthetic left held after a real three-finger drag lift |

The default `FeelConfig` is tested to build an Arbiter configuration exactly
equal to `m16-production-v1`. M17 therefore changes nothing unless the user
edits a value and explicitly selects M17.

## Deliberately not exposed

- M10 takeover confirmation/duration/grab/cleanup rules;
- output/backend qualification;
- device quirks and normalization;
- M8/M9 tap and secondary-tap timing;
- M16 reconnect/service/autostart policy;
- pressure/haptic settings that are unsupported on the current qualified
  hardware/output boundary.

Keeping these out of the feel editor prevents an ergonomics tweak from
silently becoming a safety or lifecycle change.
