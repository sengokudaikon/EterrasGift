//! The player data Unity writes without type trees (`resources.assets`, with its scripts in
//! `globalgamemanagers.assets`), read with layouts rebuilt from the IL2CPP build. Each object must decode with its
//! layout exactly, so a layout the game no longer fits is refused rather than misread.

use crate::archive::Archive;
use crate::error::{Error, Result};
use crate::il2cpp::Il2Cpp;
use crate::layout::{Layouts, mono_behaviour_header};
use crate::serialized::{MONO_BEHAVIOUR, MONO_SCRIPT, SerializedFile};
use crate::typetree::Node;
use crate::value::Value;
use std::path::Path;

pub(crate) struct PlayerData {
    resources: SerializedFile,
    managers: SerializedFile,
    /// `MonoScript`'s layout, as the bundles ship it (the same Unity build wrote both).
    script_tree: Node,
    il2cpp: Il2Cpp,
}

fn open(path: &Path) -> Result<SerializedFile> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    SerializedFile::parse(bytes).map_err(|e| e.context(path.display()))
}

impl PlayerData {
    pub(crate) fn open(game: &Path, data: &Path) -> Result<PlayerData> {
        let mut scripts = Archive::open([data.join("StreamingAssets").join("LEAssetBundles").join("monoscripts.bundle")])?;
        let file = scripts.main_file(0)?;
        let script = file
            .objects()
            .iter()
            .find(|o| o.class_id == MONO_SCRIPT)
            .ok_or_else(|| Error::data("monoscripts.bundle holds no MonoScript"))?;
        let script_tree = (**file.tree(script)?).clone();
        Ok(PlayerData {
            resources: open(&data.join("resources.assets"))?,
            managers: open(&data.join("globalgamemanagers.assets"))?,
            script_tree,
            il2cpp: Il2Cpp::open(&game.join("GameAssembly.dll"), &data.join("il2cpp_data").join("Metadata").join("global-metadata.dat"))?,
        })
    }

    /// The one MonoBehaviour in `resources.assets` named `name` whose script is `assembly`'s `namespace.class`,
    /// decoded whole with the class's rebuilt layout.
    pub(crate) fn behaviour(&self, name: &str, (assembly, namespace, class): (&str, &str, &str)) -> Result<Value> {
        let header = mono_behaviour_header();
        let mut found = Vec::new();
        for object in self.resources.objects().iter().filter(|o| o.class_id == MONO_BEHAVIOUR) {
            let head = self.resources.decode_prefix_with(object, &header, "m_Name")?;
            if head.field("m_Name", Value::as_str)? != name {
                continue;
            }
            let (file_id, path_id) = head.field("m_Script", Value::as_pptr)?;
            let external = self
                .resources
                .external(file_id)?
                .ok_or_else(|| Error::data(format!("{name}'s script is not in globalgamemanagers.assets")))?;
            if external.file_name() != "globalgamemanagers.assets" {
                return Err(Error::data(format!("{name}'s script is in {}", external.path)));
            }
            let script = self
                .managers
                .object(path_id)
                .filter(|o| o.class_id == MONO_SCRIPT)
                .ok_or_else(|| Error::data(format!("{name}'s script {path_id} is not a MonoScript")))?;
            let script = self.managers.decode_with(script, &self.script_tree)?;
            let names = |field| script.field(field, |v| v.as_str().map(str::to_owned));
            if (names("m_AssemblyName")?, names("m_Namespace")?, names("m_ClassName")?) == (assembly.to_owned(), namespace.to_owned(), class.to_owned()) {
                found.push(*object);
            }
        }
        let [object] = found.as_slice() else {
            return Err(Error::data(format!("{} {class} objects named {name} in resources.assets, not one", found.len())));
        };
        let layout = Layouts::new(&self.il2cpp).mono_behaviour(assembly, namespace, class)?;
        self.resources.decode_with(object, &layout).map_err(|e| e.context(name))
    }
}
