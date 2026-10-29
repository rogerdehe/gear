//! The terminal the board draws on: ANSI, raw mode, one key at a time.
//!
//! The board owns the screen while it is open: it takes it over, overwrites
//! the frame in place (T-403: nothing is cleared whole, so the page does not
//! blink) and gives the terminal back when the user quits.

use std::io::Write;
use std::os::fd::{AsRawFd, RawFd};
use std::time::Duration;

/// What the terminal sends the board: a key or a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A key press.
    Key(char),
    /// The arrow up.
    Up,
    /// The arrow down.
    Down,
    /// The arrow left.
    Left,
    /// The arrow right.
    Right,
    /// The page up (T-411): the selection moves up a screenful.
    PageUp,
    /// The page down (T-411): the selection moves down a screenful.
    PageDown,
    /// The shift-tab (the tab before).
    BackTab,
    /// A click (the SGR mouse): the button (0 = the left one) and the
    /// position, counted from 1.
    Mouse { button: u8, x: u16, y: u16 },
}

/// Where the board's key presses come from. The real one is the terminal; a
/// test feeds its own.
pub trait Keys {
    /// The next event, waiting up to `timeout`; `None` on timeout.
    fn next_event(&mut self, timeout: Duration) -> Option<Event>;

    /// The size of the screen the board draws on: the columns and the rows.
    ///
    /// The terminal knows it; a test says how big its screen is.
    fn size(&self) -> (usize, usize);
}

/// Is the board running on a terminal? Without one the board is printed once
/// instead of taking over a screen nobody can see.
pub fn is_terminal() -> bool {
    // SAFETY: isatty only reads the descriptor.
    unsafe { libc::isatty(libc::STDOUT_FILENO) == 1 && libc::isatty(libc::STDIN_FILENO) == 1 }
}

/// The keyboard of the terminal the board runs in.
pub struct Terminal {
    stdin: RawFd,
    /// The mode to restore when the board leaves; `None` when the terminal
    /// refused to change (not a terminal at all).
    saved: Option<libc::termios>,
}

impl Terminal {
    /// The keyboard of the terminal the board runs in.
    pub fn new() -> Terminal {
        Terminal::on(std::io::stdin().as_raw_fd())
    }

    /// The keyboard of a given terminal. A test drives a pty this way, so
    /// that the raw-mode path is the one that is really exercised.
    pub fn on(fd: RawFd) -> Terminal {
        let saved = raw_mode(fd);
        Terminal { stdin: fd, saved }
    }
}

impl Keys for Terminal {
    fn next_event(&mut self, timeout: Duration) -> Option<Event> {
        self.saved?;
        let mut fds = libc::pollfd { fd: self.stdin, events: libc::POLLIN, revents: 0 };
        let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: `fds` is one pollfd we own; the call only waits.
        let ready = unsafe { libc::poll(&mut fds, 1, millis) };
        if ready <= 0 {
            return None;
        }
        let mut byte = [0u8; 1];
        // SAFETY: the buffer is one byte long and the fd is in raw mode.
        let read = unsafe { libc::read(self.stdin, byte.as_mut_ptr() as *mut libc::c_void, 1) };
        if read != 1 {
            return None;
        }
        if byte[0] != 0x1b {
            // One byte is one key for everything the board listens for.
            return Some(Event::Key(byte[0] as char));
        }
        // An escape: the arrows, the shift-tab and the mouse.
        let rest = self.read_sequence()?;
        match rest.as_bytes().first() {
            // The page keys end with `~` (T-411): `[5~` is the page up and
            // `[6~` the page down. The mouse keeps its own sequence: the wheel
            // arrives as the SGR buttons 64 and 65.
            Some(b'[') => match rest.as_str() {
                "[5~" => Some(Event::PageUp),
                "[6~" => Some(Event::PageDown),
                _ => match rest.as_bytes().get(1) {
                    Some(b'A') => Some(Event::Up),
                    Some(b'B') => Some(Event::Down),
                    Some(b'C') => Some(Event::Right),
                    Some(b'D') => Some(Event::Left),
                    Some(b'Z') => Some(Event::BackTab),
                    Some(b'<') => mouse_event(&rest),
                    _ => Some(Event::Key('\u{1b}')),
                },
            },
            _ => Some(Event::Key('\u{1b}')),
        }
    }

    fn size(&self) -> (usize, usize) {
        terminal_size(self.stdin)
    }
}

/// The size of a terminal: the columns and the rows.
///
/// The default (120 columns, 40 rows) is used when the terminal does not say:
/// a pipe has no size, and the board still has to draw.
pub fn terminal_size(fd: RawFd) -> (usize, usize) {
    let mut window: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: `window` is a winsize to fill in; the ioctl only writes it.
    let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut window) };
    if ok != 0 || window.ws_col == 0 || window.ws_row == 0 {
        return (DEFAULT_WIDTH, DEFAULT_ROWS);
    }
    (window.ws_col as usize, window.ws_row as usize)
}

/// The size the board falls back to when the terminal does not say.
pub const DEFAULT_WIDTH: usize = 120;
/// The size the board falls back to when the terminal does not say.
pub const DEFAULT_ROWS: usize = 40;

impl Terminal {
    /// Read the rest of an escape sequence: what follows the escape byte.
    fn read_sequence(&mut self) -> Option<String> {
        let mut out = String::new();
        // A sequence arrives at once; a few bytes are enough for the arrows
        // and for the mouse.
        for _ in 0..32 {
            let mut byte = [0u8; 1];
            // SAFETY: the buffer is one byte long and the fd is in raw mode.
            let read = unsafe { libc::read(self.stdin, byte.as_mut_ptr() as *mut libc::c_void, 1) };
            if read != 1 {
                break;
            }
            out.push(byte[0] as char);
            // The mouse sequence ends with M or m, the page keys with `~`.
            if byte[0] == b'M' || byte[0] == b'm' || byte[0] == b'~' {
                break;
            }
            // The arrows and the shift-tab end after their last letter.
            if matches!(out.as_bytes(), [b'[', b'A'] | [b'[', b'B'] | [b'[', b'C'] | [b'[', b'D'] | [b'[', b'Z']) {
                break;
            }
        }
        Some(out)
    }
}

/// The click of an SGR mouse sequence: `[<{button};{x};{y}M` (the escape
/// byte is read before).
fn mouse_event(sequence: &str) -> Option<Event> {
    let rest = sequence.strip_prefix("[<")?;
    let rest = rest.strip_suffix('M').or_else(|| rest.strip_suffix('m'))?;
    let mut parts = rest.split(';');
    let button: u8 = parts.next()?.parse().ok()?;
    let x: u16 = parts.next()?.parse().ok()?;
    let y: u16 = parts.next()?.parse().ok()?;
    Some(Event::Mouse { button, x, y })
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(saved) = self.saved {
            // SAFETY: `saved` came from tcgetattr on this descriptor.
            unsafe { libc::tcsetattr(self.stdin, libc::TCSANOW, &saved) };
        }
    }
}

/// Take over the screen: the alternate buffer keeps the board out of the
/// terminal's scrollback, the cursor stays out of the way, and the mouse
/// reports its clicks (the SGR mode, which carries the position).
pub fn enter(out: &mut dyn Write) -> std::io::Result<()> {
    write!(out, "\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h")?;
    out.flush()
}

/// Give the terminal back.
pub fn leave(out: &mut dyn Write) -> std::io::Result<()> {
    // The mouse reporting is switched off again: a terminal that keeps it on
    // sends its clicks into the user's shell.
    write!(out, "\x1b[?1006l\x1b[?1000l\x1b[?25h\x1b[?1049l")?;
    out.flush()
}

/// Draw `text` over the last frame, in place (T-403).
///
/// The frame is written inside a synchronized update (`\x1b[?2026h` ...
/// `\x1b[?2026l`), so the terminal shows all of it or none of it, and every
/// line erases its rest with `\x1b[K` before the newline: a line that got
/// shorter leaves nothing of the old one behind. `\x1b[J` erases below the
/// last line. The screen is never cleared whole (`\x1b[2J`): that clear is
/// what made the page blink. (`[2026` is ignored by a terminal that does not
/// know it: the frame is still a full overwrite from the home position.)
///
/// The canvas is written **without** a trailing newline (F2): a newline after
/// the footer's row scrolls the screen by one line, so the title leaves the
/// top and every visible row sits one line above where the hit testing
/// expects it.
pub fn draw(out: &mut dyn Write, text: &str) -> std::io::Result<()> {
    write!(out, "\x1b[?2026h\x1b[H")?;
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        // Every line erases its rest. The lines are kept apart by the newline
        // the board has always written; the last line's one is dropped (F2).
        write!(out, "{line}\x1b[K")?;
        if lines.peek().is_some() {
            write!(out, "\n")?;
        }
    }
    write!(out, "\x1b[J\x1b[?2026l")?;
    out.flush()
}

/// Raw mode: every key arrives at once and `q` is a key like any other. The
/// old mode is returned so that it can be restored.
///
/// The output processing stays on: `cfmakeraw` clears `OPOST`, and a frame
/// whose newline does not return to the left margin drifts right and wraps
/// (the review of T-3). `OPOST` with `ONLCR` gives the board back the
/// carriage return that belongs to a line feed.
fn raw_mode(fd: RawFd) -> Option<libc::termios> {
    let mut term: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: `term` is a termios to fill in; `fd` is the terminal.
    if unsafe { libc::tcgetattr(fd, &mut term) } != 0 {
        return None;
    }
    let saved = term;
    // SAFETY: `term` is a termios we own.
    unsafe { libc::cfmakeraw(&mut term) };
    term.c_oflag |= libc::OPOST | libc::ONLCR;
    // No blocking read: the poll timeout is the only wait.
    term.c_cc[libc::VMIN] = 0;
    term.c_cc[libc::VTIME] = 0;
    // SAFETY: `term` is a termios we own.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &term) } != 0 {
        return None;
    }
    Some(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-403: a frame is written over the last one, in place, and the screen
    /// is never cleared whole — that clear is what made the page blink.
    #[test]
    fn draw_overwrites_the_screen_in_place() {
        let mut out: Vec<u8> = Vec::new();
        draw(&mut out, "GEAR · p\nLANES\n  T-1 done").unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("\x1b[?2026h\x1b[H"), "{text:?}");
        assert!(!text.contains("\x1b[2J"), "the whole screen is never cleared: {text:?}");
        assert!(
            text.ends_with("\x1b[J\x1b[?2026l"),
            "the erase below and the end of the synchronized update close the frame: {text:?}"
        );
        // Every line erases its rest, the last one included; only the lines
        // between newlines take a newline (F2: the frame has no trailing one).
        let body = text
            .strip_prefix("\x1b[?2026h\x1b[H")
            .and_then(|rest| rest.strip_suffix("\x1b[J\x1b[?2026l"))
            .expect("the frame between the update's two ends");
        let lines: Vec<&str> = body.split('\n').collect();
        assert_eq!(lines.len(), 3, "{body:?}");
        for line in &lines {
            assert!(line.ends_with("\x1b[K"), "every line erases its rest: {line:?}");
        }
        assert_eq!(lines[2], "  T-1 done\x1b[K", "{body:?}");
    }

    /// An empty frame is an update of its own: `\x1b[J` erases what is left.
    #[test]
    fn an_empty_frame_is_a_whole_update() {
        let mut out: Vec<u8> = Vec::new();
        draw(&mut out, "").unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\x1b[?2026h\x1b[H\x1b[K\x1b[J\x1b[?2026l"
        );
    }
}
