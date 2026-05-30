use super::category::Category;
use super::category_button::CategoryButton;
use crate::color::TEXT_BUMP_SMALL;
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{Fonts, NORMAL_STYLE, font_from_style};
use crate::framebuffer::Framebuffer;
use crate::geom::{Point, Rectangle, big_half, divide, small_half};
use crate::view::filler::Filler;
use crate::view::{Align, Bus, Event, Hub, ID_FEEDER, Id, RenderQueue, View};

/// Horizontal navigation bar displaying category tabs.
///
/// This component shows all available settings categories as horizontal tabs,
/// wrapping onto additional rows when the categories no longer fit on a single
/// line. The selected category is visually highlighted. The bar's height is
/// determined by `SettingsCategoryProvider::estimate_line_count`, which must
/// match the number of rows required.
///
/// # Structure
///
/// ```text
/// ┌─────────────────────────────────────────────┐
/// │ [General] [Libraries] [Intermissions] ...   │
/// │ [OtherCategory] ...                         │
/// └─────────────────────────────────────────────┘
/// ```
pub struct CategoryNavigationBar {
    id: Id,
    pub rect: Rectangle,
    children: Vec<Box<dyn View>>,
    pub selected: Category,
}

impl CategoryNavigationBar {
    #[cfg_attr(feature = "tracing", tracing::instrument())]
    pub fn new(rect: Rectangle, selected: Category) -> Self {
        let id = ID_FEEDER.next();

        CategoryNavigationBar {
            id,
            rect,
            children: Vec::new(),
            selected,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fonts)))]
    pub fn update_content(&mut self, selected: Category, fonts: &mut Fonts) {
        self.selected = selected;
        self.children.clear();
        self.children = Self::build_category_buttons(self.rect, selected, fonts);
    }

    /// Layout all category buttons in rows, distributing vertical space evenly.
    ///
    /// This method implements a two-pass layout algorithm:
    ///
    /// ## Pass 1: Horizontal measurement
    /// Each category is measured to determine its button width. Categories are
    /// packed left-to-right; when a button doesn't fit, a new row begins.
    ///
    /// ## Pass 2: Vertical distribution
    /// Available vertical space is divided evenly using the baseline method (same
    /// as `DirectoriesBar`). For `row_count` rows, the total non-text space is
    /// divided into `row_count + 1` gaps. Each row claims `big_half` of the gap
    /// above it and `small_half` of the gap below. This ensures consistent
    /// centering regardless of the number of rows.
    ///
    /// ## Child views:
    /// Background fillers are added to cover all non-button regions (top strip,
    /// left margin per row, trailing space per row, bottom strip) to prevent
    /// stale framebuffer content from showing through.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(fonts)))]
    fn build_category_buttons(
        rect: Rectangle,
        selected: Category,
        fonts: &mut Fonts,
    ) -> Vec<Box<dyn View>> {
        let mut children = Vec::new();
        let categories = Category::all();
        let dpi = CURRENT_DEVICE.dpi;
        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let padding = font.em() as i32;
        let x_height = font.x_heights.0 as i32;
        let background = TEXT_BUMP_SMALL[0];

        let mut rows: Vec<Vec<(Category, i32)>> = vec![Vec::new()];
        let mut x_pos = rect.min.x + padding / 2;

        for category in categories.iter() {
            let plan = font.plan(category.label(), None, None);
            let button_width = plan.width + padding;

            if x_pos + button_width > rect.max.x && !rows.last().unwrap().is_empty() {
                rows.push(Vec::new());
                x_pos = rect.min.x + padding / 2;
            }

            rows.last_mut().unwrap().push((*category, button_width));
            x_pos += button_width;
        }

        let row_count = rows.len();
        let vertical_space = rect.height() as i32 - row_count as i32 * x_height;
        let baselines = divide(vertical_space, row_count as i32 + 1);

        let mut row_top = rect.min.y + small_half(baselines[0]);

        children.push(Box::new(Filler::new(
            rect![rect.min.x, rect.min.y, rect.max.x, row_top],
            background,
        )) as Box<dyn View>);

        for (row_index, row) in rows.iter().enumerate() {
            let row_height =
                big_half(baselines[row_index]) + x_height + small_half(baselines[row_index + 1]);

            let mut x_pos = rect.min.x + padding / 2;

            children.push(Box::new(Filler::new(
                rect![rect.min.x, row_top, x_pos, row_top + row_height],
                background,
            )) as Box<dyn View>);

            for (category, button_width) in row {
                let button_rect = rect![x_pos, row_top, x_pos + button_width, row_top + row_height];

                children.push(Box::new(CategoryButton::new(
                    button_rect,
                    *category,
                    *category == selected,
                    Align::Left(padding / 2),
                )) as Box<dyn View>);

                x_pos += button_width;
            }

            if x_pos < rect.max.x {
                children.push(Box::new(Filler::new(
                    rect![x_pos, row_top, rect.max.x, row_top + row_height],
                    background,
                )) as Box<dyn View>);
            }

            row_top += row_height;
        }

        if row_top < rect.max.y {
            children.push(Box::new(Filler::new(
                rect![rect.min.x, row_top, rect.max.x, rect.max.y],
                background,
            )) as Box<dyn View>);
        }

        children
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fonts)))]
    pub fn update_selection(&mut self, selected: Category, fonts: &mut Fonts) {
        if self.selected == selected {
            return;
        }

        self.update_content(selected, fonts);
    }

    pub fn resize_by(&mut self, _delta_y: i32, _fonts: &mut Fonts) -> i32 {
        unimplemented!("there is no need for this bar to be resizable");
    }

    pub fn shift(&mut self, delta: Point) {
        self.rect += delta;
        for child in &mut self.children {
            *child.rect_mut() += delta;
        }
    }
}

impl View for CategoryNavigationBar {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _hub, _bus, _rq, _context), fields(event = ?_evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        _evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        false
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
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
