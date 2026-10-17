# Trainer validation gates

Investigation started 2026-10-17 after the first evaluation checkpoint, at the user's request to make runtime, hook and
assembly conclusions stronger before protocol/session implementation. Original app source remains at
`22eb727b0590ed825957cb05c2039b4c0768c5e2`. No protocol, operation or UI work is part of this investigation.

## Implementation verdict

The Rust port is viable. Proceed with a narrow, audited runtime boundary rather than adopting the evaluated bridge
unchanged. Implementing a layer is distinct from clearing it for live use: the following work can proceed against the
controlled fixtures while the remaining acceptance gates stay explicit.

| Area | Ready to implement or already implemented | Gate before game use |
|---|---|---|
| Runtime calls | Export resolution exists. Real 6000.2 IL2CPP invocation passes the tested scalar/reference/ref-out, value receiver and exception cases. Add narrowly typed argument construction and exception results. | Matching 6000.4 runtime; generic/interface/virtual dispatch and unsupported signatures must be explicit. |
| Thread/GC ownership | Production thread-affine attachment leases and strong roots pass against actual 6000.2 IL2CPP. Native reference field/array edges survive collection. | Matching version, incremental collection and game shutdown ordering. Attachment does not establish Unity main-thread authority. |
| Main-thread commands | Owned-command dispatcher, cancellation and uncertain completion exist and pass tests. | Verified Unity callback, patch/unwind correctness and code lifetime. Neither evaluated hook engine is adopted. |
| Managed files | Dotscope can implement offline transforms with independent before/after validation. | Decide malformed-type policy explicitly; broader format features remain separate requirements. |

Unsafe code belongs at export calls, pointer/receiver construction, object ownership transitions and eventually the
hook boundary. A typed Rust wrapper cannot establish an arbitrary game's metadata/ABI or runtime lifetime by itself.
Use `runtime_invoke` for the first verified signatures; do not expose a generic safe `call<T>` over arbitrary pointers.
Nightly is available, but the established foundation does not need nightly-only language features.

Continue with matching-version runtime and player-loop execution before starting protocol/session and trainer operations,
as requested. The broader hook corpus now passes the experimental retained-owner policy described below. A fake GUI
is unnecessary: the disposable process host covers the boundary and existing UI captures
already use fake sessions. No live game validation has occurred.

## Evidence standard and scope

Keep these evidence levels separate: source/header contracts; synthetic ABI experiments; static inspection of the exact
game artifacts; execution in a controlled IL2CPP fixture; execution in the actual game. One does not stand in for the next.
Live game testing has not been requested. Original app tools and installed artifacts may be used offline; installed game
files stay untouched. Generated assemblies and test executables run only in explicitly selected disposable processes.

The active goal is not complete merely because synthetic tests pass. Record residual uncertainty and improve the next
experiment rather than weakening acceptance criteria. Exact game runtime claims require actual runtime evidence later.

## 1. Runtime calls, attachment and collector

Required evidence:

- Exact-version export signatures and GC handle width, corroborated against installed artifact identity.
- Scalar, reference, null, byref/out and unboxed value-type receiver conventions; exception-out behaviour.
- Explicit treatment of virtual/interface/generic dispatch instead of name/count lookup as a general substitute.
- Reference field/array writes and barriers; root ownership through collection, replacement and thread shutdown.
- Controlled IL2CPP execution beyond a handwritten mock, with exact-version limits visible.

Current evidence:

- [Source contract audit](../tools/trainer-eval/runtime-contract/contract.json): installed 6000.2.9f1 source plus pinned
  archived 6000.4.8f1 headers, compiled signature assertions and a handwritten cross-language argument oracle.
- [Native artifact inspection](../tools/trainer-eval/native-inspect/README.md): exact installed GameAssembly hash,
  generator-cache identity, metadata version 39, runtime exports and dispatcher code inspected without loading them.
- Both versions' headers and the actual binary confirm `il2cpp_array_length` returns uint32. The original helper's
  uintptr_t declaration is inaccurate even though x64 zero-extension can conceal that mismatch in small tests.
- Exact binary GC-handle target code consumes a full-width pointer. Raw invocation clears the exception-out pointer
  and forwards method/object/argument pointers; it does not make arbitrary argument construction safe.
- Source shows ordinary references and ref/out slots require different representations. Value receivers need explicit
  unboxing with the owner rooted. Field setters distinguish direct reference values from addresses of scalar storage.
- Original affix replacement roots retained and newly created objects before clearing the old list. Preserve this order.
- [Generated invoker execution](../tools/trainer-eval/unity-fixture/report.json): the installed IL2CPP compiler generated
  real C++ from our managed fixture. Registration tables select the emitted invokers, whose definitions are copied
  unchanged into the cross-language probe. Two tests pass on stable debug and nightly release: mixed scalar widths,
  direct references, out-reference slots, trailing MethodInfo, an unboxed receiver and return storage. This executes
  generated invokers, not the collector, allocation, managed exception handling or runtime_invoke implementation.

Controlled actual runtime execution now passes. The installed builder selects native DLL versus executable output from
the output extension. Using `.dll` excludes executable-only `Libil2cppLackey` and builds successfully from unchanged
runtime/generated sources plus our native probe. Fifteen native checks pass: attachment, rooting after worker exit,
collection, invocation/boxed return, unboxed receiver mutation, managed exception capture/rooting and reference-field
set/clear. Shutdown and the disposable native host's unload also return normally. This resolves the former build gate;
omitting `--libil2cpp-static` alone was insufficient and remains recorded as a failed experiment.

The [Rust runtime host](../tools/trainer-eval/unity-rust-host/README.md) now exercises the production export resolver,
attachment leases and strong roots against that actual runtime on stable debug and nightly release. Borrowed entry-thread
attachment remains intact after nested lease/root cleanup. A new Rust worker attaches, invokes, roots, collects, reads,
releases roots before its final lease, verifies detachment and joins before native shutdown. The Rust host retains the
DLL until process exit and makes no active-helper unload claim.

The expanded fixture passes 26 native checks, also through the Rust host on nightly release. Ordinary null reference
and scalar ref/out cases pass. Separate workers create children and store them only through a rooted parent or rooted
array, then detach/join before the main thread collects and first reads them. No separate child handle is retained.
Both children survive; explicit reference clearing passes. Array addresses use runtime-reported header/element sizes
and bounds, with the exported reference barrier. Incremental GC and strict barrier checking both report disabled.

Remaining: incremental collection/barriers, virtual/generic dispatch and exact-game runtime validation. This fixture
uses installed 6000.2.9f1, not the game's 6000.4.8f1, and has no Unity player loop. Its official Baselib prerequisite was
[extracted as data](../tools/trainer-eval/unity-support/README.md) into a verified TEMP distribution copy. The user has
now authorized installing matching editor 6000.4.8f1 plus Windows IL2CPP support. Both official downloads passed size,
MD5 and Authenticode verification. Windows administrator consent was cancelled before the editor installer started;
the module was not launched. See the [installation record](../tools/trainer-eval/unity-support/install-6000.4.md).
The [controlled player fixture](../tools/trainer-eval/unity-player/README.md) is prepared and its C# sources compile
against the existing editor references with warnings as errors. Actual matching-version build/execution is pending.
It will record the captured UnitySynchronizationContext, worker posts, callback thread/FIFO order and Update/LateUpdate
events. This preparation is not player-loop evidence. The game remains untouched.

## 2. Hook unwinding, patching and teardown

Required evidence:

- Correct forwarding/relocation for the actual dispatcher prologue and representative hook shapes.
- Correct caller/stack/nonvolatile recovery at relocated instructions and jump-back, including a thrown foreign exception.
- Concurrent callers during enable/disable, explicit draining and a justified trampoline/code lifetime policy.
- Known unsupported unwind records, callback coexistence and target validation must fail closed.

Current evidence:

- [Hook experiments](../tools/trainer-eval/hook-eval/README.md) preserve both passing controls and failing cases.
- Actual `ExecuteTasks` begins with a 40-byte stack allocation and a RIP-relative initialization check before its calls.
  The original app calls it first and retains created trampolines when disabling hooks.
- NeoHook's relocated call returns to a jump-back which Windows unwinds as an epilogue, restoring the wrong stack
  pointer. A deterministic `RtlVirtualUnwind` probe reproduces this independently of Rust destructor code shape.
- A scratch NOP insertion changes the call-return PC and repairs synchronous cleanup in the small fixture, but recovery
  at the jump-back instruction itself remains wrong. This is causal evidence, not an adoption-ready fix.
- A separate scratch experiment emits a CS-prefixed direct near jump (`2E E9 rel32`). All copied instruction boundaries,
  normal execution, actual foreign throws, Rust cleanup and restoration pass in the tested simple and dispatcher-shaped
  fixtures. A broader corpus also passes saved/clobbered RBX, an RBP frame, saved/clobbered XMM6 and a 520-byte
  allocation on stable debug/release and nightly debug. A far relay forces a 14-byte entry patch so these instructions
  really are copied; the tested jump-back remains near. This relies on observed Windows epilogue recognition, not a
  documented cross-version guarantee. No dependency patch is adopted.
- The harness rejects a real language-handler target and a chained-unwind target before installation. The pinned
  upstream library accepts those targets while dropping handler flags or generating no unwind entry. A shipping
  acceptance gate must therefore inspect metadata itself; successful library installation does not establish support.
- A further transfer experiment replaces the jump-back with complementary `JE rel32; JNE rel32` branches, preserving
  registers, stack and flags. Both branch PCs unwind correctly for both ZF states across the register/frame corpus on
  stable debug/release and nightly debug. The relocated-call fixture executes each branch normally and passes foreign
  throw/Rust cleanup. Emitted instructions are decoded and alternate encodings rejected. Conditional branches are
  outside the documented epilogue grammar, giving this candidate a stronger rationale than the prefix workaround.
  Eight deterministic worker cases now pass stable debug/release and nightly debug: both branch PCs, both ZF values,
  and either quiescent disable/enable or supported transactional detach/fresh attach. Owner retention keeps code and
  metadata live; no reverse PC mapping is needed because suspended threads remain in the retained trampoline. The
  architecturally unreachable second-PC/ZF=1 state is inspected, then normalized before resuming. This is not adoption.
- Retention must keep the actual hook owners alive. Dropping an owner enters library quarantine; later reclamation
  unregisters unwind tables before suspending threads. That path is outside the validated policy. The worker harness
  also needed explicit 16-byte alignment for Windows CONTEXT; plain windows-sys 0.61.2 CONTEXT has 8-byte Rust alignment.
  Its release failure is reproduced and fixed in the harness. NeoHook's plain context locals are a source-level concern,
  not a separately demonstrated engine failure.
- Detour still lacks unwind metadata for its relocated stack-changing prefix. A successful body exception does not
  establish asynchronous unwind correctness at every trampoline instruction.
- Four known concurrent callers pass 100 transactional enable/disable pairs, then stop and join before trampoline
  release. This validates a controlled caller set. Callback counts alone miss callers in a relay/copied prefix;
  retaining executable storage until process exit remains the conservative game-helper lifetime policy.

Remaining: a complete supported transfer/unwind strategy and explicit unsupported-target policy. The actual dispatcher
prefix changes RSP, so it fails a conservative leaf-prefix-only policy. Passing finite concurrent/draining experiments
is evidence for those scenarios, not a proof of race freedom or safe helper unloading.
Microsoft's [x64 exception handling](https://learn.microsoft.com/en-us/cpp/build/exception-handling-x64?view=msvc-170)
and [epilogue rules](https://learn.microsoft.com/en-us/cpp/build/prolog-and-epilog?view=msvc-170) explain why dynamically
generated nonleaf code needs accurate unwind information and why instruction bytes affect epilogue recognition.

## 3. Managed assembly preservation

Required evidence:

- Actual CoreModule input/current backup, independent raw metadata and .NET 6 loader observations.
- Compare the exact current original-app repair, Cecil oracle and Rust repair on the same copies.
- Preserve referenced types, fields, signatures, decoded IL, generic metadata, attributes, resources and exception regions;
  report all intentional changes explicitly. Repeated repair must preserve semantics.
- Exercise metadata table/index-width thresholds and original-loader compatibility. PDB/signing requirements remain
  separate until a real required use case establishes them.

Current evidence:

- Synthetic clean/damaged/repeated repairs pass with dotscope; this is the earlier result, not a real-file verdict.
- The installed backup has 3,195 types; the installed repaired file has 3,188. Running the current original app's
  source-linked repair on a scratch copy produces a byte-identical installed result and reports zero renames.
- Cecil drops seven orphan nested TypeDefs and three fields while reading/writing. The original comment promising
  that types are never removed disagrees with the library's actual behaviour. Treat the observed output as authoritative.
- Dotscope preserves those orphan types. Sorting/renaming removes the duplicate-name load failure, but .NET 6 still
  rejects seven malformed types with NestedPublic visibility and no enclosing relationship. Even the installed repair
  retains two failed reflected types; those are a baseline limitation, not a passing full-load result.

The [extended assembly report](../tools/trainer-eval/assembly-validation.md) now characterizes references/counterparts,
checks decoded IL, signatures, constraints, attributes, resources and exception regions, and executes the generated
fixtures on actual .NET 6.0.36. The large fixture crosses 65,536 TypeDef/Field rows and a 1 MB string heap; repair,
payload comparison and all 65,537 reflected types pass. Method/parameter and every coded-index threshold remain untested.

An explicitly separate normalization experiment changes the seven orphan visibility flags to valid top-level visibility
and renames two duplicates, retaining 3,195 TypeDefs and 16,217 fields. It loads 3,192 reflected types, with the same two
remaining failures as the installed file (which loads 3,185). Repeated normalization preserves the tested payload.
This demonstrates transform capability and baseline parity, not recovered parent semantics or approval to change public
type identity in production. No deletion-based repair has been adopted. Full type loading outside MelonLoader still fails.

[Loader diagnostics](../tools/trainer-eval/loader-diagnostics.md) identify both remaining failures as pre-existing
metadata defects. The comparer has an incorrect framework scope and no abstract-method overrides; StringBuilderStream
uses incompatible IL2CPP types in managed Stream override signatures. Its Seek method has a body. Independent framework
metadata and actual .NET 6 resolution distinguish these failures from missing loader dependencies. No additional repair
has been applied.

## Tooling provenance

The current original app tools CLI only reports its catalog; its former `capture` command was removed. Its private
research tooling supplies an existing `pefile` dependency; Mono.Cecil and installed LLVM provide independent metadata
and disassembly views. The original app is not launched. REA's doctor found unsupported Node 25 and missing native
providers/registration, so static findings here come from ordinary tools, with their identities and limits recorded.
