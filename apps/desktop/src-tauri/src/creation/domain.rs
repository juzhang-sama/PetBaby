use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CreationMethod {
    Upload,
    Composer,
    Adoption,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CreationSessionStatus {
    Draft,
    CandidateReady,
    Finalizing,
    RetryableFailure,
    Completed,
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerRecipe {
    pub recipe_version: u32,
    pub pack_id: String,
    pub pack_version: u32,
    pub layer_contract_version: u32,
    pub body_id: String,
    pub ears_id: String,
    pub eyes_id: String,
    pub muzzle_id: String,
    pub tail_id: String,
    pub color_id: String,
    pub pattern_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreationSnapshot {
    pub session_id: String,
    pub pet_id: String,
    pub method: CreationMethod,
    pub status: CreationSessionStatus,
    pub last_stable_status: CreationSessionStatus,
    pub current_step: String,
    pub display_name: Option<String>,
    pub job_id: Option<String>,
    pub job_status: Option<String>,
    pub candidate_id: Option<String>,
    pub recipe: Option<ComposerRecipe>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedCreation {
    pub request_id: String,
    pub session_id: String,
    pub pet_id: String,
    pub variant_id: String,
    pub already_completed: bool,
}

static ENTITY_NONCE: AtomicU64 = AtomicU64::new(0);

pub fn new_entity_id(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let nonce = ENTITY_NONCE.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{:x}-{nanos:x}-{nonce:x}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn public_enums_serialize_as_camel_case() {
        assert_eq!(
            serde_json::to_string(&CreationMethod::Adoption).unwrap(),
            "\"adoption\""
        );
        assert_eq!(
            serde_json::to_string(&CreationSessionStatus::CandidateReady).unwrap(),
            "\"candidateReady\""
        );
        assert_eq!(
            serde_json::from_str::<CreationSessionStatus>("\"retryableFailure\"").unwrap(),
            CreationSessionStatus::RetryableFailure
        );
    }

    #[test]
    fn entity_ids_do_not_collide_within_the_same_process() {
        let ids = (0..1_000)
            .map(|_| new_entity_id("session"))
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 1_000);
        assert!(ids.iter().all(|id| id.starts_with("session-")));
    }

    #[test]
    fn snapshots_and_recipes_use_stable_camel_case_fields() {
        let snapshot = CreationSnapshot {
            session_id: "session-1".into(),
            pet_id: "pet-1".into(),
            method: CreationMethod::Composer,
            status: CreationSessionStatus::Draft,
            last_stable_status: CreationSessionStatus::Draft,
            current_step: "ears".into(),
            display_name: Some("奶糖".into()),
            job_id: None,
            job_status: None,
            candidate_id: None,
            recipe: Some(ComposerRecipe {
                recipe_version: 1,
                pack_id: "cat-cute-v1".into(),
                pack_version: 1,
                layer_contract_version: 1,
                body_id: "body-1".into(),
                ears_id: "ears-1".into(),
                eyes_id: "eyes-1".into(),
                muzzle_id: "muzzle-1".into(),
                tail_id: "tail-1".into(),
                color_id: "color-1".into(),
                pattern_id: "pattern-none".into(),
            }),
            error: None,
        };

        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value["sessionId"], "session-1");
        assert_eq!(value["lastStableStatus"], "draft");
        assert_eq!(value["recipe"]["layerContractVersion"], 1);

        let prepared = PreparedCreation {
            request_id: "request-1".into(),
            session_id: "session-1".into(),
            pet_id: "pet-1".into(),
            variant_id: "variant-1".into(),
            already_completed: false,
        };
        let value = serde_json::to_value(prepared).unwrap();
        assert_eq!(value["alreadyCompleted"], false);
    }
}
