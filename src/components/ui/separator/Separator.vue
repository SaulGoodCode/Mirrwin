<script setup lang="ts">
import { Separator } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/utils'

const props = withDefaults(
  defineProps<{
    class?: HTMLAttributes['class']
    orientation?: 'horizontal' | 'vertical'
    decorative?: boolean
  }>(),
  { orientation: 'horizontal', decorative: true },
)
</script>

<template>
  <Separator
    :class="cn(
      'shrink-0 bg-border',
      props.orientation === 'vertical' ? 'h-full w-px' : 'h-px w-full',
      props.class,
    )"
    :decorative="props.decorative"
    :orientation="props.orientation"
  />
</template>
