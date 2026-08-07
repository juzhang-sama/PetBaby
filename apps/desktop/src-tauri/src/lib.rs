mod creation;
mod generation;
mod pets;
mod platform;
mod preferences;
mod runtime_assets;
mod storage;
mod windowing;

use pets::pet::{IdentityMode, Pet, PetSummary, Species};
use pets::{ActivePetSession, SharedActivePetSession, SharedPetRepository};
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
    let files = match &manifest {
        runtime_assets::manifest::RuntimeAssetManifest::V1(value) => &value.files,
        runtime_assets::manifest::RuntimeAssetManifest::V2(value) => &value.files,
    };
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
    pet_id: String,
    variant_id: String,
    cutout_path: String,
) -> Result<runtime_assets::compiler::CompileResult, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let dest = data_dir.join("pets").join(&pet_id).join("assets");
    runtime_assets::compiler::compile_single_image(
        &pet_id,
        &variant_id,
        std::path::Path::new(&cutout_path),
        &dest,
    )
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
fn pet_delete(state: tauri::State<'_, SharedPetRepository>, pet_id: String) -> Result<(), String> {
    let repo = state.lock().map_err(|_| "pets lock poisoned")?;
    repo.delete(&pet_id)
}

#[tauri::command]
fn pet_set_active(
    state: tauri::State<'_, SharedActivePetSession>,
    pet_id: String,
) -> Result<(), String> {
    let mut session = state.lock().map_err(|_| "session lock poisoned")?;
    session.set_active(pet_id)
}

#[tauri::command]
fn pet_get_active(
    state: tauri::State<'_, SharedActivePetSession>,
) -> Result<Option<String>, String> {
    let session = state.lock().map_err(|_| "session lock poisoned")?;
    Ok(session.active().cloned())
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

#[tauri::command]
fn gen_cutout_path(app: tauri::AppHandle, job_id: String) -> Result<String, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let cutout = data_dir.join("jobs").join(&job_id).join("cutout.png");
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
    let cutout = data_dir.join("jobs").join(&job_id).join("cutout.png");
    let bytes = std::fs::read(&cutout).map_err(|error| error.to_string())?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
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

#[tauri::command]
fn gen_cleanup_pet(app: tauri::AppHandle, pet_id: String) -> Result<(), String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    // remove the pet record and its job artifacts; jobs rows cascade on pet delete
    let storage = app.state::<pets::SharedPetRepository>();
    let repo = storage.lock().map_err(|_| "pets lock poisoned")?;
    repo.delete(&pet_id)?;
    drop(repo);

    let jobs_root = data_dir.join("jobs");
    if let Ok(entries) = std::fs::read_dir(&jobs_root) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("job-") {
                // best effort: job dirs are small
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(())
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
pub fn run() {
    tauri::Builder::default()
        .register_uri_scheme_protocol("pet-asset", |ctx, request| {
            use tauri::http::Response;
            let app = ctx.app_handle();
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::new());
            let relative = request.uri().path().trim_start_matches('/');
            // pet-asset://localhost/<pet_id>/assets/<file>
            let file = data_dir.join("pets").join(relative);
            match std::fs::read(&file) {
                Ok(bytes) => Response::builder().status(200).body(bytes).unwrap(),
                Err(_) => Response::builder().status(404).body(Vec::new()).unwrap(),
            }
        })
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| error.to_string())?;
            let storage = Arc::new(Mutex::new(storage::Storage::open(&data_dir.join("pets"))?));
            app.manage(Arc::new(Mutex::new(pets::repository::PetRepository::new(
                storage.clone(),
            ))) as SharedPetRepository);
            app.manage(Arc::new(Mutex::new(ActivePetSession::new())) as SharedActivePetSession);
            let state_store = Arc::new(Mutex::new(pets::state::StateStore::new(storage)));
            app.manage(state_store.clone() as pets::state::SharedStateStore);

            let creation_store = Arc::new(Mutex::new(creation::CreationStore::new(Arc::new(
                Mutex::new(storage::Storage::open(&data_dir.join("pets"))?),
            ))));
            app.manage(creation_store.clone() as creation::SharedCreationStore);

            let manager = generation::tasks::GenerationManager::new(
                creation_store,
                state_store,
                Arc::from(data_dir.join("jobs").as_path()),
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
            pet_delete,
            pet_set_active,
            pet_get_active,
            pet_state_load,
            pet_state_save,
            app_setting_get,
            app_setting_set,
            pet_calibration_load,
            pet_calibration_save,
            gen_start,
            gen_cancel,
            gen_list,
            gen_resume,
            gen_cutout_path,
            gen_cutout_b64,
            gen_cleanup_pet,
            debug_windows
        ])
        .run(tauri::generate_context!())
        .expect("failed to run desktop pet runtime");
}

#[cfg(test)]
mod tests {
    #[test]
    fn probe_version_is_m0() {
        assert_eq!(super::probe_version(), "m0");
    }
}
