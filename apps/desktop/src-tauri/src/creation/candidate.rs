use crate::creation::domain::{new_entity_id, ComposerRecipe};
use crate::runtime_assets::motion_profile::{parse_motion_profile, MotionProfileV1};
use base64::Engine as _;
use image::ImageDecoder as _;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::{
    fs::OpenOptionsExt,
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
};

const MAX_COMPOSER_PNG_BYTES: usize = 10 * 1024 * 1024;
const MAX_COMPOSER_B64_BYTES: usize = MAX_COMPOSER_PNG_BYTES.div_ceil(3) * 4;
const MAX_COMPOSER_JSON_BYTES: u64 = 64 * 1024;
const LEGACY_COMPOSER_INTENT_FILE: &str = ".candidate-publish-intent.json";
const RESERVED_COMPOSER_INTENT_FILE: &str = ".candidate-publish-intent-reserved.json";
const OWNED_COMPOSER_INTENT_FILE: &str = ".candidate-publish-intent-owned.json";
const COMPLETE_COMPOSER_INTENT_FILE: &str = ".candidate-publish-intent-complete.json";
const RESERVED_ADOPTION_INTENT_FILE: &str = ".adoption-candidate-publish-intent-reserved.json";
const OWNED_ADOPTION_INTENT_FILE: &str = ".adoption-candidate-publish-intent-owned.json";
const COMPLETE_ADOPTION_INTENT_FILE: &str = ".adoption-candidate-publish-intent-complete.json";

#[derive(Clone, Copy)]
struct PublishNamespace {
    label: &'static str,
    version: u32,
    reserved_intent: &'static str,
    owned_intent: &'static str,
    complete_intent: &'static str,
}

const COMPOSER_NAMESPACE: PublishNamespace = PublishNamespace {
    label: "composer",
    version: 2,
    reserved_intent: RESERVED_COMPOSER_INTENT_FILE,
    owned_intent: OWNED_COMPOSER_INTENT_FILE,
    complete_intent: COMPLETE_COMPOSER_INTENT_FILE,
};
const ADOPTION_NAMESPACE: PublishNamespace = PublishNamespace {
    label: "adoption",
    version: 3,
    reserved_intent: RESERVED_ADOPTION_INTENT_FILE,
    owned_intent: OWNED_ADOPTION_INTENT_FILE,
    complete_intent: COMPLETE_ADOPTION_INTENT_FILE,
};

struct CandidateFile<'a> {
    name: &'static str,
    bytes: &'a [u8],
    limit: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidatePublishCheckpoint {
    AfterReserved,
    AfterOwned,
    AfterComplete,
    AfterMove,
}

#[cfg(test)]
type CandidateCrashPoint = CandidatePublishCheckpoint;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ComposerPublishIntent {
    version: u32,
    phase: Option<ComposerPublishPhase>,
    session_id: String,
    stage_name: String,
    body_sha256: String,
    profile_sha256: String,
    recipe_sha256: String,
    directory_identity: Option<FileIdentity>,
    file_identities: Option<Vec<FileIdentity>>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ComposerPublishPhase {
    Reserved,
    Owned,
    Complete,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileIdentity {
    volume_serial: u32,
    file_index: u64,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StandardCandidate {
    pub candidate_id: String,
    pub session_id: String,
    pub pet_id: String,
    pub job_id: Option<String>,
    pub body_path: String,
    pub motion_profile_path: String,
    pub quality: String,
    pub created_at: String,
}

pub struct DecodedComposerPng {
    pub bytes: Vec<u8>,
}

pub fn decode_composer_png(encoded: &str) -> Result<DecodedComposerPng, String> {
    if encoded.is_empty() || encoded.len() > MAX_COMPOSER_B64_BYTES {
        return Err(format!(
            "composer PNG base64 exceeds the {} byte encoded limit",
            MAX_COMPOSER_B64_BYTES
        ));
    }
    if encoded.len() % 4 != 0 {
        return Err("composer PNG base64 length is invalid".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("composer PNG base64 is invalid: {error}"))?;
    if bytes.is_empty() || bytes.len() > MAX_COMPOSER_PNG_BYTES {
        return Err(format!(
            "composer PNG exceeds the {} byte decoded limit",
            MAX_COMPOSER_PNG_BYTES
        ));
    }
    if image::guess_format(&bytes).ok() != Some(image::ImageFormat::Png) {
        return Err("composer candidate must be a PNG image".into());
    }

    let decoder = image::codecs::png::PngDecoder::new(Cursor::new(&bytes))
        .map_err(|error| format!("composer PNG header is invalid: {error}"))?;
    let (width, height) = decoder.dimensions();
    if width != 1024 || height != 1024 {
        return Err(format!(
            "composer candidate must be exactly 1024x1024, got {width}x{height}"
        ));
    }
    if decoder.color_type() != image::ColorType::Rgba8 {
        return Err("composer candidate must be an RGBA PNG".into());
    }
    let allocation = decoder.total_bytes();
    if allocation != 1024 * 1024 * 4 || allocation > 8 * 1024 * 1024 {
        return Err("composer candidate decoded allocation is invalid".into());
    }
    let mut pixels = vec![0; allocation as usize];
    decoder
        .read_image(&mut pixels)
        .map_err(|error| format!("composer PNG decode failed: {error}"))?;
    let has_subject = pixels.chunks_exact(4).any(|pixel| pixel[3] >= 8);
    let has_transparent_background = pixels.chunks_exact(4).any(|pixel| pixel[3] == 0);
    if !has_subject {
        return Err("composer candidate has no visible alpha silhouette".into());
    }
    if !has_transparent_background {
        return Err("composer candidate must include transparent background pixels".into());
    }
    image::RgbaImage::from_raw(width, height, pixels)
        .ok_or("composer candidate RGBA allocation is inconsistent")?;
    Ok(DecodedComposerPng { bytes })
}

pub struct PublishedComposerCandidate {
    pub body_path: PathBuf,
    pub motion_profile_path: PathBuf,
    candidate_dir: PathBuf,
    newly_published: bool,
    committed: bool,
    _parent_guards: Vec<OwnedDirectoryGuard>,
    candidate_guard: Option<OwnedDirectoryGuard>,
    file_guards: Vec<std::fs::File>,
    intent_files: Vec<OwnedIntentFile>,
    #[cfg(test)]
    intent_cleanup_failure: Option<ComposerPublishPhase>,
}

struct OwnedIntentFile {
    path: PathBuf,
    guard: std::fs::File,
}

struct OwnedDirectoryGuard {
    path: PathBuf,
    identity: FileIdentity,
    #[cfg(windows)]
    handle: OwnedHandle,
}

impl OwnedDirectoryGuard {
    #[cfg(windows)]
    fn open(path: &Path, label: &str) -> Result<Self, String> {
        Self::open_with_delete_sharing(path, label, false)
    }

    #[cfg(windows)]
    fn open_movable(path: &Path, label: &str) -> Result<Self, String> {
        Self::open_with_delete_sharing(path, label, true)
    }

    #[cfg(windows)]
    fn open_with_delete_sharing(
        path: &Path,
        label: &str,
        share_delete: bool,
    ) -> Result<Self, String> {
        use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE,
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };

        let wide = crate::platform::windows::encode_windows_path(path)?;
        let share =
            FILE_SHARE_READ | FILE_SHARE_WRITE | if share_delete { FILE_SHARE_DELETE } else { 0 };
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | GENERIC_WRITE | DELETE,
                share,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "open owned {label} directory handle: {}",
                std::io::Error::last_os_error()
            ));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) } == 0 {
            return Err(format!(
                "inspect owned {label} directory handle: {}",
                std::io::Error::last_os_error()
            ));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(format!("{label} must be a real non-reparse directory"));
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("{label} directory cannot be resolved: {error}"))?;
        Ok(Self {
            path: canonical,
            identity: identity_from_info(&info),
            handle,
        })
    }

    #[cfg(not(windows))]
    fn open(_path: &Path, _label: &str) -> Result<Self, String> {
        Err("secure composer publication currently requires Windows handle-relative I/O".into())
    }

    #[cfg(not(windows))]
    fn open_movable(_path: &Path, _label: &str) -> Result<Self, String> {
        Err("durable composer directory publication currently requires Windows".into())
    }

    #[cfg(windows)]
    fn mark_delete(&self) -> Result<(), String> {
        mark_raw_handle_delete(self.handle.as_raw_handle(), "composer candidate directory")
    }

    #[cfg(not(windows))]
    fn mark_delete(&self) -> Result<(), String> {
        Err("secure composer cleanup currently requires Windows handle-relative deletion".into())
    }
}

#[cfg(windows)]
fn identity_from_info(
    info: &windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION,
) -> FileIdentity {
    FileIdentity {
        volume_serial: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    }
}

#[cfg(windows)]
fn file_identity(file: &std::fs::File) -> Result<FileIdentity, String> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(format!(
            "inspect composer candidate file identity: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(identity_from_info(&info))
}

#[cfg(not(windows))]
fn file_identity(_file: &std::fs::File) -> Result<FileIdentity, String> {
    Err("secure composer file identity currently requires Windows".into())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

impl PublishedComposerCandidate {
    pub fn commit(&mut self) {
        self.committed = true;
        let _ = self.remove_intents();
    }

    pub fn rollback(&mut self) -> Result<(), String> {
        if self.committed || !self.newly_published {
            return Ok(());
        }
        for file in &self.file_guards {
            mark_file_delete(file)?;
        }
        self.file_guards.clear();
        let candidate = self
            .candidate_guard
            .take()
            .ok_or("published composer candidate lost its directory guard")?;
        candidate.mark_delete()?;
        drop(candidate);
        if std::fs::symlink_metadata(&self.candidate_dir).is_ok() {
            return Err(
                "unpublished composer candidate remained after handle-relative deletion".into(),
            );
        }
        self.remove_intents()?;
        self.newly_published = false;
        Ok(())
    }

    fn remove_intents(&mut self) -> Result<(), String> {
        let intent_files = std::mem::take(&mut self.intent_files);
        #[cfg(test)]
        {
            let failure = self.intent_cleanup_failure.take();
            return delete_locked_intent_files_with_hook(intent_files, move |intent| {
                if failure == intent_phase_for_path(&intent.path) {
                    Err(format!(
                        "injected {:?} intent cleanup failure",
                        failure.expect("matched cleanup phase")
                    ))
                } else {
                    Ok(())
                }
            });
        }
        #[cfg(not(test))]
        delete_locked_intent_files(intent_files)
    }

    #[cfg(test)]
    fn simulate_process_exit_before_database_commit(&mut self) {
        self.committed = true;
    }

    #[cfg(test)]
    fn simulate_intent_cleanup_failure_at(&mut self, phase: ComposerPublishPhase) {
        self.intent_cleanup_failure = Some(phase);
    }
}

impl Drop for PublishedComposerCandidate {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.rollback();
        }
    }
}

pub fn publish_composer_candidate(
    app_data_dir: &Path,
    session_id: &str,
    png: &[u8],
    profile: &MotionProfileV1,
    recipe: &ComposerRecipe,
) -> Result<PublishedComposerCandidate, String> {
    publish_composer_candidate_inner(
        app_data_dir,
        session_id,
        png,
        profile,
        recipe,
        || {},
        |_| {},
        |_| {},
        |_| {},
        |_| {},
        || Ok(()),
    )
}

pub fn publish_adoption_candidate(
    app_data_dir: &Path,
    session_id: &str,
    body: &[u8],
    motion_profile: &[u8],
) -> Result<PublishedComposerCandidate, String> {
    validate_adoption_candidate_bytes(body, motion_profile)?;
    publish_adoption_candidate_inner(app_data_dir, session_id, body, motion_profile)
}

#[cfg(test)]
fn publish_adoption_candidate_with_crash_point(
    app_data_dir: &Path,
    session_id: &str,
    body: &[u8],
    motion_profile: &[u8],
    crash_point: CandidateCrashPoint,
) -> Result<PublishedComposerCandidate, String> {
    publish_adoption_candidate_inner_with_checkpoint(
        app_data_dir,
        session_id,
        body,
        motion_profile,
        move |checkpoint| {
            if checkpoint == crash_point {
                panic!("simulated adoption crash at {crash_point:?}");
            }
        },
    )
}

#[cfg(test)]
pub fn publish_composer_candidate_with_hook(
    app_data_dir: &Path,
    session_id: &str,
    png: &[u8],
    profile: &MotionProfileV1,
    recipe: &ComposerRecipe,
    hook: impl FnOnce(),
) -> Result<PublishedComposerCandidate, String> {
    publish_composer_candidate_inner(
        app_data_dir,
        session_id,
        png,
        profile,
        recipe,
        hook,
        |_| {},
        |_| {},
        |_| {},
        |_| {},
        || Ok(()),
    )
}

#[cfg(test)]
pub fn publish_composer_candidate_with_post_rename_hook(
    app_data_dir: &Path,
    session_id: &str,
    png: &[u8],
    profile: &MotionProfileV1,
    recipe: &ComposerRecipe,
    hook: impl FnOnce() -> Result<(), String>,
) -> Result<PublishedComposerCandidate, String> {
    publish_composer_candidate_inner(
        app_data_dir,
        session_id,
        png,
        profile,
        recipe,
        || {},
        |_| {},
        |_| {},
        |_| {},
        |_| {},
        hook,
    )
}

#[cfg(test)]
pub fn publish_composer_candidate_with_staging_hooks(
    app_data_dir: &Path,
    session_id: &str,
    png: &[u8],
    profile: &MotionProfileV1,
    recipe: &ComposerRecipe,
    before_first_write: impl FnOnce(&Path),
    before_move: impl FnOnce(&Path),
) -> Result<PublishedComposerCandidate, String> {
    publish_composer_candidate_inner(
        app_data_dir,
        session_id,
        png,
        profile,
        recipe,
        || {},
        |_| {},
        |_| {},
        before_first_write,
        before_move,
        || Ok(()),
    )
}

#[cfg(test)]
pub fn publish_composer_candidate_with_intent_phase_hooks(
    app_data_dir: &Path,
    session_id: &str,
    png: &[u8],
    profile: &MotionProfileV1,
    recipe: &ComposerRecipe,
    before_owned_intent: impl FnOnce(&Path),
    before_complete_intent: impl FnOnce(&Path),
) -> Result<PublishedComposerCandidate, String> {
    publish_composer_candidate_inner(
        app_data_dir,
        session_id,
        png,
        profile,
        recipe,
        || {},
        before_owned_intent,
        before_complete_intent,
        |_| {},
        |_| {},
        || Ok(()),
    )
}

fn publish_composer_candidate_inner(
    app_data_dir: &Path,
    session_id: &str,
    png: &[u8],
    profile: &MotionProfileV1,
    recipe: &ComposerRecipe,
    before_publish: impl FnOnce(),
    before_owned_intent: impl FnOnce(&Path),
    before_complete_intent: impl FnOnce(&Path),
    before_first_write: impl FnOnce(&Path),
    before_move: impl FnOnce(&Path),
    after_rename: impl FnOnce() -> Result<(), String>,
) -> Result<PublishedComposerCandidate, String> {
    let profile_json = serde_json::to_vec_pretty(profile).map_err(|error| error.to_string())?;
    parse_motion_profile(std::str::from_utf8(&profile_json).map_err(|error| error.to_string())?)?;
    let recipe_json = serde_json::to_vec_pretty(recipe).map_err(|error| error.to_string())?;
    let files = [
        CandidateFile {
            name: "body.png",
            bytes: png,
            limit: MAX_COMPOSER_PNG_BYTES as u64,
        },
        CandidateFile {
            name: "motion-profile.json",
            bytes: &profile_json,
            limit: MAX_COMPOSER_JSON_BYTES,
        },
        CandidateFile {
            name: "recipe.json",
            bytes: &recipe_json,
            limit: MAX_COMPOSER_JSON_BYTES,
        },
    ];
    publish_candidate_inner(
        app_data_dir,
        session_id,
        COMPOSER_NAMESPACE,
        &files,
        before_publish,
        before_owned_intent,
        before_complete_intent,
        before_first_write,
        before_move,
        after_rename,
        |_| {},
    )
}

#[allow(clippy::too_many_arguments)]
fn publish_candidate_inner(
    app_data_dir: &Path,
    session_id: &str,
    namespace: PublishNamespace,
    files: &[CandidateFile<'_>],
    before_publish: impl FnOnce(),
    before_owned_intent: impl FnOnce(&Path),
    before_complete_intent: impl FnOnce(&Path),
    before_first_write: impl FnOnce(&Path),
    before_move: impl FnOnce(&Path),
    after_rename: impl FnOnce() -> Result<(), String>,
    mut checkpoint: impl FnMut(CandidatePublishCheckpoint),
) -> Result<PublishedComposerCandidate, String> {
    validate_component(session_id, "session id")?;
    if files.len() < 2 || files.len() > 3 {
        return Err("standard candidate must define two or three file roles".into());
    }

    let app_data_guard = OwnedDirectoryGuard::open(app_data_dir, "app data")?;
    let (_sessions_root, sessions_guard) =
        ensure_locked_child(&app_data_guard, "creation-sessions", "creation sessions")?;
    let (session_dir, session_guard) =
        ensure_locked_child(&sessions_guard, session_id, "creation session")?;
    before_publish();
    let candidate_dir = session_dir.join("candidate");
    if std::fs::symlink_metadata(&candidate_dir).is_ok() {
        return Err(format!(
            "{} candidate already exists; exact recovery is required",
            namespace.label
        ));
    }

    for name in [
        LEGACY_COMPOSER_INTENT_FILE,
        RESERVED_COMPOSER_INTENT_FILE,
        OWNED_COMPOSER_INTENT_FILE,
        COMPLETE_COMPOSER_INTENT_FILE,
        RESERVED_ADOPTION_INTENT_FILE,
        OWNED_ADOPTION_INTENT_FILE,
        COMPLETE_ADOPTION_INTENT_FILE,
    ] {
        if std::fs::symlink_metadata(session_dir.join(name)).is_ok() {
            return Err(format!(
                "candidate publish intent already exists in namespace {name}; exact recovery is required"
            ));
        }
    }
    let staging_name = format!(".candidate-stage-{}", new_entity_id("publish"));
    validate_component(staging_name.trim_start_matches('.'), "candidate staging id")?;
    let staging = session_dir.join(&staging_name);
    let mut intent = ComposerPublishIntent {
        version: namespace.version,
        phase: Some(ComposerPublishPhase::Reserved),
        session_id: session_id.to_owned(),
        stage_name: staging_name.clone(),
        body_sha256: sha256_hex(files[0].bytes),
        profile_sha256: sha256_hex(files[1].bytes),
        recipe_sha256: sha256_hex(files.get(2).map_or(&[], |file| file.bytes)),
        directory_identity: None,
        file_identities: None,
    };
    let reserved_intent_path = session_dir.join(namespace.reserved_intent);
    let mut intent_files = vec![OwnedIntentFile {
        path: reserved_intent_path.clone(),
        guard: publish_durable_intent(&session_dir, &reserved_intent_path, &intent)?,
    }];
    checkpoint(CandidatePublishCheckpoint::AfterReserved);
    std::fs::create_dir(&staging).map_err(|error| {
        format!(
            "create {} candidate staging directory: {error}",
            namespace.label
        )
    })?;
    let staging_guard = OwnedDirectoryGuard::open(&staging, "candidate staging")?;
    if staging_guard.path.parent() != Some(session_dir.as_path()) {
        return Err("candidate staging directory escapes its creation session".into());
    }
    intent.phase = Some(ComposerPublishPhase::Owned);
    intent.directory_identity = Some(staging_guard.identity);
    let owned_intent_path = session_dir.join(namespace.owned_intent);
    before_owned_intent(&owned_intent_path);
    match publish_durable_intent(&session_dir, &owned_intent_path, &intent) {
        Ok(guard) => intent_files.push(OwnedIntentFile {
            path: owned_intent_path,
            guard,
        }),
        Err(error) => {
            let cleanup = delete_locked_directory(staging_guard, Vec::new())
                .and_then(|()| delete_locked_intent_files(intent_files));
            return Err(match cleanup {
                Ok(()) => error,
                Err(cleanup) => format!("{error}; owned phase cleanup failed: {cleanup}"),
            });
        }
    }
    checkpoint(CandidatePublishCheckpoint::AfterOwned);
    before_first_write(&staging);

    let mut file_guards = Vec::new();
    let stage_result: Result<Vec<FileIdentity>, String> = (|| {
        for file in files {
            file_guards.push(write_new_synced_file(&staging.join(file.name), file.bytes)?);
        }
        validate_locked_candidate_files_for(&staging_guard, &file_guards, files, namespace.label)?;
        file_guards
            .iter()
            .map(file_identity)
            .collect::<Result<Vec<_>, _>>()
    })();
    let source_file_identities = match stage_result {
        Ok(identities) => identities,
        Err(error) => {
            file_guards.clear();
            let names = files.iter().map(|file| file.name).collect::<Vec<_>>();
            let cleanup = lock_owned_partial_file_set_for(
                &staging_guard,
                &names,
                &format!("owned {} staging directory", namespace.label),
            )
            .and_then(|files| delete_locked_directory(staging_guard, files))
            .and_then(|()| delete_locked_intent_files(intent_files));
            return Err(match cleanup {
                Ok(()) => error,
                Err(cleanup) => format!("{error}; staging cleanup failed: {cleanup}"),
            });
        }
    };
    intent.phase = Some(ComposerPublishPhase::Complete);
    intent.file_identities = Some(source_file_identities.clone());
    let complete_intent_path = session_dir.join(namespace.complete_intent);
    before_complete_intent(&complete_intent_path);
    match publish_durable_intent(&session_dir, &complete_intent_path, &intent) {
        Ok(guard) => intent_files.push(OwnedIntentFile {
            path: complete_intent_path,
            guard,
        }),
        Err(error) => {
            file_guards.clear();
            let names = files.iter().map(|file| file.name).collect::<Vec<_>>();
            let cleanup = lock_owned_partial_file_set_for(
                &staging_guard,
                &names,
                &format!("owned {} staging directory", namespace.label),
            )
            .and_then(|files| delete_locked_directory(staging_guard, files))
            .and_then(|()| delete_locked_intent_files(intent_files));
            return Err(match cleanup {
                Ok(()) => error,
                Err(cleanup) => format!("{error}; complete phase cleanup failed: {cleanup}"),
            });
        }
    }
    checkpoint(CandidatePublishCheckpoint::AfterComplete);

    file_guards.clear();
    let staged_identity = staging_guard.identity;
    drop(staging_guard);
    let mut staging_guard =
        OwnedDirectoryGuard::open_movable(&staging, "movable candidate staging")?;
    if staging_guard.identity != staged_identity {
        return Err(
            "candidate staging identity changed before durable move; publish intent retained"
                .into(),
        );
    }
    before_move(&staging);
    crate::platform::durable_move_directory(&staging, &candidate_dir).map_err(|error| {
        format!(
            "durable {} candidate move failed: {error}; publish intent retained",
            namespace.label
        )
    })?;
    staging_guard.path = candidate_dir.clone();
    checkpoint(CandidatePublishCheckpoint::AfterMove);
    let verification_guard =
        match OwnedDirectoryGuard::open_movable(&candidate_dir, "candidate verification") {
            Ok(guard) => guard,
            Err(error) => {
                return Err(format!(
                "open durable composer candidate after rename: {error}; publish intent retained"
            ))
            }
        };
    if verification_guard.path.parent() != Some(session_dir.as_path()) {
        return Err("published candidate directory escapes its creation session".into());
    }
    if verification_guard.identity != staging_guard.identity {
        return Err(
            "durable composer candidate identity differs from the staged directory; publish intent retained"
                .into(),
        );
    }
    let final_file_guards =
        match lock_candidate_files_for(&verification_guard, files, namespace.label) {
            Ok(files) => files,
            Err(error) => {
                return Err(format!(
                    "re-lock durable composer candidate: {error}; publish intent retained"
                ))
            }
        };
    for (source_identity, final_file) in source_file_identities.iter().zip(&final_file_guards) {
        if *source_identity != file_identity(final_file)? {
            return Err(
                "durable composer candidate file identity changed during publication; publish intent retained"
                    .into(),
            );
        }
    }
    let published_identity = verification_guard.identity;
    // The final file handles deliberately do not share delete access. They pin the
    // directory contents while the two movable directory handles are released, so
    // the path cannot be swapped before acquiring the long-lived directory guard.
    drop(verification_guard);
    drop(staging_guard);
    let candidate_guard = match OwnedDirectoryGuard::open(&candidate_dir, "candidate directory") {
        Ok(guard) => guard,
        Err(error) => {
            return Err(format!(
                "lock durable composer candidate after identity verification: {error}; publish intent retained"
            ))
        }
    };
    if candidate_guard.identity != published_identity {
        return Err(
            "durable composer candidate identity changed before final lock; publish intent retained"
                .into(),
        );
    }
    if let Err(error) = after_rename() {
        let cleanup = delete_published_directory(
            candidate_guard,
            final_file_guards,
            intent_files,
            &candidate_dir,
        );
        return Err(match cleanup {
            Ok(()) => error,
            Err(cleanup) => format!("{error}; exact post-rename cleanup failed: {cleanup}"),
        });
    }
    Ok(candidate_projection(
        candidate_dir,
        true,
        vec![app_data_guard, sessions_guard, session_guard],
        candidate_guard,
        final_file_guards,
        intent_files,
    ))
}

fn publish_adoption_candidate_inner(
    app_data_dir: &Path,
    session_id: &str,
    body: &[u8],
    motion_profile: &[u8],
) -> Result<PublishedComposerCandidate, String> {
    publish_adoption_candidate_inner_with_checkpoint(
        app_data_dir,
        session_id,
        body,
        motion_profile,
        |_| {},
    )
}

fn publish_adoption_candidate_inner_with_checkpoint(
    app_data_dir: &Path,
    session_id: &str,
    body: &[u8],
    motion_profile: &[u8],
    checkpoint: impl FnMut(CandidatePublishCheckpoint),
) -> Result<PublishedComposerCandidate, String> {
    validate_adoption_candidate_bytes(body, motion_profile)?;
    let files = [
        CandidateFile {
            name: "body.png",
            bytes: body,
            limit: MAX_COMPOSER_PNG_BYTES as u64,
        },
        CandidateFile {
            name: "motion-profile.json",
            bytes: motion_profile,
            limit: MAX_COMPOSER_JSON_BYTES,
        },
    ];
    publish_candidate_inner(
        app_data_dir,
        session_id,
        ADOPTION_NAMESPACE,
        &files,
        || {},
        |_| {},
        |_| {},
        |_| {},
        |_| {},
        || Ok(()),
        checkpoint,
    )
}

fn candidate_projection(
    candidate_dir: PathBuf,
    newly_published: bool,
    parent_guards: Vec<OwnedDirectoryGuard>,
    candidate_guard: OwnedDirectoryGuard,
    file_guards: Vec<std::fs::File>,
    intent_files: Vec<OwnedIntentFile>,
) -> PublishedComposerCandidate {
    PublishedComposerCandidate {
        body_path: candidate_dir.join("body.png"),
        motion_profile_path: candidate_dir.join("motion-profile.json"),
        candidate_dir,
        newly_published,
        committed: false,
        _parent_guards: parent_guards,
        candidate_guard: Some(candidate_guard),
        file_guards,
        intent_files,
        #[cfg(test)]
        intent_cleanup_failure: None,
    }
}

struct LockedIntentChain {
    files: Vec<OwnedIntentFile>,
    highest: ComposerPublishIntent,
}

fn load_locked_intent_chain(
    session_dir: &Path,
    session_id: &str,
    expected_profile: &MotionProfileV1,
    expected_recipe: &ComposerRecipe,
) -> Result<Option<LockedIntentChain>, String> {
    load_locked_intent_chain_for(session_dir, COMPOSER_NAMESPACE, true, |intent| {
        validate_publish_intent(intent, session_id, expected_profile, expected_recipe)
    })
}

fn load_locked_intent_chain_for(
    session_dir: &Path,
    namespace: PublishNamespace,
    reject_legacy_composer: bool,
    validate: impl Fn(&ComposerPublishIntent) -> Result<ComposerPublishPhase, String>,
) -> Result<Option<LockedIntentChain>, String> {
    if reject_legacy_composer {
        let legacy_path = session_dir.join(LEGACY_COMPOSER_INTENT_FILE);
        match std::fs::symlink_metadata(&legacy_path) {
            Ok(_) => return Err(
                "legacy composer publish intent lacks immutable phase ownership and was preserved"
                    .into(),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("inspect legacy composer publish intent: {error}")),
        }
    }

    let mut files = Vec::new();
    let mut intents = Vec::new();
    let mut missing_phase = false;
    for (expected_phase, name) in [
        (ComposerPublishPhase::Reserved, namespace.reserved_intent),
        (ComposerPublishPhase::Owned, namespace.owned_intent),
        (ComposerPublishPhase::Complete, namespace.complete_intent),
    ] {
        let path = session_dir.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(_) if missing_phase => {
                return Err(format!(
                    "{} publish intent phase prefix is incomplete before {expected_phase:?}; preserved",
                    namespace.label
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing_phase = true;
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "inspect {expected_phase:?} {} publish intent: {error}",
                    namespace.label
                ))
            }
        }
        let (guard, bytes) = lock_bounded_regular_file(
            &path,
            session_dir,
            &format!("{expected_phase:?} {} publish intent", namespace.label),
            32 * 1024,
        )?;
        let intent: ComposerPublishIntent = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "{expected_phase:?} {} publish intent is invalid and was preserved: {error}",
                namespace.label
            )
        })?;
        let actual_phase = validate(&intent)?;
        if actual_phase != expected_phase {
            return Err(format!(
                "{} publish intent file contains the wrong phase {actual_phase:?}; preserved",
                namespace.label
            ));
        }
        files.push(OwnedIntentFile { path, guard });
        intents.push(intent);
    }
    if intents.is_empty() {
        return Ok(None);
    }
    let reserved = &intents[0];
    for intent in intents.iter().skip(1) {
        if intent.version != reserved.version
            || intent.session_id != reserved.session_id
            || intent.stage_name != reserved.stage_name
            || intent.body_sha256 != reserved.body_sha256
            || intent.profile_sha256 != reserved.profile_sha256
            || intent.recipe_sha256 != reserved.recipe_sha256
        {
            return Err(format!(
                "{} publish intent phase records do not share one immutable prefix; preserved",
                namespace.label
            ));
        }
    }
    if intents.len() == 3 && intents[1].directory_identity != intents[2].directory_identity {
        return Err(format!(
            "{} publish intent complete phase changed its owned directory identity; preserved",
            namespace.label
        ));
    }
    Ok(Some(LockedIntentChain {
        files,
        highest: intents.pop().expect("non-empty intent chain"),
    }))
}

fn validate_publish_intent(
    intent: &ComposerPublishIntent,
    session_id: &str,
    expected_profile: &MotionProfileV1,
    expected_recipe: &ComposerRecipe,
) -> Result<ComposerPublishPhase, String> {
    validate_component(
        intent.stage_name.trim_start_matches('.'),
        "candidate staging id",
    )?;
    let profile_json =
        serde_json::to_vec_pretty(expected_profile).map_err(|error| error.to_string())?;
    let recipe_json =
        serde_json::to_vec_pretty(expected_recipe).map_err(|error| error.to_string())?;
    if intent.version != 2
        || intent.session_id != session_id
        || !intent.stage_name.starts_with(".candidate-stage-")
        || intent.body_sha256.len() != 64
        || intent.profile_sha256 != sha256_hex(&profile_json)
        || intent.recipe_sha256 != sha256_hex(&recipe_json)
    {
        return Err("composer publish intent does not match the durable session facts".into());
    }
    validate_publish_phase_ownership(intent, 3, "composer")
}

fn validate_publish_phase_ownership(
    intent: &ComposerPublishIntent,
    expected_file_count: usize,
    label: &str,
) -> Result<ComposerPublishPhase, String> {
    let phase = intent
        .phase
        .ok_or_else(|| format!("{label} publish intent has no durable ownership phase"))?;
    match phase {
        ComposerPublishPhase::Reserved
            if intent.directory_identity.is_none() && intent.file_identities.is_none() => {}
        ComposerPublishPhase::Owned
            if intent.directory_identity.is_some() && intent.file_identities.is_none() => {}
        ComposerPublishPhase::Complete
            if intent.directory_identity.is_some()
                && intent
                    .file_identities
                    .as_ref()
                    .is_some_and(|ids| ids.len() == expected_file_count) => {}
        _ => {
            return Err(format!(
                "{label} publish intent phase ownership is inconsistent"
            ))
        }
    }
    Ok(phase)
}

fn verify_owned_directory_identity(
    directory: &OwnedDirectoryGuard,
    intent: &ComposerPublishIntent,
) -> Result<(), String> {
    if intent.directory_identity != Some(directory.identity) {
        return Err(
            "composer publication directory identity does not match its durable intent; preserved"
                .into(),
        );
    }
    Ok(())
}

fn verify_partial_file_identities(
    directory: &OwnedDirectoryGuard,
    files: &[std::fs::File],
    expected: &[FileIdentity],
) -> Result<(), String> {
    verify_file_identities_for(
        directory,
        files,
        expected,
        &["body.png", "motion-profile.json", "recipe.json"],
        "owned composer publication",
    )
}

fn verify_file_identities_for(
    directory: &OwnedDirectoryGuard,
    files: &[std::fs::File],
    expected: &[FileIdentity],
    roles: &[&str],
    label: &str,
) -> Result<(), String> {
    if files.len() != expected.len() || files.len() > roles.len() {
        return Err(format!(
            "{label} durable file ownership count is inconsistent"
        ));
    }
    let entries = collect_bounded_directory_names(
        std::fs::read_dir(&directory.path)
            .map_err(|error| format!("read {label}: {error}"))?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|error| error.to_string())
            }),
        label,
    )?;
    if entries.len() != files.len() {
        return Err(format!("{label} changed while being locked"));
    }
    for (name, file) in entries.iter().zip(files) {
        let role = roles
            .iter()
            .position(|expected_name| expected_name == name)
            .ok_or_else(|| format!("{label} contains an unexpected file"))?;
        if file_identity(file)? != expected[role] {
            return Err(format!(
                "{label} {name} identity does not match its durable intent; preserved"
            ));
        }
    }
    Ok(())
}

fn load_locked_adoption_intent_chain(
    session_dir: &Path,
    session_id: &str,
    expected_body_sha256: &str,
    expected_profile_sha256: &str,
) -> Result<Option<LockedIntentChain>, String> {
    load_locked_intent_chain_for(session_dir, ADOPTION_NAMESPACE, false, |intent| {
        validate_adoption_publish_intent(
            intent,
            session_id,
            expected_body_sha256,
            expected_profile_sha256,
        )
    })
}

fn validate_adoption_publish_intent(
    intent: &ComposerPublishIntent,
    session_id: &str,
    expected_body_sha256: &str,
    expected_profile_sha256: &str,
) -> Result<ComposerPublishPhase, String> {
    validate_component(
        intent.stage_name.trim_start_matches('.'),
        "candidate staging id",
    )?;
    if intent.version != 3
        || intent.session_id != session_id
        || !intent.stage_name.starts_with(".candidate-stage-")
        || intent.body_sha256 != expected_body_sha256
        || intent.profile_sha256 != expected_profile_sha256
        || intent.recipe_sha256 != sha256_hex(&[])
    {
        return Err("adoption publish intent does not match durable template facts".into());
    }
    validate_publish_phase_ownership(intent, 2, "adoption")
}

fn lock_exact_adoption_candidate_file_set(
    directory: &OwnedDirectoryGuard,
) -> Result<(Vec<std::fs::File>, Vec<Vec<u8>>), String> {
    let entries = collect_bounded_directory_names(
        std::fs::read_dir(&directory.path)
            .map_err(|error| format!("read adoption candidate directory: {error}"))?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|error| error.to_string())
            }),
        "adoption candidate directory",
    )?;
    let expected = ["body.png", "motion-profile.json"]
        .into_iter()
        .map(str::to_string)
        .collect::<std::collections::BTreeSet<_>>();
    if entries != expected {
        return Err("adoption candidate does not contain exactly two standard files".into());
    }
    let mut files = Vec::new();
    let mut bytes = Vec::new();
    for (name, limit) in [
        ("body.png", MAX_COMPOSER_PNG_BYTES as u64),
        ("motion-profile.json", MAX_COMPOSER_JSON_BYTES),
    ] {
        let (file, content) = lock_bounded_regular_file(
            &directory.path.join(name),
            &directory.path,
            &format!("adoption candidate {name}"),
            limit,
        )?;
        files.push(file);
        bytes.push(content);
    }
    Ok((files, bytes))
}

fn verify_adoption_file_identities(
    directory: &OwnedDirectoryGuard,
    files: &[std::fs::File],
    expected: &[FileIdentity],
) -> Result<(), String> {
    verify_file_identities_for(
        directory,
        files,
        expected,
        &["body.png", "motion-profile.json"],
        "adoption publication",
    )
}

pub fn recover_exact_adoption_orphan(
    app_data_dir: &Path,
    session_id: &str,
    expected_body_sha256: &str,
    expected_profile_sha256: &str,
) -> Result<bool, String> {
    validate_component(session_id, "session id")?;
    let app_data_guard = OwnedDirectoryGuard::open(app_data_dir, "app data")?;
    let sessions_path = app_data_guard.path.join("creation-sessions");
    match std::fs::symlink_metadata(&sessions_path) {
        Ok(metadata) if crate::platform::is_link_or_reparse_point(&metadata) => {
            return Err("creation sessions root is a reparse point".into())
        }
        Ok(metadata) if !metadata.is_dir() => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("inspect creation sessions root: {error}")),
    }
    let sessions_guard = OwnedDirectoryGuard::open(&sessions_path, "creation sessions")?;
    if sessions_guard.path.parent() != Some(app_data_guard.path.as_path()) {
        return Err("creation sessions directory escapes app data".into());
    }
    let session_path = sessions_guard.path.join(session_id);
    match std::fs::symlink_metadata(&session_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("inspect adoption session directory: {error}")),
    }
    let session_guard = OwnedDirectoryGuard::open(&session_path, "adoption creation session")?;
    if session_guard.path.parent() != Some(sessions_guard.path.as_path()) {
        return Err("adoption session directory escapes creation sessions".into());
    }
    let chain = match load_locked_adoption_intent_chain(
        &session_guard.path,
        session_id,
        expected_body_sha256,
        expected_profile_sha256,
    )? {
        Some(chain) => chain,
        None => {
            if std::fs::symlink_metadata(session_guard.path.join("candidate")).is_ok() {
                return Err("adoption candidate exists without an exact publish intent".into());
            }
            return Ok(false);
        }
    };
    let intent = &chain.highest;
    let phase = intent.phase.expect("validated adoption phase");
    let candidate_path = session_guard.path.join("candidate");
    let stage_path = session_guard.path.join(&intent.stage_name);
    let candidate_exists = std::fs::symlink_metadata(&candidate_path).is_ok();
    let stage_exists = std::fs::symlink_metadata(&stage_path).is_ok();
    if candidate_exists && stage_exists {
        return Err("adoption intent owns both candidate and staging directories".into());
    }
    match phase {
        ComposerPublishPhase::Reserved => {
            if candidate_exists {
                return Err("reserved adoption intent cannot own a candidate".into());
            }
            if stage_exists {
                let directory =
                    OwnedDirectoryGuard::open(&stage_path, "reserved adoption staging")?;
                if directory.path.parent() != Some(session_guard.path.as_path())
                    || std::fs::read_dir(&directory.path)
                        .map_err(|error| error.to_string())?
                        .next()
                        .is_some()
                {
                    return Err("reserved adoption staging is not empty or contained".into());
                }
                delete_locked_directory(directory, Vec::new())?;
            }
        }
        ComposerPublishPhase::Owned | ComposerPublishPhase::Complete => {
            let owned_path = if candidate_exists {
                &candidate_path
            } else if stage_exists {
                &stage_path
            } else {
                delete_locked_intent_files(chain.files)?;
                return Ok(true);
            };
            if phase == ComposerPublishPhase::Owned && candidate_exists {
                return Err("owned adoption intent cannot own a candidate directory".into());
            }
            let directory = OwnedDirectoryGuard::open(owned_path, "owned adoption publication")?;
            if directory.path.parent() != Some(session_guard.path.as_path()) {
                return Err("owned adoption publication escapes its session".into());
            }
            verify_owned_directory_identity(&directory, intent)?;
            let files = lock_owned_partial_file_set_for(
                &directory,
                &["body.png", "motion-profile.json"],
                "owned adoption publication",
            )?;
            if let Some(expected) = &intent.file_identities {
                verify_adoption_file_identities(&directory, &files, expected)?;
            }
            delete_locked_directory(directory, files)?;
        }
    }
    delete_locked_intent_files(chain.files)?;
    Ok(true)
}

pub fn verify_committed_adoption_candidate(
    app_data_dir: &Path,
    session_id: &str,
    expected_body_sha256: &str,
    expected_profile_sha256: &str,
) -> Result<bool, String> {
    validate_component(session_id, "session id")?;
    let app_data_guard = OwnedDirectoryGuard::open(app_data_dir, "app data")?;
    let sessions_guard = OwnedDirectoryGuard::open(
        &app_data_guard.path.join("creation-sessions"),
        "creation sessions",
    )?;
    let session_guard = OwnedDirectoryGuard::open(
        &sessions_guard.path.join(session_id),
        "adoption creation session",
    )?;
    if sessions_guard.path.parent() != Some(app_data_guard.path.as_path())
        || session_guard.path.parent() != Some(sessions_guard.path.as_path())
    {
        return Err("adoption session directory escapes app data".into());
    }
    let chain = load_locked_adoption_intent_chain(
        &session_guard.path,
        session_id,
        expected_body_sha256,
        expected_profile_sha256,
    )?;
    let candidate_guard = OwnedDirectoryGuard::open(
        &session_guard.path.join("candidate"),
        "stored adoption candidate",
    )?;
    if candidate_guard.path.parent() != Some(session_guard.path.as_path()) {
        return Err("stored adoption candidate escapes its session".into());
    }
    let (files, bytes) = lock_exact_adoption_candidate_file_set(&candidate_guard)?;
    validate_adoption_candidate_bytes(&bytes[0], &bytes[1])?;
    if sha256_hex(&bytes[0]) != expected_body_sha256
        || sha256_hex(&bytes[1]) != expected_profile_sha256
    {
        return Err("committed adoption candidate does not match durable provenance".into());
    }
    if let Some(chain) = chain {
        let intent = &chain.highest;
        let phase = intent.phase.expect("validated adoption phase");
        if phase != ComposerPublishPhase::Complete {
            return Err("database candidate has an incomplete adoption publish intent".into());
        }
        verify_owned_directory_identity(&candidate_guard, intent)?;
        verify_adoption_file_identities(
            &candidate_guard,
            &files,
            intent.file_identities.as_deref().unwrap_or_default(),
        )?;
        if sha256_hex(&bytes[0]) != intent.body_sha256
            || sha256_hex(&bytes[1]) != intent.profile_sha256
        {
            return Err("committed adoption candidate does not match its intent".into());
        }
        delete_locked_intent_files(chain.files)?;
        return Ok(true);
    }
    Ok(false)
}

pub fn remove_empty_adoption_session_directory(
    app_data_dir: &Path,
    session_id: &str,
) -> Result<bool, String> {
    validate_component(session_id, "session id")?;
    let app_data_guard = OwnedDirectoryGuard::open(app_data_dir, "app data")?;
    let sessions_path = app_data_guard.path.join("creation-sessions");
    let sessions_metadata = match std::fs::symlink_metadata(&sessions_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("inspect creation sessions root: {error}")),
    };
    if crate::platform::is_link_or_reparse_point(&sessions_metadata) {
        return Err("creation sessions root cannot be a reparse point".into());
    }
    if !sessions_metadata.is_dir() {
        return Ok(false);
    }
    let sessions_guard = OwnedDirectoryGuard::open(&sessions_path, "creation sessions")?;
    let session_path = sessions_guard.path.join(session_id);
    match std::fs::symlink_metadata(&session_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("inspect adoption session directory: {error}")),
    }
    let session_guard = OwnedDirectoryGuard::open(&session_path, "empty adoption session")?;
    if session_guard.path.parent() != Some(sessions_guard.path.as_path()) {
        return Err("adoption session escapes creation sessions".into());
    }
    if std::fs::read_dir(&session_guard.path)
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        return Err("adoption session directory is not empty after rollback".into());
    }
    delete_locked_directory(session_guard, Vec::new())?;
    Ok(true)
}

pub fn recover_exact_composer_orphan(
    app_data_dir: &Path,
    session_id: &str,
    expected_profile: &MotionProfileV1,
    expected_recipe: &ComposerRecipe,
) -> Result<bool, String> {
    validate_component(session_id, "session id")?;
    let app_data_guard = OwnedDirectoryGuard::open(app_data_dir, "app data")?;
    let sessions_path = app_data_guard.path.join("creation-sessions");
    if std::fs::symlink_metadata(&sessions_path).is_err() {
        return Ok(false);
    }
    let sessions_guard = OwnedDirectoryGuard::open(&sessions_path, "creation sessions")?;
    if sessions_guard.path.parent() != Some(app_data_guard.path.as_path()) {
        return Err("creation sessions directory escapes app data".into());
    }
    let session_path = sessions_guard.path.join(session_id);
    if std::fs::symlink_metadata(&session_path).is_err() {
        return Ok(false);
    }
    let session_guard = OwnedDirectoryGuard::open(&session_path, "creation session")?;
    if session_guard.path.parent() != Some(sessions_guard.path.as_path()) {
        return Err("creation session directory escapes creation sessions".into());
    }
    let chain = match load_locked_intent_chain(
        &session_guard.path,
        session_id,
        expected_profile,
        expected_recipe,
    )? {
        Some(chain) => chain,
        None => {
            if std::fs::symlink_metadata(session_guard.path.join("candidate")).is_ok() {
                return Err(
                    "composer candidate exists without an exact durable publish intent".into(),
                );
            }
            return Ok(false);
        }
    };
    let phase = chain
        .highest
        .phase
        .expect("validated intent chain has a phase");
    let intent = &chain.highest;
    let candidate_path = session_guard.path.join("candidate");
    let stage_path = session_guard.path.join(&intent.stage_name);
    let candidate_exists = std::fs::symlink_metadata(&candidate_path).is_ok();
    let stage_exists = std::fs::symlink_metadata(&stage_path).is_ok();
    if candidate_exists && stage_exists {
        return Err("composer publish intent has both candidate and staging directories".into());
    }
    match phase {
        ComposerPublishPhase::Reserved => {
            if candidate_exists {
                return Err("reserved composer intent cannot own a candidate directory".into());
            }
            if stage_exists {
                let directory =
                    OwnedDirectoryGuard::open(&stage_path, "reserved composer staging")?;
                if directory.path.parent() != Some(session_guard.path.as_path()) {
                    return Err("reserved composer staging escapes its creation session".into());
                }
                if std::fs::read_dir(&directory.path)
                    .map_err(|error| format!("read reserved composer staging: {error}"))?
                    .next()
                    .is_some()
                {
                    return Err("reserved composer staging is not empty and was preserved".into());
                }
                delete_locked_directory(directory, Vec::new())?;
            }
        }
        ComposerPublishPhase::Owned | ComposerPublishPhase::Complete => {
            let owned_path = if candidate_exists {
                &candidate_path
            } else if stage_exists {
                &stage_path
            } else {
                delete_locked_intent_files(chain.files)?;
                return Ok(true);
            };
            if phase == ComposerPublishPhase::Owned && candidate_exists {
                return Err("owned-phase composer intent cannot own a candidate directory".into());
            }
            let directory = OwnedDirectoryGuard::open(owned_path, "owned composer publication")?;
            if directory.path.parent() != Some(session_guard.path.as_path()) {
                return Err("owned composer publication escapes its creation session".into());
            }
            verify_owned_directory_identity(&directory, &intent)?;
            let files = lock_owned_partial_file_set(&directory)?;
            if let Some(expected) = &intent.file_identities {
                verify_partial_file_identities(&directory, &files, expected)?;
            }
            delete_locked_directory(directory, files)?;
        }
    }
    delete_locked_intent_files(chain.files)?;
    drop((session_guard, sessions_guard, app_data_guard));
    Ok(true)
}

pub fn clear_committed_composer_publish_intent(
    app_data_dir: &Path,
    session_id: &str,
    expected_profile: &MotionProfileV1,
    expected_recipe: &ComposerRecipe,
) -> Result<bool, String> {
    validate_component(session_id, "session id")?;
    let app_data_guard = OwnedDirectoryGuard::open(app_data_dir, "app data")?;
    let sessions_guard = OwnedDirectoryGuard::open(
        &app_data_guard.path.join("creation-sessions"),
        "creation sessions",
    )?;
    if sessions_guard.path.parent() != Some(app_data_guard.path.as_path()) {
        return Err("creation sessions directory escapes app data".into());
    }
    let session_guard =
        OwnedDirectoryGuard::open(&sessions_guard.path.join(session_id), "creation session")?;
    if session_guard.path.parent() != Some(sessions_guard.path.as_path()) {
        return Err("creation session directory escapes creation sessions".into());
    }
    let chain = match load_locked_intent_chain(
        &session_guard.path,
        session_id,
        expected_profile,
        expected_recipe,
    )? {
        Some(chain) => chain,
        None => return Ok(false),
    };
    let intent = &chain.highest;
    let phase = intent.phase.expect("validated intent chain has a phase");
    let candidate_guard = OwnedDirectoryGuard::open(
        &session_guard.path.join("candidate"),
        "stored composer candidate",
    )?;
    if candidate_guard.path.parent() != Some(session_guard.path.as_path()) {
        return Err("stored composer candidate escapes its creation session".into());
    }
    if phase != ComposerPublishPhase::Reserved {
        verify_owned_directory_identity(&candidate_guard, intent)?;
    }
    let (candidate_files, bytes) = lock_exact_candidate_file_set(&candidate_guard)?;
    if phase == ComposerPublishPhase::Complete {
        verify_partial_file_identities(
            &candidate_guard,
            &candidate_files,
            intent.file_identities.as_deref().unwrap_or_default(),
        )?;
    }
    validate_exact_candidate_bytes(&bytes, expected_profile, expected_recipe)?;
    if sha256_hex(&bytes[0]) != intent.body_sha256
        || sha256_hex(&bytes[1]) != intent.profile_sha256
        || sha256_hex(&bytes[2]) != intent.recipe_sha256
    {
        return Err("committed composer candidate does not match its publish intent".into());
    }
    // The caller has already established DB ownership, and the exact candidate
    // bytes above match this validated intent prefix. Any object later appearing
    // at the old random stage name is deliberately preserved as unknown while
    // only the stale marker prefix is retired.
    delete_locked_intent_files(chain.files)?;
    Ok(true)
}

pub struct StoredComposerCandidate {
    pub body: Vec<u8>,
    pub motion_profile: MotionProfileV1,
}

pub fn read_exact_composer_candidate(
    app_data_dir: &Path,
    session_id: &str,
    expected_profile: &MotionProfileV1,
    expected_recipe: &ComposerRecipe,
) -> Result<StoredComposerCandidate, String> {
    try_read_exact_composer_candidate(app_data_dir, session_id, expected_profile, expected_recipe)?
        .ok_or_else(|| "stored composer candidate is missing".into())
}

pub fn try_read_exact_composer_candidate(
    app_data_dir: &Path,
    session_id: &str,
    expected_profile: &MotionProfileV1,
    expected_recipe: &ComposerRecipe,
) -> Result<Option<StoredComposerCandidate>, String> {
    validate_component(session_id, "session id")?;
    let app_data_guard = OwnedDirectoryGuard::open(app_data_dir, "app data")?;
    let sessions_guard = OwnedDirectoryGuard::open(
        &app_data_guard.path.join("creation-sessions"),
        "creation sessions",
    )?;
    if sessions_guard.path.parent() != Some(app_data_guard.path.as_path()) {
        return Err("creation sessions directory escapes app data".into());
    }
    let session_guard =
        OwnedDirectoryGuard::open(&sessions_guard.path.join(session_id), "creation session")?;
    if session_guard.path.parent() != Some(sessions_guard.path.as_path()) {
        return Err("creation session directory escapes creation sessions".into());
    }
    let candidate_path = session_guard.path.join("candidate");
    match std::fs::symlink_metadata(&candidate_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("inspect stored composer candidate: {error}")),
    }
    let candidate_guard = OwnedDirectoryGuard::open(&candidate_path, "stored composer candidate")?;
    if candidate_guard.path.parent() != Some(session_guard.path.as_path()) {
        return Err("stored composer candidate escapes its creation session".into());
    }
    let (files, bytes) = lock_exact_candidate_file_set(&candidate_guard)?;
    let motion_profile = validate_exact_candidate_bytes(&bytes, expected_profile, expected_recipe)?;
    let stored = StoredComposerCandidate {
        body: bytes[0].clone(),
        motion_profile,
    };
    drop(files);
    drop(candidate_guard);
    drop((session_guard, sessions_guard, app_data_guard));
    let _ = clear_committed_composer_publish_intent(
        app_data_dir,
        session_id,
        expected_profile,
        expected_recipe,
    );
    Ok(Some(stored))
}

fn validate_exact_candidate_bytes(
    bytes: &[Vec<u8>],
    expected_profile: &MotionProfileV1,
    expected_recipe: &ComposerRecipe,
) -> Result<MotionProfileV1, String> {
    if bytes.len() != 3 {
        return Err("composer candidate must contain exactly three file bodies".into());
    }
    let recipe: ComposerRecipe = serde_json::from_slice(&bytes[2])
        .map_err(|error| format!("composer recipe is invalid: {error}"))?;
    if &recipe != expected_recipe {
        return Err("composer recipe does not match the durable session".into());
    }
    let profile = parse_motion_profile(
        std::str::from_utf8(&bytes[1])
            .map_err(|error| format!("composer motion profile is not UTF-8: {error}"))?,
    )?;
    if &profile != expected_profile {
        return Err("composer motion profile does not match trusted body semantics".into());
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes[0]);
    decode_composer_png(&encoded)?;
    Ok(profile)
}

#[cfg(windows)]
fn lock_bounded_regular_file(
    path: &Path,
    expected_parent: &Path,
    label: &str,
    limit: u64,
) -> Result<(std::fs::File, Vec<u8>), String> {
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .access_mode(GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| format!("open {label}: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("inspect {label}: {error}"))?;
    if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(format!("{label} must be a regular non-reparse file"));
    }
    if metadata.len() > limit {
        return Err(format!("{label} exceeds the {limit} byte limit"));
    }
    if path
        .canonicalize()
        .map_err(|error| format!("resolve {label}: {error}"))?
        .parent()
        != Some(expected_parent)
    {
        return Err(format!("{label} escapes its owned directory"));
    }
    let identity = file_identity(&file)?;
    let reader = file.try_clone().map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read {label}: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{label} exceeds the {limit} byte limit"));
    }
    let after = file
        .metadata()
        .map_err(|error| format!("reinspect {label}: {error}"))?;
    if after.len() != metadata.len() || file_identity(&file)? != identity {
        return Err(format!("{label} changed while it was read"));
    }
    Ok((file, bytes))
}

#[cfg(not(windows))]
fn lock_bounded_regular_file(
    _path: &Path,
    _expected_parent: &Path,
    _label: &str,
    _limit: u64,
) -> Result<(std::fs::File, Vec<u8>), String> {
    Err("secure composer file locking currently requires Windows".into())
}

fn collect_bounded_directory_names(
    entries: impl Iterator<Item = Result<std::ffi::OsString, String>>,
    label: &str,
) -> Result<std::collections::BTreeSet<String>, String> {
    let mut names = std::collections::BTreeSet::new();
    for entry in entries {
        let name = entry?;
        if names.len() == 3 {
            return Err(format!("{label} contains more than three entries"));
        }
        let name = name
            .into_string()
            .map_err(|_| format!("{label} contains a non-Unicode name"))?;
        if !names.insert(name) {
            return Err(format!("{label} contains a duplicate name"));
        }
    }
    Ok(names)
}

fn lock_exact_candidate_file_set(
    directory: &OwnedDirectoryGuard,
) -> Result<(Vec<std::fs::File>, Vec<Vec<u8>>), String> {
    let entries = collect_bounded_directory_names(
        std::fs::read_dir(&directory.path)
            .map_err(|error| format!("read orphan candidate directory: {error}"))?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|error| error.to_string())
            }),
        "orphan candidate directory",
    )?;
    let expected = ["body.png", "motion-profile.json", "recipe.json"]
        .into_iter()
        .map(str::to_string)
        .collect::<std::collections::BTreeSet<_>>();
    if entries != expected {
        return Err("orphan candidate does not contain exactly the three standard files".into());
    }

    #[cfg(not(windows))]
    {
        let _ = directory;
        Err("secure composer orphan recovery currently requires Windows handle I/O".into())
    }
    #[cfg(windows)]
    {
        let mut files = Vec::new();
        let mut bytes = Vec::new();
        for (name, limit) in [
            ("body.png", MAX_COMPOSER_PNG_BYTES as u64),
            ("motion-profile.json", MAX_COMPOSER_JSON_BYTES),
            ("recipe.json", MAX_COMPOSER_JSON_BYTES),
        ] {
            let path = directory.path.join(name);
            let (file, content) = lock_bounded_regular_file(
                &path,
                &directory.path,
                &format!("composer candidate {name}"),
                limit,
            )?;
            files.push(file);
            bytes.push(content);
        }
        Ok((files, bytes))
    }
}

fn lock_owned_partial_file_set(
    directory: &OwnedDirectoryGuard,
) -> Result<Vec<std::fs::File>, String> {
    lock_owned_partial_file_set_for(
        directory,
        &["body.png", "motion-profile.json", "recipe.json"],
        "owned composer staging directory",
    )
}

fn lock_owned_partial_file_set_for(
    directory: &OwnedDirectoryGuard,
    expected_names: &[&str],
    label: &str,
) -> Result<Vec<std::fs::File>, String> {
    let entries = collect_bounded_directory_names(
        std::fs::read_dir(&directory.path)
            .map_err(|error| format!("read {label}: {error}"))?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|error| error.to_string())
            }),
        label,
    )?;
    let expected = expected_names
        .iter()
        .copied()
        .map(str::to_string)
        .collect::<std::collections::BTreeSet<_>>();
    if !entries.is_subset(&expected) {
        return Err(format!("{label} contains an unexpected file"));
    }

    #[cfg(not(windows))]
    {
        let _ = directory;
        Err("secure candidate partial cleanup currently requires Windows handle I/O".into())
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::GENERIC_READ;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        let mut files = Vec::new();
        for name in entries {
            let path = directory.path.join(&name);
            let file = std::fs::OpenOptions::new()
                .read(true)
                .access_mode(GENERIC_READ | DELETE)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&path)
                .map_err(|error| format!("open {label} {name}: {error}"))?;
            let metadata = file
                .metadata()
                .map_err(|error| format!("inspect {label} {name}: {error}"))?;
            if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
                return Err(format!("{label} {name} is not a regular file"));
            }
            if path
                .canonicalize()
                .map_err(|error| format!("resolve owned composer staging {name}: {error}"))?
                .parent()
                != Some(directory.path.as_path())
            {
                return Err(format!("{label} {name} escapes its directory"));
            }
            files.push(file);
        }
        Ok(files)
    }
}

fn publish_durable_intent(
    session_dir: &Path,
    intent_path: &Path,
    intent: &ComposerPublishIntent,
) -> Result<std::fs::File, String> {
    publish_durable_intent_with_io_hooks(
        session_dir,
        intent_path,
        intent,
        |file, bytes| {
            file.write_all(bytes)
                .map_err(|error| format!("write staged composer candidate file: {error}"))
        },
        |file| {
            file.sync_all()
                .map_err(|error| format!("sync staged composer candidate file: {error}"))
        },
    )
}

fn publish_durable_intent_with_io_hooks(
    session_dir: &Path,
    intent_path: &Path,
    intent: &ComposerPublishIntent,
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> Result<(), String>,
    sync: impl FnOnce(&std::fs::File) -> Result<(), String>,
) -> Result<std::fs::File, String> {
    let bytes = serde_json::to_vec_pretty(intent).map_err(|error| error.to_string())?;
    if bytes.len() > 32 * 1024 {
        return Err("composer publish intent exceeds its bounded size".into());
    }
    let temp_path = session_dir.join(format!(
        ".candidate-intent-temp-{}",
        new_entity_id("intent")
    ));
    let mut temp_guard = create_new_composer_file(&temp_path)?;
    if let Err(error) = write(&mut temp_guard, &bytes) {
        return Err(intent_temp_failure(error, temp_guard, &temp_path));
    }
    if let Err(error) = sync(&temp_guard) {
        return Err(intent_temp_failure(error, temp_guard, &temp_path));
    }
    let temp_identity = match file_identity(&temp_guard) {
        Ok(identity) => identity,
        Err(error) => return Err(intent_temp_failure(error, temp_guard, &temp_path)),
    };
    let moved = crate::platform::durable_move_file(&temp_path, intent_path);
    if let Err(error) = moved {
        let cleanup = delete_locked_file(temp_guard, &temp_path);
        return Err(match cleanup {
            Ok(()) => format!("durably replace composer publish intent: {error}"),
            Err(cleanup) => format!(
                "durably replace composer publish intent: {error}; exact temp cleanup failed: {cleanup}"
            ),
        });
    }
    drop(temp_guard);
    let (intent_guard, actual) = lock_bounded_regular_file(
        intent_path,
        session_dir,
        "composer publish intent",
        32 * 1024,
    )?;
    if file_identity(&intent_guard)? != temp_identity || actual != bytes {
        return Err("durable composer publish intent identity/content mismatch".into());
    }
    Ok(intent_guard)
}

fn intent_temp_failure(error: String, temp_guard: std::fs::File, temp_path: &Path) -> String {
    match delete_locked_file(temp_guard, temp_path) {
        Ok(()) => error,
        Err(cleanup) => format!("{error}; exact intent temp cleanup failed: {cleanup}"),
    }
}

#[cfg(windows)]
fn create_new_composer_file(path: &Path) -> Result<std::fs::File, String> {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_DELETE,
        FILE_SHARE_READ,
    };
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)
        .open(path)
        .map_err(|error| format!("create staged composer candidate file: {error}"))
}

#[cfg(not(windows))]
fn create_new_composer_file(_path: &Path) -> Result<std::fs::File, String> {
    Err("secure composer file creation currently requires Windows handle I/O".into())
}

#[cfg(windows)]
fn write_new_synced_file(path: &Path, bytes: &[u8]) -> Result<std::fs::File, String> {
    let mut file = create_new_composer_file(path)?;
    file.write_all(bytes)
        .map_err(|error| format!("write staged composer candidate file: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync staged composer candidate file: {error}"))?;
    Ok(file)
}

#[cfg(not(windows))]
fn write_new_synced_file(_path: &Path, _bytes: &[u8]) -> Result<std::fs::File, String> {
    Err("secure composer file creation currently requires Windows handle I/O".into())
}

fn validate_adoption_candidate_bytes(body: &[u8], motion_profile: &[u8]) -> Result<(), String> {
    if body.is_empty() || body.len() > MAX_COMPOSER_PNG_BYTES {
        return Err("adoption body exceeds its bounded size".into());
    }
    if image::guess_format(body).ok() != Some(image::ImageFormat::Png) {
        return Err("adoption body must be a PNG image".into());
    }
    let decoder = image::codecs::png::PngDecoder::new(Cursor::new(body))
        .map_err(|error| format!("adoption body PNG header is invalid: {error}"))?;
    let (width, height) = decoder.dimensions();
    if width != 1024 || height != 1024 || decoder.color_type() != image::ColorType::Rgba8 {
        return Err("adoption body must be exactly 1024x1024 RGBA PNG".into());
    }
    let allocation = decoder.total_bytes();
    if allocation != 1024 * 1024 * 4 || allocation > 8 * 1024 * 1024 {
        return Err("adoption body decoded allocation is invalid".into());
    }
    let mut pixels = vec![0; allocation as usize];
    decoder
        .read_image(&mut pixels)
        .map_err(|error| format!("adoption body PNG decode failed: {error}"))?;
    if !pixels.chunks_exact(4).any(|pixel| pixel[3] >= 8)
        || !pixels.chunks_exact(4).any(|pixel| pixel[3] == 0)
    {
        return Err("adoption body requires visible content and transparent background".into());
    }
    if motion_profile.is_empty() || motion_profile.len() as u64 > MAX_COMPOSER_JSON_BYTES {
        return Err("adoption motion profile exceeds its bounded size".into());
    }
    let json = std::str::from_utf8(motion_profile)
        .map_err(|error| format!("adoption motion profile is not UTF-8: {error}"))?;
    parse_motion_profile(json)?;
    Ok(())
}

fn lock_candidate_files_for(
    directory: &OwnedDirectoryGuard,
    expected_files: &[CandidateFile<'_>],
    label: &str,
) -> Result<Vec<std::fs::File>, String> {
    #[cfg(not(windows))]
    {
        let _ = (directory, expected_files, label);
        return Err("secure candidate file validation currently requires Windows handles".into());
    }
    #[cfg(windows)]
    {
        let mut files = Vec::new();
        for expected in expected_files {
            let (file, actual) = lock_bounded_regular_file(
                &directory.path.join(expected.name),
                &directory.path,
                &format!("{label} candidate {}", expected.name),
                expected.limit,
            )?;
            if actual != expected.bytes {
                return Err(format!(
                    "{label} candidate {} changed during publication",
                    expected.name
                ));
            }
            files.push(file);
        }
        validate_locked_candidate_files_for(directory, &files, expected_files, label)?;
        Ok(files)
    }
}

fn validate_locked_candidate_files_for(
    directory: &OwnedDirectoryGuard,
    files: &[std::fs::File],
    expected_files: &[CandidateFile<'_>],
    label: &str,
) -> Result<(), String> {
    if files.len() != expected_files.len() {
        return Err(format!(
            "{label} candidate does not own exactly {} locked files",
            expected_files.len()
        ));
    }
    let actual_names = collect_bounded_directory_names(
        std::fs::read_dir(&directory.path)
            .map_err(|error| format!("read locked {label} candidate: {error}"))?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|error| error.to_string())
            }),
        &format!("locked {label} candidate"),
    )?;
    let expected_names = expected_files
        .iter()
        .map(|file| file.name.to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    if actual_names != expected_names {
        return Err(format!(
            "{label} candidate file roles changed during publication"
        ));
    }
    for (index, expected) in expected_files.iter().enumerate() {
        let canonical = directory
            .path
            .join(expected.name)
            .canonicalize()
            .map_err(|error| {
                format!(
                    "resolve locked {label} candidate {}: {error}",
                    expected.name
                )
            })?;
        if canonical.parent() != Some(directory.path.as_path()) {
            return Err(format!(
                "{label} candidate {} escapes its directory",
                expected.name
            ));
        }
        let mut reader = files[index]
            .try_clone()
            .map_err(|error| error.to_string())?;
        reader
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        let mut actual = Vec::new();
        reader
            .take(expected.limit + 1)
            .read_to_end(&mut actual)
            .map_err(|error| error.to_string())?;
        if actual != expected.bytes {
            return Err(format!(
                "locked {label} candidate {} changed",
                expected.name
            ));
        }
    }
    Ok(())
}

fn ensure_locked_child(
    parent: &OwnedDirectoryGuard,
    name: &str,
    label: &str,
) -> Result<(PathBuf, OwnedDirectoryGuard), String> {
    validate_component(name, label)?;
    let child = parent.path.join(name);
    match std::fs::symlink_metadata(&child) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&child)
                .map_err(|error| format!("create {label} directory: {error}"))?;
            crate::platform::sync_existing_directory_entry(&child)?;
        }
        Err(error) => return Err(format!("inspect {label} directory: {error}")),
    }
    let guard = OwnedDirectoryGuard::open(&child, label)?;
    if guard.path.parent() != Some(parent.path.as_path()) {
        return Err(format!("{label} directory escapes its owned parent"));
    }
    Ok((guard.path.clone(), guard))
}

fn delete_locked_file(file: std::fs::File, path: &Path) -> Result<(), String> {
    mark_file_delete(&file)?;
    drop(file);
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(format!(
            "locked composer file remained after deletion: {}",
            path.display()
        ));
    }
    Ok(())
}

fn delete_published_directory(
    candidate: OwnedDirectoryGuard,
    mut files: Vec<std::fs::File>,
    intent_files: Vec<OwnedIntentFile>,
    candidate_path: &Path,
) -> Result<(), String> {
    for file in &files {
        mark_file_delete(file)?;
    }
    files.clear();
    candidate.mark_delete()?;
    drop(candidate);
    if std::fs::symlink_metadata(candidate_path).is_ok() {
        return Err("exact published composer candidate remained after deletion".into());
    }
    delete_locked_intent_files(intent_files)
}

fn delete_locked_intent_files(intent_files: Vec<OwnedIntentFile>) -> Result<(), String> {
    delete_locked_intent_files_with_hook(intent_files, |_| Ok(()))
}

fn delete_locked_intent_files_with_hook(
    intent_files: Vec<OwnedIntentFile>,
    mut before_delete: impl FnMut(&OwnedIntentFile) -> Result<(), String>,
) -> Result<(), String> {
    // Delete newest-to-oldest. Stopping at the first failure can then leave only
    // a valid immutable prefix; deleting an older phase first could create a gap
    // that exact startup recovery must preserve forever.
    for intent in intent_files.iter().rev() {
        before_delete(intent)?;
        mark_file_delete(&intent.guard)?;
    }
    let paths = intent_files
        .iter()
        .map(|intent| intent.path.clone())
        .collect::<Vec<_>>();
    drop(intent_files);
    for path in paths {
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(format!(
                "composer publish intent remained after exact deletion: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
fn intent_phase_for_path(path: &Path) -> Option<ComposerPublishPhase> {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(RESERVED_COMPOSER_INTENT_FILE) => Some(ComposerPublishPhase::Reserved),
        Some(OWNED_COMPOSER_INTENT_FILE) => Some(ComposerPublishPhase::Owned),
        Some(COMPLETE_COMPOSER_INTENT_FILE) => Some(ComposerPublishPhase::Complete),
        Some(RESERVED_ADOPTION_INTENT_FILE) => Some(ComposerPublishPhase::Reserved),
        Some(OWNED_ADOPTION_INTENT_FILE) => Some(ComposerPublishPhase::Owned),
        Some(COMPLETE_ADOPTION_INTENT_FILE) => Some(ComposerPublishPhase::Complete),
        _ => None,
    }
}

fn delete_locked_directory(
    directory: OwnedDirectoryGuard,
    mut files: Vec<std::fs::File>,
) -> Result<(), String> {
    for file in &files {
        mark_file_delete(file)?;
    }
    files.clear();
    directory.mark_delete()?;
    let path = directory.path.clone();
    drop(directory);
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err("locked composer staging directory remained after deletion".into());
    }
    Ok(())
}

#[cfg(windows)]
fn mark_file_delete(file: &std::fs::File) -> Result<(), String> {
    mark_raw_handle_delete(file.as_raw_handle(), "composer candidate file")
}

#[cfg(not(windows))]
fn mark_file_delete(_file: &std::fs::File) -> Result<(), String> {
    Err("secure composer cleanup currently requires Windows handle-relative deletion".into())
}

#[cfg(windows)]
fn mark_raw_handle_delete(
    handle: std::os::windows::io::RawHandle,
    label: &str,
) -> Result<(), String> {
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfo,
            std::ptr::from_ref(&disposition).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(format!(
            "mark locked {label} for deletion: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn validate_component(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!("invalid {label}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creation::composer::{parse_pack, ComposerPackManifest};
    use crate::creation::domain::new_entity_id;
    use crate::creation::domain::{ComposerRecipe, CreationMethod, CreationSessionStatus};
    use crate::creation::{CreationService, CreationStore};
    use crate::pets::active::{ActivePetService, BUILTIN_PET_ID};
    use crate::pets::deletion::PetDeletionService;
    use crate::pets::mutation::PetMutationGate;
    use crate::pets::pet::{IdentityMode, Species};
    use crate::pets::repository::PetRepository;
    use crate::pets::{ActivePetSession, SharedActivePetSession};
    use crate::storage::Storage;
    use image::ImageEncoder as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static COUNTER: AtomicU32 = AtomicU32::new(0);
    fn assert_no_phase_intents(session_dir: &Path) {
        for name in [
            RESERVED_COMPOSER_INTENT_FILE,
            OWNED_COMPOSER_INTENT_FILE,
            COMPLETE_COMPOSER_INTENT_FILE,
        ] {
            assert!(
                !session_dir.join(name).exists(),
                "immutable intent phase remained: {name}"
            );
        }
    }

    fn assert_intent_prefix(session_dir: &Path, highest: ComposerPublishPhase) {
        let expected = match highest {
            ComposerPublishPhase::Reserved => [true, false, false],
            ComposerPublishPhase::Owned => [true, true, false],
            ComposerPublishPhase::Complete => [true, true, true],
        };
        for ((name, exists), expected) in [
            RESERVED_COMPOSER_INTENT_FILE,
            OWNED_COMPOSER_INTENT_FILE,
            COMPLETE_COMPOSER_INTENT_FILE,
        ]
        .into_iter()
        .map(|name| (name, session_dir.join(name).exists()))
        .zip(expected)
        {
            assert_eq!(exists, expected, "unexpected phase presence for {name}");
        }
    }

    struct ComposerCandidateHarness {
        root: PathBuf,
        storage: Arc<Mutex<Storage>>,
        service: CreationService,
        session_id: String,
        recipe: ComposerRecipe,
    }

    impl Drop for ComposerCandidateHarness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl ComposerCandidateHarness {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let root = std::env::temp_dir().join(format!(
                "desktop-pet-composer-candidate-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let storage = Arc::new(Mutex::new(Storage::open(&root.join("pets")).unwrap()));
            let active_session: SharedActivePetSession =
                Arc::new(Mutex::new(ActivePetSession::new()));
            active_session
                .lock()
                .unwrap()
                .set_active(BUILTIN_PET_ID.into())
                .unwrap();
            let gate = Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60)));
            let active = Arc::new(ActivePetService::new(
                storage.clone(),
                active_session,
                root.join("pets"),
                gate.clone(),
            ));
            let deletion = Arc::new(PetDeletionService::new(
                storage.clone(),
                active,
                root.clone(),
                gate.clone(),
            ));
            let content =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/creation-content");
            let content_root = crate::creation::content::test_content_root(&content).unwrap();
            let service =
                CreationService::new(storage.clone(), root.clone(), deletion, content_root, gate);
            let session = service.start(CreationMethod::Composer).unwrap();
            let pack: ComposerPackManifest = parse_pack(
                &std::fs::read_to_string(content.join("composer/cat-cute-v1/manifest.json"))
                    .unwrap(),
            )
            .unwrap();
            let body = pack
                .bodies
                .iter()
                .find(|body| body.id == "body-round")
                .unwrap();
            let recipe = ComposerRecipe {
                recipe_version: 1,
                pack_id: pack.pack_id,
                pack_version: pack.pack_version,
                layer_contract_version: pack.layer_contract_version,
                body_id: body.id.clone(),
                ears_id: body.defaults.ears_id.clone(),
                eyes_id: body.defaults.eyes_id.clone(),
                muzzle_id: body.defaults.muzzle_id.clone(),
                tail_id: body.defaults.tail_id.clone(),
                color_id: body.defaults.color_id.clone(),
                pattern_id: body.defaults.pattern_id.clone(),
            };
            service
                .save_composer_recipe(&session.session_id, &recipe, "ears")
                .unwrap();
            Self {
                root,
                storage,
                service,
                session_id: session.session_id,
                recipe,
            }
        }

        fn candidate_count(&self) -> i64 {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT COUNT(*) FROM appearance_variants WHERE session_id=?1",
                    [&self.session_id],
                    |row| row.get(0),
                )
                .unwrap()
        }
    }

    fn png_b64(width: u32, height: u32, color: image::ColorType, visible: bool) -> String {
        let channels = color.bytes_per_pixel() as usize;
        let mut pixels = vec![0; width as usize * height as usize * channels];
        if visible && color == image::ColorType::Rgba8 {
            let center = ((height / 2 * width + width / 2) * 4) as usize;
            pixels[center..center + 4].copy_from_slice(&[100, 80, 60, 255]);
        }
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&pixels, width, height, color.into())
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn valid_candidate_b64() -> String {
        png_b64(1024, 1024, image::ColorType::Rgba8, true)
    }

    fn valid_candidate_b64_with_compression(
        compression: image::codecs::png::CompressionType,
    ) -> String {
        let mut pixels = vec![0; 1024 * 1024 * 4];
        let center = ((512 * 1024 + 512) * 4) as usize;
        pixels[center..center + 4].copy_from_slice(&[100, 80, 60, 255]);
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new_with_quality(
            &mut bytes,
            compression,
            image::codecs::png::FilterType::Adaptive,
        )
        .write_image(&pixels, 1024, 1024, image::ExtendedColorType::Rgba8)
        .unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn production_profile(recipe: &ComposerRecipe) -> MotionProfileV1 {
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/creation-content");
        let root = crate::creation::content::test_content_root(&content).unwrap();
        let pack = crate::creation::composer::load_production_pack_manifest(&root).unwrap();
        crate::creation::composer::motion_profile_for_recipe(&pack, recipe).unwrap()
    }

    fn reserved_intent_fixture(
        session_id: &str,
        body: &[u8],
        profile: &MotionProfileV1,
        recipe: &ComposerRecipe,
    ) -> ComposerPublishIntent {
        ComposerPublishIntent {
            version: 2,
            phase: Some(ComposerPublishPhase::Reserved),
            session_id: session_id.to_owned(),
            stage_name: ".candidate-stage-intent-io-test".into(),
            body_sha256: sha256_hex(body),
            profile_sha256: sha256_hex(&serde_json::to_vec_pretty(profile).unwrap()),
            recipe_sha256: sha256_hex(&serde_json::to_vec_pretty(recipe).unwrap()),
            directory_identity: None,
            file_identities: None,
        }
    }

    fn assert_no_intent_temps(session_dir: &Path) {
        let temps = std::fs::read_dir(session_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| {
                name.to_string_lossy()
                    .starts_with(".candidate-intent-temp-")
            })
            .collect::<Vec<_>>();
        assert!(temps.is_empty(), "leaked intent temps: {temps:?}");
    }

    fn write_v2_owned_intent(
        session_dir: &Path,
        session_id: &str,
        stage_name: &str,
        identity: FileIdentity,
        body: &[u8],
        profile: &MotionProfileV1,
        recipe: &ComposerRecipe,
    ) {
        let profile = serde_json::to_vec_pretty(profile).unwrap();
        let recipe = serde_json::to_vec_pretty(recipe).unwrap();
        let reserved = serde_json::json!({
            "version": 2,
            "phase": "reserved",
            "sessionId": session_id,
            "stageName": stage_name,
            "bodySha256": sha256_hex(body),
            "profileSha256": sha256_hex(&profile),
            "recipeSha256": sha256_hex(&recipe),
            "directoryIdentity": null,
            "fileIdentities": null,
        });
        let owned = serde_json::json!({
            "version": 2,
            "phase": "owned",
            "sessionId": session_id,
            "stageName": stage_name,
            "bodySha256": sha256_hex(body),
            "profileSha256": sha256_hex(&profile),
            "recipeSha256": sha256_hex(&recipe),
            "directoryIdentity": {
                "volumeSerial": identity.volume_serial,
                "fileIndex": identity.file_index,
            },
            "fileIdentities": null,
        });
        std::fs::write(
            session_dir.join(RESERVED_COMPOSER_INTENT_FILE),
            serde_json::to_vec_pretty(&reserved).unwrap(),
        )
        .unwrap();
        std::fs::write(
            session_dir.join(OWNED_COMPOSER_INTENT_FILE),
            serde_json::to_vec_pretty(&owned).unwrap(),
        )
        .unwrap();
    }

    fn write_v2_reserved_intent(
        session_dir: &Path,
        session_id: &str,
        stage_name: &str,
        body: &[u8],
        profile: &MotionProfileV1,
        recipe: &ComposerRecipe,
    ) {
        let profile = serde_json::to_vec_pretty(profile).unwrap();
        let recipe = serde_json::to_vec_pretty(recipe).unwrap();
        let reserved = serde_json::json!({
            "version": 2,
            "phase": "reserved",
            "sessionId": session_id,
            "stageName": stage_name,
            "bodySha256": sha256_hex(body),
            "profileSha256": sha256_hex(&profile),
            "recipeSha256": sha256_hex(&recipe),
            "directoryIdentity": null,
            "fileIdentities": null,
        });
        std::fs::write(
            session_dir.join(RESERVED_COMPOSER_INTENT_FILE),
            serde_json::to_vec_pretty(&reserved).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn candidate_directory_name_scan_stops_after_the_fourth_entry() {
        let seen = std::cell::Cell::new(0_usize);
        let names = std::iter::from_fn(|| {
            let next = seen.get() + 1;
            assert!(next <= 4, "bounded scan read a fifth directory entry");
            seen.set(next);
            Some(Ok(std::ffi::OsString::from(format!("entry-{next}"))))
        });

        let error = collect_bounded_directory_names(names, "test candidate directory")
            .expect_err("four entries must exceed the three-file contract");

        assert!(error.contains("more than three"), "{error}");
        assert_eq!(seen.get(), 4);
    }

    #[test]
    fn composer_candidate_rejects_invalid_png_shapes_without_changing_draft() {
        let invalid = [
            (
                "wrong dimensions",
                png_b64(512, 512, image::ColorType::Rgba8, true),
            ),
            (
                "non rgba",
                png_b64(1024, 1024, image::ColorType::Rgb8, false),
            ),
            (
                "empty alpha",
                png_b64(1024, 1024, image::ColorType::Rgba8, false),
            ),
        ];
        for (label, encoded) in invalid {
            let test = ComposerCandidateHarness::new();
            assert!(
                test.service
                    .store_composer_candidate(&test.session_id, Some(&encoded))
                    .is_err(),
                "accepted {label}"
            );
            let snapshot = test.service.snapshot(&test.session_id).unwrap();
            assert_eq!(snapshot.status, CreationSessionStatus::Draft);
            assert_eq!(snapshot.recipe, Some(test.recipe.clone()));
            assert_eq!(test.candidate_count(), 0);
        }
    }

    #[test]
    fn composer_candidate_rejects_non_png_opaque_and_oversized_base64_before_state_change() {
        for encoded in [
            base64::engine::general_purpose::STANDARD.encode(b"not a png"),
            {
                let pixels = vec![255; 1024 * 1024 * 4];
                let mut bytes = Vec::new();
                image::codecs::png::PngEncoder::new(&mut bytes)
                    .write_image(&pixels, 1024, 1024, image::ExtendedColorType::Rgba8)
                    .unwrap();
                base64::engine::general_purpose::STANDARD.encode(bytes)
            },
            "A".repeat(15 * 1024 * 1024),
        ] {
            let test = ComposerCandidateHarness::new();
            assert!(test
                .service
                .store_composer_candidate(&test.session_id, Some(&encoded))
                .is_err());
            assert_eq!(
                test.service.snapshot(&test.session_id).unwrap().status,
                CreationSessionStatus::Draft
            );
            assert_eq!(test.candidate_count(), 0);
        }
    }

    #[test]
    fn composer_candidate_rejects_wrong_method_missing_recipe_and_terminal_sessions() {
        let encoded = valid_candidate_b64();

        let missing_recipe = ComposerCandidateHarness::new();
        missing_recipe
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "DELETE FROM composer_recipes WHERE session_id=?1",
                [&missing_recipe.session_id],
            )
            .unwrap();
        assert!(missing_recipe
            .service
            .store_composer_candidate(&missing_recipe.session_id, Some(&encoded))
            .is_err());
        assert_eq!(missing_recipe.candidate_count(), 0);

        let wrong_method = ComposerCandidateHarness::new();
        wrong_method
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions SET method='upload' WHERE session_id=?1",
                [&wrong_method.session_id],
            )
            .unwrap();
        assert!(wrong_method
            .service
            .store_composer_candidate(&wrong_method.session_id, Some(&encoded))
            .is_err());
        assert_eq!(wrong_method.candidate_count(), 0);

        let terminal = ComposerCandidateHarness::new();
        terminal
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions
                 SET status='completed', last_stable_status='completed', current_step='completed'
                 WHERE session_id=?1",
                [&terminal.session_id],
            )
            .unwrap();
        assert!(terminal
            .service
            .store_composer_candidate(&terminal.session_id, Some(&encoded))
            .is_err());
        assert_eq!(terminal.candidate_count(), 0);
    }

    #[test]
    fn composer_candidate_projects_only_trusted_body_motion_semantics() {
        let test = ComposerCandidateHarness::new();
        let encoded = valid_candidate_b64();
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/creation-content");
        let root = crate::creation::content::test_content_root(&content).unwrap();
        let pack = crate::creation::composer::load_production_pack(&root).unwrap();
        let expected =
            crate::creation::composer::motion_profile_for_recipe(&pack, &test.recipe).unwrap();

        let projection = test
            .service
            .store_composer_candidate(&test.session_id, Some(&encoded))
            .unwrap();

        assert_eq!(projection.motion_profile, expected);
        assert!(projection.body_url.starts_with("data:image/png;base64,"));
        assert!(!projection
            .body_url
            .contains(test.root.to_string_lossy().as_ref()));
        let profile_path = test
            .root
            .join("creation-sessions")
            .join(&test.session_id)
            .join("candidate/motion-profile.json");
        let stored = parse_motion_profile(&std::fs::read_to_string(profile_path).unwrap()).unwrap();
        assert_eq!(stored, expected);
    }

    #[test]
    fn composer_candidate_is_idempotent_and_database_failure_removes_only_this_attempt() {
        let test = ComposerCandidateHarness::new();
        let encoded = valid_candidate_b64();
        let first = test
            .service
            .store_composer_candidate(&test.session_id, Some(&encoded))
            .unwrap();
        let second = test
            .service
            .store_composer_candidate(&test.session_id, None)
            .unwrap();
        assert_eq!(first.snapshot.candidate_id, second.snapshot.candidate_id);
        assert_eq!(test.candidate_count(), 1);
        assert_no_phase_intents(&test.root.join("creation-sessions").join(&test.session_id));

        let failing = ComposerCandidateHarness::new();
        failing
            .storage
            .lock()
            .unwrap()
            .db
            .execute_batch(
                "CREATE TRIGGER fail_local_candidate BEFORE INSERT ON appearance_variants
                 WHEN NEW.job_id IS NULL
                 BEGIN SELECT RAISE(ABORT, 'forced local candidate failure'); END;",
            )
            .unwrap();
        assert!(failing
            .service
            .store_composer_candidate(&failing.session_id, Some(&encoded))
            .is_err());
        assert!(!failing
            .root
            .join("creation-sessions")
            .join(&failing.session_id)
            .join("candidate")
            .exists());
        assert_eq!(
            failing
                .service
                .snapshot(&failing.session_id)
                .unwrap()
                .status,
            CreationSessionStatus::Draft
        );
        assert_no_phase_intents(
            &failing
                .root
                .join("creation-sessions")
                .join(&failing.session_id),
        );
    }

    #[test]
    fn interrupted_commit_cleanup_leaves_a_recoverable_intent_prefix_for_every_phase() {
        for failure_phase in [
            ComposerPublishPhase::Complete,
            ComposerPublishPhase::Owned,
            ComposerPublishPhase::Reserved,
        ] {
            let test = ComposerCandidateHarness::new();
            let body = decode_composer_png(&valid_candidate_b64()).unwrap().bytes;
            let profile = production_profile(&test.recipe);
            let mut published = publish_composer_candidate(
                &test.root,
                &test.session_id,
                &body,
                &profile,
                &test.recipe,
            )
            .unwrap();
            CreationStore::new(test.storage.clone())
                .record_local_candidate(
                    &test.session_id,
                    &published.body_path,
                    &published.motion_profile_path,
                )
                .unwrap();
            published.simulate_intent_cleanup_failure_at(failure_phase);

            published.commit();
            drop(published);

            let session = test.root.join("creation-sessions").join(&test.session_id);
            assert_intent_prefix(&session, failure_phase);
            let first = test.service.recover_composer_orphans().unwrap();
            let second = test.service.recover_composer_orphans().unwrap();
            assert_eq!(first.recovered_count, 0, "{failure_phase:?}");
            assert!(
                first.warnings.is_empty(),
                "{failure_phase:?}: {:?}",
                first.warnings
            );
            assert_eq!(second.recovered_count, 0, "{failure_phase:?}");
            assert!(second.warnings.is_empty(), "{failure_phase:?}");
            assert_no_phase_intents(&session);
            assert!(session.join("candidate/body.png").is_file());
            assert_eq!(test.candidate_count(), 1);
            assert_eq!(
                test.service.snapshot(&test.session_id).unwrap().status,
                CreationSessionStatus::CandidateReady
            );
        }
    }

    #[test]
    fn interrupted_rollback_cleanup_leaves_a_recoverable_intent_prefix_for_every_phase() {
        for failure_phase in [
            ComposerPublishPhase::Complete,
            ComposerPublishPhase::Owned,
            ComposerPublishPhase::Reserved,
        ] {
            let test = ComposerCandidateHarness::new();
            let body = decode_composer_png(&valid_candidate_b64()).unwrap().bytes;
            let profile = production_profile(&test.recipe);
            let mut published = publish_composer_candidate(
                &test.root,
                &test.session_id,
                &body,
                &profile,
                &test.recipe,
            )
            .unwrap();
            published.simulate_intent_cleanup_failure_at(failure_phase);

            assert!(published.rollback().is_err(), "{failure_phase:?}");
            drop(published);

            let session = test.root.join("creation-sessions").join(&test.session_id);
            assert!(!session.join("candidate").exists(), "{failure_phase:?}");
            assert_intent_prefix(&session, failure_phase);
            let first = test.service.recover_composer_orphans().unwrap();
            let second = test.service.recover_composer_orphans().unwrap();
            assert_eq!(first.recovered_count, 1, "{failure_phase:?}");
            assert!(
                first.warnings.is_empty(),
                "{failure_phase:?}: {:?}",
                first.warnings
            );
            assert_eq!(second.recovered_count, 0, "{failure_phase:?}");
            assert!(second.warnings.is_empty(), "{failure_phase:?}");
            assert_no_phase_intents(&session);
            assert_eq!(test.candidate_count(), 0);
            assert_eq!(
                test.service.snapshot(&test.session_id).unwrap().status,
                CreationSessionStatus::Draft
            );
        }
    }

    #[test]
    fn retryable_finalization_reuses_the_exact_composer_candidate_for_preview() {
        let test = ComposerCandidateHarness::new();
        let encoded = valid_candidate_b64();
        test.service
            .store_composer_candidate(&test.session_id, Some(&encoded))
            .unwrap();
        test.storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE creation_sessions
                 SET status='retryableFailure', last_stable_status='candidateReady',
                     current_step='review', error='desktop unavailable'
                 WHERE session_id=?1",
                [&test.session_id],
            )
            .unwrap();

        let projection = test
            .service
            .store_composer_candidate(&test.session_id, None)
            .unwrap();

        assert_eq!(
            projection.snapshot.status,
            CreationSessionStatus::RetryableFailure
        );
        assert_eq!(
            projection.snapshot.last_stable_status,
            CreationSessionStatus::CandidateReady
        );
        assert_eq!(test.candidate_count(), 1);
    }

    #[test]
    fn existing_candidate_projection_reads_db_owned_files_without_png_byte_identity() {
        let test = ComposerCandidateHarness::new();
        let original =
            valid_candidate_b64_with_compression(image::codecs::png::CompressionType::Best);
        let equivalent =
            valid_candidate_b64_with_compression(image::codecs::png::CompressionType::Fast);
        assert_ne!(original, equivalent);
        test.service
            .store_composer_candidate(&test.session_id, Some(&original))
            .unwrap();

        let projection = test
            .service
            .store_composer_candidate(&test.session_id, None)
            .expect("durable projection must not depend on re-exported PNG bytes");

        assert_eq!(
            projection.body_url,
            format!("data:image/png;base64,{original}")
        );
        assert_eq!(test.candidate_count(), 1);
    }

    #[test]
    fn startup_recovery_converges_a_db_candidate_whose_durable_directory_is_missing() {
        let test = ComposerCandidateHarness::new();
        test.service
            .store_composer_candidate(&test.session_id, Some(&valid_candidate_b64()))
            .unwrap();
        std::fs::remove_dir_all(
            test.root
                .join("creation-sessions")
                .join(&test.session_id)
                .join("candidate"),
        )
        .unwrap();

        let report = test.service.recover_composer_orphans().unwrap();
        let restored = test.service.snapshot(&test.session_id).unwrap();

        assert_eq!(restored.status, CreationSessionStatus::Draft);
        assert_eq!(test.candidate_count(), 0);
        assert_eq!(report.recovered_count, 1);
        assert_eq!(
            test.service
                .recover_composer_orphans()
                .unwrap()
                .recovered_count,
            0
        );
    }

    #[test]
    fn startup_recovery_keeps_a_valid_db_candidate_and_clears_only_its_stale_intent() {
        let test = ComposerCandidateHarness::new();
        let decoded = decode_composer_png(&valid_candidate_b64()).unwrap();
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/creation-content");
        let root = crate::creation::content::test_content_root(&content).unwrap();
        let pack = crate::creation::composer::load_production_pack(&root).unwrap();
        let profile =
            crate::creation::composer::motion_profile_for_recipe(&pack, &test.recipe).unwrap();
        let mut published = publish_composer_candidate(
            &test.root,
            &test.session_id,
            &decoded.bytes,
            &profile,
            &test.recipe,
        )
        .unwrap();
        CreationStore::new(test.storage.clone())
            .record_local_candidate(
                &test.session_id,
                &published.body_path,
                &published.motion_profile_path,
            )
            .unwrap();
        published.simulate_process_exit_before_database_commit();
        drop(published);

        let first = test.service.recover_composer_orphans().unwrap();
        let second = test.service.recover_composer_orphans().unwrap();

        assert_eq!(first.recovered_count, 0, "{:?}", first.warnings);
        assert!(first.warnings.is_empty());
        assert_eq!(second.recovered_count, 0);
        assert_eq!(test.candidate_count(), 1);
        assert_eq!(
            test.service.snapshot(&test.session_id).unwrap().status,
            CreationSessionStatus::CandidateReady
        );
        let session = test.root.join("creation-sessions").join(&test.session_id);
        assert!(session.join("candidate/body.png").is_file());
        assert_no_phase_intents(&session);
    }

    #[test]
    fn startup_recovery_cleans_a_crash_after_reserved_intent_and_empty_stage() {
        let test = ComposerCandidateHarness::new();
        let body = decode_composer_png(&valid_candidate_b64()).unwrap().bytes;
        let profile = production_profile(&test.recipe);
        let session = test.root.join("creation-sessions").join(&test.session_id);
        std::fs::create_dir_all(&session).unwrap();
        let stage_name = ".candidate-stage-reserved-crash";
        let stage = session.join(stage_name);
        std::fs::create_dir(&stage).unwrap();
        write_v2_reserved_intent(
            &session,
            &test.session_id,
            stage_name,
            &body,
            &profile,
            &test.recipe,
        );

        let first = test.service.recover_composer_orphans().unwrap();
        let second = test.service.recover_composer_orphans().unwrap();

        assert_eq!(first.recovered_count, 1, "{:?}", first.warnings);
        assert!(first.warnings.is_empty());
        assert_eq!(second.recovered_count, 0);
        assert!(!stage.exists());
        assert_no_phase_intents(&session);
    }

    #[test]
    fn startup_recovery_preserves_an_unknown_phase_sentinel_and_warns() {
        let test = ComposerCandidateHarness::new();
        let session = test.root.join("creation-sessions").join(&test.session_id);
        std::fs::create_dir_all(&session).unwrap();
        let sentinel = session.join(OWNED_COMPOSER_INTENT_FILE);
        std::fs::write(&sentinel, b"unknown-phase-sentinel").unwrap();

        let report = test.service.recover_composer_orphans().unwrap();

        assert_eq!(report.recovered_count, 0);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("intent") || warning.contains("prefix")),
            "{:?}",
            report.warnings
        );
        assert_eq!(std::fs::read(sentinel).unwrap(), b"unknown-phase-sentinel");
    }

    #[test]
    fn startup_recovery_preserves_a_legacy_single_intent_and_warns() {
        let test = ComposerCandidateHarness::new();
        let session = test.root.join("creation-sessions").join(&test.session_id);
        std::fs::create_dir_all(&session).unwrap();
        let sentinel = session.join(LEGACY_COMPOSER_INTENT_FILE);
        std::fs::write(&sentinel, b"legacy-intent-sentinel").unwrap();

        let report = test.service.recover_composer_orphans().unwrap();

        assert_eq!(report.recovered_count, 0);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("legacy") && warning.contains("preserved")),
            "{:?}",
            report.warnings
        );
        assert_eq!(std::fs::read(sentinel).unwrap(), b"legacy-intent-sentinel");
    }

    #[test]
    fn startup_recovery_preserves_inconsistent_immutable_phase_records() {
        let test = ComposerCandidateHarness::new();
        let body = decode_composer_png(&valid_candidate_b64()).unwrap().bytes;
        let profile = production_profile(&test.recipe);
        let session = test.root.join("creation-sessions").join(&test.session_id);
        let stage_name = ".candidate-stage-inconsistent-prefix";
        let stage = session.join(stage_name);
        std::fs::create_dir_all(&stage).unwrap();
        let identity = OwnedDirectoryGuard::open(&stage, "inconsistent staging")
            .unwrap()
            .identity;
        write_v2_owned_intent(
            &session,
            &test.session_id,
            stage_name,
            identity,
            &body,
            &profile,
            &test.recipe,
        );
        let owned_path = session.join(OWNED_COMPOSER_INTENT_FILE);
        let mut owned: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&owned_path).unwrap()).unwrap();
        owned["bodySha256"] = serde_json::Value::String("f".repeat(64));
        std::fs::write(&owned_path, serde_json::to_vec_pretty(&owned).unwrap()).unwrap();

        let report = test.service.recover_composer_orphans().unwrap();

        assert_eq!(report.recovered_count, 0);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("immutable prefix")),
            "{:?}",
            report.warnings
        );
        assert!(stage.is_dir());
        assert!(session.join(RESERVED_COMPOSER_INTENT_FILE).is_file());
        assert!(owned_path.is_file());
    }

    #[test]
    fn startup_recovery_cleans_owned_partial_staging_for_zero_one_two_and_partial_files() {
        for file_case in 0..4 {
            let test = ComposerCandidateHarness::new();
            let body = decode_composer_png(&valid_candidate_b64()).unwrap().bytes;
            let profile = production_profile(&test.recipe);
            let profile_json = serde_json::to_vec_pretty(&profile).unwrap();
            let session = test.root.join("creation-sessions").join(&test.session_id);
            std::fs::create_dir_all(&session).unwrap();
            let stage_name = format!(".candidate-stage-partial-{file_case}");
            let stage = session.join(&stage_name);
            std::fs::create_dir(&stage).unwrap();
            let identity = OwnedDirectoryGuard::open(&stage, "partial staging")
                .unwrap()
                .identity;
            match file_case {
                0 => {}
                1 => std::fs::write(stage.join("body.png"), &body).unwrap(),
                2 => {
                    std::fs::write(stage.join("body.png"), &body).unwrap();
                    std::fs::write(stage.join("motion-profile.json"), &profile_json).unwrap();
                }
                3 => std::fs::write(stage.join("body.png"), b"half-written").unwrap(),
                _ => unreachable!(),
            }
            write_v2_owned_intent(
                &session,
                &test.session_id,
                &stage_name,
                identity,
                &body,
                &profile,
                &test.recipe,
            );

            let first = test.service.recover_composer_orphans().unwrap();
            let second = test.service.recover_composer_orphans().unwrap();

            assert_eq!(
                first.recovered_count, 1,
                "case {file_case}: {:?}",
                first.warnings
            );
            assert_eq!(second.recovered_count, 0, "case {file_case}");
            assert!(!stage.exists(), "case {file_case}");
            assert_no_phase_intents(&session);
        }
    }

    #[test]
    fn startup_recovery_preserves_a_stage_whose_file_id_differs_from_the_owned_intent() {
        let test = ComposerCandidateHarness::new();
        let body = decode_composer_png(&valid_candidate_b64()).unwrap().bytes;
        let profile = production_profile(&test.recipe);
        let session = test.root.join("creation-sessions").join(&test.session_id);
        std::fs::create_dir_all(&session).unwrap();
        let stage_name = ".candidate-stage-owned-id";
        let stage = session.join(stage_name);
        let displaced = session.join("displaced-owned-stage");
        std::fs::create_dir(&stage).unwrap();
        let original_identity = OwnedDirectoryGuard::open(&stage, "original staging")
            .unwrap()
            .identity;
        std::fs::rename(&stage, &displaced).unwrap();
        std::fs::create_dir(&stage).unwrap();
        std::fs::write(stage.join("sentinel.txt"), b"replacement").unwrap();
        write_v2_owned_intent(
            &session,
            &test.session_id,
            stage_name,
            original_identity,
            &body,
            &profile,
            &test.recipe,
        );

        let report = test.service.recover_composer_orphans().unwrap();

        assert_eq!(report.recovered_count, 0);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("identity")));
        assert_eq!(
            std::fs::read(stage.join("sentinel.txt")).unwrap(),
            b"replacement"
        );
        assert!(displaced.is_dir());
    }

    #[test]
    fn stored_candidate_rejects_oversized_sparse_files_before_unbounded_reads() {
        for (name, limit) in [
            ("body.png", 10 * 1024 * 1024_u64),
            ("motion-profile.json", 64 * 1024_u64),
            ("recipe.json", 64 * 1024_u64),
        ] {
            let test = ComposerCandidateHarness::new();
            test.service
                .store_composer_candidate(&test.session_id, Some(&valid_candidate_b64()))
                .unwrap();
            let path = test
                .root
                .join("creation-sessions")
                .join(&test.session_id)
                .join("candidate")
                .join(name);
            let file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            file.set_len(limit + 1).unwrap();
            drop(file);

            let error = test
                .service
                .store_composer_candidate(&test.session_id, None)
                .unwrap_err();

            assert!(error.contains(name), "{name}: {error}");
            assert!(error.contains("limit"), "{name}: {error}");
        }
    }

    #[test]
    fn startup_recovery_bounds_sparse_candidate_reads_and_preserves_the_db_owned_directory() {
        let test = ComposerCandidateHarness::new();
        let body = decode_composer_png(&valid_candidate_b64()).unwrap().bytes;
        let profile = production_profile(&test.recipe);
        let mut published =
            publish_composer_candidate(&test.root, &test.session_id, &body, &profile, &test.recipe)
                .unwrap();
        CreationStore::new(test.storage.clone())
            .record_local_candidate(
                &test.session_id,
                &published.body_path,
                &published.motion_profile_path,
            )
            .unwrap();
        published.simulate_process_exit_before_database_commit();
        drop(published);
        let candidate = test
            .root
            .join("creation-sessions")
            .join(&test.session_id)
            .join("candidate");
        let body_file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(candidate.join("body.png"))
            .unwrap();
        body_file
            .set_len(MAX_COMPOSER_PNG_BYTES as u64 + 1)
            .unwrap();
        drop(body_file);

        let report = test.service.recover_composer_orphans().unwrap();

        assert_eq!(report.recovered_count, 0);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("body.png") && warning.contains("limit")),
            "{:?}",
            report.warnings
        );
        assert!(candidate.join("body.png").exists());
        assert!(candidate
            .parent()
            .unwrap()
            .join(COMPLETE_COMPOSER_INTENT_FILE)
            .exists());
    }

    #[test]
    fn startup_recovery_cleans_stale_intent_for_a_completed_local_composer_candidate() {
        let test = ComposerCandidateHarness::new();
        let body = decode_composer_png(&valid_candidate_b64()).unwrap().bytes;
        let profile = production_profile(&test.recipe);
        let mut published =
            publish_composer_candidate(&test.root, &test.session_id, &body, &profile, &test.recipe)
                .unwrap();
        CreationStore::new(test.storage.clone())
            .record_local_candidate(
                &test.session_id,
                &published.body_path,
                &published.motion_profile_path,
            )
            .unwrap();
        published.simulate_process_exit_before_database_commit();
        drop(published);
        test.storage
            .lock()
            .unwrap()
            .db
            .execute_batch(&format!(
                "UPDATE appearance_variants SET accepted=1 WHERE session_id='{}';
                 UPDATE creation_sessions SET status='completed', last_stable_status='completed',
                   current_step='completed' WHERE session_id='{}';",
                test.session_id, test.session_id
            ))
            .unwrap();
        let session = test.root.join("creation-sessions").join(&test.session_id);
        for name in [
            RESERVED_COMPOSER_INTENT_FILE,
            OWNED_COMPOSER_INTENT_FILE,
            COMPLETE_COMPOSER_INTENT_FILE,
        ] {
            assert!(session.join(name).is_file(), "missing intent phase {name}");
        }

        let report = test.service.recover_composer_orphans().unwrap();

        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_no_phase_intents(&session);
        assert!(session.join("candidate/body.png").is_file());
        assert_eq!(
            test.service.snapshot(&test.session_id).unwrap().status,
            CreationSessionStatus::Completed
        );
    }

    #[test]
    fn committed_recovery_clears_the_intent_but_preserves_an_unknown_reused_stage_name() {
        let test = ComposerCandidateHarness::new();
        let body = decode_composer_png(&valid_candidate_b64()).unwrap().bytes;
        let profile = production_profile(&test.recipe);
        let mut published =
            publish_composer_candidate(&test.root, &test.session_id, &body, &profile, &test.recipe)
                .unwrap();
        CreationStore::new(test.storage.clone())
            .record_local_candidate(
                &test.session_id,
                &published.body_path,
                &published.motion_profile_path,
            )
            .unwrap();
        let session = test.root.join("creation-sessions").join(&test.session_id);
        let intent: ComposerPublishIntent = serde_json::from_slice(
            &std::fs::read(session.join(COMPLETE_COMPOSER_INTENT_FILE)).unwrap(),
        )
        .unwrap();
        published.simulate_process_exit_before_database_commit();
        drop(published);
        let unknown_stage = session.join(intent.stage_name);
        std::fs::create_dir(&unknown_stage).unwrap();
        std::fs::write(unknown_stage.join("sentinel.txt"), b"unknown").unwrap();

        let report = test.service.recover_composer_orphans().unwrap();

        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_no_phase_intents(&session);
        assert_eq!(
            std::fs::read(unknown_stage.join("sentinel.txt")).unwrap(),
            b"unknown"
        );
        assert!(session.join("candidate/body.png").is_file());
    }

    #[cfg(windows)]
    #[test]
    fn local_candidate_storage_rejects_a_non_unicode_windows_path_instead_of_lossy_replacement() {
        use std::os::windows::ffi::OsStringExt;
        let test = ComposerCandidateHarness::new();
        let invalid =
            std::ffi::OsString::from_wide(&[b'n' as u16, b'o' as u16, b'n' as u16, 0xd800]);
        let candidate = test.root.join(invalid).join("candidate");
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(candidate.join("body.png"), b"body").unwrap();
        std::fs::write(candidate.join("motion-profile.json"), b"{}").unwrap();

        let result = CreationStore::new(test.storage.clone()).record_local_candidate(
            &test.session_id,
            &candidate.join("body.png"),
            &candidate.join("motion-profile.json"),
        );

        assert!(result.is_err(), "non-Unicode path was stored lossily");
    }

    #[test]
    fn startup_recovery_removes_an_exact_composer_orphan_left_before_database_commit() {
        let test = ComposerCandidateHarness::new();
        let encoded = valid_candidate_b64();
        let decoded = decode_composer_png(&encoded).unwrap();
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/creation-content");
        let root = crate::creation::content::test_content_root(&content).unwrap();
        let pack = crate::creation::composer::load_production_pack(&root).unwrap();
        let profile =
            crate::creation::composer::motion_profile_for_recipe(&pack, &test.recipe).unwrap();
        let mut published = publish_composer_candidate(
            &test.root,
            &test.session_id,
            &decoded.bytes,
            &profile,
            &test.recipe,
        )
        .unwrap();
        published.simulate_process_exit_before_database_commit();
        drop(published);
        assert_eq!(test.candidate_count(), 0);

        let recovery = test.service.recover_composer_orphans().unwrap();
        assert_eq!(recovery.recovered_count, 1, "{:?}", recovery.warnings);
        assert!(recovery.warnings.is_empty());
        assert_eq!(
            test.service.snapshot(&test.session_id).unwrap().status,
            CreationSessionStatus::Draft
        );
        assert!(!test
            .root
            .join("creation-sessions")
            .join(&test.session_id)
            .join("candidate")
            .exists());
        test.service
            .store_composer_candidate(&test.session_id, Some(&encoded))
            .unwrap();
    }

    #[test]
    fn post_rename_relock_failure_moves_to_exact_recovery_and_allows_retry() {
        let test = ComposerCandidateHarness::new();
        let encoded = valid_candidate_b64();
        let decoded = decode_composer_png(&encoded).unwrap();
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/creation-content");
        let content_root = crate::creation::content::test_content_root(&content).unwrap();
        let pack = crate::creation::composer::load_production_pack(&content_root).unwrap();
        let profile =
            crate::creation::composer::motion_profile_for_recipe(&pack, &test.recipe).unwrap();
        let candidate = test
            .root
            .join("creation-sessions")
            .join(&test.session_id)
            .join("candidate");
        let result = publish_composer_candidate_with_post_rename_hook(
            &test.root,
            &test.session_id,
            &decoded.bytes,
            &profile,
            &test.recipe,
            || Err("forced post-rename validation failure".into()),
        );

        assert!(result.is_err());
        assert!(!candidate.exists());
        let recovery = test.service.recover_composer_orphans().unwrap();
        assert_eq!(recovery.recovered_count, 0, "{:?}", recovery.warnings);
        test.service
            .store_composer_candidate(&test.session_id, Some(&encoded))
            .unwrap();
    }

    #[test]
    fn startup_recovery_removes_only_exact_owned_staging_and_preserves_unknown_sentinels() {
        let test = ComposerCandidateHarness::new();
        let decoded = decode_composer_png(&valid_candidate_b64()).unwrap();
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/creation-content");
        let root = crate::creation::content::test_content_root(&content).unwrap();
        let pack = crate::creation::composer::load_production_pack(&root).unwrap();
        let profile =
            crate::creation::composer::motion_profile_for_recipe(&pack, &test.recipe).unwrap();
        let session_dir = test.root.join("creation-sessions").join(&test.session_id);
        let owned = session_dir.join(".candidate-stage-publish-owned");
        std::fs::create_dir_all(&owned).unwrap();
        std::fs::write(owned.join("body.png"), &decoded.bytes).unwrap();
        std::fs::write(
            owned.join("motion-profile.json"),
            serde_json::to_vec_pretty(&profile).unwrap(),
        )
        .unwrap();
        std::fs::write(
            owned.join("recipe.json"),
            serde_json::to_vec_pretty(&test.recipe).unwrap(),
        )
        .unwrap();
        let identity = OwnedDirectoryGuard::open(&owned, "test owned staging")
            .unwrap()
            .identity;
        write_v2_owned_intent(
            &session_dir,
            &test.session_id,
            ".candidate-stage-publish-owned",
            identity,
            &decoded.bytes,
            &profile,
            &test.recipe,
        );
        let unknown = session_dir.join(".candidate-stage-do-not-touch");
        std::fs::create_dir(&unknown).unwrap();
        std::fs::write(unknown.join("sentinel.txt"), b"keep").unwrap();

        let report = test.service.recover_composer_orphans().unwrap();

        assert_eq!(report.recovered_count, 1, "{:?}", report.warnings);
        assert!(!owned.exists());
        assert!(unknown.join("sentinel.txt").exists());
    }

    #[test]
    fn orphan_recovery_warns_for_one_malformed_session_and_continues_exact_recovery() {
        let test = ComposerCandidateHarness::new();
        let second_session_id = new_entity_id("session");
        let now = crate::creation::profiles::now_iso();
        {
            let mut storage = test.storage.lock().unwrap();
            let tx = storage.db.transaction().unwrap();
            // Exercise recovery defensively across multiple rows even though the current
            // schema normally enforces one long-running draft.
            tx.execute("DROP INDEX creation_one_long_draft", [])
                .unwrap();
            let pet =
                PetRepository::reserve_in_transaction(&tx, CreationMethod::Composer, None).unwrap();
            tx.execute(
                "INSERT INTO creation_sessions
                 (session_id, pet_id, method, status, last_stable_status, current_step,
                  schema_version, created_at, updated_at)
                 VALUES (?1, ?2, 'composer', 'draft', 'draft', 'ears', 1, ?3, ?3)",
                rusqlite::params![second_session_id, pet.pet_id, now],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO composer_recipes
                 (session_id, recipe_version, pack_id, pack_version, layer_contract_version,
                  body_id, ears_id, eyes_id, muzzle_id, tail_id, color_id, pattern_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    second_session_id,
                    test.recipe.recipe_version,
                    test.recipe.pack_id,
                    test.recipe.pack_version,
                    test.recipe.layer_contract_version,
                    test.recipe.body_id,
                    test.recipe.ears_id,
                    test.recipe.eyes_id,
                    test.recipe.muzzle_id,
                    test.recipe.tail_id,
                    test.recipe.color_id,
                    test.recipe.pattern_id,
                    now,
                ],
            )
            .unwrap();
            tx.commit().unwrap();
        }

        let bad_candidate = test
            .root
            .join("creation-sessions")
            .join(&test.session_id)
            .join("candidate");
        std::fs::create_dir_all(&bad_candidate).unwrap();
        std::fs::write(bad_candidate.join("unexpected.txt"), b"do not delete").unwrap();

        let second_session_dir = test.root.join("creation-sessions").join(&second_session_id);
        std::fs::create_dir_all(&second_session_dir).unwrap();
        let decoded = decode_composer_png(&valid_candidate_b64()).unwrap();
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/creation-content");
        let content_root = crate::creation::content::test_content_root(&content).unwrap();
        let pack = crate::creation::composer::load_production_pack(&content_root).unwrap();
        let profile =
            crate::creation::composer::motion_profile_for_recipe(&pack, &test.recipe).unwrap();
        let mut published = publish_composer_candidate(
            &test.root,
            &second_session_id,
            &decoded.bytes,
            &profile,
            &test.recipe,
        )
        .unwrap();
        published.simulate_process_exit_before_database_commit();
        drop(published);

        let recovery = test.service.recover_composer_orphans().unwrap();

        assert_eq!(recovery.recovered_count, 1, "{:?}", recovery.warnings);
        assert_eq!(recovery.warnings.len(), 1);
        assert!(recovery.warnings[0].contains(&test.session_id));
        assert!(bad_candidate.join("unexpected.txt").exists());
        assert!(!second_session_dir.join("candidate").exists());
    }

    #[test]
    fn session_directory_cannot_be_swapped_to_a_junction_during_publication() {
        let test = ComposerCandidateHarness::new();
        let encoded = valid_candidate_b64();
        let decoded = decode_composer_png(&encoded).unwrap();
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/creation-content");
        let content_root = crate::creation::content::test_content_root(&content).unwrap();
        let pack = crate::creation::composer::load_production_pack(&content_root).unwrap();
        let profile =
            crate::creation::composer::motion_profile_for_recipe(&pack, &test.recipe).unwrap();
        let session_dir = test.root.join("creation-sessions").join(&test.session_id);
        let moved = test.root.join("moved-session");
        let outside = test.root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        let attempted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let attempted_in_hook = attempted.clone();

        let mut published = publish_composer_candidate_with_hook(
            &test.root,
            &test.session_id,
            &decoded.bytes,
            &profile,
            &test.recipe,
            || {
                attempted_in_hook.store(true, Ordering::SeqCst);
                if std::fs::rename(&session_dir, &moved).is_ok() {
                    crate::platform::create_directory_link(&outside, &session_dir);
                }
            },
        )
        .unwrap();
        published.rollback().unwrap();

        assert!(attempted.load(Ordering::SeqCst));
        assert!(!outside.join("candidate").exists());
        assert!(session_dir.exists());
        assert!(!crate::platform::is_link_or_reparse_point(
            &std::fs::symlink_metadata(&session_dir).unwrap()
        ));
    }

    #[test]
    fn staging_is_pinned_against_a_junction_swap_before_the_first_file_write() {
        let test = ComposerCandidateHarness::new();
        let decoded = decode_composer_png(&valid_candidate_b64()).unwrap();
        let profile = production_profile(&test.recipe);
        let session_dir = test.root.join("creation-sessions").join(&test.session_id);
        let outside = test.root.join("outside-before-write");
        let moved = test.root.join("moved-stage-before-write");
        std::fs::create_dir(&outside).unwrap();
        let outside_in_hook = outside.clone();
        let swapped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let swapped_in_hook = swapped.clone();

        let result = publish_composer_candidate_with_staging_hooks(
            &test.root,
            &test.session_id,
            &decoded.bytes,
            &profile,
            &test.recipe,
            |stage_path| {
                if std::fs::rename(stage_path, &moved).is_ok() {
                    crate::platform::create_directory_link(&outside_in_hook, stage_path);
                    swapped_in_hook.store(true, Ordering::SeqCst);
                }
            },
            |_| {},
        );

        assert!(result.is_ok(), "{:?}", result.as_ref().err());
        assert!(session_dir.join("candidate").exists());
        assert!(!swapped.load(Ordering::SeqCst));
        assert!(std::fs::read_dir(outside).unwrap().next().is_none());
    }

    #[test]
    fn pre_move_path_substitution_is_detected_by_identity_and_never_cleaned_as_owned() {
        let test = ComposerCandidateHarness::new();
        let decoded = decode_composer_png(&valid_candidate_b64()).unwrap();
        let profile = production_profile(&test.recipe);
        let outside = test.root.join("outside-before-move");
        let moved = test.root.join("moved-stage-before-move");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("sentinel.txt"), b"external").unwrap();
        let outside_in_hook = outside.clone();
        let swapped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let swapped_in_hook = swapped.clone();

        let result = publish_composer_candidate_with_staging_hooks(
            &test.root,
            &test.session_id,
            &decoded.bytes,
            &profile,
            &test.recipe,
            |_| {},
            |stage_path| {
                if std::fs::rename(stage_path, &moved).is_ok() {
                    crate::platform::create_directory_link(&outside_in_hook, stage_path);
                    swapped_in_hook.store(true, Ordering::SeqCst);
                }
            },
        );

        assert!(swapped.load(Ordering::SeqCst));
        assert!(result.is_err());
        assert!(outside.join("sentinel.txt").exists());
        assert!(moved.join("body.png").exists());
        let recovery = test.service.recover_composer_orphans().unwrap();
        assert_eq!(recovery.recovered_count, 0, "{:?}", recovery.warnings);
        assert!(
            recovery
                .warnings
                .iter()
                .any(|warning| warning.contains("identity") || warning.contains("non-reparse")),
            "{:?}",
            recovery.warnings
        );
        assert!(outside.join("sentinel.txt").exists());
        assert!(moved.join("body.png").exists());
    }

    #[test]
    fn owned_intent_publish_never_replaces_an_unknown_competitor_file() {
        let test = ComposerCandidateHarness::new();
        let decoded = decode_composer_png(&valid_candidate_b64()).unwrap();
        let profile = production_profile(&test.recipe);
        let observed = std::sync::Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let observed_in_hook = observed.clone();

        let result = publish_composer_candidate_with_intent_phase_hooks(
            &test.root,
            &test.session_id,
            &decoded.bytes,
            &profile,
            &test.recipe,
            move |path| {
                if path.exists() {
                    std::fs::remove_file(path).unwrap();
                }
                std::fs::write(path, b"unknown-owned-sentinel").unwrap();
                *observed_in_hook.lock().unwrap() = Some(path.to_path_buf());
            },
            |_| {},
        );

        assert!(result.is_err(), "unknown owned intent target was replaced");
        let path = observed.lock().unwrap().clone().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"unknown-owned-sentinel");
    }

    #[test]
    fn complete_intent_publish_never_replaces_an_unknown_competitor_file() {
        let test = ComposerCandidateHarness::new();
        let decoded = decode_composer_png(&valid_candidate_b64()).unwrap();
        let profile = production_profile(&test.recipe);
        let observed = std::sync::Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let observed_in_hook = observed.clone();

        let result = publish_composer_candidate_with_intent_phase_hooks(
            &test.root,
            &test.session_id,
            &decoded.bytes,
            &profile,
            &test.recipe,
            |_| {},
            move |path| {
                if path.exists() {
                    std::fs::remove_file(path).unwrap();
                }
                std::fs::write(path, b"unknown-complete-sentinel").unwrap();
                *observed_in_hook.lock().unwrap() = Some(path.to_path_buf());
            },
        );

        assert!(
            result.is_err(),
            "unknown complete intent target was replaced"
        );
        let path = observed.lock().unwrap().clone().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"unknown-complete-sentinel");
    }

    #[test]
    fn intent_temp_is_exactly_removed_after_an_injected_write_failure() {
        let test = ComposerCandidateHarness::new();
        let body = decode_composer_png(&valid_candidate_b64()).unwrap().bytes;
        let profile = production_profile(&test.recipe);
        let intent = reserved_intent_fixture(&test.session_id, &body, &profile, &test.recipe);
        let session = test.root.join("creation-sessions").join(&test.session_id);
        std::fs::create_dir_all(&session).unwrap();
        let target = session.join(RESERVED_COMPOSER_INTENT_FILE);

        let result = publish_durable_intent_with_io_hooks(
            &session,
            &target,
            &intent,
            |_: &mut std::fs::File, _: &[u8]| Err("injected intent write failure".into()),
            |_: &std::fs::File| Ok(()),
        );

        assert!(result.is_err());
        assert!(!target.exists());
        assert_no_intent_temps(&session);
    }

    #[test]
    fn intent_temp_is_exactly_removed_after_an_injected_sync_failure() {
        let test = ComposerCandidateHarness::new();
        let body = decode_composer_png(&valid_candidate_b64()).unwrap().bytes;
        let profile = production_profile(&test.recipe);
        let intent = reserved_intent_fixture(&test.session_id, &body, &profile, &test.recipe);
        let session = test.root.join("creation-sessions").join(&test.session_id);
        std::fs::create_dir_all(&session).unwrap();
        let target = session.join(RESERVED_COMPOSER_INTENT_FILE);

        let result = publish_durable_intent_with_io_hooks(
            &session,
            &target,
            &intent,
            |file: &mut std::fs::File, bytes: &[u8]| {
                file.write_all(bytes)
                    .map_err(|error| format!("write intent temp: {error}"))
            },
            |_: &std::fs::File| Err("injected intent sync failure".into()),
        );

        assert!(result.is_err());
        assert!(!target.exists());
        assert_no_intent_temps(&session);
    }

    #[test]
    fn windows_directory_publish_uses_the_documented_write_through_primitive() {
        let platform = include_str!("../platform/windows.rs");
        let candidate = include_str!("candidate.rs");
        assert!(platform.contains("fn durable_move_directory"));
        assert!(platform.contains("MOVEFILE_WRITE_THROUGH"));
        assert!(candidate.contains("durable_move_directory"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_durable_directory_publish_moves_a_real_unicode_directory() {
        let test = ComposerCandidateHarness::new();
        let source = test.root.join("\u{5019}\u{9009}\u{53d1}\u{5e03}\u{6e90}");
        let target = test
            .root
            .join("\u{5019}\u{9009}\u{53d1}\u{5e03}\u{76ee}\u{6807}");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("sentinel.txt"), b"durable").unwrap();

        crate::platform::durable_move_directory(&source, &target).unwrap();

        assert!(!source.exists());
        assert_eq!(
            std::fs::read(target.join("sentinel.txt")).unwrap(),
            b"durable"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_durable_intent_move_refuses_to_replace_an_unknown_target() {
        let test = ComposerCandidateHarness::new();
        let source = test.root.join("intent-source.json");
        let target = test.root.join("intent-target.json");
        std::fs::write(&source, b"owned").unwrap();
        std::fs::write(&target, b"unknown").unwrap();

        let result = crate::platform::durable_move_file(&source, &target);

        assert!(result.is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"owned");
        assert_eq!(std::fs::read(&target).unwrap(), b"unknown");
    }

    #[test]
    fn non_windows_file_creation_is_a_separate_compile_time_fail_closed_implementation() {
        let candidate = include_str!("candidate.rs");
        let function = ["fn write_new", "_synced_file("].concat();
        let windows = ["#[cfg(windows)]", "\n", "fn write_new", "_synced_file"].concat();
        let other = ["#[cfg(not(windows))]", "\n", "fn write_new", "_synced_file"].concat();
        assert_eq!(candidate.matches(&function).count(), 2);
        assert!(candidate.contains(&windows));
        assert!(candidate.contains(&other));
        let movable = ["fn open", "_movable("].concat();
        let other_movable = [
            "#[cfg(not(windows))]\n    fn open",
            "_movable(_path: &Path, _label: &str)",
        ]
        .concat();
        assert_eq!(candidate.matches(&movable).count(), 2);
        assert!(candidate.contains(&other_movable));
    }

    #[test]
    fn composer_publish_supports_unicode_and_extended_length_app_data_paths() {
        let test = ComposerCandidateHarness::new();
        let app_data = test
            .root
            .join("候选目录".repeat(20))
            .join(format!("segment-a-{}", "a".repeat(112)))
            .join(format!("segment-b-{}", "b".repeat(112)));
        let session_id = "session-long-unicode";
        std::fs::create_dir_all(app_data.join("creation-sessions").join(session_id)).unwrap();
        let decoded = decode_composer_png(&valid_candidate_b64()).unwrap();
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/creation-content");
        let content_root = crate::creation::content::test_content_root(&content).unwrap();
        let pack = crate::creation::composer::load_production_pack(&content_root).unwrap();
        let profile =
            crate::creation::composer::motion_profile_for_recipe(&pack, &test.recipe).unwrap();

        let mut published = publish_composer_candidate(
            &app_data,
            session_id,
            &decoded.bytes,
            &profile,
            &test.recipe,
        )
        .expect("Unicode extended-length app data path must publish");
        published.rollback().unwrap();
    }

    #[test]
    fn adoption_recovery_never_recognizes_or_cleans_composer_phase_files() {
        let test = ComposerCandidateHarness::new();
        let decoded = decode_composer_png(&valid_candidate_b64()).unwrap();
        let profile = production_profile(&test.recipe);
        let session_dir = test.root.join("creation-sessions").join(&test.session_id);
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = publish_composer_candidate_with_intent_phase_hooks(
                &test.root,
                &test.session_id,
                &decoded.bytes,
                &profile,
                &test.recipe,
                |_| panic!("simulated composer crash after reserved phase"),
                |_| {},
            );
        }));
        assert!(crashed.is_err());
        assert!(session_dir.join(RESERVED_COMPOSER_INTENT_FILE).exists());

        let recovered = recover_exact_adoption_orphan(
            &test.root,
            &test.session_id,
            &sha256_hex(&decoded.bytes),
            &sha256_hex(&serde_json::to_vec_pretty(&profile).unwrap()),
        )
        .unwrap();

        assert!(!recovered);
        assert!(session_dir.join(RESERVED_COMPOSER_INTENT_FILE).exists());
    }

    #[test]
    fn adoption_phase_crashes_are_recovered_from_one_shared_publish_protocol() {
        for crash_point in [
            CandidateCrashPoint::AfterReserved,
            CandidateCrashPoint::AfterOwned,
            CandidateCrashPoint::AfterComplete,
            CandidateCrashPoint::AfterMove,
        ] {
            let test = ComposerCandidateHarness::new();
            let decoded = decode_composer_png(&valid_candidate_b64()).unwrap();
            let profile = serde_json::to_vec_pretty(&production_profile(&test.recipe)).unwrap();
            let body_hash = sha256_hex(&decoded.bytes);
            let profile_hash = sha256_hex(&profile);
            let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = publish_adoption_candidate_with_crash_point(
                    &test.root,
                    &test.session_id,
                    &decoded.bytes,
                    &profile,
                    crash_point,
                );
            }));
            assert!(crashed.is_err(), "{crash_point:?} did not crash");

            assert!(
                recover_exact_adoption_orphan(
                    &test.root,
                    &test.session_id,
                    &body_hash,
                    &profile_hash,
                )
                .unwrap(),
                "{crash_point:?} left no recoverable publication"
            );
            let session_dir = test.root.join("creation-sessions").join(&test.session_id);
            assert!(!session_dir.join("candidate").exists());
            assert!(std::fs::read_dir(&session_dir).unwrap().all(|entry| {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                !name.starts_with(".candidate-stage-")
                    && !name.starts_with(".adoption-candidate-publish-intent-")
            }));
        }
    }

    #[test]
    fn adoption_db_record_crash_reuses_candidate_and_clears_complete_intent() {
        let test = ComposerCandidateHarness::new();
        let decoded = decode_composer_png(&valid_candidate_b64()).unwrap();
        let profile = serde_json::to_vec_pretty(&production_profile(&test.recipe)).unwrap();
        let body_hash = sha256_hex(&decoded.bytes);
        let profile_hash = sha256_hex(&profile);
        let mut published =
            publish_adoption_candidate(&test.root, &test.session_id, &decoded.bytes, &profile)
                .unwrap();
        CreationStore::new(test.storage.clone())
            .record_local_candidate(
                &test.session_id,
                &published.body_path,
                &published.motion_profile_path,
            )
            .unwrap();
        published.simulate_process_exit_before_database_commit();
        drop(published);
        let session_dir = test.root.join("creation-sessions").join(&test.session_id);
        assert!(session_dir.join(COMPLETE_ADOPTION_INTENT_FILE).exists());
        assert_eq!(test.candidate_count(), 1);

        assert!(verify_committed_adoption_candidate(
            &test.root,
            &test.session_id,
            &body_hash,
            &profile_hash,
        )
        .unwrap());

        assert!(session_dir.join("candidate").exists());
        assert!(!session_dir.join(COMPLETE_ADOPTION_INTENT_FILE).exists());
        assert_eq!(test.candidate_count(), 1);
    }

    struct CandidateHarness {
        root: PathBuf,
        storage: Arc<Mutex<Storage>>,
        store: CreationStore,
        session_id: String,
        pet_id: String,
    }

    impl Drop for CandidateHarness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl CandidateHarness {
        fn session(method: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let root = std::env::temp_dir().join(format!(
                "desktop-pet-standard-candidate-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
            let (species, identity) = match method {
                "composer" => (Species::Dog, IdentityMode::Guided),
                _ => (Species::Cat, IdentityMode::RealPet),
            };
            let pet = PetRepository::new(storage.clone())
                .create(species, identity)
                .unwrap();
            let session_id = new_entity_id("session");
            let now = crate::creation::profiles::now_iso();
            storage
                .lock()
                .unwrap()
                .db
                .execute(
                    "INSERT INTO creation_sessions
                     (session_id, pet_id, method, status, last_stable_status, current_step,
                      schema_version, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'draft', 'draft', ?3, 1, ?4, ?4)",
                    rusqlite::params![session_id, pet.pet_id, method, now],
                )
                .unwrap();
            Self {
                root,
                store: CreationStore::new(storage.clone()),
                storage,
                session_id,
                pet_id: pet.pet_id,
            }
        }

        fn upload() -> Self {
            Self::session("upload")
        }

        fn composer() -> Self {
            Self::session("composer")
        }

        fn candidate_files(&self, job_id: &str) -> (String, String, String) {
            let dir = self.root.join("jobs").join(job_id);
            std::fs::create_dir_all(&dir).unwrap();
            let raw = dir.join("raw.png");
            let cutout = dir.join("cutout.png");
            let profile = dir.join("motion-profile.json");
            std::fs::write(&raw, b"raw").unwrap();
            std::fs::write(&cutout, b"cutout").unwrap();
            std::fs::write(&profile, b"{}").unwrap();
            (
                raw.to_string_lossy().into_owned(),
                cutout.to_string_lossy().into_owned(),
                profile.to_string_lossy().into_owned(),
            )
        }

        fn session_state(&self) -> (String, String, String, Option<String>) {
            self.storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT status, last_stable_status, current_step, error
                     FROM creation_sessions WHERE session_id=?1",
                    [&self.session_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap()
        }
    }

    #[test]
    fn upload_job_and_candidate_are_owned_by_the_session() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "prompt", "sha", Some("task-1"))
            .unwrap();
        let (raw, cutout, profile) = test.candidate_files("job-1");
        test.store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &test.root.join("jobs"),
                &raw,
                &cutout,
                &profile,
                "acceptable",
            )
            .unwrap();

        let candidate = test.store.candidate_for_session(&test.session_id).unwrap();
        assert_eq!(candidate.session_id, test.session_id);
        assert_eq!(candidate.pet_id, test.pet_id);
        assert_eq!(candidate.job_id.as_deref(), Some("job-1"));
        assert_eq!(
            PathBuf::from(candidate.motion_profile_path),
            PathBuf::from(profile).canonicalize().unwrap()
        );
        assert!(candidate.candidate_id.starts_with("candidate-"));
        assert_eq!(
            test.session_state(),
            (
                "candidateReady".into(),
                "candidateReady".into(),
                "review".into(),
                None
            )
        );
    }

    #[test]
    fn submitting_job_cannot_record_a_candidate_before_task_attachment() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "prompt", "sha", None)
            .unwrap();
        let (raw, cutout, profile) = test.candidate_files("job-1");

        assert!(test
            .store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &test.root.join("jobs"),
                &raw,
                &cutout,
                &profile,
                "acceptable",
            )
            .is_err());
        assert_eq!(test.store.job("job-1").unwrap().status, "submitting");
        assert!(test.store.candidate_for_session(&test.session_id).is_err());
    }

    #[test]
    fn composer_session_cannot_start_an_upload_job() {
        let test = CandidateHarness::composer();
        assert!(test
            .store
            .create_job_for_session("job-1", &test.session_id, "prompt", "sha", None)
            .unwrap_err()
            .contains("upload session"));
    }

    #[test]
    fn candidate_rejects_a_job_owned_by_another_session() {
        let first = CandidateHarness::upload();
        first
            .store
            .create_job_for_session("job-1", &first.session_id, "p", "h", None)
            .unwrap();
        {
            let db = &first.storage.lock().unwrap().db;
            db.execute(
                "UPDATE creation_sessions SET status='completed', last_stable_status='completed'
                 WHERE session_id=?1",
                [&first.session_id],
            )
            .unwrap();
        }
        let second_session = new_entity_id("session");
        let second_pet = PetRepository::new(first.storage.clone())
            .create(Species::Dog, IdentityMode::RealPet)
            .unwrap();
        let now = crate::creation::profiles::now_iso();
        first
            .storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO creation_sessions
                 (session_id, pet_id, method, status, last_stable_status, current_step,
                  schema_version, created_at, updated_at)
                 VALUES (?1, ?2, 'upload', 'draft', 'draft', 'upload', 1, ?3, ?3)",
                rusqlite::params![second_session, second_pet.pet_id, now],
            )
            .unwrap();
        let (raw, cutout, profile) = first.candidate_files("job-1");

        assert!(first
            .store
            .record_upload_candidate(
                "job-1",
                &second_session,
                &first.root.join("jobs"),
                &raw,
                &cutout,
                &profile,
                "acceptable",
            )
            .unwrap_err()
            .contains("owned"));
        assert!(first.store.candidate_for_session(&second_session).is_err());
    }

    #[test]
    fn a_second_current_candidate_requires_explicit_replacement() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", Some("task-1"))
            .unwrap();
        let (raw, cutout, profile) = test.candidate_files("job-1");
        test.store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &test.root.join("jobs"),
                &raw,
                &cutout,
                &profile,
                "acceptable",
            )
            .unwrap();
        {
            let db = &test.storage.lock().unwrap().db;
            db.execute(
                "UPDATE creation_sessions
                 SET status='draft', last_stable_status='draft', current_step='generating'",
                [],
            )
            .unwrap();
            db.execute(
                "INSERT INTO generation_jobs
                 (job_id, pet_id, session_id, prompt, ref_sha256, status, created_at)
                 VALUES ('job-2', ?1, ?2, 'p', 'h', 'pending', ?3)",
                rusqlite::params![
                    test.pet_id,
                    test.session_id,
                    crate::creation::profiles::now_iso()
                ],
            )
            .unwrap();
        }
        let (raw2, cutout2, profile2) = test.candidate_files("job-2");

        assert!(test
            .store
            .record_upload_candidate(
                "job-2",
                &test.session_id,
                &test.root.join("jobs"),
                &raw2,
                &cutout2,
                &profile2,
                "acceptable",
            )
            .unwrap_err()
            .contains("current candidate"));
        assert_eq!(
            test.store
                .candidate_for_session(&test.session_id)
                .unwrap()
                .job_id
                .as_deref(),
            Some("job-1")
        );
    }

    #[test]
    fn candidate_paths_must_be_standard_files_under_the_matching_job() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();
        let (raw, cutout, profile) = test.candidate_files("other-job");

        assert!(test
            .store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &test.root.join("jobs"),
                &raw,
                &cutout,
                &profile,
                "acceptable",
            )
            .unwrap_err()
            .contains("job directory"));
    }

    #[test]
    fn candidate_rejects_an_external_directory_with_the_same_job_name() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();
        let external = test.root.join("external").join("job-1");
        std::fs::create_dir_all(&external).unwrap();
        let raw = external.join("raw.png");
        let cutout = external.join("cutout.png");
        let profile = external.join("motion-profile.json");
        std::fs::write(&raw, b"raw").unwrap();
        std::fs::write(&cutout, b"cutout").unwrap();
        std::fs::write(&profile, b"{}").unwrap();

        assert!(test
            .store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &test.root.join("jobs"),
                &raw.to_string_lossy(),
                &cutout.to_string_lossy(),
                &profile.to_string_lossy(),
                "acceptable",
            )
            .unwrap_err()
            .contains("configured jobs root"));
    }

    #[test]
    fn candidate_rejects_a_job_directory_link_escape() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();
        let outside = test.root.join("outside-job-1");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("raw.png"), b"raw").unwrap();
        std::fs::write(outside.join("cutout.png"), b"cutout").unwrap();
        std::fs::write(outside.join("motion-profile.json"), b"{}").unwrap();
        let jobs_root = test.root.join("jobs");
        std::fs::create_dir_all(&jobs_root).unwrap();
        crate::platform::create_directory_link(&outside, &jobs_root.join("job-1"));

        assert!(test
            .store
            .record_upload_candidate(
                "job-1",
                &test.session_id,
                &jobs_root,
                &jobs_root.join("job-1/raw.png").to_string_lossy(),
                &jobs_root.join("job-1/cutout.png").to_string_lossy(),
                &jobs_root
                    .join("job-1/motion-profile.json")
                    .to_string_lossy(),
                "acceptable",
            )
            .unwrap_err()
            .contains("link or reparse point"));
    }

    #[test]
    fn failed_job_converges_the_session_without_a_candidate() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();
        test.store
            .fail_upload_job("job-1", &test.session_id, &test.pet_id, "profile failed")
            .unwrap();

        assert_eq!(
            test.session_state(),
            (
                "retryableFailure".into(),
                "draft".into(),
                "upload".into(),
                Some("profile failed".into())
            )
        );
        assert!(test.store.candidate_for_session(&test.session_id).is_err());
        let jobs = test.store.upload_jobs(&test.session_id).unwrap();
        assert_eq!(jobs[0].status, "failed");
    }

    #[test]
    fn retry_reuses_the_session_and_preserves_job_history() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();
        test.store
            .fail_upload_job("job-1", &test.session_id, &test.pet_id, "first failed")
            .unwrap();
        test.store
            .create_job_for_session("job-2", &test.session_id, "p2", "h2", None)
            .unwrap();

        let jobs = test.store.upload_jobs(&test.session_id).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(
            jobs[0].session_id.as_deref(),
            Some(test.session_id.as_str())
        );
        assert_eq!(jobs[0].status, "failed");
        assert_eq!(jobs[1].status, "submitting");
        assert_eq!(
            test.session_state(),
            ("draft".into(), "draft".into(), "generating".into(), None)
        );
    }

    #[test]
    fn upload_session_rejects_a_second_active_job() {
        let test = CandidateHarness::upload();
        test.store
            .create_job_for_session("job-1", &test.session_id, "p", "h", None)
            .unwrap();

        assert!(test
            .store
            .create_job_for_session("job-2", &test.session_id, "p2", "h2", None)
            .unwrap_err()
            .contains("active upload job"));
        assert_eq!(test.store.upload_jobs(&test.session_id).unwrap().len(), 1);
    }
}
