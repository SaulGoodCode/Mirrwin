use std::sync::Mutex;

/// Application-wide shared state, wrapped in an `Arc` and managed by Tauri.
///
/// Sensible defaults are provided so the first run already advertises a usable
/// device name (`AirPlay Mirror`) and listens on the conventional port (7000).
pub struct AppState {
    pub running: Mutex<bool>,
    /// Re-entrancy guard: true while a start_mirror is in flight. Prevents
    /// rapid double-clicks from loading the native DLL twice (which corrupts
    /// the DLL's global server handle and can crash the process).
    pub starting: Mutex<bool>,
    /// "demo" or "real"
    pub mode: Mutex<String>,
    pub device_name: Mutex<String>,
    pub port: Mutex<u16>,
    pub device_id: Mutex<String>,
    pub connected_device: Mutex<Option<String>>,
    pub demo: Mutex<bool>,
    pub save_dir: Mutex<String>,
    /// Whether the native protocol DLL was located at startup.
    pub mirror_lib_present: Mutex<bool>,
    /// Play the device's audio as well as its screen. Off by default.
    pub enable_audio: Mutex<bool>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            running: Mutex::new(false),
            starting: Mutex::new(false),
            mode: Mutex::new("real".to_string()),
            device_name: Mutex::new("AirPlay Mirror".to_string()),
            port: Mutex::new(7000),
            device_id: Mutex::new(String::new()),
            connected_device: Mutex::new(None),
            demo: Mutex::new(false),
            save_dir: Mutex::new(String::new()),
            mirror_lib_present: Mutex::new(false),
            enable_audio: Mutex::new(false),
        }
    }
}
