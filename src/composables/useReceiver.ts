import { ref } from 'vue'
import { invoke, Channel } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { ReceiverStatus, StartOptions, TrackInfo, ViewMode } from '@/types'
import { PcmPlayer } from '@/lib/pcmPlayer'

// Reactive app state shared across components.
const status = ref<ReceiverStatus>({
  running: false,
  mode: 'real',
  deviceName: 'AirPlay Mirror',
  port: 7000,
  deviceId: '',
  connectedDevice: null,
  demo: false,
  saveDir: '',
  mirrorLibPresent: false,
  libReady: false,
  enableAudio: false,
  audioPlaying: false,
  track: null,
  width: 0,
  height: 0,
  fps: 0,
})
const logs = ref<string[]>([])
// True while a live picture is actually being shown (frames decoding). Distinct
// from `status.running` (receiver started but iPhone maybe not mirroring yet).
// Shared so the top status bar and the canvas stay in sync.
const mirroring = ref(false)
// Which pane the user is looking at. Independent of what the phone is doing:
// switching to 音频 while mirroring should not tear the session down.
const viewMode = ref<ViewMode>('mirror')
// Volume the phone last reported, in dB (0 = full, -144 = mute), or null.
const volumeDb = ref<number | null>(null)
// Album art as a data URL, or empty when the track has none. Kept out of
// ReceiverStatus so a ~200 KB string is not copied on every status update.
const artwork = ref('')

// Subscribers receive each raw H.264 (Annex-B) chunk as it arrives from the
// backend. A Set (not a reactive ref) so no chunk is coalesced away — every
// byte must reach the decoder in order.
type FrameHandler = (bytes: Uint8Array) => void
const frameSubscribers = new Set<FrameHandler>()

let listenersReady = false
const unlisteners: UnlistenFn[] = []

async function ensureListeners() {
  if (listenersReady) return
  unlisteners.push(
    await listen<ReceiverStatus>('status', (e) => {
      status.value = e.payload
    }),
  )
  unlisteners.push(
    await listen<string>('device_connected', (e) => {
      status.value = { ...status.value, connectedDevice: e.payload }
    }),
  )
  unlisteners.push(
    await listen<string>('log', (e) => {
      logs.value.push(e.payload)
      if (logs.value.length > 200) logs.value.shift()
    }),
  )
  // The backend fires this when the protocol library reports that the device
  // stopped mirroring (RTSP TEARDOWN or a dropped socket) — the authoritative
  // disconnect signal, as opposed to a merely idle/static screen (which keeps
  // the picture up).
  unlisteners.push(
    await listen('video_ended', () => {
      mirroring.value = false
    }),
  )
  // An audio-only session: the phone is using this machine as a speaker, so
  // there is no picture coming. Switch the view for the user rather than
  // leaving them on an empty canvas wondering whether it worked.
  unlisteners.push(
    await listen('audio_started', () => {
      status.value = { ...status.value, audioPlaying: true }
      viewMode.value = 'audio'
    }),
  )
  unlisteners.push(
    await listen('audio_ended', () => {
      status.value = { ...status.value, audioPlaying: false, track: null }
      artwork.value = ''
      audioPlayer?.stop()
      audioPlayer = null
    }),
  )
  unlisteners.push(
    await listen<TrackInfo>('track_metadata', (e) => {
      status.value = { ...status.value, track: e.payload }
    }),
  )
  unlisteners.push(
    await listen<number>('volume', (e) => {
      volumeDb.value = e.payload
    }),
  )
  unlisteners.push(
    await listen<string>('track_artwork', (e) => {
      artwork.value = e.payload
    }),
  )
  listenersReady = true
}

function toU8(msg: ArrayBuffer | Uint8Array): Uint8Array {
  return msg instanceof Uint8Array ? msg : new Uint8Array(msg)
}

// Built lazily on the first PCM chunk, which only arrives when the backend was
// started with audio enabled — so the frontend needs no branch of its own.
let audioPlayer: PcmPlayer | null = null

export function useReceiver() {
  async function refresh() {
    status.value = await invoke<ReceiverStatus>('get_status')
  }

  /** Subscribe to raw H.264 chunks. Returns an unsubscribe function. */
  function subscribeFrames(fn: FrameHandler): () => void {
    frameSubscribers.add(fn)
    return () => frameSubscribers.delete(fn)
  }

  async function start(opts?: StartOptions) {
    await ensureListeners()
    // Binary channel carrying the H.264 elementary stream from the backend.
    const channel = new Channel<ArrayBuffer>()
    let n = 0
    channel.onmessage = (buf: ArrayBuffer | Uint8Array) => {
      const bytes = toU8(buf)
      if (n < 3) {
        console.log(`[frontend] h264 chunk #${n}: ${bytes.byteLength} bytes`)
        n++
      }
      frameSubscribers.forEach((fn) => fn(bytes))
    }

    // Always opened; the backend only feeds it when audio is switched on.
    const audioChannel = new Channel<ArrayBuffer>()
    audioChannel.onmessage = (buf: ArrayBuffer | Uint8Array) => {
      if (!audioPlayer) {
        if (!PcmPlayer.isSupported()) {
          console.error('[audio] this WebView has no AudioWorklet, audio disabled')
          return
        }
        audioPlayer = new PcmPlayer()
      }
      audioPlayer.push(toU8(buf))
    }

    status.value = await invoke<ReceiverStatus>('start_mirror', {
      options: opts ?? {},
      frameChannel: channel,
      audioChannel,
    })
  }

  async function stop() {
    mirroring.value = false
    audioPlayer?.stop()
    audioPlayer = null
    status.value = await invoke<ReceiverStatus>('stop_mirror')
  }

  /** Persist settings without starting or stopping the receiver. */
  async function saveSettings(opts: StartOptions) {
    status.value = await invoke<ReceiverStatus>('update_settings', { options: opts })
  }

  /** Live frequency data for the visualiser, or null when nothing is playing. */
  function getAudioAnalyser(): AnalyserNode | null {
    return audioPlayer?.getAnalyser() ?? null
  }

  return {
    status,
    logs,
    mirroring,
    viewMode,
    volumeDb,
    artwork,
    getAudioAnalyser,
    refresh,
    start,
    stop,
    saveSettings,
    subscribeFrames,
    ensureListeners,
  }
}
