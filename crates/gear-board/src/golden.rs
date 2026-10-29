//! The golden planes of a frame (T-276's format, T-340).
//!
//! A frame is read back as three planes of one character per cell: the text,
//! the colour and the reverse video. The golden files of the prototype
//! (`gear/voyages/M5/board/golden`) are written in these planes, so a test
//! compares the real board with the prototype cell by cell and says where a
//! colour differs, not only where the text does.

use crate::frame::Canvas;
use crate::style::Color;

/// One frame as three planes: one `String` per canvas row, one character per
/// cell. The rows are the same length (`width`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planes {
    /// The character of the cell.
    pub text: Vec<String>,
    /// The colour of the cell: `.` for none, `o` Ok, `w` Warn, `b` Bad, `d`
    /// Dim, `a` Acc, `m` Mag, `u` Blu; uppercase when the cell is bold, `*`
    /// when it is bold with no colour.
    pub colour: Vec<String>,
    /// `R` when the cell is in reverse video, else `.`.
    pub reverse: Vec<String>,
}

/// The three planes of a canvas, in the format of T-276's golden files.
pub fn planes(canvas: &Canvas) -> Planes {
    let width = canvas.width();
    let height = canvas.height();
    let mut text = Vec::with_capacity(height);
    let mut colour = Vec::with_capacity(height);
    let mut reverse = Vec::with_capacity(height);
    for y in 0..height {
        let mut text_row = String::with_capacity(width);
        let mut colour_row = String::with_capacity(width);
        let mut reverse_row = String::with_capacity(width);
        for x in 0..width {
            text_row.push(canvas.char_at(x, y));
            let style = canvas.style_at(x, y);
            colour_row.push(colour_char(style.fg, style.bold));
            reverse_row.push(if style.reverse { 'R' } else { '.' });
        }
        text.push(text_row);
        colour.push(colour_row);
        reverse.push(reverse_row);
    }
    Planes { text, colour, reverse }
}

/// The character of a colour in the colour plane.
fn colour_char(fg: Option<Color>, bold: bool) -> char {
    let character = match fg {
        None => {
            return if bold { '*' } else { '.' };
        }
        Some(Color::Ok) => 'o',
        Some(Color::Warn) => 'w',
        Some(Color::Bad) => 'b',
        Some(Color::Dim) => 'd',
        Some(Color::Acc) => 'a',
        Some(Color::Mag) => 'm',
        Some(Color::Blu) => 'u',
    };
    if bold {
        character.to_ascii_uppercase()
    } else {
        character
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Canvas;
    use crate::style::{Color, Style};

    #[test]
    fn the_planes_read_a_frame_cell_by_cell() {
        let mut canvas = Canvas::new(20, 8);
        canvas.put_styled(1, 0, "ab", Style::fg(Color::Ok).and_bold());
        canvas.put_styled(3, 0, "c", Style::bold());
        canvas.light_row(1);

        let planes = planes(&canvas);

        assert_eq!(planes.text[0], format!(" abc{}", " ".repeat(16)));
        assert_eq!(planes.colour[0], format!(".OO*{}", ".".repeat(16)));
        assert_eq!(planes.reverse[0], ".".repeat(20));
        assert_eq!(planes.text[1], " ".repeat(20));
        assert_eq!(planes.colour[1], ".".repeat(20));
        assert_eq!(planes.reverse[1], format!(".{}{}", "R".repeat(18), "."));
    }

    #[test]
    fn put_styled_never_writes_the_last_column() {
        let mut canvas = Canvas::new(20, 8);
        canvas.put_styled(17, 2, "xyz", Style::fg(Color::Warn));

        assert_eq!(canvas.char_at(17, 2), 'x');
        assert_eq!(canvas.char_at(18, 2), 'y');
        assert_eq!(canvas.char_at(19, 2), ' ', "the last column is never written");
        // A text that starts at the last column writes nothing.
        canvas.put_styled(19, 3, "abc", Style::PLAIN);
        assert_eq!(canvas.char_at(19, 3), ' ');
        assert_eq!(canvas.style_at(19, 3), Style::PLAIN);
    }

    #[test]
    fn light_row_lights_the_columns_the_prototype_lights() {
        let mut canvas = Canvas::new(20, 8);
        canvas.put(0, 2, "abcdefghij");
        canvas.light_row(2);

        assert_eq!(canvas.style_at(0, 2), Style::PLAIN, "column 0 stays");
        assert_eq!(
            canvas.style_at(1, 2),
            Style { fg: None, bold: false, reverse: true }
        );
        assert_eq!(
            canvas.style_at(18, 2),
            Style { fg: None, bold: false, reverse: true }
        );
        assert_eq!(canvas.style_at(19, 2), Style::PLAIN, "the last column stays");
        assert_eq!(canvas.char_at(1, 2), 'b', "the characters stay");
        // A row off the canvas is not lit and does not panic.
        canvas.light_row(8);
    }

    #[test]
    fn copy_rows_copies_the_cells_and_their_styles() {
        let mut from = Canvas::new(20, 8);
        from.put_styled(1, 2, "ab", Style::fg(Color::Ok).and_bold());
        from.light_row(3);
        let mut canvas = Canvas::new(20, 8);
        canvas.copy_rows(&from, 2, 5, 3);

        assert_eq!(canvas.char_at(1, 5), 'a');
        assert_eq!(canvas.style_at(1, 5), Style::fg(Color::Ok).and_bold());
        assert_eq!(canvas.char_at(2, 5), 'b');
        assert_eq!(canvas.style_at(1, 6).reverse, true, "the lit row keeps its style");
        assert_eq!(canvas.style_at(0, 6), Style::PLAIN, "the lit row's first column stays");
        assert_eq!(canvas.text().lines().nth(5).unwrap_or_default(), " ab");
        // Rows off either canvas are not copied and do not panic.
        let before = canvas.text();
        from.put_styled(1, 7, "z", Style::fg(Color::Bad));
        canvas.copy_rows(&from, 7, 8, 4);
        canvas.copy_rows(&from, 8, 0, 4);
        assert_eq!(canvas.text(), before);
    }

    #[test]
    fn a_cell_outside_the_canvas_is_a_space() {
        let canvas = Canvas::new(20, 8);

        assert_eq!(canvas.char_at(20, 0), ' ');
        assert_eq!(canvas.char_at(0, 8), ' ');
        assert_eq!(canvas.width(), 20);
        assert_eq!(canvas.height(), 8);
    }
}
