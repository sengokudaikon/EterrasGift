# Trainer shipping scope

The C# implementation at `../EterrasGift` remains the behavior reference. Current architecture,
interop assumptions and retained evaluation conclusions are in [trainer-interop.md](trainer-interop.md).

| Area | Implemented | Remaining |
|---|---|---|
| Desktop composition | Worker thread, manual/automatic connection, helper package validation; trainer startup failure leaves Home/Mods running | Final bundle assembly and restart handling |
| Bootstrap | DLL load/start/connect, callback-local managed main-ID check; process/log/artifact gates and pending-call retention | Actual-game validation of the simplified callback path |
| Reads | Owned rooted forge snapshots, literal limits, completed locale, catalog subset and names/cache | Formatted descriptions, previews, mapping; getter replacements must match readiness behavior |
| Forge writes | One EditForge request/ForgePlan/WriteOutcome path: seven status fields, base/subtype, unique/set identity and regular affix-roll batches | Replace/tier/seal, add/remove, unique/implicit rolls |
| Editor | Ordered single-edit drafts, fresh reads, Pending/Confirmed/refusal/NotKept/Uncertain, conditional reverse Undo; status/base/identity controls connected | Actual-value mapping; unsupported controls stay disabled |
| Other features | Typed attack/leech observations and one conditional rate primitive; original feature descriptors/inventories | UI field integration, other six field sets, all gameplay detours/patches and offline/owner-loss effect cleanup |
| Hook backend | Existing scheduler and byte-patch suspension code retained; independent candidate regressions | Finish the general backend trial before gameplay hooks |

Ordinary refusal stops the draft and keeps Undo for confirmed steps. Known mismatch clears drafts
and Undo, keeps confirmed history and may keep the connection. Uncertainty clears drafts/history/
Undo, never retries the write and requires explicit recovery. A confirmed native ACK still needs
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

Next: complete affix replacement, add/remove and unique/implicit roll operations one at a time
through native, service, UI and captures. Consolidate the remaining presentation/domain and
feature-table duplication as those areas are touched. Keep the hook-engine trial separate.
Historical reports and ignored artifacts are archived as described in the interop notes;
maintain tests and fixture sources instead of adding per-feature report JSON.
