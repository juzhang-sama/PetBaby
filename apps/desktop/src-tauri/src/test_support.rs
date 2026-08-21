use std::path::{Path, PathBuf};

const STORAGE_ROOT_CLAIM_ATTEMPTS: usize = 32;

#[derive(Debug)]
pub(crate) struct TestStorageRoot {
    path: PathBuf,
}

impl TestStorageRoot {
    pub(crate) fn claim(prefix: &str) -> std::io::Result<Self> {
        Self::claim_with(|| {
            std::env::temp_dir().join(crate::creation::domain::new_entity_id(prefix))
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn claim_with(mut next_candidate: impl FnMut() -> PathBuf) -> std::io::Result<Self> {
        for _ in 0..STORAGE_ROOT_CLAIM_ATTEMPTS {
            let candidate = next_candidate();
            match std::fs::create_dir(&candidate) {
                Ok(()) => return Ok(Self { path: candidate }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "failed to claim a unique test storage root after {STORAGE_ROOT_CLAIM_ATTEMPTS} attempts"
            ),
        ))
    }
}

impl Drop for TestStorageRoot {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "failed to remove test storage root {}: {error}",
                    self.path.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn retries_collisions_without_touching_an_existing_root() {
        let existing = candidate("collision-existing");
        let fresh = candidate("collision-fresh");
        std::fs::create_dir(&existing).unwrap();
        let sentinel = existing.join("desktop-pet.db");
        std::fs::write(&sentinel, b"must remain untouched").unwrap();
        let mut candidates = vec![existing.clone(), fresh.clone()].into_iter();

        let claimed = TestStorageRoot::claim_with(|| candidates.next().unwrap()).unwrap();

        assert_eq!(claimed.path(), fresh);
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"must remain untouched");
        drop(claimed);
        assert!(!fresh.exists());
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"must remain untouched");
        std::fs::remove_dir_all(existing).unwrap();
    }

    #[test]
    fn stops_after_32_collisions_without_owning_the_existing_root() {
        let existing = candidate("collision-exhausted");
        std::fs::create_dir(&existing).unwrap();
        let sentinel = existing.join("desktop-pet.db");
        std::fs::write(&sentinel, b"must remain untouched").unwrap();
        let mut attempts = 0;

        let error = TestStorageRoot::claim_with(|| {
            attempts += 1;
            existing.clone()
        })
        .unwrap_err();

        assert_eq!(attempts, 32);
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(error.to_string().contains("32"));
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"must remain untouched");
        std::fs::remove_dir_all(existing).unwrap();
    }

    fn candidate(label: &str) -> PathBuf {
        std::env::temp_dir().join(crate::creation::domain::new_entity_id(label))
    }
}
