# spiderland

A small frame for desktop UIs: one-way state, and the words the UI shows.

| Crate | What it is | Depends on |
|---|---|---|
| `spiderland` | One-way UI state | Nothing |
| `spiderland-slint` | What a Slint host needs | `spiderland`, `spiderland-phrases`, Slint, `tokio`'s channels |
| `spiderland-phrases` | Words and numbers, ready for translation | Nothing but one Windows call |
| `spiderland-phrases-build` | Translations into a table, and the shown words into a gettext template | `rspolib` |

A feature crate takes `spiderland` and `spiderland-phrases` and stays free of Slint. The framework creates no tasks: the app gives
`Background` a way to start a job on its own pool, and `Background` only brings the result back to the UI thread.

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
| `callbacks!` | `spiderland` | A view's callbacks as intents, one per line |
| `Background` | `spiderland-slint` | Work off the UI thread, on the app's own pool, that comes back as an intent, with panics as values |
| `Background::wait` | `spiderland-slint` | An answer awaited on the UI thread, with no pool thread held, that comes back as an intent |
| `local` | `spiderland-slint` | Waiting on the UI thread for work that can be let go of, even from what its own result causes |
| `finish` | `spiderland-slint` | Waiting on the UI thread for work whose result must always be heard |
| `on_watch`, `on_wake` | `spiderland-slint` | A service on another thread telling a feature that something changed |
| `every`, `after` | `spiderland-slint` | A repeating intent, and a delayed one that makes a debounce |
| `Rows` | `spiderland-slint` | A list model updated row by row, so a row under the pointer survives |
| `select_language` | `spiderland-slint` | The user's language, for the views and the phrases alike |
| `testing` | `spiderland-slint` | The real window with no display, driven by real input (feature `testing`) |
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
3. **Host, in the crate with the window.** `callbacks!` maps view callbacks to intents and nothing else. `render` runs each
   `Slice`, syncs `Rows` and `Subscriptions`. `perform` matches on the effect.
4. **View.** Components take properties and callbacks. Only the page root reads the page's global. What only the view cares
   about, such as hover, highlights and scroll, stays in Slint.
5. **Tests.** Reducer rules go through `step`, with no window. Behaviour across the view goes through the real window on real
   background work, with `spiderland_slint::testing`. A complete small feature is in the `spiderland` crate's docs.

## Words

- **Views** write `@tr("Item: {}", name)`, and counts `@tr("{n} file" | "{n} files" % count)`. A context is
  `@tr("key" => "Delete")`.
- **Rust** declares a `phrases!` block beside its use, `Name: "English"`, with `"context" => "English"` and `"one" | "other"`
  (see the macro). Values go in by name, `"Added {name}."`, through `Phrase::with`, so a translation can move them. A count goes
  through a `Plural`, whose `{n}` picks the form. Never build a sentence from pieces.
- **One msgid space.** The English is the msgid, and both sides leave the default context off: a Slint build script calls
  `with_default_translation_context(DefaultTranslationContext::None)`, and `slint-tr-extractor` gets
  `--no-default-translation-context`, which `Template` passes.
- **The template.** A task runs `Template`, which writes every word to one `.pot`, with references to files only and no date, so
  a check compares as written and moving code leaves it current. Keep `slint-tr-extractor` at the version of Slint the views build
  with.
- **A translation** goes in `<lang>/<language>/LC_MESSAGES/<domain>.po`. The build script's `bundle_catalogs` makes the table for
  the Rust phrases, and hands back the languages found, to pass the same folder to `with_bundled_translations` only when there are
  any, because Slint refuses a folder with none. `select_language` then shows the views and the phrases in the same language.

## Rules worth keeping

- A write's outcome must always reach a store that is still alive. `Background` runs work to its end whatever happens, but
  delivers nowhere once its store is gone, so never tie a write to a scope that can close first.
- A panic in background work is `Err(Panicked)`, and each feature decides what it means. The panic then goes on to the app's
  pool, which can keep a record of it.
- A `Slice` key that leaves out an input shows stale values. Keep the key beside the function it guards.
- Don't name a component's callback after a value some binding inside it uses: a callback `start` breaks `alignment: start`.
- A phrase's English is a literal: the template is read from the sources, not from a build.
- A Slint global can't be named `Text`: that is the element. Name a page's words `HomeText`.
