<script setup lang="ts">
// Session card for the Source inspector: save / load the whole project as a
// session file. The workspace also autosaves in the app on its own.
import { FolderOpen, Save } from '@lucide/vue';

defineProps<{
    inputPath: string;
    hasMediaFile: boolean;
}>();

defineEmits<{
    (e: 'save-session'): void;
    (e: 'load-session'): void;
}>();
</script>

<template>
    <section class="panel p-4" aria-labelledby="session-card-title">
        <h2 id="session-card-title" class="section-title">Session</h2>
        <p class="hint mt-1">
            <template v-if="inputPath">
                Your work is autosaved in the app. Save a session file to back it up or move it to
                another machine.
            </template>
            <template v-else>
                Pick up where you left off by loading a saved session file.
            </template>
        </p>
        <div class="mt-3 grid grid-cols-2 gap-2">
            <button type="button" class="btn-secondary btn-sm" @click="$emit('save-session')">
                <Save aria-hidden="true" />
                Save session
            </button>
            <button type="button" class="btn-secondary btn-sm" @click="$emit('load-session')">
                <FolderOpen aria-hidden="true" />
                Load session
            </button>
        </div>
    </section>
</template>
