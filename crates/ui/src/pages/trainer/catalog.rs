//! Trainer page presentation of the shared feature registry. Native availability comes
//! from the connected backend; `presented` means that a row view exists.
use eg_trainer::features::{DESCRIPTORS, FeatureDescriptor};
pub use eg_trainer::features::{FeatureField, FeatureKind, FeatureValue, PAGES, TrainerArea as Area, TrainerPage};

/// A feature as the pages list it: its registry descriptor, and whether a row view exists for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feature {
    pub descriptor: &'static FeatureDescriptor,
    pub presented: bool,
}
impl Feature {
    pub const fn id(&self) -> &'static str {
        self.descriptor.key.as_str()
    }
}
impl std::ops::Deref for Feature {
    type Target = FeatureDescriptor;
    fn deref(&self) -> &FeatureDescriptor {
        self.descriptor
    }
}
const fn rows() -> [Feature; DESCRIPTORS.len()] {
    let mut rows = [Feature {
        descriptor: &DESCRIPTORS[0],
        presented: true,
    }; DESCRIPTORS.len()];
    let mut index = 0;
    while index < rows.len() {
        rows[index].descriptor = &DESCRIPTORS[index];
        index += 1;
    }
    rows
}
pub const FEATURES: &[Feature] = &rows();
pub fn page(area: Area) -> &'static TrainerPage {
    PAGES.iter().find(|page| page.area == area).expect("every area has a page")
}
pub fn feature(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|feature| feature.id() == id)
}
