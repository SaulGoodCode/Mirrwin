<script setup lang="ts">
// The audio-only view: what the app shows when the phone is using this machine
// as a speaker rather than mirroring its screen.
//
// The bars are driven by an AnalyserNode tapped off the live playback graph,
// so they follow what is actually being heard rather than what was queued. If
// nothing is playing the loop still runs but reads silence, which decays the
// bars to zero instead of freezing them mid-height.
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { Music } from 'lucide-vue-next'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useReceiver } from '@/composables/useReceiver'

const { status, getAudioAnalyser, volumeDb } = useReceiver()

const canvasRef = ref<HTMLCanvasElement | null>(null)
let raf: number | null = null
// Typed against ArrayBuffer specifically: getByteFrequencyData rejects a view
// that might be backed by a SharedArrayBuffer.
let bins: Uint8Array<ArrayBuffer> | null = null
// Whether the RAF loop should currently be running. Paused while the window is
// minimized so we don't burn CPU/GPU on bars nobody can see.
let rafRunning = false

function startRaf() {
  if (rafRunning) return
  rafRunning = true
  if (raf === null) raf = requestAnimationFrame(draw)
}

function stopRaf() {
  rafRunning = false
  if (raf !== null) {
    cancelAnimationFrame(raf)
    raf = null
  }
}

const track = computed(() => status.value.track)
const hasTrack = computed(() => !!(track.value?.title || track.value?.artist))

// AirPlay reports volume in dB: 0 is full scale and -144 means muted. Map the
// usable part of that range onto a percentage for display.
const volumePercent = computed(() => {
  const db = volumeDb.value
  if (db === null) return null
  if (db <= -144) return 0
  return Math.max(0, Math.min(100, Math.round(((db + 30) / 30) * 100)))
})

function draw() {
  raf = requestAnimationFrame(draw)
  const canvas = canvasRef.value
  if (!canvas) return
  const ctx = canvas.getContext('2d')
  if (!ctx) return

  // Match the backing store to the CSS size so the bars stay crisp.
  const width = canvas.clientWidth
  const height = canvas.clientHeight
  if (canvas.width !== width) canvas.width = width
  if (canvas.height !== height) canvas.height = height
  ctx.clearRect(0, 0, width, height)

  const analyser = getAudioAnalyser()
  if (!analyser) return
  if (!bins || bins.length !== analyser.frequencyBinCount) {
    bins = new Uint8Array(new ArrayBuffer(analyser.frequencyBinCount))
  }
  analyser.getByteFrequencyData(bins)

  // Only the lower part of the spectrum carries anything interesting for
  // music; the top bins are almost always empty and would waste the width.
  const used = Math.floor(bins.length * 0.6)
  const barCount = 48
  const gap = 3
  const barWidth = Math.max(2, (width - gap * (barCount - 1)) / barCount)

  for (let i = 0; i < barCount; i++) {
    // Group bins logarithmically so low frequencies are not all crammed into
    // the first bar or two.
    const from = Math.floor(Math.pow(i / barCount, 2) * used)
    const to = Math.max(from + 1, Math.floor(Math.pow((i + 1) / barCount, 2) * used))
    let peak = 0
    for (let b = from; b < to && b < bins.length; b++) peak = Math.max(peak, bins[b])

    const magnitude = peak / 255
    const barHeight = Math.max(2, magnitude * height)
    const x = i * (barWidth + gap)
    const y = height - barHeight

    const gradient = ctx.createLinearGradient(0, height, 0, y)
    gradient.addColorStop(0, 'rgba(99, 102, 241, 0.35)')
    gradient.addColorStop(1, `rgba(129, 140, 248, ${0.45 + magnitude * 0.55})`)
    ctx.fillStyle = gradient
    ctx.beginPath()
    ctx.roundRect(x, y, barWidth, barHeight, barWidth / 2)
    ctx.fill()
  }
}

// Pause the visualizer whenever the window is minimized, resume when it comes
// back. `tauri://focus` / `tauri://blur` fire on minimize and restore, but blur
// also fires in other situations (clicking another app), so we re-check
// `isMinimized()` before pausing or resuming to avoid stutter on alt-tab.
let unlistenFocus: (() => void) | null = null
let unlistenBlur: (() => void) | null = null

onMounted(async () => {
  startRaf()
  const win = getCurrentWindow()
  unlistenFocus = await win.listen('tauri://focus', async () => {
    // Restored from minimize: focus returns. Only resume if we actually paused
    // for minimize — otherwise this fires on every alt-tab back to the app.
    if (!rafRunning) {
      const min = await win.isMinimized()
      if (!min) startRaf()
    }
  })
  unlistenBlur = await win.listen('tauri://blur', async () => {
    // Pause only when truly minimized, not on every blur.
    if (rafRunning) {
      const min = await win.isMinimized()
      if (min) stopRaf()
    }
  })
})

onUnmounted(() => {
  stopRaf()
  unlistenFocus?.()
  unlistenBlur?.()
})
</script>

<template>
  <div class="flex flex-col h-full gap-3">
    <div
      class="flex-1 min-h-0 rounded-xl bg-gradient-to-b from-slate-900 to-slate-800 flex flex-col items-center justify-center gap-6 px-6 py-8 overflow-hidden"
    >
      <div class="flex flex-col items-center gap-2 text-center shrink-0">
        <div class="h-16 w-16 rounded-full bg-white/10 flex items-center justify-center">
          <Music class="h-8 w-8 text-indigo-300" />
        </div>
        <template v-if="hasTrack">
          <p class="text-lg font-semibold text-white truncate max-w-full">
            {{ track?.title || '未知曲目' }}
          </p>
          <p class="text-sm text-slate-300 truncate max-w-full">
            {{ track?.artist || '未知艺人' }}
          </p>
        </template>
        <template v-else>
          <p class="text-lg font-semibold text-white">
            {{ status.audioPlaying ? '正在播放' : '等待播放' }}
          </p>
          <p class="text-sm text-slate-400">
            在 iPhone 的「播放目标」中选择本设备即可播放音乐
          </p>
        </template>
        <p v-if="volumePercent !== null" class="text-xs text-slate-400">
          音量 {{ volumePercent }}%
        </p>
      </div>

      <canvas ref="canvasRef" class="w-full h-28 shrink-0"></canvas>
    </div>
  </div>
</template>
