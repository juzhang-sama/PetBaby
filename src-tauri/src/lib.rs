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

#[tauri::command]
fn set_window_mode(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    mode: windowing::WindowMode,
) -> Result<platform::WindowModeEvidence, String> {
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0 as isize;
    state
        .platform
        .set_window_mode(hwnd, mode)
        .map_err(|error| error.to_string())
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

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::{
        menu::{Menu, MenuItem},
        tray::TrayIconBuilder,
    };

    let companion = MenuItem::with_id(app, "companion", "陪伴模式", true, None::<&str>)?;
    let desktop = MenuItem::with_id(app, "desktop", "桌面模式（实验性）", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "显示或隐藏", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&companion, &desktop, &toggle, &quit])?;
    let mut builder = TrayIconBuilder::new().menu(&menu);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder
        .on_menu_event(|app, event| {
            let Some(window) = app.get_webview_window("pet") else {
                return;
            };
            match event.id().as_ref() {
                "companion" | "desktop" => {
                    let mode = if event.id().as_ref() == "companion" {
                        windowing::WindowMode::Companion
                    } else {
                        windowing::WindowMode::Desktop
                    };
                    if let (Ok(hwnd), state) = (window.hwnd(), app.state::<AppState>()) {
                        match state.platform.set_window_mode(hwnd.0 as isize, mode) {
                            Ok(evidence) => println!(
                                "[desktop-pet] mode applied: requested={:?} strategy={} parent={:?}",
                                evidence.requested, evidence.strategy, evidence.parent_hwnd
                            ),
                            Err(error) => println!(
                                "[desktop-pet] set_window_mode failed for {:?}: {error}",
                                mode
                            ),
                        }
                    }
                }
                "toggle" => {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        let _ = window.show();
                    }
                }
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .build(app)?;
    Ok(())
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
            begin_drag,
            set_window_mode,
            probe_fullscreen
        ])
        .setup(|app| {
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
