//! The colours of the board: the prototype's palette (`gear/voyages/M4/board`).
//!
//! A cell of the screen has a style: a colour from a small fixed set, bold
//! and reverse video. A [`Line`] is a text with one style per character; the
//! views build lines and the canvas turns them into the terminal's SGR codes.

/// The colours of the prototype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Green: on time, done, merged.
    Ok,
    /// Yellow: over the estimate, worth a look.
    Warn,
    /// Red: past the eta, idle, out of quota.
    Bad,
    /// Grey: the secondary text.
    Dim,
    /// Cyan: the accent, the review.
    Acc,
    /// Magenta.
    Mag,
    /// Blue.
    Blu,
}

impl Color {
    /// The SGR code of the colour (the foreground).
    fn code(self) -> &'static str {
        match self {
            Color::Ok => "32",
            Color::Warn => "33",
            Color::Bad => "31",
            Color::Dim => "90",
            Color::Acc => "36",
            Color::Mag => "35",
            Color::Blu => "34",
        }
    }
}

/// The style of a cell: a colour, bold, reverse video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bold: bool,
    pub reverse: bool,
}

impl Style {
    /// No style: the terminal's own.
    pub const PLAIN: Style = Style {
        fg: None,
        bold: false,
        reverse: false,
    };

    /// A colour.
    pub const fn fg(color: Color) -> Style {
        Style {
            fg: Some(color),
            bold: false,
            reverse: false,
        }
    }

    /// Bold, without a colour.
    pub const fn bold() -> Style {
        Style {
            fg: None,
            bold: true,
            reverse: false,
        }
    }

    /// The same style, bold.
    pub const fn and_bold(self) -> Style {
        Style { bold: true, ..self }
    }

    /// The same style, in reverse video when `on`.
    pub const fn reversed_if(self, on: bool) -> Style {
        Style {
            reverse: self.reverse || on,
            ..self
        }
    }

    /// Does the style show anything on a space? Only reverse video does.
    pub fn shows_on_space(self) -> bool {
        self.reverse
    }

    /// The SGR sequence of the style; `colour` false drops the colour (NO_COLOR)
    /// and keeps bold and reverse video. Empty when there is nothing to set.
    pub fn sgr(self, colour: bool) -> String {
        let mut codes: Vec<&str> = Vec::new();
        if self.bold {
            codes.push("1");
        }
        if self.reverse {
            codes.push("7");
        }
        if colour {
            if let Some(color) = self.fg {
                codes.push(color.code());
            }
        }
        if codes.is_empty() {
            String::new()
        } else {
            format!("\u{1b}[{}m", codes.join(";"))
        }
    }
}

/// The colour of a lane: by its order, cycling through the prototype's lane
/// colours in its lane order (hull cyan, deck magenta, keel yellow, mast
/// blue). A lane that is not in the order is grey.
pub fn lane_color<S: AsRef<str>>(lanes: &[S], lane: &str) -> Color {
    const CYCLE: [Color; 4] = [Color::Acc, Color::Mag, Color::Warn, Color::Blu];
    match lanes.iter().position(|name| name.as_ref() == lane) {
        Some(index) => CYCLE[index % CYCLE.len()],
        None => Color::Dim,
    }
}

/// The width of a column of lane names: the longest name, never less than
/// `min`, plus `gap` spaces after it.
pub fn lane_width<'a, I: IntoIterator<Item = &'a str>>(names: I, min: usize, gap: usize) -> usize {
    names
        .into_iter()
        .map(|name| name.chars().count() + gap)
        .max()
        .unwrap_or(0)
        .max(min)
}

/// A text with one style per character.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    pub text: String,
    pub styles: Vec<Style>,
}

impl Line {
    /// An empty line.
    pub fn new() -> Line {
        Line::default()
    }

    /// A text without a style.
    pub fn plain(text: impl Into<String>) -> Line {
        Line::new().push(text, Style::PLAIN)
    }

    /// A text in one style.
    pub fn styled(text: impl Into<String>, style: Style) -> Line {
        Line::new().push(text, style)
    }

    /// The line with a text in a style after it.
    pub fn push(mut self, text: impl Into<String>, style: Style) -> Line {
        let text = text.into();
        self.styles
            .extend(std::iter::repeat_n(style, text.chars().count()));
        self.text.push_str(&text);
        self
    }

    /// The line padded with spaces (no style) to `width` characters.
    pub fn pad(self, width: usize) -> Line {
        let count = self.text.chars().count();
        if count >= width {
            return self;
        }
        self.push(" ".repeat(width - count), Style::PLAIN)
    }

    /// The number of characters.
    pub fn len(&self) -> usize {
        self.styles.len()
    }

    /// Is the line empty?
    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }

    /// The characters and their styles.
    pub fn cells(&self) -> impl Iterator<Item = (char, Style)> + '_ {
        self.text.chars().zip(self.styles.iter().copied())
    }
}

impl From<String> for Line {
    fn from(text: String) -> Line {
        Line::plain(text)
    }
}

impl From<&str> for Line {
    fn from(text: &str) -> Line {
        Line::plain(text)
    }
}

/// Is colour allowed? `NO_COLOR` set to anything but the empty text turns it
/// off (no-color.org).
pub fn colour_allowed(no_color: Option<&std::ffi::OsStr>) -> bool {
    no_color.map(|value| value.is_empty()).unwrap_or(true)
}
