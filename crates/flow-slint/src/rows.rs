//! A list model updated in place.

use slint::{Model, ModelRc, VecModel};
use std::rc::Rc;

/// Rows are slots: rows both lists have are updated in place, and only if they differ, and the rest come or go at the
/// end. Slint updates a changed row's existing instance, so a row being dragged, typed in or holding focus carries on
/// while others change. Handing the view a new model instead would rebuild every row.
pub struct Rows<T: 'static>(Rc<VecModel<T>>);

impl<T: 'static> Default for Rows<T> {
    fn default() -> Self {
        Rows(Rc::new(VecModel::default()))
    }
}

impl<T: Clone + PartialEq + 'static> Rows<T> {
    /// The model to give the view, once. Later changes go through [`Rows::sync`].
    pub fn model(&self) -> ModelRc<T> {
        ModelRc::from(self.0.clone())
    }

    pub fn sync(&self, rows: impl IntoIterator<Item = T>) {
        let rows: Vec<T> = rows.into_iter().collect();
        while self.0.row_count() > rows.len() {
            self.0.remove(self.0.row_count() - 1);
        }
        let kept = self.0.row_count();
        for (index, row) in rows.into_iter().enumerate() {
            if index >= kept {
                self.0.push(row);
            } else if self.0.row_data(index).as_ref() != Some(&row) {
                self.0.set_row_data(index, row);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contents(rows: &Rows<i32>) -> Vec<i32> {
        rows.model().iter().collect()
    }

    #[test]
    fn it_grows_changes_and_shrinks_in_place() {
        let rows = Rows::default();
        let model = rows.model();
        rows.sync([1, 2]);
        assert_eq!(contents(&rows), [1, 2]);
        rows.sync([1, 3, 4]);
        assert_eq!(contents(&rows), [1, 3, 4]);
        rows.sync([5]);
        assert_eq!(contents(&rows), [5]);
        assert_eq!(model.row_count(), 1, "the model given to the view is the one that changes");
    }
}
