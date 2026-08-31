# M19 Execution Plan

1. Add neutral-boundary reconfiguration seam to Arbiter/ArbiterSink/TakeoverBridge.
2. Add `m19-live-v1` profile inheriting M18.
3. Add deterministic settings-file watcher and last-good/pending semantics.
4. Add `--watch-settings` + `settings-patch` CLI.
5. Add tests, acceptance docs, full gates and review.
