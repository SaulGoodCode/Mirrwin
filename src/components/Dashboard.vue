<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { toast } from 'vue-sonner'
import { Button } from '@/components/ui/button'
import { Settings as SettingsIcon, Radio, Square } from 'lucide-vue-next'
import { useReceiver } from '@/composables/useReceiver'
import MirrorCanvas from '@/components/MirrorCanvas.vue'
import StatusBar from '@/components/StatusBar.vue'
import SettingsDialog from '@/components/SettingsDialog.vue'

const { status, start, stop, refresh } = useReceiver()
const settingsOpen = ref(false)
const starting = ref(false)
// Startup readiness gate. Loading the native protocol DLL during the first
// couple of seconds after launch (while the app/WebView2 is still initializing)
// crashes the process; the same load is safe once things settle. So we block
// "开始接收" briefly on startup — this automates the "wait a few seconds"
// workaround. Tune READY_DELAY_MS if a slow machine still crashes on first click.
const ready = ref(false)
const READY_DELAY_MS = 3500
let readyTimer: number | null = null

onMounted(() => {
  refresh()
  readyTimer = window.setTimeout(() => {
    ready.value = true
  }, READY_DELAY_MS)
})

onUnmounted(() => {
  if (readyTimer != null) window.clearTimeout(readyTimer)
})

async function onStart() {
  if (starting.value || !ready.value) return
  starting.value = true
  try {
    await start({
      deviceName: status.value.deviceName,
      port: status.value.port,
      saveDir: status.value.saveDir,
      enableAudio: status.value.enableAudio,
    })
  } catch (e) {
    toast.error(`启动失败：${String(e)}`)
  } finally {
    starting.value = false
  }
}

async function onStop() {
  await stop()
}

async function onSaveSettings(opts: { deviceName: string; port: number; saveDir: string; width: number; height: number; fps: number; enableAudio: boolean }) {
  const wasRunning = status.value.running
  if (wasRunning) stop()
  status.value.deviceName = opts.deviceName
  status.value.port = opts.port
  status.value.saveDir = opts.saveDir
  status.value.enableAudio = opts.enableAudio
  if (wasRunning) {
    try {
      await start({ deviceName: opts.deviceName, port: opts.port, saveDir: opts.saveDir, width: opts.width, height: opts.height, fps: opts.fps, enableAudio: opts.enableAudio })
    } catch (e) {
      toast.error(`启动失败：${String(e)}`)
    }
  }
}
</script>

<template>
  <div class="mx-auto max-w-3xl w-full h-screen flex flex-col px-4 py-3 gap-3 overflow-hidden">
    <div class="flex items-center justify-between shrink-0">
      <h1 class="text-xl font-bold tracking-tight">AirPlay Mirror</h1>
      <div class="flex items-center gap-2">
        <Button v-if="!status.running" size="sm" :disabled="starting || !ready" @click="onStart">
          <Radio class="h-4 w-4" /> {{ !ready ? '初始化中…' : starting ? '启动中…' : '开始接收' }}
        </Button>
        <Button v-else size="sm" variant="destructive" @click="onStop">
          <Square class="h-4 w-4" /> 停止
        </Button>
        <Button variant="outline" size="icon" @click="settingsOpen = true">
          <SettingsIcon class="h-4 w-4" />
        </Button>
      </div>
    </div>

    <div class="shrink-0">
      <StatusBar />
    </div>

    <div class="flex-1 min-h-0">
      <MirrorCanvas />
    </div>

    <SettingsDialog
      :open="settingsOpen"
      @update:open="settingsOpen = $event"
      @save="onSaveSettings"
    />
  </div>
</template>
