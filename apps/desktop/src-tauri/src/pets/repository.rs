use crate::creation::domain::{new_entity_id, CreationMethod};
use crate::pets::pet::{IdentityMode, Pet, PetSummary, Species};
use crate::storage::Storage;
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
        self.insert(Species::Cat, mode, method, source_template)
    }

    fn insert(
        &self,
        species: Species,
        mode: IdentityMode,
        method: CreationMethod,
        source_template: Option<(&str, u32)>,
    ) -> Result<Pet, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::domain::CreationMethod;
    use crate::pets::pet::{IdentityMode, Species};
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
