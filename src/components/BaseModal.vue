<script setup lang="ts">
import { Dialog, DialogPanel, DialogTitle, TransitionChild, TransitionRoot } from "@headlessui/vue";

withDefaults(defineProps<{ open: boolean; title?: string; width?: string }>(), {
  width: "max-w-md",
});
const emit = defineEmits<{ (e: "close"): void }>();
</script>

<template>
  <TransitionRoot :show="open" as="template">
    <Dialog as="div" class="relative z-50" @close="emit('close')">
      <TransitionChild
        as="template"
        enter="duration-200 ease-out"
        enter-from="opacity-0"
        enter-to="opacity-100"
        leave="duration-150 ease-in"
        leave-from="opacity-100"
        leave-to="opacity-0"
      >
        <div class="fixed inset-0 bg-black/40" aria-hidden="true" />
      </TransitionChild>
      <div class="fixed inset-0 overflow-y-auto">
        <div class="flex min-h-full items-center justify-center p-4">
          <TransitionChild
            as="template"
            enter="duration-200 ease-out"
            enter-from="opacity-0 scale-95"
            enter-to="opacity-100 scale-100"
            leave="duration-150 ease-in"
            leave-from="opacity-100 scale-100"
            leave-to="opacity-0 scale-95"
          >
            <DialogPanel
              class="w-full rounded-2xl bg-[var(--gosslan-panel)] p-5 shadow-xl text-left align-middle text-[var(--gosslan-text)]"
              :class="width"
            >
              <DialogTitle v-if="title" as="h3" class="text-base font-semibold mb-4">
                {{ title }}
              </DialogTitle>
              <slot />
            </DialogPanel>
          </TransitionChild>
        </div>
      </div>
    </Dialog>
  </TransitionRoot>
</template>
