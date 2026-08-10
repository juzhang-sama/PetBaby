use crate::pets::SharedActivePetSession;
use crate::runtime_assets::loader::inspect_pet_asset;
use crate::storage::Storage;
use rusqlite::OptionalExtension;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const BUILTIN_PET_ID: &str = "pet-live2d-v1";
const ACTIVE_KEY: &str = "app:active_pet_id";

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimePetSource {
    Builtin,
    Installed,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePetDescriptor {
    pub pet_id: String,
    pub source: RuntimePetSource,
}

pub type SharedActivePetService = Arc<ActivePetService>;

pub struct ActivePetService {
    storage: Arc<Mutex<Storage>>,
    session: SharedActivePetSession,
    pets_dir: PathBuf,
}

impl ActivePetService {
    pub fn new(
        storage: Arc<Mutex<Storage>>,
        session: SharedActivePetSession,
        pets_dir: PathBuf,
    ) -> Self {
        Self {
            storage,
            session,
            pets_dir,
        }
    }

    pub fn restore(&self) -> Result<String, String> {
        let persisted = self.read_persisted_active()?;
        let active_pet_id = match persisted.as_deref() {
            Some(BUILTIN_PET_ID) => BUILTIN_PET_ID.to_owned(),
            Some(pet_id) if self.installed_pet_is_healthy(pet_id)? => pet_id.to_owned(),
            _ => {
                self.save_persisted_active(BUILTIN_PET_ID)?;
                BUILTIN_PET_ID.to_owned()
            }
        };
        self.session
            .lock()
            .map_err(|_| "session lock poisoned")?
            .set_active(active_pet_id.clone())?;
        Ok(active_pet_id)
    }

    pub fn active(&self) -> Result<String, String> {
        self.session
            .lock()
            .map_err(|_| "session lock poisoned")?
            .active()
            .cloned()
            .ok_or_else(|| "active pet has not been restored".into())
    }

    pub fn prepare(&self, pet_id: &str) -> Result<RuntimePetDescriptor, String> {
        if pet_id == BUILTIN_PET_ID {
            return Ok(RuntimePetDescriptor {
                pet_id: pet_id.into(),
                source: RuntimePetSource::Builtin,
            });
        }
        self.require_installed_pet(pet_id)?;
        Ok(RuntimePetDescriptor {
            pet_id: pet_id.into(),
            source: RuntimePetSource::Installed,
        })
    }

    pub fn commit(&self, pet_id: &str, accepted_variant_id: Option<&str>) -> Result<(), String> {
        self.prepare(pet_id)?;
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        if let Some(variant_id) = accepted_variant_id {
            let affected = tx
                .execute(
                    "UPDATE appearance_variants SET accepted = 1
                     WHERE variant_id = ?1 AND pet_id = ?2
                     AND accepted = 0
                     AND EXISTS (SELECT 1 FROM variants v WHERE v.variant_id = ?1 AND v.pet_id = ?2)",
                    rusqlite::params![variant_id, pet_id],
                )
                .map_err(|error| error.to_string())?;
            if affected != 1 {
                return Err("candidate does not belong to pet".into());
            }
        }
        tx.execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![ACTIVE_KEY, pet_id],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        drop(storage);
        self.session
            .lock()
            .map_err(|_| "session lock poisoned")?
            .set_active(pet_id.into())
    }

    pub fn rollback_commit(
        &self,
        previous_pet_id: &str,
        pet_id: &str,
        accepted_variant_id: Option<&str>,
    ) -> Result<(), String> {
        self.prepare(previous_pet_id)?;
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        let current: Option<String> = tx
            .query_row(
                "SELECT value FROM state WHERE key = ?1",
                rusqlite::params![ACTIVE_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let already_rolled_back = current.as_deref() == Some(previous_pet_id);
        if !already_rolled_back && current.as_deref() != Some(pet_id) {
            return Err("active pet changed before switch rollback".into());
        }
        if let Some(variant_id) = accepted_variant_id {
            if already_rolled_back {
                let candidate_is_unaccepted: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM appearance_variants av
                         WHERE av.variant_id = ?1 AND av.pet_id = ?2 AND av.accepted = 0
                         AND EXISTS (SELECT 1 FROM variants v WHERE v.variant_id = ?1 AND v.pet_id = ?2)",
                        rusqlite::params![variant_id, pet_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| error.to_string())?;
                if candidate_is_unaccepted.is_none() {
                    return Err("candidate does not belong to pet".into());
                }
            } else {
                let affected = tx
                    .execute(
                        "UPDATE appearance_variants SET accepted = 0
                         WHERE variant_id = ?1 AND pet_id = ?2 AND accepted = 1",
                        rusqlite::params![variant_id, pet_id],
                    )
                    .map_err(|error| error.to_string())?;
                if affected != 1 {
                    return Err("candidate does not belong to pet".into());
                }
            }
        }
        if !already_rolled_back {
            tx.execute(
                "INSERT INTO state (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![ACTIVE_KEY, previous_pet_id],
            )
            .map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())?;
        drop(storage);
        self.session
            .lock()
            .map_err(|_| "session lock poisoned")?
            .set_active(previous_pet_id.into())
    }

    fn read_persisted_active(&self) -> Result<Option<String>, String> {
        self.storage
            .lock()
            .map_err(|_| "storage lock poisoned")?
            .db
            .query_row(
                "SELECT value FROM state WHERE key = ?1",
                rusqlite::params![ACTIVE_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    fn save_persisted_active(&self, pet_id: &str) -> Result<(), String> {
        self.storage
            .lock()
            .map_err(|_| "storage lock poisoned")?
            .db
            .execute(
                "INSERT INTO state (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![ACTIVE_KEY, pet_id],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn require_installed_pet(&self, pet_id: &str) -> Result<(), String> {
        if self.installed_pet_is_healthy(pet_id)? {
            Ok(())
        } else {
            Err(format!("installed pet is unavailable: {pet_id}"))
        }
    }

    fn installed_pet_is_healthy(&self, pet_id: &str) -> Result<bool, String> {
        let exists = self
            .storage
            .lock()
            .map_err(|_| "storage lock poisoned")?
            .db
            .query_row(
                "SELECT 1 FROM pets WHERE pet_id = ?1",
                rusqlite::params![pet_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .is_some();
        Ok(exists && inspect_pet_asset(&self.pets_dir, pet_id).status == "healthy")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pets::{ActivePetSession, SharedActivePetSession};
    use crate::runtime_assets::importer::import_png_source;
    use crate::storage::Storage;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct ActiveHarness {
        root: PathBuf,
        pets_dir: PathBuf,
        storage: Arc<Mutex<Storage>>,
        session: SharedActivePetSession,
        service: ActivePetService,
    }

    impl ActiveHarness {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let root =
                std::env::temp_dir().join(format!("desktop-pet-active-{}-{n}", std::process::id()));
            let pets_dir = root.join("pets");
            let storage = Arc::new(Mutex::new(Storage::open(&pets_dir).unwrap()));
            let session = Arc::new(Mutex::new(ActivePetSession::new()));
            let service = ActivePetService::new(storage.clone(), session.clone(), pets_dir.clone());
            Self {
                root,
                pets_dir,
                storage,
                session,
                service,
            }
        }

        fn with_healthy_pet(pet_id: &str, variant_id: &str) -> Self {
            let test = Self::new();
            test.insert_pet(pet_id);
            let source = test.root.join("source.png");
            write_png(&source, 64, 64);
            import_png_source(pet_id, &source, &test.pets_dir.join(pet_id).join("assets")).unwrap();
            let storage = test.storage.lock().unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO variants (variant_id, pet_id, style_id, manifest_path, created_at)
                     VALUES (?1, ?2, 'style', 'assets/manifest.json', '0')",
                    rusqlite::params![variant_id, pet_id],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO appearance_variants
                     (variant_id, pet_id, image_path, quality, accepted, created_at)
                     VALUES (?1, ?2, 'source.png', 'good', 0, '0')",
                    rusqlite::params![variant_id, pet_id],
                )
                .unwrap();
            drop(storage);
            test
        }

        fn insert_pet(&self, pet_id: &str) {
            self.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO pets
                     (pet_id, schema_version, species, identity_mode, created_at, updated_at)
                     VALUES (?1, 1, 'cat', 'realpet', '0', '0')",
                    rusqlite::params![pet_id],
                )
                .unwrap();
        }

        fn save_active(&self, pet_id: &str) {
            self.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO state (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    rusqlite::params![ACTIVE_KEY, pet_id],
                )
                .unwrap();
        }

        fn persisted_active(&self) -> Option<String> {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT value FROM state WHERE key = ?1",
                    rusqlite::params![ACTIVE_KEY],
                    |row| row.get(0),
                )
                .ok()
        }

        fn session_active(&self) -> Option<String> {
            self.session.lock().unwrap().active().cloned()
        }

        fn variant_accepted(&self, variant_id: &str) -> bool {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT accepted FROM appearance_variants WHERE variant_id = ?1",
                    rusqlite::params![variant_id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
                == 1
        }
    }

    fn write_png(path: &Path, width: u32, height: u32) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(b"IEND");
        bytes.extend_from_slice(&[0; 4]);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn missing_or_invalid_selection_repairs_to_builtin() {
        let test = ActiveHarness::new();
        assert_eq!(test.service.restore().unwrap(), BUILTIN_PET_ID);
        assert_eq!(test.persisted_active().as_deref(), Some(BUILTIN_PET_ID));
        test.insert_pet("pet-broken");
        test.save_active("pet-broken");
        assert_eq!(test.service.restore().unwrap(), BUILTIN_PET_ID);
        assert_eq!(test.session_active().as_deref(), Some(BUILTIN_PET_ID));
    }

    #[test]
    fn restores_a_healthy_installed_pet() {
        let test = ActiveHarness::with_healthy_pet("pet-user", "variant-1");
        test.save_active("pet-user");
        assert_eq!(test.service.restore().unwrap(), "pet-user");
        assert_eq!(test.session_active().as_deref(), Some("pet-user"));
    }

    #[test]
    fn prepare_describes_builtin_and_healthy_installed_pet() {
        let test = ActiveHarness::with_healthy_pet("pet-user", "variant-1");
        assert_eq!(
            test.service.prepare(BUILTIN_PET_ID).unwrap(),
            RuntimePetDescriptor {
                pet_id: BUILTIN_PET_ID.into(),
                source: RuntimePetSource::Builtin,
            }
        );
        assert_eq!(
            test.service.prepare("pet-user").unwrap().source,
            RuntimePetSource::Installed
        );
    }

    #[test]
    fn creation_commit_accepts_variant_and_persists_active_atomically() {
        let test = ActiveHarness::with_healthy_pet("pet-user", "variant-1");
        test.service.commit("pet-user", Some("variant-1")).unwrap();
        assert_eq!(test.persisted_active().as_deref(), Some("pet-user"));
        assert!(test.variant_accepted("variant-1"));
        assert_eq!(test.service.active().unwrap(), "pet-user");
    }

    #[test]
    fn rollback_commit_restores_previous_selection_and_unaccepts_the_candidate() {
        let test = ActiveHarness::with_healthy_pet("pet-user", "variant-1");
        test.service.commit("pet-user", Some("variant-1")).unwrap();

        test.service
            .rollback_commit(BUILTIN_PET_ID, "pet-user", Some("variant-1"))
            .unwrap();
        test.service
            .rollback_commit(BUILTIN_PET_ID, "pet-user", Some("variant-1"))
            .unwrap();

        assert_eq!(test.persisted_active().as_deref(), Some(BUILTIN_PET_ID));
        assert!(!test.variant_accepted("variant-1"));
        assert_eq!(test.service.active().unwrap(), BUILTIN_PET_ID);
    }

    #[test]
    fn rollback_commit_without_a_variant_is_idempotent_after_the_previous_selection_is_restored() {
        let test = ActiveHarness::with_healthy_pet("pet-user", "variant-1");
        test.service.commit("pet-user", None).unwrap();

        test.service
            .rollback_commit(BUILTIN_PET_ID, "pet-user", None)
            .unwrap();
        test.service
            .rollback_commit(BUILTIN_PET_ID, "pet-user", None)
            .unwrap();

        assert_eq!(test.persisted_active().as_deref(), Some(BUILTIN_PET_ID));
        assert_eq!(test.session_active().as_deref(), Some(BUILTIN_PET_ID));
    }

    #[test]
    fn commit_rejects_an_already_accepted_candidate() {
        let test = ActiveHarness::with_healthy_pet("pet-user", "variant-1");
        test.service.commit("pet-user", Some("variant-1")).unwrap();

        assert!(test.service.commit("pet-user", Some("variant-1")).is_err());
        assert!(test.variant_accepted("variant-1"));
    }

    #[test]
    fn rollback_commit_rejects_a_stale_active_selection_without_unaccepting_the_variant() {
        let test = ActiveHarness::with_healthy_pet("pet-user", "variant-1");
        test.service.commit("pet-user", Some("variant-1")).unwrap();
        test.service.commit(BUILTIN_PET_ID, None).unwrap();

        assert!(test
            .service
            .rollback_commit(BUILTIN_PET_ID, "pet-user", Some("variant-1"))
            .is_err());
        assert_eq!(test.persisted_active().as_deref(), Some(BUILTIN_PET_ID));
        assert!(test.variant_accepted("variant-1"));
    }
}
