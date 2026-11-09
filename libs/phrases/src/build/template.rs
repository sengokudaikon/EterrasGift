//! Every word an app shows, in one gettext template for translators. Two sources, one msgid space: each entry of a
//! `phrases!` block and each `Phrase::new("…")`, `Phrase::ctx("…", "…")` and `Plural::new("…", "…")` in the Rust sources, and
//! each `@tr` in the views, read by `slint-tr-extractor`; a context (`"ctx" => …`, `Phrase::ctx`, `@tr("ctx" => …)`) is the
//! entry's msgctxt. The Rust sources are read as Rust's tokens, leaving out what is only built for tests. References name files
//! only, so moving code does not make the template stale; the header carries no date, so a check compares as written.

use proc_macro2::{Delimiter, Group, Spacing, TokenStream, TokenTree};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::mem;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

/// Marks the file defining `phrases!`: its arms spell out the calls with metavariables, not with words.
const MACRO_DEFINITION: &str = "macro_rules! phrases";
const NEWLINE: &str = "\r\n";

/// One phrase as the sources write it: its context, its English (a plural's singular) and a plural's other form.
#[derive(Debug, PartialEq)]
struct Found {
    context: Option<String>,
    msgid: String,
    plural: Option<String>,
}

/// One msgid with where it comes from.
#[derive(Default)]
struct Entry {
    plural: Option<String>,
    files: Vec<String>,
}

/// Why a template couldn't be checked or written. Each message carries its cause.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A file or folder couldn't be read or written.
    #[error("{}: {error}", path.display())]
    Io {
        /// The file or folder.
        path: PathBuf,
        /// What went wrong there.
        error: io::Error,
    },
    /// A source holds a phrase that isn't written the way `phrases!`, `Phrase::new`, `Phrase::ctx` and `Plural::new` ask.
    #[error("{file}: {reason}")]
    Unreadable {
        /// The source, as the template names it.
        file: String,
        /// What it holds, and what was expected.
        reason: String,
    },
    /// `slint-tr-extractor` couldn't be started.
    #[error("slint-tr-extractor, of the Slint version the views build with, could not run: {0}")]
    NoExtractor(io::Error),
    /// `slint-tr-extractor` ran and failed.
    #[error("slint-tr-extractor failed: {0}")]
    ExtractorFailed(ExitStatus),
    /// The template on disk differs from what the sources say.
    #[error("{path} is out of date, run `{command}`")]
    OutOfDate {
        /// The template, as [`Template::path`] names it.
        path: String,
        /// The command that writes it, as [`Template::command`] names it.
        command: String,
    },
}

/// One template and where its words come from. Paths are relative to `root`, and are how the template refers to files.
#[derive(Debug)]
pub struct Template<'a> {
    /// The folder every other path starts from, usually the workspace's.
    pub root: &'a Path,
    /// The file to write.
    pub path: &'a str,
    /// The project's name, for the header.
    pub project: &'a str,
    /// The command that writes the template, for whoever finds it out of date.
    pub command: &'a str,
    /// Folders of Rust sources, searched in depth. Each one's tests and benches are left out.
    pub sources: &'a [&'a str],
    /// The folder of `.slint` views.
    pub views: &'a str,
}

impl Template<'_> {
    /// Fails with [`Error::OutOfDate`] if the template on disk differs from what the sources say, and writes nothing.
    pub fn check(&self) -> Result<(), Error> {
        let (out, _) = self.generate()?;
        if self.current().as_deref() == Some(out.as_str()) {
            Ok(())
        } else {
            Err(Error::OutOfDate {
                path: self.path.to_owned(),
                command: self.command.to_owned(),
            })
        }
    }

    /// Writes the template, unless it already says what the sources do: then the file is left as it is, so nothing that
    /// watches it rebuilds.
    pub fn write(&self) -> Result<Written, Error> {
        let (out, phrases) = self.generate()?;
        let changed = self.current().as_deref() != Some(out.as_str());
        if changed {
            let path = self.root.join(self.path);
            let folder = path.parent().unwrap_or(self.root);
            fs::create_dir_all(folder).map_err(|error| io_error(folder, error))?;
            fs::write(&path, &out).map_err(|error| io_error(&path, error))?;
        }
        Ok(Written { phrases, changed })
    }

    fn current(&self) -> Option<String> {
        fs::read_to_string(self.root.join(self.path)).ok()
    }

    /// The template's text and how many phrases it holds.
    fn generate(&self) -> Result<(String, usize), Error> {
        generate(self)
    }
}

/// What [`Template::write`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    /// How many phrases the template holds.
    pub phrases: usize,
    /// Whether the file changed; an unchanged template isn't written again.
    pub changed: bool,
}

fn io_error(path: &Path, error: io::Error) -> Error {
    Error::Io { path: path.to_owned(), error }
}

fn generate(template: &Template) -> Result<(String, usize), Error> {
    let root = template.root;
    let mut entries: BTreeMap<(Option<String>, String), Entry> = BTreeMap::new();
    let mut add = |found: Found, file: String| {
        let entry = entries.entry((found.context, found.msgid)).or_default();
        entry.plural = entry.plural.take().or(found.plural);
        if !entry.files.contains(&file) {
            entry.files.push(file);
        }
    };
    let mut sources = Vec::new();
    for folder in template.sources {
        rust_files(&root.join(folder), &mut sources)?;
    }
    sources.sort();
    for path in &sources {
        let file = relative(root, path);
        let text = fs::read_to_string(path).map_err(|error| io_error(path, error))?;
        if text.contains(MACRO_DEFINITION) {
            continue;
        }
        for found in rust_phrases(&text).map_err(|reason| Error::Unreadable { file: file.clone(), reason })? {
            add(found, file.clone());
        }
    }
    for (found, file) in view_phrases(root, template.views)? {
        add(found, file);
    }

    // Grouped by where each first appears, so a page's words sit together.
    let mut ordered: Vec<(&(Option<String>, String), &Entry)> = entries.iter().collect();
    ordered.sort_by(|a, b| (&a.1.files[0], a.0).cmp(&(&b.1.files[0], b.0)));
    let mut out = String::new();
    let mut line = |text: &str| {
        out.push_str(text);
        out.push_str(NEWLINE);
    };
    line(&format!("# {}: the words it shows. Generated by `{}`; do not edit.", template.project, template.command));
    line("msgid \"\"");
    line("msgstr \"\"");
    line(&format!("\"Project-Id-Version: {}\\n\"", template.project));
    line("\"MIME-Version: 1.0\\n\"");
    line("\"Content-Type: text/plain; charset=UTF-8\\n\"");
    line("\"Content-Transfer-Encoding: 8bit\\n\"");
    line("\"Plural-Forms: nplurals=2; plural=(n != 1);\\n\"");
    for ((context, msgid), entry) in ordered {
        line("");
        line(&format!("#: {}", entry.files.join(" ")));
        if let Some(context) = context {
            line(&format!("msgctxt {}", quote(context)));
        }
        line(&format!("msgid {}", quote(msgid)));
        match &entry.plural {
            Some(plural) => {
                line(&format!("msgid_plural {}", quote(plural)));
                line("msgstr[0] \"\"");
                line("msgstr[1] \"\"");
            }
            None => line("msgstr \"\""),
        }
    }
    Ok((out, entries.len()))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Error> {
    for entry in fs::read_dir(dir).map_err(|error| io_error(dir, error))? {
        let path = entry.map_err(|error| io_error(dir, error))?.path();
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if path.is_dir() {
            // Tests and benches show the user nothing.
            if !matches!(name, "tests" | "benches" | "target") {
                rust_files(&path, out)?;
            }
        } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
            out.push(path);
        }
    }
    Ok(())
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// Each entry of a `phrases!` block and each `Phrase::new("…")`, `Phrase::ctx("…", "…")` and `Plural::new("…", "…")`, read from
/// Rust's tokens, so a comment or a string that mentions one isn't one. An item under `#[cfg(test)]` or `#[cfg(all(test, …))]`
/// shows the user nothing, so it is left out with all it holds.
fn rust_phrases(text: &str) -> Result<Vec<Found>, String> {
    // Rust reads a line break inside a literal as `\n`, whatever the file's line endings.
    let tokens: TokenStream = text.replace("\r\n", "\n").parse().map_err(|error| format!("can't be read as Rust: {error}"))?;
    let mut found = Vec::new();
    scan(tokens, &mut found)?;
    Ok(found)
}

fn scan(tokens: TokenStream, found: &mut Vec<Found>) -> Result<(), String> {
    let tokens: Vec<TokenTree> = tokens.into_iter().collect();
    let mut at = 0;
    while let Some(token) = tokens.get(at) {
        if let Some(attribute) = attribute(&tokens, at) {
            at = match attribute {
                Attribute { tests_only: false, end, .. } => end,
                // `#![cfg(test)]`: all that holds it is for tests.
                Attribute { inner: true, .. } => return Ok(()),
                Attribute { end, .. } => past_item(&tokens, end),
            };
            continue;
        }
        match token {
            // A `!` joined to what follows is `!=`: `phrases != …` compares a value of that name.
            TokenTree::Ident(name)
                if name == "phrases" && matches!(tokens.get(at + 1), Some(TokenTree::Punct(bang)) if bang.as_char() == '!' && bang.spacing() == Spacing::Alone) =>
            {
                let Some(TokenTree::Group(block)) = tokens
                    .get(at + 2)
                    .filter(|token| matches!(token, TokenTree::Group(group) if group.delimiter() == Delimiter::Brace))
                else {
                    return Err(format!("phrases! takes its entries in braces, near {}", near(&tokens, at)));
                };
                block_phrases(block.stream(), found)?;
                at += 3;
            }
            _ if let Some((call, arguments)) = call(&tokens, at) => {
                found.push(call_phrase(call, arguments)?);
                at += 5;
            }
            TokenTree::Group(group) => {
                scan(group.stream(), found)?;
                at += 1;
            }
            _ => at += 1,
        }
    }
    Ok(())
}

/// An attribute, `#[…]` or `#![…]`.
struct Attribute {
    /// Where the tokens after it start.
    end: usize,
    /// `#![…]`, which is on what holds it rather than on what follows.
    inner: bool,
    /// What it is on is only built for tests.
    tests_only: bool,
}

fn attribute(tokens: &[TokenTree], at: usize) -> Option<Attribute> {
    if !is_punct(tokens.get(at)?, '#') {
        return None;
    }
    let inner = tokens.get(at + 1).is_some_and(|token| is_punct(token, '!'));
    let body = at + 1 + usize::from(inner);
    let TokenTree::Group(group) = tokens.get(body)? else {
        return None;
    };
    (group.delimiter() == Delimiter::Bracket).then(|| Attribute {
        end: body + 1,
        inner,
        tests_only: only_in_tests(group.stream()),
    })
}

/// `cfg(test)`, or `cfg(all(…))` with `test` among what it needs.
fn only_in_tests(attribute: TokenStream) -> bool {
    let tokens: Vec<TokenTree> = attribute.into_iter().collect();
    let [TokenTree::Ident(cfg), TokenTree::Group(condition)] = &tokens[..] else {
        return false;
    };
    let condition: Vec<TokenTree> = condition.stream().into_iter().collect();
    cfg == "cfg"
        && match &condition[..] {
            [TokenTree::Ident(test)] => test == "test",
            [TokenTree::Ident(all), TokenTree::Group(needs)] if all == "all" => needs.stream().into_iter().any(|token| matches!(&token, TokenTree::Ident(test) if test == "test")),
            _ => false,
        }
}

/// Where the item starting at `at` ends: after its first `;` or `{…}`, or after a `,` that ends a field or an arm. A `,` between
/// angle brackets, as in `<A, B>`, doesn't end it.
fn past_item(tokens: &[TokenTree], mut at: usize) -> usize {
    let mut angles = 0usize;
    while let Some(token) = tokens.get(at) {
        let arrow = at > 0 && matches!(&tokens[at - 1], TokenTree::Punct(before) if matches!(before.as_char(), '-' | '=') && before.spacing() == Spacing::Joint);
        at += 1;
        match token {
            TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => return at,
            TokenTree::Punct(punct) => match punct.as_char() {
                ';' => return at,
                ',' if angles == 0 => return at,
                '<' => angles += 1,
                '>' if !arrow => angles = angles.saturating_sub(1),
                _ => {}
            },
            _ => {}
        }
    }
    at
}

/// `Phrase::new(…)`, `Phrase::ctx(…)` or `Plural::new(…)` at `at`, with its arguments.
fn call(tokens: &[TokenTree], at: usize) -> Option<(&'static str, &Group)> {
    let [
        TokenTree::Ident(kind),
        TokenTree::Punct(first),
        TokenTree::Punct(second),
        TokenTree::Ident(function),
        TokenTree::Group(arguments),
    ] = tokens.get(at..at + 5)?
    else {
        return None;
    };
    if first.as_char() != ':' || first.spacing() != Spacing::Joint || second.as_char() != ':' || arguments.delimiter() != Delimiter::Parenthesis {
        return None;
    }
    let call = match (kind.to_string().as_str(), function.to_string().as_str()) {
        ("Phrase", "new") => "Phrase::new",
        ("Phrase", "ctx") => "Phrase::ctx",
        ("Plural", "new") => "Plural::new",
        _ => return None,
    };
    Some((call, arguments))
}

/// The phrase a call makes: its arguments are string literals, as the crate's docs ask.
fn call_phrase(call: &str, arguments: &Group) -> Result<Found, String> {
    let unreadable = || {
        format!(
            "{call}{arguments} takes {}",
            if call == "Phrase::new" { "one string literal" } else { "two string literals" }
        )
    };
    let mut texts = Vec::new();
    let mut tokens = arguments.stream().into_iter();
    while let Some(token) = tokens.next() {
        texts.push(literal(&token).ok_or_else(unreadable)?);
        if tokens.next().is_some_and(|comma| !is_punct(&comma, ',')) {
            return Err(unreadable());
        }
    }
    Ok(match (call, &mut texts[..]) {
        ("Phrase::new", [msgid]) => Found {
            context: None,
            msgid: mem::take(msgid),
            plural: None,
        },
        ("Phrase::ctx", [context, msgid]) => Found {
            context: Some(mem::take(context)),
            msgid: mem::take(msgid),
            plural: None,
        },
        ("Plural::new", [one, other]) => Found {
            context: None,
            msgid: mem::take(one),
            plural: Some(mem::take(other)),
        },
        _ => return Err(unreadable()),
    })
}

/// The entries of a `phrases!` block. An entry under `#[cfg(test)]` is left out.
fn block_phrases(block: TokenStream, found: &mut Vec<Found>) -> Result<(), String> {
    let tokens: Vec<TokenTree> = block.into_iter().collect();
    let mut at = 0;
    while at < tokens.len() {
        let mut tests_only = false;
        while let Some(attribute) = attribute(&tokens, at) {
            tests_only |= attribute.tests_only;
            at = attribute.end;
        }
        // The visibility, which the macro has already checked.
        if matches!(tokens.get(at), Some(TokenTree::Ident(word)) if word == "pub") {
            at += 1;
            if matches!(tokens.get(at), Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Parenthesis) {
                at += 1;
            }
        }
        let Some(TokenTree::Ident(name)) = tokens.get(at) else {
            return Err(format!("phrases! has no name near {}", near(&tokens, at)));
        };
        let unreadable = || format!("phrases! entry {name} isn't `Name: \"English\"`, `Name: \"context\" => \"English\"` or `Name: \"one\" | \"other\"`");
        if !tokens.get(at + 1).is_some_and(|token| is_punct(token, ':')) {
            return Err(unreadable());
        }
        let first = tokens.get(at + 2).and_then(literal).ok_or_else(unreadable)?;
        at += 3;
        let arrow = matches!(tokens.get(at), Some(TokenTree::Punct(equals)) if equals.as_char() == '=' && equals.spacing() == Spacing::Joint)
            && tokens.get(at + 1).is_some_and(|token| is_punct(token, '>'));
        let entry = if arrow {
            let msgid = tokens.get(at + 2).and_then(literal).ok_or_else(unreadable)?;
            at += 3;
            Found {
                context: Some(first),
                msgid,
                plural: None,
            }
        } else if tokens.get(at).is_some_and(|token| is_punct(token, '|')) {
            let other = tokens.get(at + 1).and_then(literal).ok_or_else(unreadable)?;
            at += 2;
            Found {
                context: None,
                msgid: first,
                plural: Some(other),
            }
        } else {
            Found {
                context: None,
                msgid: first,
                plural: None,
            }
        };
        if !tests_only {
            found.push(entry);
        }
    }
    Ok(())
}

/// A string literal's text, as Rust reads it.
fn literal(token: &TokenTree) -> Option<String> {
    let TokenTree::Literal(literal) = token else {
        return None;
    };
    match syn::Lit::new(literal.clone()) {
        syn::Lit::Str(text) => Some(text.value()),
        _ => None,
    }
}

fn is_punct(token: &TokenTree, char: char) -> bool {
    matches!(token, TokenTree::Punct(punct) if punct.as_char() == char)
}

/// The tokens from `at`, for an error to say where it is.
fn near(tokens: &[TokenTree], at: usize) -> String {
    let near: Vec<String> = tokens.iter().skip(at).take(6).map(ToString::to_string).collect();
    if near.is_empty() { "its end".to_owned() } else { format!("`{}`", near.join(" ")) }
}

/// The views' `@tr` strings, as `slint-tr-extractor` reads them, with the file each is in. It reads one file a run: handed
/// several, the extractor (1.18.1) writes a line of references too long to fit as its first file twice and drops the rest.
fn view_phrases(root: &Path, folder: &str) -> Result<Vec<(Found, String)>, Error> {
    let mut views = Vec::new();
    slint_files(&root.join(folder), &mut views)?;
    views.sort();
    let output = std::env::temp_dir().join(format!("phrases-{}.pot", std::process::id()));
    let mut found = Vec::new();
    for view in &views {
        let file = relative(root, view);
        let status = Command::new("slint-tr-extractor")
            .current_dir(root)
            .arg("--no-default-translation-context")
            .arg("-o")
            .arg(&output)
            .arg(&file)
            .status()
            .map_err(Error::NoExtractor);
        let text = fs::read_to_string(&output);
        let _ = fs::remove_file(&output);
        let status = status?;
        if !status.success() {
            return Err(Error::ExtractorFailed(status));
        }
        let text = text.map_err(|error| io_error(&output, error))?;
        let unreadable = |reason| Error::Unreadable {
            file: format!("slint-tr-extractor's output for {file}"),
            reason,
        };
        found.extend(extracted(&text).map_err(unreadable)?.into_iter().map(|phrase| (phrase, file.clone())));
    }
    Ok(found)
}

/// The entries of a `.pot` the extractor wrote, in its order. Their references are left out: each run reads one file.
fn extracted(text: &str) -> Result<Vec<Found>, String> {
    let mut found = Vec::new();
    for block in text.replace("\r\n", "\n").split("\n\n") {
        let mut fields: BTreeMap<&str, String> = BTreeMap::new();
        let mut current = None;
        for line in block.lines() {
            if let Some((key, value)) = line.split_once(' ').filter(|(key, _)| matches!(*key, "msgid" | "msgid_plural" | "msgctxt")) {
                fields.insert(key, po_string(value)?);
                current = Some(key);
            } else if line.starts_with('"') {
                if let Some(key) = current {
                    fields.entry(key).or_default().push_str(&po_string(line)?);
                }
            } else {
                current = None;
            }
        }
        if let Some(msgid) = fields.remove("msgid").filter(|msgid| !msgid.is_empty()) {
            found.push(Found {
                context: fields.remove("msgctxt"),
                msgid,
                plural: fields.remove("msgid_plural"),
            });
        }
    }
    Ok(found)
}

fn slint_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Error> {
    for entry in fs::read_dir(dir).map_err(|error| io_error(dir, error))? {
        let path = entry.map_err(|error| io_error(dir, error))?.path();
        if path.is_dir() {
            slint_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "slint") {
            out.push(path);
        }
    }
    Ok(())
}

/// A quoted PO string, unescaped.
fn po_string(quoted: &str) -> Result<String, String> {
    let body = quoted
        .trim()
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| format!("not a PO string: {quoted}"))?;
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => return Err(format!("a PO string ends in a backslash: {quoted}")),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

/// A PO string literal; a msgid with line breaks is split after each, as gettext's tools write it.
fn quote(text: &str) -> String {
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('\t', "\\t").replace('\n', "\\n");
    if !text.contains('\n') || text.ends_with('\n') && text.matches('\n').count() == 1 {
        return format!("\"{}\"", escape(text));
    }
    let mut out = String::from("\"\"");
    for piece in text.split_inclusive('\n') {
        out.push_str(NEWLINE);
        out.push_str(&format!("\"{}\"", escape(piece)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msgids(text: &str) -> Vec<String> {
        rust_phrases(text).unwrap().into_iter().map(|found| found.msgid).collect()
    }

    #[test]
    fn rust_literals_unescape_as_rust_does() {
        assert_eq!(msgids(r#"const A: Phrase = Phrase::new("Say \"hi\"\n{name}");"#), ["Say \"hi\"\n{name}"]);
        assert_eq!(msgids("Phrase::new(\"one \\\n        two\")"), ["one two"]);
        assert_eq!(msgids(r###"Phrase::new(r#"raw "quoted""#)"###), ["raw \"quoted\""]);
        assert_eq!(msgids(r#"Phrase::new("T1\u{a0}only")"#), ["T1\u{a0}only"]);
        assert_eq!(msgids(r#"Phrase::new("\x41\x7e")"#), ["A~"]);
        assert_eq!(msgids("Phrase::new(\"two\r\nlines\")"), ["two\nlines"], "a CRLF checkout reads as Rust reads it");
        assert!(rust_phrases("Phrase::new(NAME)").is_err());
        assert!(rust_phrases(r#"Phrase::new("a" "b")"#).is_err());
    }

    #[test]
    fn the_extractors_entries_are_read_in_its_order_without_the_header() {
        let pot = "#\nmsgid \"\"\nmsgstr \"\"\n\"Project-Id-Version: PACKAGE VERSION\\n\"\n\n\
            #: ui/a.slint:6 ui/a.slint:9\nmsgid \"Last Epoch\"\nmsgstr \"\"\n\n\
            #: ui/a.slint:12\nmsgctxt \"key\"\nmsgid \"Delete\"\nmsgstr \"\"\n\n\
            #: ui/a.slint:14\nmsgid \"\"\n\"{n} file \"\n\"left\"\nmsgid_plural \"{n} files left\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
        let found = |context: Option<&str>, msgid: &str, plural: Option<&str>| Found {
            context: context.map(str::to_owned),
            msgid: msgid.to_owned(),
            plural: plural.map(str::to_owned),
        };
        assert_eq!(
            extracted(pot).unwrap(),
            [
                found(None, "Last Epoch", None),
                found(Some("key"), "Delete", None),
                found(None, "{n} file left", Some("{n} files left")),
            ]
        );
        assert!(extracted("msgid \"unterminated\nmsgstr \"\"\n").is_err());
    }

    #[test]
    fn what_is_built_only_for_tests_is_left_out_and_what_follows_it_is_read() {
        let text = r##"
            #[cfg(test)]
            mod lib_tests;
            const FIRST: Phrase = Phrase::new("After a test module's declaration");
            #[cfg(test)]
            #[allow(unused)]
            fn helper<A, B>() -> Phrase { Phrase::new("In a test helper") }
            #[cfg(all(windows, test))]
            mod windows_tests { const A: Phrase = Phrase::new("In Windows' tests"); }
            #[cfg(not(test))]
            const SHOWN: Phrase = Phrase::new("Outside tests");
            fn shown(case: u8) -> Phrase {
                match case {
                    #[cfg(test)]
                    0 => Phrase::new("In a test arm"),
                    _ => Phrase::new("In the next arm"),
                }
            }
            phrases! {
                #[cfg(test)]
                ForATest: "In a test entry"
                Kept: "Kept"
            }
            #[cfg(test)]
            mod tests {
                const A: Phrase = Phrase::new("In the tests");
            }
            const LAST: Phrase = Phrase::new("After the tests");
        "##;
        assert_eq!(
            msgids(text),
            ["After a test module's declaration", "Outside tests", "In the next arm", "Kept", "After the tests"]
        );
        assert!(msgids("#![cfg(test)]\nconst A: Phrase = Phrase::new(\"In a file of tests\");").is_empty());
    }

    #[test]
    fn a_comment_or_a_string_that_mentions_a_phrase_is_not_one() {
        let text = r##"
            // Phrase::new("In a line comment")
            /* Plural::new("In a block", "comment") */
            /// Phrase::new("In a doc comment")
            const A: &str = "Phrase::new(\"In a string\")";
            const B: Phrase = Phrase::new("Real"); // phrases! { Trailing: "comment" }
            const C: fn(&'static str) -> Phrase = Phrase::new;
            fn some(phrases: usize) -> bool { phrases != 0 }
        "##;
        assert_eq!(msgids(text), ["Real"]);
    }

    #[test]
    fn contexts_and_plurals_are_read_in_order() {
        let found = rust_phrases("Plural::new(\"{n} file\", \"{n} files\") Phrase::ctx(\"key\",\n    \"Delete\") Phrase::new(\"Delete\")").unwrap();
        let entry = |context: Option<&str>, msgid: &str, plural: Option<&str>| Found {
            context: context.map(Into::into),
            msgid: msgid.into(),
            plural: plural.map(Into::into),
        };
        assert_eq!(
            found,
            [entry(None, "{n} file", Some("{n} files")), entry(Some("key"), "Delete", None), entry(None, "Delete", None)]
        );
        assert!(rust_phrases(r#"Phrase::ctx("key")"#).is_err());
    }

    #[test]
    fn a_block_reads_each_shape_and_what_follows_it() {
        let found = rust_phrases(
            r#"
            phrases! {
                #[cfg(windows)]
                pub Added: "Added {name}." // braces in the English do not end the block
                pub(super) Key: "key" => "Delete"
                Files: "{n} file" | "{n} files"
                Split: "one \
                        two"
            }
            phrases! { Local: "Inside a function" }
            Phrase::new("After the blocks")
            "#,
        )
        .unwrap();
        let entry = |context: Option<&str>, msgid: &str, plural: Option<&str>| Found {
            context: context.map(Into::into),
            msgid: msgid.into(),
            plural: plural.map(Into::into),
        };
        assert_eq!(
            found,
            [
                entry(None, "Added {name}.", None),
                entry(Some("key"), "Delete", None),
                entry(None, "{n} file", Some("{n} files")),
                entry(None, "one two", None),
                entry(None, "Inside a function", None),
                entry(None, "After the blocks", None),
            ]
        );
    }

    #[test]
    fn a_block_it_cannot_read_is_an_error() {
        for broken in [
            "phrases!",
            "phrases! (A: \"a\")",
            "phrases! { A: NAME }",
            "phrases! { A \"a\" }",
            "phrases! { A: \"a\" | }",
            "phrases! { A: \"a\"",
        ] {
            assert!(rust_phrases(broken).is_err(), "{broken}");
        }
    }

    #[test]
    fn po_strings_round_trip() {
        for text in ["plain", "with \"quotes\" and \\", "two\nlines", "ends\n"] {
            let quoted = quote(text);
            let joined: String = quoted.split(NEWLINE).map(|piece| po_string(piece).unwrap()).collect();
            assert_eq!(joined, text);
        }
    }

    /// A template over `root`, reading Rust from `src` and views from `ui`.
    fn template(root: &Path) -> Template<'_> {
        Template {
            root,
            path: "lang/app.pot",
            project: "app",
            command: "write the template",
            sources: &["src"],
            views: "ui",
        }
    }

    #[test]
    fn a_folder_it_cannot_read_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let error = template(dir.path()).check().unwrap_err();
        assert!(matches!(&error, Error::Io { path, .. } if *path == dir.path().join("src")), "{error}");
        assert!(error.to_string().starts_with(&format!("{}: ", dir.path().join("src").display())), "{error}");
    }

    #[test]
    fn a_source_it_cannot_read_is_named_with_what_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "const A: Phrase = Phrase::new(NAME);").unwrap();
        let error = template(dir.path()).write().unwrap_err();
        assert!(matches!(&error, Error::Unreadable { file, .. } if file == "src/lib.rs"), "{error}");
        assert_eq!(error.to_string(), "src/lib.rs: Phrase::new(NAME) takes one string literal");
        assert!(!dir.path().join("lang").exists(), "and nothing is written");
    }
}
