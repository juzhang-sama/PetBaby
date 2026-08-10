use crate::creation::domain::new_entity_id;
use crate::creation::{CreationStore, JobRecord};
use crate::generation::cutout;
use crate::generation::lk888::Lk888Client;
use crate::runtime_assets::motion_profile;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type SharedGenerationManager = Arc<GenerationManager>;

#[allow(dead_code)] // returned by persistence for subsequent candidate processing
#[derive(Debug, Clone)]
pub struct CandidatePaths {
    pub image_path: PathBuf,
    pub cutout_path: PathBuf,
    pub motion_profile_path: PathBuf,
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
        let job_id = new_entity_id("job");
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

    pub fn start_for_session(
        &self,
        session_id: &str,
        prompt: &str,
        ref_png: &[u8],
        ref_sha256: &str,
    ) -> Result<String, String> {
        let pet_id = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .upload_session_pet(session_id)?;
        let job_id = new_entity_id("job");
        self.store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .create_job_for_session(&job_id, session_id, prompt, ref_sha256, None)?;

        let result: Result<(), String> = (|| {
            let client = self.client_for()?;
            let task_id =
                tauri::async_runtime::block_on(client.submit(prompt, Some(ref_png), "auto"))
                    .map_err(|error| format!("submit failed: {error}"))?;
            self.store
                .lock()
                .map_err(|_| "store lock poisoned")?
                .attach_task_to_upload_job(&job_id, session_id, &pet_id, &task_id)?;
            self.state_store
                .lock()
                .map_err(|_| "state lock poisoned")?
                .remove(&format!("creation:{pet_id}:compile_error"))?;
            Ok(())
        })();
        if let Err(error) = result {
            self.store
                .lock()
                .map_err(|_| "store lock poisoned")?
                .fail_upload_job(&job_id, session_id, &pet_id, &error)?;
            return Err(error);
        }
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
                self.settle_failure(&job, "generation job has no remote task id")?;
                finished.push(job.job_id);
                continue;
            };
            let client = match self.client_for() {
                Ok(client) => client,
                Err(error) => {
                    self.settle_failure(&job, &error)?;
                    finished.push(job.job_id);
                    continue;
                }
            };
            let state = match tauri::async_runtime::block_on(client.poll(&task_id)) {
                Ok(state) => state,
                Err(error) => {
                    self.settle_failure(&job, &error.to_string())?;
                    finished.push(job.job_id);
                    continue;
                }
            };
            if !state.is_final {
                continue;
            }
            let error = if state.state == "success" {
                let Some(url) = state.result_url.clone() else {
                    self.settle_failure(&job, "no result url")?;
                    finished.push(job.job_id);
                    continue;
                };
                match tauri::async_runtime::block_on(client.download(&url)) {
                    Ok(bytes) => {
                        let _ = self.complete_download(&job.job_id, &url, &bytes);
                        finished.push(job.job_id);
                        continue;
                    }
                    Err(error) => error.to_string(),
                }
            } else {
                state.error.unwrap_or_else(|| "generation failed".into())
            };
            self.settle_failure(&job, &error)?;
            finished.push(job.job_id);
        }
        Ok(finished)
    }

    /// Resume unfinished jobs from a previous run.
    pub fn resume(&self) -> Result<usize, String> {
        let finished = self.poll_all()?;
        Ok(finished.len())
    }

    fn persist_result(&self, job_id: &str, bytes: &[u8]) -> Result<CandidatePaths, String> {
        let dir = self.prepare_job_dir(job_id)?;
        let raw_path = dir.join("raw.png");
        reject_symbolic_or_non_file(&raw_path)?;
        std::fs::write(&raw_path, bytes).map_err(|error| error.to_string())?;
        let image = image::load_from_memory(bytes).map_err(|error| error.to_string())?;
        let (rgba, report) = cutout::remove_background(&image);
        let cutout_path = dir.join("cutout.png");
        reject_symbolic_or_non_file(&cutout_path)?;
        rgba.save(&cutout_path).map_err(|error| error.to_string())?;
        let profile = motion_profile::generate_motion_profile(&rgba)?;
        let motion_profile_path = dir.join("motion-profile.json");
        reject_symbolic_or_non_file(&motion_profile_path)?;
        motion_profile::write_motion_profile_atomic(&motion_profile_path, &profile)?;
        let quality = if report.is_acceptable() {
            "acceptable"
        } else {
            "needs-review"
        };
        Ok(CandidatePaths {
            image_path: raw_path,
            cutout_path,
            motion_profile_path,
            quality: quality.into(),
        })
    }

    fn complete_download(
        &self,
        job_id: &str,
        result_url: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let job = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .job(job_id)?;
        let persisted = match self.persist_result(job_id, bytes) {
            Ok(paths) => paths,
            Err(error) => {
                self.settle_failure(&job, &error)?;
                return Err(error);
            }
        };
        let persisted_result = {
            let store = self.store.lock().map_err(|_| "store lock poisoned")?;
            if let Some(session_id) = job.session_id.as_deref() {
                store
                    .record_upload_candidate_with_result_url(
                        job_id,
                        session_id,
                        &persisted.image_path.to_string_lossy(),
                        &persisted.cutout_path.to_string_lossy(),
                        &persisted.motion_profile_path.to_string_lossy(),
                        &persisted.quality,
                        result_url,
                    )
                    .map(|_| ())
            } else {
                store.record_candidate(
                    job_id,
                    &job.pet_id,
                    &persisted.image_path.to_string_lossy(),
                    &persisted.cutout_path.to_string_lossy(),
                    &persisted.quality,
                )?;
                store.update_job_status(job_id, "success", Some(result_url), None)
            }
        };
        if let Err(error) = persisted_result {
            self.settle_failure(&job, &error)?;
            return Err(error);
        }
        Ok(())
    }

    fn settle_failure(&self, job: &JobRecord, error: &str) -> Result<(), String> {
        let store = self.store.lock().map_err(|_| "store lock poisoned")?;
        if let Some(session_id) = job.session_id.as_deref() {
            store.fail_upload_job(&job.job_id, session_id, &job.pet_id, error)
        } else {
            store.update_job_status(&job.job_id, "failed", None, Some(error))
        }
    }

    fn prepare_job_dir(&self, job_id: &str) -> Result<PathBuf, String> {
        validate_component(job_id, "job id")?;
        std::fs::create_dir_all(self.jobs_dir.as_ref()).map_err(|error| error.to_string())?;
        let root_metadata =
            std::fs::symlink_metadata(self.jobs_dir.as_ref()).map_err(|error| error.to_string())?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err("jobs directory cannot be a symbolic link".into());
        }
        let canonical_root = self
            .jobs_dir
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let dir = self.jobs_dir.join(job_id);
        if let Ok(metadata) = std::fs::symlink_metadata(&dir) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("job directory cannot be a symbolic link".into());
            }
        }
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let canonical_dir = dir.canonicalize().map_err(|error| error.to_string())?;
        if canonical_dir.parent() != Some(canonical_root.as_path())
            || canonical_dir.file_name().and_then(|name| name.to_str()) != Some(job_id)
        {
            return Err("job directory escapes the configured jobs directory".into());
        }
        Ok(canonical_dir)
    }
}

fn validate_component(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!("invalid {label}"));
    }
    Ok(())
}

fn reject_symbolic_or_non_file(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(format!(
            "generation output is not a regular file: {}",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::profiles::now_iso;
    use crate::pets::pet::{IdentityMode, Species};
    use crate::pets::repository::PetRepository;
    use crate::pets::state::StateStore;
    use crate::storage::Storage;
    use image::{DynamicImage, ImageFormat, RgbaImage};
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct ManagerHarness {
        root: std::path::PathBuf,
        manager: GenerationManager,
        store: Arc<Mutex<CreationStore>>,
        storage: Arc<Mutex<Storage>>,
        session_id: String,
        pet_id: String,
        png: Vec<u8>,
    }

    impl Drop for ManagerHarness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn manager_harness_with_job(create_job: bool) -> ManagerHarness {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-motion-task-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let pet = PetRepository::new(storage.clone())
            .create(Species::Cat, IdentityMode::RealPet)
            .unwrap();
        let session_id = crate::creation::domain::new_entity_id("session");
        let now = crate::creation::profiles::now_iso();
        storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO creation_sessions
                 (session_id, pet_id, method, status, last_stable_status, current_step,
                  schema_version, created_at, updated_at)
                 VALUES (?1, ?2, 'upload', 'draft', 'draft', 'upload', 1, ?3, ?3)",
                rusqlite::params![session_id, pet.pet_id, now],
            )
            .unwrap();
        let store = Arc::new(Mutex::new(CreationStore::new(storage.clone())));
        if create_job {
            store
                .lock()
                .unwrap()
                .create_job_for_session("job-1", &session_id, "p", "h", Some("task-1"))
                .unwrap();
        }
        let state = Arc::new(Mutex::new(StateStore::new(storage.clone())));
        let manager =
            GenerationManager::new(store.clone(), state, Arc::from(root.join("jobs").as_path()));
        ManagerHarness {
            root,
            manager,
            store,
            storage,
            session_id,
            pet_id: pet.pet_id,
            png: png_bytes(),
        }
    }

    fn manager_harness() -> ManagerHarness {
        manager_harness_with_job(true)
    }

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
    fn persisted_candidate_contains_a_valid_motion_profile() {
        let test = manager_harness();
        let result = test.manager.persist_result("job-1", &test.png).unwrap();
        assert!(result.motion_profile_path.ends_with("motion-profile.json"));
        let json = std::fs::read_to_string(result.motion_profile_path).unwrap();
        assert_eq!(
            crate::runtime_assets::motion_profile::parse_motion_profile(&json)
                .unwrap()
                .engine_profile,
            "life-v1"
        );
    }

    #[test]
    fn motion_profile_failure_does_not_record_candidate_or_success() {
        let test = manager_harness();
        std::fs::create_dir_all(test.root.join("jobs/job-1/motion-profile.json")).unwrap();

        assert!(test
            .manager
            .complete_download("job-1", "https://example.invalid/out.png", &test.png,)
            .is_err());

        let store = test.manager.store.lock().unwrap();
        let job = store.upload_jobs(&test.session_id).unwrap().remove(0);
        assert_eq!(job.status, "failed");
        assert!(store.candidate_for_session(&test.session_id).is_err());
        let session: (String, String, String) = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT status, last_stable_status, current_step
                 FROM creation_sessions WHERE session_id=?1",
                [&test.session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            session,
            ("retryableFailure".into(), "draft".into(), "upload".into())
        );
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
        let session_id = crate::creation::domain::new_entity_id("session");
        let now = now_iso();
        storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO creation_sessions
                 (session_id, pet_id, method, status, last_stable_status, current_step,
                  schema_version, created_at, updated_at)
                 VALUES (?1, ?2, 'upload', 'draft', 'draft', 'upload', 1, ?3, ?3)",
                rusqlite::params![session_id, pet.pet_id, now],
            )
            .unwrap();
        let store = Arc::new(Mutex::new(CreationStore::new(storage.clone())));
        store
            .lock()
            .unwrap()
            .create_job_for_session("job-1", &session_id, "p", "h", Some("task-1"))
            .unwrap();
        {
            let db = &storage.lock().unwrap().db;
            db.execute(
                "INSERT INTO appearance_variants
                 (variant_id, pet_id, job_id, session_id, image_path, cutout_path,
                  motion_profile_path, quality, accepted, created_at)
                 VALUES ('candidate-existing', ?1, 'job-1', ?2, 'existing.png',
                         'existing-cutout.png', 'motion-profile.json', 'acceptable', 0, ?3)",
                rusqlite::params![pet.pet_id, session_id, now_iso()],
            )
            .unwrap();
        }
        let state = Arc::new(Mutex::new(StateStore::new(storage)));
        let manager =
            GenerationManager::new(store.clone(), state, Arc::from(root.join("jobs").as_path()));

        assert!(manager
            .complete_download("job-1", "https://example.invalid/out.png", &png_bytes())
            .is_err());
        let job = store
            .lock()
            .unwrap()
            .upload_jobs(&session_id)
            .unwrap()
            .remove(0);
        assert_eq!(job.status, "failed");
        assert!(job.error.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn successful_download_atomically_records_standard_candidate_and_session_state() {
        let test = manager_harness();

        test.manager
            .complete_download("job-1", "https://example.invalid/out.png", &test.png)
            .unwrap();

        let candidate = test
            .store
            .lock()
            .unwrap()
            .candidate_for_session(&test.session_id)
            .unwrap();
        assert_eq!(candidate.session_id, test.session_id);
        assert_eq!(candidate.pet_id, test.pet_id);
        assert!(candidate.body_path.ends_with("cutout.png"));
        assert!(candidate
            .motion_profile_path
            .ends_with("motion-profile.json"));
        let jobs = test
            .store
            .lock()
            .unwrap()
            .upload_jobs(&test.session_id)
            .unwrap();
        assert_eq!(jobs[0].status, "success");
        assert_eq!(
            jobs[0].result_url.as_deref(),
            Some("https://example.invalid/out.png")
        );
    }

    #[test]
    fn api_configuration_failure_marks_new_job_and_session_retryable() {
        let test = manager_harness_with_job(false);

        let error = test
            .manager
            .start_for_session(&test.session_id, "p", &test.png, "h")
            .unwrap_err();

        assert!(error.contains("API Key"));
        let jobs = test
            .store
            .lock()
            .unwrap()
            .upload_jobs(&test.session_id)
            .unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].status, "failed");
        assert!(jobs[0].job_id.starts_with("job-"));
        let state: (String, String, String) = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT status, last_stable_status, current_step
                 FROM creation_sessions WHERE session_id=?1",
                [&test.session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            state,
            ("retryableFailure".into(), "draft".into(), "upload".into())
        );
    }

    #[test]
    fn result_persistence_rejects_path_like_job_ids() {
        let test = manager_harness();
        assert!(test.manager.persist_result("../escape", &test.png).is_err());
        assert!(!test.root.join("escape").exists());
    }
}
