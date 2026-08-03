pub mod migrate;

use rusqlite::Connection;
use std::path::Path;

pub struct Storage {
    pub(crate) db: Connection,
}

impl Storage {
    pub fn open(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        let db = Connection::open(dir.join("desktop-pet.db")).map_err(|error| error.to_string())?;
        db.pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| error.to_string())?;
        db.pragma_update(None, "foreign_keys", "ON")
            .map_err(|error| error.to_string())?;
        let storage = Self { db };
        migrate::apply(&storage.db)?;
        Ok(storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    #[test]
    fn opens_and_migrates_fresh_database() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-storage-{}-{n}", std::process::id()));
        let dir = root.join("db");
        let storage = Storage::open(&dir).unwrap();
        let version: i64 = storage
            .db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, migrate::MIGRATIONS.len() as i64);
        let _ = std::fs::remove_dir_all(root);
    }
}
