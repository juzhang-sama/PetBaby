use crate::pets::active::{SharedActivePetService, BUILTIN_PET_ID};
use crate::runtime_assets::{
    loader::inspect_pet_asset,
    manifest::{parse_manifest, RuntimeAssetManifest},
};
use crate::storage::Storage;
use rusqlite::OptionalExtension;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PetLifecycle {
    Ready,
    Generating,
    GenerationFailed,
    AwaitingConfirm,
    CompileRetryable,
    AwaitingActivation,
    Corrupt,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetCatalogEntry {
    pub pet_id: String,
    pub source: String,
    pub species: String,
    pub identity_mode: String,
    pub created_at: Option<String>,
    pub is_current: bool,
    pub deletable: bool,
    pub status: PetLifecycle,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreationResume {
    pub pet_id: String,
    pub status: PetLifecycle,
    pub job_id: Option<String>,
    pub variant_id: Option<String>,
    pub error: Option<String>,
}

pub type SharedPetCatalogService = Arc<PetCatalogService>;

pub struct PetCatalogService {
    storage: Arc<Mutex<Storage>>,
    active: SharedActivePetService,
    pets_dir: PathBuf,
}

#[derive(Default)]
struct PetFacts {
    latest_job_status: Option<String>,
    has_candidate: bool,
    has_runtime_variant: bool,
    accepted: bool,
    asset_healthy: bool,
    legacy_healthy_asset: bool,
    compile_error: Option<String>,
}

struct LatestJob {
    job_id: String,
    status: String,
    error: Option<String>,
}

struct ManifestIdentity {
    pet_id: String,
    variant_id: String,
}

impl PetCatalogService {
    pub fn new(
        storage: Arc<Mutex<Storage>>,
        active: SharedActivePetService,
        pets_dir: PathBuf,
    ) -> Self {
        Self {
            storage,
            active,
            pets_dir,
        }
    }

    pub fn list(&self) -> Result<Vec<PetCatalogEntry>, String> {
        let active_pet_id = self.active.active()?;
        let pets = self.pets()?;
        let mut entries = Vec::with_capacity(pets.len() + 1);
        entries.push(PetCatalogEntry {
            pet_id: BUILTIN_PET_ID.into(),
            source: "builtin".into(),
            species: "cat".into(),
            identity_mode: "builtin".into(),
            created_at: None,
            is_current: active_pet_id == BUILTIN_PET_ID,
            deletable: false,
            status: PetLifecycle::Ready,
            issue: None,
        });
        for (pet_id, species, identity_mode, created_at) in pets {
            let facts = self.facts_for_pet(&pet_id)?;
            let status = project(&facts);
            let is_current = active_pet_id == pet_id;
            entries.push(PetCatalogEntry {
                pet_id,
                source: "installed".into(),
                species,
                identity_mode,
                created_at: Some(created_at),
                is_current,
                deletable: true,
                issue: issue_for(&facts, status, None),
                status,
            });
        }
        Ok(entries)
    }

    pub fn creation_resume(&self, pet_id: &str) -> Result<CreationResume, String> {
        if pet_id == BUILTIN_PET_ID {
            return Ok(CreationResume {
                pet_id: pet_id.into(),
                status: PetLifecycle::Ready,
                job_id: None,
                variant_id: None,
                error: None,
            });
        }
        if !self.pet_exists(pet_id)? {
            return Err(format!("pet not found: {pet_id}"));
        }
        let Some(job) = self.latest_job(pet_id)? else {
            let facts = self.facts_for_pet(pet_id)?;
            let status = project(&facts);
            return Ok(CreationResume {
                pet_id: pet_id.into(),
                status,
                job_id: None,
                variant_id: None,
                error: issue_for(&facts, status, None),
            });
        };

        let facts = self.facts_for_job(pet_id, &job)?;
        let status = project(&facts);
        if status == PetLifecycle::Ready {
            return Ok(CreationResume {
                pet_id: pet_id.into(),
                status,
                job_id: None,
                variant_id: None,
                error: None,
            });
        }
        Ok(CreationResume {
            pet_id: pet_id.into(),
            status,
            job_id: Some(job.job_id.clone()),
            variant_id: facts.has_candidate.then_some(job.job_id),
            error: issue_for(&facts, status, job.error),
        })
    }

    fn pets(&self) -> Result<Vec<(String, String, String, String)>, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let mut statement = storage
            .db
            .prepare(
                "SELECT pet_id, species, identity_mode, created_at
                 FROM pets ORDER BY created_at, rowid",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|error| error.to_string())?;
        rows.map(|row| row.map_err(|error| error.to_string()))
            .collect()
    }

    fn pet_exists(&self, pet_id: &str) -> Result<bool, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage
            .db
            .query_row(
                "SELECT 1 FROM pets WHERE pet_id = ?1",
                rusqlite::params![pet_id],
                |_| Ok(()),
            )
            .optional()
            .map(|value| value.is_some())
            .map_err(|error| error.to_string())
    }

    fn latest_job(&self, pet_id: &str) -> Result<Option<LatestJob>, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage
            .db
            .query_row(
                "SELECT job_id, status, error FROM generation_jobs
                 WHERE pet_id = ?1 AND status <> 'cancelled'
                 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                rusqlite::params![pet_id],
                |row| {
                    Ok(LatestJob {
                        job_id: row.get(0)?,
                        status: row.get(1)?,
                        error: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    fn facts_for_pet(&self, pet_id: &str) -> Result<PetFacts, String> {
        let latest_job = self.latest_job(pet_id)?;
        let mut facts = match latest_job.as_ref() {
            Some(job) => self.facts_for_job(pet_id, job)?,
            None => PetFacts::default(),
        };
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let runtime: Option<(String, bool)> = storage
            .db
            .query_row(
                "SELECT v.variant_id,
                        EXISTS(SELECT 1 FROM appearance_variants av
                               WHERE av.variant_id = v.variant_id AND av.pet_id = v.pet_id
                               AND av.accepted = 1)
                 FROM variants v WHERE v.pet_id = ?1 ORDER BY v.created_at DESC, v.rowid DESC LIMIT 1",
                rusqlite::params![pet_id],
                |row| Ok((row.get(0)?, row.get::<_, i64>(1)? != 0)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        facts.has_runtime_variant = runtime.is_some();
        facts.accepted = runtime.as_ref().is_some_and(|(_, accepted)| *accepted);
        facts.compile_error = storage
            .db
            .query_row(
                "SELECT value FROM state WHERE key = ?1",
                rusqlite::params![format!("creation:{pet_id}:compile_error")],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        drop(storage);
        let manifest = self.healthy_manifest_identity(pet_id);
        facts.asset_healthy = runtime.as_ref().is_some_and(|(variant_id, _)| {
            manifest.as_ref().is_some_and(|manifest| {
                manifest.pet_id == pet_id && manifest.variant_id == *variant_id
            })
        });
        facts.legacy_healthy_asset = match manifest.as_ref() {
            Some(manifest) if manifest.pet_id == pet_id => {
                self.manifest_variant_has_no_runtime_row(pet_id, &manifest.variant_id)?
            }
            _ => false,
        };
        Ok(facts)
    }

    fn facts_for_job(&self, pet_id: &str, job: &LatestJob) -> Result<PetFacts, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        let has_candidate = storage
            .db
            .query_row(
                "SELECT 1 FROM appearance_variants
                 WHERE pet_id = ?1 AND variant_id = ?2 AND job_id = ?2",
                rusqlite::params![pet_id, job.job_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .is_some();
        let runtime: Option<bool> = storage
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM appearance_variants av
                               WHERE av.variant_id = v.variant_id AND av.pet_id = v.pet_id
                               AND av.accepted = 1)
                 FROM variants v WHERE v.pet_id = ?1 AND v.variant_id = ?2",
                rusqlite::params![pet_id, job.job_id],
                |row| Ok(row.get::<_, i64>(0)? != 0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let compile_error = storage
            .db
            .query_row(
                "SELECT value FROM state WHERE key = ?1",
                rusqlite::params![format!("creation:{pet_id}:compile_error")],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        drop(storage);
        let asset_healthy = runtime.is_some() && {
            self.healthy_manifest_identity(pet_id)
                .is_some_and(|manifest| {
                    manifest.pet_id == pet_id && manifest.variant_id == job.job_id
                })
        };
        Ok(PetFacts {
            latest_job_status: Some(job.status.clone()),
            has_candidate,
            has_runtime_variant: runtime.is_some(),
            accepted: runtime.unwrap_or(false),
            asset_healthy,
            compile_error,
            ..Default::default()
        })
    }

    fn healthy_manifest_identity(&self, pet_id: &str) -> Option<ManifestIdentity> {
        if inspect_pet_asset(&self.pets_dir, pet_id).status != "healthy" {
            return None;
        }
        let manifest_path = self
            .pets_dir
            .join(pet_id)
            .join("assets")
            .join("manifest.json");
        let manifest = std::fs::read_to_string(manifest_path).ok()?;
        let (manifest_pet_id, variant_id) = match parse_manifest(&manifest).ok()? {
            RuntimeAssetManifest::V1(manifest) => (manifest.pet_id, manifest.variant_id),
            RuntimeAssetManifest::V2(manifest) => (manifest.pet_id, manifest.variant_id),
        };
        Some(ManifestIdentity {
            pet_id: manifest_pet_id,
            variant_id,
        })
    }

    fn manifest_variant_has_no_runtime_row(
        &self,
        pet_id: &str,
        variant_id: &str,
    ) -> Result<bool, String> {
        let storage = self.storage.lock().map_err(|_| "storage lock poisoned")?;
        storage
            .db
            .query_row(
                "SELECT 1 FROM variants WHERE pet_id = ?1 AND variant_id = ?2",
                rusqlite::params![pet_id, variant_id],
                |_| Ok(()),
            )
            .optional()
            .map(|value| value.is_none())
            .map_err(|error| error.to_string())
    }
}

fn project(facts: &PetFacts) -> PetLifecycle {
    if facts.has_runtime_variant {
        if !facts.asset_healthy {
            return PetLifecycle::Corrupt;
        }
        return if facts.accepted {
            PetLifecycle::Ready
        } else {
            PetLifecycle::AwaitingActivation
        };
    }
    if facts.legacy_healthy_asset {
        return PetLifecycle::Ready;
    }
    match facts.latest_job_status.as_deref() {
        Some("pending" | "running") => PetLifecycle::Generating,
        Some("failed" | "cancelled") => PetLifecycle::GenerationFailed,
        Some("success") if facts.compile_error.is_some() => PetLifecycle::CompileRetryable,
        Some("success") if facts.has_candidate => PetLifecycle::AwaitingConfirm,
        _ => PetLifecycle::GenerationFailed,
    }
}

fn issue_for(facts: &PetFacts, status: PetLifecycle, job_error: Option<String>) -> Option<String> {
    match status {
        PetLifecycle::CompileRetryable => facts.compile_error.clone(),
        PetLifecycle::GenerationFailed => job_error,
        PetLifecycle::Corrupt => Some("runtime asset is unavailable".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pets::{
        active::{ActivePetService, BUILTIN_PET_ID},
        ActivePetSession,
    };
    use crate::runtime_assets::importer::import_png_source;
    use crate::storage::Storage;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct CatalogHarness {
        root: PathBuf,
        pets_dir: PathBuf,
        storage: Arc<Mutex<Storage>>,
        active: Arc<ActivePetService>,
        service: PetCatalogService,
    }

    impl CatalogHarness {
        fn new(active_pet_id: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let root = std::env::temp_dir()
                .join(format!("desktop-pet-catalog-{}-{n}", std::process::id()));
            let pets_dir = root.join("pets");
            let storage = Arc::new(Mutex::new(Storage::open(&pets_dir).unwrap()));
            let active = Arc::new(ActivePetService::new(
                storage.clone(),
                Arc::new(Mutex::new(ActivePetSession::new())),
                pets_dir.clone(),
            ));
            active.restore().unwrap();
            if active_pet_id != BUILTIN_PET_ID {
                storage
                    .lock()
                    .unwrap()
                    .db
                    .execute(
                        "INSERT INTO state (key, value) VALUES ('app:active_pet_id', ?1)",
                        rusqlite::params![active_pet_id],
                    )
                    .unwrap();
            }
            Self {
                root,
                pets_dir: pets_dir.clone(),
                storage: storage.clone(),
                active: active.clone(),
                service: PetCatalogService::new(storage, active, pets_dir),
            }
        }

        fn insert_pet(&self, pet_id: &str, created_at: &str) {
            self.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO pets
                     (pet_id, schema_version, species, identity_mode, created_at, updated_at)
                     VALUES (?1, 1, 'cat', 'realpet', ?2, ?2)",
                    rusqlite::params![pet_id, created_at],
                )
                .unwrap();
        }

        fn insert_job(
            &self,
            job_id: &str,
            pet_id: &str,
            status: &str,
            error: Option<&str>,
            created_at: &str,
        ) {
            self.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO generation_jobs
                     (job_id, pet_id, prompt, ref_sha256, status, error, created_at)
                     VALUES (?1, ?2, 'prompt', 'hash', ?3, ?4, ?5)",
                    rusqlite::params![job_id, pet_id, status, error, created_at],
                )
                .unwrap();
        }

        fn insert_candidate(&self, variant_id: &str, pet_id: &str, accepted: bool) {
            self.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO appearance_variants
                     (variant_id, pet_id, job_id, image_path, quality, accepted, created_at)
                     VALUES (?1, ?2, ?1, 'image.png', 'good', ?3, ?1)",
                    rusqlite::params![variant_id, pet_id, i64::from(accepted)],
                )
                .unwrap();
        }

        fn insert_runtime_variant(&self, variant_id: &str, pet_id: &str) {
            self.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO variants (variant_id, pet_id, style_id, manifest_path, created_at)
                     VALUES (?1, ?2, 'style', 'assets/manifest.json', ?1)",
                    rusqlite::params![variant_id, pet_id],
                )
                .unwrap();
        }

        fn set_compile_error(&self, pet_id: &str, error: &str) {
            self.storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO state (key, value) VALUES (?1, ?2)",
                    rusqlite::params![format!("creation:{pet_id}:compile_error"), error],
                )
                .unwrap();
        }

        fn write_healthy_asset(&self, pet_id: &str, variant_id: &str) {
            self.write_healthy_asset_with_manifest_identity(pet_id, pet_id, variant_id);
        }

        fn write_healthy_asset_with_manifest_identity(
            &self,
            asset_pet_id: &str,
            manifest_pet_id: &str,
            variant_id: &str,
        ) {
            let source = self.root.join(format!("{asset_pet_id}.png"));
            write_png(&source, 32, 32);
            import_png_source(
                asset_pet_id,
                &source,
                &self.pets_dir.join(asset_pet_id).join("assets"),
            )
            .unwrap();
            let manifest_path = self
                .pets_dir
                .join(asset_pet_id)
                .join("assets")
                .join("manifest.json");
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
            manifest["petId"] = serde_json::Value::String(manifest_pet_id.into());
            manifest["variantId"] = serde_json::Value::String(variant_id.into());
            std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        }

        fn cleanup(self) {
            let root = self.root.clone();
            drop(self);
            let _ = std::fs::remove_dir_all(root);
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

    fn facts(status: &str) -> PetFacts {
        PetFacts {
            latest_job_status: Some(status.into()),
            ..Default::default()
        }
    }

    impl PetFacts {
        fn candidate(mut self) -> Self {
            self.has_candidate = true;
            self
        }

        fn runtime(mut self) -> Self {
            self.has_runtime_variant = true;
            self
        }

        fn accepted(mut self) -> Self {
            self.accepted = true;
            self
        }

        fn healthy(mut self) -> Self {
            self.asset_healthy = true;
            self
        }

        fn compile_error(mut self) -> Self {
            self.compile_error = Some("compile failed".into());
            self
        }
    }

    #[test]
    fn projects_every_creation_lifecycle() {
        assert_eq!(project(&facts("running")), PetLifecycle::Generating);
        assert_eq!(project(&facts("failed")), PetLifecycle::GenerationFailed);
        assert_eq!(
            project(&facts("success").candidate()),
            PetLifecycle::AwaitingConfirm
        );
        assert_eq!(
            project(&facts("success").candidate().compile_error()),
            PetLifecycle::CompileRetryable
        );
        assert_eq!(
            project(&facts("success").candidate().runtime().healthy()),
            PetLifecycle::AwaitingActivation
        );
        assert_eq!(
            project(&facts("success").candidate().runtime().accepted().healthy()),
            PetLifecycle::Ready
        );
        assert_eq!(
            project(&facts("success").candidate().runtime().accepted()),
            PetLifecycle::Corrupt
        );
    }

    #[test]
    fn builtin_is_first_ready_current_and_not_deletable() {
        let test = CatalogHarness::new(BUILTIN_PET_ID);
        let entries = test.service.list().unwrap();
        assert_eq!(entries[0].pet_id, BUILTIN_PET_ID);
        assert_eq!(entries[0].status, PetLifecycle::Ready);
        assert!(entries[0].is_current);
        assert!(!entries[0].deletable);
        test.cleanup();
    }

    #[test]
    fn catalog_projects_durable_creation_facts() {
        let test = CatalogHarness::new(BUILTIN_PET_ID);
        test.insert_pet("pet-1", "1");
        test.insert_job("job-1", "pet-1", "success", None, "1");
        test.insert_candidate("job-1", "pet-1", false);
        test.set_compile_error("pet-1", "compiler unavailable");

        let entry = test
            .service
            .list()
            .unwrap()
            .into_iter()
            .find(|entry| entry.pet_id == "pet-1")
            .unwrap();
        assert_eq!(entry.source, "installed");
        assert_eq!(entry.species, "cat");
        assert_eq!(entry.identity_mode, "realpet");
        assert_eq!(entry.status, PetLifecycle::CompileRetryable);
        assert_eq!(entry.issue.as_deref(), Some("compiler unavailable"));
        test.cleanup();
    }

    #[test]
    fn creation_resume_uses_only_the_latest_unfinished_creation() {
        let test = CatalogHarness::new(BUILTIN_PET_ID);
        test.insert_pet("pet-1", "1");
        test.insert_job("job-old", "pet-1", "failed", Some("old failure"), "1");
        test.insert_job("job-new", "pet-1", "success", None, "2");
        test.insert_candidate("job-new", "pet-1", false);

        let resume = test.service.creation_resume("pet-1").unwrap();
        assert_eq!(resume.status, PetLifecycle::AwaitingConfirm);
        assert_eq!(resume.job_id.as_deref(), Some("job-new"));
        assert_eq!(resume.variant_id.as_deref(), Some("job-new"));
        assert_eq!(resume.error, None);
        test.cleanup();
    }

    #[test]
    fn healthy_legacy_asset_without_a_runtime_row_is_ready() {
        let test = CatalogHarness::new(BUILTIN_PET_ID);
        test.insert_pet("pet-legacy", "1");
        test.write_healthy_asset("pet-legacy", "legacy-variant");

        let entry = test
            .service
            .list()
            .unwrap()
            .into_iter()
            .find(|entry| entry.pet_id == "pet-legacy")
            .unwrap();
        assert_eq!(entry.status, PetLifecycle::Ready);
        assert_eq!(entry.issue, None);
        test.cleanup();
    }

    #[test]
    fn accepted_healthy_runtime_variant_is_current_and_ready() {
        let test = CatalogHarness::new(BUILTIN_PET_ID);
        test.insert_pet("pet-1", "1");
        test.insert_job("job-1", "pet-1", "success", None, "1");
        test.insert_candidate("job-1", "pet-1", false);
        test.insert_runtime_variant("job-1", "pet-1");
        test.write_healthy_asset("pet-1", "job-1");
        test.active.commit("pet-1", Some("job-1")).unwrap();

        let entry = test
            .service
            .list()
            .unwrap()
            .into_iter()
            .find(|entry| entry.pet_id == "pet-1")
            .unwrap();
        assert_eq!(entry.status, PetLifecycle::Ready);
        assert!(entry.is_current);
        assert!(entry.deletable);
        test.cleanup();
    }

    #[test]
    fn runtime_variant_must_match_the_healthy_manifest_variant_id() {
        let test = CatalogHarness::new(BUILTIN_PET_ID);
        test.insert_pet("pet-1", "1");
        test.insert_runtime_variant("runtime-variant", "pet-1");
        test.write_healthy_asset("pet-1", "manifest-variant");

        let entry = test
            .service
            .list()
            .unwrap()
            .into_iter()
            .find(|entry| entry.pet_id == "pet-1")
            .unwrap();
        assert_eq!(entry.status, PetLifecycle::Corrupt);
        test.cleanup();
    }

    #[test]
    fn mismatched_manifest_pet_id_is_not_a_healthy_legacy_asset() {
        let test = CatalogHarness::new(BUILTIN_PET_ID);
        test.insert_pet("pet-1", "1");
        test.write_healthy_asset_with_manifest_identity("pet-1", "other-pet", "legacy-variant");

        let entry = test
            .service
            .list()
            .unwrap()
            .into_iter()
            .find(|entry| entry.pet_id == "pet-1")
            .unwrap();
        assert_ne!(entry.status, PetLifecycle::Ready);
        test.cleanup();
    }

    #[test]
    fn runtime_variant_with_a_wrong_manifest_pet_id_is_corrupt() {
        let test = CatalogHarness::new(BUILTIN_PET_ID);
        test.insert_pet("pet-1", "1");
        test.insert_runtime_variant("runtime-variant", "pet-1");
        test.write_healthy_asset_with_manifest_identity("pet-1", "other-pet", "runtime-variant");

        let entry = test
            .service
            .list()
            .unwrap()
            .into_iter()
            .find(|entry| entry.pet_id == "pet-1")
            .unwrap();
        assert_eq!(entry.status, PetLifecycle::Corrupt);
        test.cleanup();
    }

    #[test]
    fn creation_resume_skips_the_latest_cancelled_job() {
        let test = CatalogHarness::new(BUILTIN_PET_ID);
        test.insert_pet("pet-1", "1");
        test.insert_job("job-running", "pet-1", "running", None, "1");
        test.insert_job("job-cancelled", "pet-1", "cancelled", None, "2");

        let resume = test.service.creation_resume("pet-1").unwrap();
        assert_eq!(resume.status, PetLifecycle::Generating);
        assert_eq!(resume.job_id.as_deref(), Some("job-running"));
        test.cleanup();
    }

    #[test]
    fn pending_job_takes_priority_over_an_older_compile_error() {
        let test = CatalogHarness::new(BUILTIN_PET_ID);
        test.insert_pet("pet-1", "1");
        test.insert_job("job-old", "pet-1", "success", None, "1");
        test.insert_candidate("job-old", "pet-1", false);
        test.set_compile_error("pet-1", "old compiler failure");
        test.insert_job("job-new", "pet-1", "pending", None, "2");

        let entry = test
            .service
            .list()
            .unwrap()
            .into_iter()
            .find(|entry| entry.pet_id == "pet-1")
            .unwrap();
        assert_eq!(entry.status, PetLifecycle::Generating);
        test.cleanup();
    }
}
