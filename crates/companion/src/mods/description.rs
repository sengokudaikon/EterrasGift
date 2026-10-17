//! A mod's embedded `ModSettings.json`, read as data without loading its assembly. The optional description owns
//! each category it names and orders the settings the page shows; entries it omits are old settings, not guesses.

use serde_json::{Map, Value};
use std::collections::HashSet;
use std::io;

pub const RESOURCE_NAME: &str = "ModSettings.json";
pub const MAX_RESOURCE_LENGTH: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingChoice {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingDescription {
    pub key: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub group: Option<String>,
    pub unit: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub choices: Option<Vec<SettingChoice>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CategoryDescription {
    pub name: String,
    pub settings: Vec<SettingDescription>,
}

impl CategoryDescription {
    pub fn setting(&self, key: &str) -> Option<&SettingDescription> {
        self.settings.iter().find(|setting| setting.key == key)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModSettingsDescription {
    pub categories: Vec<CategoryDescription>,
}

impl ModSettingsDescription {
    pub fn category(&self, name: &str) -> Option<&CategoryDescription> {
        self.categories.iter().find(|category| category.name == name)
    }

    /// The resource reader checks its PE bounds; this boundary checks the resource's size and JSON text.
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_RESOURCE_LENGTH {
            return Err(invalid("The settings description is larger than 1 MB."));
        }
        let text = std::str::from_utf8(bytes).map_err(|error| invalid(format!("The settings description is not UTF-8: {error}")))?;
        Self::parse(text.trim_start_matches('\u{feff}'))
    }

    pub fn parse(text: &str) -> io::Result<Self> {
        let json = json_with_comments(text)?;
        let value: Value = serde_json::from_slice(&json).map_err(|error| invalid(format!("It is not valid JSON: {error}")))?;
        let root = object(&value, "The description")?;
        let version = root
            .get("schemaVersion")
            .filter(|value| value.is_number())
            .ok_or_else(|| invalid("It has no schemaVersion."))?;
        if version.as_i64() != Some(1) {
            return Err(invalid(format!("It is written for format {version}; this app reads format 1.")));
        }
        let categories = array(root, "categories", "The description")?.iter().map(category).collect::<io::Result<_>>()?;
        Ok(Self { categories })
    }
}

fn category(value: &Value) -> io::Result<CategoryDescription> {
    let root = object(value, "A category")?;
    let name = text(root, "name", "A category")?.ok_or_else(|| invalid("A category has no name."))?;
    let settings = array(root, "settings", &format!("Category {name}"))?
        .iter()
        .map(|value| setting(value, &name))
        .collect::<io::Result<Vec<_>>>()?;
    let mut keys = HashSet::new();
    for setting in &settings {
        if !keys.insert(&setting.key) {
            return Err(invalid(format!("Category {name} describes {} twice.", setting.key)));
        }
    }
    Ok(CategoryDescription { name, settings })
}

fn setting(value: &Value, category: &str) -> io::Result<SettingDescription> {
    let root = object(value, &format!("A setting in {category}"))?;
    let key = text(root, "key", &format!("A setting in {category}"))?.ok_or_else(|| invalid(format!("A setting in {category} has no key.")))?;
    let location = format!("{category}.{key}");
    let min = number(root, "min", &location)?;
    let max = number(root, "max", &location)?;
    if min.zip(max).is_some_and(|(min, max)| min > max) {
        return Err(invalid(format!("{location} has a min above its max.")));
    }
    let choices = if root.contains_key("choices") {
        let choices = array(root, "choices", &location)?
            .iter()
            .map(|value| choice(value, &location))
            .collect::<io::Result<Vec<_>>>()?;
        if choices.is_empty() {
            return Err(invalid(format!("{location} has no choices in its choices.")));
        }
        Some(choices)
    } else {
        None
    };
    Ok(SettingDescription {
        key,
        label: text(root, "label", &location)?,
        description: text(root, "description", &location)?,
        group: text(root, "group", &location)?,
        unit: text(root, "unit", &location)?,
        min,
        max,
        choices,
    })
}

fn choice(value: &Value, location: &str) -> io::Result<SettingChoice> {
    let root = object(value, &format!("A choice of {location}"))?;
    let value = match root.get("value") {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) if value.as_i64().is_some() => value.as_i64().unwrap().to_string(),
        Some(_) => return Err(invalid(format!("A choice of {location} has a value that is neither text nor a whole number."))),
        None => return Err(invalid(format!("A choice of {location} has no value."))),
    };
    let label = text(root, "label", location)?.unwrap_or_else(|| value.clone());
    Ok(SettingChoice { value, label })
}

fn object<'a>(value: &'a Value, location: &str) -> io::Result<&'a Map<String, Value>> {
    value.as_object().ok_or_else(|| invalid(format!("{location} is not a JSON object.")))
}

fn array<'a>(root: &'a Map<String, Value>, name: &str, location: &str) -> io::Result<&'a Vec<Value>> {
    root.get(name).and_then(Value::as_array).ok_or_else(|| invalid(format!("{location} has no {name} list.")))
}

fn text(root: &Map<String, Value>, name: &str, location: &str) -> io::Result<Option<String>> {
    match root.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.is_empty() => Ok(Some(text.clone())),
        _ => Err(invalid(format!("{location} has a {name} that is not text."))),
    }
}

fn number(root: &Map<String, Value>, name: &str, location: &str) -> io::Result<Option<f64>> {
    match root.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_f64().map(Some).ok_or_else(|| invalid(format!("{location} has a {name} that is not a number."))),
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// System.Text.Json allows comments and trailing commas here. Replace only those extensions with whitespace,
/// leaving strings alone and keeping line/column diagnostics meaningful for the JSON parser.
fn json_with_comments(text: &str) -> io::Result<Vec<u8>> {
    let mut bytes = text.as_bytes().to_vec();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => at = string_end(&bytes, at),
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                while at < bytes.len() && !matches!(bytes[at], b'\r' | b'\n') {
                    bytes[at] = b' ';
                    at += 1;
                }
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                let start = at;
                at += 2;
                while at + 1 < bytes.len() && &bytes[at..at + 2] != b"*/" {
                    at += 1;
                }
                if at + 1 == bytes.len() || at == bytes.len() {
                    return Err(invalid("It is not valid JSON: an unfinished comment."));
                }
                at += 2;
                for byte in &mut bytes[start..at] {
                    if !matches!(byte, b'\r' | b'\n') {
                        *byte = b' ';
                    }
                }
            }
            _ => at += 1,
        }
    }
    at = 0;
    while at < bytes.len() {
        if bytes[at] == b'"' {
            at = string_end(&bytes, at);
        } else {
            if bytes[at] == b','
                && bytes[..at]
                    .iter()
                    .rev()
                    .find(|byte| !byte.is_ascii_whitespace())
                    .is_some_and(|byte| !matches!(byte, b'[' | b'{' | b',' | b':'))
                && bytes[at + 1..]
                    .iter()
                    .find(|byte| !byte.is_ascii_whitespace())
                    .is_some_and(|byte| matches!(byte, b']' | b'}'))
            {
                bytes[at] = b' ';
            }
            at += 1;
        }
    }
    Ok(bytes)
}

fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut at = start + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at = (at + 2).min(bytes.len()),
            b'"' => return at + 1,
            _ => at += 1,
        }
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_keep_order_ranges_and_choices_with_json_comments() {
        let json = br#"{
            // A hand-written description may have comments and trailing commas.
            "schemaVersion": 1, "categories": [{ "name": "Lantern", "settings": [
                {"key":"Style", "label":"Light style", "group":"Look", "choices":[{"value":"warm","label":"Warm"},{"value":2},]},
                {"key":"Radius", "description":"https://example.test/a/*b*/", "unit":"m", "min":1, "max":20},
            ]}], /* keep strings intact */
        }"#;
        let mut bytes = b"\xef\xbb\xbf".to_vec();
        bytes.extend_from_slice(json);
        let read = ModSettingsDescription::from_bytes(&bytes).unwrap();
        let category = read.category("Lantern").unwrap();
        assert_eq!(category.settings.iter().map(|setting| setting.key.as_str()).collect::<Vec<_>>(), ["Style", "Radius"]);
        let style = category.setting("Style").unwrap();
        assert_eq!(style.group.as_deref(), Some("Look"));
        assert_eq!(
            style.choices.as_ref().unwrap()[1],
            SettingChoice {
                value: "2".into(),
                label: "2".into()
            }
        );
        let radius = category.setting("Radius").unwrap();
        assert_eq!((radius.min, radius.max, radius.unit.as_deref()), (Some(1.0), Some(20.0), Some("m")));
        assert_eq!(radius.description.as_deref(), Some("https://example.test/a/*b*/"));
    }

    #[test]
    fn malformed_descriptions_say_why() {
        for (json, message) in [
            ("[]", "The description is not a JSON object."),
            (r#"{"categories":[]}"#, "It has no schemaVersion."),
            (r#"{"schemaVersion":2,"categories":[]}"#, "It is written for format 2; this app reads format 1."),
            (r#"{"schemaVersion":1}"#, "The description has no categories list."),
        ] {
            assert_eq!(ModSettingsDescription::parse(json).unwrap_err().to_string(), message);
        }
        for (settings, message) in [
            (r#"{"key":"B"},{"key":"B"}"#, "Category A describes B twice."),
            (r#"{"key":"B","min":3,"max":1}"#, "A.B has a min above its max."),
            (r#"{"key":"B","min":"low"}"#, "A.B has a min that is not a number."),
            (r#"{"key":"B","choices":[]}"#, "A.B has no choices in its choices."),
            (
                r#"{"key":"B","choices":[{"value":1.5}]}"#,
                "A choice of A.B has a value that is neither text nor a whole number.",
            ),
        ] {
            let json = format!(r#"{{"schemaVersion":1,"categories":[{{"name":"A","settings":[{settings}]}}]}}"#);
            assert_eq!(ModSettingsDescription::parse(&json).unwrap_err().to_string(), message);
        }
        assert!(ModSettingsDescription::parse("{/*").is_err());
        assert!(ModSettingsDescription::parse(r#"{"schemaVersion":1,"categories":[,]}"#).is_err());
        assert!(ModSettingsDescription::parse(r#"{"schemaVersion":1,"categories":[], ,}"#).is_err());
        assert!(ModSettingsDescription::from_bytes(&[0xff]).is_err());
        assert!(ModSettingsDescription::from_bytes(&vec![b' '; MAX_RESOURCE_LENGTH + 1]).is_err());
    }
}
