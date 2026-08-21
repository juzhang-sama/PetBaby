use super::domain::{parse_pixel_appearance_profile_v1, PixelStyleProfileId};
use super::pixel_contract::{
    parse_pixel_avatar_audit, PixelAvatarAudit, PIXEL_V1_REFERENCE_SHA256,
    PIXEL_V1_STYLE_PROFILE_SHA256, PIXEL_V2_REFERENCE_SHA256, PIXEL_V2_STYLE_PROFILE_SHA256,
};
use serde_json::{json, Value};

fn alpha_report() -> Value {
    json!({
        "visiblePixels": 100,
        "partialAlphaPixels": 0,
        "partialAlphaRatio": 0.0,
        "largestComponentPixels": 100,
        "largestComponentShare": 1.0,
        "boundsLeft": 20,
        "boundsTop": 20,
        "boundsRight": 1004,
        "boundsBottom": 1004,
        "marginLeft": 20,
        "marginTop": 20,
        "marginRight": 20,
        "marginBottom": 20
    })
}

fn audit(style_profile_id: &str, schema_version: u8) -> Value {
    let (style_profile_sha256, reference_sha256, prompt_template_version) = match style_profile_id {
        "pixel-style-v1" => (
            PIXEL_V1_STYLE_PROFILE_SHA256,
            PIXEL_V1_REFERENCE_SHA256,
            "pixel-style-v1-prompt-v1",
        ),
        "pixel-style-v2-animation-ready" => (
            PIXEL_V2_STYLE_PROFILE_SHA256,
            PIXEL_V2_REFERENCE_SHA256,
            "pixel-style-v2-animation-ready-prompt-v2",
        ),
        value => panic!("unsupported test style: {value}"),
    };
    let mut value = json!({
        "schemaVersion": schema_version,
        "sessionId": "session-a",
        "revision": 1,
        "attempt": 1,
        "provider": "lk888",
        "providerModel": "gpt-image-2",
        "providerTaskId": "108652999",
        "styleProfileId": style_profile_id,
        "styleProfileSha256": style_profile_sha256,
        "referenceSha256": reference_sha256,
        "promptTemplateVersion": prompt_template_version,
        "identityProfileSha256": "00".repeat(32),
        "providerRawSha256": "11".repeat(32),
        "normalizedSha256": "22".repeat(32),
        "width": 1024,
        "height": 1024,
        "alphaReport": alpha_report(),
        "privacyPolicyVersion": "unverified",
        "retentionPolicy": "unverified",
        "upstreamDeleteApi": "unsupported",
        "status": "succeeded",
        "errorCode": null,
        "createdAt": "2026-08-21T00:00:00Z",
        "completedAt": "2026-08-21T00:00:01Z"
    });
    if schema_version == 2 {
        let object = value
            .as_object_mut()
            .expect("audit fixture must be an object");
        object.insert("logicalGridSize".into(), json!(160));
        object.insert("paletteColorLimit".into(), json!(24));
        object.insert("visibleColorCount".into(), json!(2));
        object.insert("quantizeMethod".into(), json!("maxcoverage"));
        object.insert("dither".into(), json!("none"));
        object.insert("protectedAccentSlots".into(), json!(4));
        object.insert("protectedAccentCount".into(), json!(1));
        object.insert("downsample".into(), json!("box"));
        object.insert("upsample".into(), json!("nearest"));
    }
    value
}

#[test]
fn parses_historical_v1_and_current_v2_audits() {
    let legacy = parse_pixel_avatar_audit(audit("pixel-style-v1", 1)).unwrap();
    let current = parse_pixel_avatar_audit(audit("pixel-style-v2-animation-ready", 2)).unwrap();

    assert!(matches!(legacy, PixelAvatarAudit::V1(_)));
    assert!(matches!(current, PixelAvatarAudit::V2(_)));
}

#[test]
fn rejects_v2_audit_with_v1_fingerprints() {
    let mut value = audit("pixel-style-v2-animation-ready", 2);
    value["styleProfileSha256"] = json!(PIXEL_V1_STYLE_PROFILE_SHA256);

    let error = parse_pixel_avatar_audit(value).unwrap_err();

    assert!(error.contains("fixed metadata"), "{error}");
}

#[test]
fn shared_pixel_contract_fixture_preserves_v1_and_v2() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/photo-avatar/pixel-style-v1-v2.json"
    ))
    .unwrap();
    let legacy_profile =
        parse_pixel_appearance_profile_v1(&fixture["legacyProfile"].to_string()).unwrap();
    let current_profile =
        parse_pixel_appearance_profile_v1(&fixture["currentProfile"].to_string()).unwrap();
    let legacy_audit = parse_pixel_avatar_audit(fixture["legacyAudit"].clone()).unwrap();
    let current_audit = parse_pixel_avatar_audit(fixture["currentAudit"].clone()).unwrap();

    assert_eq!(legacy_profile.style_profile_id, PixelStyleProfileId::V1);
    assert_eq!(
        current_profile.style_profile_id,
        PixelStyleProfileId::V2AnimationReady
    );
    assert!(matches!(legacy_audit, PixelAvatarAudit::V1(_)));
    assert!(matches!(current_audit, PixelAvatarAudit::V2(_)));

    let mut tampered = fixture["currentAudit"].clone();
    tampered["styleProfileSha256"] = json!(PIXEL_V1_STYLE_PROFILE_SHA256);
    assert!(parse_pixel_avatar_audit(tampered).is_err());
}
