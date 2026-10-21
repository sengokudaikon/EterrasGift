# Trainer service

`eg-trainer` owns offline admission, game catalogs, edit planning, conditional Undo and the
serialized background worker. `eg-trainer-wire` owns the bounded messages and codecs;
`eg-game-access` owns platform adapters and runtime access. The companion remains independent.

The backend reads Player.log before accessing a selected process and before publishing results.
Preparation opens read-only leases on the game assembly and metadata, hashes each once, and
shares the assembly lease with native preparation. A connected session keeps those leases;
requests validate file identities without reading their contents again. Platforms or share modes
that cannot provide leases use explicit full-content validation. Native module leases are
released after their retained process handle signals exit, including while monitoring is off.

The session transports requests and decodes owned responses. Its lifecycle prevents reusing an
abandoned exchange. It does not repeat Player.log admission. The helper checks the current
offline graph on Unity's main thread before reading or writing it.

Requests have a 12-byte signature/operation/body-length header and sequential bounded bodies.
The helper build ID identifies compatible code. Discovery returns the dispatcher and build ID;
field offsets and separate protocol/adapter versions are not transmitted.

The desktop Forge editor submits ordered mixed drafts. Supported edits include status fields,
base and unique identity, affix rolls, replacement/tier/seal and add/remove. Regular affix values
come from game tooltip sampling on detached copies. Implicit and unique descriptions and private
unique previews are connected; unique/implicit value mapping and roll writes remain unavailable.
Attack/leech rates have typed read and conditional write commands. Gameplay hooks remain unavailable.

A write remains Pending through its native acknowledgement and independent readback. Confirmed,
NotApplied, NotKept and Uncertain have distinct behavior. Nothing retries a mutation automatically.
Uncertainty stops a draft batch and preserves its earlier confirmed history in the editor; fresh
admission and observation are required before further work. Conditional Undo checks the complete
item, manager, process, catalog and connection generation.

Feature descriptions come from one shared registry. UI row presentation does not claim native
implementation; connected pages obtain availability from the backend. Settings changes reach the
worker immediately, and their persistence is handled separately.

Run `mise run trainer:verify` for release trainer/controller tests, UI tests and workspace
verification. Captures and product tests use fixtures and fake hosts. Native fixture execution
is separate; the real game is tested only when requested.

See [current scope](../../research/trainer-readiness.md),
[runtime assumptions](../../research/trainer-interop.md), and
[helper implementation](../trainer-helper/README.md).
