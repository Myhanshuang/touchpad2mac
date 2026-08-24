# Third-party dependencies and licenses

Direct third-party dependencies of the workspace crates, their **locked**
versions (`Cargo.lock`), licenses, and use. M5 (`apps/touchpadctl`) added no
new third-party crate to `Cargo.lock`: it depends on the workspace crates
(path dependencies, not third-party) and on `serde_json`/`thiserror`, which
were already locked. **M6 (`crates/touchpad-desktop`) added `zbus` and
`libloading`** (both pure-Rust, no system library linked) and uses the
system's **libei** library at run time via `libloading` — libei is *not* a
crate dependency (see below). **M10 added no new crate to `Cargo.lock`**: the
takeover slice reuses the existing workspace crates and the already-locked
`libc` dependency (the new `Sys::poll` readiness seam uses `libc::poll`
inside the existing `touchpad-linux` unsafe FFI boundary) and the existing
`touchpad-desktop` zbus/libei stack.

## Direct third-party dependencies

| Crate | Version (locked) | License | Used by | Purpose |
| --- | --- | --- | --- | --- |
| `serde` (with `derive`) | 1.0.229 | MIT OR Apache-2.0 | `touchpad-core`, `touchpad-trace`, `touchpad-desktop` | Serialization of core types, the JSON-Lines trace schema, and zbus body bounds |
| `serde_json` | 1.0.151 | MIT OR Apache-2.0 | `touchpad-trace`, `touchpadctl` | JSON Lines trace reader/writer; `touchpadctl replay` prints one JSON `ContactFrame` per line on stdout (M5) |
| `thiserror` | 2.0.20 | MIT OR Apache-2.0 | all crates | Structured error enums |
| `libc` | `=0.2.186` (pinned) | MIT OR Apache-2.0 | `touchpad-linux` (`sys::ffi`, Linux-only), `touchpad-desktop` (`ffi`, Linux-only) | C `input_*` struct layouts and raw syscalls (M4); `poll(2)` on the libei fd and the libei C types (M6) |
| `zbus` (with `blocking-api`, `async-io`) | 5.19.0 | MIT | `touchpad-desktop` | Pure-Rust D-Bus client for the XDG RemoteDesktop portal (M6): session bus connection, `CreateSession`/`SelectDevices`/`Start`/`ConnectToEIS`, `Session.Close`, property reads. No system D-Bus library is linked. |
| `libloading` | 0.8.9 | ISC | `touchpad-desktop` (`ffi`) | **Run-time** loading of the system `libei.so.1` and resolution of its sender API symbols (M6). Nothing links against libei at build time. |

## Runtime-loaded system library (not a crate dependency)

| Library | Version (host) | License | Use |
| --- | --- | --- | --- |
| `libei.so.1` (libei 1.6.0; pkg-config `libei-1.0`) | 1.6.0 | MIT | The Emulated Input **sender** API that emits relative pointer motion, buttons, and pixel-precise scroll events to the compositor's EIS implementation (M6). Resolved at run time via `libloading`; a missing library is a structured `LibraryMissing` result reported by `touchpadctl output-probe`, never a build failure. |

`liboeffis` (1.6.0) is the **portal-side** helper library used by KWin's
portal implementation to accept EIS connections; the client never links or
loads it. It is listed for completeness of the "portal + libei/liboeffis
stack available on this host" ABI decision.

## Transitive dependencies (locked, for completeness)

Brought in by the direct crates above (M6's `zbus` pulls a larger pure-Rust
tree); the workspace never uses them directly:

| Crate | Version (locked) | License | Pulled in by |
| --- | --- | --- | --- |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 | `serde` |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 | `serde` (derive feature) |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 | `serde_json` |
| `memchr` | 2.8.3 | MIT OR Apache-2.0 | `serde_json` |
| `zmij` | 1.0.23 | MIT OR Apache-2.0 | `serde_json` (float formatting) |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 | `serde_derive`, `thiserror-impl` |
| `quote` | 1.0.47 | MIT OR Apache-2.0 | `serde_derive`, `thiserror-impl` |
| `syn` | 3.0.3 | MIT OR Apache-2.0 | `serde_derive`, `thiserror-impl` |
| `thiserror-impl` | 2.0.20 | MIT OR Apache-2.0 | `thiserror` |
| `unicode-ident` | 1.0.24 | MIT OR Apache-2.0 | `proc-macro2` |
| (zbus transitive tree) | — | MIT / Apache-2.0 / ISC / BSD / Zlib / Unicode-DFS-2016 | `zbus` |

The `zbus` transitive tree is pure Rust (async-io/polling/rustix for the
event loop, futures-*, event-listener, enumflags2, indexmap, tempfile, etc.;
see `Cargo.lock` for exact locked versions). All are permissively licensed.

## Policy

- Dependencies are kept deliberately small (IMPLEMENTATION_BRIEF §10). The
  CLI hand-rolls argument parsing and help text rather than pulling in a
  CLI framework. M6 adds the two minimal pure-Rust crates that make the
  portal/libei output path testable without a desktop and buildable without
  a system D-Bus library.
- The workspace declares **MSRV `rust-version = 1.87`** — the real minimum
  of the locked dependency graph, because `zbus` 5.19 (and the `zvariant`
  family) declare `rust-version 1.87` (M6 re-review R6: the manifest no
  longer claims 1.85 while the lockfile rejects it). The declared MSRV is
  not yet independently tested on a 1.87 toolchain; all gates run on
  rustc/cargo 1.97.1. This is documented in `DESIGN_V2.md` and the README.
- No code from Apple, libinput, linux-3-finger-drag, or other referenced
  projects is copied into this workspace; they are behavior/interface
  references only (design.md §16). The libei FFI is a hand-written minimal
  binding to the installed `libei.so.1` ABI (documented safety invariants in
  `crates/touchpad-desktop/src/ffi.rs`), because no maintained safe Rust
  binding satisfied the protocol and lifecycle requirements (M6_TASK.md).
- No API key, token, or other secret is stored in any source file,
  documentation, log, trace, or fixture.
