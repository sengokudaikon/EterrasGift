//! Saved item templates: their JSON and the folder that keeps them.

use eg_trainer::item::template::{ItemTemplate, TemplateAffix, TemplateItem, TemplateStatus, TemplateStore};

fn affixes() -> Vec<TemplateAffix> {
    vec![
        TemplateAffix {
            id: 25,
            tier: 7,
            seal: 0,
            roll: 255,
        },
        TemplateAffix {
            id: 1148,
            tier: 8,
            seal: 2,
            roll: 9,
        },
    ]
}

fn whole() -> ItemTemplate {
    ItemTemplate {
        name: "Boots of Haste".into(),
        affixes: affixes(),
        item: Some(TemplateItem {
            item_type: 3,
            sub_type: 12,
            identity: Some((911, vec![17, 255])),
            implicit_rolls: vec![128, 0],
            status: TemplateStatus {
                forging_potential: 0,
                legendary_potential: 2,
                weavers_will: 0,
                weavers_touch: 0,
                corrupted: true,
                ruined: false,
            },
        }),
    }
}

#[test]
fn templates_round_trip_and_refuse_values_the_game_would_not_hold() {
    for template in [
        whole(),
        ItemTemplate {
            name: "Affixes".into(),
            affixes: affixes(),
            item: None,
        },
    ] {
        assert_eq!(ItemTemplate::from_json(&template.to_json()), Some(template));
    }
    let mut json = whole().to_json();
    json["affixes"][0]["tier"] = 9.into();
    assert_eq!(ItemTemplate::from_json(&json), None, "no tier 9");
    let mut json = whole().to_json();
    json["item"]["identity"]["rolls"] = serde_json::json!([1, 2, 3, 4, 5, 6, 7, 8, 9]);
    assert_eq!(ItemTemplate::from_json(&json), None, "a unique holds at most 8 rolls");
    let mut json = whole().to_json();
    json["version"] = 2.into();
    assert_eq!(ItemTemplate::from_json(&json), None, "a later version is not guessed at");
}

#[test]
fn the_store_keeps_one_file_per_name_whatever_its_case() {
    let folder = tempfile::tempdir().unwrap();
    let store = TemplateStore::new(folder.path().join("forge-templates"));
    assert!(store.list().is_empty(), "no folder yet is no templates");
    store.save(&whole()).unwrap();
    let mut renamed = whole();
    renamed.name = "boots of haste".into();
    renamed.affixes.pop();
    store.save(&renamed).unwrap();
    let mut other = whole();
    other.name = "Amulet: crit/speed?".into();
    store.save(&other).unwrap();
    std::fs::write(store.directory().join("broken.json"), b"{").unwrap();
    let names: Vec<String> = store.list().into_iter().map(|template| template.name).collect();
    assert_eq!(names, ["Amulet: crit/speed?", "boots of haste"]);
    store.delete("BOOTS OF HASTE").unwrap();
    store.delete("never saved").unwrap();
    assert_eq!(store.list().len(), 1);
}
