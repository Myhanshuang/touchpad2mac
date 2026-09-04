## What changed

Describe the behavior change and why it is needed.

## Validation

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] Regression test added for behavior/bug changes
- [ ] User documentation updated when CLI/settings behavior changed
- [ ] Hardware changes include diagnostics + qualification evidence

## Safety / ownership

- [ ] No new input path can emit before gesture ownership commits
- [ ] Held buttons/scroll are released on cancellation/failure
- [ ] No keyboard key codes or sensitive input are newly persisted
