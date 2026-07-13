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

pub struct TerminalRenderer {
    char_width: i32,
    char_height: i32,
    baseline_offset: i32,
    font_size: u32,
    dpi: u16,
    previous_screen: Vec<Vec<CellState>>,
    previous_cursor: (u16, u16),
}

impl TerminalRenderer {
    pub fn calculate_grid_for_font_size(
        available_width: i32,
        available_height: i32,
        font_size: u32,
        fonts: &mut Fonts,
        dpi: u16,
    ) -> (u16, u16) {
        let font = &mut fonts.monospace.bold;

        font.set_size(font_size, dpi);

        let plan = font.plan("M", None, None);
        let char_width = plan.width.max(1);
        let line_height = (font.ascender() - font.descender()).max(1);

        let cols = (available_width / char_width).max(20) as u16;
        let rows = (available_height / line_height).max(1) as u16;

        (rows, cols)
    }

    pub fn new_with_font_size(
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
            previous_cursor: (0, 0),
        }
    }

    /// Render directly from a vt100 Screen to a Pixmap
    pub fn render_screen(
        &mut self,
        screen: &vt100::Screen,
        pixmap: &mut Pixmap,
        fonts: &mut Fonts,
    ) -> Option<Rectangle> {
        let font = &mut fonts.monospace.bold;
        font.set_size(self.font_size, self.dpi);

        let (current_rows, current_cols) = screen.size();
        let cursor_pos = screen.cursor_position();
        let mut dirty_rect: Option<Rectangle> = None;

        // Use the minimum of screen size and our buffer size
        let render_rows = (current_rows as usize).min(self.previous_screen.len());
        let render_cols =
            (current_cols as usize).min(self.previous_screen.first().map(|r| r.len()).unwrap_or(0));

        // Check for cursor movement
        if cursor_pos != self.previous_cursor {
            let (old_row, old_col) = self.previous_cursor;
            if (old_row as usize) < render_rows && (old_col as usize) < render_cols {
                let old_cell = screen.cell(old_row, old_col);
                self.render_vt100_cell(old_row, old_col, old_cell, pixmap, font);
                let cell_rect = Rectangle::new(
                    Point::new(
                        old_col as i32 * self.char_width,
                        old_row as i32 * self.char_height,
                    ),
                    Point::new(
                        (old_col as i32 + 1) * self.char_width,
                        (old_row as i32 + 1) * self.char_height,
                    ),
                );
                dirty_rect = Some(cell_rect);
            }
            self.previous_cursor = cursor_pos;
        }

        // Compare cells and render only changed ones
        for row in 0..render_rows {
            for col in 0..render_cols {
                let cell = screen.cell(row as u16, col as u16);
                let current_state = cell
                    .map(|c| CellState {
                        contents: c.contents().to_string(),
                        inverse: c.inverse(),
                        bold: c.bold(),
                        is_wide: c.is_wide(),
                        is_wide_continuation: c.is_wide_continuation(),
                        has_bg: !matches!(c.bgcolor(), vt100::Color::Default),
                    })
                    .unwrap_or_default();

                if current_state != self.previous_screen[row][col] {
                    self.render_vt100_cell(row as u16, col as u16, cell, pixmap, font);
                    self.previous_screen[row][col] = current_state;
                    let cell_rect = Rectangle::new(
                        Point::new(col as i32 * self.char_width, row as i32 * self.char_height),
                        Point::new(
                            (col as i32 + 1) * self.char_width,
                            (row as i32 + 1) * self.char_height,
                        ),
                    );
                    if let Some(ref mut rect) = dirty_rect {
                        rect.absorb(&cell_rect);
                    } else {
                        dirty_rect = Some(cell_rect);
                    }
                }
            }
        }

        // Draw cursor on current position
        let (cursor_row, cursor_col) = cursor_pos;
        if (cursor_row as usize) < render_rows && (cursor_col as usize) < render_cols {
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

        dirty_rect
    }

    pub fn clear_screen_state(&mut self) {
        for row in &mut self.previous_screen {
            for cell in row {
                *cell = CellState::default();
            }
        }
        self.previous_cursor = (0, 0);
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
