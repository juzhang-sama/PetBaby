use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Species {
    Cat,
    Dog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IdentityMode {
    RealPet,
    Reference,
    Guided,
    Adopted,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pet {
    pub pet_id: String,
    pub schema_version: i32,
    pub species: Species,
    pub identity_mode: IdentityMode,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetSummary {
    pub pet_id: String,
    pub species: Species,
    pub identity_mode: IdentityMode,
    pub created_at: String,
}

#[expect(dead_code)] // constructed by the asset importer in Task 5
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetVariant {
    pub variant_id: String,
    pub pet_id: String,
    pub style_id: String,
    pub manifest_path: String,
    pub created_at: String,
}
