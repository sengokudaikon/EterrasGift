//! Incremental updates preserve row identity and text-field focus.
use slint::{Model, VecModel};

/// Rows are slots: the rows both lists have are updated in place (only those that differ) and only the rest come or go
/// at the end. A row being dragged, typed in or holding keyboard focus keeps going while rows elsewhere change; one remade
/// under the pointer would never hear its release.
pub(crate) fn update_rows<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    while model.row_count() > rows.len() {
        model.remove(model.row_count() - 1);
    }
    let kept = model.row_count();
    for (index, row) in rows.into_iter().enumerate() {
        if index >= kept {
            model.push(row);
        } else if model.row_data(index).as_ref() != Some(&row) {
            model.set_row_data(index, row);
        }
    }
}
