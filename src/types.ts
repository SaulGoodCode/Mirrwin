// Shared types between the Vue frontend and the Rust/Tauri backend.
// Keep these in sync with the structs in src-tauri/src/types.rs.

export type MirrorMode = 'demo' | 'real'

export interface ReceiverStatus {
  /** Whether the receiver is currently running. */
  running: boolean
  /** "demo" (local test pattern) or "real" (native library decode). */
  mode: MirrorMode
  /** Friendly name advertised to the iPhone (appears in Screen Mirroring). */
  deviceName: string
  /** TCP port the handshake / RTSP server listens on. */
  port: number
  /** AirPlay `deviceid` (usually the PC MAC address). */
  deviceId: string
  /** Name of the iPhone currently connected, or null. */
  connectedDevice: string | null
  /** When true, the backend streams a local test pattern instead of a real device. */
  demo: boolean
  /** Directory screenshots / recordings are saved to. */
  saveDir: string
  /** Whether the native protocol DLL was found (real mode usable). */
  mirrorLibPresent: boolean
  /** Whether the startup probe finished loading the native library. */
  libReady: boolean
  /** Whether the device's audio is played alongside the picture. */
  enableAudio: boolean
  /** Last chosen capture size / rate (0 = let the library decide). */
  width: number
  height: number
  fps: number
}

export interface StartOptions {
  deviceName?: string
  port?: number
  /** false => real mode (native FFI library); true => demo (local pattern). */
  demo?: boolean
  saveDir?: string
  /** Requested capture resolution / fps (0 = let the library decide). */
  width?: number
  height?: number
  fps?: number
  /** Play the device's audio too (default off). */
  enableAudio?: boolean
}
