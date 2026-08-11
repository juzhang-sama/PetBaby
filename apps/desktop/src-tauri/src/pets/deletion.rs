use crate::creation::domain::new_entity_id;
use crate::pets::active::{SharedActivePetService, BUILTIN_PET_ID};
use crate::pets::mutation::{MutationKind, SharedPetMutationGate};
use crate::storage::Storage;
use rusqlite::{Connection, OptionalExtension};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
const JOURNAL_FILE: &str = "journal.json";
const PREVIOUS_JOURNAL_FILE: &str = "journal.previous.json";

pub type SharedPetDeletionService = Arc<PetDeletionService>;

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOutcome {
    pub warning: Option<String>,
}

pub struct PetDeletionService {
    storage: Arc<Mutex<Storage>>,
    active: SharedActivePetService,
    app_data_dir: PathBuf,
    mutation_gate: SharedPetMutationGate,
    journal_publish_ops: Arc<dyn JournalPublishOps>,
}

#[derive(Debug, Clone)]
struct QuarantinedPath {
    original: PathBuf,
    quarantined: PathBuf,
    original_parent: PathBuf,
    quarantine_parent: PathBuf,
}

#[derive(Debug)]
struct QuarantineMoveError {
    message: String,
    uncertain_path: Option<QuarantinedPath>,
}

impl std::fmt::Display for QuarantineMoveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum DeletionPhase {
    Prepared,
    Quarantined,
    Committed,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeletionJournal {
    pet_id: String,
    job_ids: Vec<String>,
    #[serde(default)]
    session_ids: Vec<String>,
    phase: DeletionPhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeletionPlan {
    job_ids: Vec<String>,
    session_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalPublishStep {
    StagingSynced,
    PreviousPublished,
    CurrentPublished,
}

trait JournalPublishOps: Send + Sync {
    fn durable_rename(&self, source: &Path, target: &Path) -> Result<(), String>;

    fn sync_existing_directory_entry(&self, root: &Path) -> Result<(), String> {
        crate::platform::sync_existing_directory_entry(root)
    }

    fn checkpoint(&self, _step: JournalPublishStep) -> Result<(), String> {
        Ok(())
    }
}

struct PlatformJournalPublishOps;

impl JournalPublishOps for PlatformJournalPublishOps {
    fn durable_rename(&self, source: &Path, target: &Path) -> Result<(), String> {
        crate::platform::durable_replace_file(source, target)
    }
}

impl PetDeletionService {
    pub fn new(
        storage: Arc<Mutex<Storage>>,
        active: SharedActivePetService,
        app_data_dir: PathBuf,
        mutation_gate: SharedPetMutationGate,
    ) -> Self {
        Self {
            storage,
            active,
            app_data_dir,
            mutation_gate,
            journal_publish_ops: Arc::new(PlatformJournalPublishOps),
        }
    }

    pub fn delete(&self, pet_id: &str) -> Result<DeleteOutcome, String> {
        validate_component(pet_id, "pet id")?;
        let request_id = new_entity_id("delete");
        let _operation = self
            .mutation_gate
            .scoped(&request_id, MutationKind::Delete, pet_id)?;
        if pet_id == BUILTIN_PET_ID {
            return Err("the built-in pet cannot be deleted".into());
        }

        let plan = self.require_deletable_pet(pet_id)?;
        let quarantine_root = self.quarantine_root();
        prepare_quarantine_root(
            &self.app_data_dir,
            &quarantine_root,
            self.journal_publish_ops.as_ref(),
        )?;
        let mut journal = DeletionJournal {
            pet_id: pet_id.into(),
            job_ids: plan.job_ids.clone(),
            session_ids: plan.session_ids.clone(),
            phase: DeletionPhase::Prepared,
        };
        write_journal_with_ops(
            &quarantine_root,
            &journal,
            self.journal_publish_ops.as_ref(),
        )?;
        let pets_root = self.app_data_dir.join("pets");
        let jobs_root = self.app_data_dir.join("jobs");
        let sessions_root = self.app_data_dir.join("creation-sessions");
        validate_owned_root(&self.app_data_dir, &pets_root)?;
        validate_owned_root(&self.app_data_dir, &jobs_root)?;
        validate_owned_root(&self.app_data_dir, &sessions_root)?;
        let mut planned_paths = vec![(pets_root.join(pet_id), pets_root, "pet".to_owned())];
        for job_id in &plan.job_ids {
            validate_component(job_id, "job id")?;
            planned_paths.push((
                jobs_root.join(job_id),
                jobs_root.clone(),
                format!("job-{job_id}"),
            ));
        }
        for session_id in &plan.session_ids {
            validate_component(session_id, "session id")?;
            planned_paths.push((
                sessions_root.join(session_id),
                sessions_root.clone(),
                format!("session-{session_id}"),
            ));
        }

        for (source, parent, _) in &planned_paths {
            validate_source_path(source, parent)?;
        }

        let mut quarantined = Vec::new();
        for (source, _, name) in &planned_paths {
            match quarantine_path_with_ops(
                source,
                &quarantine_root,
                name,
                self.journal_publish_ops.as_ref(),
            ) {
                Ok(Some(path)) => quarantined.push(path),
                Ok(None) => {}
                Err(error) => {
                    if let Some(path) = error.uncertain_path.clone() {
                        quarantined.push(path);
                    }
                    return Err(recover_uncommitted_with_ops(
                        &quarantine_root,
                        format!("failed to quarantine {}: {error}", source.display()),
                        &quarantined,
                        self.journal_publish_ops.as_ref(),
                    ));
                }
            }
        }

        if let Err(error) = self.delete_rows(pet_id, &plan.job_ids, &plan.session_ids) {
            return Err(recover_uncommitted_with_ops(
                &quarantine_root,
                error,
                &quarantined,
                self.journal_publish_ops.as_ref(),
            ));
        }

        journal.phase = DeletionPhase::Committed;
        let mut warnings = Vec::new();
        if let Err(error) = write_journal_with_ops(
            &quarantine_root,
            &journal,
            self.journal_publish_ops.as_ref(),
        ) {
            warnings.push(format!("could not record committed deletion: {error}"));
        }
        if let Err(error) = remove_operation(&quarantine_root) {
            warnings.push(format!("quarantine cleanup failed: {error}"));
        }
        Ok(DeleteOutcome {
            warning: (!warnings.is_empty()).then(|| warnings.join("; ")),
        })
    }

    pub fn abandon_creation(&self, session_id: &str) -> Result<(), String> {
        validate_component(session_id, "session id")?;
        let request_id = new_entity_id("abandon");
        let _operation =
            self.mutation_gate
                .scoped(&request_id, MutationKind::Delete, session_id)?;

        let Some((pet_id, method, status, job_ids)) = self.creation_abandon_plan(session_id)?
        else {
            return Ok(());
        };
        if status == "completed" {
            return Err("a completed creation session cannot be abandoned".into());
        }
        validate_component(&pet_id, "pet id")?;
        for job_id in &job_ids {
            validate_component(job_id, "job id")?;
        }

        let quarantine_root = self.quarantine_root();
        prepare_quarantine_root(
            &self.app_data_dir,
            &quarantine_root,
            self.journal_publish_ops.as_ref(),
        )?;
        let mut journal = DeletionJournal {
            pet_id: pet_id.clone(),
            job_ids: job_ids.clone(),
            session_ids: vec![session_id.into()],
            phase: DeletionPhase::Prepared,
        };
        write_journal_with_ops(
            &quarantine_root,
            &journal,
            self.journal_publish_ops.as_ref(),
        )?;

        let pets_root = self.app_data_dir.join("pets");
        let jobs_root = self.app_data_dir.join("jobs");
        let sessions_root = self.app_data_dir.join("creation-sessions");
        validate_owned_root(&self.app_data_dir, &pets_root)?;
        validate_owned_root(&self.app_data_dir, &jobs_root)?;
        validate_owned_root(&self.app_data_dir, &sessions_root)?;
        let mut planned_paths = vec![
            (
                sessions_root.join(session_id),
                sessions_root,
                format!("session-{session_id}"),
            ),
            (pets_root.join(&pet_id), pets_root, "pet".to_owned()),
        ];
        for job_id in &job_ids {
            planned_paths.push((
                jobs_root.join(job_id),
                jobs_root.clone(),
                format!("job-{job_id}"),
            ));
        }
        for (source, parent, _) in &planned_paths {
            validate_source_path(source, parent)?;
        }

        let mut quarantined = Vec::new();
        for (source, _, name) in &planned_paths {
            match quarantine_path_with_ops(
                source,
                &quarantine_root,
                name,
                self.journal_publish_ops.as_ref(),
            ) {
                Ok(Some(path)) => quarantined.push(path),
                Ok(None) => {}
                Err(error) => {
                    if let Some(path) = error.uncertain_path.clone() {
                        quarantined.push(path);
                    }
                    return Err(recover_uncommitted_with_ops(
                        &quarantine_root,
                        format!("failed to quarantine {}: {error}", source.display()),
                        &quarantined,
                        self.journal_publish_ops.as_ref(),
                    ));
                }
            }
        }

        if let Err(error) = self.abandon_creation_rows(session_id, &pet_id, &method, &job_ids) {
            return Err(recover_uncommitted_with_ops(
                &quarantine_root,
                error,
                &quarantined,
                self.journal_publish_ops.as_ref(),
            ));
        }

        journal.phase = DeletionPhase::Committed;
        if let Err(error) = write_journal_with_ops(
            &quarantine_root,
            &journal,
            self.journal_publish_ops.as_ref(),
        ) {
            eprintln!(
                "[desktop-pet] could not record committed creation abandonment {session_id}: {error}"
            );
        }
        if let Err(error) = remove_operation(&quarantine_root) {
            eprintln!(
                "[desktop-pet] creation abandonment quarantine cleanup failed {session_id}: {error}"
            );
        }
        Ok(())
    }

    pub fn cleanup_quarantine(&self) -> Result<(), String> {
        let request_id = new_entity_id("cleanup");
        let _operation =
            self.mutation_gate
                .scoped(&request_id, MutationKind::Delete, "quarantine")?;
        let trash_root = self.app_data_dir.join("trash");
        let root = trash_root.join("pet-delete");
        if !root.exists() {
            return Ok(());
        }
        validate_owned_root(&self.app_data_dir, &trash_root)?;
        validate_owned_root(&trash_root, &root)?;
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        let mut issues = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    issues.push(error.to_string());
                    continue;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    issues.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            if !file_type.is_dir() {
                continue;
            }
            let result = self.cleanup_operation(&root, &path);
            if let Err(error) = result {
                issues.push(format!("{}: {error}", path.display()));
            }
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "quarantine cleanup incomplete: {}",
                issues.join("; ")
            ))
        }
    }

    fn require_deletable_pet(&self, pet_id: &str) -> Result<DeletionPlan, String> {
        let session_active = self.active.active().ok();
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let persisted_active: Option<String> = storage
            .db
            .query_row(
                "SELECT value FROM state WHERE key = 'app:active_pet_id'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if persisted_active.as_deref() == Some(pet_id)
            || (persisted_active.is_none() && session_active.as_deref() == Some(pet_id))
        {
            return Err("the active pet cannot be deleted".into());
        }

        let exists = storage
            .db
            .query_row(
                "SELECT 1 FROM pets WHERE pet_id = ?1",
                rusqlite::params![pet_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .is_some();
        if !exists {
            return Err("pet not found".into());
        }

        deletion_plan(&storage.db, pet_id)
    }

    fn creation_abandon_plan(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, String, String, Vec<String>)>, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tombstoned = storage
            .db
            .query_row(
                "SELECT 1 FROM creation_session_tombstones WHERE session_id=?1",
                [session_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .is_some();
        if tombstoned {
            return Ok(None);
        }
        let session: Option<(String, String, String)> = storage
            .db
            .query_row(
                "SELECT pet_id, method, status FROM creation_sessions WHERE session_id=?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (pet_id, method, status) =
            session.ok_or_else(|| format!("creation session not found: {session_id}"))?;
        let job_ids = creation_job_ids(&storage.db, session_id, &pet_id)?;
        Ok(Some((pet_id, method, status, job_ids)))
    }

    fn abandon_creation_rows(
        &self,
        session_id: &str,
        pet_id: &str,
        method: &str,
        expected_job_ids: &[String],
    ) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let tombstoned = tx
            .query_row(
                "SELECT 1 FROM creation_session_tombstones WHERE session_id=?1",
                [session_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .is_some();
        if tombstoned {
            return Ok(());
        }
        let current: Option<(
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        )> = tx
            .query_row(
                "SELECT cs.pet_id, cs.method, cs.status, p.lifecycle, p.completed_at,
                        (SELECT value FROM state WHERE key='app:active_pet_id')
                 FROM creation_sessions cs JOIN pets p ON p.pet_id=cs.pet_id
                 WHERE cs.session_id=?1",
                [session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (
            current_pet_id,
            current_method,
            current_status,
            pet_lifecycle,
            pet_completed_at,
            active_pet_id,
        ) = current
            .ok_or_else(|| format!("creation session changed during abandonment: {session_id}"))?;
        if current_pet_id != pet_id || current_method != method {
            return Err("creation session ownership changed during abandonment".into());
        }
        if current_status == "completed" {
            return Err("a completed creation session cannot be abandoned".into());
        }
        if pet_lifecycle == "ready"
            || pet_completed_at.is_some()
            || active_pet_id.as_deref() == Some(pet_id)
        {
            return Err("a ready or active pet cannot be abandoned".into());
        }
        let current_job_ids = creation_job_ids(&tx, session_id, pet_id)?;
        let mut expected_job_ids = expected_job_ids.to_vec();
        expected_job_ids.sort();
        if current_job_ids != expected_job_ids {
            return Err("creation jobs changed during abandonment".into());
        }
        tx.execute(
            "INSERT INTO creation_session_tombstones
             (session_id, pet_id, method, abandoned_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                session_id,
                pet_id,
                method,
                crate::creation::profiles::now_iso()
            ],
        )
        .map_err(|error| error.to_string())?;
        delete_owned_rows(&tx, pet_id, &[session_id.to_owned()])?;
        let affected = tx
            .execute("DELETE FROM pets WHERE pet_id=?1", [pet_id])
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("reserved pet changed during abandonment".into());
        }
        tx.commit().map_err(|error| error.to_string())
    }

    fn delete_rows(
        &self,
        pet_id: &str,
        expected_job_ids: &[String],
        expected_session_ids: &[String],
    ) -> Result<(), String> {
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let active_pet: Option<String> = tx
            .query_row(
                "SELECT value FROM state WHERE key = 'app:active_pet_id'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if active_pet.as_deref() == Some(pet_id) {
            return Err("the active pet cannot be deleted".into());
        }
        let current_plan = deletion_plan(&tx, pet_id)?;
        let mut expected_job_ids = expected_job_ids.to_vec();
        expected_job_ids.sort();
        let mut expected_session_ids = expected_session_ids.to_vec();
        expected_session_ids.sort();
        if current_plan.job_ids != expected_job_ids {
            return Err("generation jobs changed during deletion".into());
        }
        if current_plan.session_ids != expected_session_ids {
            return Err("creation sessions changed during deletion".into());
        }
        delete_owned_rows(&tx, pet_id, &expected_session_ids)?;
        let affected = tx
            .execute(
                "DELETE FROM pets WHERE pet_id = ?1",
                rusqlite::params![pet_id],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("pet not found".into());
        }
        tx.commit().map_err(|error| error.to_string())
    }

    fn cleanup_operation(&self, root: &Path, operation: &Path) -> Result<(), String> {
        validate_operation_directory(root, operation)?;
        let journal = read_journal(operation)?;
        let paths = self.paths_from_journal(operation, &journal)?;
        let pet_exists = self.pet_exists(&journal.pet_id)?;
        if journal.phase == DeletionPhase::Committed || !pet_exists {
            return remove_operation(operation);
        }
        restore_all_with_ops(&paths, self.journal_publish_ops.as_ref())?;
        remove_operation(operation)
    }

    fn paths_from_journal(
        &self,
        operation: &Path,
        journal: &DeletionJournal,
    ) -> Result<Vec<QuarantinedPath>, String> {
        validate_journal(journal)?;
        let pets_root = self.app_data_dir.join("pets");
        let jobs_root = self.app_data_dir.join("jobs");
        let mut paths = vec![QuarantinedPath {
            original: pets_root.join(&journal.pet_id),
            quarantined: operation.join("pet"),
            original_parent: pets_root,
            quarantine_parent: operation.into(),
        }];
        for job_id in &journal.job_ids {
            paths.push(QuarantinedPath {
                original: jobs_root.join(job_id),
                quarantined: operation.join(format!("job-{job_id}")),
                original_parent: jobs_root.clone(),
                quarantine_parent: operation.into(),
            });
        }
        let sessions_root = self.app_data_dir.join("creation-sessions");
        for session_id in &journal.session_ids {
            paths.push(QuarantinedPath {
                original: sessions_root.join(session_id),
                quarantined: operation.join(format!("session-{session_id}")),
                original_parent: sessions_root.clone(),
                quarantine_parent: operation.into(),
            });
        }
        Ok(paths)
    }

    fn pet_exists(&self, pet_id: &str) -> Result<bool, String> {
        self.storage
            .lock()
            .map_err(|_| "storage lock poisoned")?
            .db
            .query_row(
                "SELECT 1 FROM pets WHERE pet_id = ?1",
                rusqlite::params![pet_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| error.to_string())
            .map(|value| value.is_some())
    }

    fn quarantine_root(&self) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        self.app_data_dir
            .join("trash")
            .join("pet-delete")
            .join(format!("{}-{nanos}", std::process::id()))
    }
}

fn deletion_plan(db: &Connection, pet_id: &str) -> Result<DeletionPlan, String> {
    let session_ids = {
        let mut statement = db
            .prepare("SELECT session_id FROM creation_sessions WHERE pet_id=?1 ORDER BY session_id")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([pet_id], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .map(|row| row.map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for session_id in &session_ids {
        validate_component(session_id, "session id")?;
    }
    let owned_sessions = session_ids
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    let mut statement = db
        .prepare(
            "SELECT job_id, pet_id, session_id FROM generation_jobs
             WHERE pet_id=?1 OR session_id IN
                   (SELECT session_id FROM creation_sessions WHERE pet_id=?1)
             ORDER BY job_id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([pet_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut job_ids = Vec::new();
    for row in rows {
        let (job_id, actual_pet_id, session_id) = row.map_err(|error| error.to_string())?;
        validate_component(&job_id, "job id")?;
        if actual_pet_id != pet_id
            || session_id
                .as_ref()
                .is_some_and(|session_id| !owned_sessions.contains(session_id))
        {
            return Err(format!(
                "generation job {job_id} is not owned by pet {pet_id}"
            ));
        }
        job_ids.push(job_id);
    }
    Ok(DeletionPlan {
        job_ids,
        session_ids,
    })
}

fn delete_owned_rows(
    tx: &rusqlite::Transaction<'_>,
    pet_id: &str,
    expected_session_ids: &[String],
) -> Result<(), String> {
    let mut actual_session_ids = {
        let mut statement = tx
            .prepare("SELECT session_id FROM creation_sessions WHERE pet_id=?1 ORDER BY session_id")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([pet_id], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .map(|row| row.map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let mut expected_session_ids = expected_session_ids.to_vec();
    actual_session_ids.sort();
    expected_session_ids.sort();
    if actual_session_ids != expected_session_ids {
        return Err("creation sessions changed during row deletion".into());
    }
    for sql in [
        "DELETE FROM variants WHERE pet_id=?1",
        "DELETE FROM appearance_variants WHERE pet_id=?1",
        "DELETE FROM generation_jobs WHERE pet_id=?1",
        "DELETE FROM creation_upload_sources WHERE session_id IN
             (SELECT session_id FROM creation_sessions WHERE pet_id=?1)",
        "DELETE FROM creation_adoption_provenance WHERE session_id IN
             (SELECT session_id FROM creation_sessions WHERE pet_id=?1)",
        "DELETE FROM composer_recipes WHERE session_id IN
             (SELECT session_id FROM creation_sessions WHERE pet_id=?1)",
        "DELETE FROM creation_sessions WHERE pet_id=?1",
        "DELETE FROM identity_profiles WHERE pet_id=?1",
    ] {
        tx.execute(sql, [pet_id])
            .map_err(|error| error.to_string())?;
    }
    tx.execute(
        "DELETE FROM state WHERE key=?1",
        [format!("creation:{pet_id}:compile_error")],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn creation_job_ids(
    db: &Connection,
    session_id: &str,
    pet_id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = db
        .prepare(
            "SELECT job_id, pet_id, session_id FROM generation_jobs
             WHERE pet_id=?1 OR session_id=?2 ORDER BY job_id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(rusqlite::params![pet_id, session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut job_ids = Vec::new();
    for row in rows {
        let (job_id, actual_pet_id, actual_session_id) = row.map_err(|error| error.to_string())?;
        validate_component(&job_id, "job id")?;
        if actual_pet_id != pet_id || actual_session_id.as_deref() != Some(session_id) {
            return Err(format!(
                "generation job {job_id} is not owned by creation session {session_id} and pet {pet_id}"
            ));
        }
        job_ids.push(job_id);
    }
    Ok(job_ids)
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

fn validate_source_path(source: &Path, expected_parent: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    validate_path_parent(source, expected_parent)
}

fn validate_regular_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {label}: {error}"))?;
    if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(format!("{label} cannot be a link or reparse point"));
    }
    Ok(())
}

fn validate_owned_root(parent: &Path, root: &Path) -> Result<(), String> {
    validate_regular_directory(parent, "deletion parent")?;
    if !root.exists() {
        std::fs::create_dir(root).map_err(|error| error.to_string())?;
    }
    validate_existing_owned_root(parent, root)
}

fn validate_existing_owned_root(parent: &Path, root: &Path) -> Result<(), String> {
    validate_regular_directory(parent, "deletion parent")?;
    validate_regular_directory(root, "deletion root")?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| format!("cannot resolve deletion parent: {error}"))?;
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve deletion root: {error}"))?;
    if canonical_root.parent() != Some(canonical_parent.as_path()) {
        return Err(format!(
            "deletion root is outside its authoritative parent: {}",
            root.display()
        ));
    }
    Ok(())
}

fn prepare_quarantine_root(
    app_data_dir: &Path,
    operation: &Path,
    ops: &dyn JournalPublishOps,
) -> Result<(), String> {
    let trash = app_data_dir.join("trash");
    let delete_root = trash.join("pet-delete");
    ensure_owned_root_durable(app_data_dir, &trash, ops)?;
    ensure_owned_root_durable(&trash, &delete_root, ops)?;
    ensure_owned_root_durable(&delete_root, operation, ops)
}

fn ensure_owned_root_durable(
    parent: &Path,
    root: &Path,
    ops: &dyn JournalPublishOps,
) -> Result<(), String> {
    validate_regular_directory(parent, "deletion parent")?;
    match std::fs::symlink_metadata(root) {
        Ok(_) => {
            validate_existing_owned_root(parent, root)?;
            ops.sync_existing_directory_entry(root)?;
            return validate_existing_owned_root(parent, root);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }

    let root_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "durable deletion root has no standard file name".to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let staging = parent.join(format!(
        ".{root_name}-{}-{nonce}.staging",
        std::process::id()
    ));
    std::fs::create_dir(&staging).map_err(|error| error.to_string())?;
    if let Err(error) = validate_existing_owned_root(parent, &staging) {
        cleanup_empty_staging_directory(parent, &staging);
        return Err(error);
    }

    if let Err(error) = ops.durable_rename(&staging, root) {
        cleanup_empty_staging_directory(parent, &staging);
        return match std::fs::symlink_metadata(root) {
            Ok(_) => match validate_existing_owned_root(parent, root) {
                Ok(()) => Err(error),
                Err(validation) => Err(format!(
                    "{error}; published deletion root is invalid: {validation}"
                )),
            },
            Err(inspect_error) if inspect_error.kind() == std::io::ErrorKind::NotFound => {
                Err(error)
            }
            Err(inspect_error) => Err(format!(
                "{error}; cannot inspect possibly published deletion root: {inspect_error}"
            )),
        };
    }
    validate_existing_owned_root(parent, root)
}

fn cleanup_empty_staging_directory(parent: &Path, staging: &Path) {
    if validate_existing_owned_root(parent, staging).is_ok() {
        let _ = std::fs::remove_dir(staging);
    }
}

fn validate_path_parent(path: &Path, expected_parent: &Path) -> Result<(), String> {
    validate_regular_directory(expected_parent, "deletion source parent")?;
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| format!("cannot inspect deletion source: {error}"))?;
        if crate::platform::is_link_or_reparse_point(&metadata) {
            return Err(format!(
                "deletion source cannot be a link or reparse point: {}",
                path.display()
            ));
        }
        if !metadata.is_dir() {
            return Err(format!("refusing non-directory source: {}", path.display()));
        }
    }
    let expected_parent = expected_parent
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize deletion parent: {error}"))?;
    let path_parent = path
        .parent()
        .ok_or_else(|| "deletion source has no parent".to_string())?
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize deletion source parent: {error}"))?;
    if path_parent != expected_parent {
        return Err(format!(
            "refusing path outside deletion scope: {}",
            path.display()
        ));
    }
    if path.exists() {
        let canonical_path = path
            .canonicalize()
            .map_err(|error| format!("cannot canonicalize deletion source: {error}"))?;
        if canonical_path.parent() != Some(expected_parent.as_path()) {
            return Err(format!(
                "refusing path outside deletion scope: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
fn quarantine_path(
    source: &Path,
    root: &Path,
    name: &str,
) -> Result<Option<QuarantinedPath>, String> {
    quarantine_path_with_ops(source, root, name, &PlatformJournalPublishOps)
        .map_err(|error| error.message)
}

fn quarantine_path_with_ops(
    source: &Path,
    root: &Path,
    name: &str,
    ops: &dyn JournalPublishOps,
) -> Result<Option<QuarantinedPath>, QuarantineMoveError> {
    if !source.exists() {
        return Ok(None);
    }
    std::fs::create_dir_all(root).map_err(|error| QuarantineMoveError {
        message: error.to_string(),
        uncertain_path: None,
    })?;
    let target = root.join(name);
    if target.exists() {
        return Err(QuarantineMoveError {
            message: format!("quarantine target already exists: {}", target.display()),
            uncertain_path: None,
        });
    }
    let path = QuarantinedPath {
        original: source.into(),
        quarantined: target.clone(),
        original_parent: source
            .parent()
            .ok_or_else(|| QuarantineMoveError {
                message: "deletion source has no parent".to_string(),
                uncertain_path: None,
            })?
            .into(),
        quarantine_parent: root.into(),
    };
    if let Err(error) = ops.durable_rename(source, &target) {
        return Err(QuarantineMoveError {
            message: error,
            uncertain_path: Some(path),
        });
    }
    Ok(Some(path))
}

fn restore_all_with_ops(
    paths: &[QuarantinedPath],
    ops: &dyn JournalPublishOps,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for path in paths.iter().rev() {
        if !path.quarantined.exists() {
            if !path.original.exists() {
                errors.push(format!(
                    "resource exists at neither original nor quarantine path: {}",
                    path.original.display()
                ));
            }
            continue;
        }
        if let Err(error) = std::fs::create_dir_all(&path.original_parent) {
            errors.push(format!("{}: {error}", path.original_parent.display()));
            continue;
        }
        if let Err(error) = validate_path_parent(&path.original, &path.original_parent) {
            errors.push(error);
            continue;
        }
        if let Err(error) = validate_source_path(&path.quarantined, &path.quarantine_parent) {
            errors.push(error);
            continue;
        }
        if path.original.exists() {
            errors.push(format!(
                "restore conflict: {} already exists",
                path.original.display()
            ));
            continue;
        }
        if let Err(error) = ops.durable_rename(&path.quarantined, &path.original) {
            errors.push(format!("{}: {error}", path.original.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(test)]
fn recover_uncommitted(root: &Path, error: String, paths: &[QuarantinedPath]) -> String {
    recover_uncommitted_with_ops(root, error, paths, &PlatformJournalPublishOps)
}

fn recover_uncommitted_with_ops(
    root: &Path,
    error: String,
    paths: &[QuarantinedPath],
    ops: &dyn JournalPublishOps,
) -> String {
    match restore_all_with_ops(paths, ops) {
        Ok(()) => match remove_operation(root) {
            Ok(()) => error,
            Err(cleanup_error) => {
                format!("{error}; restored data but cleanup failed: {cleanup_error}")
            }
        },
        Err(restore_error) => format!("{error}; quarantine restore failed: {restore_error}"),
    }
}

#[cfg(test)]
fn write_journal(root: &Path, journal: &DeletionJournal) -> Result<(), String> {
    write_journal_with_ops(root, journal, &PlatformJournalPublishOps)
}

fn write_journal_with_ops(
    root: &Path,
    journal: &DeletionJournal,
    ops: &dyn JournalPublishOps,
) -> Result<(), String> {
    validate_journal(journal)?;
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    validate_regular_directory(root, "deletion journal directory")?;
    let bytes = serde_json::to_vec(journal).map_err(|error| error.to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let staging = root.join(format!(".journal-{}-{nonce}.staging", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staging)
        .map_err(|error| error.to_string())?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&staging);
        return Err(error.to_string());
    }
    drop(file);
    if let Err(error) = ops.checkpoint(JournalPublishStep::StagingSynced) {
        return Err(error);
    }

    let current = root.join(JOURNAL_FILE);
    let previous = root.join(PREVIOUS_JOURNAL_FILE);
    validate_journal_path(&current, "deletion journal")?;
    validate_journal_path(&previous, "previous deletion journal")?;

    if current.exists() && read_valid_journal(&current).is_ok() {
        if let Err(error) = ops.durable_rename(&current, &previous) {
            let _ = std::fs::remove_file(&staging);
            return Err(error);
        }
        if let Err(error) = ops.checkpoint(JournalPublishStep::PreviousPublished) {
            return Err(error);
        }
    }
    if let Err(error) = ops.durable_rename(&staging, &current) {
        let _ = std::fs::remove_file(&staging);
        return Err(error);
    }
    ops.checkpoint(JournalPublishStep::CurrentPublished)
}

fn read_journal(operation: &Path) -> Result<DeletionJournal, String> {
    let current = operation.join(JOURNAL_FILE);
    match read_valid_journal(&current) {
        Ok(journal) => return Ok(journal),
        Err(current_error) => {
            let previous = operation.join(PREVIOUS_JOURNAL_FILE);
            match read_valid_journal(&previous) {
                Ok(journal) => return Ok(journal),
                Err(previous_error) => {
                    return Err(format!(
                        "current journal invalid: {current_error}; previous journal invalid: {previous_error}"
                    ));
                }
            }
        }
    }
}

fn read_valid_journal(path: &Path) -> Result<DeletionJournal, String> {
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.file_type().is_file() {
        return Err("journal is not a regular file".into());
    }
    let journal = serde_json::from_slice(&std::fs::read(path).map_err(|error| error.to_string())?)
        .map_err(|error| format!("invalid journal: {error}"))?;
    validate_journal(&journal)?;
    Ok(journal)
}

fn validate_journal_path(path: &Path, label: &str) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_file() =>
        {
            Err(format!("{label} is not a regular file"))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn validate_journal(journal: &DeletionJournal) -> Result<(), String> {
    validate_component(&journal.pet_id, "journal pet id")?;
    let mut job_ids = std::collections::BTreeSet::new();
    for job_id in &journal.job_ids {
        validate_component(job_id, "journal job id")?;
        if !job_ids.insert(job_id) {
            return Err("journal contains duplicate job id".into());
        }
    }
    let mut session_ids = std::collections::BTreeSet::new();
    for session_id in &journal.session_ids {
        validate_component(session_id, "journal session id")?;
        if !session_ids.insert(session_id) {
            return Err("journal contains duplicate session id".into());
        }
    }
    Ok(())
}

fn validate_operation_directory(root: &Path, operation: &Path) -> Result<(), String> {
    validate_path_parent(operation, root)
}

fn remove_operation(operation: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(operation) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pets::mutation::{MutationKind, PetMutationGate, SharedPetMutationGate};
    use crate::pets::{active::ActivePetService, ActivePetSession, SharedActivePetSession};
    use crate::storage::Storage;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct InterruptingJournalPublishOps {
        interrupt_after: JournalPublishStep,
    }

    impl JournalPublishOps for InterruptingJournalPublishOps {
        fn durable_rename(&self, source: &Path, target: &Path) -> Result<(), String> {
            crate::platform::durable_replace_file(source, target)
        }

        fn checkpoint(&self, step: JournalPublishStep) -> Result<(), String> {
            if step == self.interrupt_after {
                Err(format!("interrupted after {step:?}"))
            } else {
                Ok(())
            }
        }
    }

    struct FailAfterDurableRenameOps {
        fail_call: u32,
        calls: Arc<AtomicU32>,
    }

    impl JournalPublishOps for FailAfterDurableRenameOps {
        fn durable_rename(&self, source: &Path, target: &Path) -> Result<(), String> {
            crate::platform::durable_replace_file(source, target)?;
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.fail_call {
                Err(format!("durability barrier failed after rename {call}"))
            } else {
                Ok(())
            }
        }
    }

    struct FailBeforeDurableRenameOps {
        fail_call: u32,
        calls: Arc<AtomicU32>,
    }

    struct FailExistingRootSyncOps {
        fail_call: u32,
        calls: Arc<AtomicU32>,
    }

    impl JournalPublishOps for FailExistingRootSyncOps {
        fn durable_rename(&self, source: &Path, target: &Path) -> Result<(), String> {
            crate::platform::durable_replace_file(source, target)
        }

        fn sync_existing_directory_entry(&self, root: &Path) -> Result<(), String> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.fail_call {
                Err(format!("existing root sync failed at {call}"))
            } else {
                crate::platform::sync_existing_directory_entry(root)
            }
        }
    }

    #[derive(Default)]
    struct RecordingDirectoryOps {
        existing_syncs: Mutex<Vec<PathBuf>>,
        publishes: Mutex<Vec<PathBuf>>,
    }

    impl JournalPublishOps for RecordingDirectoryOps {
        fn durable_rename(&self, source: &Path, target: &Path) -> Result<(), String> {
            crate::platform::durable_replace_file(source, target)?;
            self.publishes.lock().unwrap().push(target.into());
            Ok(())
        }

        fn sync_existing_directory_entry(&self, root: &Path) -> Result<(), String> {
            crate::platform::sync_existing_directory_entry(root)?;
            self.existing_syncs.lock().unwrap().push(root.into());
            Ok(())
        }
    }

    impl JournalPublishOps for FailBeforeDurableRenameOps {
        fn durable_rename(&self, source: &Path, target: &Path) -> Result<(), String> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.fail_call {
                Err(format!("directory publication failed before rename {call}"))
            } else {
                crate::platform::durable_replace_file(source, target)
            }
        }
    }

    struct DeletionHarness {
        root: PathBuf,
        storage: Arc<Mutex<Storage>>,
        service: PetDeletionService,
        gate: SharedPetMutationGate,
    }

    impl DeletionHarness {
        fn two_pets() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "desktop-pet-deletion-{}-{n}-{nonce}",
                std::process::id()
            ));
            let pets_dir = root.join("pets");
            let storage = Arc::new(Mutex::new(Storage::open(&pets_dir).unwrap()));
            let session: SharedActivePetSession = Arc::new(Mutex::new(ActivePetSession::new()));
            session
                .lock()
                .unwrap()
                .set_active(BUILTIN_PET_ID.into())
                .unwrap();
            let gate = Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60)));
            let active = Arc::new(ActivePetService::new(
                storage.clone(),
                session,
                pets_dir,
                gate.clone(),
            ));
            let service =
                PetDeletionService::new(storage.clone(), active, root.clone(), gate.clone());
            let test = Self {
                root,
                storage,
                service,
                gate,
            };
            test.insert_pet_with_job("pet-a", "job-a");
            test.insert_pet_with_job("pet-b", "job-b");
            test
        }

        fn current(pet_id: &str) -> Self {
            let test = Self::two_pets();
            test.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO state (key, value) VALUES ('app:active_pet_id', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    rusqlite::params![pet_id],
                )
                .unwrap();
            test
        }

        fn with_journal_publish_ops(ops: Arc<dyn JournalPublishOps>) -> Self {
            let mut test = Self::two_pets();
            test.service.journal_publish_ops = ops;
            test
        }

        fn with_forced_db_failure(pet_id: &str) -> Self {
            let test = Self::two_pets();
            test.storage
                .lock()
                .unwrap()
                .db
                .execute_batch(&format!(
                    "CREATE TRIGGER fail_pet_delete BEFORE DELETE ON pets
                     WHEN OLD.pet_id = '{pet_id}'
                     BEGIN SELECT RAISE(ABORT, 'forced delete failure'); END;"
                ))
                .unwrap();
            test
        }

        fn insert_pet_with_job(&self, pet_id: &str, job_id: &str) {
            let storage = self.storage.lock().unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO pets
                     (pet_id, schema_version, species, identity_mode, created_at, updated_at)
                     VALUES (?1, 1, 'cat', 'realpet', '0', '0')",
                    rusqlite::params![pet_id],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO identity_profiles
                     (profile_id, pet_id, schema_version, species, identity_mode, locked_traits, created_at)
                     VALUES (?1, ?2, 1, 'cat', 'realpet', '{}', '0')",
                    rusqlite::params![format!("profile-{pet_id}"), pet_id],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO generation_jobs
                     (job_id, pet_id, prompt, ref_sha256, status, created_at)
                     VALUES (?1, ?2, 'prompt', 'hash', 'done', '0')",
                    rusqlite::params![job_id, pet_id],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO appearance_variants
                     (variant_id, pet_id, job_id, image_path, quality, accepted, created_at)
                     VALUES (?1, ?2, ?3, 'image.png', 'good', 0, '0')",
                    rusqlite::params![format!("appearance-{pet_id}"), pet_id, job_id],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO variants (variant_id, pet_id, style_id, manifest_path, created_at)
                     VALUES (?1, ?2, 'style', 'assets/manifest.json', '0')",
                    rusqlite::params![format!("runtime-{pet_id}"), pet_id],
                )
                .unwrap();
            drop(storage);
            std::fs::create_dir_all(self.pet_dir(pet_id).join("assets")).unwrap();
            std::fs::write(
                self.pet_dir(pet_id).join("assets").join("asset.txt"),
                b"pet",
            )
            .unwrap();
            std::fs::create_dir_all(self.job_dir(job_id)).unwrap();
            std::fs::write(self.job_dir(job_id).join("result.txt"), b"job").unwrap();
        }

        fn pet_exists(&self, pet_id: &str) -> bool {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT 1 FROM pets WHERE pet_id = ?1",
                    rusqlite::params![pet_id],
                    |_| Ok(()),
                )
                .is_ok()
        }

        fn job_exists(&self, job_id: &str) -> bool {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT 1 FROM generation_jobs WHERE job_id = ?1",
                    rusqlite::params![job_id],
                    |_| Ok(()),
                )
                .is_ok()
        }

        fn pet_dir(&self, pet_id: &str) -> PathBuf {
            self.root.join("pets").join(pet_id)
        }

        fn job_dir(&self, job_id: &str) -> PathBuf {
            self.root.join("jobs").join(job_id)
        }

        fn session_dir(&self, session_id: &str) -> PathBuf {
            self.root.join("creation-sessions").join(session_id)
        }

        fn bind_pet_a_job_to_creation_session(&self) {
            self.storage
                .lock()
                .unwrap()
                .db
                .execute_batch(
                    "INSERT INTO creation_sessions
                     (session_id, pet_id, method, status, last_stable_status, current_step,
                      schema_version, created_at, updated_at)
                     VALUES ('session-a', 'pet-a', 'upload', 'draft', 'draft', 'upload',
                             1, '0', '0');
                     UPDATE pets SET lifecycle='draft', completed_at=NULL WHERE pet_id='pet-a';
                     UPDATE generation_jobs SET session_id='session-a' WHERE job_id='job-a';",
                )
                .unwrap();
            std::fs::create_dir_all(self.session_dir("session-a")).unwrap();
            std::fs::write(self.session_dir("session-a").join("draft.txt"), b"session").unwrap();
        }

        fn bind_completed_adoption(&self, template_id: &str) {
            self.storage
                .lock()
                .unwrap()
                .db
                .execute_batch(&format!(
                    "UPDATE pets
                     SET display_name='雾团', identity_mode='adopted', creation_method='adoption',
                         source_template_id='{template_id}', source_template_version=3,
                         lifecycle='ready', completed_at='1'
                     WHERE pet_id='pet-a';
                     INSERT INTO creation_sessions
                     (session_id, pet_id, method, status, last_stable_status, current_step,
                      schema_version, created_at, updated_at, completed_at)
                     VALUES ('session-a', 'pet-a', 'adoption', 'completed', 'completed',
                             'completed', 1, '0', '1', '1');
                     INSERT INTO creation_adoption_provenance
                     (session_id, source_template_id, source_template_version,
                      runtime_schema_version, body_sha256, motion_profile_sha256, created_at)
                     VALUES ('session-a', '{template_id}', 3, 3,
                             '{body_hash}', '{profile_hash}', '0');
                     UPDATE generation_jobs SET session_id='session-a' WHERE job_id='job-a';",
                    body_hash = "1".repeat(64),
                    profile_hash = "2".repeat(64),
                ))
                .unwrap();
            std::fs::create_dir_all(self.session_dir("session-a")).unwrap();
            std::fs::write(self.session_dir("session-a").join("source.txt"), b"session").unwrap();
        }

        fn bind_draft_adoption(&self, template_id: &str) {
            self.bind_pet_a_job_to_creation_session();
            self.storage
                .lock()
                .unwrap()
                .db
                .execute_batch(&format!(
                    "UPDATE pets
                     SET identity_mode='adopted', creation_method='adoption',
                         source_template_id='{template_id}', source_template_version=1
                     WHERE pet_id='pet-a';
                     UPDATE creation_sessions SET method='adoption' WHERE session_id='session-a';
                     INSERT INTO creation_adoption_provenance
                     (session_id, source_template_id, source_template_version,
                      runtime_schema_version, body_sha256, motion_profile_sha256, created_at)
                     VALUES ('session-a', '{template_id}', 1, 3, '{}', '{}', '0');",
                    "1".repeat(64),
                    "2".repeat(64),
                ))
                .unwrap();
        }

        fn source_count(&self, template_id: &str) -> i64 {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT COUNT(*) FROM pets WHERE source_template_id=?1",
                    [template_id],
                    |row| row.get(0),
                )
                .unwrap()
        }

        fn session_exists(&self, session_id: &str) -> bool {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT 1 FROM creation_sessions WHERE session_id=?1",
                    [session_id],
                    |_| Ok(()),
                )
                .is_ok()
        }

        fn isolate_creation_resources(&self, operation: &Path) -> Vec<QuarantinedPath> {
            vec![
                quarantine_path(
                    &self.session_dir("session-a"),
                    operation,
                    "session-session-a",
                )
                .unwrap()
                .unwrap(),
                quarantine_path(&self.pet_dir("pet-a"), operation, "pet")
                    .unwrap()
                    .unwrap(),
                quarantine_path(&self.job_dir("job-a"), operation, "job-job-a")
                    .unwrap()
                    .unwrap(),
            ]
        }

        fn quarantined_creation_operation(&self, phase: &str) -> PathBuf {
            self.bind_pet_a_job_to_creation_session();
            let operation = self
                .root
                .join("trash")
                .join("pet-delete")
                .join("creation-recovery-test");
            self.isolate_creation_resources(&operation);
            std::fs::write(
                operation.join("journal.json"),
                format!(
                    r#"{{"petId":"pet-a","jobIds":["job-a"],"sessionIds":["session-a"],"phase":"{phase}"}}"#
                ),
            )
            .unwrap();
            operation
        }

        fn quarantined_operation(&self, phase: &str) -> PathBuf {
            let operation = self
                .root
                .join("trash")
                .join("pet-delete")
                .join("recovery-test");
            std::fs::create_dir_all(&operation).unwrap();
            std::fs::rename(self.pet_dir("pet-a"), operation.join("pet")).unwrap();
            std::fs::rename(self.job_dir("job-a"), operation.join("job-job-a")).unwrap();
            std::fs::write(
                operation.join("journal.json"),
                format!(r#"{{"petId":"pet-a","jobIds":["job-a"],"phase":"{phase}"}}"#),
            )
            .unwrap();
            operation
        }
    }

    impl Drop for DeletionHarness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn deletes_only_target_pet_rows_and_job_directories() {
        let test = DeletionHarness::two_pets();
        let outcome = test.service.delete("pet-a").unwrap();
        assert_eq!(outcome.warning, None);
        assert!(!test.pet_exists("pet-a"));
        assert!(!test.pet_dir("pet-a").exists());
        assert!(!test.job_dir("job-a").exists());
        assert!(test.pet_exists("pet-b"));
        assert!(test.pet_dir("pet-b").exists());
        assert!(test.job_dir("job-b").exists());
    }

    #[test]
    fn deleting_an_adopted_pet_removes_its_session_and_releases_template_id() {
        let test = DeletionHarness::two_pets();
        test.bind_completed_adoption("template-misty");

        test.service.delete("pet-a").unwrap();

        assert_eq!(test.source_count("template-misty"), 0);
        assert!(!test.session_dir("session-a").exists());
        assert!(!test.job_dir("job-a").exists());
        assert!(!test.pet_dir("pet-a").exists());
    }

    #[test]
    fn failed_adopted_pet_delete_restores_session_job_and_pet_resources() {
        let test = DeletionHarness::two_pets();
        test.bind_completed_adoption("template-misty");
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "CREATE TRIGGER fail_pet_delete BEFORE DELETE ON pets
                 WHEN OLD.pet_id='pet-a'
                 BEGIN SELECT RAISE(ABORT, 'forced adopted delete failure'); END;",
            )
            .unwrap();

        assert!(test.service.delete("pet-a").is_err());

        assert!(test.pet_exists("pet-a"));
        assert!(test.session_exists("session-a"));
        assert_eq!(test.source_count("template-misty"), 1);
        assert!(test.pet_dir("pet-a").join("assets/asset.txt").exists());
        assert!(test.job_dir("job-a").join("result.txt").exists());
        assert!(test.session_dir("session-a").join("source.txt").exists());
        let source_version: i64 = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT source_template_version FROM pets WHERE pet_id='pet-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_version, 3);
        let provenance_count: i64 = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM creation_adoption_provenance
                 WHERE session_id='session-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provenance_count, 1);
    }

    #[test]
    fn deletion_explicitly_removes_all_owned_rows_without_foreign_key_cascades() {
        let test = DeletionHarness::two_pets();
        test.bind_completed_adoption("template-misty");
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO composer_recipes
                 (session_id, recipe_version, pack_id, pack_version, layer_contract_version,
                  body_id, ears_id, eyes_id, muzzle_id, tail_id, color_id, pattern_id, updated_at)
                 VALUES ('session-a', 1, 'pack', 1, 1, 'body', 'ears', 'eyes', 'muzzle',
                         'tail', 'color', 'pattern', '1')",
                [],
            )
            .unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO creation_upload_sources
                 (session_id, normalized_png, sha256, mime_type, byte_size, created_at)
                 VALUES ('session-a', X'89', ?1, 'image/png', 1, '1')",
                ["0".repeat(64)],
            )
            .unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch("PRAGMA foreign_keys=OFF")
            .unwrap();

        test.service.delete("pet-a").unwrap();

        let storage = test.storage.lock().unwrap();
        for (table, column, value) in [
            ("variants", "pet_id", "pet-a"),
            ("appearance_variants", "pet_id", "pet-a"),
            ("generation_jobs", "pet_id", "pet-a"),
            ("creation_upload_sources", "session_id", "session-a"),
            ("creation_adoption_provenance", "session_id", "session-a"),
            ("composer_recipes", "session_id", "session-a"),
            ("creation_sessions", "pet_id", "pet-a"),
            ("identity_profiles", "pet_id", "pet-a"),
            ("pets", "pet_id", "pet-a"),
        ] {
            let count: i64 = storage
                .db
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE {column}=?1"),
                    [value],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0, "{table} retained an owned row");
        }
    }

    #[test]
    fn abandonment_explicitly_removes_adoption_provenance_without_foreign_keys() {
        let test = DeletionHarness::two_pets();
        test.bind_draft_adoption("template-misty");
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch("PRAGMA foreign_keys=OFF")
            .unwrap();

        test.service.abandon_creation("session-a").unwrap();

        let count: i64 = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM creation_adoption_provenance
                 WHERE session_id='session-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn failed_abandonment_restores_provenance_with_foreign_keys_disabled() {
        let test = DeletionHarness::two_pets();
        test.bind_draft_adoption("template-misty");
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "PRAGMA foreign_keys=OFF;
                 CREATE TRIGGER fail_adoption_pet_delete BEFORE DELETE ON pets
                 WHEN OLD.pet_id='pet-a'
                 BEGIN SELECT RAISE(ABORT, 'forced adoption abandon failure'); END;",
            )
            .unwrap();

        let error = test.service.abandon_creation("session-a").unwrap_err();

        assert!(error.contains("forced adoption abandon failure"), "{error}");
        assert!(test.pet_exists("pet-a"));
        assert!(test.session_exists("session-a"));
        let provenance_count: i64 = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM creation_adoption_provenance
                 WHERE session_id='session-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provenance_count, 1);
        assert!(test.session_dir("session-a").join("draft.txt").exists());
    }

    #[test]
    fn deletion_refuses_a_jobs_root_junction_without_touching_external_files() {
        let test = DeletionHarness::two_pets();
        let outside = test.root.with_file_name(format!(
            "{}-outside-jobs",
            test.root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(outside.join("job-a")).unwrap();
        let sentinel = outside.join("job-a/sentinel.txt");
        std::fs::write(&sentinel, b"outside must remain").unwrap();
        std::fs::remove_dir_all(test.root.join("jobs")).unwrap();
        crate::platform::create_directory_link(&outside, &test.root.join("jobs"));

        let result = test.service.delete("pet-a");

        assert!(result.unwrap_err().contains("link or reparse point"));
        assert!(test.pet_exists("pet-a"));
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside must remain");
        let _ = std::fs::remove_dir_all(test.root.join("jobs"));
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn deletion_refuses_a_creation_sessions_root_junction_without_touching_external_files() {
        let test = DeletionHarness::two_pets();
        test.bind_completed_adoption("template-misty");
        let outside = test.root.with_file_name(format!(
            "{}-outside-sessions",
            test.root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(outside.join("session-a")).unwrap();
        let sentinel = outside.join("session-a/sentinel.txt");
        std::fs::write(&sentinel, b"outside must remain").unwrap();
        std::fs::remove_dir_all(test.root.join("creation-sessions")).unwrap();
        crate::platform::create_directory_link(&outside, &test.root.join("creation-sessions"));

        let result = test.service.delete("pet-a");

        assert!(result.unwrap_err().contains("link or reparse point"));
        assert!(test.pet_exists("pet-a"));
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside must remain");
        let _ = std::fs::remove_dir_all(test.root.join("creation-sessions"));
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn deletion_refuses_a_session_directory_junction_without_touching_external_files() {
        let test = DeletionHarness::two_pets();
        test.bind_completed_adoption("template-misty");
        let outside = test.root.with_file_name(format!(
            "{}-outside-session",
            test.root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        let sentinel = outside.join("sentinel.txt");
        std::fs::write(&sentinel, b"outside must remain").unwrap();
        std::fs::remove_dir_all(test.session_dir("session-a")).unwrap();
        crate::platform::create_directory_link(&outside, &test.session_dir("session-a"));

        let result = test.service.delete("pet-a");

        assert!(result.unwrap_err().contains("link or reparse point"));
        assert!(test.pet_exists("pet-a"));
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside must remain");
        let _ = std::fs::remove_dir_all(test.session_dir("session-a"));
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn deletion_refuses_a_pet_directory_junction_without_touching_external_files() {
        let test = DeletionHarness::two_pets();
        let outside = test.root.with_file_name(format!(
            "{}-outside-pet",
            test.root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        let sentinel = outside.join("sentinel.txt");
        std::fs::write(&sentinel, b"outside must remain").unwrap();
        std::fs::remove_dir_all(test.pet_dir("pet-a")).unwrap();
        crate::platform::create_directory_link(&outside, &test.pet_dir("pet-a"));

        let result = test.service.delete("pet-a");

        assert!(result.unwrap_err().contains("link or reparse point"));
        assert!(test.pet_exists("pet-a"));
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside must remain");
        let _ = std::fs::remove_dir_all(test.pet_dir("pet-a"));
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn refuses_builtin_and_current_pet() {
        let test = DeletionHarness::current("pet-a");
        assert!(test
            .service
            .delete(BUILTIN_PET_ID)
            .unwrap_err()
            .contains("built-in"));
        assert!(test
            .service
            .delete("pet-a")
            .unwrap_err()
            .contains("active pet"));
    }

    #[test]
    fn delete_releases_the_shared_gate_after_error() {
        let test = DeletionHarness::two_pets();
        assert!(test.service.delete(BUILTIN_PET_ID).is_err());
        assert!(test
            .gate
            .begin("switch-2", MutationKind::Switch, "pet-b")
            .is_ok());
    }

    #[test]
    fn restores_quarantined_files_when_database_transaction_fails() {
        let test = DeletionHarness::with_forced_db_failure("pet-a");
        assert!(test.service.delete("pet-a").is_err());
        assert!(test.pet_exists("pet-a"));
        assert!(test.pet_dir("pet-a").exists());
        assert!(test.job_dir("job-a").exists());
    }

    #[test]
    fn restores_quarantine_when_pet_becomes_active_after_isolation() {
        let test = DeletionHarness::two_pets();
        let quarantine = test
            .root
            .join("trash")
            .join("pet-delete")
            .join("test-operation");
        let quarantined = vec![
            quarantine_path(&test.pet_dir("pet-a"), &quarantine, "pet")
                .unwrap()
                .unwrap(),
            quarantine_path(&test.job_dir("job-a"), &quarantine, "job-job-a")
                .unwrap()
                .unwrap(),
        ];
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO state (key, value) VALUES ('app:active_pet_id', 'pet-a')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [],
            )
            .unwrap();

        let error = test
            .service
            .delete_rows("pet-a", &["job-a".into()], &[])
            .unwrap_err();
        assert!(error.contains("active pet"));
        assert!(recover_uncommitted(&quarantine, error, &quarantined).contains("active pet"));
        assert!(test.pet_exists("pet-a"));
        assert!(test.pet_dir("pet-a").exists());
        assert!(test.job_dir("job-a").exists());
    }

    #[test]
    fn creation_abandon_final_recheck_restores_isolated_paths_after_owner_changes() {
        let test = DeletionHarness::two_pets();
        test.bind_pet_a_job_to_creation_session();
        let quarantine = test
            .root
            .join("trash")
            .join("pet-delete")
            .join("creation-owner-race");
        let quarantined = test.isolate_creation_resources(&quarantine);
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE generation_jobs SET session_id=NULL WHERE job_id='job-a'",
                [],
            )
            .unwrap();

        let error = test
            .service
            .abandon_creation_rows("session-a", "pet-a", "upload", &["job-a".into()])
            .unwrap_err();

        assert!(error.contains("job-a"));
        recover_uncommitted(&quarantine, error, &quarantined);
        assert!(test.session_dir("session-a").join("draft.txt").exists());
        assert!(test.pet_dir("pet-a").exists());
        assert!(test.job_dir("job-a").exists());
        assert!(test.pet_exists("pet-a"));
    }

    #[test]
    fn creation_abandon_final_recheck_restores_isolated_paths_after_job_is_added() {
        let test = DeletionHarness::two_pets();
        test.bind_pet_a_job_to_creation_session();
        let quarantine = test
            .root
            .join("trash")
            .join("pet-delete")
            .join("creation-added-job-race");
        let quarantined = test.isolate_creation_resources(&quarantine);
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO generation_jobs
                 (job_id, pet_id, session_id, prompt, ref_sha256, status, created_at)
                 VALUES ('job-late', 'pet-a', 'session-a', 'prompt', 'hash', 'pending', '1')",
                [],
            )
            .unwrap();
        std::fs::create_dir_all(test.job_dir("job-late")).unwrap();
        std::fs::write(test.job_dir("job-late").join("late.txt"), b"late").unwrap();

        let error = test
            .service
            .abandon_creation_rows("session-a", "pet-a", "upload", &["job-a".into()])
            .unwrap_err();

        assert!(error.contains("jobs changed"));
        recover_uncommitted(&quarantine, error, &quarantined);
        assert!(test.session_dir("session-a").join("draft.txt").exists());
        assert!(test.pet_dir("pet-a").exists());
        assert!(test.job_dir("job-a").exists());
        assert!(test.job_dir("job-late").join("late.txt").exists());
        assert!(test.pet_exists("pet-a"));
    }

    #[test]
    fn startup_cleanup_restores_uncommitted_quarantine_and_preserves_database_rows() {
        let test = DeletionHarness::two_pets();
        let operation = test.quarantined_operation("quarantined");

        test.service.cleanup_quarantine().unwrap();

        assert!(test.pet_exists("pet-a"));
        assert!(test.job_exists("job-a"));
        assert!(test.pet_dir("pet-a").exists());
        assert!(test.job_dir("job-a").exists());
        assert!(!operation.exists());
    }

    #[test]
    fn startup_cleanup_discards_committed_quarantine_without_restoring_files() {
        let test = DeletionHarness::two_pets();
        let operation = test.quarantined_operation("committed");
        test.service
            .delete_rows("pet-a", &["job-a".into()], &[])
            .unwrap();

        test.service.cleanup_quarantine().unwrap();

        assert!(!test.pet_exists("pet-a"));
        assert!(!test.pet_dir("pet-a").exists());
        assert!(!test.job_dir("job-a").exists());
        assert!(!operation.exists());
    }

    #[test]
    fn startup_cleanup_restores_prepared_creation_session_quarantine() {
        let test = DeletionHarness::two_pets();
        let operation = test.quarantined_creation_operation("prepared");

        test.service.cleanup_quarantine().unwrap();

        assert!(test.pet_exists("pet-a"));
        assert!(test.job_exists("job-a"));
        assert!(test.pet_dir("pet-a").exists());
        assert!(test.job_dir("job-a").exists());
        assert!(test.session_dir("session-a").join("draft.txt").exists());
        assert!(!operation.exists());
    }

    #[test]
    fn startup_cleanup_discards_prepared_creation_quarantine_after_database_commit() {
        let test = DeletionHarness::two_pets();
        let operation = test.quarantined_creation_operation("prepared");
        test.service
            .delete_rows("pet-a", &["job-a".into()], &["session-a".into()])
            .unwrap();

        test.service.cleanup_quarantine().unwrap();

        assert!(!test.pet_exists("pet-a"));
        assert!(!test.pet_dir("pet-a").exists());
        assert!(!test.job_dir("job-a").exists());
        assert!(!test.session_dir("session-a").exists());
        assert!(!operation.exists());
    }

    #[test]
    fn startup_cleanup_keeps_conflicted_job_quarantine_but_restores_pet() {
        let test = DeletionHarness::two_pets();
        let operation = test.quarantined_operation("quarantined");
        std::fs::create_dir_all(test.job_dir("job-a")).unwrap();

        assert!(test.service.cleanup_quarantine().is_err());

        assert!(test.pet_dir("pet-a").exists());
        assert!(test.job_dir("job-a").exists());
        assert!(operation.join("journal.json").exists());
        assert!(operation.join("job-job-a").exists());
    }

    #[test]
    fn startup_cleanup_preserves_unknown_or_corrupt_operations() {
        let test = DeletionHarness::two_pets();
        let missing = test.root.join("trash").join("pet-delete").join("missing");
        std::fs::create_dir_all(&missing).unwrap();
        let corrupt = test.root.join("trash").join("pet-delete").join("corrupt");
        std::fs::create_dir_all(&corrupt).unwrap();
        std::fs::write(corrupt.join("journal.json"), b"not-json").unwrap();
        let invalid = test.root.join("trash").join("pet-delete").join("invalid");
        std::fs::create_dir_all(&invalid).unwrap();
        std::fs::write(
            invalid.join("journal.json"),
            br#"{"petId":"../pet-a","jobIds":[],"phase":"prepared"}"#,
        )
        .unwrap();

        assert!(test.service.cleanup_quarantine().is_err());

        assert!(missing.exists());
        assert!(corrupt.exists());
        assert!(invalid.exists());
    }

    #[test]
    fn interrupted_journal_publish_recovers_the_previous_generation() {
        let test = DeletionHarness::two_pets();
        let operation = test
            .root
            .join("trash")
            .join("pet-delete")
            .join("journal-interrupted");
        let journal = DeletionJournal {
            pet_id: "pet-a".into(),
            job_ids: vec!["job-a".into()],
            session_ids: Vec::new(),
            phase: DeletionPhase::Prepared,
        };
        write_journal(&operation, &journal).unwrap();
        std::fs::rename(
            operation.join(JOURNAL_FILE),
            operation.join("journal.previous.json"),
        )
        .unwrap();

        assert_eq!(read_journal(&operation).unwrap().pet_id, "pet-a");
    }

    #[test]
    fn unconfirmed_initial_prepared_journal_never_starts_resource_isolation() {
        let test =
            DeletionHarness::with_journal_publish_ops(Arc::new(InterruptingJournalPublishOps {
                interrupt_after: JournalPublishStep::StagingSynced,
            }));
        test.bind_pet_a_job_to_creation_session();

        assert!(test.service.delete("pet-a").is_err());

        assert!(test.pet_exists("pet-a"));
        assert!(test.job_exists("job-a"));
        assert!(test.session_exists("session-a"));
        assert!(test.pet_dir("pet-a").join("assets/asset.txt").exists());
        assert!(test.job_dir("job-a").join("result.txt").exists());
        assert!(test.session_dir("session-a").join("draft.txt").exists());
    }

    #[test]
    fn each_owned_root_publish_failure_stops_before_later_roots_journal_or_isolation() {
        for fail_call in 1..=3 {
            let calls = Arc::new(AtomicU32::new(0));
            let test =
                DeletionHarness::with_journal_publish_ops(Arc::new(FailBeforeDurableRenameOps {
                    fail_call,
                    calls: calls.clone(),
                }));
            test.bind_pet_a_job_to_creation_session();

            assert!(test.service.delete("pet-a").is_err());

            assert_eq!(calls.load(Ordering::SeqCst), fail_call);
            assert!(test.pet_exists("pet-a"));
            assert!(test.job_exists("job-a"));
            assert!(test.session_exists("session-a"));
            assert!(test.pet_dir("pet-a").join("assets/asset.txt").exists());
            assert!(test.job_dir("job-a").join("result.txt").exists());
            assert!(test.session_dir("session-a").join("draft.txt").exists());

            let trash = test.root.join("trash");
            let delete_root = trash.join("pet-delete");
            match fail_call {
                1 => assert!(!trash.exists()),
                2 => {
                    assert!(trash.exists());
                    assert!(!delete_root.exists());
                    assert_eq!(std::fs::read_dir(&trash).unwrap().count(), 0);
                }
                3 => {
                    assert!(delete_root.exists());
                    assert_eq!(std::fs::read_dir(&delete_root).unwrap().count(), 0);
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn each_post_move_root_barrier_failure_keeps_only_the_valid_published_prefix() {
        for fail_call in 1..=3 {
            let calls = Arc::new(AtomicU32::new(0));
            let test =
                DeletionHarness::with_journal_publish_ops(Arc::new(FailAfterDurableRenameOps {
                    fail_call,
                    calls: calls.clone(),
                }));
            test.bind_pet_a_job_to_creation_session();

            assert!(test.service.delete("pet-a").is_err());

            assert_eq!(calls.load(Ordering::SeqCst), fail_call);
            assert!(test.pet_exists("pet-a"));
            assert!(test.job_exists("job-a"));
            assert!(test.session_exists("session-a"));
            assert!(test.pet_dir("pet-a").join("assets/asset.txt").exists());
            assert!(test.job_dir("job-a").join("result.txt").exists());
            assert!(test.session_dir("session-a").join("draft.txt").exists());

            let trash = test.root.join("trash");
            let delete_root = trash.join("pet-delete");
            validate_existing_owned_root(&test.root, &trash).unwrap();
            if fail_call == 1 {
                assert_eq!(std::fs::read_dir(&trash).unwrap().count(), 0);
                continue;
            }
            validate_existing_owned_root(&trash, &delete_root).unwrap();
            if fail_call == 2 {
                assert_eq!(std::fs::read_dir(&delete_root).unwrap().count(), 0);
                continue;
            }
            let operations: Vec<_> = std::fs::read_dir(&delete_root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            assert_eq!(operations.len(), 1);
            validate_existing_owned_root(&delete_root, &operations[0]).unwrap();
            assert!(!operations[0].join(JOURNAL_FILE).exists());
            assert!(!operations[0].join(PREVIOUS_JOURNAL_FILE).exists());
        }
    }

    #[test]
    fn retry_requires_each_leftover_published_root_barrier_before_progressing() {
        for failed_layer in 1..=3 {
            let test = DeletionHarness::two_pets();
            test.bind_pet_a_job_to_creation_session();
            let operation = test
                .root
                .join("trash")
                .join("pet-delete")
                .join(format!("retry-operation-{failed_layer}"));
            let first_calls = Arc::new(AtomicU32::new(0));
            let first = FailAfterDurableRenameOps {
                fail_call: failed_layer,
                calls: first_calls,
            };
            assert!(prepare_quarantine_root(&test.root, &operation, &first).is_err());

            let retry_calls = Arc::new(AtomicU32::new(0));
            let retry = FailExistingRootSyncOps {
                fail_call: failed_layer,
                calls: retry_calls.clone(),
            };
            assert!(prepare_quarantine_root(&test.root, &operation, &retry).is_err());
            assert_eq!(retry_calls.load(Ordering::SeqCst), failed_layer);
            assert!(!operation.join(JOURNAL_FILE).exists());
            assert!(test.pet_exists("pet-a"));
            assert!(test.pet_dir("pet-a").exists());
            assert!(test.job_dir("job-a").exists());
            assert!(test.session_dir("session-a").exists());

            prepare_quarantine_root(&test.root, &operation, &PlatformJournalPublishOps).unwrap();
            test.service.delete("pet-a").unwrap();
            assert!(!test.pet_exists("pet-a"));
        }
    }

    #[test]
    fn existing_root_barriers_run_in_order_before_publishing_the_missing_operation() {
        let test = DeletionHarness::two_pets();
        let trash = test.root.join("trash");
        let delete_root = trash.join("pet-delete");
        let operation = delete_root.join("existing-root-order");
        std::fs::create_dir_all(&delete_root).unwrap();
        let ops = RecordingDirectoryOps::default();

        prepare_quarantine_root(&test.root, &operation, &ops).unwrap();

        assert_eq!(
            *ops.existing_syncs.lock().unwrap(),
            vec![trash, delete_root]
        );
        assert_eq!(*ops.publishes.lock().unwrap(), vec![operation]);
    }

    #[test]
    fn resource_barrier_failure_after_os_move_restores_before_database_deletion() {
        let calls = Arc::new(AtomicU32::new(0));
        let test = DeletionHarness::with_journal_publish_ops(Arc::new(FailAfterDurableRenameOps {
            fail_call: 5,
            calls,
        }));
        test.bind_pet_a_job_to_creation_session();

        assert!(test.service.delete("pet-a").is_err());

        assert!(test.pet_exists("pet-a"));
        assert!(test.job_exists("job-a"));
        assert!(test.session_exists("session-a"));
        assert!(test.pet_dir("pet-a").join("assets/asset.txt").exists());
        assert!(test.job_dir("job-a").join("result.txt").exists());
        assert!(test.session_dir("session-a").join("draft.txt").exists());
    }

    #[test]
    fn unknown_resource_ownership_keeps_the_operation_journal_for_later_recovery() {
        let test = DeletionHarness::two_pets();
        let operation = test
            .root
            .join("trash")
            .join("pet-delete")
            .join("unknown-resource-ownership");
        let journal = DeletionJournal {
            pet_id: "pet-a".into(),
            job_ids: Vec::new(),
            session_ids: Vec::new(),
            phase: DeletionPhase::Prepared,
        };
        write_journal(&operation, &journal).unwrap();
        let missing = QuarantinedPath {
            original: test.root.join("missing-original"),
            quarantined: operation.join("missing-quarantine"),
            original_parent: test.root.clone(),
            quarantine_parent: operation.clone(),
        };

        let error = recover_uncommitted(&operation, "barrier result unknown".into(), &[missing]);

        assert!(error.contains("exists at neither original nor quarantine path"));
        assert!(operation.exists());
        assert!(operation.join(JOURNAL_FILE).exists());
    }

    #[test]
    fn interrupted_current_rotation_leaves_previous_generation_readable() {
        let test = DeletionHarness::two_pets();
        let operation = test
            .root
            .join("trash")
            .join("pet-delete")
            .join("journal-current-rotation");
        let prepared = DeletionJournal {
            pet_id: "pet-a".into(),
            job_ids: vec!["job-a".into()],
            session_ids: Vec::new(),
            phase: DeletionPhase::Prepared,
        };
        write_journal(&operation, &prepared).unwrap();
        let committed = DeletionJournal {
            phase: DeletionPhase::Committed,
            ..prepared.clone()
        };
        let ops = InterruptingJournalPublishOps {
            interrupt_after: JournalPublishStep::PreviousPublished,
        };

        assert!(write_journal_with_ops(&operation, &committed, &ops).is_err());
        assert_eq!(
            read_journal(&operation).unwrap().phase,
            DeletionPhase::Prepared
        );
    }

    #[test]
    fn interrupted_new_current_publish_prefers_the_new_durable_generation() {
        let test = DeletionHarness::two_pets();
        let operation = test
            .root
            .join("trash")
            .join("pet-delete")
            .join("journal-new-current");
        let prepared = DeletionJournal {
            pet_id: "pet-a".into(),
            job_ids: vec!["job-a".into()],
            session_ids: Vec::new(),
            phase: DeletionPhase::Prepared,
        };
        write_journal(&operation, &prepared).unwrap();
        let committed = DeletionJournal {
            phase: DeletionPhase::Committed,
            ..prepared.clone()
        };
        let ops = InterruptingJournalPublishOps {
            interrupt_after: JournalPublishStep::CurrentPublished,
        };

        assert!(write_journal_with_ops(&operation, &committed, &ops).is_err());
        assert_eq!(
            read_journal(&operation).unwrap().phase,
            DeletionPhase::Committed
        );
        assert_eq!(
            read_valid_journal(&operation.join(PREVIOUS_JOURNAL_FILE))
                .unwrap()
                .phase,
            DeletionPhase::Prepared
        );
    }

    #[test]
    fn committed_database_with_interrupted_journal_publish_cleans_quarantine_without_originals() {
        let test = DeletionHarness::two_pets();
        let operation = test.quarantined_creation_operation("prepared");
        test.service
            .delete_rows("pet-a", &["job-a".into()], &["session-a".into()])
            .unwrap();
        let committed = DeletionJournal {
            pet_id: "pet-a".into(),
            job_ids: vec!["job-a".into()],
            session_ids: vec!["session-a".into()],
            phase: DeletionPhase::Committed,
        };
        let ops = InterruptingJournalPublishOps {
            interrupt_after: JournalPublishStep::PreviousPublished,
        };
        assert!(write_journal_with_ops(&operation, &committed, &ops).is_err());

        test.service.cleanup_quarantine().unwrap();

        assert!(!test.pet_exists("pet-a"));
        assert!(!test.pet_dir("pet-a").exists());
        assert!(!test.job_dir("job-a").exists());
        assert!(!test.session_dir("session-a").exists());
        assert!(!operation.exists());
    }

    #[test]
    fn rotating_with_an_existing_previous_keeps_the_former_current_not_the_stale_backup() {
        let test = DeletionHarness::two_pets();
        let operation = test
            .root
            .join("trash")
            .join("pet-delete")
            .join("journal-existing-previous");
        let prepared = DeletionJournal {
            pet_id: "pet-a".into(),
            job_ids: vec!["job-a".into()],
            session_ids: Vec::new(),
            phase: DeletionPhase::Prepared,
        };
        write_journal(&operation, &prepared).unwrap();
        let quarantined = DeletionJournal {
            phase: DeletionPhase::Quarantined,
            ..prepared.clone()
        };
        write_journal(&operation, &quarantined).unwrap();
        let committed = DeletionJournal {
            phase: DeletionPhase::Committed,
            ..prepared
        };
        let ops = InterruptingJournalPublishOps {
            interrupt_after: JournalPublishStep::PreviousPublished,
        };

        assert!(write_journal_with_ops(&operation, &committed, &ops).is_err());
        assert_eq!(
            read_journal(&operation).unwrap().phase,
            DeletionPhase::Quarantined
        );
    }

    #[test]
    fn startup_cleanup_falls_back_to_valid_previous_when_current_journal_is_corrupt() {
        let test = DeletionHarness::two_pets();
        let operation = test.quarantined_creation_operation("prepared");
        std::fs::rename(
            operation.join(JOURNAL_FILE),
            operation.join(PREVIOUS_JOURNAL_FILE),
        )
        .unwrap();
        std::fs::write(operation.join(JOURNAL_FILE), b"truncated-json").unwrap();

        test.service.cleanup_quarantine().unwrap();

        assert!(test.pet_exists("pet-a"));
        assert!(test.job_exists("job-a"));
        assert!(test.session_exists("session-a"));
        assert!(test.pet_dir("pet-a").join("assets/asset.txt").exists());
        assert!(test.job_dir("job-a").join("result.txt").exists());
        assert!(test.session_dir("session-a").join("draft.txt").exists());
        assert!(!operation.exists());
    }

    #[test]
    fn startup_cleanup_prefers_valid_current_over_stale_previous_journal() {
        let test = DeletionHarness::two_pets();
        let operation = test.quarantined_creation_operation("prepared");
        std::fs::write(
            operation.join(PREVIOUS_JOURNAL_FILE),
            br#"{"petId":"pet-b","jobIds":["job-b"],"sessionIds":[],"phase":"committed"}"#,
        )
        .unwrap();

        test.service.cleanup_quarantine().unwrap();

        assert!(test.pet_exists("pet-a"));
        assert!(test.job_exists("job-a"));
        assert!(test.session_exists("session-a"));
        assert!(test.pet_dir("pet-a").join("assets/asset.txt").exists());
        assert!(test.job_dir("job-a").join("result.txt").exists());
        assert!(test.session_dir("session-a").join("draft.txt").exists());
        assert!(!operation.exists());
    }

    #[test]
    fn current_validation_failure_falls_back_and_both_errors_keep_their_context() {
        let test = DeletionHarness::two_pets();
        let operation = test
            .root
            .join("trash")
            .join("pet-delete")
            .join("journal-validation-fallback");
        std::fs::create_dir_all(&operation).unwrap();
        std::fs::write(
            operation.join(JOURNAL_FILE),
            br#"{"petId":"../pet-a","jobIds":[],"sessionIds":[],"phase":"prepared"}"#,
        )
        .unwrap();
        std::fs::write(
            operation.join(PREVIOUS_JOURNAL_FILE),
            br#"{"petId":"pet-a","jobIds":["job-a"],"sessionIds":[],"phase":"prepared"}"#,
        )
        .unwrap();

        assert_eq!(read_journal(&operation).unwrap().pet_id, "pet-a");

        std::fs::write(operation.join(PREVIOUS_JOURNAL_FILE), b"also-corrupt").unwrap();
        let error = read_journal(&operation).unwrap_err();
        assert!(error.contains("current journal invalid: invalid journal pet id"));
        assert!(error.contains("previous journal invalid: invalid journal"));
    }

    #[test]
    fn journal_publish_rejects_current_and_previous_reparse_points() {
        for journal_name in [JOURNAL_FILE, PREVIOUS_JOURNAL_FILE] {
            let test = DeletionHarness::two_pets();
            let operation = test
                .root
                .join("trash")
                .join("pet-delete")
                .join(format!("journal-link-{journal_name}"));
            let outside = test.root.join(format!("outside-{journal_name}"));
            std::fs::create_dir_all(&operation).unwrap();
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("sentinel.txt"), "outside").unwrap();
            crate::platform::create_directory_link(&outside, &operation.join(journal_name));
            let journal = DeletionJournal {
                pet_id: "pet-a".into(),
                job_ids: Vec::new(),
                session_ids: Vec::new(),
                phase: DeletionPhase::Prepared,
            };

            assert!(write_journal(&operation, &journal).is_err());
            assert_eq!(
                std::fs::read_to_string(outside.join("sentinel.txt")).unwrap(),
                "outside"
            );
        }
    }

    #[test]
    fn failed_journal_replacement_keeps_the_old_generation_recoverable() {
        let test = DeletionHarness::two_pets();
        let operation = test
            .root
            .join("trash")
            .join("pet-delete")
            .join("journal-write-failure");
        let prepared = DeletionJournal {
            pet_id: "pet-a".into(),
            job_ids: vec!["job-a".into()],
            session_ids: Vec::new(),
            phase: DeletionPhase::Prepared,
        };
        write_journal(&operation, &prepared).unwrap();
        std::fs::create_dir(operation.join("journal.previous.json")).unwrap();
        let committed = DeletionJournal {
            phase: DeletionPhase::Committed,
            ..prepared.clone()
        };

        assert!(write_journal(&operation, &committed).is_err());
        assert_eq!(
            read_journal(&operation).unwrap().phase,
            DeletionPhase::Prepared
        );
    }

    #[test]
    fn missing_artifact_directories_do_not_block_database_deletion() {
        let test = DeletionHarness::two_pets();
        std::fs::remove_dir_all(test.pet_dir("pet-a")).unwrap();
        std::fs::remove_dir_all(test.job_dir("job-a")).unwrap();

        let outcome = test.service.delete("pet-a").unwrap();
        assert_eq!(outcome.warning, None);
        assert!(!test.pet_exists("pet-a"));
        assert!(test.pet_exists("pet-b"));
        assert!(test.pet_dir("pet-b").exists());
        assert!(test.job_dir("job-b").exists());
    }

    #[test]
    fn cleanup_quarantine_preserves_unknown_direct_operation_directories() {
        let test = DeletionHarness::two_pets();
        let quarantine = test.root.join("trash").join("pet-delete");
        std::fs::create_dir_all(quarantine.join("operation")).unwrap();
        std::fs::write(quarantine.join("keep.txt"), b"keep").unwrap();
        assert!(test.service.cleanup_quarantine().is_err());
        assert!(quarantine.join("operation").exists());
        assert!(quarantine.join("keep.txt").exists());
    }

    #[test]
    fn rejects_path_escape_pet_identifiers_without_touching_siblings() {
        let test = DeletionHarness::two_pets();
        assert!(test.service.delete("../pet-b").is_err());
        assert!(test.pet_exists("pet-b"));
        assert!(test.pet_dir("pet-b").exists());
    }

    #[test]
    fn rejects_database_job_path_escape_without_touching_outside_sentinel() {
        let test = DeletionHarness::two_pets();
        let malicious_job_id = "../outside-job";
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO generation_jobs
                 (job_id, pet_id, prompt, ref_sha256, status, created_at)
                 VALUES (?1, 'pet-a', 'prompt', 'hash', 'done', '0')",
                rusqlite::params![malicious_job_id],
            )
            .unwrap();
        let sentinel = test.root.join("outside-job");
        std::fs::create_dir_all(&sentinel).unwrap();
        std::fs::write(sentinel.join("sentinel.txt"), b"must remain").unwrap();

        assert!(test.service.delete("pet-a").is_err());
        assert!(test.pet_exists("pet-a"));
        assert!(test.job_exists("job-a"));
        assert!(test.job_exists(malicious_job_id));
        assert!(test.pet_dir("pet-a").exists());
        assert!(test.job_dir("job-a").exists());
        assert_eq!(
            std::fs::read(sentinel.join("sentinel.txt")).unwrap(),
            b"must remain"
        );
    }
}
