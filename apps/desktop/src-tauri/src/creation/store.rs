use crate::creation::profiles;
use crate::creation::StandardCandidate;
use crate::storage::Storage;
use rusqlite::OptionalExtension;
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
                 WHERE session_id=?1 AND status IN ('pending','running')",
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
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
                 WHERE job_id=?1 AND session_id=?2 AND pet_id=?3 AND status='pending'",
                rusqlite::params![job_id, session_id, pet_id, task_id],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("upload job ownership changed before task attachment".into());
        }
        Ok(())
    }

    pub fn record_upload_candidate(
        &self,
        job_id: &str,
        session_id: &str,
        raw_path: &str,
        cutout_path: &str,
        motion_profile_path: &str,
        quality: &str,
    ) -> Result<StandardCandidate, String> {
        self.record_upload_candidate_transaction(
            job_id,
            session_id,
            raw_path,
            cutout_path,
            motion_profile_path,
            quality,
            None,
        )
    }

    pub fn record_upload_candidate_with_result_url(
        &self,
        job_id: &str,
        session_id: &str,
        raw_path: &str,
        cutout_path: &str,
        motion_profile_path: &str,
        quality: &str,
        result_url: &str,
    ) -> Result<StandardCandidate, String> {
        self.record_upload_candidate_transaction(
            job_id,
            session_id,
            raw_path,
            cutout_path,
            motion_profile_path,
            quality,
            Some(result_url),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_upload_candidate_transaction(
        &self,
        job_id: &str,
        session_id: &str,
        raw_path: &str,
        cutout_path: &str,
        motion_profile_path: &str,
        quality: &str,
        result_url: Option<&str>,
    ) -> Result<StandardCandidate, String> {
        let (raw_path, cutout_path, motion_profile_path) =
            validate_candidate_paths(job_id, raw_path, cutout_path, motion_profile_path)?;
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
              motion_profile_path, quality, accepted, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
            rusqlite::params![
                candidate_id,
                job_pet_id,
                job_id,
                session_id,
                raw_path,
                cutout_path,
                motion_profile_path,
                quality,
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
            created_at,
        })
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
                   AND status IN ('pending','running')",
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

    pub fn candidate_for_session(&self, session_id: &str) -> Result<StandardCandidate, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        db.query_row(
            "SELECT av.variant_id, av.session_id, av.pet_id, av.job_id, av.cutout_path,
                    av.motion_profile_path, av.quality, av.created_at
             FROM appearance_variants av
             JOIN generation_jobs gj ON gj.job_id=av.job_id
             JOIN creation_sessions cs ON cs.session_id=av.session_id
             WHERE av.session_id=?1 AND av.pet_id=cs.pet_id
               AND gj.session_id=av.session_id AND gj.pet_id=av.pet_id
               AND gj.status='success' AND cs.status!='abandoned'
               AND av.cutout_path IS NOT NULL AND av.motion_profile_path IS NOT NULL
               AND av.motion_profile_path!=''
             ORDER BY av.created_at DESC, av.rowid DESC LIMIT 1",
            [session_id],
            |row| {
                Ok(StandardCandidate {
                    candidate_id: row.get(0)?,
                    session_id: row.get(1)?,
                    pet_id: row.get(2)?,
                    job_id: row.get(3)?,
                    body_path: row.get(4)?,
                    motion_profile_path: row.get(5)?,
                    quality: row.get(6)?,
                    created_at: row.get(7)?,
                })
            },
        )
        .map_err(|error| format!("candidate is not available for creation session: {error}"))
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
                 FROM generation_jobs WHERE status IN ('pending','running')",
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

fn validate_candidate_paths(
    job_id: &str,
    raw_path: &str,
    cutout_path: &str,
    motion_profile_path: &str,
) -> Result<(String, String, String), String> {
    let expected = [
        (raw_path, "raw.png"),
        (cutout_path, "cutout.png"),
        (motion_profile_path, "motion-profile.json"),
    ];
    let mut canonical_paths = Vec::with_capacity(expected.len());
    let mut canonical_job_dir = None;
    for (value, expected_name) in expected {
        let path = Path::new(value);
        if path.file_name().and_then(|name| name.to_str()) != Some(expected_name) {
            return Err(format!("candidate path must end with {expected_name}"));
        }
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| format!("candidate file {expected_name} is missing: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "candidate file {expected_name} is not a regular file"
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| "candidate path has no job directory".to_string())?;
        let parent_metadata = std::fs::symlink_metadata(parent)
            .map_err(|error| format!("candidate job directory is missing: {error}"))?;
        if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
            return Err("candidate job directory cannot be a symbolic link".into());
        }
        if parent.file_name().and_then(|name| name.to_str()) != Some(job_id) {
            return Err("candidate path is outside the matching job directory".into());
        }
        let canonical_parent = parent.canonicalize().map_err(|error| error.to_string())?;
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
    Ok((
        canonical_paths.remove(0),
        canonical_paths.remove(0),
        canonical_paths.remove(0),
    ))
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
