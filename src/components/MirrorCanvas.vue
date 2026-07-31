<script setup lang="ts">
import { ref, watch, onMounted, onUnmounted, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { toast } from 'vue-sonner'
import { useReceiver } from '@/composables/useReceiver'
import { Button } from '@/components/ui/button'
import { H264Decoder } from '@/lib/h264Decoder'

const { status, subscribeFrames, mirroring } = useReceiver()

const canvasRef = ref<HTMLCanvasElement | null>(null)
const recording = ref(false)
// Live video dimensions, so the container hugs the real aspect ratio
// (portrait for phone mirroring) instead of a fixed landscape letterbox.
const videoW = ref(0)
const videoH = ref(0)

let decoder: H264Decoder | null = null
let unsubscribe: (() => void) | null = null
let ctx: CanvasRenderingContext2D | null = null
let recorder: MediaRecorder | null = null
let chunks: BlobPart[] = []

const isRunning = computed(() => status.value.running)
const aspectRatio = computed(() =>
  videoW.value && videoH.value ? `${videoW.value} / ${videoH.value}` : '9 / 16',
)

function clearCanvas() {
  const c = canvasRef.value
  if (c && ctx) ctx.clearRect(0, 0, c.width, c.height)
}

function drawFrame(frame: VideoFrame) {
  const c = canvasRef.value
  if (!c) {
    frame.close()
    return
  }
  if (!ctx) ctx = c.getContext('2d')
  const w = frame.displayWidth
  const h = frame.displayHeight
  if (c.width !== w) c.width = w
  if (c.height !== h) c.height = h
  if (videoW.value !== w) videoW.value = w
  if (videoH.value !== h) videoH.value = h
  ctx?.drawImage(frame, 0, 0)
  frame.close()
  // As long as frames arrive we're mirroring. There is deliberately NO
  // "no frames for N seconds → disconnected" watchdog: a static iPhone screen
  // legitimately sends no frames for long stretches. Disconnect is instead
  // signalled by the backend `video_ended` event (real pipe close).
  mirroring.value = true
}

// Clear the picture only when the receiver truly loses the stream (mirroring
// flipped to false by the video_ended event or by stop).
watch(mirroring, (v) => {
  if (!v) {
    videoW.value = 0
    videoH.value = 0
    clearCanvas()
  }
})

function startDecoding() {
  if (!H264Decoder.isSupported()) {
    toast.error('当前 WebView 不支持 WebCodecs，无法解码视频（请更新 WebView2 运行时）')
    return
  }
  decoder = new H264Decoder({
    onFrame: drawFrame,
    onError: (e) => console.error('[h264] decode error:', e),
  })
  unsubscribe = subscribeFrames((bytes) => decoder?.push(bytes))
}

function stopDecoding() {
  unsubscribe?.()
  unsubscribe = null
  decoder?.reset()
  decoder = null
  mirroring.value = false
  videoW.value = 0
  videoH.value = 0
  clearCanvas()
}

watch(isRunning, (running) => {
  if (running) {
    mirroring.value = false
    startDecoding()
  } else {
    stopDecoding()
  }
})

onMounted(() => {
  const c = canvasRef.value
  if (c) ctx = c.getContext('2d')
})

onUnmounted(() => {
  stopDecoding()
})

async function screenshot() {
  const c = canvasRef.value
  if (!c || !mirroring.value) {
    toast.error('暂无画面，无法截图')
    return
  }
  let base64: string
  try {
    const dataUrl = c.toDataURL('image/png')
    base64 = dataUrl.split(',')[1] ?? ''
    if (!base64) throw new Error('画面为空')
  } catch (e) {
    toast.error(`截图失败：${String(e)}`)
    return
  }
  try {
    const path = await invoke<string>('save_screenshot', {
      path: status.value.saveDir,
      filename: `screenshot-${Date.now()}.png`,
      data: base64,
    })
    toast.success(`截图成功，已保存到：${path}`)
  } catch (e) {
    toast.error(`截图保存失败：${String(e)}`)
  }
}

function base64FromArrayBuffer(buf: ArrayBuffer): string {
  let binary = ''
  const bytes = new Uint8Array(buf)
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i])
  return btoa(binary)
}

async function toggleRecord() {
  const c = canvasRef.value
  if (!c) return
  if (recording.value) {
    recorder?.stop()
    return
  }
  try {
    const stream = (c as HTMLCanvasElement).captureStream(30)
    recorder = new MediaRecorder(stream, { mimeType: 'video/webm' })
    chunks = []
    recorder.ondataavailable = (e) => chunks.push(e.data)
    recorder.onstop = async () => {
      const blob = new Blob(chunks, { type: 'video/webm' })
      const buf = await blob.arrayBuffer()
      const base64 = base64FromArrayBuffer(buf)
      try {
        const path = await invoke<string>('save_recording', {
          path: status.value.saveDir,
          filename: `recording-${Date.now()}.webm`,
          data: base64,
        })
        toast.success(`录像已保存：${path}`)
      } catch (e) {
        toast.error(`保存录像失败：${String(e)}`)
      }
      recording.value = false
    }
    recorder.start()
    recording.value = true
    toast.info('开始录制…')
  } catch (e) {
    toast.error(`无法录制：${String(e)}`)
  }
}
</script>

<template>
  <div class="flex flex-col h-full gap-2">
    <!-- Video area fills all remaining vertical space; the black box hugs the
         video's real aspect ratio (portrait), so there is no big black block. -->
    <div class="flex-1 min-h-0 flex items-center justify-center">
      <div
        class="relative h-full rounded-lg border bg-black/90 overflow-hidden flex items-center justify-center"
        :style="{ aspectRatio, maxWidth: '100%' }"
      >
        <canvas
          ref="canvasRef"
          class="w-full h-full object-contain"
          :style="{ visibility: mirroring ? 'visible' : 'hidden' }"
        />
        <div
          v-if="!isRunning"
          class="absolute inset-0 flex items-center justify-center text-muted-foreground text-sm px-6 text-center"
        >
          点击「开始接收」后，iPhone 在「屏幕镜像」中选择本设备即可投屏。
        </div>
        <div
          v-else-if="!mirroring"
          class="absolute inset-0 flex flex-col items-center justify-center text-muted-foreground text-sm gap-2 px-6 text-center"
        >
          <p class="text-base text-yellow-400 font-medium">● 接收中</p>
          <p>已就绪，请在 iPhone 控制中心选择「{{ status.deviceName }}」…</p>
        </div>
      </div>
    </div>

    <div class="flex items-center gap-2 shrink-0">
      <Button size="sm" :disabled="!mirroring" @click="screenshot">
        截图 PNG
      </Button>
      <Button
        size="sm"
        :disabled="!mirroring && !recording"
        :variant="recording ? 'destructive' : 'outline'"
        @click="toggleRecord"
      >
        {{ recording ? '停止录制' : '录制 WebM' }}
      </Button>
      <span v-if="recording" class="text-xs text-red-500 animate-pulse">● 录制中</span>
    </div>
  </div>
</template>
