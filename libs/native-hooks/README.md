# native-hooks

Function hooks in a running x64 Windows process: retained detours on stock `detour = 0.10.0`, generated gates (`leaf`, `postfix`,
`scalar`, `callback`), the assembler that writes their effects (`emit`) and code patches (`patch`), with groups of them handled
alike (`entry`, `gate`). In-game code only.

## Generated effects

`emit::Code` assembles an effect's bytes. `LeafCode` and `PostfixCode` open a gated effect in their family's frame and close it into
that family's effect; `Code::gated` is the same admission for code written directly, as a frame's displacements are per family. An
app's effects can be pinned byte for byte by preparing their gateways from fixed inputs and reading the published pages: nothing is
called, and only the page addresses differ between runs.

Leaf and postfix gates also retain a two-word settings snapshot. Its serialized owner uses `set_wide`; `Code::snapshot`
loads the payload into XMM4/XMM5 only when its version stayed even and unchanged. Payload words are atomic, and a
reader makes one attempt: a paused or concurrent writer skips the effect instead of making a caller spin. This storage,
like the gateway itself, survives the module that prepared it.

Generated unwind entries occupy a separate region after the data page's first 128 bytes. Compile-time layout checks
keep each gateway's mutable state out of that region, including both words of wide settings. Settings publication
must never change the registered function entry or its unwind program.

`postfix::scope` adds invocation-local restoration for up to four four-byte receiver fields and four argument
positions. The effect saves after admission and before mutation; the gateway restores on return and its retained
Windows unwind handler restores during exception unwind, never first-pass search. Original results and opaque
register arguments survive. The receiver must remain live and serialized through the original call; recursive
calls get distinct stack records. Same-receiver concurrent field mutation is outside that contract. Cleanup,
snapshots and unwind metadata remain valid after closing the gate and releasing its Rust owner.

`scalar::prepare_before` writes a pinned field before the original reads it. A field value different from the
last override becomes the new baseline. Closing/draining precedes conditional restoration and pin release;
a newer foreign write wins. Unlike the original-first scalar, it never treats an unchanged override as a new
game baseline. Its active span covers only its managed-field accesses, so an exception from the original does
not leave the scalar busy.

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

## Groups

`entry::commit` changes a group of detours and interior patches together. A mixed activation commits the closed detours, then the
patches, and restores the detours' prior states if the patches fail: this is compensation, not one suspended write, so interior
patches can take effect between the two transactions. Cleanup attempts both families even when one fails. Every owner is retained
after any failure.

`callback::restore_and_drain` closes a callback gate, restores its entry and waits for calls already past the gate. On failure the
caller must keep the module loaded.
