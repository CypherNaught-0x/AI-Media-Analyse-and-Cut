<script setup lang="ts">
// Modal for analysis failures: what went wrong, the AI's raw reply when there
// is one, and ways to report it (export logs, copy the details).
import { computed, ref } from 'vue';
import { save } from '@tauri-apps/plugin-dialog';
import { Check, Copy, Download, TriangleAlert } from '@lucide/vue';
import UiDialog from './ui/UiDialog.vue';
import { useSettings } from '../composables/useSettings';
import { commands } from '../bindings';
import { errorMessage } from '../utils/appError';

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

/** Only failures that came with an AI reply are "AI response" errors. */
const isAiError = computed(() => !!props.rawResponse || !!props.parseError);
const title = computed(() => (isAiError.value ? 'AI response error' : 'Something went wrong'));

const rawCopied = ref(false);
let rawCopiedTimer: ReturnType<typeof setTimeout> | null = null;

async function copyRawResponse() {
    try {
        await navigator.clipboard.writeText(props.rawResponse);
        rawCopied.value = true;
        if (rawCopiedTimer) clearTimeout(rawCopiedTimer);
        rawCopiedTimer = setTimeout(() => (rawCopied.value = false), 1500);
        emit('update:status', 'Raw AI response copied to clipboard.');
    } catch (e) {
        console.error('Failed to copy:', e);
    }
}

async function copyErrorDetails() {
    const details = `Error: ${props.message}

Parse Error: ${props.parseError || 'N/A'}

Raw AI Response:
${props.rawResponse}

---
Timestamp: ${new Date().toISOString()}
Model: ${settings.value.model}
Base URL: ${settings.value.baseUrl}`;

    try {
        await navigator.clipboard.writeText(details);
        emit('update:status', 'Error details copied to clipboard.');
    } catch (e) {
        console.error('Failed to copy:', e);
    }
}

async function exportLogs() {
    try {
        const path = await save({
            filters: [
                {
                    name: 'Zip Files',
                    extensions: ['zip'],
                },
            ],
            defaultPath: 'ai-media-cutter-logs.zip',
        });

        if (!path) return;

        await commands.zipLogs(path);
        emit('update:status', 'Logs exported successfully.');
    } catch (e) {
        console.error('Failed to export logs:', e);
        emit('update:status', `Failed to export logs: ${errorMessage(e)}`);
    }
}
</script>

<template>
    <UiDialog
        :open="show"
        :title="title"
        :description="message"
        size="lg"
        tone="danger"
        @close="emit('dismiss')"
    >
        <template #icon>
            <span
                class="flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-danger/15 text-danger"
            >
                <TriangleAlert class="h-4 w-4" aria-hidden="true" />
            </span>
        </template>

        <div class="space-y-4">
            <div v-if="parseError">
                <p class="field-label">Parse error</p>
                <code
                    class="well block px-3 py-2 font-mono text-[12px] break-words whitespace-pre-wrap text-danger"
                    >{{ parseError }}</code
                >
            </div>

            <div v-if="rawResponse">
                <div class="mb-1.5 flex items-center justify-between gap-2">
                    <p class="field-label mb-0">Raw AI response</p>
                    <button
                        type="button"
                        class="btn-ghost btn-sm"
                        data-testid="copy-raw-response"
                        @click="copyRawResponse"
                    >
                        <Check v-if="rawCopied" aria-hidden="true" />
                        <Copy v-else aria-hidden="true" />
                        {{ rawCopied ? 'Copied' : 'Copy' }}
                    </button>
                </div>
                <pre
                    class="well max-h-[40vh] overflow-auto p-3 font-mono text-[11.5px] leading-relaxed break-words whitespace-pre-wrap text-fg-muted"
                    >{{ rawResponse }}</pre>
            </div>

            <p class="hint">Export the logs and copy these details when you report the issue.</p>
        </div>

        <template #footer>
            <div class="mr-auto flex gap-2">
                <button type="button" class="btn-secondary" @click="exportLogs">
                    <Download aria-hidden="true" />
                    Export logs
                </button>
                <button type="button" class="btn-secondary" @click="copyErrorDetails">
                    <Copy aria-hidden="true" />
                    Copy all
                </button>
            </div>
            <button type="button" class="btn-primary" data-autofocus @click="emit('dismiss')">
                Dismiss
            </button>
        </template>
    </UiDialog>
</template>
