//! Whether a text still sits unsent in a pane's input box (T-371, T-453).
//!
//! A step that types a text and presses Enter must confirm the Enter took (the
//! composer emptied) before the next step types: otherwise a command left in
//! the box joins the packet that follows it (`/new` + packet) and neither is
//! read the way it was meant. The test is T-371's, proven on pi, claude and
//! codex: the text's first line (its first [`NEEDLE_CHARS`] characters) shows
//! in one of the tail's last [`TAIL_LINES`] non-empty lines. The last line
//! alone is not enough — on several CLIs it is a status or border line below
//! the composer.

/// How much of the text's first line identifies it in the pane.
pub const NEEDLE_CHARS: usize = 40;

/// How many of the tail's last non-empty lines must show the text.
pub const TAIL_LINES: usize = 6;

/// The needle a pane must show for `text`: its first line, cut to its first
/// [`NEEDLE_CHARS`] characters. `None` = nothing to look for.
pub fn needle(text: &str) -> Option<String> {
    let needle: String = text
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(NEEDLE_CHARS)
        .collect();
    if needle.trim().is_empty() {
        return None;
    }
    Some(needle)
}

/// Whether one of the tail's last [`TAIL_LINES`] non-empty lines shows
/// `needle`: the text is still in the pane's input box.
pub fn holds(tail: &str, text: &str) -> bool {
    let Some(needle) = needle(text) else {
        return false;
    };
    let lines: Vec<&str> = tail
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = lines.len().saturating_sub(TAIL_LINES);
    lines[start..].iter().any(|line| line.contains(&needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_in_the_input_box_is_held() {
        let tail = "> earlier output\n> Continue T-1: run `gear` first";
        assert!(holds(tail, "Continue T-1: run `gear` first"));
    }

    #[test]
    fn an_empty_composer_does_not_hold_the_text() {
        let tail = "> \n  ? for shortcuts, 0 tokens";
        assert!(!holds(tail, "Continue T-1: run `gear` first"));
    }

    #[test]
    fn only_the_tail_window_counts() {
        let tail = format!("{}\n> \n  ? for shortcuts", "old output\n".repeat(10));
        assert!(!holds(&tail, "Continue T-1: run `gear` first"));
    }

    #[test]
    fn a_blank_first_line_has_no_needle() {
        assert_eq!(needle("\n  second line"), None);
        assert!(!holds("> anything", "\n  second line"));
    }
}
