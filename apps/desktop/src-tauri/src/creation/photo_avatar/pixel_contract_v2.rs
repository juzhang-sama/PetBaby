use super::pixel_contract::{
    is_lower_sha256, PixelAlphaReportV1, PIXEL_V2_PROMPT_TEMPLATE_VERSION,
    PIXEL_V2_REFERENCE_SHA256, PIXEL_V2_STYLE_PROFILE_SHA256,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PixelAvatarAuditV2 {
    pub schema_version: u32,
    pub session_id: String,
    pub revision: u32,
    pub attempt: u8,
    pub provider: String,
    pub provider_model: String,
    pub provider_task_id: String,
    pub style_profile_id: String,
    pub style_profile_sha256: String,
    pub reference_sha256: String,
    pub prompt_template_version: String,
    pub identity_profile_sha256: String,
    pub provider_raw_sha256: String,
    pub normalized_sha256: String,
    pub width: u32,
    pub height: u32,
    pub alpha_report: PixelAlphaReportV1,
    pub privacy_policy_version: String,
    pub retention_policy: String,
    pub upstream_delete_api: String,
    pub status: String,
    pub error_code: Option<String>,
    pub created_at: String,
    pub completed_at: String,
    pub logical_grid_size: u32,
    pub palette_color_limit: u16,
    pub visible_color_count: u16,
    pub quantize_method: String,
    pub dither: String,
    pub protected_accent_slots: u8,
    pub protected_accent_count: u8,
    pub downsample: String,
    pub upsample: String,
}

impl PixelAvatarAuditV2 {
    pub fn validate_success(&self) -> Result<(), String> {
        if self.schema_version != 2
            || self.provider != "lk888"
            || self.provider_model != "gpt-image-2"
            || self.style_profile_id != "pixel-style-v2-animation-ready"
            || self.style_profile_sha256 != PIXEL_V2_STYLE_PROFILE_SHA256
            || self.reference_sha256 != PIXEL_V2_REFERENCE_SHA256
            || self.prompt_template_version != PIXEL_V2_PROMPT_TEMPLATE_VERSION
            || self.width != 1024
            || self.height != 1024
            || self.logical_grid_size != 160
            || self.palette_color_limit != 24
            || self.quantize_method != "maxcoverage"
            || self.dither != "none"
            || self.protected_accent_slots != 4
            || self.downsample != "box"
            || self.upsample != "nearest"
            || self.upstream_delete_api != "unsupported"
            || self.status != "succeeded"
            || self.error_code.is_some()
        {
            return Err("pixel avatar audit v2 fixed metadata is invalid".into());
        }
        if self.session_id.is_empty()
            || !(1..=3).contains(&self.attempt)
            || self.provider_task_id.is_empty()
            || !self
                .provider_task_id
                .bytes()
                .all(|byte| byte.is_ascii_digit())
            || !(1..=24).contains(&self.visible_color_count)
            || self.protected_accent_count > 4
            || self.privacy_policy_version.is_empty()
            || self.retention_policy.is_empty()
            || self.created_at.is_empty()
            || self.completed_at.is_empty()
        {
            return Err("pixel avatar audit v2 fields are invalid".into());
        }
        for value in [
            &self.style_profile_sha256,
            &self.reference_sha256,
            &self.identity_profile_sha256,
            &self.provider_raw_sha256,
            &self.normalized_sha256,
        ] {
            if !is_lower_sha256(value) {
                return Err("pixel avatar audit v2 sha256 is invalid".into());
            }
        }
        self.alpha_report.validate(self.width, self.height)
    }
}
