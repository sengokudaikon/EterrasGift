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
| Application worker and observable state | Serialized actor and safe session connector pass 31 focused lifecycle/admission tests. Nonblocking commands, generation-bound publication, cache states and cleanup barriers exist. Inert log factory and watcher-input adapter pass two tests; production preparer and UI adapters still need composition. |
| Production helper preparation/bootstrap | Rust Windows remote loading/start and one-shot stop pass the owned Unity 6000.4.8f1 child, alongside plugin and Python routes. Full pristine dispatcher body/table/unwind proof precedes first Status; stopped proof uses RPM only. Nine controller tests cover pending-call retention and validation. Its unsafe runtime/GC/thread/patch/lifetime admission and app-specific composition remain required for game use. |
| Feature hooks and field editors | Require exact target/body contracts, wanted-state synchronization and independent owner/offline cleanup. Enabling features is currently unsupported. |
| Forge drafts, apply, readback and undo | Pure edit planning and operation-specific readback diagnostics pass 20 tests, with complete observation/process/generation/catalog binding. No submission or native write exists yet. Batch planning, main-thread stale comparison, uncertainty recovery and undo remain required. A fingerprint is insufficient. |
| UI adapter and captures | Existing Slint views use stand-ins. Connect owned state/commands after backend contracts pass; do not perform synchronous capture/bootstrap on the Slint thread. |

The current game process has a stopped, pinned protocol 24 helper. Replacing a helper does not unload old code.
Further live tests must follow fresh process/observer/provenance admission, and request a restart if retained-generation
limits require it. Authored child processes and their test setup can be automated throughout.

The [Rust bootstrap checkpoint](../tools/trainer-eval/persistent-player/bootstrap-report.json) records exact artifacts,
all three authored startup routes and 264 foundation tests on stable and nightly release. Capture-scoped Windows file
leases hash each held artifact once, then validate path/file identity around page requests; unsupported leases retain
the full-hash fallback. Twelve focused tests cover lease behavior. This changes capture I/O cost without making saved
catalogs current or granting write authority.
