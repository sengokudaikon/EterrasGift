# Trainer validation gates

Investigation started 2026-10-17 after the first evaluation checkpoint, at the user's request to make runtime, hook and
assembly conclusions stronger before protocol/session implementation. Original app source remains at
`22eb727b0590ed825957cb05c2039b4c0768c5e2` for this investigation. Product protocol, gameplay operations and UI wiring
remain separate; one read-only live scheduler request is now validated.

## Implementation verdict

The Rust port is viable. Proceed with a narrow, audited runtime boundary rather than adopting the evaluated bridge
unchanged. Implementing a layer is distinct from clearing it for live use: the following work can proceed against the
controlled fixtures while the remaining acceptance gates stay explicit.

| Area | Ready to implement or already implemented | Gate before game use |
|---|---|---|
| Runtime calls | Checked invocation now constructs primitive and scalar ref/out storage, validates receivers/reference classes, and roots results/exceptions. The production layer executes in the real 6000.4.8 fixture, including explicit virtual/interface resolution. | Managed-reference ref/out, enums, structs and generic signatures remain rejected. Validate each actual game operation separately. |
| Thread/GC ownership | Production thread-affine attachment leases and strong roots pass against actual 6000.4.8. Native reference field/array edges survive collection with incremental mode disabled/enabled. | Incremental phase interleaving and game shutdown ordering. Attachment does not establish Unity main-thread authority. |
| Main-thread commands | First-party scheduler with process-thread transactions passes owned native tests, authored Unity normal/exception/worker/external-bootstrap runs, and one authorized offline-game checked frame read followed by entry restoration. | Protocol/session/offline admission, per-operation authority and shutdown/transient effects. External patch/context actors and code reclamation are not validated. Neither stock hook engine is adopted. |
| Managed files | Dotscope can implement offline transforms with independent before/after validation. | Decide malformed-type policy explicitly; broader format features remain separate requirements. |

Unsafe code belongs at export calls, pointer/receiver construction, object ownership transitions and the
hook boundary. A typed Rust wrapper cannot establish an arbitrary game's metadata/ABI or runtime lifetime by itself.
Use `runtime_invoke` for the first verified signatures; do not expose a generic safe `call<T>` over arbitrary pointers.
Nightly is available, but the established foundation does not need nightly-only language features.

Matching-version runtime and managed player-loop experiments pass. The first checked invocation subset and narrow
scheduler adapter are implemented. Authored Unity tests include a managed exception crossing its Rust frame and
subsequent engine recovery; the external bootstrap and one offline-game frame read also pass. The assembly investigation
separately distinguishes writer preservation from missing input semantics. Protocol/session and trainer operations
remain separate work. A fake GUI is unnecessary: disposable process hosts cover the boundary and existing UI captures
use fake sessions. The live probe restores its entry and retains the helper until process exit.

## Evidence standard and scope

Keep these evidence levels separate: source/header contracts; synthetic ABI experiments; static inspection of the exact
game artifacts; execution in a controlled IL2CPP fixture; execution in the actual game. One does not stand in for the next.
The user authorized the first live observation on 2026-10-18 after starting a fully offline game. Its bounded scope is
query/read and noninvasive debugger inspection, without helper injection, game API calls or hooks. Installed game files
stay untouched. The later separately authorized helper/bootstrap performs one queued read-only getter in that same
offline game, restores its entry and retains its code until exit. Assembly transforms and other experiments remain
in disposable fixtures. Neither milestone authorizes arbitrary gameplay edits or further live sessions.

Synthetic tests alone do not clear runtime acceptance. Record residual uncertainty and improve the next experiment
rather than weakening acceptance criteria. Exact game runtime claims require actual runtime evidence later.

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

### Checked invocation implementation, 2026-10-18

`eg-game-access::il2cpp` now exposes `InvocationContext`, `CheckedMethod`, explicit `Scalar`/`Argument` storage and
rooted `InvocationOutcome` values. `raw::InvocationApi` resolves its complete public-export table separately from the
existing ownership table. No runtime structure layout or generic Rust `unbox<T>` is introduced.

The two unsafe admission steps have separate obligations. Constructing the context establishes the matching live API,
immutable metadata, nonmoving collector and externally coordinated runtime lifetime. Binding a method authorizes its
body, callees, initializers and callbacks for **every** value/receiver accepted by the checked signature, on this thread,
without retaining native argument storage. Neither metadata checks nor attachment establish Unity main-thread permission,
native engine-object liveness, aliasing safety or the safety of arbitrary game/native code. Bound methods and results
remain tied to the attachment and are neither Send nor Sync.

Safe invocation checks arity, exact primitive kind, byref shape, root provenance and receiver/reference class before
executing the body. Scalars use owned temporary storage; managed code never writes a Rust enum discriminant or bool.
Boolean payloads use bytes and normalize on copyback. Reference argument entries contain the rooted object pointer
directly. Boxed value receivers require the exact declaring class and are unboxed while their owner remains rooted.
Non-null results and exceptions are rooted before metadata queries; primitive boxes require exact class validation
before decoding. Scalar byref writes copy back even on a managed exception or result-root failure.

Invocation is deliberately direct. Virtual/interface execution resolves `il2cpp_object_get_virtual_method` explicitly,
then independently authorizes/binds the selected method. Direct binding rejects abstract declarations. This keeps
override authorization visible instead of silently selecting another body.

All three final-source runs pass, including the existing 34 native checks and normal shutdown:

| Runtime GC setting | Rust configuration | Build artifact | Execution artifact |
|---|---|---|---|
| Incremental 3 ms | Nightly 1.101 release | `unity-codegen-20261018-125600-9a5a566c` | `unity-rust-38a1d7d0764d4b10b43b7e41b3d2d409` |
| Incremental 3 ms | Stable 1.99.1 debug | same build | `unity-rust-2ca7fd6423bb4911bffde753dc1e8a14` |
| Incremental disabled | Nightly 1.101 release | `unity-codegen-20261018-125722-dbdb6b48` | `unity-rust-65114510b096499e91593f2d6605b4d6` |

Artifacts are under `artifacts/trainer-eval/`. The committed [host report](../tools/trainer-eval/unity-rust-host/report.json)
indexes identities and scope. The runner records all compiled host/runtime Rust sources before building and verifies
them after execution, together with runtime/data identities. The actual host tests every supported primitive input kind,
Boolean/i32 boxed results, scalar ref/out, null argument/result, void return, array element/rank compatibility,
mutating boxed receivers, rooted exceptions through collection, and byref writes completed before a managed throw.
Wrong arity, same-width wrong scalar, byref shape, receiver, context and array class are rejected. Explicit base dispatch
returns 103; resolved virtual/interface dispatch returns 203. Seven fake tests add noncanonical Boolean byte 3,
root failures, wrong result-box class and unsupported metadata cases. Independent read-only review found no concrete
soundness defect under the documented unsafe contracts; this is not a formal proof.

This is a useful subset, not trainer operation parity. Managed-reference ref/out, enums, arbitrary struct parameters or
results, pointer/function-pointer signatures, byref returns, generic methods and constructed generic declaring types
are rejected. Full collection with incremental mode enabled still does not prove phase-interleaved barrier behavior.
An invocation error does **not** guarantee that the body was skipped: result rooting/validation may fail after writes.
The future operation/session layer must preserve uncertain outcomes and never infer rollback or automatic retry safety.

The original native source is byte-identical between research baseline `22eb727` and current adjacent app `e1c1ed4`.
It identifies the next concrete gaps: `ItemAffix..ctor` needs enum identity/underlying-width checks; faction/monolith
lookups need GC-traced object ref/out storage; affix edits need methods on constructed `List<ItemAffix>`, which is distinct
from constructing generic methods. Add those cases and public-API reference writes to the controlled fixture boundary,
then production helper admission/lifecycle and protocol/session integration. Real operation tests must precede app wiring.

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

The [bounded target policy](../tools/trainer-eval/hook-eval/supported-policy.md) defines a specific conditional-transfer
strategy, near-entry requirement and rejection conditions. The actual dispatcher changes RSP and therefore needs the
tested dynamic unwind metadata; a leaf-prefix-only engine remains unsuitable.

The [combined player probe](../tools/trainer-eval/unity-hook-player/report.json) now invokes the real 6000.4.8
ExecuteTasks through that native Rust adapter. The worker posts marker 7 and queues owned value 42 before a bounded
join in Update. Unity's original method runs its managed callback before the Rust dispatcher executes once on the
captured main thread. Later quiescent disable restores entry bytes and leaves Unity delivering marker 9, with no Rust
replay across three more Updates. All native code/table/module owners remain retained.

The optional exception run posts a genuinely throwing managed callback. Its exception crosses the original method
and the `C-unwind` Rust hook to Unity's invoker; the Rust original-call destructor runs once. Three hook entries,
two normal returns and one exceptional cleanup are recorded. Unity logs the expected InvalidOperationException,
delivers recovery marker 8 and continues after disable. The scalar command remains executed exactly once. Both final
normal/exception runs exit zero on the same hashed player with the release Rust plugin.

This integration's actual policy is DEP enabled/permanent, dynamic-code prohibition off, CFG off, shadow stacks off
(`0x100`). It does not transfer the independent mitigation fixture's enabled-profile results to the combined player.
Remaining before game use: production rejection/cleanup paths, real installation quiescence and exclusive ownership,
and actual-game opt-in validation. Passing finite concurrency experiments is not a proof of race freedom or safe helper unloading.
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

## Native debugger readiness, 2026-10-18

At the user's request, Microsoft.WinDbg 1.2610.1001.0 is installed with its bundled x64 CDB 10.0.29661.1004.
CDB's Microsoft signature is valid. An authored native Rust process passes actual attach, private PDB resolution,
named breakpoint, register/stack/source inspection, explicit detach, post-detach resume and normal exit. The probe
selects only the child it creates; it has no PID/name option for other processes. Exact identities, evidence and the
repeatable mise task are in [debugger/README.md](../tools/trainer-eval/debugger/README.md).

Microsoft/Unity symbol stores and a local cache are configured through the user's `_NT_SYMBOL_PATH`. This test uses
local matching Rust symbols and makes no claim about available game PDBs or remote symbol retrieval. Earlier Swift
LLDB tests did not establish clean target recovery; use the verified Microsoft debugger route. The setup probe did not
start or attach a game. The subsequent user-started session supports the read-only observation below. Native debugger
readiness does not enable managed C# debugging in a release build.

### First live observation

The user confirmed the running game was offline. Explicit PID 10472 and executable path were checked against the
opened process's creation identity before/after observation. GameAssembly/UnityPlayer file identities match the prior
static investigation. All 81 resident ExecuteTasks bytes at RVA `0x405c790` match both the baseline and current file;
its live unwind bytes are `0104010004420000`. CDB independently decodes the same `[0x405c790,0x405c7e1)` function,
version 1/no-handler record and alloc-40 prologue. No helper or entry patch is present at that target.

Actual process policy queries succeed: DEP flags `0x3`/permanent, ASLR `0x5`, dynamic-code `0`, CFG `0`, signature `0`,
shadow-stack `0x100` (enable bit clear). This matches the authored player's observed hook execution profile, closing
the former static-PE-versus-live-policy uncertainty for this session. It is not proof of supported strict CET or a
different process/configuration. Each future installation must query its actual process again.

CDB noninvasive inspection captures 393 bounded-depth thread stacks, including the engine-named Unity main thread
TID `0xe718`. It quits/detaches in 1.55 seconds; process creation identity is unchanged, the game remains alive and
responsive, and both selected DLL files are unchanged. Game PDBs were not used: nearest-export labels with large
offsets must not be treated as exact function names. The captured stack does not itself observe ExecuteTasks execution.
Evidence and exact limits: [live-report.json](../tools/trainer-eval/debugger/live-report.json). This is ordinary Windows
API/CDB evidence, not REA Evidence; REA tools remain unavailable in this session.

### First live scheduler request

The next authorized milestone is now complete. `eg-game-access::scheduler::SchedulerHook` replaces the evaluation
NeoHook dependency with a target-specific implementation. It admits only the exact SUB/CMP prefix and alloc-40
record, publishes a 23-byte JE/JNE trampoline with registered unwind info, and retains all exposed storage/modules.
Transactions serialize before freezing; opened threads are checked back to this process; changed native censuses,
entry PCs, stale bytes, handler records, cross-page writes and unsupported policies reject. There is no RIP redirection.
Suspended sections allocate/log/call no managed code. Five owned native tests pass on stable debug/nightly release,
including per-PC unwind with both branch outcomes and eight callers across 40 toggles. OS suspension/context APIs
have no wall-clock guarantee, and persistent resume failure remains a recovery condition.

The final authored Unity player passes normal and managed-throw modes with background-worker installation/restoration
and a checked frame-count read matching managed Time.frameCount. A separate idle-mode child invokes no helper import;
the external controller loads the DLL and starts its explicit bootstrap after loader completion. It passes one queued
read/restoration and independent continuing-frame checks. Controller/dependency/helper hashes gate subsequent live use.

Fresh read-only preflight confirms the same PID 10472, main TID 59160, module hashes,81 dispatcher bytes, original
unwind record and observed policies. The authorized live bootstrap attaches its worker, resolves the target through
metadata, installs, runs original ExecuteTasks before dispatch, invokes exactly one checked Time.get_frameCount,
acknowledges completion, closes admission and restores the original entry. Returned frame: 372,952. There are two hook
entries/two normal original returns, one command execution, zero errors/thread mismatches, and execution TID 59160.
All 81 original bytes are restored; unwind and runtime DLL files are unchanged; the same game remains responsive.
[Exact evidence](../tools/trainer-eval/debugger/scheduler-report.json) records native/controller identities and scopes.
This is ordinary WindowsAPI evidence, not REA Evidence.

The helper stays loaded with its executable page, relay and registered table until process exit. Already admitted
callbacks can finish safely; restoration is not a drain/unload proof. Exclusive target ownership, cross-library patch
coordination and external context/thread actors remain unsafe caller obligations. The controller is one-shot and rejects
another helper load in this process. This success establishes the narrow live read path, not arbitrary call safety,
strict CET, gameplay writes, protocol 21/session/offline gating or product disconnect/shutdown behavior.

Next implementation: helper/protocol 21 transport and a session state machine against authored hosts, with positive
offline admission and per-operation metadata/authority checks. Add read-only game operations before persistent/transient
writes; reference-writing, enum/struct/generic support and incremental-GC interleaving remain their own gates.

## Tooling provenance

The current original app tools CLI only reports its catalog; its former `capture` command was removed. Its private
research tooling supplies an existing `pefile` dependency; Mono.Cecil and installed LLVM provide independent metadata
and disassembly views. The original app is not launched. REA's doctor found unsupported Node 25 and missing native
providers/registration, so static findings here come from ordinary tools, with their identities and limits recorded.
