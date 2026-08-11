use crate::creation::candidate;
use crate::creation::content::ContentRoot;
use crate::creation::domain::{
    new_entity_id, CreationMethod, CreationSessionStatus, CreationSnapshot,
};
use crate::creation::name::normalize_display_name;
use crate::creation::store::CreationStore;
use crate::pets::active::BUILTIN_PET_ID;
use crate::pets::mutation::{MutationKind, SharedPetMutationGate};
use crate::pets::repository::PetRepository;
use crate::runtime_assets::motion_profile::parse_motion_profile;
use crate::storage::Storage;
use image::ImageDecoder as _;
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use unicode_segmentation::UnicodeSegmentation;

#[cfg(windows)]
use std::os::windows::{
    fs::OpenOptionsExt,
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
};

const CATALOG_SCHEMA_VERSION: u32 = 1;
const RUNTIME_SCHEMA_VERSION: u32 = 3;
const TEMPLATE_COUNT: usize = 8;
const MAX_CATALOG_BYTES: u64 = 256 * 1024;
const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;
const MAX_THUMBNAIL_BYTES: u64 = 5 * 1024 * 1024;
const MAX_MOTION_PROFILE_BYTES: u64 = 64 * 1024;
const MAX_PERSONALITY_GRAPHEMES: usize = 200;

static ADOPTION_PUBLICATION_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

#[cfg(test)]
thread_local! {
    static TEST_RESERVATION_BARRIER: std::cell::RefCell<Option<Arc<std::sync::Barrier>>> =
        const { std::cell::RefCell::new(None) };
    static TEST_UNIQUE_LOSER_COUNTER: std::cell::RefCell<Option<Arc<std::sync::atomic::AtomicUsize>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn set_thread_adoption_reservation_barrier(
    barrier: Arc<std::sync::Barrier>,
    unique_loser_counter: Arc<std::sync::atomic::AtomicUsize>,
) {
    TEST_RESERVATION_BARRIER.with(|slot| *slot.borrow_mut() = Some(barrier));
    TEST_UNIQUE_LOSER_COUNTER.with(|slot| *slot.borrow_mut() = Some(unique_loser_counter));
}

#[cfg(test)]
fn wait_at_test_reservation_barrier() {
    TEST_RESERVATION_BARRIER.with(|slot| {
        if let Some(barrier) = slot.borrow_mut().take() {
            barrier.wait();
        }
    });
}

#[cfg(not(test))]
fn wait_at_test_reservation_barrier() {}

#[cfg(test)]
fn record_test_unique_loser() {
    TEST_UNIQUE_LOSER_COUNTER.with(|slot| {
        if let Some(counter) = slot.borrow_mut().take() {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    });
}

#[cfg(not(test))]
fn record_test_unique_loser() {}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdoptionTemplate {
    pub template_id: String,
    pub template_version: u32,
    pub runtime_schema_version: u32,
    pub default_name: String,
    pub personality: String,
    pub thumbnail_path: String,
    pub body_path: String,
    pub motion_profile_path: String,
    pub thumbnail_sha256: String,
    pub body_sha256: String,
    pub motion_profile_sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionCatalogEntry {
    pub template: AdoptionTemplate,
    pub adopted_pet_id: Option<String>,
    pub retry_session_id: Option<String>,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdoptionCatalogManifest {
    schema_version: u32,
    templates: Vec<AdoptionTemplate>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
struct StrictMotionProfileShape {
    profile_version: u32,
    engine_profile: String,
    alpha_bounds: StrictRectShape,
    breath_zone: StrictRectShape,
    sway_pivot: StrictPointShape,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct StrictRectShape {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct StrictPointShape {
    x: f32,
    y: f32,
}

struct ValidatedTemplate {
    template: AdoptionTemplate,
    body: Vec<u8>,
    motion_profile: Vec<u8>,
}

enum LoadedTemplate {
    Available(ValidatedTemplate),
    Unavailable {
        template: AdoptionTemplate,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogSecurityFailure {
    Access,
    InvalidPath,
    ReparsePoint,
    PathContainment,
    IdentityChanged,
    BoundedRead,
}

#[derive(Debug)]
struct CatalogSecurityError {
    kind: CatalogSecurityFailure,
    reason: String,
}

impl CatalogSecurityError {
    fn new(kind: CatalogSecurityFailure, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for CatalogSecurityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

#[derive(Debug)]
struct CatalogFileIoError {
    kind: std::io::ErrorKind,
    reason: String,
}

impl CatalogFileIoError {
    fn new(kind: std::io::ErrorKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for CatalogFileIoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

#[derive(Debug)]
enum CatalogReadFailure {
    Security(CatalogSecurityError),
    FileIo(CatalogFileIoError),
}

impl From<CatalogSecurityError> for CatalogReadFailure {
    fn from(error: CatalogSecurityError) -> Self {
        Self::Security(error)
    }
}

impl From<CatalogFileIoError> for CatalogReadFailure {
    fn from(error: CatalogFileIoError) -> Self {
        Self::FileIo(error)
    }
}

impl std::fmt::Display for CatalogReadFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Security(error) => error.fmt(formatter),
            Self::FileIo(error) => error.fmt(formatter),
        }
    }
}

enum TemplateAssetFailure {
    Security(CatalogSecurityError),
    FileIo(CatalogFileIoError),
    Content(String),
}

impl From<CatalogSecurityError> for TemplateAssetFailure {
    fn from(error: CatalogSecurityError) -> Self {
        Self::Security(error)
    }
}

impl From<CatalogReadFailure> for TemplateAssetFailure {
    fn from(error: CatalogReadFailure) -> Self {
        match error {
            CatalogReadFailure::Security(error) => Self::Security(error),
            CatalogReadFailure::FileIo(error) => Self::FileIo(error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateAssetFailureScope {
    WholeCatalog,
    SingleTemplate,
}

fn template_asset_failure_scope(failure: &TemplateAssetFailure) -> TemplateAssetFailureScope {
    match failure {
        TemplateAssetFailure::Security(error) => {
            let _kind = error.kind;
            TemplateAssetFailureScope::WholeCatalog
        }
        TemplateAssetFailure::FileIo(error) => {
            let _kind = error.kind;
            TemplateAssetFailureScope::SingleTemplate
        }
        TemplateAssetFailure::Content(_) => TemplateAssetFailureScope::SingleTemplate,
    }
}

impl LoadedTemplate {
    fn template(&self) -> &AdoptionTemplate {
        match self {
            Self::Available(validated) => &validated.template,
            Self::Unavailable { template, .. } => template,
        }
    }
}

#[derive(Debug)]
struct AdoptionFact {
    pet_id: String,
    pet_schema_version: i64,
    species: String,
    identity_mode: String,
    creation_method: String,
    source_template_id: Option<String>,
    source_template_version: u32,
    lifecycle: String,
    pet_completed_at: Option<String>,
    session_id: Option<String>,
    session_method: Option<String>,
    status: Option<String>,
    last_stable_status: Option<String>,
    session_schema_version: Option<i64>,
    session_completed_at: Option<String>,
    provenance_session_id: Option<String>,
    provenance_source_template_id: Option<String>,
    provenance_source_template_version: Option<u32>,
    provenance_runtime_schema_version: Option<u32>,
    provenance_body_sha256: Option<String>,
    provenance_motion_profile_sha256: Option<String>,
    session_candidate_count: i64,
    pet_candidate_count: i64,
    candidate_count: i64,
    unaccepted_count: i64,
    accepted_count: i64,
    accepted_runtime_count: i64,
    accepted_owned_runtime_count: i64,
    accepted_animated_runtime_count: i64,
    accepted_runtime_manifest_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdoptionFactState {
    Adopted,
    Retryable,
    IgnoredAbandoned,
}

struct AdoptionReservation {
    session_id: String,
    pet_id: String,
    source_template_version: u32,
    newly_created: bool,
}

#[derive(Debug)]
struct AdoptionProvenance {
    source_template_id: String,
    source_template_version: u32,
    runtime_schema_version: u32,
    body_sha256: String,
    motion_profile_sha256: String,
}

pub(crate) fn catalog(
    storage: &Arc<Mutex<Storage>>,
    app_data_dir: &Path,
    content_root: &ContentRoot,
) -> Result<Vec<AdoptionCatalogEntry>, String> {
    let templates = load_catalog(content_root)?;
    let storage = storage.lock().map_err(|_| "storage lock poisoned")?;
    templates
        .into_iter()
        .map(|loaded| {
            let facts = adoption_facts(&storage.db, &loaded.template().template_id)?;
            let (adopted_pet_id, retry_session_id) =
                project_facts(&facts, loaded.template(), app_data_dir)?;
            let (template, unavailable_reason) = match loaded {
                LoadedTemplate::Available(validated) => (validated.template, None),
                LoadedTemplate::Unavailable { template, reason } => (template, Some(reason)),
            };
            Ok(AdoptionCatalogEntry {
                template,
                adopted_pet_id,
                retry_session_id,
                unavailable_reason,
            })
        })
        .collect()
}

pub(crate) fn start(
    storage: &Arc<Mutex<Storage>>,
    app_data_dir: &Path,
    content_root: &ContentRoot,
    mutation_gate: &SharedPetMutationGate,
    template_id: &str,
    display_name: &str,
) -> Result<CreationSnapshot, String> {
    validate_template_id(template_id)?;
    let display_name = normalize_display_name(display_name)?;
    let templates = load_catalog(content_root)?;
    let loaded = templates
        .into_iter()
        .find(|candidate| candidate.template().template_id == template_id)
        .ok_or_else(|| format!("adoption template not found: {template_id}"))?;
    let template = match loaded {
        LoadedTemplate::Available(template) => template,
        LoadedTemplate::Unavailable { reason, .. } => {
            return Err(format!("adoption template is unavailable: {reason}"));
        }
    };
    let reservation = reserve_or_load(storage, app_data_dir, &template.template, &display_name)?;
    let request_id = new_entity_id("adoption-start");
    let _operation =
        mutation_gate.scoped(&request_id, MutationKind::Creation, &reservation.pet_id)?;
    let _publication = ADOPTION_PUBLICATION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "adoption publication lock poisoned")?;

    let stable = load_stable_reservation(storage, app_data_dir, &template.template, &reservation)?;
    let provenance = load_adoption_provenance(storage, &stable.snapshot.session_id)?;
    validate_provenance(&provenance, &stable, &template.template)?;
    if stable.source_template_version != template.template.template_version
        && stable.snapshot.candidate_id.is_none()
    {
        return Err(format!(
            "adoption template version {} is unavailable for retry session {}",
            stable.source_template_version, stable.snapshot.session_id
        ));
    }
    if stable.snapshot.candidate_id.is_some() {
        verify_candidate_database_paths(storage, app_data_dir, &stable.snapshot)?;
        candidate::verify_committed_adoption_candidate(
            app_data_dir,
            &stable.snapshot.session_id,
            &provenance.body_sha256,
            &provenance.motion_profile_sha256,
        )?;
        return Ok(stable.snapshot);
    }

    let attempt = (|| {
        candidate::recover_exact_adoption_orphan(
            app_data_dir,
            &stable.snapshot.session_id,
            &template.template.body_sha256,
            &template.template.motion_profile_sha256,
        )?;
        let mut published = candidate::publish_adoption_candidate(
            app_data_dir,
            &stable.snapshot.session_id,
            &template.body,
            &template.motion_profile,
        )?;
        let store = CreationStore::new(storage.clone());
        match store.record_local_candidate(
            &stable.snapshot.session_id,
            &published.body_path,
            &published.motion_profile_path,
        ) {
            Ok(_) => {
                published.commit();
                snapshot_for_session(storage, &stable.snapshot.session_id)
            }
            Err(error) => {
                let rollback = published.rollback();
                Err(match rollback {
                    Ok(()) => error,
                    Err(rollback) => format!("{error}; candidate rollback failed: {rollback}"),
                })
            }
        }
    })();

    match attempt {
        Ok(snapshot) => Ok(snapshot),
        Err(error) => {
            let cleanup =
                cleanup_failed_attempt(storage, app_data_dir, &reservation, &template.template);
            Err(match cleanup {
                Ok(()) => error,
                Err(cleanup) => format!("{error}; adoption cleanup failed: {cleanup}"),
            })
        }
    }
}

struct StableReservation {
    snapshot: CreationSnapshot,
    source_template_version: u32,
}

fn reserve_or_load(
    storage: &Arc<Mutex<Storage>>,
    app_data_dir: &Path,
    template: &AdoptionTemplate,
    display_name: &str,
) -> Result<AdoptionReservation, String> {
    let mut storage = storage.lock().map_err(|_| "storage lock poisoned")?;
    let facts = adoption_facts(&storage.db, &template.template_id)?;
    if let Some(existing) = one_existing_reservation(&facts, template, app_data_dir)? {
        return reservation_from_fact(existing, false);
    }
    wait_at_test_reservation_barrier();
    let tx = storage
        .db
        .transaction()
        .map_err(|error| error.to_string())?;

    let pet = match PetRepository::reserve_in_transaction(
        &tx,
        CreationMethod::Adoption,
        Some((&template.template_id, template.template_version)),
    ) {
        Ok(pet) => pet,
        Err(error) => {
            drop(tx);
            if error.contains("UNIQUE constraint failed") {
                record_test_unique_loser();
            }
            if error.contains("UNIQUE constraint failed")
                || error.contains("database is locked")
                || error.contains("database is busy")
            {
                let winner = adoption_facts(&storage.db, &template.template_id)?;
                if let Some(existing) = one_existing_reservation(&winner, template, app_data_dir)? {
                    return reservation_from_fact(existing, false);
                }
            }
            return Err(error);
        }
    };
    let session_id = new_entity_id("session");
    validate_identifier(&session_id, "session id")?;
    let now = crate::creation::profiles::now_iso();
    let named = tx
        .execute(
            "UPDATE pets SET display_name=?2, updated_at=?3
             WHERE pet_id=?1 AND creation_method='adoption' AND lifecycle='draft'",
            rusqlite::params![pet.pet_id, display_name, now],
        )
        .map_err(|error| error.to_string())?;
    if named != 1 {
        return Err("new adoption pet could not store its normalized name".into());
    }
    tx.execute(
        "INSERT INTO creation_sessions
         (session_id, pet_id, method, status, last_stable_status, current_step,
          schema_version, created_at, updated_at)
         VALUES (?1, ?2, 'adoption', 'draft', 'draft', 'adoption', 1, ?3, ?3)",
        rusqlite::params![session_id, pet.pet_id, now],
    )
    .map_err(|error| error.to_string())?;
    tx.execute(
        "INSERT INTO creation_adoption_provenance
         (session_id, source_template_id, source_template_version,
          runtime_schema_version, body_sha256, motion_profile_sha256, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            session_id,
            template.template_id,
            template.template_version,
            template.runtime_schema_version,
            template.body_sha256,
            template.motion_profile_sha256,
            now,
        ],
    )
    .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(AdoptionReservation {
        session_id,
        pet_id: pet.pet_id,
        source_template_version: template.template_version,
        newly_created: true,
    })
}

fn one_existing_reservation<'a>(
    facts: &'a [AdoptionFact],
    template: &AdoptionTemplate,
    app_data_dir: &Path,
) -> Result<Option<&'a AdoptionFact>, String> {
    let mut existing = None;
    for fact in facts {
        let state = classify_fact(fact, template, app_data_dir)?;
        match state {
            AdoptionFactState::IgnoredAbandoned => continue,
            AdoptionFactState::Adopted | AdoptionFactState::Retryable if existing.is_none() => {
                existing = Some((fact, state));
            }
            AdoptionFactState::Adopted | AdoptionFactState::Retryable => {
                return Err(
                    "contradictory adoption facts: multiple live pets use one template".into(),
                )
            }
        }
    }
    match existing {
        Some((fact, AdoptionFactState::Adopted)) => Err(format!(
            "adoption template already adopted by petId={}",
            fact.pet_id
        )),
        Some((fact, AdoptionFactState::Retryable)) => Ok(Some(fact)),
        None => Ok(None),
        Some((_, AdoptionFactState::IgnoredAbandoned)) => unreachable!(),
    }
}

fn reservation_from_fact(
    fact: &AdoptionFact,
    newly_created: bool,
) -> Result<AdoptionReservation, String> {
    Ok(AdoptionReservation {
        session_id: fact
            .session_id
            .clone()
            .ok_or("contradictory adoption facts: live pet has no session")?,
        pet_id: fact.pet_id.clone(),
        source_template_version: fact.source_template_version,
        newly_created,
    })
}

fn load_stable_reservation(
    storage: &Arc<Mutex<Storage>>,
    app_data_dir: &Path,
    template: &AdoptionTemplate,
    expected: &AdoptionReservation,
) -> Result<StableReservation, String> {
    let storage = storage.lock().map_err(|_| "storage lock poisoned")?;
    let facts = adoption_facts(&storage.db, &template.template_id)?;
    let mut live = facts
        .iter()
        .filter_map(|fact| match classify_fact(fact, template, app_data_dir) {
            Ok(AdoptionFactState::IgnoredAbandoned) => None,
            result => Some(result.map(|_| fact)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if live.len() != 1 {
        return Err("contradictory adoption facts: reservation disappeared or multiplied".into());
    }
    let fact = live.pop().expect("one live adoption fact");
    if fact.pet_id != expected.pet_id || fact.session_id.as_deref() != Some(&expected.session_id) {
        return Err("adoption reservation changed before candidate publication".into());
    }
    match classify_fact(fact, template, app_data_dir)? {
        AdoptionFactState::Adopted => {
            return Err(format!(
                "adoption template already adopted by petId={}",
                fact.pet_id
            ))
        }
        AdoptionFactState::Retryable => {}
        AdoptionFactState::IgnoredAbandoned => unreachable!("ignored facts were filtered"),
    }
    Ok(StableReservation {
        snapshot: snapshot_from_db(&storage.db, &expected.session_id)?,
        source_template_version: fact.source_template_version,
    })
}

fn cleanup_failed_attempt(
    storage: &Arc<Mutex<Storage>>,
    app_data_dir: &Path,
    reservation: &AdoptionReservation,
    template: &AdoptionTemplate,
) -> Result<(), String> {
    candidate::recover_exact_adoption_orphan(
        app_data_dir,
        &reservation.session_id,
        &template.body_sha256,
        &template.motion_profile_sha256,
    )?;
    if !reservation.newly_created {
        return Ok(());
    }
    candidate::remove_empty_adoption_session_directory(app_data_dir, &reservation.session_id)?;
    let mut storage = storage.lock().map_err(|_| "storage lock poisoned")?;
    let tx = storage
        .db
        .transaction()
        .map_err(|error| error.to_string())?;
    let provenance_removed = tx
        .execute(
            "DELETE FROM creation_adoption_provenance WHERE session_id=?1",
            [&reservation.session_id],
        )
        .map_err(|error| error.to_string())?;
    if provenance_removed != 1 {
        return Err("new adoption provenance changed before rollback".into());
    }
    let removed = tx
        .execute(
            "DELETE FROM pets
             WHERE pet_id=?1 AND source_template_id=?2
               AND source_template_version=?3 AND creation_method='adoption'
               AND lifecycle='draft' AND completed_at IS NULL
               AND EXISTS (SELECT 1 FROM creation_sessions cs
                           WHERE cs.session_id=?4 AND cs.pet_id=pets.pet_id
                             AND cs.method='adoption'
                             AND cs.status NOT IN ('completed','abandoned'))",
            rusqlite::params![
                reservation.pet_id,
                template.template_id,
                reservation.source_template_version,
                reservation.session_id
            ],
        )
        .map_err(|error| error.to_string())?;
    if removed != 1 {
        return Err("new adoption reservation changed before rollback".into());
    }
    tx.commit().map_err(|error| error.to_string())
}

fn load_adoption_provenance(
    storage: &Arc<Mutex<Storage>>,
    session_id: &str,
) -> Result<AdoptionProvenance, String> {
    let storage = storage.lock().map_err(|_| "storage lock poisoned")?;
    storage
        .db
        .query_row(
            "SELECT source_template_id, source_template_version, runtime_schema_version,
                    body_sha256, motion_profile_sha256
             FROM creation_adoption_provenance WHERE session_id=?1",
            [session_id],
            |row| {
                let source_version: i64 = row.get(1)?;
                let runtime_version: i64 = row.get(2)?;
                Ok(AdoptionProvenance {
                    source_template_id: row.get(0)?,
                    source_template_version: u32::try_from(source_version).unwrap_or_default(),
                    runtime_schema_version: u32::try_from(runtime_version).unwrap_or_default(),
                    body_sha256: row.get(3)?,
                    motion_profile_sha256: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("adoption provenance is missing for session {session_id}"))
}

fn validate_provenance(
    provenance: &AdoptionProvenance,
    stable: &StableReservation,
    current_template: &AdoptionTemplate,
) -> Result<(), String> {
    validate_identifier(
        &provenance.source_template_id,
        "adoption provenance template id",
    )?;
    validate_sha256(&provenance.body_sha256, "adoption provenance body")?;
    validate_sha256(
        &provenance.motion_profile_sha256,
        "adoption provenance motion profile",
    )?;
    if provenance.source_template_id != current_template.template_id
        || provenance.source_template_version != stable.source_template_version
        || provenance.source_template_version == 0
        || provenance.runtime_schema_version != RUNTIME_SCHEMA_VERSION
    {
        return Err("adoption provenance contradicts the reserved template facts".into());
    }
    if provenance.source_template_version == current_template.template_version
        && (provenance.body_sha256 != current_template.body_sha256
            || provenance.motion_profile_sha256 != current_template.motion_profile_sha256)
    {
        return Err("adoption provenance contradicts the current immutable template".into());
    }
    Ok(())
}

fn verify_candidate_database_paths(
    storage: &Arc<Mutex<Storage>>,
    app_data_dir: &Path,
    snapshot: &CreationSnapshot,
) -> Result<(), String> {
    let storage = storage.lock().map_err(|_| "storage lock poisoned")?;
    let row = storage
        .db
        .query_row(
            "SELECT COUNT(*), MIN(variant_id), MIN(pet_id), MIN(job_id),
                    MIN(image_path), MIN(cutout_path), MIN(motion_profile_path),
                    MIN(quality), SUM(accepted)
             FROM appearance_variants WHERE session_id=?1",
            [&snapshot.session_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                ))
            },
        )
        .map_err(|error| error.to_string())?;
    let candidate_dir = app_data_dir
        .join("creation-sessions")
        .join(&snapshot.session_id)
        .join("candidate")
        .canonicalize()
        .map_err(|error| format!("adoption candidate directory is unavailable: {error}"))?;
    let expected_body = candidate_dir.join("body.png");
    let expected_profile = candidate_dir.join("motion-profile.json");
    if row.0 != 1
        || row.1.as_deref() != snapshot.candidate_id.as_deref()
        || row.2.as_deref() != Some(snapshot.pet_id.as_str())
        || row.3.is_some()
        || row.4.as_deref() != expected_body.to_str()
        || row.5.as_deref() != expected_body.to_str()
        || row.6.as_deref() != expected_profile.to_str()
        || row.7.as_deref() != Some("acceptable")
        || row.8 != Some(0)
    {
        return Err("adoption candidate database paths or ownership are contradictory".into());
    }
    Ok(())
}

fn project_facts(
    facts: &[AdoptionFact],
    template: &AdoptionTemplate,
    app_data_dir: &Path,
) -> Result<(Option<String>, Option<String>), String> {
    let mut projection = None;
    for fact in facts {
        let next = match classify_fact(fact, template, app_data_dir)? {
            AdoptionFactState::IgnoredAbandoned => continue,
            AdoptionFactState::Adopted => (Some(fact.pet_id.clone()), None),
            AdoptionFactState::Retryable => (
                None,
                Some(
                    fact.session_id
                        .clone()
                        .ok_or("contradictory adoption facts: retry pet has no session")?,
                ),
            ),
        };
        if projection.replace(next).is_some() {
            return Err("contradictory adoption facts: multiple live pets use one template".into());
        }
    }
    Ok(projection.unwrap_or((None, None)))
}

fn classify_fact(
    fact: &AdoptionFact,
    template: &AdoptionTemplate,
    app_data_dir: &Path,
) -> Result<AdoptionFactState, String> {
    let template_id = &template.template_id;
    validate_identifier(&fact.pet_id, "adoption pet id")?;
    if fact.pet_schema_version != 1
        || fact.species != "cat"
        || fact.identity_mode != "adopted"
        || fact.creation_method != "adoption"
        || fact.source_template_id.as_deref() != Some(template_id.as_str())
    {
        return Err(format!(
            "contradictory adoption metadata for pet {}",
            fact.pet_id
        ));
    }
    if fact.source_template_version == 0 {
        return Err("contradictory adoption facts: source version must be positive".into());
    }
    let session_id = fact
        .session_id
        .as_deref()
        .ok_or("contradictory adoption facts: live adoption pet has no session")?;
    validate_identifier(session_id, "adoption session id")?;
    if fact.session_method.as_deref() != Some("adoption") || fact.session_schema_version != Some(1)
    {
        return Err("contradictory adoption facts: session method does not match pet".into());
    }
    let status = fact
        .status
        .as_deref()
        .ok_or("contradictory adoption facts: adoption session has no status")?;
    let provenance_missing = fact.provenance_session_id.is_none()
        && fact.provenance_source_template_id.is_none()
        && fact.provenance_source_template_version.is_none()
        && fact.provenance_runtime_schema_version.is_none()
        && fact.provenance_body_sha256.is_none()
        && fact.provenance_motion_profile_sha256.is_none();
    let provenance_hashes_are_valid = match (
        fact.provenance_body_sha256.as_deref(),
        fact.provenance_motion_profile_sha256.as_deref(),
    ) {
        (Some(body), Some(profile)) => {
            validate_sha256(body, "adoption fact provenance body")?;
            validate_sha256(profile, "adoption fact provenance motion profile")?;
            true
        }
        _ => false,
    };
    let provenance_matches = fact.provenance_session_id.as_deref() == Some(session_id)
        && fact.provenance_source_template_id.as_deref() == Some(template_id.as_str())
        && fact.provenance_source_template_version == Some(fact.source_template_version)
        && fact.provenance_runtime_schema_version == Some(RUNTIME_SCHEMA_VERSION)
        && provenance_hashes_are_valid
        && (fact.provenance_source_template_version != Some(template.template_version)
            || (fact.provenance_body_sha256.as_deref() == Some(template.body_sha256.as_str())
                && fact.provenance_motion_profile_sha256.as_deref()
                    == Some(template.motion_profile_sha256.as_str())));
    let candidate_relationships_match = fact.session_candidate_count == fact.candidate_count
        && fact.pet_candidate_count == fact.candidate_count
        && fact.unaccepted_count + fact.accepted_count == fact.candidate_count;
    if !candidate_relationships_match {
        return Err(format!(
            "contradictory adoption candidate ownership for pet {} and session {session_id}",
            fact.pet_id
        ));
    }
    if fact.lifecycle == "abandoned" || status == "abandoned" {
        let abandoned = fact.lifecycle == "abandoned"
            && status == "abandoned"
            && fact.last_stable_status.as_deref() == Some("abandoned")
            && fact.pet_completed_at.is_none()
            && fact.session_completed_at.is_none()
            && fact.candidate_count == 0
            && fact.unaccepted_count == 0
            && fact.accepted_count == 0
            && fact.accepted_runtime_count == 0
            && fact.accepted_owned_runtime_count == 0
            && fact.accepted_animated_runtime_count == 0
            && (provenance_missing || provenance_matches);
        return if abandoned {
            Ok(AdoptionFactState::IgnoredAbandoned)
        } else {
            Err(format!(
                "contradictory abandoned adoption facts for pet {} and session {session_id}",
                fact.pet_id
            ))
        };
    }
    if !provenance_matches {
        return Err(format!(
            "contradictory adoption facts: provenance does not match pet {} and session {session_id}",
            fact.pet_id
        ));
    }

    let expected_runtime_manifest_path = app_data_dir
        .join("pets")
        .join(&fact.pet_id)
        .join("assets")
        .join("manifest.json");
    let runtime_manifest_path_matches = fact
        .accepted_runtime_manifest_path
        .as_deref()
        .map(Path::new)
        == Some(expected_runtime_manifest_path.as_path());
    let durable = fact.lifecycle == "ready"
        && fact.pet_completed_at.is_some()
        && fact.accepted_count == 1
        && fact.accepted_runtime_count == 1
        && fact.accepted_owned_runtime_count == 1
        && fact.accepted_animated_runtime_count == 1
        && fact.candidate_count == 1
        && fact.unaccepted_count == 0
        && status == "completed"
        && fact.last_stable_status.as_deref() == Some("completed")
        && fact.session_completed_at.is_some()
        && runtime_manifest_path_matches;
    if durable {
        return Ok(AdoptionFactState::Adopted);
    }

    let pending_candidate_count_is_valid = match status {
        "draft" => fact.last_stable_status.as_deref() == Some("draft") && fact.candidate_count == 0,
        "candidateReady" => {
            fact.last_stable_status.as_deref() == Some("candidateReady")
                && fact.candidate_count == 1
        }
        "finalizing" => {
            fact.last_stable_status.as_deref() == Some("candidateReady")
                && fact.candidate_count == 1
        }
        "retryableFailure" => match fact.last_stable_status.as_deref() {
            Some("draft") => fact.candidate_count == 0,
            Some("candidateReady") => fact.candidate_count == 1,
            _ => false,
        },
        _ => false,
    };
    let pending = fact.lifecycle == "draft"
        && fact.pet_completed_at.is_none()
        && fact.session_completed_at.is_none()
        && fact.accepted_count == 0
        && fact.accepted_runtime_count == 0
        && fact.accepted_owned_runtime_count == 0
        && fact.accepted_animated_runtime_count == 0
        && pending_candidate_count_is_valid;
    if pending {
        Ok(AdoptionFactState::Retryable)
    } else {
        Err(format!(
            "contradictory adoption facts for pet {} and session {session_id}",
            fact.pet_id
        ))
    }
}

fn adoption_facts(db: &Connection, template_id: &str) -> Result<Vec<AdoptionFact>, String> {
    let mut statement = db
        .prepare(
            "WITH relevant_pairs(pet_id, session_id) AS (
               SELECT p.pet_id, cs.session_id
               FROM pets p
               LEFT JOIN creation_sessions cs ON cs.pet_id=p.pet_id
               WHERE p.source_template_id=?1
               UNION
               SELECT cs.pet_id, cap.session_id
               FROM creation_adoption_provenance cap
               LEFT JOIN creation_sessions cs ON cs.session_id=cap.session_id
               WHERE cap.source_template_id=?1
             )
             SELECT COALESCE(p.pet_id, ''), COALESCE(p.schema_version, 0),
                    COALESCE(p.species, ''), COALESCE(p.identity_mode, ''),
                    COALESCE(p.creation_method, ''), p.source_template_id,
                    p.source_template_version,
                    COALESCE(p.lifecycle, ''), p.completed_at,
                    cs.session_id, cs.method, cs.status, cs.last_stable_status,
                    cs.schema_version, cs.completed_at,
                    cap.session_id, cap.source_template_id, cap.source_template_version,
                    cap.runtime_schema_version, cap.body_sha256, cap.motion_profile_sha256,
                    (SELECT COUNT(*) FROM appearance_variants av
                     WHERE av.session_id=cs.session_id),
                    (SELECT COUNT(*) FROM appearance_variants av
                     WHERE av.pet_id=p.pet_id),
                    (SELECT COUNT(*) FROM appearance_variants av
                     WHERE av.pet_id=p.pet_id AND av.session_id=cs.session_id),
                    (SELECT COUNT(*) FROM appearance_variants av
                     WHERE av.pet_id=p.pet_id AND av.session_id=cs.session_id AND av.accepted=0),
                    (SELECT COUNT(*) FROM appearance_variants av
                     WHERE av.pet_id=p.pet_id AND av.session_id=cs.session_id AND av.accepted=1),
                    (SELECT COUNT(*) FROM appearance_variants av
                     JOIN variants rv ON rv.variant_id=av.variant_id
                     WHERE av.pet_id=p.pet_id AND av.session_id=cs.session_id AND av.accepted=1),
                    (SELECT COUNT(*) FROM appearance_variants av
                     JOIN variants rv ON rv.variant_id=av.variant_id AND rv.pet_id=av.pet_id
                     WHERE av.pet_id=p.pet_id AND av.session_id=cs.session_id AND av.accepted=1),
                    (SELECT COUNT(*) FROM appearance_variants av
                     JOIN variants rv ON rv.variant_id=av.variant_id AND rv.pet_id=av.pet_id
                     WHERE av.pet_id=p.pet_id AND av.session_id=cs.session_id AND av.accepted=1
                       AND rv.style_id='animated-image-v1'),
                    (SELECT MIN(rv.manifest_path) FROM appearance_variants av
                     JOIN variants rv ON rv.variant_id=av.variant_id AND rv.pet_id=av.pet_id
                     WHERE av.pet_id=p.pet_id AND av.session_id=cs.session_id AND av.accepted=1)
             FROM relevant_pairs rp
             LEFT JOIN pets p ON p.pet_id=rp.pet_id
             LEFT JOIN creation_sessions cs ON cs.session_id=rp.session_id
             LEFT JOIN creation_adoption_provenance cap ON cap.session_id=cs.session_id
             ORDER BY p.rowid, cs.rowid, cap.rowid",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([template_id], |row| {
            let version: Option<i64> = row.get(6)?;
            let provenance_source_version: Option<i64> = row.get(17)?;
            let provenance_runtime_version: Option<i64> = row.get(18)?;
            Ok(AdoptionFact {
                pet_id: row.get(0)?,
                pet_schema_version: row.get(1)?,
                species: row.get(2)?,
                identity_mode: row.get(3)?,
                creation_method: row.get(4)?,
                source_template_id: row.get(5)?,
                source_template_version: version
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or_default(),
                lifecycle: row.get(7)?,
                pet_completed_at: row.get(8)?,
                session_id: row.get(9)?,
                session_method: row.get(10)?,
                status: row.get(11)?,
                last_stable_status: row.get(12)?,
                session_schema_version: row.get(13)?,
                session_completed_at: row.get(14)?,
                provenance_session_id: row.get(15)?,
                provenance_source_template_id: row.get(16)?,
                provenance_source_template_version: provenance_source_version
                    .map(|value| u32::try_from(value).unwrap_or_default()),
                provenance_runtime_schema_version: provenance_runtime_version
                    .map(|value| u32::try_from(value).unwrap_or_default()),
                provenance_body_sha256: row.get(19)?,
                provenance_motion_profile_sha256: row.get(20)?,
                session_candidate_count: row.get(21)?,
                pet_candidate_count: row.get(22)?,
                candidate_count: row.get(23)?,
                unaccepted_count: row.get(24)?,
                accepted_count: row.get(25)?,
                accepted_runtime_count: row.get(26)?,
                accepted_owned_runtime_count: row.get(27)?,
                accepted_animated_runtime_count: row.get(28)?,
                accepted_runtime_manifest_path: row.get(29)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn snapshot_for_session(
    storage: &Arc<Mutex<Storage>>,
    session_id: &str,
) -> Result<CreationSnapshot, String> {
    let storage = storage.lock().map_err(|_| "storage lock poisoned")?;
    snapshot_from_db(&storage.db, session_id)
}

fn snapshot_from_db(db: &Connection, session_id: &str) -> Result<CreationSnapshot, String> {
    db.query_row(
        "SELECT cs.session_id, cs.pet_id, cs.status, cs.last_stable_status,
                cs.current_step, p.display_name, cs.error,
                (SELECT av.variant_id FROM appearance_variants av
                 WHERE av.session_id=cs.session_id AND av.pet_id=cs.pet_id
                 ORDER BY av.created_at DESC, av.rowid DESC LIMIT 1)
         FROM creation_sessions cs JOIN pets p ON p.pet_id=cs.pet_id
         WHERE cs.session_id=?1 AND cs.method='adoption' AND p.creation_method='adoption'",
        [session_id],
        |row| {
            let status: String = row.get(2)?;
            let last_stable_status: String = row.get(3)?;
            Ok(CreationSnapshot {
                session_id: row.get(0)?,
                pet_id: row.get(1)?,
                method: CreationMethod::Adoption,
                status: parse_status(&status).map_err(to_sql_error)?,
                last_stable_status: parse_status(&last_stable_status).map_err(to_sql_error)?,
                current_step: row.get(4)?,
                display_name: row.get(5)?,
                job_id: None,
                job_status: None,
                candidate_id: row.get(7)?,
                recipe: None,
                error: row.get(6)?,
            })
        },
    )
    .optional()
    .map_err(|error| error.to_string())?
    .ok_or_else(|| format!("creation session not found: {session_id}"))
}

fn to_sql_error(error: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
    )
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

fn load_catalog(content_root: &ContentRoot) -> Result<Vec<LoadedTemplate>, String> {
    (|| {
        let content_guard = ReadOnlyDirectoryGuard::open(content_root.as_path(), "content root")
            .map_err(|error| error.to_string())?;
        if content_guard.path != content_root.as_path() {
            return Err(CatalogSecurityError::new(
                CatalogSecurityFailure::IdentityChanged,
                "content root identity changed after setup",
            )
            .to_string());
        }
        let adoption_guard = content_guard
            .child("adoption", "adoption catalog directory")
            .map_err(|error| error.to_string())?;
        let catalog_bytes = adoption_guard
            .read_file(
                "catalog.json",
                "adoption catalog manifest",
                MAX_CATALOG_BYTES,
            )
            .map_err(|error| error.to_string())?;
        let manifest: AdoptionCatalogManifest = serde_json::from_slice(&catalog_bytes)
            .map_err(|error| format!("adoption catalog JSON is invalid: {error}"))?;
        if manifest.schema_version != CATALOG_SCHEMA_VERSION {
            return Err(format!(
                "adoption catalog schema version must be {CATALOG_SCHEMA_VERSION}"
            ));
        }
        if manifest.templates.len() != TEMPLATE_COUNT {
            return Err(format!(
                "adoption catalog must contain exactly {TEMPLATE_COUNT} templates"
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for template in &manifest.templates {
            validate_template(template)?;
            if !ids.insert(template.template_id.clone()) {
                return Err(format!(
                    "adoption catalog contains duplicate template id: {}",
                    template.template_id
                ));
            }
        }
        let mut validated = Vec::with_capacity(TEMPLATE_COUNT);
        for template in manifest.templates {
            let assets = (|| {
                let template_guard =
                    adoption_guard.child(&template.template_id, "adoption template directory")?;
                let thumbnail = template_guard.read_relative_file(
                    &template.thumbnail_path,
                    "adoption thumbnail",
                    MAX_THUMBNAIL_BYTES,
                )?;
                verify_hash(&thumbnail, &template.thumbnail_sha256, "adoption thumbnail")
                    .map_err(TemplateAssetFailure::Content)?;
                validate_png(&thumbnail, 512, 512, false, "adoption thumbnail")
                    .map_err(TemplateAssetFailure::Content)?;
                let body = template_guard.read_relative_file(
                    &template.body_path,
                    "adoption body",
                    MAX_BODY_BYTES,
                )?;
                verify_hash(&body, &template.body_sha256, "adoption body")
                    .map_err(TemplateAssetFailure::Content)?;
                validate_png(&body, 1024, 1024, true, "adoption body")
                    .map_err(TemplateAssetFailure::Content)?;
                let motion_profile = template_guard.read_relative_file(
                    &template.motion_profile_path,
                    "adoption motion profile",
                    MAX_MOTION_PROFILE_BYTES,
                )?;
                verify_hash(
                    &motion_profile,
                    &template.motion_profile_sha256,
                    "adoption motion profile",
                )
                .map_err(TemplateAssetFailure::Content)?;
                let motion_json = std::str::from_utf8(&motion_profile).map_err(|error| {
                    TemplateAssetFailure::Content(format!(
                        "adoption motion profile is not UTF-8: {error}"
                    ))
                })?;
                parse_strict_motion_profile(motion_json).map_err(TemplateAssetFailure::Content)?;
                Ok::<_, TemplateAssetFailure>((body, motion_profile))
            })();
            validated.push(match assets {
                Ok((body, motion_profile)) => LoadedTemplate::Available(ValidatedTemplate {
                    template,
                    body,
                    motion_profile,
                }),
                Err(failure)
                    if template_asset_failure_scope(&failure)
                        == TemplateAssetFailureScope::WholeCatalog =>
                {
                    let TemplateAssetFailure::Security(error) = failure else {
                        unreachable!("whole-catalog template failures are security failures")
                    };
                    return Err(error.to_string());
                }
                Err(TemplateAssetFailure::Content(reason)) => {
                    LoadedTemplate::Unavailable { template, reason }
                }
                Err(TemplateAssetFailure::FileIo(error)) => LoadedTemplate::Unavailable {
                    template,
                    reason: error.to_string(),
                },
                Err(TemplateAssetFailure::Security(_)) => {
                    unreachable!("security failures are handled as whole-catalog failures")
                }
            });
        }
        Ok(validated)
    })()
    .map_err(|error: String| format!("adoption catalog is unavailable: {error}"))
}

fn parse_strict_motion_profile(
    json: &str,
) -> Result<crate::runtime_assets::motion_profile::MotionProfileV1, String> {
    serde_json::from_str::<StrictMotionProfileShape>(json)
        .map_err(|error| format!("strict motion profile contract is invalid: {error}"))?;
    parse_motion_profile(json)
}

fn validate_template(template: &AdoptionTemplate) -> Result<(), String> {
    validate_template_id(&template.template_id)?;
    if template.template_version == 0 {
        return Err("adoption template version must be positive".into());
    }
    if template.runtime_schema_version != RUNTIME_SCHEMA_VERSION {
        return Err(format!(
            "adoption runtime schema version must be {RUNTIME_SCHEMA_VERSION}"
        ));
    }
    if normalize_display_name(&template.default_name)? != template.default_name {
        return Err("adoption default name must be stored in normalized form".into());
    }
    let personality = template.personality.trim();
    let personality_length = personality.graphemes(true).count();
    if personality != template.personality
        || personality_length == 0
        || personality_length > MAX_PERSONALITY_GRAPHEMES
        || personality.chars().any(|character| {
            character.is_control() || matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}')
        })
    {
        return Err(format!(
            "adoption personality must be normalized plain text with 1 to {MAX_PERSONALITY_GRAPHEMES} characters"
        ));
    }
    for (path, label) in [
        (&template.thumbnail_path, "thumbnail path"),
        (&template.body_path, "body path"),
        (&template.motion_profile_path, "motion profile path"),
    ] {
        validate_relative_path(path, label)?;
    }
    for (hash, label) in [
        (&template.thumbnail_sha256, "thumbnail SHA-256"),
        (&template.body_sha256, "body SHA-256"),
        (&template.motion_profile_sha256, "motion profile SHA-256"),
    ] {
        validate_sha256(hash, label)?;
    }
    Ok(())
}

fn validate_sha256(hash: &str, label: &str) -> Result<(), String> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{label} must be 64 lowercase hexadecimal digits"));
    }
    Ok(())
}

fn validate_template_id(value: &str) -> Result<(), String> {
    if value == BUILTIN_PET_ID {
        return Err(format!(
            "adoption template id is a reserved pet id: {BUILTIN_PET_ID}"
        ));
    }
    validate_identifier(value, "adoption template id")
}

fn validate_identifier(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("invalid {label}"));
    }
    Ok(())
}

fn validate_relative_path<'a>(value: &'a str, label: &str) -> Result<Vec<&'a str>, String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_)
                    | Component::RootDir
                    | Component::ParentDir
                    | Component::CurDir
            )
        })
    {
        return Err(format!("{label} must be a restricted relative path"));
    }
    let parts = value.split('/').collect::<Vec<_>>();
    if parts.is_empty()
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || part.contains(':')
                || part.chars().any(char::is_control)
        })
    {
        return Err(format!("{label} must be a restricted relative path"));
    }
    Ok(parts)
}

fn verify_hash(bytes: &[u8], expected: &str, label: &str) -> Result<(), String> {
    let mut digest = Sha256::new();
    digest.update(bytes);
    let actual = format!("{:x}", digest.finalize());
    if actual != expected {
        return Err(format!("{label} SHA-256 does not match its manifest"));
    }
    Ok(())
}

fn validate_png(
    bytes: &[u8],
    expected_width: u32,
    expected_height: u32,
    require_transparent_background: bool,
    label: &str,
) -> Result<(), String> {
    if image::guess_format(bytes).ok() != Some(image::ImageFormat::Png) {
        return Err(format!("{label} must be a PNG image"));
    }
    let decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes))
        .map_err(|error| format!("{label} PNG header is invalid: {error}"))?;
    let (width, height) = decoder.dimensions();
    if width != expected_width || height != expected_height {
        return Err(format!(
            "{label} must be exactly {expected_width}x{expected_height}, got {width}x{height}"
        ));
    }
    if decoder.color_type() != image::ColorType::Rgba8 {
        return Err(format!("{label} must be an RGBA PNG"));
    }
    let expected_allocation = u64::from(expected_width) * u64::from(expected_height) * 4;
    if decoder.total_bytes() != expected_allocation || expected_allocation > 8 * 1024 * 1024 {
        return Err(format!("{label} decoded allocation is invalid"));
    }
    let mut pixels = vec![0; expected_allocation as usize];
    decoder
        .read_image(&mut pixels)
        .map_err(|error| format!("{label} PNG decode failed: {error}"))?;
    if !pixels.chunks_exact(4).any(|pixel| pixel[3] >= 8) {
        return Err(format!("{label} has no visible content"));
    }
    if require_transparent_background && !pixels.chunks_exact(4).any(|pixel| pixel[3] == 0) {
        return Err(format!(
            "{label} must include transparent background pixels"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    volume_serial: u32,
    file_index: u64,
}

struct ReadOnlyDirectoryGuard {
    path: PathBuf,
    #[cfg(windows)]
    _handle: OwnedHandle,
}

impl ReadOnlyDirectoryGuard {
    #[cfg(windows)]
    fn open(path: &Path, label: &str) -> Result<Self, CatalogSecurityError> {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ, OPEN_EXISTING,
        };
        let wide = crate::platform::windows::encode_windows_path(path).map_err(|error| {
            CatalogSecurityError::new(CatalogSecurityFailure::InvalidPath, error)
        })?;
        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(CatalogSecurityError::new(
                CatalogSecurityFailure::Access,
                format!(
                    "open read-only {label}: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) } == 0 {
            return Err(CatalogSecurityError::new(
                CatalogSecurityFailure::Access,
                format!(
                    "inspect read-only {label}: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(CatalogSecurityError::new(
                CatalogSecurityFailure::ReparsePoint,
                format!("{label} must be a real non-reparse directory"),
            ));
        }
        let canonical = path.canonicalize().map_err(|error| {
            CatalogSecurityError::new(
                CatalogSecurityFailure::Access,
                format!("resolve {label}: {error}"),
            )
        })?;
        Ok(Self {
            path: canonical,
            _handle: handle,
        })
    }

    #[cfg(not(windows))]
    fn open(_path: &Path, _label: &str) -> Result<Self, CatalogSecurityError> {
        Err(CatalogSecurityError::new(
            CatalogSecurityFailure::Access,
            "secure adoption catalog access currently requires Windows handles",
        ))
    }

    fn child(&self, name: &str, label: &str) -> Result<Self, CatalogSecurityError> {
        validate_identifier(name, label).map_err(|error| {
            CatalogSecurityError::new(CatalogSecurityFailure::InvalidPath, error)
        })?;
        let child = Self::open(&self.path.join(name), label)?;
        if child.path.parent() != Some(self.path.as_path()) {
            return Err(CatalogSecurityError::new(
                CatalogSecurityFailure::PathContainment,
                format!("{label} escapes its read-only parent"),
            ));
        }
        Ok(child)
    }

    fn read_file(
        &self,
        name: &str,
        label: &str,
        limit: u64,
    ) -> Result<Vec<u8>, CatalogReadFailure> {
        if validate_relative_path(name, label)
            .map_err(|error| CatalogSecurityError::new(CatalogSecurityFailure::InvalidPath, error))?
            .len()
            != 1
        {
            return Err(CatalogSecurityError::new(
                CatalogSecurityFailure::InvalidPath,
                format!("{label} must be a direct child file"),
            )
            .into());
        }
        read_bounded_regular_file(&self.path.join(name), &self.path, label, limit)
    }

    fn read_relative_file(
        &self,
        relative: &str,
        label: &str,
        limit: u64,
    ) -> Result<Vec<u8>, CatalogReadFailure> {
        let parts = validate_relative_path(relative, label).map_err(|error| {
            CatalogSecurityError::new(CatalogSecurityFailure::InvalidPath, error)
        })?;
        let mut directories = Vec::new();
        let mut parent = self.path.clone();
        for part in &parts[..parts.len() - 1] {
            let directory = ReadOnlyDirectoryGuard::open(&parent.join(part), label)?;
            if directory.path.parent() != Some(parent.as_path()) {
                return Err(CatalogSecurityError::new(
                    CatalogSecurityFailure::PathContainment,
                    format!("{label} directory escapes its template"),
                )
                .into());
            }
            parent = directory.path.clone();
            directories.push(directory);
        }
        let bytes =
            read_bounded_regular_file(&parent.join(parts[parts.len() - 1]), &parent, label, limit)?;
        drop(directories);
        Ok(bytes)
    }
}

#[cfg(windows)]
fn handle_identity(
    file: &std::fs::File,
    label: &str,
) -> Result<(FileIdentity, u32), CatalogSecurityError> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(CatalogSecurityError::new(
            CatalogSecurityFailure::Access,
            format!(
                "inspect {label} identity: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    Ok((
        FileIdentity {
            volume_serial: info.dwVolumeSerialNumber,
            file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        },
        info.dwFileAttributes,
    ))
}

#[cfg(windows)]
fn read_bounded_regular_file(
    path: &Path,
    expected_parent: &Path,
    label: &str,
    limit: u64,
) -> Result<Vec<u8>, CatalogReadFailure> {
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_READ,
    };
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .access_mode(GENERIC_READ)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN)
        .open(path)
        .map_err(|error| {
            CatalogFileIoError::new(error.kind(), format!("open read-only {label}: {error}"))
        })?;
    let (before_identity, before_attributes) = handle_identity(&file, label)?;
    if before_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || before_attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    {
        return Err(CatalogSecurityError::new(
            CatalogSecurityFailure::ReparsePoint,
            format!("{label} must be a regular non-reparse file"),
        )
        .into());
    }
    let metadata = file.metadata().map_err(|error| {
        CatalogFileIoError::new(error.kind(), format!("inspect {label}: {error}"))
    })?;
    if metadata.len() == 0 || metadata.len() > limit {
        return Err(CatalogSecurityError::new(
            CatalogSecurityFailure::BoundedRead,
            format!("{label} exceeds its bounded size"),
        )
        .into());
    }
    let canonical = path.canonicalize().map_err(|error| {
        CatalogSecurityError::new(
            CatalogSecurityFailure::Access,
            format!("resolve {label}: {error}"),
        )
    })?;
    if canonical.parent() != Some(expected_parent) {
        return Err(CatalogSecurityError::new(
            CatalogSecurityFailure::PathContainment,
            format!("{label} escapes its read-only template directory"),
        )
        .into());
    }
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|error| CatalogFileIoError::new(error.kind(), format!("seek {label}: {error}")))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CatalogFileIoError::new(error.kind(), format!("read {label}: {error}")))?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > limit {
        return Err(CatalogSecurityError::new(
            CatalogSecurityFailure::IdentityChanged,
            format!("{label} changed while it was read"),
        )
        .into());
    }
    let (after_identity, after_attributes) = handle_identity(&file, label)?;
    if after_identity != before_identity
        || after_attributes != before_attributes
        || path.canonicalize().map_err(|error| {
            CatalogSecurityError::new(
                CatalogSecurityFailure::Access,
                format!("re-resolve {label}: {error}"),
            )
        })? != canonical
    {
        return Err(CatalogSecurityError::new(
            CatalogSecurityFailure::IdentityChanged,
            format!("{label} identity changed while it was read"),
        )
        .into());
    }
    Ok(bytes)
}

#[cfg(not(windows))]
fn read_bounded_regular_file(
    _path: &Path,
    _expected_parent: &Path,
    _label: &str,
    _limit: u64,
) -> Result<Vec<u8>, CatalogReadFailure> {
    Err(CatalogSecurityError::new(
        CatalogSecurityFailure::Access,
        "secure adoption catalog file access currently requires Windows handles",
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::{adoption_facts, load_catalog, project_facts};
    use crate::creation::domain::{CreationMethod, CreationSessionStatus, CreationSnapshot};
    use crate::creation::service::CreationService;
    use crate::pets::active::{ActivePetService, BUILTIN_PET_ID};
    use crate::pets::deletion::PetDeletionService;
    use crate::pets::mutation::PetMutationGate;
    use crate::pets::{ActivePetSession, SharedActivePetSession};
    use crate::runtime_assets::motion_profile::generate_motion_profile;
    use crate::storage::Storage;
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct AdoptionHarness {
        root: PathBuf,
        content_root: PathBuf,
        storage: Arc<Mutex<Storage>>,
        service: Arc<CreationService>,
    }

    impl AdoptionHarness {
        fn with_templates() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "desktop-pet-adoption-{}-{n}-{unique}",
                std::process::id()
            ));
            let pets_dir = root.join("pets");
            let content_root = root.join("creation-content");
            std::fs::create_dir_all(root.join("jobs")).unwrap();
            std::fs::create_dir_all(&content_root).unwrap();
            write_catalog(&content_root);

            let storage = Arc::new(Mutex::new(Storage::open(&pets_dir).unwrap()));
            let active_session: SharedActivePetSession =
                Arc::new(Mutex::new(ActivePetSession::new()));
            active_session
                .lock()
                .unwrap()
                .set_active(BUILTIN_PET_ID.into())
                .unwrap();
            let gate = Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60)));
            let active = Arc::new(ActivePetService::new(
                storage.clone(),
                active_session,
                pets_dir,
                gate.clone(),
            ));
            let deletion = Arc::new(PetDeletionService::new(
                storage.clone(),
                active,
                root.clone(),
                gate.clone(),
            ));
            let service = Arc::new(CreationService::new(
                storage.clone(),
                root.clone(),
                deletion,
                crate::creation::content::test_content_root(&content_root).unwrap(),
                gate,
            ));
            Self {
                root,
                content_root,
                storage,
                service,
            }
        }

        fn candidate_file(&self, session: &CreationSnapshot, file: &str) -> PathBuf {
            self.root
                .join("creation-sessions")
                .join(&session.session_id)
                .join("candidate")
                .join(file)
        }

        fn pet_count_for(&self, template_id: &str) -> i64 {
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

        fn row_count(&self, table: &str) -> i64 {
            assert!(matches!(
                table,
                "pets"
                    | "creation_sessions"
                    | "appearance_variants"
                    | "creation_adoption_provenance"
            ));
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        }

        fn independent_service(&self) -> Arc<CreationService> {
            let pets_dir = self.root.join("pets");
            let storage = Arc::new(Mutex::new(Storage::open(&pets_dir).unwrap()));
            let active_session: SharedActivePetSession =
                Arc::new(Mutex::new(ActivePetSession::new()));
            active_session
                .lock()
                .unwrap()
                .set_active(BUILTIN_PET_ID.into())
                .unwrap();
            let gate = Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60)));
            let active = Arc::new(ActivePetService::new(
                storage.clone(),
                active_session,
                pets_dir,
                gate.clone(),
            ));
            let deletion = Arc::new(PetDeletionService::new(
                storage.clone(),
                active,
                self.root.clone(),
                gate.clone(),
            ));
            Arc::new(CreationService::new(
                storage,
                self.root.clone(),
                deletion,
                crate::creation::content::test_content_root(&self.content_root).unwrap(),
                gate,
            ))
        }

        fn catalog_path(&self) -> PathBuf {
            self.content_root.join("adoption/catalog.json")
        }

        fn catalog_json(&self) -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(self.catalog_path()).unwrap()).unwrap()
        }

        fn template(&self, template_id: &str) -> super::AdoptionTemplate {
            let catalog = self.catalog_json();
            serde_json::from_value(
                catalog["templates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|template| template["templateId"] == template_id)
                    .unwrap()
                    .clone(),
            )
            .unwrap()
        }

        fn write_catalog_json(&self, catalog: &serde_json::Value) {
            std::fs::write(
                self.catalog_path(),
                serde_json::to_vec_pretty(catalog).unwrap(),
            )
            .unwrap();
        }

        fn display_name(&self, pet_id: &str) -> String {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT display_name FROM pets WHERE pet_id=?1",
                    [pet_id],
                    |row| row.get(0),
                )
                .unwrap()
        }

        fn complete(&self, session: &CreationSnapshot) {
            let storage = self.storage.lock().unwrap();
            let candidate_id: String = storage
                .db
                .query_row(
                    "SELECT variant_id FROM appearance_variants WHERE session_id=?1",
                    [&session.session_id],
                    |row| row.get(0),
                )
                .unwrap();
            let manifest_path = self
                .root
                .join("pets")
                .join(&session.pet_id)
                .join("assets")
                .join("manifest.json")
                .to_string_lossy()
                .into_owned();
            storage
                .db
                .execute(
                    "INSERT INTO variants (variant_id, pet_id, style_id, manifest_path, created_at)
                     VALUES (?1, ?2, 'animated-image-v1', ?3, '1')",
                    rusqlite::params![candidate_id, session.pet_id, manifest_path],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "UPDATE appearance_variants SET accepted=1 WHERE variant_id=?1",
                    [&candidate_id],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "UPDATE pets SET lifecycle='ready', completed_at='1' WHERE pet_id=?1",
                    [&session.pet_id],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "UPDATE creation_sessions SET status='completed',
                     last_stable_status='completed', current_step='completed', completed_at='1'
                     WHERE session_id=?1",
                    [&session.session_id],
                )
                .unwrap();
        }
    }

    impl Drop for AdoptionHarness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn sha256(bytes: &[u8]) -> String {
        let mut digest = Sha256::new();
        digest.update(bytes);
        format!("{:x}", digest.finalize())
    }

    fn write_png(path: &Path, size: u32, transparent_background: bool) -> Vec<u8> {
        let mut image = image::RgbaImage::new(size, size);
        if !transparent_background {
            for pixel in image.pixels_mut() {
                *pixel = image::Rgba([245, 245, 245, 255]);
            }
        }
        for y in size / 4..size * 3 / 4 {
            for x in size / 4..size * 3 / 4 {
                image.put_pixel(x, y, image::Rgba([90, 120, 180, 255]));
            }
        }
        image.save(path).unwrap();
        std::fs::read(path).unwrap()
    }

    fn write_catalog(content_root: &Path) {
        let adoption_root = content_root.join("adoption");
        std::fs::create_dir_all(&adoption_root).unwrap();
        let ids = [
            "cat-misty",
            "cat-sunny",
            "cat-mochi",
            "cat-cocoa",
            "cat-snow",
            "cat-amber",
            "cat-luna",
            "cat-pepper",
        ];
        let mut templates = Vec::new();
        for (index, template_id) in ids.into_iter().enumerate() {
            let template_root = adoption_root.join(template_id);
            std::fs::create_dir_all(&template_root).unwrap();
            let body = write_png(&template_root.join("body.png"), 1024, true);
            let thumbnail = write_png(&template_root.join("thumbnail.png"), 512, true);
            let rgba = image::load_from_memory(&body).unwrap().to_rgba8();
            let profile =
                serde_json::to_vec_pretty(&generate_motion_profile(&rgba).unwrap()).unwrap();
            std::fs::write(template_root.join("motion-profile.json"), &profile).unwrap();
            templates.push(serde_json::json!({
                "templateId": template_id,
                "templateVersion": 1,
                "runtimeSchemaVersion": 3,
                "defaultName": format!("猫咪{}", index + 1),
                "personality": format!("温柔又好奇的猫咪{}", index + 1),
                "thumbnailPath": "thumbnail.png",
                "bodyPath": "body.png",
                "motionProfilePath": "motion-profile.json",
                "thumbnailSha256": sha256(&thumbnail),
                "bodySha256": sha256(&body),
                "motionProfileSha256": sha256(&profile),
            }));
        }
        std::fs::write(
            adoption_root.join("catalog.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schemaVersion": 1,
                "templates": templates,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn production_catalog_reads_all_eight_validated_asset_sets() {
        let content_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("public/creation-content");
        let content_root = crate::creation::content::test_content_root(&content_root).unwrap();
        let templates = load_catalog(&content_root).unwrap();

        let expected = [
            ("cat-misty", "雾雾"),
            ("cat-tangerine", "橘子"),
            ("cat-dumpling", "团子"),
            ("cat-ink", "墨墨"),
            ("cat-cloud", "云朵"),
            ("cat-chestnut", "栗子"),
            ("cat-sesame", "芝麻"),
            ("cat-starlight", "星星"),
        ];
        assert_eq!(templates.len(), expected.len());
        for (loaded, (template_id, default_name)) in templates.iter().zip(expected) {
            let super::LoadedTemplate::Available(validated) = loaded else {
                panic!("production template {template_id} is unavailable");
            };
            assert_eq!(validated.template.template_id, template_id);
            assert_eq!(validated.template.default_name, default_name);
            assert!(!validated.body.is_empty());
            assert!(!validated.motion_profile.is_empty());
        }
    }

    #[test]
    fn catalog_projects_adoptable_adopted_and_retryable_states() {
        let test = AdoptionHarness::with_templates();
        let initial = test.service.adoption_catalog().unwrap();
        assert!(initial
            .iter()
            .all(|entry| { entry.adopted_pet_id.is_none() && entry.retry_session_id.is_none() }));
        let session = test.service.start_adoption("cat-misty", "雾雾").unwrap();
        let pending = test.service.adoption_catalog().unwrap();
        assert_eq!(
            pending[0].retry_session_id.as_deref(),
            Some(session.session_id.as_str())
        );
        assert!(pending[0].adopted_pet_id.is_none());
        test.complete(&session);
        let adopted = test.service.adoption_catalog().unwrap();
        assert_eq!(
            adopted[0].adopted_pet_id.as_deref(),
            Some(session.pet_id.as_str())
        );
        assert!(adopted[0].retry_session_id.is_none());
    }

    #[test]
    fn adopted_projection_accepts_the_formal_finalization_manifest_path() {
        let test = AdoptionHarness::with_templates();
        let session = test
            .service
            .start_adoption("cat-misty", "formal-path")
            .unwrap();

        test.complete(&session);

        let catalog = test.service.adoption_catalog().unwrap();
        assert_eq!(
            catalog[0].adopted_pet_id.as_deref(),
            Some(session.pet_id.as_str())
        );
    }

    #[test]
    fn start_copies_trusted_template_into_a_user_candidate() {
        let test = AdoptionHarness::with_templates();
        let source_body =
            std::fs::read(test.content_root.join("adoption/cat-misty/body.png")).unwrap();
        let session = test.service.start_adoption("cat-misty", "雾雾").unwrap();
        assert_eq!(session.method, CreationMethod::Adoption);
        assert_eq!(session.status, CreationSessionStatus::CandidateReady);
        assert_eq!(
            std::fs::read(test.candidate_file(&session, "body.png")).unwrap(),
            source_body
        );
        assert!(test
            .candidate_file(&session, "motion-profile.json")
            .exists());
        let names = std::fs::read_dir(test.candidate_file(&session, "body.png").parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            names,
            ["body.png".to_owned(), "motion-profile.json".to_owned()]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn repeated_start_returns_the_same_retry_session_not_a_second_pet() {
        let test = AdoptionHarness::with_templates();
        let first = test.service.start_adoption("cat-misty", "雾雾").unwrap();
        let second = test
            .service
            .start_adoption("cat-misty", "另一个名字")
            .unwrap();
        assert_eq!(second.session_id, first.session_id);
        assert_eq!(second.pet_id, first.pet_id);
        assert_eq!(test.pet_count_for("cat-misty"), 1);
        assert_eq!(test.display_name(&first.pet_id), "雾雾");
    }

    #[test]
    fn catalog_is_loaded_lazily_and_reports_a_missing_adoption_catalog() {
        let test = AdoptionHarness::with_templates();
        std::fs::remove_file(test.catalog_path()).unwrap();

        let error = test.service.adoption_catalog().unwrap_err();

        assert!(error.contains("adoption catalog is unavailable"), "{error}");
        assert_eq!(test.row_count("pets"), 0);
    }

    #[test]
    fn one_missing_template_asset_disables_only_that_card_and_preserves_the_other_seven() {
        for asset in ["thumbnail.png", "body.png", "motion-profile.json"] {
            let test = AdoptionHarness::with_templates();
            let missing = test.content_root.join("adoption/cat-misty").join(asset);
            assert!(missing.starts_with(&test.root));
            std::fs::remove_file(&missing).unwrap();

            let catalog = test
                .service
                .adoption_catalog()
                .unwrap_or_else(|error| panic!("missing {asset} blocked the catalog: {error}"));

            assert_eq!(catalog.len(), 8);
            let misty = catalog
                .iter()
                .find(|entry| entry.template.template_id == "cat-misty")
                .unwrap();
            assert!(misty.unavailable_reason.is_some(), "missing {asset}");
            assert_eq!(
                catalog
                    .iter()
                    .filter(|entry| entry.unavailable_reason.is_none())
                    .count(),
                7,
                "missing {asset}"
            );
            let healthy = test.service.start_adoption("cat-sunny", "健康模板");
            assert!(healthy.is_ok(), "missing {asset}: {healthy:?}");
        }
    }

    #[test]
    fn one_corrupt_template_is_disabled_without_blocking_healthy_templates() {
        let test = AdoptionHarness::with_templates();
        let corrupt_body = test.content_root.join("adoption/cat-misty/body.png");
        assert!(corrupt_body.starts_with(&test.root));
        std::fs::write(&corrupt_body, b"corrupt isolated test body").unwrap();

        let catalog = test
            .service
            .adoption_catalog()
            .expect("one corrupt template must not make the catalog unavailable");
        assert_eq!(catalog.len(), 8);
        let misty = catalog
            .iter()
            .find(|entry| entry.template.template_id == "cat-misty")
            .unwrap();
        let misty_json = serde_json::to_value(misty).unwrap();
        assert!(misty_json["unavailableReason"].as_str().is_some());
        assert!(catalog
            .iter()
            .filter(|entry| entry.template.template_id != "cat-misty")
            .all(|entry| serde_json::to_value(entry).unwrap()["unavailableReason"].is_null()));
        assert!(test
            .service
            .start_adoption("cat-misty", "损坏模板")
            .is_err());
        let healthy = test.service.start_adoption("cat-sunny", "健康模板");
        assert!(healthy.is_ok(), "healthy template was blocked: {healthy:?}");
    }

    #[test]
    fn template_path_escape_and_identity_change_fail_the_catalog_closed_by_type() {
        for kind in [
            super::CatalogSecurityFailure::PathContainment,
            super::CatalogSecurityFailure::IdentityChanged,
        ] {
            let failure = super::TemplateAssetFailure::Security(super::CatalogSecurityError::new(
                kind,
                "typed security failure",
            ));

            assert_eq!(
                super::template_asset_failure_scope(&failure),
                super::TemplateAssetFailureScope::WholeCatalog,
            );
        }
    }

    #[test]
    fn template_content_corruption_remains_scoped_to_one_template_by_type() {
        let failure = super::TemplateAssetFailure::Content("invalid PNG role".into());

        assert_eq!(
            super::template_asset_failure_scope(&failure),
            super::TemplateAssetFailureScope::SingleTemplate,
        );
    }

    #[test]
    fn template_file_io_remains_scoped_to_one_template_by_type() {
        let failure = super::TemplateAssetFailure::FileIo(super::CatalogFileIoError::new(
            std::io::ErrorKind::NotFound,
            "missing trusted template asset",
        ));

        assert_eq!(
            super::template_asset_failure_scope(&failure),
            super::TemplateAssetFailureScope::SingleTemplate,
        );
    }

    #[test]
    fn catalog_requires_exactly_eight_unique_templates_and_runtime_v3() {
        let test = AdoptionHarness::with_templates();
        let mut catalog = test.catalog_json();
        catalog["templates"].as_array_mut().unwrap().pop();
        test.write_catalog_json(&catalog);
        assert!(test
            .service
            .adoption_catalog()
            .unwrap_err()
            .contains("exactly 8"));

        write_catalog(&test.content_root);
        let mut catalog = test.catalog_json();
        catalog["templates"][1]["templateId"] = serde_json::json!("cat-misty");
        test.write_catalog_json(&catalog);
        assert!(test
            .service
            .adoption_catalog()
            .unwrap_err()
            .contains("duplicate template id"));

        write_catalog(&test.content_root);
        let mut catalog = test.catalog_json();
        catalog["templates"][0]["runtimeSchemaVersion"] = serde_json::json!(2);
        test.write_catalog_json(&catalog);
        assert!(test
            .service
            .adoption_catalog()
            .unwrap_err()
            .contains("runtime schema version"));
    }

    #[test]
    fn catalog_rejects_the_builtin_pet_id_before_touching_template_assets() {
        let test = AdoptionHarness::with_templates();
        let mut catalog = test.catalog_json();
        catalog["templates"][7]["templateId"] = serde_json::json!(BUILTIN_PET_ID);
        catalog["templates"][0]["thumbnailPath"] = serde_json::json!("missing-thumbnail.png");
        catalog["templates"][0]["bodyPath"] = serde_json::json!("missing-body.png");
        catalog["templates"][0]["motionProfilePath"] =
            serde_json::json!("missing-motion-profile.json");
        test.write_catalog_json(&catalog);

        let error = test.service.adoption_catalog().unwrap_err();

        assert!(error.contains("reserved pet id"), "{error}");
        assert_eq!(test.row_count("pets"), 0);
        assert!(test
            .content_root
            .join("adoption/cat-misty/body.png")
            .exists());
    }

    #[test]
    fn catalog_rejects_untrusted_paths_hashes_names_and_personality() {
        let test = AdoptionHarness::with_templates();
        let mut catalog = test.catalog_json();
        catalog["templates"][0]["bodyPath"] = serde_json::json!("../body.png");
        test.write_catalog_json(&catalog);
        assert!(test
            .service
            .adoption_catalog()
            .unwrap_err()
            .contains("restricted relative path"));

        write_catalog(&test.content_root);
        let mut catalog = test.catalog_json();
        catalog["templates"][0]["bodySha256"] = serde_json::json!("0".repeat(64));
        test.write_catalog_json(&catalog);
        let entries = test
            .service
            .adoption_catalog()
            .expect("a single asset hash mismatch must not block healthy templates");
        assert!(entries[0]
            .unavailable_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("SHA-256 does not match")));
        assert!(entries[1..]
            .iter()
            .all(|entry| entry.unavailable_reason.is_none()));

        write_catalog(&test.content_root);
        let mut catalog = test.catalog_json();
        catalog["templates"][0]["defaultName"] = serde_json::json!(" 雾雾 ");
        test.write_catalog_json(&catalog);
        assert!(test
            .service
            .adoption_catalog()
            .unwrap_err()
            .contains("normalized"));

        write_catalog(&test.content_root);
        let mut catalog = test.catalog_json();
        catalog["templates"][0]["personality"] = serde_json::json!("gentle\ncurious");
        test.write_catalog_json(&catalog);
        assert!(test
            .service
            .adoption_catalog()
            .unwrap_err()
            .contains("personality"));
    }

    #[test]
    fn catalog_requires_the_exact_motion_profile_contract() {
        let test = AdoptionHarness::with_templates();
        let profile_path = test
            .content_root
            .join("adoption")
            .join("cat-misty")
            .join("motion-profile.json");
        let mut profile: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&profile_path).unwrap()).unwrap();
        profile["clientOwnedMotion"] = serde_json::json!(true);
        let bytes = serde_json::to_vec_pretty(&profile).unwrap();
        std::fs::write(&profile_path, &bytes).unwrap();
        let mut catalog = test.catalog_json();
        catalog["templates"][0]["motionProfileSha256"] = serde_json::json!(sha256(&bytes));
        test.write_catalog_json(&catalog);

        let entries = test
            .service
            .adoption_catalog()
            .expect("one invalid motion profile must not block healthy templates");
        assert!(entries[0]
            .unavailable_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("strict motion profile")));
        assert!(entries[1..]
            .iter()
            .all(|entry| entry.unavailable_reason.is_none()));
    }

    #[test]
    fn catalog_enforces_bounded_rgba_png_roles() {
        let test = AdoptionHarness::with_templates();
        let template_root = test.content_root.join("adoption").join("cat-misty");
        let wrong_thumbnail = std::fs::read(template_root.join("body.png")).unwrap();
        std::fs::write(template_root.join("thumbnail.png"), &wrong_thumbnail).unwrap();
        let mut catalog = test.catalog_json();
        catalog["templates"][0]["thumbnailSha256"] = serde_json::json!(sha256(&wrong_thumbnail));
        test.write_catalog_json(&catalog);
        let entries = test
            .service
            .adoption_catalog()
            .expect("one wrongly sized thumbnail must not block healthy templates");
        assert!(entries[0]
            .unavailable_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("512x512")));
        assert!(entries[1..]
            .iter()
            .all(|entry| entry.unavailable_reason.is_none()));

        write_catalog(&test.content_root);
        let template_root = test.content_root.join("adoption").join("cat-misty");
        let opaque_body = write_png(&template_root.join("body.png"), 1024, false);
        let mut catalog = test.catalog_json();
        catalog["templates"][0]["bodySha256"] = serde_json::json!(sha256(&opaque_body));
        test.write_catalog_json(&catalog);
        let entries = test
            .service
            .adoption_catalog()
            .expect("one opaque body must not block healthy templates");
        assert!(entries[0]
            .unavailable_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("transparent background")));
        assert!(entries[1..]
            .iter()
            .all(|entry| entry.unavailable_reason.is_none()));
    }

    #[cfg(windows)]
    #[test]
    fn catalog_rejects_reparse_components_even_when_the_target_is_contained() {
        let test = AdoptionHarness::with_templates();
        let template_root = test.content_root.join("adoption").join("cat-misty");
        let real = template_root.join("real");
        std::fs::create_dir(&real).unwrap();
        let body = std::fs::read(template_root.join("body.png")).unwrap();
        std::fs::write(real.join("body.png"), &body).unwrap();
        crate::platform::create_directory_link(&real, &template_root.join("linked"));
        let mut catalog = test.catalog_json();
        catalog["templates"][0]["bodyPath"] = serde_json::json!("linked/body.png");
        catalog["templates"][0]["bodySha256"] = serde_json::json!(sha256(&body));
        test.write_catalog_json(&catalog);

        let error = test.service.adoption_catalog().unwrap_err();

        assert!(error.contains("reparse"), "{error}");
    }

    #[test]
    fn concurrent_starts_converge_on_one_pet_and_one_session() {
        let test = AdoptionHarness::with_templates();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut threads = Vec::new();
        for name in ["雾雾", "云云"] {
            let barrier = barrier.clone();
            let service = test.service.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                service.start_adoption("cat-misty", name).unwrap()
            }));
        }
        barrier.wait();
        let first = threads.remove(0).join().unwrap();
        let second = threads.remove(0).join().unwrap();
        assert_eq!(first.session_id, second.session_id);
        assert_eq!(first.pet_id, second.pet_id);
        assert_eq!(test.pet_count_for("cat-misty"), 1);
        assert_eq!(test.row_count("creation_sessions"), 1);
    }

    #[test]
    fn independent_sqlite_connections_race_through_unique_and_reread_the_winner() {
        let test = AdoptionHarness::with_templates();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let unique_losers = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut threads = Vec::new();
        for (service, name) in [
            (test.independent_service(), "闆鹃浘"),
            (test.independent_service(), "浜戜簯"),
        ] {
            let barrier = barrier.clone();
            let unique_losers = unique_losers.clone();
            threads.push(std::thread::spawn(move || {
                super::set_thread_adoption_reservation_barrier(barrier, unique_losers);
                service.start_adoption("cat-misty", name)
            }));
        }
        let first = threads.remove(0).join().unwrap().unwrap();
        let second = threads.remove(0).join().unwrap().unwrap();

        assert_eq!(first.pet_id, second.pet_id);
        assert_eq!(first.session_id, second.session_id);
        assert_eq!(test.row_count("pets"), 1);
        assert_eq!(test.row_count("creation_sessions"), 1);
        assert_eq!(test.row_count("creation_adoption_provenance"), 1);
        assert_eq!(test.row_count("appearance_variants"), 1);
        assert_eq!(
            unique_losers.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the independent connection loser must hit UNIQUE and reread the winner"
        );
    }

    #[test]
    fn a_new_reservation_is_removed_when_candidate_publication_fails() {
        let test = AdoptionHarness::with_templates();
        let sessions_root = test.root.join("creation-sessions");
        std::fs::write(&sessions_root, b"not a directory").unwrap();
        let source_before =
            std::fs::read(test.content_root.join("adoption/cat-misty/body.png")).unwrap();

        assert!(test.service.start_adoption("cat-misty", "雾雾").is_err());
        assert_eq!(test.row_count("pets"), 0);
        assert_eq!(test.row_count("creation_sessions"), 0);
        assert_eq!(
            std::fs::read(test.content_root.join("adoption/cat-misty/body.png")).unwrap(),
            source_before
        );

        std::fs::remove_file(sessions_root).unwrap();
        assert!(test.service.start_adoption("cat-misty", "雾雾").is_ok());
    }

    #[test]
    fn database_candidate_failure_rolls_back_new_reservation_and_provenance() {
        let test = AdoptionHarness::with_templates();
        let source_before =
            std::fs::read(test.content_root.join("adoption/cat-misty/body.png")).unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "CREATE TRIGGER fail_adoption_candidate
                 BEFORE INSERT ON appearance_variants
                 BEGIN SELECT RAISE(ABORT, 'injected candidate DB failure'); END;",
            )
            .unwrap();

        let error = test
            .service
            .start_adoption("cat-misty", "闆鹃浘")
            .unwrap_err();

        assert!(error.contains("injected candidate DB failure"), "{error}");
        assert_eq!(test.row_count("pets"), 0);
        assert_eq!(test.row_count("creation_sessions"), 0);
        assert_eq!(test.row_count("creation_adoption_provenance"), 0);
        assert_eq!(test.row_count("appearance_variants"), 0);
        assert_eq!(
            std::fs::read(test.content_root.join("adoption/cat-misty/body.png")).unwrap(),
            source_before
        );
    }

    #[test]
    fn database_candidate_failure_preserves_an_existing_retry_and_its_provenance() {
        let test = AdoptionHarness::with_templates();
        let catalog = test.catalog_json();
        let template = &catalog["templates"][0];
        let source_before =
            std::fs::read(test.content_root.join("adoption/cat-misty/body.png")).unwrap();
        {
            let storage = test.storage.lock().unwrap();
            storage
                .db
                .execute_batch(
                    "INSERT INTO pets
                     (pet_id, schema_version, species, identity_mode, display_name,
                      creation_method, source_template_id, source_template_version,
                      lifecycle, created_at, updated_at)
                     VALUES ('pet-existing-retry', 1, 'cat', 'adopted', 'existing',
                             'adoption', 'cat-misty', 1, 'draft', '1', '1');
                     INSERT INTO creation_sessions
                     (session_id, pet_id, method, status, last_stable_status, current_step,
                      schema_version, created_at, updated_at)
                     VALUES ('session-existing-retry', 'pet-existing-retry', 'adoption',
                             'draft', 'draft', 'adoption', 1, '1', '1');",
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO creation_adoption_provenance
                     (session_id, source_template_id, source_template_version,
                      runtime_schema_version, body_sha256, motion_profile_sha256, created_at)
                     VALUES ('session-existing-retry', 'cat-misty', 1, 3, ?1, ?2, '1')",
                    rusqlite::params![
                        template["bodySha256"].as_str().unwrap(),
                        template["motionProfileSha256"].as_str().unwrap()
                    ],
                )
                .unwrap();
            storage
                .db
                .execute_batch(
                    "CREATE TRIGGER fail_existing_adoption_candidate
                     BEFORE INSERT ON appearance_variants
                     BEGIN SELECT RAISE(ABORT, 'injected existing candidate DB failure'); END;",
                )
                .unwrap();
        }

        let error = test
            .service
            .start_adoption("cat-misty", "ignored")
            .unwrap_err();

        assert!(
            error.contains("injected existing candidate DB failure"),
            "{error}"
        );
        assert_eq!(test.row_count("pets"), 1);
        assert_eq!(test.row_count("creation_sessions"), 1);
        assert_eq!(test.row_count("creation_adoption_provenance"), 1);
        assert_eq!(test.row_count("appearance_variants"), 0);
        assert!(!test
            .root
            .join("creation-sessions/session-existing-retry/candidate")
            .exists());
        assert_eq!(
            std::fs::read(test.content_root.join("adoption/cat-misty/body.png")).unwrap(),
            source_before
        );
    }

    #[test]
    fn response_loss_retry_reuses_the_committed_candidate_exactly_once() {
        let test = AdoptionHarness::with_templates();
        let lost_response = test.service.start_adoption("cat-misty", "闆鹃浘").unwrap();

        let recovered = test.service.start_adoption("cat-misty", "ignored").unwrap();

        assert_eq!(recovered.pet_id, lost_response.pet_id);
        assert_eq!(recovered.session_id, lost_response.session_id);
        assert_eq!(recovered.candidate_id, lost_response.candidate_id);
        assert_eq!(test.row_count("pets"), 1);
        assert_eq!(test.row_count("creation_sessions"), 1);
        assert_eq!(test.row_count("creation_adoption_provenance"), 1);
        assert_eq!(test.row_count("appearance_variants"), 1);
    }

    #[test]
    fn provenance_insert_failure_rolls_back_pet_and_session_in_the_same_transaction() {
        let test = AdoptionHarness::with_templates();
        let source_before =
            std::fs::read(test.content_root.join("adoption/cat-misty/body.png")).unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "CREATE TRIGGER fail_adoption_provenance
                 BEFORE INSERT ON creation_adoption_provenance
                 BEGIN SELECT RAISE(ABORT, 'injected provenance failure'); END;",
            )
            .unwrap();

        let error = test
            .service
            .start_adoption("cat-misty", "闆鹃浘")
            .unwrap_err();

        assert!(error.contains("injected provenance failure"), "{error}");
        assert_eq!(test.row_count("pets"), 0);
        assert_eq!(test.row_count("creation_sessions"), 0);
        assert_eq!(test.row_count("creation_adoption_provenance"), 0);
        assert_eq!(test.row_count("appearance_variants"), 0);
        assert_eq!(
            std::fs::read(test.content_root.join("adoption/cat-misty/body.png")).unwrap(),
            source_before
        );
    }

    #[test]
    fn adopted_errors_are_deterministic_and_abandon_releases_the_template() {
        let test = AdoptionHarness::with_templates();
        let first = test.service.start_adoption("cat-misty", "雾雾").unwrap();
        test.complete(&first);
        let error = test
            .service
            .start_adoption("cat-misty", "云云")
            .unwrap_err();
        assert!(error.contains("already adopted"), "{error}");
        assert!(error.contains(&first.pet_id), "{error}");

        let retry = test.service.start_adoption("cat-sunny", "阳阳").unwrap();
        let source = std::fs::read(test.content_root.join("adoption/cat-sunny/body.png")).unwrap();
        test.service.abandon(&retry.session_id).unwrap();
        let replacement = test.service.start_adoption("cat-sunny", "新阳阳").unwrap();
        assert_ne!(replacement.pet_id, retry.pet_id);
        assert_eq!(
            std::fs::read(test.content_root.join("adoption/cat-sunny/body.png")).unwrap(),
            source
        );
    }

    #[test]
    fn catalog_rejects_contradictory_or_incomplete_durable_facts() {
        let test = AdoptionHarness::with_templates();
        let first = test.service.start_adoption("cat-misty", "雾雾").unwrap();
        {
            let storage = test.storage.lock().unwrap();
            storage
                .db
                .execute_batch("DROP INDEX pets_unique_adoption_source")
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO pets
                     (pet_id, schema_version, species, identity_mode, display_name,
                      creation_method, source_template_id, source_template_version,
                      lifecycle, created_at, updated_at)
                     VALUES ('pet-duplicate', 1, 'cat', 'adopted', '重复', 'adoption',
                             'cat-misty', 1, 'draft', '2', '2')",
                    [],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO creation_sessions
                     (session_id, pet_id, method, status, last_stable_status, current_step,
                      schema_version, created_at, updated_at)
                     VALUES ('session-duplicate', 'pet-duplicate', 'adoption', 'draft',
                             'draft', 'adoption', 1, '2', '2')",
                    [],
                )
                .unwrap();
        }
        let error = test.service.adoption_catalog().unwrap_err();
        assert!(error.contains("contradictory adoption facts"), "{error}");

        {
            let storage = test.storage.lock().unwrap();
            storage
                .db
                .execute("DELETE FROM pets WHERE pet_id='pet-duplicate'", [])
                .unwrap();
            storage
                .db
                .execute(
                    "UPDATE pets SET lifecycle='ready', completed_at='3' WHERE pet_id=?1",
                    [&first.pet_id],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "UPDATE creation_sessions SET status='completed',
                     last_stable_status='completed', completed_at='3' WHERE session_id=?1",
                    [&first.session_id],
                )
                .unwrap();
        }
        let error = test.service.adoption_catalog().unwrap_err();
        assert!(error.contains("contradictory adoption facts"), "{error}");
    }

    #[test]
    fn catalog_rejects_a_non_positive_durable_source_version() {
        let test = AdoptionHarness::with_templates();
        let session = test.service.start_adoption("cat-misty", "雾雾").unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE pets SET source_template_version=0 WHERE pet_id=?1",
                [&session.pet_id],
            )
            .unwrap();

        let error = test.service.adoption_catalog().unwrap_err();

        assert!(error.contains("source version"), "{error}");
    }

    #[test]
    fn catalog_rejects_a_pet_hidden_by_a_ghost_source_when_provenance_still_names_the_template() {
        let test = AdoptionHarness::with_templates();
        let session = test.service.start_adoption("cat-misty", "ghost").unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE pets SET source_template_id='ghost-template' WHERE pet_id=?1",
                [&session.pet_id],
            )
            .unwrap();

        let projected = test.service.adoption_catalog();

        assert!(
            projected.is_err(),
            "ghost source pet disappeared from durable facts: {projected:?}"
        );
    }

    #[test]
    fn catalog_rejects_a_candidate_linked_to_the_session_but_owned_by_another_pet() {
        let test = AdoptionHarness::with_templates();
        let session = test
            .service
            .start_adoption("cat-misty", "cross-owned")
            .unwrap();
        let storage = test.storage.lock().unwrap();
        storage
            .db
            .execute(
                "INSERT INTO pets
                 (pet_id, schema_version, species, identity_mode, display_name,
                  creation_method, source_template_id, source_template_version,
                  lifecycle, completed_at, created_at, updated_at)
                 VALUES ('pet-cross-owner', 1, 'cat', 'realPet', 'other',
                         'upload', NULL, NULL, 'ready', '1', '1', '1')",
                [],
            )
            .unwrap();
        storage
            .db
            .execute(
                "UPDATE creation_sessions
                 SET status='draft', last_stable_status='draft' WHERE session_id=?1",
                [&session.session_id],
            )
            .unwrap();
        storage
            .db
            .execute(
                "UPDATE appearance_variants SET pet_id=?2 WHERE session_id=?1",
                rusqlite::params![session.session_id, "pet-cross-owner"],
            )
            .unwrap();
        drop(storage);

        let projected = test.service.adoption_catalog();

        assert!(
            projected.is_err(),
            "cross-owned candidate was hidden by the exact-key count: {projected:?}"
        );
    }

    #[test]
    fn catalog_rejects_same_version_provenance_hash_tampering() {
        let mut unexpected_successes = Vec::new();
        for column in ["body_sha256", "motion_profile_sha256"] {
            let test = AdoptionHarness::with_templates();
            let session = test
                .service
                .start_adoption("cat-misty", "hash-check")
                .unwrap();
            test.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    &format!(
                        "UPDATE creation_adoption_provenance SET {column}=?2 WHERE session_id=?1"
                    ),
                    rusqlite::params![session.session_id, "0".repeat(64)],
                )
                .unwrap();
            if test.service.adoption_catalog().is_ok() {
                unexpected_successes.push(column);
            }
        }

        assert!(
            unexpected_successes.is_empty(),
            "same-version provenance hash tampering was projected: {unexpected_successes:?}"
        );
    }

    #[test]
    fn adopted_projection_requires_the_animated_runtime_manifest_contract() {
        let mut unexpected_successes = Vec::new();
        for (label, mutation) in [
            (
                "static-png runtime",
                "UPDATE variants SET style_id='static-png'",
            ),
            (
                "nonstandard manifest path",
                "UPDATE variants SET manifest_path='manifest.json'",
            ),
        ] {
            let test = AdoptionHarness::with_templates();
            let session = test
                .service
                .start_adoption("cat-misty", "runtime-check")
                .unwrap();
            test.complete(&session);
            test.storage
                .lock()
                .unwrap()
                .db
                .execute_batch(mutation)
                .unwrap();
            if test.service.adoption_catalog().is_ok() {
                unexpected_successes.push(label);
            }
        }

        assert!(
            unexpected_successes.is_empty(),
            "invalid accepted runtimes were projected: {unexpected_successes:?}"
        );
    }

    #[test]
    fn durable_fact_projection_rejects_metadata_and_lifecycle_contradictions() {
        let test = AdoptionHarness::with_templates();
        let session = test.service.start_adoption("cat-misty", "闆鹃浘").unwrap();
        let template = test.template("cat-misty");
        let cases = [
            (
                "wrong species",
                "UPDATE pets SET species='dog' WHERE source_template_id='cat-misty'",
                "UPDATE pets SET species='cat' WHERE source_template_id='cat-misty'",
            ),
            (
                "wrong identity",
                "UPDATE pets SET identity_mode='realPet' WHERE source_template_id='cat-misty'",
                "UPDATE pets SET identity_mode='adopted' WHERE source_template_id='cat-misty'",
            ),
            (
                "wrong pet schema",
                "UPDATE pets SET schema_version=99 WHERE source_template_id='cat-misty'",
                "UPDATE pets SET schema_version=1 WHERE source_template_id='cat-misty'",
            ),
            (
                "wrong session schema",
                "UPDATE creation_sessions SET schema_version=99 WHERE method='adoption'",
                "UPDATE creation_sessions SET schema_version=1 WHERE method='adoption'",
            ),
            (
                "candidateReady with draft lastStable",
                "UPDATE creation_sessions SET last_stable_status='draft' WHERE method='adoption'",
                "UPDATE creation_sessions SET last_stable_status='candidateReady' WHERE method='adoption'",
            ),
            (
                "one-sided abandoned pet",
                "UPDATE pets SET lifecycle='abandoned' WHERE source_template_id='cat-misty'",
                "UPDATE pets SET lifecycle='draft' WHERE source_template_id='cat-misty'",
            ),
            (
                "ready pet without completion",
                "UPDATE pets SET lifecycle='ready' WHERE source_template_id='cat-misty'",
                "UPDATE pets SET lifecycle='draft' WHERE source_template_id='cat-misty'",
            ),
            (
                "completed session with draft pet",
                "UPDATE creation_sessions
                 SET status='completed', last_stable_status='completed', completed_at='1'
                 WHERE method='adoption'",
                "UPDATE creation_sessions
                 SET status='candidateReady', last_stable_status='candidateReady',
                     completed_at=NULL WHERE method='adoption'",
            ),
            (
                "wrong session method",
                "UPDATE creation_sessions SET method='composer' WHERE method='adoption'",
                "UPDATE creation_sessions SET method='adoption' WHERE method='composer'",
            ),
            (
                "wrong provenance template",
                "UPDATE creation_adoption_provenance
                 SET source_template_id='cat-sunny'",
                "UPDATE creation_adoption_provenance
                 SET source_template_id='cat-misty'",
            ),
            (
                "wrong provenance source version",
                "UPDATE creation_adoption_provenance SET source_template_version=2",
                "UPDATE creation_adoption_provenance SET source_template_version=1",
            ),
            (
                "wrong provenance runtime version",
                "UPDATE creation_adoption_provenance SET runtime_schema_version=2",
                "UPDATE creation_adoption_provenance SET runtime_schema_version=3",
            ),
        ];
        let mut unexpected_successes = Vec::new();
        for (label, mutate, restore) in cases {
            let storage = test.storage.lock().unwrap();
            storage.db.execute_batch(mutate).unwrap();
            let facts = adoption_facts(&storage.db, "cat-misty").unwrap();
            if project_facts(&facts, &template, &test.root).is_ok() {
                unexpected_successes.push(label);
            }
            storage.db.execute_batch(restore).unwrap();
        }

        {
            let storage = test.storage.lock().unwrap();
            storage
                .db
                .execute_batch(
                    "DROP TRIGGER pets_validate_source_template_update;
                     UPDATE pets SET creation_method='composer'
                     WHERE source_template_id='cat-misty';",
                )
                .unwrap();
            let facts = adoption_facts(&storage.db, "cat-misty").unwrap();
            if project_facts(&facts, &template, &test.root).is_ok() {
                unexpected_successes.push("wrong pet creation method");
            }
            storage
                .db
                .execute(
                    "UPDATE pets SET creation_method='adoption' WHERE pet_id=?1",
                    [&session.pet_id],
                )
                .unwrap();
        }

        test.complete(&session);
        {
            let storage = test.storage.lock().unwrap();
            storage
                .db
                .execute(
                    "UPDATE creation_sessions
                     SET status='finalizing', last_stable_status='candidateReady'
                     WHERE session_id=?1",
                    [&session.session_id],
                )
                .unwrap();
            let facts = adoption_facts(&storage.db, "cat-misty").unwrap();
            if project_facts(&facts, &template, &test.root).is_ok() {
                unexpected_successes.push("ready accepted pet with finalizing session");
            }
        }

        assert!(
            unexpected_successes.is_empty(),
            "facts incorrectly accepted: {unexpected_successes:?}"
        );
    }

    #[test]
    fn coherent_abandoned_facts_are_ignored_but_abandoned_candidates_are_not() {
        let test = AdoptionHarness::with_templates();
        let session = test.service.start_adoption("cat-misty", "闆鹃浘").unwrap();
        let template = test.template("cat-misty");
        let storage = test.storage.lock().unwrap();
        storage
            .db
            .execute(
                "UPDATE pets SET lifecycle='abandoned' WHERE pet_id=?1",
                [&session.pet_id],
            )
            .unwrap();
        storage
            .db
            .execute(
                "UPDATE creation_sessions
                 SET status='abandoned', last_stable_status='abandoned', current_step='abandoned'
                 WHERE session_id=?1",
                [&session.session_id],
            )
            .unwrap();

        let facts = adoption_facts(&storage.db, "cat-misty").unwrap();
        assert!(project_facts(&facts, &template, &test.root).is_err());

        storage
            .db
            .execute(
                "DELETE FROM appearance_variants WHERE session_id=?1",
                [&session.session_id],
            )
            .unwrap();
        let facts = adoption_facts(&storage.db, "cat-misty").unwrap();
        assert_eq!(
            project_facts(&facts, &template, &test.root).unwrap(),
            (None, None)
        );
    }

    #[test]
    fn retry_always_verifies_no_intent_candidate_files_and_database_paths() {
        let test = AdoptionHarness::with_templates();
        let session = test.service.start_adoption("cat-misty", "闆鹃浘").unwrap();
        let body_path = test.candidate_file(&session, "body.png");
        let profile_path = test.candidate_file(&session, "motion-profile.json");
        let source_body =
            std::fs::read(test.content_root.join("adoption/cat-misty/body.png")).unwrap();
        let source_profile = std::fs::read(
            test.content_root
                .join("adoption/cat-misty/motion-profile.json"),
        )
        .unwrap();
        let mut unexpected_successes = Vec::new();

        let mut tampered = image::load_from_memory(&source_body).unwrap().to_rgba8();
        tampered.put_pixel(300, 300, image::Rgba([1, 2, 3, 255]));
        tampered.save(&body_path).unwrap();
        if test.service.start_adoption("cat-misty", "ignored").is_ok() {
            unexpected_successes.push("same-version valid PNG tamper");
        }
        std::fs::write(&body_path, &source_body).unwrap();

        std::fs::remove_file(&profile_path).unwrap();
        if test.service.start_adoption("cat-misty", "ignored").is_ok() {
            unexpected_successes.push("missing profile without intent");
        }
        std::fs::write(&profile_path, &source_profile).unwrap();

        std::fs::write(&profile_path, b"not-json").unwrap();
        if test.service.start_adoption("cat-misty", "ignored").is_ok() {
            unexpected_successes.push("corrupt profile without intent");
        }
        std::fs::write(&profile_path, &source_profile).unwrap();

        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE appearance_variants SET image_path=?2 WHERE session_id=?1",
                rusqlite::params![
                    session.session_id,
                    test.content_root
                        .join("adoption/cat-misty/body.png")
                        .to_string_lossy()
                ],
            )
            .unwrap();
        if test.service.start_adoption("cat-misty", "ignored").is_ok() {
            unexpected_successes.push("database body path outside owned candidate");
        }

        assert!(
            unexpected_successes.is_empty(),
            "retry verification skipped: {unexpected_successes:?}"
        );
        assert_eq!(
            std::fs::read(test.content_root.join("adoption/cat-misty/body.png")).unwrap(),
            source_body
        );
    }

    #[test]
    fn older_version_retry_uses_original_provenance_and_rejects_tamper() {
        let test = AdoptionHarness::with_templates();
        let session = test.service.start_adoption("cat-misty", "闆鹃浘").unwrap();
        let original_candidate = std::fs::read(test.candidate_file(&session, "body.png")).unwrap();
        let source_path = test.content_root.join("adoption/cat-misty/body.png");
        let mut upgraded = image::load_from_memory(&original_candidate)
            .unwrap()
            .to_rgba8();
        upgraded.put_pixel(350, 350, image::Rgba([9, 8, 7, 255]));
        upgraded.save(&source_path).unwrap();
        let upgraded_bytes = std::fs::read(&source_path).unwrap();
        let mut catalog = test.catalog_json();
        catalog["templates"][0]["templateVersion"] = serde_json::json!(2);
        catalog["templates"][0]["bodySha256"] = serde_json::json!(sha256(&upgraded_bytes));
        test.write_catalog_json(&catalog);

        let retry = test.service.start_adoption("cat-misty", "ignored").unwrap();
        assert_eq!(retry.session_id, session.session_id);
        assert_eq!(
            std::fs::read(test.candidate_file(&session, "body.png")).unwrap(),
            original_candidate
        );

        let mut tampered = image::load_from_memory(&original_candidate)
            .unwrap()
            .to_rgba8();
        tampered.put_pixel(400, 400, image::Rgba([4, 5, 6, 255]));
        tampered
            .save(test.candidate_file(&session, "body.png"))
            .unwrap();
        let error = test
            .service
            .start_adoption("cat-misty", "ignored")
            .unwrap_err();
        assert!(error.contains("candidate"), "{error}");
    }

    #[test]
    fn retry_fails_closed_when_adoption_provenance_is_missing_or_corrupt() {
        for mutation in [
            "DELETE FROM creation_adoption_provenance WHERE session_id=?1",
            "UPDATE creation_adoption_provenance
             SET body_sha256='0000000000000000000000000000000000000000000000000000000000000000'
             WHERE session_id=?1",
            "UPDATE creation_adoption_provenance
             SET source_template_id='cat-sunny' WHERE session_id=?1",
            "UPDATE creation_adoption_provenance
             SET source_template_version=2 WHERE session_id=?1",
            "UPDATE creation_adoption_provenance
             SET runtime_schema_version=2 WHERE session_id=?1",
        ] {
            let test = AdoptionHarness::with_templates();
            let session = test.service.start_adoption("cat-misty", "闆鹃浘").unwrap();
            {
                let storage = test.storage.lock().unwrap();
                storage.db.execute(mutation, [&session.session_id]).unwrap();
            }

            let error = test
                .service
                .start_adoption("cat-misty", "ignored")
                .unwrap_err();
            assert!(error.contains("provenance"), "{error}");
            assert!(test.candidate_file(&session, "body.png").exists());
        }
    }
}
