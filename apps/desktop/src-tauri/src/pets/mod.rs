pub mod active;
pub mod pet;
pub mod repository;
pub mod state;

use std::sync::{Arc, Mutex};

pub type PetId = String;
pub type SharedPetRepository = Arc<Mutex<repository::PetRepository>>;

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
}
