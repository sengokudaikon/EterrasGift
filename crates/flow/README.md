# eg-flow

One-way UI state. `eg-flow` has no dependencies and names no app, game or toolkit. `eg-flow-slint` adds what a Slint host
needs. `crates/app/tests/architecture.rs` holds both to the Library tier, so neither can reach the app.

| Primitive | Crate | What it is for |
|---|---|---|
| `Feature` | `eg-flow` | The pure half: `reduce(state, intent, effects)` |
| `Host` | `eg-flow` | The impure half: `render` the state, `perform` the effects |
| `Store` | `eg-flow` | Runs the two on the UI thread and queues intents sent while it works |
| `Tracked`, `Slice` | `eg-flow` | Redo only the parts of a render whose inputs changed |
| `CheckedSlice` | `eg-flow` | A `Slice` that fails a test build when its key misses an input |
| `Subscriptions` | `eg-flow` | Timers and watchers that run for as long as the state wants them |
| `Effects::scoped` | `eg-flow` | A child scope's reducer inside its parent's |
| `step` | `eg-flow` | One reducer step in a test |
| `Background` | `eg-flow-slint` | Work off the UI thread, on an `eg_runtime` scope, that comes back as an intent, with panics as values |
| `local` | `eg-flow-slint` | Waiting on the UI thread for work that can be let go of, even from what its own result causes |
| `on_watch`, `on_wake` | `eg-flow-slint` | A service on another thread telling a feature that something changed |
| `every` | `eg-flow-slint` | A repeating intent |
| `Rows` | `eg-flow-slint` | A list model updated row by row, so a row under the pointer survives |

## Porting a page

`eg_companion::home` and `crates/ui/src/pages/home.rs` are the worked example.

1. **Feature, in a crate without Slint.** State is plain data. Anything that touches a disk, a process or a worker is an
   `Effect`, and its result comes back as an `Intent`. Anything from outside, such as a worker's snapshot, arrives as an
   intent too and is kept in the state.
2. **`present`, next to the feature.** It takes only what it reads and returns plain structs. Keep what it reads in
   `Tracked` so a render can tell when it needs running again.
3. **Host, in `eg-ui`.** Map view callbacks to intents and nothing else. `render` runs each `Slice`, syncs `Rows` and
   `Subscriptions`. `perform` matches on the effect.
4. **View.** Components take properties and callbacks. Only the page root reads the page's global. What only the view cares
   about, such as hover, highlights and scroll, stays in Slint.
5. **Tests.** Reducer rules go through `step`, with no window. Behaviour across the view goes in `crates/ui/tests/captures.rs`
   on real workers.

## Rules worth keeping

- A write's outcome must always reach a store that is still alive. `Background` runs work to its end whatever happens, but
  delivers nowhere once its store is gone, so never tie a write to a scope that can close first.
- A panic in background work is `Err(Panicked)`, and each feature decides what it means. The runtime's shutdown still reports
  it, and the app's exit code follows.
- A `Slice` key that leaves out an input shows stale values. Keep the key beside the function it guards.
- Don't name a component's callback after a value some binding inside it uses: a callback `start` breaks `alignment: start`.
