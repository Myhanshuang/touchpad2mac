# M12 Review — Scroll Fidelity and Momentum

Date: 2026-08-22  
Decision: **APPROVED — M12 code-complete; live-unqualified**

M12 adds an optional, platform-independent scroll-fidelity stage on the M9
two-finger owner: time-domain velocity EMA, bounded smoothstep gain,
axis-lock hysteresis, reversal reset, and software momentum. The M9 linear
branch remains unchanged when M12 is disabled. Momentum keeps the existing
pixel-scroll lifecycle/remainder, is driven only by explicit monotonic ticks,
and is cancelled by new ownership, buttons, discontinuity, cleanup or output
failure. `Arbiter::tick` deliberately does not advance the input-frame
regression baseline, avoiding false regressions from queued kernel events.

The tick path is carried through `ArbiterSink` and `TakeoverBridge`; the
bounded takeover loop uses a 16 ms readiness quantum only while momentum is
active and otherwise preserves the M10 100 ms bound. `m12-scroll-v1` is an
explicit experimental profile and adds no safety flag/default.

Independent verification:

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass.
- `cargo test --workspace --locked`: pass, 0 failed.
- `cargo test --release --workspace --locked`: pass, 0 failed.
- M10/M11 profiles do not attach M12 scroll fidelity.
- No new dependency or unsafe code was introduced by M12.
- No live takeover/device/portal/libei/system-setting action was executed.

`docs/M12_ACCEPTANCE.md` is a future user-run procedure and has not been
executed. M12 therefore remains live-unqualified. M13 may begin.
