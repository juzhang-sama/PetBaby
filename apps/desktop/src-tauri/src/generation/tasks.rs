use crate::creation::domain::new_entity_id;
use crate::creation::{CreationStore, JobRecord};
use crate::generation::cutout;
use crate::generation::lk888::Lk888Client;
use crate::pets::mutation::{MutationKind, SharedPetMutationGate};
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
    mutation_gate: SharedPetMutationGate,
    #[cfg(test)]
    after_stage_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    before_candidate_commit_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    fail_next_stage_promote_rename: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    submit_hook: Mutex<Option<Arc<dyn Fn() -> Result<String, String> + Send + Sync>>>,
    #[cfg(test)]
    after_task_attach_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    fail_next_task_attach: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    fail_next_backup_cleanup: std::sync::atomic::AtomicBool,
}

impl GenerationManager {
    pub fn new(
        store: Arc<Mutex<CreationStore>>,
        state_store: Arc<Mutex<crate::pets::state::StateStore>>,
        jobs_dir: Arc<Path>,
        mutation_gate: SharedPetMutationGate,
    ) -> Self {
        Self {
            store,
            state_store,
            jobs_dir,
            mutation_gate,
            #[cfg(test)]
            after_stage_hook: Mutex::new(None),
            #[cfg(test)]
            before_candidate_commit_hook: Mutex::new(None),
            #[cfg(test)]
            fail_next_stage_promote_rename: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            submit_hook: Mutex::new(None),
            #[cfg(test)]
            after_task_attach_hook: Mutex::new(None),
            #[cfg(test)]
            fail_next_task_attach: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_next_backup_cleanup: std::sync::atomic::AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    fn set_after_stage_hook(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self.after_stage_hook.lock().unwrap() = Some(Arc::new(hook));
    }

    #[cfg(test)]
    fn set_before_candidate_commit_hook(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self.before_candidate_commit_hook.lock().unwrap() = Some(Arc::new(hook));
    }

    #[cfg(test)]
    fn fail_next_stage_promote_rename(&self) {
        self.fail_next_stage_promote_rename
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    fn set_submit_hook(&self, hook: impl Fn() -> Result<String, String> + Send + Sync + 'static) {
        *self.submit_hook.lock().unwrap() = Some(Arc::new(hook));
    }

    #[cfg(test)]
    fn set_after_task_attach_hook(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self.after_task_attach_hook.lock().unwrap() = Some(Arc::new(hook));
    }

    #[cfg(test)]
    fn fail_next_task_attach(&self) {
        self.fail_next_task_attach
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    fn fail_next_backup_cleanup(&self) {
        self.fail_next_backup_cleanup
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    fn run_hook(hook: &Mutex<Option<Arc<dyn Fn() + Send + Sync>>>) {
        if let Some(hook) = hook.lock().unwrap().clone() {
            hook();
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

    fn submit_task(&self, prompt: &str, ref_png: &[u8]) -> Result<String, String> {
        #[cfg(test)]
        if let Some(hook) = self.submit_hook.lock().unwrap().clone() {
            return hook();
        }
        let client = self.client_for()?;
        tauri::async_runtime::block_on(client.submit(prompt, Some(ref_png), "auto"))
            .map_err(|error| format!("submit failed: {error}"))
    }

    pub fn start(
        &self,
        pet_id: &str,
        prompt: &str,
        ref_png: &[u8],
        ref_sha256: &str,
    ) -> Result<String, String> {
        let job_id = new_entity_id("job");
        let _operation = self
            .mutation_gate
            .scoped(&job_id, MutationKind::Creation, pet_id)?;
        self.store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .create_job(&job_id, pet_id, prompt, ref_sha256, None)?;
        let task_id = match self.submit_task(prompt, ref_png) {
            Ok(task_id) => task_id,
            Err(error) => {
                self.store
                    .lock()
                    .map_err(|_| "store lock poisoned")?
                    .fail_legacy_job_if_active(&job_id, &error)?;
                return Err(error);
            }
        };
        if let Err(error) = self.attach_legacy_task(&job_id, pet_id, &task_id) {
            let preservation = self
                .store
                .lock()
                .map_err(|_| "store lock poisoned")?
                .preserve_legacy_task_after_attach_failure(&job_id, pet_id, &task_id);
            return Err(combine_attach_and_preservation_error(error, preservation));
        }
        #[cfg(test)]
        Self::run_hook(&self.after_task_attach_hook);
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
        let job_id = new_entity_id("job");
        let _operation = self
            .mutation_gate
            .scoped(&job_id, MutationKind::Creation, session_id)?;
        let pet_id = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .upload_session_pet(session_id)?;
        self.store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .create_job_for_session(&job_id, session_id, prompt, ref_sha256, None)?;

        let task_id = match self.submit_task(prompt, ref_png) {
            Ok(task_id) => task_id,
            Err(error) => {
                self.store
                    .lock()
                    .map_err(|_| "store lock poisoned")?
                    .fail_upload_job(&job_id, session_id, &pet_id, &error)?;
                return Err(error);
            }
        };
        if let Err(error) = self.attach_upload_task(&job_id, session_id, &pet_id, &task_id) {
            let preservation = self
                .store
                .lock()
                .map_err(|_| "store lock poisoned")?
                .preserve_upload_task_after_attach_failure(&job_id, session_id, &pet_id, &task_id);
            return Err(combine_attach_and_preservation_error(error, preservation));
        }
        #[cfg(test)]
        Self::run_hook(&self.after_task_attach_hook);
        self.state_store
            .lock()
            .map_err(|_| "state lock poisoned")?
            .remove(&format!("creation:{pet_id}:compile_error"))?;
        Ok(job_id)
    }

    fn attach_upload_task(
        &self,
        job_id: &str,
        session_id: &str,
        pet_id: &str,
        task_id: &str,
    ) -> Result<(), String> {
        #[cfg(test)]
        if self
            .fail_next_task_attach
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err("injected upload task attachment failure".into());
        }
        self.store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .attach_task_to_upload_job(job_id, session_id, pet_id, task_id)
    }

    fn attach_legacy_task(&self, job_id: &str, pet_id: &str, task_id: &str) -> Result<(), String> {
        #[cfg(test)]
        if self
            .fail_next_task_attach
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err("injected legacy task attachment failure".into());
        }
        self.store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .attach_task_to_legacy_job(job_id, pet_id, task_id)
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), String> {
        let request_id = new_entity_id("cancel");
        let _operation = self
            .mutation_gate
            .scoped(&request_id, MutationKind::Creation, job_id)?;
        let store = self.store.lock().map_err(|_| "store lock poisoned")?;
        store.cancel_job(job_id)
    }

    pub fn poll_all(&self) -> Result<Vec<String>, String> {
        let running = {
            let store = self.store.lock().map_err(|_| "store lock poisoned")?;
            store.running_jobs()?
        };
        let mut finished = Vec::new();
        for job in running {
            let Some(task_id) = job.task_id.clone() else {
                self.record_failed_poll(
                    &job,
                    "generation job has no remote task id",
                    &mut finished,
                )?;
                continue;
            };
            let client = match self.client_for() {
                Ok(client) => client,
                Err(error) => {
                    self.record_failed_poll(&job, &error, &mut finished)?;
                    continue;
                }
            };
            let state = match tauri::async_runtime::block_on(client.poll(&task_id)) {
                Ok(state) => state,
                Err(error) => {
                    self.record_failed_poll(&job, &error.to_string(), &mut finished)?;
                    continue;
                }
            };
            if !state.is_final {
                continue;
            }
            let error = if state.state == "success" {
                let Some(url) = state.result_url.clone() else {
                    self.record_failed_poll(&job, "no result url", &mut finished)?;
                    continue;
                };
                match tauri::async_runtime::block_on(client.download(&url)) {
                    Ok(bytes) => {
                        let completion = self.complete_download(&job.job_id, &url, &bytes);
                        self.record_poll_completion(&job.job_id, completion, &mut finished)?;
                        continue;
                    }
                    Err(error) => error.to_string(),
                }
            } else {
                state.error.unwrap_or_else(|| "generation failed".into())
            };
            self.record_failed_poll(&job, &error, &mut finished)?;
        }
        Ok(finished)
    }

    /// Resume unfinished jobs from a previous run.
    pub fn resume(&self) -> Result<usize, String> {
        let stale = {
            let request_id = new_entity_id("resume");
            let _operation = self.mutation_gate.scoped(
                &request_id,
                MutationKind::Creation,
                "generation-resume",
            )?;
            self.store
                .lock()
                .map_err(|_| "store lock poisoned")?
                .fail_stale_submitting_jobs("generation submission was interrupted")?
        };
        let finished = self.poll_all()?;
        Ok(stale + finished.len())
    }

    fn stage_result(&self, job_id: &str, bytes: &[u8]) -> Result<StagedCandidate, String> {
        validate_component(job_id, "job id")?;
        let jobs_root = self.canonical_jobs_root()?;
        let staging_dir = jobs_root.join(format!(".{job_id}-staging-{}", new_entity_id("stage")));
        std::fs::create_dir(&staging_dir).map_err(|error| error.to_string())?;
        let mut staged = StagedCandidate::new(staging_dir);
        let raw_path = staged.dir.join("raw.png");
        reject_symbolic_or_non_file(&raw_path)?;
        std::fs::write(&raw_path, bytes).map_err(|error| error.to_string())?;
        let image = image::load_from_memory(bytes).map_err(|error| error.to_string())?;
        let (rgba, report) = cutout::remove_background(&image);
        let cutout_path = staged.dir.join("cutout.png");
        reject_symbolic_or_non_file(&cutout_path)?;
        rgba.save(&cutout_path).map_err(|error| error.to_string())?;
        let profile = motion_profile::generate_motion_profile(&rgba)?;
        let motion_profile_path = staged.dir.join("motion-profile.json");
        reject_symbolic_or_non_file(&motion_profile_path)?;
        motion_profile::write_motion_profile_atomic(&motion_profile_path, &profile)?;
        staged.quality = if report.is_acceptable() {
            "acceptable".into()
        } else {
            "needs-review".into()
        };
        Ok(staged)
    }

    fn complete_download(
        &self,
        job_id: &str,
        result_url: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let observed_job = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .job(job_id)?;
        let staged_result = self.stage_result(job_id, bytes);
        #[cfg(test)]
        if staged_result.is_ok() {
            Self::run_hook(&self.after_stage_hook);
        }

        let request_id = format!("complete-{job_id}");
        let _operation =
            self.mutation_gate
                .scoped(&request_id, MutationKind::Creation, &observed_job.pet_id)?;
        let job = self.revalidate_job_for_completion(&observed_job)?;
        let mut staged = match staged_result {
            Ok(staged) => staged,
            Err(error) => {
                return Err(self.failure_with_settlement(&job, &error));
            }
        };
        let persisted = match self.promote_staged_candidate(
            &mut staged,
            &self.canonical_jobs_root()?,
            job_id,
        ) {
            Ok(paths) => paths,
            Err(error) => {
                let rollback = staged.rollback();
                return Err(self.failure_with_rollback_and_settlement(&job, &error, rollback));
            }
        };

        #[cfg(test)]
        Self::run_hook(&self.before_candidate_commit_hook);

        let persisted_result = {
            let store = self.store.lock().map_err(|_| "store lock poisoned")?;
            if let Some(session_id) = job.session_id.as_deref() {
                store
                    .record_upload_candidate_with_result_url(
                        job_id,
                        session_id,
                        &self.jobs_dir,
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
            let rollback = staged.rollback();
            return Err(self.failure_with_rollback_and_settlement(&job, &error, rollback));
        }
        #[cfg(test)]
        let skip_backup_cleanup = self
            .fail_next_backup_cleanup
            .swap(false, std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(test))]
        let skip_backup_cleanup = false;
        staged.commit(skip_backup_cleanup);
        Ok(())
    }

    fn promote_staged_candidate(
        &self,
        staged: &mut StagedCandidate,
        jobs_root: &Path,
        job_id: &str,
    ) -> Result<CandidatePaths, String> {
        #[cfg(test)]
        if self
            .fail_next_stage_promote_rename
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return staged.promote_with(jobs_root, job_id, |_, _| {
                Err("injected staging promotion rename failure".into())
            });
        }
        staged.promote_with(jobs_root, job_id, |from, to| {
            std::fs::rename(from, to).map_err(|error| error.to_string())
        })
    }

    fn revalidate_job_for_completion(&self, observed: &JobRecord) -> Result<JobRecord, String> {
        let store = self.store.lock().map_err(|_| "store lock poisoned")?;
        if let Some(session_id) = observed.session_id.as_deref() {
            store.live_upload_job_for_completion(&observed.job_id, session_id, &observed.pet_id)
        } else {
            let current = store.job(&observed.job_id)?;
            if current.session_id.is_some()
                || current.pet_id != observed.pet_id
                || !matches!(current.status.as_str(), "pending" | "running")
            {
                return Err("legacy generation job is no longer active".into());
            }
            Ok(current)
        }
    }

    fn failure_with_settlement(&self, job: &JobRecord, error: &str) -> String {
        match self.settle_failure_locked(job, error) {
            Ok(()) => error.into(),
            Err(settlement) => format!("{error}; failure settlement failed: {settlement}"),
        }
    }

    fn failure_with_rollback_and_settlement(
        &self,
        job: &JobRecord,
        error: &str,
        rollback: Result<(), String>,
    ) -> String {
        let mut combined = error.to_string();
        if let Err(rollback) = rollback {
            combined.push_str(&format!("; output rollback failed: {rollback}"));
        }
        self.failure_with_settlement(job, &combined)
    }

    fn settle_failure(&self, job: &JobRecord, error: &str) -> Result<(), String> {
        let request_id = format!("settle-{}", job.job_id);
        let _operation =
            self.mutation_gate
                .scoped(&request_id, MutationKind::Creation, &job.pet_id)?;
        self.settle_failure_locked(job, error)
    }

    fn settle_failure_locked(&self, job: &JobRecord, error: &str) -> Result<(), String> {
        let store = self.store.lock().map_err(|_| "store lock poisoned")?;
        if let Some(session_id) = job.session_id.as_deref() {
            store.fail_upload_job(&job.job_id, session_id, &job.pet_id, error)
        } else {
            store.fail_legacy_job_if_active(&job.job_id, error)
        }
    }

    fn record_failed_poll(
        &self,
        job: &JobRecord,
        error: &str,
        finished: &mut Vec<String>,
    ) -> Result<(), String> {
        let settlement = self.settle_failure(job, error);
        self.record_poll_completion(&job.job_id, settlement, finished)
    }

    fn record_poll_completion(
        &self,
        job_id: &str,
        completion: Result<(), String>,
        finished: &mut Vec<String>,
    ) -> Result<(), String> {
        let status = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .job_status(job_id)?;
        if matches!(status.as_deref(), Some("success" | "failed" | "cancelled")) {
            finished.push(job_id.into());
            return Ok(());
        }
        Err(match completion {
            Ok(()) => format!("job {job_id} completion was not durably confirmed"),
            Err(error) => error,
        })
    }

    fn canonical_jobs_root(&self) -> Result<PathBuf, String> {
        std::fs::create_dir_all(self.jobs_dir.as_ref()).map_err(|error| error.to_string())?;
        let metadata =
            std::fs::symlink_metadata(self.jobs_dir.as_ref()).map_err(|error| error.to_string())?;
        if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            return Err("jobs directory cannot be a link or reparse point".into());
        }
        self.jobs_dir
            .canonicalize()
            .map_err(|error| error.to_string())
    }
}

struct StagedCandidate {
    dir: PathBuf,
    quality: String,
    target_dir: Option<PathBuf>,
    backup_dir: Option<PathBuf>,
    promoted: bool,
    rolled_back: bool,
    committed: bool,
}

impl StagedCandidate {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            quality: String::new(),
            target_dir: None,
            backup_dir: None,
            promoted: false,
            rolled_back: false,
            committed: false,
        }
    }

    fn promote_with(
        &mut self,
        jobs_root: &Path,
        job_id: &str,
        promote_rename: impl FnOnce(&Path, &Path) -> Result<(), String>,
    ) -> Result<CandidatePaths, String> {
        let final_dir = jobs_root.join(job_id);
        self.target_dir = Some(final_dir.clone());
        if let Ok(metadata) = std::fs::symlink_metadata(&final_dir) {
            if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
                return Err(
                    "final job directory is a link, reparse point, or non-directory".into(),
                );
            }
            let canonical_final = final_dir
                .canonicalize()
                .map_err(|error| error.to_string())?;
            if canonical_final != final_dir {
                return Err("final job directory escapes the configured jobs root".into());
            }
            let backup_dir = jobs_root.join(format!(
                ".{job_id}-backup-{}",
                new_entity_id("candidate-backup")
            ));
            if std::fs::symlink_metadata(&backup_dir).is_ok() {
                return Err("candidate backup path unexpectedly exists".into());
            }
            std::fs::rename(&final_dir, &backup_dir).map_err(|error| error.to_string())?;
            self.backup_dir = Some(backup_dir);
        }
        if let Err(error) = promote_rename(&self.dir, &final_dir) {
            let restore = self.restore_backup();
            return Err(match restore {
                Ok(()) => error,
                Err(restore) => format!("{error}; backup restore failed: {restore}"),
            });
        }
        self.dir = final_dir.clone();
        self.promoted = true;
        Ok(CandidatePaths {
            image_path: final_dir.join("raw.png"),
            cutout_path: final_dir.join("cutout.png"),
            motion_profile_path: final_dir.join("motion-profile.json"),
            quality: self.quality.clone(),
        })
    }

    fn rollback(&mut self) -> Result<(), String> {
        if self.committed || self.rolled_back {
            return Ok(());
        }
        let mut errors = Vec::new();
        if self.promoted {
            if let Err(error) = std::fs::remove_dir_all(&self.dir) {
                errors.push(format!("could not remove promoted output: {error}"));
            } else {
                self.promoted = false;
            }
        } else if self.dir.exists() {
            if let Err(error) = std::fs::remove_dir_all(&self.dir) {
                errors.push(format!("could not remove staging output: {error}"));
            }
        }
        if let Err(error) = self.restore_backup() {
            errors.push(error);
        }
        if errors.is_empty() {
            self.rolled_back = true;
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn restore_backup(&mut self) -> Result<(), String> {
        let Some(backup_dir) = self.backup_dir.as_ref() else {
            return Ok(());
        };
        let final_dir = self
            .target_dir
            .as_ref()
            .ok_or_else(|| "candidate backup has no target directory".to_string())?;
        if std::fs::symlink_metadata(final_dir).is_ok() {
            return Err("cannot restore candidate backup over an existing final directory".into());
        }
        std::fs::rename(backup_dir, final_dir)
            .map_err(|error| format!("could not restore candidate backup: {error}"))?;
        self.backup_dir = None;
        Ok(())
    }

    fn commit(&mut self, skip_backup_cleanup: bool) {
        self.committed = true;
        if skip_backup_cleanup {
            return;
        }
        if let Some(backup_dir) = self.backup_dir.as_ref() {
            let removable = std::fs::symlink_metadata(backup_dir)
                .map(|metadata| {
                    metadata.is_dir() && !crate::platform::is_link_or_reparse_point(&metadata)
                })
                .unwrap_or(false);
            if removable && std::fs::remove_dir_all(backup_dir).is_ok() {
                self.backup_dir = None;
            }
        }
    }
}

impl Drop for StagedCandidate {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.rollback();
        }
    }
}

fn combine_attach_and_preservation_error(
    attach_error: String,
    preservation: Result<(), String>,
) -> String {
    match preservation {
        Ok(()) => format!("{attach_error}; remote task id was preserved for retryable recovery"),
        Err(preservation_error) => {
            format!("{attach_error}; remote task preservation failed: {preservation_error}")
        }
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
    use crate::pets::active::{ActivePetService, BUILTIN_PET_ID};
    use crate::pets::deletion::PetDeletionService;
    use crate::pets::mutation::{PetMutationGate, SharedPetMutationGate};
    use crate::pets::pet::{IdentityMode, Species};
    use crate::pets::repository::PetRepository;
    use crate::pets::state::StateStore;
    use crate::pets::{ActivePetSession, SharedActivePetSession};
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
        gate: SharedPetMutationGate,
    }

    struct LegacyManagerHarness {
        root: std::path::PathBuf,
        manager: GenerationManager,
        store: Arc<Mutex<CreationStore>>,
        storage: Arc<Mutex<Storage>>,
        pet_id: String,
        png: Vec<u8>,
        gate: SharedPetMutationGate,
    }

    impl Drop for ManagerHarness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Drop for LegacyManagerHarness {
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
        let gate = Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60)));
        let manager = GenerationManager::new(
            store.clone(),
            state,
            Arc::from(root.join("jobs").as_path()),
            gate.clone(),
        );
        ManagerHarness {
            root,
            manager,
            store,
            storage,
            session_id,
            pet_id: pet.pet_id,
            png: png_bytes(),
            gate,
        }
    }

    fn manager_harness() -> ManagerHarness {
        manager_harness_with_job(true)
    }

    fn legacy_manager_harness() -> LegacyManagerHarness {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-legacy-manager-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let pet = PetRepository::new(storage.clone())
            .create(Species::Cat, IdentityMode::RealPet)
            .unwrap();
        let store = Arc::new(Mutex::new(CreationStore::new(storage.clone())));
        let state = Arc::new(Mutex::new(StateStore::new(storage.clone())));
        let gate = Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60)));
        let manager = GenerationManager::new(
            store.clone(),
            state,
            Arc::from(root.join("jobs").as_path()),
            gate.clone(),
        );
        LegacyManagerHarness {
            root,
            manager,
            store,
            storage,
            pet_id: pet.pet_id,
            png: png_bytes(),
            gate,
        }
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

    fn uniform_white_png_bytes() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            64,
            64,
            image::Rgba([255, 255, 255, 255]),
        ))
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
        bytes.into_inner()
    }

    fn deletion_service(test: &ManagerHarness) -> PetDeletionService {
        let session: SharedActivePetSession = Arc::new(Mutex::new(ActivePetSession::new()));
        session
            .lock()
            .unwrap()
            .set_active(BUILTIN_PET_ID.into())
            .unwrap();
        let active = Arc::new(ActivePetService::new(
            test.storage.clone(),
            session,
            test.root.join("pets"),
            test.gate.clone(),
        ));
        PetDeletionService::new(
            test.storage.clone(),
            active,
            test.root.clone(),
            test.gate.clone(),
        )
    }

    fn legacy_deletion_service(test: &LegacyManagerHarness) -> PetDeletionService {
        let session: SharedActivePetSession = Arc::new(Mutex::new(ActivePetSession::new()));
        session
            .lock()
            .unwrap()
            .set_active(BUILTIN_PET_ID.into())
            .unwrap();
        let active = Arc::new(ActivePetService::new(
            test.storage.clone(),
            session,
            test.root.join("pets"),
            test.gate.clone(),
        ));
        PetDeletionService::new(
            test.storage.clone(),
            active,
            test.root.clone(),
            test.gate.clone(),
        )
    }

    fn install_existing_final(test: &ManagerHarness) {
        let final_dir = test.root.join("jobs/job-1");
        std::fs::create_dir_all(&final_dir).unwrap();
        std::fs::write(final_dir.join("raw.png"), b"old raw").unwrap();
        std::fs::write(final_dir.join("cutout.png"), b"old cutout").unwrap();
        std::fs::write(final_dir.join("motion-profile.json"), b"old profile").unwrap();
        std::fs::write(final_dir.join("sentinel"), b"keep old final").unwrap();
    }

    fn assert_existing_final_restored(test: &ManagerHarness) {
        let final_dir = test.root.join("jobs/job-1");
        assert_eq!(
            std::fs::read(final_dir.join("raw.png")).unwrap(),
            b"old raw"
        );
        assert_eq!(
            std::fs::read(final_dir.join("cutout.png")).unwrap(),
            b"old cutout"
        );
        assert_eq!(
            std::fs::read(final_dir.join("motion-profile.json")).unwrap(),
            b"old profile"
        );
        assert_eq!(
            std::fs::read(final_dir.join("sentinel")).unwrap(),
            b"keep old final"
        );
    }

    fn assert_no_job_siblings(test: &ManagerHarness) {
        let names: Vec<_> = std::fs::read_dir(test.root.join("jobs"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name != "job-1")
            .collect();
        assert!(names.is_empty(), "unexpected job siblings: {names:?}");
    }

    #[test]
    fn persisted_candidate_contains_a_valid_motion_profile() {
        let test = manager_harness();
        let staged = test.manager.stage_result("job-1", &test.png).unwrap();
        let motion_profile_path = staged.dir.join("motion-profile.json");
        assert!(motion_profile_path.ends_with("motion-profile.json"));
        let json = std::fs::read_to_string(motion_profile_path).unwrap();
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

        assert!(test
            .manager
            .complete_download(
                "job-1",
                "https://example.invalid/out.png",
                &uniform_white_png_bytes(),
            )
            .is_err());

        let store = test.manager.store.lock().unwrap();
        let job = store.upload_jobs(&test.session_id).unwrap().remove(0);
        assert_eq!(job.status, "failed");
        assert!(store.candidate_for_session(&test.session_id).is_err());
        assert!(!test.root.join("jobs/job-1").exists());
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
        let manager = GenerationManager::new(
            store.clone(),
            state,
            Arc::from(root.join("jobs").as_path()),
            Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60))),
        );

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
    fn upload_submit_reservation_is_not_polled_before_task_attachment() {
        let test = manager_harness_with_job(false);
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        test.manager.set_submit_hook({
            let entered = entered.clone();
            let release = release.clone();
            move || {
                entered.wait();
                release.wait();
                Ok("remote-task".into())
            }
        });

        std::thread::scope(|scope| {
            let start = scope.spawn(|| {
                test.manager
                    .start_for_session(&test.session_id, "p", &test.png, "h")
            });
            entered.wait();
            let jobs = test
                .store
                .lock()
                .unwrap()
                .upload_jobs(&test.session_id)
                .unwrap();
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].status, "submitting");
            assert!(jobs[0].task_id.is_none());
            assert!(test.manager.poll_all().unwrap().is_empty());
            release.wait();
            let job_id = start.join().unwrap().unwrap();
            let attached = test.store.lock().unwrap().job(&job_id).unwrap();
            assert_eq!(attached.status, "running");
            assert_eq!(attached.task_id.as_deref(), Some("remote-task"));
        });
    }

    #[test]
    fn legacy_submit_reserves_an_unpollable_job_before_remote_completion() {
        let test = legacy_manager_harness();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        test.manager.set_submit_hook({
            let entered = entered.clone();
            let release = release.clone();
            move || {
                entered.wait();
                release.wait();
                Ok("remote-task".into())
            }
        });

        std::thread::scope(|scope| {
            let start = scope.spawn(|| test.manager.start(&test.pet_id, "p", &test.png, "h"));
            entered.wait();
            let jobs = test.store.lock().unwrap().job_list(&test.pet_id).unwrap();
            let polled = test.manager.poll_all().unwrap();
            release.wait();
            let start_result = start.join().unwrap();
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].status, "submitting");
            assert!(polled.is_empty());
            let job_id = start_result.unwrap();
            let attached = test.store.lock().unwrap().job(&job_id).unwrap();
            assert_eq!(attached.status, "running");
            assert_eq!(attached.task_id.as_deref(), Some("remote-task"));
        });
    }

    #[test]
    fn resume_converges_stale_session_and_legacy_submissions_to_failed() {
        let session = manager_harness_with_job(false);
        session
            .store
            .lock()
            .unwrap()
            .create_job_for_session("stale-session", &session.session_id, "p", "h", None)
            .unwrap();
        let legacy = legacy_manager_harness();
        legacy
            .store
            .lock()
            .unwrap()
            .create_job("stale-legacy", &legacy.pet_id, "p", "h", None)
            .unwrap();

        assert_eq!(session.manager.resume().unwrap(), 1);
        assert_eq!(legacy.manager.resume().unwrap(), 1);
        assert_eq!(
            session
                .store
                .lock()
                .unwrap()
                .job("stale-session")
                .unwrap()
                .status,
            "failed"
        );
        assert_eq!(
            legacy
                .store
                .lock()
                .unwrap()
                .job("stale-legacy")
                .unwrap()
                .status,
            "failed"
        );
        let session_state: (String, String, String) = session
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT status, last_stable_status, current_step
                 FROM creation_sessions WHERE session_id=?1",
                [&session.session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            session_state,
            ("retryableFailure".into(), "draft".into(), "upload".into())
        );
    }

    #[test]
    fn upload_start_holds_deletion_until_remote_task_is_attached() {
        let test = manager_harness_with_job(false);
        let deletion = deletion_service(&test);
        let submit_entered = Arc::new(std::sync::Barrier::new(2));
        let submit_release = Arc::new(std::sync::Barrier::new(2));
        test.manager.set_submit_hook({
            let entered = submit_entered.clone();
            let release = submit_release.clone();
            move || {
                entered.wait();
                release.wait();
                Ok("remote-task".into())
            }
        });
        let (attached_tx, attached_rx) = std::sync::mpsc::channel();
        let (attach_release_tx, attach_release_rx) = std::sync::mpsc::channel();
        let attach_release_rx = Arc::new(Mutex::new(attach_release_rx));
        test.manager.set_after_task_attach_hook(move || {
            attached_tx.send(()).unwrap();
            attach_release_rx.lock().unwrap().recv().unwrap();
        });

        std::thread::scope(|scope| {
            let start = scope.spawn(|| {
                test.manager
                    .start_for_session(&test.session_id, "p", &test.png, "h")
            });
            submit_entered.wait();
            let abandon = scope.spawn(|| deletion.abandon_creation(&test.session_id));
            submit_release.wait();
            let attached = attached_rx.recv_timeout(std::time::Duration::from_secs(1));
            if attached.is_ok() {
                let job = test
                    .store
                    .lock()
                    .unwrap()
                    .upload_jobs(&test.session_id)
                    .unwrap()
                    .remove(0);
                assert_eq!(job.status, "running");
                assert_eq!(job.task_id.as_deref(), Some("remote-task"));
            }
            let _ = attach_release_tx.send(());
            assert!(attached.is_ok(), "attach did not win before abandon");
            assert!(start.join().unwrap().is_ok());
            assert!(abandon.join().unwrap().is_ok());
        });
        assert!(test
            .store
            .lock()
            .unwrap()
            .upload_jobs(&test.session_id)
            .unwrap()
            .is_empty());
        let tombstoned: i64 = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM creation_session_tombstones WHERE session_id=?1",
                [&test.session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tombstoned, 1);
    }

    #[test]
    fn legacy_start_holds_deletion_until_remote_task_is_attached() {
        let test = legacy_manager_harness();
        let deletion = legacy_deletion_service(&test);
        let submit_entered = Arc::new(std::sync::Barrier::new(2));
        let submit_release = Arc::new(std::sync::Barrier::new(2));
        test.manager.set_submit_hook({
            let entered = submit_entered.clone();
            let release = submit_release.clone();
            move || {
                entered.wait();
                release.wait();
                Ok("remote-task".into())
            }
        });
        let (attached_tx, attached_rx) = std::sync::mpsc::channel();
        let (attach_release_tx, attach_release_rx) = std::sync::mpsc::channel();
        let attach_release_rx = Arc::new(Mutex::new(attach_release_rx));
        test.manager.set_after_task_attach_hook(move || {
            attached_tx.send(()).unwrap();
            attach_release_rx.lock().unwrap().recv().unwrap();
        });

        std::thread::scope(|scope| {
            let start = scope.spawn(|| test.manager.start(&test.pet_id, "p", &test.png, "h"));
            submit_entered.wait();
            let delete = scope.spawn(|| deletion.delete(&test.pet_id));
            submit_release.wait();
            let attached = attached_rx.recv_timeout(std::time::Duration::from_secs(1));
            if attached.is_ok() {
                let job = test
                    .store
                    .lock()
                    .unwrap()
                    .job_list(&test.pet_id)
                    .unwrap()
                    .remove(0);
                assert_eq!(job.status, "running");
                assert_eq!(job.task_id.as_deref(), Some("remote-task"));
            }
            let _ = attach_release_tx.send(());
            assert!(attached.is_ok(), "attach did not win before delete");
            assert!(start.join().unwrap().is_ok());
            assert!(delete.join().unwrap().is_ok());
        });
        assert!(test
            .store
            .lock()
            .unwrap()
            .job_list(&test.pet_id)
            .unwrap()
            .is_empty());
        let pet_count: i64 = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM pets WHERE pet_id=?1",
                [&test.pet_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pet_count, 0);
    }

    #[test]
    fn attach_failure_preserves_the_remote_task_without_reporting_finished() {
        let test = manager_harness_with_job(false);
        test.manager.set_submit_hook(|| Ok("remote-task".into()));
        test.manager.fail_next_task_attach();

        assert!(test
            .manager
            .start_for_session(&test.session_id, "p", &test.png, "h")
            .is_err());

        let job = test
            .store
            .lock()
            .unwrap()
            .upload_jobs(&test.session_id)
            .unwrap()
            .remove(0);
        assert_eq!(job.status, "submitting");
        assert_eq!(job.task_id.as_deref(), Some("remote-task"));
        assert!(test.manager.poll_all().unwrap().is_empty());
    }

    #[test]
    fn cancelled_legacy_job_is_not_overwritten_by_late_failure() {
        let test = legacy_manager_harness();
        test.store
            .lock()
            .unwrap()
            .create_job("job-legacy", &test.pet_id, "p", "h", Some("task"))
            .unwrap();
        let observed = test.store.lock().unwrap().job("job-legacy").unwrap();
        test.manager.cancel("job-legacy").unwrap();
        let completion = test
            .manager
            .settle_failure(&observed, "late remote failure");
        let mut finished = Vec::new();

        test.manager
            .record_poll_completion("job-legacy", completion, &mut finished)
            .unwrap();

        assert_eq!(finished, vec!["job-legacy"]);
        let job = test.store.lock().unwrap().job("job-legacy").unwrap();
        assert_eq!(job.status, "cancelled");
        assert!(job.error.is_none());
    }

    #[test]
    fn stage_promote_rename_failure_restores_the_existing_final_directory() {
        let test = manager_harness();
        install_existing_final(&test);
        test.manager.fail_next_stage_promote_rename();

        assert!(test
            .manager
            .complete_download("job-1", "https://example.invalid/out.png", &test.png)
            .is_err());

        assert_existing_final_restored(&test);
        assert_no_job_siblings(&test);
    }

    #[test]
    fn candidate_transaction_failure_restores_the_existing_final_directory() {
        let test = manager_harness();
        install_existing_final(&test);
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO appearance_variants
                 (variant_id, pet_id, job_id, session_id, image_path, cutout_path,
                  motion_profile_path, quality, accepted, created_at)
                 VALUES ('candidate-existing', ?1, 'job-1', ?2, 'existing.png',
                         'existing-cutout.png', 'existing-profile.json', 'acceptable', 0, ?3)",
                rusqlite::params![test.pet_id, test.session_id, now_iso()],
            )
            .unwrap();

        assert!(test
            .manager
            .complete_download("job-1", "https://example.invalid/out.png", &test.png)
            .is_err());

        assert_existing_final_restored(&test);
        assert_no_job_siblings(&test);
    }

    #[test]
    fn double_settlement_failure_restores_the_existing_final_directory() {
        let test = manager_harness();
        install_existing_final(&test);
        test.manager.set_before_candidate_commit_hook({
            let storage = test.storage.clone();
            move || {
                storage
                    .lock()
                    .unwrap()
                    .db
                    .execute("DELETE FROM generation_jobs WHERE job_id='job-1'", [])
                    .unwrap();
            }
        });

        assert!(test
            .manager
            .complete_download("job-1", "https://example.invalid/out.png", &test.png)
            .is_err());

        assert_existing_final_restored(&test);
        assert_no_job_siblings(&test);
    }

    #[test]
    fn successful_candidate_commit_replaces_final_and_removes_the_backup() {
        let test = manager_harness();
        install_existing_final(&test);

        test.manager
            .complete_download("job-1", "https://example.invalid/out.png", &test.png)
            .unwrap();

        let final_dir = test.root.join("jobs/job-1");
        assert_eq!(std::fs::read(final_dir.join("raw.png")).unwrap(), test.png);
        assert!(!final_dir.join("sentinel").exists());
        assert_no_job_siblings(&test);
    }

    #[test]
    fn existing_final_directory_link_is_never_moved_or_deleted() {
        let test = manager_harness();
        let outside = test.root.join("outside-existing-final");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("raw.png"), b"outside raw").unwrap();
        std::fs::write(outside.join("cutout.png"), b"outside cutout").unwrap();
        std::fs::write(outside.join("motion-profile.json"), b"outside profile").unwrap();
        std::fs::write(outside.join("sentinel"), b"outside sentinel").unwrap();
        let jobs_root = test.root.join("jobs");
        std::fs::create_dir_all(&jobs_root).unwrap();
        crate::platform::create_directory_link(&outside, &jobs_root.join("job-1"));

        assert!(test
            .manager
            .complete_download("job-1", "https://example.invalid/out.png", &test.png)
            .is_err());

        let link_metadata = std::fs::symlink_metadata(jobs_root.join("job-1")).unwrap();
        assert!(crate::platform::is_link_or_reparse_point(&link_metadata));
        assert_eq!(
            std::fs::read(outside.join("sentinel")).unwrap(),
            b"outside sentinel"
        );
        assert_eq!(
            std::fs::read(outside.join("raw.png")).unwrap(),
            b"outside raw"
        );
    }

    #[test]
    fn backup_cleanup_failure_preserves_durable_candidate_and_identifiable_backup() {
        let test = manager_harness();
        install_existing_final(&test);
        test.manager.fail_next_backup_cleanup();

        test.manager
            .complete_download("job-1", "https://example.invalid/out.png", &test.png)
            .unwrap();

        let job = test.store.lock().unwrap().job("job-1").unwrap();
        assert_eq!(job.status, "success");
        assert!(test
            .store
            .lock()
            .unwrap()
            .candidate_for_session(&test.session_id)
            .is_ok());
        let final_dir = test.root.join("jobs/job-1");
        assert_eq!(std::fs::read(final_dir.join("raw.png")).unwrap(), test.png);
        let backups: Vec<_> = std::fs::read_dir(test.root.join("jobs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".job-1-backup-candidate-backup-"))
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read(backups[0].join("sentinel")).unwrap(),
            b"keep old final"
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
    fn legacy_start_rejects_a_reserved_pet_before_api_configuration() {
        let test = manager_harness_with_job(false);

        let error = test
            .manager
            .start(&test.pet_id, "p", &test.png, "h")
            .unwrap_err();

        assert!(error.contains("creation_upload_start"));
        assert!(test
            .store
            .lock()
            .unwrap()
            .upload_jobs(&test.session_id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn cancelling_a_session_job_atomically_restores_upload_draft() {
        let test = manager_harness();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions SET error='old failure' WHERE session_id=?1",
                [&test.session_id],
            )
            .unwrap();

        test.manager.cancel("job-1").unwrap();

        let job = test.store.lock().unwrap().job("job-1").unwrap();
        assert_eq!(job.status, "cancelled");
        let session: (String, String, String, Option<String>) = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT status, last_stable_status, current_step, error
                 FROM creation_sessions WHERE session_id=?1",
                [&test.session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            session,
            ("draft".into(), "draft".into(), "upload".into(), None)
        );
    }

    #[test]
    fn cancelling_a_legacy_job_keeps_the_legacy_status_update() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-legacy-cancel-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let pet = PetRepository::new(storage.clone())
            .create(Species::Cat, IdentityMode::RealPet)
            .unwrap();
        let store = Arc::new(Mutex::new(CreationStore::new(storage.clone())));
        store
            .lock()
            .unwrap()
            .create_job("job-legacy", &pet.pet_id, "p", "h", Some("task"))
            .unwrap();
        let state = Arc::new(Mutex::new(StateStore::new(storage)));
        let manager = GenerationManager::new(
            store.clone(),
            state,
            Arc::from(root.join("jobs").as_path()),
            Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60))),
        );

        manager.cancel("job-legacy").unwrap();

        assert_eq!(
            store.lock().unwrap().job("job-legacy").unwrap().status,
            "cancelled"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn result_persistence_rejects_path_like_job_ids() {
        let test = manager_harness();
        assert!(test.manager.stage_result("../escape", &test.png).is_err());
        assert!(!test.root.join("escape").exists());
    }

    #[test]
    fn invalid_png_cleans_staging_and_partial_final_outputs() {
        let test = manager_harness();

        assert!(test
            .manager
            .complete_download("job-1", "https://example.invalid/out.png", b"not a png",)
            .is_err());

        assert!(!test.root.join("jobs/job-1").exists());
        assert!(std::fs::read_dir(test.root.join("jobs"))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn abandon_during_the_staging_window_cannot_recreate_job_outputs() {
        let test = manager_harness();
        let deletion = deletion_service(&test);
        let staged = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        test.manager.set_after_stage_hook({
            let staged = staged.clone();
            let resume = resume.clone();
            move || {
                staged.wait();
                resume.wait();
            }
        });

        std::thread::scope(|scope| {
            let completion = scope.spawn(|| {
                test.manager.complete_download(
                    "job-1",
                    "https://example.invalid/out.png",
                    &test.png,
                )
            });
            staged.wait();
            deletion.abandon_creation(&test.session_id).unwrap();
            resume.wait();
            assert!(completion.join().unwrap().is_err());
        });

        assert!(!test.root.join("jobs/job-1").exists());
        assert!(std::fs::read_dir(test.root.join("jobs"))
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true));
        let tombstoned: i64 = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM creation_session_tombstones WHERE session_id=?1",
                [&test.session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tombstoned, 1);
    }

    #[test]
    fn unconfirmed_candidate_and_failure_transactions_are_not_reported_finished() {
        let test = manager_harness();
        test.manager.set_before_candidate_commit_hook({
            let storage = test.storage.clone();
            move || {
                storage
                    .lock()
                    .unwrap()
                    .db
                    .execute("DELETE FROM generation_jobs WHERE job_id='job-1'", [])
                    .unwrap();
            }
        });
        let completion =
            test.manager
                .complete_download("job-1", "https://example.invalid/out.png", &test.png);
        let mut finished = Vec::new();

        assert!(test
            .manager
            .record_poll_completion("job-1", completion, &mut finished)
            .is_err());
        assert!(finished.is_empty());
        assert!(!test.root.join("jobs/job-1").exists());
    }
}
