# Trainer service

`eg-trainer` owns offline admission (`connect`), the game catalog (`definitions`), the feature catalog and profiles
(`features`), Forge edit planning and conditional Undo (`item`), and the serialized background worker (`backend`).
`eg-trainer-wire` owns the bounded messages and codecs; the helper (`eg-trainer-helper`) runs them in the game. The
companion stays independent of all of it.

A write is Pending through its native acknowledgement and an independent readback; Confirmed, NotApplied, NotKept and
Uncertain behave differently, and nothing retries a mutation. Uncertainty stops a draft batch and needs fresh
admission before further work. Conditional Undo checks the whole item, manager, process, catalog and connection.

Runtime assumptions are in [trainer-interop.md](../../research/trainer-interop.md), the helper in
[its README](../trainer-helper/README.md), and what is still to build in
[trainer-features.md](../../research/trainer-features.md).
