//! Slint model helpers.

use slint::{Model, ModelRc, VecModel};

/// Builds a model of `rows`, or returns `None` when `current` already holds
/// exactly those rows.
///
/// Slint compares a model property by pointer, so assigning a fresh model
/// always marks it changed. Every repeater row is rebuilt and the whole list
/// is repainted, which on an e-ink panel means a bigger refresh. Assign only
/// when this returns a model.
pub(crate) fn changed_model<T>(current: &ModelRc<T>, rows: Vec<T>) -> Option<ModelRc<T>>
where
    T: Clone + PartialEq + 'static,
{
    let same = current.row_count() == rows.len()
        && current
            .iter()
            .zip(rows.iter())
            .all(|(held, next)| held == *next);
    (!same).then(|| ModelRc::new(VecModel::from(rows)))
}

#[cfg(test)]
mod tests {
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
    fn different_rows_build_a_new_model() {
        let next = changed_model(&model(&[1, 2]), vec![1, 3]).expect("a changed row");
        assert_eq!(next.iter().collect::<Vec<_>>(), vec![1, 3]);
        assert!(changed_model(&model(&[1, 2]), vec![1]).is_some());
        assert!(changed_model(&model(&[1]), vec![1, 2]).is_some());
    }
}
