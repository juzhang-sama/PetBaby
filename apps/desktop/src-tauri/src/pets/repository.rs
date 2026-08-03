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

fn new_id(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{prefix}-{nanos:x}")
}

impl PetRepository {
    pub fn new(storage: Arc<Mutex<Storage>>) -> Self {
        Self { storage }
    }

    pub fn list(&self) -> Result<Vec<PetSummary>, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let mut statement = db
            .prepare(
                "SELECT pet_id, species, identity_mode, created_at FROM pets ORDER BY created_at",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                let species: String = row.get(1)?;
                let mode: String = row.get(2)?;
                Ok((
                    row.get::<_, String>(0)?,
                    species,
                    mode,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        let mut summaries = Vec::new();
        for row in rows {
            let (pet_id, species, mode, created_at) = row.map_err(|error| error.to_string())?;
            summaries.push(PetSummary {
                pet_id,
                species: parse_species(&species),
                identity_mode: parse_mode(&mode),
                created_at,
            });
        }
        Ok(summaries)
    }

    pub fn create(&self, species: Species, mode: IdentityMode) -> Result<Pet, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let pet_id = new_id("pet");
        let now = now_iso();
        db.execute(
            "INSERT INTO pets (pet_id, schema_version, species, identity_mode, created_at, updated_at)
             VALUES (?1, 1, ?2, ?3, ?4, ?4)",
            rusqlite::params![
                pet_id,
                format!("{species:?}").to_lowercase(),
                format!("{mode:?}").to_lowercase(),
                now
            ],
        )
        .map_err(|error| error.to_string())?;
        Ok(Pet {
            pet_id,
            schema_version: 1,
            species,
            identity_mode: mode,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn get(&self, pet_id: &str) -> Result<Option<Pet>, String> {
        let db = &self.storage.lock().map_err(|_| "storage lock poisoned")?.db;
        let mut statement = db
            .prepare(
                "SELECT pet_id, schema_version, species, identity_mode, created_at, updated_at
                 FROM pets WHERE pet_id = ?1",
            )
            .map_err(|error| error.to_string())?;
        let mut rows = statement
            .query_map(rusqlite::params![pet_id], |row| {
                let species: String = row.get(2)?;
                let mode: String = row.get(3)?;
                Ok(Pet {
                    pet_id: row.get(0)?,
                    schema_version: row.get(1)?,
                    species: parse_species(&species),
                    identity_mode: parse_mode(&mode),
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
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
