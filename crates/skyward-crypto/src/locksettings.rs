//! What the phone's lock settings keep: a value under a name, for each
//! user. Among them is which of the protectors the phone keeps for a user
//! is the one that user's own credential opens.

use anyhow::{Context, Result};
use std::path::Path;

/// Where the phone keeps them.
pub const DATABASE: &str = "/data/system/locksettings.db";

/// The name the protector of a user's own secret is recorded under. Its
/// value is the number that protector's files are named after.
const CURRENT_PROTECTOR: &str = "sp-handle";

/// The number that names no protector. The files the phone keeps for a
/// user's secret itself, rather than for any one protector, are named
/// after it.
const NO_PROTECTOR: i64 = 0;

/// Which protector `user`'s own credential opens, named as its files are.
/// A phone that recorded none for this user gives nothing.
pub fn current_protector(database: &Path, user: u32) -> Result<Option<String>> {
    let Some(recorded) = value(database, CURRENT_PROTECTOR, user)? else { return Ok(None) };
    let id: i64 = recorded
        .parse()
        .with_context(|| format!("{} records user {user}'s protector as {recorded:?}", database.display()))?;
    Ok((id != NO_PROTECTOR).then(|| crate::user::protector_name(id)))
}

/// What the lock settings keep for `user` under `name`, where they keep
/// anything. Read committed WAL pages too: after a screen-lock change,
/// the current protector can exist only in the WAL until a checkpoint.
/// A read-only connection keeps the phone's database and WAL unchanged;
/// immutable=1 would silently select an older protector.
fn value(database: &Path, name: &str, user: u32) -> Result<Option<String>> {
    let db = rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {}", database.display()))?;
    let mut query = db
        .prepare("SELECT value FROM locksettings WHERE name = ?1 AND user = ?2")
        .context("ask the lock settings what they keep")?;
    let mut rows = query
        .query((name, user))
        .with_context(|| format!("ask the lock settings what {name} is for user {user}"))?;
    let Some(row) = rows.next().with_context(|| format!("read what {} says about {name}", database.display()))? else {
        return Ok(None);
    };
    row.get(0).with_context(|| format!("read what {name} is for user {user}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lock settings as the phone writes them: the table it keeps, holding
    /// the rows given.
    fn scratch(test: &str, rows: &[(&str, u32, Option<&str>)]) -> std::path::PathBuf {
        let at = std::env::temp_dir().join(format!("skyward-locksettings-{test}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&at);
        let db = rusqlite::Connection::open(&at).unwrap();
        db.execute("CREATE TABLE locksettings (_id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, user INTEGER, value TEXT)", ()).unwrap();
        for (name, user, value) in rows {
            db.execute("INSERT INTO locksettings (name, user, value) VALUES (?1, ?2, ?3)", (name, user, value)).unwrap();
        }
        at
    }

    #[test]
    fn committed_wal_protector_replaces_the_old_handle_without_rewriting_the_store() {
        let at = scratch("wal-current", &[("sp-handle", 0, Some("1")), ("sp-handle", 10, Some("2"))]);
        let writer = rusqlite::Connection::open(&at).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;").unwrap();
        writer.execute("UPDATE locksettings SET value = '-1' WHERE name = 'sp-handle' AND user = 0", ()).unwrap();
        let wal = at.with_file_name(format!("{}-wal", at.file_name().unwrap().to_str().unwrap()));
        let before = (std::fs::read(&at).unwrap(), std::fs::read(&wal).unwrap());
        let immutable = rusqlite::Connection::open_with_flags(
            format!("file:{}?immutable=1", at.display()),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        ).unwrap();
        assert_eq!(immutable.query_row("SELECT value FROM locksettings WHERE user = 0", (), |row| row.get::<_, String>(0)).unwrap(), "1");
        assert_eq!(current_protector(&at, 0).unwrap().as_deref(), Some("ffffffffffffffff"));
        assert_eq!(current_protector(&at, 10).unwrap().as_deref(), Some("0000000000000002"));
        writer.execute_batch("BEGIN; UPDATE locksettings SET value = '-2' WHERE user = 0;").unwrap();
        assert_eq!(current_protector(&at, 0).unwrap().as_deref(), Some("ffffffffffffffff"), "uncommitted changes must not select a protector");
        writer.execute_batch("ROLLBACK;").unwrap();
        assert_eq!(std::fs::read(&at).unwrap(), before.0);
        assert_eq!(std::fs::read(&wal).unwrap(), before.1);
        drop(immutable);
        drop(writer);
        std::fs::remove_file(&at).unwrap();
    }

    #[test]
    fn locksettings_schema_can_live_only_in_the_committed_wal() {
        let at = scratch("wal-schema", &[]);
        std::fs::remove_file(&at).unwrap();
        let writer = rusqlite::Connection::open(&at).unwrap();
        writer.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
             CREATE TABLE locksettings (name TEXT, user INTEGER, value TEXT);
             INSERT INTO locksettings VALUES ('sp-handle', 0, '-1');"
        ).unwrap();
        let wal = at.with_file_name(format!("{}-wal", at.file_name().unwrap().to_str().unwrap()));
        let before = (std::fs::read(&at).unwrap(), std::fs::read(&wal).unwrap());
        assert_eq!(current_protector(&at, 0).unwrap().as_deref(), Some("ffffffffffffffff"));
        assert_eq!(current_protector(&at, 10).unwrap(), None);
        assert_eq!(std::fs::read(&at).unwrap(), before.0);
        assert_eq!(std::fs::read(&wal).unwrap(), before.1);
        drop(writer);
        std::fs::remove_file(&at).unwrap();
    }

    #[test]
    fn the_protector_recorded_for_a_user_is_the_one_named() {
        let at = scratch("recorded", &[
            ("sp-handle", 0, Some("623346955855960006")),
            ("sp-handle", 10, Some("-1")),
            ("lockscreen.password_type", 0, Some("65536")),
        ]);
        assert_eq!(current_protector(&at, 0).unwrap().as_deref(), Some("08a692c0f39d97c6"));
        assert_eq!(current_protector(&at, 10).unwrap().as_deref(), Some("ffffffffffffffff"));
        assert_eq!(current_protector(&at, 11).unwrap(), None, "a user the phone recorded nothing for");
        std::fs::remove_file(&at).unwrap();
    }

    #[test]
    fn the_number_of_no_protector_and_a_row_holding_nothing_give_none() {
        let at = scratch("none", &[("sp-handle", 0, Some("0")), ("sp-handle", 10, None)]);
        assert_eq!(current_protector(&at, 0).unwrap(), None);
        assert_eq!(current_protector(&at, 10).unwrap(), None, "a row holding nothing");
        std::fs::remove_file(&at).unwrap();
    }

    #[test]
    fn a_value_that_is_no_number_and_a_database_that_is_not_there_are_errors() {
        let at = scratch("odd", &[("sp-handle", 0, Some("behind the sofa"))]);
        assert!(current_protector(&at, 0).is_err());
        assert!(current_protector(Path::new("/nowhere/locksettings.db"), 0).is_err());
        std::fs::remove_file(&at).unwrap();
    }
}
