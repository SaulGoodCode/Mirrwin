// Plays the PCM the receiver sends alongside the picture.
//
// The native library hands Rust interleaved PCM already decoded from the
// stream's AAC (in practice 16-bit signed little-endian, 480 frames per
// packet), and Rust forwards it over a Tauri binary channel with an 8-byte
// header per chunk:
//
//   [u32 sampleRate][u16 channels][u16 bitsPerSample]  little-endian, then samples
//
// Playback goes through an AudioWorklet rather than scheduled AudioBuffers: at
// ~90 packets a second, scheduling a node per packet drifts and churns, while a
// worklet just drains a queue on the audio thread. A short prefill absorbs the
// jitter of packets crossing the IPC boundary; on underrun the worklet emits
// silence and re-prefills instead of glitching repeatedly.

const HEADER_BYTES = 8

// Buffered before playback starts, and re-armed after an underrun. ~120 ms is
// enough to ride out IPC hiccups without a noticeable lag behind the picture.
const PREFILL_MS = 120

// Runs on the audio thread. Kept as a string and loaded from a blob URL so it
// needs no separate asset in the bundle.
const WORKLET_SOURCE = `
class PcmSink extends AudioWorkletProcessor {
  constructor(options) {
    super()
    const o = options.processorOptions
    this.channels = o.channels
    this.prefill = o.prefill
    this.chunks = []
    this.head = 0
    this.queued = 0
    this.playing = false
    this.port.onmessage = (e) => {
      if (e.data === 'flush') {
        this.chunks = []
        this.head = 0
        this.queued = 0
        this.playing = false
        return
      }
      this.chunks.push(e.data)
      this.queued += e.data.length
    }
  }

  process(_inputs, outputs) {
    const out = outputs[0]
    if (!out || out.length === 0) return true
    const frames = out[0].length
    const ch = this.channels

    if (!this.playing) {
      if (this.queued < this.prefill) {
        for (let c = 0; c < out.length; c++) out[c].fill(0)
        return true
      }
      this.playing = true
    }

    for (let f = 0; f < frames; f++) {
      const cur = this.chunks[0]
      if (cur === undefined) {
        // Ran dry: finish the block in silence and wait for the buffer to
        // refill rather than stuttering through every following block.
        for (let c = 0; c < out.length; c++) out[c][f] = 0
        this.playing = false
        continue
      }
      for (let c = 0; c < out.length; c++) {
        const v = cur[this.head + (c < ch ? c : ch - 1)]
        out[c][f] = v === undefined ? 0 : v
      }
      this.head += ch
      this.queued -= ch
      if (this.head >= cur.length) {
        this.chunks.shift()
        this.head = 0
      }
    }
    return true
  }
}
registerProcessor('pcm-sink', PcmSink)
`

interface PcmFormat {
  sampleRate: number
  channels: number
  bits: number
}

function readHeader(bytes: Uint8Array): PcmFormat {
  const view = new DataView(bytes.buffer, bytes.byteOffset, HEADER_BYTES)
  return {
    sampleRate: view.getUint32(0, true),
    channels: view.getUint16(4, true),
    bits: view.getUint16(6, true),
  }
}

/** Signed 16-bit little-endian samples to the Float32 the audio graph wants. */
function toFloat32(bytes: Uint8Array): Float32Array {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const count = bytes.byteLength >> 1
  const out = new Float32Array(count)
  for (let i = 0; i < count; i++) out[i] = view.getInt16(i * 2, true) / 32768
  return out
}

export class PcmPlayer {
  private ctx: AudioContext | null = null
  private node: AudioWorkletNode | null = null
  private setup: Promise<void> | null = null
  private format: PcmFormat | null = null
  private warned = false

  static isSupported(): boolean {
    return typeof AudioContext !== 'undefined' && typeof AudioWorkletNode !== 'undefined'
  }

  /** Feed one chunk exactly as it arrived from the backend channel. */
  push(msg: Uint8Array) {
    if (msg.byteLength <= HEADER_BYTES) return
    const format = readHeader(msg)
    const body = msg.subarray(HEADER_BYTES)

    if (format.bits !== 16) {
      if (!this.warned) {
        this.warned = true
        console.error(`[audio] unsupported sample size ${format.bits}-bit, muting`)
      }
      return
    }
    if (!format.sampleRate || !format.channels) return

    // The graph is built for one format. A mid-session change (the phone
    // switching streams) means rebuilding it rather than resampling by hand.
    if (
      this.format &&
      (this.format.sampleRate !== format.sampleRate || this.format.channels !== format.channels)
    ) {
      console.warn('[audio] stream format changed, restarting playback graph')
      this.stop()
    }
    if (!this.setup) {
      this.format = format
      this.setup = this.build(format).catch((e) => {
        console.error('[audio] could not start playback:', e)
      })
    }
    // Dropped while the graph is still coming up; that is only the first few
    // packets, and starting mid-stream is normal for a live feed anyway.
    this.node?.port.postMessage(toFloat32(body))
  }

  private async build(format: PcmFormat) {
    const ctx = new AudioContext({ sampleRate: format.sampleRate, latencyHint: 'interactive' })
    this.ctx = ctx

    const url = URL.createObjectURL(new Blob([WORKLET_SOURCE], { type: 'application/javascript' }))
    try {
      await ctx.audioWorklet.addModule(url)
    } finally {
      URL.revokeObjectURL(url)
    }

    const node = new AudioWorkletNode(ctx, 'pcm-sink', {
      numberOfInputs: 0,
      numberOfOutputs: 1,
      outputChannelCount: [format.channels],
      processorOptions: {
        channels: format.channels,
        prefill: Math.round((format.sampleRate * PREFILL_MS) / 1000) * format.channels,
      },
    })
    node.connect(ctx.destination)
    this.node = node

    // Chromium starts a context suspended until the page has been interacted
    // with. Starting the receiver is a click, so this normally just resolves.
    if (ctx.state === 'suspended') {
      await ctx.resume()
    }
    if (ctx.state !== 'running') {
      console.warn(`[audio] context is ${ctx.state}; audio stays silent until it resumes`)
    }
  }

  /** Tear down the graph (call on stop / disconnect). Safe to call twice. */
  stop() {
    this.node?.port.postMessage('flush')
    this.node?.disconnect()
    this.node = null
    const ctx = this.ctx
    this.ctx = null
    this.setup = null
    this.format = null
    this.warned = false
    void ctx?.close().catch(() => {
      /* already closed */
    })
  }
}
