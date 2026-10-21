//! The game's text, from Unity Localization's string tables: one Addressables bundle per locale holding a
//! `StringTable` per collection (`Item_Affixes_en`), whose entries name their keys by ID in a `SharedTableData`
//! kept in a bundle shared by every locale.

use crate::error::{Error, Result};
use crate::value::Value;
use std::collections::{BTreeMap, HashMap};

pub(crate) const STRING_TABLE: (&str, &str, &str) = ("Unity.Localization", "UnityEngine.Localization.Tables", "StringTable");
pub(crate) const SHARED_TABLE: (&str, &str, &str) = ("Unity.Localization", "UnityEngine.Localization.Tables", "SharedTableData");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Strings {
    pub locale: String,
    /// Collection (`Item_Affixes`) → key (`Item_Affix_25_DisplayName`) → text.
    pub tables: BTreeMap<String, BTreeMap<String, String>>,
    /// Entries whose ID no key names. Unity Localization looks text up by key, so the game cannot reach them by name
    /// either; the shipped tables keep them after their keys are deleted (about a sixth of them in 2026-10).
    pub unkeyed: usize,
    /// Entries whose key a later ID in the shared data also names (176 keys in 2026-10, 39 with different text). The
    /// later ID wins, as a dictionary filled in order would: its text is the newer one where they differ ("PH: Chasing
    /// Apophis" becomes "Chasing Apophis"). Not yet checked against the game's own lookup.
    pub shadowed: usize,
}

impl Strings {
    pub fn get(&self, collection: &str, key: &str) -> Option<&str> {
        self.tables.get(collection)?.get(key).map(String::as_str)
    }
}

/// What a `SharedTableData` says an ID is: the key it names, or shadowed by a later ID with the same key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Named(String),
    Shadowed,
}

/// A `SharedTableData`: its collection name and what each ID is. A key named twice belongs to its later ID.
pub(crate) fn shared_keys(shared: &Value) -> Result<(String, HashMap<i64, Key>)> {
    let name = shared.field("m_TableCollectionName", |v| v.as_str().map(str::to_owned))?;
    let keys = shared
        .field("m_Entries", |entries| {
            let (mut keys, mut owner) = (HashMap::new(), HashMap::new());
            for entry in entries.as_array()? {
                let (id, key) = (entry.field("m_Id", Value::as_i64)?, entry.field("m_Key", |v| v.as_str().map(str::to_owned))?);
                if keys.contains_key(&id) {
                    return Err(Error::data(format!("ID {id} appears twice")));
                }
                if let Some(earlier) = owner.insert(key.clone(), id) {
                    keys.insert(earlier, Key::Shadowed);
                }
                keys.insert(id, Key::Named(key));
            }
            Ok(keys)
        })
        .map_err(|e| e.context(&name))?;
    Ok((name, keys))
}

/// A table's entries under their keys, with the counts of those left out.
#[derive(Debug)]
pub(crate) struct Entries {
    pub(crate) keyed: BTreeMap<String, String>,
    pub(crate) unkeyed: usize,
    pub(crate) shadowed: usize,
}

/// A `StringTable`'s entries under their keys. An ID with two entries is refused.
pub(crate) fn table_entries(table: &Value, collection: &str, keys: &HashMap<i64, Key>) -> Result<Entries> {
    table
        .field("m_TableData", |entries| {
            let mut out = Entries {
                keyed: BTreeMap::new(),
                unkeyed: 0,
                shadowed: 0,
            };
            for entry in entries.as_array()? {
                let id = entry.field("m_Id", Value::as_i64)?;
                let text = entry.field("m_Localized", |v| v.as_str().map(str::to_owned))?;
                match keys.get(&id) {
                    Some(Key::Named(key)) => {
                        if out.keyed.insert(key.clone(), text).is_some() {
                            return Err(Error::data(format!("ID {id} ({key}) has two entries")));
                        }
                    }
                    Some(Key::Shadowed) => out.shadowed += 1,
                    None => out.unkeyed += 1,
                }
            }
            Ok(out)
        })
        .map_err(|e| e.context(collection))
}

pub(crate) fn locale_code(table: &Value) -> Result<String> {
    table.field("m_LocaleId", |id| id.field("m_Code", |v| v.as_str().map(str::to_owned)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    #[test]
    fn entries_take_their_keys_from_the_shared_data() {
        let key = |id, key: &str| object(vec![("m_Id", Value::Int(id)), ("m_Key", Value::String(key.into()))]);
        let shared = object(vec![
            ("m_TableCollectionName", Value::String("Quests".into())),
            (
                "m_Entries",
                Value::Array(vec![key(9, "Quest_147_Name"), key(11, "Quest_1_Name"), key(12, "Quest_147_Name")]),
            ),
        ]);
        let (name, keys) = shared_keys(&shared).unwrap();
        assert_eq!(name, "Quests");
        let entry = |id, text: &str| object(vec![("m_Id", Value::Int(id)), ("m_Localized", Value::String(text.into()))]);
        let table = object(vec![(
            "m_TableData",
            Value::Array(vec![entry(9, "PH: Chasing Apophis"), entry(10, "gone"), entry(12, "Chasing Apophis")]),
        )]);
        let entries = table_entries(&table, &name, &keys).unwrap();
        assert_eq!(entries.keyed["Quest_147_Name"], "Chasing Apophis", "the later ID owns a key named twice");
        assert_eq!((entries.keyed.len(), entries.unkeyed, entries.shadowed), (1, 1, 1));
        let twice = object(vec![("m_TableData", Value::Array(vec![entry(11, "a"), entry(11, "b")]))]);
        assert!(table_entries(&twice, &name, &keys).unwrap_err().to_string().contains("two entries"));
        let repeated = object(vec![
            ("m_TableCollectionName", Value::String("Quests".into())),
            ("m_Entries", Value::Array(vec![key(9, "a"), key(9, "b")])),
        ]);
        assert!(shared_keys(&repeated).unwrap_err().to_string().contains("ID 9 appears twice"));
    }
}
