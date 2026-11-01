//! A game's text, from Unity Localization's string tables: one Addressables bundle per locale holding a
//! `StringTable` per collection (`Menus_en`), whose entries name their keys by ID in a `SharedTableData`
//! kept in a bundle shared by every locale. [`locales`] and [`read`] take the folder of Addressables bundles; every bundle
//! there is opened, as the tables' scripts are in the Addressables `MonoScript` bundle.

use crate::archive::{self, Archive};
use crate::value::Value;
use binary_reader::{Error, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub const STRING_TABLE: (&str, &str, &str) = ("Unity.Localization", "UnityEngine.Localization.Tables", "StringTable");
pub const SHARED_TABLE: (&str, &str, &str) = ("Unity.Localization", "UnityEngine.Localization.Tables", "SharedTableData");
/// Unity Localization's default Addressables group naming, one bundle per locale.
pub const STRING_TABLE_BUNDLES: &str = "localization-string-tables-";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Strings {
    pub locale: String,
    /// Collection (`Menus`) → key (`Menu_Title`) → text.
    pub tables: BTreeMap<String, BTreeMap<String, String>>,
    /// Entries whose ID no key names. Unity Localization looks text up by key, so the game cannot reach them by name
    /// either; the shipped tables keep them after their keys are deleted.
    pub unkeyed: usize,
    /// Entries whose key a later ID in the shared data also names. The later ID wins, as a dictionary filled in order
    /// would: its text is the newer one where they differ ("PH: Chasing Apophis" becomes "Chasing Apophis").
    pub shadowed: usize,
}

impl Strings {
    pub fn get(&self, collection: &str, key: &str) -> Option<&str> {
        self.tables.get(collection)?.get(key).map(String::as_str)
    }
}

/// What a `SharedTableData` says an ID is: the key it names, or shadowed by a later ID with the same key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Named(String),
    Shadowed,
}

/// A `SharedTableData`: its collection name and what each ID is. A key named twice belongs to its later ID.
pub fn shared_keys(shared: &Value) -> Result<(String, HashMap<i64, Key>)> {
    let name = shared.field("m_TableCollectionName", Value::as_string)?;
    let keys = shared
        .field("m_Entries", |entries| {
            let (mut keys, mut owner) = (HashMap::new(), HashMap::new());
            for entry in entries.as_array()? {
                let (id, key) = (entry.field("m_Id", Value::as_i64)?, entry.field("m_Key", Value::as_string)?);
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
pub struct Entries {
    pub keyed: BTreeMap<String, String>,
    pub unkeyed: usize,
    pub shadowed: usize,
}

/// A `StringTable`'s entries under their keys. An ID with two entries is refused.
pub fn table_entries(table: &Value, collection: &str, keys: &HashMap<i64, Key>) -> Result<Entries> {
    table
        .field("m_TableData", |entries| {
            let mut out = Entries {
                keyed: BTreeMap::new(),
                unkeyed: 0,
                shadowed: 0,
            };
            for entry in entries.as_array()? {
                let id = entry.field("m_Id", Value::as_i64)?;
                let text = entry.field("m_Localized", Value::as_string)?;
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

pub fn locale_code(table: &Value) -> Result<String> {
    table.field("m_LocaleId", |id| id.field("m_Code", Value::as_string))
}

/// The locale codes the bundles in `folder` hold text for (`en`, `de`, `es-ES`…), each with its bundle.
pub fn locales(folder: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut archive = Archive::open(archive::bundles_in(folder)?)?;
    Ok(locales_in(&mut archive)?
        .into_iter()
        .map(|(code, index)| (code, archive.bundles()[index].path().to_path_buf()))
        .collect())
}

/// The locales in the archive of every addressable bundle, each with its bundle's index: the folder is listed once,
/// so a bundle that comes or goes meanwhile (an update) cannot leave a name without its bundle.
fn locales_in(archive: &mut Archive) -> Result<BTreeMap<String, usize>> {
    let candidates: Vec<usize> = (0..archive.bundles().len())
        .filter(|&index| {
            let path = archive.bundles()[index].path();
            path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(STRING_TABLE_BUNDLES))
        })
        .collect();
    let mut locales = BTreeMap::new();
    for index in candidates {
        let path = archive.bundles()[index].path().to_path_buf();
        let file = archive.main_file(index)?;
        let tables = archive.behaviours(&file, None, STRING_TABLE, "m_LocaleId")?;
        let codes = tables.iter().map(|(_, head)| locale_code(head)).collect::<Result<BTreeSet<_>>>()?;
        match codes.into_iter().collect::<Vec<_>>().as_slice() {
            [code] => {
                if let Some(other) = locales.insert(code.clone(), index) {
                    return Err(Error::data(format!(
                        "locale {code} is in {} and {}",
                        archive.bundles()[other].path().display(),
                        path.display()
                    )));
                }
            }
            codes => return Err(Error::data(format!("{} holds tables for {} locales", path.display(), codes.len()))),
        }
    }
    Ok(locales)
}

/// Every string table for one locale code, from the bundles in `folder`.
pub fn read(folder: &Path, locale: &str) -> Result<Strings> {
    let mut archive = Archive::open(archive::bundles_in(folder)?)?;
    let index = locales_in(&mut archive)?
        .remove(locale)
        .ok_or_else(|| Error::data(format!("the game ships no text for locale {locale}")))?;
    let file = archive.main_file(index)?;
    let (mut tables, mut unkeyed, mut shadowed) = (BTreeMap::new(), 0, 0);
    for (object, _) in archive.behaviours(&file, None, STRING_TABLE, "m_Name")? {
        let table = file.decode(&object)?;
        let (shared_file, shared_object) = archive.resolve(&file, table.field("m_SharedData", Value::as_pptr)?)?;
        let shared = shared_file.decode(&shared_object)?;
        if !archive.script(&shared_file, &shared)?.is(SHARED_TABLE.0, SHARED_TABLE.1, SHARED_TABLE.2) {
            return Err(Error::data("m_SharedData is not a SharedTableData"));
        }
        let (collection, keys) = shared_keys(&shared)?;
        let entries = table_entries(&table, &collection, &keys)?;
        (unkeyed, shadowed) = (unkeyed + entries.unkeyed, shadowed + entries.shadowed);
        if tables.insert(collection.clone(), entries.keyed).is_some() {
            return Err(Error::data(format!("two {locale} tables for {collection}")));
        }
    }
    Ok(Strings {
        locale: locale.to_owned(),
        tables,
        unkeyed,
        shadowed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{BundleBuilder, SerializedBuilder, TreeBuilder, Writer, object};

    /// What a behaviour starts with: no game object, enabled, and its script by its path ID in `CAB-scripts`.
    fn behaviour_head(script: i64, name: &str) -> Writer {
        let mut w = Writer::default();
        w.i32(0).i64(0).u8(1).align().i32(1).i64(script).string(name);
        w
    }

    fn behaviour_tree(type_name: &str) -> TreeBuilder {
        let mut tree = TreeBuilder::new(type_name, "Base");
        tree.pptr("m_GameObject").field("UInt8", "m_Enabled", 1, 0x4000).pptr("m_Script").string("m_Name");
        tree
    }

    /// A table of the one collection: its locale code and its entries' IDs and texts.
    type Table = (&'static str, &'static [(i64, &'static str)]);

    /// A folder of Addressables: the scripts, one collection's shared data (IDs 1 and 2) and the given bundles by name, each
    /// with its tables.
    fn addressables(bundles: &[(&str, &[Table])]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let mut script_tree = TreeBuilder::new("MonoScript", "Base");
        script_tree.string("m_Name").string("m_ClassName").string("m_Namespace").string("m_AssemblyName");
        let script = |(assembly, namespace, class): (&str, &str, &str)| {
            let mut w = Writer::default();
            w.string(class).string(class).string(namespace).string(assembly);
            w.bytes
        };
        let scripts = SerializedBuilder::new()
            .mono_script(&script_tree.build())
            .object(1, 0, &script(STRING_TABLE))
            .object(2, 0, &script(SHARED_TABLE))
            .build();
        BundleBuilder::new().file("CAB-scripts", &scripts).write(&directory.path().join("monoscripts.bundle"));

        let mut shared_tree = behaviour_tree("MonoBehaviour");
        shared_tree
            .string("m_TableCollectionName")
            .open_vector("m_Entries", "SharedTableEntry")
            .field("SInt64", "m_Id", 8, 0)
            .string("m_Key")
            .close_vector();
        let mut w = behaviour_head(2, "Menus Shared Data");
        w.string("Menus").i32(2).i64(1).string("Title").i64(2).string("Quit");
        let shared = SerializedBuilder::new()
            .external("archive:/CAB-scripts/CAB-scripts")
            .mono_behaviour(&shared_tree.build())
            .object(1, 0, &w.bytes)
            .build();
        BundleBuilder::new().file("CAB-shared", &shared).write(&directory.path().join("shared.bundle"));

        let mut table_tree = behaviour_tree("MonoBehaviour");
        table_tree
            .open_class("LocaleIdentifier", "m_LocaleId")
            .string("m_Code")
            .close_class()
            .pptr("m_SharedData")
            .open_vector("m_TableData", "TableEntryData")
            .field("SInt64", "m_Id", 8, 0)
            .string("m_Localized")
            .close_vector();
        let table_tree = table_tree.build();
        for (index, (bundle, tables)) in bundles.iter().enumerate() {
            let mut data = SerializedBuilder::new()
                .external("archive:/CAB-scripts/CAB-scripts")
                .external("archive:/CAB-shared/CAB-shared")
                .mono_behaviour(&table_tree);
            for (at, (code, entries)) in tables.iter().enumerate() {
                let mut w = behaviour_head(1, &format!("Menus_{code}"));
                w.string(code).i32(2).i64(1).i32(entries.len() as i32);
                for (id, text) in *entries {
                    w.i64(*id).string(text);
                }
                data = data.object(at as i64 + 1, 0, &w.bytes);
            }
            let cab = format!("CAB-table-{index}");
            BundleBuilder::new().file(&cab, &data.build()).write(&directory.path().join(bundle));
        }
        directory
    }

    const ENGLISH: &[Table] = &[("en", &[(1, "Eternity"), (2, "Leave"), (9, "gone")])];
    const GERMAN: &[Table] = &[("de", &[(1, "Ewigkeit")])];

    #[test]
    fn locales_are_found_by_their_tables_and_read_through_the_shared_keys() {
        let directory = addressables(&[("localization-string-tables-english.bundle", ENGLISH), ("localization-string-tables-german.bundle", GERMAN)]);
        let found = locales(directory.path()).unwrap();
        assert_eq!(found.keys().map(String::as_str).collect::<Vec<_>>(), ["de", "en"]);
        assert_eq!(found["en"], directory.path().join("localization-string-tables-english.bundle"));

        let english = read(directory.path(), "en").unwrap();
        assert_eq!((english.locale.as_str(), english.get("Menus", "Quit")), ("en", Some("Leave")));
        assert_eq!((english.unkeyed, english.shadowed), (1, 0));
        assert_eq!(read(directory.path(), "de").unwrap().get("Menus", "Title"), Some("Ewigkeit"));
        assert!(read(directory.path(), "xx").unwrap_err().to_string().contains("no text for locale xx"));
    }

    #[test]
    fn a_bundle_by_any_other_name_is_not_a_locale() {
        let directory = addressables(&[("other-assets.bundle", ENGLISH), ("localization-string-tables-german.bundle", GERMAN)]);
        assert_eq!(locales(directory.path()).unwrap().into_keys().collect::<Vec<_>>(), ["de"]);
    }

    #[test]
    fn a_locale_in_two_bundles_or_two_locales_in_one_are_refused() {
        let twice = addressables(&[("localization-string-tables-a.bundle", ENGLISH), ("localization-string-tables-b.bundle", ENGLISH)]);
        assert!(locales(twice.path()).unwrap_err().to_string().contains("locale en is in"));
        let both: &[Table] = &[("en", &[(1, "Eternity")]), ("de", &[(1, "Ewigkeit")])];
        let mixed = addressables(&[("localization-string-tables-a.bundle", both)]);
        assert!(locales(mixed.path()).unwrap_err().to_string().contains("tables for 2 locales"));
    }

    #[test]
    fn two_tables_for_one_collection_are_refused() {
        let both: &[Table] = &[("en", &[(1, "Eternity")]), ("en", &[(2, "Leave")])];
        let directory = addressables(&[("localization-string-tables-a.bundle", both)]);
        assert!(read(directory.path(), "en").unwrap_err().to_string().contains("two en tables for Menus"));
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
