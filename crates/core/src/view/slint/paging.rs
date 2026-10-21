//! One page of a list, shared by screens and hardware page buttons.

/// The visible page of a list and how many rows fit on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::view) struct Pager {
    page: u32,
    per_page: u8,
}

impl Pager {
    /// Starts on the first page. `per_page` of 0 becomes 1.
    pub(in crate::view) fn new(per_page: u8) -> Self {
        Self {
            page: 0,
            per_page: per_page.max(1),
        }
    }

    /// Current page index. Read by tests.
    #[cfg(test)]
    pub(in crate::view) fn page(self) -> u32 {
        self.page
    }

    /// Rows that fit on one page. Read by tests.
    #[cfg(test)]
    pub(in crate::view) fn per_page(self) -> u8 {
        self.per_page
    }

    pub(in crate::view) fn set_per_page(&mut self, per_page: u8) {
        self.per_page = per_page.max(1);
    }

    pub(in crate::view) fn reset(&mut self) {
        self.page = 0;
    }

    pub(in crate::view) fn page_count(self, len: usize) -> u32 {
        let size = u32::from(self.per_page).max(1);
        let len = u32::try_from(len).unwrap_or(u32::MAX);
        len.div_ceil(size).max(1)
    }

    pub(in crate::view) fn clamp(&mut self, len: usize) {
        let last = self.page_count(len).saturating_sub(1);
        self.page = self.page.min(last);
    }

    pub(in crate::view) fn step(&mut self, delta: i32, len: usize) {
        self.clamp(len);
        if delta < 0 {
            self.page = self.page.saturating_sub(1);
            return;
        }
        if self.page + 1 < self.page_count(len) {
            self.page += 1;
        }
    }

    pub(in crate::view) fn can_previous(self) -> bool {
        self.page > 0
    }

    pub(in crate::view) fn can_next(self, len: usize) -> bool {
        self.page + 1 < self.page_count(len)
    }

    pub(in crate::view) fn label(self, len: usize) -> String {
        format!("{} / {}", self.page + 1, self.page_count(len))
    }

    pub(in crate::view) fn slice<T>(self, items: &[T]) -> &[T] {
        let size = usize::from(self.per_page);
        let start = usize::try_from(self.page).unwrap_or(0).saturating_mul(size);
        let end = start.saturating_add(size).min(items.len());
        items.get(start..end).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::Pager;

    #[test]
    fn a_short_list_is_one_page() {
        let pager = Pager::new(10);
        assert_eq!(pager.slice(&[1, 2, 3]), &[1, 2, 3]);
        assert_eq!(pager.page_count(3), 1);
        assert!(!pager.can_next(3));
        assert_eq!(pager.label(3), "1 / 1");
    }

    #[test]
    fn step_stops_on_the_last_page() {
        let mut pager = Pager::new(10);
        let items: Vec<_> = (0..11).collect();
        pager.step(1, items.len());
        assert_eq!(pager.page(), 1);
        assert_eq!(pager.slice(&items), &[10]);
        pager.step(1, items.len());
        assert_eq!(pager.page(), 1);
        pager.step(-1, items.len());
        assert_eq!(pager.page(), 0);
        pager.step(-1, items.len());
        assert_eq!(pager.page(), 0);
    }

    #[test]
    fn clamp_follows_a_shorter_list() {
        let mut pager = Pager::new(10);
        pager.step(1, 25);
        assert_eq!(pager.page(), 1);
        pager.clamp(5);
        assert_eq!(pager.page(), 0);
    }

    #[test]
    fn a_zero_page_size_becomes_one() {
        let pager = Pager::new(0);
        assert_eq!(pager.per_page(), 1);
        assert_eq!(pager.page_count(0), 1);
    }
}
