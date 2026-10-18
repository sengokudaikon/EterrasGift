# Trainer control sessions

The trainer owns the offline gate, artifact profile, serialized control session and owned-command dispatcher.
`eg-game-access` owns protocol codecs, explicit process identity, the Windows pipe client and unsafe runtime access.
The app/UI do not use these services yet. The companion remains independent.

## Implemented subset

Protocol 23 requests are discovery (6), status (7), clearing all gameplay hooks (15 with count 0 and **no tail**),
the read-only forge snapshot (18), and literal item limits (19). The original C# app reads the snapshot externally;
the Rust extensions deliberately increment the handshake version, so protocol 21/22 peers are rejected. The separate
[Rust helper](../trainer-helper/README.md) implements the persistent server and runtime handlers.
Item/field writes, enabling gameplay features, full definitions capture and production injection are not implemented
here. `protocol::Request::decode` deliberately rejects other operations and nonzero reserved
fields in this subset. This is not a claim to implement every operation of the original protocol.

`BuildProfile::capture` reads local files and captures GameAssembly/metadata hashes plus the expected helper source
build ID. `Session::new` verifies the transport's PID/creation-time/executable identity, and binds those artifacts to
`GameAssembly.dll` and `<exe-stem>_Data/il2cpp_data/Metadata/global-metadata.dat` beside the selected executable.
No process is accessed by constructing a session. File hashing and validation are synchronous: compose this service
on the background worker, not the UI thread.

Sessions start disabled. `enable` permits an explicit `connect`; it does not discover a process or send anything.
Player.log is read against the target's exact process start time. Release admission requires a current log showing
offline play. An unusable log waits during 60 seconds of startup, then refuses. Development probing is an explicit
`GatePolicy::Development`, never an implicit release fallback. Discovery is followed by protocol/adapter/PID/build-ID,
all 37 required offsets, artifact hash and file-backed executable dispatcher validation. The log is read again before
the first Status, accounting for time elapsed during discovery. Status may install the legacy helper's scheduler.
Runtime status must independently report offline play and a nonzero local manager before the session becomes Ready.
The returned manager address is an ephemeral observation, never a root or durable managed-object token.

`read_forge` requires Ready, rechecks Player.log before and after installation-artifact verification, and rechecks the
log after receiving the complete reply. The helper independently requires runtime offline play and a live native
manager in the main-thread read itself. An absent crafting/main/entry/item is a successful `None`; malformed data
fails the complete read. The snapshot owns every string/vector and contains no managed root or memory borrow.
Its item address and individual ID are observations scoped to the selected process identity; they do not authorize
later dereferences or edits. The legacy fingerprint omits names, class requirement, special affix type and set-item
identity; full snapshot equality remains necessary for those fields.

`read_item_limits` shares those admission, artifact-validation, cancellation and failure rules. It returns
owned ranges for forging potential, legendary potential, Weaver's Will, Weaver's Touch and level requirement.
The helper reads exact Int32 metadata literals, rechecks runtime admission and rejects changed types/flags or
out-of-byte/reversed ranges. It does not invoke game methods or claim a localized catalog. The caller must retain
the process/build profile alongside these ranges; raw ranges have no standalone cache identity.

Read failure drops the cached attachment and blocks further reads until an explicit new handshake. Offline-required
revokes it as usual. A canceled read remains Connecting, with no cached runtime status, until explicit disconnect.
Neither a malformed response nor a timeout triggers replay. Rich modifier descriptions, definitions, mappings and
domain/UI composition remain separate from this raw snapshot.

Polling with `refresh` rechecks the log before transport. Log denial, an online/no-character status, or helper code 5
revokes Ready, discards the cached discovery and stops further client helper access. Cleanup becomes unconfirmed;
the native helper must independently remove gameplay features when offline play ends. This Rust slice cannot enable
any such feature. Missing-log refusal is intentionally stricter than the C# coordinator's conditional best-effort
cleanup of already-held features: this layer makes no further helper call after refusal.

Explicit `disconnect`/`disable`, while an attachment is still held, sends an empty wanted hook set with a 3-second
best-effort deadline. Only a valid reply reporting every returned hook off yields `Cleanup::ConfirmedOff`. Failure,
malformed reply or deadline yields Unconfirmed. A new attachment invalidates an earlier cleanup acknowledgement.
Neither disconnect nor dropping the session unloads the helper or removes its scheduler entry. The application must
await explicit cleanup; Drop does not promise asynchronous I/O.

Dropping a connection future leaves Connecting; dropping cleanup leaves Closing. `connect`/`refresh` refuse those
states and `enable` does not erase the barrier. Finish an explicit disconnect before attaching again. The interface
requires an active Tokio I/O/time runtime. It exposes no automatic mutation retry or reconnect loop.

## Transport and uncertainty

The pipe client connects only to `\\.\pipe\eterras-gift-<explicit PID>`, opens query/synchronize process rights and
checks creation time/path before and after exchanging data. It checks the pipe server PID before the first byte.
It does not load a helper or open game memory. Direct low-level client callers must establish offline admission;
the trainer composition should use Session, which also checks that its transport belongs to that exact target.

Each request has a fresh byte-mode connection: 15 seconds to connect, then a separate 15-second write/header/body
deadline. A connect timeout checks pipe presence three times 100 ms apart: present or listing failure means Busy;
absence means Missing. Both prove NotSent. Once a write is attempted, I/O failure, malformed framing or deadline
means MayHaveRun, including a possible partial write. Header size/code/magic and the 1 MiB payload bound are checked
before allocation. The connection is closed after the response. No request is resent automatically.

Helper code 1 is an uncertain failure. Codes 2–6 are definite no-operation failures under the legacy helper contract;
human text never determines the classification. For cancellation/deadline, the Rust server must not report NotReady
after dispatch started: canceling still-pending work can prove no execution, while running work owns its slot until it
finishes. Client disconnection cannot retract that work.

Protocol 22 retains the legacy framing and has no request ID, session nonce, game hash fields or authentication. Local binary/metadata hashes are
profile identity, not values received from the helper or a known-build whitelist. The source build ID and server PID
are compatibility/identity checks, not authentication against code already executing in that same process.

## Source and validation

The port follows original app commit `e1c1ed4`: `native/protocol.h`, `helper.cpp`, `dispatch.h`, and C# `NativeBridge`,
`RuntimeDiscovery`, `InjectionGate`, `GameAttachment` and `GameSessionCoordinator`. The code corrects the handoff's
possible ambiguity about an empty hooks tail: empty wanted means count 0, with zero bytes after the 336-byte frame.
Other deliberate differences are stricter malformed UTF-8/reserved-byte rejection, server-PID checking and always
requiring the expected helper source build ID, without the legacy DLL-hash fast path.

```powershell
mise run trainer:test
mise run trainer:test:release
mise run verify
```

Seven protocol, ten forge-codec and five item-limit codec tests cover independent wire examples and malformed/bounded input. Six pipe tests launch only
owned child test processes, checking fragmentation, fresh connections, Busy/Missing, PID reuse, wrong-server
rejection before sending, malformed/truncated replies and uncertain deadlines without replay. Session tests cover
admission, artifacts, helper identity, offline transitions, cleanup and future-cancellation barriers. The combined
test copies its own executable to TEMP and serves three requests there, using the real log reader, profile/session
and pipe client, then exits. Its PE/metadata/runtime replies are authored data; they do not discover IL2CPP fields or
validate gameplay effects. Ignored child entry points are invoked explicitly by their parent tests.

The persistent helper/bootstrap and read-only forge operation now pass the full authored Unity IL2CPP fixture,
including nested enums, Unicode, GC, replacement and malformed graphs. One explicitly authorized offline-game run
returns three identical plain-Hatchet snapshots under Release policy, then confirms cleanup, entry/unwind restoration
and the same live game process. Populated live affixes and unique/set data remain unvalidated on that game session;
their authored cases pass. See the [forge evidence index](../../tools/trainer-eval/persistent-player/forge-report.json).
The subsequent protocol 23 literal-ranges capability passes both authored Unity startup routes, including GC,
replacement and runtime refusals. It has not been run in the real game. The
[limits evidence index](../../tools/trainer-eval/persistent-player/limits-report.json) preserves exact build/source
identities and scope. Next: narrow boxed locale/enum invocation validation, localized definitions and descriptions,
then mappings and per-operation write/readback with independent effect cleanup before gameplay features or UI wiring.
The older frame-count probe remains separate.
Validation commands, counts and the retained log/source index are in [validation.json](validation.json).

Windows API contract: [GetNamedPipeServerProcessId](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeserverprocessid).
Tokio API usage follows the restored, pinned 1.53.3 source at `src/net/windows/named_pipe.rs`.
