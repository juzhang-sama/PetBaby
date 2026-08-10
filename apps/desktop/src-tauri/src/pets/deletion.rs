use crate::pets::active::{SharedActivePetService, BUILTIN_PET_ID};
use crate::storage::Storage;
use rusqlite::OptionalExtension;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static DELETION_OPERATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
const JOURNAL_FILE: &str = "journal.json";

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
}

#[derive(Debug)]
struct QuarantinedPath {
    original: PathBuf,
    quarantined: PathBuf,
    original_parent: PathBuf,
    quarantine_parent: PathBuf,
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

impl PetDeletionService {
    pub fn new(
        storage: Arc<Mutex<Storage>>,
        active: SharedActivePetService,
        app_data_dir: PathBuf,
    ) -> Self {
        Self {
            storage,
            active,
            app_data_dir,
        }
    }

    pub fn delete(&self, pet_id: &str) -> Result<DeleteOutcome, String> {
        let _operation = deletion_operation_lock()
            .lock()
            .map_err(|_| "deletion operation lock poisoned")?;
        validate_component(pet_id, "pet id")?;
        if pet_id == BUILTIN_PET_ID {
            return Err("the built-in pet cannot be deleted".into());
        }

        let job_ids = self.require_deletable_pet(pet_id)?;
        let quarantine_root = self.quarantine_root();
        let mut journal = DeletionJournal {
            pet_id: pet_id.into(),
            job_ids: job_ids.clone(),
            session_ids: Vec::new(),
            phase: DeletionPhase::Prepared,
        };
        write_journal(&quarantine_root, &journal)?;
        let pets_root = self.app_data_dir.join("pets");
        let jobs_root = self.app_data_dir.join("jobs");
        let mut planned_paths = vec![(pets_root.join(pet_id), pets_root, "pet".to_owned())];
        for job_id in &job_ids {
            validate_component(job_id, "job id")?;
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
            match quarantine_path(source, &quarantine_root, name) {
                Ok(Some(path)) => quarantined.push(path),
                Ok(None) => {}
                Err(error) => {
                    return Err(recover_uncommitted(
                        &quarantine_root,
                        format!("failed to quarantine {}: {error}", source.display()),
                        &quarantined,
                    ));
                }
            }
        }

        if let Err(error) = self.delete_rows(pet_id, &job_ids) {
            return Err(recover_uncommitted(&quarantine_root, error, &quarantined));
        }

        journal.phase = DeletionPhase::Committed;
        let mut warnings = Vec::new();
        if let Err(error) = write_journal(&quarantine_root, &journal) {
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
        let _operation = deletion_operation_lock()
            .lock()
            .map_err(|_| "deletion operation lock poisoned")?;
        validate_component(session_id, "session id")?;

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
        let mut journal = DeletionJournal {
            pet_id: pet_id.clone(),
            job_ids: job_ids.clone(),
            session_ids: vec![session_id.into()],
            phase: DeletionPhase::Prepared,
        };
        write_journal(&quarantine_root, &journal)?;

        let pets_root = self.app_data_dir.join("pets");
        let jobs_root = self.app_data_dir.join("jobs");
        let sessions_root = self.app_data_dir.join("creation-sessions");
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
            match quarantine_path(source, &quarantine_root, name) {
                Ok(Some(path)) => quarantined.push(path),
                Ok(None) => {}
                Err(error) => {
                    return Err(recover_uncommitted(
                        &quarantine_root,
                        format!("failed to quarantine {}: {error}", source.display()),
                        &quarantined,
                    ));
                }
            }
        }

        if let Err(error) = self.abandon_creation_rows(session_id, &pet_id, &method, &job_ids) {
            return Err(recover_uncommitted(&quarantine_root, error, &quarantined));
        }

        journal.phase = DeletionPhase::Committed;
        if let Err(error) = write_journal(&quarantine_root, &journal) {
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
        let _operation = deletion_operation_lock()
            .lock()
            .map_err(|_| "deletion operation lock poisoned")?;
        let root = self.app_data_dir.join("trash").join("pet-delete");
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

    fn require_deletable_pet(&self, pet_id: &str) -> Result<Vec<String>, String> {
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

        let mut statement = storage
            .db
            .prepare("SELECT job_id FROM generation_jobs WHERE pet_id = ?1")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(rusqlite::params![pet_id], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        rows.map(|row| row.map_err(|error| error.to_string()))
            .collect()
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
        let job_ids = {
            let mut statement = storage
                .db
                .prepare("SELECT job_id FROM generation_jobs WHERE pet_id=?1 ORDER BY job_id")
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([&pet_id], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?
                .map(|row| row.map_err(|error| error.to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
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
        let current: Option<(String, String, String)> = tx
            .query_row(
                "SELECT pet_id, method, status FROM creation_sessions WHERE session_id=?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (current_pet_id, current_method, current_status) = current
            .ok_or_else(|| format!("creation session changed during abandonment: {session_id}"))?;
        if current_pet_id != pet_id || current_method != method {
            return Err("creation session ownership changed during abandonment".into());
        }
        if current_status == "completed" {
            return Err("a completed creation session cannot be abandoned".into());
        }
        let current_job_ids = {
            let mut statement = tx
                .prepare("SELECT job_id FROM generation_jobs WHERE pet_id=?1 ORDER BY job_id")
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([pet_id], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?
                .map(|row| row.map_err(|error| error.to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
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
        let affected = tx
            .execute("DELETE FROM pets WHERE pet_id=?1", [pet_id])
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("reserved pet changed during abandonment".into());
        }
        tx.commit().map_err(|error| error.to_string())
    }

    fn delete_rows(&self, pet_id: &str, expected_job_ids: &[String]) -> Result<(), String> {
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
        let current_job_ids = {
            let mut statement = tx
                .prepare("SELECT job_id FROM generation_jobs WHERE pet_id = ?1 ORDER BY job_id")
                .map_err(|error| error.to_string())?;
            let job_ids = statement
                .query_map(rusqlite::params![pet_id], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?
                .map(|row| row.map_err(|error| error.to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            job_ids
        };
        let mut expected_job_ids = expected_job_ids.to_vec();
        expected_job_ids.sort();
        if current_job_ids != expected_job_ids {
            return Err("generation jobs changed during deletion".into());
        }
        tx.execute(
            "DELETE FROM variants WHERE pet_id = ?1",
            rusqlite::params![pet_id],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "DELETE FROM appearance_variants WHERE pet_id = ?1",
            rusqlite::params![pet_id],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "DELETE FROM generation_jobs WHERE pet_id = ?1",
            rusqlite::params![pet_id],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "DELETE FROM identity_profiles WHERE pet_id = ?1",
            rusqlite::params![pet_id],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "DELETE FROM state WHERE key = ?1",
            rusqlite::params![format!("creation:{pet_id}:compile_error")],
        )
        .map_err(|error| error.to_string())?;
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
        restore_all(&paths)?;
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

fn deletion_operation_lock() -> &'static Mutex<()> {
    DELETION_OPERATION_LOCK.get_or_init(|| Mutex::new(()))
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

fn validate_path_parent(path: &Path, expected_parent: &Path) -> Result<(), String> {
    if path.exists() && !path.is_dir() {
        return Err(format!("refusing non-directory source: {}", path.display()));
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

fn quarantine_path(
    source: &Path,
    root: &Path,
    name: &str,
) -> Result<Option<QuarantinedPath>, String> {
    if !source.exists() {
        return Ok(None);
    }
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let target = root.join(name);
    std::fs::rename(source, &target).map_err(|error| error.to_string())?;
    Ok(Some(QuarantinedPath {
        original: source.into(),
        quarantined: target,
        original_parent: source
            .parent()
            .ok_or_else(|| "deletion source has no parent".to_string())?
            .into(),
        quarantine_parent: root.into(),
    }))
}

fn restore_all(paths: &[QuarantinedPath]) -> Result<(), String> {
    let mut errors = Vec::new();
    for path in paths.iter().rev() {
        if !path.quarantined.exists() {
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
        if let Err(error) = std::fs::rename(&path.quarantined, &path.original) {
            errors.push(format!("{}: {error}", path.original.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn recover_uncommitted(root: &Path, error: String, paths: &[QuarantinedPath]) -> String {
    match restore_all(paths) {
        Ok(()) => match remove_operation(root) {
            Ok(()) => error,
            Err(cleanup_error) => {
                format!("{error}; restored data but cleanup failed: {cleanup_error}")
            }
        },
        Err(restore_error) => format!("{error}; quarantine restore failed: {restore_error}"),
    }
}

fn write_journal(root: &Path, journal: &DeletionJournal) -> Result<(), String> {
    validate_journal(journal)?;
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(journal).map_err(|error| error.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(root.join(JOURNAL_FILE))
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn read_journal(operation: &Path) -> Result<DeletionJournal, String> {
    let path = operation.join(JOURNAL_FILE);
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("journal is not a regular file".into());
    }
    let journal = serde_json::from_slice(&std::fs::read(path).map_err(|error| error.to_string())?)
        .map_err(|error| format!("invalid journal: {error}"))?;
    validate_journal(&journal)?;
    Ok(journal)
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
    use crate::pets::{active::ActivePetService, ActivePetSession, SharedActivePetSession};
    use crate::storage::Storage;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct DeletionHarness {
        root: PathBuf,
        storage: Arc<Mutex<Storage>>,
        service: PetDeletionService,
    }

    impl DeletionHarness {
        fn two_pets() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let root = std::env::temp_dir()
                .join(format!("desktop-pet-deletion-{}-{n}", std::process::id()));
            let pets_dir = root.join("pets");
            let storage = Arc::new(Mutex::new(Storage::open(&pets_dir).unwrap()));
            let session: SharedActivePetSession = Arc::new(Mutex::new(ActivePetSession::new()));
            session
                .lock()
                .unwrap()
                .set_active(BUILTIN_PET_ID.into())
                .unwrap();
            let active = Arc::new(ActivePetService::new(storage.clone(), session, pets_dir));
            let service = PetDeletionService::new(storage.clone(), active, root.clone());
            let test = Self {
                root,
                storage,
                service,
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
            .delete_rows("pet-a", &["job-a".into()])
            .unwrap_err();
        assert!(error.contains("active pet"));
        assert!(recover_uncommitted(&quarantine, error, &quarantined).contains("active pet"));
        assert!(test.pet_exists("pet-a"));
        assert!(test.pet_dir("pet-a").exists());
        assert!(test.job_dir("job-a").exists());
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
            .delete_rows("pet-a", &["job-a".into()])
            .unwrap();

        test.service.cleanup_quarantine().unwrap();

        assert!(!test.pet_exists("pet-a"));
        assert!(!test.pet_dir("pet-a").exists());
        assert!(!test.job_dir("job-a").exists());
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
