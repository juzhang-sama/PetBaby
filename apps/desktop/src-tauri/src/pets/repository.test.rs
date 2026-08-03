use crate::pets::pet::{IdentityMode, Species};
use crate::pets::repository::PetRepository;
use crate::storage::Storage;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn temp_repo() -> (PetRepository, std::path::PathBuf) {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("desktop-pet-repo-{}-{n}", std::process::id()));
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
