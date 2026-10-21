//! A set of bundles read together, so references between them resolve: a `PPtr` into another bundle's file, and a
//! MonoBehaviour's script in the bundle that holds the `MonoScript`s.

use crate::bundle::Bundle;
use crate::error::{Error, Result};
use crate::serialized::{MONO_BEHAVIOUR, MONO_SCRIPT, Object, SerializedFile};
use crate::value::Value;
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

pub(crate) struct Archive {
    bundles: Vec<Bundle>,
    files: HashMap<String, Rc<SerializedFile>>,
    scripts: HashMap<(String, i64), Script>,
}

impl Archive {
    pub(crate) fn open(paths: impl IntoIterator<Item = PathBuf>) -> Result<Archive> {
        let bundles = paths.into_iter().map(Bundle::open).collect::<Result<_>>()?;
        Ok(Archive {
            bundles,
            files: HashMap::new(),
            scripts: HashMap::new(),
        })
    }

    pub(crate) fn bundles(&self) -> &[Bundle] {
        &self.bundles
    }

    /// The one serialized file a bundle holds.
    pub(crate) fn main_file(&mut self, bundle: usize) -> Result<Rc<SerializedFile>> {
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
    pub(crate) fn file(&mut self, name: &str) -> Result<Rc<SerializedFile>> {
        if let Some(file) = self.files.get(name) {
            return Ok(file.clone());
        }
        let bundle = self
            .bundles
            .iter()
            .find(|b| b.entry(name).is_some())
            .ok_or_else(|| Error::data(format!("no bundle here holds {name}")))?;
        let file = Rc::new(SerializedFile::parse(bundle.read(name)?).map_err(|e| e.context(format!("{}, {name}", bundle.path().display())))?);
        self.files.insert(name.to_owned(), file.clone());
        Ok(file)
    }

    /// The object a `PPtr` in `from` points at, and the file it lives in.
    pub(crate) fn resolve(&mut self, from: &Rc<SerializedFile>, (file_id, path_id): (i32, i64)) -> Result<(Rc<SerializedFile>, Object)> {
        let file = match from.external(file_id)? {
            None => from.clone(),
            Some(external) => {
                let name = external.file_name().to_owned();
                self.file(&name)?
            }
        };
        let object = *file
            .object(path_id)
            .ok_or_else(|| Error::data(format!("no object {path_id} in the file PPtr {file_id} names")))?;
        Ok((file, object))
    }

    /// The script of a decoded MonoBehaviour.
    pub(crate) fn script(&mut self, from: &Rc<SerializedFile>, behaviour: &Value) -> Result<Script> {
        let pointer = behaviour.field("m_Script", Value::as_pptr)?;
        let key = (from.external(pointer.0)?.map_or_else(String::new, |e| e.file_name().to_owned()), pointer.1);
        if let Some(script) = self.scripts.get(&key) {
            return Ok(script.clone());
        }
        let (file, object) = self.resolve(from, pointer)?;
        if object.class_id != MONO_SCRIPT {
            return Err(Error::data(format!("m_Script points at class {}, not a MonoScript", object.class_id)));
        }
        let value = file.decode(&object)?;
        let script = Script {
            assembly: value.field("m_AssemblyName", |v| v.as_str().map(str::to_owned))?,
            namespace: value.field("m_Namespace", |v| v.as_str().map(str::to_owned))?,
            class: value.field("m_ClassName", |v| v.as_str().map(str::to_owned))?,
        };
        self.scripts.insert(key, script.clone());
        Ok(script)
    }

    /// The MonoBehaviours in `file` with this script (and `m_Name`, if given), each with its fields through `last`
    /// decoded. Only the leading fields are read, so this is cheap even over a file of thousands of objects.
    pub(crate) fn behaviours(&mut self, file: &Rc<SerializedFile>, name: Option<&str>, script: (&str, &str, &str), last: &str) -> Result<Vec<(Object, Value)>> {
        let mut found = Vec::new();
        for object in file.objects().iter().filter(|o| o.class_id == MONO_BEHAVIOUR) {
            let head = file.decode_prefix(object, "m_Name")?;
            if name.is_some_and(|name| head.get("m_Name").and_then(Value::as_str).ok() != Some(name)) {
                continue;
            }
            if !self.script(file, &head)?.is(script.0, script.1, script.2) {
                continue;
            }
            let head = if last == "m_Name" { head } else { file.decode_prefix(object, last)? };
            found.push((*object, head));
        }
        Ok(found)
    }

    /// The one MonoBehaviour with this name and script, decoded whole. Any other count is refused.
    pub(crate) fn behaviour(&mut self, file: &Rc<SerializedFile>, name: &str, script: (&str, &str, &str)) -> Result<Value> {
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
            w.string(class).string(class).string("").string("LE");
            w.bytes
        };
        let scripts = SerializedBuilder::new()
            .mono_script(&script_tree.build())
            .object(1, 0, &script("AffixList"))
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
        let (_directory, mut archive) = archive(&[("MasterAffixesList", 2), ("MasterAffixesList", 1), ("Elsewhere", 1)]);
        let file = archive.main_file(0).unwrap();
        let value = archive.behaviour(&file, "MasterAffixesList", ("LE", "", "AffixList")).unwrap();
        assert_eq!(
            value
                .field("canRollOn", |v| v.as_array()?.iter().map(Value::int::<i32>).collect::<Result<Vec<_>>>())
                .unwrap(),
            [3, 1]
        );
        let found = archive.behaviours(&file, None, ("LE", "", "AffixList"), "m_Name").unwrap();
        assert_eq!(found.iter().map(|(o, _)| o.path_id).collect::<Vec<_>>(), [101, 102]);
        assert!(
            archive
                .behaviour(&file, "Missing", ("LE", "", "AffixList"))
                .unwrap_err()
                .to_string()
                .contains("0 AffixList objects")
        );
    }

    #[test]
    fn two_matches_are_refused() {
        let (_directory, mut archive) = archive(&[("MasterAffixesList", 1), ("MasterAffixesList", 1)]);
        let file = archive.main_file(0).unwrap();
        assert!(
            archive
                .behaviour(&file, "MasterAffixesList", ("LE", "", "AffixList"))
                .unwrap_err()
                .to_string()
                .contains("2 AffixList objects")
        );
    }

    #[test]
    fn a_script_outside_the_archive_is_an_error() {
        let (directory, _) = archive(&[("MasterAffixesList", 1)]);
        let mut archive = Archive::open([directory.path().join("data.bundle")]).unwrap();
        let file = archive.main_file(0).unwrap();
        assert!(
            archive
                .behaviour(&file, "MasterAffixesList", ("LE", "", "AffixList"))
                .unwrap_err()
                .to_string()
                .contains("no bundle here holds CAB-scripts")
        );
    }
}
