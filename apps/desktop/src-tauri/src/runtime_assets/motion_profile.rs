use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct NormalizedPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MotionProfileV1 {
    pub profile_version: u32,
    pub engine_profile: String,
    pub alpha_bounds: NormalizedRect,
    pub breath_zone: NormalizedRect,
    pub sway_pivot: NormalizedPoint,
}

pub fn generate_motion_profile(image: &image::RgbaImage) -> Result<MotionProfileV1, String> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Err("image dimensions must be positive".into());
    }

    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel.0[3] < 8 {
            continue;
        }
        bounds = Some(match bounds {
            None => (x, y, x, y),
            Some((left, top, right, bottom)) => {
                (left.min(x), top.min(y), right.max(x), bottom.max(y))
            }
        });
    }

    let (left, top, right, bottom) = bounds.ok_or("opaque silhouette is missing")?;
    let alpha_bounds = NormalizedRect {
        left: left as f32 / width as f32,
        top: top as f32 / height as f32,
        right: (right + 1) as f32 / width as f32,
        bottom: (bottom + 1) as f32 / height as f32,
    };
    let width = alpha_bounds.right - alpha_bounds.left;
    let height = alpha_bounds.bottom - alpha_bounds.top;
    let profile = MotionProfileV1 {
        profile_version: 1,
        engine_profile: "life-v1".into(),
        alpha_bounds: alpha_bounds.clone(),
        breath_zone: NormalizedRect {
            left: alpha_bounds.left + width * 0.15,
            top: alpha_bounds.top + height * 0.46,
            right: alpha_bounds.right - width * 0.15,
            bottom: alpha_bounds.top + height * 0.85,
        },
        sway_pivot: NormalizedPoint {
            x: alpha_bounds.left + width * 0.50,
            y: alpha_bounds.top + height * 0.74,
        },
    };
    validate_motion_profile(&profile)?;
    Ok(profile)
}

fn validate_motion_profile(profile: &MotionProfileV1) -> Result<(), String> {
    if profile.profile_version != 1 {
        return Err("unknown profile version".into());
    }
    if profile.engine_profile != "life-v1" {
        return Err("unknown engine profile".into());
    }
    validate_rect(&profile.alpha_bounds, "alpha bounds")?;
    validate_rect(&profile.breath_zone, "breath zone")?;
    validate_point(&profile.sway_pivot, "sway pivot")?;

    if profile.breath_zone.left < profile.alpha_bounds.left
        || profile.breath_zone.top < profile.alpha_bounds.top
        || profile.breath_zone.right > profile.alpha_bounds.right
        || profile.breath_zone.bottom > profile.alpha_bounds.bottom
    {
        return Err("breath zone is outside alpha bounds".into());
    }
    let face_safety_line =
        profile.alpha_bounds.top + (profile.alpha_bounds.bottom - profile.alpha_bounds.top) * 0.40;
    if profile.breath_zone.top < face_safety_line {
        return Err("breath zone violates face safety line".into());
    }
    if profile.sway_pivot.x < profile.alpha_bounds.left
        || profile.sway_pivot.x > profile.alpha_bounds.right
        || profile.sway_pivot.y < profile.alpha_bounds.top
        || profile.sway_pivot.y > profile.alpha_bounds.bottom
    {
        return Err("sway pivot is outside alpha bounds".into());
    }
    Ok(())
}

fn validate_rect(rect: &NormalizedRect, name: &str) -> Result<(), String> {
    for value in [rect.left, rect.top, rect.right, rect.bottom] {
        if !value.is_finite() {
            return Err(format!("{name} contains a non-finite value"));
        }
        if !(0.0..=1.0).contains(&value) {
            return Err(format!("{name} is out of range"));
        }
    }
    if rect.left >= rect.right || rect.top >= rect.bottom {
        return Err(format!("{name} is an inverted rect"));
    }
    Ok(())
}

fn validate_point(point: &NormalizedPoint, name: &str) -> Result<(), String> {
    for value in [point.x, point.y] {
        if !value.is_finite() {
            return Err(format!("{name} contains a non-finite value"));
        }
        if !(0.0..=1.0).contains(&value) {
            return Err(format!("{name} is out of range"));
        }
    }
    Ok(())
}

pub fn parse_motion_profile(json: &str) -> Result<MotionProfileV1, String> {
    let profile =
        serde_json::from_str(json).map_err(|error| format!("invalid motion profile: {error}"))?;
    validate_motion_profile(&profile)?;
    Ok(profile)
}

pub fn write_motion_profile_atomic(path: &Path, profile: &MotionProfileV1) -> Result<(), String> {
    validate_motion_profile(profile)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or("motion profile path must have a parent directory")?;
    let file_name = path
        .file_name()
        .ok_or("motion profile path must have a file name")?
        .to_string_lossy();
    let bytes = serde_json::to_vec_pretty(profile).map_err(|error| error.to_string())?;
    let (temporary, mut file) = create_unique_temporary_file(parent, &file_name)?;
    let mut owns_temporary = true;

    let result = (|| {
        file.write_all(&bytes)
            .map_err(|error| format!("write temporary motion profile: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync temporary motion profile: {error}"))?;
        drop(file);

        if path.exists() {
            let backup = move_to_unique_backup(path, parent, &file_name)?;
            if let Err(error) = rename_without_replacing(&temporary, path) {
                let restore = rename_without_replacing(&backup, path);
                return match restore {
                    Ok(()) => Err(format!("replace motion profile: {error}")),
                    Err(restore_error) => Err(format!(
                        "replace motion profile: {error}; restore backup: {restore_error}"
                    )),
                };
            }
            owns_temporary = false;
            fs::remove_file(&backup)
                .map_err(|error| format!("remove motion profile backup: {error}"))?;
        } else {
            rename_without_replacing(&temporary, path)
                .map_err(|error| format!("install motion profile: {error}"))?;
            owns_temporary = false;
        }
        Ok(())
    })();

    if owns_temporary {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn next_file_nonce() -> String {
    #[cfg(test)]
    if let Some(nonce) = TEST_NONCES.lock().unwrap().pop_front() {
        return nonce;
    }

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}-{timestamp}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

#[cfg(test)]
static TEST_NONCE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
static TEST_NONCES: std::sync::Mutex<std::collections::VecDeque<String>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

#[cfg(test)]
fn set_test_nonce_sequence<const N: usize>(
    nonces: [&str; N],
) -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_NONCE_LOCK.lock().unwrap();
    *TEST_NONCES.lock().unwrap() = nonces.into_iter().map(str::to_owned).collect();
    guard
}

fn create_unique_temporary_file(
    parent: &Path,
    file_name: &str,
) -> Result<(PathBuf, fs::File), String> {
    for _ in 0..32 {
        let temporary = parent.join(format!(".{file_name}.{}.tmp", next_file_nonce()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("create temporary motion profile: {error}")),
        }
    }
    Err("allocate unique temporary motion profile name".into())
}

fn move_to_unique_backup(path: &Path, parent: &Path, file_name: &str) -> Result<PathBuf, String> {
    for _ in 0..32 {
        let backup = parent.join(format!(".{file_name}.{}.bak", next_file_nonce()));
        match rename_without_replacing(path, &backup) {
            Ok(()) => return Ok(backup),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("backup motion profile: {error}")),
        }
    }
    Err("allocate unique motion profile backup name".into())
}

fn rename_without_replacing(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{iter::once, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(once(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(once(0))
        .collect();
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        generate_motion_profile, parse_motion_profile, set_test_nonce_sequence,
        write_motion_profile_atomic, MotionProfileV1, NormalizedPoint, NormalizedRect,
    };

    #[test]
    fn generates_a_bounded_chest_zone_below_the_face_safety_line() {
        let mut image = image::RgbaImage::new(100, 100);
        for y in 5..96 {
            for x in 10..90 {
                image.put_pixel(x, y, image::Rgba([80, 90, 100, 255]));
            }
        }
        let profile = generate_motion_profile(&image).unwrap();
        let safe_top = profile.alpha_bounds.top
            + (profile.alpha_bounds.bottom - profile.alpha_bounds.top) * 0.40;
        assert!(profile.breath_zone.top >= safe_top);
        assert!(profile.breath_zone.bottom <= profile.alpha_bounds.bottom);
        assert!(profile.breath_zone.left >= profile.alpha_bounds.left);
        assert!(profile.breath_zone.right <= profile.alpha_bounds.right);
        assert!(profile.sway_pivot.x >= profile.alpha_bounds.left);
        assert!(profile.sway_pivot.x <= profile.alpha_bounds.right);
    }

    #[test]
    fn rejects_an_empty_alpha_silhouette() {
        let image = image::RgbaImage::new(64, 64);
        assert!(generate_motion_profile(&image)
            .unwrap_err()
            .contains("opaque silhouette"));
    }

    #[test]
    fn rejects_a_breath_zone_that_reaches_the_face() {
        let mut profile = MotionProfileV1 {
            profile_version: 1,
            engine_profile: "life-v1".into(),
            alpha_bounds: NormalizedRect {
                left: 0.1,
                top: 0.05,
                right: 0.9,
                bottom: 0.96,
            },
            breath_zone: NormalizedRect {
                left: 0.2,
                top: 0.50,
                right: 0.8,
                bottom: 0.84,
            },
            sway_pivot: NormalizedPoint { x: 0.5, y: 0.72 },
        };
        profile.breath_zone.top = profile.alpha_bounds.top + 0.01;
        let json = serde_json::to_string(&profile).unwrap();
        assert!(parse_motion_profile(&json)
            .unwrap_err()
            .contains("face safety"));
    }

    #[test]
    fn rejects_invalid_profile_values_and_geometry() {
        let valid = MotionProfileV1 {
            profile_version: 1,
            engine_profile: "life-v1".into(),
            alpha_bounds: NormalizedRect {
                left: 0.1,
                top: 0.05,
                right: 0.9,
                bottom: 0.96,
            },
            breath_zone: NormalizedRect {
                left: 0.2,
                top: 0.5,
                right: 0.8,
                bottom: 0.84,
            },
            sway_pivot: NormalizedPoint { x: 0.5, y: 0.72 },
        };
        for (expected, change) in [
            ("unknown profile version", 0),
            ("unknown engine", 1),
            ("out of range", 2),
            ("inverted rect", 3),
            ("outside alpha", 4),
            ("outside alpha", 5),
        ] {
            let mut profile = valid.clone();
            match change {
                0 => profile.profile_version = 2,
                1 => profile.engine_profile = "other".into(),
                2 => profile.alpha_bounds.right = 1.1,
                3 => profile.alpha_bounds.right = 0.1,
                4 => profile.breath_zone.left = 0.05,
                5 => profile.sway_pivot.x = 0.95,
                _ => unreachable!(),
            }
            let json = serde_json::to_string(&profile).unwrap();
            let error = parse_motion_profile(&json).unwrap_err();
            assert!(error.contains(expected), "expected {expected}, got {error}");
        }
    }

    #[test]
    fn rejects_non_finite_profile_values_before_writing() {
        let profile = MotionProfileV1 {
            profile_version: 1,
            engine_profile: "life-v1".into(),
            alpha_bounds: NormalizedRect {
                left: f32::NAN,
                top: 0.05,
                right: 0.9,
                bottom: 0.96,
            },
            breath_zone: NormalizedRect {
                left: 0.2,
                top: 0.5,
                right: 0.8,
                bottom: 0.84,
            },
            sway_pivot: NormalizedPoint { x: 0.5, y: 0.72 },
        };
        assert!(
            write_motion_profile_atomic(std::path::Path::new("motion-profile.json"), &profile)
                .unwrap_err()
                .contains("non-finite")
        );
    }

    #[test]
    fn atomically_replaces_an_existing_motion_profile() {
        let _nonce_guard =
            set_test_nonce_sequence(["first-temporary", "second-temporary", "second-backup"]);
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-motion-profile-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("motion-profile.json");
        let mut image = image::RgbaImage::new(64, 64);
        for pixel in image.pixels_mut() {
            *pixel = image::Rgba([80, 90, 100, 255]);
        }
        let mut profile = generate_motion_profile(&image).unwrap();
        write_motion_profile_atomic(&path, &profile).unwrap();
        profile.sway_pivot.x = 0.55;
        write_motion_profile_atomic(&path, &profile).unwrap();
        assert_eq!(
            parse_motion_profile(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            profile
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preserves_stale_temp_and_backup_collisions_while_replacing() {
        let _nonce_guard =
            set_test_nonce_sequence(["collision", "fresh-temporary", "collision", "fresh-backup"]);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-motion-profile-collision-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("motion-profile.json");
        let stale_temp = root.join(".motion-profile.json.collision.tmp");
        let stale_backup = root.join(".motion-profile.json.collision.bak");
        std::fs::write(&path, "previous profile").unwrap();
        std::fs::write(&stale_temp, "stale temporary").unwrap();
        std::fs::write(&stale_backup, "stale backup").unwrap();

        let mut image = image::RgbaImage::new(64, 64);
        for pixel in image.pixels_mut() {
            *pixel = image::Rgba([80, 90, 100, 255]);
        }
        let profile = generate_motion_profile(&image).unwrap();
        write_motion_profile_atomic(&path, &profile).unwrap();

        assert_eq!(
            parse_motion_profile(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            profile
        );
        assert_eq!(
            std::fs::read_to_string(stale_temp).unwrap(),
            "stale temporary"
        );
        assert_eq!(
            std::fs::read_to_string(stale_backup).unwrap(),
            "stale backup"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
