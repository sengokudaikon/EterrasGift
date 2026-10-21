//! Trainer page presentation of the shared feature registry. Native availability comes
//! from the connected backend; `presented` means that a row view exists.
use eg_trainer::features::{DESCRIPTORS, FeatureDescriptor};
pub use eg_trainer::features::{FeatureField, FeatureKind, FeatureValue, PAGES, TrainerArea as Area, TrainerPage};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feature {
    pub id: &'static str,
    pub area: Area,
    pub section: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub kind: FeatureKind,
    pub presented: bool,
    pub values: &'static [FeatureValue],
    pub fields: &'static [FeatureField],
    pub needs: Option<&'static str>,
    pub fills_section: bool,
}
impl Feature {
    const fn of(feature: &FeatureDescriptor) -> Self {
        Self {
            id: feature.key.as_str(),
            area: feature.area,
            section: feature.section,
            title: feature.title,
            description: feature.description,
            kind: feature.kind,
            presented: true,
            values: feature.values,
            fields: feature.fields,
            needs: feature.needs,
            fills_section: feature.fills_section,
        }
    }
    /// What its (?) says: what it does, what the game has to be showing for it, and what its values mean.
    pub fn help(&self) -> String {
        let mut lines = vec![self.description.to_owned()];
        lines.extend(self.needs.map(str::to_owned));
        lines.extend(self.values.iter().filter_map(|value| value.hint.map(|hint| format!("{}: {hint}", value.label))));
        lines.join("\n")
    }
}
const fn rows() -> [Feature; DESCRIPTORS.len()] {
    let mut rows = [Feature::of(&DESCRIPTORS[0]); DESCRIPTORS.len()];
    let mut index = 0;
    while index < rows.len() {
        rows[index] = Feature::of(&DESCRIPTORS[index]);
        index += 1;
    }
    rows
}
pub const FEATURES: &[Feature] = &rows();
pub fn page(area: Area) -> &'static TrainerPage {
    PAGES.iter().find(|page| page.area == area).expect("every area has a page")
}
pub fn feature(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|feature| feature.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_every_section_is_a_page_section() {
        let mut ids: Vec<&str> = FEATURES.iter().map(|f| f.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), FEATURES.len());
        for feature in FEATURES {
            assert!(page(feature.area).sections.contains(&feature.section), "{} sits in a section of its page", feature.id);
        }
    }

    #[test]
    fn help_joins_description_needs_and_hints() {
        let timer = feature("morditas-timer").unwrap();
        assert_eq!(
            timer.help(),
            "Gives a Rage of Morditas hunt more time: a longer start, a slower countdown, more time for each kill.\n\
             Starting time: From the next hunt you start.\n\
             Timer speed: 1 is the game's own; 0 stops the countdown, so the hunt ends when no foes are left.\n\
             Time per kill: As in the game, a kill adds less the longer the hunt has grown."
        );
        assert_eq!(feature("move-speed").unwrap().help(), "You move faster.\nRe-equip your boots after changing it.");
    }

    #[test]
    fn defaults_follow_the_catalog_helpers() {
        let rarity = feature("monster-rarity").unwrap().values[1];
        assert_eq!((rarity.minimum, rarity.maximum, rarity.step, rarity.unit), (0.0, 20.0, 0.5, Some("×")));
        let wardens = feature("rune-wardens").unwrap().values[0];
        assert_eq!((wardens.maximum, wardens.step, wardens.unit), (20.0, 1.0, None));
        let favor = feature("faction-standing").unwrap().fields[4];
        assert_eq!((favor.label, favor.maximum, favor.group), ("Favor", 999_999.0, Some("Merchant's Guild")));
        assert_eq!(feature("currency").unwrap().fields[0].maximum, i32::MAX as f64);
    }
}
