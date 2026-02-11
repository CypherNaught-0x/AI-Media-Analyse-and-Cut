<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { check } from '@tauri-apps/plugin-updater';
import { ask } from '@tauri-apps/plugin-dialog';
import { relaunch } from '@tauri-apps/plugin-process';
import Toast from './components/Toast.vue';

const toastVisible = ref(false);
const toastMessage = ref('');
const toastTone = ref<'info' | 'success' | 'error'>('info');
const toastProgress = ref<number | null>(null);
const toastActionLabel = ref('');
const downloadedBytes = ref(0);
const totalBytes = ref(0);
let pendingRelaunch = false;

function showToast(message: string, tone: 'info' | 'success' | 'error' = 'info') {
  toastMessage.value = message;
  toastTone.value = tone;
  toastVisible.value = true;
}

function hideToast() {
  toastVisible.value = false;
  toastActionLabel.value = '';
  toastProgress.value = null;
}

async function handleToastAction() {
  if (pendingRelaunch) {
    await relaunch();
  }
}

onMounted(async () => {
  try {
    const update = await check();
    if (update?.available) {
      const yes = await ask(
        `Update to ${update.version} is available!\n\nRelease notes:\n${update.body}`,
        { title: 'Update Available', kind: 'info', okLabel: 'Update', cancelLabel: 'Cancel' }
      );
      if (yes) {
        pendingRelaunch = false;
        toastActionLabel.value = '';
        showToast('Downloading update...', 'info');
        toastProgress.value = 0;
        downloadedBytes.value = 0;
        totalBytes.value = 0;
        try {
          await update.downloadAndInstall((event) => {
            switch (event.event) {
              case 'Started':
                toastMessage.value = 'Downloading update...';
                toastProgress.value = 0;
                downloadedBytes.value = 0;
                totalBytes.value = event.data.contentLength || 0;
                break;
              case 'Progress': {
                const chunkLength = event.data.chunkLength || 0;
                downloadedBytes.value += chunkLength;
                if (totalBytes.value > 0) {
                  const nextValue = Math.min(100, (downloadedBytes.value / totalBytes.value) * 100);
                  toastProgress.value = Math.max(toastProgress.value || 0, nextValue);
                } else {
                  toastProgress.value = null;
                }
                break;
              }
              case 'Finished':
                toastMessage.value = 'Installing update...';
                toastProgress.value = null;
                break;
            }
          });
        } catch (error) {
          console.error('Failed to download and install update:', error);
          showToast('Update failed to download or install.', 'error');
          return;
        }
        pendingRelaunch = true;
        toastActionLabel.value = 'Restart now';
        showToast('Update installed. Restart to apply changes.', 'success');
      }
    }
  } catch (error) {
    console.error('Failed to check for updates:', error);
    showToast('Update check failed. Try again in Settings.', 'error');
  }
});
</script>

<template>
  <router-view />
  <Toast
    :show="toastVisible"
    :message="toastMessage"
    :tone="toastTone"
    :progress="toastProgress"
    :action-label="toastActionLabel"
    @dismiss="hideToast"
    @action="handleToastAction"
  />
</template>

<style>
/* Global transitions */
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.5s ease, transform 0.5s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
  transform: translateY(20px);
}
</style>
