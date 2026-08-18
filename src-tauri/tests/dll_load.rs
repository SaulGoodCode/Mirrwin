//! Drives the bundled protocol DLL through its ABI: loading from a path
//! containing a space, starting, stopping, restarting, and refusing a taken
//! port.
//!
//! This guards two bugs that were expensive to find. The app used to hang
//! forever at start when installed to the default `...\AirPlay Mirror\...`
//! directory — the bundled FFmpeg DLLs were Cygwin builds whose `msys-2.0.dll`
//! path mangling deadlocked there — and release builds have no console to
//! report it. And a failed startup used to be indistinguishable from a good
//! one, so a taken port left the UI claiming to receive while nothing listened.
//!
//! No phone is required: without one, the receiver simply advertises and stops.

#![cfg(windows)]

use std::ffi::{c_char, CString};
use std::time::{Duration, Instant};

use libloading::{Library, Symbol};

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

type VideoCb = unsafe extern "C" fn(*const u8, i32, i32);
type StateCb = unsafe extern "C" fn(i32, *const c_char, *const c_char);
type AudioCb = unsafe extern "C" fn(*const u8, i32, i32, i32, i32);
type MirrorStartAv =
    unsafe extern "C" fn(*const MirrorCfg, VideoCb, StateCb, Option<AudioCb>) -> i32;
type MirrorStop = unsafe extern "C" fn();

unsafe extern "C" fn on_video(_data: *const u8, _len: i32, _frame_type: i32) {}
unsafe extern "C" fn on_state(_event: i32, _name: *const c_char, _id: *const c_char) {}
unsafe extern "C" fn on_audio(_pcm: *const u8, _len: i32, _rate: i32, _ch: i32, _bits: i32) {}

#[link(name = "kernel32")]
extern "system" {
    fn SetDllDirectoryW(lpPathName: *const u16) -> i32;
}

fn set_dll_dir(dir: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe { SetDllDirectoryW(wide.as_ptr()) };
}

#[test]
fn loads_starts_and_reports_a_busy_port() {
    let resources = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("airplay");
    assert!(
        resources.join("airplay2dll.dll").exists(),
        "protocol DLL missing from {resources:?} — see tools/build-airplay-dll.sh"
    );

    // The space is the whole point of the test.
    let staged = std::env::temp_dir().join("airplay mirror dll test");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).expect("create staging dir");
    for entry in std::fs::read_dir(&resources).expect("read resources").flatten() {
        let p = entry.path();
        if p.extension().map(|e| e.eq_ignore_ascii_case("dll")).unwrap_or(false) {
            std::fs::copy(&p, staged.join(entry.file_name())).expect("stage dll");
        }
    }
    assert!(
        staged.to_string_lossy().contains(' '),
        "staging path lost its space, the test would prove nothing"
    );

    set_dll_dir(&staged);
    let dll = staged.join("airplay2dll.dll");

    let t0 = Instant::now();
    let lib = unsafe { Library::new(&dll) }.expect("load airplay2dll.dll from a spaced path");
    let load_time = t0.elapsed();
    assert!(load_time < Duration::from_secs(5), "load took {load_time:?}");

    let start: Symbol<MirrorStartAv> =
        unsafe { lib.get(b"mirror_start_av\0") }.expect("missing export mirror_start_av");
    let stop: Symbol<MirrorStop> =
        unsafe { lib.get(b"mirror_stop\0") }.expect("missing export mirror_stop");

    // A port unlikely to collide with a running instance of the app.
    let name = CString::new("Mirrwin DLL test").unwrap();
    let cfg = MirrorCfg {
        server_name: name.as_ptr(),
        raop_port: 7019,
        airplay_port: 7020,
        password: std::ptr::null(),
        width: 0,
        height: 0,
        fps: 0,
    };

    let t0 = Instant::now();
    let rc = unsafe { start(&cfg as *const MirrorCfg, on_video, on_state, Some(on_audio)) };
    let start_time = t0.elapsed();
    assert_eq!(rc, 0, "mirror_start_av returned {rc}");
    // The historical failure was an indefinite hang here, not a slow start.
    assert!(start_time < Duration::from_secs(5), "start took {start_time:?}");

    unsafe { stop() };

    // Restarting on the same never-unloaded library is how the UI's stop/start
    // works, and the DLL keeps global state across it.
    let rc = unsafe { start(&cfg as *const MirrorCfg, on_video, on_state, Some(on_audio)) };
    assert_eq!(rc, 0, "restart returned {rc}");
    unsafe { stop() };

    // A taken port must be reported, not swallowed. Upstream binds the exact
    // port it is given and fails, but used to return success anyway, leaving
    // the UI claiming to receive while nothing listened.
    let busy = std::net::TcpListener::bind(("0.0.0.0", 7041)).expect("occupy a port");
    let busy_cfg = MirrorCfg {
        server_name: name.as_ptr(),
        raop_port: 7040,
        airplay_port: 7041,
        password: std::ptr::null(),
        width: 0,
        height: 0,
        fps: 0,
    };
    let rc = unsafe { start(&busy_cfg as *const MirrorCfg, on_video, on_state, None) };
    assert_eq!(rc, -4, "a busy port should report -4, got {rc}");
    drop(busy);

    // And the receiver still starts normally afterwards: a rejected start must
    // not leave the library wedged.
    let rc = unsafe { start(&cfg as *const MirrorCfg, on_video, on_state, Some(on_audio)) };
    assert_eq!(rc, 0, "start after a rejected start returned {rc}");
    unsafe { stop() };

    let _ = std::fs::remove_dir_all(&staged);
}
