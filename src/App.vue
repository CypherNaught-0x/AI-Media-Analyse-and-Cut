<script setup lang="ts">
import { onMounted } from 'vue';
import { check } from '@tauri-apps/plugin-updater';
import { ask } from '@tauri-apps/plugin-dialog';
import { relaunch } from '@tauri-apps/plugin-process';

onMounted(async () => {
  try {
    const update = await check();
    if (update?.available) {
      const yes = await ask(
        `Update to ${update.version} is available!\n\nRelease notes: ${update.body}`, 
        { title: 'Update Available', kind: 'info', okLabel: 'Update', cancelLabel: 'Cancel' }
      );
      if (yes) {
        await update.downloadAndInstall();
        await relaunch();
      }
    }
  } catch (error) {
    console.error('Failed to check for updates:', error);
  }
});
</script>

<template>
  <router-view />
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