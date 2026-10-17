# Rust trainer interop research

Research date: 2026-10-17. Target: Windows x64, Last Epoch 1.5, Unity 6000.4.8f1.
The adjacent app at `22eb727b0590ed825957cb05c2039b4c0768c5e2` is the behavioural baseline.
The older handoff specs identify `2d2d8d5`; consult current source when they disagree.

The trainer can be ported to Rust, including the in-process helper. Existing libraries can shorten discovery and
binding work, but none evaluated in this research establishes audited compatibility with the exact target runtime.
Thread ownership, GC roots, ABI validation and dispatch remain responsibilities of the port.

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
| [il2cpp-bridge-rs](https://github.com/Batchhh/il2cpp-bridge-rs) | Runtime export resolution, metadata cache, invocation and object access; Windows MSVC x64 listed. Published 0.1.4 in 2026. MIT. | Most relevant candidate for selective reuse, subject to source and target-runtime checks. |
| [il2cpp_rs](https://github.com/ElCapor/il2cpp_rs) | Windows-oriented bindings; documentation calls it development-stage. GPL-3.0. | Reference; exact Unity 6 coverage not established. |
| [quest-hook-rs](https://github.com/StackDoubleFlow/quest-hook-rs) | Practical Rust modding wrappers and hooks; nightly; documented Unity 2018/2019 support. | Nightly is acceptable, but current runtime layouts still require work. |
| [il2cpp_utils 0.2.0](https://docs.rs/crate/il2cpp_utils/0.2.0) | Released 2026-09-30; macros, mirrors and hooks over il2cpp-bridge-rs. Android-focused; optional Dobby; Windows examples disable the default hook engine. | Study bindings and snapshots. Panics on missing bindings and native hook dependencies need adaptation. |
| [unityforge](https://github.com/abix-/modforge/tree/main/unityforge) | Rust mod library loaded through C# BepInEx/MelonLoader/game-loader shims, with dispatch and Harmony integration. | Real Rust-authored modding precedent, not an independent Rust IL2CPP runtime. |

No maintained repository under the exact name `unity-il2cpp-rs` was established by this search.

Two concrete audit points in retrieved il2cpp-bridge-rs main-branch source:

- [Thread guard](https://github.com/Batchhh/il2cpp-bridge-rs/blob/main/src/api/core/runtime/thread.rs) implements
  `Send` and `Sync`, although destruction can detach an owned thread. Our guard must remain on its creating OS thread.
- [Initialization](https://github.com/Batchhh/il2cpp-bridge-rs/blob/main/src/init.rs) attaches its worker, but dispatches
  completion callbacks onto additional threads without attachment in those paths. Cached Rust-data access and arbitrary
  runtime calls have different requirements. Initialization completion must not imply thread attachment.

These observations concern retrieved source, not a reproduced failure of a pinned package. Compare the downloaded
package before deciding whether to fork, wrap, contribute a fix, or retain our narrow implementation.

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
silently applied to historical runtimes. Matching 6000.4 headers and runtime behaviour remain an acceptance gate.

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

Both are genuine writers. Neither has yet established Cecil-equivalent round-trip preservation on our fixtures.
dotscope's [changelog](https://github.com/ATRAPSLLC/dotscope/blob/main/CHANGELOG.md) includes recent fixes to metadata
tables, index widths, PE layout and exception handling. Test the exact pinned package. PDBs, strong names and mixed-mode
assemblies need separate capability checks when required.

The first acceptance case is `../EterrasGift/src/Infrastructure/EterrasGift.GameFiles/Mods/MelonCoreModule.cs`:
an unsorted NestedClass table makes .NET 6 mistake nested <>O types for top-level duplicates. Cecil rewrites the table
sorted; genuine remaining duplicates are renamed without deleting referenced types. The original is backed up and the
replacement validated. A writer that merely preserves the broken table does not pass.

The current C# test fixture creates genuine duplicate top-level types; it does not reproduce the unsorted NestedClass
case. The Rust evaluation therefore needs both cases, plus a clean assembly for preservation checks. Compare with an
independent System.Reflection.Metadata reader and executable semantic checks. Test idempotence, nested generics,
attributes, resources, exception handlers and index-width thresholds before broad adoption.

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
