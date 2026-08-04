use serde::{Deserialize, Serialize};

pub fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IdentityProfile {
    pub profile_id: String,
    pub pet_id: String,
    pub schema_version: i32,
    pub species: String,
    pub identity_mode: String,
    pub locked_traits: serde_json::Value,
    pub ref_asset_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceVariant {
    pub variant_id: String,
    pub pet_id: String,
    pub job_id: Option<String>,
    pub image_path: String,
    pub cutout_path: Option<String>,
    pub quality: String,
    pub accepted: bool,
    pub created_at: String,
}
