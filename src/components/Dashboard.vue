<script setup lang="ts">
import { onMounted, ref } from 'vue'
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

onMounted(refresh)

async function onStart() {
  if (starting.value) return
  starting.value = true
  try {
    await start({
      deviceName: status.value.deviceName,
      port: status.value.port,
      saveDir: status.value.saveDir,
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

async function onSaveSettings(opts: { deviceName: string; port: number; saveDir: string; width: number; height: number; fps: number }) {
  const wasRunning = status.value.running
  if (wasRunning) stop()
  status.value.deviceName = opts.deviceName
  status.value.port = opts.port
  status.value.saveDir = opts.saveDir
  if (wasRunning) {
    try {
      await start({ deviceName: opts.deviceName, port: opts.port, saveDir: opts.saveDir, width: opts.width, height: opts.height, fps: opts.fps })
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
        <Button v-if="!status.running" size="sm" :disabled="starting" @click="onStart">
          <Radio class="h-4 w-4" /> {{ starting ? '启动中…' : '开始接收' }}
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
