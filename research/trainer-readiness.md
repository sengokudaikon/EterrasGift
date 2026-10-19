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
| Complete numeric metadata and game formatting | Ordinary properties and all five special selectors pass all four authored routes, including SP43's ready ailment dictionary. Generated names, localized rarity names and actual tooltip/roll mapping still require implementation and validation. |
| Application worker and observable state | Serialized actor and safe session connector pass 34 focused lifecycle/admission tests. Nonblocking commands, generation-bound publication, cache states and cleanup barriers exist. Capture progress is observable and a cleanup intent stops between completed requests without losing cleanup authority. Inert log factory and watcher-input adapter pass two tests; production preparer and UI adapters still need composition. |
| Production helper preparation/bootstrap | `eg-trainer-host` implements the retained HelperPreparer/VerifiedTransport, with explicit unsafe per-target RuntimeAttestation. The fourth authored route reuses one helper across three preparations and a new session after disconnect. Twelve host tests cover admission, no-replay/panic fences, guard retention and exact dispatcher proof. Windows remote start/stop remains validated; a production Last Epoch authority and app composition remain required. |
| Feature hooks and field editors | Require exact target/body contracts, wanted-state synchronization and independent owner/offline cleanup. Enabling features is currently unsupported. |
| Forge drafts, apply, readback and undo | Pure planning exists for seven edit kinds. Protocol26's level-requirement submission passes all four authored routes: complete expected snapshot/manager comparison, metadata-validated Int32 store, exact readback and retained uncertainty. Other native edits, batch planning, draft workflows and Undo remain required. A fingerprint is insufficient. |
| UI adapter and captures | Existing Slint views use stand-ins. Connect owned state/commands after backend contracts pass; do not perform synchronous capture/bootstrap on the Slint thread. |

The current game process has a stopped, pinned protocol 24 helper. Replacing a helper does not unload old code.
Further live tests must follow fresh process/observer/provenance admission, and request a restart if retained-generation
limits require it. Authored child processes and their test setup can be automated throughout.

The [Rust bootstrap checkpoint](../tools/trainer-eval/persistent-player/bootstrap-report.json) records exact artifacts,
all three authored startup routes and 264 foundation tests on stable and nightly release. Capture-scoped Windows file
leases hash each held artifact once, then validate path/file identity around page requests; unsupported leases retain
the full-hash fallback. Twelve focused tests cover lease behavior. This changes capture I/O cost without making saved
catalogs current or granting write authority.

The later [special registry checkpoint](../tools/trainer-eval/persistent-player/special-numeric-report.json) passes
280 foundation tests on both toolchains and all three Unity routes (333 requests, 292 main-thread commands each).
Extraction version 3 prevents reuse of older numeric capability results. Ten special-registry scenarios and four
new malformed graphs pass, including signed tags versus UInt16 ability IDs, first-match/default selection and exact
info inheritance. Authored capture stops after its first page, acknowledges ClearHooks, reconnects explicitly and
keeps the previous catalog/cache unchanged. Seven-page completion progress matches actual payload sizes.

The [native preparer checkpoint](../tools/trainer-eval/persistent-player/native-preparer-report.json) adds the concrete
composition and pure sampled mapping/tooltip/preview models. All four authored routes pass 333 requests and 292
main-thread commands; 317 foundation tests pass both toolchains. Actual game formatting, a display parser and native
mapping/preview requests remain disabled. The mapping builder requires all 256 captured display samples and pins
full observation/query/context/language/unit; its injected parser grants no host/game locale parity by itself.

The [ailment checkpoint](../tools/trainer-eval/persistent-player/ailment-report.json) completes the five special
numeric selectors with extraction version 4. Four real standalone GC configurations prove opaque generic Entry
boxing/child rooting through the existing nongeneric Array.GetValue interface. The full Unity adapter then passes
12 valid/local scenarios and seven malformed dictionary cases through every startup route (440 requests, 392
main-thread commands each). The 323 foundation tests pass stable/nightly release. Destroyed native wrappers and
custom comparers remain explicitly unavailable; no game dictionary getter or cache-fill routine is called.

The first [write checkpoint](../tools/trainer-eval/persistent-player/level-write-report.json) passes all four startup
routes:578 requests,527 main-thread commands each. It changes only the validated Int32 level backing field; no
RebuildID or game setter runs. Bounds, unique/set/legendary refusals, full stale state, replacement and manager/offline
guards pass, with independent fixture observations and explicit fresh-plan restoration. There are361 foundation
cases on each toolchain. This is a single-operation path, not the full draft/Apply/Undo workflow or app composition.
Level requirement can be recalculated later by the game; no persistence guarantee is implied.

The [standalone display parser](../tools/trainer-eval/unity-rust-host/display-parser-report.json) passes stable debug
and nightly release with incremental GC off/on using the existing checked enum/outDouble invocation contracts.
It strips only the exact unit suffix and parses the captured culture through the matching runtime's BCL. Unity
Mono's own parent-culture fallback remains observable. No host parser fallback or production mapping opcode exists.
