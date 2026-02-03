use crate::color::SEPARATOR_NORMAL;
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::Framebuffer;
use crate::geom::{Dir, Point, Rectangle};
use crate::unit::scale_by_dpi;
use crate::view::filler::Filler;
use crate::view::UpdateMode;
use crate::view::{Bus, Event, Hub, Id, RenderData, RenderQueue, View, ID_FEEDER};
use crate::view::{SMALL_BAR_HEIGHT, THICKNESS_MEDIUM};
use std::collections::BTreeMap;

/// Domain adapter for [`StackNavigationBar`].
///
/// A `NavigationProvider` tells the container how to traverse hierarchical levels
/// (e.g. directory parents), and how to populate each bar with pre-fetched data.
/// This trait abstracts the domain-specific logic from the navigation bar's layout
/// and interaction logic.
///
/// # Implementation Notes
///
/// When implementing `resize_bar_by()`, ensure the bar respects minimum height
/// constraints (typically `SMALL_BAR_HEIGHT - THICKNESS_MEDIUM` scaled by DPI).
/// The method should return the actual resize amount after applying constraints.
pub trait NavigationProvider {
    /// Key that identifies a level in the stack.
    type LevelKey: Eq + Ord + Clone;

    /// Data needed to render a level.
    type LevelData;

    /// Concrete view used to render a level.
    type Bar: View;

    /// Returns the key to consider "selected".
    ///
    /// Some domains want to select the parent when the leaf level is empty.
    fn selected_leaf_key(&self, selected: &Self::LevelKey) -> Self::LevelKey {
        selected.clone()
    }

    /// Returns the starting key for bar traversal.
    ///
    /// This may differ from `selected` when the selected level is empty.
    /// For example, if a directory has no subdirectories, this might return
    /// the parent directory to start the bar hierarchy from there.
    fn leaf_for_bar_traversal(
        &self,
        selected: &Self::LevelKey,
        _context: &Context,
    ) -> Self::LevelKey {
        self.selected_leaf_key(selected)
    }

    /// Returns the parent key, if any.
    fn parent(&self, current: &Self::LevelKey) -> Option<Self::LevelKey>;

    /// Returns true if `ancestor` is an ancestor of `descendant`.
    fn is_ancestor(&self, ancestor: &Self::LevelKey, descendant: &Self::LevelKey) -> bool;

    /// Returns true if the key is the root of the stack.
    fn is_root(&self, key: &Self::LevelKey, context: &Context) -> bool;

    /// Fetch the data for a level.
    fn fetch_level_data(&self, key: &Self::LevelKey, context: &mut Context) -> Self::LevelData;

    /// Estimates how many visual lines (rows) the bar will need to display its content.
    ///
    /// This value is used to calculate the vertical height of the bar. Each line
    /// corresponds to one row in the visual layout:
    /// - For vertical layouts (e.g., DirectoriesBar), this typically equals the
    ///   number of items to display since each item occupies one line.
    /// - For horizontal layouts (e.g., CategoryNavigationBar), this should return
    ///   `1` since all items are arranged horizontally on a single line.
    ///
    /// The height formula is:
    /// ```rust,ignore
    /// height = line_count * x_height + (line_count + 1) * padding / 2
    /// ```
    ///
    /// # Returns
    ///
    /// The number of visual lines needed.
    ///
    /// Returning `0` indicates that the level has no visible content and allows
    /// `StackNavigationBar` to treat this level as empty (for example, by not
    /// inserting a bar for it). Values `>= 1` correspond to the number of visual
    /// lines that should be allocated for the bar's content.
    fn estimate_line_count(&self, key: &Self::LevelKey, data: &Self::LevelData) -> usize;

    /// Creates a new empty bar for the given level key.
    ///
    /// This method is responsible for instantiating a concrete bar view that will
    /// display content for a specific level in the navigation hierarchy. The bar is
    /// created with an initial rectangle and is positioned by `StackNavigationBar`.
    ///
    /// The returned bar should be empty or minimally initialized, its content will be
    /// populated later via `update_bar()` once the necessary data is fetched from the
    /// domain layer. This separation allows bars to be created before their content
    /// is available, enabling flexible reuse and repositioning strategies.
    ///
    /// # Arguments
    ///
    /// * `rect` - The initial rectangle where the bar will be positioned. This
    ///   rectangle is computed by `StackNavigationBar` based on layout metrics and
    ///   available space. The bar should use this rect as its initial bounds.
    /// * `key` - The level identifier (e.g., a directory path or category ID) that
    ///   uniquely identifies which level this bar represents in the hierarchy.
    ///
    /// # Returns
    ///
    /// A new bar view instance initialized with the provided rectangle. The bar's
    /// content should be empty or a placeholder at this point.
    ///
    /// # Implementation Notes
    ///
    /// - The bar's rectangle **must** be stored and accessible via the `View` trait's
    ///   `rect()` and `rect_mut()` methods.
    /// - Do not fetch or populate content in this method; that happens in `update_bar()`.
    /// - The `key` parameter is provided for reference but typically stored separately
    ///   by the domain layer (see `bar_key()` to retrieve it).
    /// - If the concrete bar type needs additional context (e.g., fonts or device info)
    ///   during creation, access it from a shared source rather than requiring it as
    ///   a method parameter.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// fn create_bar(&self, rect: Rectangle, key: &Self::LevelKey) -> Self::Bar {
    ///     MyBar::new(rect, key.clone())
    /// }
    /// ```
    fn create_bar(&self, rect: Rectangle, key: &Self::LevelKey) -> Self::Bar;

    /// Returns the key that is currently displayed by a bar.
    fn bar_key(&self, bar: &Self::Bar) -> Self::LevelKey;

    /// Update bar content using only fonts (no context borrowing).
    fn update_bar(
        &self,
        bar: &mut Self::Bar,
        data: &Self::LevelData,
        selected: &Self::LevelKey,
        fonts: &mut Fonts,
    );

    /// Update bar selection when the content is unchanged.
    fn update_bar_selection(&self, bar: &mut Self::Bar, selected: &Self::LevelKey);

    /// Apply a vertical resize delta to a bar.
    ///
    /// This method should mutate the bar's rectangle and update its content to
    /// reflect the new size. The bar must enforce minimum height constraints
    /// (typically `SMALL_BAR_HEIGHT - THICKNESS_MEDIUM` scaled by DPI).
    ///
    /// # Arguments
    ///
    /// * `bar` - The bar to resize
    /// * `delta_y` - The vertical resize amount (positive = grow, negative = shrink)
    /// * `fonts` - Font registry for text rendering calculations
    ///
    /// # Returns
    ///
    /// The actual resize amount applied after enforcing constraints. This may differ
    /// from `delta_y` if minimum/maximum height limits are reached.
    ///
    /// # Important
    ///
    /// Do NOT pre-modify the bar's rect before calling this method. The provider
    /// will handle the entire resize operation, including constraint enforcement.
    fn resize_bar_by(&self, bar: &mut Self::Bar, delta_y: i32, fonts: &mut Fonts) -> i32;

    /// Shift a bar by a delta.
    fn shift_bar(&self, bar: &mut Self::Bar, delta: Point);
}

/// A vertically-stacked navigation bar with dynamic height and level management.
///
/// `StackNavigationBar` displays a stack of navigation levels (e.g., directory hierarchy)
/// with separators between them. It supports interactive resizing via swipe gestures,
/// automatic level management based on available space, and reuse of existing bars
/// when navigating to related items.
///
/// # Architecture
///
/// The navigation bar uses a generic `NavigationProvider` trait to abstract domain-specific
/// logic (e.g., file system navigation, category hierarchies). This separation allows the
/// same container to work with different hierarchical data structures.
///
/// # Layout Structure
///
/// Children are stored in alternating order:
/// - Even indices (0, 2, 4...): Navigation bars for each level
/// - Odd indices (1, 3, 5...): Separator fillers between bars
///
/// The container's rect is dynamically adjusted to match the total height of all children.
///
/// ## ASCII illustration (top = smaller y, bottom = larger y):
///
/// ```txt
///   container.rect.min.y
///   +--------------------------------------+
///   | Bar (index 0)                        |  <-- even indices are bars (level 0)
///   +--------------------------------------+
///   | Separator (index 1)                  |  <-- odd indices are separators
///   +--------------------------------------+
///   | Bar (index 2)                        |  <-- even indices are bars (level 1)
///   +--------------------------------------+
///   | Separator (index 3)                  |
///   +--------------------------------------+
///   | Bar (index 4)                        |  <-- deeper level / leaf
///   +--------------------------------------+
///   container.rect.max.y
/// ```
///
/// The diagram shows the alternating bar/separator pattern and how the container's
/// min.y and max.y encompass the stacked children.
///
/// # Interactive Resize
///
/// Users can resize individual bars via vertical (up/down) swipe gestures. The container:
/// 1. Calculates the desired size based on grid-snapped line counts
/// 2. Delegates actual resize to the provider via `resize_bar_by()`
/// 3. Updates the container rect to match the last child's position
///
/// Minimum height constraints are enforced by the provider to prevent 1px collapse bugs.
///
/// # Level Management
///
/// When `set_selected()` is called:
/// 1. Existing bars are reused when navigating to ancestors/descendants
/// 2. New bars are created only when needed
/// 3. Excess bars (beyond `max_levels`) are trimmed
/// 4. Empty levels are skipped unless they're the selected level
///
/// # Type Parameters
///
/// * `P` - The navigation provider that implements domain-specific traversal logic
///
/// # Why `P: 'static`?
///
/// The view tree stores views as owned trait objects (`Box<dyn View>`) inside containers.
/// Those boxed trait objects are used without borrowing from caller stack frames or
/// tied lifetimes, so the concrete view types placed in the boxes must not contain
/// non-'static references. `StackNavigationBar` owns its `provider: P` field directly,
/// therefore to safely store `StackNavigationBar<P>` as a boxed view the provider type
/// must be `'static`. This keeps the view-tree API simple and avoids needing to
/// propagate lifetimes through the entire view hierarchy.
#[derive(Debug)]
pub struct StackNavigationBar<P: NavigationProvider + 'static> {
    /// Unique view identifier
    id: Id,
    /// Container rectangle (dynamically adjusted to fit children)
    pub rect: Rectangle,
    /// Child views: bars at even indices, separators at odd indices
    children: Vec<Box<dyn View>>,
    /// Currently selected level key
    selected: P::LevelKey,
    /// Maximum Y coordinate for the navigation bar's bottom edge
    pub vertical_limit: i32,
    /// Maximum number of levels to display simultaneously
    max_levels: usize,
    /// Domain-specific navigation logic provider
    provider: P,
    /// If this bar type should allow resizing via gesture
    enable_resize: bool,
}

impl<P: NavigationProvider + 'static> StackNavigationBar<P> {
    /// Creates a new navigation bar.
    ///
    /// The bar starts empty and must be populated via `set_selected()`.
    ///
    /// # Arguments
    ///
    /// * `rect` - Initial container rectangle
    /// * `vertical_limit` - Maximum Y coordinate for the bar's bottom edge
    /// * `max_levels` - Maximum number of hierarchy levels to display
    /// * `provider` - Domain-specific navigation provider
    /// * `selected` - Initial selected level (bar remains empty until `set_selected()` is called)
    pub fn new(
        rect: Rectangle,
        vertical_limit: i32,
        max_levels: usize,
        provider: P,
        selected: P::LevelKey,
    ) -> Self {
        Self {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            selected,
            vertical_limit,
            max_levels,
            provider,
            enable_resize: true,
        }
    }

    pub fn disable_resize(mut self) -> Self {
        self.enable_resize = false;
        self
    }

    /// Removes all child bars and separators.
    pub fn clear(&mut self) {
        self.children.clear();
    }

    /// Returns the currently selected level key.
    pub fn selected(&self) -> &P::LevelKey {
        &self.selected
    }

    /// Updates the selected level and rebuilds the navigation bar hierarchy.
    ///
    /// This method reuses existing bars when navigating to related
    /// levels (ancestors or descendants) to minimize rendering work. New bars are
    /// created only when necessary, and excess bars are trimmed.
    ///
    /// # Algorithm
    ///
    /// 1. Trim trailing bars that are no longer ancestors of the selected level
    /// 2. Prefetch data for all levels from selected up to root (or max_levels)
    /// 3. Build bar hierarchy bottom-up from leaf to root
    /// 4. Reuse existing bars when they're still valid
    /// 5. Position all bars starting from container's min.y
    /// 6. Update container rect to match total children height
    ///
    /// # Arguments
    ///
    /// * `selected` - The new selected level key
    /// * `rq` - Render queue for scheduling redraws
    /// * `context` - Application context with fonts and other resources
    pub fn set_selected(
        &mut self,
        selected: P::LevelKey,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) {
        let layout = Layout::new(context);

        let first_key = self.first_bar_key();
        let mut last_key = self.last_bar_key();

        self.trim_trailing_children(&selected, &mut last_key);

        let data_by_level = self.prefetch_needed_levels(&selected, context);
        let leaf = self.provider.leaf_for_bar_traversal(&selected, context);

        let mut levels = 1usize;
        let mut index = self.children.len();
        let mut y_max = self.vertical_limit;

        let mut current = leaf.clone();
        loop {
            if self.can_reuse_existing(&first_key, &last_key, &current) {
                let db_index = index - 1;

                let (next_index, new_y_max) =
                    self.reuse_existing_bar_and_separator(index, y_max, layout.thickness);

                if self.children[db_index].rect().min.y < self.rect.min.y {
                    break;
                }

                index = next_index;
                y_max = new_y_max;
                levels += 1;
            } else if self.should_insert_bar(&selected, &current, &data_by_level) {
                let Some(data) = data_by_level.get(&current) else {
                    break;
                };

                let (height, ok) = self.compute_bar_height(&layout, &current, data, y_max);
                if !ok {
                    break;
                }

                self.insert_bar_and_separator(&layout, &current, height, &mut index, &mut y_max);
                levels += 1;
            }

            if levels > self.max_levels || self.provider.is_root(&current, context) {
                break;
            }

            let Some(parent) = self.provider.parent(&current) else {
                break;
            };

            current = parent;
        }

        self.children.drain(..index);

        self.ensure_minimum_bar(&layout, &selected);
        self.remove_extra_leading_separator();

        self.position_and_populate_children(
            &selected,
            &leaf,
            &data_by_level,
            &first_key,
            &last_key,
            rq,
            &mut context.fonts,
        );

        self.rect.max.y = self.children[self.children.len() - 1].rect().max.y;
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Partial));

        self.selected = selected;
    }

    #[inline]
    fn first_bar_key(&self) -> Option<P::LevelKey> {
        self.children
            .first()
            .and_then(|child| child.downcast_ref::<P::Bar>())
            .map(|bar| self.provider.bar_key(bar))
    }

    #[inline]
    fn last_bar_key(&self) -> Option<P::LevelKey> {
        self.children
            .last()
            .and_then(|child| child.downcast_ref::<P::Bar>())
            .map(|bar| self.provider.bar_key(bar))
    }

    /// Trim trailing children that are no longer ancestors of `selected`.
    ///
    /// `children` stores views in alternating order: bar views at even indices and
    /// separator filler views at odd indices. That means one logical navigation
    /// "level" corresponds to **two** entries in `children` (bar + separator).
    ///
    /// `leftovers` is the number of logical levels that should be removed from the
    /// end, so we drain `2 * leftovers` entries.
    ///
    /// `saturating_sub` ensures we never underflow if `2 * leftovers > children.len()`
    /// (in that case we simply drain from `0..` and clear the vector).
    ///
    // TODO(ogkevin): it might be beneficial to refactor this so that `(bar + separator)` is a single component.
    #[inline]
    fn trim_trailing_children(
        &mut self,
        selected: &P::LevelKey,
        last_key: &mut Option<P::LevelKey>,
    ) {
        let Some(last) = last_key.clone() else {
            return;
        };

        let Some((leftovers, ancestor)) =
            find_closest_ancestor_by_provider(&self.provider, &last, selected)
        else {
            return;
        };

        if leftovers == 0 {
            return;
        }

        self.children
            .drain(self.children.len().saturating_sub(2 * leftovers)..);
        *last_key = Some(ancestor);
    }

    #[inline]
    fn prefetch_needed_levels(
        &self,
        selected: &P::LevelKey,
        context: &mut Context,
    ) -> BTreeMap<P::LevelKey, P::LevelData> {
        let leaf_key = self.provider.selected_leaf_key(selected);
        let mut data_by_level = BTreeMap::new();
        let mut current = leaf_key.clone();

        loop {
            let data = self.provider.fetch_level_data(&current, context);
            data_by_level.insert(current.clone(), data);

            if data_by_level.len() >= self.max_levels {
                break;
            }

            if self.provider.is_root(&current, context) {
                break;
            }

            let Some(parent) = self.provider.parent(&current) else {
                break;
            };

            current = parent;
        }

        data_by_level
    }

    /// Returns true if an existing contiguous range of bars can be reused for
    /// the given `current` level when rebuilding the navigation stack.
    ///
    /// Reuse is possible only when both `first` and `last` boundaries are
    /// present and `current` lies between them in the ancestry chain. Concretely,
    /// this method returns true when:
    /// - `provider.is_ancestor(current, last)` (i.e. `current` is an ancestor of `last`)
    /// - `provider.is_ancestor(first, current)` (i.e. `first` is an ancestor of `current`)
    ///
    /// If either `first` or `last` is `None`, reuse is not possible and the
    /// function returns `false`.
    #[inline]
    fn can_reuse_existing(
        &self,
        first: &Option<P::LevelKey>,
        last: &Option<P::LevelKey>,
        current: &P::LevelKey,
    ) -> bool {
        let (Some(first), Some(last)) = (first.as_ref(), last.as_ref()) else {
            return false;
        };

        self.provider.is_ancestor(current, last) && self.provider.is_ancestor(first, current)
    }

    #[inline]
    fn reuse_existing_bar_and_separator(
        &mut self,
        index: usize,
        y_max: i32,
        thickness: i32,
    ) -> (usize, i32) {
        let db_index = index - 1;
        let sep_index = index.saturating_sub(2);

        let y_shift = y_max - self.children[db_index].rect().max.y;
        if let Some(bar) = self.children[db_index].downcast_mut::<P::Bar>() {
            self.provider.shift_bar(bar, pt!(0, y_shift));
        }

        let mut next_y_max = y_max - self.children[db_index].rect().height() as i32;

        if sep_index != db_index {
            let y_shift = next_y_max - self.children[sep_index].rect().max.y;
            *self.children[sep_index].rect_mut() += pt!(0, y_shift);
            next_y_max -= thickness;
        }

        (sep_index, next_y_max)
    }

    /// Decide whether a bar for `current` should be created while rebuilding the stack.
    ///
    /// Rules:
    /// - If `current` is not the `selected` level, we always insert a bar for it.
    ///   This ensures ancestor/ancestor-sibling levels remain visible when traversing.
    /// - If `current` is the `selected` level, we only insert a bar when there is
    ///   content to show. That is determined by consulting `data_by_level` and
    ///   calling the provider's `estimate_line_count`; an estimate > 0 indicates
    ///   the selected level is non-empty and should be represented by a bar.
    ///
    /// If `data_by_level` does not contain an entry for `selected`, the function
    /// conservatively returns `false` (do not insert).
    #[inline]
    fn should_insert_bar(
        &self,
        selected: &P::LevelKey,
        current: &P::LevelKey,
        data_by_level: &BTreeMap<P::LevelKey, P::LevelData>,
    ) -> bool {
        if current != selected {
            return true;
        }

        data_by_level
            .get(selected)
            .map(|data| self.provider.estimate_line_count(selected, data) > 0)
            .unwrap_or(false)
    }

    /// Compute the visual height for a bar representing `key` with `data`, and
    /// indicate whether that bar can be placed without overlapping the container's
    /// top edge.
    ///
    /// Calculation details:
    /// - The provider's `estimate_line_count` is used to determine how many lines
    ///   the bar should display. The count is clamped to a minimum of 1.
    /// - The height formula is:
    ///   height = count * layout.x_height + (count + 1) * layout.padding / 2
    ///   which accounts for per-line x-height and vertical padding between/around lines.
    /// - The returned boolean is `true` when the bar fits between `self.rect.min.y`
    ///   and `y_max` after reserving space for a separator (layout.thickness). If
    ///   placing the bar would push it above `self.rect.min.y` the function returns
    ///   `(height, false)` to signal that the bar cannot be created at the requested
    ///   position.
    ///
    /// Parameters:
    /// - `layout` : Precomputed layout metrics (x_height, padding, thickness).
    /// - `key` / `data` : Provider-specific level identifier and data used to estimate lines.
    /// - `y_max` : The candidate bottom y coordinate (inclusive) where the bar would end.
    ///
    /// Returns:
    /// - `(height, ok)` where `height` is the computed pixel height for the bar and `ok`
    ///   indicates whether the bar can be placed without exceeding the top bound.
    #[inline]
    fn compute_bar_height(
        &self,
        layout: &Layout,
        key: &P::LevelKey,
        data: &P::LevelData,
        y_max: i32,
    ) -> (i32, bool) {
        let count = self.provider.estimate_line_count(key, data).max(1) as i32;
        let height = count * layout.x_height + (count + 1) * layout.padding / 2;

        if y_max - height - layout.thickness < self.rect.min.y {
            return (height, false);
        }

        (height, true)
    }

    /// Insert a bar and its separator into the children vector at the given insertion
    /// index, updating the available bottom coordinate (`y_max`) accordingly.
    ///
    /// The function ensures the visual ordering is correct (bar immediately above
    /// its separator) and handles two insertion scenarios:
    /// - If the current element at `*index` is absent or already a `Filler`, the
    ///   bar is inserted first followed by the separator so the resulting sequence
    ///   is: [bar, separator, ...].
    /// - Otherwise the separator is inserted first and the bar after it so that the
    ///   separator sits directly at `y_max` and the bar sits immediately above it.
    ///
    /// After inserting each element the function subtracts its height from `y_max`
    /// so the caller can continue inserting further elements above the ones just
    /// added. Note that `index` itself is not modified to account for the inserted
    /// children; callers should update it if they need a different insertion anchor.
    ///
    /// ```txt
    ///   layout when a bar and separator are added:
    ///   +----------------------+  <- top (smaller y)
    ///   | BAR (newly inserted) |
    ///   +----------------------+  <- separator immediately below the bar
    ///   | SEPARATOR (filler)   |
    ///   +----------------------+  <- bottom (larger y)
    /// ```
    ///
    /// Vector ordering note:
    /// - Conceptually the visual stack is Bar above Separator (top -> bottom).
    /// - Depending on insertion order and index arithmetic, the vector indices may
    ///   be impacted by the insert() semantics (inserting at the same index shifts
    ///   previously-inserted items to the right). The implementation below follows
    ///   the established convention used by this container to maintain the
    ///   alternating bar/filler pattern.
    #[inline]
    fn insert_bar_and_separator(
        &mut self,
        layout: &Layout,
        key: &P::LevelKey,
        height: i32,
        index: &mut usize,
        y_max: &mut i32,
    ) {
        if self
            .children
            .get(*index)
            .is_none_or(|child| child.is::<Filler>())
        {
            let rect = rect![self.rect.min.x, *y_max - height, self.rect.max.x, *y_max];
            self.children
                .insert(*index, Box::new(self.provider.create_bar(rect, key)));
            *y_max -= height;

            let sep_rect = rect![
                self.rect.min.x,
                *y_max - layout.thickness,
                self.rect.max.x,
                *y_max
            ];
            self.children
                .insert(*index, Box::new(Filler::new(sep_rect, SEPARATOR_NORMAL)));
            *y_max -= layout.thickness;

            return;
        }

        let sep_rect = rect![
            self.rect.min.x,
            *y_max - layout.thickness,
            self.rect.max.x,
            *y_max
        ];
        self.children
            .insert(*index, Box::new(Filler::new(sep_rect, SEPARATOR_NORMAL)));
        *y_max -= layout.thickness;

        let rect = rect![self.rect.min.x, *y_max - height, self.rect.max.x, *y_max];
        self.children
            .insert(*index, Box::new(self.provider.create_bar(rect, key)));
        *y_max -= height;
    }

    #[inline]
    fn ensure_minimum_bar(&mut self, layout: &Layout, selected: &P::LevelKey) {
        if !self.children.is_empty() {
            return;
        }

        let rect = rect![
            self.rect.min.x,
            self.rect.min.y,
            self.rect.max.x,
            self.rect.min.y + layout.min_height
        ];

        self.children
            .push(Box::new(self.provider.create_bar(rect, selected)));
    }

    #[inline]
    fn remove_extra_leading_separator(&mut self) {
        if self.children.len().is_multiple_of(2) {
            self.children.remove(0);
        }
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn position_and_populate_children(
        &mut self,
        selected: &P::LevelKey,
        leaf: &P::LevelKey,
        data_by_level: &BTreeMap<P::LevelKey, P::LevelData>,
        first: &Option<P::LevelKey>,
        last: &Option<P::LevelKey>,
        rq: &mut RenderQueue,
        fonts: &mut Fonts,
    ) {
        let mut current = leaf.clone();
        let y_shift = self.rect.min.y - self.children[0].rect().min.y;

        let mut index = self.children.len();
        while index > 0 {
            index -= 1;

            if self.children[index].is::<Filler>() {
                *self.children[index].rect_mut() += pt!(0, y_shift);
                continue;
            }

            let bar = self.children[index].downcast_mut::<P::Bar>().unwrap();
            self.provider.shift_bar(bar, pt!(0, y_shift));

            let reuse_ok = first
                .as_ref()
                .zip(last.as_ref())
                .is_some_and(|(first, last)| {
                    self.provider.is_ancestor(&current, last)
                        && self.provider.is_ancestor(first, &current)
                });

            if !reuse_ok {
                if let Some(data) = data_by_level.get(&current) {
                    self.provider.update_bar(bar, data, selected, fonts);
                }
            } else if last.as_ref().is_some_and(|last| *last == current) {
                self.provider.update_bar_selection(bar, selected);
            }

            let Some(parent) = self.provider.parent(&current) else {
                break;
            };

            current = parent;
        }

        self.rect.max.y = self.children[self.children.len() - 1].rect().max.y;
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Partial));
    }

    /// Shifts the entire navigation bar and all its children by a delta.
    ///
    /// This is typically used when repositioning the bar within the parent view.
    pub fn shift(&mut self, delta: Point) {
        for child in &mut self.children {
            if let Some(bar) = child.downcast_mut::<P::Bar>() {
                self.provider.shift_bar(bar, delta);
            } else {
                *child.rect_mut() += delta;
            }
        }

        self.rect += delta;
    }

    /// Shrinks the navigation bar by distributing resize across all bars.
    ///
    /// This method proportionally shrinks all bars based on their available space
    /// (height minus minimum height). Bars that cannot shrink further are left at
    /// minimum height. If needed, entire bar+separator pairs are removed from the
    /// top of the stack.
    ///
    /// # Arguments
    ///
    /// * `delta_y` - Target shrink amount (negative number)
    /// * `fonts` - Font registry for resize calculations
    ///
    /// # Returns
    ///
    /// Actual shrink amount achieved (maybe less than requested if minimum heights
    /// prevent further shrinking)
    pub fn shrink(&mut self, delta_y: i32, fonts: &mut Fonts) -> i32 {
        let layout = Layout::new_for_fonts(fonts);
        let bars_count = self.children.len().div_ceil(2);
        let mut values = vec![0; bars_count];

        for (i, value) in values.iter_mut().enumerate().take(bars_count) {
            *value = self.children[2 * i].rect().height() as i32 - layout.min_height;
        }

        let sum: i32 = values.iter().sum();
        let mut y_shift = 0;

        if sum > 0 {
            for i in (0..bars_count).rev() {
                let local_delta_y = ((values[i] as f32 / sum as f32) * delta_y as f32) as i32;
                y_shift += self.resize_child(2 * i, local_delta_y, fonts);
                if y_shift <= delta_y {
                    break;
                }
            }
        }

        while self.children.len() > 1 && y_shift > delta_y {
            let mut dy = 0;
            for child in self.children.drain(0..2) {
                dy += child.rect().height() as i32;
            }

            for child in &mut self.children {
                if let Some(bar) = child.downcast_mut::<P::Bar>() {
                    self.provider.shift_bar(bar, pt!(0, -dy));
                } else {
                    *child.rect_mut() += pt!(0, -dy);
                }
            }

            y_shift -= dy;
        }

        self.rect.max.y = self.children[self.children.len() - 1].rect().max.y;

        y_shift
    }

    #[inline]
    fn resize_child(&mut self, child_index: usize, delta_y: i32, fonts: &mut Fonts) -> i32 {
        let layout = Layout::new_for_fonts(fonts);
        let rect = *self.children[child_index].rect();

        let delta_y_max = (self.vertical_limit - self.rect.max.y).max(0);
        let y_max = (rect.max.y + delta_y.min(delta_y_max)).max(rect.min.y + layout.min_height);

        let height = y_max - rect.min.y;

        let count = ((height - layout.padding / 2) / (layout.x_height + layout.padding / 2)).max(1);
        let height = count * layout.x_height + (count + 1) * layout.padding / 2;
        let y_max = rect.min.y + height;

        let y_shift = y_max - rect.max.y;

        let bar = self.children[child_index].downcast_mut::<P::Bar>().unwrap();
        let resized = self.provider.resize_bar_by(bar, y_shift, fonts);

        for i in child_index + 1..self.children.len() {
            if let Some(bar) = self.children[i].downcast_mut::<P::Bar>() {
                self.provider.shift_bar(bar, pt!(0, resized));
            } else {
                *self.children[i].rect_mut() += pt!(0, resized);
            }
        }

        self.rect.max.y = self.children[self.children.len() - 1].rect().max.y;

        resized
    }
}

/// Layout measurements used by StackNavigationBar to compute bar sizes and spacing.
///
/// This small value object caches DPI- and font-dependent sizing parameters that
/// are computed once and reused across layout and resizing logic:
/// - `thickness`: thickness of the separator between bars (scaled by DPI)
/// - `min_height`: minimum allowed height for a bar (usually SMALL_BAR_HEIGHT - thickness)
/// - `x_height`: font x-height used to compute line heights
/// - `padding`: extra vertical padding inside a bar (derived from min_height and x_height)
///
/// Keeping these values together makes it easier to reason about sizing and to
/// pass a consistent set of layout metrics into functions that need them.
#[derive(Debug, Clone, Copy)]
struct Layout {
    /// Thickness of the separators between bars (in pixels).
    thickness: i32,
    /// Minimum height of a bar (in pixels).
    min_height: i32,
    /// Font x-height used to compute per-line heights (in pixels).
    x_height: i32,
    /// Vertical padding used inside bars (in pixels).
    padding: i32,
}

impl Layout {
    fn new(context: &mut Context) -> Self {
        let dpi = CURRENT_DEVICE.dpi;
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let min_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32 - thickness;
        let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
        let x_height = font.x_heights.0 as i32;
        let padding = min_height - x_height;

        Self {
            thickness,
            min_height,
            x_height,
            padding,
        }
    }

    fn new_for_fonts(fonts: &mut Fonts) -> Self {
        let dpi = CURRENT_DEVICE.dpi;
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let min_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32 - thickness;
        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let x_height = font.x_heights.0 as i32;
        let padding = min_height - x_height;

        Self {
            thickness,
            min_height,
            x_height,
            padding,
        }
    }
}

/// Walks up the ancestry chain starting from `last`, looking for the closest
/// ancestor that is also an ancestor of `selected`.
///
/// This utility uses the provided `NavigationProvider` to traverse parent
/// relationships and to check ancestry. It returns the number of steps taken
/// from `last` to the matching ancestor along with that ancestor key.
///
/// # Arguments
///
/// * `provider` - The domain-specific navigation provider used to query parents
///   and ancestry relationships.
/// * `last` - The starting key from which to walk upwards.
/// * `selected` - The key that we want to find an ancestor for.
///
/// # Returns
///
/// Returns `Some((distance, ancestor_key))` where `distance` is the number of
/// parent hops from `last` to `ancestor_key`. If no such ancestor is found
/// (either because the chain terminates or the search exceeds a safety bound),
/// returns `None`.
///
/// # Safety / limits
///
/// The search is bounded by a fixed iteration limit (128) to avoid pathological
/// or cyclic provider implementations causing an infinite loop.
#[inline]
fn find_closest_ancestor_by_provider<P: NavigationProvider>(
    provider: &P,
    last: &P::LevelKey,
    selected: &P::LevelKey,
) -> Option<(usize, P::LevelKey)> {
    let mut count = 0usize;
    let mut current = last.clone();

    while count < 128 {
        if provider.is_ancestor(&current, selected) {
            return Some((count, current));
        }

        let parent = provider.parent(&current)?;

        current = parent;
        count += 1;
    }

    None
}

impl<P: NavigationProvider + 'static> View for StackNavigationBar<P> {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _hub, bus, _rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        _rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Gesture(crate::gesture::GestureEvent::Swipe {
                dir, start, end, ..
            }) if self.enable_resize && (self.rect.includes(start) || self.rect.includes(end)) => {
                match dir {
                    Dir::North | Dir::South => {
                        let pt = if dir == Dir::North { end } else { start };

                        let bar_index = (0..self.children.len())
                            .step_by(2)
                            .find(|&index| self.children[index].rect().includes(pt));

                        if let Some(index) = bar_index {
                            let delta_y = end.y - start.y;
                            let resized = self.resize_child(index, delta_y, &mut context.fonts);
                            bus.push_back(Event::NavigationBarResized(resized));
                        }

                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {}

    fn rect(&self) -> &Rectangle {
        &self.rect
    }

    fn rect_mut(&mut self) -> &mut Rectangle {
        &mut self.rect
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &self.children
    }

    fn children_mut(&mut self) -> &mut Vec<Box<dyn View>> {
        &mut self.children
    }

    fn id(&self) -> Id {
        self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
    struct Key(i32);

    struct Provider;

    impl NavigationProvider for Provider {
        type LevelKey = Key;
        type LevelData = usize;
        type Bar = Filler;

        fn parent(&self, current: &Self::LevelKey) -> Option<Self::LevelKey> {
            if current.0 == 0 {
                return None;
            }

            Some(Key(current.0 - 1))
        }

        fn is_ancestor(&self, ancestor: &Self::LevelKey, descendant: &Self::LevelKey) -> bool {
            ancestor.0 <= descendant.0
        }

        fn is_root(&self, key: &Self::LevelKey, _context: &Context) -> bool {
            key.0 == 0
        }

        fn fetch_level_data(
            &self,
            key: &Self::LevelKey,
            _context: &mut Context,
        ) -> Self::LevelData {
            key.0 as usize
        }

        fn estimate_line_count(&self, _key: &Self::LevelKey, data: &Self::LevelData) -> usize {
            *data
        }

        fn create_bar(&self, rect: Rectangle, _key: &Self::LevelKey) -> Self::Bar {
            Filler::new(rect, SEPARATOR_NORMAL)
        }

        fn bar_key(&self, _bar: &Self::Bar) -> Self::LevelKey {
            Key(0)
        }

        fn update_bar(
            &self,
            _bar: &mut Self::Bar,
            _data: &Self::LevelData,
            _selected: &Self::LevelKey,
            _fonts: &mut Fonts,
        ) {
        }

        fn update_bar_selection(&self, _bar: &mut Self::Bar, _selected: &Self::LevelKey) {}

        fn resize_bar_by(&self, bar: &mut Self::Bar, delta_y: i32, _fonts: &mut Fonts) -> i32 {
            let rect = *bar.rect();
            let dpi = CURRENT_DEVICE.dpi;
            let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
            let min_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32 - thickness;

            let y_max = (rect.max.y + delta_y).max(rect.min.y + min_height);
            let resized = y_max - rect.max.y;

            bar.rect_mut().max.y = y_max;

            resized
        }

        fn shift_bar(&self, bar: &mut Self::Bar, delta: Point) {
            *bar.rect_mut() += delta;
        }
    }

    #[test]
    fn closest_ancestor_count_is_distance() {
        let provider = Provider;
        let last = Key(5);
        let selected = Key(3);

        let (count, ancestor) =
            find_closest_ancestor_by_provider(&provider, &last, &selected).unwrap();
        assert_eq!(count, 2);
        assert_eq!(ancestor, Key(3));
    }

    #[test]
    fn closest_ancestor_is_none_when_unrelated() {
        let provider = Provider;
        let last = Key(5);
        let selected = Key(-1);

        assert!(find_closest_ancestor_by_provider(&provider, &last, &selected).is_none());
    }

    fn create_test_context_for_nav_bar() -> Context {
        use crate::battery::{Battery, FakeBattery};
        use crate::framebuffer::Pixmap;
        use crate::frontlight::{Frontlight, LightLevels};
        use crate::lightsensor::LightSensor;
        use std::env;
        use std::path::Path;

        let fb = Box::new(Pixmap::new(600, 800, 1)) as Box<dyn Framebuffer>;
        let battery = Box::new(FakeBattery::new()) as Box<dyn Battery>;
        let frontlight = Box::new(LightLevels::default()) as Box<dyn Frontlight>;
        let lightsensor = Box::new(0u16) as Box<dyn LightSensor>;

        let fonts = crate::font::Fonts::load_from(
            Path::new(
                &env::var("TEST_ROOT_DIR").expect("TEST_ROOT_DIR must be set for this test."),
            )
            .to_path_buf(),
        )
        .expect(
            "Failed to load fonts. Tests require font files to be present. \
             Run tests from the project root directory.",
        );

        Context::new(
            fb,
            None,
            crate::library::Library::new(Path::new("/tmp"), crate::settings::LibraryMode::Database)
                .unwrap(),
            crate::settings::Settings::default(),
            fonts,
            battery,
            frontlight,
            lightsensor,
        )
    }

    #[test]
    fn set_selected_with_single_child_no_panic() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 100];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 5, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(0), &mut rq, &mut context);
        assert!(!nav_bar.children.is_empty());

        nav_bar.set_selected(Key(1), &mut rq, &mut context);
        assert!(!nav_bar.children.is_empty());
    }

    #[test]
    fn set_selected_from_empty_state() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 100];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 5, provider, Key(0));
        let mut rq = RenderQueue::new();

        assert!(nav_bar.children.is_empty());

        nav_bar.set_selected(Key(3), &mut rq, &mut context);

        assert!(!nav_bar.children.is_empty());
        assert_eq!(nav_bar.selected, Key(3));
    }

    #[test]
    fn set_selected_reuses_existing_bars() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 200];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 5, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(2), &mut rq, &mut context);
        assert!(!nav_bar.children.is_empty());

        nav_bar.set_selected(Key(3), &mut rq, &mut context);

        assert!(!nav_bar.children.is_empty());
        assert_eq!(nav_bar.selected, Key(3));
    }

    #[test]
    fn set_selected_to_parent_reduces_bars() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 200];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 5, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(5), &mut rq, &mut context);
        assert!(!nav_bar.children.is_empty());

        nav_bar.set_selected(Key(2), &mut rq, &mut context);

        assert!(!nav_bar.children.is_empty());
        assert_eq!(nav_bar.selected, Key(2));
    }

    #[test]
    fn set_selected_handles_max_levels() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 200];
        let max_levels = 3;
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, max_levels, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(10), &mut rq, &mut context);

        assert!(!nav_bar.children.is_empty());
    }

    #[test]
    fn resize_child_with_aggressive_north_swipe_maintains_minimum_height() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 68, 600, 590];
        let vertical_limit = 642;
        let mut nav_bar = StackNavigationBar::new(rect, vertical_limit, 1, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(0), &mut rq, &mut context);
        assert_eq!(nav_bar.children.len(), 1);

        let initial_rect = *nav_bar.children[0].rect();
        let initial_height = initial_rect.height() as i32;

        let aggressive_delta_y = -(initial_height * 2);
        nav_bar.resize_child(0, aggressive_delta_y, &mut context.fonts);

        let dpi = CURRENT_DEVICE.dpi;
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let min_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32 - thickness;

        let final_child_rect = *nav_bar.children[0].rect();
        let final_height = final_child_rect.height() as i32;

        assert!(
            final_height >= min_height,
            "Child bar height {} should be at least min_height {}",
            final_height,
            min_height
        );

        let container_height = nav_bar.rect.max.y - nav_bar.rect.min.y;
        assert!(
            container_height >= min_height,
            "Container height {} should be at least min_height {}. Container rect: {:?}",
            container_height,
            min_height,
            nav_bar.rect
        );

        assert_eq!(
            nav_bar.rect.max.y, final_child_rect.max.y,
            "Container max.y should match last child's max.y"
        );
    }

    #[test]
    fn shrink_proportionally_distributes_across_multiple_bars() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 400];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 5, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(3), &mut rq, &mut context);

        let initial_heights: Vec<i32> = (0..nav_bar.children.len())
            .step_by(2)
            .map(|i| nav_bar.children[i].rect().height() as i32)
            .collect();

        let shrink_amount = -50;
        let actual_shrink = nav_bar.shrink(shrink_amount, &mut context.fonts);

        let final_heights: Vec<i32> = (0..nav_bar.children.len())
            .step_by(2)
            .map(|i| nav_bar.children[i].rect().height() as i32)
            .collect();

        assert!(actual_shrink <= 0, "Should return negative shrink amount");
        assert!(
            actual_shrink <= shrink_amount,
            "Actual shrink should be at most the requested amount (more negative = more shrink)"
        );

        for (initial, final_h) in initial_heights.iter().zip(final_heights.iter()) {
            assert!(
                final_h <= initial,
                "Each bar should shrink or stay same: initial={}, final={}",
                initial,
                final_h
            );
        }
    }

    #[test]
    fn shrink_removes_bars_when_exceeding_available_space() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 300];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 5, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(3), &mut rq, &mut context);

        let initial_bar_count = nav_bar.children.len().div_ceil(2);

        let aggressive_shrink = -500;
        nav_bar.shrink(aggressive_shrink, &mut context.fonts);

        let final_bar_count = nav_bar.children.len().div_ceil(2);

        assert!(
            final_bar_count <= initial_bar_count,
            "Bar count should decrease or stay same when shrinking aggressively"
        );
        assert!(final_bar_count >= 1, "Should always keep at least one bar");
    }

    #[test]
    fn shrink_handles_all_bars_at_minimum_height() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 100];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 2, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(1), &mut rq, &mut context);

        let dpi = CURRENT_DEVICE.dpi;
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let min_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32 - thickness;

        for i in (0..nav_bar.children.len()).step_by(2) {
            let bar = nav_bar.children[i].downcast_mut::<Filler>().unwrap();
            bar.rect_mut().max.y = bar.rect().min.y + min_height;
        }

        let shrink_amount = -20;
        let actual_shrink = nav_bar.shrink(shrink_amount, &mut context.fonts);

        assert!(
            actual_shrink <= 0,
            "When all bars at minimum, shrink should remove bars or do nothing"
        );
    }

    #[test]
    fn resize_child_expansion_respects_vertical_limit() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 200];
        let vertical_limit = 250;
        let mut nav_bar = StackNavigationBar::new(rect, vertical_limit, 3, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(2), &mut rq, &mut context);

        let last_bar_index = ((nav_bar.children.len() - 1) / 2) * 2;
        let initial_container_max = nav_bar.rect.max.y;

        let large_expansion = 200;
        let actual_resize =
            nav_bar.resize_child(last_bar_index, large_expansion, &mut context.fonts);

        let final_container_max = nav_bar.rect.max.y;
        let expected_max = (initial_container_max + actual_resize).min(vertical_limit);

        assert!(
            final_container_max <= vertical_limit,
            "Navigation bar should not exceed vertical_limit: {} > {}",
            final_container_max,
            vertical_limit
        );

        assert_eq!(
            final_container_max, expected_max,
            "Container should expand by actual_resize amount or hit vertical_limit"
        );
    }

    #[test]
    fn resize_child_expansion_shifts_subsequent_children() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 300];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 5, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(3), &mut rq, &mut context);

        if nav_bar.children.len() < 4 {
            return;
        }

        let target_index = 0;
        let initial_rects: Vec<Rectangle> = nav_bar
            .children
            .iter()
            .skip(target_index + 1)
            .map(|child| *child.rect())
            .collect();

        let expansion = 20;
        let actual_resize = nav_bar.resize_child(target_index, expansion, &mut context.fonts);

        let final_rects: Vec<Rectangle> = nav_bar
            .children
            .iter()
            .skip(target_index + 1)
            .map(|child| *child.rect())
            .collect();

        for (initial, final_rect) in initial_rects.iter().zip(final_rects.iter()) {
            let shift = final_rect.min.y - initial.min.y;
            assert_eq!(
                shift, actual_resize,
                "All subsequent children should shift by the actual resize amount"
            );
        }
    }

    #[test]
    fn shift_moves_all_children_and_container() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 200];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 3, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(2), &mut rq, &mut context);

        let initial_container = nav_bar.rect;
        let initial_child_rects: Vec<Rectangle> =
            nav_bar.children.iter().map(|c| *c.rect()).collect();

        let delta = pt!(10, 20);
        nav_bar.shift(delta);

        assert_eq!(
            nav_bar.rect,
            initial_container + delta,
            "Container should shift by delta"
        );

        for (i, initial_rect) in initial_child_rects.iter().enumerate() {
            let expected = *initial_rect + delta;
            assert_eq!(
                *nav_bar.children[i].rect(),
                expected,
                "Child {} should shift by delta",
                i
            );
        }
    }

    #[test]
    fn handle_event_north_swipe_resizes_bar() {
        use crate::gesture::GestureEvent;

        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 100, 600, 300];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 3, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(2), &mut rq, &mut context);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub = tx;
        let mut bus = std::collections::VecDeque::new();

        let start = pt!(300, 200);
        let end = pt!(300, 150);

        let event = Event::Gesture(GestureEvent::Swipe {
            dir: Dir::North,
            start,
            end,
        });

        let handled = nav_bar.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled, "North swipe should be handled");

        let events: Vec<Event> = bus.drain(..).collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::NavigationBarResized(_))),
            "Should emit NavigationBarResized event"
        );
    }

    #[test]
    fn handle_event_south_swipe_resizes_bar() {
        use crate::gesture::GestureEvent;

        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 100, 600, 300];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 3, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(2), &mut rq, &mut context);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub = tx;
        let mut bus = std::collections::VecDeque::new();

        let start = pt!(300, 150);
        let end = pt!(300, 200);

        let event = Event::Gesture(GestureEvent::Swipe {
            dir: Dir::South,
            start,
            end,
        });

        let handled = nav_bar.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled, "South swipe should be handled");

        let events: Vec<Event> = bus.drain(..).collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::NavigationBarResized(_))),
            "Should emit NavigationBarResized event"
        );
    }

    #[test]
    fn handle_event_ignores_swipe_outside_rect() {
        use crate::gesture::GestureEvent;

        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 100, 600, 300];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 3, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(2), &mut rq, &mut context);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub = tx;
        let mut bus = std::collections::VecDeque::new();

        let start = pt!(300, 50);
        let end = pt!(300, 10);

        let event = Event::Gesture(GestureEvent::Swipe {
            dir: Dir::North,
            start,
            end,
        });

        let handled = nav_bar.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            !handled,
            "Swipe outside rect should not be handled when both points are outside"
        );
    }

    #[test]
    fn handle_event_ignores_horizontal_swipe() {
        use crate::gesture::GestureEvent;

        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 100, 600, 300];
        let mut nav_bar = StackNavigationBar::new(rect, rect.max.y, 3, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(2), &mut rq, &mut context);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub = tx;
        let mut bus = std::collections::VecDeque::new();

        let start = pt!(200, 200);
        let end = pt!(400, 200);

        let event = Event::Gesture(GestureEvent::Swipe {
            dir: Dir::East,
            start,
            end,
        });

        let handled = nav_bar.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(!handled, "Horizontal swipe should not be handled");
    }

    #[test]
    fn set_selected_handles_vertical_limit_constraint() {
        let mut context = create_test_context_for_nav_bar();

        let provider = Provider;
        let rect = rect![0, 0, 600, 50];
        let vertical_limit = 100;
        let mut nav_bar = StackNavigationBar::new(rect, vertical_limit, 10, provider, Key(0));
        let mut rq = RenderQueue::new();

        nav_bar.set_selected(Key(10), &mut rq, &mut context);

        assert!(
            nav_bar.rect.max.y <= vertical_limit,
            "Navigation bar should respect vertical_limit even with many levels"
        );

        assert!(
            !nav_bar.children.is_empty(),
            "Should have at least one bar even with tight constraints"
        );
    }
}
