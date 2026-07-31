// WebCodecs H.264 (Annex-B) decoder.
//
// The native AirPlay DLL muxes a raw H.264 Annex-B elementary stream to a named
// pipe; Rust forwards those bytes to us over a Tauri channel. We reassemble NAL
// units, configure a hardware `VideoDecoder` from the stream's SPS, and emit
// decoded `VideoFrame`s. WebView2 (Chromium 150) supports WebCodecs with
// hardware acceleration, so this decodes 1080p60 with negligible CPU and draws
// straight into the app's own canvas — no external player, no window embedding.

export interface H264DecoderOptions {
  onFrame: (frame: VideoFrame) => void
  onError?: (e: unknown) => void
}

// NAL unit types we care about.
const NAL_SPS = 7
const NAL_PPS = 8

function isVcl(type: number): boolean {
  return type >= 1 && type <= 5
}

function hex2(n: number): string {
  return n.toString(16).padStart(2, '0')
}

export class H264Decoder {
  private decoder: VideoDecoder | null = null
  private configured = false
  private sawKey = false
  private codecString = ''
  private frameIndex = 0
  // Non-VCL NALs (SPS/PPS/SEI/AUD) that precede the next coded picture. Each
  // entry includes its 4-byte Annex-B start code so we can feed the decoder a
  // valid Annex-B access unit.
  private pending: Uint8Array[] = []
  private buffer = new Uint8Array(0)

  constructor(private opts: H264DecoderOptions) {}

  static isSupported(): boolean {
    return typeof VideoDecoder !== 'undefined'
  }

  /** Feed a chunk of the Annex-B byte stream (arbitrary boundaries). */
  push(chunk: Uint8Array) {
    if (this.buffer.length === 0) {
      this.buffer = chunk.slice()
    } else {
      const merged = new Uint8Array(this.buffer.length + chunk.length)
      merged.set(this.buffer)
      merged.set(chunk, this.buffer.length)
      this.buffer = merged
    }
    this.parse()
  }

  /** Tear down the decoder and reset state (call on stop). */
  reset() {
    try {
      this.decoder?.close()
    } catch {
      /* already closed */
    }
    this.decoder = null
    this.configured = false
    this.sawKey = false
    this.pending = []
    this.buffer = new Uint8Array(0)
    this.frameIndex = 0
  }

  private ensureDecoder() {
    if (this.decoder) return
    this.decoder = new VideoDecoder({
      output: (frame) => this.opts.onFrame(frame),
      error: (e) => this.opts.onError?.(e),
    })
  }

  // Locate Annex-B start codes and dispatch each complete NAL. The trailing
  // bytes after the last start code are kept for the next push (a NAL is only
  // complete once the *following* start code arrives).
  private parse() {
    const buf = this.buffer
    const starts: Array<{ pos: number; len: number }> = []
    let i = 0
    while (i + 2 < buf.length) {
      if (buf[i] === 0 && buf[i + 1] === 0) {
        if (buf[i + 2] === 1) {
          starts.push({ pos: i, len: 3 })
          i += 3
          continue
        }
        if (i + 3 < buf.length && buf[i + 2] === 0 && buf[i + 3] === 1) {
          starts.push({ pos: i, len: 4 })
          i += 4
          continue
        }
      }
      i++
    }
    if (starts.length < 2) return // need a following start code to bound a NAL

    for (let k = 0; k < starts.length - 1; k++) {
      const nalStart = starts[k].pos + starts[k].len
      const nalEnd = starts[k + 1].pos
      if (nalEnd <= nalStart) continue
      // annexB includes the start code so the decoder gets a valid AU.
      const annexB = buf.subarray(starts[k].pos, nalEnd)
      const type = buf[nalStart] & 0x1f
      this.handleNal(type, buf.subarray(nalStart, nalEnd), annexB)
    }

    // Retain from the last (still-open) start code onward.
    this.buffer = buf.slice(starts[starts.length - 1].pos)
  }

  private handleNal(type: number, rbsp: Uint8Array, annexB: Uint8Array) {
    if (type === NAL_SPS) {
      this.configureFromSps(rbsp)
      this.pending.push(annexB)
    } else if (type === NAL_PPS) {
      this.pending.push(annexB)
    } else if (isVcl(type)) {
      // A coded slice ends the access unit. AirPlay mirroring uses single-slice
      // frames, so treat each VCL NAL as one picture.
      const isIdr = type === 5
      const hasParamSets = this.pending.length > 0
      const key = isIdr || hasParamSets
      const parts = [...this.pending, annexB]
      this.pending = []
      this.decodeAccessUnit(parts, key)
    } else {
      // SEI (6), AUD (9), etc. — carry along with the next picture.
      this.pending.push(annexB)
    }
  }

  private configureFromSps(rbsp: Uint8Array) {
    // rbsp[0] = NAL header (0x67); [1]=profile_idc, [2]=constraint flags,
    // [3]=level_idc. These bytes never contain emulation-prevention escapes.
    if (rbsp.length < 4) return
    const profile = rbsp[1]
    const constraint = rbsp[2]
    const level = rbsp[3]
    const codec = `avc1.${hex2(profile)}${hex2(constraint)}${hex2(level)}`
    if (this.configured && codec === this.codecString) return
    this.codecString = codec
    this.ensureDecoder()
    try {
      // No `description` ⇒ the decoder expects Annex-B input (start codes),
      // which is exactly what we feed it.
      this.decoder!.configure({
        codec,
        optimizeForLatency: true,
      } as VideoDecoderConfig)
      this.configured = true
    } catch (e) {
      this.opts.onError?.(e)
    }
  }

  private decodeAccessUnit(parts: Uint8Array[], key: boolean) {
    if (!this.configured) return // wait for the first SPS
    if (!this.sawKey) {
      if (!key) return // a decoder must start on a keyframe
      this.sawKey = true
    }
    let total = 0
    for (const p of parts) total += p.length
    const data = new Uint8Array(total)
    let off = 0
    for (const p of parts) {
      data.set(p, off)
      off += p.length
    }
    try {
      this.decoder!.decode(
        new EncodedVideoChunk({
          type: key ? 'key' : 'delta',
          timestamp: this.frameIndex * 33333, // µs; monotonic, ~30fps pacing
          data,
        }),
      )
      this.frameIndex++
    } catch (e) {
      this.opts.onError?.(e)
    }
  }
}
