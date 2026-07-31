//! FFI bridge to the native AirPlay protocol + FFmpeg decode library.
//!
//! We deliberately do NOT launch `uxplay.exe`. Instead we `dlopen` a native C
//! library (built from xenos1337/AirPlayServer's `AirPlayServerLib` +
//! `airplay2dll`, or any library that honours the ABI below) and receive the
//! H.264 stream already decoded into YUV420 planar buffers through a C callback.
//! Rust then forwards those raw frames to the frontend over a Tauri binary
//! `Channel` (efficient, no base64 bloat). The frontend renders YUV420 -> RGB
//! via WebGL.
//!
//! ## C ABI contract (the native library must export these)
//!
//! ```c
//! typedef void (*frame_cb)(
//!     const uint8_t* y, const uint8_t* u, const uint8_t* v,
//!     int width, int height,
//!     int stride_y, int stride_u, int stride_v,
//!     void* userdata);
//!
//! typedef struct {
//!     const char* device_name; // UTF-8, e.g. "AirPlay Mirror"
//!     int         rtsp_port;   // 0 = default (7000)
//!     int         width;       // requested capture width (0 = auto)
//!     int         height;      // requested capture height (0 = auto)
//!     int         fps;         // requested fps (0 = auto)
//!     void*       userdata;    // opaque, passed back to frame_cb
//! } mirror_cfg;
//!
//! // returns 0 on success, non-zero on failure
//! int  mirror_start(const mirror_cfg* cfg, frame_cb cb);
//! void mirror_stop(void);
//! ```
//!
//! The library is responsible for: mDNS advertisement, RTSP/RTP handshake,
//! FairPlay session, and H.264 decode (via FFmpeg). It only needs to hand us
//! the decoded YUV420 planes. See `docs/ffi-contract.md` for a reference C
//! shim that wraps the SDL renderer in xenos1337's project.

use std::ffi::{c_char, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use libloading::{Library, Symbol};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter};

// ── Windows DLL search-path fix ──────────────────────────────────────────
// `Library::new()` calls `LoadLibraryExW` with an absolute path, but Windows
// does NOT search the DLL's own directory for its dependencies by default.
// `airplay2dll.dll` depends on `avcodec-58.dll`, `avutil-56.dll`,
// `swscale-5.dll`, `libwinpthread-1.dll`, and `msys-2.0.dll` — all sitting
// right next to it in `resources/ffmpeg/`. `SetDllDirectoryW` adds that
// directory to the search path so Windows can find them.
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn SetDllDirectoryW(lpPathName: *const u16) -> i32;
    fn GetShortPathNameW(long: *const u16, short: *mut u16, cch: u32) -> u32;
    fn PeekNamedPipe(
        h: *mut core::ffi::c_void,
        buf: *mut core::ffi::c_void,
        n_buf: u32,
        bytes_read: *mut u32,
        total_avail: *mut u32,
        bytes_left: *mut u32,
    ) -> i32;
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
/// `mirror_start`/`mirror_stop` on the same instance lets the receiver be
/// stopped and restarted cleanly within one session.
static DLL: OnceLock<Library> = OnceLock::new();

/// Diagnostic counter for `on_frame`. The current prebuilt DLL never calls the
/// callback (it muxes H.264 to a pipe instead), so this stays at 0 — but if a
/// future DLL rebuild wires up `outputVideo`→`frame_cb`, this log will show it.
static FRAME_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Load the DLL once (idempotent). Subsequent calls return the cached instance.
fn load_dll(dll_path: &str) -> Result<&'static Library, String> {
    if let Some(lib) = DLL.get() {
        return Ok(lib);
    }
    // CRITICAL: the MSYS2/FFmpeg DLL chain hangs (mirror_start never returns) when
    // loaded from a directory whose path contains a space — e.g. the default
    // install path `...\AirPlay Mirror\resources\ffmpeg`. Resolve a space-free
    // path before loading. Verified: an 8.3 short path (or a staged copy) fixes it.
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

/// Resolve a path to `airplay2dll.dll` that contains **no space** in any
/// component, because the DLL's runtime hangs otherwise. Strategy:
/// 1. If the given path already has no space, use it.
/// 2. Try the 8.3 short path (strips spaces where 8.3 names are enabled).
/// 3. Fall back to copying all sibling DLLs into a space-free staging dir.
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

/// Matches the ACTUAL exported ABI of `airplay2dll.dll`'s `Bridge.cpp`:
/// `void frame_cb(const uint8_t* y,u,v, int w,h, int sy,su,sv)` — note there
/// is NO trailing `userdata` argument (the earlier Rust signature had a spare
/// one that the DLL never pushes).
type FrameCb = unsafe extern "C" fn(
    *const u8,
    *const u8,
    *const u8,
    i32,
    i32,
    i32,
    i32,
    i32,
);

/// Byte-for-byte match of `Bridge.cpp`'s `struct mirror_cfg`:
/// `{ const char* server_name; unsigned raop_port; unsigned airplay_port;
///    const char* password; int width; int height; int fps; }`.
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

type MirrorStart = unsafe extern "C" fn(*const MirrorCfg, FrameCb) -> i32;
type MirrorStop = unsafe extern "C" fn();

/// C callback the DLL *would* invoke per decoded YUV frame. `mirror_start`
/// requires a non-null callback, but this prebuilt DLL never calls it (video is
/// delivered via the H.264 named pipe instead — see `spawn_pipe_forwarder`), so
/// this only logs if the situation ever changes.
unsafe extern "C" fn on_frame(
    _y: *const u8,
    _u: *const u8,
    _v: *const u8,
    width: i32,
    height: i32,
    _stride_y: i32,
    _stride_u: i32,
    _stride_v: i32,
) {
    let n = FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
    if n < 3 {
        eprintln!("[ffi] on_frame #{n}: {width}x{height} (unexpected — DLL now calls frame_cb)");
    }
}

/// Read the DLL's H.264 Annex-B named pipe and forward every chunk to the
/// frontend over the binary `Channel`.
///
/// The prebuilt `airplay2dll.dll` only muxes the raw H.264 elementary stream to
/// `\\.\pipe\AirPlayVideo`; it never invokes `frame_cb` (verified at runtime —
/// `on_frame` is never called). So instead of relying on the dead YUV callback,
/// we read that pipe here and ship the H.264 to the webview, which decodes it
/// with WebCodecs. This is also the reader that keeps the DLL's writer from
/// blocking, so no separate drain thread is needed.
#[cfg(windows)]
pub fn spawn_pipe_forwarder(channel: Channel<Vec<u8>>, running: Arc<AtomicBool>, app: AppHandle) {
    use std::io::Read;
    use std::os::windows::io::AsRawHandle;
    std::thread::spawn(move || {
        const PIPE: &str = r"\\.\pipe\AirPlayVideo";
        // Outer loop: (re)connect the pipe once per mirroring session so the
        // iPhone can disconnect and reconnect without restarting the receiver.
        while running.load(Ordering::Relaxed) {
            // Open the pipe (with retries — the DLL (re)creates it per session).
            let mut file = None;
            for _ in 0..100 {
                if !running.load(Ordering::Relaxed) {
                    return;
                }
                match std::fs::OpenOptions::new().read(true).open(PIPE) {
                    Ok(f) => {
                        file = Some(f);
                        break;
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
                }
            }
            let Some(mut f) = file else {
                // Not available yet; keep waiting while running.
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            };
            eprintln!("[forward] connected to {PIPE}; forwarding H.264 to webview");
            let handle = f.as_raw_handle() as *mut core::ffi::c_void;
            let mut buf = vec![0u8; 128 * 1024];
            let mut got_data = false;
            let mut last_data = std::time::Instant::now();
            let mut ended_signaled = false;
            // How long the (still-open) pipe must stay silent before we treat it
            // as a disconnect. This DLL does NOT close the pipe when the iPhone
            // stops AirPlay — it just goes quiet — so pipe-close alone never
            // fires. A static-but-connected screen also goes quiet, but only for
            // short gaps; this threshold sits above those. Tunable (raise if a
            // static screen flickers to 接收中; lower for a snappier clear).
            const IDLE_DISCONNECT: std::time::Duration = std::time::Duration::from_secs(3);

            // Inner loop: forward this session's H.264. The session ends either
            // when the pipe CLOSES or when it stays silent past IDLE_DISCONNECT.
            loop {
                if !running.load(Ordering::Relaxed) {
                    return;
                }
                let mut avail: u32 = 0;
                let ok = unsafe {
                    PeekNamedPipe(
                        handle,
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut avail,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    eprintln!("[forward] pipe closed — session ended");
                    break;
                }
                if avail == 0 {
                    // Silent. If we'd been streaming and it's been quiet too long,
                    // signal disconnect ONCE so the UI clears the frozen frame.
                    if got_data && !ended_signaled && last_data.elapsed() >= IDLE_DISCONNECT {
                        eprintln!(
                            "[forward] no data for {}s — signalling disconnect",
                            IDLE_DISCONNECT.as_secs()
                        );
                        let _ = app.emit("video_ended", ());
                        ended_signaled = true;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                match f.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        got_data = true;
                        last_data = std::time::Instant::now();
                        ended_signaled = false; // stream is live again
                        if channel.send(buf[..n].to_vec()).is_err() {
                            return; // frontend channel gone
                        }
                    }
                    Err(_) => break,
                }
            }

            // Pipe closed. If we were streaming and haven't already signalled the
            // idle disconnect, tell the UI now. Then loop to re-open for the next
            // session (iPhone reconnect / new AirPlay session).
            if got_data && !ended_signaled {
                let _ = app.emit("video_ended", ());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
}

#[cfg(not(windows))]
pub fn spawn_pipe_forwarder(_channel: Channel<Vec<u8>>, _running: Arc<AtomicBool>, _app: AppHandle) {}

/// Start mirroring on the (persistently loaded) native library.
///
/// `dll_path` points to the protocol+decode DLL. The `port` configures the
/// AirPlay (`_airplay._tcp`) service the iPhone connects to for screen
/// mirroring; RAOP audio uses `port - 1`. `width`/`height`/`fps` request a
/// capture size (0 = native).
pub fn start_mirror(
    dll_path: &str,
    device_name: &str,
    port: u16,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(), String> {
    let lib = load_dll(dll_path)?;

    let start: Symbol<MirrorStart> = unsafe {
        lib.get(b"mirror_start\0")
            .map_err(|_| "DLL 缺少导出符号 `mirror_start`".to_string())?
    };

    let c_name = CString::new(device_name)
        .map_err(|_| "设备名包含非法字符 (NUL)".to_string())?;

    // RAOP (audio) and AirPlay (mirroring) must be distinct ports. The UI's
    // configured `port` (default 7000) is the AirPlay/mirroring port; RAOP
    // takes the port just below it.
    let airplay_port = port as u32;
    let raop_port = port.saturating_sub(1) as u32;
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
        "[ffi] mirror_start: name='{device_name}' raop_port={raop_port} airplay_port={airplay_port} \
         size={width}x{height}@{fps}"
    );
    let rc = unsafe { start(&cfg as *const MirrorCfg, on_frame) };
    eprintln!("[ffi] mirror_start returned rc={rc}");
    if rc != 0 {
        return Err(format!("mirror_start 返回错误码 {rc}"));
    }

    // Keep the CString alive for the session.
    std::mem::forget(c_name);

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
        unsafe { stop() };
    }
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

