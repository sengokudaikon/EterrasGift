# Mods port — 2026-10-18

Implemented in the `mods-port` worktree beside the main checkout. The source of truth is the C# app's
`ModLibrary`, `ModInventory`, `ModsViewModel`, `ModEntry`, `ModsView`, and their tests, with the original
`.codex/log/decisions.md` and dated notes used for intent. No live game is needed for this slice.

## Boundaries and persistence

`eg-companion::mods` owns the versioned DLL library, schema-1 profile and placement records, MelonPreferences,
managed metadata inspection, loader diagnostics/repair, log interpretation, and local source-project builds.
It depends on neither trainer nor game access. Inspecting an assembly never loads or executes it.

The UI keeps its current inventory and unfinished text edits in memory. It submits library work directly to a
single admission queue and polls completion receipts; it never waits for file IO. Game preparation uses that
same queue, so Play cannot overtake an already accepted edit. Source discovery and compiler processes run
outside the UI thread. Compiler output is bounded and both output streams are drained; cancellation terminates
the process tree. Closing drains accepted library mutations and surfaces a newly failed save before hiding.

The app library stores DLL versions by content hash. Add, on/off, remove, undo, and settings edits change that
library. The next offline-with-mods start prepares the game folder and preferences. Ordinary offline and online
starts retain the existing proxy-isolation behavior. Manual additions/rebuilds/deletions in Mods are reconciled
without overwriting unowned DLLs. Preparation records partial placement on failure so the next read knows what
actually happened.

Metadata uses pinned `dotscope` 0.9.1; TOML syntax preservation uses pinned `toml_edit` 0.25.17. The fake managed
fixtures retain their C# source alongside embedded binaries; Cargo tests do not need an installed .NET SDK.

## Source decisions that override older notes

- Remove takes a mod out of the profile and offers Undo. Its game DLL and old library copies are removed during
  preparation, rather than immediately sent to the recycle bin.
- Revert clears only settings exposed by the current inventory. Unknown/unclaimed settings remain until normal
  preparation validates or drops them.
- Update compares the source project's evaluated version with the installed assembly's informational/project
  version. MelonInfo's display version is not a substitute.
- Metadata distinguishes an unresolved base type from an unknown mod kind; the older spec omitted this case.

Deliberate hardening: malformed profile paths and all-dot DLL stems cannot escape the library. Housekeeping
does not traverse directory links/reparse points. CoreModule repair preserves metadata identities, verifies its
result, and backs up the original; unsupported orphan/cyclic nesting or signed input is refused before mutation,
instead of risking Cecil's lossy behavior for malformed assemblies.

DLL copies are hashed again in their temporary destination before publication, both when storing a library version
and when preparing the fake or real game folder. This prevents an external rebuild between hashing and copying from
storing bytes under the wrong content hash. Existing library copies are checked instead of trusted by filename; a
matching source can repair corruption. A failed verification leaves the previous destination and records intact.

## UI scope and validation

The Mods surface follows the original list/details/action-bar layout and its existing tokens, including narrow
and wide layouts, settings descriptions and groups, pending-change cues, source projects, and last-game logs.
The requested Impeccable pass is limited to this surface. Shared changes are only integration APIs needed for
save-and-close coordination; no broader visual corrections are part of the port.

Validation uses throwaway files, synthetic assemblies, fake compiler runners, and `FakeGameHost`. Live launch,
injection, and bridge testing remain separate, explicitly requested work.

The workspace test run passes 164 unit/integration tests, one compile-fail documentation test, and the headless
capture/input suite. The latter drives Space, Delete/Undo, Revert, context menus, numeric/text commits, choice
selection, and fake source build/install. A deliberately blocked library worker verifies that input and navigation
continue and the latest setting reaches disk; queued Delete followed by Refresh retains its Undo receipt.

The bounded Impeccable pass compared the original captures with populated, narrow, wide, high-contrast, and error
states. It corrected local divider/icon placement, wrapping in the footer, checkbox geometry, and keyboard focus
after the final row is removed. Generated design tokens and unrelated pages were not changed by this port.
