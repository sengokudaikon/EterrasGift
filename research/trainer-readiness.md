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
| Complete numeric metadata and game formatting | Ordinary properties and all five special selectors pass all four authored routes, including SP43. Text-only formatting, exact-provider parsing and synchronous exact-context composition pass authored four-mode ABI/root proofs under explicit nonmutation. Actual-game arithmetic, generated/localized names, descriptions and tooltip/roll mapping remain open. |
| Application worker and observable state | Real authored worker level Apply/Undo and conditional rate writes pass all four startup routes, with Pending publication, separate confirmation reads and cleanup barriers. Nonblocking commands, generation-bound state, cache/progress and inert log adapters exist. Worker proofs use the direct verified-child Session connector; full NativePreparer-to-worker and app/UI composition remain open. |
| Production helper preparation/bootstrap | `eg-trainer-host` implements the retained HelperPreparer/VerifiedTransport, with explicit unsafe per-target RuntimeAttestation. The fourth authored route reuses one helper across three preparations and a new session after disconnect. Twelve host tests cover admission, no-replay/panic fences, guard retention and exact dispatcher proof. Windows remote start/stop remains validated; a production Last Epoch authority and app composition remain required. |
| Feature hooks and field editors | All76 descriptors and local intents have compiled C# parity. Explicit attack/leech observations and conditional one-field changes pass native/Session/worker authored proofs. The first unpatched instance-Boolean callback passes forwarding and foreign-unwind fixtures; no physical gameplay hook is supported yet. The other field sets, physical ownership and callback offline cleanup still need implementation. The audit maps68 hooks/74 owners/31 patches/7 field sets. |
| Forge drafts, apply, readback and undo | One level Apply/Undo path passes real worker execution on all four authored routes: full stale guard, Int32 store, native ACK plus separate ReadForge, retained reverse plan, uncertainty and cleanup. Pure planning exists for seven edit kinds. Other native edits, multi-edit batches and the full draft/Undo workflow remain required. |
| UI adapter and captures | Existing Slint views use stand-ins. Connect owned state/commands after backend contracts pass; do not perform synchronous capture/bootstrap on the Slint thread. |

The last retained live evidence describes a stopped, pinned protocol24 helper; the current process has not been
queried during this authored-work checkpoint. Replacing a helper does not unload old code.
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

## Conditional-rate checkpoint, 2026-10-19

The [rate-write path](../tools/trainer-eval/persistent-player/rate-write-README.md) passes
all four authored helper routes:750 requests/692 main-thread commands each, with exact
current-player guards, native full ACK, separate readback, independent C# bits and worker
Pending/sequence publication. Forge history remains independent. No generic field/hook
capability or rate Undo is implied. The source-bound unsafe native-lifetime/main-thread
contracts still apply; full equality is not historical selection or replay authority.

Foundation490 stable/nightly plus ownership/architecture guards and full workspace
verification pass. The numeric text/provider adapters and exact
format-context capture/composition pass separate four-mode authored standalone proofs.
Root integration validation is indexed in the context evaluation below.
The [first gameplay-hook proposal](trainer-first-hook.md) deliberately needs a new ABI,
physical owner and callback-admission experiment. None of this connects the existing
Slint stand-ins or establishes actual-game writes.


The [numeric-context checkpoint](../tools/trainer-eval/unity-rust-host/numeric-context-report.json)
retains all 102 reviewed candidate inputs, 58 root-source inputs, both complete
native builds and four passing root-source modes, with the earlier candidate
matrix indexed separately.
Exact Thread/CultureInfo/NFI roots, derived-culture refusal and numInfo consistency
are available for a single synchronous admitted operation. External exclusion of
culture/provider/ExecutionContext/AsyncLocal changes remains an unsafe obligation;
recapture is diagnostic and supplies no synchronization or ABA proof. Framework
lazy initialization/cache effects are permitted, while GameLocale stays separate.
No helper formatting/mapping endpoint or actual-game execution is implied.

The integrated context checkpoint passes 498 foundation cases on both toolchains
(503 stable tests including architecture/doc checks; 499 nightly including docs).
Host Clippy/formatting and full workspace lint, formatting, tokens and captures
pass; the combat-rates capture was inspected. This remains authored evidence.


The [numeric modifier leaf](../tools/trainer-eval/unity-rust-host/numeric-modifier-README.md)
now validates exact already-produced Value/min/max/property-info fields and feeds
the existing formatter, returning owned raw bits and text. Four reviewed authored
standalone candidate and root-source modes and ten new full-entrypoint fake cases
pass. The integrated 508 foundation cases pass on both toolchains, host Clippy/format
and full workspace verification pass, and the unchanged combat-rates capture was
inspected. This adds a narrow mechanism for later tooltip
work, with no producer arithmetic/acquisition proof, description, sampled mapping,
helper endpoint or actual-game formatter authority. Exact synchronous graph and
culture/provider exclusion remains the unsafe caller's responsibility.
