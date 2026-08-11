use crate::creation::content::ContentRoot;
use crate::creation::domain::{
    new_entity_id, ComposerRecipe, CreationMethod, CreationSessionStatus, CreationSnapshot,
};
use crate::creation::name::normalize_display_name;
use crate::creation::{candidate, composer, CreationStore};
use crate::pets::deletion::SharedPetDeletionService;
use crate::pets::mutation::{MutationKind, SharedPetMutationGate};
use crate::pets::repository::PetRepository;
use crate::runtime_assets::motion_profile::MotionProfileV1;
use crate::storage::Storage;
use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub type SharedCreationService = Arc<CreationService>;

pub struct CreationService {
    storage: Arc<Mutex<Storage>>,
    app_data_dir: PathBuf,
    deletion: SharedPetDeletionService,
    content_root: ContentRoot,
    mutation_gate: SharedPetMutationGate,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerCandidateProjection {
    pub snapshot: CreationSnapshot,
    pub body_url: String,
    pub motion_profile: MotionProfileV1,
}

#[derive(Debug, Clone, Default)]
pub struct ComposerOrphanRecoveryReport {
    pub recovered_count: usize,
    pub warnings: Vec<String>,
}

impl CreationService {
    pub fn new(
        storage: Arc<Mutex<Storage>>,
        app_data_dir: PathBuf,
        deletion: SharedPetDeletionService,
        content_root: ContentRoot,
        mutation_gate: SharedPetMutationGate,
    ) -> Self {
        Self {
            storage,
            app_data_dir,
            deletion,
            content_root,
            mutation_gate,
        }
    }

    pub fn start(&self, method: CreationMethod) -> Result<CreationSnapshot, String> {
        if method == CreationMethod::Adoption {
            return Err(
                "adoption creation requires a template source; use the adoption flow".into(),
            );
        }

        let session_id = new_entity_id("session");
        validate_component(&session_id, "session id")?;
        let current_step = method_value(method);
        let now = crate::creation::profiles::now_iso();
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;

        let pet = PetRepository::reserve_in_transaction(&tx, method, None)?;
        validate_component(&pet.pet_id, "pet id")?;
        if let Err(error) = tx.execute(
            "INSERT INTO creation_sessions
             (session_id, pet_id, method, status, last_stable_status, current_step,
              schema_version, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'draft', 'draft', ?4, 1, ?5, ?5)",
            rusqlite::params![
                session_id,
                pet.pet_id,
                method_value(method),
                current_step,
                now
            ],
        ) {
            if let Some(existing) = find_long_draft_id(&tx)? {
                return Err(format!(
                    "a creation draft is already active; continue or abandon session {existing}"
                ));
            }
            return Err(error.to_string());
        }
        tx.commit().map_err(|error| error.to_string())?;
        drop(storage);
        self.snapshot(&session_id)
    }

    pub fn draft(&self) -> Result<Option<CreationSnapshot>, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let session_id = find_long_draft_id(&storage.db)?;
        session_id
            .as_deref()
            .map(|id| snapshot_from_db(&storage.db, id))
            .transpose()
    }

    pub fn snapshot(&self, session_id: &str) -> Result<CreationSnapshot, String> {
        validate_component(session_id, "session id")?;
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        if let Some(snapshot) = live_snapshot_from_db(&storage.db, session_id)? {
            return Ok(snapshot);
        }
        tombstone_snapshot_from_db(&storage.db, session_id)?
            .ok_or_else(|| format!("creation session not found: {session_id}"))
    }

    pub fn set_name(&self, session_id: &str, value: &str) -> Result<CreationSnapshot, String> {
        validate_component(session_id, "session id")?;
        let display_name = normalize_display_name(value)?;
        let now = crate::creation::profiles::now_iso();
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let session: Option<(String, String)> = tx
            .query_row(
                "SELECT pet_id, status FROM creation_sessions WHERE session_id=?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (pet_id, status) = session
            .ok_or_else(|| format!("creation session not found or abandoned: {session_id}"))?;
        let status = parse_status(&status)?;
        if matches!(
            status,
            CreationSessionStatus::Completed | CreationSessionStatus::Abandoned
        ) {
            return Err(format!(
                "cannot rename a terminal creation session: {session_id}"
            ));
        }
        let affected = tx
            .execute(
                "UPDATE pets SET display_name=?2, updated_at=?3
                 WHERE pet_id=?1 AND EXISTS (
                   SELECT 1 FROM creation_sessions
                   WHERE session_id=?4 AND pet_id=?1
                 )",
                rusqlite::params![pet_id, display_name, now, session_id],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("creation session is not bound to its reserved pet".into());
        }
        tx.execute(
            "UPDATE creation_sessions SET updated_at=?2 WHERE session_id=?1",
            rusqlite::params![session_id, now],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        drop(storage);
        self.snapshot(session_id)
    }

    pub fn save_composer_recipe(
        &self,
        session_id: &str,
        recipe: &ComposerRecipe,
        current_step: &str,
    ) -> Result<CreationSnapshot, String> {
        validate_component(session_id, "session id")?;
        validate_composer_step(current_step)?;
        let observed = self.snapshot(session_id)?;
        let request_id = new_entity_id("composer-save");
        let _operation =
            self.mutation_gate
                .scoped(&request_id, MutationKind::Creation, &observed.pet_id)?;
        let now = crate::creation::profiles::now_iso();
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
        if pet_id != observed.pet_id || method != "composer" || status != "draft" {
            return Err("recipe save requires an editable composer draft".into());
        }

        let pack = composer::load_production_pack_manifest(&self.content_root)?;
        composer::validate_recipe(&pack, recipe)?;
        composer::validate_recipe_assets(&pack, &self.content_root, recipe)?;
        let first_save: bool = tx
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM composer_recipes WHERE session_id=?1)",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if first_save && current_step != "ears" {
            return Err("the first body selection must advance to ears".into());
        }
        if first_save && !composer::recipe_matches_body_defaults(&pack, recipe)? {
            return Err("the first body selection must use that body's complete defaults".into());
        }

        tx.execute(
            "INSERT INTO composer_recipes
             (session_id, recipe_version, pack_id, pack_version, layer_contract_version,
              body_id, ears_id, eyes_id, muzzle_id, tail_id, color_id, pattern_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(session_id) DO UPDATE SET
               recipe_version=excluded.recipe_version, pack_id=excluded.pack_id,
               pack_version=excluded.pack_version,
               layer_contract_version=excluded.layer_contract_version,
               body_id=excluded.body_id, ears_id=excluded.ears_id,
               eyes_id=excluded.eyes_id, muzzle_id=excluded.muzzle_id,
               tail_id=excluded.tail_id, color_id=excluded.color_id,
               pattern_id=excluded.pattern_id, updated_at=excluded.updated_at",
            rusqlite::params![
                session_id,
                recipe.recipe_version,
                recipe.pack_id,
                recipe.pack_version,
                recipe.layer_contract_version,
                recipe.body_id,
                recipe.ears_id,
                recipe.eyes_id,
                recipe.muzzle_id,
                recipe.tail_id,
                recipe.color_id,
                recipe.pattern_id,
                now,
            ],
        )
        .map_err(|error| error.to_string())?;
        let affected = tx
            .execute(
                "UPDATE creation_sessions SET current_step=?2, error=NULL, updated_at=?3
                 WHERE session_id=?1 AND pet_id=?4 AND method='composer' AND status='draft'",
                rusqlite::params![session_id, current_step, now, pet_id],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err("composer draft changed while its recipe was being saved".into());
        }
        tx.commit().map_err(|error| error.to_string())?;
        drop(storage);
        self.snapshot(session_id)
    }

    pub fn store_composer_candidate(
        &self,
        session_id: &str,
        png_b64: Option<&str>,
    ) -> Result<ComposerCandidateProjection, String> {
        validate_component(session_id, "session id")?;
        let observed = self.snapshot(session_id)?;
        let request_id = new_entity_id("composer-candidate");
        let _operation =
            self.mutation_gate
                .scoped(&request_id, MutationKind::Creation, &observed.pet_id)?;
        let stable = self.snapshot(session_id)?;
        if stable.pet_id != observed.pet_id || stable.method != CreationMethod::Composer {
            return Err("candidate requires its original composer session".into());
        }
        let existing_candidate = stable.status == CreationSessionStatus::CandidateReady
            || (stable.status == CreationSessionStatus::RetryableFailure
                && stable.last_stable_status == CreationSessionStatus::CandidateReady
                && stable.candidate_id.is_some());
        if stable.status != CreationSessionStatus::Draft && !existing_candidate {
            return Err("composer session is not eligible for a candidate".into());
        }
        if existing_candidate && png_b64.is_some() {
            return Err(
                "an existing composer candidate must be read without replacement input".into(),
            );
        }
        if !existing_candidate && png_b64.is_none() {
            return Err("a composer draft requires exported PNG input".into());
        }
        let recipe = stable
            .recipe
            .clone()
            .ok_or_else(|| "composer candidate requires a saved recipe".to_string())?;
        let pack = composer::load_production_pack_manifest(&self.content_root)?;
        composer::validate_recipe(&pack, &recipe)?;
        let motion_profile = composer::motion_profile_for_recipe(&pack, &recipe)?;
        let (snapshot, body, projection_profile) = if existing_candidate {
            let stored = candidate::read_exact_composer_candidate(
                &self.app_data_dir,
                session_id,
                &motion_profile,
                &recipe,
            )?;
            (stable, stored.body, stored.motion_profile)
        } else {
            composer::validate_recipe_assets(&pack, &self.content_root, &recipe)?;
            let decoded = candidate::decode_composer_png(png_b64.unwrap())?;
            let mut published = candidate::publish_composer_candidate(
                &self.app_data_dir,
                session_id,
                &decoded.bytes,
                &motion_profile,
                &recipe,
            )?;
            let store = CreationStore::new(self.storage.clone());
            let snapshot = match store.record_local_candidate(
                session_id,
                &published.body_path,
                &published.motion_profile_path,
            ) {
                Ok(_) => {
                    // The database now owns this exact published candidate. Commit the
                    // filesystem guard before any fallible post-commit projection read,
                    // so a read error cannot roll back files referenced by durable rows.
                    published.commit();
                    self.snapshot(session_id)?
                }
                Err(error) => {
                    let rollback = published.rollback();
                    return Err(match rollback {
                        Ok(()) => error,
                        Err(rollback) => format!("{error}; candidate rollback failed: {rollback}"),
                    });
                }
            };
            published.commit();
            (snapshot, decoded.bytes, motion_profile)
        };
        Ok(ComposerCandidateProjection {
            snapshot,
            body_url: format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&body)
            ),
            motion_profile: projection_profile,
        })
    }

    pub fn recover_composer_orphans(&self) -> Result<ComposerOrphanRecoveryReport, String> {
        let (drafts, committed): (Vec<(String, String)>, Vec<(String, String)>) = {
            let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
            let mut statement = storage
                .db
                .prepare(
                    "SELECT cs.session_id, cs.pet_id
                     FROM creation_sessions cs
                     WHERE cs.method='composer' AND cs.status='draft'
                       AND EXISTS (SELECT 1 FROM composer_recipes cr
                                   WHERE cr.session_id=cs.session_id)
                       AND NOT EXISTS (SELECT 1 FROM appearance_variants av
                                       WHERE av.session_id=cs.session_id)
                     ORDER BY cs.rowid",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            let mut statement = storage
                .db
                .prepare(
                    "SELECT cs.session_id, cs.pet_id
                     FROM creation_sessions cs
                     WHERE cs.method='composer'
                       AND EXISTS (SELECT 1 FROM appearance_variants av
                                   WHERE av.session_id=cs.session_id
                                     AND av.pet_id=cs.pet_id AND av.job_id IS NULL)
                     ORDER BY cs.rowid",
                )
                .map_err(|error| error.to_string())?;
            let committed = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            (rows, committed)
        };
        if drafts.is_empty() && committed.is_empty() {
            return Ok(ComposerOrphanRecoveryReport::default());
        }
        let mut report = ComposerOrphanRecoveryReport::default();
        let pack = match composer::load_production_pack_manifest(&self.content_root) {
            Ok(pack) => pack,
            Err(error) => {
                report.warnings.push(format!(
                    "trusted composer pack unavailable during orphan recovery: {error}"
                ));
                return Ok(report);
            }
        };
        for (session_id, pet_id) in committed {
            let result = (|| {
                let request_id = new_entity_id("composer-recover-committed");
                let _operation =
                    self.mutation_gate
                        .scoped(&request_id, MutationKind::Creation, &pet_id)?;
                let snapshot = self.snapshot(&session_id)?;
                if snapshot.method != CreationMethod::Composer
                    || snapshot.pet_id != pet_id
                    || snapshot.candidate_id.is_none()
                {
                    return Ok(false);
                }
                let recipe = snapshot
                    .recipe
                    .ok_or_else(|| "committed composer candidate lost its recipe".to_string())?;
                composer::validate_recipe(&pack, &recipe)?;
                let profile = composer::motion_profile_for_recipe(&pack, &recipe)?;
                match candidate::try_read_exact_composer_candidate(
                    &self.app_data_dir,
                    &session_id,
                    &profile,
                    &recipe,
                )? {
                    Some(_) => {
                        candidate::clear_committed_composer_publish_intent(
                            &self.app_data_dir,
                            &session_id,
                            &profile,
                            &recipe,
                        )?;
                        Ok(false)
                    }
                    None if snapshot.status == CreationSessionStatus::CandidateReady
                        || (snapshot.status == CreationSessionStatus::RetryableFailure
                            && snapshot.last_stable_status
                                == CreationSessionStatus::CandidateReady) =>
                    {
                        CreationStore::new(self.storage.clone())
                            .revert_missing_local_composer_candidate(&session_id)
                    }
                    None => Ok(false),
                }
            })();
            match result {
                Ok(true) => report.recovered_count += 1,
                Ok(false) => {}
                Err(error) => report.warnings.push(format!(
                    "committed composer candidate {session_id}: {error}"
                )),
            }
        }
        for (session_id, pet_id) in drafts {
            let result = (|| {
                let request_id = new_entity_id("composer-recover");
                let _operation =
                    self.mutation_gate
                        .scoped(&request_id, MutationKind::Creation, &pet_id)?;
                let snapshot = self.snapshot(&session_id)?;
                if snapshot.status != CreationSessionStatus::Draft
                    || snapshot.method != CreationMethod::Composer
                    || snapshot.pet_id != pet_id
                    || snapshot.candidate_id.is_some()
                {
                    return Ok(false);
                }
                let recipe = snapshot
                    .recipe
                    .ok_or_else(|| "composer recovery lost its durable recipe".to_string())?;
                composer::validate_recipe(&pack, &recipe)?;
                let profile = composer::motion_profile_for_recipe(&pack, &recipe)?;
                candidate::recover_exact_composer_orphan(
                    &self.app_data_dir,
                    &session_id,
                    &profile,
                    &recipe,
                )
            })();
            match result {
                Ok(true) => report.recovered_count += 1,
                Ok(false) => {}
                Err(error) => report
                    .warnings
                    .push(format!("composer orphan {session_id}: {error}")),
            }
        }
        Ok(report)
    }

    pub fn abandon(&self, session_id: &str) -> Result<(), String> {
        validate_component(session_id, "session id")?;
        let _resource_root = &self.app_data_dir;
        self.deletion.abandon_creation(session_id)
    }
}

#[derive(Debug)]
struct SnapshotRow {
    session_id: String,
    pet_id: String,
    method: String,
    status: String,
    last_stable_status: String,
    current_step: String,
    display_name: Option<String>,
    job_id: Option<String>,
    job_status: Option<String>,
    candidate_id: Option<String>,
    error: Option<String>,
}

fn find_long_draft_id(db: &Connection) -> Result<Option<String>, String> {
    db.query_row(
        "SELECT session_id FROM creation_sessions
         WHERE method IN ('upload','composer')
           AND status NOT IN ('completed','abandoned')
         ORDER BY updated_at DESC, rowid DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn snapshot_from_db(db: &Connection, session_id: &str) -> Result<CreationSnapshot, String> {
    live_snapshot_from_db(db, session_id)?
        .ok_or_else(|| format!("creation session not found: {session_id}"))
}

fn live_snapshot_from_db(
    db: &Connection,
    session_id: &str,
) -> Result<Option<CreationSnapshot>, String> {
    let row = db
        .query_row(
            "SELECT cs.session_id, cs.pet_id, cs.method, cs.status, cs.last_stable_status,
                    cs.current_step, p.display_name,
                    (SELECT gj.job_id FROM generation_jobs gj
                     WHERE gj.session_id=cs.session_id
                     ORDER BY gj.created_at DESC, gj.rowid DESC LIMIT 1),
                    (SELECT gj.status FROM generation_jobs gj
                     WHERE gj.session_id=cs.session_id
                     ORDER BY gj.created_at DESC, gj.rowid DESC LIMIT 1),
                    (SELECT av.variant_id FROM appearance_variants av
                     WHERE av.session_id=cs.session_id
                     ORDER BY av.created_at DESC, av.rowid DESC LIMIT 1),
                    cs.error
             FROM creation_sessions cs
             JOIN pets p ON p.pet_id=cs.pet_id
             WHERE cs.session_id=?1",
            [session_id],
            |row| {
                Ok(SnapshotRow {
                    session_id: row.get(0)?,
                    pet_id: row.get(1)?,
                    method: row.get(2)?,
                    status: row.get(3)?,
                    last_stable_status: row.get(4)?,
                    current_step: row.get(5)?,
                    display_name: row.get(6)?,
                    job_id: row.get(7)?,
                    job_status: row.get(8)?,
                    candidate_id: row.get(9)?,
                    error: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    row.map(|row| {
        let recipe = recipe_from_db(db, &row.session_id)?;
        Ok(CreationSnapshot {
            session_id: row.session_id,
            pet_id: row.pet_id,
            method: parse_method(&row.method)?,
            status: parse_status(&row.status)?,
            last_stable_status: parse_status(&row.last_stable_status)?,
            current_step: row.current_step,
            display_name: row.display_name,
            job_id: row.job_id,
            job_status: row.job_status,
            candidate_id: row.candidate_id,
            recipe,
            error: row.error,
        })
    })
    .transpose()
}

fn recipe_from_db(db: &Connection, session_id: &str) -> Result<Option<ComposerRecipe>, String> {
    db.query_row(
        "SELECT recipe_version, pack_id, pack_version, layer_contract_version,
                body_id, ears_id, eyes_id, muzzle_id, tail_id, color_id, pattern_id
         FROM composer_recipes WHERE session_id=?1",
        [session_id],
        |row| {
            Ok(ComposerRecipe {
                recipe_version: row.get(0)?,
                pack_id: row.get(1)?,
                pack_version: row.get(2)?,
                layer_contract_version: row.get(3)?,
                body_id: row.get(4)?,
                ears_id: row.get(5)?,
                eyes_id: row.get(6)?,
                muzzle_id: row.get(7)?,
                tail_id: row.get(8)?,
                color_id: row.get(9)?,
                pattern_id: row.get(10)?,
            })
        },
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn tombstone_snapshot_from_db(
    db: &Connection,
    session_id: &str,
) -> Result<Option<CreationSnapshot>, String> {
    let row: Option<(String, String, String)> = db
        .query_row(
            "SELECT session_id, pet_id, method FROM creation_session_tombstones
             WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    row.map(|(session_id, pet_id, method)| {
        Ok(CreationSnapshot {
            session_id,
            pet_id,
            method: parse_method(&method)?,
            status: CreationSessionStatus::Abandoned,
            last_stable_status: CreationSessionStatus::Abandoned,
            current_step: "abandoned".into(),
            display_name: None,
            job_id: None,
            job_status: None,
            candidate_id: None,
            recipe: None,
            error: None,
        })
    })
    .transpose()
}

fn method_value(method: CreationMethod) -> &'static str {
    match method {
        CreationMethod::Upload => "upload",
        CreationMethod::Composer => "composer",
        CreationMethod::Adoption => "adoption",
    }
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

fn validate_composer_step(value: &str) -> Result<(), String> {
    if matches!(
        value,
        "body" | "ears" | "eyes" | "muzzle" | "tail" | "coat" | "name" | "preview"
    ) {
        Ok(())
    } else {
        Err("invalid composer current step".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::domain::{CreationMethod, CreationSessionStatus};
    use crate::pets::active::{ActivePetService, BUILTIN_PET_ID};
    use crate::pets::deletion::PetDeletionService;
    use crate::pets::mutation::PetMutationGate;
    use crate::pets::{ActivePetSession, SharedActivePetSession};
    use crate::storage::Storage;
    use rusqlite::OptionalExtension;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct ServiceHarness {
        root: PathBuf,
        storage: Arc<Mutex<Storage>>,
        service: CreationService,
    }

    impl ServiceHarness {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let root = std::env::temp_dir().join(format!(
                "desktop-pet-creation-service-{}-{n}",
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
            let deletion = Arc::new(PetDeletionService::new(
                storage.clone(),
                active,
                root.clone(),
                gate.clone(),
            ));
            let content =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/creation-content");
            let service = CreationService::new(
                storage.clone(),
                root.clone(),
                deletion,
                crate::creation::content::test_content_root(&content).unwrap(),
                gate,
            );
            Self {
                root,
                storage,
                service,
            }
        }

        fn count(&self, table: &str) -> i64 {
            assert!(matches!(
                table,
                "pets" | "creation_sessions" | "creation_session_tombstones"
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

        fn create_resources(&self, session_id: &str, pet_id: &str, job_id: &str) {
            self.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO generation_jobs
                     (job_id, pet_id, session_id, prompt, ref_sha256, status, created_at)
                     VALUES (?1, ?2, ?3, 'prompt', 'hash', 'pending', '0')",
                    rusqlite::params![job_id, pet_id, session_id],
                )
                .unwrap();
            for directory in [
                self.root.join("creation-sessions").join(session_id),
                self.root.join("pets").join(pet_id),
                self.root.join("jobs").join(job_id),
            ] {
                std::fs::create_dir_all(&directory).unwrap();
                std::fs::write(directory.join("owned.txt"), b"owned").unwrap();
            }
        }

        fn insert_other_session(&self) {
            self.storage
                .lock()
                .unwrap()
                .db
                .execute_batch(
                    "INSERT INTO pets
                     (pet_id, schema_version, species, identity_mode, creation_method, lifecycle,
                      created_at, updated_at)
                     VALUES ('pet-other', 1, 'cat', 'adopted', 'adoption', 'ready', '0', '0');
                     INSERT INTO creation_sessions
                     (session_id, pet_id, method, status, last_stable_status, current_step,
                      schema_version, created_at, updated_at, completed_at)
                     VALUES ('session-other', 'pet-other', 'adoption', 'completed', 'completed',
                             'completed', 1, '0', '0', '0');",
                )
                .unwrap();
            std::fs::create_dir_all(self.root.join("creation-sessions/session-other")).unwrap();
            std::fs::write(
                self.root.join("creation-sessions/session-other/keep.txt"),
                b"keep",
            )
            .unwrap();
        }
    }

    impl Drop for ServiceHarness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn assert_safe_component(value: &str) {
        assert!(!value.is_empty());
        assert!(value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
    }

    #[test]
    fn starts_one_upload_or_composer_draft_and_returns_it() {
        let test = ServiceHarness::new();
        let upload = test.service.start(CreationMethod::Upload).unwrap();
        assert_eq!(upload.method, CreationMethod::Upload);
        assert_eq!(upload.status, CreationSessionStatus::Draft);
        assert_eq!(upload.last_stable_status, CreationSessionStatus::Draft);
        assert_eq!(test.service.draft().unwrap().unwrap(), upload);
        assert_safe_component(&upload.session_id);
        assert_safe_component(&upload.pet_id);

        let error = test.service.start(CreationMethod::Composer).unwrap_err();
        assert!(error.contains(&upload.session_id));
        assert_eq!(test.count("pets"), 1);
        assert_eq!(test.count("creation_sessions"), 1);
    }

    #[test]
    fn rejects_adoption_without_reserving_a_pet() {
        let test = ServiceHarness::new();
        let error = test.service.start(CreationMethod::Adoption).unwrap_err();
        assert!(error.contains("adoption"));
        assert_eq!(test.count("pets"), 0);
        assert_eq!(test.count("creation_sessions"), 0);
    }

    #[test]
    fn rolls_back_the_reserved_pet_when_session_creation_fails() {
        let test = ServiceHarness::new();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "CREATE TRIGGER fail_session_insert BEFORE INSERT ON creation_sessions
                 BEGIN SELECT RAISE(ABORT, 'forced session failure'); END;",
            )
            .unwrap();

        assert!(test.service.start(CreationMethod::Upload).is_err());
        assert_eq!(test.count("pets"), 0);
        assert_eq!(test.count("creation_sessions"), 0);
    }

    #[test]
    fn restores_the_same_persisted_draft_from_a_new_service_instance() {
        let test = ServiceHarness::new();
        let draft = test.service.start(CreationMethod::Composer).unwrap();
        let reopened = CreationService::new(
            test.storage.clone(),
            test.root.clone(),
            test.service.deletion.clone(),
            test.service.content_root.clone(),
            test.service.mutation_gate.clone(),
        );
        assert_eq!(reopened.draft().unwrap(), Some(draft));
    }

    #[test]
    fn saves_a_normalized_name_on_only_the_session_pet() {
        let test = ServiceHarness::new();
        let draft = test.service.start(CreationMethod::Composer).unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO pets
                 (pet_id, schema_version, species, identity_mode, creation_method, lifecycle,
                  created_at, updated_at)
                 VALUES ('pet-other', 1, 'cat', 'realpet', 'upload', 'ready', '0', '0')",
                [],
            )
            .unwrap();

        let saved = test
            .service
            .set_name(&draft.session_id, "  团子  ")
            .unwrap();
        assert_eq!(saved.display_name.as_deref(), Some("团子"));
        assert_eq!(saved.pet_id, draft.pet_id);
        let other_name: Option<String> = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT display_name FROM pets WHERE pet_id='pet-other'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(other_name, None);
        assert!(test.service.set_name("session-missing", "奶糖").is_err());
    }

    #[test]
    fn set_name_rejects_terminal_sessions_and_invalid_names() {
        let test = ServiceHarness::new();
        let draft = test.service.start(CreationMethod::Upload).unwrap();
        assert!(test.service.set_name(&draft.session_id, "\n").is_err());
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions SET status='completed' WHERE session_id=?1",
                [&draft.session_id],
            )
            .unwrap();
        assert!(test.service.set_name(&draft.session_id, "奶糖").is_err());
    }

    #[test]
    fn abandoning_twice_is_idempotent_and_snapshot_converges_on_tombstone() {
        let test = ServiceHarness::new();
        let draft = test.service.start(CreationMethod::Upload).unwrap();
        test.service.abandon(&draft.session_id).unwrap();
        test.service.abandon(&draft.session_id).unwrap();
        assert!(test.service.draft().unwrap().is_none());
        assert_eq!(test.count("pets"), 0);
        assert_eq!(test.count("creation_sessions"), 0);
        assert_eq!(test.count("creation_session_tombstones"), 1);
        let abandoned = test.service.snapshot(&draft.session_id).unwrap();
        assert_eq!(abandoned.session_id, draft.session_id);
        assert_eq!(abandoned.pet_id, draft.pet_id);
        assert_eq!(abandoned.method, draft.method);
        assert_eq!(abandoned.status, CreationSessionStatus::Abandoned);
        assert_eq!(
            abandoned.last_stable_status,
            CreationSessionStatus::Abandoned
        );
        assert!(test.service.start(CreationMethod::Composer).is_ok());
    }

    #[test]
    fn abandon_removes_only_the_exact_session_pet_and_job_resources() {
        let test = ServiceHarness::new();
        let draft = test.service.start(CreationMethod::Upload).unwrap();
        test.create_resources(&draft.session_id, &draft.pet_id, "job-owned");
        for directory in [
            test.root.join("creation-sessions").join("session-other"),
            test.root.join("pets").join("pet-other-files"),
            test.root.join("jobs").join("job-other"),
        ] {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("keep.txt"), b"keep").unwrap();
        }

        test.service.abandon(&draft.session_id).unwrap();

        assert!(!test
            .root
            .join("creation-sessions")
            .join(&draft.session_id)
            .exists());
        assert!(!test.root.join("pets").join(&draft.pet_id).exists());
        assert!(!test.root.join("jobs").join("job-owned").exists());
        assert!(test
            .root
            .join("creation-sessions/session-other/keep.txt")
            .exists());
        assert!(test.root.join("pets/pet-other-files/keep.txt").exists());
        assert!(test.root.join("jobs/job-other/keep.txt").exists());
    }

    #[test]
    fn abandon_restores_all_isolated_resources_when_the_transaction_fails() {
        let test = ServiceHarness::new();
        let draft = test.service.start(CreationMethod::Upload).unwrap();
        test.create_resources(&draft.session_id, &draft.pet_id, "job-owned");
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(&format!(
                "CREATE TRIGGER fail_creation_pet_delete BEFORE DELETE ON pets
                 WHEN OLD.pet_id = '{}'
                 BEGIN SELECT RAISE(ABORT, 'forced abandon failure'); END;",
                draft.pet_id
            ))
            .unwrap();

        assert!(test.service.abandon(&draft.session_id).is_err());

        assert!(test
            .root
            .join("creation-sessions")
            .join(&draft.session_id)
            .exists());
        assert!(test.root.join("pets").join(&draft.pet_id).exists());
        assert!(test.root.join("jobs/job-owned").exists());
        assert_eq!(test.count("pets"), 1);
        assert_eq!(test.count("creation_sessions"), 1);
        assert_eq!(test.count("creation_session_tombstones"), 0);
    }

    #[test]
    fn abandon_rejects_a_null_job_session_before_isolating_resources() {
        let test = ServiceHarness::new();
        let draft = test.service.start(CreationMethod::Upload).unwrap();
        test.create_resources(&draft.session_id, &draft.pet_id, "job-owned");
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE generation_jobs SET session_id=NULL WHERE job_id='job-owned'",
                [],
            )
            .unwrap();

        let error = test.service.abandon(&draft.session_id).unwrap_err();

        assert!(error.contains("job-owned"));
        assert!(test
            .root
            .join("creation-sessions")
            .join(&draft.session_id)
            .exists());
        assert!(test.root.join("pets").join(&draft.pet_id).exists());
        assert!(test.root.join("jobs/job-owned").exists());
        assert_eq!(test.count("pets"), 1);
        assert_eq!(test.count("creation_session_tombstones"), 0);
    }

    #[test]
    fn abandon_rejects_a_job_owned_by_another_session_without_isolating_it() {
        let test = ServiceHarness::new();
        let draft = test.service.start(CreationMethod::Composer).unwrap();
        test.create_resources(&draft.session_id, &draft.pet_id, "job-owned");
        test.insert_other_session();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE generation_jobs SET session_id='session-other' WHERE job_id='job-owned'",
                [],
            )
            .unwrap();

        let error = test.service.abandon(&draft.session_id).unwrap_err();

        assert!(error.contains("job-owned"));
        assert!(test
            .root
            .join("creation-sessions")
            .join(&draft.session_id)
            .exists());
        assert!(test.root.join("pets").join(&draft.pet_id).exists());
        assert!(test.root.join("jobs/job-owned").exists());
        assert!(test
            .root
            .join("creation-sessions/session-other/keep.txt")
            .exists());
        assert_eq!(test.count("creation_session_tombstones"), 0);
    }

    #[test]
    fn abandon_rejects_a_job_with_the_target_session_but_another_pet() {
        let test = ServiceHarness::new();
        let draft = test.service.start(CreationMethod::Upload).unwrap();
        test.insert_other_session();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO generation_jobs
                 (job_id, pet_id, session_id, prompt, ref_sha256, status, created_at)
                 VALUES ('job-cross-owned', 'pet-other', ?1, 'prompt', 'hash', 'pending', '0')",
                [&draft.session_id],
            )
            .unwrap();
        std::fs::create_dir_all(test.root.join("jobs/job-cross-owned")).unwrap();
        std::fs::write(test.root.join("jobs/job-cross-owned/keep.txt"), b"keep").unwrap();

        let error = test.service.abandon(&draft.session_id).unwrap_err();

        assert!(error.contains("job-cross-owned"));
        assert!(test.root.join("jobs/job-cross-owned/keep.txt").exists());
        assert!(test
            .root
            .join("creation-sessions/session-other/keep.txt")
            .exists());
        assert_eq!(test.count("creation_session_tombstones"), 0);
    }

    #[test]
    fn session_ids_cannot_escape_their_resource_roots() {
        let test = ServiceHarness::new();
        assert!(test.service.snapshot("../outside").is_err());
        assert!(test.service.abandon("../outside").is_err());
        let tombstone = test
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT pet_id FROM creation_session_tombstones WHERE session_id='../outside'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .unwrap();
        assert_eq!(tombstone, None);
    }
}
