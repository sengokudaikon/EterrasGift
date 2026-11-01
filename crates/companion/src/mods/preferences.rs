//! MelonPreferences.cfg is TOML. Edits replace only original value spans, preserving comments, order, spacing,
//! unknown entries and line endings. The library applies drafts just before a start, while the game is closed.

use super::description::{SettingChoice, SettingDescription};
use super::invalid;
use phrases::phrases;
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::ops::Range;
use std::path::Path;
use toml_edit::{Document, Table, Value};

phrases! {
    EnterWhole: "Enter a whole number, like 2."
    EnterNumber: "Enter a number, with a dot for decimals (like 1.5)."
    WholeBetween: "Enter a whole number from {min} to {max}."
    WholeAtLeast: "Enter a whole number of {min} or more."
    WholeAtMost: "Enter a whole number of {max} or less."
    NumberBetween: "Enter a number from {min} to {max}."
    NumberAtLeast: "Enter a number of {min} or more."
    NumberAtMost: "Enter a number of {max} or less."
    WasOn: "Was on"
    WasOff: "Was off"
    Was: "Was {value}"
}

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
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(file_ops::text::strip_bom(&text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::parse(""),
            Err(error) => Err(error),
        }
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

/// The editable state of one setting. A text box may hold an invalid draft; committing it removes a previously
/// saved draft, so the game never gets an older value that the box no longer displays.
#[derive(Clone, Debug)]
pub struct ModSetting {
    pub category: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub kind: PreferenceKind,
    pub unit: Option<String>,
    pub group: Option<String>,
    pub choices: Vec<SettingChoice>,
    pub is_on: bool,
    pub value: String,
    was: Option<PreferenceValue>,
    min: Option<f64>,
    max: Option<f64>,
}

impl ModSetting {
    pub fn new(category: &str, entry: &PreferenceEntry, draft: Option<&PreferenceValue>, described: Option<&SettingDescription>) -> Self {
        let was = entry.value.clone();
        let current = draft.filter(|value| entry.kind.accepts(value)).or(was.as_ref());
        let mut choices = if matches!(entry.kind, PreferenceKind::Text | PreferenceKind::WholeNumber) {
            described.and_then(|description| description.choices.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };
        if !choices.is_empty() {
            for value in [was.as_ref(), current].into_iter().flatten().map(PreferenceValue::format) {
                if !value.is_empty() && !choices.iter().any(|choice| choice.value == value) {
                    choices.push(SettingChoice { label: value.clone(), value });
                }
            }
        }
        let number = matches!(entry.kind, PreferenceKind::WholeNumber | PreferenceKind::Number);
        Self {
            category: category.into(),
            key: entry.key.clone(),
            name: described.and_then(|description| description.label.clone()).unwrap_or_else(|| words(&entry.key)),
            description: described
                .and_then(|description| description.description.clone())
                .or_else(|| entry.description.clone())
                .or_else(|| entry.comment.clone())
                .unwrap_or_default(),
            kind: entry.kind,
            unit: described.and_then(|description| description.unit.clone()),
            group: described.and_then(|description| description.group.clone()),
            choices,
            is_on: current == Some(&PreferenceValue::Bool(true)),
            value: current.map(PreferenceValue::format).unwrap_or_default(),
            was,
            min: described.filter(|_| number).and_then(|description| description.min),
            max: described.filter(|_| number).and_then(|description| description.max),
        }
    }

    pub fn typed(&self) -> Option<PreferenceValue> {
        match self.kind {
            PreferenceKind::Switch => Some(PreferenceValue::Bool(self.is_on)),
            PreferenceKind::WholeNumber | PreferenceKind::Number => self.number().filter(|number| self.in_range(number) || self.same_as_before(number)),
            PreferenceKind::Text => Some(PreferenceValue::Text(self.value.clone())),
            PreferenceKind::Other => None,
        }
    }

    pub fn is_drafted(&self) -> bool {
        self.typed().is_none_or(|value| !self.same_as_before(&value))
    }

    /// None means remove the saved draft, for unchanged and invalid inputs alike.
    pub fn draft(&self) -> Option<PreferenceValue> {
        self.typed().filter(|value| !self.same_as_before(value))
    }

    pub fn is_choice(&self) -> bool {
        !self.choices.is_empty()
    }

    pub fn error(&self) -> Option<String> {
        if self.typed().is_some() {
            return None;
        }
        let whole = self.kind == PreferenceKind::WholeNumber;
        if self.number().is_none() {
            return Some(if whole { EnterWhole } else { EnterNumber }.into());
        }
        let format = |value| PreferenceValue::Float(value).format();
        match (self.min, self.max) {
            (Some(min), Some(max)) => Some(if whole { WholeBetween } else { NumberBetween }.with(&[("min", &format(min)), ("max", &format(max))])),
            (Some(min), None) => Some(if whole { WholeAtLeast } else { NumberAtLeast }.with(&[("min", &format(min))])),
            (None, Some(max)) => Some(if whole { WholeAtMost } else { NumberAtMost }.with(&[("max", &format(max))])),
            _ => None,
        }
    }

    pub fn was_text(&self) -> String {
        if self.kind == PreferenceKind::Switch {
            return if self.was == Some(PreferenceValue::Bool(true)) { WasOn } else { WasOff }.into();
        }
        let value = self.was.as_ref().map(PreferenceValue::format).unwrap_or_default();
        let label = self
            .choices
            .iter()
            .find(|choice| choice.value == value)
            .map_or(value.as_str(), |choice| choice.label.as_str());
        Was.with(&[("value", &label)])
    }

    pub fn revert(&mut self) {
        self.is_on = self.was == Some(PreferenceValue::Bool(true));
        self.value = self.was.as_ref().map(PreferenceValue::format).unwrap_or_default();
    }

    fn same_as_before(&self, value: &PreferenceValue) -> bool {
        self.was.as_ref().is_some_and(|old| value.same_for_setting(old))
    }

    fn number(&self) -> Option<PreferenceValue> {
        let text = self.value.trim();
        match self.kind {
            PreferenceKind::WholeNumber => text.parse().ok().map(PreferenceValue::Integer),
            PreferenceKind::Number => text.parse().ok().map(PreferenceValue::Float),
            _ => None,
        }
    }

    fn in_range(&self, number: &PreferenceValue) -> bool {
        let value = match number {
            PreferenceValue::Integer(value) => *value as f64,
            PreferenceValue::Float(value) => *value,
            _ => return false,
        };
        !self.min.is_some_and(|min| value < min) && !self.max.is_some_and(|max| value > max)
    }
}

fn words(key: &str) -> String {
    let mut text = String::new();
    let mut previous = None;
    for character in key.chars() {
        if character.is_uppercase() && previous.is_some_and(|previous: char| !previous.is_uppercase()) {
            text.push(' ');
            text.extend(character.to_lowercase());
        } else {
            text.push(character);
        }
        previous = Some(character);
    }
    text
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
    fn settings_preserve_float_equivalence_and_invalid_edits_clear_the_saved_draft() {
        let read = MelonPreferences::parse(FILE).unwrap();
        let entry = read.category("FlickerDreams").unwrap().entry("ReachMultiplier").unwrap();
        let description = SettingDescription {
            key: entry.key.clone(),
            min: Some(1.0),
            max: Some(2.0),
            ..Default::default()
        };
        let mut setting = ModSetting::new("FlickerDreams", entry, None, Some(&description));
        assert_eq!(setting.value, "1.3333334");
        assert_eq!(setting.name, "Reach multiplier");
        assert!(!setting.is_drafted());
        setting.value = "1.5".into();
        assert_eq!(setting.draft(), Some(PreferenceValue::Float(1.5)));
        setting.value = "3".into();
        assert_eq!(setting.error().as_deref(), Some("Enter a number from 1 to 2."));
        assert_eq!(setting.draft(), None);
        setting.value = "1,5".into();
        assert!(setting.error().unwrap().contains("dot for decimals"));
        assert!(setting.is_drafted());
        setting.revert();
        assert!(!setting.is_drafted());
        // A value already in the file stays valid even when a later description narrows the range.
        let description = SettingDescription { max: Some(1.0), ..description };
        assert!(ModSetting::new("FlickerDreams", entry, None, Some(&description)).error().is_none());
    }

    #[test]
    fn choices_preserve_unlisted_old_and_draft_values_and_json_preserves_types() {
        let read = MelonPreferences::parse(FILE).unwrap();
        let entry = read.category("FlickerDreams").unwrap().entry("Blink").unwrap();
        let description = SettingDescription {
            key: entry.key.clone(),
            choices: Some(vec![SettingChoice {
                value: "off".into(),
                label: "Off".into(),
            }]),
            ..Default::default()
        };
        let mut setting = ModSetting::new("FlickerDreams", entry, Some(&PreferenceValue::Text("end".into())), Some(&description));
        assert_eq!(setting.choices.iter().map(|choice| choice.value.as_str()).collect::<Vec<_>>(), ["off", "start", "end"]);
        assert_eq!(setting.was_text(), "Was start");
        setting.revert();
        assert_eq!(setting.value, "start");
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

    #[test]
    fn toggle_and_integer_drafts_use_the_existing_type_and_revert_cleanly() {
        let read = MelonPreferences::parse(FILE).unwrap();
        let entry = read.category("AbilityRanges").unwrap().entry("Enabled").unwrap();
        let mut toggle = ModSetting::new("AbilityRanges", entry, Some(&PreferenceValue::Bool(true)), None);
        assert!(toggle.is_on && toggle.is_drafted());
        assert_eq!(toggle.was_text(), "Was off");
        toggle.revert();
        assert_eq!(toggle.draft(), None);
        let entry = read.category("FlickerDreams").unwrap().entry("Flicks").unwrap();
        let description = SettingDescription {
            key: "Flicks".into(),
            label: Some("Extra dashes".into()),
            description: Some("How many more.".into()),
            group: Some("Movement".into()),
            min: Some(0.0),
            ..Default::default()
        };
        let mut setting = ModSetting::new("FlickerDreams", entry, Some(&PreferenceValue::Integer(2)), Some(&description));
        assert_eq!(
            (setting.name.as_str(), setting.description.as_str(), setting.group.as_deref()),
            ("Extra dashes", "How many more.", Some("Movement"))
        );
        assert_eq!(setting.draft(), Some(PreferenceValue::Integer(2)));
        for invalid in ["2.5", "9223372036854775808", ""] {
            setting.value = invalid.into();
            assert_eq!(setting.error().as_deref(), Some("Enter a whole number, like 2."));
            assert_eq!(setting.draft(), None);
        }
        setting.value = "-1".into();
        assert_eq!(setting.error().as_deref(), Some("Enter a whole number of 0 or more."));
        setting.value = " +3 ".into();
        assert_eq!(setting.draft(), Some(PreferenceValue::Integer(3)));
        setting.revert();
        assert!(!setting.is_drafted());
    }
}
