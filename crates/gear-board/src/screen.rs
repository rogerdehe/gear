//! The terminal the board draws on: ANSI, raw mode, one key at a time.
//!
//! The board owns the screen while it is open: it clears it, redraws the
//! frame and gives the terminal back when the user quits.

use std::io::Write;
use std::os::fd::{AsRawFd, RawFd};
use std::time::Duration;

/// Where the board's key presses come from. The real one is the terminal; a
/// test feeds its own.
pub trait Keys {
    /// The next key press, waiting up to `timeout`; `None` on timeout.
    fn next_key(&mut self, timeout: Duration) -> Option<char>;
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
    fn next_key(&mut self, timeout: Duration) -> Option<char> {
        if self.saved.is_none() {
            return None;
        }
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
        // One byte is one key for everything the board listens for.
        Some(byte[0] as char)
    }
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
/// terminal's scrollback, and the cursor stays out of the way.
pub fn enter(out: &mut dyn Write) -> std::io::Result<()> {
    write!(out, "\x1b[?1049h\x1b[?25l")?;
    out.flush()
}

/// Give the terminal back.
pub fn leave(out: &mut dyn Write) -> std::io::Result<()> {
    write!(out, "\x1b[?25h\x1b[?1049l")?;
    out.flush()
}

/// Clear the screen and draw `text` at the top.
pub fn draw(out: &mut dyn Write, text: &str) -> std::io::Result<()> {
    write!(out, "\x1b[H\x1b[2J{text}\n")?;
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
