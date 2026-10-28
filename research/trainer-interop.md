# Trainer interop and supported scope

The original C#/C++ app at `../EterrasGift` is the behavior reference. Rust replaces the desktop,
injection/transport and native helper; it does not require a general safe Unity interop library.
This document records the execution assumptions and retained findings. Product readiness is in
[trainer-readiness.md](trainer-readiness.md).

## Runtime boundary

`eg-trainer-wire` owns the pure messages, codecs, process identity data, framed I/O and
owned dispatcher, and forbids unsafe code. `eg-trainer` consumes those contracts directly.
Callers, including the retained harnesses, import wire directly; game-access no longer
re-exports these modules. Game discovery requires game-access's explicit `il2cpp` feature, which brings in the game-neutral
`eg-il2cpp-runtime` (exports, roots, metadata lookups, invocation): the helper enables it, a desktop-only build does not, and
the standalone native harness uses the runtime crate alone. The helper depends on wire, not
the trainer worker/domain crate, and its source identity follows that dependency change.

The supported boundary is Windows x64, initialized IL2CPP and the inspected Unity 6000.4 game
layout. Authored native fixtures use Unity 6000.4.8f1. Export signatures, metadata storage,
method ABI and the small object/array/MethodInfo layout adapters must match the selected build.
Loaded runtime code and metadata must remain valid while used. The application owns neither
the engine's publication/lifetime rules nor competing native patchers; types and file hashes
cannot establish those facts. Unsupported metadata refuses the operation.

Before any remote work, the host reads UnityPlayer.dll's fixed version resource and
refuses versions outside 6000.4 (also refusing missing/invalid resources). Hashing brackets
that check and binds the checked disk bytes to the target's loaded module. This selects
the supported engine family; it does not prove compatibility of every patch or the
external graph/lifetime assumptions below. Per-operation metadata checks still apply.

Our code controls attachment leases, strong GC roots, copied snapshots, bounded decoding,
serialized commands and retained helper generations. The control thread owns its attachment;
a request's roots stay alive through every getter, allocation, refresh and exception boundary.
Game object pointers and manager IDs in messages are observations, never dereference tickets.
Native Unity wrappers are checked for a live cached pointer before following the managed graph.

The helper resolves attachment, discovery, invocation and mutation tables together through one
private Runtime bundle using a single GameAssembly module. Operations no longer resolve an
independent invocation/write table. Raw unsafe table constructors remain for authored runtimes;
their caller must still supply valid ABI/export provenance. A thread-local attachment registry
shares ownership across ThreadContexts for the same export table, so dropping one context's
last lease cannot detach another context's live leases or roots. Engine-owned attachments
are never detached. External runtime shutdown/detach remains an unsafe caller obligation.

Startup is load DLL, start outside loader lock, connect. The ExecuteTasks callback forwards the
original, then invokes the metadata-checked UnityEngine.Object.CurrentThreadIsMainThread native
predicate. A matching callback latches its Windows ID for dispatch. No separate
pre-bootstrap nomination, runtime-thread reader, authority or attestation is required. The
callback-local check does not depend on the replaceable managed SynchronizationContext.Current.

The full object graph read/write is synchronous on that callback. It depends on the supported
game not concurrently replacing or destroying that graph during the operation. Roots preserve
managed reachability; they do not prevent unrelated field writes or native-object destruction.
ABI declarations, receiver/type checks and foreign exception boundaries still need correct
implementation. Rust panics must not escape a native callback.

## Product protections

The workspace warns on undocumented unsafe blocks (strict Clippy treats warnings as errors).
xtask, trainer and wire forbid unsafe; core, companion, UI, app, trainer-host and helper deny
it except for named platform/interop modules. IL2CPP discovery opts out of the per-block
safety-comment lint: entrypoints document shared runtime assumptions, and local notes explain
specific bounds, ownership and ABI details. Explicit unsafe blocks remain required. Architecture tests
use Cargo metadata, explicit package tiers including tool workspaces, and recursive Rust import/re-export
scans, including UI feature areas. They are syntactic checks, not arbitrary macro analysis.

Player.log must identify an offline character in the exact process. Process creation time/path,
artifact profile, runtime offline state and current manager are rechecked at the relevant I/O
and mutation boundaries. The named-pipe server has one owner; a new client does not take over
unfinished work. Unfinished remote calls retain their thread/process handles and input buffers.
A helper load is reused only while resident. Owner-only Unload fences callbacks, restores the dispatcher, drains helper frames, and unloads on its control thread; native departure evidence is required before a fresh load. Failed departure requires a game restart.

The connected session retains its game binary/metadata read lease across polling, catalog
capture, mapping and writes, releasing it on disconnect or lost admission. Windows helper
and module verification also check retained file identity without rereading content. The
profile's full-hash fallback remains for platforms/storage or existing writers that prevent
a lease; it still validates each operation rather than trusting an unpinned hash.

The helper retains one pipe instance across per-request EOFs in a connected session, with a protected DACL
granting access only to the game process's user and SYSTEM. The client still verifies server
PID; the server still binds ownership to client PID and creation time. Buffered input and
unfinished buffered output are settled before accepting the next peer. Authored child-process
tests cover name retention, malformed suffixes, unread replies and owner recovery. Windows'
[default pipe DACL](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)
also permits Everyone/anonymous read access, so it is not used here.

Reference field stores keep the raw export private and check both roots' runtime origin,
reference class/type code, mutability and declaring-class compatibility before the barrier.
Detached unique previews repeat bounds and live-alias checks at the byte-write site. Direct
refresh lookup rejects virtual/abstract/native replacements before mutation; read-side virtual
getters resolve their implementation explicitly. Closed List<T> calls use CheckedMethod with
an explicit concrete reference-class allowlist, preserving receiver/argument/result checks;
ordinary binding still refuses generics. These checks do not establish the external graph and
runtime lifetime assumptions documented above.

Every Forge edit validates all destinations before its first mutation and checks the complete
expected snapshot, including fields omitted by the legacy fingerprint. This prevents a late
validation refusal after an earlier store. A managed refresh may still fail after mutation;
that is Uncertain, with no implicit rollback or retry.

One EditForge request and `ForgePlan::edit(..., override_enabled)` cover supported edits.
Native ACKs are provisional until a separate admitted read. Results are Pending, Confirmed,
NotApplied before any change, NotKept with known same-target state, or Uncertain. A malformed,
missing or replaced readback is not NotKept. Later connection loss does not downgrade an
already confirmed historical result. Offline/protocol/identity/process failures revoke access;
ordinary refusals and known mismatches can keep a healthy session.

`edits::inspect_readback` is the single comparison policy. The direct level-cache store
requires an exact complete snapshot: it does not rebuild the item. Rebuilding status and roll
edits permit derived names/level to change. Affix reconstruction also permits those derived
values and compares set identity, fixing the original strict-overload omission. Base and identity changes also permit the game's
derived class requirement. Each policy retains its other item/status/identity/roll checks.
Undo is a new conditional reverse edit, with current catalog, full stale-state and override
checks; it is not a raw memory rollback. Mixed UI drafts run ordered single writes, obtain a
fresh read per step and advance only on Confirmed. Only confirmed steps can be undone.

## Read and formatting findings

Optional UI queries return `Lookup::{Ready, Loading, Unavailable}`. Preview, value-table and
description readiness no longer compares display strings. Worker snapshot publication wakes
the Slint observer through watch::changed; a lightweight timer polls only while receipts or
preview debounce remain pending. Capture fixtures supply an event-loop proxy and pump those
wakes, so they exercise the same subscription path rather than relying on idle UI polling.

Owned rooted reads replace tear-prone remote object walking. Existing field readers remain
until a replacement getter passes equivalent readiness/refusal tests. In particular, the old
locale getter can wait for initialization: the current reader inspects only a completed selected
locale operation, then calls the audited identifier/code/version getters. Its identifier has
two references, a 16-byte payload and 8-byte alignment. Empty cached item names are also accepted
by the original C# reader; they are not evidence that field walking is inherently wrong.

The unused numeric formatter and its probes are archived at `%USERPROFILE%/Developer/EterrasGiftRS-archive/2026-10-20-trainer-evaluations/unused-formatters`. Virtual dispatch must use the exact
resolved override; culture and provider affect text. Raw numeric quantities do not always have
the same scale as display text. None of those mechanisms provides a producer, description,
preview or roll-mapping operation by itself. Implicit descriptions now use the game tooltip
builder directly and resolve virtual text/range methods; they do not use the staged numeric
field adapter or infer display values from numeric fields. The unused in-game display parser/provider/context
modules were removed. Host parsing now follows the captured locale as the original app did: explicitly named Windows
numeric symbols without user overrides, finite Float/AllowThousands-style display parsing and
exact unit removal. Standard CultureInfo for the captured language is the supported assumption;
custom game NumberFormatInfo remains unsupported. en-US/de-DE/fr-FR authored comparisons cover
all 256 sampled values; they do not establish every possible culture/provider combination.

## Hook work remains separate

The custom scheduler has been replaced by stock `detour = 0.10.0`. The original
inventory has 68 feature registrations over 74 owners: 54 detour targets, 31 byte patches and
7 field sets. Gameplay detours follow the caller's thread, not a universal main-thread rule.
Offline transitions and owner loss must turn effects off and report actual removal failures.

The [detour trial and product checks](../tools/trainer-eval/hook-eval/detour-trial.md) include
the authored Unity ExecuteTasks route: a throwing task with one Rust cleanup, subsequent task
progress, injected second-target failure with dispatcher recovery, owner-exit restoration and
reinstallation by a new owner. No actual-game execution or gameplay-specific ABI proof is claimed.

`hooks::RetainedHook` leaks each RawDetour immediately; callers retain module lifetime through entry restoration and callback drain. The helper takes no module pins and uses an external, registered gateway that closes callback admission before restoration and counts complete DLL calls. Trampoline/gateway pages and tables remain process-owned after the DLL leaves. It exposes no direct
enable/disable/drop path. Transactions verify entry bytes/state, attempt explicit recovery to
each prior state on failure, and report recovery errors or unknown state. Copied calls and other
control flow are rejected. Original unwind codes cover unchanged relocated instruction offsets. The offhand pairing
work also preserves MSVC redundant REX bytes on register pushes when the stock encoder drops
them: only byte-identical push encodings minus that redundant byte are accepted, and only
when the entire copied prefix has no RIP-relative operands. Other layout changes still refuse;
handler/chained records with this normalization also refuse. A native fixture checks each
prologue/tail boundary with RtlVirtualUnwind.

The old adapter's two conditional jump-back instructions preserve unwind coverage at the tail.
Simply registering metadata over the stock unconditional jump failed the retained virtual-unwind
test at that jump. Detour still owns allocation, relocation, target patching and thread contexts.

The accepted product policy is stock Threads::All best-effort suspension, matching the original
MinHook policy. OpenThread/SuspendThread can fail for reasons other than exit; skipped threads
remain a patch-time risk. The dependency also ignores resume and internal rollback errors. Our
wrapper verifies hook recovery but cannot guarantee atomicity under arbitrary OS failures or
prove every caller was suspended. These limits do not require another backend or dependency fork
under the chosen scope. Handler/chained unwind records remain refused pending an actual target.

The former scheduler/threads.rs is retained as hooks/threads.rs for byte patches: its census,
suspension failure handling and refusal when a thread is inside the target bytes are unchanged.
Detour has no public suspension scope for arbitrary stores.

The suspended section avoids Rust allocations but still calls Win32 context, protection,
cache and resume wrappers. Only NtQuerySystemInformation's census stub is checked. Other
wrappers can be instrumented by overlays/security software and wait on a suspended thread's
lock; deadlock freedom is not established. The unsafe hook/start contract excludes such
instrumentation. Checking a few additional NT stubs would not validate the entire wrapper path.

## Evidence and archive

Current tests, authored fixture sources and the retained hook regressions are the maintained
evidence. `mise run trainer:verify` runs trainer/tier/controller tests, UI tests and workspace
verification with fake-host captures. Native fixture execution remains an explicit, separate
owned-child check. No live-game access is authorized by a passing fixture.

Retained tools: persistent-player; standalone unity-rust-host with its unity-fixture builder;
native-inspect/inspect_pe.py; native-abi; and hook-eval for focused ABI regressions. The original
hook/ABI inventories stay in hooks-fields-audit. One-off loader, bridge, debugger, parser,
main-thread identity and assembly-repair experiments are archived.

Git tag `archive/trainer-evaluations-2026-10-19` at `0a2c4cc` preserves the previous sources and
reports. A verified Git bundle and the ignored outputs are also kept locally at
`%USERPROFILE%/Developer/EterrasGiftRS-archive/2026-10-19-trainer-cleanup/`.
The latest authored player Build and Assets were copied there and all 319 source/product
manifest entries were SHA256-verified before pruning. `workspace/` preserves original relative
paths, and `moves.tsv` lists moved entries/counts/bytes. Moving outputs out of the checkout does
not reclaim disk space. The active helper package and latest authored build pair remain under
artifacts; rebuild receipts are local diagnostics, never a runtime `passed` certificate.

Affix list changes call managed Clear/Add for reference barriers. Constructor refusal precedes
the rewrite; failure after Clear, including the game rarity method, remains Uncertain. The host
rarity projection is an expectation checked by independent readback; it is not the native writer.


Regular-affix mappings use one conditional request shape for context and sixteen sample pages,
with the artifact lease retained for the connected session. Each page deserializes an owned item copy, rejects
cross-aliasing of live/copy list storage and affixes, clones unique/implicit arrays, preserves
set-count fields, and checks the original numeric context before projecting a draft. Only the
private copy is mutated. The game tooltip producer/virtual formatter supplies exact display and
effect text; a varying secondary modifier may become the primary numeric effect. The context
includes serialized draft state and endpoint effects; every page must agree before publication.
No endpoint interpolation or raw-quantity fallback supplies displayed values.

IL2CPP reference Field::SetValue takes the direct object pointer, unlike scalar setters that
take an address of the scalar slot (installed runtime Field.cpp, deref_pointer=false). The detached
array replacement uses that reference barrier; rooted receivers/children survive managed calls.
Constructor/tooltip/culture behavior and absence of concurrent external graph changes are explicit
supported-target assumptions, tested with owned fixtures rather than advertised as universal Rust
safety guarantees. Tables remain full-observation/draft bound in the worker; mapping failures are
optional read failures unless process/protocol/identity/offline access is lost. Normal monitor
polls retain stable unavailable tables and retry only transient failures after a successful read.
Explicit Refresh permits another attempt at stable refusal too. Before a mapping cache hit,
the worker reads the completed locale; monitoring checks it as well, so idle cached tables
expire on the next poll. A change recaptures localized definitions and atomically invalidates
old mappings, descriptions and previews. WrongContext is retryable and rechecks locale rather
than memoizing a failure under the old language. No write certainty is inferred from formatting.

The UI's disposable MappingRead receipt stops obsolete draft lookups before submission or
between completed sample pages. An in-flight RPC always drains and fatal errors still revoke
the connection. Ordinary receipts, including writes, retain their completion semantics. The
editor reports all current requests, including its cached tables, so returning to a cached
draft cancels unused queued work. Reloading keeps the last value text visible, but its stale
table cannot authorize typing or scrubbing. Retryable connection waits remain visible in
snapshots/events without writing a fault file on each automatic retry.


Unique tooltip/preview requests share the conditional full-Forge description envelope. Preview
adds a nonempty, distinct, storage-bounded list of unique-roll byte changes; it is not EditForge
and never enters write certainty/history. The native reader calls getUnique(UInt16), validates
and roots its Entry, walks the bounded display order and invokes GetUniqueModifierInfo with the
actual Int32 slot enum. Both property and preformatted lines use virtual text/range methods.
Preformatted replacements are a validated closed List of game references; their range and roll
getters use checked invocation, while the closed-list getter is one narrow raw invoke with an
immediately rooted return/exception. No host copy of the game's description parser is introduced.

The game's lookup/parser/tooltip cache effects remain supported-target assumptions, as does
stable culture and no concurrent graph mutation during a synchronous operation. Roots establish
retention across GC; they do not prove those external behaviors. Preview reuses the existing
independent-copy path, compares initial text/ranges/associations against the selected item and
clones roll arrays through the runtime reference barrier before byte stores on the private copy.
Native publication rechecks the full live graph and locale. The latest worker preview is bound
to both ExpectedObservation and RollEdit; cleanup/generation/item changes discard it. A replaced
UI draft never displays an earlier draft's text, and dropping its receipt never cancels accepted
native work or authorizes replay. Production unique mapping and unique/implicit writes remain
separate unsupported operations.

## Helper departure and travel (2026-10-22)

The helper caches ClientStateManager's static app-state getter and enum literal metadata;
250 ms checks stop on Login/CharacterSelect or online state, preserve InGame travel, and refuse
work during unknown state with a three-second departure grace. Bootstrap's 96-byte configuration
binds controller PID/birth before Discover. No module pins are taken.

CharacterLoading is a pre-store response for a missing offline manager while the app remains
InGame. Sessions revoke their previous manager/status and editor observations, retain their
binary lease and transport, and recover through a fresh read. A transport read failure followed
by proven departure can reconnect later; uncertain writes remain fenced and are never replayed.

`mise run trainer:unload` passed 100 concurrent native reloads plus blocked-callback retention.
`mise run trainer:player:lifecycle` passed six external Unity 6000.4.8f1 loads, four seconds of
InGame manager absence, and module-absence/pristine-dispatcher proof after each departure.
The player never calls a helper export in lifecycle mode, avoiding its own IL2CPP loader reference.
This is authored fixture evidence; live Last Epoch testing remains deferred until requested.

## Windows loader wrapper and Steam overlay (2026-10-22)

Read-only inspection of the user's reported preflight failure found kernel32!LoadLibraryW
redirected through a private trampoline into gameoverlayrenderer64.dll. The local wrapper was
an ordinary x64 RIP-relative import thunk; KernelBase!LoadLibraryW matched both processes.
Bootstrap now resolves that file-backed local thunk only to the exact declared KernelBase
export, then verifies the implementation's local file bytes, target module identity and remote
instructions. It never follows the target's overlay trampoline or changes its hook. Unexpected
local thunks/import targets and patched implementation bytes still fail before submission.

Game-data freshness distinguishes a successful source-file check from a definitions rebuild.
The rebuild timestamp is saved in files.json and survives cache reuse; older indexes have an
unknown timestamp. Failed reads display their error rather than relabeling stale data as current.

## Live callback refusal (2026-10-22)

During the reported connect/wait loop, read-only inspection confirmed the loaded helper's full
bytes matched the running debug desktop's embedded helper. The process-owned gateway's active
counter changed, but Shared.main_thread stayed zero and the watchdog never ran. The request
timed out as NotReady: "Request canceled before the main thread took it; nothing ran". This
isolates the refusal before queue dispatch, in the managed synchronization-context identity
check; it does not identify which game/mod replaced or removed the context.

The callback now asks UnityEngine.Object.CurrentThreadIsMainThread, checked as a static zero-arg
Boolean method on UnityEngine.Object. The initialized Unity6000.4 native predicate and its cold
icall resolution are supported-target assumptions. The installed metadata contains the method;
prior retained native inspection of this engine established its OS-thread comparison. No native
global address, guessed TID or pre-bootstrap main-thread nomination is used.

A NotReady handshake no longer becomes retryable merely because cleanup proved the helper
gone. Transport departure during a handshake rereads offline admission: a character leaving is
a normal gate wait; unchanged admission reports a stopped connection and requires explicit Retry.
Normal departure after an established session still allows automatic connection for the next
offline character. No submitted gameplay write is retried.

Verification passed: trainer:verify, inspected Settings/Forge captures, and six external Unity
6000.4.8f1 helper loads whose first Status runs with null or replacement synchronization contexts.
All departure triggers and the four-second InGame travel check still passed. The fresh selected
desktop release build embeds the corrected helper. The user confirmed the connection now works
in live Last Epoch on 2026-10-22; remaining functionality was only described as mostly working,
so this does not establish complete Forge or gameplay-hook coverage.
Investigation receipts and cleanup notes: `../EterrasGiftRS-archive/2026-10-22-trainer-connect/NOTES.md`.
