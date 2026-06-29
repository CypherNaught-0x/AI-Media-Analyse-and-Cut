<script setup lang="ts">
interface WorkspaceTab {
  id: string;
  label: string;
  disabled?: boolean;
}

defineProps<{
  tabs: WorkspaceTab[];
  activeTab: string;
}>();

const emit = defineEmits<{
  (e: 'update:activeTab', value: string): void;
}>();

function selectTab(tab: WorkspaceTab) {
  if (tab.disabled) return;
  emit('update:activeTab', tab.id);
}
</script>

<template>
  <div
    role="tablist"
    class="mb-8 flex flex-wrap gap-2 rounded-2xl border border-white/10 bg-white/5 p-1.5 backdrop-blur-md shadow-2xl"
  >
    <button
      v-for="tab in tabs"
      :key="tab.id"
      type="button"
      role="tab"
      :data-testid="`workspace-tab-${tab.id}`"
      :disabled="tab.disabled"
      :aria-selected="activeTab === tab.id"
      :aria-disabled="tab.disabled ? 'true' : 'false'"
      class="relative min-w-[8rem] flex-1 rounded-xl px-4 py-2.5 text-sm font-semibold outline-none transition-all focus-visible:ring-2 focus-visible:ring-blue-500/50"
      :class="activeTab === tab.id
        ? 'border border-blue-500/40 bg-blue-600/30 text-white shadow-lg'
        : tab.disabled
          ? 'cursor-not-allowed border border-transparent text-gray-600'
          : 'border border-transparent text-gray-300 hover:bg-white/10 hover:text-white'"
      @click="selectTab(tab)"
    >
      {{ tab.label }}
    </button>
  </div>
</template>
