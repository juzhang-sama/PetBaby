#[tauri::command]
fn probe_version() -> &'static str {
    "m0"
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![probe_version])
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
