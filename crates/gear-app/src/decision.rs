//! The captain's decision log (design §7): the lines of `# Decisions`, the
//! kinds, the star, and the key that makes an apply idempotent.
//!
//! §7.1: a decision is one line
//!
//! ```text
//! - <YYYY-MM-DD HH:MM> <kind>: <what> | why: <reason> | else: <the alternative not taken> | changes: <scope/time/quality/risk>
//! ```
//!
//! `why:` is required; `else:` and `changes:` are optional. `- tail: …` and
//! `- station M-<n>: …` keep their own forms (kinds `tail` and `station`), and
//! the user's corrections at landing are `- <date> user: <text>` (kind
//! `user`).
//!
//! §7.2: a line that does not match that form (and is not tail/station) is
//! kept as kind `note` without a star, and the apply says so.
//!
//! §7.3: every apply re-reads every line; the key is the sha256 of the line
//! with its whitespace collapsed, so a line that is already in the log is
//! skipped and an edited line is a new decision.

/// The kinds a decision may have (§7.2).
pub const KINDS: [&str; 16] = [
    "scope",
    "cut",
    "split",
    "override",
    "logged",
    "rework",
    "estimate",
    "staff",
    "model",
    "config",
    "hand-fix",
    "note",
    "tail",
    "station",
    "user",
    "confirmed",
];

/// One decision of the log, as the captain wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The kind (§7.2).
    pub kind: String,
    /// What the decision is.
    pub what: String,
    /// Why it was taken (required for a structured line, §7.1).
    pub why: Option<String>,
    /// The alternative not taken.
    pub alt: Option<String>,
    /// What it changed: scope, time, quality, risk.
    pub changes: Option<String>,
    /// True when the decision is worth a look at landing (§7.2).
    pub star: bool,
    /// The date written on the line (`YYYY-MM-DD HH:MM`), empty when the line
    /// carries none.
    pub at: String,
    /// True when the line is not structured (§7.2): it is kept as a note.
    pub note: bool,
}

/// The decision a line carries: the structured form of §7.1, the kinds that
/// keep their own form (`tail`, `station`, `user`), or a note (§7.2).
pub fn parse(line: &str) -> Decision {
    let mut text = line.trim();
    // The star is the captain's, written right after the dash (§7.2).
    let mut star = false;
    if let Some(rest) = text.strip_prefix('★') {
        star = true;
        text = rest.trim_start();
    }
    // The kinds that keep their own form (§7.1).
    if let Some(rest) = text.strip_prefix("tail:") {
        return Decision {
            kind: "tail".to_string(),
            what: rest.trim().to_string(),
            why: None,
            alt: None,
            changes: None,
            star: false,
            at: String::new(),
            note: false,
        };
    }
    if text.starts_with("station ") {
        let what = text
            .split_once(':')
            .map(|(_, what)| what.trim().to_string())
            .unwrap_or_else(|| text.to_string());
        return Decision {
            kind: "station".to_string(),
            what,
            why: None,
            alt: None,
            changes: None,
            star: false,
            at: String::new(),
            note: false,
        };
    }
    // The structured form: <date> <kind>: <what> | why: … | else: … | changes: …
    let mut parts = text.split(" | ");
    let first = parts.next().unwrap_or_default().trim();
    let Some((head, what)) = first.split_once(": ") else {
        return note(line, star);
    };
    let (head, what) = (head.trim(), what.trim().to_string());
    let mut why = None;
    let mut alt = None;
    let mut changes = None;
    for part in parts {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("why:") {
            why = Some(value.trim().to_string());
        } else if let Some(value) = part.strip_prefix("else:") {
            alt = Some(value.trim().to_string());
        } else if let Some(value) = part.strip_prefix("changes:") {
            changes = Some(value.trim().to_string());
        } else {
            // A part that is not one of the three: the line is not structured.
            return note(line, star);
        }
    }
    // The head: the date and the kind.
    let (at, kind) = match head.rsplit_once(' ') {
        Some((date, kind)) => (date.trim().to_string(), kind.trim().to_string()),
        None => (String::new(), String::new()),
    };
    // The user's own correction at the landing (§7.1): `- <date> user: <text>`
    // — a date and a text, no why and no warning.
    if kind == "user" && is_date(&at) {
        return Decision {
            kind: "user".to_string(),
            what,
            why: None,
            alt,
            changes,
            star,
            at,
            note: false,
        };
    }
    if !KINDS.contains(&kind.as_str()) || why.is_none() || !is_date_time(&at) {
        // §7.1: `why:` is required, the kind must be one of §7.2's and the
        // date must be the shape §7.1 writes (`YYYY-MM-DD HH:MM`).
        return note(line, star);
    }
    Decision {
        kind,
        what,
        why,
        alt,
        changes,
        star,
        at,
        note: false,
    }
}

/// A line that is not structured (§7.2): a note, without a star.
fn note(line: &str, _star: bool) -> Decision {
    Decision {
        kind: "note".to_string(),
        what: line.trim().to_string(),
        why: None,
        alt: None,
        changes: None,
        star: false,
        at: String::new(),
        note: true,
    }
}

/// The key of a line (§7.3): the sha256 of the line with its whitespace
/// collapsed. The same line always has the same key, and an edited line a
/// different one.
pub fn key(line: &str) -> String {
    sha256_hex(collapsed(line).as_bytes())
}

/// The key of an entry GEAR records itself (§7.4): the sha256 of its own
/// normalized text, so the same event is recorded once.
pub fn key_of(text: &str) -> String {
    sha256_hex(collapsed(text).as_bytes())
}

/// A line with its whitespace collapsed: the runs of whitespace become one
/// space, and the ends are trimmed.
pub fn collapsed(line: &str) -> String {
    line.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// A date as §7.1 writes it: `YYYY-MM-DD`.
fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

/// A time as §7.1 writes it: `HH:MM`.
fn is_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 5
        && bytes[2] == b':'
        && bytes[..2].iter().all(u8::is_ascii_digit)
        && bytes[3..5].iter().all(u8::is_ascii_digit)
        && (0..24).contains(&((bytes[0] - b'0') * 10 + (bytes[1] - b'0')))
        && (0..60).contains(&((bytes[3] - b'0') * 10 + (bytes[4] - b'0')))
}

/// A date and a time as §7.1 writes it: `YYYY-MM-DD HH:MM`.
fn is_date_time(text: &str) -> bool {
    match text.split_once(' ') {
        Some((date, time)) => is_date(date) && is_time(time),
        None => false,
    }
}

/// Whether the text names this task (a whole identifier: `T-1` does not name
/// `T-10`).
pub fn names_task(text: &str, task: &str) -> bool {
    let bytes = text.as_bytes();
    let needle = task.as_bytes();
    if needle.is_empty() || needle.len() > bytes.len() {
        return false;
    }
    let mut at = 0;
    while at + needle.len() <= bytes.len() {
        if &bytes[at..at + needle.len()] == needle {
            let before = at == 0 || !is_id_byte(bytes[at - 1]);
            let after = at + needle.len();
            let after = after >= bytes.len() || !is_id_byte(bytes[after]);
            if before && after {
                return true;
            }
        }
        at += 1;
    }
    false
}

/// A byte that can be part of a task identifier.
fn is_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

/// The moment an apply happened, as the log writes it (`YYYY-MM-DD HH:MM`).
pub fn apply_time(now: i64) -> String {
    let days = now.div_euclid(86_400);
    let seconds = now.rem_euclid(86_400);
    format!(
        "{} {:02}:{:02}",
        crate::plan::utc_date(days * 86_400),
        seconds / 3600,
        (seconds % 3600) / 60
    )
}

/// The sha256 of a text, in hex.
///
/// No crypto library in the tree: the algorithm is small, standard and
/// checked against its published test vectors.
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// The sha256 digest of a message (FIPS 180-4).
fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    // The padding: a 1 bit, zeros, and the length in bits.
    let mut message = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());
    for chunk in message.chunks(64) {
        let mut w = [0u32; 64];
        for (index, word) in chunk.chunks(4).enumerate() {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut digest = [0u8; 32];
    for (index, word) in h.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published test vectors of sha256.
    #[test]
    fn sha256_matches_its_test_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"The quick brown fox jumps over the lazy dog"),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
    }
}
