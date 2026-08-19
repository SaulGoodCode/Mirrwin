mod commands;
mod ffi;
mod settings;
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
            // Probe for the native protocol DLL so the frontend can show
            // whether real mirroring is available before the user starts.
            let present = crate::ffi::locate_dll(app.handle()).is_some();
            let st = app.state::<Arc<AppState>>();
            *st.mirror_lib_present.lock().unwrap() = present;

            // Restore what the user last chose. Anything never saved keeps its
            // default, and the save directory falls back to system Downloads.
            let saved = crate::settings::load(app.handle());
            *st.device_name.lock().unwrap() = saved.device_name;
            *st.port.lock().unwrap() = saved.port;
            *st.enable_audio.lock().unwrap() = saved.enable_audio;
            *st.width.lock().unwrap() = saved.width;
            *st.height.lock().unwrap() = saved.height;
            *st.fps.lock().unwrap() = saved.fps;

            let mut save_dir = st.save_dir.lock().unwrap();
            *save_dir = saved.save_dir;
            if save_dir.is_empty() {
                if let Ok(downloads) = app.path().download_dir() {
                    *save_dir = downloads.to_string_lossy().to_string();
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::start_mirror,
            commands::stop_mirror,
            commands::get_status,
            commands::update_settings,
            commands::save_screenshot,
            commands::save_recording,
            commands::open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
