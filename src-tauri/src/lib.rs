mod platform;
mod preferences;
mod windowing;

use platform::{PlatformAdapter, WindowsPlatformAdapter};
use std::sync::Arc;
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            platform: Arc::new(WindowsPlatformAdapter),
        })
        .invoke_handler(tauri::generate_handler![
            probe_version,
            apply_hit_region,
            load_preferences,
            save_preferences,
            begin_drag
        ])
        .setup(|app| {
            let window = app.get_webview_window("pet").ok_or("pet window missing")?;
            let hwnd = window.hwnd()?.0 as isize;
            app.state::<AppState>()
                .platform
                .configure_pet_window(hwnd)
                .map_err(|error| error.to_string())?;
            window.set_always_on_top(true)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run desktop pet probe");
}

#[cfg(test)]
mod tests {
    #[test]
    fn probe_version_is_m0() {
        assert_eq!(super::probe_version(), "m0");
    }
}
