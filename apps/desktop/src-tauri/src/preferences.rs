use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProbePreferences {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub flipped: bool,
    pub mode: String,
}

impl Default for ProbePreferences {
    fn default() -> Self {
        Self {
            x: 1200,
            y: 500,
            width: 420,
            height: 520,
            scale: 1.0,
            flipped: false,
            mode: "companion".into(),
        }
    }
}

pub fn load(path: &Path) -> io::Result<ProbePreferences> {
    if !path.exists() {
        return Ok(ProbePreferences::default());
    }
    match serde_json::from_slice(&fs::read(path)?) {
        Ok(value) => Ok(value),
        // corrupt preferences fall back to defaults instead of blocking startup
        Err(_) => Ok(ProbePreferences::default()),
    }
}

pub fn save(path: &Path, value: &ProbePreferences) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(value).map_err(io::Error::other)?,
    )?;
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_preferences_atomically() {
        let root = std::env::temp_dir().join(format!("desktop-pet-{}", std::process::id()));
        let path = root.join("preferences.json");
        let value = ProbePreferences {
            x: 1,
            y: 2,
            ..ProbePreferences::default()
        };
        save(&path, &value).unwrap();
        assert_eq!(load(&path).unwrap(), value);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_preferences_fall_back_to_defaults() {
        let root = std::env::temp_dir().join(format!("desktop-pet-pref-{}", std::process::id()));
        let path = root.join("preferences.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, b"{ this is not valid json !").unwrap();
        assert_eq!(load(&path).unwrap(), ProbePreferences::default());
        let _ = fs::remove_dir_all(root);
    }
}
