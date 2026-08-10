use crate::creation::domain::{new_entity_id, ComposerRecipe};
use crate::runtime_assets::motion_profile::{parse_motion_profile, MotionProfileV1};
use base64::Engine as _;
use image::ImageDecoder as _;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::{
    ffi::OsStrExt,
    fs::OpenOptionsExt,
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
};

const MAX_COMPOSER_PNG_BYTES: usize = 10 * 1024 * 1024;
const MAX_COMPOSER_B64_BYTES: usize = MAX_COMPOSER_PNG_BYTES.div_ceil(3) * 4;

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
}

struct OwnedDirectoryGuard {
    path: PathBuf,
    #[cfg(windows)]
    handle: OwnedHandle,
}

impl OwnedDirectoryGuard {
    #[cfg(windows)]
    fn open(path: &Path, label: &str) -> Result<Self, String> {
        use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE,
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };

        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | GENERIC_WRITE | DELETE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
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
            handle,
        })
    }

    #[cfg(not(windows))]
    fn open(_path: &Path, _label: &str) -> Result<Self, String> {
        Err("secure composer publication currently requires Windows handle-relative I/O".into())
    }

    #[cfg(windows)]
    fn rename_child_handle_to(
        &self,
        child: &mut OwnedDirectoryGuard,
        target_name: &str,
    ) -> Result<(), String> {
        use windows_sys::Win32::Storage::FileSystem::{
            FileRenameInfo, FlushFileBuffers, SetFileInformationByHandle, FILE_RENAME_INFO,
        };
        validate_component(target_name, "candidate target name")?;
        let target = self.path.join(target_name);
        let target_text = target.to_string_lossy();
        let win32_target = target_text.strip_prefix(r"\\?\").unwrap_or(&target_text);
        let encoded: Vec<u16> = std::ffi::OsStr::new(win32_target).encode_wide().collect();
        let header = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
        let byte_len = header + (encoded.len() + 1) * std::mem::size_of::<u16>();
        let words = byte_len.div_ceil(std::mem::size_of::<usize>());
        let mut storage = vec![0usize; words];
        let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        unsafe {
            (*info).Anonymous.ReplaceIfExists = false;
            (*info).RootDirectory = std::ptr::null_mut();
            (*info).FileNameLength = (encoded.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                encoded.as_ptr(),
                (*info).FileName.as_mut_ptr(),
                encoded.len(),
            );
        }
        if unsafe {
            SetFileInformationByHandle(
                child.handle.as_raw_handle(),
                FileRenameInfo,
                info.cast(),
                byte_len as u32,
            )
        } == 0
        {
            return Err(format!(
                "publish composer candidate by directory handle: {}",
                std::io::Error::last_os_error()
            ));
        }
        child.path = target;
        if unsafe { FlushFileBuffers(child.handle.as_raw_handle()) } == 0 {
            return Err(format!(
                "flush published composer candidate directory handle: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    #[cfg(windows)]
    fn mark_delete(&self) -> Result<(), String> {
        mark_raw_handle_delete(self.handle.as_raw_handle(), "composer candidate directory")
    }

    #[cfg(not(windows))]
    fn rename_child_handle_to(
        &self,
        _child: &mut OwnedDirectoryGuard,
        _target_name: &str,
    ) -> Result<(), String> {
        Err("secure composer publication currently requires Windows handle-relative rename".into())
    }

    #[cfg(not(windows))]
    fn mark_delete(&self) -> Result<(), String> {
        Err("secure composer cleanup currently requires Windows handle-relative deletion".into())
    }
}

impl PublishedComposerCandidate {
    pub fn commit(&mut self) {
        self.committed = true;
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
        self.newly_published = false;
        Ok(())
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
    publish_composer_candidate_inner(app_data_dir, session_id, png, profile, recipe, || {})
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
    publish_composer_candidate_inner(app_data_dir, session_id, png, profile, recipe, hook)
}

fn publish_composer_candidate_inner(
    app_data_dir: &Path,
    session_id: &str,
    png: &[u8],
    profile: &MotionProfileV1,
    recipe: &ComposerRecipe,
    hook: impl FnOnce(),
) -> Result<PublishedComposerCandidate, String> {
    validate_component(session_id, "session id")?;
    let profile_json = serde_json::to_vec_pretty(profile).map_err(|error| error.to_string())?;
    parse_motion_profile(std::str::from_utf8(&profile_json).map_err(|error| error.to_string())?)?;
    let recipe_json = serde_json::to_vec_pretty(recipe).map_err(|error| error.to_string())?;

    let app_data_guard = OwnedDirectoryGuard::open(app_data_dir, "app data")?;
    let (_sessions_root, sessions_guard) =
        ensure_locked_child(&app_data_guard, "creation-sessions", "creation sessions")?;
    let (session_dir, session_guard) =
        ensure_locked_child(&sessions_guard, session_id, "creation session")?;
    hook();
    let candidate_dir = session_dir.join("candidate");
    if std::fs::symlink_metadata(&candidate_dir).is_ok() {
        let candidate_guard = OwnedDirectoryGuard::open(&candidate_dir, "candidate directory")?;
        if candidate_guard.path.parent() != Some(session_dir.as_path()) {
            return Err("candidate directory escapes its creation session".into());
        }
        let file_guards = lock_candidate_files(&candidate_guard, png, &profile_json, &recipe_json)?;
        return Ok(candidate_projection(
            candidate_dir,
            png,
            false,
            vec![app_data_guard, sessions_guard, session_guard],
            candidate_guard,
            file_guards,
        ));
    }

    let staging_name = format!(".candidate-stage-{}", new_entity_id("publish"));
    validate_component(staging_name.trim_start_matches('.'), "candidate staging id")?;
    let staging = session_dir.join(&staging_name);
    std::fs::create_dir(&staging)
        .map_err(|error| format!("create composer candidate staging directory: {error}"))?;
    let mut staging_guard = OwnedDirectoryGuard::open(&staging, "candidate staging")?;
    if staging_guard.path.parent() != Some(session_dir.as_path()) {
        return Err("candidate staging directory escapes its creation session".into());
    }
    let mut file_guards = Vec::new();
    let stage_result = (|| {
        file_guards.push(write_new_synced_file(&staging.join("body.png"), png)?);
        file_guards.push(write_new_synced_file(
            &staging.join("motion-profile.json"),
            &profile_json,
        )?);
        file_guards.push(write_new_synced_file(
            &staging.join("recipe.json"),
            &recipe_json,
        )?);
        validate_locked_candidate_files(
            &staging_guard,
            &file_guards,
            png,
            &profile_json,
            &recipe_json,
        )?;
        // Windows cannot rename a directory while child handles deny delete sharing. The
        // directory handle remains locked and is the source of the rename; final files are
        // immediately re-opened without delete sharing and revalidated below.
        file_guards.clear();
        session_guard.rename_child_handle_to(&mut staging_guard, "candidate")?;
        crate::platform::sync_existing_directory_entry(&candidate_dir)
    })();
    if let Err(error) = stage_result {
        let cleanup_files = if file_guards.is_empty() {
            lock_candidate_files(&staging_guard, png, &profile_json, &recipe_json)
                .map_err(|lock| format!("re-lock staged composer candidate for cleanup: {lock}"))
        } else {
            Ok(file_guards)
        };
        let cleanup = cleanup_files.and_then(|files| delete_locked_directory(staging_guard, files));
        return Err(match cleanup {
            Ok(()) => error,
            Err(cleanup) => format!("{error}; staging cleanup failed: {cleanup}"),
        });
    }
    let candidate_guard = staging_guard;
    if candidate_guard.path.parent() != Some(session_dir.as_path()) {
        return Err("published candidate directory escapes its creation session".into());
    }
    file_guards = lock_candidate_files(&candidate_guard, png, &profile_json, &recipe_json)?;
    Ok(candidate_projection(
        candidate_dir,
        png,
        true,
        vec![app_data_guard, sessions_guard, session_guard],
        candidate_guard,
        file_guards,
    ))
}

fn candidate_projection(
    candidate_dir: PathBuf,
    _body: &[u8],
    newly_published: bool,
    parent_guards: Vec<OwnedDirectoryGuard>,
    candidate_guard: OwnedDirectoryGuard,
    file_guards: Vec<std::fs::File>,
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
    }
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
    let candidate_path = session_guard.path.join("candidate");
    if std::fs::symlink_metadata(&candidate_path).is_err() {
        return Ok(false);
    }
    let candidate_guard = OwnedDirectoryGuard::open(&candidate_path, "orphan candidate")?;
    if candidate_guard.path.parent() != Some(session_guard.path.as_path()) {
        return Err("orphan candidate directory escapes its creation session".into());
    }
    let (files, bytes) = lock_exact_candidate_file_set(&candidate_guard)?;
    let recipe: ComposerRecipe = serde_json::from_slice(&bytes[2])
        .map_err(|error| format!("orphan composer recipe is invalid: {error}"))?;
    if &recipe != expected_recipe {
        return Err("orphan composer recipe does not match the durable draft".into());
    }
    let profile = parse_motion_profile(
        std::str::from_utf8(&bytes[1])
            .map_err(|error| format!("orphan motion profile is not UTF-8: {error}"))?,
    )?;
    if &profile != expected_profile {
        return Err("orphan composer motion profile does not match trusted body semantics".into());
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes[0]);
    decode_composer_png(&encoded)?;
    delete_locked_directory(candidate_guard, files)?;
    drop((session_guard, sessions_guard, app_data_guard));
    Ok(true)
}

fn lock_exact_candidate_file_set(
    directory: &OwnedDirectoryGuard,
) -> Result<(Vec<std::fs::File>, Vec<Vec<u8>>), String> {
    let entries = std::fs::read_dir(&directory.path)
        .map_err(|error| format!("read orphan candidate directory: {error}"))?
        .map(|entry| {
            entry
                .map_err(|error| error.to_string())?
                .file_name()
                .into_string()
                .map_err(|_| "orphan candidate contains a non-Unicode file name".to_string())
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
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
        use windows_sys::Win32::Foundation::GENERIC_READ;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        let mut files = Vec::new();
        let mut bytes = Vec::new();
        for name in ["body.png", "motion-profile.json", "recipe.json"] {
            let path = directory.path.join(name);
            let file = std::fs::OpenOptions::new()
                .read(true)
                .access_mode(GENERIC_READ | DELETE)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&path)
                .map_err(|error| format!("open orphan composer candidate {name}: {error}"))?;
            let metadata = file.metadata().map_err(|error| error.to_string())?;
            if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
                return Err(format!(
                    "orphan composer candidate {name} is not a regular file"
                ));
            }
            if path
                .canonicalize()
                .map_err(|error| error.to_string())?
                .parent()
                != Some(directory.path.as_path())
            {
                return Err(format!(
                    "orphan composer candidate {name} escapes its directory"
                ));
            }
            let mut reader = file.try_clone().map_err(|error| error.to_string())?;
            let mut content = Vec::new();
            reader
                .read_to_end(&mut content)
                .map_err(|error| error.to_string())?;
            files.push(file);
            bytes.push(content);
        }
        Ok((files, bytes))
    }
}

fn write_new_synced_file(path: &Path, bytes: &[u8]) -> Result<std::fs::File, String> {
    #[cfg(not(windows))]
    {
        let _ = (path, bytes);
        return Err("secure composer file creation currently requires Windows handle I/O".into());
    }
    #[cfg(windows)]
    let mut file = {
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ,
        };
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)
            .open(path)
            .map_err(|error| format!("create staged composer candidate file: {error}"))?
    };
    file.write_all(bytes)
        .map_err(|error| format!("write staged composer candidate file: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync staged composer candidate file: {error}"))?;
    Ok(file)
}

fn lock_candidate_files(
    directory: &OwnedDirectoryGuard,
    body: &[u8],
    profile: &[u8],
    recipe: &[u8],
) -> Result<Vec<std::fs::File>, String> {
    #[cfg(not(windows))]
    {
        let _ = (directory, body, profile, recipe);
        return Err("secure composer file validation currently requires Windows handle I/O".into());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::GENERIC_READ;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        let mut files = Vec::new();
        for (name, expected) in [
            ("body.png", body),
            ("motion-profile.json", profile),
            ("recipe.json", recipe),
        ] {
            let path = directory.path.join(name);
            let file = std::fs::OpenOptions::new()
                .read(true)
                .access_mode(GENERIC_READ | DELETE)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&path)
                .map_err(|error| format!("open locked composer candidate {name}: {error}"))?;
            let metadata = file
                .metadata()
                .map_err(|error| format!("inspect locked composer candidate {name}: {error}"))?;
            if crate::platform::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
                return Err(format!("composer candidate {name} must be a regular file"));
            }
            let canonical = path
                .canonicalize()
                .map_err(|error| format!("resolve locked composer candidate {name}: {error}"))?;
            if canonical.parent() != Some(directory.path.as_path()) {
                return Err(format!(
                    "composer candidate {name} escapes its candidate directory"
                ));
            }
            let mut reader = file.try_clone().map_err(|error| error.to_string())?;
            reader
                .seek(std::io::SeekFrom::Start(0))
                .map_err(|error| error.to_string())?;
            let mut actual = Vec::new();
            reader
                .read_to_end(&mut actual)
                .map_err(|error| error.to_string())?;
            if actual != expected {
                return Err(format!(
                    "existing composer candidate {name} is not owned by this request"
                ));
            }
            files.push(file);
        }
        Ok(files)
    }
}

fn validate_locked_candidate_files(
    directory: &OwnedDirectoryGuard,
    files: &[std::fs::File],
    body: &[u8],
    profile: &[u8],
    recipe: &[u8],
) -> Result<(), String> {
    if files.len() != 3 {
        return Err("composer candidate does not own exactly three locked files".into());
    }
    for (index, (name, expected)) in [
        ("body.png", body),
        ("motion-profile.json", profile),
        ("recipe.json", recipe),
    ]
    .into_iter()
    .enumerate()
    {
        let canonical = directory
            .path
            .join(name)
            .canonicalize()
            .map_err(|error| format!("resolve locked composer candidate {name}: {error}"))?;
        if canonical.parent() != Some(directory.path.as_path()) {
            return Err(format!(
                "composer candidate {name} escapes its candidate directory"
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
            .read_to_end(&mut actual)
            .map_err(|error| error.to_string())?;
        if actual != expected {
            return Err(format!(
                "locked composer candidate {name} changed during publication"
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
                    .store_composer_candidate(&test.session_id, &encoded)
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
                .store_composer_candidate(&test.session_id, &encoded)
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
            .store_composer_candidate(&missing_recipe.session_id, &encoded)
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
            .store_composer_candidate(&wrong_method.session_id, &encoded)
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
            .store_composer_candidate(&terminal.session_id, &encoded)
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
            .store_composer_candidate(&test.session_id, &encoded)
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
            .store_composer_candidate(&test.session_id, &encoded)
            .unwrap();
        let second = test
            .service
            .store_composer_candidate(&test.session_id, &encoded)
            .unwrap();
        assert_eq!(first.snapshot.candidate_id, second.snapshot.candidate_id);
        assert_eq!(test.candidate_count(), 1);

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
            .store_composer_candidate(&failing.session_id, &encoded)
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
    }

    #[test]
    fn retryable_finalization_reuses_the_exact_composer_candidate_for_preview() {
        let test = ComposerCandidateHarness::new();
        let encoded = valid_candidate_b64();
        test.service
            .store_composer_candidate(&test.session_id, &encoded)
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
            .store_composer_candidate(&test.session_id, &encoded)
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
        published.commit();
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
            .store_composer_candidate(&test.session_id, &encoded)
            .unwrap();
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
        published.commit();
        drop(published);

        let recovery = test.service.recover_composer_orphans().unwrap();

        assert_eq!(recovery.recovered_count, 1);
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
