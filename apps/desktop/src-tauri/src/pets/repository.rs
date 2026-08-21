use crate::creation::domain::{new_entity_id, CreationMethod};
use crate::pets::active::BUILTIN_PET_ID;
use crate::pets::pet::{IdentityMode, Pet, PetSummary, Species};
use crate::pets::profile::{
    validate_birth_date, validate_profile_update, PetGender, PetProfile, PetProfileUpdate,
};
use crate::storage::Storage;
use rusqlite::{OptionalExtension, TransactionBehavior};
use std::sync::{Arc, Mutex};

pub struct PetRepository {
    storage: Arc<Mutex<Storage>>,
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

fn profile_now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_else(|_| "0".into())
}

impl PetRepository {
    pub fn new(storage: Arc<Mutex<Storage>>) -> Self {
        Self { storage }
    }

    pub fn list(&self) -> Result<Vec<PetSummary>, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let mut statement = db
            .prepare(
                "SELECT pet_id, species, identity_mode, display_name, creation_method,
                        source_template_id, source_template_version, lifecycle, completed_at,
                        created_at
                 FROM pets ORDER BY created_at",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                let species: String = row.get(1)?;
                let mode: String = row.get(2)?;
                let method: String = row.get(4)?;
                Ok(PetSummary {
                    pet_id: row.get(0)?,
                    species: parse_species(&species),
                    identity_mode: parse_mode(&mode),
                    display_name: row.get(3)?,
                    creation_method: parse_creation_method_column(&method, 4)?,
                    source_template_id: row.get(5)?,
                    source_template_version: row.get(6)?,
                    lifecycle: row.get(7)?,
                    completed_at: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })
            .map_err(|error| error.to_string())?;
        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row.map_err(|error| error.to_string())?);
        }
        Ok(summaries)
    }

    pub fn create(&self, species: Species, mode: IdentityMode) -> Result<Pet, String> {
        let method = method_for_identity_mode(mode);
        self.insert(species, mode, method, None)
    }

    pub fn reserve(
        &self,
        method: CreationMethod,
        source_template: Option<(&str, u32)>,
    ) -> Result<Pet, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        reserve_on_connection(&storage.db, method, source_template)
    }

    pub(crate) fn reserve_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        method: CreationMethod,
        source_template: Option<(&str, u32)>,
    ) -> Result<Pet, String> {
        reserve_on_connection(tx, method, source_template)
    }

    fn insert(
        &self,
        species: Species,
        mode: IdentityMode,
        method: CreationMethod,
        source_template: Option<(&str, u32)>,
    ) -> Result<Pet, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        insert_on_connection(&storage.db, species, mode, method, source_template)
    }

    pub fn get(&self, pet_id: &str) -> Result<Option<Pet>, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let mut statement = db
            .prepare(
                "SELECT pet_id, schema_version, species, identity_mode, display_name,
                        creation_method, source_template_id, source_template_version,
                        lifecycle, completed_at, created_at, updated_at
                 FROM pets WHERE pet_id = ?1",
            )
            .map_err(|error| error.to_string())?;
        let mut rows = statement
            .query_map(rusqlite::params![pet_id], |row| {
                let species: String = row.get(2)?;
                let mode: String = row.get(3)?;
                let method: String = row.get(5)?;
                Ok(Pet {
                    pet_id: row.get(0)?,
                    schema_version: row.get(1)?,
                    species: parse_species(&species),
                    identity_mode: parse_mode(&mode),
                    display_name: row.get(4)?,
                    creation_method: parse_creation_method_column(&method, 5)?,
                    source_template_id: row.get(6)?,
                    source_template_version: row.get(7)?,
                    lifecycle: row.get(8)?,
                    completed_at: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })
            .map_err(|error| error.to_string())?;
        match rows.next() {
            Some(row) => row.map(Some).map_err(|error| error.to_string()),
            None => Ok(None),
        }
    }

    pub fn get_profile(&self, pet_id: &str) -> Result<Option<PetProfile>, String> {
        if pet_id == BUILTIN_PET_ID {
            return Ok(Some(PetProfile {
                schema_version: 1,
                pet_id: BUILTIN_PET_ID.into(),
                display_name: "默认猫 · Live2D".into(),
                gender: PetGender::Unknown,
                birth_date: None,
                editable: false,
                updated_at: String::new(),
            }));
        }

        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let rows = {
            let mut statement = tx
                .prepare(
                    "SELECT pets.display_name, pets.lifecycle, pets.completed_at, pets.updated_at,
                        pet_profiles.pet_id, pet_profiles.schema_version, pet_profiles.gender_code,
                        pet_profiles.birth_date, pet_profiles.updated_at
                 FROM pets
                 LEFT JOIN pet_profiles ON pet_profiles.pet_id = pets.pet_id
                 WHERE pets.pet_id = ?1",
                )
                .map_err(|error| error.to_string())?;
            let mapped = statement
                .query_map([pet_id], |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                    ))
                })
                .map_err(|error| error.to_string())?;
            mapped
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?
        };
        if rows.len() > 1 {
            return Err(format!("multiple profile rows found for pet: {pet_id}"));
        }
        let Some((
            display_name,
            lifecycle,
            completed_at,
            pet_updated_at,
            profile_pet_id,
            schema_version,
            gender,
            birth_date,
            profile_updated_at,
        )) = rows.into_iter().next()
        else {
            tx.commit().map_err(|error| error.to_string())?;
            return Ok(None);
        };
        if lifecycle != "ready" || completed_at.is_none() {
            return Err(format!(
                "pet profile is unavailable until completion: {pet_id}"
            ));
        }
        let display_name =
            display_name.ok_or_else(|| format!("completed pet has no name: {pet_id}"))?;

        let (schema_version, gender, birth_date, updated_at) =
            if let Some(profile_pet_id) = profile_pet_id {
                if profile_pet_id != pet_id {
                    return Err(format!("profile row belongs to a different pet: {pet_id}"));
                }
                if schema_version != Some(1) {
                    return Err(format!("invalid pet profile schema version: {pet_id}"));
                }
                if let Some(value) = birth_date.as_deref() {
                    validate_birth_date(value)?;
                }
                (
                    1,
                    parse_gender_code(gender.as_deref())?,
                    birth_date,
                    profile_updated_at
                        .ok_or_else(|| "pet profile is missing updated_at".to_string())?,
                )
            } else {
                tx.execute(
                    "INSERT INTO pet_profiles
                         (pet_id, schema_version, gender_code, birth_date, updated_at)
                     VALUES (?1, 1, NULL, NULL, ?2)
                     ON CONFLICT(pet_id) DO NOTHING",
                    rusqlite::params![pet_id, pet_updated_at],
                )
                .map_err(|error| error.to_string())?;
                (1, PetGender::Unknown, None, pet_updated_at)
            };
        let profile = PetProfile {
            schema_version,
            pet_id: pet_id.into(),
            display_name,
            gender,
            birth_date,
            editable: true,
            updated_at,
        };
        tx.commit().map_err(|error| error.to_string())?;
        Ok(Some(profile))
    }

    pub fn update_profile(
        &self,
        pet_id: &str,
        update: PetProfileUpdate,
    ) -> Result<PetProfile, String> {
        if pet_id == BUILTIN_PET_ID {
            return Err("the built-in pet profile is read-only".into());
        }
        let update = validate_profile_update(update)?;
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let state = tx
            .query_row(
                "SELECT lifecycle, completed_at FROM pets WHERE pet_id = ?1",
                [pet_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("pet not found: {pet_id}"))?;
        if state.0 != "ready" || state.1.is_none() {
            return Err(format!(
                "pet profile is unavailable until completion: {pet_id}"
            ));
        }
        let updated_at = profile_now_iso();

        let affected = tx
            .execute(
                "UPDATE pets SET display_name = ?2, updated_at = ?3
                 WHERE pet_id = ?1 AND lifecycle = 'ready' AND completed_at IS NOT NULL",
                rusqlite::params![pet_id, update.display_name, updated_at],
            )
            .map_err(|error| error.to_string())?;
        if affected != 1 {
            return Err(format!("pet changed while updating profile: {pet_id}"));
        }
        let gender_code = gender_code(update.gender);
        tx.execute(
            "INSERT INTO pet_profiles
                 (pet_id, schema_version, gender_code, birth_date, updated_at)
             VALUES (?1, 1, ?2, ?3, ?4)
             ON CONFLICT(pet_id) DO UPDATE SET
                 schema_version = 1,
                 gender_code = excluded.gender_code,
                 birth_date = excluded.birth_date,
                 updated_at = excluded.updated_at",
            rusqlite::params![pet_id, gender_code, update.birth_date, updated_at],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;

        Ok(PetProfile {
            schema_version: 1,
            pet_id: pet_id.into(),
            display_name: update.display_name,
            gender: update.gender,
            birth_date: update.birth_date,
            editable: true,
            updated_at,
        })
    }

    pub fn delete(&self, pet_id: &str) -> Result<(), String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let affected = db
            .execute(
                "DELETE FROM pets WHERE pet_id = ?1",
                rusqlite::params![pet_id],
            )
            .map_err(|error| error.to_string())?;
        if affected == 0 {
            return Err(format!("pet not found: {pet_id}"));
        }
        Ok(())
    }
}

fn reserve_on_connection(
    db: &rusqlite::Connection,
    method: CreationMethod,
    source_template: Option<(&str, u32)>,
) -> Result<Pet, String> {
    match (method, source_template) {
        (CreationMethod::Adoption, None) => {
            return Err("adoption creation requires a source template".into());
        }
        (CreationMethod::Upload | CreationMethod::Composer, Some(_)) => {
            return Err("only adoption creation accepts a source template".into());
        }
        _ => {}
    }
    let mode = match method {
        CreationMethod::Upload => IdentityMode::RealPet,
        CreationMethod::Composer => IdentityMode::Guided,
        CreationMethod::Adoption => IdentityMode::Adopted,
    };
    insert_on_connection(db, Species::Cat, mode, method, source_template)
}

fn insert_on_connection(
    db: &rusqlite::Connection,
    species: Species,
    mode: IdentityMode,
    method: CreationMethod,
    source_template: Option<(&str, u32)>,
) -> Result<Pet, String> {
    let pet_id = new_entity_id("pet");
    let now = now_iso();
    let (source_template_id, source_template_version) = source_template
        .map(|(id, version)| (Some(id), Some(version)))
        .unwrap_or((None, None));
    db.execute(
        "INSERT INTO pets
             (pet_id, schema_version, species, identity_mode, display_name, creation_method,
              source_template_id, source_template_version, lifecycle, completed_at,
              created_at, updated_at)
             VALUES (?1, 1, ?2, ?3, NULL, ?4, ?5, ?6, 'draft', NULL, ?7, ?7)",
        rusqlite::params![
            pet_id,
            format!("{species:?}").to_lowercase(),
            format!("{mode:?}").to_lowercase(),
            creation_method_value(method),
            source_template_id,
            source_template_version,
            now,
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(Pet {
        pet_id,
        schema_version: 1,
        species,
        identity_mode: mode,
        display_name: None,
        creation_method: method,
        source_template_id: source_template_id.map(str::to_owned),
        source_template_version,
        lifecycle: "draft".into(),
        completed_at: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

fn parse_species(value: &str) -> Species {
    match value {
        "dog" => Species::Dog,
        _ => Species::Cat,
    }
}

fn parse_mode(value: &str) -> IdentityMode {
    match value {
        "reference" => IdentityMode::Reference,
        "guided" => IdentityMode::Guided,
        "adopted" => IdentityMode::Adopted,
        _ => IdentityMode::RealPet,
    }
}

fn gender_code(gender: PetGender) -> Option<&'static str> {
    match gender {
        PetGender::Unknown => None,
        PetGender::Male => Some("male"),
        PetGender::Female => Some("female"),
    }
}

fn parse_gender_code(value: Option<&str>) -> Result<PetGender, String> {
    match value {
        None => Ok(PetGender::Unknown),
        Some("male") => Ok(PetGender::Male),
        Some("female") => Ok(PetGender::Female),
        Some(value) => Err(format!("unknown pet gender code: {value}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::domain::CreationMethod;
    use crate::pets::active::BUILTIN_PET_ID;
    use crate::pets::pet::{IdentityMode, Species};
    use crate::pets::profile::{PetGender, PetProfileUpdate};
    use crate::storage::Storage;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_repo() -> (PetRepository, std::path::PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-repo-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        (PetRepository::new(storage), root)
    }

    fn ready_user_pet_repo(pet_id: &str) -> (PetRepository, std::path::PathBuf) {
        let (repo, root) = temp_repo();
        repo.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO pets
                 (pet_id, schema_version, species, identity_mode, display_name, creation_method,
                  source_template_id, source_template_version, lifecycle, completed_at,
                  created_at, updated_at)
                 VALUES (?1, 1, 'cat', 'realpet', '旧名字', 'upload', NULL, NULL,
                         'ready', 'old', 'old', 'old')",
                [pet_id],
            )
            .unwrap();
        (repo, root)
    }

    fn profile_update(name: &str, birth_date: Option<&str>) -> PetProfileUpdate {
        PetProfileUpdate {
            display_name: name.into(),
            gender: PetGender::Female,
            birth_date: birth_date.map(str::to_owned),
        }
    }

    #[test]
    fn empty_list_returns_no_pets() {
        let (repo, root) = temp_repo();
        assert!(repo.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn create_then_get_round_trips() {
        let (repo, root) = temp_repo();
        let pet = repo.create(Species::Cat, IdentityMode::RealPet).unwrap();
        let loaded = repo.get(&pet.pet_id).unwrap().unwrap();
        assert_eq!(loaded.pet_id, pet.pet_id);
        assert_eq!(loaded.species, Species::Cat);
        assert_eq!(loaded.identity_mode, IdentityMode::RealPet);
        assert_eq!(loaded.schema_version, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn delete_removes_pet() {
        let (repo, root) = temp_repo();
        let pet = repo.create(Species::Dog, IdentityMode::Adopted).unwrap();
        assert!(repo.get(&pet.pet_id).unwrap().is_some());
        repo.delete(&pet.pet_id).unwrap();
        assert!(repo.get(&pet.pet_id).unwrap().is_none());
        assert!(repo.delete(&pet.pet_id).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reserve_creates_a_cat_draft_with_creation_metadata() {
        let (repo, root) = temp_repo();
        let pet = repo
            .reserve(CreationMethod::Adoption, Some(("template-a", 7)))
            .unwrap();
        assert_eq!(pet.species, Species::Cat);
        assert_eq!(pet.identity_mode, IdentityMode::Adopted);
        assert_eq!(pet.creation_method, CreationMethod::Adoption);
        assert_eq!(pet.source_template_id.as_deref(), Some("template-a"));
        assert_eq!(pet.source_template_version, Some(7));
        assert_eq!(pet.lifecycle, "draft");
        assert_eq!(pet.display_name, None);
        assert_eq!(pet.completed_at, None);

        let loaded = repo.get(&pet.pet_id).unwrap().unwrap();
        assert_eq!(loaded, pet);
        let listed = repo.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].creation_method, CreationMethod::Adoption);
        assert_eq!(listed[0].source_template_id.as_deref(), Some("template-a"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_create_preserves_species_and_identity_while_mapping_method() {
        let (repo, root) = temp_repo();
        let pet = repo.create(Species::Dog, IdentityMode::Guided).unwrap();
        assert_eq!(pet.species, Species::Dog);
        assert_eq!(pet.identity_mode, IdentityMode::Guided);
        assert_eq!(pet.creation_method, CreationMethod::Composer);
        assert_eq!(pet.source_template_id, None);
        assert_eq!(pet.source_template_version, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reserve_rejects_sources_for_non_adoption_methods() {
        let (repo, root) = temp_repo();
        for method in [CreationMethod::Upload, CreationMethod::Composer] {
            assert!(repo.reserve(method, Some(("template-a", 1))).is_err());
        }
        assert!(repo.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reserve_requires_a_source_for_adoption() {
        let (repo, root) = temp_repo();
        assert!(repo.reserve(CreationMethod::Adoption, None).is_err());
        assert!(repo.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn abandoned_adoption_source_can_be_reserved_again() {
        let (repo, root) = temp_repo();
        let abandoned = repo
            .reserve(CreationMethod::Adoption, Some(("template-a", 1)))
            .unwrap();
        repo.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE pets SET lifecycle='abandoned' WHERE pet_id=?1",
                [&abandoned.pet_id],
            )
            .unwrap();

        let replacement = repo
            .reserve(CreationMethod::Adoption, Some(("template-a", 1)))
            .unwrap();
        assert_ne!(replacement.pet_id, abandoned.pet_id);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_adopted_create_remains_source_free() {
        let (repo, root) = temp_repo();
        let pet = repo.create(Species::Dog, IdentityMode::Adopted).unwrap();
        assert_eq!(pet.creation_method, CreationMethod::Adoption);
        assert_eq!(pet.source_template_id, None);
        assert_eq!(pet.source_template_version, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_creation_method_is_an_explicit_parse_error() {
        let error = parse_creation_method("corrupt").unwrap_err();
        assert!(error.contains("unknown creation method"));
    }

    #[test]
    fn profile_update_normalizes_name_and_updates_both_tables() {
        let (repo, root) = ready_user_pet_repo("pet-a");
        let saved = repo
            .update_profile("pet-a", profile_update("  米米  ", Some("2024-02-29")))
            .unwrap();

        assert_eq!(saved.display_name, "米米");
        assert_eq!(saved.gender, PetGender::Female);
        assert_eq!(saved.birth_date.as_deref(), Some("2024-02-29"));
        assert_eq!(
            repo.get("pet-a").unwrap().unwrap().display_name.as_deref(),
            Some("米米")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_invalid_input_leaves_name_unchanged() {
        let (repo, root) = ready_user_pet_repo("pet-a");
        let before = repo.get("pet-a").unwrap().unwrap();

        assert!(repo
            .update_profile("pet-a", profile_update("新名字", Some("2025-02-29")),)
            .is_err());
        assert!(repo
            .update_profile("pet-a", profile_update("   ", None))
            .is_err());
        assert_eq!(repo.get("pet-a").unwrap().unwrap(), before);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_get_handles_missing_draft_and_builtin_pets() {
        let (repo, root) = temp_repo();
        assert!(repo.get_profile("missing").unwrap().is_none());

        let draft = repo.create(Species::Cat, IdentityMode::RealPet).unwrap();
        assert!(repo.get_profile(&draft.pet_id).is_err());
        repo.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE pets SET lifecycle='ready', completed_at=NULL WHERE pet_id=?1",
                [&draft.pet_id],
            )
            .unwrap();
        assert!(repo.get_profile(&draft.pet_id).is_err());

        let builtin = repo.get_profile(BUILTIN_PET_ID).unwrap().unwrap();
        assert_eq!(builtin.pet_id, BUILTIN_PET_ID);
        assert_eq!(builtin.display_name, "默认猫 · Live2D");
        assert_eq!(builtin.gender, PetGender::Unknown);
        assert_eq!(builtin.birth_date, None);
        assert!(!builtin.editable);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_get_backfills_one_default_row_and_is_repeatable() {
        let (repo, root) = ready_user_pet_repo("pet-a");
        let first = repo.get_profile("pet-a").unwrap().unwrap();
        let second = repo.get_profile("pet-a").unwrap().unwrap();

        assert_eq!(first, second);
        assert_eq!(first.display_name, "旧名字");
        assert_eq!(first.gender, PetGender::Unknown);
        assert_eq!(first.birth_date, None);
        assert!(first.editable);
        let rows: i64 = repo
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM pet_profiles WHERE pet_id='pet-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_unknown_gender_round_trips_as_database_null() {
        let (repo, root) = ready_user_pet_repo("pet-a");
        let saved = repo
            .update_profile(
                "pet-a",
                PetProfileUpdate {
                    display_name: "米米".into(),
                    gender: PetGender::Unknown,
                    birth_date: None,
                },
            )
            .unwrap();
        assert_eq!(saved.gender, PetGender::Unknown);
        let gender: Option<String> = repo
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT gender_code FROM pet_profiles WHERE pet_id='pet-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(gender, None);
        assert_eq!(
            repo.get_profile("pet-a").unwrap().unwrap().gender,
            PetGender::Unknown
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_get_rejects_invalid_and_future_persisted_birth_dates() {
        let (repo, root) = ready_user_pet_repo("pet-a");
        let set_corrupt_birth_date = |date: &str| {
            let storage = repo.storage.lock().unwrap();
            storage
                .db
                .pragma_update(None, "ignore_check_constraints", "ON")
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO pet_profiles
                     (pet_id, schema_version, gender_code, birth_date, updated_at)
                     VALUES ('pet-a', 1, NULL, ?1, 'old')
                     ON CONFLICT(pet_id) DO UPDATE SET birth_date=excluded.birth_date",
                    [date],
                )
                .unwrap();
            storage
                .db
                .pragma_update(None, "ignore_check_constraints", "OFF")
                .unwrap();
        };

        set_corrupt_birth_date("2025-02-29");
        assert!(repo.get_profile("pet-a").is_err());
        set_corrupt_birth_date("9999-12-31");
        assert!(repo.get_profile("pet-a").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_get_rejects_duplicate_joined_profile_rows() {
        let (repo, root) = ready_user_pet_repo("pet-a");
        repo.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "DROP TABLE pet_profiles;
                 CREATE TABLE pet_profiles (
                   pet_id TEXT,
                   schema_version INTEGER,
                   gender_code TEXT,
                   birth_date TEXT,
                   updated_at TEXT
                 );
                 INSERT INTO pet_profiles VALUES ('pet-a', 1, NULL, NULL, 'first');
                 INSERT INTO pet_profiles VALUES ('pet-a', 1, 'male', NULL, 'second');",
            )
            .unwrap();

        assert!(repo.get_profile("pet-a").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_get_rejects_null_schema_in_an_existing_profile_row() {
        let (repo, root) = ready_user_pet_repo("pet-a");
        repo.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "DROP TABLE pet_profiles;
                 CREATE TABLE pet_profiles (
                   pet_id TEXT UNIQUE,
                   schema_version INTEGER,
                   gender_code TEXT,
                   birth_date TEXT,
                   updated_at TEXT
                 );
                 INSERT INTO pet_profiles VALUES ('pet-a', NULL, NULL, NULL, 'old');",
            )
            .unwrap();

        assert!(repo.get_profile("pet-a").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_get_rejects_unsupported_profile_schema() {
        let (repo, root) = ready_user_pet_repo("pet-a");
        repo.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "DROP TABLE pet_profiles;
                 CREATE TABLE pet_profiles (
                   pet_id TEXT UNIQUE,
                   schema_version INTEGER,
                   gender_code TEXT,
                   birth_date TEXT,
                   updated_at TEXT
                 );
                 INSERT INTO pet_profiles VALUES ('pet-a', 2, NULL, NULL, 'old');",
            )
            .unwrap();

        assert!(repo.get_profile("pet-a").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_update_rejects_missing_draft_and_builtin_pets() {
        let (repo, root) = temp_repo();
        let draft = repo.create(Species::Cat, IdentityMode::RealPet).unwrap();
        let update = || profile_update("米米", None);

        assert!(repo.update_profile("missing", update()).is_err());
        assert!(repo.update_profile(&draft.pet_id, update()).is_err());
        repo.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE pets SET lifecycle='ready', completed_at=NULL WHERE pet_id=?1",
                [&draft.pet_id],
            )
            .unwrap();
        assert!(repo.update_profile(&draft.pet_id, update()).is_err());
        assert!(repo.update_profile(BUILTIN_PET_ID, update()).is_err());
        assert_eq!(repo.get(&draft.pet_id).unwrap().unwrap().display_name, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_sql_failure_rolls_back_pet_name_and_timestamp() {
        let (repo, root) = ready_user_pet_repo("pet-a");
        repo.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "CREATE TRIGGER reject_profile_insert
                 BEFORE INSERT ON pet_profiles
                 BEGIN SELECT RAISE(ABORT, 'profile write rejected'); END;",
            )
            .unwrap();
        let before = repo.get("pet-a").unwrap().unwrap();

        assert!(repo
            .update_profile("pet-a", profile_update("新名字", None))
            .is_err());
        assert_eq!(repo.get("pet-a").unwrap().unwrap(), before);
        let rows: i64 = repo
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM pet_profiles WHERE pet_id='pet-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_update_failure_on_existing_profile_rolls_back_both_tables() {
        let (repo, root) = ready_user_pet_repo("pet-a");
        repo.update_profile("pet-a", profile_update("初始名字", None))
            .unwrap();
        repo.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "CREATE TRIGGER reject_profile_update
                 BEFORE UPDATE ON pet_profiles
                 BEGIN SELECT RAISE(ABORT, 'profile update rejected'); END;",
            )
            .unwrap();
        let before_pet = repo.get("pet-a").unwrap().unwrap();
        let before_profile = repo.get_profile("pet-a").unwrap().unwrap();

        assert!(repo
            .update_profile("pet-a", profile_update("新名字", Some("2024-02-29")))
            .is_err());
        assert_eq!(repo.get("pet-a").unwrap().unwrap(), before_pet);
        assert_eq!(repo.get_profile("pet-a").unwrap().unwrap(), before_profile);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn builtin_profile_stays_read_only_even_if_a_database_row_uses_its_id() {
        let (repo, root) = ready_user_pet_repo(BUILTIN_PET_ID);
        let before = repo.get(BUILTIN_PET_ID).unwrap().unwrap();

        assert!(repo
            .update_profile(BUILTIN_PET_ID, profile_update("数据库名字", None))
            .is_err());
        assert_eq!(repo.get(BUILTIN_PET_ID).unwrap().unwrap(), before);
        let projected = repo.get_profile(BUILTIN_PET_ID).unwrap().unwrap();
        assert!(!projected.editable);
        assert_ne!(projected.display_name, before.display_name.unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_update_timestamps_after_waiting_for_the_storage_lock() {
        use std::sync::Barrier;
        use std::time::Duration;

        let (repo, root) = ready_user_pet_repo("pet-a");
        let repo = Arc::new(repo);
        let storage_guard = repo.storage.lock().unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let worker_repo = Arc::clone(&repo);
        let worker_barrier = Arc::clone(&barrier);
        let worker = std::thread::spawn(move || {
            worker_barrier.wait();
            worker_repo.update_profile("pet-a", profile_update("米米", None))
        });
        barrier.wait();
        std::thread::sleep(Duration::from_millis(25));
        let before_unlock = profile_now_iso().parse::<u128>().unwrap();
        drop(storage_guard);

        let saved = worker.join().unwrap().unwrap();
        assert!(saved.updated_at.parse::<u128>().unwrap() > before_unlock);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_update_uses_one_new_timestamp_for_both_tables() {
        let (repo, root) = ready_user_pet_repo("pet-a");
        let saved = repo
            .update_profile("pet-a", profile_update("米米", None))
            .unwrap();
        let (pet_updated_at, profile_updated_at): (String, String) = repo
            .storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT pets.updated_at, pet_profiles.updated_at
                 FROM pets JOIN pet_profiles USING (pet_id)
                 WHERE pet_id='pet-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_ne!(pet_updated_at, "old");
        assert_eq!(pet_updated_at, profile_updated_at);
        assert_eq!(saved.updated_at, pet_updated_at);
        let _ = std::fs::remove_dir_all(root);
    }
}

fn creation_method_value(method: CreationMethod) -> &'static str {
    match method {
        CreationMethod::Upload => "upload",
        CreationMethod::Composer => "composer",
        CreationMethod::Adoption => "adoption",
    }
}

fn method_for_identity_mode(mode: IdentityMode) -> CreationMethod {
    match mode {
        IdentityMode::Guided => CreationMethod::Composer,
        IdentityMode::Adopted => CreationMethod::Adoption,
        IdentityMode::RealPet | IdentityMode::Reference => CreationMethod::Upload,
    }
}

fn parse_creation_method(value: &str) -> Result<CreationMethod, String> {
    match value {
        "upload" => Ok(CreationMethod::Upload),
        "composer" => Ok(CreationMethod::Composer),
        "adoption" => Ok(CreationMethod::Adoption),
        _ => Err(format!("unknown creation method: {value}")),
    }
}

fn parse_creation_method_column(value: &str, column: usize) -> rusqlite::Result<CreationMethod> {
    parse_creation_method(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
        )
    })
}
