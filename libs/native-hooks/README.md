# native-hooks

Function hooks in a running x64 Windows process: retained detours on stock `detour = 0.10.0`, generated gates (`leaf`, `postfix`,
`scalar`, `callback`) and code patches (`patch`). In-game code only.

## Retained detours

`RetainedHook` leaks each `RawDetour` immediately; callers retain module lifetime through entry restoration and callback drain.
Trampoline and gateway pages and tables stay process-owned after the DLL leaves. It exposes no direct enable, disable or drop
path. Transactions verify entry bytes and state, attempt explicit recovery to each prior state on failure, and report recovery
errors or unknown state. Copied calls and other control flow are rejected.

Original unwind codes cover unchanged relocated instruction offsets. MSVC's redundant REX bytes on register pushes are preserved
when the stock encoder drops them: only byte-identical push encodings minus that byte are accepted, and only when the whole
copied prefix has no RIP-relative operands. Other layout changes refuse, and so do handler and chained records with this
normalization. A native fixture checks each prologue and tail boundary with `RtlVirtualUnwind`.

The two conditional jump-back instructions preserve unwind coverage at the tail: simply registering metadata over the stock
unconditional jump failed the virtual-unwind test at that jump. Detour still owns allocation, relocation, target patching and
thread contexts.

Argument-only leaves preserve Windows x64 argument locations and tail-jump without a new frame.

## Suspension limits

Commits use `Threads::All`, best-effort suspension. `OpenThread` and `SuspendThread` can fail for reasons other than exit, so
skipped threads remain a patch-time risk, and the dependency ignores resume and internal rollback errors. The wrapper verifies
hook recovery but cannot guarantee atomicity under arbitrary OS failures or prove every caller was suspended. Handler and chained
unwind records stay refused until an actual target needs them.

## Code patches

`patch` is a separate byte transaction behind the same lifecycle: detour has no public suspension scope for arbitrary stores. It
prepares every range before writing, compares actual bytes, retains page and cache uncertainty, and recovers pre-transaction bytes
on failure. Its Toolhelp snapshot opens handles and allocates aligned contexts before suspension; a peer inside a changing range
refuses the transaction. It cannot establish a complete census while threads are being created, and its unsafe boundary excludes
execution by snapshot-missed or new threads and return addresses inside a changed range.

The suspended section avoids Rust allocations but still calls Win32 context, protection, cache and resume wrappers. Overlays or
security software can instrument those and wait on a suspended thread's lock, so deadlock freedom is not established. The unsafe
contract excludes such instrumentation.
