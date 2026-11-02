# Trainer service

`eg-trainer` owns offline admission (`connect`), the game catalog (`definitions`), the feature catalog and profiles
(`features`), Forge edit planning and conditional Undo (`item`), and the serialized background worker (`backend`).

Count editors share typed read/apply commands and receipts. Their observations bind the process, connection generation,
manager and full container/entry snapshot. Apply never retries and needs independent readback; the helper owns currency,
quantity and removal semantics. The feature catalog declares these editors separately from persistent gameplay hooks.
`eg-trainer-wire` owns the bounded messages and codecs; the helper (`eg-trainer-helper`) runs them in the game. The
companion stays independent of all of it.

A write is Pending through its native acknowledgement and an independent readback; Confirmed, NotApplied, NotKept and
Uncertain behave differently, and nothing retries a mutation. Uncertainty stops a draft batch and needs fresh
admission before further work. Conditional Undo checks the whole item, manager, process, catalog and connection.

The helper is in [its README](../trainer-helper/README.md), with what is still to build in
[BACKLOG.md](../trainer-helper/BACKLOG.md).

## Admission

Before any remote work, the host reads UnityPlayer.dll's fixed version resource and
refuses versions outside 6000.4 (also refusing missing/invalid resources). Hashing brackets
that check and binds the checked disk bytes to the target's loaded module. This selects
the supported engine family; it does not prove compatibility of every patch or the
external graph/lifetime assumptions below. Per-operation metadata checks still apply.

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

## Writes

Detached unique previews repeat bounds and live-alias checks at the byte-write site. Direct refresh lookup rejects virtual,
abstract and native replacements before mutation; read-side virtual getters resolve their implementation explicitly.

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

## Lookups

Optional UI queries return `Lookup::{Ready, Loading, Unavailable}`. Preview, value-table and
description readiness no longer compares display strings. Worker snapshot publication wakes
the Slint observer through watch::changed; a lightweight timer polls only while receipts or
preview debounce remain pending. Capture fixtures supply an event-loop proxy and pump those
wakes, so they exercise the same subscription path rather than relying on idle UI polling.
