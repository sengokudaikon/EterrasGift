# Trainer control sessions

The trainer owns the offline gate, artifact profile, serialized control session and owned-command dispatcher.
`eg-game-access` owns protocol codecs, explicit process identity, the Windows pipe client and unsafe runtime access.
The app/UI do not use these services yet. The companion remains independent.

## Implemented subset

Protocol26 requests are discovery (6), status (7), clearing all gameplay hooks (15 with count 0 and **no tail**),
the forge snapshot (18), literal item limits (19), completed locale/version capture (20), catalog pages (21),
and one explicit level-requirement write with complete expected-state tail (22).
The Rust extensions deliberately increment the handshake version, so older peers are rejected. The separate
[Rust helper](../trainer-helper/README.md) implements the persistent server and runtime handlers.
Other item/field writes, enabling gameplay features, complete localized formatting and production Last Epoch authority are not implemented
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

`read_locale` shares those session rules. It returns owned language and application-version strings, preserving their
case and text with no fabricated locale. The helper requires an already completed, current selected-locale operation
and live Settings/Locale native objects. It never calls settings initialization, locale-selection or waiting methods.
Only the audited identifier/code/version getters execute; the latter permits framework initialization and allocation
effects. Unready data and failures revoke the session and require explicit reconnection, without automatic replay.
The payload is two LE-u32-length strict UTF-8 texts, capped at 4096 bytes each. Language must be nonblank; an empty
application version remains valid as in the original identity model. This is not full definitions capture.

`read_catalog_page` and `capture_catalog` copy already-loaded base/subtype/implicit definitions, affix identity and
restriction/tier metadata, and unique/set identity. Pages carry kind, cursor, total and locale/version, with at most
16 entries and a 1 MiB payload. The complete capture pins locale and each population, rejects duplicate identities
across pages, and checks locale again before publishing anything. A rejected page poisons the assembler; session
failure requires an explicit reconnect. Immutable definition assets remain a runtime contract: pinned counts do not
make this an atomic snapshot. Each request currently rehashes game artifacts; large captures need performance work.

Definition names retain their source (stored display, internal name, or runtime display). Generated affix names,
rarity names and implicit endpoint formatting are explicitly unavailable in this adapter. Ordinary property numeric
ranges, rounding and display flags come from already-loaded PropertyList metadata, including ordered first-match
selection/default and rounding overrides. Five special registry-backed SP identities remain explicitly unavailable;
no property factory/getter or linear displayed-value interpolation is invoked. Raw implicit ranges remain available.
`GameCatalog::resolve_item_name` preserves the raw forge observation and
returns separate provenance: the runtime cache first, then a matching unique/set or base definition. A base fallback
does not reproduce a full affixed name. The native full-name builder also writes lore and is deliberately not invoked.

`CatalogCache` stores fully revalidated canonical pages with identity and SHA-256 integrity, a 64 MiB total bound,
atomic catalog/last-key writes and three-build retention. Missing files are cache misses; corruption is explicit.
Cache identity includes binary/metadata hashes, language, application version and extraction version. Saved data is
never proof of current runtime verification or permission to edit. See [catalog evidence](../../tools/trainer-eval/persistent-player/catalog-report.json).
Extraction version 2 adds the ordinary numeric route; [numeric evidence](../../tools/trainer-eval/persistent-player/numeric-report.json)
records its authored coverage and special-property/formatting limitations.

`backend::service` supplies a serialized background worker, bounded command queue, watch snapshots and typed events.
Construction does no I/O or spawning. Enable/monitor/connect are explicit, connection phase is separate from read
activity, and initial catalog/forge results publish together under a generation. Accepted cleanup barriers suppress
old results and drain in-flight operations; receipt cancellation never aborts a submitted operation. Saved catalogs
remain unverified until fresh capture. Writes, feature hooks and field editors stay explicit false capabilities.

`SessionConnector` repeats the exact process-bound offline gate around an injected helper preparer and the session
handshake. Independent log cursors, immutable profile/helper identity and transport binding are checked before pipe
use. Loaded or uncertain preparation failures retain MayHaveRun and never retry automatically. This is the safe
orchestration boundary; an app-owned Rust remote preparer and UI composition are still separate work.

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
replacement and runtime refusals. It also passes the later protocol 24 live offline capture. The
[limits evidence index](../../tools/trainer-eval/persistent-player/limits-report.json) preserves exact build/source
identities and scope. Next: full localized definitions, resolved item names and descriptions,
then mappings and per-operation write/readback with independent effect cleanup before gameplay features or UI wiring.
The older frame-count probe remains separate.
The opt-in boxed-return/Int32-enum invocation contracts now pass authored standalone IL2CPP execution on the matching
runtime with GC on/off. See the [value-type evidence](../../tools/trainer-eval/unity-rust-host/value-types-report.json).
The subsequent [locale capture checkpoint](../../tools/trainer-eval/persistent-player/locale-report.json) audits the
actual identifier layout and getter bodies, then validates protocol 24 capture through both authored Unity startup
routes. It observes completed selection fields to avoid the legacy getter's blocking/initialization behavior.
The restarted offline game now passes Release-gated capture: language `en`, game version `1.5.2.2`, exact item limits
and three identical plain-item snapshots. Independent retained counters confirm seven commands on Unity main TID
51628, eight accepted requests and 331 original normal returns. Entry/unwind restoration, stopped server, acknowledged
empty-feature cleanup and the same responsive process are confirmed. One helper generation remains pinned.
The raw `_fullName` cache is empty in this process; the C# reader also accepts it without name enrichment. This does
not yet provide a guaranteed display name. Locale transitions and populated live affixes/unique/set cases remain
separate validation, as do full localized definitions and mappings.
Validation commands, counts and the retained log/source index are in [validation.json](validation.json).

`writes::LevelRequirementPlan` is the first submitted edit plan. It requires a current runtime catalog,
complete observation, process/generation and manager identity. `Session::set_level_requirement` sends one explicit
protocol26 request with the entire expected snapshot, then requires the exact projected snapshot in response.
It uses Release admission even in a development session. Errors before transport starts remain local; malformed
or mismatched acknowledgements, potential delivery and lost confirmation provenance remain Uncertain. An abandoned
future retains Pending and a busy session until explicit cleanup records uncertainty. No path retries a write.

The background service accepts `ApplyLevelRequirement` explicitly. It rechecks generation/full observation and
fresh runtime status before submission, drains writes through queued cleanup, and retains the outcome with its
original plan. Only its separate `level_requirement` capability is available on supported ready connections;
generic `write_forge`, hooks and fields remain false. Readiness is revoked on failed pre-write refresh. Single
operation support is not the complete Apply/draft/batch/Undo workflow, and app/UI composition remains separate.

The [standalone parser proof](../../tools/trainer-eval/unity-rust-host/display-parser-report.json) validates the
exact BCL `Double.TryParse(String, NumberStyles, IFormatProvider, out Double)` overload with style231, captured
culture and exact unit suffix. Stable/nightly and incremental GC off/on pass. Unicode minus and host locale
normalization are not added. Unity's own culture parent fallback is retained explicitly. This proof enables no
native formatting/mapping opcode and does not prove the target game's BCL binary equivalent.

The [Rust bootstrap checkpoint](../../tools/trainer-eval/persistent-player/bootstrap-report.json) passes plugin,
Python and Rust startup against the same authored Unity 6000.4.8f1 child build. Rust checks full pristine dispatcher
body/table/unwind spans before Status and uses remote-memory-only polling after its one-shot stop. Pending remote
calls retain input and handles when completion is uncertain. App-specific unsafe admission/preparer composition
still needs implementation; an artifact hash and named thread alone do not establish runtime safety.

`sources` supplies an inert absolute-path log factory and a watch-channel process locator so composition can use the
existing watcher without coupling trainer and companion. `edits` supplies pure owned planning for seven edit kinds
and detailed readback diagnostics. These models do not send writes or prove a native mutation safe. Catalog capture
uses short-lived Windows read leases when available; public individual reads and unsupported lease capabilities
retain full artifact revalidation.

The [special numeric checkpoint](../../tools/trainer-eval/persistent-player/special-numeric-report.json) adds field-only
Ability, Player, Tracker and IdolAltar selection with extraction version 3. SP43 remains unavailable until ready
ailment-dictionary semantics are proven. All three authored startup routes pass 333 requests and 292 main-thread
commands. `capture_catalog_observed` reports page/byte progress and returns a distinct Stopped outcome between
completed RPCs; it keeps Ready/discovery for explicit cleanup and publishes no partial catalog. Backend cleanup
intent suppresses stale progress/data and drains the current response before stopping. The foundation now passes
280 tests on stable and nightly release, plus ownership and architecture checks.

The [native preparer checkpoint](../../tools/trainer-eval/persistent-player/native-preparer-report.json) adds
`eg-trainer-host`, keeping unsafe admission outside this crate. Its concrete `HelperPreparer` retains native
generations and uncertain calls, proves pristine dispatcher code through its returned transport, and reuses a
loaded helper after session disconnect. The fourth authored route verifies three preparations with one runtime
attestation and one retained/verified target. Production game authority remains separate work.

`mapping` provides pure observation-bound tooltip/preview models and a poisoned builder for all 256 actual samples.
It requires exact context/language/unit/draft equality and an explicit display parser. Raw game quantities stay
diagnostic because their scale can differ from displayed values. No mapping/preview opcode or native formatter is
enabled. The combined foundation passes 317 tests on stable and nightly release; 25 cover these pure mapping models.

The [ailment checkpoint](../../tools/trainer-eval/persistent-player/ailment-report.json) completes all five special
numeric selectors with extraction version 4. SP43 reads the already-initialized dictionary, validates full bucket/
free-chain/hash/key/version coherence and roots values copied through opaque Entry boxes. Found nulls do not fall
back; only true misses select the first ready source entry. Custom comparers and destroyed native objects are
unavailable. All four authored startup routes pass 440 requests and 392 main-thread commands, with 323 foundation
tests on stable and nightly release. Numeric capture now permits audited framework reflection/boxing/allocation
and GC/profiler effects; dictionary/gameplay getters, fills and writes remain excluded.

Windows API contract: [GetNamedPipeServerProcessId](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeserverprocessid).
Tokio API usage follows the restored, pinned 1.53.3 source at `src/net/windows/named_pipe.rs`.
