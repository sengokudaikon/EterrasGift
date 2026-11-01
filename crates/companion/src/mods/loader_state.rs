//! MelonLoader's own preparation records and its narrow generated-CoreModule repair.
//! These routines operate on files only, and callers run repair only while the game is stopped.

use super::assembly::{LoaderVersion, loader_version, read_view, rows};
use super::invalid;
use crate::game::install::GameInstall;
use crate::launch::loader::melon_paths;
use dotscope::CilAssemblyView;
use dotscope::metadata::tables::{NestedClassRaw, TableDataOwned, TableId, TypeDefRaw};
use dotscope::metadata::token::Token;
use file_ops::atomic;
use file_ops::hashes::digest_unchanged;
use file_ops::stamp::FileStamp;
use sha2::Sha512;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use toml_edit::DocumentMut;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoaderPreparation {
    Prepared,
    WillPrepare,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoaderInstall {
    pub root: PathBuf,
    pub version: LoaderVersion,
}

impl LoaderInstall {
    pub fn find(root: &Path) -> Option<Self> {
        ["net6", "net35"].into_iter().find_map(|framework| {
            loader_version(&melon_paths::loader(root).join(framework).join("MelonLoader.dll"))
                .ok()
                .map(|version| Self { root: root.into(), version })
        })
    }

    /// Hashing a game assembly is deliberately kept off the UI thread.
    pub fn preparation_for(&self, game: &GameInstall) -> LoaderPreparation {
        let Some(record) = read_generator_record(&self.root) else {
            return LoaderPreparation::WillPrepare;
        };
        let Some(recorded) = record.game_assembly_hash else {
            return LoaderPreparation::WillPrepare;
        };
        if forces_regeneration(&self.root) {
            return LoaderPreparation::WillPrepare;
        }
        match game_assembly_hash(&game.game_assembly()) {
            Ok(hash) if hash == recorded && game.read_unity_version().is_none_or(|unity| unity == record.unity_version) => LoaderPreparation::Prepared,
            _ => LoaderPreparation::WillPrepare,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratorRecord {
    pub game_assembly_hash: Option<String>,
    pub unity_version: String,
}

pub fn read_generator_record(root: &Path) -> Option<GeneratorRecord> {
    let text = fs::read_to_string(melon_paths::loader(root).join("Dependencies/Il2CppAssemblyGenerator/Config.cfg")).ok()?;
    let document: DocumentMut = file_ops::text::strip_bom(&text).parse().ok()?;
    let generator = document.get("Il2CppAssemblyGenerator")?.as_table_like()?;
    Some(GeneratorRecord {
        game_assembly_hash: generator.get("GameAssemblyHash").and_then(|item| item.as_str()).map(str::to_owned),
        unity_version: generator.get("UnityVersion").and_then(|item| item.as_str()).unwrap_or("0.0.0.0").into(),
    })
}

fn forces_regeneration(root: &Path) -> bool {
    let Ok(text) = fs::read_to_string(melon_paths::user_data(root).join("Loader.cfg")) else {
        return false;
    };
    let Ok(document) = file_ops::text::strip_bom(&text).parse::<DocumentMut>() else {
        return false;
    };
    document
        .get("unityengine")
        .and_then(|item| item.as_table_like())
        .and_then(|table| table.get("force_regeneration"))
        .and_then(|item| item.as_bool())
        == Some(true)
}

#[derive(Clone)]
struct CachedHash {
    stamp: FileStamp,
    hash: String,
}
static HASHES: LazyLock<Mutex<HashMap<PathBuf, CachedHash>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn game_assembly_hash(path: &Path) -> io::Result<String> {
    let key = fs::canonicalize(path)?;
    let stamp = FileStamp::of(path)?;
    if let Some(known) = HASHES.lock().unwrap().get(&key)
        && known.stamp == stamp
    {
        return Ok(known.hash.clone());
    }
    // Do not cache an observation that changed while being read.
    let Some(hash) = digest_unchanged::<Sha512>(path, &stamp)? else {
        return Err(io::Error::other("GameAssembly.dll changed while its build was being checked."));
    };
    HASHES.lock().unwrap().insert(key, CachedHash { stamp, hash: hash.clone() });
    Ok(hash)
}

pub fn core_module_path(root: &Path) -> PathBuf {
    melon_paths::loader(root).join("Il2CppAssemblies/UnityEngine.CoreModule.dll")
}

pub fn needs_repair(root: &Path) -> bool {
    // The Mods page asks every two seconds while it follows a start; the DLL is read again only once it has changed.
    /// The DLL checked last, its stamp then, and the answer.
    type Checked = (PathBuf, FileStamp, bool);
    static CHECKED: Mutex<Option<Checked>> = Mutex::new(None);
    let path = core_module_path(root);
    let stamp = FileStamp::of(&path).ok();
    let mut checked = CHECKED.lock().unwrap_or_else(|p| p.into_inner());
    if let (Some(stamp), Some((known, at, answer))) = (stamp, checked.as_ref())
        && *known == path
        && *at == stamp
    {
        return *answer;
    }
    let answer = core_module_unsorted(&path);
    *checked = stamp.map(|stamp| (path, stamp, answer));
    answer
}

fn core_module_unsorted(path: &Path) -> bool {
    let Ok(view) = read_view(path) else {
        return false;
    };
    let Ok(nested) = rows::<NestedClassRaw>(&view) else {
        return false;
    };
    !nested.windows(2).all(|pair| pair[0].nested_class < pair[1].nested_class) || duplicates(&view, &nested).is_ok_and(|names| !names.is_empty())
}

/// Groups types using the loader's sorted-table lookup, so unsorted parent mappings expose the same false duplicates.
pub fn loader_duplicates(path: &Path) -> io::Result<Vec<String>> {
    let view = read_view(path)?;
    duplicates(&view, &rows::<NestedClassRaw>(&view)?)
}

fn duplicates(view: &CilAssemblyView, nested: &[NestedClassRaw]) -> io::Result<Vec<String>> {
    let strings = view.strings().ok_or_else(|| invalid("Missing string heap."))?;
    let types = rows::<TypeDefRaw>(view)?;
    let mut seen = BTreeMap::<String, usize>::new();
    for ty in &types {
        let name = strings.get(ty.type_name as usize).map_err(invalid)?;
        let key = if let Ok(index) = nested.binary_search_by_key(&ty.rid, |row| row.nested_class) {
            let parent = nested[index].enclosing_class;
            let declaring = types.iter().find(|ty| ty.rid == parent).ok_or_else(|| invalid("Missing enclosing type."))?;
            format!("{}#{parent}/{name}", strings.get(declaring.type_name as usize).map_err(invalid)?)
        } else {
            format!("{}.{name}", strings.get(ty.type_namespace as usize).map_err(invalid)?)
        };
        *seen.entry(key).or_default() += 1;
    }
    Ok(seen.into_iter().filter(|(_, count)| *count > 1).map(|(name, count)| format!("{name} x{count}")).collect())
}

/// Sorts NestedClass and renames genuine duplicate names without dropping any type or member.
/// Unsupported orphan/cyclic relationships fail before either the DLL or its backup is changed.
/// A clean assembly is left byte-for-byte unchanged, including an earlier backup.
pub fn repair_core_module(root: &Path) -> io::Result<usize> {
    let path = core_module_path(root);
    let original = fs::read(&path)?;
    let view = CilAssemblyView::from_mem(original.clone()).map_err(invalid)?;
    let (repaired, renamed) = repaired_bytes(view)?;
    let Some(repaired) = repaired else {
        return Ok(0);
    };
    atomic::replace(&path, |temporary| {
        fs::write(temporary, &repaired)?;
        // Validate the bytes actually written, before preserving the original and replacing it.
        let check = read_view(temporary)?;
        let nested = rows::<NestedClassRaw>(&check)?;
        validate_nesting(&rows::<TypeDefRaw>(&check)?, &nested)?;
        if !nested.windows(2).all(|pair| pair[0].nested_class < pair[1].nested_class) || !duplicates(&check, &nested)?.is_empty() {
            return Err(invalid("UnityEngine.CoreModule.dll still has duplicate types after a repair, so it was left as it was."));
        }
        if fs::read(&path)? != original {
            return Err(io::Error::other("UnityEngine.CoreModule.dll changed during its repair; it was left as it was."));
        }
        atomic::write(&path.with_extension("dll.bak"), &original)
    })?;
    Ok(renamed)
}

fn validate_nesting(types: &[TypeDefRaw], nested: &[NestedClassRaw]) -> io::Result<HashMap<u32, u32>> {
    let rids: HashSet<_> = types.iter().map(|row| row.rid).collect();
    let mut parents = HashMap::new();
    for row in nested {
        if !rids.contains(&row.nested_class) || !rids.contains(&row.enclosing_class) || parents.insert(row.nested_class, row.enclosing_class).is_some() {
            return Err(invalid("CoreModule contains invalid or duplicate nested-type relationships; it was left unchanged."));
        }
    }
    for ty in types {
        let nested_visibility = ty.flags & 7 >= 2;
        if nested_visibility != parents.contains_key(&ty.rid) {
            return Err(invalid(
                "CoreModule contains a nested type without a valid enclosing relationship. Repair would change its identity, so the DLL was left unchanged.",
            ));
        }
        let mut visited = HashSet::new();
        let mut current = ty.rid;
        while let Some(parent) = parents.get(&current) {
            if !visited.insert(current) {
                return Err(invalid("CoreModule contains cyclic nested types; it was left unchanged."));
            }
            current = *parent;
        }
    }
    Ok(parents)
}

fn repaired_bytes(view: CilAssemblyView) -> io::Result<(Option<Vec<u8>>, usize)> {
    let types = rows::<TypeDefRaw>(&view)?;
    let mut nested = rows::<NestedClassRaw>(&view)?;
    let parents = validate_nesting(&types, &nested)?;
    if view.cor20header().strong_name_signature_size != 0 {
        return Err(invalid(
            "The generated CoreModule is signed; repairing it would invalidate its signature, so it was left unchanged.",
        ));
    }
    let strings = view.strings().ok_or_else(|| invalid("Missing string heap."))?;
    let mut named = Vec::new();
    for row in &types {
        let parent = parents.get(&row.rid).copied().unwrap_or(0);
        let namespace = if parent == 0 {
            strings.get(row.type_namespace as usize).map_err(invalid)?.to_owned()
        } else {
            String::new()
        };
        named.push((row.clone(), (parent, namespace), strings.get(row.type_name as usize).map_err(invalid)?.to_owned()));
    }
    let mut reserved: HashSet<_> = named.iter().map(|(_, scope, name)| (scope.clone(), name.clone())).collect();
    let mut seen = HashSet::new();
    let mut renames = HashMap::new();
    for (row, scope, name) in &named {
        if !seen.insert((scope.clone(), name.clone())) {
            let mut suffix = 2_u32;
            loop {
                let candidate = format!("{name}_{suffix}");
                if reserved.insert((scope.clone(), candidate.clone())) {
                    renames.insert(row.rid, candidate);
                    break;
                }
                suffix = suffix.checked_add(1).ok_or_else(|| invalid("Duplicate suffix overflow."))?;
            }
        }
    }
    let sorted = nested.windows(2).all(|pair| pair[0].nested_class < pair[1].nested_class);
    if sorted && renames.is_empty() {
        return Ok((None, 0));
    }
    let mut assembly = view.to_owned();
    for (mut row, _, _) in named {
        if let Some(name) = renames.get(&row.rid) {
            row.type_name = assembly.string_add(name).map_err(invalid)?.placeholder();
            assembly.table_row_update(TableId::TypeDef, row.rid, TableDataOwned::TypeDef(row)).map_err(invalid)?;
        }
    }
    nested.sort_by_key(|row| row.nested_class);
    for (index, mut row) in nested.iter().cloned().enumerate() {
        let rid = u32::try_from(index + 1).map_err(invalid)?;
        row.rid = rid;
        row.token = Token::from_parts(TableId::NestedClass, rid);
        assembly.table_row_update(TableId::NestedClass, rid, TableDataOwned::NestedClass(row)).map_err(invalid)?;
    }
    let bytes = assembly.to_memory().map_err(invalid)?;
    let after = CilAssemblyView::from_mem(bytes.clone()).map_err(invalid)?;
    let after_types = rows::<TypeDefRaw>(&after)?;
    let after_nested = rows::<NestedClassRaw>(&after)?;
    if validate_nesting(&after_types, &after_nested)? != parents || after_types.len() != types.len() {
        return Err(invalid("CoreModule repair changed its type inventory; the DLL was left unchanged."));
    }
    let after_strings = after.strings().ok_or_else(|| invalid("Missing output string heap."))?;
    let before_strings = assembly.view().strings().ok_or_else(|| invalid("Missing input string heap."))?;
    for (before, after) in types.iter().zip(&after_types) {
        let expected = renames
            .get(&before.rid)
            .map(String::as_str)
            .unwrap_or(before_strings.get(before.type_name as usize).map_err(invalid)?);
        if before.rid != after.rid
            || before.flags != after.flags
            || before.field_list != after.field_list
            || before.method_list != after.method_list
            || (before.extends.tag, before.extends.row) != (after.extends.tag, after.extends.row)
            || before_strings.get(before.type_namespace as usize).map_err(invalid)? != after_strings.get(after.type_namespace as usize).map_err(invalid)?
            || expected != after_strings.get(after.type_name as usize).map_err(invalid)?
        {
            return Err(invalid("CoreModule repair changed an unexpected type field; the DLL was left unchanged."));
        }
    }
    Ok((Some(bytes), renames.len()))
}
