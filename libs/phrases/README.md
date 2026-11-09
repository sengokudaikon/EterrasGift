# phrases

The words and numbers a UI shows, in the user's language. The English is the msgid. Views and Rust share one msgid space, so
the same English is translated once.

The `build` feature adds `phrases::build`, for a build script and the task that writes the template. Take it there only: it
brings a `.po` parser and a Rust tokenizer, which the shown words don't need.

## In views

```slint
@tr("Item: {}", name)
@tr("{n} file" | "{n} files" % count)
@tr("key" => "Delete")
```

The last one has a context, which tells apart two uses of the same English.

## In Rust

Declare words in a `phrases!` block beside the code that uses them:

```rust
phrases::phrases! {
    AddItem: "Add item"
    Added: "Added {name}."
    OpenVerb: "verb" => "Open"
    Files: "{n} file" | "{n} files"
}

assert_eq!(AddItem.get(), "Add item");
assert_eq!(Added.with(&[("name", &"notes.txt")]), "Added notes.txt.");
assert_eq!(Files.count(2), "2 files");
```

- Values go in by name through `Phrase::with`, so a translation can move them.
- A count goes through a `Plural`, whose `{n}` picks the form.
- Never build a sentence from pieces.
- The English has to be a literal: the template is read from the sources, not from a build.

## Turning off the default context

Both sides have to leave it off, or the same English becomes two entries:

- A Slint build script calls `with_default_translation_context(DefaultTranslationContext::None)`.
- `slint-tr-extractor` gets `--no-default-translation-context`, which `Template` passes for you.

## The template

Run `build::Template` from a task. `write` puts every word in one `.pot`, and `check` fails if that file is out of date.

- It collects only the app's sources: this crate has no words of its own.
- References name files only, so moving code doesn't make the template stale.
- The header has no date, so a check compares the file as written.
- Keep `slint-tr-extractor` at the version of Slint your views build with.

## Adding a translation

1. Put it in `<lang>/<language>/LC_MESSAGES/<domain>.po`.
2. In the build script, call `build::bundle_catalogs`. It writes the table for the Rust phrases and returns the languages it
   found. With no folder there it bundles none and warns; an empty folder bundles none quietly. It panics on a folder it can't
   read.
3. Pass the same folder to `with_bundled_translations`, but only if there are any languages: Slint refuses a folder with none.
   Slint reads `<package>.po`, named after the package that compiles the views, so that is the domain to give both calls.
   When the views compile in a crate of their own, its build script asks `build::languages` instead, which writes no table.
4. As soon as the first window exists, before any page or message is built, pick the language once for all: `Translations::new`
   on the table, `Translations::choose` on the user's locale, then `Translations::select` and `locale::select` with what it
   chose, and Slint's `select_bundled_translation` for the views. Views, phrases and numbers then show in the same language.
5. Test that `Translations::new` takes your catalogs. It fails when a catalog's plural rule can't be read.
