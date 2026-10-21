# Native trainer composition

`eg-trainer-host` supplies the Windows x64 `HelperPreparer` used by
`eg-trainer::connection::SessionConnector`. Construction is inert. Explicit
preparation runs on the trainer worker, after the process-bound Player.log gate.

`desktop::GameProcesses` supplies exact query-only process identities and retains
all matching instances so the worker can reject ambiguity. The desktop embeds the
helper it was built with (an artifact dependency of `crates/app`); `HelperPackage`
knows it by SHA-256 and the source ID compiled into it. After Release admission,
`DesktopConnector` writes it to `eg_trainer_helper-<SHA-256 prefix>.dll` under the
app's local folder (reusing an identical file a game may still hold), then retains
the same NativePreparer/SessionConnector across reconnections. Build/evaluation
reports and their `passed` flags do not participate in runtime admission. A file
that cannot be written blocks with an actionable reason and explicit Retry.
The desktop keeps the unsafe supported-game assumptions at composition; these
types do not certify a runtime based on a filename or manifest.

`NativePreparer` loads the matching helper, starts it outside loader lock and
returns a verified named-pipe transport. It checks the process birth/path, local
artifact identity and helper source ID. No runtime authority object, main-thread
nomination or pre-bootstrap thread probe is required.

The constructor is the small unsafe, game-specific boundary: targets must be the
supported initialized Unity6000.4 Windows x64 game or an owned matching fixture.
Runtime/module lifetime and absence of competing scheduler patchers are supported
execution assumptions. File hashes do not establish those external facts.

The helper checks its execution thread inside the verified ExecuteTasks callback:
the current UnitySynchronizationContext's managed main ID must equal the current
managed thread ID. Only then is the callback's Windows ID retained for dispatch.
The original ExecuteTasks still runs first; worker calls cannot claim the queue.

Preparation retains a registry keyed by process birth/path and artifact profile.
It reserves an entry before native startup, reuses a loaded helper and fences any
unfinished/uncertain attempt. Factual NotSent failures release the reservation.
`RemoteFailure`/`PendingCall` retain unfinished calls and their input allocations;
no unrelated host-side ownership journal or eight-target capacity limit remains.
Dropping preparation submits no stop/unload and cannot release an unfinished
remote call's storage. Published helper/trampoline code remains resident until
process exit. Host-side registry pruning after confirmed exit remains future work.

`VerifiedTransport` repeats Release offline admission and checks the complete
file-derived dispatcher body/table/unwind before the first Status can install the
scheduler. Later requests must carry that same dispatcher. Disconnect clears
features but does not unload the helper or authorize another load.

Run `mise run trainer:verify` for the trainer product suite followed by workspace
lint, formatting, token checks and fake-host captures. The persistent-player
controller exercises the complete owned `--rust-preparer-worker` route. No test
selects a running Last Epoch process. The pre-refactor source and tests remain at
Git tag `archive/trainer-evaluations-2026-10-19`; [interop notes](../../research/trainer-interop.md) record current assumptions and the local archive.
