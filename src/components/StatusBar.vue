<script setup lang="ts">
import { Badge } from '@/components/ui/badge'
import { useReceiver } from '@/composables/useReceiver'

const { status, mirroring } = useReceiver()
</script>

<template>
  <div class="flex flex-wrap items-center gap-2 text-sm">
    <Badge :variant="mirroring ? 'success' : status.running ? 'default' : 'secondary'">
      {{ mirroring ? '投屏中' : status.running ? '接收中' : '已停止' }}
    </Badge>
    <span class="text-muted-foreground">设备名：{{ status.deviceName }}</span>
    <span class="text-muted-foreground">端口：{{ status.port }}</span>
    <span v-if="status.deviceId" class="text-muted-foreground">ID：{{ status.deviceId }}</span>
  </div>
</template>
