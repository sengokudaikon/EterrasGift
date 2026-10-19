# Trainer control sessions

The trainer owns the offline gate, artifact profile, serialized control session and owned-command dispatcher.
`eg-game-access` owns protocol codecs, explicit process identity, the Windows pipe client and unsafe runtime access.
The desktop now composes one background worker and a command-based Slint forge adapter. The companion remains independent.

The desktop Forge editor now uses worker commands with typed Pending, Confirmed, NotApplied,
NotKept and Uncertain outcomes. Mixed drafts execute in order, re-read before each edit and stop
at the first non-confirmation. Undo reverses only confirmed edits. Known mismatch and uncertainty
clear drafts and Undo; uncertainty never retries automatically. Status, base and identity controls
use this path; actual-value mapping and remaining edit kinds stay unavailable. Run
`mise run capture:trainer` for the fake worker-to-Slint route and `cargo test -p eg-ui --lib`
for delayed editor tests. No actual game is used. See [trainer readiness](../../research/trainer-readiness.md).

Connect/Retry and monitoring use owned worker snapshots. Missing formatting/mappings stay
explicit; gate, identity and connection loss clear the visible item. Construction does no native
work. `--no-connect` skips automatic startup, and a later manual Connect starts reads. Live
settings/cache actions and the remaining shared feature/domain model cleanup are still open.

Source runs prepare the helper with `mise run trainer:helper:build` before connecting. That task publishes
`eg_trainer_helper.json` beside both debug/release apps, referencing the retained versioned DLL.
The connector opens and validates it only after Release admission. Missing/invalid packages show a
reason and Retry. Final bundle assembly remains separate. UI integration tests use fake connections
and FakeGameHost only; this is not a new live-game validation claim.

The current [native preparer](../trainer-host/README.md) uses a small unsafe supported-game boundary,
with callback-local thread validation instead of pre-bootstrap RuntimeAttestation. Historical checkpoint
notes below describe their original implementation. `mise run trainer:verify` runs the trainer tests and
workspace verification; native fixtures remain a separate gate.

The current write service accepts `Command::Write(WritePlan)` and `Session::write` for both
forge edits and field changes. `WriteOutcome<T>` represents Pending, Confirmed, NotApplied,
NotKept (a complete independent observation of the same target), or Uncertain. Native ACKs
remain provisional until the separate read completes under renewed admission. A missing,
malformed or replaced target cannot become NotKept. `BackendSnapshot::write_outcome(&plan)`
exposes the retained typed result independently of connection readiness; receipts report
command completion and diagnostics, while watch snapshots carry write certainty.

Ordinary pre-store refusals and NotKept preserve a healthy connection. NotKept publishes the
actual observation, advances field observation sequences and creates no Undo. Transport,
protocol, identity, restart or offline failures release the connection. Later publication
gate loss suppresses live data without downgrading an already independently confirmed
historical result. Nothing retries a mutation. Protocol32 carries all seven status fields and an
explicit override bit through operation 10. `ForgePlan`/`ForgeUndo` cover
level requirement, forging/legendary potential, Weaver's Will/Touch, corrupted and ruined;
`status_fields` advertises support separately from item eligibility. Rebuilding fields use the
game's RebuildID/UpdateEntry methods and accept its derived level requirement under the existing readback rules. A managed refresh failure after
the store is uncertain, with no automatic retry or implicit rollback. Unique eligibility calls
the game's metadata-checked getUnique(UInt16), with a rooted result and a fresh full stale check.
Host and helper share the small status rule function, using their own observed definition data.
Plan construction has one entry point, `ForgePlan::edit(..., override_enabled)`, and
revalidation always receives the current override setting. `edits::inspect_readback` is the
single readback policy for both pure and submitted plans. The direct level-cache store expects
an exact complete snapshot, including names and fingerprint, because it does not rebuild the
item. Rebuilding status and roll edits allow derived names and level to change; base/identity
edits also allow the game's derived class requirement. Their other guards remain unchanged.
The worker defaults override off, serializes `SetOverride` with writes, and checks the current
setting before submission and Undo. The desktop supplies the saved startup preference; live
settings actions remain separate. A new Undo is retained only
when a valid reverse plan can be prepared under the current game rules (an above-maximum Will
reduction may not be reversible); using the token rechecks current permission and observations.

Base changes (op 11) and unique/set identity changes (op 17) now use the same plans, submission,
certainty and Undo lifecycle. `forge_plan(ItemEdit)` admits these implemented edit kinds only;
`base_change` and `identity_change` advertise their availability. The helper independently calls
ItemList.get() or getUnique(UInt16) and checks the returned definitions, item eligibility and full
expected snapshot before the first store. It validates all destinations and refresh signatures
first, then calls AfterIDChange, optional MakeFullName for identity, RebuildID and UpdateEntry.
Game-derived names, level and class changes follow the existing edit readback policies; meaningful
collateral drift is NotKept. All failures after the first store remain uncertain, including partial
multi-field changes. Undo is a freshly validated reverse edit, not raw memory restoration. Idol
type changes and unrestricted identities recheck the current override before Apply and Undo.
The archived authored checkpoint
records owned Unity validation; actual-game writes remain unvalidated.

Affix roll batches (op 2, affix target only) now share this path too. `forge_plan(ItemEdit::Roll)`
accepts one to 32 distinct current slots and byte roll values; `affix_rolls` advertises support.
Host planning checks the captured compatibility/tier rules and current override. The helper
independently checks the whole expected item, every selected current definition/tier and every
exact mutable Byte destination before the first store, retaining the affixes across refresh/GC.
The helper stores only selected roll bytes, leaving affix reconstruction to the game. RebuildID
and UpdateEntry run once per batch. A reverse plan restores the selected original rolls, with
fresh stale/override checks. Native ACKs still need separate readback; game normalization is
NotKept and refresh failures remain uncertain. Unique/implicit roll targets remain refused until native mapping
queries exist.
The editor integration above uses these native operations; formatting, previews and mappings remain separate.


Historical per-feature reports are preserved at `archive/trainer-evaluations-2026-10-19`.
Current assumptions, test gates and archive location are in
[trainer-interop.md](../../research/trainer-interop.md).

Affix replacement, tier and seal changes (op 1, protocol33) use the same plan/outcome and editor
command path. `affix_change` advertises support. The native helper invokes the game's exact
ItemAffix(UInt16, Byte, Byte, Byte itemType, SealedAffixType) constructor, roots its result and
validates the resulting layout before changing the existing list through managed Clear/Add.
Seal flags and refresh signatures are prevalidated. Constructor normalization is observed through
independent readback; refresh/rewrite exceptions are uncertain. Undo reconstructs the prior affix
at its moved slot and rechecks override permission. The original strict-affix set-identity
readback omission is fixed. Actual-value mapping controls remain disabled.

Add/remove (ops 13/14, protocol34, `affix_list`) are connected through the same service and editor.
They reuse the managed list rewrite and seal flags, then invoke the game rarity method for items
without unique backing before RebuildID/UpdateEntry. All these signatures are checked before the
first mutation; any later exception is uncertain, with no automatic rollback or retry. Removal
checks the slot and ID against the full expected observation, can remove an unserializable ID,
and never offers to reconstruct such an ID. Conditional Undo otherwise follows current catalog
and override rules. Inserted duplicate IDs are identified by their resulting slot for Undo.
