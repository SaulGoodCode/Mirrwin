//! FFI bridge to the native AirPlay protocol library.
//!
//! We `dlopen` `airplay2dll.dll` (built from xenos1337/AirPlayServer plus the
//! overlay in `tools/airplay-dll/` — see `tools/build-airplay-dll.sh`) and let
//! it run the AirPlay stack: mDNS advertisement, RTSP/RTP, FairPlay. It hands
//! back the H.264 elementary stream as it arrives, which we forward to the
//! webview over a Tauri binary `Channel`; the frontend decodes it with
//! WebCodecs. Nothing here decodes video, and no named pipe is involved.
//!
//! ## C ABI contract (see `tools/airplay-dll/Bridge.cpp`)
//!
//! ```c
//! // frame_type: 0 = SPS/PPS parameter sets, 1 = picture data
//! typedef void (*video_cb)(const uint8_t* data, int len, int frame_type);
//! // event: 0 = connected, 1 = disconnected
//! typedef void (*state_cb)(int event, const char* remote_name, const char* device_id);
//!
//! typedef struct {
//!     const char*  server_name;
//!     unsigned int raop_port;
//!     unsigned int airplay_port;
//!     const char*  password;
//!     int width, height, fps;   // reserved
//! } mirror_cfg;
//!
//! int  mirror_start_ex(const mirror_cfg* cfg, video_cb vcb, state_cb scb);
//! void mirror_stop(void);
//! ```
//!
//! Both callbacks run on the DLL's network threads, so they must return
//! promptly and must not hold a lock across a call back into the DLL. The
//! `data` pointer is only valid for the duration of the call.

use std::ffi::{c_char, CStr, CString};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use libloading::{Library, Symbol};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

// ── Windows DLL search-path fix ──────────────────────────────────────────
// `Library::new()` calls `LoadLibraryExW` with an absolute path, but Windows
// does NOT search the DLL's own directory for its dependencies by default.
// `airplay2dll.dll` needs `libwinpthread-1.dll` from the same directory, so
// `SetDllDirectoryW` puts that directory on the search path.
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn SetDllDirectoryW(lpPathName: *const u16) -> i32;
    fn GetShortPathNameW(long: *const u16, short: *mut u16, cch: u32) -> u32;
}

#[cfg(windows)]
fn to_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn add_dll_dir(dir: &str) {
    // Safe: SetDllDirectoryW just modifies the search order, no UB.
    unsafe { SetDllDirectoryW(to_wide(dir).as_ptr()) };
}

/// Return the 8.3 short-path form of an existing path (strips spaces), or None
/// if the volume has 8.3 names disabled / the call fails.
#[cfg(windows)]
fn short_path(path: &str) -> Option<String> {
    let wide = to_wide(path);
    let mut buf = vec![0u16; 1024];
    let len = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 || (len as usize) >= buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// The native protocol DLL, loaded exactly once for the process lifetime.
///
/// We deliberately never unload it: this DLL keeps global/static server state
/// and spawns its own worker threads, so `FreeLibrary`-then-reload on every
/// stop/start is a crash and hang source. Loading once and calling
/// `mirror_start_ex`/`mirror_stop` on the same instance lets the receiver be
/// stopped and restarted cleanly within one session.
static DLL: OnceLock<Library> = OnceLock::new();

/// Keeps the device-name buffer alive for the duration of a session. The DLL
/// only reads it during registration, but it costs nothing to outlive the call
/// and it removes a class of dangling-pointer question from the FFI boundary.
static SERVER_NAME: Mutex<Option<CString>> = Mutex::new(None);

/// Load the DLL once (idempotent). Subsequent calls return the cached instance.
fn load_dll(dll_path: &str) -> Result<&'static Library, String> {
    if let Some(lib) = DLL.get() {
        return Ok(lib);
    }
    let load_path = resolve_space_free_dll(dll_path)?;
    #[cfg(windows)]
    if let Some(parent) = std::path::Path::new(&load_path).parent() {
        add_dll_dir(&parent.to_string_lossy());
    }
    let lib = unsafe { Library::new(&load_path) }
        .map_err(|e| format!("无法加载协议库 DLL ({load_path}): {e}"))?;
    // If another thread raced us, keep whichever won; both are the same DLL.
    let _ = DLL.set(lib);
    Ok(DLL.get().unwrap())
}

/// Resolve a load path for `airplay2dll.dll` with no space in any component.
///
/// A space in the path used to hang `mirror_start` forever. The cause was the
/// bundled FFmpeg DLLs, which were Cygwin builds pulling in `msys-2.0.dll` and
/// its path mangling; the current DLL decodes nothing and imports neither, and
/// a spaced path now loads and starts in milliseconds. This is kept as cheap
/// insurance — it is a no-op for paths without a space — and can be dropped
/// once the installed build has been exercised from `C:\Program Files\…`.
#[cfg(windows)]
fn resolve_space_free_dll(dll_path: &str) -> Result<String, String> {
    if !dll_path.contains(' ') {
        return Ok(dll_path.to_string());
    }
    if let Some(sp) = short_path(dll_path) {
        if !sp.contains(' ') {
            return Ok(sp);
        }
    }
    stage_dlls_space_free(dll_path)
}

#[cfg(not(windows))]
fn resolve_space_free_dll(dll_path: &str) -> Result<String, String> {
    Ok(dll_path.to_string())
}

/// Copy every DLL sitting next to `dll_path` into a space-free directory and
/// return the staged path of `airplay2dll.dll`. Used only when 8.3 short names
/// are unavailable on the install volume.
#[cfg(windows)]
fn stage_dlls_space_free(dll_path: &str) -> Result<String, String> {
    let src_path = std::path::Path::new(dll_path);
    let src_dir = src_path.parent().ok_or("无法解析 DLL 目录")?;
    let dll_name = src_path.file_name().ok_or("无法解析 DLL 文件名")?;

    let root = pick_space_free_root()?;
    let dst_dir = std::path::Path::new(&root).join("airplay-mirror-lib");
    std::fs::create_dir_all(&dst_dir).map_err(|e| format!("创建暂存目录失败：{e}"))?;

    for entry in std::fs::read_dir(src_dir).map_err(|e| e.to_string())?.flatten() {
        let p = entry.path();
        let is_dll = p
            .extension()
            .map(|e| e.eq_ignore_ascii_case("dll"))
            .unwrap_or(false);
        if !is_dll {
            continue;
        }
        let dst = dst_dir.join(entry.file_name());
        // Idempotent: only copy when missing or a different size.
        let need = match (std::fs::metadata(&p), std::fs::metadata(&dst)) {
            (Ok(a), Ok(b)) => a.len() != b.len(),
            _ => true,
        };
        if need {
            std::fs::copy(&p, &dst).map_err(|e| format!("复制 {:?} 失败：{e}", entry.file_name()))?;
        }
    }

    let staged = dst_dir.join(dll_name).to_string_lossy().to_string();
    if staged.contains(' ') {
        // Staging root had a space and no 8.3 form — try short-pathing it.
        if let Some(sp) = short_path(&staged) {
            if !sp.contains(' ') {
                return Ok(sp);
            }
        }
        return Err("无法找到无空格的 DLL 加载路径".to_string());
    }
    Ok(staged)
}

/// Pick a directory root with no space in its path (short-pathing candidates as
/// needed). Prefers TEMP, then ProgramData, then the system drive root.
#[cfg(windows)]
fn pick_space_free_root() -> Result<String, String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(t) = std::env::var("TEMP") {
        candidates.push(t);
    }
    if let Ok(pd) = std::env::var("ProgramData") {
        candidates.push(pd);
    }
    candidates.push(
        std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string()) + "\\",
    );
    for c in candidates {
        if !std::path::Path::new(&c).exists() {
            continue;
        }
        if !c.contains(' ') {
            return Ok(c);
        }
        if let Some(sp) = short_path(&c) {
            if !sp.contains(' ') {
                return Ok(sp);
            }
        }
    }
    Err("找不到无空格的暂存目录".to_string())
}

// ── ABI ──────────────────────────────────────────────────────────────────

type VideoCb = unsafe extern "C" fn(*const u8, i32, i32);
type StateCb = unsafe extern "C" fn(i32, *const c_char, *const c_char);

/// Byte-for-byte match of `Bridge.cpp`'s `struct mirror_cfg`.
#[repr(C)]
struct MirrorCfg {
    server_name: *const c_char,
    raop_port: u32,
    airplay_port: u32,
    password: *const c_char,
    width: i32,
    height: i32,
    fps: i32,
}

type MirrorStartEx = unsafe extern "C" fn(*const MirrorCfg, VideoCb, StateCb) -> i32;
type MirrorStop = unsafe extern "C" fn();

const EVENT_CONNECTED: i32 = 0;
const EVENT_DISCONNECTED: i32 = 1;

// ── callback sink ────────────────────────────────────────────────────────

/// Where the DLL's callbacks deliver to. The C ABI carries no userdata, so the
/// destination has to be reachable from a plain `extern "C" fn`.
struct Sink {
    channel: Channel<Vec<u8>>,
    app: AppHandle,
}

static SINK: RwLock<Option<Sink>> = RwLock::new(None);

/// Per-session H.264 byte counter, for the first-frame log line only.
static STREAM_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Called by the DLL for each H.264 access unit, on its network thread.
/// `data` is freed as soon as this returns, so the bytes are copied here.
unsafe extern "C" fn on_video(data: *const u8, len: i32, frame_type: i32) {
    if data.is_null() || len <= 0 {
        return;
    }
    let bytes = std::slice::from_raw_parts(data, len as usize).to_vec();
    // A panic unwinding into the DLL's thread would abort the process, so the
    // Rust side of every callback is contained.
    guard_callback(move || {
        let prev = STREAM_BYTES.fetch_add(bytes.len(), std::sync::atomic::Ordering::Relaxed);
        if prev == 0 {
            eprintln!("[ffi] first H.264 packet: frame_type={frame_type} len={len}");
        }
        if let Ok(guard) = SINK.read() {
            if let Some(sink) = guard.as_ref() {
                let _ = sink.channel.send(bytes);
            }
        }
    });
}

/// Called by the DLL when a device starts or stops mirroring, on its network
/// thread. AirPlayServerLib raises this from `raop_rtp_start_mirror` and
/// `raop_rtp_mirror_stop`, so a disconnect is reported both for a clean RTSP
/// TEARDOWN and for an abruptly dropped socket.
unsafe extern "C" fn on_state(event: i32, name: *const c_char, id: *const c_char) {
    let name = cstr_to_string(name);
    let id = cstr_to_string(id);
    guard_callback(move || on_state_inner(event, name, id));
}

fn on_state_inner(event: i32, name: String, id: String) {
    let label = match event {
        EVENT_CONNECTED => "connected",
        EVENT_DISCONNECTED => "disconnected",
        // Ignore codes this build does not know rather than mistaking one for
        // a disconnect and clearing the picture.
        other => {
            eprintln!("[ffi] state: unknown event {other}, ignored");
            return;
        }
    };
    eprintln!("[ffi] state: {label} name='{name}' id='{id}'");

    // Clone the handle out so no lock is held while emitting.
    let app = match SINK.read() {
        Ok(guard) => match guard.as_ref() {
            Some(sink) => sink.app.clone(),
            None => return,
        },
        Err(_) => return,
    };

    let display = if name.is_empty() { id } else { name };
    let state = app.state::<Arc<AppState>>().inner().clone();

    if event == EVENT_CONNECTED {
        *state.connected_device.lock().unwrap() = Some(display.clone());
        let _ = app.emit("device_connected", display);
    } else {
        STREAM_BYTES.store(0, std::sync::atomic::Ordering::Relaxed);
        *state.connected_device.lock().unwrap() = None;
        // The frontend keeps the last picture up until this fires — it is the
        // authoritative "stopped mirroring" edge, not a guess from idle time.
        let _ = app.emit("video_ended", ());
    }
    crate::commands::emit_status(&app, &state);
}

/// Run a callback body so that a panic is reported instead of unwinding into
/// the C++ frames that called us.
fn guard_callback(f: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
        eprintln!("[ffi] panic inside a native callback was contained");
    }
}

fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Install the callback destination. Must happen before `mirror_start_ex`,
/// since the DLL may call back immediately.
fn set_sink(channel: Channel<Vec<u8>>, app: AppHandle) {
    if let Ok(mut guard) = SINK.write() {
        *guard = Some(Sink { channel, app });
    }
    STREAM_BYTES.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// Drop the callback destination. Only call this *after* the DLL has stopped:
/// taking the write lock while a callback holds the read lock inside a thread
/// the DLL is joining would deadlock.
fn clear_sink() {
    if let Ok(mut guard) = SINK.write() {
        *guard = None;
    }
}

// ── public API ───────────────────────────────────────────────────────────

/// Start mirroring on the (persistently loaded) native library.
///
/// `port` is the AirPlay (`_airplay._tcp`) port the iPhone connects to for
/// screen mirroring; RAOP audio uses `port - 1`. `width`/`height`/`fps` are
/// accepted for ABI compatibility but the receiver always uses the device's
/// native stream.
#[allow(clippy::too_many_arguments)]
pub fn start_mirror(
    dll_path: &str,
    device_name: &str,
    port: u16,
    width: u32,
    height: u32,
    fps: u32,
    channel: Channel<Vec<u8>>,
    app: AppHandle,
) -> Result<(), String> {
    let lib = load_dll(dll_path)?;

    let start: Symbol<MirrorStartEx> = unsafe {
        lib.get(b"mirror_start_ex\0").map_err(|_| {
            "DLL 缺少导出符号 `mirror_start_ex`（协议库版本过旧，请用 \
             tools/build-airplay-dll.sh 重新构建）"
                .to_string()
        })?
    };

    let c_name = CString::new(device_name)
        .map_err(|_| "设备名包含非法字符 (NUL)".to_string())?;

    let airplay_port = port as u32;
    let raop_port = port.saturating_sub(1) as u32;

    // Callbacks can fire before mirror_start_ex returns.
    set_sink(channel, app);

    let cfg = MirrorCfg {
        server_name: c_name.as_ptr(),
        raop_port,
        airplay_port,
        password: std::ptr::null(),
        width: width as i32,
        height: height as i32,
        fps: fps as i32,
    };

    eprintln!(
        "[ffi] mirror_start_ex: name='{device_name}' raop_port={raop_port} \
         airplay_port={airplay_port} size={width}x{height}@{fps}"
    );
    let rc = unsafe { start(&cfg as *const MirrorCfg, on_video, on_state) };
    eprintln!("[ffi] mirror_start_ex returned rc={rc}");
    if rc != 0 {
        clear_sink();
        return Err(format!("mirror_start_ex 返回错误码 {rc}"));
    }

    *SERVER_NAME.lock().unwrap() = Some(c_name);
    Ok(())
}

/// Stop mirroring by calling the DLL's `mirror_stop` on the persistent library.
/// Safe to call even if never started (the DLL guards its own state).
pub fn stop_mirror() -> Result<(), String> {
    let Some(lib) = DLL.get() else {
        return Ok(());
    };
    let stop: Result<Symbol<MirrorStop>, _> = unsafe { lib.get(b"mirror_stop\0") };
    if let Ok(stop) = stop {
        eprintln!("[ffi] mirror_stop");
        // No Rust lock may be held here: mirror_stop joins the DLL's network
        // threads, and those threads take the sink's read lock.
        unsafe { stop() };
    }
    clear_sink();
    *SERVER_NAME.lock().unwrap() = None;
    Ok(())
}

/// Locate the protocol library inside the bundled resources directory.
///
/// Looks for a few common names so the user doesn't have to rename precisely.
pub fn locate_dll(resources_dir: &std::path::Path) -> Option<String> {
    const CANDIDATES: &[&str] = &[
        "airplay_bridge.dll",
        "airplay2dll.dll",
        "airplayserverlib.dll",
        "libairplay.dll",
    ];
    for name in CANDIDATES {
        let p = resources_dir.join(name);
        if p.exists() {
            return Some(p.to_string_lossy().to_string());
        }
    }
    None
}
