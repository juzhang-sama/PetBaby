use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[
    // v1: pets, variants and state tables
    r#"
    CREATE TABLE pets (
      pet_id TEXT PRIMARY KEY,
      schema_version INTEGER NOT NULL,
      species TEXT NOT NULL,
      identity_mode TEXT NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE variants (
      variant_id TEXT PRIMARY KEY,
      pet_id TEXT NOT NULL REFERENCES pets(pet_id) ON DELETE CASCADE,
      style_id TEXT NOT NULL,
      manifest_path TEXT NOT NULL,
      created_at TEXT NOT NULL
    );
    CREATE TABLE state (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL
    );
    "#,
];

pub fn apply(db: &Connection) -> Result<(), String> {
    let current: i64 = db
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if current as usize >= MIGRATIONS.len() {
        return Ok(());
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        db.execute_batch(migration)
            .map_err(|error| format!("migration {index} failed: {error}"))?;
        db.pragma_update(None, "user_version", (index + 1) as i64)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_db() -> (Connection, std::path::PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-migrate-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        (Connection::open(root.join("m.db")).unwrap(), root)
    }

    #[test]
    fn migrates_empty_database_to_latest() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        let tables: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('pets','variants','state')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 3);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reapply_is_idempotent() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        apply(&db).unwrap();
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        let _ = std::fs::remove_dir_all(root);
    }
}
