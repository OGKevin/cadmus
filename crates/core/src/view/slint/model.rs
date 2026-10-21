//! Slint model helpers.

use slint::{Model, ModelRc, VecModel};

/// Builds a model of `rows`, or returns `None` when `current` already shows them.
///
/// A [`VecModel`] of the same length is updated in place. Assigning a fresh
/// model rebuilds every repeater row, which drops widget state — a bar's
/// position, for example — and repaints the whole list. A different length,
/// or a model that is not a [`VecModel`], still returns a new model for the
/// caller to assign.
pub(crate) fn changed_model<T>(current: &ModelRc<T>, rows: Vec<T>) -> Option<ModelRc<T>>
where
    T: Clone + PartialEq + 'static,
{
    if let Some(model) = current.as_any().downcast_ref::<VecModel<T>>()
        && model.row_count() == rows.len()
    {
        for (index, row) in rows.into_iter().enumerate() {
            if model.row_data(index).as_ref() != Some(&row) {
                model.set_row_data(index, row);
            }
        }
        return None;
    }
    let same = current.row_count() == rows.len()
        && current
            .iter()
            .zip(rows.iter())
            .all(|(held, next)| held == *next);
    (!same).then(|| ModelRc::new(VecModel::from(rows)))
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use super::*;

    fn model(rows: &[i32]) -> ModelRc<i32> {
        ModelRc::new(VecModel::from(rows.to_vec()))
    }

    #[test]
    fn equal_rows_keep_the_current_model() {
        assert!(changed_model(&model(&[1, 2]), vec![1, 2]).is_none());
        assert!(changed_model(&model(&[]), Vec::new()).is_none());
    }

    #[test]
    fn a_changed_row_stays_in_the_current_model() {
        let current = model(&[1, 2, 3]);
        assert!(changed_model(&current, vec![1, 9, 3]).is_none());
        assert_eq!(current.iter().collect::<Vec<_>>(), vec![1, 9, 3]);
        assert!(changed_model(&current, vec![1, 9, 3]).is_none());
    }

    #[test]
    fn a_different_length_builds_a_new_model() {
        let current = model(&[1, 2]);
        let next = changed_model(&current, vec![1]).expect("shorter");
        assert_eq!(next.iter().collect::<Vec<_>>(), vec![1]);
        assert_eq!(current.iter().collect::<Vec<_>>(), vec![1, 2]);
        assert!(changed_model(&model(&[1]), vec![1, 2]).is_some());
    }

    /// A model Slint did not build as a [`VecModel`]. Same-length edits cannot
    /// be written back, so the caller still receives a new model.
    struct FixedPair(i32, i32);

    impl Model for FixedPair {
        type Data = i32;

        fn row_count(&self) -> usize {
            2
        }

        fn row_data(&self, row: usize) -> Option<i32> {
            match row {
                0 => Some(self.0),
                1 => Some(self.1),
                _ => None,
            }
        }

        fn model_tracker(&self) -> &dyn slint::ModelTracker {
            &()
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn a_model_that_cannot_change_a_row_is_replaced() {
        let current = ModelRc::new(FixedPair(1, 2));
        let next = changed_model(&current, vec![1, 9]).expect("replaced");
        assert_eq!(next.iter().collect::<Vec<_>>(), vec![1, 9]);
        assert_eq!(current.row_data(1), Some(2));
    }
}
