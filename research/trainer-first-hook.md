# First gameplay-hook experiment

Checkpoint: 2026-10-19. The authored unpatched Boolean ABI fixture passes all four
configurations. Physical entry installation remains a separate experiment; gameplay-hook
capabilities remain unsupported. The existing scheduler and conditional rate writer are separate layers.
The [source audit and provenance](../tools/trainer-eval/hooks-fields-audit/README.md)
bind the original repository at `e1c1ed4e9e0ab6404574113f2dcf1179b2113629`.

## Candidate and source behavior

`mysterious-rift` is the smallest initial candidate. The original implementation in
`../EterrasGift/native/hooks/world.h:5` declares an instance Boolean callback with no
explicit arguments:

```cpp
bool (*originalNextEncounterEggs)(void* tracker, void* method);
// Existing behavior:
return active(mysteriousRift) || originalNextEncounterEggs(tracker, method);
```

The corresponding descriptor is in
`../EterrasGift/src/Trainer/EterrasGift.Trainer.Application/Features/TrainerCatalog.cs:259`.
Enabled behavior deliberately skips the original, including its side effects. Disabled
behavior forwards both arguments unchanged and preserves the original result and exceptions.
No additional receiver dereference, player filtering, numeric configuration or restoration
of gameplay fields is needed.

The cached LE method inventory identifies
`TimeBeastTracker.NextEncounterShouldBeEggs()` as nonstatic, nonvirtual, nonabstract,
returning `System.Boolean`, at RVA `0x1580C30`, length `0xB3`. It records no other method
with that RVA in that LE image. This does not exclude cross-image aliases, interior
entries, thunks or shared code. Admission must establish those facts separately.

The proposed ABI oracle is:

```rust
type RiftOriginal = unsafe extern "C-unwind" fn(
    *mut core::ffi::c_void,
    *const MethodInfo,
) -> bool;
```

Generated C++ and an authored executable must validate the Boolean return, hidden
argument forwarding and unwind behavior. This cannot be cast to `ExecuteTasks`.
`SchedulerHook` admits a different static ABI, entry prefix and alloc-40 unwind shape.

## Completed unpatched ABI experiment

The [source-bound oracle](../tools/trainer-eval/unpatched-bool-abi/README.md) passes
stable debug and nightly release with incremental GC off/on. Each mode covers 46
cases, 12 managed exceptions and eight unwind paths through Rust cleanup guards.
Both workers join, all roots are released, and the existing native checks and shutdown pass.
Enabled bypass, disabled forwarding, receiver/hidden-argument forwarding and recovery
are tested without patching an entry. The generated hidden argument is unused; its
forwarding witness is the native caller and spy. Synthetic permission and the standalone
entry thread do not establish production owner/offline/main-thread admission.

Static inspection also shows why a new adapter is required. The authored target starts
with `SUB RSP,40; INC dword ptr [RCX+0x14]`, seven bytes. Its unwind record describes
a later conditional RBX save as well as the initial allocation. Normal return occurs
before the reported prologue end. A trampoline must describe only its displaced
instructions, with actual Windows unwind checks at each meaningful instruction boundary.
The [retained design](../artifacts/trainer-eval/bool-adapter-design-a87be882edca4cc384f96167b17898f7/manifest.json)
proposes a separate bounded adapter and complementary conditional jumps back to the
continuation. Microsoft's [RtlVirtualUnwind contract](https://learn.microsoft.com/en-us/windows/win32/api/winnt/nf-winnt-rtlvirtualunwind)
classifies an unconditional relative jump outside the function as an epilogue marker;
an ordinary jump-back therefore needs more than instruction-relocation reasoning.
Encoding, unwind behavior, alias rejection and caller exclusion remain
unproved by the ABI result. In particular, the old spy calls the original symbol;
a physically patched experiment must first publish and forward through a typed
trampoline, or it would recurse.

## Completed Windows metadata experiment

The [synthetic unwind oracle](../tools/trainer-eval/unpatched-bool-abi/bridge-unwind-report.json)
passes both frozen native images on stable debug and nightly release under Windows
build 26100. Each run checks 52 original instruction-boundary contexts and eight
proposed bridge contexts, including both ZF states, the early return and late RBX
save. Stack/return address, nonvolatile GPRs and XMM6-15 lower 128 bits match the
source-derived expectations. The bridge has its own allocation-only unwind record;
the relay is only a null-lookup leaf control.

Bridge bytes remain data in a read/write page. No bridge instructions, IL2CPP
initialization, GC pressure or entry writes occur. These are OS- and image-specific
metadata observations, with all inputs and OS identities unchanged. Actual bridge
execution, complete alias/caller admission, a closed caller gate and physical
installation/restoration remain separate tests. The portable ABI tool also has its
own [four-run reproduction receipt](../tools/trainer-eval/unpatched-bool-abi/portable-report.json),
without changing the original proof-bearing Rust or fixture sources.

## Ownership and callback admission

Keep physical entry ownership separate from logical permission. A resident owner retains
the original trampoline, relay, code, unwind registration and module lifetimes before
the first possible patch write, including uncertain installation failures. Publish a valid
original trampoline before exposing an entry that can reach the callback.

Logical disable closes an atomic permission word. An installed entry can remain in
resident passthrough while restoration is unproved. Report that physical state accurately;
logical off does not prove original bytes or callback quiescence. No allocations or code
can be reclaimed merely because a callback counter momentarily reaches zero. The initial
experiment retains them until its owned child exits.

The callback performs no allocation, logging or mutex acquisition. Other threads forward
immediately to the original before IL2CPP access or attachment. The effect path requires
the independently identified main thread, an existing engine attachment, current owner
lease/epoch, a fresh runtime offline Boolean and a narrow non-reentrancy guard. An owner
lease needs retained process-liveness evidence, not a cached PID. Owner loss must disarm
permission before stop paths return; a later client must not inherit it.

Do not call `RuntimeAccess::status` inside the hook. Its current contract requires execution
after the original dispatcher and no active gameplay hooks. It rediscovers metadata,
roots a graph and reports an empty feature set. Instead, investigate a separately admitted
cached exact `_isOnlinePlay` field and its initialized static-Boolean read. That read still
requires source permission for runtime, initialization, locking, GC/profiler and reentry
effects. Do not manufacture a Rust atomic reference over managed storage.

A cached status or manager token cannot prove current character/native lifetime. The
authored fixture can exclude destruction explicitly; production needs a callback-safe
liveness predicate or independently established lifetime ownership. It also needs a new
status/feature-report contract compatible with the admitted active hook.

Physical installation needs its own instruction, unwind, alias and overlap proof. Reject
unsupported prefixes, handlers/chaining, ambiguous targets and changed entry bytes. A
closed authored caller set can prevent target calls during installation. That does not
establish arbitrary-game thread creation, saved-context/APC, suspension, mitigation or
exclusive-patcher guarantees. Hidden `MethodInfo*` identity cannot distinguish folded
callers when generated calls can omit it.

## Authored validation

Use a noninlined instance Boolean method with receiver-specific results, original-call
counters and an intentional side effect. Test:

- Disabled forwarding executes the original once; enabled behavior skips it and returns true.
- Worker calls pass through with zero hook-side IL2CPP/attach calls.
- An online transition immediately before a call passes through before scheduler polling.
- Owner loss and reentry close the effect path without automatic rearming.
- Original exceptions cross the callback/trampoline and reset callback bookkeeping.
- Repeated logical toggles retain one physical owner and accurate passthrough state.
- Deliberately shared/aliased targets are refused by the initial policy.
- Any restoration occurs only after fixture-controlled caller quiescence; memory remains retained.

Helper integration must also resolve the gameplay-cleanup TODO in `server.rs::reap` and
disarm effects before stop-path early returns. This experiment does not establish support
for offhand, player-stat hooks, global RNG/perfect rolls, raw patches or the remaining
feature catalog.

## Completed bridge execution experiment

The [authored bridge harness](../tools/trainer-eval/bridge-execution/README.md)
now executes the 19-byte relocated prefix and rejoins the unchanged original
continuation. Both original native GC builds pass stable debug/nightly release,
with 92 cases, 24 managed exceptions and 16 foreign Rust cleanups per process.
Starts of -1/0 exercise both real INC flag outcomes. Every original body result and
side effect is independently checked; the full 108-byte target stays unchanged.

The harness publishes private RX code, allocation-only unwind metadata and its
original pointers under a closed reserved/active caller gate. All code and
registrations remain resident until owned child exit. Four joined workers retain
the existing root/attachment contracts. The separate checked-in package replay
also passes all four modes using current ROOT dependencies and the exact retained
native images; both source sets and independent reviews are retained separately.

This closes the synchronous bridge-call/foreign-unwind experiment. It adds no
entry writes, alias inventory, physical owner, runtime offline/owner-liveness
policy, gameplay capability or actual-game execution. Physical installation and
restoration remain unvalidated.


## Fresh offline-flag read: static contract audit

The [retained offline-read audit](../tools/trainer-eval/bridge-execution/offline-contract-report.json)
independently binds 22 native spans and 18 SDK/original-app source snapshots.
For the exact warm normal-static Boolean path, public static_get copies one byte
without metadata locking, managed invocation, allocation or explicit GC/profiler
calls. This is a narrow mechanism under valid stable metadata/storage, not callback
authority. Cold metadata setup has locking/allocation/GC effects and remains excluded.

Four obligations remain separate: layout/SetupFields warmness; successful completed
managed initialization; meaningful current offline character/owner/generation;
and writer/transition exclusion through the whole synchronous decision.
il2cpp_class_is_inited reports only size_inited, explicitly permitting incomplete
managed initialization. The game's getter can execute class initialization; it
cannot replace the warm reader under the same contract. Setter, inlined Reset and
cctor write the flag, and seven direct caller sites include async continuations.
Their names do not prove thread affinity or a complete writer inventory.

A false flag alone supplies no character/receiver/native lifetime or owner lease.
RuntimeAccess::status remains forbidden in gameplay callbacks: its explicit
after-original/no-active-hooks contract, discovery, allocation and rooting remain
unchanged. No new reader, callback, target invocation or production code was added.
