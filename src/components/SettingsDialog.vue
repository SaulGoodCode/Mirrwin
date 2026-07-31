<script setup lang="ts">
import { ref, watch } from 'vue'
import { Dialog } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Separator } from '@/components/ui/separator'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { invoke } from '@tauri-apps/api/core'
import { toast } from 'vue-sonner'
import { useReceiver } from '@/composables/useReceiver'

const props = defineProps<{ open: boolean }>()
const emit = defineEmits<{
  'update:open': [v: boolean]
  save: [opts: { deviceName: string; port: number; saveDir: string; width: number; height: number; fps: number }]
}>()

const { status } = useReceiver()

const deviceName = ref(status.value.deviceName)
const port = ref(status.value.port)
const saveDir = ref(status.value.saveDir)
const width = ref(0)
const height = ref(0)
const fps = ref(0)

watch(
  () => status.value,
  (s) => {
    deviceName.value = s.deviceName
    port.value = s.port
    saveDir.value = s.saveDir
  },
  { deep: true },
)

async function pickDir() {
  const sel = await openDialog({ directory: true, multiple: false })
  if (typeof sel === 'string') saveDir.value = sel
}

async function openDir() {
  if (!saveDir.value) return
  try {
    await invoke('open_path', { path: saveDir.value })
  } catch (e) {
    toast.error(`无法打开目录：${String(e)}`)
  }
}

function onSave() {
  emit('save', {
    deviceName: deviceName.value,
    port: Number(port.value) || 7000,
    saveDir: saveDir.value,
    width: Number(width.value) || 0,
    height: Number(height.value) || 0,
    fps: Number(fps.value) || 0,
  })
  emit('update:open', false)
}
</script>

<template>
  <Dialog
    :open="open"
    @update:open="(v) => emit('update:open', v)"
    title="设置"
    description="配置 AirPlay 接收服务"
  >
    <div class="space-y-4 py-2">
      <div class="space-y-2">
        <Label for="deviceName">设备名称（iPhone 上显示）</Label>
        <Input id="deviceName" v-model="deviceName" placeholder="AirPlay Mirror" />
      </div>
      <div class="space-y-2">
        <Label for="port">监听端口</Label>
        <Input id="port" v-model="port" type="number" />
      </div>
      <Separator />
      <div class="space-y-2">
        <Label>分辨率 / 帧率（0 = 由协议库自动决定）</Label>
        <div class="grid grid-cols-3 gap-2">
          <Input id="width" v-model="width" type="number" placeholder="宽 0" />
          <Input id="height" v-model="height" type="number" placeholder="高 0" />
          <Input id="fps" v-model="fps" type="number" placeholder="fps 0" />
        </div>
        <p class="text-xs text-muted-foreground">
          设置较小的分辨率可显著降低 CPU 占用。
        </p>
      </div>
      <Separator />
      <div class="space-y-2">
        <Label>截图 / 录制保存目录</Label>
        <div class="flex gap-2">
          <Input :model-value="saveDir" readonly placeholder="未选择" />
          <Button variant="outline" size="sm" @click="pickDir">选择</Button>
          <Button variant="outline" size="sm" :disabled="!saveDir" @click="openDir">打开</Button>
        </div>
      </div>
    </div>
    <div class="flex justify-end gap-2 pt-2">
      <Button variant="ghost" @click="emit('update:open', false)">取消</Button>
      <Button @click="onSave">保存</Button>
    </div>
  </Dialog>
</template>
