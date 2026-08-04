use crate::creation::profiles::now_iso;
use crate::creation::CreationStore;
use crate::generation::cutout;
use crate::generation::lk888::Lk888Client;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type SharedGenerationManager = Arc<GenerationManager>;

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
        let store = self.store.lock().map_err(|_| "store lock poisoned")?;
        store.create_job(&job_id, pet_id, prompt, ref_sha256, Some(&task_id))?;
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
            let store = self.store.lock().map_err(|_| "store lock poisoned")?;
            if state.state == "success" {
                let Some(url) = state.result_url.clone() else {
                    store.update_job_status(&job.job_id, "failed", None, Some("no result url"))?;
                    finished.push(job.job_id);
                    continue;
                };
                match tauri::async_runtime::block_on(client.download(&url)) {
                    Ok(bytes) => {
                        self.persist_result(&job.job_id, &bytes);
                        store.update_job_status(&job.job_id, "success", Some(&url), None)?;
                    }
                    Err(error) => {
                        store.update_job_status(
                            &job.job_id,
                            "failed",
                            None,
                            Some(&error.to_string()),
                        )?;
                    }
                }
            } else {
                store.update_job_status(
                    &job.job_id,
                    "failed",
                    None,
                    Some(&state.error.unwrap_or_else(|| "generation failed".into())),
                )?;
            }
            finished.push(job.job_id);
        }
        Ok(finished)
    }

    /// Resume unfinished jobs from a previous run.
    pub fn resume(&self) -> Result<usize, String> {
        let finished = self.poll_all()?;
        Ok(finished.len())
    }

    fn persist_result(&self, job_id: &str, bytes: &[u8]) -> Option<PathBuf> {
        let dir = self.jobs_dir.join(job_id);
        std::fs::create_dir_all(&dir).ok()?;
        let raw_path = dir.join("raw.png");
        std::fs::write(&raw_path, bytes).ok()?;
        let image = image::load_from_memory(bytes).ok()?;
        let (rgba, _report) = cutout::remove_background(&image);
        let cutout_path = dir.join("cutout.png");
        rgba.save(&cutout_path).ok()?;
        Some(cutout_path)
    }
}
