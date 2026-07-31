use serde::{Deserialize, Serialize};

/// Shared with the frontend (see src/types.ts). Field names are serialized as
/// camelCase so they line up with the TypeScript interfaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverStatus {
    pub running: bool,
    /// "demo" (local test pattern) or "real" (native library decode).
    pub mode: String,
    pub device_name: String,
    pub port: u16,
    pub device_id: String,
    pub connected_device: Option<String>,
    pub demo: bool,
    pub save_dir: String,
    /// Whether the native protocol+decode DLL was found and can be used.
    pub mirror_lib_present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StartOptions {
    pub device_name: Option<String>,
    pub port: Option<u16>,
    /// false => real mode (native FFI library); true => demo (local pattern).
    pub demo: Option<bool>,
    pub save_dir: Option<String>,
    /// Requested capture resolution / fps (0 = let the library decide).
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<u32>,
}
