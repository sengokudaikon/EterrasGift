//! Every word an app shows, in one gettext template for translators. Two sources, one msgid space: each entry of a
//! `phrases!` block and each `Phrase::new("…")`, `Phrase::ctx("…", "…")` and `Plural::new("…", "…")` in the Rust sources (up
//! to a file's `#[cfg(test)]`), and each `@tr` in the views, read by `slint-tr-extractor`; a context (`"ctx" => …`,
//! `Phrase::ctx`, `@tr("ctx" => …)`) is the entry's msgctxt. References name files only, so moving code does not make the
//! template stale; the header carries no date, so a check compares as written.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// One template and where its words come from. Paths are relative to `root`, and are how the template refers to files.
pub struct Template<'a> {
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
    /// Fails if the template on disk differs from what the sources say, and writes nothing.
    pub fn check(&self) -> Result<(), String> {
        let (out, _) = self.generate()?;
        if self.current().as_deref() == Some(out.as_str()) {
            Ok(())
        } else {
            Err(format!("{} is out of date, run `{}`", self.path, self.command))
        }
    }

    /// Writes the template and says what it did.
    pub fn write(&self) -> Result<String, String> {
        let (out, count) = self.generate()?;
        let unchanged = self.current().as_deref() == Some(out.as_str());
        let path = self.root.join(self.path);
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(&path, &out).map_err(|e| format!("{path:?}: {e}"))?;
        Ok(format!("wrote {} ({count} phrases{})", self.path, if unchanged { ", unchanged" } else { "" }))
    }

    fn current(&self) -> Option<String> {
        fs::read_to_string(self.root.join(self.path)).ok()
    }

    /// The template's text and how many phrases it holds.
    fn generate(&self) -> Result<(String, usize), String> {
        generate(self)
    }
}

fn generate(template: &Template) -> Result<(String, usize), String> {
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
        let text = fs::read_to_string(path).map_err(|e| format!("{path:?}: {e}"))?;
        if text.contains(MACRO_DEFINITION) {
            continue;
        }
        // Comments only talk about phrases, and a file's tests (`#[cfg(test)]`, `#[cfg(all(test, …))]`) show the user nothing.
        let text: String = text
            .lines()
            .take_while(|line| !["#[cfg(test)]", "#[cfg(all(test,"].iter().any(|start| line.trim_start().starts_with(start)))
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        let text = text.as_str();
        for found in rust_phrases(text).map_err(|e| format!("{file}: {e}"))? {
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

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("{dir:?}: {e}"))? {
        let path = entry.map_err(|e| e.to_string())?.path();
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

/// Each entry of a `phrases!` block and each `Phrase::new("…")`, `Phrase::ctx("…", "…")` and `Plural::new("…", "…")`:
/// plain or raw string literals, as the crate's docs ask.
fn rust_phrases(text: &str) -> Result<Vec<Found>, String> {
    const BLOCK: &str = "phrases!";
    const CALLS: [&str; 4] = [BLOCK, "Phrase::new(", "Phrase::ctx(", "Plural::new("];
    let mut found = Vec::new();
    let mut rest = text;
    while let Some((at, call)) = CALLS.iter().filter_map(|call| rest.find(call).map(|at| (at, *call))).min() {
        rest = &rest[at + call.len()..];
        if call == BLOCK {
            rest = block_phrases(rest, &mut found)?;
            continue;
        }
        let mut literal = |first: bool| {
            if !first {
                rest = rest.trim_start().strip_prefix(',').ok_or_else(|| format!("{call} takes two string literals"))?;
            }
            let (text, after) = rust_literal(rest.trim_start()).ok_or_else(|| format!("{call} without a string literal near {:?}", &rest[..rest.len().min(40)]))?;
            rest = after;
            Ok::<_, String>(text)
        };
        let first = literal(true)?;
        found.push(match call {
            "Phrase::new(" => Found {
                context: None,
                msgid: first,
                plural: None,
            },
            "Phrase::ctx(" => Found {
                context: Some(first),
                msgid: literal(false)?,
                plural: None,
            },
            _ => Found {
                context: None,
                msgid: first,
                plural: Some(literal(false)?),
            },
        });
    }
    Ok(found)
}

/// The entries of a `phrases!` block, `text` starting after the macro's name, and what follows the block. Entry by
/// entry rather than to the closing brace, since the English holds braces of its own.
fn block_phrases<'a>(text: &'a str, found: &mut Vec<Found>) -> Result<&'a str, String> {
    let near = |text: &str| format!("phrases! near {:?}", text.chars().take(40).collect::<String>());
    let mut rest = text.trim_start().strip_prefix('{').ok_or_else(|| near(text))?;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix('}') {
            return Ok(after);
        }
        if let Some(attribute) = rest.strip_prefix("#[") {
            let close = attribute.find(']').ok_or_else(|| near(rest))?;
            rest = &attribute[close + 1..];
            continue;
        }
        if rest.starts_with("//") {
            rest = rest.split_once('\n').map_or("", |(_, after)| after);
            continue;
        }
        // The visibility and name, which the macro has already checked.
        let (declared, after) = rest.split_once(':').filter(|(declared, _)| !declared.contains(['"', '{', '}'])).ok_or_else(|| near(rest))?;
        let name = declared.trim();
        let (first, after) = rust_literal(after.trim_start()).ok_or_else(|| near(name))?;
        rest = after.trim_start();
        let mut second = |operator: &str| -> Result<Option<String>, String> {
            let Some(after) = rest.strip_prefix(operator) else {
                return Ok(None);
            };
            let (second, after) = rust_literal(after.trim_start()).ok_or_else(|| near(name))?;
            rest = after;
            Ok(Some(second))
        };
        found.push(if let Some(msgid) = second("=>")? {
            Found {
                context: Some(first),
                msgid,
                plural: None,
            }
        } else {
            Found {
                context: None,
                msgid: first,
                plural: second("|")?,
            }
        });
    }
}

/// A Rust string literal at the start of `text`, unescaped, and what follows it.
fn rust_literal(text: &str) -> Option<(String, &str)> {
    if let Some(raw) = text.strip_prefix('r') {
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        let body = raw[hashes..].strip_prefix('"')?;
        let close = format!("\"{}", "#".repeat(hashes));
        let end = body.find(&close)?;
        return Some((body[..end].to_owned(), &body[end + close.len()..]));
    }
    let body = text.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Some((out, &body[at + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                'u' => {
                    let digits: String = chars.by_ref().map(|(_, c)| c).skip(1).take_while(|c| *c != '}').collect();
                    out.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
                }
                // A line continuation: the break and the next line's indent are not part of the string.
                '\n' | '\r' => {
                    let tail = &body[at + 1..];
                    let skipped = tail.len() - tail.trim_start().len();
                    for _ in 0..tail[..skipped].chars().count() - 1 {
                        chars.next();
                    }
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// The views' `@tr` strings, as `slint-tr-extractor` reads them, with each file they are in.
fn view_phrases(root: &Path, folder: &str) -> Result<Vec<(Found, String)>, String> {
    let mut views = Vec::new();
    slint_files(&root.join(folder), &mut views)?;
    views.sort();
    let output = std::env::temp_dir().join(format!("spiderland-phrases-{}.pot", std::process::id()));
    let status = Command::new("slint-tr-extractor")
        .current_dir(root)
        .arg("--no-default-translation-context")
        .arg("-o")
        .arg(&output)
        .args(views.iter().map(|path| relative(root, path)))
        .status()
        .map_err(|e| format!("slint-tr-extractor, of the Slint version the views build with, could not run: {e}"));
    let text = fs::read_to_string(&output);
    let _ = fs::remove_file(&output);
    if !status?.success() {
        return Err("slint-tr-extractor failed".into());
    }
    let text = text.map_err(|e| format!("{output:?}: {e}"))?;
    let mut found = Vec::new();
    for block in text.replace("\r\n", "\n").split("\n\n") {
        let mut files: Vec<String> = Vec::new();
        let mut fields: BTreeMap<&str, String> = BTreeMap::new();
        let mut current = None;
        for line in block.lines() {
            if let Some(references) = line.strip_prefix("#: ") {
                for at in references.split_whitespace() {
                    let file = at.rsplit_once(':').map_or(at, |(path, _)| path).to_owned();
                    if !files.contains(&file) {
                        files.push(file);
                    }
                }
            } else if let Some((key, value)) = line.split_once(' ').filter(|(key, _)| matches!(*key, "msgid" | "msgid_plural" | "msgctxt")) {
                fields.insert(key, po_string(value)?);
                current = Some(key);
            } else if line.starts_with('"') {
                if let Some(key) = current {
                    let more = po_string(line)?;
                    fields.entry(key).or_default().push_str(&more);
                }
            } else {
                current = None;
            }
        }
        if let Some(msgid) = fields.remove("msgid").filter(|msgid| !msgid.is_empty()) {
            let (context, plural) = (fields.remove("msgctxt"), fields.remove("msgid_plural"));
            for file in files {
                let found_here = Found {
                    context: context.clone(),
                    msgid: msgid.clone(),
                    plural: plural.clone(),
                };
                found.push((found_here, file));
            }
        }
    }
    Ok(found)
}

fn slint_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("{dir:?}: {e}"))? {
        let path = entry.map_err(|e| e.to_string())?.path();
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
        assert!(rust_phrases("Phrase::new(NAME)").is_err());
    }

    #[test]
    fn contexts_and_plurals_are_read_in_order() {
        let found = rust_phrases("Plural::new(\"{n} mod\", \"{n} mods\") Phrase::ctx(\"key\",\n    \"Delete\") Phrase::new(\"Delete\")").unwrap();
        let entry = |context: Option<&str>, msgid: &str, plural: Option<&str>| Found {
            context: context.map(Into::into),
            msgid: msgid.into(),
            plural: plural.map(Into::into),
        };
        assert_eq!(
            found,
            [entry(None, "{n} mod", Some("{n} mods")), entry(Some("key"), "Delete", None), entry(None, "Delete", None)]
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
                Drafts: "{n} draft" | "{n} drafts"
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
                entry(None, "{n} draft", Some("{n} drafts")),
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
}
