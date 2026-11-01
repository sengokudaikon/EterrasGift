# spiderland

A small frame for desktop UIs: one-way state, and the words the UI shows. It names no app or game.

| Crate | What it is | Depends on |
|---|---|---|
| `spiderland` | One-way UI state | Nothing |
| `spiderland-slint` | What a Slint host needs | `spiderland`, `task-runtime`, Slint |
| `spiderland-phrases` | Words and numbers, ready for translation | Nothing but one Windows call |
| `spiderland-phrases-build` | Translations into a table, and the shown words into a gettext template | `rspolib` |

A feature crate takes `spiderland` and `spiderland-phrases` and stays free of Slint. Background work stays in `task-runtime`,
which services use with no UI at all: `spiderland-slint` only brings its results back to the UI thread.

| Primitive | Crate | What it is for |
|---|---|---|
| `Feature` | `spiderland` | The pure half: `reduce(state, intent, effects)` |
| `Host` | `spiderland` | The impure half: `render` the state, `perform` the effects |
| `Store` | `spiderland` | Runs the two on the UI thread and queues intents sent while it works |
| `Tracked`, `Slice` | `spiderland` | Redo only the parts of a render whose inputs changed |
| `CheckedSlice` | `spiderland` | A `Slice` that fails a test build when its key misses an input |
| `Subscriptions` | `spiderland` | Timers and watchers that run for as long as the state wants them |
| `Effects::scoped` | `spiderland` | A child scope's reducer inside its parent's |
| `step` | `spiderland` | One reducer step in a test |
| `Background` | `spiderland-slint` | Work off the UI thread, on a `task_runtime` scope, that comes back as an intent, with panics as values |
| `local` | `spiderland-slint` | Waiting on the UI thread for work that can be let go of, even from what its own result causes |
| `on_watch`, `on_wake` | `spiderland-slint` | A service on another thread telling a feature that something changed |
| `every` | `spiderland-slint` | A repeating intent |
| `Rows` | `spiderland-slint` | A list model updated row by row, so a row under the pointer survives |
| `phrases!`, `Phrase`, `Plural`, `list` | `spiderland-phrases` | Words declared beside the code that uses them, by name |
| `Translations`, `locale` | `spiderland-phrases` | The bundled catalogs, the one language shown, number symbols |
| `bundle_catalogs` | `spiderland-phrases-build` | A build script's table of catalogs, from `.po` files |
| `Template` | `spiderland-phrases-build` | Every `phrases!` entry and `@tr` in one `.pot` |

## Writing a page

1. **Feature, in a crate without Slint.** State is plain data. Anything that touches a disk, a process or a worker is an
   `Effect`, and its result comes back as an `Intent`. Anything from outside, such as a worker's snapshot, arrives as an
   intent too and is kept in the state.
2. **`present`, next to the feature.** It takes only what it reads and returns plain structs. Keep what it reads in
   `Tracked` so a render can tell when it needs running again.
3. **Host, in the crate with the window.** Map view callbacks to intents and nothing else. `render` runs each `Slice`, syncs
   `Rows` and `Subscriptions`. `perform` matches on the effect.
4. **View.** Components take properties and callbacks. Only the page root reads the page's global. What only the view cares
   about, such as hover, highlights and scroll, stays in Slint.
5. **Tests.** Reducer rules go through `step`, with no window. Behaviour across the view goes through the real window on real
   background work.

## Rules worth keeping

- A write's outcome must always reach a store that is still alive. `Background` runs work to its end whatever happens, but
  delivers nowhere once its store is gone, so never tie a write to a scope that can close first.
- A panic in background work is `Err(Panicked)`, and each feature decides what it means. The runtime's shutdown still reports
  it, so whoever composes the app can act on it.
- A `Slice` key that leaves out an input shows stale values. Keep the key beside the function it guards.
- Don't name a component's callback after a value some binding inside it uses: a callback `start` breaks `alignment: start`.
- A phrase's English is a literal: the template is read from the sources, not from a build.
