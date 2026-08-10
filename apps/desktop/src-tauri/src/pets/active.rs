use crate::pets::mutation::{MutationKind, SharedPetMutationGate};
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

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CommitReconciliationStatus {
    NotCommitted,
    Compensated,
    Unknown,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommitReconciliation {
    pub status: CommitReconciliationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

pub type SharedActivePetService = Arc<ActivePetService>;

pub struct ActivePetService {
    storage: Arc<Mutex<Storage>>,
    session: SharedActivePetSession,
    pets_dir: PathBuf,
    mutation_gate: SharedPetMutationGate,
    #[cfg(test)]
    after_owner_pin_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl ActivePetService {
    pub fn new(
        storage: Arc<Mutex<Storage>>,
        session: SharedActivePetSession,
        pets_dir: PathBuf,
        mutation_gate: SharedPetMutationGate,
    ) -> Self {
        Self {
            storage,
            session,
            pets_dir,
            mutation_gate,
            #[cfg(test)]
            after_owner_pin_hook: Mutex::new(None),
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

    pub fn prepare(
        &self,
        request_id: Option<&str>,
        pet_id: &str,
    ) -> Result<RuntimePetDescriptor, String> {
        if let Some(request_id) = request_id {
            self.mutation_gate
                .begin(request_id, MutationKind::Switch, pet_id)?;
        }
        self.describe(pet_id)
    }

    fn describe(&self, pet_id: &str) -> Result<RuntimePetDescriptor, String> {
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

    pub fn commit(
        &self,
        request_id: Option<&str>,
        pet_id: &str,
        accepted_variant_id: Option<&str>,
    ) -> Result<(), String> {
        let _owner_pin = request_id
            .map(|request_id| {
                self.mutation_gate
                    .assert_owner(request_id, MutationKind::Switch, pet_id)
            })
            .transpose()?;
        #[cfg(test)]
        self.run_after_owner_pin_hook();
        self.describe(pet_id)?;
        let mut storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let tx = storage
            .db
            .transaction()
            .map_err(|error| error.to_string())?;
        if pet_id != BUILTIN_PET_ID {
            let target_exists = tx
                .query_row(
                    "SELECT 1 FROM pets WHERE pet_id = ?1",
                    rusqlite::params![pet_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|error| error.to_string())?
                .is_some();
            if !target_exists {
                return Err("installed pet is unavailable".into());
            }
        }
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
        request_id: Option<&str>,
        previous_pet_id: &str,
        pet_id: &str,
        accepted_variant_id: Option<&str>,
    ) -> Result<(), String> {
        let _owner_pin = request_id
            .map(|request_id| {
                self.mutation_gate
                    .assert_owner(request_id, MutationKind::Switch, pet_id)
            })
            .transpose()?;
        #[cfg(test)]
        self.run_after_owner_pin_hook();
        self.describe(previous_pet_id)?;
        self.rollback_database(previous_pet_id, pet_id, accepted_variant_id)?;
        self.sync_session(previous_pet_id)
    }

    pub fn reconcile_commit(
        &self,
        request_id: &str,
        previous_pet_id: &str,
        pet_id: &str,
        accepted_variant_id: Option<&str>,
    ) -> Result<CommitReconciliation, String> {
        let _owner_pin =
            self.mutation_gate
                .assert_owner(request_id, MutationKind::Switch, pet_id)?;
        #[cfg(test)]
        self.run_after_owner_pin_hook();

        if previous_pet_id == pet_id {
            return Ok(CommitReconciliation {
                status: CommitReconciliationStatus::Unknown,
                warning: Some("previous and target pet are indistinguishable".into()),
            });
        }
        let current = match self.read_persisted_active() {
            Ok(current) => current,
            Err(error) => {
                return Ok(CommitReconciliation {
                    status: CommitReconciliationStatus::Unknown,
                    warning: Some(error),
                });
            }
        };
        if current.as_deref() == Some(previous_pet_id) {
            return Ok(CommitReconciliation {
                status: CommitReconciliationStatus::NotCommitted,
                warning: None,
            });
        }
        if current.as_deref() != Some(pet_id) {
            return Ok(CommitReconciliation {
                status: CommitReconciliationStatus::Unknown,
                warning: Some("persisted active pet is neither previous nor target".into()),
            });
        }
        if let Err(error) = self
            .describe(previous_pet_id)
            .and_then(|_| self.rollback_database(previous_pet_id, pet_id, accepted_variant_id))
        {
            return Ok(CommitReconciliation {
                status: CommitReconciliationStatus::Unknown,
                warning: Some(error),
            });
        }

        Ok(CommitReconciliation {
            status: CommitReconciliationStatus::Compensated,
            warning: self.sync_session(previous_pet_id).err(),
        })
    }

    fn rollback_database(
        &self,
        previous_pet_id: &str,
        pet_id: &str,
        accepted_variant_id: Option<&str>,
    ) -> Result<(), String> {
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
        Ok(())
    }

    fn sync_session(&self, pet_id: &str) -> Result<(), String> {
        self.session
            .lock()
            .map_err(|_| "session lock poisoned")?
            .set_active(pet_id.into())
    }

    pub fn cancel(&self, request_id: &str) -> Result<(), String> {
        self.mutation_gate.finish(request_id)
    }

    pub fn finish(&self, request_id: &str) -> Result<(), String> {
        self.mutation_gate.finish(request_id)
    }

    #[cfg(test)]
    fn set_after_owner_pin_hook(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self.after_owner_pin_hook.lock().unwrap() = Some(Arc::new(hook));
    }

    #[cfg(test)]
    fn run_after_owner_pin_hook(&self) {
        if let Some(hook) = self.after_owner_pin_hook.lock().unwrap().clone() {
            hook();
        }
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
    use crate::pets::mutation::{MutationKind, PetMutationGate, SharedPetMutationGate};
    use crate::pets::{ActivePetSession, SharedActivePetSession};
    use crate::runtime_assets::{
        importer::import_png_source,
        migration::{migrate_v1_pet_assets, MigrationOutcome},
    };
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
        gate: SharedPetMutationGate,
    }

    impl ActiveHarness {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let root =
                std::env::temp_dir().join(format!("desktop-pet-active-{}-{n}", std::process::id()));
            let pets_dir = root.join("pets");
            let storage = Arc::new(Mutex::new(Storage::open(&pets_dir).unwrap()));
            let session = Arc::new(Mutex::new(ActivePetSession::new()));
            let gate = Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60)));
            let service = ActivePetService::new(
                storage.clone(),
                session.clone(),
                pets_dir.clone(),
                gate.clone(),
            );
            Self {
                root,
                pets_dir,
                storage,
                session,
                service,
                gate,
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

        fn with_current_pet(pet_id: &str, variant_id: &str) -> Self {
            let test = Self::with_healthy_pet(pet_id, variant_id);
            assert_eq!(
                migrate_v1_pet_assets(&test.pets_dir.join(pet_id).join("assets")).unwrap(),
                MigrationOutcome::Migrated
            );
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
        image::RgbaImage::from_pixel(width, height, image::Rgba([80, 90, 100, 255]))
            .save(path)
            .unwrap();
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
    fn restore_does_not_activate_an_unmigrated_v1_pet() {
        let test = ActiveHarness::with_healthy_pet("pet-user", "variant-1");
        test.save_active("pet-user");
        assert_eq!(test.service.restore().unwrap(), BUILTIN_PET_ID);
        assert_eq!(test.session_active().as_deref(), Some(BUILTIN_PET_ID));
    }

    #[test]
    fn restores_a_current_installed_pet() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.save_active("pet-user");
        assert_eq!(test.service.restore().unwrap(), "pet-user");
        assert_eq!(test.session_active().as_deref(), Some("pet-user"));
    }

    #[test]
    fn prepare_describes_builtin_and_healthy_installed_pet() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        assert_eq!(
            test.service.prepare(None, BUILTIN_PET_ID).unwrap(),
            RuntimePetDescriptor {
                pet_id: BUILTIN_PET_ID.into(),
                source: RuntimePetSource::Builtin,
            }
        );
        assert_eq!(
            test.service.prepare(None, "pet-user").unwrap().source,
            RuntimePetSource::Installed
        );
    }

    #[test]
    fn creation_commit_accepts_variant_and_persists_active_atomically() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.service
            .commit(None, "pet-user", Some("variant-1"))
            .unwrap();
        assert_eq!(test.persisted_active().as_deref(), Some("pet-user"));
        assert!(test.variant_accepted("variant-1"));
        assert_eq!(test.service.active().unwrap(), "pet-user");
    }

    #[test]
    fn rollback_commit_restores_previous_selection_and_unaccepts_the_candidate() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.service
            .commit(None, "pet-user", Some("variant-1"))
            .unwrap();

        test.service
            .rollback_commit(None, BUILTIN_PET_ID, "pet-user", Some("variant-1"))
            .unwrap();
        test.service
            .rollback_commit(None, BUILTIN_PET_ID, "pet-user", Some("variant-1"))
            .unwrap();

        assert_eq!(test.persisted_active().as_deref(), Some(BUILTIN_PET_ID));
        assert!(!test.variant_accepted("variant-1"));
        assert_eq!(test.service.active().unwrap(), BUILTIN_PET_ID);
    }

    #[test]
    fn rollback_commit_without_a_variant_is_idempotent_after_the_previous_selection_is_restored() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.service.commit(None, "pet-user", None).unwrap();

        test.service
            .rollback_commit(None, BUILTIN_PET_ID, "pet-user", None)
            .unwrap();
        test.service
            .rollback_commit(None, BUILTIN_PET_ID, "pet-user", None)
            .unwrap();

        assert_eq!(test.persisted_active().as_deref(), Some(BUILTIN_PET_ID));
        assert_eq!(test.session_active().as_deref(), Some(BUILTIN_PET_ID));
    }

    #[test]
    fn commit_rejects_an_already_accepted_candidate() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.service
            .commit(None, "pet-user", Some("variant-1"))
            .unwrap();

        assert!(test
            .service
            .commit(None, "pet-user", Some("variant-1"))
            .is_err());
        assert!(test.variant_accepted("variant-1"));
    }

    #[test]
    fn rollback_commit_rejects_a_stale_active_selection_without_unaccepting_the_variant() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.service
            .commit(None, "pet-user", Some("variant-1"))
            .unwrap();
        test.service.commit(None, BUILTIN_PET_ID, None).unwrap();

        assert!(test
            .service
            .rollback_commit(None, BUILTIN_PET_ID, "pet-user", Some("variant-1"))
            .is_err());
        assert_eq!(test.persisted_active().as_deref(), Some(BUILTIN_PET_ID));
        assert!(test.variant_accepted("variant-1"));
    }

    #[test]
    fn switch_request_holds_the_shared_gate_until_cancel() {
        let test = ActiveHarness::new();
        let gate = Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60)));
        let service = ActivePetService::new(
            test.storage.clone(),
            test.session.clone(),
            test.pets_dir.clone(),
            gate.clone(),
        );

        service.prepare(Some("switch-1"), BUILTIN_PET_ID).unwrap();
        assert!(gate
            .begin("delete-1", MutationKind::Delete, "pet-user")
            .is_err());
        service.cancel("switch-1").unwrap();
        assert!(gate
            .begin("delete-1", MutationKind::Delete, "pet-user")
            .is_ok());
        gate.finish("delete-1").unwrap();
    }

    #[test]
    fn switch_commit_requires_the_matching_gate_owner() {
        let test = ActiveHarness::new();
        let gate = Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60)));
        let service = ActivePetService::new(
            test.storage.clone(),
            test.session.clone(),
            test.pets_dir.clone(),
            gate,
        );

        assert!(service
            .commit(Some("missing-request"), BUILTIN_PET_ID, None)
            .is_err());
    }

    #[test]
    fn commit_pin_blocks_cancel_and_other_mutations_until_commit_returns() {
        let test = ActiveHarness::new();
        test.service
            .prepare(Some("switch-pinned"), BUILTIN_PET_ID)
            .unwrap();
        let commit_entered = Arc::new(std::sync::Barrier::new(2));
        let commit_release = Arc::new(std::sync::Barrier::new(2));
        test.service.set_after_owner_pin_hook({
            let entered = commit_entered.clone();
            let release = commit_release.clone();
            move || {
                entered.wait();
                release.wait();
            }
        });

        std::thread::scope(|scope| {
            let commit = scope.spawn(|| {
                test.service
                    .commit(Some("switch-pinned"), BUILTIN_PET_ID, None)
            });
            commit_entered.wait();

            assert!(test.service.cancel("switch-pinned").is_err());
            assert!(test
                .gate
                .begin("creation-blocked", MutationKind::Creation, "pet-a")
                .is_err());

            commit_release.wait();
            assert!(commit.join().unwrap().is_ok());
        });

        test.service.finish("switch-pinned").unwrap();
        assert!(test
            .gate
            .begin("creation-after", MutationKind::Creation, "pet-a")
            .is_ok());
    }

    #[test]
    fn rollback_pin_blocks_cancel_until_session_sync_returns() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.save_active(BUILTIN_PET_ID);
        test.service
            .prepare(Some("switch-rollback-pinned"), "pet-user")
            .unwrap();
        test.service
            .commit(Some("switch-rollback-pinned"), "pet-user", None)
            .unwrap();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        test.service.set_after_owner_pin_hook({
            let entered = entered.clone();
            let release = release.clone();
            move || {
                entered.wait();
                release.wait();
            }
        });

        std::thread::scope(|scope| {
            let rollback = scope.spawn(|| {
                test.service.rollback_commit(
                    Some("switch-rollback-pinned"),
                    BUILTIN_PET_ID,
                    "pet-user",
                    None,
                )
            });
            entered.wait();
            assert!(test.service.cancel("switch-rollback-pinned").is_err());
            release.wait();
            assert!(rollback.join().unwrap().is_ok());
        });
        test.service.finish("switch-rollback-pinned").unwrap();
    }

    #[test]
    fn reconcile_pin_blocks_cancel_until_db_compensation_is_classified() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.save_active(BUILTIN_PET_ID);
        test.service
            .prepare(Some("switch-reconcile-pinned"), "pet-user")
            .unwrap();
        test.service
            .commit(Some("switch-reconcile-pinned"), "pet-user", None)
            .unwrap();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        test.service.set_after_owner_pin_hook({
            let entered = entered.clone();
            let release = release.clone();
            move || {
                entered.wait();
                release.wait();
            }
        });

        std::thread::scope(|scope| {
            let reconcile = scope.spawn(|| {
                test.service.reconcile_commit(
                    "switch-reconcile-pinned",
                    BUILTIN_PET_ID,
                    "pet-user",
                    None,
                )
            });
            entered.wait();
            assert!(test.service.cancel("switch-reconcile-pinned").is_err());
            release.wait();
            assert_eq!(
                reconcile.join().unwrap().unwrap().status,
                CommitReconciliationStatus::Compensated
            );
        });
        test.service.finish("switch-reconcile-pinned").unwrap();
    }

    #[test]
    fn reconcile_reports_not_committed_when_persistence_is_still_previous() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.save_active(BUILTIN_PET_ID);
        test.service
            .prepare(Some("switch-not-committed"), "pet-user")
            .unwrap();

        let reconciliation = test
            .service
            .reconcile_commit(
                "switch-not-committed",
                BUILTIN_PET_ID,
                "pet-user",
                Some("variant-1"),
            )
            .unwrap();

        assert_eq!(
            reconciliation.status,
            CommitReconciliationStatus::NotCommitted
        );
        assert_eq!(reconciliation.warning, None);
        test.service.cancel("switch-not-committed").unwrap();
    }

    #[test]
    fn reconcile_compensates_a_commit_that_succeeded_before_transport_rejected() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.save_active(BUILTIN_PET_ID);
        test.service
            .prepare(Some("switch-committed"), "pet-user")
            .unwrap();
        test.service
            .commit(Some("switch-committed"), "pet-user", Some("variant-1"))
            .unwrap();

        let reconciliation = test
            .service
            .reconcile_commit(
                "switch-committed",
                BUILTIN_PET_ID,
                "pet-user",
                Some("variant-1"),
            )
            .unwrap();

        assert_eq!(
            reconciliation.status,
            CommitReconciliationStatus::Compensated
        );
        assert_eq!(test.persisted_active().as_deref(), Some(BUILTIN_PET_ID));
        assert!(!test.variant_accepted("variant-1"));
        test.service.finish("switch-committed").unwrap();
    }

    #[test]
    fn reconcile_reports_db_compensated_when_session_sync_is_poisoned() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.save_active(BUILTIN_PET_ID);
        test.service
            .prepare(Some("switch-poisoned-session"), "pet-user")
            .unwrap();
        test.service
            .commit(
                Some("switch-poisoned-session"),
                "pet-user",
                Some("variant-1"),
            )
            .unwrap();
        let session = test.session.clone();
        assert!(std::thread::spawn(move || {
            let _session = session.lock().unwrap();
            panic!("poison active session");
        })
        .join()
        .is_err());

        let reconciliation = test
            .service
            .reconcile_commit(
                "switch-poisoned-session",
                BUILTIN_PET_ID,
                "pet-user",
                Some("variant-1"),
            )
            .unwrap();

        assert_eq!(
            reconciliation.status,
            CommitReconciliationStatus::Compensated
        );
        assert!(reconciliation.warning.is_some());
        assert_eq!(test.persisted_active().as_deref(), Some(BUILTIN_PET_ID));
        assert!(!test.variant_accepted("variant-1"));
        test.service.finish("switch-poisoned-session").unwrap();
    }

    #[test]
    fn reconcile_reports_unknown_for_an_unrelated_persisted_owner() {
        let test = ActiveHarness::with_current_pet("pet-user", "variant-1");
        test.save_active("pet-unrelated");
        test.service
            .prepare(Some("switch-unknown"), "pet-user")
            .unwrap();

        let reconciliation = test
            .service
            .reconcile_commit(
                "switch-unknown",
                BUILTIN_PET_ID,
                "pet-user",
                Some("variant-1"),
            )
            .unwrap();

        assert_eq!(reconciliation.status, CommitReconciliationStatus::Unknown);
        assert!(reconciliation.warning.is_some());
        test.service.finish("switch-unknown").unwrap();
    }

    #[test]
    fn reconciliation_dto_uses_protocol_status_and_omits_empty_warning() {
        let value = serde_json::to_value(CommitReconciliation {
            status: CommitReconciliationStatus::NotCommitted,
            warning: None,
        })
        .unwrap();

        assert_eq!(value["status"], "notCommitted");
        assert!(value.get("warning").is_none());
    }
}
