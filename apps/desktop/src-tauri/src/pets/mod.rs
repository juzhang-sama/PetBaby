pub mod pet;
pub mod repository;
pub mod state;

use std::sync::{Arc, Mutex};

pub type PetId = String;
pub type SharedPetRepository = Arc<Mutex<repository::PetRepository>>;

const ACTIVE_PET_KEY: &str = "app:active_pet_id";

pub struct ActivePetSession {
    active_pet_id: Option<PetId>,
}

impl ActivePetSession {
    pub fn new() -> Self {
        Self {
            active_pet_id: None,
        }
    }

    pub fn set_active(&mut self, pet_id: PetId) -> Result<(), String> {
        if pet_id.is_empty() {
            return Err("pet_id must not be empty".into());
        }
        self.active_pet_id = Some(pet_id);
        Ok(())
    }

    pub fn active(&self) -> Option<&PetId> {
        self.active_pet_id.as_ref()
    }

    pub fn clear(&mut self) {
        self.active_pet_id = None;
    }

    pub fn persist_to(&self, store: &crate::pets::state::StateStore) -> Result<(), String> {
        let value = self.active_pet_id.clone().unwrap_or_default();
        store.save(ACTIVE_PET_KEY, &value)
    }

    pub fn load_from(store: &crate::pets::state::StateStore) -> Result<Option<PetId>, String> {
        Ok(store
            .load(ACTIVE_PET_KEY)?
            .filter(|value| !value.is_empty()))
    }
}

pub type SharedActivePetSession = Arc<Mutex<ActivePetSession>>;

#[cfg(test)]
mod session_tests {
    use super::*;

    #[test]
    fn starts_inactive_and_tracks_single_active_pet() {
        let mut session = ActivePetSession::new();
        assert_eq!(session.active(), None);
        session.set_active("pet-a".into()).unwrap();
        assert_eq!(session.active(), Some(&"pet-a".to_string()));
        session.set_active("pet-b".into()).unwrap();
        assert_eq!(session.active(), Some(&"pet-b".to_string()));
    }

    #[test]
    fn rejects_empty_pet_id() {
        let mut session = ActivePetSession::new();
        assert!(session.set_active(String::new()).is_err());
    }

    #[test]
    fn clear_resets_active_pet() {
        let mut session = ActivePetSession::new();
        session.set_active("pet-a".into()).unwrap();
        session.clear();
        assert_eq!(session.active(), None);
    }

    #[test]
    fn active_session_persists_and_restores_from_state_store() {
        use crate::pets::state::StateStore;
        use crate::storage::Storage;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::{Arc, Mutex};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("desktop-pet-session-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let store = StateStore::new(Arc::new(Mutex::new(Storage::open(&root).unwrap())));

        let mut session = ActivePetSession::new();
        session.set_active("pet-a".into()).unwrap();
        session.persist_to(&store).unwrap();

        let mut restored = ActivePetSession::new();
        if let Some(id) = ActivePetSession::load_from(&store).unwrap() {
            restored.set_active(id).unwrap();
        }
        assert_eq!(restored.active(), Some(&"pet-a".to_string()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn clearing_active_pet_persists_empty_state() {
        use crate::pets::state::StateStore;
        use crate::storage::Storage;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::{Arc, Mutex};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-session-clear-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = StateStore::new(Arc::new(Mutex::new(Storage::open(&root).unwrap())));

        let mut session = ActivePetSession::new();
        session.set_active("pet-a".into()).unwrap();
        session.persist_to(&store).unwrap();
        session.clear();
        session.persist_to(&store).unwrap();

        assert_eq!(ActivePetSession::load_from(&store).unwrap(), None);
        let _ = std::fs::remove_dir_all(root);
    }
}
