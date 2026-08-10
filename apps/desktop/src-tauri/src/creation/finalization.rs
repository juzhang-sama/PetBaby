use crate::creation::domain::{
    CreationMethod, CreationSessionStatus, CreationSnapshot, PreparedCreation,
};
use crate::creation::name::normalize_display_name;
use crate::pets::mutation::{MutationKind, SharedPetMutationGate};
use crate::runtime_assets::compiler::compile_animated_image;
use crate::storage::Storage;
use rusqlite::OptionalExtension;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type SharedCreationFinalizationService = Arc<CreationFinalizationService>;
pub type SharedSwitchTransaction = Arc<Mutex<()>>;

#[derive(Debug, Default, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryReport {
    pub completed_session_ids: Vec<String>,
    pub retryable_session_ids: Vec<String>,
    pub cleaned_session_ids: Vec<String>,
    pub warnings: Vec<String>,
}

pub struct CreationFinalizationService {
    storage: Arc<Mutex<Storage>>,
    app_data_dir: PathBuf,
    jobs_root: PathBuf,
    mutation_gate: SharedPetMutationGate,
    switch_transaction: SharedSwitchTransaction,
}

#[derive(Debug)]
struct FinalizationRecord {
    session_id: String,
    pet_id: String,
    method: String,
    status: String,
    last_stable_status: String,
    current_step: String,
    display_name: Option<String>,
    pet_method: String,
    lifecycle: String,
    pet_completed_at: Option<String>,
    active_pet_id: Option<String>,
    error: Option<String>,
    candidate_id: String,
    job_id: Option<String>,
    job_status: Option<String>,
    job_pet_id: Option<String>,
    job_session_id: Option<String>,
    raw_path: PathBuf,
    body_path: PathBuf,
    motion_profile_path: PathBuf,
    accepted: bool,
    runtime_pet_id: Option<String>,
    manifest_path: Option<String>,
}

impl CreationFinalizationService {
    pub fn new(
        storage: Arc<Mutex<Storage>>,
        app_data_dir: PathBuf,
        jobs_root: PathBuf,
        mutation_gate: SharedPetMutationGate,
        switch_transaction: SharedSwitchTransaction,
    ) -> Self {
        Self {
            storage,
            app_data_dir,
            jobs_root,
            mutation_gate,
            switch_transaction,
        }
    }

    pub fn prepare(&self, session_id: &str, request_id: &str) -> Result<PreparedCreation, String> {
        validate_component(session_id, "session id")?;
        validate_component(request_id, "request id")?;
        let _switch_transaction = self
            .switch_transaction
            .lock()
            .map_err(|_| "switch transaction lock poisoned")?;
        let record = self.finalization_record(session_id)?;
        self.validate_record(&record)?;

        if record.status == "completed" {
            if !record.accepted
                || record.runtime_pet_id.as_deref() != Some(record.pet_id.as_str())
                || record.manifest_path.is_none()
            {
                return Err("completed creation is missing its accepted runtime variant".into());
            }
            let expected_manifest = self.assets_dir(&record.pet_id).join("manifest.json");
            if record.manifest_path.as_deref().map(Path::new) != Some(expected_manifest.as_path())
                || !self.install_is_owned(&record)?
            {
                return Err("completed creation runtime manifest is not authoritative".into());
            }
            return Ok(prepared(&record, request_id, true));
        }
        let retryable =
            record.status == "retryableFailure" && record.last_stable_status == "candidateReady";
        let resumable_finalizing = record.status == "finalizing"
            && record.runtime_pet_id.as_deref() == Some(record.pet_id.as_str())
            && record.manifest_path.is_some();
        if record.status != "candidateReady" && !retryable && !resumable_finalizing {
            return Err(format!(
                "creation session is not ready for finalization: {}",
                record.status
            ));
        }
        self.validate_candidate_paths(&record)?;
        if self.assets_dir(&record.pet_id).exists() && !resumable_finalizing {
            return Err("creation pet assets already exist and require exact recovery".into());
        }
        if resumable_finalizing && !self.install_is_owned(&record)? {
            return Err("finalizing runtime assets are not owned by this candidate".into());
        }

        self.mutation_gate
            .begin(request_id, MutationKind::Switch, &record.pet_id)?;
        let owner_pin =
            match self
                .mutation_gate
                .assert_owner(request_id, MutationKind::Switch, &record.pet_id)
            {
                Ok(pin) => pin,
                Err(error) => {
                    let _ = self.mutation_gate.finish(request_id);
                    return Err(error);
                }
            };

        let result: Result<PreparedCreation, String> = (|| {
            if resumable_finalizing {
                let manifest = record
                    .manifest_path
                    .as_deref()
                    .ok_or_else(|| "finalizing creation has no runtime manifest".to_string())?;
                if Path::new(manifest) != self.assets_dir(&record.pet_id).join("manifest.json") {
                    return Err("runtime manifest is outside the session pet assets".into());
                }
                return Ok(prepared(&record, request_id, false));
            }
            self.mark_finalizing(&record)?;
            let assets_dir = self.assets_dir(&record.pet_id);
            let compiled = compile_animated_image(
                &record.pet_id,
                &record.candidate_id,
                &record.body_path,
                &record.motion_profile_path,
                &assets_dir,
            )?;
            self.record_runtime_variant(&record, &compiled.manifest_path)?;
            Ok(prepared(&record, request_id, false))
        })();

        drop(owner_pin);
        match result {
            Ok(prepared) => Ok(prepared),
            Err(error) => {
                let cleanup_warning = self
                    .compensate_failed_prepare(&record, &error)
                    .err()
                    .map(|warning| format!("; compensation failed: {warning}"))
                    .unwrap_or_default();
                let finish_warning = self
                    .mutation_gate
                    .finish(request_id)
                    .err()
                    .map(|warning| format!("; gate release failed: {warning}"))
                    .unwrap_or_default();
                Err(format!("{error}{cleanup_warning}{finish_warning}"))
            }
        }
    }

    pub fn abort(&self, session_id: &str, error: &str) -> Result<CreationSnapshot, String> {
        validate_component(session_id, "session id")?;
        let _switch_transaction = self
            .switch_transaction
            .lock()
            .map_err(|_| "switch transaction lock poisoned")?;
        let record = self.finalization_record(session_id)?;
        if record.status == "completed" {
            return snapshot(&record);
        }
        if record.status == "abandoned" {
            return Err("cannot abort an abandoned creation session".into());
        }
        self.validate_record(&record)?;
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|db_error| db_error.to_string())?;
        tx.execute(
            "DELETE FROM variants
             WHERE variant_id=?1 AND pet_id=?2
               AND EXISTS (
                 SELECT 1 FROM appearance_variants av
                 WHERE av.variant_id=?1 AND av.session_id=?3 AND av.pet_id=?2 AND av.accepted=0
               )",
            rusqlite::params![record.candidate_id, record.pet_id, record.session_id],
        )
        .map_err(|db_error| db_error.to_string())?;
        tx.execute(
            "UPDATE creation_sessions
             SET status='retryableFailure', last_stable_status='candidateReady',
                 current_step='review', error=?2, updated_at=?3
             WHERE session_id=?1 AND pet_id=?4 AND status!='completed' AND status!='abandoned'",
            rusqlite::params![
                record.session_id,
                error,
                crate::creation::profiles::now_iso(),
                record.pet_id
            ],
        )
        .map_err(|db_error| db_error.to_string())?;
        tx.commit().map_err(|db_error| db_error.to_string())?;
        drop(storage);
        self.remove_owned_install(&record)?;
        let refreshed = self.finalization_record(session_id)?;
        snapshot(&refreshed)
    }

    pub fn recover(&self) -> Result<RecoveryReport, String> {
        let _switch_transaction = self
            .switch_transaction
            .lock()
            .map_err(|_| "switch transaction lock poisoned")?;
        let session_rows: Vec<(String, String)> = {
            let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
            let mut statement = storage
                .db
                .prepare(
                    "SELECT cs.session_id, cs.status FROM creation_sessions cs
                     WHERE status IN ('finalizing','completed')
                        OR (status='retryableFailure' AND last_stable_status='candidateReady')
                        OR (status='candidateReady' AND last_stable_status='candidateReady'
                            AND EXISTS (
                              SELECT 1 FROM appearance_variants av
                              JOIN variants rv ON rv.variant_id=av.variant_id AND rv.pet_id=av.pet_id
                              WHERE av.session_id=cs.session_id AND av.pet_id=cs.pet_id
                                AND av.accepted=0
                            ))
                     ORDER BY cs.created_at, cs.session_id",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|error| error.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?
        };
        let mut report = RecoveryReport::default();
        for (session_id, status) in session_rows {
            if status == "completed" {
                report.completed_session_ids.push(session_id);
                continue;
            }
            let recovered = (|| {
                validate_component(&session_id, "session id")?;
                let record = self.finalization_record(&session_id)?;
                self.validate_record(&record)?;
                self.validate_candidate_paths(&record)?;
                let had_install = self.install_is_owned(&record)?;
                let removed_runtime = self.clean_interrupted_database(&record)?;
                self.remove_owned_install(&record)?;
                Ok::<bool, String>(had_install || removed_runtime)
            })();
            match recovered {
                Ok(cleaned) => {
                    report.retryable_session_ids.push(session_id.clone());
                    if cleaned {
                        report.cleaned_session_ids.push(session_id);
                    }
                }
                Err(error) => report
                    .warnings
                    .push(format!("creation recovery skipped {session_id}: {error}")),
            }
        }
        Ok(report)
    }

    fn finalization_record(&self, session_id: &str) -> Result<FinalizationRecord, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let session_facts: Option<(String, String)> = storage
            .db
            .query_row(
                "SELECT status, method FROM creation_sessions WHERE session_id=?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (session_status, session_method) =
            session_facts.ok_or_else(|| format!("creation session not found: {session_id}"))?;
        let completed = session_status == "completed";
        if completed {
            let accepted_runtime_count: i64 = storage
                .db
                .query_row(
                    "SELECT COUNT(*) FROM appearance_variants av
                     JOIN creation_sessions cs ON cs.session_id=av.session_id
                     JOIN variants rv ON rv.variant_id=av.variant_id AND rv.pet_id=av.pet_id
                     WHERE av.session_id=?1 AND av.pet_id=cs.pet_id AND av.accepted=1",
                    [session_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if accepted_runtime_count != 1 {
                return Err(
                    "completed creation must have exactly one accepted runtime candidate".into(),
                );
            }
        }
        storage
            .db
            .query_row(
                "SELECT cs.session_id, cs.pet_id, cs.method, cs.status, cs.last_stable_status,
                        cs.current_step, p.display_name, cs.error,
                        av.variant_id, av.job_id, av.image_path, av.cutout_path,
                        av.motion_profile_path, av.accepted, rv.pet_id, rv.manifest_path,
                        gj.pet_id, gj.session_id, gj.status,
                        p.creation_method, p.lifecycle, p.completed_at,
                        (SELECT value FROM state WHERE key='app:active_pet_id')
                 FROM creation_sessions cs
                 JOIN pets p ON p.pet_id=cs.pet_id
                 JOIN appearance_variants av ON av.session_id=cs.session_id AND av.pet_id=cs.pet_id
                 LEFT JOIN variants rv ON rv.variant_id=av.variant_id AND rv.pet_id=av.pet_id
                 LEFT JOIN generation_jobs gj ON gj.job_id=av.job_id
                 WHERE cs.session_id=?1 AND av.pet_id=cs.pet_id
                   AND ((?2=1 AND av.accepted=1 AND rv.variant_id IS NOT NULL)
                        OR (?2=0 AND ?3='upload' AND av.accepted=0
                            AND av.job_id IS NOT NULL
                            AND gj.session_id=cs.session_id AND gj.pet_id=cs.pet_id
                            AND gj.status='success')
                        OR (?2=0 AND ?3 IN ('composer','adoption') AND av.accepted=0
                            AND av.job_id IS NULL))
                 ORDER BY av.created_at DESC, av.rowid DESC
                 LIMIT 1",
                rusqlite::params![session_id, completed, session_method],
                |row| {
                    let method: String = row.get(2)?;
                    let candidate_job_id: Option<String> = row.get(9)?;
                    let job_pet_id: Option<String> = row.get(16)?;
                    let job_session_id: Option<String> = row.get(17)?;
                    let job_status: Option<String> = row.get(18)?;
                    let session_pet_id: String = row.get(1)?;
                    let stored_session_id: String = row.get(0)?;
                    Ok(FinalizationRecord {
                        session_id: stored_session_id,
                        pet_id: session_pet_id,
                        method,
                        status: row.get(3)?,
                        last_stable_status: row.get(4)?,
                        current_step: row.get(5)?,
                        display_name: row.get(6)?,
                        pet_method: row.get(19)?,
                        lifecycle: row.get(20)?,
                        pet_completed_at: row.get(21)?,
                        active_pet_id: row.get(22)?,
                        error: row.get(7)?,
                        candidate_id: row.get(8)?,
                        job_id: candidate_job_id,
                        job_status,
                        job_pet_id,
                        job_session_id,
                        raw_path: PathBuf::from(row.get::<_, String>(10)?),
                        body_path: PathBuf::from(row.get::<_, String>(11)?),
                        motion_profile_path: PathBuf::from(row.get::<_, String>(12)?),
                        accepted: row.get::<_, i64>(13)? != 0,
                        runtime_pet_id: row.get(14)?,
                        manifest_path: row.get(15)?,
                    })
                },
            )
            .optional()
            .map_err(|error| format!("creation finalization record is invalid: {error}"))?
            .ok_or_else(|| format!("creation session not found: {session_id}"))
    }

    fn validate_record(&self, record: &FinalizationRecord) -> Result<(), String> {
        validate_component(&record.session_id, "session id")?;
        validate_component(&record.pet_id, "pet id")?;
        validate_component(&record.candidate_id, "candidate id")?;
        if let Some(job_id) = &record.job_id {
            validate_component(job_id, "job id")?;
        }
        if record.pet_method != record.method {
            return Err("creation session method does not match its pet".into());
        }
        if record.status == "completed" {
            if record.lifecycle != "ready"
                || record.pet_completed_at.is_none()
                || !record.accepted
                || record.runtime_pet_id.as_deref() != Some(record.pet_id.as_str())
            {
                return Err("completed creation facts are inconsistent".into());
            }
        } else if record.lifecycle != "draft"
            || record.pet_completed_at.is_some()
            || record.active_pet_id.as_deref() == Some(record.pet_id.as_str())
            || record.accepted
        {
            return Err("unfinished creation must own an inactive draft pet".into());
        }
        if record.status != "completed" {
            match record.method.as_str() {
                "upload" => {
                    if record.job_id.is_none()
                        || record.job_pet_id.as_deref() != Some(record.pet_id.as_str())
                        || record.job_session_id.as_deref() != Some(record.session_id.as_str())
                        || record.job_status.as_deref() != Some("success")
                    {
                        return Err(
                            "upload candidate requires its session's successful generation job"
                                .into(),
                        );
                    }
                }
                "composer" | "adoption" if record.job_id.is_some() => {
                    return Err(
                        "composer and adoption candidates cannot reference a generation job".into(),
                    );
                }
                "composer" | "adoption" => {}
                _ => return Err(format!("unknown creation method: {}", record.method)),
            }
        }
        let name = record
            .display_name
            .as_deref()
            .ok_or("creation pet name has not been saved")?;
        if normalize_display_name(name)? != name {
            return Err("creation pet name is not stored in normalized form".into());
        }
        Ok(())
    }

    fn validate_candidate_paths(&self, record: &FinalizationRecord) -> Result<(), String> {
        let (root, session_dir, expected_dir, expected_body_name) =
            if let Some(job_id) = &record.job_id {
                (
                    self.jobs_root.clone(),
                    None,
                    self.jobs_root.join(job_id),
                    "cutout.png",
                )
            } else {
                let root = self.app_data_dir.join("creation-sessions");
                let session_dir = root.join(&record.session_id);
                (
                    root.clone(),
                    Some(session_dir.clone()),
                    session_dir.join("candidate"),
                    "body.png",
                )
            };
        let root_metadata = std::fs::symlink_metadata(&root)
            .map_err(|error| format!("configured candidate root is missing: {error}"))?;
        if crate::platform::is_link_or_reparse_point(&root_metadata) || !root_metadata.is_dir() {
            return Err("configured candidate root cannot be a link or reparse point".into());
        }
        if let Some(session_dir) = &session_dir {
            let metadata = std::fs::symlink_metadata(session_dir)
                .map_err(|error| format!("creation session directory is missing: {error}"))?;
            if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
                return Err("creation session directory cannot be a link or reparse point".into());
            }
        }
        let expected_metadata = std::fs::symlink_metadata(&expected_dir)
            .map_err(|error| format!("candidate directory is missing: {error}"))?;
        if crate::platform::is_link_or_reparse_point(&expected_metadata)
            || !expected_metadata.is_dir()
        {
            return Err("candidate directory cannot be a link or reparse point".into());
        }
        let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
        let canonical_expected = expected_dir
            .canonicalize()
            .map_err(|error| format!("candidate directory is missing: {error}"))?;
        if !canonical_expected.starts_with(&canonical_root) {
            return Err("candidate directory escapes its configured root".into());
        }
        let raw_name = if record.job_id.is_some() {
            "raw.png"
        } else if record.raw_path == record.body_path {
            expected_body_name
        } else {
            "raw.png"
        };
        for (path, file_name) in [
            (&record.raw_path, raw_name),
            (&record.body_path, expected_body_name),
            (&record.motion_profile_path, "motion-profile.json"),
        ] {
            if path.file_name().and_then(|name| name.to_str()) != Some(file_name) {
                return Err(format!("candidate path must end with {file_name}"));
            }
            let metadata = std::fs::symlink_metadata(path)
                .map_err(|error| format!("candidate file {file_name} is missing: {error}"))?;
            if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
                return Err(format!("candidate file {file_name} is not a regular file"));
            }
            let parent = path
                .parent()
                .ok_or("candidate path has no candidate directory")?;
            let parent_metadata = std::fs::symlink_metadata(parent)
                .map_err(|error| format!("candidate directory is missing: {error}"))?;
            if crate::platform::is_link_or_reparse_point(&parent_metadata)
                || !parent_metadata.is_dir()
            {
                return Err("candidate directory cannot be a link or reparse point".into());
            }
            let canonical_parent = parent.canonicalize().map_err(|error| error.to_string())?;
            if canonical_parent != canonical_expected {
                return Err(
                    "candidate path is outside the matching authoritative directory".into(),
                );
            }
            let canonical_path = path.canonicalize().map_err(|error| error.to_string())?;
            if canonical_path.parent() != Some(canonical_parent.as_path()) {
                return Err("candidate path escapes its standard job directory".into());
            }
        }
        Ok(())
    }

    fn mark_finalizing(&self, record: &FinalizationRecord) -> Result<(), String> {
        let affected = self
            .storage
            .lock()
            .map_err(|_| "storage lock poisoned")?
            .db
            .execute(
                "UPDATE creation_sessions
                 SET status='finalizing', last_stable_status='candidateReady',
                     current_step='finalizing', error=NULL, updated_at=?2
                 WHERE session_id=?1 AND pet_id=?3
                   AND (status='candidateReady'
                        OR (status='retryableFailure' AND last_stable_status='candidateReady'))
                   AND EXISTS (SELECT 1 FROM pets p
                               WHERE p.pet_id=?3 AND p.lifecycle='draft' AND p.completed_at IS NULL)
                   AND NOT EXISTS (SELECT 1 FROM state
                                   WHERE key='app:active_pet_id' AND value=?3)",
                rusqlite::params![
                    record.session_id,
                    crate::creation::profiles::now_iso(),
                    record.pet_id
                ],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("creation session changed before finalization".into());
        }
        Ok(())
    }

    fn record_runtime_variant(
        &self,
        record: &FinalizationRecord,
        manifest_path: &str,
    ) -> Result<(), String> {
        let expected_manifest = self.assets_dir(&record.pet_id).join("manifest.json");
        if Path::new(manifest_path) != expected_manifest {
            return Err("compiler returned a manifest outside the target pet assets".into());
        }
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let status: Option<String> = tx
            .query_row(
                "SELECT cs.status FROM creation_sessions cs
                 JOIN appearance_variants av ON av.session_id=cs.session_id AND av.pet_id=cs.pet_id
                 JOIN pets p ON p.pet_id=cs.pet_id
                 WHERE cs.session_id=?1 AND cs.pet_id=?2 AND av.variant_id=?3 AND av.accepted=0
                   AND p.lifecycle='draft' AND p.completed_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM state
                                   WHERE key='app:active_pet_id' AND value=?2)",
                rusqlite::params![record.session_id, record.pet_id, record.candidate_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if status.as_deref() != Some("finalizing") {
            return Err("creation session changed while runtime assets were compiling".into());
        }
        let affected = tx
            .execute(
                "INSERT INTO variants (variant_id, pet_id, style_id, manifest_path, created_at)
                 VALUES (?1, ?2, 'animated-image-v1', ?3, ?4)
                 ON CONFLICT(variant_id) DO UPDATE SET manifest_path=excluded.manifest_path
                 WHERE variants.pet_id=excluded.pet_id",
                rusqlite::params![
                    record.candidate_id,
                    record.pet_id,
                    manifest_path,
                    crate::creation::profiles::now_iso()
                ],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("runtime variant belongs to a different pet".into());
        }
        tx.commit().map_err(|error| error.to_string())
    }

    fn compensate_failed_prepare(
        &self,
        record: &FinalizationRecord,
        error: &str,
    ) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|db_error| db_error.to_string())?;
        tx.execute(
            "DELETE FROM variants
             WHERE variant_id=?1 AND pet_id=?2
               AND EXISTS (SELECT 1 FROM appearance_variants
                           WHERE variant_id=?1 AND session_id=?3 AND accepted=0)",
            rusqlite::params![record.candidate_id, record.pet_id, record.session_id],
        )
        .map_err(|db_error| db_error.to_string())?;
        tx.execute(
            "UPDATE creation_sessions
             SET status='retryableFailure', last_stable_status='candidateReady',
                 current_step='review', error=?2, updated_at=?3
             WHERE session_id=?1 AND pet_id=?4 AND status!='completed' AND status!='abandoned'
               AND EXISTS (SELECT 1 FROM pets p
                           WHERE p.pet_id=?4 AND p.lifecycle='draft' AND p.completed_at IS NULL)
               AND NOT EXISTS (SELECT 1 FROM state
                               WHERE key='app:active_pet_id' AND value=?4)",
            rusqlite::params![
                record.session_id,
                error,
                crate::creation::profiles::now_iso(),
                record.pet_id
            ],
        )
        .map_err(|db_error| db_error.to_string())?;
        tx.commit().map_err(|db_error| db_error.to_string())?;
        drop(storage);
        self.remove_owned_install(record)?;
        Ok(())
    }

    fn clean_interrupted_database(&self, record: &FinalizationRecord) -> Result<bool, String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let removed = tx
            .execute(
                "DELETE FROM variants
                 WHERE variant_id=?1 AND pet_id=?2
                   AND EXISTS (SELECT 1 FROM appearance_variants
                               WHERE variant_id=?1 AND session_id=?3 AND pet_id=?2 AND accepted=0)",
                rusqlite::params![record.candidate_id, record.pet_id, record.session_id],
            )
            .map_err(|error| error.to_string())?;
        let updated = tx
            .execute(
                "UPDATE creation_sessions
                 SET status=CASE WHEN status='candidateReady' THEN 'candidateReady'
                                 ELSE 'retryableFailure' END,
                     last_stable_status='candidateReady',
                     current_step='review',
                     error=CASE WHEN status='finalizing' THEN 'recovered interrupted finalization'
                                ELSE COALESCE(error, 'recovered incomplete finalization cleanup') END,
                     updated_at=?2
                 WHERE session_id=?1 AND pet_id=?3
                   AND status IN ('candidateReady','finalizing','retryableFailure')
                   AND EXISTS (SELECT 1 FROM pets p
                               WHERE p.pet_id=?3 AND p.lifecycle='draft' AND p.completed_at IS NULL)
                   AND NOT EXISTS (SELECT 1 FROM state
                                   WHERE key='app:active_pet_id' AND value=?3)",
                rusqlite::params![
                    record.session_id,
                    crate::creation::profiles::now_iso(),
                    record.pet_id
                ],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("creation session changed during recovery".into());
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(removed == 1)
    }

    fn install_is_owned(&self, record: &FinalizationRecord) -> Result<bool, String> {
        let assets_dir = self.assets_dir(&record.pet_id);
        if !assets_dir.exists() {
            return Ok(false);
        }
        let metadata = std::fs::symlink_metadata(&assets_dir).map_err(|error| error.to_string())?;
        if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            return Err("pet assets are not a regular owned directory".into());
        }
        let manifest_path = assets_dir.join("manifest.json");
        let manifest_metadata = std::fs::symlink_metadata(&manifest_path)
            .map_err(|_| "existing pet assets have no ownership manifest".to_string())?;
        if crate::platform::is_link_or_reparse_point(&manifest_metadata)
            || !manifest_metadata.is_file()
        {
            return Err("pet asset ownership manifest is not a regular file".into());
        }
        let raw = std::fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?;
        let manifest = crate::runtime_assets::manifest::parse_manifest(&raw)
            .map_err(|error| format!("pet asset ownership manifest is invalid: {error}"))?;
        if !matches!(
            &manifest,
            crate::runtime_assets::manifest::RuntimeAssetManifest::V3(_)
        ) {
            return Err("pet asset ownership manifest is not a finalization v3 manifest".into());
        }
        let (pet_id, variant_id) = crate::runtime_assets::manifest::manifest_identity(&manifest);
        if pet_id != record.pet_id || variant_id != record.candidate_id {
            return Err("existing pet assets belong to a different pet or candidate".into());
        }
        Ok(true)
    }

    fn remove_owned_install(&self, record: &FinalizationRecord) -> Result<(), String> {
        if !self.install_is_owned(record)? {
            return Ok(());
        }
        std::fs::remove_dir_all(self.assets_dir(&record.pet_id)).map_err(|error| error.to_string())
    }

    fn pet_dir(&self, pet_id: &str) -> PathBuf {
        self.app_data_dir.join("pets").join(pet_id)
    }

    fn assets_dir(&self, pet_id: &str) -> PathBuf {
        self.pet_dir(pet_id).join("assets")
    }
}

fn prepared(
    record: &FinalizationRecord,
    request_id: &str,
    already_completed: bool,
) -> PreparedCreation {
    PreparedCreation {
        request_id: request_id.into(),
        session_id: record.session_id.clone(),
        pet_id: record.pet_id.clone(),
        variant_id: record.candidate_id.clone(),
        already_completed,
    }
}

fn snapshot(record: &FinalizationRecord) -> Result<CreationSnapshot, String> {
    Ok(CreationSnapshot {
        session_id: record.session_id.clone(),
        pet_id: record.pet_id.clone(),
        method: parse_method(&record.method)?,
        status: parse_status(&record.status)?,
        last_stable_status: parse_status(&record.last_stable_status)?,
        current_step: record.current_step.clone(),
        display_name: record.display_name.clone(),
        job_id: record.job_id.clone(),
        job_status: record.job_status.clone(),
        candidate_id: Some(record.candidate_id.clone()),
        recipe: None,
        error: record.error.clone(),
    })
}

fn parse_method(value: &str) -> Result<CreationMethod, String> {
    match value {
        "upload" => Ok(CreationMethod::Upload),
        "composer" => Ok(CreationMethod::Composer),
        "adoption" => Ok(CreationMethod::Adoption),
        _ => Err(format!("unknown creation method: {value}")),
    }
}

fn parse_status(value: &str) -> Result<CreationSessionStatus, String> {
    match value {
        "draft" => Ok(CreationSessionStatus::Draft),
        "candidateReady" => Ok(CreationSessionStatus::CandidateReady),
        "finalizing" => Ok(CreationSessionStatus::Finalizing),
        "retryableFailure" => Ok(CreationSessionStatus::RetryableFailure),
        "completed" => Ok(CreationSessionStatus::Completed),
        "abandoned" => Ok(CreationSessionStatus::Abandoned),
        _ => Err(format!("unknown creation session status: {value}")),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::profiles;
    use crate::pets::mutation::{MutationKind, PetMutationGate};
    use crate::storage::Storage;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct FinalizationHarness {
        root: PathBuf,
        storage: Arc<Mutex<Storage>>,
        gate: Arc<PetMutationGate>,
        switch_transaction: SharedSwitchTransaction,
        service: CreationFinalizationService,
        session_id: String,
        pet_id: String,
        variant_id: String,
        body_path: PathBuf,
    }

    impl FinalizationHarness {
        fn candidate_ready() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let root = std::env::temp_dir().join(format!(
                "desktop-pet-finalization-{}-{n}",
                std::process::id()
            ));
            let pets_dir = root.join("pets");
            let jobs_root = root.join("jobs");
            let job_id = format!("job-{n}");
            let session_id = format!("session-{n}");
            let pet_id = format!("pet-{n}");
            let variant_id = format!("candidate-{n}");
            let job_dir = jobs_root.join(&job_id);
            std::fs::create_dir_all(&job_dir).unwrap();
            let raw_path = job_dir.join("raw.png");
            let body_path = job_dir.join("cutout.png");
            let profile_path = job_dir.join("motion-profile.json");
            write_png(&raw_path);
            write_png(&body_path);
            let rgba = image::open(&body_path).unwrap().to_rgba8();
            let profile =
                crate::runtime_assets::motion_profile::generate_motion_profile(&rgba).unwrap();
            crate::runtime_assets::motion_profile::write_motion_profile_atomic(
                &profile_path,
                &profile,
            )
            .unwrap();

            let storage = Arc::new(Mutex::new(Storage::open(&pets_dir).unwrap()));
            let now = profiles::now_iso();
            storage
                .lock()
                .unwrap()
                .db
                .execute_batch(&format!(
                    "INSERT INTO pets
                     (pet_id, schema_version, species, identity_mode, display_name,
                      creation_method, lifecycle, created_at, updated_at)
                     VALUES ('{pet_id}', 1, 'cat', 'realPet', '奶糖', 'upload', 'draft', '{now}', '{now}');
                     INSERT INTO creation_sessions
                     (session_id, pet_id, method, status, last_stable_status, current_step,
                      schema_version, created_at, updated_at)
                     VALUES ('{session_id}', '{pet_id}', 'upload', 'candidateReady',
                             'candidateReady', 'review', 1, '{now}', '{now}');
                     INSERT INTO generation_jobs
                     (job_id, pet_id, session_id, prompt, ref_sha256, status, created_at)
                     VALUES ('{job_id}', '{pet_id}', '{session_id}', 'prompt', 'hash', 'success', '{now}');"
                ))
                .unwrap();
            storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO appearance_variants
                     (variant_id, pet_id, job_id, session_id, image_path, cutout_path,
                      motion_profile_path, quality, accepted, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'acceptable', 0, ?8)",
                    rusqlite::params![
                        variant_id,
                        pet_id,
                        job_id,
                        session_id,
                        raw_path.to_string_lossy(),
                        body_path.to_string_lossy(),
                        profile_path.to_string_lossy(),
                        now
                    ],
                )
                .unwrap();
            let gate = Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60)));
            let switch_transaction = Arc::new(Mutex::new(()));
            let service = CreationFinalizationService::new(
                storage.clone(),
                root.clone(),
                jobs_root,
                gate.clone(),
                switch_transaction.clone(),
            );
            Self {
                root,
                storage,
                gate,
                switch_transaction,
                service,
                session_id,
                pet_id,
                variant_id,
                body_path,
            }
        }

        fn status(&self) -> (String, String, Option<String>) {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT status, last_stable_status, error FROM creation_sessions
                     WHERE session_id=?1",
                    [&self.session_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap()
        }

        fn runtime_variant_count(&self) -> i64 {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT COUNT(*) FROM variants WHERE variant_id=?1 AND pet_id=?2",
                    rusqlite::params![self.variant_id, self.pet_id],
                    |row| row.get(0),
                )
                .unwrap()
        }

        fn assert_gate_is_free(&self, request_id: &str) {
            self.gate
                .begin(request_id, MutationKind::Delete, "pet-other")
                .unwrap();
            self.gate.finish(request_id).unwrap();
        }

        fn assets(&self) -> PathBuf {
            self.root.join("pets").join(&self.pet_id).join("assets")
        }

        fn complete(&self) {
            let manifest = self.assets().join("manifest.json");
            compile_animated_image(
                &self.pet_id,
                &self.variant_id,
                &self.body_path,
                &self.body_path.parent().unwrap().join("motion-profile.json"),
                &self.assets(),
            )
            .unwrap();
            self.storage
                .lock()
                .unwrap()
                .db
                .execute_batch(&format!(
                    "INSERT INTO variants (variant_id, pet_id, style_id, manifest_path, created_at)
                     VALUES ('{}', '{}', 'signature-cartoon-v1', '{}', '0');
                     UPDATE appearance_variants SET accepted=1 WHERE variant_id='{}';
                     UPDATE pets SET lifecycle='ready', completed_at='0' WHERE pet_id='{}';
                     UPDATE creation_sessions SET status='completed', last_stable_status='completed',
                       current_step='completed', completed_at='0' WHERE session_id='{}';",
                    self.variant_id,
                    self.pet_id,
                    manifest.to_string_lossy().replace('\\', "\\\\"),
                    self.variant_id,
                    self.pet_id,
                    self.session_id,
                ))
                .unwrap();
        }
    }

    impl Drop for FinalizationHarness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn write_png(path: &Path) {
        image::RgbaImage::from_pixel(64, 64, image::Rgba([80, 90, 100, 255]))
            .save(path)
            .unwrap();
    }

    #[test]
    fn prepare_compiles_only_the_stored_candidate_and_marks_finalizing() {
        let test = FinalizationHarness::candidate_ready();

        let prepared = test.service.prepare(&test.session_id, "req-1").unwrap();

        assert_eq!(prepared.pet_id, test.pet_id);
        assert_eq!(prepared.variant_id, test.variant_id);
        assert!(test.assets().join("manifest.json").exists());
        assert_eq!(test.status().0, "finalizing");
        assert!(test
            .gate
            .begin("req-other", MutationKind::Switch, "pet-other")
            .is_err());
        test.gate.finish("req-1").unwrap();
    }

    #[test]
    fn abort_removes_installed_assets_but_keeps_source_candidate() {
        let test = FinalizationHarness::candidate_ready();
        test.service.prepare(&test.session_id, "req-abort").unwrap();

        for _ in 0..2 {
            test.service
                .abort(&test.session_id, "desktop unavailable")
                .unwrap();
        }

        assert!(!test.assets().exists());
        assert!(test.body_path.exists());
        assert_eq!(
            test.status(),
            (
                "retryableFailure".into(),
                "candidateReady".into(),
                Some("desktop unavailable".into())
            )
        );
        test.gate.finish("req-abort").unwrap();
    }

    #[test]
    fn preparing_a_completed_session_is_idempotent_without_taking_the_gate() {
        let test = FinalizationHarness::candidate_ready();
        test.complete();

        let prepared = test
            .service
            .prepare(&test.session_id, "req-completed")
            .unwrap();

        assert!(prepared.already_completed);
        assert_eq!(prepared.pet_id, test.pet_id);
        assert_eq!(prepared.variant_id, test.variant_id);
        test.gate
            .begin("req-free", MutationKind::Delete, "pet-other")
            .unwrap();
        test.gate.finish("req-free").unwrap();
    }

    #[test]
    fn completed_migration_selects_the_only_accepted_runtime_among_historical_candidates() {
        let test = FinalizationHarness::candidate_ready();
        test.complete();
        let historical_id = format!("{}-history", test.variant_id);
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO appearance_variants
                 (variant_id, pet_id, job_id, session_id, image_path, cutout_path,
                  motion_profile_path, quality, accepted, created_at)
                 SELECT ?1, pet_id, job_id, session_id, image_path, cutout_path,
                        motion_profile_path, quality, 0, 'history'
                 FROM appearance_variants WHERE variant_id=?2",
                rusqlite::params![historical_id, test.variant_id],
            )
            .unwrap();

        let prepared = test
            .service
            .prepare(&test.session_id, "req-completed-history")
            .unwrap();
        let snapshot = test.service.abort(&test.session_id, "late retry").unwrap();

        assert!(prepared.already_completed);
        assert_eq!(prepared.variant_id, test.variant_id);
        assert_eq!(snapshot.status, CreationSessionStatus::Completed);
        assert!(test.assets().exists());
        test.assert_gate_is_free("req-after-completed-history");
    }

    #[test]
    fn completed_migration_is_idempotent_without_generation_job_or_source_candidate_files() {
        let test = FinalizationHarness::candidate_ready();
        test.complete();
        let raw = test.body_path.parent().unwrap().join("raw.png");
        let profile = test.body_path.parent().unwrap().join("motion-profile.json");
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE appearance_variants SET job_id=NULL WHERE variant_id=?1",
                [&test.variant_id],
            )
            .unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "DELETE FROM generation_jobs WHERE session_id=?1",
                [&test.session_id],
            )
            .unwrap();
        std::fs::remove_file(raw).unwrap();
        std::fs::remove_file(&test.body_path).unwrap();
        std::fs::remove_file(profile).unwrap();

        let prepared = test
            .service
            .prepare(&test.session_id, "req-completed-migrated")
            .unwrap();
        let snapshot = test
            .service
            .abort(&test.session_id, "late timeout")
            .unwrap();

        assert!(prepared.already_completed);
        assert_eq!(prepared.variant_id, test.variant_id);
        assert_eq!(snapshot.status, CreationSessionStatus::Completed);
        assert!(test.assets().exists());
        assert_eq!(test.runtime_variant_count(), 1);
        test.assert_gate_is_free("req-after-completed-migrated");
    }

    #[test]
    fn candidate_ready_migration_selects_the_latest_successful_upload_candidate() {
        let test = FinalizationHarness::candidate_ready();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE appearance_variants SET created_at='middle' WHERE variant_id=?1",
                [&test.variant_id],
            )
            .unwrap();
        for (suffix, status, created_at) in [
            ("older-success", "success", "earlier"),
            ("newer-failed", "failed", "later"),
        ] {
            let job_id = format!("job-{suffix}");
            let variant_id = format!("candidate-{suffix}");
            let job_dir = test.root.join("jobs").join(&job_id);
            std::fs::create_dir_all(&job_dir).unwrap();
            let raw = job_dir.join("raw.png");
            let body = job_dir.join("cutout.png");
            let profile = job_dir.join("motion-profile.json");
            std::fs::copy(&test.body_path, &raw).unwrap();
            std::fs::copy(&test.body_path, &body).unwrap();
            std::fs::copy(
                test.body_path.parent().unwrap().join("motion-profile.json"),
                &profile,
            )
            .unwrap();
            test.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO generation_jobs
                     (job_id, pet_id, session_id, prompt, ref_sha256, status, created_at)
                     VALUES (?1, ?2, ?3, 'migration', 'hash', ?4, ?5)",
                    rusqlite::params![job_id, test.pet_id, test.session_id, status, created_at],
                )
                .unwrap();
            test.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO appearance_variants
                     (variant_id, pet_id, job_id, session_id, image_path, cutout_path,
                      motion_profile_path, quality, accepted, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'acceptable', 0, ?8)",
                    rusqlite::params![
                        variant_id,
                        test.pet_id,
                        job_id,
                        test.session_id,
                        raw.to_string_lossy(),
                        body.to_string_lossy(),
                        profile.to_string_lossy(),
                        created_at
                    ],
                )
                .unwrap();
        }

        let prepared = test
            .service
            .prepare(&test.session_id, "req-migrated-latest-success")
            .unwrap();

        assert_eq!(prepared.variant_id, test.variant_id);
        assert_eq!(test.status().0, "finalizing");
        test.gate.finish("req-migrated-latest-success").unwrap();
    }

    #[test]
    fn compile_failure_restores_retryable_candidate_and_releases_exact_owner() {
        let test = FinalizationHarness::candidate_ready();
        std::fs::write(&test.body_path, b"not an image").unwrap();

        assert!(test
            .service
            .prepare(&test.session_id, "req-compile")
            .is_err());

        assert_eq!(test.status().0, "retryableFailure");
        assert_eq!(test.status().1, "candidateReady");
        assert_eq!(test.runtime_variant_count(), 0);
        assert!(!test.assets().exists());
        assert!(test.body_path.exists());
        test.assert_gate_is_free("req-after-compile");
    }

    #[test]
    fn installer_failure_preserves_non_owned_blocker_and_releases_exact_owner() {
        let test = FinalizationHarness::candidate_ready();
        let pet_install = test.root.join("pets").join(&test.pet_id);
        std::fs::write(&pet_install, b"blocks the install directory").unwrap();

        assert!(test
            .service
            .prepare(&test.session_id, "req-installer")
            .is_err());

        assert_eq!(test.status().0, "retryableFailure");
        assert_eq!(test.status().1, "candidateReady");
        assert_eq!(test.runtime_variant_count(), 0);
        assert_eq!(
            std::fs::read_to_string(&pet_install).unwrap(),
            "blocks the install directory"
        );
        assert!(test.body_path.exists());
        test.assert_gate_is_free("req-after-installer");
    }

    #[test]
    fn runtime_variant_database_failure_removes_only_this_install_and_releases_owner() {
        let test = FinalizationHarness::candidate_ready();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "CREATE TRIGGER fail_runtime_variant BEFORE INSERT ON variants
                 BEGIN SELECT RAISE(ABORT, 'forced runtime variant failure'); END;",
            )
            .unwrap();

        let error = test
            .service
            .prepare(&test.session_id, "req-runtime-db")
            .unwrap_err();

        assert!(error.contains("forced runtime variant failure"));
        assert_eq!(test.status().0, "retryableFailure");
        assert_eq!(test.runtime_variant_count(), 0);
        assert!(!test.assets().exists());
        assert!(test.body_path.exists());
        test.assert_gate_is_free("req-after-runtime-db");
    }

    #[test]
    fn repeated_prepare_reuses_the_installed_runtime_without_replacing_it() {
        let test = FinalizationHarness::candidate_ready();
        test.service
            .prepare(&test.session_id, "req-repeat")
            .unwrap();
        std::fs::write(test.assets().join("runtime-sentinel.txt"), "same install").unwrap();

        let repeated = test
            .service
            .prepare(&test.session_id, "req-repeat")
            .unwrap();

        assert!(!repeated.already_completed);
        assert_eq!(repeated.variant_id, test.variant_id);
        assert_eq!(
            std::fs::read_to_string(test.assets().join("runtime-sentinel.txt")).unwrap(),
            "same install"
        );
        test.gate.finish("req-repeat").unwrap();
    }

    #[test]
    fn recover_cleans_a_candidate_ready_orphan_left_between_rollback_and_abort() {
        let test = FinalizationHarness::candidate_ready();
        test.service
            .prepare(&test.session_id, "req-rollback-crash")
            .unwrap();
        test.gate.finish("req-rollback-crash").unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions
                 SET status='candidateReady', last_stable_status='candidateReady',
                     current_step='review', error='runtime switch rolled back'
                 WHERE session_id=?1",
                [&test.session_id],
            )
            .unwrap();

        let report = test.service.recover().unwrap();

        assert_eq!(report.retryable_session_ids, vec![test.session_id.clone()]);
        assert_eq!(report.cleaned_session_ids, vec![test.session_id.clone()]);
        assert_eq!(test.status().0, "candidateReady");
        assert_eq!(test.status().1, "candidateReady");
        assert_eq!(test.runtime_variant_count(), 0);
        assert!(!test.assets().exists());
        assert!(test.body_path.exists());
    }

    #[test]
    fn a_different_request_cannot_reprepare_the_same_finalizing_session() {
        let test = FinalizationHarness::candidate_ready();
        test.service.prepare(&test.session_id, "req-owner").unwrap();

        assert!(test
            .service
            .prepare(&test.session_id, "req-contender")
            .unwrap_err()
            .contains("进行"));
        assert_eq!(test.status().0, "finalizing");
        assert!(test.assets().join("manifest.json").exists());
        test.gate.finish("req-owner").unwrap();
    }

    #[test]
    fn path_escape_is_rejected_before_the_gate_or_session_state_changes() {
        let test = FinalizationHarness::candidate_ready();
        let outside = test.root.join("outside").join("cutout.png");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        write_png(&outside);
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE appearance_variants SET cutout_path=?2 WHERE variant_id=?1",
                rusqlite::params![test.variant_id, outside.to_string_lossy()],
            )
            .unwrap();

        assert!(test
            .service
            .prepare(&test.session_id, "req-escape")
            .unwrap_err()
            .contains("authoritative directory"));
        assert_eq!(test.status().0, "candidateReady");
        test.assert_gate_is_free("req-after-escape");
    }

    #[test]
    fn migrated_ready_pet_is_never_prepared_aborted_or_recovered_as_a_draft() {
        let test = FinalizationHarness::candidate_ready();
        let sentinel = test.assets().join("existing-ready-runtime.txt");
        std::fs::create_dir_all(test.assets()).unwrap();
        std::fs::write(&sentinel, "ready runtime").unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE pets SET lifecycle='ready', completed_at='migrated' WHERE pet_id=?1",
                [&test.pet_id],
            )
            .unwrap();

        assert!(test
            .service
            .prepare(&test.session_id, "req-ready-migration")
            .is_err());
        assert!(test.service.abort(&test.session_id, "late abort").is_err());
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "ready runtime");
        test.assert_gate_is_free("req-after-ready-migration");

        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions SET status='finalizing' WHERE session_id=?1",
                [&test.session_id],
            )
            .unwrap();
        let report = test.service.recover().unwrap();
        assert!(report.cleaned_session_ids.is_empty());
        assert_eq!(report.warnings.len(), 1);
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "ready runtime");
    }

    #[test]
    fn active_draft_pet_is_rejected_before_compilation_or_gate_ownership() {
        let test = FinalizationHarness::candidate_ready();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO state (key, value) VALUES ('app:active_pet_id', ?1)",
                [&test.pet_id],
            )
            .unwrap();

        assert!(test
            .service
            .prepare(&test.session_id, "req-active-draft")
            .is_err());
        assert_eq!(test.status().0, "candidateReady");
        assert!(!test.assets().exists());
        test.assert_gate_is_free("req-after-active-draft");
    }

    #[test]
    fn non_owned_existing_assets_are_never_replaced_or_removed() {
        let test = FinalizationHarness::candidate_ready();
        let sentinel = test.assets().join("unrelated.txt");
        std::fs::create_dir_all(test.assets()).unwrap();
        std::fs::write(&sentinel, "not this attempt").unwrap();

        assert!(test
            .service
            .prepare(&test.session_id, "req-unowned-install")
            .is_err());

        assert_eq!(
            std::fs::read_to_string(&sentinel).unwrap(),
            "not this attempt"
        );
        assert_eq!(test.status().0, "candidateReady");
        assert_eq!(test.runtime_variant_count(), 0);
        test.assert_gate_is_free("req-after-unowned-install");
    }

    #[test]
    fn adoption_candidate_without_a_generation_job_can_finalize_from_its_session_ownership() {
        let test = FinalizationHarness::candidate_ready();
        let candidate_dir = test
            .root
            .join("creation-sessions")
            .join(&test.session_id)
            .join("candidate");
        std::fs::create_dir_all(&candidate_dir).unwrap();
        let body = candidate_dir.join("body.png");
        let profile = candidate_dir.join("motion-profile.json");
        std::fs::copy(&test.body_path, &body).unwrap();
        std::fs::copy(
            test.body_path.parent().unwrap().join("motion-profile.json"),
            &profile,
        )
        .unwrap();
        {
            let storage = test.storage.lock().unwrap();
            storage
                .db
                .execute(
                    "UPDATE appearance_variants
                     SET job_id=NULL, image_path=?2, cutout_path=?2, motion_profile_path=?3
                     WHERE variant_id=?1",
                    rusqlite::params![
                        test.variant_id,
                        body.to_string_lossy(),
                        profile.to_string_lossy()
                    ],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "UPDATE creation_sessions SET method='adoption' WHERE session_id=?1",
                    [&test.session_id],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "UPDATE pets
                     SET identity_mode='adopted', creation_method='adoption',
                         source_template_id='template-1', source_template_version=1
                     WHERE pet_id=?1",
                    [&test.pet_id],
                )
                .unwrap();
        }

        let prepared = test
            .service
            .prepare(&test.session_id, "req-adoption")
            .unwrap();

        assert_eq!(prepared.pet_id, test.pet_id);
        assert!(test.assets().join("manifest.json").exists());
        test.gate.finish("req-adoption").unwrap();
    }

    #[test]
    fn composer_candidate_cannot_mix_in_an_upload_generation_job() {
        let test = FinalizationHarness::candidate_ready();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions SET method='composer' WHERE session_id=?1",
                [&test.session_id],
            )
            .unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE pets SET creation_method='composer', identity_mode='guided' WHERE pet_id=?1",
                [&test.pet_id],
            )
            .unwrap();

        assert!(test
            .service
            .prepare(&test.session_id, "req-mixed-method")
            .is_err());
        assert_eq!(test.status().0, "candidateReady");
        test.assert_gate_is_free("req-after-mixed-method");
    }

    #[test]
    fn no_job_candidate_cannot_read_another_creation_session_directory() {
        let test = FinalizationHarness::candidate_ready();
        let other = test.root.join("creation-sessions/session-other/candidate");
        std::fs::create_dir_all(&other).unwrap();
        let body = other.join("body.png");
        let profile = other.join("motion-profile.json");
        std::fs::copy(&test.body_path, &body).unwrap();
        std::fs::copy(
            test.body_path.parent().unwrap().join("motion-profile.json"),
            &profile,
        )
        .unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE appearance_variants
                 SET job_id=NULL, image_path=?2, cutout_path=?2, motion_profile_path=?3
                 WHERE variant_id=?1",
                rusqlite::params![
                    test.variant_id,
                    body.to_string_lossy(),
                    profile.to_string_lossy()
                ],
            )
            .unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(&format!(
                "UPDATE creation_sessions SET method='adoption' WHERE session_id='{}';
                 UPDATE pets SET creation_method='adoption', identity_mode='adopted',
                    source_template_id='template-other-session', source_template_version=1
                 WHERE pet_id='{}';",
                test.session_id, test.pet_id
            ))
            .unwrap();

        assert!(test
            .service
            .prepare(&test.session_id, "req-other-session-path")
            .is_err());
        assert_eq!(test.status().0, "candidateReady");
        test.assert_gate_is_free("req-after-other-session-path");
    }

    #[test]
    fn no_job_candidate_rejects_an_intermediate_session_directory_alias() {
        let test = FinalizationHarness::candidate_ready();
        let sessions_root = test.root.join("creation-sessions");
        let other_session = sessions_root.join("session-alias-target");
        let other_candidate = other_session.join("candidate");
        std::fs::create_dir_all(&other_candidate).unwrap();
        let real_body = other_candidate.join("body.png");
        let real_profile = other_candidate.join("motion-profile.json");
        std::fs::copy(&test.body_path, &real_body).unwrap();
        std::fs::copy(
            test.body_path.parent().unwrap().join("motion-profile.json"),
            &real_profile,
        )
        .unwrap();
        let session_alias = sessions_root.join(&test.session_id);
        crate::platform::create_directory_link(&other_session, &session_alias);
        let aliased_body = session_alias.join("candidate/body.png");
        let aliased_profile = session_alias.join("candidate/motion-profile.json");
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE appearance_variants
                 SET job_id=NULL, image_path=?2, cutout_path=?2, motion_profile_path=?3
                 WHERE variant_id=?1",
                rusqlite::params![
                    test.variant_id,
                    aliased_body.to_string_lossy(),
                    aliased_profile.to_string_lossy()
                ],
            )
            .unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(&format!(
                "UPDATE creation_sessions SET method='adoption' WHERE session_id='{}';
                 UPDATE pets SET creation_method='adoption', identity_mode='adopted',
                    source_template_id='template-session-alias', source_template_version=1
                 WHERE pet_id='{}';",
                test.session_id, test.pet_id
            ))
            .unwrap();

        assert!(test
            .service
            .prepare(&test.session_id, "req-session-alias")
            .unwrap_err()
            .contains("link or reparse point"));
        assert_eq!(test.status().0, "candidateReady");
        assert!(real_body.exists());
        test.assert_gate_is_free("req-after-session-alias");
    }

    #[test]
    fn aborting_a_completed_session_is_a_noop() {
        let test = FinalizationHarness::candidate_ready();
        test.complete();

        let snapshot = test
            .service
            .abort(&test.session_id, "late desktop timeout")
            .unwrap();

        assert_eq!(snapshot.status, CreationSessionStatus::Completed);
        assert!(test.assets().exists());
        assert_eq!(test.runtime_variant_count(), 1);
    }

    #[test]
    fn abort_waits_for_creation_commit_and_preserves_the_completed_fact() {
        let test = FinalizationHarness::candidate_ready();
        let commit_guard = test.switch_transaction.lock().unwrap();
        let (result_tx, result_rx) = std::sync::mpsc::channel();

        std::thread::scope(|scope| {
            scope.spawn(|| {
                result_tx
                    .send(test.service.abort(&test.session_id, "late timeout"))
                    .unwrap();
            });
            assert!(result_rx
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err());
            test.complete();
            drop(commit_guard);

            let snapshot = result_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .unwrap();
            assert_eq!(snapshot.status, CreationSessionStatus::Completed);
        });

        assert!(test.assets().exists());
        assert_eq!(test.runtime_variant_count(), 1);
    }

    #[test]
    fn recover_cleans_only_the_interrupted_finalizing_session() {
        let test = FinalizationHarness::candidate_ready();
        test.service
            .prepare(&test.session_id, "req-crashed")
            .unwrap();
        test.gate.finish("req-crashed").unwrap();

        let report = test.service.recover().unwrap();

        assert_eq!(report.retryable_session_ids, vec![test.session_id.clone()]);
        assert_eq!(report.cleaned_session_ids, vec![test.session_id.clone()]);
        assert_eq!(test.status().0, "retryableFailure");
        assert_eq!(test.runtime_variant_count(), 0);
        assert!(!test.assets().exists());
        assert!(test.body_path.exists());
    }

    #[test]
    fn recover_handles_a_crash_after_marking_finalizing_before_install() {
        let test = FinalizationHarness::candidate_ready();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions
                 SET status='finalizing', current_step='finalizing'
                 WHERE session_id=?1",
                [&test.session_id],
            )
            .unwrap();

        let report = test.service.recover().unwrap();

        assert_eq!(report.retryable_session_ids, vec![test.session_id.clone()]);
        assert!(report.cleaned_session_ids.is_empty());
        assert_eq!(test.status().0, "retryableFailure");
        assert_eq!(test.runtime_variant_count(), 0);
        assert!(!test.assets().exists());
        assert!(test.body_path.exists());
    }

    #[test]
    fn recover_removes_an_owned_install_left_before_runtime_database_recording() {
        let test = FinalizationHarness::candidate_ready();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions
                 SET status='finalizing', current_step='finalizing'
                 WHERE session_id=?1",
                [&test.session_id],
            )
            .unwrap();
        compile_animated_image(
            &test.pet_id,
            &test.variant_id,
            &test.body_path,
            &test.body_path.parent().unwrap().join("motion-profile.json"),
            &test.assets(),
        )
        .unwrap();

        let report = test.service.recover().unwrap();

        assert_eq!(report.cleaned_session_ids, vec![test.session_id.clone()]);
        assert_eq!(test.status().0, "retryableFailure");
        assert_eq!(test.runtime_variant_count(), 0);
        assert!(!test.assets().exists());
        assert!(test.body_path.exists());
    }

    #[test]
    fn recover_reports_completed_sessions_without_cleaning_their_assets() {
        let test = FinalizationHarness::candidate_ready();
        test.complete();

        let report = test.service.recover().unwrap();

        assert_eq!(report.completed_session_ids, vec![test.session_id.clone()]);
        assert!(report.retryable_session_ids.is_empty());
        assert!(report.cleaned_session_ids.is_empty());
        assert!(test.assets().exists());
        assert_eq!(test.runtime_variant_count(), 1);
    }

    #[test]
    fn recover_ignores_retryable_sessions_that_were_not_finalizing_candidates() {
        let test = FinalizationHarness::candidate_ready();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions
                 SET status='retryableFailure', last_stable_status='draft', error='generation failed'
                 WHERE session_id=?1",
                [&test.session_id],
            )
            .unwrap();

        let report = test.service.recover().unwrap();

        assert!(report.retryable_session_ids.is_empty());
        assert!(report.cleaned_session_ids.is_empty());
        assert_eq!(test.status().1, "draft");
        assert_eq!(test.status().2.as_deref(), Some("generation failed"));
    }
}
