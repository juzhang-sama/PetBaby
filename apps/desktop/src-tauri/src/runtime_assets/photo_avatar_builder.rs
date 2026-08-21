use crate::creation::photo_avatar::domain::{
    parse_appearance_profile_v1, AppearanceProfileV1, CanonicalTextureAuditV1,
};
use crate::runtime_assets::cat_character::{
    CatEdgeTailMappingV1, CatMotionMappingV1, RuntimeAssetManifestV5,
};
use crate::runtime_assets::installer::{install_staged_assets, staging_directory_for};
use crate::runtime_assets::loader::validate_asset_directory;
use crate::runtime_assets::manifest::{
    normalize_relative_path, parse_manifest, Live2DLicense, ManifestFileEntry, RuntimeAssetManifest,
};
use image::ImageFormat;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

const MODULE_SEMANTIC_VERSION: &str = "cat-a-live2d-v1";
const REQUIRED_MOTIONS: [&str; 12] = [
    "breathing",
    "blink",
    "ear-twitch",
    "tail-idle",
    "pointer-focus",
    "pet-happy",
    "sleepy-yawn",
    "half-stand-stretch",
    "edge-tail-left",
    "edge-tail-right",
    "edge-tail-top",
    "edge-tail-bottom",
];
const PRIMARY_MOTIONS: [&str; 8] = [
    "breathing",
    "blink",
    "ear-twitch",
    "tail-idle",
    "pointer-focus",
    "pet-happy",
    "sleepy-yawn",
    "half-stand-stretch",
];
const EDGE_NAMES: [&str; 4] = ["left", "right", "top", "bottom"];
const BODY_MODULE_IDS: [&str; 3] = ["body-slender-v1", "body-balanced-v1", "body-rounded-v1"];
const CANONICAL_AUDIT_FILE: &str = "canonical-texture-audit.json";
const CANONICAL_AUDIT_ROLE: &str = "canonical-texture-audit";
const SEMANTIC_LAYER_IDS: [&str; 7] = [
    "body-base",
    "face",
    "eyes-eyelids",
    "ears",
    "chest-forelegs",
    "tail",
    "occlusion-underlay",
];
// 语义层掩码资产的 sha256（Live2D 纹理合成链路，已冻结为技术沉淀）。
// 若更换掩码 PNG 资产，必须同步此常量，否则 validate_success 会静默失败。
const SEMANTIC_MASK_SHA256: &str =
    "ea0812149b2bb367eca38438b22a928e1148a5d348d4ad17f0a3c95cb182d404";

#[derive(Debug, Clone)]
pub struct BuildPhotoAvatarRequest {
    pub session_id: String,
    pub revision: u32,
    pub pet_id: String,
    pub variant_id: String,
    pub profile: AppearanceProfileV1,
    pub texture_png: Vec<u8>,
    pub texture_sha256: String,
    pub texture_audit: CanonicalTextureAuditV1,
}

#[derive(Debug, Clone)]
pub struct BuiltPhotoAvatarPackage {
    pub preview_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
    texture_path: PathBuf,
}

impl BuiltPhotoAvatarPackage {
    pub fn texture(&self) -> &Path {
        &self.texture_path
    }
}

#[derive(Debug, Clone)]
pub struct PhotoAvatarBuilder {
    module_root: PathBuf,
    preview_root: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModuleContract {
    schema_version: u32,
    semantic_version: String,
    read_only: bool,
    module_ids: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModuleManifest {
    schema_version: u32,
    module_id: String,
    semantic_version: String,
    read_only: bool,
    compatible_modules: BTreeMap<String, Vec<String>>,
    required_parameters: Vec<String>,
    tail_art_mesh: String,
    files: BTreeMap<String, String>,
    hashes: BTreeMap<String, String>,
    motions: BTreeMap<String, ModuleMotion>,
    approved_amplitude: serde_json::Value,
    motion_spatial_profile: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModuleMotion {
    relative_path: String,
    sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SemanticLayerAuditV1 {
    layer_id: String,
    provider_raw_sha256: String,
    canonical_layer_sha256: String,
    mask_sha256: String,
    attempt: u8,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SemanticAtlasAuditV1 {
    identity_reference_sha256: String,
    profile_sha256: String,
    layers: Vec<SemanticLayerAuditV1>,
    canonical_atlas_sha256: String,
    body_module_id: String,
}

impl PhotoAvatarBuilder {
    pub fn new(module_root: &Path, preview_root: &Path) -> Self {
        Self {
            module_root: module_root.to_path_buf(),
            preview_root: preview_root.to_path_buf(),
        }
    }

    pub fn body_module_contract_sha256(&self, module_id: &str) -> Result<String, String> {
        self.validate_module_contract(module_id)?;
        let module_dir = self.module_directory(module_id)?;
        let contract = read_regular_file(&module_dir.join("模块.json"))?;
        Ok(sha256_hex(&contract))
    }

    pub fn build_preview(
        &self,
        request: BuildPhotoAvatarRequest,
    ) -> Result<BuiltPhotoAvatarPackage, String> {
        self.validate_request(&request)?;
        let module_id = &request.profile.body_module_id;
        self.validate_module_contract(module_id)?;
        let module_dir = self.module_directory(module_id)?;
        let module = self.read_module_manifest(&module_dir, module_id)?;
        self.validate_module(&module_dir, &module)?;
        validate_canonical_texture(
            &request.texture_png,
            &request.texture_sha256,
            &request.texture_audit,
            &module,
            &module_dir,
        )?;

        let preview_dir = self.preview_directory(&request.session_id, request.revision)?;
        let staging = staging_directory_for(&preview_dir)?;
        let result = self.write_preview(&staging, &module_dir, &module, &request);
        if let Err(error) = result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }

        validate_asset_directory(&staging)?;
        install_staged_assets(&staging, &preview_dir)?;
        let manifest_path = preview_dir.join("manifest.json");
        let manifest_bytes = std::fs::read(&manifest_path).map_err(|error| error.to_string())?;
        Ok(BuiltPhotoAvatarPackage {
            texture_path: preview_dir.join(&module.files["neutralTexture"]),
            preview_dir,
            manifest_path,
            manifest_sha256: sha256_hex(&manifest_bytes),
        })
    }

    pub fn install_preview(
        &self,
        session_id: &str,
        revision: u32,
        destination: &Path,
    ) -> Result<(), String> {
        validate_id(session_id, "sessionId")?;
        validate_destination(destination)?;
        self.validate_preview(session_id, revision)?;
        let preview_dir = self.preview_directory(session_id, revision)?;
        let staging = staging_directory_for(destination)?;
        if let Err(error) = copy_directory(&preview_dir, &staging) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
        install_staged_assets(&staging, destination)
    }

    pub fn validate_preview(&self, session_id: &str, revision: u32) -> Result<(), String> {
        validate_id(session_id, "sessionId")?;
        let preview_dir = self.preview_directory(session_id, revision)?;
        validate_asset_directory(&preview_dir)?;
        let manifest_bytes = read_regular_file(&preview_dir.join("manifest.json"))?;
        let manifest = parse_manifest(
            std::str::from_utf8(&manifest_bytes)
                .map_err(|_| "photo avatar preview manifest is not UTF-8")?,
        )?;
        let RuntimeAssetManifest::V5(manifest) = manifest else {
            return Err("photo avatar preview must use RuntimeAssetManifestV5".into());
        };
        self.validate_module_contract(&manifest.body_module_id)?;
        let module_dir = self.module_directory(&manifest.body_module_id)?;
        let module = self.read_module_manifest(&module_dir, &manifest.body_module_id)?;
        self.validate_module(&module_dir, &module)?;
        let texture = manifest
            .files
            .iter()
            .find(|entry| entry.role == "texture")
            .ok_or("photo avatar preview texture is missing")?;
        if texture.relative_path != module.files["neutralTexture"] {
            return Err("photo avatar preview texture does not match its body module".into());
        }
        let texture_png = read_regular_file(&preview_dir.join(&texture.relative_path))?;
        let audit_entry = manifest
            .files
            .iter()
            .find(|entry| entry.role == CANONICAL_AUDIT_ROLE)
            .ok_or("photo avatar canonical texture audit is missing")?;
        if audit_entry.relative_path != CANONICAL_AUDIT_FILE {
            return Err("photo avatar canonical texture audit path is invalid".into());
        }
        let audit: CanonicalTextureAuditV1 =
            serde_json::from_slice(&read_regular_file(&preview_dir.join(CANONICAL_AUDIT_FILE))?)
                .map_err(|error| format!("invalid canonical texture audit: {error}"))?;
        if audit.session_id != session_id
            || audit.revision != revision
            || audit.body_module_id != manifest.body_module_id
        {
            return Err("canonical texture audit does not match preview identity".into());
        }
        validate_canonical_texture(&texture_png, &texture.sha256, &audit, &module, &module_dir)
    }

    fn validate_request(&self, request: &BuildPhotoAvatarRequest) -> Result<(), String> {
        validate_id(&request.session_id, "sessionId")?;
        validate_id(&request.pet_id, "petId")?;
        validate_id(&request.variant_id, "variantId")?;
        let profile_json =
            serde_json::to_string(&request.profile).map_err(|error| error.to_string())?;
        let parsed = parse_appearance_profile_v1(&profile_json)?;
        if parsed != request.profile {
            return Err("appearance profile must be canonical".into());
        }
        request.texture_audit.validate_success()?;
        if request.texture_audit.session_id != request.session_id
            || request.texture_audit.revision != request.revision
            || request.texture_audit.body_module_id != request.profile.body_module_id
        {
            return Err("canonical texture audit does not match build request".into());
        }
        Ok(())
    }

    fn module_directory(&self, module_id: &str) -> Result<PathBuf, String> {
        validate_id(module_id, "bodyModuleId")?;
        if !self.module_root.is_absolute() || !self.module_root.is_dir() {
            return Err("cat character module root is unavailable".into());
        }
        let module_dir = self.module_root.join(module_id);
        if !module_dir.is_dir() || is_symlink(&module_dir)? {
            return Err("body module is unavailable".into());
        }
        Ok(module_dir)
    }

    fn validate_module_contract(&self, module_id: &str) -> Result<(), String> {
        let bytes = read_regular_file(&self.module_root.join("模块合同.json"))?;
        let contract: ModuleContract = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid body module contract: {error}"))?;
        let expected_ids = BODY_MODULE_IDS
            .map(str::to_string)
            .into_iter()
            .collect::<BTreeSet<_>>();
        let declared_ids = contract.module_ids.into_iter().collect::<BTreeSet<_>>();
        if contract.schema_version != 1
            || contract.semantic_version != MODULE_SEMANTIC_VERSION
            || !contract.read_only
            || declared_ids != expected_ids
            || !declared_ids.contains(module_id)
        {
            return Err("body module root contract does not match the supported whitelist".into());
        }
        Ok(())
    }

    fn preview_directory(&self, session_id: &str, revision: u32) -> Result<PathBuf, String> {
        if !self.preview_root.is_absolute() {
            return Err("preview root must be an absolute path".into());
        }
        let destination = self
            .preview_root
            .join(session_id)
            .join(format!("r{revision}"));
        validate_destination(&destination)?;
        Ok(destination)
    }

    fn read_module_manifest(
        &self,
        module_dir: &Path,
        module_id: &str,
    ) -> Result<ModuleManifest, String> {
        let manifest_path = module_dir.join("模块.json");
        let bytes = read_regular_file(&manifest_path)?;
        let manifest: ModuleManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid body module manifest: {error}"))?;
        if manifest.schema_version != 1
            || manifest.module_id != module_id
            || manifest.semantic_version != MODULE_SEMANTIC_VERSION
            || !manifest.read_only
        {
            return Err(
                "body module manifest does not describe a read-only cat-a-live2d-v1 module".into(),
            );
        }
        Ok(manifest)
    }

    fn validate_module(&self, module_dir: &Path, module: &ModuleManifest) -> Result<(), String> {
        let compatible_modules = BTreeMap::from([
            ("face".into(), vec!["face-standard-v1".into()]),
            ("ears".into(), vec!["ears-independent-v1".into()]),
            ("eyes".into(), vec!["eyes-independent-v1".into()]),
            ("tail".into(), vec!["tail-independent-v1".into()]),
        ]);
        let required_parameters = [
            "ParamEyeLOpen",
            "ParamEyeROpen",
            "ParamEarL",
            "ParamEarR",
            "ParamTailAngle",
            "ParamTailCurl",
            "ParamTailTip",
            "ParamBreath",
            "ParamBodyStretch",
        ]
        .map(str::to_string)
        .into_iter()
        .collect::<BTreeSet<_>>();
        let declared_parameters = module
            .required_parameters
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let profile_amplitude = module
            .motion_spatial_profile
            .get("amplitude")
            .ok_or("body module motionSpatialProfile.amplitude is missing")?;
        if module.compatible_modules != compatible_modules
            || declared_parameters != required_parameters
            || module.required_parameters.len() != required_parameters.len()
            || module.tail_art_mesh != "ArtMeshTail"
            || &module.approved_amplitude != profile_amplitude
        {
            return Err("body module binding contract is invalid".into());
        }
        let file_roles = BTreeSet::from([
            "moc3".to_string(),
            "model3".to_string(),
            "displayInfo".to_string(),
            "neutralTexture".to_string(),
        ]);
        if module.files.keys().cloned().collect::<BTreeSet<_>>() != file_roles
            || module.hashes.keys().cloned().collect::<BTreeSet<_>>() != file_roles
        {
            return Err("body module must declare the exact Live2D file whitelist".into());
        }
        let motion_names = REQUIRED_MOTIONS
            .map(str::to_string)
            .into_iter()
            .collect::<BTreeSet<_>>();
        if module.motions.keys().cloned().collect::<BTreeSet<_>>() != motion_names {
            return Err("body module must declare eight motions and four edge-tail states".into());
        }
        for (role, relative_path) in &module.files {
            verify_declared_file(module_dir, relative_path, &module.hashes[role])?;
        }
        for motion in module.motions.values() {
            verify_declared_file(module_dir, &motion.relative_path, &motion.sha256)?;
        }
        self.validate_model_references(module_dir, module)
    }

    fn validate_model_references(
        &self,
        module_dir: &Path,
        module: &ModuleManifest,
    ) -> Result<(), String> {
        let model_path = module_dir.join(&module.files["model3"]);
        let model: serde_json::Value = serde_json::from_slice(&read_regular_file(&model_path)?)
            .map_err(|error| format!("invalid model3 json: {error}"))?;
        let references = model
            .get("FileReferences")
            .and_then(serde_json::Value::as_object)
            .ok_or("model3 FileReferences is missing")?;
        let expected_keys = BTreeSet::from([
            "Moc".to_string(),
            "Textures".to_string(),
            "DisplayInfo".to_string(),
            "Motions".to_string(),
        ]);
        if references.keys().cloned().collect::<BTreeSet<_>>() != expected_keys {
            return Err("model3 has unsupported external references".into());
        }
        if references.get("Moc").and_then(serde_json::Value::as_str) != Some(&module.files["moc3"])
            || references
                .get("DisplayInfo")
                .and_then(serde_json::Value::as_str)
                != Some(&module.files["displayInfo"])
            || references
                .get("Textures")
                .and_then(serde_json::Value::as_array)
                != Some(&vec![serde_json::Value::String(
                    module.files["neutralTexture"].clone(),
                )])
        {
            return Err("model3 references do not match the module whitelist".into());
        }
        let motions = references
            .get("Motions")
            .and_then(serde_json::Value::as_object)
            .ok_or("model3 motions are missing")?;
        if motions.keys().cloned().collect::<BTreeSet<_>>()
            != REQUIRED_MOTIONS.map(str::to_string).into_iter().collect()
        {
            return Err("model3 motions do not match the module whitelist".into());
        }
        for (name, expected) in &module.motions {
            let entries = motions
                .get(name)
                .and_then(serde_json::Value::as_array)
                .ok_or("model3 motion group must be an array")?;
            if entries.len() != 1 {
                return Err("model3 motion group must contain exactly one entry".into());
            }
            let entry = entries[0]
                .as_object()
                .ok_or("model3 motion entry must be an object")?;
            if entry.keys().map(String::as_str).collect::<BTreeSet<_>>() != BTreeSet::from(["File"])
            {
                return Err("model3 motion entry must only declare File".into());
            }
            let path = entry.get("File").and_then(serde_json::Value::as_str);
            if path != Some(expected.relative_path.as_str()) {
                return Err("model3 motion reference does not match the module whitelist".into());
            }
        }
        Ok(())
    }

    fn write_preview(
        &self,
        staging: &Path,
        module_dir: &Path,
        module: &ModuleManifest,
        request: &BuildPhotoAvatarRequest,
    ) -> Result<(), String> {
        let mut files = Vec::new();
        for role in ["moc3", "model3", "displayInfo"] {
            let relative = &module.files[role];
            copy_module_file(module_dir, staging, relative)?;
            files.push(file_entry(role, relative, &staging.join(relative))?);
        }
        let texture_relative = &module.files["neutralTexture"];
        write_file(staging, texture_relative, &request.texture_png)?;
        files.push(file_entry(
            "texture",
            texture_relative,
            &staging.join(texture_relative),
        )?);
        let audit_bytes = serde_json::to_vec_pretty(&request.texture_audit)
            .map_err(|error| format!("serialize canonical texture audit: {error}"))?;
        write_file(staging, CANONICAL_AUDIT_FILE, &audit_bytes)?;
        files.push(file_entry(
            CANONICAL_AUDIT_ROLE,
            CANONICAL_AUDIT_FILE,
            &staging.join(CANONICAL_AUDIT_FILE),
        )?);
        for name in REQUIRED_MOTIONS {
            let motion = &module.motions[name];
            copy_module_file(module_dir, staging, &motion.relative_path)?;
            files.push(file_entry(
                &format!("motion-{name}"),
                &motion.relative_path,
                &staging.join(&motion.relative_path),
            )?);
        }
        let spatial_profile = serde_json::to_vec_pretty(&module.motion_spatial_profile)
            .map_err(|error| error.to_string())?;
        write_file(staging, "motion-spatial-profile.json", &spatial_profile)?;
        files.push(file_entry(
            "motion-spatial-profile",
            "motion-spatial-profile.json",
            &staging.join("motion-spatial-profile.json"),
        )?);
        let license_bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "id": "photo-avatar-project-owned-v1",
            "author": "PetBaby",
            "source": "project-owned photo avatar texture and redistributable body module",
            "commercialUse": true,
            "redistributable": true
        }))
        .map_err(|error| error.to_string())?;
        write_file(staging, "许可证.json", &license_bytes)?;
        files.push(file_entry(
            "license",
            "许可证.json",
            &staging.join("许可证.json"),
        )?);
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

        let manifest = RuntimeAssetManifestV5 {
            schema_version: 5,
            renderer: "cat-spatial-live2d-v1".into(),
            pet_id: request.pet_id.clone(),
            variant_id: request.variant_id.clone(),
            skeleton_version: MODULE_SEMANTIC_VERSION.into(),
            body_module_id: request.profile.body_module_id.clone(),
            model_entry: module.files["model3"].clone(),
            preview_image: texture_relative.clone(),
            motion_spatial_profile: "motion-spatial-profile.json".into(),
            files,
            motions: motion_mappings(),
            parameters: parameter_mappings(),
            hit_areas: BTreeMap::from([
                ("body".into(), "ArtMeshBody".into()),
                ("edgeTail".into(), "ArtMeshTail".into()),
            ]),
            edge_tail_states: edge_tail_mappings(),
            license: Live2DLicense {
                id: "photo-avatar-project-owned-v1".into(),
                author: "PetBaby".into(),
                source: "project-owned photo avatar texture and redistributable body module".into(),
                commercial_use: true,
                redistributable: true,
            },
        };
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
        write_file(staging, "manifest.json", &manifest_bytes)
    }
}

fn validate_canonical_texture(
    texture_png: &[u8],
    declared_sha256: &str,
    audit: &CanonicalTextureAuditV1,
    module: &ModuleManifest,
    module_dir: &Path,
) -> Result<(), String> {
    audit.validate_success()?;
    if !is_sha256(declared_sha256)
        || sha256_hex(texture_png) != declared_sha256
        || audit.canonical_sha256 != declared_sha256
    {
        return Err("canonical texture hash mismatch".into());
    }
    if audit.body_module_id != module.module_id {
        return Err("canonical texture body module mismatch".into());
    }
    let module_contract = read_regular_file(&module_dir.join("模块.json"))?;
    if sha256_hex(&module_contract) != audit.module_contract_sha256 {
        return Err("canonical module contract hash mismatch".into());
    }
    let neutral = read_regular_file(&module_dir.join(&module.files["neutralTexture"]))?;
    if sha256_hex(&neutral) != audit.source_texture_sha256 {
        return Err("canonical source texture hash mismatch".into());
    }
    if texture_png == neutral {
        return Err("texture atlas must replace the neutral module texture".into());
    }
    let texture = decode_rgba(texture_png, "canonical texture")?;
    let source = decode_rgba(&neutral, "body module neutral texture")?;
    if texture.dimensions() != (2048, 2048) || source.dimensions() != (2048, 2048) {
        return Err("canonical texture and body module must be exactly 2048x2048".into());
    }
    for (canonical, module_pixel) in texture.pixels().zip(source.pixels()) {
        if canonical[3] != module_pixel[3] {
            return Err("canonical alpha does not match body module UV alpha layout".into());
        }
        if canonical[3] == 0 && canonical.0[..3] != [0, 0, 0] {
            return Err("canonical transparent RGB must be zero".into());
        }
    }
    let alpha = texture.pixels().map(|pixel| pixel[3]).collect::<Vec<_>>();
    if sha256_hex(&alpha) != audit.source_alpha_sha256 {
        return Err("canonical source alpha hash mismatch".into());
    }
    validate_semantic_audit(audit, module)?;
    Ok(())
}

fn validate_semantic_audit(
    audit: &CanonicalTextureAuditV1,
    module: &ModuleManifest,
) -> Result<(), String> {
    let semantic: SemanticAtlasAuditV1 = serde_json::from_value(audit.coverage_report.clone())
        .map_err(|error| format!("invalid semantic layer audit: {error}"))?;
    if semantic.canonical_atlas_sha256 != audit.canonical_sha256
        || semantic.body_module_id != module.module_id
        || !is_lower_sha256(&semantic.identity_reference_sha256)
        || !is_lower_sha256(&semantic.profile_sha256)
    {
        return Err("semantic atlas audit binding is invalid".into());
    }
    if semantic.layers.len() != SEMANTIC_LAYER_IDS.len() {
        return Err("semantic atlas audit layer set is invalid".into());
    }
    for (layer, expected_id) in semantic.layers.iter().zip(SEMANTIC_LAYER_IDS) {
        if layer.layer_id != expected_id
            || !(1..=3).contains(&layer.attempt)
            || !is_lower_sha256(&layer.provider_raw_sha256)
            || !is_lower_sha256(&layer.canonical_layer_sha256)
            || layer.mask_sha256 != SEMANTIC_MASK_SHA256
        {
            return Err("semantic atlas audit layer is invalid".into());
        }
    }
    if semantic_audit_digest(&semantic) != audit.provider_raw_sha256 {
        return Err("semantic atlas audit immutable digest mismatch".into());
    }
    Ok(())
}

fn semantic_audit_digest(audit: &SemanticAtlasAuditV1) -> String {
    let mut fields = vec![
        audit.identity_reference_sha256.clone(),
        audit.profile_sha256.clone(),
    ];
    for layer in &audit.layers {
        fields.extend([
            layer.layer_id.clone(),
            layer.provider_raw_sha256.clone(),
            layer.canonical_layer_sha256.clone(),
            layer.mask_sha256.clone(),
            layer.attempt.to_string(),
        ]);
    }
    fields.push(audit.canonical_atlas_sha256.clone());
    fields.push(audit.body_module_id.clone());
    sha256_hex(fields.join("\n").as_bytes())
}

fn decode_rgba(bytes: &[u8], label: &str) -> Result<image::RgbaImage, String> {
    image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .map_err(|_| format!("{label} must be a PNG"))
        .map(|image| image.to_rgba8())
}

fn motion_mappings() -> BTreeMap<String, CatMotionMappingV1> {
    PRIMARY_MOTIONS
        .into_iter()
        .map(|name| {
            (
                name.to_string(),
                CatMotionMappingV1 {
                    group: name.to_string(),
                    index: Some(0),
                },
            )
        })
        .collect()
}

fn parameter_mappings() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("eyeOpenLeft".into(), "ParamEyeLOpen".into()),
        ("eyeOpenRight".into(), "ParamEyeROpen".into()),
        ("eyeBallX".into(), "ParamEyeBallX".into()),
        ("eyeBallY".into(), "ParamEyeBallY".into()),
        ("earLeft".into(), "ParamEarL".into()),
        ("earRight".into(), "ParamEarR".into()),
        ("tailAngle".into(), "ParamTailAngle".into()),
        ("tailCurl".into(), "ParamTailCurl".into()),
        ("tailTip".into(), "ParamTailTip".into()),
        ("bodyBreath".into(), "ParamBreath".into()),
        ("bodyStretch".into(), "ParamBodyStretch".into()),
        ("mouthOpen".into(), "ParamMouthOpenY".into()),
    ])
}

fn edge_tail_mappings() -> BTreeMap<String, CatEdgeTailMappingV1> {
    EDGE_NAMES
        .into_iter()
        .map(|edge| {
            (
                edge.to_string(),
                CatEdgeTailMappingV1 {
                    group: format!("edge-tail-{edge}"),
                    index: Some(0),
                    tail_art_mesh: "ArtMeshTail".into(),
                },
            )
        })
        .collect()
}

fn validate_id(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!("{field} must be a safe non-empty identifier"));
    }
    Ok(())
}

fn validate_destination(destination: &Path) -> Result<(), String> {
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err("asset destination must be an absolute directory path".into());
    }
    if destination
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("asset destination must not contain parent traversal".into());
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_symlink(path: &Path) -> Result<bool, String> {
    std::fs::symlink_metadata(path)
        .map_err(|error| error.to_string())
        .map(|metadata| metadata.file_type().is_symlink())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, String> {
    if is_symlink(path)? || !path.is_file() {
        return Err(format!(
            "module asset is not a regular file: {}",
            path.display()
        ));
    }
    std::fs::read(path).map_err(|error| error.to_string())
}

fn verify_declared_file(
    module_dir: &Path,
    relative: &str,
    expected_hash: &str,
) -> Result<(), String> {
    let normalized = normalize_relative_path(relative)?;
    if !is_sha256(expected_hash) {
        return Err("module asset hash is invalid".into());
    }
    let bytes = read_regular_file(&module_dir.join(&normalized))?;
    if sha256_hex(&bytes) != expected_hash.to_ascii_lowercase() {
        return Err(format!("module asset hash mismatch: {normalized}"));
    }
    Ok(())
}

fn copy_module_file(module_dir: &Path, staging: &Path, relative: &str) -> Result<(), String> {
    let normalized = normalize_relative_path(relative)?;
    let bytes = read_regular_file(&module_dir.join(&normalized))?;
    write_file(staging, &normalized, &bytes)
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), String> {
    let normalized = normalize_relative_path(relative)?;
    let path = root.join(normalized);
    let parent = path
        .parent()
        .ok_or("asset output has no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

fn file_entry(role: &str, relative_path: &str, path: &Path) -> Result<ManifestFileEntry, String> {
    let relative_path = normalize_relative_path(relative_path)?;
    Ok(ManifestFileEntry {
        role: role.into(),
        sha256: sha256_hex(&std::fs::read(path).map_err(|error| error.to_string())?),
        relative_path,
    })
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if is_symlink(&source_path)? {
            return Err("preview contains a symbolic link".into());
        }
        if source_path.is_dir() {
            std::fs::create_dir_all(&destination_path).map_err(|error| error.to_string())?;
            copy_directory(&source_path, &destination_path)?;
        } else if source_path.is_file() {
            std::fs::copy(&source_path, &destination_path).map_err(|error| error.to_string())?;
        } else {
            return Err("preview contains an unsupported filesystem entry".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        semantic_audit_digest, BuildPhotoAvatarRequest, PhotoAvatarBuilder, SemanticAtlasAuditV1,
        SemanticLayerAuditV1, SEMANTIC_LAYER_IDS, SEMANTIC_MASK_SHA256,
    };
    use crate::creation::photo_avatar::domain::{
        parse_appearance_profile_v1, AppearanceProfileV1, CanonicalTextureAuditV1,
    };
    use crate::runtime_assets::loader::validate_asset_directory;
    use crate::runtime_assets::manifest::{parse_manifest, RuntimeAssetManifest};
    use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        module_root: PathBuf,
        preview_root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "desktop-pet-photo-builder-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            let module_root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../public/cat-character-modules/cat-a-live2d-v1");
            let preview_root = root.join("previews");
            std::fs::create_dir_all(&preview_root).unwrap();
            Self {
                root,
                module_root,
                preview_root,
            }
        }

        fn builder(&self) -> PhotoAvatarBuilder {
            PhotoAvatarBuilder::new(&self.module_root, &self.preview_root)
        }

        fn profile(&self, module_id: &str) -> AppearanceProfileV1 {
            let mut profile = parse_appearance_profile_v1(include_str!(
                "../../tests/fixtures/photo-avatar/appearance-profile.json"
            ))
            .unwrap();
            profile.body_module_id = module_id.into();
            profile
        }

        fn atlas(&self, module_id: &str, value: u8) -> Vec<u8> {
            let neutral = std::fs::read(
                self.module_root
                    .join(module_id)
                    .join(format!("{module_id}.2048/texture_00.png")),
            )
            .unwrap();
            let mut image = image::load_from_memory_with_format(&neutral, image::ImageFormat::Png)
                .unwrap()
                .to_rgba8();
            for pixel in image.pixels_mut() {
                if pixel[3] == 0 {
                    pixel.0[..3].copy_from_slice(&[0, 0, 0]);
                } else {
                    pixel.0[0] = value;
                    pixel.0[1] = value.wrapping_add(1);
                    pixel.0[2] = value.wrapping_add(2);
                }
            }
            let mut png = Vec::new();
            PngEncoder::new(&mut png)
                .write_image(image.as_raw(), 2048, 2048, ColorType::Rgba8.into())
                .unwrap();
            png
        }

        fn atlas_with_wrong_alpha_layout(&self, value: u8) -> Vec<u8> {
            let pixels = vec![value; 2048 * 2048 * 4];
            let mut png = Vec::new();
            PngEncoder::new(&mut png)
                .write_image(&pixels, 2048, 2048, ColorType::Rgba8.into())
                .unwrap();
            png
        }

        fn atlas_with_hidden_rgb_outside_alpha(&self, module_id: &str, value: u8) -> Vec<u8> {
            let canonical = self.atlas(module_id, value);
            let mut image =
                image::load_from_memory_with_format(&canonical, image::ImageFormat::Png)
                    .unwrap()
                    .to_rgba8();
            let pixel = image
                .pixels_mut()
                .find(|pixel| pixel[3] == 0)
                .expect("module fixture must contain transparent pixels");
            pixel.0[..3].copy_from_slice(&[7, 8, 9]);
            let mut png = Vec::new();
            PngEncoder::new(&mut png)
                .write_image(image.as_raw(), 2048, 2048, ColorType::Rgba8.into())
                .unwrap();
            png
        }

        fn audit(&self, module_id: &str, texture_png: &[u8]) -> CanonicalTextureAuditV1 {
            let module_dir = self.module_root.join(module_id);
            let source_texture =
                std::fs::read(module_dir.join(format!("{module_id}.2048/texture_00.png"))).unwrap();
            let source_alpha =
                image::load_from_memory_with_format(&source_texture, image::ImageFormat::Png)
                    .unwrap()
                    .to_rgba8()
                    .pixels()
                    .map(|pixel| pixel[3])
                    .collect::<Vec<_>>();
            let canonical_sha256 = sha256(texture_png);
            let semantic_audit = SemanticAtlasAuditV1 {
                identity_reference_sha256: "77".repeat(32),
                profile_sha256: "88".repeat(32),
                layers: SEMANTIC_LAYER_IDS
                    .into_iter()
                    .map(|layer_id| SemanticLayerAuditV1 {
                        layer_id: layer_id.into(),
                        provider_raw_sha256: "99".repeat(32),
                        canonical_layer_sha256: "aa".repeat(32),
                        mask_sha256: SEMANTIC_MASK_SHA256.into(),
                        attempt: 1,
                    })
                    .collect(),
                canonical_atlas_sha256: canonical_sha256.clone(),
                body_module_id: module_id.into(),
            };
            let provider_raw_sha256 = semantic_audit_digest(&semantic_audit);
            CanonicalTextureAuditV1 {
                schema_version: 1,
                session_id: "session-a".into(),
                revision: 1,
                attempt: 1,
                provider: "lk888".into(),
                provider_model: "gpt-image-2".into(),
                model_display_name: "GPT-image-2.0".into(),
                api_contract_version: "lk888-media-generate-v1".into(),
                privacy_policy_version: "unverified".into(),
                retention_policy: "unverified".into(),
                upstream_delete_api: "unsupported".into(),
                provider_task_id: "task-1".into(),
                provider_raw_sha256,
                canonical_sha256,
                body_module_id: module_id.into(),
                module_contract_sha256: sha256(
                    &std::fs::read(module_dir.join("模块.json")).unwrap(),
                ),
                source_texture_sha256: sha256(&source_texture),
                source_alpha_sha256: sha256(&source_alpha),
                work_canvas_sha256: "55".repeat(32),
                region_map_sha256: "66".repeat(32),
                composer_version: "deterministic-alpha-v1".into(),
                png_encoder_version: "pillow-png-v1".into(),
                coverage_report: serde_json::to_value(semantic_audit).unwrap(),
                status: "succeeded".into(),
                error_code: None,
                created_at: "2026-08-17T00:00:00Z".into(),
                completed_at: "2026-08-17T00:00:01Z".into(),
            }
        }

        fn request(&self, module_id: &str, value: u8) -> BuildPhotoAvatarRequest {
            let texture_png = self.atlas(module_id, value);
            let texture_audit = self.audit(module_id, &texture_png);
            BuildPhotoAvatarRequest {
                session_id: "session-a".into(),
                revision: 1,
                pet_id: "pet-photo-a".into(),
                variant_id: "photo-r1".into(),
                profile: self.profile(module_id),
                texture_sha256: sha256(&texture_png),
                texture_png,
                texture_audit,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn tree_hash(root: &Path) -> String {
        fn collect(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    collect(root, &path, files);
                } else {
                    files.push(path.strip_prefix(root).unwrap().to_path_buf());
                }
            }
        }
        let mut files = Vec::new();
        collect(root, root, &mut files);
        files.sort();
        let mut digest = Sha256::new();
        for relative in files {
            digest.update(relative.to_string_lossy().as_bytes());
            digest.update(std::fs::read(root.join(relative)).unwrap());
        }
        format!("{:x}", digest.finalize())
    }

    #[test]
    fn builder_replaces_neutral_texture_and_emits_a_strict_v5_package() {
        let fixture = Fixture::new();
        let request = fixture.request("body-slender-v1", 37);
        let atlas = request.texture_png.clone();

        let built = fixture.builder().build_preview(request).unwrap();
        let manifest =
            parse_manifest(&std::fs::read_to_string(&built.manifest_path).unwrap()).unwrap();
        let RuntimeAssetManifest::V5(manifest) = manifest else {
            panic!("photo avatar builder must emit schema v5");
        };
        assert_eq!(manifest.body_module_id, "body-slender-v1");
        assert_eq!(std::fs::read(built.texture()).unwrap(), atlas);
        assert_ne!(
            sha256(&std::fs::read(built.texture()).unwrap()),
            sha256(
                &std::fs::read(
                    fixture
                        .module_root
                        .join("body-slender-v1/body-slender-v1.2048/texture_00.png")
                )
                .unwrap()
            )
        );
        validate_asset_directory(&built.preview_dir).unwrap();
    }

    #[test]
    fn builder_rejects_tampered_semantic_layer_audit_fields() {
        let fixture = Fixture::new();
        for mutation in [
            "layer-id",
            "layer-order",
            "layer-hash",
            "mask-hash",
            "attempt",
        ] {
            let mut request = fixture.request("body-balanced-v1", 39);
            let layers = request.texture_audit.coverage_report["layers"]
                .as_array_mut()
                .unwrap();
            match mutation {
                "layer-id" => layers[0]["layerId"] = serde_json::json!("unknown"),
                "layer-order" => layers.swap(0, 1),
                "layer-hash" => {
                    layers[0]["canonicalLayerSha256"] = serde_json::json!("BB".repeat(32));
                }
                "mask-hash" => {
                    layers[0]["maskSha256"] = serde_json::json!("00".repeat(32));
                }
                "attempt" => layers[0]["attempt"] = serde_json::json!(4),
                _ => unreachable!(),
            }

            assert!(
                fixture.builder().build_preview(request).is_err(),
                "accepted semantic audit mutation: {mutation}"
            );
        }
    }

    #[test]
    fn builder_rejects_semantic_audit_not_bound_to_atlas_and_module() {
        let fixture = Fixture::new();
        for field in ["canonicalAtlasSha256", "bodyModuleId"] {
            let mut request = fixture.request("body-rounded-v1", 40);
            request.texture_audit.coverage_report[field] = if field == "bodyModuleId" {
                serde_json::json!("body-balanced-v1")
            } else {
                serde_json::json!("00".repeat(32))
            };

            assert!(
                fixture.builder().build_preview(request).is_err(),
                "accepted semantic audit mutation: {field}"
            );
        }
    }

    #[test]
    fn builder_rejects_unknown_module_wrong_atlas_hash_and_module_mutation() {
        let fixture = Fixture::new();
        let before = tree_hash(&fixture.module_root);
        let mut unknown_module = fixture.request("body-balanced-v1", 41);
        unknown_module.profile.body_module_id = "body-unknown".into();
        assert!(fixture.builder().build_preview(unknown_module).is_err());
        let mut wrong_hash = fixture.request("body-balanced-v1", 42);
        wrong_hash.texture_sha256 = "0".repeat(64);
        assert!(fixture.builder().build_preview(wrong_hash).is_err());
        assert_eq!(tree_hash(&fixture.module_root), before);
    }

    #[test]
    fn builder_is_idempotent_and_install_rejects_a_corrupt_preview() {
        let fixture = Fixture::new();
        let first = fixture
            .builder()
            .build_preview(fixture.request("body-rounded-v1", 51))
            .unwrap();
        let second = fixture
            .builder()
            .build_preview(fixture.request("body-rounded-v1", 51))
            .unwrap();
        assert_eq!(first.preview_dir, second.preview_dir);
        assert_eq!(first.manifest_sha256, second.manifest_sha256);

        let destination = fixture.root.join("installed");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("old.txt"), b"old").unwrap();
        std::fs::write(second.texture(), b"corrupt").unwrap();
        assert!(fixture
            .builder()
            .install_preview("session-a", 1, &destination)
            .is_err());
        assert_eq!(std::fs::read(destination.join("old.txt")).unwrap(), b"old");
    }

    #[test]
    fn builder_rejects_wrong_dimensions_neutral_texture_and_unsafe_ids() {
        let fixture = Fixture::new();
        let mut wrong_dimensions = fixture.request("body-balanced-v1", 63);
        wrong_dimensions.texture_png = {
            let mut png = Vec::new();
            PngEncoder::new(&mut png)
                .write_image(&[0, 0, 0, 255], 1, 1, ColorType::Rgba8.into())
                .unwrap();
            png
        };
        wrong_dimensions.texture_sha256 = sha256(&wrong_dimensions.texture_png);
        assert!(fixture.builder().build_preview(wrong_dimensions).is_err());

        let neutral = std::fs::read(
            fixture
                .module_root
                .join("body-balanced-v1/body-balanced-v1.2048/texture_00.png"),
        )
        .unwrap();
        let mut neutral_request = fixture.request("body-balanced-v1", 64);
        neutral_request.texture_sha256 = sha256(&neutral);
        neutral_request.texture_png = neutral;
        assert!(fixture.builder().build_preview(neutral_request).is_err());

        let mut unsafe_request = fixture.request("body-balanced-v1", 65);
        unsafe_request.session_id = "../escape".into();
        assert!(fixture.builder().build_preview(unsafe_request).is_err());
    }

    #[test]
    fn builder_rejects_texture_with_different_uv_alpha_layout() {
        let fixture = Fixture::new();
        let mut request = fixture.request("body-balanced-v1", 66);
        request.texture_png = fixture.atlas_with_wrong_alpha_layout(66);
        request.texture_sha256 = sha256(&request.texture_png);
        request.texture_audit.canonical_sha256 = request.texture_sha256.clone();

        let error = fixture.builder().build_preview(request).unwrap_err();

        assert!(error.contains("UV alpha layout"), "{error}");
    }

    #[test]
    fn builder_rejects_hidden_rgb_and_audit_even_when_manifest_hash_was_updated() {
        let fixture = Fixture::new();
        let mut request = fixture.request("body-balanced-v1", 73);
        request.texture_png = fixture.atlas_with_hidden_rgb_outside_alpha("body-balanced-v1", 73);
        request.texture_sha256 = sha256(&request.texture_png);
        request.texture_audit.canonical_sha256 = request.texture_sha256.clone();

        let error = fixture.builder().build_preview(request).unwrap_err();

        assert!(error.contains("transparent RGB"), "{error}");
    }

    #[test]
    fn builder_rejects_canonical_audit_hash_mismatches() {
        let fixture = Fixture::new();
        let mut source_alpha = fixture.request("body-balanced-v1", 74);
        source_alpha.texture_audit.source_alpha_sha256 = "77".repeat(32);
        assert!(fixture
            .builder()
            .build_preview(source_alpha)
            .unwrap_err()
            .contains("source alpha"));

        let mut module_contract = fixture.request("body-balanced-v1", 75);
        module_contract.texture_audit.module_contract_sha256 = "77".repeat(32);
        assert!(fixture
            .builder()
            .build_preview(module_contract)
            .unwrap_err()
            .contains("module contract"));

        let mut canonical = fixture.request("body-balanced-v1", 76);
        canonical.texture_audit.canonical_sha256 = "77".repeat(32);
        assert!(fixture
            .builder()
            .build_preview(canonical)
            .unwrap_err()
            .contains("canonical texture hash"));
    }

    #[test]
    fn validate_preview_rejects_synchronized_hidden_rgb_tampering() {
        let fixture = Fixture::new();
        let builder = fixture.builder();
        let built = builder
            .build_preview(fixture.request("body-balanced-v1", 77))
            .unwrap();
        let tampered = fixture.atlas_with_hidden_rgb_outside_alpha("body-balanced-v1", 77);
        let tampered_sha = sha256(&tampered);
        std::fs::write(built.texture(), &tampered).unwrap();

        let audit_path = built.preview_dir.join("canonical-texture-audit.json");
        let mut audit: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&audit_path).unwrap()).unwrap();
        audit["canonicalSha256"] = tampered_sha.clone().into();
        let audit_bytes = serde_json::to_vec_pretty(&audit).unwrap();
        std::fs::write(&audit_path, &audit_bytes).unwrap();

        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&built.manifest_path).unwrap()).unwrap();
        for file in manifest["files"].as_array_mut().unwrap() {
            match file["role"].as_str().unwrap() {
                "texture" => file["sha256"] = tampered_sha.clone().into(),
                "canonical-texture-audit" => file["sha256"] = sha256(&audit_bytes).into(),
                _ => {}
            }
        }
        std::fs::write(
            &built.manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        validate_asset_directory(&built.preview_dir).unwrap();

        let error = builder.validate_preview("session-a", 1).unwrap_err();

        assert!(error.contains("transparent RGB"), "{error}");
    }

    #[test]
    fn builder_revalidates_historical_preview_texture_layout() {
        let fixture = Fixture::new();
        let builder = fixture.builder();
        let built = builder
            .build_preview(fixture.request("body-balanced-v1", 67))
            .unwrap();
        let wrong_atlas = fixture.atlas_with_wrong_alpha_layout(67);
        std::fs::write(built.texture(), &wrong_atlas).unwrap();
        let audit_path = built.preview_dir.join(super::CANONICAL_AUDIT_FILE);
        let mut audit: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&audit_path).unwrap()).unwrap();
        audit["canonicalSha256"] = sha256(&wrong_atlas).into();
        let audit_bytes = serde_json::to_vec_pretty(&audit).unwrap();
        std::fs::write(&audit_path, &audit_bytes).unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&built.manifest_path).unwrap()).unwrap();
        let texture = manifest["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry["role"] == "texture")
            .unwrap();
        texture["sha256"] = sha256(&wrong_atlas).into();
        manifest["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry["role"] == super::CANONICAL_AUDIT_ROLE)
            .unwrap()["sha256"] = sha256(&audit_bytes).into();
        std::fs::write(
            &built.manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        validate_asset_directory(&built.preview_dir).unwrap();

        let error = builder.validate_preview("session-a", 1).unwrap_err();

        assert!(error.contains("UV alpha layout"), "{error}");
    }

    #[test]
    fn builder_rejects_a_hashed_module_model_with_an_external_reference() {
        let fixture = Fixture::new();
        let copied_modules = fixture.root.join("copied-modules");
        std::fs::create_dir_all(&copied_modules).unwrap();
        super::copy_directory(&fixture.module_root, &copied_modules).unwrap();
        let module_dir = copied_modules.join("body-slender-v1");
        let model_path = module_dir.join("body-slender-v1.model3.json");
        let mut model: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&model_path).unwrap()).unwrap();
        model["FileReferences"]["Physics"] = "external.physics3.json".into();
        std::fs::write(&model_path, serde_json::to_vec_pretty(&model).unwrap()).unwrap();

        let module_manifest_path = module_dir.join("模块.json");
        let mut module_manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&module_manifest_path).unwrap()).unwrap();
        module_manifest["hashes"]["model3"] = sha256(&std::fs::read(&model_path).unwrap()).into();
        std::fs::write(
            &module_manifest_path,
            serde_json::to_vec_pretty(&module_manifest).unwrap(),
        )
        .unwrap();

        let builder = PhotoAvatarBuilder::new(&copied_modules, &fixture.preview_root);
        assert!(builder
            .build_preview(fixture.request("body-slender-v1", 72))
            .is_err());
    }

    #[test]
    fn builder_rejects_a_second_external_entry_in_a_motion_group() {
        let fixture = Fixture::new();
        let copied_modules = fixture.root.join("copied-modules");
        std::fs::create_dir_all(&copied_modules).unwrap();
        super::copy_directory(&fixture.module_root, &copied_modules).unwrap();
        let module_dir = copied_modules.join("body-rounded-v1");
        let model_path = module_dir.join("body-rounded-v1.model3.json");
        let mut model: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&model_path).unwrap()).unwrap();
        model["FileReferences"]["Motions"]["breathing"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "File": "external.motion3.json" }));
        std::fs::write(&model_path, serde_json::to_vec_pretty(&model).unwrap()).unwrap();

        let module_manifest_path = module_dir.join("模块.json");
        let mut module_manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&module_manifest_path).unwrap()).unwrap();
        module_manifest["hashes"]["model3"] = sha256(&std::fs::read(&model_path).unwrap()).into();
        std::fs::write(
            &module_manifest_path,
            serde_json::to_vec_pretty(&module_manifest).unwrap(),
        )
        .unwrap();

        let builder = PhotoAvatarBuilder::new(&copied_modules, &fixture.preview_root);
        assert!(builder
            .build_preview(fixture.request("body-rounded-v1", 75))
            .is_err());
    }

    #[test]
    fn builder_rejects_a_module_omitted_from_the_root_contract() {
        let fixture = Fixture::new();
        let copied_modules = fixture.root.join("copied-modules");
        std::fs::create_dir_all(&copied_modules).unwrap();
        super::copy_directory(&fixture.module_root, &copied_modules).unwrap();
        let contract_path = copied_modules.join("模块合同.json");
        let mut contract: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&contract_path).unwrap()).unwrap();
        contract["moduleIds"] = serde_json::json!(["body-balanced-v1", "body-rounded-v1"]);
        std::fs::write(
            &contract_path,
            serde_json::to_vec_pretty(&contract).unwrap(),
        )
        .unwrap();

        let builder = PhotoAvatarBuilder::new(&copied_modules, &fixture.preview_root);
        assert!(builder
            .build_preview(fixture.request("body-slender-v1", 73))
            .is_err());
    }

    #[test]
    fn builder_rejects_a_module_with_tampered_binding_contract_fields() {
        let fixture = Fixture::new();
        let copied_modules = fixture.root.join("copied-modules");
        std::fs::create_dir_all(&copied_modules).unwrap();
        super::copy_directory(&fixture.module_root, &copied_modules).unwrap();
        let manifest_path = copied_modules.join("body-balanced-v1/模块.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["tailArtMesh"] = "ArtMeshBody".into();
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let builder = PhotoAvatarBuilder::new(&copied_modules, &fixture.preview_root);
        assert!(builder
            .build_preview(fixture.request("body-balanced-v1", 74))
            .is_err());
    }
}
