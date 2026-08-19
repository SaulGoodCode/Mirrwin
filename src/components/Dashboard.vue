<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { toast } from 'vue-sonner'
import { Button } from '@/components/ui/button'
import { Settings as SettingsIcon, Radio, Square, MonitorSmartphone, Music } from 'lucide-vue-next'
import { useReceiver } from '@/composables/useReceiver'
import MirrorCanvas from '@/components/MirrorCanvas.vue'
import AudioVisualizer from '@/components/AudioVisualizer.vue'
import StatusBar from '@/components/StatusBar.vue'
import SettingsDialog from '@/components/SettingsDialog.vue'

const { status, start, stop, refresh, saveSettings, ensureListeners, viewMode } = useReceiver()
const settingsOpen = ref(false)
const starting = ref(false)
// "开始接收" waits until the backend reports the native library loaded, rather
// than on a fixed delay. Loading it used to wedge the process if done while the
// app was still coming up — that was the bundled Cygwin FFmpeg DLLs, which this
// build no longer has — and the backend now loads it at startup in a few
// milliseconds, so this is normally true before the window is even painted.
const ready = computed(() => status.value.libReady)

onMounted(async () => {
  await ensureListeners()
  await refresh()
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
  if (wasRunning) await stop()
  // Persist first: this is the only path that reaches the backend when the
  // receiver is stopped, and it is what makes settings survive a restart.
  try {
    await saveSettings(opts)
  } catch (e) {
    toast.error(`保存设置失败：${String(e)}`)
    return
  }
  if (wasRunning) {
    try {
      await start(opts)
    } catch (e) {
      toast.error(`启动失败：${String(e)}`)
    }
  }
}
</script>

<template>
  <div class="mx-auto max-w-3xl w-full h-screen flex flex-col px-4 py-3 gap-3 overflow-hidden">
    <div class="flex items-center justify-between shrink-0">
      <div class="flex items-center gap-3">
        <h1 class="text-xl font-bold tracking-tight">AirPlay Mirror</h1>
        <!-- Which pane to show. Purely a view choice: switching does not touch
             the receiver, so a running session keeps running either way. -->
        <div class="flex items-center rounded-md border p-0.5" role="group" aria-label="显示模式">
          <button
            type="button"
            class="flex items-center justify-center h-7 w-8 rounded transition-colors"
            :class="viewMode === 'mirror' ? 'bg-primary text-primary-foreground' : 'text-muted-foreground hover:bg-muted'"
            :aria-pressed="viewMode === 'mirror'"
            title="投屏画面"
            @click="viewMode = 'mirror'"
          >
            <MonitorSmartphone class="h-4 w-4" />
          </button>
          <button
            type="button"
            class="flex items-center justify-center h-7 w-8 rounded transition-colors"
            :class="viewMode === 'audio' ? 'bg-primary text-primary-foreground' : 'text-muted-foreground hover:bg-muted'"
            :aria-pressed="viewMode === 'audio'"
            title="音频播放"
            @click="viewMode = 'audio'"
          >
            <Music class="h-4 w-4" />
          </button>
        </div>
      </div>
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
      <div v-show="viewMode === 'mirror'" class="h-full">
        <MirrorCanvas />
      </div>
      <AudioVisualizer v-if="viewMode === 'audio'" />
    </div>

    <SettingsDialog
      :open="settingsOpen"
      @update:open="settingsOpen = $event"
      @save="onSaveSettings"
    />
  </div>
</template>
