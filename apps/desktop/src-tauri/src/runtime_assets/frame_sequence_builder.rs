//! `packFrameSequence` 交回来的 zip → 一个能装进运行时的预览目录。
//!
//! ## 与像素风那个 builder 最大的差别：**这一层不写 manifest**
//!
//! 像素风是自己拼 manifest（v3）再校验；写实风的 zip **自带** `manifest.json`（schema 7），
//! 由 Python 侧 `frames.packing` 生成。所以这里的活只有三件：
//! **解压 → 交给 `validate_asset_directory` 判 → 原子安装**。
//! manifest 里逐文件写着 sha256，**一个字节都不许改** —— 改了 `validate_asset_directory`
//! 立刻报 `asset directory is corrupt`。
//!
//! ## 为什么解压要自己写循环，而不是 `ZipArchive::extract`
//!
//! `extract` 内部虽然也做了路径检查，但那是**藏在依赖里的**、版本之间还会变。
//! 我们面对的是**从网络下来的一包字节**，落盘位置的边界必须是这个仓库里能读到、
//! 能测到的三行判断（见 `extract_zip` 的三道闸）。

use crate::runtime_assets::installer::{install_staged_assets, staging_directory_for};
use crate::runtime_assets::loader::validate_asset_directory;
use crate::runtime_assets::manifest::{parse_manifest, RuntimeAssetManifest};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 写实风 manifest 的 `variantId`。**产品固定值**，不是每只宠物一个。
///
/// Python 侧 `frames.packing.pack_frames` 的默认参数就是 `"combo-loop-v1"`，
/// `build_frame_sequence` 不覆盖它（单视频循环包，一个包就是一个动作）。
/// 安装时 `install_preview` 要拿它跟 manifest 对，所以这里必须是常量而不是拼出来的 id
/// —— 像素风那边是 `photo-avatar-<session>-<revision>`，两条路线在这点上是**不同**的。
pub const FRAME_SEQUENCE_VARIANT_ID: &str = "combo-loop-v1";

/// 解压后**总字节**上限。
///
/// 真实包：288 帧 WebP，解压后约 10 MB。512 MiB 给足余量，
/// 又拦得住「几百 KB 的 zip 炸出几十 GB」这种解压炸弹。
const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024;
/// 条目数上限。真实包 289 个（1 个 manifest + 288 帧）。
const MAX_ARCHIVE_ENTRIES: usize = 4096;

#[derive(Debug, Clone)]
pub struct BuildFrameSequenceRequest {
    pub session_id: String,
    pub revision: u32,
    pub pet_id: String,
    pub variant_id: String,
    /// `packFrameSequence` 交付的那支 zip 的**原始字节**。
    pub zip_bytes: Vec<u8>,
    pub zip_sha256: String,
}

#[derive(Debug, Clone)]
pub struct BuiltFrameSequencePackage {
    pub preview_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
}

#[derive(Debug, Clone)]
pub struct FrameSequenceBuilder {
    preview_root: PathBuf,
}

impl FrameSequenceBuilder {
    pub fn new(preview_root: &Path) -> Self {
        Self {
            preview_root: preview_root.to_path_buf(),
        }
    }

    pub fn build_preview(
        &self,
        request: BuildFrameSequenceRequest,
    ) -> Result<BuiltFrameSequencePackage, String> {
        validate_id(&request.session_id, "sessionId")?;
        validate_id(&request.pet_id, "petId")?;
        validate_id(&request.variant_id, "variantId")?;
        if sha256_hex(&request.zip_bytes) != request.zip_sha256 {
            return Err("frame sequence zip hash mismatch".into());
        }
        let preview_dir = self.preview_directory(&request.session_id, request.revision)?;
        let staging = staging_directory_for(&preview_dir)?;
        if let Err(error) = extract_zip(&request.zip_bytes, &staging) {
            // 解压到一半失败要清干净：`install_staged_assets` 只认 staging 里的东西，
            // 留一半残骸下一次会变成「看起来有目录、其实缺帧」。
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
        validate_asset_directory(&staging)?;
        install_staged_assets(&staging, &preview_dir)?;
        self.validate_preview(&request.session_id, request.revision)?;
        let manifest_path = preview_dir.join("manifest.json");
        let manifest_bytes = std::fs::read(&manifest_path).map_err(|error| error.to_string())?;
        Ok(BuiltFrameSequencePackage {
            preview_dir,
            manifest_path,
            manifest_sha256: sha256_hex(&manifest_bytes),
        })
    }

    /// 预览目录自检：`validate_asset_directory` 已经把 manifest 与**每个文件的 sha256**
    /// 都验过一遍，这里只再钉两件事 —— 必须是 V7，且 `renderer` 对得上。
    ///
    /// 不重复校验 sha256：那件事只有一处真源（`validate_asset_directory`），
    /// 抄第二遍就会有两套「什么算合格」。
    pub fn validate_preview(&self, session_id: &str, revision: u32) -> Result<(), String> {
        let preview_dir = self.preview_directory(session_id, revision)?;
        validate_asset_directory(&preview_dir)?;
        let manifest = read_manifest(&preview_dir)?;
        if manifest.renderer != "frame-sequence-v1" {
            return Err("frame sequence renderer must be frame-sequence-v1".into());
        }
        Ok(())
    }

    pub fn preview_manifest(
        &self,
        session_id: &str,
        revision: u32,
    ) -> Result<serde_json::Value, String> {
        let preview_dir = self.preview_directory(session_id, revision)?;
        let bytes = std::fs::read(preview_dir.join("manifest.json")).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }

    pub fn install_preview(
        &self,
        session_id: &str,
        revision: u32,
        destination: &Path,
    ) -> Result<(), String> {
        self.validate_preview(session_id, revision)?;
        let source = self.preview_directory(session_id, revision)?;
        let staging = staging_directory_for(destination)?;
        copy_directory(&source, &staging)?;
        install_staged_assets(&staging, destination)
    }

    fn preview_directory(&self, session_id: &str, revision: u32) -> Result<PathBuf, String> {
        validate_id(session_id, "sessionId")?;
        Ok(self
            .preview_root
            .join(session_id)
            .join(revision.to_string()))
    }
}

fn read_manifest(preview_dir: &Path) -> Result<crate::runtime_assets::frame_sequence::RuntimeAssetManifestV7, String> {
    let bytes =
        std::fs::read(preview_dir.join("manifest.json")).map_err(|error| error.to_string())?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "frame sequence manifest must be UTF-8")?;
    match parse_manifest(text)? {
        RuntimeAssetManifest::V7(manifest) => Ok(manifest),
        _ => Err("frame sequence preview must use manifest schema 7".into()),
    }
}

/// 把 zip 解到 `destination`（一个**新建的 staging 目录**）。
///
/// 三道闸，都是为了「服务端发回来的这包字节不能想写哪就写哪」：
///
/// 1. **条目路径必须被 `enclosed_name()` 认**。它把绝对路径、`..`、
///    Windows 盘符一律判为 `None` —— 没有这道，一个 `../../AppData/...` 的条目
///    就能写到预览目录外面（**zip slip**）。
/// 2. **条目数与解压总字节有上限** —— 几百 KB 的 zip 能炸出几十 GB（**解压炸弹**）。
/// 3. **symlink 条目直接拒**。符号链接会把后续写入引到别处，而我们的包里
///    一个链接都没有，没有「合法链接」需要放行。
fn extract_zip(bytes: &[u8], destination: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|error| format!("frame sequence zip is unreadable: {error}"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "frame sequence zip has too many entries: {}",
            archive.len()
        ));
    }
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let mut extracted: u64 = 0;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("frame sequence zip entry {index} is unreadable: {error}"))?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(format!(
                "frame sequence zip entry has an unsafe path: {}",
                entry.name()
            ));
        };
        if entry.is_symlink() {
            return Err(format!(
                "frame sequence zip must not contain symlinks: {}",
                entry.name()
            ));
        }
        if entry.is_dir() {
            std::fs::create_dir_all(destination.join(&relative))
                .map_err(|error| error.to_string())?;
            continue;
        }
        extracted = extracted.saturating_add(entry.size());
        if extracted > MAX_EXTRACTED_BYTES {
            return Err("frame sequence zip expands beyond the size limit".into());
        }
        let target = destination.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut file = std::fs::File::create(&target).map_err(|error| error.to_string())?;
        std::io::copy(&mut entry, &mut file).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn validate_id(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return Err(format!("invalid {label}"));
    }
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let target = destination.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            copy_directory(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 产品会发出的全部动作（与 Python 侧 `frames.packing.PRODUCT_MOTIONS` 同源）。
    /// schema 7 校验器要求 `semantics` 把每一个都显式声明出来。
    const MOTIONS: [&str; 9] = [
        "idle",
        "look-left",
        "look-right",
        "react-happy",
        "react-curious",
        "carried",
        "landed",
        "sleep",
        "wake",
    ];

    const FRAME_NAMES: [&str; 2] = ["frames/idle-combo/f0000.webp", "frames/idle-combo/f0001.webp"];
    const FRAME_BYTES: [&[u8]; 2] = [b"RIFF0000WEBPfirst", b"RIFF0000WEBPsecond"];

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-frame-builder-{label}-{}",
            crate::creation::domain::new_entity_id("frame")
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn zip_of(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(name.as_str(), options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// 造一个**真能过校验器**的 schema 7 包（帧是假字节，但 sha256 是真的）。
    fn valid_pack(pet_id: &str) -> Vec<u8> {
        let files: Vec<serde_json::Value> = FRAME_NAMES
            .iter()
            .zip(FRAME_BYTES.iter())
            .map(|(name, data)| {
                serde_json::json!({
                    "role": "frame",
                    "relativePath": name,
                    "sha256": sha256_hex(data),
                })
            })
            .collect();
        let semantics: serde_json::Map<String, serde_json::Value> = MOTIONS
            .iter()
            .map(|motion| {
                (
                    (*motion).to_string(),
                    serde_json::Value::String("idle-combo".into()),
                )
            })
            .collect();
        let manifest = serde_json::json!({
            "schemaVersion": 7,
            "renderer": "frame-sequence-v1",
            "petId": pet_id,
            "variantId": FRAME_SEQUENCE_VARIANT_ID,
            "displayName": "我的猫",
            "species": "cat",
            "baseImage": FRAME_NAMES[0],
            "defaultAction": "idle-combo",
            "anchorPolicy": "fixed",
            "actions": [{
                "actionId": "idle-combo",
                "loop": true,
                "frameDurationMs": 42,
                "frames": FRAME_NAMES.to_vec(),
            }],
            "semantics": serde_json::Value::Object(semantics),
            "files": files,
        });
        let mut entries: Vec<(String, Vec<u8>)> = FRAME_NAMES
            .iter()
            .zip(FRAME_BYTES.iter())
            .map(|(name, data)| ((*name).to_string(), data.to_vec()))
            .collect();
        entries.push((
            "manifest.json".to_string(),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        ));
        zip_of(&entries)
    }

    fn request(zip: Vec<u8>, revision: u32) -> BuildFrameSequenceRequest {
        BuildFrameSequenceRequest {
            session_id: "session-a".into(),
            revision,
            pet_id: "pet-frame".into(),
            variant_id: FRAME_SEQUENCE_VARIANT_ID.into(),
            zip_sha256: sha256_hex(&zip),
            zip_bytes: zip,
        }
    }

    #[test]
    fn a_real_pack_extracts_validates_and_installs_as_preview() {
        let root = temp_root("ok");
        let builder = FrameSequenceBuilder::new(&root);

        let built = builder.build_preview(request(valid_pack("pet-frame"), 1)).unwrap();

        assert!(built.preview_dir.join("manifest.json").is_file());
        assert!(built.preview_dir.join(FRAME_NAMES[0]).is_file());
        assert!(built.preview_dir.join(FRAME_NAMES[1]).is_file());
        assert_eq!(
            built.manifest_sha256,
            sha256_hex(&std::fs::read(&built.manifest_path).unwrap())
        );
        builder.validate_preview("session-a", 1).unwrap();

        let manifest = builder.preview_manifest("session-a", 1).unwrap();
        assert_eq!(manifest["variantId"], FRAME_SEQUENCE_VARIANT_ID);
        assert_eq!(manifest["petId"], "pet-frame");

        // 装到别处也要过（`install_preview` 内部复跑一遍自检）。
        let destination = root.join("installed");
        builder.install_preview("session-a", 1, &destination).unwrap();
        assert!(destination.join("manifest.json").is_file());
        assert!(destination.join(FRAME_NAMES[1]).is_file());

        // staging 不留残骸。
        let leftovers: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("staging"))
            .collect();
        assert!(leftovers.is_empty(), "staging 残骸: {leftovers:?}");

        let _ = std::fs::remove_dir_all(root);
    }

    /// 🔴 这是解压那一段最要紧的一条：从网络下来的包**不许想写哪就写哪**。
    #[test]
    fn a_zip_entry_that_escapes_the_preview_directory_is_rejected() {
        let root = temp_root("slip");
        let builder = FrameSequenceBuilder::new(&root);

        let zip = zip_of(&[("../escaped.txt".to_string(), b"pwned".to_vec())]);
        let error = builder.build_preview(request(zip, 1)).unwrap_err();

        assert!(error.contains("unsafe path"), "{error}");
        assert!(
            !root.join("escaped.txt").exists(),
            "zip slip 条目绝不该落到预览目录外面"
        );
        assert!(
            !std::env::temp_dir().join("escaped.txt").exists(),
            "也不能落到 temp 根"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_zip_whose_bytes_do_not_match_the_declared_hash_is_rejected() {
        let root = temp_root("hash");
        let builder = FrameSequenceBuilder::new(&root);

        let mut input = request(valid_pack("pet-frame"), 1);
        input.zip_sha256 = "00".repeat(32);

        assert_eq!(
            builder.build_preview(input).unwrap_err(),
            "frame sequence zip hash mismatch"
        );
        assert!(
            !root.join("session-a").exists(),
            "哈希不符就不该在预览根下留任何东西"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 帧字节与 manifest 里写的 sha256 对不上 —— 拦在**安装之前**，
    /// 否则一个缺帧/换帧的包会被当成好的装进运行时，表现是「桌宠卡住不动」。
    #[test]
    fn a_pack_whose_frames_do_not_match_the_manifest_is_rejected() {
        let root = temp_root("corrupt");
        let builder = FrameSequenceBuilder::new(&root);

        // manifest 里写的仍是**原始**帧字节的哈希，但包里放的是被换过的帧。
        let files: Vec<serde_json::Value> = FRAME_NAMES
            .iter()
            .zip(FRAME_BYTES.iter())
            .map(|(name, data)| {
                serde_json::json!({
                    "role": "frame",
                    "relativePath": name,
                    "sha256": sha256_hex(data),
                })
            })
            .collect();
        let semantics: serde_json::Map<String, serde_json::Value> = MOTIONS
            .iter()
            .map(|motion| {
                (
                    (*motion).to_string(),
                    serde_json::Value::String("idle-combo".into()),
                )
            })
            .collect();
        let manifest = serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": 7,
            "renderer": "frame-sequence-v1",
            "petId": "pet-frame",
            "variantId": FRAME_SEQUENCE_VARIANT_ID,
            "displayName": "我的猫",
            "species": "cat",
            "baseImage": FRAME_NAMES[0],
            "defaultAction": "idle-combo",
            "anchorPolicy": "fixed",
            "actions": [{
                "actionId": "idle-combo",
                "loop": true,
                "frameDurationMs": 42,
                "frames": FRAME_NAMES.to_vec(),
            }],
            "semantics": serde_json::Value::Object(semantics),
            "files": files,
        }))
        .unwrap();
        let entries = vec![
            (FRAME_NAMES[0].to_string(), b"tampered".to_vec()),
            (FRAME_NAMES[1].to_string(), FRAME_BYTES[1].to_vec()),
            ("manifest.json".to_string(), manifest),
        ];

        let error = builder
            .build_preview(request(zip_of(&entries), 1))
            .unwrap_err();
        assert_eq!(error, "asset directory is corrupt");
        assert!(
            !root.join("session-a").join("1").exists(),
            "坏包不许进预览目录"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unsafe_session_or_pet_id_never_reaches_the_filesystem() {
        let root = temp_root("ids");
        let builder = FrameSequenceBuilder::new(&root);

        let mut traversal = request(valid_pack("pet-frame"), 1);
        traversal.session_id = "../outside".into();
        assert_eq!(
            builder.build_preview(traversal).unwrap_err(),
            "invalid sessionId"
        );

        let mut bad_pet = request(valid_pack("pet-frame"), 1);
        bad_pet.pet_id = "pet frame".into();
        assert_eq!(builder.build_preview(bad_pet).unwrap_err(), "invalid petId");
        let _ = std::fs::remove_dir_all(root);
    }
}
