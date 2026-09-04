<script setup lang="ts">
import { onMounted } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import ResponsiveLayout from "@/layouts/ResponsiveLayout.vue";

const app = useAppStore();
const chat = useChatStore();

onMounted(async () => {
  await app.init();
  await chat.init();
});
</script>

<template>
  <ResponsiveLayout />
</template>
