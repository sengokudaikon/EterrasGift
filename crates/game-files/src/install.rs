//! Where an installed Last Epoch keeps the data this crate reads, and the reads themselves.

use crate::definitions::{self, Definitions};
use crate::uniques::{self, PlayerDefinitions};
use binary_reader::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use unity_files::archive::{self, Archive};
use unity_files::folder::PlayerFolder;
use unity_files::player::PlayerData;
use unity_files::strings::{self, Strings};
use unity_files::value::Value;

const AFFIX_LIST: (&str, &str, &str) = ("LE", "", "AffixList");
const ITEM_LIST: (&str, &str, &str) = ("LE", "", "ItemList");
const AILMENT_LIST: (&str, &str, &str) = ("LE", "", "AilmentList");
const ABILITY_MANAGER: (&str, &str, &str) = ("LE", "", "AbilityManager");
const CLASS_LIST: (&str, &str, &str) = ("LE", "", "CharacterClassList");

pub struct Install {
    player: PlayerFolder,
}

impl Install {
    /// The game `executable` starts, whose data folder takes its name: `Last Epoch.exe` beside `Last Epoch_Data`.
    pub fn of(executable: impl AsRef<Path>) -> Result<Install> {
        let player = PlayerFolder::of(executable.as_ref());
        if !player.data().is_dir() {
            let name = player.data().file_name().unwrap_or_default().display();
            return Err(Error::data(format!("{} has no {name} folder", player.folder().display())));
        }
        Ok(Install { player })
    }

    pub fn data_folder(&self) -> &Path {
        self.player.data()
    }

    fn bundles(&self) -> PathBuf {
        self.data_folder().join("StreamingAssets").join("LEAssetBundles")
    }

    fn addressables(&self) -> PathBuf {
        self.data_folder().join("StreamingAssets").join("aa").join("StandaloneWindows64")
    }

    pub fn game_assembly(&self) -> PathBuf {
        self.player.game_assembly()
    }

    pub fn metadata(&self) -> PathBuf {
        self.player.metadata()
    }

    /// The bundle of `MonoScript`s the other bundles' behaviours run.
    fn monoscripts(&self) -> PathBuf {
        self.bundles().join("monoscripts.bundle")
    }

    /// The bundle holding the masters (affixes, items, ailments).
    fn perma_load(&self) -> PathBuf {
        self.bundles().join("PermaLoad.bundle")
    }

    /// The build hash the game ships in `build_hash.txt`: changes with every game update.
    pub fn build_hash(&self) -> Result<String> {
        let path = self.data_folder().join("build_hash.txt");
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        Ok(text.trim().to_owned())
    }

    /// The files [`Install::strings`] reads: every addressable bundle, as the locales are found among them.
    pub fn text_sources(&self) -> Result<Vec<PathBuf>> {
        archive::bundles_in(&self.addressables())
    }

    /// The files [`Install::definitions`] reads.
    pub fn definition_sources(&self) -> Vec<PathBuf> {
        vec![self.perma_load(), self.monoscripts()]
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
        let ailments = definitions::ailments(&list, |pointer| {
            let (file, object) = archive.resolve(&masters, pointer)?;
            file.decode(&object)
        })
        .map_err(|e| e.context("AilmentList"))?;
        let manager = archive.behaviour(&masters, "Ability Manager", ABILITY_MANAGER)?;
        let abilities = definitions::abilities(&manager, |pointer| {
            let (file, object) = archive.resolve(&masters, pointer)?;
            file.decode_prefix(&object, "abilityName")
        })
        .map_err(|e| e.context("Ability Manager"))?;
        let list = archive.behaviour(&masters, "Character Class List", CLASS_LIST)?;
        let classes = definitions::classes(&list, |pointer| {
            let (file, object) = archive.resolve(&masters, pointer)?;
            file.decode_prefix(&object, "specialTagForClassSpecificLevelOfSkillsStats")
        })
        .map_err(|e| e.context("Character Class List"))?;
        Ok(Definitions {
            affixes: definitions::affixes(&affixes).map_err(|e| e.context("MasterAffixesList"))?,
            item_types: definitions::item_types(&items).map_err(|e| e.context("MasterItemsList"))?,
            idol_altar,
            omen_idol_affix_effect_modifier: items.field("omenIdolAffixEffectModifier", Value::as_f32)?,
            ailments,
            abilities,
            classes,
        })
    }

    /// The files [`Install::player_definitions`] reads.
    pub fn player_sources(&self) -> Vec<PathBuf> {
        vec![self.game_assembly(), self.metadata(), self.player.resources(), self.player.managers(), self.monoscripts()]
    }

    /// Uniques, properties and set bonuses, from the player data Unity stores without type trees. Needs a game build
    /// whose IL2CPP metadata this reader knows (format 39); any other is refused as unsupported.
    pub fn player_definitions(&self) -> Result<PlayerDefinitions> {
        let player = PlayerData::open(
            &self.game_assembly(),
            &self.metadata(),
            &self.player.resources(),
            &self.player.managers(),
            &self.monoscripts(),
        )?;
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
            tracker: player
                .behaviour("TrackerPropertyList", ("LE", "", "TrackerPropertyList"))
                .and_then(|v| uniques::tracker_registry(&v))
                .map_err(|e| e.context("TrackerPropertyList"))?,
            set_bonuses: uniques::set_bonuses(&player.behaviour("SetBonusesList", ("LE", "", "SetBonusesList"))?).map_err(|e| e.context("SetBonusesList"))?,
            constants: uniques::ItemConstants {
                forging_potential: pair("ForgingPotential")?,
                legendary_potential: pair("LegendaryPotential")?,
                weavers_will: pair("WeaversWill")?,
                weavers_touch: pair("WeaversTouch")?,
                max_level: constant("ExperienceTracker", "maxLevel")?,
            },
            conditional_damage: uniques::conditional_damage(&properties).map_err(|e| e.context("MasterPropertyList"))?,
            stat_limits: properties
                .field("extraStatData", Value::as_pptr)
                .and_then(|pointer| player.behaviour_at(pointer, ("LE", "", "ExtraStatData")))
                .and_then(|v| uniques::stat_limits(&v))
                .map_err(|e| e.context("extraStatData"))?,
            ailment_names: player.il2cpp().byte_enum("LE", "", "AilmentID")?,
        })
    }

    /// The locale codes the game ships text for (`en`, `de`, `es-ES`…), each with its bundle.
    pub fn locales(&self) -> Result<BTreeMap<String, PathBuf>> {
        strings::locales(&self.addressables())
    }

    /// Every string table for one locale code, as the game's settings name it.
    pub fn strings(&self, locale: &str) -> Result<Strings> {
        strings::read(&self.addressables(), locale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_folder_takes_the_executables_name() {
        let game = tempfile::tempdir().unwrap();
        std::fs::create_dir(game.path().join("Renamed_Data")).unwrap();
        let install = Install::of(game.path().join("Renamed.exe")).unwrap();
        assert_eq!(install.data_folder(), game.path().join("Renamed_Data"));
        let refused = Install::of(game.path().join("Last Epoch.exe")).err().unwrap().to_string();
        assert!(refused.contains("Last Epoch_Data"), "{refused}");
    }
}
