//! Loads the bundled protocol DLL from a path containing a space and drives one
//! start/stop cycle.
//!
//! This guards a bug that was expensive to find: the app used to hang forever
//! at `mirror_start` when installed to the default `...\AirPlay Mirror\...`
//! directory, and release builds have no console to report it. The cause was
//! the bundled FFmpeg DLLs, which were Cygwin builds pulling in `msys-2.0.dll`
//! and its path mangling. Those are gone and `ffi.rs` no longer carries a
//! short-path workaround, so this test is what keeps the regression visible.
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
type MirrorStartEx = unsafe extern "C" fn(*const MirrorCfg, VideoCb, StateCb) -> i32;
type MirrorStop = unsafe extern "C" fn();

unsafe extern "C" fn on_video(_data: *const u8, _len: i32, _frame_type: i32) {}
unsafe extern "C" fn on_state(_event: i32, _name: *const c_char, _id: *const c_char) {}

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
fn loads_and_starts_from_a_path_containing_a_space() {
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

    let start: Symbol<MirrorStartEx> =
        unsafe { lib.get(b"mirror_start_ex\0") }.expect("missing export mirror_start_ex");
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
    let rc = unsafe { start(&cfg as *const MirrorCfg, on_video, on_state) };
    let start_time = t0.elapsed();
    assert_eq!(rc, 0, "mirror_start_ex returned {rc}");
    // The historical failure was an indefinite hang here, not a slow start.
    assert!(start_time < Duration::from_secs(5), "start took {start_time:?}");

    unsafe { stop() };

    // Restarting on the same never-unloaded library is how the UI's stop/start
    // works, and the DLL keeps global state across it.
    let rc = unsafe { start(&cfg as *const MirrorCfg, on_video, on_state) };
    assert_eq!(rc, 0, "restart returned {rc}");
    unsafe { stop() };

    let _ = std::fs::remove_dir_all(&staged);
}
