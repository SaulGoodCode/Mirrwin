import { createApp } from 'vue'
import App from './App.vue'
import './assets/css/tailwind.css'
// vue-sonner v2 no longer auto-injects its styles — without this the toast
// notifications fire but render invisibly (no position/background).
import 'vue-sonner/style.css'

try {
  createApp(App).mount('#app')
} catch (err) {
  const app = document.getElementById('app')
  if (app) {
    app.innerHTML = `<pre style="padding:1rem;color:#b91c1c;background:#fef2f2;white-space:pre-wrap;font-family:monospace;">Mount error:\n${err instanceof Error ? err.stack || err.message : String(err)}</pre>`
  }
  // eslint-disable-next-line no-console
  console.error('Failed to mount Vue app:', err)
}
