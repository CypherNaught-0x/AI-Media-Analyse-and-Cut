<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import { useSettings } from '../composables/useSettings';

const { settings } = useSettings();

interface Props {
  show: boolean;
  message: string;
  rawResponse: string;
  parseError?: string;
}

const props = defineProps<Props>();

const emit = defineEmits<{
  dismiss: [];
  'update:status': [message: string];
}>();

async function copyErrorDetails() {
  const details = `Error: ${props.message}

Parse Error: ${props.parseError || "N/A"}

Raw AI Response:
${props.rawResponse}

---
Timestamp: ${new Date().toISOString()}
Model: ${settings.value.model}
Base URL: ${settings.value.baseUrl}`;

  try {
    await navigator.clipboard.writeText(details);
    emit('update:status', "Error details copied to clipboard.");
  } catch (e) {
    console.error("Failed to copy:", e);
  }
}

async function exportLogs() {
  try {
    const path = await save({
      filters: [{
        name: 'Zip Files',
        extensions: ['zip']
      }],
      defaultPath: 'ai-media-cutter-logs.zip'
    });

    if (!path) return;

    await invoke('zip_logs', { targetPath: path });
    emit('update:status', 'Logs exported successfully.');
  } catch (e) {
    console.error("Failed to export logs:", e);
    emit('update:status', `Failed to export logs: ${e}`);
  }
}
</script>

<template>
  <transition name="fade">
    <div v-if="show" class="fixed inset-0 z-[100] flex items-center justify-center p-4 bg-black/80 backdrop-blur-sm">
      <div class="w-full max-w-3xl max-h-[80vh] bg-gray-900 rounded-2xl border border-red-500/30 shadow-2xl shadow-red-500/10 flex flex-col overflow-hidden">
        <!-- Header -->
        <div class="flex items-center justify-between p-6 border-b border-white/10 bg-red-500/10">
          <div class="flex items-center gap-3">
            <div class="w-10 h-10 rounded-full bg-red-500/20 flex items-center justify-center">
              <svg class="w-6 h-6 text-red-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
              </svg>
            </div>
            <div>
              <h2 class="text-xl font-bold text-red-400">AI Response Error</h2>
              <p class="text-sm text-gray-400">{{ message }}</p>
            </div>
          </div>
          <button @click="emit('dismiss')" class="p-2 hover:bg-white/10 rounded-lg transition-colors focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none">
            <svg class="w-5 h-5 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        <!-- Content -->
        <div class="flex-1 overflow-y-auto p-6 space-y-4">
          <!-- Parse Error -->
          <div v-if="parseError" class="bg-black/30 rounded-xl p-4 border border-white/5">
            <h3 class="text-sm font-semibold text-gray-300 mb-2 uppercase tracking-wider">Parse Error</h3>
            <code class="text-sm text-red-300 font-mono">{{ parseError }}</code>
          </div>

          <!-- Raw Response -->
          <div class="bg-black/30 rounded-xl p-4 border border-white/5">
            <h3 class="text-sm font-semibold text-gray-300 mb-2 uppercase tracking-wider">Raw AI Response</h3>
            <pre class="text-xs text-gray-400 font-mono whitespace-pre-wrap break-words max-h-[40vh] overflow-y-auto bg-black/50 rounded-lg p-4">{{ rawResponse }}</pre>
          </div>
        </div>

        <!-- Footer -->
        <div class="p-6 border-t border-white/10 bg-black/30 flex items-center justify-between gap-4">
          <p class="text-xs text-gray-500">Export logs and copy this information when reporting issues.</p>
          <div class="flex gap-3">
            <button @click="emit('dismiss')"
              class="px-6 py-2 bg-white/10 hover:bg-white/20 text-gray-300 font-medium rounded-xl transition-all border border-white/10 focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none">
              Dismiss
            </button>
            <button @click="exportLogs"
              class="px-6 py-2 bg-white/10 hover:bg-white/20 text-gray-300 font-medium rounded-xl transition-all border border-white/10 flex items-center gap-2 focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none">
              <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a2 2 0 002 2h12a2 2 0 002-2v-1M7 10l5 5m0 0l5-5m-5 5V3" />
              </svg>
              Export Logs
            </button>
            <button @click="copyErrorDetails"
              class="px-6 py-2 bg-red-600 hover:bg-red-500 text-white font-medium rounded-xl transition-all flex items-center gap-2 focus-visible:ring-2 focus-visible:ring-red-500/50 focus-visible:outline-none">
              <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 5H6a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2v-1M8 5a2 2 0 002 2h2a2 2 0 002-2M8 5a2 2 0 012-2h2a2 2 0 012 2m0 0h2a2 2 0 012 2v3m2 4H10m0 0l3-3m-3 3l3 3" />
              </svg>
              Copy All
            </button>
          </div>
        </div>
      </div>
    </div>
  </transition>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.3s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
