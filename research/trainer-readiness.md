# Trainer shipping scope

The C# implementation at `../EterrasGift` remains the behavior reference. Feature Done/Todo status and implementation details are maintained in
[trainer-features.md](trainer-features.md). Runtime assumptions and retained evaluation conclusions are in
[trainer-interop.md](trainer-interop.md).

The architecture cleanup separates `eg-trainer-wire` (unsafe forbidden) from the optional
in-process discovery/scheduler build. The helper no longer pulls in the trainer worker. Host
artifact/process/platform adapters still live in game-access; the numeric-locale adapter was
moved there from core. Mapping and previews share catalog-version admission; malformed returned
mapping provenance revokes the connection, and both write families share fatal-error handling.
UI optional data is typed, and worker snapshots wake the UI through a watch subscription.

Remaining cleanup: operation-local checked field and managed collection/string views, bounded
method queries, feature-table/presentation-model duplication, editor file boundaries, shared
platform utilities, codec/metric normalization and capture/test-fixture organization. Thread
tokens must describe only checked dispatcher identity; they cannot certify exclusive access
to game-owned objects. Field flag masks must retain different rules for literals, static fields
and mutable instance fields. A generic IL2CPP crate is not a prerequisite for those extractions.

| Area | Implemented | Remaining |
|---|---|---|
| Desktop composition | Worker thread, manual/automatic connection, helper package validation; trainer startup failure leaves Home/Mods running | Final bundle assembly and restart handling |
| Bootstrap | DLL load/start/connect, callback-local native Unity main-thread check; process/log/artifact gates and pending-call retention | Actual-game validation of the corrected callback path |
| Reads | Owned rooted forge snapshots, literal limits, completed locale, catalog subset and names/cache; game-built implicit/unique descriptions, private-copy unique previews and sampled regular-affix values | Unique/implicit mapping; getter replacements must match readiness behavior |
| Forge writes | One EditForge request/ForgePlan/WriteOutcome path: seven status fields, base/subtype, unique/set identity, regular affix-roll batches, unique/set/implicit roll batches (2026-10-28), replacement/tier/seal and add/remove (a unique that gains an affix becomes legendary) | Live confirmation of the 2026-10-28 additions |
| Editor | Ordered single-edit drafts, fresh reads, Pending/Confirmed/refusal/NotKept/Uncertain, conditional reverse Undo; status/base/identity, replacement/tier/seal, add/remove and regular-affix Value controls connected; unique text and asynchronous preview adapter; unique/set/implicit value tables (ModifierMappingQuery) and rolls | Live confirmation of the unique/implicit sampler |
| Other features | Independent persistent attack-rate/leech-rate controls; mysterious rift, offhand, No mana cost and combined incoming/outgoing damage with shared retained gateway lifecycle; shard/rune/glyph replenishment controls; 17 patch-only controls over 27 ranges (see [feature checklist](trainer-features.md)), profile/UI controls, independent patch/flag readback, helper-side suspension and restore before unload | Other six field sets and remaining 42 hook features; live Last Epoch validation of the damage, rate, mana, material and interior-patch controls |
| Hook backend | Stock detour 0.10, retained owners, transaction recovery and registered trampoline unwind tables replace the custom scheduler; authored Unity throwing-task, partial-failure and owner-exit restoration pass | Gameplay-specific ABI/target coverage; accepted best-effort thread suspension remains a limitation |

Ordinary refusal stops the draft and keeps Undo for confirmed steps. Known mismatch clears drafts
and Undo, keeps confirmed history and may keep the connection. Uncertainty clears drafts and
Undo, preserves confirmed history, never retries the write and requires explicit recovery. A confirmed native ACK still needs
independent readback. A completed confirmation is not downgraded by later connection loss.

`ForgePlan::edit(..., override_enabled)` is the only plan constructor. Its explicit recheck takes
the current override setting. Pure and submitted plans share the readback policy documented in
the interop notes. The former runtime-thread reader, native display parser/provider/context and
pre-bootstrap attestation/nomination layers are removed. No new foundation is needed for the
next unsupported Forge operation: port its native/service/UI behavior together and retain tests
for observable correctness rather than implementation structure.

Required gate: `mise run trainer:verify` on pinned `nightly-2026-10-17` (Rust 1.101.0-nightly).
It runs release trainer/tier/controller tests, UI unit tests, Clippy, format/token checks and
fake-host captures. Inspect the resulting renders. `mise run capture:trainer` is the focused
worker-to-Slint route, including mixed drafts, delayed completion, partial refusal, Undo,
NotKept, Uncertain, Retry and gate loss. Native fixture execution is separate and uses only owned
children. No current cleanup check accesses Last Epoch.

Next: sampled unique/implicit values and their roll operations one at a time
through native, service, UI and captures. Consolidate the remaining presentation/domain and
feature-table duplication as those areas are touched. The hook backend is available for feature-family ports.
Historical reports and ignored artifacts are archived as described in the interop notes;
maintain tests and fixture sources instead of adding per-feature report JSON.

Affix replacement uses the game constructor, retains old/new roots, validates sealed layout and
rewrites through managed List<ItemAffix>.Clear/Add before refreshing. The helper checks every
seal-flag destination and refresh signature first. Constructor refusal/exception leaves the list
untouched; any failure after Clear is uncertain and is never automatically rolled back/retried.
Strict readback now includes set identity, fixing the original overload's omission. Conditional
Undo follows the replaced affix's moved slot; stable load grouping can change relative order
when a seal is removed. Actual game mutation remains untested and is not part of this gate.

The production replacement picker delegates class, category and idol compatibility to the trainer
domain against captured metadata and projected earlier draft steps. Presentation fixtures retain
their local rules; the worker still rechecks the fresh complete item before every edit.

Add/remove use the same conditional request and retained list roots as replacement. Rarity is
recomputed by the game's setRarityFromAffixesForNormalMagicOrRareItem method before rebuilding;
unique-backed items retain their rarity. Undo removes the actual inserted slot, including
explicitly allowed duplicate IDs, or reconstructs a removed affix under current definitions and
override permission. A removed ID outside the 12-bit save range cannot be reconstructed and does
not offer Undo. Add/remove remain immediate writes and wait for drafts to be applied/discarded.

Implicit descriptions now call the game's get_Implicits, GetItemImplicit and tooltip builder,
then virtual ToModifierString(false)/ToRangeString(false). The conditional read compares the
entire expected Forge item and manager before and after formatting; all intermediate objects
remain rooted across GC. Fixed lines beyond roll storage remain visible. Formatting refusal
clears optional descriptions while preserving the admitted session; malformed protocol data,
transport loss and offline failure still revoke it. These are read-only descriptions, not
sampled implicit/unique value tables: those roll writes and actual-value controls remain unavailable.

Tooltip text and formatting availability are presentation, matching the original StateEquals
rules. Their arrival/loss does not invalidate ordered drafts or confirmed Undo. The UI adapter
still compares the complete native ExpectedObservation before each write, including fields
absent from its presentation model. Unit and worker-to-Slint tests cover this distinction.


Regular-affix values now sample all 256 roll bytes through the game's tooltip producer and virtual
formatter on a detached deserialized item. Native code rejects shared lists/affix objects, clones
roll arrays, preserves set-count context, and compares the selected item/manager and locale before
and after each page. Host parsing uses the captured locale's standard numeric symbols; raw numeric
quantities are never substituted for display values. Tables are bound to the full observation and
draft, cached in the serialized worker, and shown/edited by the real Forge page. Fixed or
nonmonotonic mappings stay unavailable without a polling loop. Transient formatting failures can
retry after a successful read, while explicit Refresh retries stable refusals too. Mapping does
not create write history or discard confirmed Undo. The worker rechecks locale before cache
reuse and during monitoring, recapturing localized definitions and invalidating old formatting
on a language change. Disposable draft mapping reads stop between completed pages; ordinary
write receipts never cancel accepted writes. Reloading keeps the prior value visible with
editing disabled until the replacement table arrives.

The owned Unity route checks 256 samples against independent managed display/value oracles in
en-US, de-DE and fr-FR, for both existing and replacement/tier draft contexts. It also checks
aliasing, copy mismatch, producer/refresh exceptions, stale items, fixed/nonmonotonic tables and
unchanged live roll/write history. UI tests/captures cover loading, displayed values, roll Apply/
Undo, draft contexts, stale completions and recovery. Last Epoch was not accessed; actual-game
catalog/formatting behavior still needs separately requested validation.


Unique descriptions now use UniqueList.getUnique and the game's per-entry tooltip builder,
including virtual text/range dispatch and parsed replacement roll associations. Shared IDs are
deduplicated, fixed lines remain visible and invalid IDs make only their line read-only. The
worker loads text on demand and retains it for an unchanged full observation; optional refusal
is shown locally and explicit Refresh permits recovery. Loading is informational, not a warning
or a claim that the item has zero modifiers.

Unique preview validates the exact draft against current tooltip evidence, copies the item and
its mutable roll storage, checks that the copy initially formats like the live item, then changes
only the copy. Every changed index must be backed by an editable tooltip line. The helper checks
the complete original item/manager and locale before publication; the service/worker bind the
result to the original observation and exact draft. The editor adapter coalesces drafts for
150 ms, ignores results for older drafts and clears unsent work when a draft is discarded.
Accepted reads still drain. Preview never creates write history or Undo and never enables a
native write. Unique value mappings/writes are still unsupported, so production unique value
controls remain disabled; this slice exposes real descriptions and supplies the preview path
for the next roll feature.

Twelve owned Unity cases cover independent managed text oracles, shared/duplicate/invalid parsed
roll IDs, skipped/fixed/empty lines, GC during getters and virtual calls, stale items, malformed
text, alias/copy mismatch and preview-only exceptions. Independent post-preview observations
confirm unchanged live unique values. Worker tests cover cache/refusal recovery and cleanup
barriers; editor tests cover Pending/current-text/discard behavior, and captures cover real-worker
unique text/loading/unavailable states at normal and narrow widths. No live game was accessed.

Trainer fault reporting now persists Forge and field NotKept/Uncertain outcomes directly
from the worker, before publication barriers, including writes drained during shutdown.
Reports include the expected state, canonical edit, readback or its absence, differences
and recovery policy. Advisory broadcast events are not the delivery mechanism. Connection
and catalog-cache faults are logged too; retryable connection waits do not emit a file on each
retry. Reporting remains best effort.

The desktop policy follows `spec/trainer.md` §7.3: typed operational failures keep the
app running; an unexpected Trainer worker panic is logged, closes the event loop and
produces exit code 1 after shutdown, matching Mods and Settings. An optional Trainer
startup failure still leaves Home and Mods available. Authored tests cover fault files
without event subscribers, queued cleanup and fatal worker notification.

The Forge UI now re-exports the canonical trainer ItemEdit, RollTarget, SealState and
AffixKind. Its presentation carries the complete owned observation and captured catalog;
draft validation/projection, compatibility and reversibility delegate to the trainer.
The former presentation-to-domain edit conversion and simplified fixture rules are gone.
Authored sessions use the same planner, including current override permission for writes.

Applied Undo is owned by the worker's confirmed batch history and submitted as UndoForge.
The editor keeps draft ordering and the number of confirmed steps to restore. Each step
gets a fresh read; refusal retains the remaining Undo, while NotKept/Uncertain clears it.
A new Apply replaces prior history only on its first confirmation. Rebuilt names and a
bijective affix reorder may rebase the previous conditional token; every saved affix value
and other item field must still match. Foreign changes invalidate the remaining history.
Tests cover mixed batches, partial refusal, consumed tokens, slot renumbering, class rules,
override revocation and known/unknown outcomes without accessing Last Epoch.

Helper lifecycle (2026-10-22): owner-bound Unload, raw control-thread exit, retained external
callback gateway and complete drain, native module/pristine-entry proof, fresh reload after
Gone, permanent restart fence after Stuck, and backend/menu/shutdown triggers are implemented.
The cached 250 ms game watchdog distinguishes ClientStateManager Login/CharacterSelect from
InGame, preserving zone travel; unknown state refuses work and expires after three seconds.
Plain native 100-cycle reload and blocked-callback retention passed. The external Unity
6000.4.8f1 lifecycle run passed six fresh loads and module/pristine-entry checks after menu,
login, Disconnect, unknown-state timeout and online departure. Four seconds of character
absence during InGame preserved the session with typed CharacterLoading and fresh recovery.
Live Last Epoch testing remains explicitly deferred.
