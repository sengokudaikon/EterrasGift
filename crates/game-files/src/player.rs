//! The player data Unity writes without type trees (`resources.assets`, with its scripts in
//! `globalgamemanagers.assets`), read with layouts rebuilt from the IL2CPP build. Each object must decode with its
//! layout exactly, so a layout the game no longer fits is refused rather than misread.

use crate::archive::Archive;
use crate::error::{Error, Result};
use crate::il2cpp::Il2Cpp;
use crate::layout::{Layouts, mono_behaviour_header};
use crate::serialized::Object;
use crate::serialized::{MONO_BEHAVIOUR, MONO_SCRIPT, SerializedFile, Source};
use crate::typetree::Node;
use crate::value::Value;
use std::cell::OnceCell;
use std::path::Path;

/// A MonoBehaviour, its name and its `m_Script`.
type Head = (Object, String, (i32, i64));

pub(crate) struct PlayerData {
    resources: SerializedFile,
    managers: SerializedFile,
    /// `MonoScript`'s layout, as the bundles ship it (the same Unity build wrote both).
    script_tree: Node,
    il2cpp: Il2Cpp,
    /// Every MonoBehaviour in `resources.assets` with its name and its `m_Script`, read once for all the lists looked up.
    heads: OnceCell<Vec<Head>>,
    /// Scripts named so far, by their `PPtr`: assembly, namespace and class.
    scripts: std::cell::RefCell<std::collections::HashMap<(i32, i64), [String; 3]>>,
}

/// Opened in place: `resources.assets` is some 100 MB, of which a few lists are read.
fn open(path: &Path) -> Result<SerializedFile> {
    SerializedFile::open(Source::file(path)?).map_err(|e| e.context(path.display()))
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
            heads: OnceCell::new(),
            scripts: Default::default(),
        })
    }

    pub(crate) fn il2cpp(&self) -> &Il2Cpp {
        &self.il2cpp
    }

    /// The one MonoBehaviour in `resources.assets` named `name` whose script is `assembly`'s `namespace.class`,
    /// decoded whole with the class's rebuilt layout.
    pub(crate) fn behaviour(&self, name: &str, (assembly, namespace, class): (&str, &str, &str)) -> Result<Value> {
        let mut found = Vec::new();
        for (object, _, pointer) in self.heads()?.iter().filter(|(_, own, _)| own == name) {
            if self.script(name, *pointer)? == [assembly, namespace, class] {
                found.push(*object);
            }
        }
        let [object] = found.as_slice() else {
            return Err(Error::data(format!("{} {class} objects named {name} in resources.assets, not one", found.len())));
        };
        let layout = Layouts::new(&self.il2cpp).mono_behaviour(assembly, namespace, class)?;
        self.resources.decode_with(object, &layout).map_err(|e| e.context(name))
    }

    fn heads(&self) -> Result<&[Head]> {
        if let Some(heads) = self.heads.get() {
            return Ok(heads);
        }
        let header = mono_behaviour_header();
        let mut heads = Vec::new();
        for object in self.resources.objects().iter().filter(|o| o.class_id == MONO_BEHAVIOUR) {
            let head = self.resources.decode_prefix_with(object, &header, "m_Name")?;
            heads.push((*object, head.field("m_Name", Value::as_str)?.to_owned(), head.field("m_Script", Value::as_pptr)?));
        }
        Ok(self.heads.get_or_init(|| heads))
    }

    /// The assembly, namespace and class of the script `name`'s behaviour runs.
    fn script(&self, name: &str, (file_id, path_id): (i32, i64)) -> Result<[String; 3]> {
        if let Some(script) = self.scripts.borrow().get(&(file_id, path_id)) {
            return Ok(script.clone());
        }
        {
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
            let script = [names("m_AssemblyName")?, names("m_Namespace")?, names("m_ClassName")?];
            self.scripts.borrow_mut().insert((file_id, path_id), script.clone());
            Ok(script)
        }
    }
}
