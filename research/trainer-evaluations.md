# Rust trainer evaluation results

Date: 2026-10-17. Host: Windows x64 MSVC. Behavioural source: adjacent app commit
`22eb727b0590ed825957cb05c2039b4c0768c5e2`. Design and sources: [trainer-rust.md](trainer-rust.md).

The first runtime ownership and dispatch layers are implemented and exercised without a game. A cross-language C++
fixture supports the proposed Windows ABI and unwind approach. Managed fixtures reproduce the nested-type lookup bug;
the Cecil baseline repairs them. Registry access now works with network-enabled permissions. The pinned Rust writer
and bridge evaluations have run: dotscope passes after explicit repair; dotnetdll and the bridge fail acceptance gates.

## Toolchains

- Workspace pin: Rust 1.99.1. Initial runtime and dispatcher tests use this toolchain.
- User-installed nightly: Rust 1.101.0-nightly, `5d8fec0669a64607db9ccbf663e7c23125bf4748`, commit date 2026-10-16.
  The runtime/dispatcher tests, lifetime compile-fail test and optimized native ABI experiment also pass with this compiler.
- .NET SDK 10.0.402; fixture and oracle target net10.0. The actual MelonLoader .NET 6 loader remains a separate gate.
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
pointer-valued GC handles. No arbitrary object-reference, invocation, field-write or process-injection API is exposed.

`crates/trainer` contains a safe single-slot dispatcher. An operation owns the slot until it finishes even if its receiver
times out. Commands and results are destroyed outside the state lock. The future Unity adapter must establish the correct
main-thread callback; the queue itself does not. Rust panics can produce an abandoned result but foreign exceptions and
access violations are not translated by the queue.

The C++ ABI fixture is an evaluation oracle only, not a dependency of the shipped Rust crates. Hook-engine probes now
exercise executable detours in their own disposable processes; commands, source identities and exact limits are in
[hook-eval/README.md](../tools/trainer-eval/hook-eval/README.md). Neither engine clears adoption. Concurrent patching,
hook coexistence, active-call drain and trampoline reclamation are untested.

The separate fake host loads only an explicitly supplied mock DLL. It uses the real ownership and dispatcher layers
but simulates frame execution, offline policy and explicit collection of boxed integers. Owned commands contain no
managed addresses; targets resolve on the simulated engine thread under a retained root. Its collector is not Boehm
and it has no metadata, write barriers or Unity objects. It tests quiescent unload after joined workers, not live helper
unloading. The actual game and generated Unity signatures remain outside these tests.

## Workspace validation

- `cargo test -p eg-game-access -p eg-trainer -p eterras-gift --offline`: 16 runtime/dispatch tests, one compile-fail
  documentation test and four architecture tests pass.
- `cargo +nightly test -p eg-game-access -p eg-trainer --offline`: the same 16 tests and lifetime check pass.
- `cargo lint`, `cargo fmt --all --check`, and `cargo xtask tokens --check`: pass.
- `cargo capture`: generates 32 fake-host captures. Home standard, Forge narrow and Settings high contrast were
  visually inspected; no UI source changed in this work.
- The standalone native ABI package passes Clippy with warnings denied. Its five tests pass in stable debug and
  nightly release configurations.
- Both .NET evaluation projects build with zero warnings/errors. The complete local Cecil comparison run passes.
- Both Rust writer adapters build and pass isolated Clippy. Bridge library/example checks pass; its negative acceptance
  test rejects the upstream guard as intended. These isolated workspaces are not covered by root workspace checks.
- Fake-host isolated Clippy passes; all four process tests pass on stable debug and nightly release. The hook packages
  pass isolated formatting/Clippy, with their intentionally failing acceptance case preserved.

## Next acceptance gates

1. Extend the passing dotscope repair evaluation to actual .NET 6 and larger metadata-index fixtures before adoption.
2. Extend the disposable fake host as invocation, reference writes and protocol transport are implemented. No GUI clone
   is needed now; the existing Slint captures already exercise UI fake sessions.
3. Independently validate matching 6000.4 headers/generated-code fixtures. Exact archived 6000.4.8f1 headers now provide
   pointer-handle evidence (see bridge audit); they are third-party extractions, not independently checked game artifacts.
   Validate handle ABI, exported signatures,
   value-type receivers, virtual calls and generic sharing. The current raw declarations are experimental until then.
4. Diagnose the preserved NeoHook debug unwind failure and address Detour's missing trampoline unwind metadata.
   Then test concurrent execution, disable/drain behaviour and retention of trampolines while calls remain active.
5. Add the verified invocation/exception and reference-write layer, then the helper adapter and protocol 21 transport.
   Wire trainer UI only after fake-session operation tests cover the relevant semantics.
6. Live game validation remains opt-in and has not been performed.
