//! MelonPreferences.cfg is TOML. Edits replace only original value spans, preserving comments, order, spacing,
//! unknown entries and line endings.

use crate::invalid;
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::ops::Range;
use std::path::Path;
use toml_edit::{Document, Table, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferenceKind {
    Switch,
    WholeNumber,
    Number,
    Text,
    Other,
}

impl PreferenceKind {
    pub fn accepts(self, value: &PreferenceValue) -> bool {
        matches!(
            (self, value),
            (Self::Switch, PreferenceValue::Bool(_))
                | (Self::WholeNumber, PreferenceValue::Integer(_))
                | (Self::Number, PreferenceValue::Float(_) | PreferenceValue::Integer(_))
                | (Self::Text, PreferenceValue::Text(_))
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PreferenceValue {
    Bool(bool),
    Integer(i64),
    Float(f64),
    Text(String),
}

impl PreferenceValue {
    pub fn from_json(value: &JsonValue) -> Option<Self> {
        match value {
            JsonValue::Bool(value) => Some(Self::Bool(*value)),
            JsonValue::Number(value) => value.as_i64().map(Self::Integer).or_else(|| value.as_f64().map(Self::Float)),
            JsonValue::String(value) => Some(Self::Text(value.clone())),
            _ => None,
        }
    }

    /// JSON profiles cannot hold TOML's infinities/NaN. Refuse them instead of silently persisting null.
    pub fn to_json(&self) -> io::Result<JsonValue> {
        match self {
            Self::Bool(value) => Ok((*value).into()),
            Self::Integer(value) => Ok((*value).into()),
            Self::Float(value) => serde_json::Number::from_f64(*value)
                .map(JsonValue::Number)
                .ok_or_else(|| invalid("A non-finite number cannot be kept in the mod profile.")),
            Self::Text(value) => Ok(value.clone().into()),
        }
    }

    /// A float stored as 1.3333333730697632 is shown as the mod's 1.3333334.
    pub fn format(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Integer(value) => value.to_string(),
            Self::Float(value) if !value.is_finite() => display_float(*value),
            Self::Float(value) if f64::from(*value as f32) == *value => (*value as f32).to_string(),
            Self::Float(value) => value.to_string(),
            Self::Text(value) => value.clone(),
        }
    }

    /// Settings backed by a game float treat its displayed short decimal as the same value.
    pub fn same_for_setting(&self, old: &Self) -> bool {
        match (self, old) {
            (Self::Float(value), Self::Float(old)) => value == old || *value as f32 == *old as f32,
            (Self::Float(value), Self::Integer(old)) => *value == *old as f64,
            _ => self == old,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreferenceEntry {
    pub key: String,
    pub kind: PreferenceKind,
    pub literal: String,
    pub value: Option<PreferenceValue>,
    pub description: Option<String>,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreferenceCategory {
    pub name: String,
    pub entries: Vec<PreferenceEntry>,
}

impl PreferenceCategory {
    pub fn entry(&self, key: &str) -> Option<&PreferenceEntry> {
        self.entries.iter().find(|entry| entry.key == key)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreferenceChange {
    pub category: String,
    pub key: String,
    pub value: PreferenceValue,
}

#[derive(Clone, Debug)]
pub struct MelonPreferences {
    pub text: String,
    pub categories: Vec<PreferenceCategory>,
    values: BTreeMap<(String, String), Range<usize>>,
}

impl MelonPreferences {
    pub fn category(&self, name: &str) -> Option<&PreferenceCategory> {
        self.categories.iter().find(|category| category.name == name)
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let text = file_ops::read::if_present(fs::read_to_string(path))?.unwrap_or_default();
        Self::parse(file_ops::text::strip_bom(&text))
    }

    pub fn parse(text: &str) -> io::Result<Self> {
        let document = Document::parse(text).map_err(|error| {
            let offset = error.span().map_or(0, |span| span.start.min(text.len()));
            let line = text.as_bytes()[..offset].iter().filter(|&&byte| byte == b'\n').count() + 1;
            invalid(format!("MelonPreferences.cfg is not valid TOML: {} (line {line}).", error.message()))
        })?;
        let mut tables = Vec::new();
        collect_tables(document.as_table(), &[], &mut tables);
        tables.sort_by_key(|(_, table)| table.position());
        let mut values = BTreeMap::new();
        let mut categories = Vec::new();
        for (name, table) in tables {
            let mut entries = Vec::new();
            let mut fields = table.get_values();
            fields.sort_by_key(|(_, value)| value.span().map(|span| span.start));
            for (keys, value) in fields {
                let key = keys.iter().map(|key| key.get()).collect::<Vec<_>>().join(".");
                let span = value.span().ok_or_else(|| invalid("MelonPreferences.cfg has a value without its source location."))?;
                let key_start = keys.first().and_then(|key| key.span()).map_or(span.start, |span| span.start);
                let (kind, read) = read_value(value);
                entries.push(PreferenceEntry {
                    key: key.clone(),
                    kind,
                    literal: text[span.clone()].into(),
                    value: read,
                    description: description(text, key_start),
                    comment: comment(text, span.end),
                });
                values.insert((name.clone(), key), span);
            }
            categories.push(PreferenceCategory { name, entries });
        }
        Ok(Self {
            text: text.into(),
            categories,
            values,
        })
    }

    pub fn with(&self, changes: &[PreferenceChange]) -> io::Result<String> {
        let mut edits = Vec::new();
        let mut seen = HashSet::new();
        for change in changes {
            let entry = self.category(&change.category).and_then(|category| category.entry(&change.key));
            let span = self.values.get(&(change.category.clone(), change.key.clone()));
            let (Some(entry), Some(span)) = (entry, span) else {
                return Err(invalid(format!("{}.{} is not in MelonPreferences.cfg.", change.category, change.key)));
            };
            // One edit per span, avoiding corruption if a caller accidentally repeats an entry.
            if !seen.insert((span.start, span.end)) {
                return Err(invalid(format!("{}.{} was changed more than once.", change.category, change.key)));
            }
            edits.push((span.clone(), literal(entry, &change.value)?));
        }
        edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
        let mut text = self.text.clone();
        for (span, literal) in edits {
            text.replace_range(span, &literal);
        }
        Ok(text)
    }

    pub fn save(&self, path: &Path, changes: &[PreferenceChange]) -> io::Result<()> {
        file_ops::atomic::write(path, self.with(changes)?.as_bytes())
    }
}

/// Explicit table headers are categories. Dotted keys remain entries in their enclosing header; arrays of tables
/// and root-level keys are left intact without becoming editable settings.
fn collect_tables<'a>(table: &'a Table, path: &[String], tables: &mut Vec<(String, &'a Table)>) {
    for (name, item) in table.iter() {
        if let Some(child) = item.as_table() {
            let mut path = path.to_vec();
            path.push(name.to_owned());
            if !child.is_implicit() && !child.is_dotted() {
                tables.push((path.join("."), child));
            }
            collect_tables(child, &path, tables);
        }
    }
}

fn read_value(value: &Value) -> (PreferenceKind, Option<PreferenceValue>) {
    match value {
        Value::Boolean(value) => (PreferenceKind::Switch, Some(PreferenceValue::Bool(*value.value()))),
        Value::Integer(value) => (PreferenceKind::WholeNumber, Some(PreferenceValue::Integer(*value.value()))),
        Value::Float(value) => (PreferenceKind::Number, Some(PreferenceValue::Float(*value.value()))),
        Value::String(value) => (PreferenceKind::Text, Some(PreferenceValue::Text(value.value().clone()))),
        _ => (PreferenceKind::Other, None),
    }
}

fn literal(entry: &PreferenceEntry, value: &PreferenceValue) -> io::Result<String> {
    if !entry.kind.accepts(value) {
        return Err(invalid(format!("{} holds {:?}, so {} can't be written to it.", entry.key, entry.kind, value.format())));
    }
    Ok(match value {
        PreferenceValue::Bool(value) => value.to_string(),
        PreferenceValue::Integer(value) if entry.kind == PreferenceKind::WholeNumber => value.to_string(),
        PreferenceValue::Integer(value) => float_literal(*value as f64),
        PreferenceValue::Float(value) => float_literal(*value),
        PreferenceValue::Text(value) => quote(value),
    })
}

fn float_literal(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() { "inf" } else { "-inf" }.into();
    }
    let text = value.to_string();
    if text.contains(['.', 'e', 'E']) { text } else { format!("{text}.0") }
}

fn display_float(value: f64) -> String {
    if value.is_nan() {
        "NaN"
    } else if value.is_sign_positive() {
        "Infinity"
    } else {
        "-Infinity"
    }
    .into()
}

fn quote(value: &str) -> String {
    let mut text = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => text.push_str("\\\""),
            '\\' => text.push_str("\\\\"),
            '\n' => text.push_str("\\n"),
            '\r' => text.push_str("\\r"),
            '\t' => text.push_str("\\t"),
            character if character < ' ' || character == '\u{7f}' => {
                use std::fmt::Write;
                write!(text, "\\u{:04X}", character as u32).expect("writing a string cannot fail");
            }
            character => text.push(character),
        }
    }
    text.push('"');
    text
}

fn description(text: &str, entry: usize) -> Option<String> {
    let line_start = text[..entry].rfind('\n').map_or(0, |at| at + 1);
    let mut lines = Vec::new();
    for line in text[..line_start].lines().rev() {
        let line = line.trim();
        if !line.starts_with('#') {
            break;
        }
        lines.push(line.trim_start_matches('#').trim());
    }
    lines.reverse();
    (!lines.is_empty()).then(|| lines.join(" "))
}

fn comment(text: &str, end: usize) -> Option<String> {
    let rest = text[end..].split('\n').next().unwrap_or_default().trim();
    rest.starts_with('#').then(|| rest.trim_start_matches('#').trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "[AbilityRanges]\n\
        # Draw search radii. F8 toggles in game.\n\
        Enabled = false\n\
        Duration = 0.5\n\n\
        [FlickerDreams]\n\
        # Multiplies Dreamslash's base reach.\n\
        ReachMultiplier = 1.3333333730697632\n\
        # Guaranteed extra dashes.\n\
        # 0 turns flicking off.\n\
        Flicks = 0\n\
        Blink = \"start\"\n\
        CameraSmoothing = 3.0 # applies at the next area\n\n\
        [\"Other Mod\"]\n\
        Colors = [ 1, 2, 3, ]\n\
        Label = \"say \\\"hi\\\"\"\n";

    fn change(category: &str, key: &str, value: PreferenceValue) -> PreferenceChange {
        PreferenceChange {
            category: category.into(),
            key: key.into(),
            value,
        }
    }

    #[test]
    fn preferences_read_types_order_comments_and_unknown_values() {
        let read = MelonPreferences::parse(FILE).unwrap();
        assert_eq!(
            read.categories.iter().map(|category| category.name.as_str()).collect::<Vec<_>>(),
            ["AbilityRanges", "FlickerDreams", "Other Mod"]
        );
        let flicker = read.category("FlickerDreams").unwrap();
        assert_eq!(
            flicker.entry("Flicks").unwrap().description.as_deref(),
            Some("Guaranteed extra dashes. 0 turns flicking off.")
        );
        assert_eq!(flicker.entry("Flicks").unwrap().value, Some(PreferenceValue::Integer(0)));
        assert_eq!(flicker.entry("CameraSmoothing").unwrap().comment.as_deref(), Some("applies at the next area"));
        assert_eq!(flicker.entry("CameraSmoothing").unwrap().description, None);
        let colors = read.category("Other Mod").unwrap().entry("Colors").unwrap();
        assert_eq!((colors.kind, colors.literal.as_str(), &colors.value), (PreferenceKind::Other, "[ 1, 2, 3, ]", &None));
        assert_eq!(
            read.category("Other Mod").unwrap().entry("Label").unwrap().value,
            Some(PreferenceValue::Text("say \"hi\"".into()))
        );
    }

    #[test]
    fn changes_replace_only_original_value_spans_in_crlf_text() {
        let text = FILE.replace('\n', "\r\n");
        let read = MelonPreferences::parse(&text).unwrap();
        let written = read
            .with(&[
                change("FlickerDreams", "ReachMultiplier", PreferenceValue::Float(1.5)),
                change("FlickerDreams", "Flicks", PreferenceValue::Integer(2)),
                change("FlickerDreams", "Blink", PreferenceValue::Text("end \\ \"quoted\"".into())),
                change("FlickerDreams", "CameraSmoothing", PreferenceValue::Integer(2)),
                change("AbilityRanges", "Enabled", PreferenceValue::Bool(true)),
            ])
            .unwrap();
        let expected = text
            .replace("ReachMultiplier = 1.3333333730697632", "ReachMultiplier = 1.5")
            .replace("Flicks = 0", "Flicks = 2")
            .replace("Blink = \"start\"", "Blink = \"end \\\\ \\\"quoted\\\"\"")
            .replace("CameraSmoothing = 3.0 #", "CameraSmoothing = 2.0 #")
            .replace("Enabled = false", "Enabled = true");
        assert_eq!(written, expected);
        assert_eq!(
            MelonPreferences::parse(&written)
                .unwrap()
                .category("FlickerDreams")
                .unwrap()
                .entry("CameraSmoothing")
                .unwrap()
                .value,
            Some(PreferenceValue::Float(2.0))
        );
        for change in [
            change("FlickerDreams", "Flicks", PreferenceValue::Float(1.5)),
            change("FlickerDreams", "Missing", PreferenceValue::Integer(1)),
            change("Other Mod", "Colors", PreferenceValue::Text("x".into())),
        ] {
            assert!(read.with(&[change]).is_err());
        }
    }

    #[test]
    fn unicode_and_multiline_values_do_not_displace_edits_or_comments() {
        let text = "[\"Réglages 🎯\"]\r\n# Durée du cercle — 円の時間 🎯\r\nDuration  =  0.5\r\nLabel = '''café 🎯\r\nsecond line''' # whole text\r\nCount = 1\r\n";
        let read = MelonPreferences::parse(text).unwrap();
        let written = read
            .with(&[
                change("Réglages 🎯", "Duration", PreferenceValue::Float(1.25)),
                change("Réglages 🎯", "Count", PreferenceValue::Integer(2)),
            ])
            .unwrap();
        assert_eq!(written, text.replace("0.5", "1.25").replace("Count = 1", "Count = 2"));
        assert_eq!(
            read.category("Réglages 🎯").unwrap().entry("Duration").unwrap().description.as_deref(),
            Some("Durée du cercle — 円の時間 🎯")
        );
        let written = read.with(&[change("Réglages 🎯", "Label", PreferenceValue::Text("line\n\t\u{7f}🎯".into()))]).unwrap();
        assert!(written.contains("Label = \"line\\n\\t\\u007F🎯\" # whole text\r\n"));
        assert!(MelonPreferences::parse(&written).is_ok());
    }

    #[test]
    fn dotted_entries_and_table_headers_keep_source_order_and_unknowns() {
        let text = "root = 1\n[a.b]\nquoted.\"key\" = 2\nDate = 2026-10-17\nMap = {x = 1}\n[a]\nlast = 3\n[[many]]\nuntouched = 4\n";
        let read = MelonPreferences::parse(text).unwrap();
        assert_eq!(read.categories.iter().map(|category| category.name.as_str()).collect::<Vec<_>>(), ["a.b", "a"]);
        assert_eq!(read.category("a.b").unwrap().entry("Date").unwrap().kind, PreferenceKind::Other);
        let written = read.with(&[change("a.b", "quoted.key", PreferenceValue::Integer(5))]).unwrap();
        assert_eq!(written, text.replace("quoted.\"key\" = 2", "quoted.\"key\" = 5"));
    }

    #[test]
    fn preference_file_replacement_is_atomic_and_parse_errors_name_the_line() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("MelonPreferences.cfg");
        assert!(MelonPreferences::load(&path).unwrap().categories.is_empty());
        fs::write(&path, FILE).unwrap();
        let read = MelonPreferences::load(&path).unwrap();
        read.save(&path, &[change("AbilityRanges", "Duration", PreferenceValue::Float(1.25))]).unwrap();
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        let blocked = temp.path().join("blocked.cfg");
        fs::create_dir(&blocked).unwrap();
        assert!(read.save(&blocked, &[change("AbilityRanges", "Enabled", PreferenceValue::Bool(true))]).is_err());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
        assert!(MelonPreferences::parse("[Broken\nKey = 1").unwrap_err().to_string().contains("line 1"));
    }

    #[test]
    fn json_preserves_types_and_kinds_accept_what_they_hold() {
        for value in [
            PreferenceValue::Bool(false),
            PreferenceValue::Integer(i64::MAX),
            PreferenceValue::Float(1.25),
            PreferenceValue::Text("café".into()),
        ] {
            assert_eq!(PreferenceValue::from_json(&value.to_json().unwrap()), Some(value));
        }
        assert!(PreferenceValue::Float(f64::NAN).to_json().is_err());
        assert_eq!(PreferenceValue::from_json(&serde_json::json!([])), None);
        assert!(PreferenceKind::Number.accepts(&PreferenceValue::Integer(1)));
        assert!(!PreferenceKind::WholeNumber.accepts(&PreferenceValue::Float(1.0)));
    }
}
