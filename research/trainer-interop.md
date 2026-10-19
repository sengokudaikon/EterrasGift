# Trainer interop and supported scope

The original C#/C++ app at `../EterrasGift` is the behavior reference. Rust replaces the desktop,
injection/transport and native helper; it does not require a general safe Unity interop library.
This document records the execution assumptions and retained findings. Product readiness is in
[trainer-readiness.md](trainer-readiness.md).

## Runtime boundary

The supported boundary is Windows x64, initialized IL2CPP and the inspected Unity 6000.4 game
layout. Authored native fixtures use Unity 6000.4.8f1. Export signatures, metadata storage,
method ABI and the small object/array/MethodInfo layout adapters must match the selected build.
Loaded runtime code and metadata must remain valid while used. The application owns neither
the engine's publication/lifetime rules nor competing native patchers; types and file hashes
cannot establish those facts. Unsupported metadata refuses the operation.

Our code controls attachment leases, strong GC roots, copied snapshots, bounded decoding,
serialized commands and retained helper generations. The control thread owns its attachment;
a request's roots stay alive through every getter, allocation, refresh and exception boundary.
Game object pointers and manager IDs in messages are observations, never dereference tickets.
Native Unity wrappers are checked for a live cached pointer before following the managed graph.

Startup is load DLL, start outside loader lock, connect. The ExecuteTasks callback forwards the
original, then compares the current UnitySynchronizationContext's managed main ID with the
current managed thread ID. A matching callback latches its Windows ID for dispatch. No separate
pre-bootstrap nomination, runtime-thread reader, authority or attestation is required. The
callback-local check has authored coverage; its updated actual-game behavior is not claimed.

The full object graph read/write is synchronous on that callback. It depends on the supported
game not concurrently replacing or destroying that graph during the operation. Roots preserve
managed reachability; they do not prevent unrelated field writes or native-object destruction.
ABI declarations, receiver/type checks and foreign exception boundaries still need correct
implementation. Rust panics must not escape a native callback.

## Product protections

Player.log must identify an offline character in the exact process. Process creation time/path,
artifact profile, runtime offline state and current manager are rechecked at the relevant I/O
and mutation boundaries. The named-pipe server has one owner; a new client does not take over
unfinished work. Unfinished remote calls retain their thread/process handles and input buffers.
A helper generation is reused rather than unloading executing code or reinjecting over it.

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

Owned rooted reads replace tear-prone remote object walking. Existing field readers remain
until a replacement getter passes equivalent readiness/refusal tests. In particular, the old
locale getter can wait for initialization: the current reader inspects only a completed selected
locale operation, then calls the audited identifier/code/version getters. Its identifier has
two references, a 16-byte payload and 8-byte alignment. Empty cached item names are also accepted
by the original C# reader; they are not evidence that field walking is inherently wrong.

The isolated numeric formatter is retained with its tests. Virtual dispatch must use the exact
resolved override; culture and provider affect text. Raw numeric quantities do not always have
the same scale as display text. None of those mechanisms provides a producer, description,
preview or roll-mapping operation by itself. The unused in-game display parser/provider/context
modules were removed. Future host parsing follows the captured locale as the original app did.

## Hook work remains separate

The existing custom scheduler stays until a replacement passes its regressions. The original
inventory has 68 feature registrations over 74 owners: 54 detour targets, 31 byte patches and
7 field sets. Gameplay detours follow the caller's thread, not a universal main-thread rule.
Offline transitions and owner loss must turn effects off and report actual removal failures.

The retained detour 0.10 evaluation passed its exercised calls, transactions and foreign-body
exception cleanup. Missing unwind metadata in a copied prefix remains a deliberate scope
question, not proof that all trampolines are interchangeable. Its suspension guard is private,
thread suspension may skip failures, and dropping RawDetour does not provide a safe transaction.
Keep hook allocations for process lifetime and enable/disable through transactions. Reject
unsupported copied calls until the chosen engine proves the required exception behavior.

Keep scheduler/threads.rs for byte patches: its census, suspension failure handling and refusal
when a thread is inside the target bytes are product behavior. Do not replace it with an
unavailable public detour suspension scope. Do not build gameplay hooks on the specialized
scheduler adapter; the general backend trial finishes before those features.

## Evidence and archive

Current tests, authored fixture sources and the retained hook regressions are the maintained
evidence. `mise run trainer:verify` runs trainer/tier/controller tests, UI tests and workspace
verification with fake-host captures. Native fixture execution remains an explicit, separate
owned-child check. No live-game access is authorized by a passing fixture.

Retained tools: persistent-player; standalone unity-rust-host with its unity-fixture builder;
native-inspect/inspect_pe.py; native-abi; and hook-eval until the backend decision. The original
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
