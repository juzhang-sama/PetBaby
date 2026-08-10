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
    error: Option<String>,
    candidate_id: String,
    job_id: Option<String>,
    job_status: Option<String>,
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
        self.remove_pet_install(&record.pet_id)?;
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
                    "SELECT session_id, status FROM creation_sessions
                     WHERE status IN ('finalizing','completed')
                        OR (status='retryableFailure' AND last_stable_status='candidateReady')
                     ORDER BY created_at, session_id",
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
                let had_install = self.pet_dir(&record.pet_id).exists();
                let removed_runtime = self.clean_interrupted_database(&record)?;
                self.remove_pet_install(&record.pet_id)?;
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
        let candidate_count: i64 = storage
            .db
            .query_row(
                "SELECT COUNT(*) FROM appearance_variants av
                 JOIN creation_sessions cs ON cs.session_id=av.session_id
                 WHERE av.session_id=?1 AND av.pet_id=cs.pet_id",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if candidate_count != 1 {
            return Err("creation session must have exactly one authoritative candidate".into());
        }
        storage
            .db
            .query_row(
                "SELECT cs.session_id, cs.pet_id, cs.method, cs.status, cs.last_stable_status,
                        cs.current_step, p.display_name, cs.error,
                        av.variant_id, av.job_id, av.image_path, av.cutout_path,
                        av.motion_profile_path, av.accepted, rv.pet_id, rv.manifest_path,
                        gj.pet_id, gj.session_id, gj.status
                 FROM creation_sessions cs
                 JOIN pets p ON p.pet_id=cs.pet_id
                 JOIN appearance_variants av ON av.session_id=cs.session_id AND av.pet_id=cs.pet_id
                 LEFT JOIN variants rv ON rv.variant_id=av.variant_id
                 LEFT JOIN generation_jobs gj ON gj.job_id=av.job_id
                 WHERE cs.session_id=?1",
                [session_id],
                |row| {
                    let method: String = row.get(2)?;
                    let candidate_job_id: Option<String> = row.get(9)?;
                    let job_pet_id: Option<String> = row.get(16)?;
                    let job_session_id: Option<String> = row.get(17)?;
                    let job_status: Option<String> = row.get(18)?;
                    let session_pet_id: String = row.get(1)?;
                    let stored_session_id: String = row.get(0)?;
                    let job_is_required = method == "upload" || candidate_job_id.is_some();
                    if job_is_required
                        && (job_pet_id.as_deref() != Some(session_pet_id.as_str())
                            || job_session_id.as_deref() != Some(stored_session_id.as_str())
                            || job_status.as_deref() != Some("success"))
                    {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    Ok(FinalizationRecord {
                        session_id: stored_session_id,
                        pet_id: session_pet_id,
                        method,
                        status: row.get(3)?,
                        last_stable_status: row.get(4)?,
                        current_step: row.get(5)?,
                        display_name: row.get(6)?,
                        error: row.get(7)?,
                        candidate_id: row.get(8)?,
                        job_id: candidate_job_id,
                        job_status,
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
        let root_metadata = std::fs::symlink_metadata(&self.jobs_root)
            .map_err(|error| format!("configured jobs root is missing: {error}"))?;
        if crate::platform::is_link_or_reparse_point(&root_metadata) || !root_metadata.is_dir() {
            return Err("configured jobs root cannot be a link or reparse point".into());
        }
        let canonical_root = self
            .jobs_root
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let job_directory = match &record.job_id {
            Some(job_id) => job_id.as_str(),
            None => record
                .body_path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .ok_or("candidate path has no standard job directory")?,
        };
        validate_component(job_directory, "job directory")?;
        let expected_job_dir = canonical_root.join(job_directory);
        for (path, file_name) in [
            (&record.raw_path, "raw.png"),
            (&record.body_path, "cutout.png"),
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
            let parent = path.parent().ok_or("candidate path has no job directory")?;
            let parent_metadata = std::fs::symlink_metadata(parent)
                .map_err(|error| format!("candidate job directory is missing: {error}"))?;
            if crate::platform::is_link_or_reparse_point(&parent_metadata)
                || !parent_metadata.is_dir()
            {
                return Err("candidate job directory cannot be a link or reparse point".into());
            }
            let canonical_parent = parent.canonicalize().map_err(|error| error.to_string())?;
            if canonical_parent != expected_job_dir {
                return Err("candidate path is outside the matching standard job directory".into());
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
                        OR (status='retryableFailure' AND last_stable_status='candidateReady'))",
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
                 WHERE cs.session_id=?1 AND cs.pet_id=?2 AND av.variant_id=?3 AND av.accepted=0",
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
        self.remove_pet_install(&record.pet_id)?;
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
                 SET status='retryableFailure', last_stable_status='candidateReady',
                     current_step='review',
                     error=CASE WHEN status='finalizing' THEN 'recovered interrupted finalization'
                                ELSE COALESCE(error, 'recovered incomplete finalization cleanup') END,
                     updated_at=?2
                 WHERE session_id=?1 AND pet_id=?3
                   AND status IN ('finalizing','retryableFailure')",
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

    fn remove_pet_install(&self, pet_id: &str) -> Result<(), String> {
        validate_component(pet_id, "pet id")?;
        let pet_dir = self.pet_dir(pet_id);
        if !pet_dir.exists() {
            return Ok(());
        }
        let metadata = std::fs::symlink_metadata(&pet_dir).map_err(|error| error.to_string())?;
        if crate::platform::is_link_or_reparse_point(&metadata) {
            return Err("pet install directory cannot be a link or reparse point".into());
        }
        if metadata.is_dir() {
            std::fs::remove_dir_all(pet_dir).map_err(|error| error.to_string())
        } else {
            std::fs::remove_file(pet_dir).map_err(|error| error.to_string())
        }
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
            std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
            std::fs::write(&manifest, "completed").unwrap();
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
    fn installer_failure_restores_retryable_candidate_and_releases_exact_owner() {
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
        assert!(!pet_install.exists());
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
            .contains("standard job directory"));
        assert_eq!(test.status().0, "candidateReady");
        test.assert_gate_is_free("req-after-escape");
    }

    #[test]
    fn adoption_candidate_without_a_generation_job_can_finalize_from_its_session_ownership() {
        let test = FinalizationHarness::candidate_ready();
        {
            let storage = test.storage.lock().unwrap();
            storage
                .db
                .execute(
                    "UPDATE appearance_variants SET job_id=NULL WHERE variant_id=?1",
                    [&test.variant_id],
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
