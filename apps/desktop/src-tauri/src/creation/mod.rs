pub mod profiles;

use crate::storage::Storage;
use std::sync::{Arc, Mutex};

#[expect(dead_code)] // consumed by the job manager in M4 Task 4
pub type SharedCreationStore = Arc<Mutex<CreationStore>>;

#[expect(dead_code)] // consumed by the job manager in M4 Task 4
pub struct CreationStore {
    storage: Arc<Mutex<Storage>>,
}

impl CreationStore {
    pub fn new(storage: Arc<Mutex<Storage>>) -> Self {
        Self { storage }
    }

    pub fn create_job(
        &self,
        job_id: &str,
        pet_id: &str,
        prompt: &str,
        ref_sha256: &str,
        task_id: Option<&str>,
    ) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.execute(
            "INSERT INTO generation_jobs
             (job_id, pet_id, prompt, ref_sha256, task_id, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
            rusqlite::params![
                job_id,
                pet_id,
                prompt,
                ref_sha256,
                task_id,
                crate::creation::profiles::now_iso()
            ],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn update_job_status(
        &self,
        job_id: &str,
        status: &str,
        result_url: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.execute(
            "UPDATE generation_jobs SET status = ?2, result_url = ?3, error = ?4
             WHERE job_id = ?1",
            rusqlite::params![job_id, status, result_url, error],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn running_jobs(&self) -> Result<Vec<JobRecord>, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let mut statement = db
            .prepare(
                "SELECT job_id, pet_id, prompt, ref_sha256, task_id, status, result_url, error, created_at
                 FROM generation_jobs WHERE status IN ('pending','running')",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok(JobRecord {
                    job_id: row.get(0)?,
                    pet_id: row.get(1)?,
                    prompt: row.get(2)?,
                    ref_sha256: row.get(3)?,
                    task_id: row.get(4)?,
                    status: row.get(5)?,
                    result_url: row.get(6)?,
                    error: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })
            .map_err(|error| error.to_string())?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row.map_err(|error| error.to_string())?);
        }
        Ok(jobs)
    }

    pub fn job_list(&self, pet_id: &str) -> Result<Vec<JobRecord>, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let mut statement = db
            .prepare(
                "SELECT job_id, pet_id, prompt, ref_sha256, task_id, status, result_url, error, created_at
                 FROM generation_jobs WHERE pet_id = ?1 ORDER BY created_at",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(rusqlite::params![pet_id], |row| {
                Ok(JobRecord {
                    job_id: row.get(0)?,
                    pet_id: row.get(1)?,
                    prompt: row.get(2)?,
                    ref_sha256: row.get(3)?,
                    task_id: row.get(4)?,
                    status: row.get(5)?,
                    result_url: row.get(6)?,
                    error: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })
            .map_err(|error| error.to_string())?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row.map_err(|error| error.to_string())?);
        }
        Ok(jobs)
    }
}

#[expect(dead_code)] // consumed by the job manager in M4 Task 4
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub job_id: String,
    pub pet_id: String,
    pub prompt: String,
    pub ref_sha256: String,
    pub task_id: Option<String>,
    pub status: String,
    pub result_url: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::profiles::now_iso;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_store() -> (CreationStore, std::path::PathBuf, String) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-creation-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let repo = crate::pets::repository::PetRepository::new(storage.clone());
        let pet = repo
            .create(
                crate::pets::pet::Species::Cat,
                crate::pets::pet::IdentityMode::RealPet,
            )
            .unwrap();
        (CreationStore::new(storage), root, pet.pet_id)
    }

    #[test]
    fn job_round_trip_and_status_transitions() {
        let (store, root, pet_id) = temp_store();
        store
            .create_job("j1", &pet_id, "prompt", "abc123", Some("t1"))
            .unwrap();
        let running = store.running_jobs().unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].job_id, "j1");
        assert_eq!(running[0].status, "pending");

        store
            .update_job_status("j1", "running", None, None)
            .unwrap();
        assert_eq!(store.running_jobs().unwrap().len(), 1);

        store
            .update_job_status("j1", "success", Some("https://x/out.png"), None)
            .unwrap();
        assert!(store.running_jobs().unwrap().is_empty());

        let listed = store.job_list(&pet_id).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].status, "success");
        assert_eq!(listed[0].result_url.as_deref(), Some("https://x/out.png"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn jobs_are_scoped_by_pet() {
        let (store, root, pet_id) = temp_store();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let repo = crate::pets::repository::PetRepository::new(storage);
        let pet2 = repo
            .create(
                crate::pets::pet::Species::Dog,
                crate::pets::pet::IdentityMode::Adopted,
            )
            .unwrap();
        store.create_job("j1", &pet_id, "p", "h", None).unwrap();
        store
            .create_job("j2", &pet2.pet_id, "p", "h", None)
            .unwrap();
        assert_eq!(store.job_list(&pet_id).unwrap().len(), 1);
        assert_eq!(store.job_list(&pet2.pet_id).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn running_jobs_survive_for_resume() {
        let (store, root, pet_id) = temp_store();
        let now = now_iso();
        assert!(!now.is_empty());
        store
            .create_job("j1", &pet_id, "p", "h", Some("t1"))
            .unwrap();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let store2 = CreationStore::new(storage);
        let running = store2.running_jobs().unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].task_id.as_deref(), Some("t1"));
        let _ = std::fs::remove_dir_all(root);
    }
}
