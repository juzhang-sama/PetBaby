pub use super::pixel_contract_facade::{parse_pixel_avatar_audit, PixelAvatarAudit};
pub use super::pixel_contract_v2::PixelAvatarAuditV2;
use serde::{Deserialize, Serialize};

pub const PIXEL_V1_STYLE_PROFILE_SHA256: &str =
    "342d61eaf88eecba41bbb7a21c76c000aa16d6b86dce03ef570431f746e34830";
pub const PIXEL_V1_REFERENCE_SHA256: &str =
    "5ebbaece6553ffa450731660aa0d3fbb208d8f2761e48eabfe696bc20a39447a";
pub const PIXEL_V1_PROMPT_TEMPLATE_VERSION: &str = "pixel-style-v1-prompt-v1";
pub const PIXEL_V2_STYLE_PROFILE_SHA256: &str =
    "2a48f382d0d0a579010ffae2ce90a7693d364a0cf64e5463e0ce7bf0291ee4ab";
pub const PIXEL_V2_REFERENCE_SHA256: &str =
    "75171817d27aee72439f373317ad0a3f43bdb2f8a76b0f8c55e24c306ac46c85";
pub const PIXEL_V2_PROMPT_TEMPLATE_VERSION: &str = "pixel-style-v2-animation-ready-prompt-v2";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PixelAlphaReportV1 {
    pub visible_pixels: u32,
    pub partial_alpha_pixels: u32,
    pub partial_alpha_ratio: f64,
    pub largest_component_pixels: u32,
    pub largest_component_share: f64,
    pub bounds_left: u32,
    pub bounds_top: u32,
    pub bounds_right: u32,
    pub bounds_bottom: u32,
    pub margin_left: u32,
    pub margin_top: u32,
    pub margin_right: u32,
    pub margin_bottom: u32,
}

impl PixelAlphaReportV1 {
    pub fn validate(&self, width: u32, height: u32) -> Result<(), String> {
        if self.visible_pixels == 0
            || self.partial_alpha_pixels > self.visible_pixels
            || self.largest_component_pixels == 0
            || self.largest_component_pixels > self.visible_pixels
            || !ratio_matches(
                self.partial_alpha_ratio,
                self.partial_alpha_pixels,
                self.visible_pixels,
            )
            || !ratio_matches(
                self.largest_component_share,
                self.largest_component_pixels,
                self.visible_pixels,
            )
        {
            return Err("pixel alpha report ratios are invalid".into());
        }
        if self.bounds_left >= self.bounds_right
            || self.bounds_top >= self.bounds_bottom
            || self.bounds_right > width
            || self.bounds_bottom > height
            || self.margin_left != self.bounds_left
            || self.margin_top != self.bounds_top
            || self.margin_right != width - self.bounds_right
            || self.margin_bottom != height - self.bounds_bottom
        {
            return Err("pixel alpha report bounds are inconsistent".into());
        }
        Ok(())
    }

    /// 容差比较：整数字段按位相等，浮点比例字段（ratio/share）允许 1e-9 容差。
    /// Python 后端用 `int / int` 计算比例，Rust 端用 `f64 / f64`，两者对同一组
    /// 整数可能相差 1 ULP；若用 derive 的 bit 级 `!=` 比较会导致 bind_to 误判失败。
    pub fn matches(&self, other: &Self) -> bool {
        self.visible_pixels == other.visible_pixels
            && self.partial_alpha_pixels == other.partial_alpha_pixels
            && self.largest_component_pixels == other.largest_component_pixels
            && self.bounds_left == other.bounds_left
            && self.bounds_top == other.bounds_top
            && self.bounds_right == other.bounds_right
            && self.bounds_bottom == other.bounds_bottom
            && self.margin_left == other.margin_left
            && self.margin_top == other.margin_top
            && self.margin_right == other.margin_right
            && self.margin_bottom == other.margin_bottom
            && (self.partial_alpha_ratio - other.partial_alpha_ratio).abs() <= 1e-9
            && (self.largest_component_share - other.largest_component_share).abs() <= 1e-9
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PixelAvatarAuditV1 {
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
}

impl PixelAvatarAuditV1 {
    pub fn validate_success(&self) -> Result<(), String> {
        if self.schema_version != 1
            || self.provider != "lk888"
            || self.provider_model != "gpt-image-2"
            || self.style_profile_id != "pixel-style-v1"
            || self.style_profile_sha256 != PIXEL_V1_STYLE_PROFILE_SHA256
            || self.reference_sha256 != PIXEL_V1_REFERENCE_SHA256
            || self.prompt_template_version != PIXEL_V1_PROMPT_TEMPLATE_VERSION
            || self.upstream_delete_api != "unsupported"
            || self.status != "succeeded"
            || self.error_code.is_some()
        {
            return Err("pixel avatar audit fixed metadata is invalid".into());
        }
        if self.session_id.is_empty()
            || self.attempt == 0
            || self.attempt > 3
            || self.provider_task_id.is_empty()
            || !self
                .provider_task_id
                .bytes()
                .all(|byte| byte.is_ascii_digit())
            || self.width < 1024
            || self.height < 1024
            || self.width > 2048
            || self.height > 2048
            || u64::from(self.width) * u64::from(self.height) > 4_194_304
            || self.privacy_policy_version.is_empty()
            || self.retention_policy.is_empty()
            || self.created_at.is_empty()
            || self.completed_at.is_empty()
        {
            return Err("pixel avatar audit fields are invalid".into());
        }
        for value in [
            &self.style_profile_sha256,
            &self.reference_sha256,
            &self.identity_profile_sha256,
            &self.provider_raw_sha256,
            &self.normalized_sha256,
        ] {
            if !is_lower_sha256(value) {
                return Err("pixel avatar audit sha256 is invalid".into());
            }
        }
        self.alpha_report.validate(self.width, self.height)
    }
}

pub(super) fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn ratio_matches(value: f64, numerator: u32, denominator: u32) -> bool {
    value.is_finite() && (value - f64::from(numerator) / f64::from(denominator)).abs() <= 1e-9
}
