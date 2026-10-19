# Trainer backend readiness

The objective is the trainer port ready for UI integration, preserving the C# behavior and offline-only boundaries.
A green interop fixture alone does not establish that. This checklist separates the remaining application and game
contracts from already validated lower layers; unsupported capabilities must stay explicit until proven.

Current verification uses pinned `nightly-2026-10-17` (Rust 1.101.0-nightly). `mise run trainer:verify` runs the release trainer/tier and controller
suites once, followed by workspace checks. Individual test tasks remain available for focused work.
Earlier compiler matrices below are historical results, not additional gates.

The next cleanup stage consolidates both existing writes behind `WritePlan`/`WriteOutcome<T>`,
one session submission/readback lifecycle and `Command::Write`. Known same-target readback drift
is retained and published as NotKept; ordinary refusals keep healthy sessions. Historical
confirmation survives later publication gate loss. Cancellation, stale-plan and uncertain-delivery
guards remain. Native operations and supported feature scope are unchanged; app/UI integration is next.
The [checkpoint](../tools/trainer-eval/persistent-player/write-service-refactor-report.json) records
520 stable cases,516 nightly release cases, workspace verification/captures and the final owned Unity worker route.

The desktop now runs the worker on its own Tokio thread and supplies its handle to the Slint forge.
Manual connection, empty/read-only items, replacement, unsupported display values and offline-gate loss
have a worker-to-UI fake-session route and captures. Native work and hashing stay off the UI thread;
write/mapping/preview operations cannot run through this adapter. Helper package lookup is lazy and
gate-protected, with source-build receipts published beside the app. No new live-game run is claimed.
Next: carry typed write/recovery results into the editor, merge duplicate models/feature tables and
connect the remaining settings/cache actions; hook-engine and feature-family work remains after that.
Validation:530 nightly release trainer/controller cases and56 UI unit tests pass, plus full workspace
verification and inspected worker/UI captures. The helper build publishes matching debug/release receipts;
their retained DLL hashes were checked without loading a DLL or accessing a running game.

The first feature increment replaced the level-only request with protocol 29 status operation 10.
Level, forging potential, corrupted and ruined share `StatusPlan`, worker/session write handling
and conditional `StatusUndo`. Non-level edits call RebuildID and UpdateEntry; their readback
allows the derived level to change. Valid same-target native acknowledgements remain provisional,
including a differing result; the independent read decides Confirmed or NotKept. Refresh exceptions
after the store remain uncertain. The editor adapter is still read-only, and the three remaining
status fields, formatting/mapping/previews, other Forge edits and gameplay/field families remain work.
Helper package metadata is separate from evaluation reports (`passed` is ignored), and trainer
startup failure is logged/displayed in Settings while Home and Mods keep running.
The [authored status checkpoint](../tools/trainer-eval/persistent-player/status-write-report.json)
passes 226 requests/216 commands through the production worker route, with 10 preparations/one
helper load, GC during rebuild, independent Apply/Undo observations, NotKept, two post-store
refresh exceptions, restored dispatcher, stopped server and normal exit. Native rebuilding
also refuses inherited affix IDs outside the 12-bit save format before attempting a store.
That checkpoint's `mise run trainer:verify` gate passed 534 trainer/controller tests (five native
fixture entrypoints stay ignored), workspace Clippy/format/tokens and inspected fake-host captures.

Protocol30 completes the seven status fields. Legendary potential, Weaver's Will and Weaver's
Touch use the shared write/readback/Undo path, current worker override permission and an explicit
per-edit flag. Host and helper share the range/eligibility rules; native unique edits call the
game's getUnique(UInt16), root its result, check identity/type and recheck the full current item.
Missing, throwing, mismatched or stale lookups refuse before the store. Override revocation rejects
old plans and Undo before native I/O. An irreversible above-limit Will reduction remains confirmed
without offering an invalid Undo. The editor is still read-only; other Forge edits, formatting,
mapping, previews and gameplay/field families remain open.
The [authored remaining-status checkpoint](../tools/trainer-eval/persistent-player/remaining-status-report.json)
passes 291 requests/281 main-thread commands, six additional status scenarios and five independent
lookup refusal cases. Ten preparations reuse one helper, with restored dispatcher, stopped server,
unchanged player files and normal exit. No actual-game run is claimed.
The current `mise run trainer:verify` passes 538 trainer/controller tests (five ignored fixture
entrypoints), workspace Clippy/format/tokens and inspected fake-host captures.

| Requirement | Current evidence and remaining work |
|---|---|
| Exact x64 invocation, threads, roots, main-thread dispatch | Authored real IL2CPP and authorized offline-game observations; retained code lifetime and limited ABI contracts remain explicit. |
| Offline admission and serialized sessions | Player.log plus independent helper guards, identity binding, no uncertain replay, cancellation and cleanup tests. |
| Owned forge, locale and limits | Both authored routes and live protocol 24 evidence. |
| Definitions and resolved names | Protocol 25 authored pages, total/locale/duplicate checks, provenance, separate raw snapshots and bounded cache. Live catalog not yet claimed. |
| Complete numeric metadata and game formatting | Ordinary properties and all five special selectors pass all four authored routes, including SP43. Text-only formatting, exact-provider parsing and synchronous exact-context composition pass authored four-mode ABI/root proofs under explicit nonmutation. Actual-game arithmetic, generated/localized names, descriptions and tooltip/roll mapping remain open. |
| Application worker and observable state | Real authored worker level Apply/Undo and conditional rate writes pass all four earlier startup routes. The full NativePreparer/VerifiedTransport/SessionConnector-to-worker route now also passes stable debug and nightly release, including six response-boundary cleanup/gate-loss cases, helper reuse, Pending publication and separate confirmation reads. Nonblocking commands, generation-bound state, cache/progress and inert log adapters exist. The authority interface is now removed; actual-game validation of the callback-local check and app/UI composition remain open. |
| Production helper preparation/bootstrap | `eg-trainer-host` now loads/starts/connects through a small unsafe supported-game boundary. RuntimeAuthority/RuntimeAttestation and pre-load thread nomination are removed; callback-local context/managed-ID validation establishes dispatch identity. The existing scheduler, pristine dispatcher proof, Release gate, process identity and unfinished-call ownership remain. The updated owned worker route passes171 requests/163 commands, eight preparations and one helper. Actual-game execution of this new callback check is not claimed. |
| Feature hooks and field editors | All76 descriptors and local intents have compiled C# parity. Explicit attack/leech observations and conditional one-field changes pass native/Session/worker authored proofs. The first unpatched instance-Boolean callback passes forwarding and foreign-unwind fixtures; no physical gameplay hook is supported yet. The other field sets, physical ownership and callback offline cleanup still need implementation. The audit maps68 feature registrations/74 owners/54 detour targets/31 patches/7 field sets. |
| Forge drafts, apply, readback and undo | One level Apply/Undo path passes real worker execution on all four authored routes: full stale guard, Int32 store, native ACK plus separate ReadForge, retained reverse plan, uncertainty and cleanup. Pure planning exists for seven edit kinds. Other native edits, multi-edit batches and the full draft/Undo workflow remain required. |
| UI adapter and captures | The desktop has a read-only worker/snapshot adapter, manual connection and guarded owned forge display. Fake-session captures cover async navigation, empty/replaced items and gate loss. Native work stays on the worker. Typed writes, mappings/previews, settings/cache actions and duplicate model/catalog cleanup remain. |

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


The [exact bridge-call checkpoint](../tools/trainer-eval/bridge-execution/README.md)
adds passing original and current-source packaged four-mode matrices: 92 cases,
24 managed exceptions and 16 foreign Rust cleanups per child, with both INC flag
outcomes, exact retained roots/threads and a reserved/active caller gate. Original
entry bytes remain unchanged. Synchronous bridge calls are now proven for the
exact authored images; physical entry installation/restoration, alias inventory
and gameplay owner/offline/native-lifetime admission remain open.

The level worker now matches rate-write certainty for malformed adapter results:
Done/Failed cannot be labeled a definitive pre-store refusal. Its new full worker
regression raises the current foundation count to 509 on both toolchains.


The [offline callback contract audit](../tools/trainer-eval/bridge-execution/offline-contract-report.json)
confirms the exact warmed scalar-read mechanism while leaving managed cctor
completion, meaningful current offline ownership and full writer/transition
exclusion unproved. class_is_inited is layout warmness only. Existing status()
requires no active hooks and cannot be reused for gameplay callback admission.


The [full preparer/worker checkpoint](../tools/trainer-eval/persistent-player/preparer-worker-README.md)
closes the authored first-load composition gap on stable debug and nightly release.
Each run passes 171 requests, 163 main-thread commands, one helper across eight
preparations and six cleanup/gate-loss boundaries without replay. Worker/dispatcher/
server shutdown is confirmed separately from scalar rollback; OS-owner death and
production authority remain untested. Root tool checks and full workspace verification
pass, with the refreshed combat-rates capture inspected.


The [main-thread identity audit](../tools/trainer-eval/persistent-player/main-thread-contract.md)
finds a runtime-main OS-ID metadata path and the exact engine caller predicate. Runtime
initialization, Unity-main identity, canonical publication, held OS identity and safe
execution/offline/native lifetime remain distinct contracts. A later true callback
cannot retroactively authorize the current pre-bootstrap TID requirement. The owned
oracle-versus-detector experiment is being prepared without production API changes.

The [owned thread-identity checkpoint](../tools/trainer-eval/persistent-player/thread-identity-README.md)
passes one isolated Unity 6000.4.8f1 run: nine native captures, eleven independent C# observations,
45 roots freed, one scoped native-worker attach/detach and OS-exit-before-join, one foreign Rust
cleanup and three later frames before normal exit. All 696 frozen inputs remain unchanged.
This validates the narrow reader/predicate mechanism under authored lifecycle admission; forced
GC, another probe compiler and production authority/publication/lifetime remain separate work.

The [authority-attempt journal](../tools/trainer-eval/persistent-player/authority-journal-README.md)
now reserves process/profile identity before the authority future and pins registered
resource owners through cancellation/unwind. Owner-empty explicit NotLoaded remains
retryable; contradictory resident NotLoaded becomes MayBeLoaded without replay. Ten
new pure cases pass on both toolchains, including retained failing-before regressions.
Updated stable/nightly controllers also pass the full owned worker route (171/163,
13 setters, 44 Pending observations and six boundaries each) using the unchanged native
pair. Root tests/Clippy/format/tokens/captures pass; production authority remains open.

The [control-worker-first checkpoint](../tools/trainer-eval/persistent-player/control-worker-first-report.json)
now discovers a runtime-main candidate on an owned native worker before any main-side
detector preparation or C# identity oracle. One stable Unity6000.4.8f1 run passes eight
captures, ten later independent observations, 40 freed roots, one owned attach/detach,
OS-exit-before-join and three later frames before normal exit. All 879 inputs and the
complete 305-product/16-snapshot set are unchanged. Seven Rust and seven Python pure
tests, probe Clippy/format and workspace verification pass; the capture was inspected.
Initialized runtime, canonical publication, stable lifetime and later callback identity
remain authored premises. No production authority, pre-bootstrap transport, forced GC,
second compiler or actual-game operation is established by this ordering experiment.

The reusable [runtime-init candidate reader](../tools/trainer-eval/persistent-player/runtime-thread-reader-report.json)
now lives in game-access: a separate 21-export table, existing attached lease, two
local roots, exact metadata and distinct owned Windows/managed IDs. Seven full-entry
fake tests and248 game-access cases pass on each compiler; ROOT trainer suites pass
531 stable/527 nightly including guards/docs, and full workspace verification passes.
Nightly strict all-target Clippy retains one reproduced pre-existing module-order
warning; the new target passes strictly. No persistent suppression was introduced.
Shared-reader native integration is source-only preparation. The prior local-reader
runtime proof is separate; initialization/publication, GC/lifetime and later bootstrap
authority are still external obligations. Current helper source fingerprint919bea5d
differs from historical260081fc pairs; no new helper runtime validation is claimed.

The reusable runtime-init-thread reader now passes its own worker-first owned Unity
experiment:8captures/56balancedroots/10independentoracles and all1886 frozen inputs
unchanged. See [shared-reader report](../tools/trainer-eval/persistent-player/shared-reader-report.json).
Child exit0 is explicit; Python controller exit remains unknown after post-call host
receipt-writing failure. This closes the extracted-reader runtime gap only. The next
bootstrap layer needs a separately admitted, retained one-shot candidate carrier
and transport before scheduler installation; the existing full RuntimeAttestation
remains a separate unsafe contract.


The shipping refactor supersedes the historical pre-bootstrap authority requirements in the
checkpoint notes above. Current code keeps the supported-game assumptions in one unsafe
constructor and validates thread identity at the actual dispatcher callback. A first owned
run exposed the fixture's obsolete pre-callback main-TID assertion; the rebuilt fixture now
allows an unknown initial TID and still checks every later command against its independent
main-thread observation. See the [current report](../tools/trainer-eval/persistent-player/bootstrap-simplification-report.json).
