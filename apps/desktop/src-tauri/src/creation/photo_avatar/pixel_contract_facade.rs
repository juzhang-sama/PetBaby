use serde::{Deserialize, Serialize};

use super::pixel_contract::{PixelAlphaReportV1, PixelAvatarAuditV1, PixelAvatarAuditV2};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PixelAvatarAudit {
    V1(PixelAvatarAuditV1),
    V2(PixelAvatarAuditV2),
}

impl PixelAvatarAudit {
    pub fn validate_success(&self) -> Result<(), String> {
        match self {
            Self::V1(audit) => audit.validate_success(),
            Self::V2(audit) => audit.validate_success(),
        }
    }

    pub fn bind_to(
        &self,
        session_id: &str,
        revision: u32,
        attempt: u8,
        normalized_sha256: &str,
        alpha: &PixelAlphaReportV1,
    ) -> Result<(), String> {
        self.validate_success()?;
        if self.session_id() != session_id
            || self.revision() != revision
            || self.attempt() != attempt
            || self.normalized_sha256() != normalized_sha256
            || !self.alpha_report().matches(alpha)
        {
            return Err("pixel avatar audit binding is invalid".into());
        }
        Ok(())
    }

    pub fn session_id(&self) -> &str {
        match self {
            Self::V1(audit) => &audit.session_id,
            Self::V2(audit) => &audit.session_id,
        }
    }

    pub const fn revision(&self) -> u32 {
        match self {
            Self::V1(audit) => audit.revision,
            Self::V2(audit) => audit.revision,
        }
    }

    pub const fn attempt(&self) -> u8 {
        match self {
            Self::V1(audit) => audit.attempt,
            Self::V2(audit) => audit.attempt,
        }
    }

    pub fn normalized_sha256(&self) -> &str {
        match self {
            Self::V1(audit) => &audit.normalized_sha256,
            Self::V2(audit) => &audit.normalized_sha256,
        }
    }

    pub fn identity_profile_sha256(&self) -> &str {
        match self {
            Self::V1(audit) => &audit.identity_profile_sha256,
            Self::V2(audit) => &audit.identity_profile_sha256,
        }
    }

    pub const fn width(&self) -> u32 {
        match self {
            Self::V1(audit) => audit.width,
            Self::V2(audit) => audit.width,
        }
    }

    pub const fn height(&self) -> u32 {
        match self {
            Self::V1(audit) => audit.height,
            Self::V2(audit) => audit.height,
        }
    }

    pub fn alpha_report(&self) -> &PixelAlphaReportV1 {
        match self {
            Self::V1(audit) => &audit.alpha_report,
            Self::V2(audit) => &audit.alpha_report,
        }
    }

    pub fn style_profile_id(&self) -> &str {
        match self {
            Self::V1(audit) => &audit.style_profile_id,
            Self::V2(audit) => &audit.style_profile_id,
        }
    }

    pub const fn visible_color_count(&self) -> Option<u16> {
        match self {
            Self::V1(_) => None,
            Self::V2(audit) => Some(audit.visible_color_count),
        }
    }
}

pub fn parse_pixel_avatar_audit(raw: serde_json::Value) -> Result<PixelAvatarAudit, String> {
    let audit: PixelAvatarAudit = serde_json::from_value(raw)
        .map_err(|error| format!("invalid pixel avatar audit: {error}"))?;
    audit.validate_success()?;
    Ok(audit)
}
