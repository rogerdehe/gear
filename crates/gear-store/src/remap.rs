//! Rewrite commit ids after the repository's history was rewritten (user,
//! 2026-11-04: the SelyraAI history drops its archived binaries). Every text
//! value of every table that names an old commit, in full or by a unique
//! prefix of at least 7 hex digits, names the new commit instead, at the same
//! length. The old-to-new map is `git filter-repo`'s `commit-map`.
use crate::{map_sqlite, Tx};
use gear_model::GearResult;
use std::collections::BTreeMap;

/// The old-to-new commit map: full lowercase 40-hex ids.
#[derive(Debug, Default, Clone)]
pub struct CommitMap {
    map: BTreeMap<String, String>,
}

impl CommitMap {
    /// Read `git filter-repo`'s commit-map text: a header line, then
    /// `<old> <new>` per line. Lines that are not two 40-hex ids are skipped,
    /// and so is a commit that was dropped (its new id is all zeros).
    pub fn parse(text: &str) -> CommitMap {
        let mut map = BTreeMap::new();
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let (Some(old), Some(new)) = (parts.next(), parts.next()) else {
                continue;
            };
            let full = |id: &str| id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit());
            if !full(old) || !full(new) || new.bytes().all(|b| b == b'0') || old == new {
                continue;
            }
            map.insert(old.to_ascii_lowercase(), new.to_ascii_lowercase());
        }
        CommitMap { map }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The new id for `id` (7 to 40 hex digits), at the same length, when
    /// exactly one old commit starts with it.
    fn lookup(&self, id: &str) -> Option<String> {
        let id = id.to_ascii_lowercase();
        let mut found = self
            .map
            .range(id.clone()..)
            .take_while(|(old, _)| old.starts_with(&id));
        let (_, new) = found.next()?;
        if found.next().is_some() {
            return None; // ambiguous prefix
        }
        Some(new[..id.len()].to_string())
    }

    /// `text` with every commit id it names replaced. A run of hex digits is
    /// a candidate only when it is 7 to 40 long and stands alone (no hex digit
    /// or letter on either side).
    pub fn rewrite(&self, text: &str) -> Option<String> {
        let bytes = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut changed = false;
        let mut i = 0;
        let mut copied = 0;
        while i < bytes.len() {
            if !bytes[i].is_ascii_hexdigit() || (i > 0 && bytes[i - 1].is_ascii_alphanumeric()) {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            let len = i - start;
            if !(7..=40).contains(&len) || (i < bytes.len() && bytes[i].is_ascii_alphanumeric()) {
                continue;
            }
            if let Some(new) = self.lookup(&text[start..i]) {
                out.push_str(&text[copied..start]);
                out.push_str(&new);
                copied = i;
                changed = true;
            }
        }
        if !changed {
            return None;
        }
        out.push_str(&text[copied..]);
        Some(out)
    }
}

impl Tx<'_> {
    /// Rewrite every commit id in every text value of the store (see the
    /// module). Returns how many values changed, per table.
    pub fn remap_commits(&self, map: &CommitMap) -> GearResult<Vec<(String, usize)>> {
        let tables: Vec<String> = {
            let mut stmt = self
                .tx
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(map_sqlite)?;
            rows.collect::<Result<_, _>>().map_err(map_sqlite)?
        };
        let mut counts = Vec::new();
        for table in tables {
            let columns: Vec<String> = {
                let mut stmt = self
                    .tx
                    .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(map_sqlite)?;
                rows.collect::<Result<_, _>>().map_err(map_sqlite)?
            };
            let mut changed = 0;
            for column in columns {
                let updates: Vec<(i64, String)> = {
                    let mut stmt = self
                        .tx
                        .prepare(&format!(
                            "SELECT rowid, \"{column}\" FROM \"{table}\" WHERE typeof(\"{column}\")='text'"
                        ))
                        .map_err(map_sqlite)?;
                    let rows = stmt
                        .query_map([], |row| {
                            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(map_sqlite)?;
                    let mut updates = Vec::new();
                    for row in rows {
                        let (rowid, value) = row.map_err(map_sqlite)?;
                        if let Some(new) = map.rewrite(&value) {
                            updates.push((rowid, new));
                        }
                    }
                    updates
                };
                for (rowid, value) in &updates {
                    self.tx
                        .execute(
                            &format!("UPDATE \"{table}\" SET \"{column}\"=?1 WHERE rowid=?2"),
                            rusqlite::params![value, rowid],
                        )
                        .map_err(map_sqlite)?;
                }
                changed += updates.len();
            }
            if changed > 0 {
                counts.push((table, changed));
            }
        }
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "aaaaaaa1111111111111111111111111111111aa";
    const NEW: &str = "bbbbbbb2222222222222222222222222222222bb";
    const OTHER: &str = "aaaaaaa9999999999999999999999999999999aa";

    fn map() -> CommitMap {
        CommitMap::parse(&format!(
            "old new\n{OLD} {NEW}\n{OTHER} 0000000000000000000000000000000000000000\n"
        ))
    }

    #[test]
    fn full_ids_and_unique_prefixes_are_rewritten_at_their_length() {
        let map = map();
        assert_eq!(map.len(), 1, "a dropped commit is not mapped");
        assert_eq!(
            map.rewrite(&format!("candidate {OLD}.")).unwrap(),
            format!("candidate {NEW}.")
        );
        assert_eq!(
            map.rewrite("at aaaaaaa1111 ok").unwrap(),
            "at bbbbbbb2222 ok"
        );
        assert_eq!(
            map.rewrite(&format!("{{\"candidate\":\"{OLD}\"}}"))
                .unwrap(),
            format!("{{\"candidate\":\"{NEW}\"}}")
        );
    }

    #[test]
    fn short_runs_words_and_unknown_ids_stay() {
        let map = map();
        assert_eq!(map.rewrite("aaaaaa1 is six-ish"), None, "unknown id");
        assert_eq!(map.rewrite("deadbeef"), None);
        assert_eq!(map.rewrite("xaaaaaaa1111"), None, "inside a word");
        assert_eq!(map.rewrite("T-1 at 2026"), None);
    }

    #[test]
    fn an_ambiguous_prefix_stays() {
        let map = CommitMap::parse(&format!(
            "{OLD} {NEW}\n{OTHER} cccccccccccccccccccccccccccccccccccccccc\n"
        ));
        assert_eq!(
            map.rewrite("aaaaaaa"),
            None,
            "two old commits start with it"
        );
        assert_eq!(map.rewrite("aaaaaaa9999").unwrap(), "ccccccccccc");
    }
}
