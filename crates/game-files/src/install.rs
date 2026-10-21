//! Where an installed Last Epoch keeps the data this crate reads, and the reads themselves.

use crate::archive::Archive;
use crate::definitions::{self, Definitions};
use crate::error::{Error, Result};
use crate::player::PlayerData;
use crate::strings::{self, SHARED_TABLE, STRING_TABLE, Strings};
use crate::uniques::{self, PlayerDefinitions};
use crate::value::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DATA_FOLDER: &str = "Last Epoch_Data";
const AFFIX_LIST: (&str, &str, &str) = ("LE", "", "AffixList");
const ITEM_LIST: (&str, &str, &str) = ("LE", "", "ItemList");
const AILMENT_LIST: (&str, &str, &str) = ("LE", "", "AilmentList");
/// Unity Localization's default Addressables group naming, one bundle per locale.
const STRING_TABLE_BUNDLES: &str = "localization-string-tables-";

pub struct Install {
    data: PathBuf,
}

fn bundles_in(folder: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(folder).map_err(|e| Error::io(folder, e))? {
        let path = entry.map_err(|e| Error::io(folder, e))?.path();
        if path.extension().is_some_and(|e| e == "bundle") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

impl Install {
    /// The game folder, the one holding `Last Epoch.exe` and `Last Epoch_Data`.
    pub fn at(folder: impl AsRef<Path>) -> Result<Install> {
        let data = folder.as_ref().join(DATA_FOLDER);
        if !data.is_dir() {
            return Err(Error::data(format!("{} has no {DATA_FOLDER} folder", folder.as_ref().display())));
        }
        Ok(Install { data })
    }

    pub fn data_folder(&self) -> &Path {
        &self.data
    }

    fn bundles(&self) -> PathBuf {
        self.data.join("StreamingAssets").join("LEAssetBundles")
    }

    fn addressables(&self) -> PathBuf {
        self.data.join("StreamingAssets").join("aa").join("StandaloneWindows64")
    }

    /// The build hash the game ships in `build_hash.txt`: changes with every game update.
    pub fn build_hash(&self) -> Result<String> {
        let path = self.data.join("build_hash.txt");
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        Ok(text.trim().to_owned())
    }

    /// The files [`Install::definitions`] reads.
    pub fn definition_sources(&self) -> Vec<PathBuf> {
        ["PermaLoad.bundle", "monoscripts.bundle"].map(|name| self.bundles().join(name)).into()
    }

    /// Affix and item base definitions.
    pub fn definitions(&self) -> Result<Definitions> {
        let mut archive = Archive::open(self.definition_sources())?;
        let masters = archive.main_file(0)?;
        let affixes = archive.behaviour(&masters, "MasterAffixesList", AFFIX_LIST)?;
        let items = archive.behaviour(&masters, "MasterItemsList", ITEM_LIST)?;
        let (file, altar) = archive.resolve(&masters, items.field("idolAltarPropertyList", Value::as_pptr)?)?;
        let idol_altar = uniques::registry(&file.decode(&altar)?).map_err(|e| e.context("idolAltarPropertyList"))?;
        let list = archive.behaviour(&masters, "AilmentList", AILMENT_LIST)?;
        let mut ailments = Vec::new();
        for pointer in list.field("list", |l| l.as_array()?.iter().map(Value::as_pptr).collect::<Result<Vec<_>>>())? {
            let (file, object) = archive.resolve(&masters, pointer)?;
            let ailment = file.decode(&object)?;
            ailments.push(definitions::Ailment {
                id: ailment.field("id", Value::int)?,
                effect_of_increased_effectiveness: ailment.field("effectOfIncreasedEffectiveness", Value::int)?,
            });
        }
        Ok(Definitions {
            affixes: definitions::affixes(&affixes).map_err(|e| e.context("MasterAffixesList"))?,
            item_types: definitions::item_types(&items).map_err(|e| e.context("MasterItemsList"))?,
            idol_altar,
            ailments,
        })
    }

    /// The files [`Install::player_definitions`] reads.
    pub fn player_sources(&self) -> Vec<PathBuf> {
        let game = self.data.parent().unwrap_or(&self.data);
        vec![
            game.join("GameAssembly.dll"),
            self.data.join("il2cpp_data").join("Metadata").join("global-metadata.dat"),
            self.data.join("resources.assets"),
            self.data.join("globalgamemanagers.assets"),
            self.bundles().join("monoscripts.bundle"),
        ]
    }

    /// Uniques, properties and set bonuses, from the player data Unity stores without type trees. Needs a game build
    /// whose IL2CPP metadata this reader knows (format 39); any other is refused as unsupported.
    pub fn player_definitions(&self) -> Result<PlayerDefinitions> {
        let player = PlayerData::open(self.data.parent().unwrap_or(&self.data), &self.data)?;
        let properties = player.behaviour("MasterPropertyList", ("LE", "", "PropertyList"))?;
        let registry = |name: &str| player.behaviour(name, ("LE", "", name)).and_then(|v| uniques::registry(&v)).map_err(|e| e.context(name));
        let constant = |class: &str, field: &str| player.il2cpp().int_constant("LE", "", class, field);
        let pair = |name: &str| -> Result<(i32, i32)> { Ok((constant("ItemData", &format!("Min{name}"))?, constant("ItemData", &format!("Max{name}"))?)) };
        Ok(PlayerDefinitions {
            uniques: uniques::uniques(&player.behaviour("UniqueList", ("LE", "", "UniqueList"))?).map_err(|e| e.context("UniqueList"))?,
            properties: uniques::properties(&properties).map_err(|e| e.context("MasterPropertyList"))?,
            default_property: uniques::default_property(&properties).map_err(|e| e.context("MasterPropertyList"))?,
            abilities: player
                .behaviour("AbilityPropertyList", ("LE", "", "AbilityPropertyList"))
                .and_then(|v| uniques::abilities(&v))
                .map_err(|e| e.context("AbilityPropertyList"))?,
            player: registry("PlayerPropertyList")?,
            tracker: registry("TrackerPropertyList")?,
            set_bonuses: uniques::set_bonuses(&player.behaviour("SetBonusesList", ("LE", "", "SetBonusesList"))?).map_err(|e| e.context("SetBonusesList"))?,
            constants: uniques::ItemConstants {
                forging_potential: pair("ForgingPotential")?,
                legendary_potential: pair("LegendaryPotential")?,
                weavers_will: pair("WeaversWill")?,
                weavers_touch: pair("WeaversTouch")?,
                max_level: constant("ExperienceTracker", "maxLevel")?,
            },
        })
    }

    /// The locale codes the game ships text for (`en`, `de`, `es-ES`…), each with its bundle.
    pub fn locales(&self) -> Result<BTreeMap<String, PathBuf>> {
        let mut archive = Archive::open(bundles_in(&self.addressables())?)?;
        Ok(Self::locales_in(&mut archive)?
            .into_iter()
            .map(|(code, index)| (code, archive.bundles()[index].path().to_path_buf()))
            .collect())
    }

    /// The locales in the archive of every addressable bundle, each with its bundle's index: the folder is listed once,
    /// so a bundle that comes or goes meanwhile (a Steam update) cannot leave a name without its bundle.
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
            let codes = tables
                .iter()
                .map(|(_, head)| strings::locale_code(head))
                .collect::<Result<std::collections::BTreeSet<_>>>()?;
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

    /// Every string table for one locale code, as the game's settings name it.
    pub fn strings(&self, locale: &str) -> Result<Strings> {
        let mut archive = Archive::open(bundles_in(&self.addressables())?)?;
        let index = Self::locales_in(&mut archive)?
            .remove(locale)
            .ok_or_else(|| Error::data(format!("the game ships no text for locale {locale}")))?;
        let file = archive.main_file(index)?;
        let (mut tables, mut unkeyed, mut shadowed) = (BTreeMap::new(), 0, 0);
        for (object, _) in archive.behaviours(&file, None, STRING_TABLE, "m_Name")? {
            let table = file.decode(&object)?;
            let (shared_file, shared_object) = archive.resolve(&file, table.field("m_SharedData", crate::value::Value::as_pptr)?)?;
            let shared = shared_file.decode(&shared_object)?;
            if !archive.script(&shared_file, &shared)?.is(SHARED_TABLE.0, SHARED_TABLE.1, SHARED_TABLE.2) {
                return Err(Error::data("m_SharedData is not a SharedTableData"));
            }
            let (collection, keys) = strings::shared_keys(&shared)?;
            let entries = strings::table_entries(&table, &collection, &keys)?;
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
}
