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
