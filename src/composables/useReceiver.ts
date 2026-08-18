import { ref } from 'vue'
import { invoke, Channel } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { ReceiverStatus, StartOptions } from '@/types'
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
  enableAudio: false,
})
const logs = ref<string[]>([])
// True while a live picture is actually being shown (frames decoding). Distinct
// from `status.running` (receiver started but iPhone maybe not mirroring yet).
// Shared so the top status bar and the canvas stay in sync.
const mirroring = ref(false)

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

  return { status, logs, mirroring, refresh, start, stop, subscribeFrames, ensureListeners }
}
