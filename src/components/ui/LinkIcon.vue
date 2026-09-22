<script setup lang="ts">
/**
 * 连接类型图标的**唯一渲染处**（用户 2026-09-22：四种通道的图标各界面全局统一）。
 *
 * 图标名由 `utils/peerConnectionInfo.ts::linkIconName` 给出（与文案判据同源、同顺序），
 * 这里只负责把名字映射到 lucide 组件 —— 聊天头、好友列表、资料页都用它，不再各抄一份。
 *
 * ⚠️ 调用方**只应在有真实链路时**渲染本组件（`name !== "discovered"`）：发现了但没建链
 * 没有"链路类型"可言，画任意图标都是误导。`discovered` 落到下面的 `v-else` 只是兜底，
 * 正常不会被传进来。
 */
import { Bluetooth, Globe, Network, Router, Share2 } from "lucide-vue-next";
import type { LinkIconName } from "@/utils/peerConnectionInfo";

defineProps<{ name: LinkIconName }>();
</script>

<template>
  <!-- 经 N 跳 mesh 转发（桥接） -->
  <Share2 v-if="name === 'relay'" />
  <!-- 公网中转服务器的直连密封电路 -->
  <Globe v-else-if="name === 'relayServer'" />
  <Bluetooth v-else-if="name === 'bluetooth'" />
  <!-- 跨网段 / VPN 对端 IP 直达 -->
  <Network v-else-if="name === 'routed'" />
  <!-- 局域网直连：用「路由器」而不是 WiFi 扇形（用户 2026-09-17：WiFi 让人以为走无线上网） -->
  <Router v-else />
</template>
