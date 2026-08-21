use crate::creation::profiles;
use crate::creation::StandardCandidate;
use crate::generation::cutout::CandidateQualityReportV1;
use crate::storage::Storage;
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub type SharedCreationStore = Arc<Mutex<CreationStore>>;

pub struct CreationStore {
    storage: Arc<Mutex<Storage>>,
}

impl CreationStore {
    pub fn new(storage: Arc<Mutex<Storage>>) -> Self {
        Self { storage }
    }

    pub fn save_upload_source(
        &self,
        session_id: &str,
        normalized_png: &[u8],
        sha256: &str,
        mime_type: &str,
    ) -> Result<(), String> {
        if normalized_png.is_empty()
            || normalized_png.len() > crate::generation::tasks::MAX_NORMALIZED_PNG_BYTES
        {
            return Err("normalized upload source must be between 1 byte and 10 MiB".into());
        }
        if sha256_hex(normalized_png) != sha256 {
            return Err("upload source hash does not match its normalized bytes".into());
        }
        if mime_type != "image/png"
            || image::guess_format(normalized_png).ok() != Some(image::ImageFormat::Png)
        {
            return Err("upload source mime must match normalized PNG bytes".into());
        }
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        require_writable_upload_session(&tx, session_id)?;
        let existing = load_upload_source_record(&tx, session_id)?;
        if let Some(existing) = existing {
            if existing.normalized_png == normalized_png
                && existing.sha256 == sha256
                && existing.mime_type == mime_type
                && existing.byte_size == normalized_png.len() as i64
            {
                tx.commit().map_err(|error| error.to_string())?;
                return Ok(());
            }
            return Err("upload session already owns a different source image".into());
        }
        tx.execute(
            "INSERT INTO creation_upload_sources
             (session_id, normalized_png, sha256, mime_type, byte_size, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                session_id,
                normalized_png,
                sha256,
                mime_type,
                normalized_png.len() as i64,
                profiles::now_iso()
            ],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn upload_source(&self, session_id: &str) -> Result<Option<UploadSourceRecord>, String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        require_writable_upload_session(&tx, session_id)?;
        let record = load_upload_source_record(&tx, session_id)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(record)
    }

    pub fn create_job(
        &self,
        job_id: &str,
        pet_id: &str,
        prompt: &str,
        ref_sha256: &str,
        task_id: Option<&str>,
    ) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        ensure_legacy_generation_pet(&tx, pet_id)?;
        tx.execute(
            "INSERT INTO generation_jobs
             (job_id, pet_id, prompt, ref_sha256, task_id, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5,
                     CASE WHEN ?5 IS NULL THEN 'submitting' ELSE 'running' END, ?6)",
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
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn upload_session_pet(&self, session_id: &str) -> Result<String, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let session: Option<(String, String, String)> = db
            .query_row(
                "SELECT pet_id, method, status FROM creation_sessions WHERE session_id=?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (pet_id, method, status) = session
            .ok_or_else(|| format!("creation session not found or abandoned: {session_id}"))?;
        if method != "upload" {
            return Err("generation requires an upload session".into());
        }
        if matches!(status.as_str(), "completed" | "abandoned") {
            return Err("cannot generate for a terminal upload session".into());
        }
        Ok(pet_id)
    }

    pub fn create_job_for_session(
        &self,
        job_id: &str,
        session_id: &str,
        prompt: &str,
        ref_sha256: &str,
        task_id: Option<&str>,
    ) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let session: Option<(String, String, String)> = tx
            .query_row(
                "SELECT pet_id, method, status FROM creation_sessions WHERE session_id=?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (pet_id, method, status) = session
            .ok_or_else(|| format!("creation session not found or abandoned: {session_id}"))?;
        if method != "upload" {
            return Err("generation requires an upload session".into());
        }
        if matches!(status.as_str(), "completed" | "abandoned") {
            return Err("cannot generate for a terminal upload session".into());
        }
        let current_candidate: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM appearance_variants WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if current_candidate != 0 {
            return Err("upload session already has a current candidate".into());
        }
        let active_jobs: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM generation_jobs
                 WHERE session_id=?1 AND status IN ('submitting','pending','running')",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if active_jobs != 0 {
            return Err("upload session already has an active upload job".into());
        }
        let now = profiles::now_iso();
        tx.execute(
            "INSERT INTO generation_jobs
             (job_id, pet_id, session_id, prompt, ref_sha256, task_id, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                     CASE WHEN ?6 IS NULL THEN 'submitting' ELSE 'running' END, ?7)",
            rusqlite::params![job_id, pet_id, session_id, prompt, ref_sha256, task_id, now],
        )
        .map_err(|error| error.to_string())?;
        let affected = tx
            .execute(
                "UPDATE creation_sessions
                 SET status='draft', last_stable_status='draft', current_step='generating',
                     error=NULL, updated_at=?2
                 WHERE session_id=?1 AND pet_id=?3 AND method='upload'
                   AND status NOT IN ('completed','abandoned')",
                rusqlite::params![session_id, now, pet_id],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("upload session is not bound to its reserved pet".into());
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn attach_task_to_upload_job(
        &self,
        job_id: &str,
        session_id: &str,
        pet_id: &str,
        task_id: &str,
    ) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let affected = db
            .execute(
                "UPDATE generation_jobs SET task_id=?4, status='running'
                 WHERE job_id=?1 AND session_id=?2 AND pet_id=?3 AND status='submitting'",
                rusqlite::params![job_id, session_id, pet_id, task_id],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("upload job ownership changed before task attachment".into());
        }
        Ok(())
    }

    pub fn attach_task_to_legacy_job(
        &self,
        job_id: &str,
        pet_id: &str,
        task_id: &str,
    ) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let affected = db
            .execute(
                "UPDATE generation_jobs SET task_id=?3, status='running'
                 WHERE job_id=?1 AND pet_id=?2 AND session_id IS NULL
                   AND status='submitting'",
                rusqlite::params![job_id, pet_id, task_id],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("legacy job ownership changed before task attachment".into());
        }
        Ok(())
    }

    pub fn preserve_upload_task_after_attach_failure(
        &self,
        job_id: &str,
        session_id: &str,
        pet_id: &str,
        task_id: &str,
    ) -> Result<(), String> {
        self.preserve_task_after_attach_failure(job_id, Some(session_id), pet_id, task_id)
    }

    pub fn preserve_legacy_task_after_attach_failure(
        &self,
        job_id: &str,
        pet_id: &str,
        task_id: &str,
    ) -> Result<(), String> {
        self.preserve_task_after_attach_failure(job_id, None, pet_id, task_id)
    }

    fn preserve_task_after_attach_failure(
        &self,
        job_id: &str,
        session_id: Option<&str>,
        pet_id: &str,
        task_id: &str,
    ) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let affected = db
            .execute(
                "UPDATE generation_jobs SET task_id=?4
                 WHERE job_id=?1 AND pet_id=?2 AND session_id IS ?3
                   AND status='submitting' AND task_id IS NULL",
                rusqlite::params![job_id, pet_id, session_id, task_id],
            )
            .map_err(|error| error.to_string())?;
        if affected == 1 {
            return Ok(());
        }
        let durable: Option<(Option<String>, String, Option<String>)> = db
            .query_row(
                "SELECT session_id, status, task_id FROM generation_jobs
                 WHERE job_id=?1 AND pet_id=?2",
                rusqlite::params![job_id, pet_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        match durable {
            Some((actual_session, status, Some(actual_task)))
                if actual_session.as_deref() == session_id
                    && actual_task == task_id
                    && matches!(status.as_str(), "submitting" | "running") =>
            {
                Ok(())
            }
            _ => Err("remote task id could not be preserved after attachment failure".into()),
        }
    }

    pub fn record_upload_candidate(
        &self,
        job_id: &str,
        session_id: &str,
        jobs_root: &Path,
        raw_path: &str,
        cutout_path: &str,
        motion_profile_path: &str,
        quality: &str,
    ) -> Result<StandardCandidate, String> {
        self.record_upload_candidate_transaction(
            job_id,
            session_id,
            jobs_root,
            raw_path,
            cutout_path,
            Some(motion_profile_path),
            quality,
            None,
            None,
        )
    }

    pub fn record_upload_candidate_with_result_url(
        &self,
        job_id: &str,
        session_id: &str,
        jobs_root: &Path,
        raw_path: &str,
        cutout_path: &str,
        motion_profile_path: Option<&str>,
        quality_report: &CandidateQualityReportV1,
        result_url: &str,
    ) -> Result<StandardCandidate, String> {
        let quality = if quality_report.is_acceptable() {
            "acceptable"
        } else {
            "needs-review"
        };
        if quality_report.is_acceptable() != motion_profile_path.is_some() {
            return Err("motion profile presence must match candidate quality".into());
        }
        self.record_upload_candidate_transaction(
            job_id,
            session_id,
            jobs_root,
            raw_path,
            cutout_path,
            motion_profile_path,
            quality,
            Some(quality_report),
            Some(result_url),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_upload_candidate_transaction(
        &self,
        job_id: &str,
        session_id: &str,
        jobs_root: &Path,
        raw_path: &str,
        cutout_path: &str,
        motion_profile_path: Option<&str>,
        quality: &str,
        quality_report: Option<&CandidateQualityReportV1>,
        result_url: Option<&str>,
    ) -> Result<StandardCandidate, String> {
        let (raw_path, cutout_path, motion_profile_path) = validate_candidate_paths(
            jobs_root,
            job_id,
            raw_path,
            cutout_path,
            motion_profile_path,
        )?;
        let quality_report_json = quality_report
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| format!("serialize candidate quality report: {error}"))?;
        let candidate_id = crate::creation::domain::new_entity_id("candidate");
        let created_at = profiles::now_iso();
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let ownership: Option<(String, String, String, String)> = tx
            .query_row(
                "SELECT gj.pet_id, gj.session_id, cs.pet_id, cs.method
                 FROM generation_jobs gj
                 JOIN creation_sessions cs ON cs.session_id=gj.session_id
                 WHERE gj.job_id=?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (job_pet_id, job_session_id, session_pet_id, method) =
            ownership.ok_or_else(|| "upload job ownership was not found".to_string())?;
        if job_session_id != session_id || job_pet_id != session_pet_id {
            return Err("upload job is not owned by this session and pet".into());
        }
        if method != "upload" {
            return Err("candidate requires an upload session".into());
        }
        let current_candidate: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM appearance_variants WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if current_candidate != 0 {
            return Err("upload session already has a current candidate".into());
        }
        tx.execute(
            "INSERT INTO appearance_variants
             (variant_id, pet_id, job_id, session_id, image_path, cutout_path,
              motion_profile_path, quality, quality_report_json, accepted, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10)",
            rusqlite::params![
                candidate_id,
                job_pet_id,
                job_id,
                session_id,
                raw_path,
                cutout_path,
                motion_profile_path,
                quality,
                quality_report_json,
                created_at
            ],
        )
        .map_err(|error| error.to_string())?;
        let job_affected = tx
            .execute(
                "UPDATE generation_jobs SET status='success', result_url=?4, error=NULL
                 WHERE job_id=?1 AND session_id=?2 AND pet_id=?3
                   AND status IN ('pending','running')",
                rusqlite::params![job_id, session_id, job_pet_id, result_url],
            )
            .map_err(|error| error.to_string())?;
        if job_affected != 1 {
            return Err("upload job is not pending for this session".into());
        }
        let session_affected = tx
            .execute(
                "UPDATE creation_sessions
                 SET status='candidateReady', last_stable_status='candidateReady',
                     current_step='review', error=NULL, updated_at=?3
                 WHERE session_id=?1 AND pet_id=?2 AND method='upload'
                   AND status NOT IN ('completed','abandoned')",
                rusqlite::params![session_id, job_pet_id, created_at],
            )
            .map_err(|error| error.to_string())?;
        if session_affected != 1 {
            return Err("upload session is not eligible for a candidate".into());
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(StandardCandidate {
            candidate_id,
            session_id: session_id.into(),
            pet_id: job_pet_id,
            job_id: Some(job_id.into()),
            body_path: cutout_path,
            motion_profile_path,
            quality: quality.into(),
            quality_report: quality_report.cloned(),
            created_at,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn replace_upload_candidate_processing(
        &self,
        job_id: &str,
        session_id: &str,
        candidate_id: &str,
        jobs_root: &Path,
        raw_path: &str,
        cutout_path: &str,
        motion_profile_path: Option<&str>,
        quality_report: &CandidateQualityReportV1,
    ) -> Result<(), String> {
        let quality = if quality_report.is_acceptable() {
            "acceptable"
        } else {
            "needs-review"
        };
        if quality_report.is_acceptable() != motion_profile_path.is_some() {
            return Err("motion profile presence must match candidate quality".into());
        }
        let (raw_path, cutout_path, motion_profile_path) = validate_candidate_paths(
            jobs_root,
            job_id,
            raw_path,
            cutout_path,
            motion_profile_path,
        )?;
        let report_json = serde_json::to_string(quality_report)
            .map_err(|error| format!("serialize candidate quality report: {error}"))?;
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let current: Option<(String, i64, Option<String>, String, String)> = tx
            .query_row(
                "SELECT av.quality, av.accepted, av.quality_report_json,
                        gj.status, cs.status
                 FROM appearance_variants av
                 JOIN generation_jobs gj ON gj.job_id=av.job_id
                 JOIN creation_sessions cs ON cs.session_id=av.session_id
                 WHERE av.variant_id=?1 AND av.job_id=?2 AND av.session_id=?3
                   AND av.pet_id=gj.pet_id AND av.pet_id=cs.pet_id",
                rusqlite::params![candidate_id, job_id, session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (old_quality, accepted, old_report_json, job_status, session_status) = current
            .ok_or_else(|| {
                "upload candidate is no longer current for local reprocessing".to_string()
            })?;
        let old_report: CandidateQualityReportV1 = serde_json::from_str(
            old_report_json
                .as_deref()
                .ok_or("upload candidate has no prior quality report")?,
        )
        .map_err(|error| format!("upload candidate quality report is invalid: {error}"))?;
        if old_quality != "needs-review"
            || accepted != 0
            || job_status != "success"
            || session_status != "candidateReady"
            || old_report.transparent_ratio != 0.0
            || !old_report
                .reasons
                .contains(&crate::generation::cutout::QualityReason::NonUniformBackground)
        {
            return Err(
                "upload candidate is not eligible for local green-screen reprocessing".into(),
            );
        }
        let affected = tx
            .execute(
                "UPDATE appearance_variants
                 SET image_path=?4, cutout_path=?5, motion_profile_path=?6,
                     quality=?7, quality_report_json=?8
                 WHERE variant_id=?1 AND job_id=?2 AND session_id=?3
                   AND quality='needs-review' AND accepted=0",
                rusqlite::params![
                    candidate_id,
                    job_id,
                    session_id,
                    raw_path,
                    cutout_path,
                    motion_profile_path,
                    quality,
                    report_json
                ],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("upload candidate changed during local reprocessing".into());
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn record_local_candidate(
        &self,
        session_id: &str,
        body_path: &Path,
        motion_profile_path: &Path,
    ) -> Result<StandardCandidate, String> {
        let body_path = body_path
            .canonicalize()
            .map_err(|error| format!("local candidate body is unavailable: {error}"))?;
        let motion_profile_path = motion_profile_path
            .canonicalize()
            .map_err(|error| format!("local motion profile is unavailable: {error}"))?;
        let candidate_dir = body_path
            .parent()
            .ok_or("local candidate body has no candidate directory")?;
        if body_path.file_name().and_then(|name| name.to_str()) != Some("body.png")
            || motion_profile_path
                .file_name()
                .and_then(|name| name.to_str())
                != Some("motion-profile.json")
            || motion_profile_path.parent() != Some(candidate_dir)
        {
            return Err("local candidate uses non-standard files".into());
        }
        for (path, label) in [
            (&body_path, "local candidate body"),
            (&motion_profile_path, "local motion profile"),
        ] {
            let metadata = std::fs::symlink_metadata(path)
                .map_err(|error| format!("{label} is unavailable: {error}"))?;
            if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
                return Err(format!("{label} must be a regular file"));
            }
        }
        let body_path_text = body_path
            .to_str()
            .ok_or("local candidate body path is not valid Unicode")?
            .to_owned();
        let motion_profile_path_text = motion_profile_path
            .to_str()
            .ok_or("local motion profile path is not valid Unicode")?
            .to_owned();
        let candidate_id = crate::creation::domain::new_entity_id("candidate");
        let created_at = profiles::now_iso();
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let session: Option<(String, String, String)> = tx
            .query_row(
                "SELECT pet_id, method, status FROM creation_sessions WHERE session_id=?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (pet_id, method, status) =
            session.ok_or_else(|| format!("creation session not found: {session_id}"))?;
        if !matches!(method.as_str(), "composer" | "adoption") || status != "draft" {
            return Err("local candidate requires an editable local creation draft".into());
        }
        if method == "composer" {
            let has_recipe: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM composer_recipes WHERE session_id=?1)",
                    [session_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if !has_recipe {
                return Err("composer candidate requires a saved recipe".into());
            }
        }
        let current_candidate: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM appearance_variants WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if current_candidate != 0 {
            return Err("local creation session already has a current candidate".into());
        }
        tx.execute(
            "INSERT INTO appearance_variants
             (variant_id, pet_id, job_id, session_id, image_path, cutout_path,
              motion_profile_path, quality, accepted, created_at)
             VALUES (?1, ?2, NULL, ?3, ?4, ?4, ?5, 'acceptable', 0, ?6)",
            rusqlite::params![
                candidate_id,
                pet_id,
                session_id,
                &body_path_text,
                &motion_profile_path_text,
                created_at,
            ],
        )
        .map_err(|error| error.to_string())?;
        let affected = tx
            .execute(
                "UPDATE creation_sessions
                 SET status='candidateReady', last_stable_status='candidateReady',
                     current_step='review', error=NULL, updated_at=?3
                 WHERE session_id=?1 AND pet_id=?2 AND method=?4 AND status='draft'",
                rusqlite::params![session_id, pet_id, created_at, method],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("local creation session is no longer eligible for a candidate".into());
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(StandardCandidate {
            candidate_id,
            session_id: session_id.into(),
            pet_id,
            job_id: None,
            body_path: body_path_text,
            motion_profile_path: Some(motion_profile_path_text),
            quality: "acceptable".into(),
            quality_report: None,
            created_at,
        })
    }

    pub fn revert_missing_local_composer_candidate(
        &self,
        session_id: &str,
    ) -> Result<bool, String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let eligible: bool = tx
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM creation_sessions cs
                   WHERE cs.session_id=?1 AND cs.method='composer'
                     AND (cs.status='candidateReady' OR
                          (cs.status='retryableFailure' AND cs.last_stable_status='candidateReady'))
                     AND (SELECT COUNT(*) FROM appearance_variants av
                          WHERE av.session_id=cs.session_id AND av.pet_id=cs.pet_id
                            AND av.job_id IS NULL AND av.accepted=0)=1
                 )",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !eligible {
            return Ok(false);
        }
        let deleted = tx
            .execute(
                "DELETE FROM appearance_variants
                 WHERE session_id=?1 AND job_id IS NULL AND accepted=0",
                [session_id],
            )
            .map_err(|error| error.to_string())?;
        let updated = tx
            .execute(
                "UPDATE creation_sessions
                 SET status='draft', last_stable_status='draft', current_step='preview',
                     error=NULL, updated_at=?2
                 WHERE session_id=?1 AND method='composer'
                   AND (status='candidateReady' OR
                        (status='retryableFailure' AND last_stable_status='candidateReady'))",
                rusqlite::params![session_id, profiles::now_iso()],
            )
            .map_err(|error| error.to_string())?;
        if deleted != 1 || updated != 1 {
            return Err("missing local composer candidate changed during recovery".into());
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(true)
    }

    pub fn fail_upload_job(
        &self,
        job_id: &str,
        session_id: &str,
        pet_id: &str,
        error: &str,
    ) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let ownership: Option<(String, String, String, String)> = tx
            .query_row(
                "SELECT gj.pet_id, gj.session_id, cs.pet_id, cs.method
                 FROM generation_jobs gj
                 JOIN creation_sessions cs ON cs.session_id=gj.session_id
                 WHERE gj.job_id=?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|db_error| db_error.to_string())?;
        let (actual_pet_id, actual_session_id, session_pet_id, method) =
            ownership.ok_or_else(|| "upload job ownership was not found".to_string())?;
        if actual_session_id != session_id || actual_pet_id != pet_id || session_pet_id != pet_id {
            return Err("upload job is not owned by this session and pet".into());
        }
        if method != "upload" {
            return Err("job failure requires an upload session".into());
        }
        let job_affected = tx
            .execute(
                "UPDATE generation_jobs SET status='failed', result_url=NULL, error=?4
                 WHERE job_id=?1 AND session_id=?2 AND pet_id=?3
                   AND status IN ('submitting','pending','running')",
                rusqlite::params![job_id, session_id, pet_id, error],
            )
            .map_err(|db_error| db_error.to_string())?;
        if job_affected != 1 {
            return Err("upload job is not pending for failure recovery".into());
        }
        let session_affected = tx
            .execute(
                "UPDATE creation_sessions
                 SET status='retryableFailure', last_stable_status='draft',
                     current_step='upload', error=?3, updated_at=?4
                 WHERE session_id=?1 AND pet_id=?2 AND method='upload'
                   AND status NOT IN ('completed','abandoned')",
                rusqlite::params![session_id, pet_id, error, profiles::now_iso()],
            )
            .map_err(|db_error| db_error.to_string())?;
        if session_affected != 1 {
            return Err("upload session is not eligible for failure recovery".into());
        }
        tx.commit().map_err(|db_error| db_error.to_string())
    }

    pub fn fail_legacy_job_if_active(&self, job_id: &str, error: &str) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let affected = db
            .execute(
                "UPDATE generation_jobs SET status='failed', result_url=NULL, error=?2
                 WHERE job_id=?1 AND session_id IS NULL
                   AND status IN ('submitting','pending','running')",
                rusqlite::params![job_id, error],
            )
            .map_err(|db_error| db_error.to_string())?;
        if affected == 1 {
            return Ok(());
        }
        let current: Option<(Option<String>, String)> = db
            .query_row(
                "SELECT session_id, status FROM generation_jobs WHERE job_id=?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|db_error| db_error.to_string())?;
        match current {
            Some((None, status))
                if matches!(status.as_str(), "cancelled" | "success" | "failed") =>
            {
                Ok(())
            }
            Some((None, status)) => Err(format!(
                "legacy generation job is not durably terminal after failure: {status}"
            )),
            Some((Some(_), _)) => Err("generation job is not a legacy job".into()),
            None => Err(format!("generation job not found: {job_id}")),
        }
    }

    pub fn fail_stale_submitting_jobs(&self, error: &str) -> Result<usize, String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|db_error| db_error.to_string())?;
        let jobs: Vec<(String, String, Option<String>)> = {
            let mut statement = tx
                .prepare(
                    "SELECT job_id, pet_id, session_id FROM generation_jobs
                     WHERE status='submitting'",
                )
                .map_err(|db_error| db_error.to_string())?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(|db_error| db_error.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|db_error| db_error.to_string())?
        };
        for (job_id, pet_id, session_id) in &jobs {
            let job_affected = tx
                .execute(
                    "UPDATE generation_jobs SET status='failed', result_url=NULL, error=?2
                     WHERE job_id=?1 AND status='submitting'",
                    rusqlite::params![job_id, error],
                )
                .map_err(|db_error| db_error.to_string())?;
            if job_affected != 1 {
                return Err("stale submitting job changed during recovery".into());
            }
            if let Some(session_id) = session_id {
                let session_affected = tx
                    .execute(
                        "UPDATE creation_sessions
                         SET status='retryableFailure', last_stable_status='draft',
                             current_step='upload', error=?3, updated_at=?4
                         WHERE session_id=?1 AND pet_id=?2 AND method='upload'
                           AND status NOT IN ('completed','abandoned')",
                        rusqlite::params![session_id, pet_id, error, profiles::now_iso()],
                    )
                    .map_err(|db_error| db_error.to_string())?;
                if session_affected != 1 {
                    return Err("stale upload session is not eligible for recovery".into());
                }
            }
        }
        tx.commit().map_err(|db_error| db_error.to_string())?;
        Ok(jobs.len())
    }

    pub fn candidate_for_session(&self, session_id: &str) -> Result<StandardCandidate, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let (mut candidate, quality_report_json): (StandardCandidate, Option<String>) = db
            .query_row(
                "SELECT av.variant_id, av.session_id, av.pet_id, av.job_id, av.cutout_path,
                    av.motion_profile_path, av.quality, av.quality_report_json, av.created_at
             FROM appearance_variants av
             LEFT JOIN generation_jobs gj ON gj.job_id=av.job_id
             JOIN creation_sessions cs ON cs.session_id=av.session_id
             WHERE av.session_id=?1 AND av.pet_id=cs.pet_id
               AND (av.job_id IS NULL OR
                    (gj.session_id=av.session_id AND gj.pet_id=av.pet_id AND gj.status='success'))
               AND cs.status!='abandoned'
               AND av.cutout_path IS NOT NULL
               AND (av.motion_profile_path IS NULL OR av.motion_profile_path!='')
             ORDER BY av.created_at DESC, av.rowid DESC LIMIT 1",
                [session_id],
                |row| {
                    Ok((
                        StandardCandidate {
                            candidate_id: row.get(0)?,
                            session_id: row.get(1)?,
                            pet_id: row.get(2)?,
                            job_id: row.get(3)?,
                            body_path: row.get(4)?,
                            motion_profile_path: row.get(5)?,
                            quality: row.get(6)?,
                            quality_report: None,
                            created_at: row.get(8)?,
                        },
                        row.get(7)?,
                    ))
                },
            )
            .map_err(|error| format!("candidate is not available for creation session: {error}"))?;
        candidate.quality_report = quality_report_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| format!("candidate quality report is invalid: {error}"))?;
        Ok(candidate)
    }

    pub fn update_job_status(
        &self,
        job_id: &str,
        status: &str,
        result_url: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let affected = db
            .execute(
                "UPDATE generation_jobs SET status = ?2, result_url = ?3, error = ?4
             WHERE job_id = ?1",
                rusqlite::params![job_id, status, result_url, error],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err(format!("generation job not found: {job_id}"));
        }
        Ok(())
    }

    pub fn cancel_job(&self, job_id: &str) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let job: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT pet_id, session_id FROM generation_jobs WHERE job_id=?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (pet_id, session_id) =
            job.ok_or_else(|| format!("generation job not found: {job_id}"))?;
        if let Some(session_id) = session_id {
            let session_owned: bool = tx
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM creation_sessions
                       WHERE session_id=?1 AND pet_id=?2 AND method='upload'
                         AND status NOT IN ('completed','abandoned')
                     )",
                    rusqlite::params![session_id, pet_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if !session_owned {
                return Err("session generation job ownership is not live".into());
            }
            let job_affected = tx
                .execute(
                    "UPDATE generation_jobs
                     SET status='cancelled', result_url=NULL, error=NULL
                     WHERE job_id=?1 AND session_id=?2 AND pet_id=?3
                       AND status IN ('submitting','pending','running')",
                    rusqlite::params![job_id, session_id, pet_id],
                )
                .map_err(|error| error.to_string())?;
            if job_affected != 1 {
                return Err("session generation job is not active".into());
            }
            let session_affected = tx
                .execute(
                    "UPDATE creation_sessions
                     SET status='draft', last_stable_status='draft', current_step='upload',
                         error=NULL, updated_at=?3
                     WHERE session_id=?1 AND pet_id=?2 AND method='upload'
                       AND status NOT IN ('completed','abandoned')",
                    rusqlite::params![session_id, pet_id, profiles::now_iso()],
                )
                .map_err(|error| error.to_string())?;
            if session_affected != 1 {
                return Err("upload session could not be restored after cancellation".into());
            }
        } else {
            let affected = tx
                .execute(
                    "UPDATE generation_jobs SET status='cancelled', result_url=NULL, error=NULL
                     WHERE job_id=?1 AND session_id IS NULL
                       AND status IN ('submitting','pending','running')",
                    [job_id],
                )
                .map_err(|error| error.to_string())?;
            if affected != 1 {
                return Err("legacy generation job not found".into());
            }
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn job(&self, job_id: &str) -> Result<JobRecord, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.query_row(
            "SELECT job_id, pet_id, session_id, prompt, ref_sha256, task_id, status,
                    result_url, error, created_at
             FROM generation_jobs WHERE job_id=?1",
            [job_id],
            |row| {
                Ok(JobRecord {
                    job_id: row.get(0)?,
                    pet_id: row.get(1)?,
                    session_id: row.get(2)?,
                    prompt: row.get(3)?,
                    ref_sha256: row.get(4)?,
                    task_id: row.get(5)?,
                    status: row.get(6)?,
                    result_url: row.get(7)?,
                    error: row.get(8)?,
                    created_at: row.get(9)?,
                })
            },
        )
        .map_err(|error| format!("generation job not found: {error}"))
    }

    pub fn live_upload_job_for_completion(
        &self,
        job_id: &str,
        session_id: &str,
        pet_id: &str,
    ) -> Result<JobRecord, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.query_row(
            "SELECT gj.job_id, gj.pet_id, gj.session_id, gj.prompt, gj.ref_sha256,
                    gj.task_id, gj.status, gj.result_url, gj.error, gj.created_at
             FROM generation_jobs gj
             JOIN creation_sessions cs ON cs.session_id=gj.session_id
             WHERE gj.job_id=?1 AND gj.session_id=?2 AND gj.pet_id=?3
               AND cs.pet_id=?3 AND cs.method='upload'
               AND cs.status NOT IN ('completed','abandoned')
               AND gj.status IN ('pending','running')",
            rusqlite::params![job_id, session_id, pet_id],
            |row| {
                Ok(JobRecord {
                    job_id: row.get(0)?,
                    pet_id: row.get(1)?,
                    session_id: row.get(2)?,
                    prompt: row.get(3)?,
                    ref_sha256: row.get(4)?,
                    task_id: row.get(5)?,
                    status: row.get(6)?,
                    result_url: row.get(7)?,
                    error: row.get(8)?,
                    created_at: row.get(9)?,
                })
            },
        )
        .map_err(|error| format!("live upload job ownership is unavailable: {error}"))
    }

    pub fn job_status(&self, job_id: &str) -> Result<Option<String>, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.query_row(
            "SELECT status FROM generation_jobs WHERE job_id=?1",
            [job_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())
    }

    pub fn record_candidate(
        &self,
        job_id: &str,
        pet_id: &str,
        image_path: &str,
        cutout_path: &str,
        quality: &str,
    ) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let affected = db
            .execute(
                "INSERT INTO appearance_variants
             (variant_id, pet_id, job_id, image_path, cutout_path, quality, accepted, created_at)
             SELECT ?1, ?2, ?1, ?3, ?4, ?5, 0, ?6
             WHERE EXISTS (
                 SELECT 1 FROM generation_jobs WHERE job_id = ?1 AND pet_id = ?2
             )
             ON CONFLICT(variant_id) DO UPDATE SET image_path=excluded.image_path,
             cutout_path=excluded.cutout_path, quality=excluded.quality
             WHERE appearance_variants.pet_id = excluded.pet_id
             AND appearance_variants.job_id IS excluded.job_id",
                rusqlite::params![
                    job_id,
                    pet_id,
                    image_path,
                    cutout_path,
                    quality,
                    profiles::now_iso()
                ],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("candidate variant belongs to a different pet or job".into());
        }
        Ok(())
    }

    pub fn record_runtime_variant(
        &self,
        variant_id: &str,
        pet_id: &str,
        manifest_path: &str,
    ) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let affected = db
            .execute(
                "INSERT INTO variants (variant_id, pet_id, style_id, manifest_path, created_at)
             VALUES (?1, ?2, 'signature-cartoon-v1', ?3, ?4)
             ON CONFLICT(variant_id) DO UPDATE SET manifest_path=excluded.manifest_path
             WHERE variants.pet_id = excluded.pet_id",
                rusqlite::params![variant_id, pet_id, manifest_path, profiles::now_iso()],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("runtime variant belongs to a different pet".into());
        }
        Ok(())
    }

    #[allow(dead_code)] // exposed for creation workflows that share the storage connection
    pub fn set_compile_error(&self, pet_id: &str, error: &str) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![format!("creation:{pet_id}:compile_error"), error],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    #[allow(dead_code)] // exposed for creation workflows that share the storage connection
    pub fn clear_compile_error(&self, pet_id: &str) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.execute(
            "DELETE FROM state WHERE key = ?1",
            rusqlite::params![format!("creation:{pet_id}:compile_error")],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    #[allow(dead_code)] // consumed by the candidate-directory projection workflow
    pub fn candidates(&self, pet_id: &str) -> Result<Vec<AppearanceVariant>, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let mut statement = db
            .prepare(
                "SELECT variant_id, pet_id, job_id, image_path, cutout_path, quality, accepted, created_at
                 FROM appearance_variants WHERE pet_id = ?1 ORDER BY created_at",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(rusqlite::params![pet_id], |row| {
                Ok(AppearanceVariant {
                    variant_id: row.get(0)?,
                    pet_id: row.get(1)?,
                    job_id: row.get(2)?,
                    image_path: row.get(3)?,
                    cutout_path: row.get(4)?,
                    quality: row.get(5)?,
                    accepted: row.get::<_, i64>(6)? != 0,
                    created_at: row.get(7)?,
                })
            })
            .map_err(|error| error.to_string())?;
        let mut variants = Vec::new();
        for row in rows {
            variants.push(row.map_err(|error| error.to_string())?);
        }
        Ok(variants)
    }

    pub fn candidate_for_compile(
        &self,
        pet_id: &str,
        variant_id: &str,
    ) -> Result<AppearanceVariant, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.query_row(
            "SELECT av.variant_id, av.pet_id, av.job_id, av.image_path, av.cutout_path,
                    av.quality, av.accepted, av.created_at
             FROM appearance_variants av
             JOIN generation_jobs gj ON gj.job_id = av.job_id
             WHERE av.variant_id = ?1 AND av.pet_id = ?2 AND av.job_id = ?1 AND gj.pet_id = ?2",
            rusqlite::params![variant_id, pet_id],
            |row| {
                Ok(AppearanceVariant {
                    variant_id: row.get(0)?,
                    pet_id: row.get(1)?,
                    job_id: row.get(2)?,
                    image_path: row.get(3)?,
                    cutout_path: row.get(4)?,
                    quality: row.get(5)?,
                    accepted: row.get::<_, i64>(6)? != 0,
                    created_at: row.get(7)?,
                })
            },
        )
        .map_err(|error| format!("candidate is not eligible for compilation: {error}"))
    }

    #[cfg(test)]
    pub fn runtime_variant_count(&self, pet_id: &str) -> Result<i64, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.query_row(
            "SELECT COUNT(*) FROM variants WHERE pet_id = ?1",
            rusqlite::params![pet_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
    }

    pub fn running_jobs(&self) -> Result<Vec<JobRecord>, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let mut statement = db
            .prepare(
                "SELECT job_id, pet_id, session_id, prompt, ref_sha256, task_id, status, result_url, error, created_at
                 FROM generation_jobs
                 WHERE status IN ('pending','running') AND task_id IS NOT NULL",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok(JobRecord {
                    job_id: row.get(0)?,
                    pet_id: row.get(1)?,
                    session_id: row.get(2)?,
                    prompt: row.get(3)?,
                    ref_sha256: row.get(4)?,
                    task_id: row.get(5)?,
                    status: row.get(6)?,
                    result_url: row.get(7)?,
                    error: row.get(8)?,
                    created_at: row.get(9)?,
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
                "SELECT job_id, pet_id, session_id, prompt, ref_sha256, task_id, status, result_url, error, created_at
                 FROM generation_jobs WHERE pet_id = ?1 ORDER BY created_at",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(rusqlite::params![pet_id], |row| {
                Ok(JobRecord {
                    job_id: row.get(0)?,
                    pet_id: row.get(1)?,
                    session_id: row.get(2)?,
                    prompt: row.get(3)?,
                    ref_sha256: row.get(4)?,
                    task_id: row.get(5)?,
                    status: row.get(6)?,
                    result_url: row.get(7)?,
                    error: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })
            .map_err(|error| error.to_string())?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row.map_err(|error| error.to_string())?);
        }
        Ok(jobs)
    }

    pub fn upload_jobs(&self, session_id: &str) -> Result<Vec<JobRecord>, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let mut statement = db
            .prepare(
                "SELECT job_id, pet_id, session_id, prompt, ref_sha256, task_id, status,
                        result_url, error, created_at
                 FROM generation_jobs WHERE session_id=?1 ORDER BY created_at, rowid",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([session_id], |row| {
                Ok(JobRecord {
                    job_id: row.get(0)?,
                    pet_id: row.get(1)?,
                    session_id: row.get(2)?,
                    prompt: row.get(3)?,
                    ref_sha256: row.get(4)?,
                    task_id: row.get(5)?,
                    status: row.get(6)?,
                    result_url: row.get(7)?,
                    error: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }
}

fn ensure_legacy_generation_pet(db: &Connection, pet_id: &str) -> Result<(), String> {
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pets WHERE pet_id=?1)",
            [pet_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !exists {
        return Err(format!("pet not found: {pet_id}"));
    }
    let has_live_session: bool = db
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM creation_sessions
               WHERE pet_id=?1 AND status NOT IN ('completed','abandoned')
             )",
            [pet_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if has_live_session {
        return Err(
            "pet belongs to a live creation session; use creation_upload_start instead".into(),
        );
    }
    Ok(())
}

fn validate_candidate_paths(
    jobs_root: &Path,
    job_id: &str,
    raw_path: &str,
    cutout_path: &str,
    motion_profile_path: Option<&str>,
) -> Result<(String, String, Option<String>), String> {
    let root_metadata = std::fs::symlink_metadata(jobs_root)
        .map_err(|error| format!("configured jobs root is missing: {error}"))?;
    if crate::platform::is_link_or_reparse_point(&root_metadata) || !root_metadata.is_dir() {
        return Err("configured jobs root cannot be a link or reparse point".into());
    }
    let canonical_root = jobs_root
        .canonicalize()
        .map_err(|error| format!("could not canonicalize configured jobs root: {error}"))?;
    let expected_job_dir = canonical_root.join(job_id);
    let mut expected = vec![(raw_path, "raw.png"), (cutout_path, "cutout.png")];
    if let Some(motion_profile_path) = motion_profile_path {
        expected.push((motion_profile_path, "motion-profile.json"));
    }
    let mut canonical_paths = Vec::with_capacity(expected.len());
    let mut canonical_job_dir = None;
    for (value, expected_name) in expected {
        let path = Path::new(value);
        if path.file_name().and_then(|name| name.to_str()) != Some(expected_name) {
            return Err(format!("candidate path must end with {expected_name}"));
        }
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| format!("candidate file {expected_name} is missing: {error}"))?;
        if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
            return Err(format!(
                "candidate file {expected_name} is not a regular file"
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| "candidate path has no job directory".to_string())?;
        let parent_metadata = std::fs::symlink_metadata(parent)
            .map_err(|error| format!("candidate job directory is missing: {error}"))?;
        if crate::platform::is_link_or_reparse_point(&parent_metadata) || !parent_metadata.is_dir()
        {
            return Err("candidate job directory cannot be a link or reparse point".into());
        }
        if parent.file_name().and_then(|name| name.to_str()) != Some(job_id) {
            return Err("candidate path is outside the matching job directory".into());
        }
        let canonical_parent = parent.canonicalize().map_err(|error| error.to_string())?;
        if canonical_parent != expected_job_dir {
            return Err("candidate job directory is outside the configured jobs root".into());
        }
        if let Some(expected_parent) = canonical_job_dir.as_ref() {
            if expected_parent != &canonical_parent {
                return Err("candidate files do not share one job directory".into());
            }
        } else {
            canonical_job_dir = Some(canonical_parent.clone());
        }
        let canonical_path = path.canonicalize().map_err(|error| error.to_string())?;
        if canonical_path.parent() != Some(canonical_parent.as_path()) {
            return Err("candidate path escapes its job directory".into());
        }
        canonical_paths.push(canonical_path.to_string_lossy().into_owned());
    }
    let raw_path = canonical_paths.remove(0);
    let cutout_path = canonical_paths.remove(0);
    let motion_profile_path = (!canonical_paths.is_empty()).then(|| canonical_paths.remove(0));
    Ok((raw_path, cutout_path, motion_profile_path))
}

fn require_writable_upload_session(db: &Connection, session_id: &str) -> Result<(), String> {
    let session: Option<(String, String)> = db
        .query_row(
            "SELECT method, status FROM creation_sessions WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let (method, status) =
        session.ok_or_else(|| format!("creation session not found: {session_id}"))?;
    if method != "upload" {
        return Err("upload source requires an upload session".into());
    }
    if matches!(status.as_str(), "completed" | "abandoned") {
        return Err("upload source cannot belong to a terminal session".into());
    }
    Ok(())
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

struct UploadSourceMetadata {
    blob_length: i64,
    sha256: String,
    mime_type: String,
    byte_size: i64,
    created_at: String,
}

fn load_upload_source_record(
    db: &Connection,
    session_id: &str,
) -> Result<Option<UploadSourceRecord>, String> {
    let metadata: Option<UploadSourceMetadata> = db
        .query_row(
            "SELECT length(normalized_png), sha256, mime_type, byte_size, created_at
             FROM creation_upload_sources WHERE session_id=?1",
            [session_id],
            |row| {
                Ok(UploadSourceMetadata {
                    blob_length: row.get(0)?,
                    sha256: row.get(1)?,
                    mime_type: row.get(2)?,
                    byte_size: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let max = crate::generation::tasks::MAX_NORMALIZED_PNG_BYTES as i64;
    if metadata.byte_size <= 0 || metadata.byte_size > max || metadata.blob_length > max {
        return Err("upload source size must be between 1 byte and 10 MiB".into());
    }
    if metadata.blob_length != metadata.byte_size {
        return Err("upload source size metadata is corrupt".into());
    }
    if metadata.mime_type != "image/png" {
        return Err("upload source mime metadata is corrupt".into());
    }
    let normalized_png: Vec<u8> = db
        .query_row(
            "SELECT normalized_png FROM creation_upload_sources WHERE session_id=?1",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let record = UploadSourceRecord {
        normalized_png,
        sha256: metadata.sha256,
        mime_type: metadata.mime_type,
        byte_size: metadata.byte_size,
        created_at: metadata.created_at,
    };
    validate_upload_source_record(&record)?;
    Ok(Some(record))
}

fn validate_upload_source_record(record: &UploadSourceRecord) -> Result<(), String> {
    if record.normalized_png.len() as i64 != record.byte_size {
        return Err("upload source size metadata is corrupt".into());
    }
    if sha256_hex(&record.normalized_png) != record.sha256 {
        return Err("upload source hash metadata is corrupt".into());
    }
    if record.mime_type != "image/png"
        || image::guess_format(&record.normalized_png).ok() != Some(image::ImageFormat::Png)
    {
        return Err("upload source mime metadata is corrupt".into());
    }
    let mut reader = image::ImageReader::with_format(
        std::io::Cursor::new(&record.normalized_png),
        image::ImageFormat::Png,
    );
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|error| format!("upload source PNG data is corrupt: {error}"))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadSourceRecord {
    pub normalized_png: Vec<u8>,
    pub sha256: String,
    pub mime_type: String,
    pub byte_size: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub job_id: String,
    pub pet_id: String,
    pub session_id: Option<String>,
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
    use rusqlite::OptionalExtension;
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

    fn upload_store() -> (CreationStore, std::path::PathBuf, String, String) {
        let (store, root, pet_id) = temp_store();
        let session_id = format!("session-source-{pet_id}");
        store
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO creation_sessions
                 (session_id, pet_id, method, status, last_stable_status, current_step,
                  schema_version, created_at, updated_at)
                 VALUES (?1, ?2, 'upload', 'draft', 'draft', 'upload', 1, ?3, ?3)",
                rusqlite::params![session_id, pet_id, now_iso()],
            )
            .unwrap();
        (store, root, pet_id, session_id)
    }

    fn source_png(red: u8) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([red, 2, 3, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn upload_source_blob_is_session_owned_and_idempotent_only_for_identical_metadata() {
        let (store, root, _, session_id) = upload_store();
        let bytes = source_png(1);
        let hash = super::sha256_hex(&bytes);

        store
            .save_upload_source(&session_id, &bytes, &hash, "image/png")
            .unwrap();
        store
            .save_upload_source(&session_id, &bytes, &hash, "image/png")
            .unwrap();
        let source = store.upload_source(&session_id).unwrap().unwrap();

        assert_eq!(source.normalized_png, bytes);
        assert_eq!(source.sha256, hash);
        assert_eq!(source.mime_type, "image/png");
        assert_eq!(source.byte_size, bytes.len() as i64);
        let different = source_png(9);
        let different_hash = super::sha256_hex(&different);
        assert!(store
            .save_upload_source(&session_id, &different, &different_hash, "image/png")
            .unwrap_err()
            .contains("different"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn upload_source_read_rejects_tampered_blob_hash_size_or_mime_metadata() {
        let cases = [
            ("normalized_png=X'00'", "size"),
            (
                "sha256='ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'",
                "hash",
            ),
            ("byte_size=999", "size"),
            ("mime_type='image/jpeg'", "mime"),
        ];
        for (update, expected) in cases {
            let (store, root, _, session_id) = upload_store();
            let bytes = source_png(1);
            let hash = super::sha256_hex(&bytes);
            store
                .save_upload_source(&session_id, &bytes, &hash, "image/png")
                .unwrap();
            store
                .storage
                .lock()
                .unwrap()
                .db
                .execute_batch(&format!(
                    "PRAGMA ignore_check_constraints=ON;
                     UPDATE creation_upload_sources SET {update} WHERE session_id='{session_id}';
                     PRAGMA ignore_check_constraints=OFF;"
                ))
                .unwrap();

            let error = store.upload_source(&session_id).unwrap_err();

            assert!(error.contains(expected), "{update}: {error}");
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn upload_source_read_rejects_jointly_tampered_metadata_and_truncated_png() {
        let (store, root, _, session_id) = upload_store();
        let bytes = source_png(1);
        let hash = super::sha256_hex(&bytes);
        store
            .save_upload_source(&session_id, &bytes, &hash, "image/png")
            .unwrap();
        let truncated = b"\x89PNG\r\n\x1a\n";
        store
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_upload_sources
                 SET normalized_png=?2, sha256=?3, mime_type='image/png', byte_size=?4
                 WHERE session_id=?1",
                rusqlite::params![
                    session_id,
                    truncated,
                    super::sha256_hex(truncated),
                    truncated.len() as i64
                ],
            )
            .unwrap();

        assert!(store
            .upload_source(&session_id)
            .unwrap_err()
            .contains("PNG"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn upload_source_rejects_declared_oversize_before_blob_validation() {
        let (store, root, _, session_id) = upload_store();
        let bytes = source_png(1);
        let hash = super::sha256_hex(&bytes);
        store
            .save_upload_source(&session_id, &bytes, &hash, "image/png")
            .unwrap();
        store
            .storage
            .lock()
            .unwrap()
            .db
            .execute_batch(&format!(
                "PRAGMA ignore_check_constraints=ON;
                 UPDATE creation_upload_sources SET byte_size=10485761
                 WHERE session_id='{session_id}';
                 PRAGMA ignore_check_constraints=OFF;"
            ))
            .unwrap();

        let error = store.upload_source(&session_id).unwrap_err();

        assert!(
            error.contains("10 MiB"),
            "unexpected validation priority: {error}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn upload_source_rejects_non_upload_terminal_and_unowned_sessions() {
        let (store, root, pet_id, session_id) = upload_store();
        let bytes = source_png(1);
        let hash = super::sha256_hex(&bytes);
        store
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions SET method='composer' WHERE session_id=?1",
                [&session_id],
            )
            .unwrap();
        assert!(store
            .save_upload_source(&session_id, &bytes, &hash, "image/png")
            .unwrap_err()
            .contains("upload"));
        store
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions SET method='upload', status='completed' WHERE session_id=?1",
                [&session_id],
            )
            .unwrap();
        assert!(store
            .save_upload_source(&session_id, &bytes, &hash, "image/png")
            .unwrap_err()
            .contains("terminal"));
        assert!(store
            .save_upload_source("session-missing", &bytes, &hash, "image/png")
            .unwrap_err()
            .contains("not found"));
        assert_eq!(
            store
                .storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT COUNT(*) FROM creation_upload_sources WHERE session_id=?1",
                    [&session_id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert!(!pet_id.is_empty());
        let _ = std::fs::remove_dir_all(root);
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
        assert_eq!(running[0].status, "running");

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
    fn legacy_job_is_submitting_and_unpollable_until_task_attachment() {
        let (store, root, pet_id) = temp_store();
        store
            .create_job("job-submitting", &pet_id, "p", "h", None)
            .unwrap();

        let reserved = store.job("job-submitting").unwrap();
        assert_eq!(reserved.status, "submitting");
        assert!(reserved.task_id.is_none());
        assert!(store.running_jobs().unwrap().is_empty());

        store
            .attach_task_to_legacy_job("job-submitting", &pet_id, "task-1")
            .unwrap();
        let attached = store.job("job-submitting").unwrap();
        assert_eq!(attached.status, "running");
        assert_eq!(attached.task_id.as_deref(), Some("task-1"));
        assert_eq!(store.running_jobs().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn upload_job_is_submitting_and_unpollable_until_task_attachment() {
        let (store, root, pet_id) = temp_store();
        let session_id = "session-submitting";
        store
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO creation_sessions
                 (session_id, pet_id, method, status, last_stable_status, current_step,
                  schema_version, created_at, updated_at)
                 VALUES (?1, ?2, 'upload', 'draft', 'draft', 'upload', 1, ?3, ?3)",
                rusqlite::params![session_id, pet_id, now_iso()],
            )
            .unwrap();
        store
            .create_job_for_session("job-submitting", session_id, "p", "h", None)
            .unwrap();

        let reserved = store.job("job-submitting").unwrap();
        assert_eq!(reserved.status, "submitting");
        assert!(store.running_jobs().unwrap().is_empty());

        store
            .attach_task_to_upload_job("job-submitting", session_id, &pet_id, "task-1")
            .unwrap();
        let attached = store.job("job-submitting").unwrap();
        assert_eq!(attached.status, "running");
        assert_eq!(attached.task_id.as_deref(), Some("task-1"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn status_update_requires_a_durable_job_row() {
        let (store, root, _) = temp_store();

        assert!(store
            .update_job_status("missing-job", "failed", None, Some("failed"))
            .unwrap_err()
            .contains("not found"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_job_creation_rejects_a_pet_with_a_live_creation_session() {
        let (store, root, pet_id) = temp_store();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO creation_sessions
                 (session_id, pet_id, method, status, last_stable_status, current_step,
                  schema_version, created_at, updated_at)
                 VALUES ('session-live', ?1, 'upload', 'draft', 'draft', 'upload', 1, ?2, ?2)",
                rusqlite::params![pet_id, now_iso()],
            )
            .unwrap();

        assert!(store
            .create_job("job-legacy", &pet_id, "p", "h", Some("task"))
            .unwrap_err()
            .contains("creation_upload_start"));
        assert!(store.job_list(&pet_id).unwrap().is_empty());
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

    #[test]
    fn job_creation_with_separate_connection_obeys_foreign_key() {
        // simulates lib.rs setup: pet repository and creation store use
        // two independent Storage connections to the same database file
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("desktop-pet-fk-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();

        let storage_a = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let repo = crate::pets::repository::PetRepository::new(storage_a);
        let pet = repo
            .create(
                crate::pets::pet::Species::Cat,
                crate::pets::pet::IdentityMode::RealPet,
            )
            .unwrap();

        let storage_b = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let store = CreationStore::new(storage_b);
        store
            .create_job("j1", &pet.pet_id, "p", "h", Some("t1"))
            .expect("job creation must succeed across connections");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn successful_job_persists_one_candidate() {
        let (store, root, pet_id) = temp_store();
        store
            .create_job("job-1", &pet_id, "p", "h", Some("task-1"))
            .unwrap();
        store
            .record_candidate("job-1", &pet_id, "raw.png", "cutout.png", "acceptable")
            .unwrap();
        let variants = store.candidates(&pet_id).unwrap();
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].variant_id, "job-1");
        assert!(!variants[0].accepted);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_variant_upsert_is_idempotent() {
        let (store, root, pet_id) = temp_store();
        store
            .record_runtime_variant("job-1", &pet_id, "manifest.json")
            .unwrap();
        store
            .record_runtime_variant("job-1", &pet_id, "manifest.json")
            .unwrap();
        assert_eq!(store.runtime_variant_count(&pet_id).unwrap(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compile_error_can_be_set_and_cleared() {
        let (store, root, pet_id) = temp_store();
        store.set_compile_error(&pet_id, "compile failed").unwrap();
        let key = format!("creation:{pet_id}:compile_error");
        let saved: String = {
            let storage = store.storage.lock().unwrap();
            storage
                .db
                .query_row(
                    "SELECT value FROM state WHERE key = ?1",
                    rusqlite::params![&key],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(saved, "compile failed");

        store.clear_compile_error(&pet_id).unwrap();
        let missing: Option<String> = {
            let storage = store.storage.lock().unwrap();
            storage
                .db
                .query_row(
                    "SELECT value FROM state WHERE key = ?1",
                    rusqlite::params![&key],
                    |row| row.get(0),
                )
                .optional()
                .unwrap()
        };
        assert_eq!(missing, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_upsert_rejects_an_existing_variant_owned_by_another_pet() {
        let (store, root, pet_id) = temp_store();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let repo = crate::pets::repository::PetRepository::new(storage);
        let other_pet = repo
            .create(
                crate::pets::pet::Species::Dog,
                crate::pets::pet::IdentityMode::Adopted,
            )
            .unwrap();
        store.create_job("job-1", &pet_id, "p", "h", None).unwrap();
        store
            .record_candidate(
                "job-1",
                &pet_id,
                "original.png",
                "original-cutout.png",
                "acceptable",
            )
            .unwrap();

        assert!(store
            .record_candidate(
                "job-1",
                &other_pet.pet_id,
                "other.png",
                "other-cutout.png",
                "acceptable"
            )
            .is_err());
        let variants = store.candidates(&pet_id).unwrap();
        assert_eq!(variants[0].image_path, "original.png");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_insert_rejects_a_job_owned_by_another_pet() {
        let (store, root, pet_id) = temp_store();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let repo = crate::pets::repository::PetRepository::new(storage);
        let other_pet = repo
            .create(
                crate::pets::pet::Species::Dog,
                crate::pets::pet::IdentityMode::Adopted,
            )
            .unwrap();
        store.create_job("job-a", &pet_id, "p", "h", None).unwrap();

        assert!(store
            .record_candidate(
                "job-a",
                &other_pet.pet_id,
                "other.png",
                "other-cutout.png",
                "acceptable"
            )
            .is_err());
        assert!(store.candidates(&pet_id).unwrap().is_empty());
        assert!(store.candidates(&other_pet.pet_id).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_upsert_rejects_an_existing_variant_with_a_different_job() {
        let (store, root, pet_id) = temp_store();
        store.create_job("job-1", &pet_id, "p", "h", None).unwrap();
        store.create_job("job-2", &pet_id, "p", "h", None).unwrap();
        {
            let storage = store.storage.lock().unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO appearance_variants
                     (variant_id, pet_id, job_id, image_path, cutout_path, quality, accepted, created_at)
                     VALUES ('job-2', ?1, 'job-1', 'original.png', 'original-cutout.png', 'acceptable', 0, ?2)",
                    rusqlite::params![&pet_id, now_iso()],
                )
                .unwrap();
        }

        assert!(store
            .record_candidate(
                "job-2",
                &pet_id,
                "other.png",
                "other-cutout.png",
                "acceptable"
            )
            .is_err());
        let variants = store.candidates(&pet_id).unwrap();
        assert_eq!(variants[0].image_path, "original.png");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_variant_upsert_rejects_an_existing_variant_owned_by_another_pet() {
        let (store, root, pet_id) = temp_store();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let repo = crate::pets::repository::PetRepository::new(storage);
        let other_pet = repo
            .create(
                crate::pets::pet::Species::Dog,
                crate::pets::pet::IdentityMode::Adopted,
            )
            .unwrap();
        store
            .record_runtime_variant("variant-1", &pet_id, "original-manifest.json")
            .unwrap();

        assert!(store
            .record_runtime_variant("variant-1", &other_pet.pet_id, "other-manifest.json")
            .is_err());
        let db = &store.storage.lock().unwrap().db;
        let manifest_path: String = db
            .query_row(
                "SELECT manifest_path FROM variants WHERE variant_id = 'variant-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(manifest_path, "original-manifest.json");
        let _ = std::fs::remove_dir_all(root);
    }
}

#[allow(dead_code)] // returned by the candidate-directory projection workflow
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceVariant {
    pub variant_id: String,
    pub pet_id: String,
    pub job_id: Option<String>,
    pub image_path: String,
    pub cutout_path: Option<String>,
    pub quality: String,
    pub accepted: bool,
    pub created_at: String,
}
