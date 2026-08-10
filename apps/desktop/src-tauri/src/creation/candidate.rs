#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StandardCandidate {
    pub candidate_id: String,
    pub session_id: String,
    pub pet_id: String,
    pub job_id: Option<String>,
    pub body_path: String,
    pub motion_profile_path: String,
    pub quality: String,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use crate::creation::domain::new_entity_id;
    use crate::creation::CreationStore;
    use crate::pets::pet::{IdentityMode, Species};
    use crate::pets::repository::PetRepository;
    use crate::storage::Storage;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct CandidateHarness {
        root: PathBuf,
        storage: Arc<Mutex<Storage>>,
        store: CreationStore,
        session_id: String,
        pet_id: String,
    }

    impl Drop for CandidateHarness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl CandidateHarness {
        fn session(method: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let root = std::env::temp_dir().join(format!(
                "desktop-pet-standard-candidate-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
            let (species, identity) = match method {
                "composer" => (Species::Dog, IdentityMode::Guided),
                _ => (Species::Cat, IdentityMode::RealPet),
            };
            let pet = PetRepository::new(storage.clone())
                .create(species, identity)
                .unwrap();
            let session_id = new_entity_id("session");
            let now = crate::creation::profiles::now_iso();
            storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO creation_sessions
                     (session_id, pet_id, method, status, last_stable_status, current_step,
                      schema_version, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'draft', 'draft', ?3, 1, ?4, ?4)",
                    rusqlite::params![session_id, pet.pet_id, method, now],
                )
                .unwrap();
            Self {
                root,
                store: CreationStore::new(storage.clone()),
                storage,
                session_id,
                pet_id: pet.pet_id,
            }
        }

        fn upload() -> Self {
            Self::session("upload")
        }

        fn composer() -> Self {
            Self::session("composer")
        }

        fn candidate_files(&self, job_id: &str) -> (String, String, String) {
            let dir = self.root.join("jobs").join(job_id);
            std::fs::create_dir_all(&dir).unwrap();
            let raw = dir.join("raw.png");
            let cutout = dir.join("cutout.png");
            let profile = dir.join("motion-profile.json");
            std::fs::write(&raw, b"raw").unwrap();
            std::fs::write(&cutout, b"cutout").unwrap();
            std::fs::write(&profile, b"{}").unwrap();
            (
                raw.to_string_lossy().into_owned(),
                cutout.to_string_lossy().into_owned(),
                profile.to_string_lossy().into_owned(),
            )
        }

        fn session_state(&self) -> (String, String, String, Option<String>) {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT status, last_stable_status, current_step, error
                     FROM creation_sessions WHERE session_id=?1",
                    [&self.session_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap()
        }
    }

    #[test]
    fn upload_job_and_candidate_are_owned_by_the_session() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "prompt", "sha", Some("task-1"))
            .unwrap();
        let (raw, cutout, profile) = test.candidate_files("job-1");
        test.store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &test.root.join("jobs"),
                &raw,
                &cutout,
                &profile,
                "acceptable",
            )
            .unwrap();

        let candidate = test.store.candidate_for_session(&test.session_id).unwrap();
        assert_eq!(candidate.session_id, test.session_id);
        assert_eq!(candidate.pet_id, test.pet_id);
        assert_eq!(candidate.job_id.as_deref(), Some("job-1"));
        assert_eq!(
            PathBuf::from(candidate.motion_profile_path),
            PathBuf::from(profile).canonicalize().unwrap()
        );
        assert!(candidate.candidate_id.starts_with("candidate-"));
        assert_eq!(
            test.session_state(),
            (
                "candidateReady".into(),
                "candidateReady".into(),
                "review".into(),
                None
            )
        );
    }

    #[test]
    fn submitting_job_cannot_record_a_candidate_before_task_attachment() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "prompt", "sha", None)
            .unwrap();
        let (raw, cutout, profile) = test.candidate_files("job-1");

        assert!(test
            .store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &test.root.join("jobs"),
                &raw,
                &cutout,
                &profile,
                "acceptable",
            )
            .is_err());
        assert_eq!(test.store.job("job-1").unwrap().status, "submitting");
        assert!(test.store.candidate_for_session(&test.session_id).is_err());
    }

    #[test]
    fn composer_session_cannot_start_an_upload_job() {
        let test = CandidateHarness::composer();
        assert!(test
            .store
            .create_job_for_session("job-1", &test.session_id, "prompt", "sha", None)
            .unwrap_err()
            .contains("upload session"));
    }

    #[test]
    fn candidate_rejects_a_job_owned_by_another_session() {
        let first = CandidateHarness::upload();
        first
            .store
            .create_job_for_session("job-1", &first.session_id, "p", "h", None)
            .unwrap();
        {
            let db = &first.storage.lock().unwrap().db;
            db.execute(
                "UPDATE creation_sessions SET status='completed', last_stable_status='completed'
                 WHERE session_id=?1",
                [&first.session_id],
            )
            .unwrap();
        }
        let second_session = new_entity_id("session");
        let second_pet = PetRepository::new(first.storage.clone())
            .create(Species::Dog, IdentityMode::RealPet)
            .unwrap();
        let now = crate::creation::profiles::now_iso();
        first
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO creation_sessions
                 (session_id, pet_id, method, status, last_stable_status, current_step,
                  schema_version, created_at, updated_at)
                 VALUES (?1, ?2, 'upload', 'draft', 'draft', 'upload', 1, ?3, ?3)",
                rusqlite::params![second_session, second_pet.pet_id, now],
            )
            .unwrap();
        let (raw, cutout, profile) = first.candidate_files("job-1");

        assert!(first
            .store
            .record_upload_candidate(
                "job-1",
                &second_session,
                &first.root.join("jobs"),
                &raw,
                &cutout,
                &profile,
                "acceptable",
            )
            .unwrap_err()
            .contains("owned"));
        assert!(first.store.candidate_for_session(&second_session).is_err());
    }

    #[test]
    fn a_second_current_candidate_requires_explicit_replacement() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", Some("task-1"))
            .unwrap();
        let (raw, cutout, profile) = test.candidate_files("job-1");
        test.store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &test.root.join("jobs"),
                &raw,
                &cutout,
                &profile,
                "acceptable",
            )
            .unwrap();
        {
            let db = &test.storage.lock().unwrap().db;
            db.execute(
                "UPDATE creation_sessions
                 SET status='draft', last_stable_status='draft', current_step='generating'",
                [],
            )
            .unwrap();
            db.execute(
                "INSERT INTO generation_jobs
                 (job_id, pet_id, session_id, prompt, ref_sha256, status, created_at)
                 VALUES ('job-2', ?1, ?2, 'p', 'h', 'pending', ?3)",
                rusqlite::params![
                    test.pet_id,
                    test.session_id,
                    crate::creation::profiles::now_iso()
                ],
            )
            .unwrap();
        }
        let (raw2, cutout2, profile2) = test.candidate_files("job-2");

        assert!(test
            .store
            .record_upload_candidate(
                "job-2",
                &test.session_id,
                &test.root.join("jobs"),
                &raw2,
                &cutout2,
                &profile2,
                "acceptable",
            )
            .unwrap_err()
            .contains("current candidate"));
        assert_eq!(
            test.store
                .candidate_for_session(&test.session_id)
                .unwrap()
                .job_id
                .as_deref(),
            Some("job-1")
        );
    }

    #[test]
    fn candidate_paths_must_be_standard_files_under_the_matching_job() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();
        let (raw, cutout, profile) = test.candidate_files("other-job");

        assert!(test
            .store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &test.root.join("jobs"),
                &raw,
                &cutout,
                &profile,
                "acceptable",
            )
            .unwrap_err()
            .contains("job directory"));
    }

    #[test]
    fn candidate_rejects_an_external_directory_with_the_same_job_name() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();
        let external = test.root.join("external").join("job-1");
        std::fs::create_dir_all(&external).unwrap();
        let raw = external.join("raw.png");
        let cutout = external.join("cutout.png");
        let profile = external.join("motion-profile.json");
        std::fs::write(&raw, b"raw").unwrap();
        std::fs::write(&cutout, b"cutout").unwrap();
        std::fs::write(&profile, b"{}").unwrap();

        assert!(test
            .store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &test.root.join("jobs"),
                &raw.to_string_lossy(),
                &cutout.to_string_lossy(),
                &profile.to_string_lossy(),
                "acceptable",
            )
            .unwrap_err()
            .contains("configured jobs root"));
    }

    #[test]
    fn candidate_rejects_a_job_directory_link_escape() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();
        let outside = test.root.join("outside-job-1");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("raw.png"), b"raw").unwrap();
        std::fs::write(outside.join("cutout.png"), b"cutout").unwrap();
        std::fs::write(outside.join("motion-profile.json"), b"{}").unwrap();
        let jobs_root = test.root.join("jobs");
        std::fs::create_dir_all(&jobs_root).unwrap();
        crate::platform::create_directory_link(&outside, &jobs_root.join("job-1"));

        assert!(test
            .store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &jobs_root,
                &jobs_root.join("job-1/raw.png").to_string_lossy(),
                &jobs_root.join("job-1/cutout.png").to_string_lossy(),
                &jobs_root
                    .join("job-1/motion-profile.json")
                    .to_string_lossy(),
                "acceptable",
            )
            .unwrap_err()
            .contains("link or reparse point"));
    }

    #[test]
    fn failed_job_converges_the_session_without_a_candidate() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();
        test.store
            .fail_upload_job("job-1", &test.session_id, &test.pet_id, "profile failed")
            .unwrap();

        assert_eq!(
            test.session_state(),
            (
                "retryableFailure".into(),
                "draft".into(),
                "upload".into(),
                Some("profile failed".into())
            )
        );
        assert!(test.store.candidate_for_session(&test.session_id).is_err());
        let jobs = test.store.upload_jobs(&test.session_id).unwrap();
        assert_eq!(jobs[0].status, "failed");
    }

    #[test]
    fn retry_reuses_the_session_and_preserves_job_history() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();
        test.store
            .fail_upload_job("job-1", &test.session_id, &test.pet_id, "first failed")
            .unwrap();
        test.store
            .create_job_for_session("job-2", &test.session_id, "p2", "h2", None)
            .unwrap();

        let jobs = test.store.upload_jobs(&test.session_id).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(
            jobs[0].session_id.as_deref(),
            Some(test.session_id.as_str())
        );
        assert_eq!(jobs[0].status, "failed");
        assert_eq!(jobs[1].status, "submitting");
        assert_eq!(
            test.session_state(),
            ("draft".into(), "draft".into(), "generating".into(), None)
        );
    }

    #[test]
    fn upload_session_rejects_a_second_active_job() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();

        assert!(test
            .store
            .create_job_for_session("job-2", &test.session_id, "p2", "h2", None)
            .unwrap_err()
            .contains("active upload job"));
        assert_eq!(test.store.upload_jobs(&test.session_id).unwrap().len(), 1);
    }
}
