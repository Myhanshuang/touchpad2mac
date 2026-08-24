# M17 Execution Plan

1. Core `FeelConfig` v1 + validation/range metadata.
2. `M17Profile`: derive M16 and replace pointer/scroll/gesture/drag feel only;
   prove default config is exactly M16-equivalent.
3. CLI: `feel-default`, `feel-check`, `feel-show`, `feel-set`, `feel-gui`.
4. Explicit bounded-takeover integration via `m17-tunable-v1 --feel-config`;
   reject missing config for M17 and reject the flag on every other profile.
5. Self-contained generated HTML editor with sliders/numeric inputs and JSON
   export; no external assets/network and no live application.
6. Docs, acceptance, full gates, independent review and status sync.
