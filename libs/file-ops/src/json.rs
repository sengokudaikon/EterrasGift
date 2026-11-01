//! JSON from files: records read whole, objects whose keys people may type by hand (a known key is matched whatever its
//! ASCII case), and the comments and trailing commas System.Text.Json lets people leave in.

use crate::read;
use crate::text;
use serde_json::{Map, Value};
use std::fmt;
use std::io;
use std::path::Path;

/// The value of `key`, matched whatever its case; the first spelling when there are several.
pub fn find<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    object.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value)
}

/// Removes `key`, matched whatever its case, and returns its value; only the first spelling when there are several.
pub fn take(object: &mut Map<String, Value>, key: &str) -> Option<Value> {
    let name = object.keys().find(|name| name.eq_ignore_ascii_case(key))?.clone();
    object.remove(&name)
}

/// Removes `key` under every spelling.
pub fn take_all(object: &mut Map<String, Value>, key: &str) {
    while take(object, key).is_some() {}
}

/// What to do with a byte-order mark at the start of a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bom {
    /// The file may come from an editor or from .NET.
    Strip,
    /// Only this program writes the file, so a mark is damage like any other.
    Refuse,
}

/// Why a record could not be loaded. Shows as the error it holds.
#[derive(Debug)]
pub enum LoadError {
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Json(error) => error.fmt(f),
        }
    }
}

/// The JSON document in the file at `path`, `None` when no file is there. What it must hold, its version among it, is the
/// caller's to check: records differ in how a version is spelled and what a wrong one means.
pub fn load(path: &Path, bom: Bom) -> Result<Option<Value>, LoadError> {
    let Some(bytes) = read::optional(path).map_err(LoadError::Io)? else {
        return Ok(None);
    };
    let bytes = match bom {
        Bom::Strip => text::strip_bom_bytes(&bytes),
        Bom::Refuse => &bytes,
    };
    serde_json::from_slice(bytes).map(Some).map_err(LoadError::Json)
}

/// A `/*` comment with no end, which [`with_comments`] cannot turn into JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnfinishedComment;

/// JSON from text that may have comments and trailing commas. Replaces only those extensions with whitespace, leaving
/// strings alone and keeping the parser's line and column numbers true to the text.
pub fn with_comments(text: &str) -> Result<Vec<u8>, UnfinishedComment> {
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
                    return Err(UnfinishedComment);
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
    fn keys_match_whatever_their_case_and_take_removes_one_spelling_at_a_time() {
        let mut object = serde_json::json!({ "AutoConnect": true, "autoconnect": false }).as_object().unwrap().clone();
        assert_eq!(find(&object, "autoConnect"), Some(&Value::Bool(true)));
        assert_eq!(take(&mut object, "AUTOCONNECT"), Some(Value::Bool(true)));
        assert_eq!(take(&mut object, "autoConnect"), Some(Value::Bool(false)));
        assert_eq!(take(&mut object, "autoConnect"), None);
    }

    #[test]
    fn take_all_leaves_no_spelling_and_the_other_keys() {
        let mut object = serde_json::json!({ "AutoConnect": true, "name": 1, "autoconnect": false, "AUTOCONNECT": 0 })
            .as_object()
            .unwrap()
            .clone();
        take_all(&mut object, "autoConnect");
        assert_eq!(Value::Object(object), serde_json::json!({ "name": 1 }));
    }

    #[test]
    fn a_record_is_missing_damaged_or_read_and_only_strip_forgives_a_mark() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("record.json");
        assert!(matches!(load(&path, Bom::Refuse), Ok(None)));
        std::fs::write(&path, br#"{"version":1}"#).unwrap();
        assert_eq!(load(&path, Bom::Refuse).unwrap().unwrap()["version"], 1);
        std::fs::write(&path, b"\xEF\xBB\xBF[1]").unwrap();
        assert_eq!(load(&path, Bom::Strip).unwrap().unwrap()[0], 1);
        assert!(matches!(load(&path, Bom::Refuse), Err(LoadError::Json(_))));
        std::fs::write(&path, "{").unwrap();
        let error = load(&path, Bom::Strip).unwrap_err();
        assert!(matches!(error, LoadError::Json(_)));
        assert!(error.to_string().contains("EOF"), "{error}");
        assert!(matches!(load(temp.path(), Bom::Strip), Err(LoadError::Io(_))));
    }

    #[test]
    fn comments_and_trailing_commas_become_whitespace_and_strings_stay() {
        let text = "{ // line\n \"a\": \"//x /*y*/ ,]\", /* block\n more */ \"b\": [1, 2, ],\n}";
        let json = with_comments(text).unwrap();
        assert_eq!(json.len(), text.len());
        assert_eq!(json.iter().filter(|&&byte| byte == b'\n').count(), 3);
        let value: Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(value, serde_json::json!({ "a": "//x /*y*/ ,]", "b": [1, 2] }));
        assert_eq!(with_comments("{} /* open"), Err(UnfinishedComment));
        assert_eq!(with_comments("{} /*"), Err(UnfinishedComment));
        // A comma with nothing before it is left for the parser to refuse.
        assert!(serde_json::from_slice::<Value>(&with_comments("[,]").unwrap()).is_err());
    }
}
