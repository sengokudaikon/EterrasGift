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
4. The first verified invocation/exception subset now passes in the real fixture. Add enum identity/width checks,
   reference writes and traced managed-reference ref/out storage; evaluate concrete collection methods separately
   from generic-method construction. Then implement persistent native helper handlers behind the verified protocol 21 control client. Extend disposable-host
   operation tests before connecting trainer UI to a real helper.
5. The first-party scheduler adapter and external bootstrap now pass the authored Unity fixture and one explicitly
   authorized offline-game request. The checked getter returns frame 372,952 on the named main thread, exactly once;
   entry/unwind/files are restored/unchanged and the game remains responsive. [Evidence](../tools/trainer-eval/debugger/scheduler-report.json).
   Helper/code/tables stay loaded until process exit. Further live work requires the user's explicit scope; next
   implementation is protocol 21/session/offline admission against authored hosts before game-specific operations.


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
