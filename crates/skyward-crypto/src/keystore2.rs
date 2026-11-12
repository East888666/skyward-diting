//! Reading what the phone's key service was asked to keep. The service
//! holds each key under a name, and what it holds is the handle the secure
//! world knows the key by.

use anyhow::{Context, Result};
use std::path::Path;

/// Where the key service keeps what it was asked to keep.
pub const DATABASE: &str = "/data/misc/keystore/persistent.sqlite";

/// The kind of thing a row holds: the key itself rather than a certificate.
const KEY_BLOB: i64 = 0;

/// The handle the secure world knows the key named `alias` by. The
/// database is opened read-only, including committed pages in its WAL.
/// Android can shut down before those pages reach the main database;
/// immutable=1 would ignore them, even when they hold the schema itself.
pub fn key_blob(database: &Path, alias: &str) -> Result<Vec<u8>> {
    let db = rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {}", database.display()))?;
    let mut query = db
        .prepare(
            "SELECT b.blob FROM blobentry b JOIN keyentry k ON b.keyentryid = k.id
             WHERE k.alias = ?1 AND b.subcomponent_type = ?2 ORDER BY b.id DESC LIMIT 1",
        )
        .context("ask the key service's store for a key")?;
    let mut rows = query.query((alias, KEY_BLOB)).context("ask the key service's store for a key")?;
    let row = rows
        .next()
        .with_context(|| format!("read what {} says about {alias}", database.display()))?
        .with_context(|| format!("{} holds no key named {alias}", database.display()))?;
    row.get(0).with_context(|| format!("read the key named {alias}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Store(PathBuf);

    impl Store {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "skyward-keystore2-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn database(&self) -> PathBuf {
            self.0.join("persistent.sqlite")
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn populate(db: &Connection) {
        db.execute_batch(
            "CREATE TABLE keyentry (id INTEGER PRIMARY KEY, alias TEXT);
             CREATE TABLE blobentry (id INTEGER PRIMARY KEY, keyentryid INTEGER, subcomponent_type INTEGER, blob BLOB);
             INSERT INTO keyentry VALUES (1, 'profile_key_name_decrypt_0');
             INSERT INTO blobentry VALUES (1, 1, 0, X'0102');
             INSERT INTO blobentry VALUES (2, 1, 0, X'0304');
             INSERT INTO blobentry VALUES (3, 1, 1, X'0506');"
        ).unwrap();
    }

    #[test]
    fn reads_committed_wal_schema_and_latest_key_without_changing_database_or_wal() {
        let store = Store::new();
        let path = store.database();
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;").unwrap();
        populate(&writer);
        // Keep the writer open so closing it cannot checkpoint the WAL.
        let main_before = fs::read(&path).unwrap();
        let wal_path = store.0.join("persistent.sqlite-wal");
        let wal_before = fs::read(&wal_path).unwrap();
        let immutable = Connection::open_with_flags(
            format!("file:{}?immutable=1", path.display()),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        ).unwrap();
        assert!(immutable.prepare("SELECT blob FROM blobentry").is_err());
        assert_eq!(key_blob(&path, "profile_key_name_decrypt_0").unwrap(), vec![3, 4]);
        assert_eq!(fs::read(&path).unwrap(), main_before);
        assert_eq!(fs::read(&wal_path).unwrap(), wal_before);
    }

    #[test]
    fn reads_checkpointed_stores_and_reports_a_missing_alias() {
        let store = Store::new();
        let path = store.database();
        {
            let writer = Connection::open(&path).unwrap();
            populate(&writer);
        }
        assert_eq!(key_blob(&path, "profile_key_name_decrypt_0").unwrap(), vec![3, 4]);
        let error = format!("{:#}", key_blob(&path, "missing'alias").unwrap_err());
        assert!(error.contains("holds no key named missing'alias"), "{error}");
    }
}
