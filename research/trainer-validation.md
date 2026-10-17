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
| Runtime calls | Export resolution exists. Real 6000.4.8 invocation passes the tested scalar/reference/ref-out, value receiver, exception and simple virtual/interface cases. Add narrowly typed argument construction and exception results. | Closed generics, broader dispatch and unsupported signatures must be explicit. Validate each actual game operation separately. |
| Thread/GC ownership | Production thread-affine attachment leases and strong roots pass against actual 6000.4.8. Native reference field/array edges survive collection with incremental mode disabled/enabled. | Incremental phase interleaving and game shutdown ordering. Attachment does not establish Unity main-thread authority. |
| Main-thread commands | Owned dispatcher tests pass. An actual 6000.4.8 IL2CPP player validates worker posts through UnitySynchronizationContext and main-thread callback order. | A native hook/Rust dispatcher adapter in the player, patch acceptance and retained code lifetime. Neither stock hook engine is adopted. |
| Managed files | Dotscope can implement offline transforms with independent before/after validation. | Decide malformed-type policy explicitly; broader format features remain separate requirements. |

Unsafe code belongs at export calls, pointer/receiver construction, object ownership transitions and eventually the
hook boundary. A typed Rust wrapper cannot establish an arbitrary game's metadata/ABI or runtime lifetime by itself.
Use `runtime_invoke` for the first verified signatures; do not expose a generic safe `call<T>` over arbitrary pointers.
Nightly is available, but the established foundation does not need nightly-only language features.

The matching-version runtime and managed player-loop experiments now pass. The hook investigation supplies a bounded
candidate and explicit acceptance conditions; the assembly investigation distinguishes writer preservation from missing
input semantics. These findings support implementing the typed invocation layer and a controlled native main-thread
adapter next. Protocol/session and trainer operations remain deferred in this investigation. A fake GUI is unnecessary:
the disposable process host covers the boundary and existing UI captures use fake sessions. No live game validation has occurred.

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

The initial fixture used installed 6000.2.9f1 and had no Unity player loop. Its official Baselib prerequisite was
[extracted as data](../tools/trainer-eval/unity-support/README.md) into a verified TEMP distribution copy. The user has
subsequently installed matching editor 6000.4.8f1 plus Windows IL2CPP support after the first automated consent prompt
was cancelled. Version, changeset and file hashes are verified in the
[installation record](../tools/trainer-eval/unity-support/install-6000.4.md). The matching-version results below supersede
the former adjacent-version limitation for these controlled cases. The game remains untouched.

The virtual/interface extension adds eight checks (34 total), also passing through the production Rust host on nightly
release (`unity-rust-901a6c6769dc46b29f34b5649cc1be02`). Invoking a base MethodInfo on a derived receiver calls the base
body (103); `il2cpp_object_get_virtual_method` resolves the derived override and both virtual/interface invocations
return 203. Thus runtime invocation alone does not establish virtual dispatch. This covers a simple inheritance and
interface fixture; closed generic MethodInfo construction and broader dispatch shapes remain unvalidated.

An additional build with `--incremental-g-c-time-slice=3` passes the same 34 native checks and production Rust ownership
checks (`unity-rust-481f6075692242368d8eb59511e6d91c`). Runtime observations are `incremental=1`, `strict_barriers=0`.
This establishes positive collection/root/reference-write cases with incremental mode enabled, not deterministic
interleaving between collection phases or strict barrier validation.

On 2026-10-18 the unchanged 34-case native fixture and production Rust ownership layer pass on **6000.4.8f1** with
incremental GC disabled and enabled at 3 ms. Nightly release passes both; stable 1.99.1 debug also passes the enabled
build. Runtime builds are `unity-codegen-20261017-222835-30d01d5c` and `unity-codegen-20261017-222911-1ce987ff`;
Rust runs are `unity-rust-2fd3a9d28ee34932a20637d3815c6ce8`, `unity-rust-5b5b58c3f7094c3888910d72c11ae4f6`
and `unity-rust-1740b6e5560b4cae82e02803c3f846dc`. Identities and limits are in the
[Rust-host report](../tools/trainer-eval/unity-rust-host/README.md).

The [controlled player](../tools/trainer-eval/unity-player/report.json) also builds and executes successfully with
6000.4.8f1's actual Windows IL2CPP backend. Awake captures UnitySynchronizationContext on thread 1; thread 5 posts two
payload callbacks after the first Update. Both run on thread 1 in FIFO order in frame 2, after Update and before
LateUpdate. Payloads match, the worker joins and the process exits zero. This is an observed managed scheduling trace,
not a native hook or injected Rust adapter test, and exact frame timing is not a promised scheduling contract.

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
  Its release failure is reproduced and fixed in the harness. A subsequent archive/snapshot audit corrects the earlier
  library concern: published NeoHook 0.16.0 already wraps both context buffers in 16-byte-aligned storage. The harness
  failure is not a NeoHook defect; see [source provenance](../tools/trainer-eval/hook-eval/source-provenance.md).
- Detour still lacks unwind metadata for its relocated stack-changing prefix. A successful body exception does not
  establish asynchronous unwind correctness at every trampoline instruction.
- Four known concurrent callers pass 100 transactional enable/disable pairs, then stop and join before trampoline
  release. This validates a controlled caller set. Callback counts alone miss callers in a relay/copied prefix;
  retaining executable storage until process exit remains the conservative game-helper lifetime policy.

The focused [mitigation audit](../tools/trainer-eval/hook-eval/mitigation-audit/README.md) also runs the conditional
candidate with CFG and CET compatibility enabled. Private negative controls distinguish real enforcement from PE
flags: invalid CFG targets and a static return-address mismatch terminate only in the mitigated control. The relocated
call and six-case unwind corpus pass with the explicit build flags. Process policy is directly queried for the relocated
call and the corpus's GPR case; the remaining five cases were not queried individually. Strict shadow-stack mode and
dynamically marking generated code as CET-compatible remain outside that evidence; compatibility-mode success is not
their substitute. The earlier suspended-thread PC probes were not rerun with context-IP validation enabled.

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
metadata and actual .NET 6 resolution distinguish these failures from missing loader dependencies.

Disposable causal interventions (`loader-interventions-6d7ea882c9104f11b96af5369de6b933`) now confirm the diagnosis.
An unchanged Cecil roundtrip of the normalized input retains the same two failures with no canonical payload changes.
Correcting only the comparer scope changes its error to the missing `Equals` implementation. Correcting that scope and
making the comparer abstract removes its failure; making the stream abstract and nonsealed independently removes the
stream failure. The combined diagnostic output loads all 3,194 reflected types on .NET 6.0.36. All variants retain
3,195 TypeDefs, 16,217 fields and 21,111 methods, with only the planned scope/reference/flag changes in the canonical
comparison. These interventions change instantiability; they do not restore missing methods or correct stream behavior.
They are causal tests, not a production repair policy, and no target methods were invoked.

## Tooling provenance

The current original app tools CLI only reports its catalog; its former `capture` command was removed. Its private
research tooling supplies an existing `pefile` dependency; Mono.Cecil and installed LLVM provide independent metadata
and disassembly views. The original app is not launched. REA's doctor found unsupported Node 25 and missing native
providers/registration, so static findings here come from ordinary tools, with their identities and limits recorded.
