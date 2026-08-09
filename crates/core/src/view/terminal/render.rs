use crate::color::{BLACK, WHITE};
use crate::font::Fonts;
use crate::framebuffer::{Framebuffer as _, Pixmap};
use crate::geom::{Point, Rectangle};

#[derive(Clone, PartialEq, Default)]
struct CellState {
    contents: String,
    inverse: bool,
    bold: bool,
    is_wide: bool,
    is_wide_continuation: bool,
    has_bg: bool,
}

impl CellState {
    fn from_cell(cell: Option<&vt100::Cell>) -> Self {
        cell.map(|cell| Self {
            contents: cell.contents().to_string(),
            inverse: cell.inverse(),
            bold: cell.bold(),
            is_wide: cell.is_wide(),
            is_wide_continuation: cell.is_wide_continuation(),
            has_bg: !matches!(cell.bgcolor(), vt100::Color::Default),
        })
        .unwrap_or_default()
    }

    fn columns(&self) -> usize {
        if self.is_wide { 2 } else { 1 }
    }
}

fn cells_that_fit(available: i32, cell_size: i32) -> u16 {
    (available.max(0) / cell_size.max(1)).clamp(1, i32::from(u16::MAX)) as u16
}

fn cell_rectangle(
    row: usize,
    col: usize,
    columns: usize,
    render_cols: usize,
    char_width: i32,
    char_height: i32,
) -> Rectangle {
    Rectangle::new(
        Point::new(col as i32 * char_width, row as i32 * char_height),
        Point::new(
            (col + columns).min(render_cols) as i32 * char_width,
            (row + 1) as i32 * char_height,
        ),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CursorState {
    position: (u16, u16),
    visible: bool,
}

fn cursor_cell_to_restore(
    previous: Option<CursorState>,
    current: CursorState,
) -> Option<(u16, u16)> {
    previous
        .filter(|previous| previous.visible && *previous != current)
        .map(|previous| previous.position)
}

pub(super) struct TerminalRenderer {
    char_width: i32,
    char_height: i32,
    baseline_offset: i32,
    font_size: u32,
    dpi: u16,
    previous_screen: Vec<Vec<CellState>>,
    previous_cursor: Option<CursorState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TerminalGeometry {
    pub rows: u16,
    pub cols: u16,
    pub char_width: i32,
    pub char_height: i32,
}

impl TerminalRenderer {
    pub(super) fn calculate_geometry_for_font_size(
        available_width: i32,
        available_height: i32,
        font_size: u32,
        fonts: &mut Fonts,
        dpi: u16,
    ) -> TerminalGeometry {
        let font = &mut fonts.monospace.bold;

        font.set_size(font_size, dpi);

        let plan = font.plan("M", None, None);
        let char_width = plan.width.max(1);
        let line_height = (font.ascender() - font.descender()).max(1);

        TerminalGeometry {
            cols: cells_that_fit(available_width, char_width),
            rows: cells_that_fit(available_height, line_height),
            char_width,
            char_height: line_height,
        }
    }

    pub(super) fn new_with_font_size(
        fonts: &mut Fonts,
        rows: u16,
        cols: u16,
        font_size: u32,
        dpi: u16,
    ) -> Self {
        let font = &mut fonts.monospace.bold;

        font.set_size(font_size, dpi);

        let plan = font.plan("M", None, None);
        let char_width = plan.width.max(1);
        let line_height = (font.ascender() - font.descender()).max(1);
        let baseline_offset = font.ascender();

        let previous_screen = vec![vec![CellState::default(); cols as usize]; rows as usize];

        TerminalRenderer {
            char_width,
            char_height: line_height,
            baseline_offset,
            font_size,
            dpi,
            previous_screen,
            previous_cursor: None,
        }
    }

    pub(super) fn reconstruct_screen(
        &mut self,
        screen: &vt100::Screen,
        pixmap: &mut Pixmap,
        fonts: &mut Fonts,
    ) {
        pixmap.data.fill(WHITE.gray());
        self.previous_screen
            .iter_mut()
            .for_each(|row| row.fill(CellState::default()));
        self.previous_cursor = None;
        self.render_screen(screen, pixmap, fonts);
    }

    /// Render directly from a vt100 Screen to a Pixmap
    pub(super) fn render_screen(
        &mut self,
        screen: &vt100::Screen,
        pixmap: &mut Pixmap,
        fonts: &mut Fonts,
    ) -> Option<Rectangle> {
        let font = &mut fonts.monospace.bold;
        font.set_size(self.font_size, self.dpi);

        let (current_rows, current_cols) = screen.size();
        let cursor = CursorState {
            position: screen.cursor_position(),
            visible: screen.scrollback() == 0 && !screen.hide_cursor(),
        };
        let cursor_changed = self.previous_cursor != Some(cursor);
        let mut dirty_rect: Option<Rectangle> = None;

        // Use the minimum of screen size and our buffer size
        let render_rows = (current_rows as usize).min(self.previous_screen.len());
        let render_cols =
            (current_cols as usize).min(self.previous_screen.first().map(|r| r.len()).unwrap_or(0));

        if let Some((old_row, old_col)) = cursor_cell_to_restore(self.previous_cursor, cursor)
            && (old_row as usize) < render_rows
            && (old_col as usize) < render_cols
        {
            let old_cell = screen.cell(old_row, old_col);
            self.render_vt100_cell(old_row, old_col, old_cell, pixmap, font);
            let cell_rect = cell_rectangle(
                old_row as usize,
                old_col as usize,
                CellState::from_cell(old_cell).columns(),
                render_cols,
                self.char_width,
                self.char_height,
            );
            dirty_rect = Some(cell_rect);
        }

        let mut cursor_cell_changed = false;
        for row in 0..render_rows {
            for col in 0..render_cols {
                let cell = screen.cell(row as u16, col as u16);
                let current_state = CellState::from_cell(cell);

                if current_state != self.previous_screen[row][col] {
                    cursor_cell_changed |= cursor.position == (row as u16, col as u16);
                    self.render_vt100_cell(row as u16, col as u16, cell, pixmap, font);
                    let changed_columns = current_state
                        .columns()
                        .max(self.previous_screen[row][col].columns());
                    self.previous_screen[row][col] = current_state;
                    let cell_rect = cell_rectangle(
                        row,
                        col,
                        changed_columns,
                        render_cols,
                        self.char_width,
                        self.char_height,
                    );
                    if let Some(ref mut rect) = dirty_rect {
                        rect.absorb(&cell_rect);
                    } else {
                        dirty_rect = Some(cell_rect);
                    }
                }
            }
        }

        let (cursor_row, cursor_col) = cursor.position;
        if cursor.visible
            && (cursor_changed || cursor_cell_changed)
            && (cursor_row as usize) < render_rows
            && (cursor_col as usize) < render_cols
        {
            let x = cursor_col as i32 * self.char_width;
            let y = cursor_row as i32 * self.char_height;
            let cursor_rect = Rectangle::new(
                Point::new(x, y + self.char_height - 3),
                Point::new(x + self.char_width, y + self.char_height - 1),
            );
            pixmap.draw_rectangle(&cursor_rect, BLACK);
            if let Some(ref mut rect) = dirty_rect {
                rect.absorb(&cursor_rect);
            } else {
                dirty_rect = Some(cursor_rect);
            }
        }

        self.previous_cursor = Some(cursor);

        dirty_rect
    }

    fn render_vt100_cell(
        &self,
        row: u16,
        col: u16,
        cell: Option<&vt100::Cell>,
        pixmap: &mut Pixmap,
        font: &mut crate::font::Font,
    ) {
        let x = col as i32 * self.char_width;
        let y = row as i32 * self.char_height;

        if let Some(cell) = cell {
            if cell.is_wide_continuation() {
                return;
            }

            let has_bg = !matches!(cell.bgcolor(), vt100::Color::Default);
            let use_inverse = cell.inverse() || has_bg;
            let (fg, bg) = if use_inverse {
                (WHITE, BLACK)
            } else {
                (BLACK, WHITE)
            };

            let cell_width = if cell.is_wide() {
                self.char_width * 2
            } else {
                self.char_width
            };
            let cell_rect = Rectangle::new(
                Point::new(x, y),
                Point::new(x + cell_width, y + self.char_height),
            );

            pixmap.draw_rectangle(&cell_rect, bg);

            let contents = cell.contents();
            if !contents.is_empty() {
                let plan = font.plan(contents, None, None);
                let pt = Point::new(x, y + self.baseline_offset);
                font.render(pixmap, fg, &plan, pt);
            }
        } else {
            let cell_rect = Rectangle::new(
                Point::new(x, y),
                Point::new(x + self.char_width, y + self.char_height),
            );
            pixmap.draw_rectangle(&cell_rect, WHITE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CellState, CursorState, cell_rectangle, cells_that_fit, cursor_cell_to_restore};
    use crate::color::{BLACK, WHITE};
    use crate::font::Fonts;
    use crate::framebuffer::Pixmap;
    use crate::geom::{Point, Rectangle};
    use std::env;
    use std::path::PathBuf;

    #[test]
    fn reconstruction_clears_pixels_missing_from_the_terminal_screen() {
        let mut fonts = Fonts::load_from(PathBuf::from(
            env::var("TEST_ROOT_DIR").expect("TEST_ROOT_DIR must be set for this test"),
        ))
        .expect("failed to load fonts");
        let mut renderer =
            super::TerminalRenderer::new_with_font_size(&mut fonts, 2, 2, 12 * 64, 300);
        let mut parser = vt100::Parser::new(2, 2, 0);
        parser.process(b"\x1b[?25l");
        let mut pixmap = Pixmap::new(100, 100, 1);
        pixmap.data.fill(BLACK.gray());

        renderer.reconstruct_screen(parser.screen(), &mut pixmap, &mut fonts);

        assert!(pixmap.data.iter().all(|pixel| *pixel == WHITE.gray()));
    }

    #[test]
    fn grid_dimensions_never_claim_cells_that_do_not_fit() {
        assert_eq!(cells_that_fit(599, 30), 19);
        assert_eq!(cells_that_fit(29, 30), 1);
        assert_eq!(cells_that_fit(0, 30), 1);
        assert_eq!(cells_that_fit(i32::MAX, 1), u16::MAX);
    }

    #[test]
    fn changed_wide_cells_dirty_both_columns() {
        let wide = CellState {
            is_wide: true,
            ..CellState::default()
        };
        let replacement = CellState {
            contents: "界".to_string(),
            is_wide: true,
            ..CellState::default()
        };

        assert_eq!(wide.columns().max(replacement.columns()), 2);
        assert_eq!(
            cell_rectangle(1, 2, 2, 10, 12, 20),
            Rectangle::new(Point::new(24, 20), Point::new(48, 40))
        );
    }

    #[test]
    fn wide_dirty_regions_are_clipped_to_the_viewport() {
        assert_eq!(
            cell_rectangle(0, 9, 2, 10, 12, 20),
            Rectangle::new(Point::new(108, 0), Point::new(120, 20))
        );
    }

    #[test]
    fn hiding_a_stationary_cursor_restores_its_cell() {
        let position = (2, 3);
        let previous = CursorState {
            position,
            visible: true,
        };
        let current = CursorState {
            position,
            visible: false,
        };

        assert_eq!(
            cursor_cell_to_restore(Some(previous), current),
            Some(position)
        );
    }

    #[test]
    fn showing_a_stationary_cursor_does_not_restore_its_cell() {
        let position = (2, 3);
        let previous = CursorState {
            position,
            visible: false,
        };
        let current = CursorState {
            position,
            visible: true,
        };

        assert_eq!(cursor_cell_to_restore(Some(previous), current), None);
    }
}
