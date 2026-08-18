use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};

use crate::ffi;
use crate::state::AppState;
use crate::types::{ReceiverStatus, StartOptions};

/// RAII guard that flips `state.starting` to `true` on creation and back to
/// `false` when dropped. Guarantees a crash / early `return` inside
/// `start_mirror` can never leave the flag stuck, which would permanently
/// block future starts.
struct StartingGuard<'a> {
    flag: &'a Mutex<bool>,
}
impl<'a> StartingGuard<'a> {
    /// Returns `Err` if a start is already in flight (prevents double-loading
    /// the native DLL, which corrupts its global server handle).
    fn try_new(flag: &'a Mutex<bool>) -> Result<Self, String> {
        let mut g = flag.lock().unwrap();
        if *g {
            return Err("正在启动中，请稍候…".to_string());
        }
        *g = true;
        Ok(StartingGuard { flag })
    }
}
impl<'a> Drop for StartingGuard<'a> {
    fn drop(&mut self) {
        *self.flag.lock().unwrap() = false;
    }
}

/// Snapshot the current state into a `ReceiverStatus` for the frontend.
pub fn read_status(state: &Arc<AppState>) -> ReceiverStatus {
    ReceiverStatus {
        running: *state.running.lock().unwrap(),
        mode: state.mode.lock().unwrap().clone(),
        device_name: state.device_name.lock().unwrap().clone(),
        port: *state.port.lock().unwrap(),
        device_id: state.device_id.lock().unwrap().clone(),
        connected_device: state.connected_device.lock().unwrap().clone(),
        demo: *state.demo.lock().unwrap(),
        save_dir: state.save_dir.lock().unwrap().clone(),
        mirror_lib_present: *state.mirror_lib_present.lock().unwrap(),
    }
}

pub fn emit_status(app: &AppHandle, state: &Arc<AppState>) {
    let _ = app.emit("status", read_status(state));
}

#[tauri::command]
pub async fn start_mirror(
    options: StartOptions,
    frame_channel: Channel<Vec<u8>>,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<ReceiverStatus, String> {
    if *state.running.lock().unwrap() {
        return Err("接收器已经在运行".to_string());
    }

    if let Some(name) = options.device_name {
        *state.device_name.lock().unwrap() = name;
    }
    if let Some(p) = options.port {
        *state.port.lock().unwrap() = p;
    }
    if let Some(d) = options.demo {
        *state.demo.lock().unwrap() = d;
    }
    if let Some(dir) = options.save_dir {
        if !dir.is_empty() {
            *state.save_dir.lock().unwrap() = dir;
        }
    }

    let demo = *state.demo.lock().unwrap();

    if demo {
        // Demo mode: the frontend renders a local test pattern. Nothing to
        // start on the backend side.
        *state.mode.lock().unwrap() = "demo".to_string();
        *state.running.lock().unwrap() = true;
        emit_status(&app, state.inner());
        return Ok(read_status(state.inner()));
    }

    // Real mode: load the native protocol library. It runs the AirPlay stack
    // and hands back the H.264 elementary stream, which we forward to the
    // webview over a binary `Channel` for WebCodecs to decode.
    *state.mode.lock().unwrap() = "real".to_string();
    let dll_path = ffi::locate_dll(&app).ok_or_else(|| {
        "未找到协议库 DLL。请将 airplay2dll.dll 放入 resources/airplay 目录 \
         （见 README 与 docs/ffi-contract.md）。"
            .to_string()
    })?;
    *state.mirror_lib_present.lock().unwrap() = true;

    let name = state.device_name.lock().unwrap().clone();
    let port = *state.port.lock().unwrap();
    let width = options.width.unwrap_or(0);
    let height = options.height.unwrap_or(0);
    let fps = options.fps.unwrap_or(0);

    // Re-entrancy guard: block concurrent starts that would double-load the
    // native DLL and corrupt its global server handle (a crash source).
    let _guard = StartingGuard::try_new(&state.starting)?;

    // The DLL delivers H.264 and connect/disconnect edges through callbacks, so
    // starting the receiver is all there is to do — no reader thread to spawn.
    // Its errors are already written for the user, so they pass through as-is
    // rather than collecting another prefix on the way to the toast.
    ffi::start_mirror(
        &dll_path,
        &name,
        port,
        width,
        height,
        fps,
        frame_channel,
        app.clone(),
    )?;

    *state.running.lock().unwrap() = true;

    emit_status(&app, state.inner());
    Ok(read_status(state.inner()))
}

#[tauri::command]
pub async fn stop_mirror(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<ReceiverStatus, String> {
    // Flip running first so any watchers unwind. No state lock may be held
    // across ffi::stop_mirror — it joins the DLL's network threads, which run
    // the callbacks that touch this same state.
    *state.running.lock().unwrap() = false;

    let _ = ffi::stop_mirror();
    *state.connected_device.lock().unwrap() = None;
    emit_status(&app, state.inner());
    Ok(read_status(state.inner()))
}

#[tauri::command]
pub fn get_status(state: State<'_, Arc<AppState>>) -> ReceiverStatus {
    read_status(state.inner())
}

#[tauri::command]
pub async fn save_screenshot(
    path: String,
    filename: String,
    data: String,
) -> Result<String, String> {
    write_file(path, filename, data, "screenshot").await
}

#[tauri::command]
pub async fn save_recording(
    path: String,
    filename: String,
    data: String,
) -> Result<String, String> {
    write_file(path, filename, data, "recording").await
}

async fn write_file(
    path: String,
    filename: String,
    data: String,
    kind: &str,
) -> Result<String, String> {
    let bytes = B64.decode(data).map_err(|e| format!("base64 decode failed: {e}"))?;
    let dir = if path.is_empty() {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string()
    } else {
        path
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let full = std::path::Path::new(&dir).join(&filename);
    std::fs::write(&full, bytes).map_err(|e| e.to_string())?;
    let _ = kind;
    Ok(full.to_string_lossy().to_string())
}

/// Open a directory (or file) in the OS file manager. Used by the settings
/// dialog's "打开" button to reveal the save folder.
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("目录为空".to_string());
    }
    if !std::path::Path::new(&path).exists() {
        return Err("目录不存在".to_string());
    }
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(&path)
            // explorer.exe returns a non-zero exit code even on success, so we
            // only care that the process launched.
            .spawn()
            .map_err(|e| format!("无法打开目录：{e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("无法打开目录：{e}"))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("无法打开目录：{e}"))?;
    }
    Ok(())
}
