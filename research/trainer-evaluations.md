# Rust trainer evaluation results

Date: 2026-10-17. Host: Windows x64 MSVC. Behavioural source: adjacent app commit
`22eb727b0590ed825957cb05c2039b4c0768c5e2`. Design and sources: [trainer-rust.md](trainer-rust.md).

The first runtime ownership and dispatch layers are implemented and exercised without a game. A cross-language C++
fixture supports the proposed Windows ABI and unwind approach. Managed fixtures reproduce the nested-type lookup bug;
the Cecil baseline repairs them. Registry access now works with network-enabled permissions. The pinned Rust writer
and bridge evaluations have run: dotscope passes after explicit repair; dotnetdll and the bridge fail acceptance gates.

The follow-up investigation is tracked in [trainer-validation.md](trainer-validation.md). It adds exact-game static
inspection, compiled header/source contracts, deterministic hook-unwind diagnosis, actual .NET 6 loading and real
CoreModule preservation checks. It now also executes a real IL2CPP runtime DLL and the production Rust ownership layer;
see [the runtime host](../tools/trainer-eval/unity-rust-host/README.md). The first-pass fixture results below remain valid within their stated scope; they are
not a verdict that the real CoreModule or a native hook is ready to ship.

## Toolchains

- Workspace pin: Rust 1.99.1. Initial runtime and dispatcher tests use this toolchain.
- User-installed nightly: Rust 1.101.0-nightly, `5d8fec0669a64607db9ccbf663e7c23125bf4748`, commit date 2026-10-16.
  The runtime/dispatcher tests, lifetime compile-fail test and optimized native ABI experiment also pass with this compiler.
- .NET SDK 10.0.402; the initial fixture and oracle target net10.0. The follow-up uses a separate net6.0 fixture and
  actual .NET 6.0.36 loading; see [the extended report](../tools/trainer-eval/assembly-validation.md).
- Native ABI fixture: Windows x64 MSVC, cc 1.8.0, C++ exception handling enabled with /EHsc.

No nightly-only language features are required by the first layers. The workspace pin has not changed. A dated nightly
pin should accompany the first actual nightly requirement; the installed `nightly` alias is not a reproducible pin.

## Results

| Experiment | Result | What it establishes |
|---|---|---|
| Runtime export resolution and ownership | 9 fake-runtime tests pass | Exact missing-export reporting; no invocation during resolution; failed attach/root handling; nested lease ownership; existing engine-thread preservation; root cleanup order; handles above uint32 range; Rust panic cleanup. |
| Thread affinity | Compile-time negative trait assertions pass | Context, attachment and root types are neither Send nor Sync. |
| Root lifetime | 1 compile-fail documentation test passes on both compilers | A live root cannot be used after explicitly dropping its borrowed attachment. |
| Dispatch | 7 tests pass | Pending cancellation, busy rejection, one-time result consumption, recursive dispatch, in-flight timeout uncertainty, dropped receiver handling and panic abandonment without replay. |
| Native ABI | 5 tests pass on stable debug and nightly release | Mixed float/pointer arguments, small aggregate pass/return, large hidden return pointer, scalar/reference byref slots, C++ exception propagation across Rust with destructor execution. |
| Clean managed fixture | Executes and returns 42 | Generic call, exception filter/finally, nested methods and embedded resource work before rewriting. |
| Unsorted NestedClass fixture | Four reversed rows produce `.Cache x3` duplicates in .NET's metadata reader | Reproduces the lookup failure mechanism missing from the original C# unit fixture. |
| Cecil 0.11.6 repair | Unsorted, genuine-duplicate and combined fixtures pass | Independent table/order and metadata-inventory checks, expected renaming, executable probe, semantic idempotence. |
| dotnetdll 0.3.0 | All four baseline cases fail, including clean input | Nested-parent relationships change. Registry source `resolution/write.rs:659` writes zero-based `enc.0` as a one-based enclosing-type RID. No hidden upstream patch applied. |
| dotscope 0.9.1 | Clean baseline passes; three damaged baselines fail; explicit repair passes all four | Raw NestedClass sorting and scoped duplicate renaming preserve inventory, executable references, result 42, and repeated repair. |
| il2cpp-bridge-rs 0.1.4 | Compiles on stable/nightly; thread-affinity acceptance rejects both | Exact package declares Send/Sync attachment guards and uint32 handles. Additional source findings are recorded in `tools/trainer-eval/bridge/audit.json`. Not suitable unchanged. |
| Fake host + mock runtime DLL | 4 process tests pass on stable debug and nightly release | Real DLL/export resolution integrates the production queue and ownership types; fixture mutation, cancellation, worker/offline rejection, root collection, quiescent unload, child failure and hang containment. |
| detour 0.10.0 | Controlled forwarding/restoration and body-exception tests pass | Relocated-call trampoline has no unwind metadata. Candidate remains restricted, not adopted. |
| neohook 0.16.0 | Unwind registration and instrumented tests pass; minimal debug hook fails on stable/nightly | Foreign exception skips Rust destructor in hooked path; direct-call control passes. Optimization/instrumentation changes the outcome, not the adoption decision. |

The first recorded assembly run is
`artifacts/trainer-eval/run-f73510d577584ce19844669b4bce91af/results.json` (git-ignored generated output).
Every rerun gets its own directory and JSON report. Full Rust comparison:
`artifacts/trainer-eval/run-656a59cf052848fca69ec819d933698a/results.json` (seven expected candidate failures).
Green explicit-repair run: `artifacts/trainer-eval/run-88d9f2517cfb4729b92d44b80792f0e3/results.json`.
These runs also preserve transcripts, tool versions and emitted assemblies.

## Reproduction

```powershell
cargo test -p eg-game-access -p eg-trainer --offline
cargo +nightly test -p eg-game-access -p eg-trainer --offline
cargo test --manifest-path tools/trainer-eval/native-abi/Cargo.toml --offline
cargo +nightly test --release --manifest-path tools/trainer-eval/native-abi/Cargo.toml --offline
./tools/trainer-eval/Run-Assemblies.ps1 -SkipRustWriters
```

For the restored, locked package evaluations:

```powershell
cargo fetch --manifest-path tools/trainer-eval/dotnetdll/Cargo.toml
cargo fetch --manifest-path tools/trainer-eval/dotscope/Cargo.toml
cargo fetch --manifest-path tools/trainer-eval/bridge/Cargo.toml
./tools/trainer-eval/Run-Assemblies.ps1
./tools/trainer-eval/Run-Assemblies.ps1 -RepairOnly
cargo check --manifest-path tools/trainer-eval/bridge/Cargo.toml
cargo test --manifest-path tools/trainer-eval/bridge/Cargo.toml
./tools/trainer-eval/Run-FakeHost.ps1
./tools/trainer-eval/Run-FakeHost.ps1 -Toolchain nightly -Release
```

The bridge test requires a non-Send/non-Sync Thread guard and intentionally rejects this exact package. Its non-initializing
surface example and audit record why. `Method::call<T>` is unsafe and checks argument count/return size; the separate
raw `invoke_method` wrapper accepts arbitrary pointers through a safe API. Callback-thread attachment, borrowed-thread
detachment and reference-write barriers also fail source review. Candidate lockfiles pin restored transitive dependencies.

The default assembly runner exits unsuccessfully when the baseline candidates fail. `-RepairOnly` runs the explicit
dotscope repair and Cecil comparison and is green; dotnetdll has no repair adapter and is explicitly skipped there.
The fixtures are generated .NET 10 assemblies, not proof for actual CoreModule, the .NET 6 loader, PDBs, signing or
large metadata-index thresholds.

Registry attempts from the sandboxed shell failed to connect to `crates.io:443` and `index.crates.io:443` before any
package download. Browser documentation access worked. Network-enabled execution subsequently restored all three
packages. This supports the user's permissions hypothesis; the earlier failures did not establish a Cargo defect.

## Implemented boundaries

`crates/game-access` contains opaque IL2CPP targets, typed function pointers, all-or-nothing export resolution and a
thread-affine context. Its unsafe constructor requires the caller to establish runtime lifetime, exact ABI, and exclusive
ownership of detachment. Multiple leases on one context share an attachment, including out-of-order lease destruction.
Strong roots borrow their lease, free exactly once and expose only raw pointers. The adapter explicitly assumes modern
pointer-valued GC handles. The follow-up checked invocation layer binds caller-authorized live metadata, checks exact
argument kinds and receiver/reference classes, and roots normal results and managed exceptions. Unsupported signatures
are rejected before invocation. There is no arbitrary safe `call<T>` or reference field-write API. The later external scheduler evaluation tool explicitly loads its narrow helper; it is not an app API.

`crates/trainer` contains a safe single-slot dispatcher. An operation owns the slot until it finishes even if its receiver
times out. Commands and results are destroyed outside the state lock. The Unity adapter establishes the captured
main-thread callback; the queue itself does not. Rust panics can produce an abandoned result but foreign exceptions and
access violations are not translated by the queue.

The C++ ABI fixture is an evaluation oracle only, not a dependency of the shipped Rust crates. Hook-engine probes now
exercise executable detours in their own disposable processes; commands, source identities and exact limits are in
[hook-eval/README.md](../tools/trainer-eval/hook-eval/README.md). Neither engine clears adoption. The follow-up now
tests transactional patching with four known callers and joins them before reclaiming storage. Hook coexistence and
uncontrolled game-thread draining/reclamation remain unvalidated.

The separate fake host loads only an explicitly supplied mock DLL. It uses the real ownership and dispatcher layers
but simulates frame execution, offline policy and explicit collection of boxed integers. Owned commands contain no
managed addresses; targets resolve on the simulated engine thread under a retained root. Its collector is not Boehm
and it has no metadata, write barriers or Unity objects. It tests quiescent unload after joined workers, not live helper
unloading. Actual-game execution remains outside these tests. A separate generated-invoker probe now executes
unchanged Unity-emitted invoker definitions; see [its report](../tools/trainer-eval/unity-fixture/report.json).

## Workspace validation

- `cargo test -p eg-game-access -p eg-trainer -p eterras-gift --offline`: 28 invocation/runtime/dispatch/scheduler tests, one compile-fail
  documentation test and four architecture tests pass.
- `cargo +nightly test -p eg-game-access -p eg-trainer --release --offline`: the same 28 tests and lifetime check pass.
- `cargo lint`, `cargo fmt --all --check`, and `cargo xtask tokens --check`: pass.
- `mise run verify`: workspace Clippy, formatting, generated-token checks and current fake-host captures pass.
  Trainer combat and unique-item captures were visually inspected after this scheduler follow-up; no UI source changed.
- The standalone native ABI package passes Clippy with warnings denied. Its five tests pass in stable debug and
  nightly release configurations.
- Both .NET evaluation projects build with zero warnings/errors. The complete local Cecil comparison run passes.
- Both Rust writer adapters build and pass isolated Clippy. Bridge library/example checks pass; its negative acceptance
  test rejects the upstream guard as intended. These isolated workspaces are not covered by root workspace checks.
- Fake-host isolated Clippy passes; all four process tests pass on stable debug and nightly release. The hook packages
  pass isolated formatting/Clippy, with their intentionally failing acceptance case preserved.

## Current acceptance gates

The detailed current verdict is in [trainer-validation.md](trainer-validation.md). Since the first checkpoint, actual
.NET 6 loading, selected large-index cases, exact-game static corroboration, generated invoker execution and a causal
NeoHook unwind diagnosis have been completed. Their limits remain explicit; they do not imply unrestricted adoption.

1. Controlled IL2CPP invocation/GC and production Rust ownership now pass on matching 6000.4.8f1 as well as 6000.2.9f1.
   The actual 6000.4.8 IL2CPP player also validates UnitySynchronizationContext callback thread/order. Simple virtual/interface
   resolution passes; enabled incremental-GC configuration passes positive full-collection cases, with phase-interleaved writes separate.
2. The hook corpus covers register saves, frame pointers and unsupported handler rejection. Complementary conditional
   transfers pass per-PC unwind/execution. The [bounded policy](../tools/trainer-eval/hook-eval/supported-policy.md) is
   now implemented by the first-party adapter; isolated candidate patches are not shipping dependencies. Native and
   [Unity-player tests](../tools/trainer-eval/unity-hook-player/report.json) pass normal/exception behavior, worker
   transactions, permanent ownership, entry restoration and the checked frame getter. One authorized live request also
   passes. Strict/dynamic-region CET, coexisting patchers and active code reclamation remain unsupported.
3. Decide how to handle the real CoreModule's malformed orphan nested types. Dotscope preserves more payload than
   the current Cecil repair; experimental normalization changes public identity and is not shipping policy. Causal
   diagnostic edits now isolate both remaining baseline loader defects; making classes abstract is not behavior restoration.
4. The first verified invocation/exception subset now passes in the real fixture. Read-only enum-field identity and
   Int32 storage checks now pass; add invocation enum identity/width checks,
   reference writes and traced managed-reference ref/out storage; evaluate concrete collection methods separately
   from generic-method construction. Persistent native discovery/status handlers now exist behind the verified
   protocol 21 control client. Extend disposable-host operation tests before connecting trainer UI to a real helper.
5. The first-party scheduler adapter and external bootstrap now pass the authored Unity fixture and one explicitly
   authorized offline-game request. The checked getter returns frame 372,952 on the named main thread, exactly once;
   entry/unwind/files are restored/unchanged and the game remains responsive. [Evidence](../tools/trainer-eval/debugger/scheduler-report.json).
   Helper/code/tables stay loaded until process exit. Further live work requires the user's explicit scope; next
   implementation is definitions and rich read-only descriptions after the validated forge snapshot, then writes/readback.


## Scheduler promotion and first live read, 2026-10-18

A first-party adapter now lives in `eg-game-access::scheduler`, with no NeoHook dependency. Its exact 11-byte
SUB/CMP prefix, 23-byte conditional trampoline and registered alloc-40 unwind record are tested at every instruction
boundary with both ZF states. Separate tests reject stale bytes and handler-bearing originals, and eight known caller
threads survive 40 install/restore toggles before joining. Stable debug and nightly release pass all five tests.
Entry changes serialize this library's transactions, verify thread ownership and two native censuses, reject PCs in
the first 11 bytes, recheck owned bytes under suspension and resume on every ordinary error path. Cross-page writes
and unsupported policies are rejected. Resume failures take priority; OS calls are not guaranteed time-bounded.
External patch/context/thread actors remain explicit caller obligations. No managed code/heap/logging runs frozen.

The updated evaluation plugin exercises the production scheduler, dispatcher and checked frame getter together.
Normal and managed-throw authored-player runs pass with native worker installation/restoration. An idle player then
passes the actual external LoadLibrary/bootstrap path and independently records continued frame progression.
Only after matching source/helper/controller identities and fresh live preflight does the game receive the helper.
That run returns frame 372,952 on Unity main TID 59160, with two original callback normal returns and one owned request.
All 81 entry bytes are restored, original unwind bytes/files are unchanged, and the same game process stays responsive.
The helper, original module, relay, trampoline and tables remain retained until process exit. No gameplay mutation,
product session/protocol or active-unload claim follows. [Result index](../tools/trainer-eval/debugger/scheduler-report.json).


## Protocol/control-session implementation, 2026-10-18

Source `e1c1ed4` now backs the protocol 21 control port: discovery/status/empty-hooks framing, strict bounded replies,
local process/server identity and delivery uncertainty in `eg-game-access`; current-log admission, installation-bound
artifact profile and explicit asynchronous lifecycle in `eg-trainer`. The app/UI remain unwired. Seven wire, six
owned-process pipe, 21 session and one combined log/pipe/session test pass. Together with prior foundations this is 63
cases on stable/nightly release plus the lifetime check; architecture checks and `mise run verify` pass, captures
inspected. Ignored child test entry points are exercised by the parent tests. No existing game was accessed.

The old source specs correctly document the general336-byte layout but can obscure that empty SetHooks has no tail.
Typed code 5 revokes attachment just like online status; it cannot leave a cached dispatcher available for later cleanup.
Dropped futures retain connection/closing barriers, and reconnect cannot reuse an old cleanup acknowledgement.
Server-PID/transport/path binding and strict UTF-8/unused-byte decoding strengthen the legacy control boundary.
Expected helper source ID is always checked; the DLL-hash fast path is deferred. Missing-log refusal makes no further
helper call, even where the original coordinator might conditionally clear already-held hooks. There is no hook-enable
or gameplay-write API in this slice. See [trainer contracts](../crates/trainer/README.md) for the complete scope.

All four dependent evaluation workspaces have refreshed offline lockfiles. The fake-host's four process cases and
the two runtime-contract checks pass with selected mock DLL/current installed 6000.4.8 plus pinned exact archived
headers. Initial direct calls omitted their input environment and failed; the corrected selections pass. The older
6000.2.9 installed-header directory is no longer available here, so that historical installed-version run was not
repeated. Unity Rust-host compiles and the native evaluation plugin builds with the new dependencies; the new plugin
was not loaded into a player/game. Persistent helper server and actual discovery/status remain next.

## Persistent Rust helper and actual discovery/status, 2026-10-18

`eg-trainer-helper` now builds a retained Windows x64 DLL, explicitly started after loader initialization. Its
dedicated control OS thread owns the IL2CPP attachment and current-thread Tokio runtime. The persistent protocol 21
server handles discovery, status and an empty ClearHooks; gameplay features remain absent. Metadata discovery resolves
the public API, checks all 37 legacy keys and selected field/method/header contracts, and caches pristine dispatcher
bytes before installing the existing scheduler. Its two allocation samples are strongly rooted. Exact MethodInfo,
nonmoving-GC and narrow scheduler assumptions remain unsafe caller contracts, not general Unity-version detection.

The server retains its owner process handle and creation identity across per-request connections. Other living
processes are refused before backend access; owner death permits takeover because this slice has no installed effects.
Pending disconnect/deadline cancellation proves no execution when successful. Running work retains the slot and
reports uncertainty. Malformed input cannot reach the backend. Eight real-pipe tests with authored child processes
exercise these cases, setup retry and shutdown during partial input. Future effects must be cleared and acknowledged
on the engine thread before owner takeover; no current empty-set acknowledgement proves future gameplay cleanup.

The complete authored Unity 6000.4.8f1 player now passes the real Session → PipeClient → server → scheduler → status
path. Its final run accepts 11 requests, completes six commands on main TID 62076, and records 78 original normal
callback returns with no wrong-thread entries. Rediscovery remains identical while the entry is patched. Runtime
online state revokes readiness despite an offline authored log; a destroyed native manager is rejected even though
its managed wrapper remains retained. Explicit disable acknowledges the empty feature set. Independent observation
changes the helper online flag from 0 to 1 between frames 10 and 66, with no intervening pipe request and the completed
command count unchanged at one. Shutdown restores the entry/stops the server, then Unity advances three frames and
exits normally with unchanged player files. Owner exit/takeover is tested in fake-backend process tests, not this run.

The fixture caught two real integration defects: Unity's log parser rejects Rust's canonical Windows verbatim paths,
and class forwarding exposes one canonical type through multiple images. Child launch arguments now use ordinary
local drive paths while identity checks retain canonical paths. Discovery accepts repeated identical class pointers
and rejects distinct identities. Installed `vm/Image.cpp` confirms exported types participate in class lookup; the
forwarding regression has a dedicated test. Initial failed reports are retained alongside passing evidence.

There are now 75 foundation cases on stable/nightly release, one lifetime compile-fail check per toolchain and four
architecture checks. Workspace Clippy/format/tokens/captures and isolated controller checks pass; captures inspected.
The [evidence index](../tools/trainer-eval/persistent-player/report.json) records exact helper/build/player identities,
installed-header hashes, logs, resolved failures and limits. No existing game was accessed in this step.

Next is a read-only game operation returning owned data, followed by exact per-operation type/lifetime checks and
write/readback experiments. Enum identity, reference writes and traced managed-reference ref/out remain unpromoted.
Independent effect cleanup must precede enabling gameplay features. Helper/code/trampolines remain retained until
process exit; restoration is not callback draining or unload permission. The app/UI remain unwired.

## Owned forge snapshots and live read, 2026-10-18

The original `ForgeReader`/`ForgeItemLocator` reads process memory directly; there is no legacy native read-forge
operation. The Rust port moves this graph traversal onto Unity's main thread and introduces operation 18 in protocol
22. Both sides deliberately reject protocol 21 discovery. The original header's game-data comment says 18, but its
actual enum assigns game data to 8 and ends at 17; the code wins. Existing operations retain their packed framing.

The returned model owns all strings/vectors and includes item/individual identity, name, type/subtype/rarity/class,
affixes, implicit rolls, unique/set identity and all existing item-status fields. It preserves the legacy fingerprint
exactly, including omission of set identity, names, special affix type and class requirement. This is not a complete
semantic state key. Addresses identify an observation in the selected process; they grant no later memory access.

The helper rechecks runtime offline play and the manager's native lifetime before traversing any item field. Every
reachable managed object is rooted before later metadata/GC-sensitive calls. Scalars use public `field_get_value`
after exact class/width checks; enum reads bind the actual field's enum class and exact corelib Int32 backing. Lists
require the closed corelib List definition and exact ItemAffix[] storage. SZARRAY rank/element identity, count versus
capacity, and strict bounded UTF-16/UTF-8 are checked. Caps are 32 affixes, three implicit rolls, eight unique rolls,
4096 List capacity and 4096 UTF-8 bytes per name. Null containers/entry/item mean an empty forge. Malformed graphs
fail the entire read. No game method or gameplay mutation occurs. Concurrent graph mutation/native destruction and
runtime shutdown remain excluded by the narrow unsafe main-thread contract.

Session requires Ready, revalidates installation artifacts and the current log before sending, and rechecks the log
after receiving a complete reply. Failure discards the attachment without replay; explicit reconnect is required.
Dropping a read future leaves the existing Connecting barrier until explicit disconnect. Eight additional session
tests cover these transitions. Ten independent codec cases and nine additional metadata/root cases cover the model,
fingerprint, rooting failures, scalar widths, nested enums, array contracts and text bounds.

Both direct P/Invoke and externally bootstrapped authored Unity players pass full snapshots, Unicode, forced GC,
empty/replaced items, unique/set identity and malformed-type/count cases. The final external run accepts 47 requests,
executes 36 commands on main TID 28208, preserves 156 original normal returns and exits after restoration plus three
frames. Expected values and fingerprint literals are independent of production encoding. External shutdown returns
retained atomic-counter addresses for read-only polling; it does not create new remote threads during restoration.

The user then explicitly authorized live offline reads. Player.log was initially empty but populated before the
actual client started, so both live attempts used Release/Allow without a development override. The first revealed
that a separately named global `ClassRequirement` lookup fails on the real metadata despite discovery's valid field
type. The reader now derives enum identity from that field; a nested-enum fixture reproduces the regression. The
failed attempt still restored all 81 entry bytes and original unwind and stopped the server with the game alive.
Its module remains retained. A distinct source-ID DLL was admitted only after checking that older generation's
exact body/identity, confirmed stop and fresh telemetry, plus pristine dispatcher/unwind evidence.

Three successful live reads return the same plain Hatchet, individual ID 918073198, fingerprint 0x8a3debae144d94d2,
type 5/subtype 0/rarity 0, no affixes, implicit bytes [0,0,0], level requirement 1 and zero/false remaining status.
Retained counters independently show five completed commands on Unity main TID 59160, six accepted requests, 285
original normal returns, no helper error and no wrong-thread entry. Explicit cleanup is ConfirmedOff; the full entry
and unwind are restored, the server is stopped and the same process stays responsive. This live case does not
validate populated affixes or unique/set items; those remain authored coverage.

All 103 Rust foundation cases pass on stable and nightly release, along with lifetime and architecture checks.
Workspace verification/captures and isolated controller checks pass; eight pure bootstrap preflight regressions pass.
The [forge evidence index](../tools/trainer-eval/persistent-player/forge-report.json) records successful/failed runs,
source/DLL identities, exact snapshots, cleanup and limitations. Definitions, rich descriptions, domain/UI composition,
sustained-polling performance, write/readback and effect cleanup remain ahead. The current helper's no-unload and
exclusive patching/graph contracts remain in force.

## Literal item limits and the remaining catalog boundary, 2026-10-18

The next read-only capability is the game's item ranges: `ItemData.MinForgingPotential` /
`MaxForgingPotential`, `MinLegendaryPotential` / `MaxLegendaryPotential`, `MinWeaversWill` /
`MaxWeaversWill`, `MinWeaversTouch` / `MaxWeaversTouch`, and `ExperienceTracker.maxLevel`.
The original `native/game-data.h::appendItemConstants` supplies zero as the level minimum.
These nine fields are metadata literals, not mutable item fields. The Rust reader requires exact
corelib Int32 identity, non-byref type, static/literal flags and declaring-class ownership before the
public static getter. That runtime's literal path reads the metadata default before static-storage
setup, so this operation invokes no class initializer or game body. Out-of-byte or reversed ranges
fail the whole result. It runs after the original scheduler callback on the established engine main
thread, with independent runtime-offline and native-manager admission before any constants lookup.

Protocol 23 introduces `ReadItemLimits` operation 19 with the existing strict 336-byte framing.
Its reply is exactly five min/max LE Int32 pairs (40 bytes), without locale or rarity names. Session
shares the forge read's Ready requirement, build/file-backed dispatcher verification, log rechecks,
cancellation barrier and failure/no-replay behavior. No separate raw-ranges disk cache is introduced:
the caller must keep the selected process/build profile with the observation.

The C# source audit changes the next implementation order:

- Full legacy `GameData` is operation **8**; comments saying 18 are stale. Its payload always begins
  with locale and application version, including when only constants are requested. Our standalone
  limits reply intentionally does not impersonate that payload or a complete `GameDataCatalog`.
- `native/game-data.h::gameLanguage` calls `LocalizationSettings.get_SelectedLocale`, then
  `Locale.get_Identifier` (a boxed value type), then `LocaleIdentifier.get_Code` with an unboxed
  receiver. The existing checked Rust invocation rejects arbitrary struct returns. This needs a
  narrow, rooted boxed-value capability with exact type/size/alignment and receiver tests, or an
  independently validated field-only alternative. Blanket struct ABI support is unnecessary here.
- Localized/generated affix names call the virtual `getAffixDisplayName`. Raw `affixDisplayName`
  can be stale. Field-only inventories must identify raw names and unavailable generated names.
  Exact metadata plus virtual target selection and method-body/thread authorization are separate
  obligations. A successfully resolved method does not establish execution safety.
- Numeric affix/unique properties call `GetPropertyInfo` and `GetRounding`; the latter may return
  an enum, also outside the current primitive/reference invocation subset. Unique names, base names,
  rarity names and implicit endpoint text add their own getters/localization/constructor calls.
- Capture must bind binary/metadata, game version, locale and extraction version; pin list totals
  across pages, reject duplicate IDs across the whole capture, and preserve null equipment filters
  versus empty ones. Match optional numeric failures explicitly, rather than inventing ranges.
  Legacy f64-to-f32 conversion lacks a post-cast finite check; Rust should reject unrepresentable
  values. Strict flags and UTF-8 also remain intentional improvements over the legacy reader.
- Mapping is a later capability: the game formats all 256 roll positions in pages of 16 against a
  serialized item/draft context. Raw numeric quantities may differ from displayed scale. A usable
  table needs a fixed context/language/unit, 256 finite displayed values, monotonicity and a changing
  value. Mapping a unique roll may name a tooltip line because one roll can affect several lines.
  Copying simple range interpolation would not preserve the existing app's behavior.

Sources are the adjacent app at `e1c1ed4`: `native/game-data.h`, `native/protocol.h`,
`Native/NativeGameData.cs`, `Sessions/Descriptions/GameDataService.cs`, domain `Models/GameData.cs`
and application `Sessions/Requests/RollMapping.cs`. Method calls above describe the C# helper's
behavior; they have not been authorized as safe Rust invocation contracts by this constants slice.

Both authored Unity startup routes pass the limits read, including GC/replacement stability and runtime denials,
alongside the earlier forge scenarios. Each accepts 52 requests and executes 41 main-thread commands, then restores
the entry/stops the server and exits normally after three more frames with unchanged files. The external route
records 158 original normal returns and zero helper errors/wrong-thread entries. There are 116 passing foundation
cases on stable/nightly release (13 new), plus architecture/lifetime and eight Python preflight checks.
[Build identities, source hashes, exact ranges and limits](../tools/trainer-eval/persistent-player/limits-report.json).
No real-game limits observation is claimed. The last observed game holds two stopped persistent generations;
the current evaluator admits only a sole older one. Another build needs a fresh game process or a separately
validated multi-generation admission path; restoring entry bytes alone never authorizes unloading old code.

## Opt-in boxed returns and exact Int32 enums, 2026-10-19

The next invocation layer is implemented without a general native struct representation.
`InvocationContext::bind_with_value_types` takes explicit `ValueTypeContract::BoxedReturn` class/size/alignment or
`I32Enum` class entries. Every contract must refer to live metadata from the same runtime. Ordinary `bind` continues
rejecting struct and enum signatures. Contracts identify classes, not intended parameter positions or overloads;
callers must still validate/select the exact method and separately authorize its body, callees and initializers.

Boxed returns require non-generic, non-inflated, non-enum value classes, a separately established payload size in
1..65536 and power-of-two alignment, both matching `il2cpp_class_value_size`. Only return positions are admitted;
arbitrary struct arguments and byrefs stay rejected. IL2CPP owns the returned GC box, which is rooted before any
metadata access and checked against the exact expected class. It may contain managed references. Rust never unboxes
or copies an opaque return payload; an existing exact boxed-receiver call can invoke its authorized getter.

Int32 enum contracts require an actual enum value class, exact corelib `System.Int32` backing identity, non-byref
type code 0x08, four-byte payload and matching Int32 alignment. Arguments carry an explicit class tag and initialized
Int32 storage. The supplied tag is only compared with signature identity, never dereferenced. Plain Int32 arguments,
different enum classes and enum byrefs are rejected. Returns preserve the exact enum box and copy only its checked
Int32 payload; no Rust enum is constructed. All signed Int32 patterns, including undefined values and flags, remain
possible arguments, so unsafe body authorization must cover them all.

Installed `Runtime::InvokeWithThrow` boxes value returns via `Object::Box`, retaining enum class identity.
`Object::Box` applies GC barriers for copied reference-containing values. Its special handling of `Nullable<T>` can
return null or a box of T, so nullable/generic/inflated classes are intentionally excluded. Byref returns, generic
methods/declaring classes, arbitrary struct inputs, managed-reference ref/out and other enum backing types remain
unsupported. Metadata/lifetime, nonmoving GC, engine state, aliasing and body/thread authorization remain unsafe
preconditions; neither validated signatures nor these experiments make arbitrary game code safe to invoke.

Eight new fake-export tests cover root-before-metadata, no opaque payload unboxing, exact receiver/getter chaining,
null/wrong result boxes, root failure, layout/duplicate/generic contracts, enum class/backing/width/alignment and
argument tags, signed extrema, exception roots and unchanged rejection boundaries. Full foundation count is 124.
The independently authored `LocaleCode` fixture contains one fresh non-interned Unicode string, with known x64
payload 8/8. It passes box-root -> collection -> exact receiver getter -> string-root -> drop box -> collection ->
managed text check. Int32 enum roundtrips include negative/undefined/extreme values, wrong class and byte/ref/out
rejections, rooted exceptions and subsequent recovery.

Fresh 6000.4.8f1 standalone IL2CPP builds pass stable debug and nightly release with incremental GC at 3 ms; nightly
release also passes with it disabled. All 34 existing native checks, attachment cleanup, input/source immutability
and normal shutdown pass afterward. Required runner markers explicitly cover both added capabilities. These are
authored runtime DLLs, not localization calls in the game or Unity player-loop evidence. Full collections in an
incremental-enabled configuration do not establish phase-interleaved writes or strict barrier validation.
[Exact builds, source hashes, runs and limits](../tools/trainer-eval/unity-rust-host/value-types-report.json).

Actual locale capture next needs an independent audit of the target's `LocaleIdentifier` payload and actual
`LocalizationSettings.get_SelectedLocale`, `Locale.get_Identifier`, `LocaleIdentifier.get_Code` and
`Application.get_version` bodies/signatures and initialization. The C++ helper's call chain identifies behavior;
it does not itself prove those execution contracts. No locale protocol request or catalog has been promoted here.
