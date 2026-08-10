mod creation;
mod generation;
mod pets;
mod platform;
mod preferences;
mod runtime_assets;
mod storage;
mod windowing;

use pets::pet::{IdentityMode, Pet, PetSummary, Species};
use pets::SharedPetRepository;
use pets::{
    active::{
        CommitCompensation, CommitReconciliation, RuntimePetDescriptor, SharedActivePetService,
    },
    catalog::{CreationResume, PetCatalogEntry, PetCatalogService, SharedPetCatalogService},
    deletion::{DeleteOutcome, PetDeletionService, SharedPetDeletionService},
    mutation::{PetMutationGate, SharedPetMutationGate},
};
use platform::{PlatformAdapter, WindowsPlatformAdapter};
use std::sync::{Arc, Mutex};
use tauri::Manager;
use windowing::{normalize_spans, scale_spans, HitRegionEvidence, HitRegionPayload};

struct AppState {
    platform: Arc<dyn PlatformAdapter>,
}

#[tauri::command]
fn probe_version() -> &'static str {
    "m0"
}

#[tauri::command]
fn frontend_ping(message: String) {
    println!("[frontend] {message}");
}

fn pet_asset_relative_path(path: &str) -> Result<String, String> {
    let encoded = path.strip_prefix('/').unwrap_or(path);
    let decoded = percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .map_err(|_| "pet asset path is not valid UTF-8".to_owned())?;
    let relative = runtime_assets::manifest::normalize_relative_path(decoded.as_ref())?;
    let segments = relative.split('/').collect::<Vec<_>>();
    if segments.len() < 3 || segments[1] != "assets" {
        return Err("pet asset path must match <pet_id>/assets/<file>".to_owned());
    }
    Ok(relative)
}

fn serve_pet_asset(file: &std::path::Path) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{
        header::{ACCESS_CONTROL_ALLOW_ORIGIN, CONTENT_TYPE},
        Response, StatusCode,
    };

    match std::fs::read(file) {
        Ok(bytes) => {
            let builder = Response::builder()
                .status(StatusCode::OK)
                .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*");
            let builder = match file.extension().and_then(|extension| extension.to_str()) {
                Some(extension) if extension.eq_ignore_ascii_case("png") => {
                    builder.header(CONTENT_TYPE, "image/png")
                }
                Some(extension) if extension.eq_ignore_ascii_case("json") => {
                    builder.header(CONTENT_TYPE, "application/json")
                }
                _ => builder,
            };
            builder.body(bytes).unwrap()
        }
        Err(_) => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .unwrap(),
    }
}

#[tauri::command]
fn apply_hit_region(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    payload: HitRegionPayload,
) -> Result<HitRegionEvidence, String> {
    let spans = normalize_spans(&payload).map_err(str::to_owned)?;
    let physical_spans = scale_spans(&spans, payload.scale_factor);
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0 as isize;
    state
        .platform
        .apply_hit_region(hwnd, &physical_spans)
        .map_err(|error| error.to_string())?;
    Ok(HitRegionEvidence {
        span_count: physical_spans.len(),
        applied: true,
        strategy: "win32-window-region",
        scale_factor: payload.scale_factor,
    })
}

#[tauri::command]
fn load_preferences(app: tauri::AppHandle) -> Result<preferences::ProbePreferences, String> {
    let path = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?
        .join("m0-preferences.json");
    preferences::load(&path).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_preferences(
    app: tauri::AppHandle,
    value: preferences::ProbePreferences,
) -> Result<(), String> {
    let path = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?
        .join("m0-preferences.json");
    preferences::save(&path, &value).map_err(|error| error.to_string())
}

#[tauri::command]
fn begin_drag(window: tauri::WebviewWindow) -> Result<(), String> {
    window.start_dragging().map_err(|error| error.to_string())
}

#[tauri::command]
fn probe_fullscreen(
    state: tauri::State<'_, AppState>,
) -> Result<platform::FullscreenSnapshot, String> {
    state
        .platform
        .probe_fullscreen(std::process::id())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn parse_manifest(json: String) -> Result<runtime_assets::manifest::RuntimeAssetManifest, String> {
    runtime_assets::manifest::parse_manifest(&json)
}

#[tauri::command]
fn asset_import(
    app: tauri::AppHandle,
    pet_id: String,
    source_path: String,
) -> Result<runtime_assets::importer::ImportedAsset, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let dest = data_dir.join("pets").join(&pet_id).join("assets");
    runtime_assets::importer::import_png_source(&pet_id, std::path::Path::new(&source_path), &dest)
}

#[tauri::command]
fn asset_scan(app: tauri::AppHandle) -> Result<Vec<runtime_assets::loader::AssetHealth>, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    Ok(runtime_assets::loader::scan_assets(&data_dir.join("pets")))
}

#[tauri::command]
fn asset_manifest(
    app: tauri::AppHandle,
    pet_id: String,
) -> Result<runtime_assets::manifest::RuntimeAssetManifest, String> {
    validate_pet_asset_id(&pet_id)?;
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("pets")
        .join(&pet_id)
        .join("assets");
    let json =
        std::fs::read_to_string(root.join("manifest.json")).map_err(|error| error.to_string())?;
    runtime_assets::manifest::parse_manifest(&json)
}

#[tauri::command]
fn asset_file_b64(
    app: tauri::AppHandle,
    pet_id: String,
    relative_path: String,
) -> Result<String, String> {
    use base64::Engine;
    validate_pet_asset_id(&pet_id)?;
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("pets")
        .join(&pet_id)
        .join("assets");
    let json =
        std::fs::read_to_string(root.join("manifest.json")).map_err(|error| error.to_string())?;
    let manifest = runtime_assets::manifest::parse_manifest(&json)?;
    let files = runtime_assets::manifest::manifest_files(&manifest);
    let normalized = runtime_assets::manifest::normalize_relative_path(&relative_path)?;
    if !files.iter().any(|file| file.relative_path == normalized) {
        return Err("asset file is not declared in manifest".into());
    }
    let bytes = std::fs::read(root.join(normalized)).map_err(|error| error.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn validate_pet_asset_id(pet_id: &str) -> Result<(), String> {
    if pet_id.is_empty()
        || !pet_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("invalid petId".into());
    }
    Ok(())
}

#[tauri::command]
fn asset_compile(
    app: tauri::AppHandle,
    store: tauri::State<'_, creation::SharedCreationStore>,
    state: tauri::State<'_, pets::state::SharedStateStore>,
    pet_id: String,
    variant_id: String,
    cutout_path: String,
) -> Result<runtime_assets::compiler::CompileResult, String> {
    let compile_error_key = format!("creation:{pet_id}:compile_error");
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string());
    match data_dir {
        Ok(data_dir) => asset_compile_stored_candidate(
            &data_dir,
            store.inner(),
            state.inner(),
            &pet_id,
            &variant_id,
            &cutout_path,
        ),
        Err(error) => {
            let state = state.lock().map_err(|_| "state lock poisoned")?;
            state.save(&compile_error_key, &error)?;
            Err(error)
        }
    }
}

fn asset_compile_stored_candidate(
    data_dir: &std::path::Path,
    store: &creation::SharedCreationStore,
    state: &pets::state::SharedStateStore,
    pet_id: &str,
    variant_id: &str,
    supplied_cutout_path: &str,
) -> Result<runtime_assets::compiler::CompileResult, String> {
    let compile_error_key = format!("creation:{pet_id}:compile_error");
    let compiled = (|| {
        let candidate = store
            .lock()
            .map_err(|_| "store lock poisoned")?
            .candidate_for_compile(pet_id, variant_id)?;
        let canonical_cutout_path = candidate
            .cutout_path
            .ok_or_else(|| "candidate has no cutout path".to_string())?;
        if supplied_cutout_path != canonical_cutout_path {
            return Err("cutout path does not match the stored candidate".into());
        }
        let canonical_cutout_path = std::path::Path::new(&canonical_cutout_path);
        let motion_profile_path = canonical_cutout_path
            .parent()
            .ok_or_else(|| "candidate cutout path has no parent directory".to_string())?
            .join("motion-profile.json");
        let dest = data_dir.join("pets").join(&pet_id).join("assets");
        runtime_assets::compiler::compile_animated_image(
            pet_id,
            variant_id,
            canonical_cutout_path,
            &motion_profile_path,
            &dest,
        )
    })();

    match compiled {
        Ok(compiled) => {
            let persisted = (|| {
                let store = store.lock().map_err(|_| "store lock poisoned")?;
                store.record_runtime_variant(variant_id, pet_id, &compiled.manifest_path)?;
                let state = state.lock().map_err(|_| "state lock poisoned")?;
                state.remove(&compile_error_key)
            })();
            match persisted {
                Ok(()) => Ok(compiled),
                Err(error) => {
                    let state = state.lock().map_err(|_| "state lock poisoned")?;
                    state.save(&compile_error_key, &error)?;
                    Err(error)
                }
            }
        }
        Err(error) => {
            let state = state.lock().map_err(|_| "state lock poisoned")?;
            state.save(&compile_error_key, &error)?;
            Err(error)
        }
    }
}

#[tauri::command]
fn pet_list(state: tauri::State<'_, SharedPetRepository>) -> Result<Vec<PetSummary>, String> {
    let repo = state.lock().map_err(|_| "pets lock poisoned")?;
    repo.list()
}

#[tauri::command]
fn pet_create(
    state: tauri::State<'_, SharedPetRepository>,
    species: Species,
    identity_mode: IdentityMode,
) -> Result<Pet, String> {
    let repo = state.lock().map_err(|_| "pets lock poisoned")?;
    repo.create(species, identity_mode)
}

#[tauri::command]
fn pet_get(
    state: tauri::State<'_, SharedPetRepository>,
    pet_id: String,
) -> Result<Option<Pet>, String> {
    let repo = state.lock().map_err(|_| "pets lock poisoned")?;
    repo.get(&pet_id)
}

#[tauri::command]
fn pet_delete_full(
    state: tauri::State<'_, SharedPetDeletionService>,
    pet_id: String,
) -> Result<DeleteOutcome, String> {
    state.delete(&pet_id)
}

#[tauri::command]
fn pet_get_active(state: tauri::State<'_, SharedActivePetService>) -> Result<String, String> {
    state.active()
}

#[tauri::command]
fn pet_catalog_list(
    state: tauri::State<'_, SharedPetCatalogService>,
) -> Result<Vec<PetCatalogEntry>, String> {
    state.list()
}

#[tauri::command]
fn pet_creation_resume(
    state: tauri::State<'_, SharedPetCatalogService>,
    pet_id: String,
) -> Result<CreationResume, String> {
    state.creation_resume(&pet_id)
}

#[tauri::command]
fn creation_start(
    service: tauri::State<'_, creation::SharedCreationService>,
    method: creation::domain::CreationMethod,
) -> Result<creation::domain::CreationSnapshot, String> {
    service.start(method)
}

#[tauri::command]
fn creation_draft(
    service: tauri::State<'_, creation::SharedCreationService>,
) -> Result<Option<creation::domain::CreationSnapshot>, String> {
    service.draft()
}

#[tauri::command]
fn creation_snapshot(
    service: tauri::State<'_, creation::SharedCreationService>,
    session_id: String,
) -> Result<creation::domain::CreationSnapshot, String> {
    service.snapshot(&session_id)
}

#[tauri::command]
fn creation_set_name(
    service: tauri::State<'_, creation::SharedCreationService>,
    session_id: String,
    display_name: String,
) -> Result<creation::domain::CreationSnapshot, String> {
    service.set_name(&session_id, &display_name)
}

#[tauri::command]
fn creation_abandon(
    service: tauri::State<'_, creation::SharedCreationService>,
    session_id: String,
) -> Result<(), String> {
    service.abandon(&session_id)
}

#[tauri::command]
fn creation_prepare_finalize(
    service: tauri::State<'_, creation::finalization::SharedCreationFinalizationService>,
    session_id: String,
    request_id: String,
) -> Result<creation::domain::PreparedCreation, String> {
    service.prepare(&session_id, &request_id)
}

#[tauri::command]
fn creation_abort_finalize(
    service: tauri::State<'_, creation::finalization::SharedCreationFinalizationService>,
    session_id: String,
    error: String,
) -> Result<creation::domain::CreationSnapshot, String> {
    service.abort(&session_id, &error)
}

#[tauri::command]
fn creation_recover_finalization(
    service: tauri::State<'_, creation::finalization::SharedCreationFinalizationService>,
) -> Result<creation::finalization::RecoveryReport, String> {
    service.recover()
}

#[tauri::command]
fn pet_prepare_switch(
    state: tauri::State<'_, SharedActivePetService>,
    request_id: String,
    pet_id: String,
) -> Result<RuntimePetDescriptor, String> {
    state.prepare(Some(&request_id), &pet_id)
}

#[tauri::command]
fn pet_prepare_startup(
    state: tauri::State<'_, SharedActivePetService>,
    pet_id: String,
) -> Result<RuntimePetDescriptor, String> {
    state.prepare_startup(&pet_id)
}

#[tauri::command]
fn pet_commit_switch(
    state: tauri::State<'_, SharedActivePetService>,
    request_id: String,
    pet_id: String,
    accepted_variant_id: Option<String>,
    creation_session_id: Option<String>,
) -> Result<(), String> {
    state.commit_switch(
        &request_id,
        &pet_id,
        accepted_variant_id.as_deref(),
        creation_session_id.as_deref(),
    )
}

#[tauri::command]
fn pet_rollback_switch(
    state: tauri::State<'_, SharedActivePetService>,
    request_id: String,
    previous_pet_id: String,
    pet_id: String,
    accepted_variant_id: Option<String>,
    creation_session_id: Option<String>,
) -> Result<CommitCompensation, String> {
    state.rollback_switch(
        &request_id,
        &previous_pet_id,
        &pet_id,
        accepted_variant_id.as_deref(),
        creation_session_id.as_deref(),
    )
}

#[tauri::command]
fn pet_reconcile_switch_commit(
    state: tauri::State<'_, SharedActivePetService>,
    request_id: String,
    previous_pet_id: String,
    pet_id: String,
    accepted_variant_id: Option<String>,
    creation_session_id: Option<String>,
) -> Result<CommitReconciliation, String> {
    state.reconcile_commit(
        &request_id,
        &previous_pet_id,
        &pet_id,
        accepted_variant_id.as_deref(),
        creation_session_id.as_deref(),
    )
}

#[tauri::command]
fn pet_cancel_switch(
    state: tauri::State<'_, SharedActivePetService>,
    request_id: String,
) -> Result<(), String> {
    state.cancel(&request_id)
}

#[tauri::command]
fn pet_finish_switch(
    state: tauri::State<'_, SharedActivePetService>,
    request_id: String,
) -> Result<(), String> {
    state.finish(&request_id)
}

#[tauri::command]
fn pet_state_load(
    state: tauri::State<'_, pets::state::SharedStateStore>,
    pet_id: String,
) -> Result<Option<String>, String> {
    let store = state.lock().map_err(|_| "state lock poisoned")?;
    store.load(&format!("pet:{pet_id}:behavior"))
}

#[tauri::command]
fn pet_state_save(
    state: tauri::State<'_, pets::state::SharedStateStore>,
    pet_id: String,
    value: String,
) -> Result<(), String> {
    let store = state.lock().map_err(|_| "state lock poisoned")?;
    store.save(&format!("pet:{pet_id}:behavior"), &value)
}

#[tauri::command]
fn app_setting_get(
    state: tauri::State<'_, pets::state::SharedStateStore>,
    key: String,
) -> Result<Option<String>, String> {
    let store = state.lock().map_err(|_| "state lock poisoned")?;
    store.load(&format!("app:{key}"))
}

#[tauri::command]
fn app_setting_set(
    state: tauri::State<'_, pets::state::SharedStateStore>,
    key: String,
    value: String,
) -> Result<(), String> {
    let store = state.lock().map_err(|_| "state lock poisoned")?;
    store.save(&format!("app:{key}"), &value)
}

#[tauri::command]
fn pet_calibration_load(
    state: tauri::State<'_, pets::state::SharedStateStore>,
) -> Result<Option<String>, String> {
    let store = state.lock().map_err(|_| "state lock poisoned")?;
    store.load("pet:probe:calibration")
}

#[tauri::command]
fn pet_calibration_save(
    state: tauri::State<'_, pets::state::SharedStateStore>,
    value: serde_json::Value,
) -> Result<(), String> {
    let store = state.lock().map_err(|_| "state lock poisoned")?;
    store.save(
        "pet:probe:calibration",
        &serde_json::to_string(&value).map_err(|error| error.to_string())?,
    )
}

#[tauri::command]
fn gen_start(
    manager: tauri::State<'_, generation::tasks::SharedGenerationManager>,
    pet_id: String,
    prompt: String,
    ref_png_b64: String,
    ref_sha256: String,
) -> Result<String, String> {
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD
        .decode(ref_png_b64)
        .map_err(|error| format!("bad base64: {error}"))?;
    manager.start(&pet_id, &prompt, &png, &ref_sha256)
}

#[tauri::command]
fn creation_upload_start(
    manager: tauri::State<'_, generation::tasks::SharedGenerationManager>,
    session_id: String,
    prompt: String,
    ref_png_b64: String,
    ref_sha256: String,
) -> Result<String, String> {
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD
        .decode(ref_png_b64)
        .map_err(|error| format!("bad base64: {error}"))?;
    manager.start_for_session(&session_id, &prompt, &png, &ref_sha256)
}

#[tauri::command]
fn creation_upload_jobs(
    store: tauri::State<'_, creation::SharedCreationStore>,
    session_id: String,
) -> Result<Vec<creation::JobRecord>, String> {
    let store = store.lock().map_err(|_| "store lock poisoned")?;
    store.upload_jobs(&session_id)
}

#[tauri::command]
fn gen_cancel(
    manager: tauri::State<'_, generation::tasks::SharedGenerationManager>,
    job_id: String,
) -> Result<(), String> {
    manager.cancel(&job_id)
}

#[tauri::command]
fn gen_list(
    store: tauri::State<'_, creation::SharedCreationStore>,
    pet_id: String,
) -> Result<Vec<creation::JobRecord>, String> {
    let store = store.lock().map_err(|_| "store lock poisoned")?;
    store.job_list(&pet_id)
}

#[tauri::command]
fn gen_resume(
    manager: tauri::State<'_, generation::tasks::SharedGenerationManager>,
) -> Result<usize, String> {
    manager.resume()
}

fn generation_job_file(
    data_dir: &std::path::Path,
    job_id: &str,
    file_name: &str,
) -> Result<std::path::PathBuf, String> {
    if job_id.is_empty()
        || !job_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("invalid jobId".into());
    }
    if !matches!(file_name, "cutout.png" | "motion-profile.json") {
        return Err("invalid generation file name".into());
    }
    Ok(data_dir.join("jobs").join(job_id).join(file_name))
}

#[tauri::command]
fn gen_cutout_path(app: tauri::AppHandle, job_id: String) -> Result<String, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let cutout = generation_job_file(&data_dir, &job_id, "cutout.png")?;
    if cutout.exists() {
        Ok(cutout.to_string_lossy().to_string())
    } else {
        Err("cutout not ready yet".into())
    }
}

#[tauri::command]
fn gen_cutout_b64(app: tauri::AppHandle, job_id: String) -> Result<String, String> {
    use base64::Engine;
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let cutout = generation_job_file(&data_dir, &job_id, "cutout.png")?;
    let bytes = std::fs::read(&cutout).map_err(|error| error.to_string())?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[tauri::command]
fn gen_motion_profile(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<runtime_assets::motion_profile::MotionProfileV1, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let profile = generation_job_file(&data_dir, &job_id, "motion-profile.json")?;
    let json = std::fs::read_to_string(profile).map_err(|error| error.to_string())?;
    runtime_assets::motion_profile::parse_motion_profile(&json)
}

#[tauri::command]
fn debug_windows(app: tauri::AppHandle) -> Vec<String> {
    app.webview_windows()
        .keys()
        .map(|label| {
            let window = app.get_webview_window(label);
            let visible = window
                .as_ref()
                .and_then(|w| w.is_visible().ok())
                .unwrap_or(false);
            format!("{label} visible={visible}")
        })
        .collect()
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::{
        menu::{Menu, MenuItem},
        tray::TrayIconBuilder,
    };

    let mode = MenuItem::with_id(app, "mode", "陪伴模式（置顶）", false, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "显示或隐藏", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
    let calibration = MenuItem::with_id(app, "calibration", "校准", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&mode, &toggle, &settings, &calibration, &quit])?;
    let mut builder = TrayIconBuilder::new().menu(&menu);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => {
                if let Some(window) = app.get_webview_window("pet") {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        let _ = window.show();
                    }
                }
            }
            "settings" => {
                if let Some(window) = app.get_webview_window("settings") {
                    let _ = window.show();
                    let _ = window.set_focus();
                } else {
                    match tauri::WebviewWindowBuilder::new(
                        app,
                        "settings",
                        tauri::WebviewUrl::App("settings.html".into()),
                    )
                    .title("桌面宠物设置")
                    .inner_size(720.0, 520.0)
                    .additional_browser_args("--disable-gpu")
                    .build()
                    {
                        Ok(window) => {
                            let _ = window.center();
                            let _ = window.show();
                            let _ = window.set_focus();
                            println!("[desktop-pet] settings window created");
                        }
                        Err(error) => {
                            println!("[desktop-pet] settings window FAILED: {error}");
                        }
                    }
                }
            }
            "calibration" => {
                if let Some(window) = app.get_webview_window("calibration") {
                    let _ = window.show();
                    let _ = window.set_focus();
                } else if let Ok(window) = tauri::WebviewWindowBuilder::new(
                    app,
                    "calibration",
                    tauri::WebviewUrl::App("calibration.html".into()),
                )
                .title("宠物校准")
                .inner_size(420.0, 460.0)
                .resizable(false)
                .additional_browser_args("--disable-gpu")
                .build()
                {
                    let _ = window.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
fn run_startup_recovery<Cleanup, Recover, Restore>(
    cleanup_quarantine: Cleanup,
    recover_finalization: Recover,
    restore_active: Restore,
) -> Result<creation::finalization::RecoveryReport, String>
where
    Cleanup: FnOnce() -> Result<(), String>,
    Recover: FnOnce() -> Result<creation::finalization::RecoveryReport, String>,
    Restore: FnOnce() -> Result<(), String>,
{
    let cleanup_warning = cleanup_quarantine().err();
    let mut recovery = recover_finalization()?;
    if let Some(error) = cleanup_warning {
        recovery
            .warnings
            .push(format!("quarantine cleanup failed: {error}"));
    }
    restore_active()?;
    Ok(recovery)
}

pub fn run() {
    tauri::Builder::default()
        .register_uri_scheme_protocol("pet-asset", |ctx, request| {
            use tauri::http::{Response, StatusCode};

            let app = ctx.app_handle();
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::new());
            let relative = match pet_asset_relative_path(request.uri().path()) {
                Ok(relative) => relative,
                Err(_) => {
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Vec::new())
                        .unwrap();
                }
            };
            // Tauri maps the custom protocol request path to <pet_id>/assets/<file>.
            let file = data_dir.join("pets").join(relative);
            serve_pet_asset(&file)
        })
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| error.to_string())?;
            let pets_dir = data_dir.join("pets");
            let storage = Arc::new(Mutex::new(storage::Storage::open(&pets_dir)?));
            let migration = runtime_assets::migration::migrate_all_v1_assets(&pets_dir);
            for failure in &migration.failures {
                eprintln!(
                    "[desktop-pet] pet motion migration failed: {}: {}",
                    failure.pet_id, failure.error
                );
            }
            let session = Arc::new(Mutex::new(pets::ActivePetSession::new()));
            let mutation_gate = Arc::new(PetMutationGate::new(std::time::Duration::from_secs(60)));
            let active = Arc::new(pets::active::ActivePetService::new(
                storage.clone(),
                session,
                pets_dir.clone(),
                mutation_gate.clone(),
            ));
            let deletion = Arc::new(PetDeletionService::new(
                storage.clone(),
                active.clone(),
                data_dir.clone(),
                mutation_gate.clone(),
            ));
            let finalization = Arc::new(
                creation::finalization::CreationFinalizationService::new(
                    storage.clone(),
                    data_dir.clone(),
                    data_dir.join("jobs"),
                    mutation_gate.clone(),
                    active.switch_transaction(),
                )
                .with_deletion(deletion.clone()),
            );
            let recovery = run_startup_recovery(
                || deletion.cleanup_quarantine(),
                || finalization.recover(),
                || active.restore().map(|_| ()),
            )?;
            for warning in recovery.warnings {
                eprintln!("[desktop-pet] creation finalization recovery: {warning}");
            }
            let catalog = Arc::new(PetCatalogService::new(
                storage.clone(),
                active.clone(),
                pets_dir,
            ));
            app.manage(active.clone() as SharedActivePetService);
            app.manage(mutation_gate.clone() as SharedPetMutationGate);
            app.manage(finalization as creation::finalization::SharedCreationFinalizationService);
            app.manage(catalog as SharedPetCatalogService);
            let creation_service = Arc::new(creation::CreationService::new(
                storage.clone(),
                data_dir.clone(),
                deletion.clone(),
            ));
            app.manage(deletion as SharedPetDeletionService);
            app.manage(creation_service as creation::SharedCreationService);
            app.manage(Arc::new(Mutex::new(pets::repository::PetRepository::new(
                storage.clone(),
            ))) as SharedPetRepository);
            let state_store = Arc::new(Mutex::new(pets::state::StateStore::new(storage.clone())));
            app.manage(state_store.clone() as pets::state::SharedStateStore);

            let creation_store =
                Arc::new(Mutex::new(creation::CreationStore::new(storage.clone())));
            app.manage(creation_store.clone() as creation::SharedCreationStore);

            let manager = generation::tasks::GenerationManager::new(
                creation_store,
                state_store,
                Arc::from(data_dir.join("jobs").as_path()),
                mutation_gate,
            );
            let manager = Arc::new(manager);
            app.manage(manager.clone() as generation::tasks::SharedGenerationManager);

            // background polling thread for generation jobs
            {
                let manager = manager.clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    // poll_all uses its own temporary runtime internally; do not
                    // wrap in async_runtime::block_on (nested runtime panic)
                    let _ = manager.poll_all();
                });
            }

            let window = app.get_webview_window("pet").ok_or("pet window missing")?;
            let hwnd = window.hwnd()?.0 as isize;
            app.state::<AppState>()
                .platform
                .configure_pet_window(hwnd)
                .map_err(|error| error.to_string())?;
            window.set_always_on_top(true)?;
            build_tray(app)?;
            Ok(())
        })
        .manage(AppState {
            platform: Arc::new(WindowsPlatformAdapter),
        })
        .invoke_handler(tauri::generate_handler![
            probe_version,
            frontend_ping,
            apply_hit_region,
            load_preferences,
            save_preferences,
            begin_drag,
            probe_fullscreen,
            parse_manifest,
            asset_import,
            asset_scan,
            asset_manifest,
            asset_file_b64,
            asset_compile,
            pet_list,
            pet_create,
            pet_get,
            pet_delete_full,
            pet_get_active,
            pet_catalog_list,
            pet_creation_resume,
            pet_prepare_switch,
            pet_prepare_startup,
            pet_commit_switch,
            pet_rollback_switch,
            pet_reconcile_switch_commit,
            pet_cancel_switch,
            pet_finish_switch,
            pet_state_load,
            pet_state_save,
            app_setting_get,
            app_setting_set,
            pet_calibration_load,
            pet_calibration_save,
            creation_start,
            creation_draft,
            creation_snapshot,
            creation_set_name,
            creation_abandon,
            creation_prepare_finalize,
            creation_abort_finalize,
            creation_recover_finalization,
            creation_upload_start,
            creation_upload_jobs,
            gen_start,
            gen_cancel,
            gen_list,
            gen_resume,
            gen_cutout_path,
            gen_cutout_b64,
            gen_motion_profile,
            debug_windows
        ])
        .run(tauri::generate_context!())
        .expect("failed to run desktop pet runtime");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pets::state::StateStore;
    use crate::storage::Storage;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    #[test]
    fn public_switch_write_commands_require_request_ids() {
        let source = include_str!("lib.rs");
        for command in [
            "pet_prepare_switch",
            "pet_commit_switch",
            "pet_rollback_switch",
        ] {
            let start = source.find(&format!("fn {command}(")).unwrap();
            let signature_end = source[start..].find(") ->").unwrap() + start;
            let signature = &source[start..signature_end];
            assert!(
                signature.contains("request_id: String"),
                "{command} must require requestId"
            );
            assert!(!signature.contains("request_id: Option<String>"));
        }
    }

    fn asset_compile_fixture() -> (
        creation::SharedCreationStore,
        pets::state::SharedStateStore,
        std::sync::Arc<std::sync::Mutex<Storage>>,
        std::path::PathBuf,
        String,
        String,
        String,
    ) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-asset-command-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let storage = std::sync::Arc::new(std::sync::Mutex::new(Storage::open(&root).unwrap()));
        let repo = pets::repository::PetRepository::new(storage.clone());
        let pet = repo
            .create(pets::pet::Species::Cat, pets::pet::IdentityMode::RealPet)
            .unwrap();
        let other_pet = repo
            .create(pets::pet::Species::Dog, pets::pet::IdentityMode::Adopted)
            .unwrap();
        let store = std::sync::Arc::new(std::sync::Mutex::new(creation::CreationStore::new(
            storage.clone(),
        )));
        store
            .lock()
            .unwrap()
            .create_job("job-1", &pet.pet_id, "p", "h", Some("task-1"))
            .unwrap();
        let canonical_cutout = root.join("jobs").join("job-1").join("cutout.png");
        store
            .lock()
            .unwrap()
            .record_candidate(
                "job-1",
                &pet.pet_id,
                "raw.png",
                &canonical_cutout.to_string_lossy(),
                "acceptable",
            )
            .unwrap();
        let state = std::sync::Arc::new(std::sync::Mutex::new(StateStore::new(storage.clone())));
        (
            store,
            state,
            storage,
            root,
            pet.pet_id,
            other_pet.pet_id,
            canonical_cutout.to_string_lossy().to_string(),
        )
    }

    #[test]
    fn probe_version_is_m0() {
        assert_eq!(super::probe_version(), "m0");
    }

    #[test]
    fn pet_catalog_commands_are_available() {
        let _list = super::pet_catalog_list;
        let _resume = super::pet_creation_resume;
    }

    #[test]
    fn creation_session_commands_are_available() {
        let _start = super::creation_start;
        let _draft = super::creation_draft;
        let _snapshot = super::creation_snapshot;
        let _set_name = super::creation_set_name;
        let _abandon = super::creation_abandon;
    }

    #[test]
    fn creation_finalization_commands_are_available() {
        let _prepare = super::creation_prepare_finalize;
        let _abort = super::creation_abort_finalize;
        let _recover = super::creation_recover_finalization;
    }

    #[test]
    fn startup_recovers_quarantine_and_finalization_before_restoring_the_active_pet() {
        let events = std::sync::Mutex::new(Vec::new());

        let report = super::run_startup_recovery(
            || {
                events.lock().unwrap().push("quarantine");
                Ok(())
            },
            || {
                events.lock().unwrap().push("finalization");
                Ok(creation::finalization::RecoveryReport::default())
            },
            || {
                events.lock().unwrap().push("active");
                Ok(())
            },
        )
        .unwrap();

        assert!(report.warnings.is_empty());
        assert_eq!(
            *events.lock().unwrap(),
            vec!["quarantine", "finalization", "active"]
        );
    }

    #[test]
    fn creation_appearance_variant_keeps_its_legacy_public_path() {
        let variant = creation::AppearanceVariant {
            variant_id: "variant-legacy".into(),
            pet_id: "pet-legacy".into(),
            job_id: Some("job-legacy".into()),
            image_path: "image.png".into(),
            cutout_path: Some("cutout.png".into()),
            quality: "acceptable".into(),
            accepted: false,
            created_at: "0".into(),
        };

        assert_eq!(variant.variant_id, "variant-legacy");
        assert_eq!(variant.job_id.as_deref(), Some("job-legacy"));
    }

    #[test]
    fn pet_asset_png_response_is_decodable_media_with_its_original_bytes() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-uri-response-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("body.PNG");
        let png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        std::fs::write(&file, &png).unwrap();

        let response = serve_pet_asset(&file);

        assert_eq!(response.status(), tauri::http::StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(tauri::http::header::CONTENT_TYPE)
                .unwrap(),
            "image/png"
        );
        assert_eq!(
            response
                .headers()
                .get(tauri::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "*"
        );
        assert_eq!(response.body(), &png);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pet_asset_json_response_preserves_cors_with_json_content_type() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-uri-json-response-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("motion-profile.JSON");
        let json = br#"{"profileVersion":1}"#.to_vec();
        std::fs::write(&file, &json).unwrap();

        let response = serve_pet_asset(&file);

        assert_eq!(response.status(), tauri::http::StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(tauri::http::header::CONTENT_TYPE)
                .unwrap(),
            "application/json"
        );
        assert_eq!(
            response
                .headers()
                .get(tauri::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "*"
        );
        assert_eq!(response.body(), &json);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pet_asset_path_decodes_the_separator_encoded_by_convert_file_src() {
        assert_eq!(
            pet_asset_relative_path("/pet-user-1%2Fassets%2Fbody.png").unwrap(),
            "pet-user-1/assets/body.png"
        );
    }

    #[test]
    fn pet_asset_path_rejects_encoded_traversal() {
        assert!(pet_asset_relative_path("/pet-user-1%2Fassets%2F..%2Fsecret.png").is_err());
        assert!(pet_asset_relative_path("/pet-user-1%2Fassets%2F..%5Csecret.png").is_err());
    }

    #[test]
    fn pet_asset_path_cannot_read_files_outside_a_pet_assets_directory() {
        assert!(pet_asset_relative_path("/desktop-pet.db").is_err());
        assert!(pet_asset_relative_path("/pet-user-1%2Fmanifest.json").is_err());
        assert!(pet_asset_relative_path("/pet-user-1%2Fassets").is_err());
    }

    #[test]
    fn asset_compile_rejects_a_mismatched_cutout_path() {
        let (store, state, _storage, root, pet_id, _other_pet_id, canonical_cutout) =
            asset_compile_fixture();
        let result = asset_compile_stored_candidate(
            &root,
            &store,
            &state,
            &pet_id,
            "job-1",
            &root.join("untrusted.png").to_string_lossy(),
        );
        assert!(result.is_err());
        assert_eq!(
            state
                .lock()
                .unwrap()
                .load(&format!("creation:{pet_id}:compile_error"))
                .unwrap()
                .is_some(),
            true
        );
        assert!(canonical_cutout.ends_with("cutout.png"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn asset_compile_rejects_a_variant_owned_by_another_pet() {
        let (store, state, _storage, root, _pet_id, other_pet_id, canonical_cutout) =
            asset_compile_fixture();
        assert!(asset_compile_stored_candidate(
            &root,
            &store,
            &state,
            &other_pet_id,
            "job-1",
            &canonical_cutout,
        )
        .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn asset_compile_rejects_a_variant_with_a_different_job() {
        let (store, state, storage, root, pet_id, _other_pet_id, canonical_cutout) =
            asset_compile_fixture();
        {
            let db = &storage.lock().unwrap().db;
            db.execute(
                "INSERT INTO appearance_variants
                 (variant_id, pet_id, job_id, image_path, cutout_path, quality, accepted, created_at)
                 VALUES ('job-2', ?1, 'job-1', 'raw.png', ?2, 'acceptable', 0, ?3)",
                rusqlite::params![&pet_id, &canonical_cutout, creation::profiles::now_iso()],
            )
            .unwrap();
        }
        assert!(asset_compile_stored_candidate(
            &root,
            &store,
            &state,
            &pet_id,
            "job-2",
            &canonical_cutout,
        )
        .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn asset_compile_rejects_a_missing_motion_profile_without_recording_runtime() {
        let (store, state, storage, root, pet_id, _other_pet_id, canonical_cutout) =
            asset_compile_fixture();
        let profile = std::path::Path::new(&canonical_cutout)
            .parent()
            .unwrap()
            .join("motion-profile.json");
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        let rgba = image::RgbaImage::from_pixel(64, 64, image::Rgba([80, 90, 100, 255]));
        rgba.save(&canonical_cutout).unwrap();
        let value = runtime_assets::motion_profile::generate_motion_profile(&rgba).unwrap();
        runtime_assets::motion_profile::write_motion_profile_atomic(&profile, &value).unwrap();
        std::fs::remove_file(profile).unwrap();

        assert!(asset_compile_stored_candidate(
            &root,
            &store,
            &state,
            &pet_id,
            "job-1",
            &canonical_cutout,
        )
        .is_err());
        let runtime_count: i64 = storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM variants WHERE pet_id = ?1",
                rusqlite::params![pet_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(runtime_count, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn generation_candidate_files_reject_path_like_job_ids() {
        let root = std::path::Path::new("C:/app-data");
        for job_id in ["../outside", "a/b", r"a\b", "", "."] {
            assert!(generation_job_file(root, job_id, "cutout.png").is_err());
            assert!(generation_job_file(root, job_id, "motion-profile.json").is_err());
        }
        let cutout = generation_job_file(root, "job-1_ok", "cutout.png").unwrap();
        let profile = generation_job_file(root, "job-1_ok", "motion-profile.json").unwrap();
        assert_eq!(cutout.parent(), profile.parent());
        assert!(cutout.ends_with("jobs/job-1_ok/cutout.png"));
        assert!(generation_job_file(root, "job-1_ok", "../secret").is_err());
    }
}
