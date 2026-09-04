<script setup lang="ts">
import { onMounted } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import ResponsiveLayout from "@/layouts/ResponsiveLayout.vue";

const app = useAppStore();
const chat = useChatStore();

onMounted(async () => {
  // 屏蔽 WebView 默认右键菜单（返回 / 刷新 / 另存为等），改为应用自定义交互：
  // 有功能的元素自行绑定右键菜单（见 MessageItem 的复制菜单），无功能的区域右键无效果。
  window.addEventListener("contextmenu", (e) => e.preventDefault());
  await app.init();
  await chat.init();
});
</script>

<template>
  <ResponsiveLayout />
</template>
