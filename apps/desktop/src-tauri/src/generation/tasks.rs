use crate::creation::profiles::now_iso;
use crate::creation::CreationStore;
use crate::generation::cutout;
use crate::generation::lk888::Lk888Client;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type SharedGenerationManager = Arc<GenerationManager>;

#[allow(dead_code)] // returned by persistence for subsequent candidate processing
#[derive(Debug, Clone)]
pub struct CandidatePaths {
    pub image_path: PathBuf,
    pub cutout_path: PathBuf,
    pub quality: String,
}

pub struct GenerationManager {
    store: Arc<Mutex<CreationStore>>,
    state_store: Arc<Mutex<crate::pets::state::StateStore>>,
    jobs_dir: Arc<Path>,
}

impl GenerationManager {
    pub fn new(
        store: Arc<Mutex<CreationStore>>,
        state_store: Arc<Mutex<crate::pets::state::StateStore>>,
        jobs_dir: Arc<Path>,
    ) -> Self {
        Self {
            store,
            state_store,
            jobs_dir,
        }
    }

    fn client_for(&self) -> Result<Lk888Client, String> {
        let state = self.state_store.lock().map_err(|_| "state lock poisoned")?;
        let key = state
            .load("app:lk888_api_key")?
            .or_else(|| std::env::var("LK888_API_KEY").ok())
            .unwrap_or_default();
        if key.is_empty() {
            return Err("请在设置页填入 lk888 API Key".into());
        }
        Ok(Lk888Client::new(key))
    }

    pub fn start(
        &self,
        pet_id: &str,
        prompt: &str,
        ref_png: &[u8],
        ref_sha256: &str,
    ) -> Result<String, String> {
        let job_id = format!("job-{}", now_iso());
        let client = self.client_for()?;
        let task_id = tauri::async_runtime::block_on(client.submit(prompt, Some(ref_png), "auto"))
            .map_err(|error| format!("submit failed: {error}"))?;
        {
            let store = self.store.lock().map_err(|_| "store lock poisoned")?;
            store.create_job(&job_id, pet_id, prompt, ref_sha256, Some(&task_id))?;
        }
        let state = self.state_store.lock().map_err(|_| "state lock poisoned")?;
        state.remove(&format!("creation:{pet_id}:compile_error"))?;
        Ok(job_id)
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), String> {
        let store = self.store.lock().map_err(|_| "store lock poisoned")?;
        store.update_job_status(job_id, "cancelled", None, None)
    }

    pub fn poll_all(&self) -> Result<Vec<String>, String> {
        let running = {
            let store = self.store.lock().map_err(|_| "store lock poisoned")?;
            store.running_jobs()?
        };
        let mut finished = Vec::new();
        for job in running {
            let Some(task_id) = job.task_id.clone() else {
                continue;
            };
            let client = self.client_for()?;
            let state = match tauri::async_runtime::block_on(client.poll(&task_id)) {
                Ok(state) => state,
                Err(error) => {
                    let store = self.store.lock().map_err(|_| "store lock poisoned")?;
                    store.update_job_status(
                        &job.job_id,
                        "failed",
                        None,
                        Some(&error.to_string()),
                    )?;
                    finished.push(job.job_id);
                    continue;
                }
            };
            if !state.is_final {
                continue;
            }
            let error = if state.state == "success" {
                let Some(url) = state.result_url.clone() else {
                    let store = self.store.lock().map_err(|_| "store lock poisoned")?;
                    store.update_job_status(&job.job_id, "failed", None, Some("no result url"))?;
                    finished.push(job.job_id);
                    continue;
                };
                match tauri::async_runtime::block_on(client.download(&url)) {
                    Ok(bytes) => {
                        let _ = self.complete_download(&job.job_id, &job.pet_id, &url, &bytes);
                        finished.push(job.job_id);
                        continue;
                    }
                    Err(error) => error.to_string(),
                }
            } else {
                state.error.unwrap_or_else(|| "generation failed".into())
            };
            let store = self.store.lock().map_err(|_| "store lock poisoned")?;
            store.update_job_status(&job.job_id, "failed", None, Some(&error))?;
            finished.push(job.job_id);
        }
        Ok(finished)
    }

    /// Resume unfinished jobs from a previous run.
    pub fn resume(&self) -> Result<usize, String> {
        let finished = self.poll_all()?;
        Ok(finished.len())
    }

    fn persist_result(
        &self,
        job_id: &str,
        pet_id: &str,
        bytes: &[u8],
    ) -> Result<CandidatePaths, String> {
        let dir = self.jobs_dir.join(job_id);
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let raw_path = dir.join("raw.png");
        std::fs::write(&raw_path, bytes).map_err(|error| error.to_string())?;
        let image = image::load_from_memory(bytes).map_err(|error| error.to_string())?;
        let (rgba, report) = cutout::remove_background(&image);
        let cutout_path = dir.join("cutout.png");
        rgba.save(&cutout_path).map_err(|error| error.to_string())?;
        let quality = if report.is_acceptable() {
            "acceptable"
        } else {
            "needs-review"
        };
        self.store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .record_candidate(
                job_id,
                pet_id,
                &raw_path.to_string_lossy(),
                &cutout_path.to_string_lossy(),
                quality,
            )?;
        Ok(CandidatePaths {
            image_path: raw_path,
            cutout_path,
            quality: quality.into(),
        })
    }

    fn complete_download(
        &self,
        job_id: &str,
        pet_id: &str,
        result_url: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let persisted = self.persist_result(job_id, pet_id, bytes);
        let store = self.store.lock().map_err(|_| "store lock poisoned")?;
        match persisted {
            Ok(_) => store.update_job_status(job_id, "success", Some(result_url), None),
            Err(error) => {
                store.update_job_status(job_id, "failed", None, Some(&error))?;
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pets::pet::{IdentityMode, Species};
    use crate::pets::repository::PetRepository;
    use crate::pets::state::StateStore;
    use crate::storage::Storage;
    use image::{DynamicImage, ImageFormat, RgbaImage};
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn png_bytes() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            64,
            64,
            image::Rgba([80, 90, 100, 255]),
        ))
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn candidate_persistence_conflict_marks_generation_job_failed() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-task-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let repo = PetRepository::new(storage.clone());
        let pet = repo.create(Species::Cat, IdentityMode::RealPet).unwrap();
        let other_pet = repo.create(Species::Dog, IdentityMode::Adopted).unwrap();
        let store = Arc::new(Mutex::new(CreationStore::new(storage.clone())));
        store
            .lock()
            .unwrap()
            .create_job("job-1", &pet.pet_id, "p", "h", Some("task-1"))
            .unwrap();
        {
            let db = &storage.lock().unwrap().db;
            db.execute(
                "INSERT INTO appearance_variants
                 (variant_id, pet_id, job_id, image_path, cutout_path, quality, accepted, created_at)
                 VALUES ('job-1', ?1, 'job-1', 'existing.png', 'existing-cutout.png', 'acceptable', 0, ?2)",
                rusqlite::params![other_pet.pet_id, now_iso()],
            )
            .unwrap();
        }
        let state = Arc::new(Mutex::new(StateStore::new(storage)));
        let manager =
            GenerationManager::new(store.clone(), state, Arc::from(root.join("jobs").as_path()));

        assert!(manager
            .complete_download(
                "job-1",
                &pet.pet_id,
                "https://example.invalid/out.png",
                &png_bytes()
            )
            .is_err());
        let job = store
            .lock()
            .unwrap()
            .job_list(&pet.pet_id)
            .unwrap()
            .remove(0);
        assert_eq!(job.status, "failed");
        assert!(job.error.is_some());
        let _ = std::fs::remove_dir_all(root);
    }
}
