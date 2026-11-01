//! Item templates: an item's affixes, or the whole item, kept under a name in the user's data and drafted onto the item in
//! the forge. A template holds what the game would write (ids, tiers, seals, roll positions, the base, the unique and the
//! status values); applying one makes ordinary drafts, checked like any other.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The most affixes a template keeps: the most an item holds that the forge reads.
pub const MAX_AFFIXES: usize = eg_trainer_wire::forge::MAX_AFFIXES;
const MAX_NAME_CHARS: usize = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TemplateAffix {
    pub id: u16,
    /// 1 to 8.
    pub tier: u8,
    /// 0 normal, 1 sealed, 2 primordial, 3 corrupted, as the game numbers them.
    pub seal: u8,
    pub roll: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TemplateStatus {
    pub forging_potential: u8,
    pub legendary_potential: u8,
    pub weavers_will: u8,
    pub weavers_touch: u8,
    pub corrupted: bool,
    pub ruined: bool,
}

/// What a whole-item template keeps beside its affixes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateItem {
    pub item_type: u8,
    pub sub_type: u16,
    /// The unique or set item and its roll positions.
    pub identity: Option<(u16, Vec<u8>)>,
    pub implicit_rolls: Vec<u8>,
    pub status: TemplateStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemTemplate {
    pub name: String,
    pub affixes: Vec<TemplateAffix>,
    /// None for an affix template, which fits any item its affixes do.
    pub item: Option<TemplateItem>,
}

impl ItemTemplate {
    /// The name as kept: trimmed and bounded.
    pub fn clean_name(name: &str) -> String {
        name.trim().chars().take(MAX_NAME_CHARS).collect()
    }

    pub fn to_json(&self) -> Value {
        let affixes: Vec<Value> = self
            .affixes
            .iter()
            .map(|affix| json!({"id": affix.id, "tier": affix.tier, "seal": affix.seal, "roll": affix.roll}))
            .collect();
        let mut value = json!({"version": 1, "name": self.name, "affixes": affixes});
        if let Some(item) = &self.item {
            let status = &item.status;
            value["item"] = json!({
                "item_type": item.item_type,
                "sub_type": item.sub_type,
                "identity": item.identity.as_ref().map(|(id, rolls)| json!({"id": id, "rolls": rolls})),
                "implicit_rolls": item.implicit_rolls,
                "status": {
                    "forging_potential": status.forging_potential,
                    "legendary_potential": status.legendary_potential,
                    "weavers_will": status.weavers_will,
                    "weavers_touch": status.weavers_touch,
                    "corrupted": status.corrupted,
                    "ruined": status.ruined,
                },
            });
        }
        value
    }

    /// A template back from its JSON, or None for one this version cannot read or whose values the game would not hold.
    pub fn from_json(value: &Value) -> Option<Self> {
        fn byte(value: &Value) -> Option<u8> {
            u8::try_from(value.as_u64()?).ok()
        }
        fn bytes(value: &Value, most: usize) -> Option<Vec<u8>> {
            let list = value.as_array()?;
            (list.len() <= most).then(|| list.iter().map(byte).collect::<Option<Vec<_>>>())?
        }
        if value["version"].as_u64() != Some(1) {
            return None;
        }
        let name = Self::clean_name(value["name"].as_str()?);
        let list = value["affixes"].as_array()?;
        if name.is_empty() || list.len() > MAX_AFFIXES {
            return None;
        }
        let affixes = list
            .iter()
            .map(|affix| {
                let affix = TemplateAffix {
                    id: u16::try_from(affix["id"].as_u64()?).ok()?,
                    tier: byte(&affix["tier"])?,
                    seal: byte(&affix["seal"])?,
                    roll: byte(&affix["roll"])?,
                };
                ((1..=8).contains(&affix.tier) && affix.seal <= 3).then_some(affix)
            })
            .collect::<Option<Vec<_>>>()?;
        let item = match &value["item"] {
            Value::Null => None,
            item => {
                let status = &item["status"];
                Some(TemplateItem {
                    item_type: byte(&item["item_type"])?,
                    sub_type: u16::try_from(item["sub_type"].as_u64()?).ok()?,
                    identity: match &item["identity"] {
                        Value::Null => None,
                        identity => Some((
                            u16::try_from(identity["id"].as_u64()?).ok()?,
                            bytes(&identity["rolls"], eg_trainer_wire::forge::MAX_UNIQUE_ROLLS)?,
                        )),
                    },
                    implicit_rolls: bytes(&item["implicit_rolls"], eg_trainer_wire::forge::MAX_IMPLICIT_ROLLS)?,
                    status: TemplateStatus {
                        forging_potential: byte(&status["forging_potential"])?,
                        legendary_potential: byte(&status["legendary_potential"])?,
                        weavers_will: byte(&status["weavers_will"])?,
                        weavers_touch: byte(&status["weavers_touch"])?,
                        corrupted: status["corrupted"].as_bool()?,
                        ruined: status["ruined"].as_bool()?,
                    },
                })
            }
        };
        Some(Self { name, affixes, item })
    }
}

/// The user's templates, one JSON file each in a folder of the user data. A template saved under a name already kept
/// replaces it.
#[derive(Clone, Debug)]
pub struct TemplateStore {
    directory: PathBuf,
}

impl TemplateStore {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Every template the folder holds, by name; a file this version cannot read is left alone and not listed.
    pub fn list(&self) -> Vec<ItemTemplate> {
        let Ok(entries) = std::fs::read_dir(&self.directory) else {
            return Vec::new();
        };
        let mut templates: Vec<ItemTemplate> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
            .filter_map(|path| file_ops::json::load(&path, file_ops::json::Bom::Refuse).ok().flatten())
            .filter_map(|value| ItemTemplate::from_json(&value))
            .collect();
        templates.sort_by_key(|template| template.name.to_lowercase());
        templates
    }

    pub fn save(&self, template: &ItemTemplate) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        let text = serde_json::to_string_pretty(&template.to_json()).unwrap_or_default();
        file_ops::atomic::write(&self.file(&template.name), text.as_bytes())
    }

    pub fn delete(&self, name: &str) -> std::io::Result<()> {
        match std::fs::remove_file(self.file(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.directory.join(format!("{}.json", file_stem(name)))
    }
}

/// A template's file name: its name with every character a file name may not hold, and case, folded, so a name maps to one
/// file whatever its case.
fn file_stem(name: &str) -> String {
    let stem: String = ItemTemplate::clean_name(name)
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let stem = stem.trim().trim_end_matches('.');
    if stem.is_empty() { "template".to_owned() } else { stem.to_owned() }
}

/// Which of the `kept` templates saving under `name` would replace: the one whose file the name maps to, which may be spelled
/// differently ("Wayfarer's set" and "wayfarer_s set" share one).
pub fn taken_by(kept: &[ItemTemplate], name: &str) -> Option<String> {
    if ItemTemplate::clean_name(name).is_empty() {
        return None;
    }
    let stem = file_stem(name);
    kept.iter().find(|template| file_stem(&template.name) == stem).map(|template| template.name.clone())
}
