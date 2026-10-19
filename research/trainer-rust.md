# Rust trainer interop research

Research date: 2026-10-17. Target: Windows x64, Last Epoch 1.5, Unity 6000.4.8f1.
The adjacent app at `22eb727b0590ed825957cb05c2039b4c0768c5e2` is the behavioural baseline.
The older handoff specs identify `2d2d8d5`; consult current source when they disagree.

The trainer can be ported to Rust, including the in-process helper. Existing libraries can shorten discovery and
binding work, but none evaluated in this research establishes audited compatibility with the exact target runtime.
Thread ownership, GC roots, ABI validation and dispatch remain responsibilities of the port.

Follow-up through 2026-10-19: [trainer-validation.md](trainer-validation.md) records actual 6000.4.8 runtime execution,
production Rust ownership/checked invocation and the first-party ExecuteTasks scheduler adapter. The authored player
passes worker installation/restoration, exact frame-count comparison, managed-exception cleanup/recovery and no replay.
The externally loaded bootstrap then passes in an idle authored player and in the explicitly authorized offline game:
one main-thread frame read returns 372,952 and the original entry is restored. [Evidence and limits](../tools/trainer-eval/debugger/scheduler-report.json).
The helper/trampolines remain loaded until process exit. Original library comparisons below remain research history;
the subsequent protocol 22 helper also returns owned forge snapshots through the complete authored Unity path.
Three explicitly authorized offline-game reads of a plain Hatchet pass under Release admission, followed by restored
entry/unwind and a stopped server. [Evidence and limits](../tools/trainer-eval/persistent-player/forge-report.json).
Protocol 23 additionally reads literal item limits in both authored Unity startup routes, with no game-body calls.
[Evidence and scope](../tools/trainer-eval/persistent-player/limits-report.json). Opt-in opaque boxed returns and
exact Int32 enums now pass authored standalone IL2CPP execution with GC on/off.
[Value-type evidence](../tools/trainer-eval/unity-rust-host/value-types-report.json). The actual locale layout/getters
are now statically audited; protocol 24 reads the completed selection without starting or waiting for it, passing
both authored Unity startup routes and the user-restarted offline game (en / 1.5.2.2), including literal ranges.
[Locale evidence and live limits](../tools/trainer-eval/persistent-player/locale-report.json).
Protocol 25 now captures already-loaded definition pages and resolves names with explicit provenance, passing
both authored Unity startup routes, with bounded cache persistence. Generated formatting and numeric interpretation
remain explicit unavailable capabilities. [Catalog evidence](../tools/trainer-eval/persistent-player/catalog-report.json).
Ordinary numeric ranges/rounding now pass both authored Unity startup routes; the application actor and safe
session connector pass scripted lifecycle tests. [Numeric and lifecycle evidence](../tools/trainer-eval/persistent-player/numeric-report.json).
Special registries, game formatting/mappings, production bootstrap/UI integration and gameplay writes remain separate work. See [trainer contracts](../crates/trainer/README.md).

## Decisions and constraints

- Prefer a Rust helper and Rust hook dependencies; retain the C++ implementation as a behavioural reference.
- Nightly is acceptable to the user. The user installed 1.101.0-nightly during evaluation, commit 5d8fec066 dated
  2026-10-16, replacing an older 1.92 nightly. Initial ownership tests pass on it and on the workspace's pinned 1.99.1.
  Pin a dated nightly before making nightly-only features a workspace requirement; keep the existing pin meanwhile.
- Preserve protocol 21 initially. Coordinated protocol changes can follow in stages.
- Preserve feature behaviour, including verification and uncertain-write handling. On app disappearance, turn off
  transient trainer effects without reverting persistent edits. Physical helper unloading is a separate problem.
- Keep the companion independent of trainer and process access. No live game testing until explicitly requested.
- Use fake runtime sessions and generated assemblies. Downloaded project sources belong in session scratch space.

## Runtime libraries

| Project | Evidence | Evaluation decision |
|---|---|---|
| [il2cpp-bridge-rs](https://github.com/Batchhh/il2cpp-bridge-rs) | Runtime export resolution, metadata cache, invocation and object access; Windows MSVC x64 listed. Published 0.1.4 in 2026. MIT. | Exact-package audit rejects unchanged adoption: thread affinity, handle width, callback attachment and reference writes. Useful discovery reference. |
| [il2cpp_rs](https://github.com/ElCapor/il2cpp_rs) | Windows-oriented bindings; documentation calls it development-stage. GPL-3.0. | Reference; exact Unity 6 coverage not established. |
| [quest-hook-rs](https://github.com/StackDoubleFlow/quest-hook-rs) | Practical Rust modding wrappers and hooks; nightly; documented Unity 2018/2019 support. | Nightly is acceptable, but current runtime layouts still require work. |
| [il2cpp_utils 0.2.0](https://docs.rs/crate/il2cpp_utils/0.2.0) | Released 2026-09-30; macros, mirrors and hooks over il2cpp-bridge-rs. Android-focused; optional Dobby; Windows examples disable the default hook engine. | Study bindings and snapshots. Panics on missing bindings and native hook dependencies need adaptation. |
| [unityforge](https://github.com/abix-/modforge/tree/main/unityforge) | Rust mod library loaded through C# BepInEx/MelonLoader/game-loader shims, with dispatch and Harmony integration. | Real Rust-authored modding precedent, not an independent Rust IL2CPP runtime. |

No maintained repository under the exact name `unity-il2cpp-rs` was established by this search.

Two concrete audit points, now confirmed in restored il2cpp-bridge-rs 0.1.4:

- [Thread guard](https://github.com/Batchhh/il2cpp-bridge-rs/blob/main/src/api/core/runtime/thread.rs) implements
  `Send` and `Sync`, although destruction can detach an owned thread. Our guard must remain on its creating OS thread.
- [Initialization](https://github.com/Batchhh/il2cpp-bridge-rs/blob/main/src/init.rs) attaches its worker, but dispatches
  completion callbacks onto additional threads without attachment in those paths. Cached Rust-data access and arbitrary
  runtime calls have different requirements. Initialization completion must not imply thread attachment.

The pinned package compiles on stable/nightly, but the non-Send/non-Sync acceptance assertion rejects it on both.
Its GC APIs also declare uint32 handles where exact-version archived headers declare pointers. Source identities,
raw invocation and write-barrier findings are recorded in [the package audit](../tools/trainer-eval/bridge/audit.json).
No runtime initialization was performed. Keep our narrow ownership boundary rather than integrating this version unchanged.

## Discovery and generation

| Tool | Current support evidence | Proposed use |
|---|---|---|
| [Il2CppInspectorRedux](https://github.com/LukeFZ/Il2CppInspectorRedux) | Lists metadata 35, 38, 39 and newer 104–110 variants. Generates C++ scaffolding, JSON and shim assemblies. | First offline extraction candidate; generate narrow Rust bindings from JSON or selected compatible headers. |
| [Original Inspector](https://github.com/djkaty/Il2CppInspector) | Development suspended. | Historical reference. |
| [Original Dumper](https://github.com/Perfare/Il2CppDumper) | Advertised Unity range ends at 2022.2. | Do not infer Unity 6 support. |
| [Cpp2IL](https://github.com/SamboyCoding/Cpp2IL) | Active rewrite, metadata analysis and IL reconstruction. | Independent comparison; reconstructed IL is not guaranteed original behaviour. |
| [il2cpp-dumper-rs](https://github.com/rodroidmods/il2cpp-dumper-rs) | Claims Unity 6 and newer metadata variants; retrieved repository had two commits. | Experimental candidate, not yet a validated replacement. |

Metadata parsing, registration discovery, object layouts, calling conventions, GC APIs and class injection are separate
compatibility claims. InspectorRedux's table does not explicitly establish our exact 6000.4.8f1 build. Test a matching
GameAssembly/metadata pair before accepting its output. Never apply addresses from another build.

Prefer runtime exports for class/member discovery and field offsets. Generate only the trainer's bindings; validate
declaring type, static flags, return type, full parameter types, byref flags and generic form. Keep unavoidable MethodInfo,
array, string or object-layout assumptions in a versioned adapter. bindgen does not establish GC ownership or thread safety.

## Frida and main thread execution

[frida-il2cpp-bridge](https://github.com/vfsfitvnm/frida-il2cpp-bridge) advertises Unity 5.3.0–6000.3.x, with testing
focused on Android/Linux. That is not verification of Windows 6000.4. Rust [frida-gum](https://docs.rs/crate/frida-gum/latest)
bindings exist (0.17.2, May 2026), but do not automatically include the TypeScript IL2CPP bridge and bring native Gum.
Use Frida as an experimental comparison tool rather than a required shipping dependency.

Its [Thread.schedule implementation](https://raw.githubusercontent.com/vfsfitvnm/frida-il2cpp-bridge/master/lib/structs/thread.ts)
uses SynchronizationContext.Post and a managed delegate where available; otherwise it falls back to Process.runOnThread.
It explicitly handles callbacks queued across script unloading. Frida's [API documentation](https://frida.re/docs/javascript-api/)
warns that runOnThread can interrupt non-reentrant code holding locks. Running on the right OS thread does not by itself
establish a safe execution point.

Keep the existing ExecuteTasks hook initially. Unity's
[SynchronizationContext source](https://raw.githubusercontent.com/Unity-Technologies/UnityCsReference/master/Runtime/Export/Scripting/UnitySynchronizationContext.cs)
associates it with ScriptRunDelayedTasks. The current helper calls the original first, then its offline watchdog and one
pending request. Preserve this order and test cancellation before versus after execution begins.

[MelonLoader](https://raw.githubusercontent.com/LavaGang/MelonLoader/master/Dependencies/SupportModules/Component.cs)
registers an injected MonoBehaviour on a persistent GameObject and forwards Update, FixedUpdate and LateUpdate.
This is practical but requires class-injection machinery. Posting our own managed delegate also requires constructing,
rooting and retaining the native callback. PlayerLoop replacement adds versioned structures and coexistence/restoration
work. None is necessary for the initial port.

## ABI and exception boundaries

Distinguish exported runtime APIs, generated managed-method bodies and engine internal calls. On Windows x64 the
[Microsoft ABI](https://learn.microsoft.com/en-us/cpp/build/x64-calling-convention?view=msvc-170) assigns the first four
argument positions to integer/pointer or positional XMM registers. Rust handles registers, shadow space and stack alignment
when given the correct ABI signature. Aggregate arguments and returns can introduce indirect pointers.

For a verified instance method `(Player*, float, int32, MethodInfo*)`, the arguments occupy RCX, XMM1, R8D and R9.
Our current static ExecuteTasks takes only a trailing MethodInfo pointer, with no dummy instance. Generic sharing,
value-type receivers, virtual dispatch, adjustor thunks and internal calls require individual validation.

Use runtime_invoke for ordinary commands. It reports managed exceptions through an out pointer. Value arguments use
addresses; ordinary managed-reference arguments use the object pointer; byref/out slots require additional treatment.
Do not expose an unchecked generic safe invocation API. Runtime metadata objects should be opaque pointer targets.

[Rust's FFI rules](https://doc.rust-lang.org/nomicon/ffi.html) require an unwind-permitting ABI when foreign exceptions
can cross Rust frames. C-unwind permits propagation; it does not translate C++ exceptions into Result. catch_unwind does
not generally catch foreign exceptions or access violations. Contain our own panic paths separately. Validate trampoline
unwind behaviour as well as function declarations before enabling native hooks.

The isolated [hook evaluations](../tools/trainer-eval/hook-eval/README.md) now exercise all-Rust `detour 0.10.0` and
`neohook 0.16.0`. Detour passes controlled forwarding/restoration and body-exception cases but has no unwind metadata
for a relocated-call trampoline. NeoHook registers relocated unwind metadata; a minimal debug hook nevertheless skips
a Rust destructor during foreign unwinding, while a direct-call control passes. Instrumentation and optimization change
the result. Neither clears adoption; release-only success does not resolve the preserved failing reproducer.

## Thread and GC contracts

An attachment guard checks the current runtime thread, attaches only when needed, owns only the attachment it created,
and cannot move or be shared between threads. Nested guards must not let the outer owner detach before a borrowed guard
finishes. Keep managed roots subordinate to an attachment. Never detach an engine-owned thread. Do not carry guards across
executor migration or invoke the runtime during DLL shutdown. A dedicated OS thread is preferable for off-main runtime
work; IPC workers should exchange owned data.

Unity documents a non-compacting Boehm collector and incremental
[write barriers](https://docs.unity3d.com/6000.0/Documentation/Manual/performance-incremental-garbage-collection.html).
A pointer in Rust-owned heap storage is not an explicit root. Use strong GC handles across operations/frames. The C++
helper already creates non-pinned strong roots. Pinning, rooting and Rust Pin are different concepts.

A [Unity 6000.3.5f2 report](https://github.com/Jadis0x/Il2CppInspectorPro/issues/22) documents pointer-valued GC handles
where older code assumed uint32. The first Rust experiment models the pointer-handle ABI explicitly; it must not be
silently applied to historical runtimes. Exact archived
[6000.4.8f1 types](https://raw.githubusercontent.com/nneonneo/Il2CppVersions/8a51ff05ea28a33fda2804197f3c2f1afff6f0c8/headers/6000.4.8f1.h)
and [API declarations](https://raw.githubusercontent.com/nneonneo/Il2CppVersions/8a51ff05ea28a33fda2804197f3c2f1afff6f0c8/api/6000.4.8f1.h)
now corroborate `typedef void* Il2CppGCHandle`. These are third-party extracted Unity headers; independent validation
against the installed editor/game and runtime behaviour remains an acceptance gate.

Rooting a UnityEngine.Object wrapper does not keep its native object alive. Check native validity separately. Avoid
Deref/DerefMut to live game layouts, since game aliases remain. Reference writes must use verified setters/barriers;
structs containing references cannot be blindly copied. Return owned snapshots to application code.

## Managed assembly writers

ECMA-335 PE/CIL assemblies, IL2CPP global-metadata.dat and GameAssembly native code are different formats. Rewriting a
dummy managed assembly does not change compiled game code or add a CIL interpreter to stock IL2CPP.

| Candidate | Published version | Actual write API | Evaluation |
|---|---|---|---|
| [dotnetdll](https://docs.rs/crate/dotnetdll/latest) | 0.3.0, 2026-06-24; GPL-3.0-or-later | Resolution::parse and Resolution::write emit managed PE bytes. | Typed construction and controlled rewrites. |
| [dotscope](https://docs.rs/crate/dotscope/latest) | 0.9.1, 2026-09-05; Apache-2.0 | CilAssembly emits to file or memory. | First candidate for existing-assembly edits. |

Both are genuine writers. The pinned dotscope package now passes our generated fixture repair/preservation matrix with
an explicit transformation. Dotnetdll changes nested parents even on clean input; a zero-based enclosing-type index
written as a one-based metadata RID is visible in its source. Results and reproduction are in
[trainer-evaluations.md](trainer-evaluations.md). Neither result establishes general Cecil-equivalent preservation.
dotscope's [changelog](https://github.com/ATRAPSLLC/dotscope/blob/main/CHANGELOG.md) includes recent fixes to metadata
tables, index widths, PE layout and exception handling. Test the exact pinned package. PDBs, strong names and mixed-mode
assemblies need separate capability checks when required.

The first acceptance case is `../EterrasGift/src/Infrastructure/EterrasGift.GameFiles/Mods/MelonCoreModule.cs`:
an unsorted NestedClass table makes .NET 6 mistake nested <>O types for top-level duplicates. Cecil rewrites the table
sorted; genuine remaining duplicates are renamed without deleting referenced types. The original is backed up and the
replacement validated. A writer that merely preserves the broken table does not pass.

The current C# test fixture creates genuine duplicate top-level types; it does not reproduce the unsorted NestedClass
case. The Rust evaluation now covers both cases, their combination and a clean assembly, using an independent
System.Reflection.Metadata reader and executable checks. Repeated repair, generics, attributes, resources, exception
handlers and calls through renamed types pass in the dotscope adapter. Real CoreModule, .NET 6 loading, nested generic
edge cases and index-width thresholds remain gates before broad adoption.

## Evaluation sequence

1. Dependency-free fake-runtime experiments: missing exports, null domain/failed attachment, existing and nested
   attachment ownership, root-before-detach ordering, non-Send/non-Sync types, pointer-sized handles and failure paths.
2. A single-slot main-thread dispatcher with cancellation and in-flight uncertainty tested without Unity.
3. Isolated package evaluation: pinned bridge and assembly writers outside the shipping workspace dependency graph.
4. Generated managed fixtures plus an independent metadata/behaviour oracle. Record bytes written separately from
   semantic preservation and successful repair.
5. Matching Unity header/generated-code fixtures for scalar, float, aggregate, byref and generic ABI checks.
6. Hook-engine experiments in a disposable test process, including unwinding and concurrent shutdown. No game required.
7. Exact game runtime validation only after the user requests live testing.

Run results and reproduction commands belong in [trainer-evaluations.md](trainer-evaluations.md). A passing fake runtime
establishes wrapper behaviour, not compatibility with Unity, its GC, or an actual native detour.

## Rust remote bootstrap checkpoint (2026-10-19)

See [bootstrap evidence](../tools/trainer-eval/persistent-player/bootstrap-report.json):
264 foundation tests pass on stable and nightly release, with one ownership compile-fail check each.
Plugin, Python and new Rust loading/start/stop routes pass the same owned Unity 6000.4.8f1 fixture
(263 requests, 227 main-thread commands per route). Rust checks full dispatcher body/table/unwind
before first Status and proves restoration with remote-memory-only polling after one stop request.
Uncertain calls retain ownership and are not replayed; no helper is unloaded. This checkpoint
performs no actual-game calls and does not establish arbitrary-target unsafe runtime admission.
Capture-local file leases remove repeated full hashes when supported; fallback retains full checks.
Pure edit planning/readback and composition inputs exist; app-specific preparation, complete game
formatting/mappings, native edits, hook/field services and async UI integration remain required.

## Special numeric and progress checkpoint (2026-10-19)

[Special numeric evidence](../tools/trainer-eval/persistent-player/special-numeric-report.json):
280 foundation tests pass on stable and nightly release. Four ready special registries now
use field-only selectors; SP43 still requires initialized ailment dictionary-cache semantics.
Extraction version 3 invalidates older numeric capability caches. All three authored Unity
startup routes pass 333 requests and 292 main-thread commands, including ten registry scenarios,
four additional malformed graphs, exact progress and first-page stopping followed by cleanup
and reconnect. No actual-game calls, gameplay edits or helper unload occur in this checkpoint.

## Retained preparation and mapping contracts (2026-10-19)

[Native preparer evidence](../tools/trainer-eval/persistent-player/native-preparer-report.json):
317 foundation tests pass stable/nightly release. The concrete eg-trainer-host preparer passes
a fourth authored route and reuses one helper across three preparations and a new session.
RuntimeAttestation remains an explicit unsafe per-target contract; actual game authority and
app composition remain work. Uncertain starts, caught-panic fences and pinned host guards
prevent reinjection/reclamation claims; confirmed-exit reclamation is not implemented.
Pure tooltip/preview codecs and complete 256-sample assembly pass25 additional tests with
exact full observation/context/query/language/unit binding and injected display parsing.
No native formatting/mapping/preview/write requests are enabled. Value parameter storage in
runtime_invoke is not managed ref/out; tooltip SlotType fits the existing enum subset.
Byte-clone constructors and object_new can initialize classes/refresh private state, so
allocation is not an inert byte-copy contract and raw ForgeSnapshot alone cannot reproduce it.

## Ready ailment dictionary checkpoint (2026-10-19)

[Ailment evidence](../tools/trainer-eval/persistent-player/ailment-report.json): all five special
numeric selectors pass authored coverage, extraction version4. Existing nongeneric Array.GetValue
returns a rooted opaque generic Entry box, avoiding generic ABI/struct layout expansion. Four real
standalone GC configurations pass; twelve Unity cache/liveness scenarios and seven malformed
dictionary cases pass all four startup routes (440 requests,392 main-thread commands each).
323 foundation tests pass both toolchains. No actual-game calls or gameplay writes occur.
StaticGetValue reads storage after metadata field setup; static GetValueObject can run a cctor.
Destroyed native wrappers/custom comparers remain explicit unavailable cases; legacy destroyed
wrapper field access is deliberately stricter here. Localized formatting, sampled values, private
clones/previews, edits/hooks/fields, game authority and UI adapters remain required.


## First native write and exact BCL display parsing, 2026-10-19

- Protocol26 operation22 carries a complete bounded expected ForgeSnapshot plus opaque manager identity. Metadata validates the exact mutable instance System.Int32 level backing field; the main-thread operation retains the whole current forge graph, compares every owned field and rechecks all edges/offline/native life immediately before one field API store. No guessed offsets or gameplay getter/rebuild/constructor calls. Every post-store failure is uncertain and no write is automatically replayed.
- All four owned Unity startup routes pass 578 requests/527 main-thread commands. Exact readback and independent fixture observations prove changed Int32 level values, unchanged serialized bytes/unrelated fields, fresh-plan restoration, bounds and full stale/refusal guards. The level may be recomputed later by game logic. The full draft/Apply/Undo workflow, other edits and actual-game write authority remain open.
- Session/actor writes bind process, generation, current catalog and manager; Release gates apply even in development sessions. Pending/cancellation/cleanup/receipt behavior preserves certainty and never publishes a result into a newer generation. 361 foundation cases pass stable/nightly release plus compile-fail ownership and architecture guards. See tools/trainer-eval/persistent-player/level-write-report.json and research/trainer-validation.md.
- The standalone game-BCL parser proof passes stable debug/nightly release with incremental GC off/on. Exact Double.TryParse(String,NumberStyles,IFormatProvider,out Double), style231, needs only the existing Int32 enum and F64 out contracts. Exact unit suffix and captured culture preserve displayed scalar scale, without host parsing or input normalization. Strong roots/managed exceptions survive actual collections. Unity Mono itself maps an unknown culture suffix such as en-Î©-ðŸŒ¿ to its en parent; truly unknown base cultures throw. No stricter returned-name policy or Rust fallback was added. See tools/trainer-eval/unity-rust-host/display-parser-report.json. This does not establish game BCL binary equivalence or enable formatter/mapping requests.


## Complete remaining hook/field source inventory, 2026-10-19

See tools/trainer-eval/hooks-fields-audit/README.md, hook-inventory.md, abi-inventory.md and report-index.json. The original e1c1ed4 source maps 76 UI descriptors: 68 hook features, 7 field sets and the forge editor; 74 physical owners include 31 byte patches and 54 declared detour ABIs. This is source evidence, not target compatibility or effect permission.

Required corrections to a literal port: an empty wanted set can falsely acknowledge cleanup despite physical unhook failure; raw patches do not consult offline flags and have a transition gap; display labels can transfer to different material identities; currency and post-write listing failures can return definitive refusal after partial mutation; unknown physical owners survive failed rollback/helper loss. Every field batch needs the first-mutation uncertainty fence; one-shot writes/injection cannot inherit HookSync's desired-state automatic retries. Stable typed field/context identity and observed physical patch restoration are required. Full source paths, line references, hashes and minimal vertical slices are retained in that audit. No native hook or field capability is enabled by this inventory.


## Service Apply/Undo and reusable display parser, 2026-10-19

The actual background worker now passes all four owned Unity routes (602 requests/550 main-thread commands each). Native ACK plus a separate ReadForge exchange precede Confirmed publication. Pending survives both responses and receipt disposal; every failed later read is uncertain, including a NotSent read. Conditional level Undo binds full after-state, manager, process/generation and catalog, prepares a fresh reverse edit and is consumed on successful restore. Context changes or attempted writes clear it. This establishes one Apply/Undo operation, not the full draft/batch or other item edits. See tools/trainer-eval/persistent-player/apply-undo-report.json.

The exact native RuntimeAccess parser candidate passes four real IL2CPP configurations with an operation-only UTF16 allocator and unchanged checked enum/outDouble contracts. It has explicit framework initialization/globalization/cache/lock/allocation/GC permissions, bounded input, exact overload identity and rooted results, no host fallback and no helper parser request. Source and proof are indexed by tools/trainer-eval/unity-rust-host/parser-adapter-report.json.

All76 feature descriptors and pure wanted/value/report state are now typed Rust data with unsupported native capability and unknown physical state. An independent compiled C# versus Rust export comparison matches all8 pages,76 descriptors,29 values,24 fields and1,108 scalar properties. Saved-value fitting preserves source behavior; live invalid input is rejected transactionally without step quantization. See tools/trainer-eval/feature-registry-parity/README.md. Foundation396 passes stable/nightly release plus ownership and architecture checks. Actual native hook/field effects, other edits, full formatting/mappings/private previews and app/UI composition remain open.

## Read-only attack/leech checkpoint, 2026-10-19

Protocol 27 adds manager-bound ReadAttackLeech (operation 23). The production field-only
reader roots the published manager/actor/stats graph, validates exact metadata and native
liveness, and copies raw f32 bits without getters, initialization, refresh or writes.
Exact CharacterStats is deliberately required because the original attack getter is virtual.
Dead/absent/changed managers revoke admission; local missing/dead/unsupported actor/stats
remain typed unavailable. Session repeats profile/log checks; the worker publishes owned
process/generation/checked-sequence results and clears them across admission/manager drift.

All four newly owned Unity 6000.4.8f1 startup routes pass 658 requests / 603 main-thread
commands each, including the previous Apply/Undo suite. Eight float boundary/raw-bit cases,
GC, object replacement, nine local unavailable and three global unavailable cases pass.
Four real worker reads publish sequences 1..4 with local-cache recovery. Getter/init/refresh
tripwires remain zero; every child proves entry restoration, stopped server, later frames,
normal exit and unchanged player files. The combined foundation passes 421 tests on stable
and nightly release, plus ownership/architecture checks. Full workspace verification passes;
headless captures were inspected. See [reader evidence and scope](../tools/trainer-eval/persistent-player/attack-leech-README.md)
and [exact identities](../tools/trainer-eval/persistent-player/attack-leech-report.json).

This adds no field-write or gameplay-hook capability and has not run in the actual game.
Initialized static caches, nonmoving GC, real main-thread execution and exclusive reachable
reference/scalar/native-lifetime access remain unsafe caller obligations. Rechecks are not
synchronization and GC roots do not preserve native lifetime. Equality tokens may be reused.

The [numeric formatter audit](../tools/trainer-eval/numeric-formatter-audit/README.md) also
identifies a smaller five-argument static route needing no new invocation ABI or item clone.
It unconditionally asks for virtual More rounding, so every modifier needs a rooted validated
override list. Formatting uses current managed-thread culture; the original host parses with
selected game language. Their equality is unproved, including custom NumberFormat values
with identical culture names. Numeric text can become independently available; mappings need
a separately admitted stable rooted provider. No formatter endpoint or provider parser is
promoted by this checkpoint. Localized descriptions and affix getter delegates remain open.

## Numeric text adapter and pure field planning, 2026-10-19

The exact five-argument static numeric formatter adapter is now implemented, independently
reviewed and exercised through its full public entrypoint against authored IL2CPP DLLs.
Review corrected the actual StatPropertyInfo -> PropertyInfoWithLocKey -> BasePropertyInfo
chain; the first draft's direct-parent assumption would have refused valid game objects.
Every modifier validates the rooted override list before invoking the audited virtual path.
The fixed false suffix and actual final-percent unit rule match the selected source route.

Production source is byte-identical to reviewed candidate C2DF673B... . Stable debug and
nightly release, each with incremental GC off/3 ms, pass all four configurations. Each records
42 formatter / 42 virtual override / zero unadmitted calls, 370 preinvoke collections,
39 rooted-result-copy collections and one rooted-exception collection. Full malformed graph,
invalid input/result, managed exception, failed root and recovery cases pass. Earlier parser,
boxed Entry, checked invocation and native probes remain in the same runs. This validates
authored ABI/root/BCL mechanics, not the actual game's numerical algorithms or live execution.

The adapter returns bounded owned text/unit only. No formatter wire request, parsed mapping,
provider capture/drift detection, description/localization delegate or item clone is enabled.
Exact active-body permission and exclusive graph/culture/provider nonmutation remain unsafe
caller obligations; same culture Name cannot establish the same custom number format.

A separate pure attack/leech single-field planner validates source f64 ranges before f32
projection, compares raw bits, retains complete process/generation/sequence/graph context and
preserves the other rate. Nine tests cover bounds, no-op, nonfinite input, preserved NaNs and
complete stale-state drift. It grants no native authority, endpoint, capability or Undo.
Full equality does not eliminate address reuse between requests: future writes need an
explicit selection-lifetime policy or retained helper ticket, beyond per-call roots.

Foundation438 passes stable/nightly release plus ownership/architecture checks; full workspace
verification and inspected captures pass. [Detailed scope](../tools/trainer-eval/unity-rust-host/numeric-formatter-README.md)
and [exact evidence](../tools/trainer-eval/unity-rust-host/numeric-formatter-report.json).


## Exact-provider parser checkpoint (2026-10-19)

The original language-based display parser remains unchanged. A separate unsafe exact-root
NumberFormatInfo parser now calls the same checked Double.TryParse overload with style231,
exact suffix/input bounds and finite-only output. Seven new full-entrypoint fake-runtime
cases plus four unchanged parser tests pass. Authored stable debug/nightly release x GC0/3ms
all pass with the exact approved candidate; production changes its first documentation line
only. The old formatter/parser/Entry/exception/native markers remain in those runs.

Same-name custom NumberFormat data matters: comma-decimal text1,25 parses as1.25 with the
actual rooted provider but125 through legacy en-US reconstruction; middle-dot text1Â·25
parses as1.25 with its custom provider and is refused by the legacy path. The owned fixture
independently establishes formatter/provider provenance and retains the root after dropping
managed owners/restoring culture. This does not establish that provenance in Last Epoch.

No context capture, culture synchronization/ABA guarantee, helper formatter/parser request,
parsed mapping capability, description/localization getter or private item clone is enabled.
The caller still owns exact runtime/body/main-thread authorization and exclusive provider/
culture/settings nonmutation. Names, pointers and readonly flags alone are insufficient.
[Scope and reproduction](../tools/trainer-eval/unity-rust-host/provider-parser-README.md) and
[retained source/runtime evidence](../tools/trainer-eval/unity-rust-host/provider-parser-report.json).

Root integration repeats all four provider runtime modes with the current FE344 documentation
revision and passes445 foundation tests on stable/nightly release. The evidence index treats
these production-source runs as primary and retains the original ABA candidate runs separately.

## Conditional rate-write primitive, 2026-10-19

The reviewed attack/leech Single-field writer is now integrated as an unsafe primitive.
It roots the current manager/actor/exact CharacterStats graph, validates the precise mutable
Single metadata, compares all six identity tokens and both raw values immediately before
one public field_set_value call, then requires the complete projected observation. Every
returned failure after entering that void setter is Failed/uncertain; no retry or rollback.

The chosen intent applies conditionally to the CURRENT offline character, matching the
existing trainer controls. It grants no historical selection, consumed request, replay
protection or cross-request lifetime guarantee. Roots retain managed wrappers only; caller
permission, exclusive native lifetime, thread/runtime/static initialization and nonmutation
remain explicit unsafe obligations. Production after-original scheduling is unchanged.

Four fresh authored Unity6000.4.8 players pass stable debug/nightly release with incremental
GC off/on:41 cases and13 real setter entries each,164 cases/52 stores total. C# independently
checks both fields; the harness also checks the returned full Rust projection, rooted GC
pressure, every pre-store refusal and an uncertain post-store readback fault. The initial
missing-Serializable evidence failure is retained and excluded from the corrected proof.
Alternate authored Update admission tests this primitive; it does not prove helper wire,
scheduler ordering, worker integration, actual-game mutation or persistence.

The combined foundation passes452 stable/nightly release cases plus ownership/architecture
guards; Clippy and formatting pass. No protocol, helper endpoint, general fields or gameplay
hook capability is added. See [primitive scope/evidence](../tools/trainer-eval/rate-write-primitive/README.md).

## Conditional rate writes through the helper and worker, 2026-10-19

Protocol28 now carries a strict336-byte one-field current-player request with full six-token
and two-raw-value expectation. The main-thread helper uses the reviewed rooted Single
primitive. Session marks Pending before transport, accepts only the full projected ACK,
and requires a separate matching ReadAttackLeech before confirmation. Every post-attempt
uncertainty is retained without retry. Strict Release log/profile/runtime offline admission
applies even in Development; tokens never become addresses or historical selection tickets.

The real worker validates process/generation/manager/sequence before status and submission,
reserves the next sequence before sending, publishes Pending through confirmation and
retains drained historical outcomes across cleanup epochs. Successful rate writes preserve
forge history and Undo. Stale old plans perform no I/O. Generic fields/hooks remain off.

All four authored bootstrap routes pass750 requests/692 main-thread commands each, including
eight Session changes, eighteen direct refusals, three Session refusals and two real worker
rate changes with independent C# backing-field checks. Each proves restored entry/stopped
server, zero owner, three later frames, exit0 and unchanged files. The first105-second suite
timeout before the rate cases is retained; a300-second total bound leaves per-request limits
unchanged. This is authored helper/scheduler/pipe/worker evidence, not actual-game mutation.

Foundation490 stable/nightly release plus ownership/architecture checks passes. Full lint,
formatting, token and capture verification passes; captures were inspected.
See [scope](../tools/trainer-eval/persistent-player/rate-write-README.md) and
[exact evidence](../tools/trainer-eval/persistent-player/rate-write-report.json).
The next [gameplay-hook hypothesis](trainer-first-hook.md) still needs its own authored
ABI/physical ownership proof. Numeric context capture remains a separately reviewed TEMP
candidate; no formatter/mapping wire or actual-game formatter calls are enabled.


## Exact numeric-format context checkpoint, 2026-10-19

The operation-local numeric context now retains the actual BCL Thread, exact CultureInfo
and exact NumberFormatInfo under roots. It composes the separately validated formatter
and provider parser without substituting the culture name or selected game locale.
Custom en-US providers show why: comma "1,25" parses as 1.25 with the actual provider
but 125 with the old name-based lookup. Exact getters and CultureInfo.numInfo metadata
are checked; derived cultures are refused before CurrentInfo virtual dispatch.

This remains one synchronous unsafe admitted span, including external exclusion over
culture/provider/settings/ExecutionContext changes. Roots and diagnostic rechecks do not
establish synchronization, detect same-object changes/ABA or authorize later requests.
Audited lazy framework initialization/cache/globalization effects remain permitted.

Eight new fake cases and both reviewed-candidate and integrated-root authored IL2CPP
matrices pass stable debug/nightly release with incremental GC off/on. Foundation498
passes both toolchains (503 stable cases including architecture/ownership guards;
499 nightly including ownership). Full workspace verification and standalone host
Clippy/formatting pass, and the combat capture was inspected. No helper opcode,
mapping/description capability, actual-game execution or UI binding is added.
See tools/trainer-eval/unity-rust-host/numeric-context-README.md and its report index.


## First gameplay callback ABI, 2026-10-19

The authored unpatched instance Boolean oracle passes stable debug/nightly release
with incremental GC off/on. Each mode has 46 cases, including 12 managed exceptions
and eight foreign unwinds through Rust cleanup guards to an outer typed C++ catcher.
Receiver and hidden-argument forwarding, original side effects, enabled bypass,
worker passthrough, exception rooting and recovery pass. Both workers join, all roots
are released, and the original native regression and shutdown pass. All 164 frozen
inputs remain unchanged; no runtime fixes or refreezes occurred. Initial build and
extraction failures remain retained.

This validates the tested C-unwind ABI under source-bound lifetime and attachment
contracts. No entry patch, trampoline, real owner/offline/main-thread admission,
production gameplay capability or actual-game execution is established. Static
inspection finds a shrinkwrapped late RBX save in the authored target's unwind data;
new bridge metadata and actual Windows unwind controls are required. Source and
results: tools/trainer-eval/unpatched-bool-abi/README.md and report.json.


## Reusable Boolean ABI tooling and Windows unwind metadata, 2026-10-19

The checked-in standalone oracle now reproduces the same 46-case/12-exception/
eight-Rust-unwind matrix in four newly owned processes. Preparation copies dependencies
to TEMP and verifies both installed and isolated SDK trees, editor/compiler identities,
all six byte-preserved proof sources, native output and data. All 4290 frozen inputs
remain unchanged. Initial workspace-lock and intermediate wiring records are retained;
there were no runtime fixes. See the portable-report.json beside its README.

A separate metadata-only experiment passes 60 source-derived Windows unwind contexts
on both native images and Rust profiles under Windows build 26100. It checks early
return, late RBX save, both proposed bridge branches, nonvolatile GPRs and XMM6-15
lower 128 bits. Bridge bytes stay in a nonexecutable read/write page; no IL2CPP/GC
initialization, instruction invocation or entry patch occurs. This is not complete
alias/caller or physical ownership proof. Exact results and limitations are in
tools/trainer-eval/unpatched-bool-abi/bridge-unwind-report.json. The next fixture
separates real bridge execution from quiescent entry installation/restoration.


## Numeric modifier field adapter checkpoint (2026-10-19)

The already-produced exact ItemTooltipPropertyModifierInfo leaf can now be read
through public validated FieldInfo APIs and fed to the existing static formatter.
Owned Value/min/max raw bits, varying, ModType, specialTag and display text leave
the adapter; roots and pointers do not. Retained getters are plain field loads;
ConvertValueToString forwards to the same five-argument formatter with suffix false.
The original parser/provider/context and formatter bodies remain unchanged.

508 foundation cases pass stable and nightly release (513 stable including
architecture/docs; 509 nightly including docs). All four ROOT-source standalone
Unity modes pass, including real GC pressure at field/root/String/invoke boundaries.
Host Clippy/format and workspace lint/format/tokens/captures pass; the fresh
combat-rates capture was inspected. Candidate and ROOT matrices are separate,
with immutable input manifests and complete native builds retained.

This requires separately admitted producer provenance and one synchronous exclusive
main-thread graph/culture/provider span. It does not acquire the modifier, reproduce
producer arithmetic, add descriptions/mapping/wire capabilities or access a game.
See tools/trainer-eval/unity-rust-host/numeric-modifier-README.md and its report.
