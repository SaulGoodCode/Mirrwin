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
//! // interleaved PCM already decoded from AAC; null = audio off
//! typedef void (*audio_cb)(const uint8_t* pcm, int len, int sample_rate, int channels,
//!                          int bits_per_sample);
//!
//! typedef struct {
//!     const char*  server_name;
//!     unsigned int raop_port;
//!     unsigned int airplay_port;
//!     const char*  password;
//!     int width, height, fps;   // reserved
//! } mirror_cfg;
//!
//! int  mirror_start_av(const mirror_cfg* cfg, video_cb vcb, state_cb scb, audio_cb acb);
//! void mirror_stop(void);
//! ```
//!
//! All callbacks run on the DLL's network threads, so they must return promptly
//! and must not hold a lock across a call back into the DLL. Their buffers are
//! only valid for the duration of the call.

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

/// The native protocol DLL, loaded exactly once for the process lifetime.
///
/// We deliberately never unload it: this DLL keeps global/static server state
/// and spawns its own worker threads, so `FreeLibrary`-then-reload on every
/// stop/start is a crash and hang source. Loading once and calling
/// `mirror_start_av`/`mirror_stop` on the same instance lets the receiver be
/// stopped and restarted cleanly within one session.
static DLL: OnceLock<Library> = OnceLock::new();

/// Keeps the device-name buffer alive for the duration of a session. The DLL
/// only reads it during registration, but it costs nothing to outlive the call
/// and it removes a class of dangling-pointer question from the FFI boundary.
static SERVER_NAME: Mutex<Option<CString>> = Mutex::new(None);

/// Load the DLL once (idempotent). Subsequent calls return the cached instance.
///
/// A path containing a space is fine. It was not always: loading from one used
/// to hang forever, because the bundled FFmpeg DLLs were Cygwin builds that
/// pulled in `msys-2.0.dll` and its path mangling. This DLL decodes nothing and
/// imports neither, and loads from a spaced path in milliseconds.
fn load_dll(dll_path: &str) -> Result<&'static Library, String> {
    if let Some(lib) = DLL.get() {
        return Ok(lib);
    }
    #[cfg(windows)]
    if let Some(parent) = std::path::Path::new(dll_path).parent() {
        add_dll_dir(&parent.to_string_lossy());
    }
    let lib = unsafe { Library::new(dll_path) }
        .map_err(|e| format!("无法加载协议库 DLL ({dll_path}): {e}"))?;
    // If another thread raced us, keep whichever won; both are the same DLL.
    let _ = DLL.set(lib);
    Ok(DLL.get().unwrap())
}

// ── ABI ──────────────────────────────────────────────────────────────────

type VideoCb = unsafe extern "C" fn(*const u8, i32, i32);
type StateCb = unsafe extern "C" fn(i32, *const c_char, *const c_char);
type AudioCb = unsafe extern "C" fn(*const u8, i32, i32, i32, i32);

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

/// `Option<AudioCb>` rather than a raw pointer: Rust's null-pointer
/// optimisation makes `None` a null function pointer across the FFI boundary,
/// which is exactly how the DLL is told to leave audio off.
type MirrorStartAv =
    unsafe extern "C" fn(*const MirrorCfg, VideoCb, StateCb, Option<AudioCb>) -> i32;
type MirrorStop = unsafe extern "C" fn();

const EVENT_CONNECTED: i32 = 0;
const EVENT_DISCONNECTED: i32 = 1;

/// Turn a `mirror_start_av` return code into something the user can act on.
/// The codes are defined in `tools/airplay-dll/Bridge.cpp`; the two that a user
/// can actually do something about are detected there specifically so they do
/// not arrive here as a generic failure.
fn describe_start_error(rc: i32, port: u16) -> String {
    match rc {
        1 => "协议库报告接收器已在运行。请先停止再重新开始。".to_string(),
        -1 => "内部错误：启动参数无效。".to_string(),
        -2 => "AirPlay 协议栈启动失败。请确认 Bonjour 服务正在运行，\
               并且防火墙允许本程序访问网络。"
            .to_string(),
        -3 => "未检测到 Bonjour（dnssd.dll）。iPhone 依靠 mDNS 发现本机，\
               缺少它就搜索不到本设备。请安装 Apple Bonjour（随 iTunes 提供）后重试。"
            .to_string(),
        -4 => format!(
            "端口 {} 或 {} 已被占用，可能已有另一个本程序或 AirPlay 接收端在运行。\
             请关闭它，或在设置中改用其他端口。",
            port,
            port.saturating_sub(1)
        ),
        other => format!("协议库返回未知错误码 {other}。"),
    }
}

// ── callback sink ────────────────────────────────────────────────────────

/// Where the DLL's callbacks deliver to. The C ABI carries no userdata, so the
/// destination has to be reachable from a plain `extern "C" fn`.
struct Sink {
    channel: Channel<Vec<u8>>,
    /// Present only while audio is enabled for this session.
    audio: Option<Channel<Vec<u8>>>,
    app: AppHandle,
}

static SINK: RwLock<Option<Sink>> = RwLock::new(None);

/// Per-session H.264 byte counter, for the first-frame log line only.
static STREAM_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Bytes of the header `on_audio` puts in front of each PCM chunk.
const AUDIO_HEADER_LEN: usize = 8;

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

/// Called by the DLL for each block of decoded PCM, on its network thread, and
/// only when audio was enabled at start.
///
/// The format is carried per chunk rather than announced once: it is whatever
/// the stream's AAC decoder reports, and re-sending eight bytes alongside a
/// ~2 KB payload costs nothing next to assuming it never changes. Layout is
/// little-endian `[u32 sample_rate][u16 channels][u16 bits_per_sample]`,
/// followed by the interleaved samples.
unsafe extern "C" fn on_audio(
    pcm: *const u8,
    len: i32,
    sample_rate: i32,
    channels: i32,
    bits_per_sample: i32,
) {
    if pcm.is_null() || len <= 0 {
        return;
    }
    let samples = std::slice::from_raw_parts(pcm, len as usize);
    let mut msg = Vec::with_capacity(AUDIO_HEADER_LEN + samples.len());
    msg.extend_from_slice(&(sample_rate.max(0) as u32).to_le_bytes());
    msg.extend_from_slice(&(channels.clamp(0, u16::MAX as i32) as u16).to_le_bytes());
    msg.extend_from_slice(&(bits_per_sample.clamp(0, u16::MAX as i32) as u16).to_le_bytes());
    msg.extend_from_slice(samples);

    guard_callback(move || {
        if let Ok(guard) = SINK.read() {
            if let Some(audio) = guard.as_ref().and_then(|s| s.audio.as_ref()) {
                let _ = audio.send(msg);
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

/// Install the callback destination. Must happen before `mirror_start_av`,
/// since the DLL may call back immediately.
fn set_sink(channel: Channel<Vec<u8>>, audio: Option<Channel<Vec<u8>>>, app: AppHandle) {
    if let Ok(mut guard) = SINK.write() {
        *guard = Some(Sink { channel, audio, app });
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
/// native stream. Passing `None` for `audio_channel` leaves audio off, and the
/// DLL then never enters its PCM path at all.
#[allow(clippy::too_many_arguments)]
pub fn start_mirror(
    dll_path: &str,
    device_name: &str,
    port: u16,
    width: u32,
    height: u32,
    fps: u32,
    channel: Channel<Vec<u8>>,
    audio_channel: Option<Channel<Vec<u8>>>,
    app: AppHandle,
) -> Result<(), String> {
    let lib = load_dll(dll_path)?;

    let start: Symbol<MirrorStartAv> = unsafe {
        lib.get(b"mirror_start_av\0").map_err(|_| {
            "DLL 缺少导出符号 `mirror_start_av`（协议库版本过旧，请用 \
             tools/build-airplay-dll.sh 重新构建）"
                .to_string()
        })?
    };

    let c_name = CString::new(device_name)
        .map_err(|_| "设备名包含非法字符 (NUL)".to_string())?;

    let airplay_port = port as u32;
    let raop_port = port.saturating_sub(1) as u32;
    let audio_on = audio_channel.is_some();

    // Callbacks can fire before mirror_start_av returns.
    set_sink(channel, audio_channel, app);

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
        "[ffi] mirror_start_av: name='{device_name}' raop_port={raop_port} \
         airplay_port={airplay_port} size={width}x{height}@{fps} audio={audio_on}"
    );
    let acb: Option<AudioCb> = if audio_on { Some(on_audio) } else { None };
    let rc = unsafe { start(&cfg as *const MirrorCfg, on_video, on_state, acb) };
    eprintln!("[ffi] mirror_start_av returned rc={rc}");
    if rc != 0 {
        clear_sink();
        return Err(describe_start_error(rc, port));
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

/// Locate the protocol library inside the app's bundled resources.
///
/// Looks for a few common names so the user doesn't have to rename precisely.
pub fn locate_dll(app: &AppHandle) -> Option<String> {
    const CANDIDATES: &[&str] = &[
        "airplay_bridge.dll",
        "airplay2dll.dll",
        "airplayserverlib.dll",
        "libairplay.dll",
    ];
    let dir = app
        .path()
        .resource_dir()
        .ok()?
        .join("resources")
        .join("airplay");
    for name in CANDIDATES {
        let p = dir.join(name);
        if p.exists() {
            return Some(p.to_string_lossy().to_string());
        }
    }
    None
}
