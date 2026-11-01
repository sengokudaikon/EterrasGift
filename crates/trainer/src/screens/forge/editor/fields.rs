//! Item fields: flags, the base and whether incompatible affixes are allowed.

use super::*;

impl ForgeEditor {
    pub fn toggle_flag(&mut self, index: usize, on: bool) {
        if !self.can_change_field(index) {
            return;
        }
        if let Some(field) = self.fields.get_mut(index)
            && field.kind == FieldKind::Flag
            && field.set(i64::from(on))
        {
            let uid = field.uid;
            self.edited(RowRef::Field(uid));
        }
    }

    pub fn pick_base(&mut self, option: usize) {
        if !self.can_change_base() {
            return;
        }
        if let Some(base) = self.base.as_mut()
            && let Some(choice) = base.options.get(option).filter(|choice| choice.available).cloned()
            && base.set(choice.value)
        {
            let uid = base.uid;
            self.edited(RowRef::Field(uid));
        }
    }

    /// The base field can be edited now: its "Allow incompatible affixes" box follows this.
    pub fn can_edit_base_field(&self) -> bool {
        self.base.as_ref().is_some_and(FieldDraft::is_editable) && self.can_mutate()
    }

    pub fn set_base_allow_incompatible(&mut self, allow: bool) {
        if !self.can_edit_base_field() {
            return;
        }
        if let Some(base) = self.base.as_mut() {
            base.allow_incompatible = allow;
        }
    }
}
