//! T-384: `PRAGMA data_version`, the one cheap query the board watches so it
//! knows when the store it shows was written.
//!
//! The board only reads, so every write it must notice comes from another
//! connection: the value moves when that connection commits, and it stays put
//! for a read and for this connection's own writes.

use gear_store::Store;

#[test]
fn data_version_moves_when_another_connection_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    let mut board = Store::open(&path).unwrap();
    let mut writer = Store::open(&path).unwrap();

    let before = board.read(|tx| tx.data_version()).unwrap();
    // Reading again changes nothing: the value is the board's to compare.
    assert_eq!(board.read(|tx| tx.data_version()).unwrap(), before);

    writer.write(|tx| tx.set_meta("k", "v1")).unwrap();
    let after = board.read(|tx| tx.data_version()).unwrap();
    assert_ne!(
        after, before,
        "the other connection's commit moved the value"
    );

    // Another commit moves it again.
    writer.write(|tx| tx.set_meta("k", "v2")).unwrap();
    let later = board.read(|tx| tx.data_version()).unwrap();
    assert_ne!(
        later, after,
        "every commit of the other connection moves it"
    );

    // This connection's own write does not move the value it sees (SQLite:
    // `data_version` reports the changes of other connections only).
    board.write(|tx| tx.set_meta("own", "write")).unwrap();
    assert_eq!(board.read(|tx| tx.data_version()).unwrap(), later);
}
