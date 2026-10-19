# Trainer backend readiness

The objective is the trainer port ready for UI integration, preserving the C# behavior and offline-only boundaries.
A green interop fixture alone does not establish that. This checklist separates the remaining application and game
contracts from already validated lower layers; unsupported capabilities must stay explicit until proven.

| Requirement | Current evidence and remaining work |
|---|---|
| Exact x64 invocation, threads, roots, main-thread dispatch | Authored real IL2CPP and authorized offline-game observations; retained code lifetime and limited ABI contracts remain explicit. |
| Offline admission and serialized sessions | Player.log plus independent helper guards, identity binding, no uncertain replay, cancellation and cleanup tests. |
| Owned forge, locale and limits | Both authored routes and live protocol 24 evidence. |
| Definitions and resolved names | Protocol 25 authored pages, total/locale/duplicate checks, provenance, separate raw snapshots and bounded cache. Live catalog not yet claimed. |
| Complete numeric metadata and game formatting | Ordinary-property field route passes both authored Unity routes. Special registries, generated names, localized rarity names and tooltip/roll mapping still require implementation and validation. |
| Application worker and observable state | Serialized actor and safe session connector pass 31 focused lifecycle/admission tests. Nonblocking commands, generation-bound publication, cache states and cleanup barriers exist; production locator/preparer and UI adapters still need composition. |
| Production helper preparation/bootstrap | Evaluation Python bootstrap is validated; pure Rust artifact/config validation passes six tests and actual compiled-helper parsing. Windows remote code is under separate implementation/audit; its runtime/body admission and app composition remain required. |
| Feature hooks and field editors | Require exact target/body contracts, wanted-state synchronization and independent owner/offline cleanup. Enabling features is currently unsupported. |
| Forge drafts, apply, readback and undo | Require full expected-state comparison, game-specific edit validation, stale-item detection and uncertain-write recovery. A fingerprint is insufficient. |
| UI adapter and captures | Existing Slint views use stand-ins. Connect owned state/commands after backend contracts pass; do not perform synchronous capture/bootstrap on the Slint thread. |

The current game process has a stopped, pinned protocol 24 helper. Replacing a helper does not unload old code.
Further live tests must follow fresh process/observer/provenance admission, and request a restart if retained-generation
limits require it. Authored child processes and their test setup can be automated throughout.
