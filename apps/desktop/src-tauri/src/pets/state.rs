use crate::storage::Storage;
use std::sync::{Arc, Mutex};

pub type SharedStateStore = Arc<Mutex<StateStore>>;

pub struct StateStore {
    storage: Arc<Mutex<Storage>>,
}

impl StateStore {
    pub fn new(storage: Arc<Mutex<Storage>>) -> Self {
        Self { storage }
    }

    pub fn load(&self, key: &str) -> Result<Option<String>, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let mut statement = db
            .prepare("SELECT value FROM state WHERE key = ?1")
            .map_err(|error| error.to_string())?;
        let mut rows = statement
            .query_map(rusqlite::params![key], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        match rows.next() {
            Some(row) => row.map(Some).map_err(|error| error.to_string()),
            None => Ok(None),
        }
    }

    pub fn save(&self, key: &str, value: &str) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn remove(&self, key: &str) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.execute("DELETE FROM state WHERE key = ?1", rusqlite::params![key])
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_store() -> (StateStore, std::path::PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-state-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        (StateStore::new(storage), root)
    }

    #[test]
    fn round_trips_state_value() {
        let (store, root) = temp_store();
        assert_eq!(store.load("pet:pet-1:behavior").unwrap(), None);
        store
            .save("pet:pet-1:behavior", r#"{"energy":0.5}"#)
            .unwrap();
        assert_eq!(
            store.load("pet:pet-1:behavior").unwrap().as_deref(),
            Some(r#"{"energy":0.5}"#)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn save_overwrites_existing_value() {
        let (store, root) = temp_store();
        store.save("k", "v1").unwrap();
        store.save("k", "v2").unwrap();
        assert_eq!(store.load("k").unwrap().as_deref(), Some("v2"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn remove_deletes_existing_value() {
        let (store, root) = temp_store();
        store
            .save("creation:pet-1:compile_error", "failed")
            .unwrap();
        store.remove("creation:pet-1:compile_error").unwrap();
        assert_eq!(store.load("creation:pet-1:compile_error").unwrap(), None);
        let _ = std::fs::remove_dir_all(root);
    }
}
