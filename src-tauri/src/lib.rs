mod commands;
mod ffi;
mod state;
mod types;

use std::sync::Arc;

use tauri::Manager;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = Arc::new(AppState::default());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .setup(|app| {
            // Probe for the native protocol + decode DLL so the frontend can
            // show whether real mirroring is available before the user starts.
            if let Some(res) = app
                .path()
                .resource_dir()
                .ok()
                .map(|d| d.join("resources").join("ffmpeg"))
            {
                let present = crate::ffi::locate_dll(&res).is_some();
                *app.state::<Arc<AppState>>()
                    .mirror_lib_present
                    .lock()
                    .unwrap() = present;
            }
            // Default the screenshot/recording directory to the system Downloads
            // folder (only if the user hasn't already chosen one).
            if let Ok(downloads) = app.path().download_dir() {
                let st = app.state::<Arc<AppState>>();
                let mut sd = st.save_dir.lock().unwrap();
                if sd.is_empty() {
                    *sd = downloads.to_string_lossy().to_string();
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::start_mirror,
            commands::stop_mirror,
            commands::get_status,
            commands::save_screenshot,
            commands::save_recording,
            commands::open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
