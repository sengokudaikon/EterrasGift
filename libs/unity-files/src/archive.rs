//! A set of bundles read together, so references between them resolve: a `PPtr` into another bundle's file, and a
//! MonoBehaviour's script in the bundle that holds the `MonoScript`s.

use crate::bundle::Bundle;
use crate::serialized::{MONO_BEHAVIOUR, MONO_SCRIPT, Object, SerializedFile, Source};
use crate::value::Value;
use binary_reader::{Error, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

/// The class a MonoBehaviour runs, as its `MonoScript` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    pub assembly: String,
    pub namespace: String,
    pub class: String,
}

impl Script {
    pub fn is(&self, assembly: &str, namespace: &str, class: &str) -> bool {
        self.assembly == assembly && self.namespace == namespace && self.class == class
    }
}

/// A serialized file inside a bundle; resource streams (`.resS`, `.resource`) are not.
fn is_serialized(name: &str) -> bool {
    !name.ends_with(".resS") && !name.ends_with(".resource")
}

/// A file's MonoBehaviours from one pass over them: a file holds tens of thousands, so each is kept small.
type Heads = Rc<Vec<Head>>;

struct Head {
    /// Its index in the file's objects.
    object: u32,
    /// Its name's hash, if it has a name: a match is then read again to compare the name itself.
    name: Option<u64>,
    /// Its `m_Script`, unless that does not read.
    script: Option<(i32, i64)>,
}

fn name_hash(name: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish()
}

pub struct Archive {
    bundles: Vec<Bundle>,
    files: HashMap<String, Rc<SerializedFile>>,
    /// By the file the `MonoScript` is in (its address: files stay open as long as the archive) and its path ID.
    scripts: HashMap<(usize, i64), Script>,
    heads: HashMap<usize, Heads>,
}

impl Archive {
    pub fn open(paths: impl IntoIterator<Item = PathBuf>) -> Result<Archive> {
        let bundles = paths.into_iter().map(Bundle::open).collect::<Result<_>>()?;
        Ok(Archive {
            bundles,
            files: HashMap::new(),
            scripts: HashMap::new(),
            heads: HashMap::new(),
        })
    }

    pub fn bundles(&self) -> &[Bundle] {
        &self.bundles
    }

    /// The one serialized file a bundle holds.
    pub fn main_file(&mut self, bundle: usize) -> Result<Rc<SerializedFile>> {
        let bundle = &self.bundles[bundle];
        let names: Vec<_> = bundle.entries().iter().map(|e| e.name.as_str()).filter(|name| is_serialized(name)).collect();
        match names.as_slice() {
            [name] => {
                let name = (*name).to_owned();
                self.file(&name)
            }
            _ => Err(Error::data(format!("{} holds {} serialized files, not one", bundle.path().display(), names.len()))),
        }
    }

    /// A serialized file by its entry name (`CAB-…`), from whichever bundle holds it.
    pub fn file(&mut self, name: &str) -> Result<Rc<SerializedFile>> {
        if let Some(file) = self.files.get(name) {
            return Ok(file.clone());
        }
        let bundle = self
            .bundles
            .iter()
            .find(|b| b.entry(name).is_some())
            .ok_or_else(|| Error::data(format!("no bundle here holds {name}")))?;
        let file = Rc::new(SerializedFile::open(Source::Bundle(bundle.stream(name)?)).map_err(|e| e.context(format!("{}, {name}", bundle.path().display())))?);
        self.files.insert(name.to_owned(), file.clone());
        Ok(file)
    }

    /// The object a `PPtr` in `from` points at, and the file it lives in.
    pub fn resolve(&mut self, from: &Rc<SerializedFile>, (file_id, path_id): (i32, i64)) -> Result<(Rc<SerializedFile>, Object)> {
        let file = match from.external(file_id)? {
            None => from.clone(),
            Some(external) => self.file(external.file_name())?,
        };
        let object = *file
            .object(path_id)
            .ok_or_else(|| Error::data(format!("no object {path_id} in the file PPtr {file_id} names")))?;
        Ok((file, object))
    }

    /// The script of a decoded MonoBehaviour.
    pub fn script(&mut self, from: &Rc<SerializedFile>, behaviour: &Value) -> Result<Script> {
        self.script_at(from, behaviour.field("m_Script", Value::as_pptr)?)
    }

    /// The script an `m_Script` in `from` points at.
    fn script_at(&mut self, from: &Rc<SerializedFile>, pointer: (i32, i64)) -> Result<Script> {
        let (file, object) = self.resolve(from, pointer)?;
        let key = (Rc::as_ptr(&file) as usize, pointer.1);
        if let Some(script) = self.scripts.get(&key) {
            return Ok(script.clone());
        }
        if object.class_id != MONO_SCRIPT {
            return Err(Error::data(format!("m_Script points at class {}, not a MonoScript", object.class_id)));
        }
        let value = file.decode(&object)?;
        let script = Script {
            assembly: value.field("m_AssemblyName", Value::as_string)?,
            namespace: value.field("m_Namespace", Value::as_string)?,
            class: value.field("m_ClassName", Value::as_string)?,
        };
        self.scripts.insert(key, script.clone());
        Ok(script)
    }

    /// The MonoBehaviours in `file` with this script (and `m_Name`, if given), each with its fields through `last`
    /// decoded. Only the leading fields are read, so this is cheap even over a file of thousands of objects.
    pub fn behaviours(&mut self, file: &Rc<SerializedFile>, name: Option<&str>, script: (&str, &str, &str), last: &str) -> Result<Vec<(Object, Value)>> {
        let mut found = Vec::new();
        let wanted = name.map(name_hash);
        for head in self.heads(file)?.iter() {
            if wanted.is_some_and(|wanted| head.name != Some(wanted)) {
                continue;
            }
            let object = &file.objects()[head.object as usize];
            // A name is compared in full; without one, only a behaviour whose script matches is decoded again.
            let mut leading = None;
            if let Some(name) = name {
                let value = leading.insert(file.decode_prefix(object, "m_Name")?);
                if value.get("m_Name").and_then(Value::as_str).ok() != Some(name) {
                    continue;
                }
            }
            // One with no readable `m_Script` says why when it is asked about.
            let pointer = match head.script {
                Some(pointer) => pointer,
                None => file.decode_prefix(object, "m_Name")?.field("m_Script", Value::as_pptr)?,
            };
            if self.script_at(file, pointer)?.is(script.0, script.1, script.2) {
                let value = match leading {
                    Some(leading) if last == "m_Name" => leading,
                    _ => file.decode_prefix(object, last)?,
                };
                found.push((*object, value));
            }
        }
        Ok(found)
    }

    /// Every MonoBehaviour's name and script, read once a file: a lookup by name reads no other object again.
    fn heads(&mut self, file: &Rc<SerializedFile>) -> Result<Heads> {
        if let Some(heads) = self.heads.get(&(Rc::as_ptr(file) as usize)) {
            return Ok(heads.clone());
        }
        let mut heads = Vec::with_capacity(file.objects().iter().filter(|o| o.class_id == MONO_BEHAVIOUR).count());
        for (index, object) in file.objects().iter().enumerate().filter(|(_, o)| o.class_id == MONO_BEHAVIOUR) {
            let leading = file.decode_prefix(object, "m_Name")?;
            heads.push(Head {
                object: u32::try_from(index).map_err(|_| Error::unsupported("more than 4 billion objects"))?,
                name: leading.get("m_Name").and_then(Value::as_str).ok().map(name_hash),
                script: leading.field("m_Script", Value::as_pptr).ok(),
            });
        }
        let heads = Rc::new(heads);
        self.heads.insert(Rc::as_ptr(file) as usize, heads.clone());
        Ok(heads)
    }

    /// The one MonoBehaviour with this name and script, decoded whole. Any other count is refused.
    pub fn behaviour(&mut self, file: &Rc<SerializedFile>, name: &str, script: (&str, &str, &str)) -> Result<Value> {
        match self.behaviours(file, Some(name), script, "m_Name")?.as_slice() {
            [(object, _)] => file.decode(object),
            found => Err(Error::data(format!("{} {} objects named {name}, not one", found.len(), script.2))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{BundleBuilder, SerializedBuilder, TreeBuilder, Writer};

    /// A scripts bundle with two `MonoScript`s, and a data bundle whose behaviours run them.
    fn archive(names: &[(&str, i64)]) -> (tempfile::TempDir, Archive) {
        let directory = tempfile::tempdir().unwrap();
        let mut script_tree = TreeBuilder::new("MonoScript", "Base");
        script_tree.string("m_Name").string("m_ClassName").string("m_Namespace").string("m_AssemblyName");
        let script = |class: &str| {
            let mut w = Writer::default();
            w.string(class).string(class).string("").string("Game");
            w.bytes
        };
        let scripts = SerializedBuilder::new()
            .mono_script(&script_tree.build())
            .object(1, 0, &script("ThingList"))
            .object(2, 0, &script("Other"))
            .build();
        BundleBuilder::new().file("CAB-scripts", &scripts).write(&directory.path().join("scripts.bundle"));

        let mut tree = TreeBuilder::new("MonoBehaviour", "Base");
        tree.pptr("m_GameObject")
            .field("UInt8", "m_Enabled", 1, 0x4000)
            .pptr("m_Script")
            .string("m_Name")
            .vector_of("canRollOn", "int", 4);
        let tree = tree.build();
        let mut data = SerializedBuilder::new().external("archive:/CAB-scripts/CAB-scripts").mono_behaviour(&tree);
        for (index, (name, script)) in names.iter().enumerate() {
            let mut w = Writer::default();
            w.i32(0).i64(0).u8(1).align().i32(1).i64(*script).string(name).i32(2).i32(3).i32(index as i32);
            data = data.object(100 + index as i64, 0, &w.bytes);
        }
        BundleBuilder::new()
            .file("CAB-data", &data.build())
            .file("CAB-data.resS", b"texture bytes")
            .write(&directory.path().join("data.bundle"));
        let archive = Archive::open([directory.path().join("data.bundle"), directory.path().join("scripts.bundle")]).unwrap();
        (directory, archive)
    }

    #[test]
    fn finds_a_behaviour_by_name_and_by_its_script_in_another_bundle() {
        let (_directory, mut archive) = archive(&[("MasterList", 2), ("MasterList", 1), ("Elsewhere", 1)]);
        let file = archive.main_file(0).unwrap();
        let value = archive.behaviour(&file, "MasterList", ("Game", "", "ThingList")).unwrap();
        assert_eq!(
            value
                .field("canRollOn", |v| v.as_array()?.iter().map(Value::int::<i32>).collect::<Result<Vec<_>>>())
                .unwrap(),
            [3, 1]
        );
        let found = archive.behaviours(&file, None, ("Game", "", "ThingList"), "m_Name").unwrap();
        assert_eq!(found.iter().map(|(o, _)| o.path_id).collect::<Vec<_>>(), [101, 102]);
        assert!(
            archive
                .behaviour(&file, "Missing", ("Game", "", "ThingList"))
                .unwrap_err()
                .to_string()
                .contains("0 ThingList objects")
        );
    }

    #[test]
    fn two_matches_are_refused() {
        let (_directory, mut archive) = archive(&[("MasterList", 1), ("MasterList", 1)]);
        let file = archive.main_file(0).unwrap();
        assert!(
            archive
                .behaviour(&file, "MasterList", ("Game", "", "ThingList"))
                .unwrap_err()
                .to_string()
                .contains("2 ThingList objects")
        );
    }

    #[test]
    fn a_script_outside_the_archive_is_an_error() {
        let (directory, _) = archive(&[("MasterList", 1)]);
        let mut archive = Archive::open([directory.path().join("data.bundle")]).unwrap();
        let file = archive.main_file(0).unwrap();
        assert!(
            archive
                .behaviour(&file, "MasterList", ("Game", "", "ThingList"))
                .unwrap_err()
                .to_string()
                .contains("no bundle here holds CAB-scripts")
        );
    }
}
