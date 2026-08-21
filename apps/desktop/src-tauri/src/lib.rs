mod creation;
mod generation;
mod pets;
mod platform;
mod preferences;
mod runtime_assets;
mod storage;
#[cfg(test)]
mod test_support;
mod window_mode;
mod windowing;

use pets::calibration::PetCalibrationV1;
use pets::pet::{Pet, PetSummary};
use pets::profile::{PetProfile, PetProfileUpdate};
use pets::SharedPetRepository;
use pets::{
    active::{
        CommitCompensation, CommitReconciliation, RuntimePetDescriptor, SharedActivePetService,
    },
    catalog::{PetCatalogEntry, PetCatalogService, SharedPetCatalogService},
    deletion::{DeleteOutcome, PetDeletionService, SharedPetDeletionService},
    mutation::{MutationKind, PetMutationGate, SharedPetMutationGate},
};
use platform::{PlatformAdapter, WindowsPlatformAdapter};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tauri::{Emitter, Manager};
use windowing::{normalize_spans, scale_spans, HitRegionEvidence, HitRegionPayload};

macro_rules! with_tauri_commands {
    ($consumer:ident) => {
        $consumer!(
            probe_version,
            frontend_ping,
            apply_hit_region,
            load_preferences,
            save_preferences,
            begin_drag,
            probe_fullscreen,
            window_fullscreen_update,
            window_visibility_reconcile,
            window_mode_get,
            window_mode_set,
            window_visibility_set,
            window_mode_runtime_ack,
            window_mode_runtime_ready,
            parse_manifest,
            asset_import,
            asset_scan,
            asset_manifest,
            asset_file_b64,
            asset_compile,
            pet_list,
            pet_get,
            pet_profile_get,
            pet_profile_update,
            pet_delete_full,
            pet_get_active,
            pet_catalog_list,
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
            settings_take_pending_navigation,
            pet_calibration_load,
            pet_calibration_save,
            creation_start,
            creation_draft,
            creation_snapshot,
            creation_set_name,
            creation_composer_save,
            creation_composer_candidate,
            creation_adoption_catalog,
            creation_adoption_start,
            creation_abandon,
            creation_prepare_finalize,
            creation_abort_finalize,
            creation_recover_finalization,
            creation_upload_start,
            creation_upload_retry,
            creation_upload_jobs,
            creation_upload_source,
            creation_upload_candidate_assets,
            creation_photo_avatar_consent,
            creation_photo_avatar_begin,
            creation_photo_avatar_status,
            creation_photo_avatar_cancel,
            creation_photo_avatar_regenerate,
            creation_photo_avatar_revise,
            creation_photo_avatar_runtime_check_passed,
            creation_photo_avatar_preview_manifest,
            creation_photo_avatar_preview_file_b64,
            gen_start,
            gen_cancel,
            gen_list,
            gen_resume,
            gen_cutout_path,
            gen_cutout_b64,
            gen_motion_profile,
            debug_windows,
        )
    };
}

macro_rules! build_tauri_handler {
    ($($command:ident),+ $(,)?) => {
        tauri::generate_handler![$($command),+]
    };
}

#[cfg(test)]
macro_rules! collect_tauri_command_names {
    ($($command:ident),+ $(,)?) => {
        &[$(stringify!($command)),+]
    };
}

#[cfg(test)]
const REGISTERED_TAURI_COMMAND_NAMES: &[&str] = with_tauri_commands!(collect_tauri_command_names);

struct AppState {
    platform: Arc<dyn PlatformAdapter>,
    preferences_lock: Arc<Mutex<()>>,
    companion_visibility: Mutex<CompanionVisibilityTracker>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompanionVisibilityAction {
    None,
    Reassert,
    DiagnoseAndReassert,
}

#[derive(Default)]
struct CompanionVisibilityTracker {
    shell_cloak_episode_handled: bool,
}

impl CompanionVisibilityTracker {
    fn decide(
        &mut self,
        expected_visible: bool,
        facts: platform::WindowVisibilityFacts,
    ) -> CompanionVisibilityAction {
        if !expected_visible {
            self.shell_cloak_episode_handled = false;
            return CompanionVisibilityAction::None;
        }
        if facts.shell_cloaked {
            if self.shell_cloak_episode_handled {
                return CompanionVisibilityAction::None;
            }
            self.shell_cloak_episode_handled = true;
            return CompanionVisibilityAction::DiagnoseAndReassert;
        }
        self.shell_cloak_episode_handled = false;
        if !facts.visible || !facts.topmost {
            CompanionVisibilityAction::Reassert
        } else {
            CompanionVisibilityAction::None
        }
    }
}

#[derive(Default)]
struct SettingsNavigationState(Mutex<Option<String>>);

impl SettingsNavigationState {
    fn publish(&self, section: &str) -> Result<(), String> {
        if section != "calibration" {
            return Err(format!("unsupported settings section: {section}"));
        }
        *self
            .0
            .lock()
            .map_err(|_| "settings navigation lock poisoned".to_owned())? =
            Some(section.to_owned());
        Ok(())
    }

    fn take(&self) -> Result<Option<String>, String> {
        Ok(self
            .0
            .lock()
            .map_err(|_| "settings navigation lock poisoned".to_owned())?
            .take())
    }
}

#[tauri::command]
fn probe_version() -> &'static str {
    "m0"
}

#[tauri::command]
fn frontend_ping(message: String) {
    println!("[frontend] {message}");
}

#[tauri::command]
fn settings_take_pending_navigation(
    navigation: tauri::State<'_, SettingsNavigationState>,
) -> Result<Option<String>, String> {
    navigation.take()
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
fn load_preferences(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<preferences::ProbePreferences, String> {
    let path = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?
        .join("m0-preferences.json");
    let _lease = state
        .preferences_lock
        .lock()
        .map_err(|_| "preferences lock poisoned".to_owned())?;
    preferences::load(&path).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_preferences(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    value: preferences::ProbePreferences,
) -> Result<(), String> {
    let path = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?
        .join("m0-preferences.json");
    // Geometry/scale callers may hold a stale mode snapshot. Only the window-mode
    // controller owns mode and manual-visibility persistence.
    let _lease = state
        .preferences_lock
        .lock()
        .map_err(|_| "preferences lock poisoned".to_owned())?;
    preferences::save_preserving_window_intent(&path, value).map_err(|error| error.to_string())
}

#[tauri::command]
fn begin_drag(window: tauri::WebviewWindow) -> Result<(), String> {
    window.start_dragging().map_err(|error| error.to_string())
}

#[tauri::command]
fn probe_fullscreen(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<platform::FullscreenSnapshot, String> {
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0 as isize;
    let snapshot = state
        .platform
        .probe_fullscreen(std::process::id(), hwnd)
        .map_err(|error| error.to_string())?;
    Ok(snapshot)
}

#[tauri::command]
async fn window_fullscreen_update(
    controller: tauri::State<'_, window_mode::SharedWindowModeController>,
    active: bool,
) -> Result<window_mode::WindowModeSnapshot, String> {
    let controller = controller.inner().clone();
    tauri::async_runtime::spawn_blocking(move || controller.fullscreen_changed(active))
        .await
        .map_err(|error| format!("window fullscreen task failed: {error}"))?
}

#[tauri::command]
fn window_mode_get(
    controller: tauri::State<'_, window_mode::SharedWindowModeController>,
) -> Result<window_mode::WindowModeSnapshot, String> {
    controller.snapshot()
}

#[tauri::command]
async fn window_mode_set(
    app: tauri::AppHandle,
    controller: tauri::State<'_, window_mode::SharedWindowModeController>,
    request_id: String,
    mode: windowing::WindowMode,
) -> Result<window_mode::WindowModeSnapshot, String> {
    let mode = window_mode::canonical_public_mode(mode);
    let controller = controller.inner().clone();
    let cancel_controller = controller.clone();
    tauri::async_runtime::spawn_blocking(move || {
        cancel_controller.cancel_startup_restore_and_wait()
    })
    .await
    .map_err(|error| format!("startup mode cancellation task failed: {error}"))??;
    let tray = app.state::<TrayModeMenu>();
    let canonical = controller.snapshot().ok();
    let tray_lease = tray
        .try_begin_transition(canonical.as_ref())
        .ok_or_else(|| "window mode transition is in progress".to_owned())?;
    let transition_controller = controller.clone();
    let result = match tauri::async_runtime::spawn_blocking(move || {
        transition_controller.set_mode(request_id, mode)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(format!("window mode transition task failed: {error}")),
    };
    app.state::<TrayModeMenu>()
        .finish_transition(tray_lease, &controller);
    result
}

#[tauri::command]
async fn window_visibility_set(
    controller: tauri::State<'_, window_mode::SharedWindowModeController>,
    visible: bool,
) -> Result<window_mode::WindowModeSnapshot, String> {
    let controller = controller.inner().clone();
    tauri::async_runtime::spawn_blocking(move || controller.set_user_visible(visible))
        .await
        .map_err(|error| format!("window visibility task failed: {error}"))?
}

#[tauri::command]
fn window_mode_runtime_ack(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, window_mode::SharedWindowModeController>,
    request_id: String,
    cycle: u64,
    phase: String,
) -> Result<bool, String> {
    pet_runtime_command(window.label(), || {
        controller.runtime_ack(
            &request_id,
            cycle,
            window_mode::RuntimeAckPhase::parse(&phase)?,
        )
    })?
}

#[tauri::command]
fn window_mode_runtime_ready(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, window_mode::SharedWindowModeController>,
) -> Result<u64, String> {
    pet_runtime_command(window.label(), || controller.runtime_ready())?
}

fn pet_runtime_command<T>(label: &str, action: impl FnOnce() -> T) -> Result<T, String> {
    if label == "pet" {
        Ok(action())
    } else {
        Err("window mode runtime handshake is restricted to the pet window".to_owned())
    }
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
        || pet_id.len() > 80
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
fn pet_get(
    state: tauri::State<'_, SharedPetRepository>,
    pet_id: String,
) -> Result<Option<Pet>, String> {
    let repo = state.lock().map_err(|_| "pets lock poisoned")?;
    repo.get(&pet_id)
}

fn validate_profile_command_id(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!("invalid {label}"));
    }
    Ok(())
}

fn pet_profile_get_from_repository(
    repo: &SharedPetRepository,
    pet_id: &str,
) -> Result<PetProfile, String> {
    validate_profile_command_id(pet_id, "pet id")?;
    let repo = repo.lock().map_err(|_| "pets lock poisoned")?;
    repo.get_profile(pet_id)?
        .ok_or_else(|| format!("pet not found: {pet_id}"))
}

fn with_pet_profile_edit<T>(
    gate: &SharedPetMutationGate,
    request_id: &str,
    pet_id: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    validate_profile_command_id(request_id, "request id")?;
    validate_profile_command_id(pet_id, "pet id")?;
    let _lease = gate.scoped(request_id, MutationKind::ProfileEdit, pet_id)?;
    operation()
}

fn pet_profile_update_from_repository(
    repo: &SharedPetRepository,
    gate: &SharedPetMutationGate,
    request_id: &str,
    pet_id: &str,
    value: PetProfileUpdate,
) -> Result<PetProfile, String> {
    with_pet_profile_edit(gate, request_id, pet_id, || {
        let repo = repo.lock().map_err(|_| "pets lock poisoned")?;
        repo.update_profile(pet_id, value)
    })
}

#[tauri::command]
fn pet_profile_get(
    repo: tauri::State<'_, SharedPetRepository>,
    pet_id: String,
) -> Result<PetProfile, String> {
    pet_profile_get_from_repository(repo.inner(), &pet_id)
}

#[tauri::command]
fn pet_profile_update(
    repo: tauri::State<'_, SharedPetRepository>,
    gate: tauri::State<'_, SharedPetMutationGate>,
    request_id: String,
    pet_id: String,
    value: PetProfileUpdate,
) -> Result<PetProfile, String> {
    pet_profile_update_from_repository(repo.inner(), gate.inner(), &request_id, &pet_id, value)
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
fn creation_composer_save(
    service: tauri::State<'_, creation::SharedCreationService>,
    session_id: String,
    recipe: creation::domain::ComposerRecipe,
    current_step: String,
) -> Result<creation::domain::CreationSnapshot, String> {
    service.save_composer_recipe(&session_id, &recipe, &current_step)
}

#[tauri::command]
fn creation_composer_candidate(
    service: tauri::State<'_, creation::SharedCreationService>,
    session_id: String,
    png_b64: Option<String>,
) -> Result<creation::service::ComposerCandidateProjection, String> {
    service.store_composer_candidate(&session_id, png_b64.as_deref())
}

#[tauri::command]
fn creation_adoption_catalog(
    service: tauri::State<'_, creation::SharedCreationService>,
) -> Result<Vec<creation::adoption::AdoptionCatalogEntry>, String> {
    service.adoption_catalog()
}

#[tauri::command]
fn creation_adoption_start(
    service: tauri::State<'_, creation::SharedCreationService>,
    template_id: String,
    display_name: String,
) -> Result<creation::domain::CreationSnapshot, String> {
    service.start_adoption(&template_id, &display_name)
}

#[tauri::command]
fn creation_abandon(
    service: tauri::State<'_, creation::SharedCreationService>,
    session_id: String,
) -> Result<(), String> {
    service.abandon(&session_id)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CreationPrepareFinalizeResponse {
    request_id: String,
    session_id: String,
    pet_id: String,
    variant_id: String,
    already_completed: bool,
    cleanup_pending: bool,
}

#[tauri::command]
fn creation_prepare_finalize(
    service: tauri::State<'_, creation::finalization::SharedCreationFinalizationService>,
    session_id: String,
    request_id: String,
    confirm_needs_review: bool,
) -> Result<CreationPrepareFinalizeResponse, String> {
    let prepared = service.prepare_with_quality_confirmation(
        &session_id,
        &request_id,
        confirm_needs_review,
    )?;
    let cleanup_pending = service.photo_avatar_cleanup_pending(&session_id)?;
    Ok(CreationPrepareFinalizeResponse {
        request_id: prepared.request_id,
        session_id: prepared.session_id,
        pet_id: prepared.pet_id,
        variant_id: prepared.variant_id,
        already_completed: prepared.already_completed,
        cleanup_pending,
    })
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

fn pet_calibration_load_from_store(
    state: &pets::state::SharedStateStore,
    pet_id: &str,
) -> Result<PetCalibrationV1, String> {
    let store = state.lock().map_err(|_| "state lock poisoned")?;
    pets::calibration::load(&store, pet_id)
}

fn pet_calibration_save_to_repository(
    active: &SharedActivePetService,
    repo: &SharedPetRepository,
    state: &pets::state::SharedStateStore,
    pet_id: &str,
    value: PetCalibrationV1,
) -> Result<PetCalibrationV1, String> {
    pet_calibration_save_to_repository_inner(active, repo, state, pet_id, value, || {})
}

fn pet_calibration_save_to_repository_inner(
    active: &SharedActivePetService,
    repo: &SharedPetRepository,
    state: &pets::state::SharedStateStore,
    pet_id: &str,
    value: PetCalibrationV1,
    before_active_recheck: impl FnOnce(),
) -> Result<PetCalibrationV1, String> {
    validate_pet_asset_id(pet_id)?;
    let active_pet_id = active.active()?;
    if active_pet_id != pet_id {
        return Err(format!(
            "calibration can only be saved for the active pet: active={active_pet_id}, requested={pet_id}"
        ));
    }
    if pet_id != pets::active::BUILTIN_PET_ID {
        let repo = repo.lock().map_err(|_| "pets lock poisoned")?;
        let pet = repo.get(pet_id)?;
        drop(repo);
        let pet = pet.ok_or_else(|| format!("pet not found: {pet_id}"))?;
        if pet.lifecycle != "ready" || pet.completed_at.is_none() {
            return Err(format!(
                "pet calibration is unavailable until completion: {pet_id}"
            ));
        }
    }
    before_active_recheck();
    let active_pet_id = active.active()?;
    if active_pet_id != pet_id {
        return Err(format!(
            "calibration can only be saved for the active pet: active={active_pet_id}, requested={pet_id}"
        ));
    }
    let store = state.lock().map_err(|_| "state lock poisoned")?;
    pets::calibration::save(&store, pet_id, value)
}

#[tauri::command]
fn pet_calibration_load(
    state: tauri::State<'_, pets::state::SharedStateStore>,
    pet_id: String,
) -> Result<PetCalibrationV1, String> {
    pet_calibration_load_from_store(state.inner(), &pet_id)
}

#[tauri::command]
fn pet_calibration_save(
    active: tauri::State<'_, SharedActivePetService>,
    repo: tauri::State<'_, SharedPetRepository>,
    state: tauri::State<'_, pets::state::SharedStateStore>,
    pet_id: String,
    value: PetCalibrationV1,
) -> Result<PetCalibrationV1, String> {
    pet_calibration_save_to_repository(active.inner(), repo.inner(), state.inner(), &pet_id, value)
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
    let png = decode_creation_upload_source(&ref_png_b64)?;
    manager.start_for_session(&session_id, &prompt, &png, &ref_sha256)
}

#[tauri::command]
fn creation_upload_retry(
    manager: tauri::State<'_, generation::tasks::SharedGenerationManager>,
    session_id: String,
    prompt: String,
) -> Result<String, String> {
    manager.retry_for_session(&session_id, &prompt)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoAvatarUpload {
    bytes_b64: String,
    sha256: String,
}

#[tauri::command]
fn creation_photo_avatar_consent(
    manager: tauri::State<'_, creation::photo_avatar::pixel_manager::SharedPixelPhotoAvatarManager>,
    accept: bool,
) -> Result<bool, String> {
    manager.save_consent(accept)
}

#[tauri::command]
fn creation_photo_avatar_begin(
    manager: tauri::State<'_, creation::photo_avatar::pixel_manager::SharedPixelPhotoAvatarManager>,
    session_id: String,
    consent_version: String,
    photos: Vec<PhotoAvatarUpload>,
) -> Result<creation::photo_avatar::domain::PixelPhotoAvatarSnapshot, String> {
    let raw = photos
        .into_iter()
        .map(|photo| {
            Ok(creation::photo_avatar::source::RawPhotoSource {
                bytes: decode_creation_upload_source(&photo.bytes_b64)?,
                claimed_sha256: photo.sha256,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    manager.inner().begin(&session_id, &consent_version, raw)
}

#[tauri::command]
fn creation_photo_avatar_status(
    manager: tauri::State<'_, creation::photo_avatar::pixel_manager::SharedPixelPhotoAvatarManager>,
    session_id: String,
) -> Result<Option<creation::photo_avatar::domain::PixelPhotoAvatarSnapshot>, String> {
    manager.status(&session_id)
}

fn run_photo_avatar_status_command(
    manager: &creation::photo_avatar::manager::SharedPhotoAvatarManager,
    session_id: &str,
) -> Result<Option<creation::photo_avatar::domain::PhotoAvatarSnapshot>, String> {
    manager.status_if_exists(session_id)
}

#[tauri::command]
fn creation_photo_avatar_cancel(
    manager: tauri::State<'_, creation::photo_avatar::pixel_manager::SharedPixelPhotoAvatarManager>,
    session_id: String,
) -> Result<creation::photo_avatar::domain::PixelPhotoAvatarSnapshot, String> {
    manager.cancel(&session_id)
}

#[tauri::command]
fn creation_photo_avatar_regenerate(
    manager: tauri::State<'_, creation::photo_avatar::pixel_manager::SharedPixelPhotoAvatarManager>,
    session_id: String,
) -> Result<creation::photo_avatar::domain::PixelPhotoAvatarSnapshot, String> {
    manager.inner().regenerate(&session_id)
}

fn run_photo_avatar_regenerate_command(
    manager: &creation::photo_avatar::manager::SharedPhotoAvatarManager,
    session_id: &str,
) -> Result<creation::photo_avatar::domain::PhotoAvatarSnapshot, String> {
    manager.regenerate_and_start_background(session_id)
}

#[tauri::command]
fn creation_photo_avatar_revise(
    manager: tauri::State<'_, creation::photo_avatar::pixel_manager::SharedPixelPhotoAvatarManager>,
    session_id: String,
    instruction: String,
) -> Result<creation::photo_avatar::domain::PixelPhotoAvatarSnapshot, String> {
    manager.inner().revise(&session_id, &instruction)
}

fn run_photo_avatar_revise_command(
    manager: &creation::photo_avatar::manager::SharedPhotoAvatarManager,
    session_id: &str,
    instruction: &str,
) -> Result<creation::photo_avatar::domain::PhotoAvatarSnapshot, String> {
    manager.revise_and_start_background(session_id, instruction)
}

#[tauri::command]
fn creation_photo_avatar_runtime_check_passed(
    manager: tauri::State<'_, creation::photo_avatar::pixel_manager::SharedPixelPhotoAvatarManager>,
    session_id: String,
    revision: u32,
    manifest_sha256: String,
) -> Result<creation::photo_avatar::domain::PixelPhotoAvatarSnapshot, String> {
    manager.runtime_check_passed(&session_id, revision, &manifest_sha256)
}

#[tauri::command]
fn creation_photo_avatar_preview_manifest(
    manager: tauri::State<'_, creation::photo_avatar::pixel_manager::SharedPixelPhotoAvatarManager>,
    session_id: String,
    revision: u32,
) -> Result<serde_json::Value, String> {
    manager.preview_manifest(&session_id, revision)
}

#[tauri::command]
fn creation_photo_avatar_preview_file_b64(
    manager: tauri::State<'_, creation::photo_avatar::pixel_manager::SharedPixelPhotoAvatarManager>,
    session_id: String,
    revision: u32,
    relative_path: String,
) -> Result<String, String> {
    manager.preview_file_b64(&session_id, revision, &relative_path)
}

fn decode_creation_upload_source(encoded: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    use generation::tasks::{MAX_UPLOAD_SOURCE_BASE64_BYTES, MAX_UPLOAD_SOURCE_BYTES};
    if encoded.len() > MAX_UPLOAD_SOURCE_BASE64_BYTES {
        return Err("upload source exceeds the 10 MiB raw byte limit".into());
    }
    if encoded.len() % 4 == 0 {
        let padding = if encoded.ends_with("==") {
            2
        } else if encoded.ends_with('=') {
            1
        } else {
            0
        };
        let decoded_len = encoded.len() / 4 * 3 - padding;
        if decoded_len > MAX_UPLOAD_SOURCE_BYTES {
            return Err("upload source exceeds the 10 MiB raw byte limit".into());
        }
    }
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("bad base64: {error}"))
}

#[tauri::command]
fn creation_upload_jobs(
    store: tauri::State<'_, creation::SharedCreationStore>,
    session_id: String,
) -> Result<Vec<creation::JobRecord>, String> {
    let store = store.lock().map_err(|_| "store lock poisoned")?;
    store.upload_jobs(&session_id)
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadSourceResponse {
    data_url: String,
    ref_sha256: String,
}

#[tauri::command]
fn creation_upload_source(
    manager: tauri::State<'_, generation::tasks::SharedGenerationManager>,
    session_id: String,
) -> Result<Option<UploadSourceResponse>, String> {
    use base64::Engine;
    manager.upload_source(&session_id).map(|source| {
        source.map(|source| UploadSourceResponse {
            data_url: format!(
                "data:{};base64,{}",
                source.mime_type,
                base64::engine::general_purpose::STANDARD.encode(source.bytes)
            ),
            ref_sha256: source.ref_sha256,
        })
    })
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
    if !matches!(file_name, "raw.png" | "cutout.png" | "motion-profile.json") {
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

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadCandidateAssets {
    schema_version: u32,
    raw_url: String,
    cutout_url: String,
    motion_profile: Option<runtime_assets::motion_profile::MotionProfileV1>,
    quality_disposition: String,
    quality: generation::cutout::CandidateQualityReportV1,
}

fn png_data_url(path: &std::path::Path) -> Result<String, String> {
    use base64::Engine;
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn owned_generation_file(
    data_dir: &std::path::Path,
    job_id: &str,
    stored_path: &str,
    file_name: &str,
) -> Result<std::path::PathBuf, String> {
    let expected = generation_job_file(data_dir, job_id, file_name)?;
    let jobs_root = data_dir.join("jobs");
    let jobs_metadata = std::fs::symlink_metadata(&jobs_root)
        .map_err(|error| format!("generation jobs root is unavailable: {error}"))?;
    if platform::is_link_or_reparse_point(&jobs_metadata) || !jobs_metadata.is_dir() {
        return Err("generation jobs root is a link or reparse point, or non-directory".into());
    }
    let expected_job_dir = jobs_root.join(job_id);
    let job_metadata = std::fs::symlink_metadata(&expected_job_dir)
        .map_err(|error| format!("generation job directory is unavailable: {error}"))?;
    if platform::is_link_or_reparse_point(&job_metadata) || !job_metadata.is_dir() {
        return Err("generation job directory is a link or reparse point, or non-directory".into());
    }
    let stored = std::path::Path::new(stored_path);
    if stored.file_name().and_then(|name| name.to_str()) != Some(file_name) {
        return Err(format!("stored candidate path must end with {file_name}"));
    }
    let metadata = std::fs::symlink_metadata(stored)
        .map_err(|error| format!("stored candidate file is unavailable: {error}"))?;
    if platform::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err("stored candidate file is not a regular file".into());
    }
    let canonical_stored = stored
        .canonicalize()
        .map_err(|error| format!("stored candidate path is invalid: {error}"))?;
    let canonical_expected = expected
        .canonicalize()
        .map_err(|error| format!("canonical candidate file is unavailable: {error}"))?;
    let canonical_jobs_root = jobs_root
        .canonicalize()
        .map_err(|error| format!("canonical generation jobs root is unavailable: {error}"))?;
    let canonical_job_dir = expected_job_dir
        .canonicalize()
        .map_err(|error| format!("canonical generation job directory is unavailable: {error}"))?;
    if canonical_job_dir != canonical_jobs_root.join(job_id)
        || canonical_expected.parent() != Some(canonical_job_dir.as_path())
    {
        return Err("canonical candidate file escapes its generation job directory".into());
    }
    if canonical_stored != canonical_expected {
        return Err("stored candidate path does not match its owned generation job".into());
    }
    Ok(canonical_stored)
}

fn upload_candidate_assets_from(
    data_dir: &std::path::Path,
    store: &creation::SharedCreationStore,
    job_id: &str,
) -> Result<UploadCandidateAssets, String> {
    let (candidate, variant) = {
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        let job = store.job(job_id)?;
        let session_id = job
            .session_id
            .as_deref()
            .ok_or_else(|| "upload candidate job has no creation session".to_string())?;
        if job.status != "success" {
            return Err("upload candidate job is not successful".into());
        }
        let candidate = store.candidate_for_session(session_id)?;
        if candidate.job_id.as_deref() != Some(job_id) || candidate.pet_id != job.pet_id {
            return Err("upload candidate is not owned by the requested job and session".into());
        }
        let variant = store
            .candidates(&job.pet_id)?
            .into_iter()
            .find(|variant| {
                variant.variant_id == candidate.candidate_id
                    && variant.job_id.as_deref() == Some(job_id)
            })
            .ok_or_else(|| "owned upload candidate paths are unavailable".to_string())?;
        (candidate, variant)
    };
    let report = candidate.quality_report.clone().ok_or_else(|| {
        "candidate quality report is unavailable; regenerate the candidate".to_string()
    })?;
    let quality_matches_report = if report.is_acceptable() {
        candidate.quality == "acceptable" && variant.quality == "acceptable"
    } else {
        matches!(candidate.quality.as_str(), "needs-review" | "user-accepted")
            && candidate.quality == variant.quality
            && (candidate.quality != "user-accepted" || report.is_user_confirmable())
    };
    if !quality_matches_report {
        return Err("candidate quality status does not match its stored report".into());
    }
    let raw = owned_generation_file(data_dir, job_id, &variant.image_path, "raw.png")?;
    let cutout = owned_generation_file(
        data_dir,
        job_id,
        variant
            .cutout_path
            .as_deref()
            .ok_or_else(|| "candidate cutout path is unavailable".to_string())?,
        "cutout.png",
    )?;
    let motion_profile = if report.is_acceptable() || candidate.quality == "user-accepted" {
        let profile_path = owned_generation_file(
            data_dir,
            job_id,
            candidate
                .motion_profile_path
                .as_deref()
                .ok_or_else(|| "acceptable candidate has no motion profile".to_string())?,
            "motion-profile.json",
        )?;
        let json = std::fs::read_to_string(profile_path).map_err(|error| error.to_string())?;
        Some(runtime_assets::motion_profile::parse_motion_profile(&json)?)
    } else {
        None
    };
    Ok(UploadCandidateAssets {
        schema_version: runtime_assets::manifest::ANIMATED_IMAGE_SCHEMA_VERSION,
        raw_url: png_data_url(&raw)?,
        cutout_url: png_data_url(&cutout)?,
        motion_profile,
        quality_disposition: match candidate.quality.as_str() {
            "acceptable" => "automatic",
            "user-accepted" => "userAccepted",
            _ => "unconfirmed",
        }
        .into(),
        quality: report,
    })
}

#[tauri::command]
fn creation_upload_candidate_assets(
    app: tauri::AppHandle,
    store: tauri::State<'_, creation::SharedCreationStore>,
    manager: tauri::State<'_, generation::tasks::SharedGenerationManager>,
    job_id: String,
) -> Result<UploadCandidateAssets, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    manager.reprocess_green_screen_candidate(&job_id)?;
    upload_candidate_assets_from(&data_dir, &store, &job_id)
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

    let toggle = MenuItem::with_id(app, "toggle", "显示或隐藏", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
    let calibration = MenuItem::with_id(app, "calibration", "校准", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &settings, &calibration, &quit])?;
    let _ = app.manage(TrayModeMenu {
        transition: TrayTransitionGate::default(),
        render_lock: Mutex::new(()),
        snapshot_revision: TraySnapshotRevisionGate::default(),
    });
    let mut builder = TrayIconBuilder::new().menu(&menu);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => {
                let controller = app
                    .state::<window_mode::SharedWindowModeController>()
                    .inner()
                    .clone();
                let visible = match controller.snapshot() {
                    Ok(snapshot) => !snapshot.user_visible,
                    Err(error) => {
                        eprintln!("[desktop-pet] visibility snapshot failed: {error}");
                        return;
                    }
                };
                tauri::async_runtime::spawn_blocking(move || {
                    if let Err(error) = controller.set_user_visible(visible) {
                        eprintln!("[desktop-pet] tray visibility change failed: {error}");
                    }
                });
            }
            "settings" => {
                if let Err(error) = show_settings_window(app, None) {
                    println!("[desktop-pet] settings window FAILED: {error}");
                }
            }
            "calibration" => {
                if let Err(error) = show_settings_window(app, Some("calibration")) {
                    println!("[desktop-pet] calibration navigation FAILED: {error}");
                }
            }
            "quit" => {
                if let Err(error) = app
                    .state::<creation::photo_avatar::manager::SharedPhotoAvatarManager>()
                    .prepare_for_full_exit()
                {
                    eprintln!("[desktop-pet] photo avatar full-exit preparation failed: {error}");
                }
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn publish_window_mode_snapshot(
    app: &tauri::AppHandle,
    snapshot: &window_mode::WindowModeSnapshot,
) -> Result<(), String> {
    app.emit(window_mode::SNAPSHOT_CHANGED_EVENT, snapshot.clone())
        .map_err(|error| error.to_string())?;
    if let Some(tray) = app.try_state::<TrayModeMenu>() {
        if tray.refresh_if_idle(Some(snapshot)) == Some(TraySnapshotAcceptance::Conflict) {
            eprintln!(
                "[desktop-pet] conflicting tray snapshot revision; reloading canonical state"
            );
            if let Some(controller) = app.try_state::<window_mode::SharedWindowModeController>() {
                if let Ok(canonical) = controller.snapshot() {
                    tray.replace_authoritative(&canonical);
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrayTransitionLease(u64);

#[derive(Default)]
struct TrayTransitionGate {
    busy: std::sync::atomic::AtomicBool,
    revision: AtomicU64,
}

#[derive(Default)]
struct TraySnapshotRevisionGate(Mutex<Option<window_mode::WindowModeSnapshot>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TraySnapshotAcceptance {
    Accepted,
    Stale,
    Conflict,
}

impl TraySnapshotRevisionGate {
    fn accept(&self, snapshot: &window_mode::WindowModeSnapshot) -> TraySnapshotAcceptance {
        let Ok(mut current) = self.0.lock() else {
            return TraySnapshotAcceptance::Conflict;
        };
        if let Some(previous) = current.as_ref() {
            if snapshot.revision < previous.revision {
                return TraySnapshotAcceptance::Stale;
            }
            if snapshot.revision == previous.revision {
                return if snapshot == previous {
                    TraySnapshotAcceptance::Accepted
                } else {
                    TraySnapshotAcceptance::Conflict
                };
            }
        }
        *current = Some(snapshot.clone());
        TraySnapshotAcceptance::Accepted
    }

    fn replace_authoritative(&self, snapshot: &window_mode::WindowModeSnapshot) {
        if let Ok(mut current) = self.0.lock() {
            *current = Some(snapshot.clone());
        }
    }
}

impl TrayTransitionGate {
    fn try_begin(&self) -> Option<TrayTransitionLease> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(TrayTransitionLease(
            self.revision.fetch_add(1, Ordering::AcqRel) + 1,
        ))
    }

    fn owns(&self, lease: TrayTransitionLease) -> bool {
        self.busy.load(Ordering::Acquire) && self.revision.load(Ordering::Acquire) == lease.0
    }

    fn finish(&self, lease: TrayTransitionLease) -> bool {
        self.owns(lease)
            && self
                .busy
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }

    fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }
}

struct TrayModeMenu {
    transition: TrayTransitionGate,
    render_lock: Mutex<()>,
    snapshot_revision: TraySnapshotRevisionGate,
}

impl TrayModeMenu {
    fn try_begin_transition(
        &self,
        canonical: Option<&window_mode::WindowModeSnapshot>,
    ) -> Option<TrayTransitionLease> {
        let _render = self.render_lock.lock().ok()?;
        let lease = self.transition.try_begin()?;
        self.render_snapshot(canonical);
        self.set_enabled(false);
        Some(lease)
    }

    fn finish_transition(
        &self,
        lease: TrayTransitionLease,
        controller: &window_mode::SharedWindowModeController,
    ) {
        let Ok(_render) = self.render_lock.lock() else {
            return;
        };
        if !self.transition.owns(lease) {
            return;
        }
        let canonical = controller.snapshot().ok();
        self.render_snapshot(canonical.as_ref());
        if self.transition.finish(lease) {
            self.set_enabled(true);
        }
    }

    fn refresh_if_idle(
        &self,
        snapshot: Option<&window_mode::WindowModeSnapshot>,
    ) -> Option<TraySnapshotAcceptance> {
        let Ok(_render) = self.render_lock.lock() else {
            return None;
        };
        if !self.transition.is_busy() {
            return self.render_snapshot(snapshot);
        }
        None
    }

    fn render_snapshot(
        &self,
        snapshot: Option<&window_mode::WindowModeSnapshot>,
    ) -> Option<TraySnapshotAcceptance> {
        if let Some(snapshot) = snapshot {
            let acceptance = self.snapshot_revision.accept(snapshot);
            if acceptance != TraySnapshotAcceptance::Accepted {
                return Some(acceptance);
            }
        }
        Some(TraySnapshotAcceptance::Accepted)
    }

    fn replace_authoritative(&self, snapshot: &window_mode::WindowModeSnapshot) {
        let Ok(_render) = self.render_lock.lock() else {
            return;
        };
        self.snapshot_revision.replace_authoritative(snapshot);
    }

    fn set_enabled(&self, _enabled: bool) {}
}

#[tauri::command]
fn window_visibility_reconcile(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    controller: tauri::State<'_, window_mode::SharedWindowModeController>,
) -> Result<window_mode::WindowModeSnapshot, String> {
    if window.label() != "pet" {
        return Err("window visibility reconciliation is restricted to the pet window".to_owned());
    }
    let snapshot = controller.snapshot()?;
    let expected_visible = snapshot.actual_mode == Some(windowing::WindowMode::Companion)
        && snapshot.user_visible
        && snapshot.suppressions.is_empty();
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0 as isize;
    let facts = state
        .platform
        .probe_window_visibility(hwnd)
        .map_err(|error| error.to_string())?;
    let action = state
        .companion_visibility
        .lock()
        .map_err(|_| "companion visibility tracker lock poisoned".to_owned())?
        .decide(expected_visible, facts);
    if action == CompanionVisibilityAction::DiagnoseAndReassert {
        eprintln!(
            "[desktop-pet] pet window was shell-cloaked while expected visible; applying one safe no-activate reassertion"
        );
    }
    if matches!(
        action,
        CompanionVisibilityAction::Reassert | CompanionVisibilityAction::DiagnoseAndReassert
    ) {
        state
            .platform
            .ensure_companion_window(hwnd)
            .map_err(|error| error.to_string())?;
    }
    Ok(snapshot)
}

fn show_settings_window(app: &tauri::AppHandle, section: Option<&str>) -> Result<(), String> {
    if let Some(section) = section {
        app.state::<SettingsNavigationState>().publish(section)?;
    }
    let window = if let Some(window) = app.get_webview_window("settings") {
        window
    } else {
        let page = match section {
            Some("calibration") => "settings.html#calibration",
            _ => "settings.html",
        };
        let window =
            tauri::WebviewWindowBuilder::new(app, "settings", tauri::WebviewUrl::App(page.into()))
                .title("桌面宠物设置")
                .inner_size(720.0, 620.0)
                .additional_browser_args("--disable-gpu")
                .build()
                .map_err(|error| error.to_string())?;
        let _ = window.center();
        println!("[desktop-pet] settings window created");
        window
    };
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    if let Some(section) = section {
        window
            .emit(
                "settings:navigate",
                serde_json::json!({ "section": section }),
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn debug_open_settings_requested(value: Option<&str>) -> bool {
    #[cfg(debug_assertions)]
    {
        value == Some("1")
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = value;
        false
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
fn run_startup_recovery<Cleanup, Composer, PhotoCleanup, PhotoResume, Recover, Restore>(
    cleanup_quarantine: Cleanup,
    recover_composer: Composer,
    cleanup_photo_sources: PhotoCleanup,
    resume_photo_avatar: PhotoResume,
    recover_finalization: Recover,
    restore_active: Restore,
) -> Result<creation::finalization::RecoveryReport, String>
where
    Cleanup: FnOnce() -> Result<(), String>,
    Composer: FnOnce() -> Result<creation::service::ComposerOrphanRecoveryReport, String>,
    PhotoCleanup: FnOnce() -> Result<Vec<String>, String>,
    PhotoResume:
        FnOnce() -> Result<creation::photo_avatar::manager::PhotoAvatarResumeReport, String>,
    Recover: FnOnce() -> Result<creation::finalization::RecoveryReport, String>,
    Restore: FnOnce() -> Result<(), String>,
{
    let cleanup_warning = cleanup_quarantine().err();
    let composer_recovery = recover_composer();
    let photo_cleanup_warning = cleanup_photo_sources().err();
    let photo_resume = resume_photo_avatar();
    let mut recovery = recover_finalization()?;
    if let Some(error) = cleanup_warning {
        recovery
            .warnings
            .push(format!("quarantine cleanup failed: {error}"));
    }
    match composer_recovery {
        Ok(composer) => recovery.warnings.extend(
            composer
                .warnings
                .into_iter()
                .map(|warning| format!("composer orphan recovery: {warning}")),
        ),
        Err(error) => recovery
            .warnings
            .push(format!("composer orphan recovery failed: {error}")),
    }
    if let Some(error) = photo_cleanup_warning {
        recovery
            .warnings
            .push(format!("photo avatar source cleanup failed: {error}"));
    }
    match photo_resume {
        Ok(report) => recovery
            .warnings
            .extend(report.failures.into_iter().map(|failure| {
                format!(
                    "photo avatar resume failed for {}: {}",
                    failure.session_id, failure.error
                )
            })),
        Err(error) => recovery
            .warnings
            .push(format!("photo avatar resume scan failed: {error}")),
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
            let resource_dir = app
                .path()
                .resource_dir()
                .map_err(|error| error.to_string())?;
            let development_public = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public");
            let content_root =
                creation::content::resolve_content_root(&resource_dir, &development_public)?;
            let controlled_photo_avatar_provider =
                creation::photo_avatar::provider::ControlledBackendProvider::from_env().ok();
            let photo_avatar_provider: Arc<
                dyn creation::photo_avatar::provider::PhotoAvatarProvider,
            > = match controlled_photo_avatar_provider.clone() {
                Some(provider) => Arc::new(provider),
                None => {
                    eprintln!("[desktop-pet] photo avatar provider unavailable");
                    Arc::new(creation::photo_avatar::manager::UnconfiguredPhotoAvatarProvider)
                }
            };
            let module_root = {
                let production = resource_dir.join("cat-character-modules/cat-a-live2d-v1");
                if production.is_dir() {
                    production
                } else {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../public/cat-character-modules/cat-a-live2d-v1")
                }
            };
            let photo_avatar_builder =
                runtime_assets::photo_avatar_builder::PhotoAvatarBuilder::new(
                    &module_root,
                    &data_dir.join("photo-avatar-previews"),
                );
            let photo_avatar_manager = Arc::new(
                creation::photo_avatar::manager::PhotoAvatarManager::with_provider(
                    creation::photo_avatar::store::PhotoAvatarStore::new(storage.clone()),
                    photo_avatar_provider,
                )
                .with_builder(photo_avatar_builder),
            );
            let pixel_photo_avatar_manager = Arc::new(
                creation::photo_avatar::pixel_manager::PixelPhotoAvatarManager::new(
                    creation::photo_avatar::store::PhotoAvatarStore::new(storage.clone()),
                    controlled_photo_avatar_provider.map(Arc::new),
                    &data_dir.join("photo-avatar-pixel-previews"),
                ),
            );
            let creation_service = Arc::new(
                creation::CreationService::new(
                    storage.clone(),
                    data_dir.clone(),
                    deletion.clone(),
                    content_root.clone(),
                    mutation_gate.clone(),
                )
                .with_photo_avatar_abandon_port(pixel_photo_avatar_manager.clone()),
            );
            let finalization = Arc::new(
                creation::finalization::CreationFinalizationService::new(
                    storage.clone(),
                    data_dir.clone(),
                    data_dir.join("jobs"),
                    mutation_gate.clone(),
                    active.switch_transaction(),
                )
                .with_deletion(deletion.clone())
                .with_photo_avatar(pixel_photo_avatar_manager.clone()),
            );
            let recovery = run_startup_recovery(
                || deletion.cleanup_quarantine(),
                || creation_service.recover_composer_orphans(),
                || creation_service.cleanup_terminal_photo_avatar_sources(),
                || photo_avatar_manager.resume_all(),
                || finalization.recover(),
                || active.restore().map(|_| ()),
            )?;
            for warning in recovery.warnings {
                eprintln!("[desktop-pet] startup recovery: {warning}");
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
            app.manage(photo_avatar_manager.clone()
                as creation::photo_avatar::manager::SharedPhotoAvatarManager);
            app.manage(pixel_photo_avatar_manager.clone()
                as creation::photo_avatar::pixel_manager::SharedPixelPhotoAvatarManager);

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
            let platform = app.state::<AppState>().platform.clone();
            let preferences_lock = app.state::<AppState>().preferences_lock.clone();
            platform
                .configure_pet_window(hwnd)
                .map_err(|error| error.to_string())?;
            window.set_always_on_top(true)?;
            let preferences_path = app
                .path()
                .app_config_dir()
                .map_err(|error| error.to_string())?
                .join("m0-preferences.json");
            let saved_preferences = preferences::load(&preferences_path)?;
            if !saved_preferences.user_visible {
                window.hide()?;
            }
            let controller = Arc::new(window_mode::WindowModeController::new(
                Arc::new(window_mode::TauriWindowModeIo::new(
                    app.handle().clone(),
                    platform,
                    hwnd,
                    preferences_path.clone(),
                    preferences_lock,
                )),
                saved_preferences.user_visible,
            ));
            app.manage(controller.clone() as window_mode::SharedWindowModeController);
            controller.start_health_monitor()?;
            build_tray(app)?;
            if debug_open_settings_requested(
                std::env::var("DESKTOP_PET_DEBUG_OPEN_SETTINGS")
                    .ok()
                    .as_deref(),
            ) {
                show_settings_window(app.handle(), None)?;
            }
            Ok(())
        })
        .manage(AppState {
            platform: Arc::new(WindowsPlatformAdapter),
            preferences_lock: Arc::new(Mutex::new(())),
            companion_visibility: Mutex::new(CompanionVisibilityTracker::default()),
        })
        .manage(SettingsNavigationState::default())
        .invoke_handler(with_tauri_commands!(build_tauri_handler))
        .build(tauri::generate_context!())
        .expect("failed to build desktop pet runtime")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                app.state::<window_mode::SharedWindowModeController>()
                    .shutdown();
            }
        });
}

#[cfg(test)]
mod lib_tests {
    use crate::pets::active::{ActivePetService, SharedActivePetService, BUILTIN_PET_ID};
    use crate::pets::mutation::{MutationKind, PetMutationGate};
    use crate::pets::profile::{PetGender, PetProfileUpdate};
    use crate::pets::repository::PetRepository;
    use crate::pets::state::{SharedStateStore, StateStore};
    use crate::pets::{ActivePetSession, SharedActivePetSession, SharedPetRepository};
    use crate::storage::Storage;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    static PROFILE_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn profile_repo() -> (SharedPetRepository, Arc<Mutex<Storage>>, std::path::PathBuf) {
        let n = PROFILE_COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-profile-command-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Mutex::new(Storage::open(&root).unwrap()));
        let repo = Arc::new(Mutex::new(PetRepository::new(storage.clone())));
        (repo, storage, root)
    }

    fn insert_ready_pet(storage: &Arc<Mutex<Storage>>, pet_id: &str) {
        storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO pets
                 (pet_id, schema_version, species, identity_mode, display_name, creation_method,
                  source_template_id, source_template_version, lifecycle, completed_at,
                  created_at, updated_at)
                 VALUES (?1, 1, 'cat', 'realpet', '旧名字', 'upload', NULL, NULL,
                         'ready', 'old', 'old', 'old')",
                [pet_id],
            )
            .unwrap();
    }

    fn insert_pet_with_completion(
        storage: &Arc<Mutex<Storage>>,
        pet_id: &str,
        lifecycle: &str,
        completed_at: Option<&str>,
    ) {
        storage
            .lock()
            .unwrap()
            .db
            .execute(
                "INSERT INTO pets
                 (pet_id, schema_version, species, identity_mode, display_name, creation_method,
                  source_template_id, source_template_version, lifecycle, completed_at,
                  created_at, updated_at)
                 VALUES (?1, 1, 'cat', 'realpet', '校准宠物', 'upload', NULL, NULL,
                         ?2, ?3, 'old', 'old')",
                rusqlite::params![pet_id, lifecycle, completed_at],
            )
            .unwrap();
    }

    fn calibration_fixture(
        active_pet_id: Option<&str>,
    ) -> (
        SharedPetRepository,
        SharedStateStore,
        SharedActivePetService,
        SharedActivePetSession,
        Arc<Mutex<Storage>>,
        std::path::PathBuf,
    ) {
        let (repo, storage, root) = profile_repo();
        let state = Arc::new(Mutex::new(StateStore::new(storage.clone())));
        let session = Arc::new(Mutex::new(ActivePetSession::new()));
        if let Some(pet_id) = active_pet_id {
            session.lock().unwrap().set_active(pet_id.into()).unwrap();
        }
        let gate = Arc::new(PetMutationGate::new(Duration::from_secs(60)));
        let active = Arc::new(ActivePetService::new(
            storage.clone(),
            session.clone(),
            root.join("pets"),
            gate,
        ));
        (repo, state, active, session, storage, root)
    }

    fn close_calibration_fixture(
        repo: SharedPetRepository,
        state: SharedStateStore,
        active: SharedActivePetService,
        session: SharedActivePetSession,
        storage: Arc<Mutex<Storage>>,
        root: std::path::PathBuf,
    ) {
        drop(repo);
        drop(state);
        drop(active);
        drop(session);
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn profile_update(display_name: &str) -> PetProfileUpdate {
        PetProfileUpdate {
            display_name: display_name.into(),
            gender: PetGender::Female,
            birth_date: Some("2024-02-29".into()),
        }
    }

    fn validate_profile_command_registry(commands: &[&str]) -> Result<(), String> {
        let mut unique = std::collections::HashSet::new();
        for command in commands {
            if !unique.insert(*command) {
                return Err(format!("duplicate command: {command}"));
            }
        }
        for required in ["pet_profile_get", "pet_profile_update"] {
            if !unique.contains(required) {
                return Err(format!("missing command: {required}"));
            }
        }
        Ok(())
    }

    #[test]
    fn all_commands_are_registered() {
        let _get = super::pet_profile_get;
        let _update = super::pet_profile_update;
        let _calibration_load = super::pet_calibration_load;
        let _calibration_save = super::pet_calibration_save;

        validate_profile_command_registry(super::REGISTERED_TAURI_COMMAND_NAMES).unwrap();
        for required in ["pet_calibration_load", "pet_calibration_save"] {
            assert_eq!(
                super::REGISTERED_TAURI_COMMAND_NAMES
                    .iter()
                    .filter(|command| **command == required)
                    .count(),
                1,
                "{required} must appear exactly once in the shared command registry"
            );
        }
    }

    #[test]
    fn settings_navigation_pending_is_latest_owned_and_consumed_once() {
        let navigation = super::SettingsNavigationState::default();

        navigation.publish("calibration").unwrap();
        navigation.publish("calibration").unwrap();

        assert_eq!(navigation.take().unwrap().as_deref(), Some("calibration"));
        assert_eq!(navigation.take().unwrap(), None);
        assert!(navigation.publish("unknown").is_err());
        assert_eq!(navigation.take().unwrap(), None);
    }

    #[test]
    fn settings_navigation_take_command_is_registered_once() {
        let _take = super::settings_take_pending_navigation;
        assert_eq!(
            super::REGISTERED_TAURI_COMMAND_NAMES
                .iter()
                .filter(|command| **command == "settings_take_pending_navigation")
                .count(),
            1
        );
    }

    #[test]
    fn window_mode_commands_are_registered_once() {
        let _get = super::window_mode_get;
        let _set = super::window_mode_set;
        let _visibility = super::window_visibility_set;
        let _ack = super::window_mode_runtime_ack;
        let _ready = super::window_mode_runtime_ready;
        let _fullscreen = super::window_fullscreen_update;
        let _visibility_reconcile = super::window_visibility_reconcile;
        for required in [
            "window_mode_get",
            "window_mode_set",
            "window_visibility_set",
            "window_mode_runtime_ack",
            "window_mode_runtime_ready",
            "window_fullscreen_update",
            "window_visibility_reconcile",
        ] {
            assert_eq!(
                super::REGISTERED_TAURI_COMMAND_NAMES
                    .iter()
                    .filter(|command| **command == required)
                    .count(),
                1,
                "{required} must appear exactly once in the shared command registry"
            );
        }
    }

    #[test]
    fn settings_and_tray_contenders_have_exactly_one_transition_owner() {
        let gate = std::sync::Arc::new(super::TrayTransitionGate::default());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let contenders: Vec<_> = ["settings", "tray"]
            .into_iter()
            .map(|name| {
                let gate = gate.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    (name, gate.try_begin())
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = contenders
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();

        assert_eq!(
            results.iter().filter(|(_, lease)| lease.is_some()).count(),
            1
        );
        assert!(gate.is_busy());
        let owner = results.into_iter().find_map(|(_, lease)| lease).unwrap();
        assert!(!gate.finish(super::TrayTransitionLease(owner.0.wrapping_add(1))));
        assert!(gate.is_busy(), "a non-owner must not release the menu");
        assert!(gate.finish(owner));
        assert!(!gate.is_busy());
    }

    #[test]
    fn two_tray_events_cannot_replace_the_first_owner_revision() {
        let gate = super::TrayTransitionGate::default();
        let first = gate.try_begin().expect("first tray event owns the lease");
        assert!(gate.try_begin().is_none());
        assert!(gate.finish(first));
        let second = gate
            .try_begin()
            .expect("next tray event owns the next lease");
        assert_eq!(second.0, first.0 + 1);
        assert!(gate.finish(second));
    }

    #[test]
    fn tray_snapshot_revision_gate_rejects_reverse_publication() {
        let gate = super::TraySnapshotRevisionGate::default();
        let snapshot =
            |revision,
             actual_mode: Option<super::windowing::WindowMode>,
             desktop_strategy: Option<super::window_mode::DesktopStrategy>| {
                super::window_mode::WindowModeSnapshot {
                    revision,
                    desired_mode: actual_mode.unwrap_or(super::windowing::WindowMode::Desktop),
                    actual_mode,
                    desktop_strategy,
                    user_visible: true,
                    suppressions: vec![],
                }
            };
        let first = snapshot(1, Some(super::windowing::WindowMode::Companion), None);
        let second = snapshot(
            2,
            Some(super::windowing::WindowMode::Desktop),
            Some(super::window_mode::DesktopStrategy::WorkerW),
        );
        assert_eq!(gate.accept(&first), super::TraySnapshotAcceptance::Accepted);
        assert_eq!(
            gate.accept(&second),
            super::TraySnapshotAcceptance::Accepted
        );
        assert_eq!(gate.accept(&first), super::TraySnapshotAcceptance::Stale);
        assert_eq!(
            gate.accept(&second),
            super::TraySnapshotAcceptance::Accepted
        );
        let conflict = snapshot(2, Some(super::windowing::WindowMode::Companion), None);
        assert_eq!(
            gate.accept(&conflict),
            super::TraySnapshotAcceptance::Conflict
        );
        gate.replace_authoritative(&conflict);
        assert_eq!(
            gate.accept(&conflict),
            super::TraySnapshotAcceptance::Accepted
        );
    }

    #[test]
    fn tray_exposes_visibility_without_public_mode_switches() {
        let source = include_str!("lib.rs");
        let start = source.find("fn build_tray(").expect("tray builder");
        let end = source[start..]
            .find("fn show_settings_window(")
            .map(|offset| start + offset)
            .expect("tray builder end");
        let tray = &source[start..end];

        assert!(tray.contains("\"toggle\""), "{tray}");
        assert!(!tray.contains("CheckMenuItem::with_id"), "{tray}");
        assert!(!tray.contains("mode-companion"), "{tray}");
        assert!(!tray.contains("mode-desktop"), "{tray}");
        assert!(!tray.contains("request_tray_mode"), "{tray}");
        assert!(!tray.contains("attach_desktop_host"), "{tray}");
        assert!(!tray.contains("set_always_on_top"), "{tray}");
        assert!(!tray.contains("SetParent"), "{tray}");
    }

    #[test]
    fn runtime_handshake_rejects_non_pet_window_labels() {
        let calls = std::cell::Cell::new(0);
        assert_eq!(
            super::pet_runtime_command("pet", || {
                calls.set(calls.get() + 1);
                7
            })
            .unwrap(),
            7
        );
        for label in ["settings", "unknown"] {
            assert!(super::pet_runtime_command(label, || {
                calls.set(calls.get() + 1);
                9
            })
            .unwrap_err()
            .contains("pet window"));
        }
        assert_eq!(
            calls.get(),
            1,
            "rejected windows must not invoke controller state"
        );
    }

    #[test]
    fn runtime_commands_inject_the_calling_webview_window() {
        let source = include_str!("lib.rs");
        for command in ["window_mode_runtime_ack", "window_mode_runtime_ready"] {
            let start = source.find(&format!("fn {command}(")).unwrap();
            let command_source = &source[start..(start + 700).min(source.len())];
            assert!(
                command_source.contains("window: tauri::WebviewWindow"),
                "{command_source}"
            );
            assert!(
                command_source.contains("pet_runtime_command(window.label(), ||"),
                "{command_source}"
            );
        }
    }

    #[test]
    fn startup_has_no_desktop_host_restore_branch() {
        let source = include_str!("lib.rs");
        let start = source.find("pub fn run()").expect("run entrypoint");
        let end = source[start..]
            .find("mod lib_tests {")
            .map(|offset| start + offset)
            .expect("test module after run entrypoint");
        let run = &source[start..end];
        assert!(!run.contains("if saved_preferences.mode == \"desktop\""));
        assert!(!run.contains("startup-mode-restore"));
        assert!(run.contains("build_tray(app)?"));
    }

    #[test]
    fn debug_settings_acceptance_requires_explicit_one() {
        assert_eq!(
            super::debug_open_settings_requested(Some("1")),
            cfg!(debug_assertions)
        );
        for value in [None, Some(""), Some("0"), Some("true"), Some(" 1 ")] {
            assert!(!super::debug_open_settings_requested(value), "{value:?}");
        }
    }

    #[test]
    fn debug_settings_acceptance_reuses_existing_window_entrypoint() {
        let source = include_str!("lib.rs");
        let start = source.find("pub fn run()").expect("run entrypoint");
        let end = source[start..]
            .find("mod lib_tests {")
            .map(|offset| start + offset)
            .expect("test module after run entrypoint");
        let run = &source[start..end];

        assert!(run.contains("debug_open_settings_requested"), "{run}");
        assert!(
            run.contains("show_settings_window(app.handle(), None)"),
            "{run}"
        );
    }

    #[test]
    fn companion_visibility_recovery_is_safe_and_episode_bounded() {
        use crate::platform::WindowVisibilityFacts;

        let mut tracker = super::CompanionVisibilityTracker::default();
        let healthy = WindowVisibilityFacts {
            visible: true,
            shell_cloaked: false,
            topmost: true,
        };
        assert_eq!(
            tracker.decide(false, healthy),
            super::CompanionVisibilityAction::None,
            "manual/fullscreen suppression must never be undone"
        );

        let shell_cloaked = WindowVisibilityFacts {
            visible: true,
            shell_cloaked: true,
            topmost: true,
        };
        assert_eq!(
            tracker.decide(true, shell_cloaked),
            super::CompanionVisibilityAction::DiagnoseAndReassert
        );
        assert_eq!(
            tracker.decide(true, shell_cloaked),
            super::CompanionVisibilityAction::None,
            "a Win+D shell-cloak episode gets at most one non-invasive reassertion"
        );

        assert_eq!(
            tracker.decide(true, healthy),
            super::CompanionVisibilityAction::None,
            "clearing the shell cloak rearms a future episode"
        );
        let not_visible = WindowVisibilityFacts {
            visible: false,
            shell_cloaked: false,
            topmost: false,
        };
        assert_eq!(
            tracker.decide(true, not_visible),
            super::CompanionVisibilityAction::Reassert
        );
    }

    #[test]
    fn calibration_commands_replace_the_legacy_global_json_interface() {
        let source = include_str!("lib.rs");
        let start = source.find("fn pet_calibration_load(").unwrap();
        let end = source[start..].find("fn gen_start(").unwrap() + start;
        let commands = &source[start..end];

        assert!(commands.contains("pet_id: String"));
        assert!(commands.contains("SharedActivePetService"));
        assert!(commands.contains("SharedPetRepository"));
        assert!(commands.contains("PetCalibrationV1"));
        assert!(!commands.contains("serde_json::Value"));
        assert!(!commands.contains("pet:probe:calibration"));

        let helper_start = source
            .find("fn pet_calibration_save_to_repository_inner(")
            .unwrap();
        let helper_end = source[helper_start..].find("#[tauri::command]").unwrap() + helper_start;
        let helper = &source[helper_start..helper_end];
        let first_active = helper.find("active.active()").unwrap();
        let repo_lock = helper.find("repo.lock()").unwrap();
        let repo_drop = helper.find("drop(repo)").unwrap();
        let second_active =
            helper[repo_drop + 1..].find("active.active()").unwrap() + repo_drop + 1;
        let state_lock = helper.find("state.lock()").unwrap();
        assert!(
            first_active < repo_lock
                && repo_lock < repo_drop
                && repo_drop < second_active
                && second_active < state_lock
        );
    }

    #[test]
    fn calibration_load_is_key_only_and_missing_safe_pet_defaults() {
        let (repo, state, active, session, storage, root) = calibration_fixture(None);
        assert_eq!(
            super::pet_calibration_load_from_store(&state, "pet-unknown").unwrap(),
            crate::pets::calibration::PetCalibrationV1::default()
        );
        close_calibration_fixture(repo, state, active, session, storage, root);
    }

    #[test]
    fn active_ready_pet_can_save_and_returns_the_canonical_value() {
        let (repo, state, active, session, storage, root) = calibration_fixture(Some("pet-ready"));
        insert_ready_pet(&storage, "pet-ready");
        let value = crate::pets::calibration::PetCalibrationV1 {
            schema_version: 1,
            breath_amplitude_percent: 3.5,
            blink_interval_scale: 1.2,
            feedback_strength: 0.8,
        };

        assert_eq!(
            super::pet_calibration_save_to_repository(
                &active,
                &repo,
                &state,
                "pet-ready",
                value.clone()
            )
            .unwrap(),
            value
        );
        assert_eq!(
            super::pet_calibration_load_from_store(&state, "pet-ready").unwrap(),
            value
        );
        close_calibration_fixture(repo, state, active, session, storage, root);
    }

    #[test]
    fn active_builtin_can_save_builtin_calibration() {
        let (repo, state, active, session, storage, root) =
            calibration_fixture(Some(BUILTIN_PET_ID));
        let value = crate::pets::calibration::PetCalibrationV1::default();
        assert_eq!(
            super::pet_calibration_save_to_repository(
                &active,
                &repo,
                &state,
                BUILTIN_PET_ID,
                value.clone(),
            )
            .unwrap(),
            value
        );
        close_calibration_fixture(repo, state, active, session, storage, root);
    }

    #[test]
    fn inactive_pet_save_is_rejected_without_state_side_effect() {
        let (repo, state, active, session, storage, root) = calibration_fixture(Some("pet-b"));
        insert_ready_pet(&storage, "pet-a");
        let key = crate::pets::calibration::state_key("pet-a").unwrap();
        let error = super::pet_calibration_save_to_repository(
            &active,
            &repo,
            &state,
            "pet-a",
            crate::pets::calibration::PetCalibrationV1::default(),
        )
        .unwrap_err();
        assert!(error.contains("active pet"));
        assert_eq!(state.lock().unwrap().load(&key).unwrap(), None);
        close_calibration_fixture(repo, state, active, session, storage, root);
    }

    #[test]
    fn active_builtin_cannot_save_a_user_pet() {
        let (repo, state, active, session, storage, root) =
            calibration_fixture(Some(BUILTIN_PET_ID));
        insert_ready_pet(&storage, "pet-a");
        let key = crate::pets::calibration::state_key("pet-a").unwrap();
        assert!(super::pet_calibration_save_to_repository(
            &active,
            &repo,
            &state,
            "pet-a",
            crate::pets::calibration::PetCalibrationV1::default(),
        )
        .is_err());
        assert_eq!(state.lock().unwrap().load(&key).unwrap(), None);
        close_calibration_fixture(repo, state, active, session, storage, root);
    }

    #[test]
    fn active_draft_ready_without_completion_and_other_lifecycle_are_rejected_without_writes() {
        let (repo, state, active, session, storage, root) = calibration_fixture(None);
        for (pet_id, lifecycle, completed_at) in [
            ("pet-draft", "draft", None),
            ("pet-ready-null", "ready", None),
            ("pet-corrupt", "corrupt", Some("old")),
        ] {
            insert_pet_with_completion(&storage, pet_id, lifecycle, completed_at);
            session
                .lock()
                .unwrap()
                .set_active(pet_id.to_owned())
                .unwrap();
            let key = crate::pets::calibration::state_key(pet_id).unwrap();
            let error = super::pet_calibration_save_to_repository(
                &active,
                &repo,
                &state,
                pet_id,
                crate::pets::calibration::PetCalibrationV1::default(),
            )
            .unwrap_err();
            assert!(error.contains("unavailable until completion"));
            assert_eq!(state.lock().unwrap().load(&key).unwrap(), None);
        }
        close_calibration_fixture(repo, state, active, session, storage, root);
    }

    #[test]
    fn unknown_pet_save_has_no_state_side_effect() {
        let (repo, state, active, session, storage, root) =
            calibration_fixture(Some("pet-missing"));
        let key = crate::pets::calibration::state_key("pet-missing").unwrap();
        assert_eq!(
            super::pet_calibration_save_to_repository(
                &active,
                &repo,
                &state,
                "pet-missing",
                crate::pets::calibration::PetCalibrationV1::default(),
            ),
            Err("pet not found: pet-missing".into())
        );
        assert_eq!(state.lock().unwrap().load(&key).unwrap(), None);
        close_calibration_fixture(repo, state, active, session, storage, root);
    }

    #[test]
    fn active_service_missing_and_poison_errors_propagate_without_writes() {
        let (repo, state, active, session, storage, root) = calibration_fixture(None);
        let key = crate::pets::calibration::state_key(BUILTIN_PET_ID).unwrap();
        assert_eq!(
            super::pet_calibration_save_to_repository(
                &active,
                &repo,
                &state,
                BUILTIN_PET_ID,
                crate::pets::calibration::PetCalibrationV1::default(),
            ),
            Err("active pet has not been restored".into())
        );
        assert_eq!(state.lock().unwrap().load(&key).unwrap(), None);
        close_calibration_fixture(repo, state, active, session, storage, root);

        let (repo, state, active, session, storage, root) =
            calibration_fixture(Some(BUILTIN_PET_ID));
        let poisoned_session = session.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned_session.lock().unwrap();
            panic!("poison active pet session");
        })
        .join();
        assert_eq!(
            super::pet_calibration_save_to_repository(
                &active,
                &repo,
                &state,
                BUILTIN_PET_ID,
                crate::pets::calibration::PetCalibrationV1::default(),
            ),
            Err("session lock poisoned".into())
        );
        assert_eq!(state.lock().unwrap().load(&key).unwrap(), None);
        close_calibration_fixture(repo, state, active, session, storage, root);
    }

    #[test]
    fn active_switch_before_recheck_is_rejected_without_a_write() {
        let (repo, state, active, session, storage, root) = calibration_fixture(Some("pet-a"));
        insert_ready_pet(&storage, "pet-a");
        let key = crate::pets::calibration::state_key("pet-a").unwrap();
        let switching_session = session.clone();
        let error = super::pet_calibration_save_to_repository_inner(
            &active,
            &repo,
            &state,
            "pet-a",
            crate::pets::calibration::PetCalibrationV1::default(),
            move || {
                switching_session
                    .lock()
                    .unwrap()
                    .set_active("pet-b".into())
                    .unwrap();
            },
        )
        .unwrap_err();
        assert!(error.contains("active pet"));
        assert_eq!(state.lock().unwrap().load(&key).unwrap(), None);
        close_calibration_fixture(repo, state, active, session, storage, root);
    }

    #[test]
    fn calibration_helpers_report_repository_and_state_poison_consistently() {
        let (repo, state, active, session, storage, root) = calibration_fixture(Some("pet-ready"));
        insert_ready_pet(&storage, "pet-ready");
        let poisoned_repo = repo.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned_repo.lock().unwrap();
            panic!("poison calibration repository");
        })
        .join();
        assert_eq!(
            super::pet_calibration_save_to_repository(
                &active,
                &repo,
                &state,
                "pet-ready",
                crate::pets::calibration::PetCalibrationV1::default(),
            ),
            Err("pets lock poisoned".into())
        );
        close_calibration_fixture(repo, state, active, session, storage, root);

        let (repo, state, active, session, storage, root) =
            calibration_fixture(Some(BUILTIN_PET_ID));
        let poisoned_state = state.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned_state.lock().unwrap();
            panic!("poison calibration state");
        })
        .join();
        assert_eq!(
            super::pet_calibration_load_from_store(&state, BUILTIN_PET_ID),
            Err("state lock poisoned".into())
        );
        assert_eq!(
            super::pet_calibration_save_to_repository(
                &active,
                &repo,
                &state,
                BUILTIN_PET_ID,
                crate::pets::calibration::PetCalibrationV1::default(),
            ),
            Err("state lock poisoned".into())
        );
        close_calibration_fixture(repo, state, active, session, storage, root);
    }

    #[test]
    fn command_registry_check_rejects_a_list_without_profile_get() {
        let without_get = super::REGISTERED_TAURI_COMMAND_NAMES
            .iter()
            .copied()
            .filter(|command| *command != "pet_profile_get")
            .collect::<Vec<_>>();

        assert_eq!(
            validate_profile_command_registry(&without_get),
            Err("missing command: pet_profile_get".into())
        );
    }

    #[test]
    fn profile_command_signatures_require_safe_ids_and_keep_get_outside_the_gate() {
        let source = include_str!("lib.rs");
        let get_start = source.find("fn pet_profile_get(").unwrap();
        let get_signature_end = source[get_start..].find(") ->").unwrap() + get_start;
        let get_signature = &source[get_start..get_signature_end];
        assert!(get_signature.contains("pet_id: String"));
        assert!(!get_signature.contains("SharedPetMutationGate"));

        let start = source.find("fn pet_profile_update(").unwrap();
        let signature_end = source[start..].find(") ->").unwrap() + start;
        let signature = &source[start..signature_end];

        assert!(signature.contains("request_id: String"));
        assert!(signature.contains("pet_id: String"));
        assert!(!signature.contains("request_id: Option<String>"));
    }

    #[test]
    fn profile_get_helper_returns_builtin_user_and_unknown_results_without_a_gate() {
        let (repo, storage, root) = profile_repo();
        insert_ready_pet(&storage, "pet-a");

        let builtin = super::pet_profile_get_from_repository(&repo, BUILTIN_PET_ID).unwrap();
        assert_eq!(builtin.pet_id, BUILTIN_PET_ID);
        assert!(!builtin.editable);

        let user = super::pet_profile_get_from_repository(&repo, "pet-a").unwrap();
        assert_eq!(user.display_name, "旧名字");
        assert!(user.editable);

        assert_eq!(
            super::pet_profile_get_from_repository(&repo, "pet-missing"),
            Err("pet not found: pet-missing".into())
        );
        drop(repo);
        drop(storage);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_command_helpers_reject_unsafe_ids_before_locking_state() {
        let (repo, storage, root) = profile_repo();
        let poisoned = repo.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("poison profile repository");
        })
        .join();
        let gate = Arc::new(PetMutationGate::new(Duration::from_secs(1)));

        assert_eq!(
            super::pet_profile_get_from_repository(&repo, "../pet-a"),
            Err("invalid pet id".into())
        );
        assert_eq!(
            super::pet_profile_update_from_repository(
                &repo,
                &gate,
                "../request",
                "pet-a",
                profile_update("新名字"),
            ),
            Err("invalid request id".into())
        );
        assert_eq!(
            super::pet_profile_update_from_repository(
                &repo,
                &gate,
                "request-safe",
                "../pet-a",
                profile_update("新名字"),
            ),
            Err("invalid pet id".into())
        );
        drop(repo);
        drop(storage);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_update_helper_returns_the_canonical_saved_profile() {
        let (repo, storage, root) = profile_repo();
        insert_ready_pet(&storage, "pet-a");
        let gate = Arc::new(PetMutationGate::new(Duration::from_secs(1)));

        let saved = super::pet_profile_update_from_repository(
            &repo,
            &gate,
            "edit-1",
            "pet-a",
            profile_update("  新名字  "),
        )
        .unwrap();
        let loaded = super::pet_profile_get_from_repository(&repo, "pet-a").unwrap();

        assert_eq!(saved, loaded);
        assert_eq!(saved.display_name, "新名字");
        assert_eq!(saved.gender, PetGender::Female);
        assert_eq!(saved.birth_date.as_deref(), Some("2024-02-29"));
        drop(repo);
        drop(storage);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_update_lease_covers_the_repository_operation() {
        let gate = Arc::new(PetMutationGate::new(Duration::from_secs(1)));
        let editing = gate.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let edit = std::thread::spawn(move || {
            super::with_pet_profile_edit(&editing, "edit-1", "pet-a", || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let deleting = gate.clone();
        let (deleted_tx, deleted_rx) = std::sync::mpsc::channel();
        let delete = std::thread::spawn(move || {
            let _lease = deleting
                .scoped("delete-1", MutationKind::Delete, "pet-a")
                .unwrap();
            deleted_tx.send(()).unwrap();
        });
        assert!(deleted_rx.recv_timeout(Duration::from_millis(20)).is_err());

        release_tx.send(()).unwrap();
        edit.join().unwrap().unwrap();
        assert_eq!(deleted_rx.recv_timeout(Duration::from_secs(1)), Ok(()));
        delete.join().unwrap();
    }

    #[test]
    fn profile_update_holds_the_lease_while_waiting_for_the_repository_mutex() {
        let (repo, storage, root) = profile_repo();
        insert_ready_pet(&storage, "pet-a");
        let gate = Arc::new(PetMutationGate::new(Duration::from_secs(1)));
        let repo_guard = repo.lock().unwrap();
        let updating_repo = repo.clone();
        let updating_gate = gate.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let update = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            super::pet_profile_update_from_repository(
                &updating_repo,
                &updating_gate,
                "edit-repo-lock",
                "pet-a",
                profile_update("  新名字  "),
            )
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let ownership_deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            match gate.finish("edit-repo-lock") {
                Err(_) => break,
                Ok(None) if std::time::Instant::now() < ownership_deadline => {
                    std::thread::yield_now();
                }
                result => panic!("profile edit never owned the gate: {result:?}"),
            }
        }

        drop(repo_guard);
        let saved = update.join().unwrap().unwrap();
        let loaded = super::pet_profile_get_from_repository(&repo, "pet-a").unwrap();
        assert_eq!(saved, loaded);
        assert_eq!(saved.display_name, "新名字");

        drop(repo);
        drop(storage);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_update_releases_the_lease_after_repository_and_lock_failures() {
        let (repo, storage, root) = profile_repo();
        let gate = Arc::new(PetMutationGate::new(Duration::from_secs(1)));

        assert!(super::pet_profile_update_from_repository(
            &repo,
            &gate,
            "edit-missing",
            "pet-missing",
            profile_update("新名字"),
        )
        .is_err());
        drop(
            gate.scoped(
                "delete-after-repo-error",
                MutationKind::Delete,
                "pet-missing",
            )
            .unwrap(),
        );

        let poisoned = repo.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("poison profile repository");
        })
        .join();
        assert_eq!(
            super::pet_profile_update_from_repository(
                &repo,
                &gate,
                "edit-poisoned",
                "pet-a",
                profile_update("新名字"),
            ),
            Err("pets lock poisoned".into())
        );
        drop(
            gate.scoped("delete-after-lock-error", MutationKind::Delete, "pet-a")
                .unwrap(),
        );

        drop(repo);
        drop(storage);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pets::state::StateStore;
    use crate::storage::Storage;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    #[test]
    fn upload_source_base64_preflight_allows_exact_raw_limit_and_rejects_one_more() {
        use base64::Engine;
        let exact = base64::engine::general_purpose::STANDARD.encode(
            vec![0_u8; crate::generation::tasks::MAX_UPLOAD_SOURCE_BYTES],
        );
        let over = base64::engine::general_purpose::STANDARD.encode(vec![
            0_u8;
            crate::generation::tasks::MAX_UPLOAD_SOURCE_BYTES
                + 1
        ]);

        assert_eq!(
            super::decode_creation_upload_source(&exact).unwrap().len(),
            crate::generation::tasks::MAX_UPLOAD_SOURCE_BYTES
        );
        assert!(super::decode_creation_upload_source(&over)
            .unwrap_err()
            .contains("10 MiB"));
    }

    #[test]
    fn oversized_invalid_upload_base64_is_rejected_before_decode_or_database_work() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-upload-preflight-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Storage::open(&root).unwrap();
        let before: i64 = storage
            .db
            .query_row(
                "SELECT (SELECT COUNT(*) FROM creation_upload_sources)
                      + (SELECT COUNT(*) FROM generation_jobs)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let encoded = "!".repeat(crate::generation::tasks::MAX_UPLOAD_SOURCE_BASE64_BYTES + 1);

        let error = super::decode_creation_upload_source(&encoded).unwrap_err();

        let after: i64 = storage
            .db
            .query_row(
                "SELECT (SELECT COUNT(*) FROM creation_upload_sources)
                      + (SELECT COUNT(*) FROM generation_jobs)",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert!(error.contains("10 MiB"));
        assert!(!error.contains("bad base64"));
        assert_eq!(after, before);
        drop(storage);
        let _ = std::fs::remove_dir_all(root);
    }

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
    }

    #[test]
    fn legacy_pet_creation_commands_are_not_exposed_by_tauri() {
        let source = include_str!("lib.rs");
        for command in [
            ["pet", "create"].join("_"),
            ["pet", "creation", "resume"].join("_"),
        ] {
            assert!(!source.contains(&format!("fn {command}(")));
            assert!(!source.contains(&format!("            {command},")));
        }
    }

    #[test]
    fn creation_session_commands_are_available() {
        let _start = super::creation_start;
        let _draft = super::creation_draft;
        let _snapshot = super::creation_snapshot;
        let _set_name = super::creation_set_name;
        let _composer_save = super::creation_composer_save;
        let _composer_candidate = super::creation_composer_candidate;
        let _abandon = super::creation_abandon;
        let _upload_source = super::creation_upload_source;
        let _upload_retry = super::creation_upload_retry;
        let _candidate_assets = super::creation_upload_candidate_assets;
    }

    #[test]
    fn photo_avatar_command_registry_registers_all_commands_once() {
        let commands = [
            "creation_photo_avatar_consent",
            "creation_photo_avatar_begin",
            "creation_photo_avatar_status",
            "creation_photo_avatar_cancel",
            "creation_photo_avatar_regenerate",
            "creation_photo_avatar_revise",
            "creation_photo_avatar_runtime_check_passed",
            "creation_photo_avatar_preview_manifest",
            "creation_photo_avatar_preview_file_b64",
        ];
        for command in commands {
            assert_eq!(
                super::REGISTERED_TAURI_COMMAND_NAMES
                    .iter()
                    .filter(|registered| **registered == command)
                    .count(),
                1,
                "{command} must appear exactly once in the shared command registry"
            );
        }
    }

    fn photo_avatar_command_fixture() -> (
        creation::photo_avatar::manager::SharedPhotoAvatarManager,
        std::sync::Arc<creation::photo_avatar::provider::FakePhotoAvatarProvider>,
        std::path::PathBuf,
        String,
    ) {
        use image::{DynamicImage, ImageFormat, RgbaImage};
        use sha2::{Digest, Sha256};
        use std::io::Cursor;

        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-photo-avatar-command-{}-{n}",
            std::process::id()
        ));
        let storage = std::sync::Arc::new(std::sync::Mutex::new(Storage::open(&root).unwrap()));
        let session_id = format!("photo-avatar-command-session-{n}");
        {
            let storage = storage.lock().unwrap();
            storage.db.execute(
                "INSERT INTO pets (pet_id, schema_version, species, identity_mode, creation_method, lifecycle, created_at, updated_at) VALUES (?1, 1, 'cat', 'realpet', 'upload', 'draft', '10', '10')",
                [format!("photo-avatar-command-pet-{n}")],
            ).unwrap();
            storage.db.execute(
                "INSERT INTO creation_sessions (session_id, pet_id, method, status, last_stable_status, current_step, schema_version, created_at, updated_at) VALUES (?1, ?2, 'upload', 'draft', 'draft', 'upload', 1, '10', '10')",
                rusqlite::params![session_id, format!("photo-avatar-command-pet-{n}")],
            ).unwrap();
        }
        let store = creation::photo_avatar::store::PhotoAvatarStore::new(storage);
        let provider = std::sync::Arc::new(
            creation::photo_avatar::provider::FakePhotoAvatarProvider::new(vec![
                creation::photo_avatar::provider::FakeOutcome::Running,
            ]),
        );
        let manager = std::sync::Arc::new(
            creation::photo_avatar::manager::PhotoAvatarManager::new(store, provider.clone()),
        );
        assert!(manager.consent(true).unwrap());
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            256,
            256,
            image::Rgba([91, 52, 31, 255]),
        ));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        manager
            .begin(
                &session_id,
                vec![creation::photo_avatar::source::RawPhotoSource {
                    claimed_sha256: format!("{:x}", Sha256::digest(&bytes)),
                    bytes,
                }],
            )
            .unwrap();
        (manager, provider, root, session_id)
    }

    #[test]
    fn photo_avatar_status_command_returns_none_for_pre_begin_draft() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-photo-avatar-status-command-{}-{n}",
            std::process::id()
        ));
        let storage = std::sync::Arc::new(std::sync::Mutex::new(Storage::open(&root).unwrap()));
        let manager =
            std::sync::Arc::new(creation::photo_avatar::manager::PhotoAvatarManager::new(
                creation::photo_avatar::store::PhotoAvatarStore::new(storage),
                std::sync::Arc::new(
                    creation::photo_avatar::provider::FakePhotoAvatarProvider::new(vec![]),
                ),
            ));

        assert_eq!(
            super::run_photo_avatar_status_command(&manager, "pre-begin-session").unwrap(),
            None
        );
        drop(manager);
        let _ = std::fs::remove_dir_all(root);
    }

    fn wait_for_photo_avatar_command_request(
        manager: &creation::photo_avatar::manager::SharedPhotoAvatarManager,
        provider: &creation::photo_avatar::provider::FakePhotoAvatarProvider,
        session_id: &str,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while provider.requests().len() != 1
            || manager
                .status(session_id)
                .unwrap()
                .provider_job_id
                .is_none()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "background request did not start"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(provider.requests().len(), 1);
    }

    #[test]
    fn regenerate_command_starts_the_new_revision_background_run() {
        let (manager, provider, root, session_id) = photo_avatar_command_fixture();

        let snapshot = super::run_photo_avatar_regenerate_command(&manager, &session_id).unwrap();
        wait_for_photo_avatar_command_request(&manager, &provider, &session_id);

        assert_eq!(snapshot.revision, 2);
        manager.cancel(&session_id).unwrap();
        drop(manager);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn revise_command_starts_the_new_revision_background_run() {
        let (manager, provider, root, session_id) = photo_avatar_command_fixture();

        let snapshot =
            super::run_photo_avatar_revise_command(&manager, &session_id, "fluffier tail").unwrap();
        wait_for_photo_avatar_command_request(&manager, &provider, &session_id);

        assert_eq!(snapshot.revision, 2);
        manager.cancel(&session_id).unwrap();
        drop(manager);
        let _ = std::fs::remove_dir_all(root);
    }

    struct Task12CommandEvidence {
        provider_submits: usize,
        submitted_photos: i64,
        schema_version: u64,
        renderer: String,
        runtime_check_passed: bool,
        installed_via_finalization_port: bool,
        originals_cleaned: bool,
        animated_image_v3_fallbacks: usize,
    }

    fn run_task_12_balanced_success_command_scenario() -> Task12CommandEvidence {
        use base64::Engine as _;
        use image::{DynamicImage, ImageFormat, RgbaImage};
        use sha2::{Digest, Sha256};
        use std::io::Cursor;

        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-task-12-command-{}-{n}",
            std::process::id()
        ));
        let storage = std::sync::Arc::new(std::sync::Mutex::new(Storage::open(&root).unwrap()));
        let session_id = format!("task-12-session-{n}");
        let pet_id = format!("task-12-pet-{n}");
        {
            let storage = storage.lock().unwrap();
            storage.db.execute(
                "INSERT INTO pets (pet_id, schema_version, species, identity_mode, creation_method, lifecycle, created_at, updated_at) VALUES (?1, 1, 'cat', 'realpet', 'upload', 'draft', '10', '10')",
                [&pet_id],
            ).unwrap();
            storage.db.execute(
                "INSERT INTO creation_sessions (session_id, pet_id, method, status, last_stable_status, current_step, schema_version, created_at, updated_at) VALUES (?1, ?2, 'upload', 'draft', 'draft', 'upload', 1, '10', '10')",
                rusqlite::params![session_id, pet_id],
            ).unwrap();
        }
        let provider = std::sync::Arc::new(
            creation::photo_avatar::provider::FakePhotoAvatarProvider::for_body_module(
                "body-balanced-v1",
            )
            .unwrap(),
        );
        let module_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../public/cat-character-modules/cat-a-live2d-v1");
        let manager = std::sync::Arc::new(
            creation::photo_avatar::manager::PhotoAvatarManager::new(
                creation::photo_avatar::store::PhotoAvatarStore::new(storage.clone()),
                provider.clone(),
            )
            .with_builder(
                runtime_assets::photo_avatar_builder::PhotoAvatarBuilder::new(
                    &module_root,
                    &root.join("previews"),
                ),
            ),
        );
        assert!(manager.consent(true).unwrap());
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            256,
            256,
            image::Rgba([91, 52, 31, 255]),
        ));
        let mut png = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let raw = creation::photo_avatar::source::RawPhotoSource {
            bytes: png.clone(),
            claimed_sha256: format!("{:x}", Sha256::digest(&png)),
        };
        let second_png = {
            let second_image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
                256,
                256,
                image::Rgba([41, 92, 131, 255]),
            ));
            let mut bytes = Vec::new();
            second_image
                .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
                .unwrap();
            bytes
        };
        manager
            .begin_with_consent(
                &session_id,
                creation::photo_avatar::domain::PHOTO_AVATAR_CONSENT_VERSION,
                vec![
                    raw.clone(),
                    creation::photo_avatar::source::RawPhotoSource {
                        claimed_sha256: format!("{:x}", Sha256::digest(&second_png)),
                        bytes: second_png,
                    },
                ],
            )
            .unwrap();
        manager.start_background(&session_id).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let snapshot = loop {
            let snapshot = manager.status(&session_id).unwrap();
            if snapshot.step == creation::photo_avatar::domain::PhotoAvatarStep::BuildV5 {
                break manager.tick(&session_id).unwrap();
            }
            if matches!(
                snapshot.step,
                creation::photo_avatar::domain::PhotoAvatarStep::RuntimeCheckPending
                    | creation::photo_avatar::domain::PhotoAvatarStep::Failed
            ) {
                break snapshot;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Task 12 fake command scenario did not reach runtime check: step={:?}, requests={}, attempts={:?}, job={:?}",
                snapshot.step,
                provider.requests().len(),
                snapshot.attempts,
                snapshot.provider_job_id
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(
            snapshot.step,
            creation::photo_avatar::domain::PhotoAvatarStep::RuntimeCheckPending,
            "{:?}",
            snapshot.error_message
        );
        let manifest = manager
            .preview_manifest(&session_id, snapshot.revision)
            .unwrap();
        let preview_image = manifest["modelEntry"].as_str().unwrap();
        let preview_b64 = manager
            .preview_file_b64(&session_id, snapshot.revision, preview_image)
            .unwrap_or_else(|error| panic!("preview_image={preview_image:?}: {error}"));
        assert!(!base64::engine::general_purpose::STANDARD
            .decode(preview_b64)
            .unwrap()
            .is_empty());
        let manifest_sha256: String = storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT sha256 FROM photo_avatar_artifacts WHERE session_id=?1 AND revision=?2 AND kind='previewPackage'",
                rusqlite::params![session_id, snapshot.revision],
                |row| row.get(0),
            )
            .unwrap();
        let submitted_photos = storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM photo_avatar_sources WHERE session_id=?1",
                [&session_id],
                |row| row.get(0),
            )
            .unwrap();
        let checked = manager
            .runtime_check_passed(&session_id, snapshot.revision, &manifest_sha256)
            .unwrap();

        let variant_id = manifest["variantId"].as_str().unwrap();
        let install_dir = root.join("installed").join(&pet_id);
        let port: &dyn creation::finalization::PhotoAvatarFinalizationPort = manager.as_ref();
        assert!(port.preview_ready(&session_id).unwrap());
        port.install_preview(&session_id, &pet_id, variant_id, &install_dir)
            .unwrap();
        let installed_manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(install_dir.join("manifest.json")).unwrap())
                .unwrap();
        let installed_via_finalization_port = installed_manifest == manifest;
        let animated_image_v3_fallbacks = usize::from(
            installed_manifest["schemaVersion"].as_u64() == Some(3)
                || installed_manifest["renderer"].as_str() != Some("cat-spatial-live2d-v1"),
        );
        if let Ok(export_root) = std::env::var("DESKTOP_PET_TASK12_FIXTURE_DIR") {
            let export_root = std::path::PathBuf::from(export_root);
            std::fs::create_dir_all(&export_root).unwrap();
            let package_root = storage
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT local_path FROM photo_avatar_artifacts WHERE session_id=?1 AND revision=?2 AND kind='previewPackage'",
                    rusqlite::params![session_id, snapshot.revision],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
            copy_directory_for_task12(std::path::Path::new(&package_root), &export_root);
        }
        assert!(matches!(
            port.cleanup_after_accept(&session_id).unwrap(),
            creation::finalization::PhotoAvatarCleanupDisposition::Complete
        ));
        let originals_cleaned = storage
            .lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM photo_avatar_sources WHERE session_id=?1",
                [&session_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            == 0;
        let evidence = Task12CommandEvidence {
            provider_submits: provider.requests().len(),
            submitted_photos,
            schema_version: manifest["schemaVersion"].as_u64().unwrap(),
            renderer: manifest["renderer"].as_str().unwrap().into(),
            runtime_check_passed: matches!(
                checked.step,
                creation::photo_avatar::domain::PhotoAvatarStep::PreviewReady
            ),
            installed_via_finalization_port,
            originals_cleaned,
            animated_image_v3_fallbacks,
        };
        drop(manager);
        drop(storage);
        let _ = std::fs::remove_dir_all(root);
        evidence
    }

    fn copy_directory_for_task12(source: &std::path::Path, destination: &std::path::Path) {
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                std::fs::create_dir_all(&target).unwrap();
                copy_directory_for_task12(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn task_12_balanced_success_crosses_commands_builder_and_finalization_port() {
        let evidence = run_task_12_balanced_success_command_scenario();

        assert_eq!(evidence.provider_submits, 3);
        assert_eq!(evidence.submitted_photos, 2);
        assert_eq!(evidence.schema_version, 5);
        assert_eq!(evidence.renderer, "cat-spatial-live2d-v1");
        assert!(evidence.runtime_check_passed);
        assert!(evidence.installed_via_finalization_port);
        assert!(evidence.originals_cleaned);
        assert_eq!(evidence.animated_image_v3_fallbacks, 0);
    }

    #[test]
    fn creation_adoption_commands_are_available() {
        let _catalog = super::creation_adoption_catalog;
        let _start = super::creation_adoption_start;
    }

    #[test]
    fn creation_finalization_commands_are_available() {
        let _prepare = super::creation_prepare_finalize;
        let _abort = super::creation_abort_finalize;
        let _recover = super::creation_recover_finalization;
    }

    #[test]
    fn startup_recovers_photo_avatar_before_finalization_and_restoring_the_active_pet() {
        let events = std::sync::Mutex::new(Vec::new());

        let report = super::run_startup_recovery(
            || {
                events.lock().unwrap().push("quarantine");
                Ok(())
            },
            || {
                events.lock().unwrap().push("composer");
                Ok(creation::service::ComposerOrphanRecoveryReport::default())
            },
            || {
                events.lock().unwrap().push("photo-cleanup");
                Ok(Vec::new())
            },
            || {
                events.lock().unwrap().push("photo-resume");
                Ok(creation::photo_avatar::manager::PhotoAvatarResumeReport {
                    resumed_session_ids: Vec::new(),
                    failures: Vec::new(),
                })
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
            vec![
                "quarantine",
                "composer",
                "photo-cleanup",
                "photo-resume",
                "finalization",
                "active"
            ]
        );
    }

    #[test]
    fn startup_reports_composer_recovery_failure_without_blocking_later_recovery() {
        let events = std::sync::Mutex::new(Vec::new());

        let report = super::run_startup_recovery(
            || Ok(()),
            || Err("composer database busy".into()),
            || Ok(Vec::new()),
            || {
                Ok(creation::photo_avatar::manager::PhotoAvatarResumeReport {
                    resumed_session_ids: Vec::new(),
                    failures: Vec::new(),
                })
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

        assert_eq!(*events.lock().unwrap(), vec!["finalization", "active"]);
        assert_eq!(
            report.warnings,
            vec!["composer orphan recovery failed: composer database busy"]
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
        let raw = generation_job_file(root, "job-1_ok", "raw.png").unwrap();
        let profile = generation_job_file(root, "job-1_ok", "motion-profile.json").unwrap();
        assert_eq!(cutout.parent(), profile.parent());
        assert_eq!(raw.parent(), cutout.parent());
        assert!(raw.ends_with("jobs/job-1_ok/raw.png"));
        assert!(cutout.ends_with("jobs/job-1_ok/cutout.png"));
        assert!(generation_job_file(root, "job-1_ok", "../secret").is_err());
    }

    #[test]
    fn upload_candidate_assets_are_projected_from_owned_database_paths_and_quality() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-upload-review-{}-{n}",
            std::process::id()
        ));
        let job_dir = root.join("jobs").join("job-1");
        std::fs::create_dir_all(&job_dir).unwrap();
        let raw = job_dir.join("raw.png");
        let cutout = job_dir.join("cutout.png");
        std::fs::write(&raw, b"raw").unwrap();
        std::fs::write(&cutout, b"cutout").unwrap();
        let storage = std::sync::Arc::new(std::sync::Mutex::new(Storage::open(&root).unwrap()));
        let repo = pets::repository::PetRepository::new(storage.clone());
        let pet = repo
            .create(pets::pet::Species::Cat, pets::pet::IdentityMode::RealPet)
            .unwrap();
        let report = generation::cutout::CandidateQualityReportV1 {
            schema_version: 1,
            status: generation::cutout::CandidateQualityStatus::NeedsReview,
            reasons: vec![generation::cutout::QualityReason::InteriorHoles],
            opaque_ratio: 0.3,
            transparent_ratio: 0.6,
            partial_alpha_ratio: 0.1,
            visible_bounds: Some([1, 2, 30, 40]),
        };
        let now = creation::profiles::now_iso();
        {
            let storage = storage.lock().unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO creation_sessions
                 (session_id, pet_id, method, status, last_stable_status, current_step,
                  schema_version, created_at, updated_at)
                 VALUES ('session-1', ?1, 'upload', 'candidateReady', 'candidateReady',
                         'review', 1, ?2, ?2)",
                    rusqlite::params![pet.pet_id, now],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO generation_jobs
                 (job_id, pet_id, session_id, prompt, ref_sha256, status, created_at)
                 VALUES ('job-1', ?1, 'session-1', 'prompt', 'hash', 'success', ?2)",
                    rusqlite::params![pet.pet_id, now],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO appearance_variants
                 (variant_id, pet_id, job_id, session_id, image_path, cutout_path,
                  motion_profile_path, quality, quality_report_json, accepted, created_at)
                 VALUES ('candidate-1', ?1, 'job-1', 'session-1', ?2, ?3,
                         NULL, 'needs-review', ?4, 0, ?5)",
                    rusqlite::params![
                        pet.pet_id,
                        raw.to_string_lossy(),
                        cutout.to_string_lossy(),
                        serde_json::to_string(&report).unwrap(),
                        now
                    ],
                )
                .unwrap();
        }
        let store = std::sync::Arc::new(std::sync::Mutex::new(creation::CreationStore::new(
            storage.clone(),
        )));

        let assets = upload_candidate_assets_from(&root, &store, "job-1").unwrap();

        assert!(assets.raw_url.ends_with("cmF3"));
        assert!(assets.cutout_url.ends_with("Y3V0b3V0"));
        assert_eq!(assets.quality, report);
        assert_eq!(assets.quality_disposition, "unconfirmed");
        assert!(assets.motion_profile.is_none());

        let profile_path = job_dir.join("motion-profile.json");
        let rgba = image::RgbaImage::from_pixel(64, 64, image::Rgba([80, 90, 100, 255]));
        let profile = runtime_assets::motion_profile::generate_motion_profile(&rgba).unwrap();
        runtime_assets::motion_profile::write_motion_profile_atomic(&profile_path, &profile)
            .unwrap();
        storage
            .lock()
            .unwrap()
            .db
            .execute(
                "UPDATE appearance_variants
                 SET quality='user-accepted', motion_profile_path=?2
                 WHERE variant_id='candidate-1'",
                rusqlite::params!["candidate-1", profile_path.to_string_lossy()],
            )
            .unwrap();

        let reviewed = upload_candidate_assets_from(&root, &store, "job-1").unwrap();
        assert_eq!(reviewed.quality, report);
        assert_eq!(reviewed.quality_disposition, "userAccepted");
        assert_eq!(reviewed.motion_profile, Some(profile));
        assert!(upload_candidate_assets_from(&root, &store, "job-other").is_err());
        drop(store);
        drop(storage);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn upload_candidate_assets_reject_an_owned_job_directory_link() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-upload-review-link-{}-{n}",
            std::process::id()
        ));
        let outside = std::env::temp_dir().join(format!(
            "desktop-pet-upload-review-outside-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("jobs")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("raw.png"), b"external-raw").unwrap();
        std::fs::write(outside.join("cutout.png"), b"external-cutout").unwrap();
        let linked_job_dir = root.join("jobs").join("job-1");
        crate::platform::create_directory_link(&outside, &linked_job_dir);
        assert!(crate::platform::is_link_or_reparse_point(
            &std::fs::symlink_metadata(&linked_job_dir).unwrap()
        ));
        let raw = linked_job_dir.join("raw.png");
        let cutout = linked_job_dir.join("cutout.png");
        let storage = std::sync::Arc::new(std::sync::Mutex::new(Storage::open(&root).unwrap()));
        let repo = pets::repository::PetRepository::new(storage.clone());
        let pet = repo
            .create(pets::pet::Species::Cat, pets::pet::IdentityMode::RealPet)
            .unwrap();
        let report = generation::cutout::CandidateQualityReportV1 {
            schema_version: 1,
            status: generation::cutout::CandidateQualityStatus::NeedsReview,
            reasons: vec![generation::cutout::QualityReason::InteriorHoles],
            opaque_ratio: 0.3,
            transparent_ratio: 0.6,
            partial_alpha_ratio: 0.1,
            visible_bounds: Some([1, 2, 30, 40]),
        };
        let now = creation::profiles::now_iso();
        {
            let storage = storage.lock().unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO creation_sessions
                 (session_id, pet_id, method, status, last_stable_status, current_step,
                  schema_version, created_at, updated_at)
                 VALUES ('session-1', ?1, 'upload', 'candidateReady', 'candidateReady',
                         'review', 1, ?2, ?2)",
                    rusqlite::params![pet.pet_id, now],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO generation_jobs
                 (job_id, pet_id, session_id, prompt, ref_sha256, status, created_at)
                 VALUES ('job-1', ?1, 'session-1', 'prompt', 'hash', 'success', ?2)",
                    rusqlite::params![pet.pet_id, now],
                )
                .unwrap();
            storage
                .db
                .execute(
                    "INSERT INTO appearance_variants
                 (variant_id, pet_id, job_id, session_id, image_path, cutout_path,
                  motion_profile_path, quality, quality_report_json, accepted, created_at)
                 VALUES ('candidate-1', ?1, 'job-1', 'session-1', ?2, ?3,
                         NULL, 'needs-review', ?4, 0, ?5)",
                    rusqlite::params![
                        pet.pet_id,
                        raw.to_string_lossy(),
                        cutout.to_string_lossy(),
                        serde_json::to_string(&report).unwrap(),
                        now
                    ],
                )
                .unwrap();
        }
        let store = std::sync::Arc::new(std::sync::Mutex::new(creation::CreationStore::new(
            storage.clone(),
        )));
        {
            let store = store.lock().unwrap();
            assert_eq!(
                store.job("job-1").unwrap().session_id.as_deref(),
                Some("session-1")
            );
            assert_eq!(
                store
                    .candidate_for_session("session-1")
                    .unwrap()
                    .job_id
                    .as_deref(),
                Some("job-1")
            );
        }

        let error = upload_candidate_assets_from(&root, &store, "job-1").unwrap_err();

        assert!(error.contains("link or reparse point"), "{error}");
        assert_eq!(
            std::fs::read(outside.join("raw.png")).unwrap(),
            b"external-raw"
        );
        drop(store);
        drop(storage);
        std::fs::remove_dir(&linked_job_dir).unwrap();
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }
}
