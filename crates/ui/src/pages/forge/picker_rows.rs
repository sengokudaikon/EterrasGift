//! The affix picker's rows, built as the list asks for each.

use crate::{PickerRowView, PillTone, PillView};
use eg_trainer_screens::forge::game::Catalog;
use eg_trainer_screens::forge::picker::{self, Tone};
use slint::{Model, ModelNotify, ModelRc, ModelTracker, VecModel};
use std::any::Any;
use std::cell::RefCell;
use std::sync::Arc;

/// What the picker lists, and what its rows are built from.
#[derive(Default)]
pub(super) struct Listed {
    pub indices: Vec<usize>,
    pub catalog: Option<Arc<Catalog>>,
    pub slot: Option<u16>,
    pub own_types: Vec<u16>,
    pub base: Option<(u16, u16)>,
}

/// The list builds only the rows in view, so a keystroke over the whole catalog costs its filter and a screenful of rows, not
/// a row for every definition.
#[derive(Default)]
pub(super) struct PickerRows {
    listed: RefCell<Listed>,
    notify: ModelNotify,
}

impl PickerRows {
    pub fn replace(&self, listed: Listed) {
        *self.listed.borrow_mut() = listed;
        self.notify.reset();
    }
}

impl Model for PickerRows {
    type Data = PickerRowView;

    fn row_count(&self) -> usize {
        self.listed.borrow().indices.len()
    }

    fn row_data(&self, row: usize) -> Option<PickerRowView> {
        let listed = self.listed.borrow();
        let catalog = listed.catalog.as_ref()?;
        let definition = catalog.definitions.get(*listed.indices.get(row)?)?;
        let entry = picker::entry(definition, catalog, listed.slot, &listed.own_types, listed.base);
        let pills: Vec<PillView> = entry
            .pills
            .into_iter()
            .map(|(text, tone)| PillView {
                text: text.into(),
                tone: match tone {
                    Tone::Neutral => PillTone::Neutral,
                    Tone::Reforged => PillTone::Reforged,
                    Tone::Unique => PillTone::Unique,
                },
            })
            .collect();
        Some(PickerRowView {
            name: entry.name.into(),
            pills: ModelRc::new(VecModel::from(pills)),
            spoken: entry.spoken.into(),
            detail: entry.detail.into(),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
