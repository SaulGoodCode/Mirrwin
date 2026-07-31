<script setup lang="ts">
import Dashboard from '@/components/Dashboard.vue'
import { Toaster } from '@/components/ui/sonner'
</script>

<template>
  <div class="h-screen overflow-hidden bg-background text-foreground antialiased">
    <Dashboard />
    <Toaster richColors position="top-right" :duration="3500" />
  </div>
</template>
