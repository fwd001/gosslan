<script setup lang="ts">
/**
 * 联系人（通讯录）右键菜单 —— 微信式（用户需求 2026-09-12 晚 #12：
 * 「联系人的右键菜单也可以参考微信，然后有『发起聊天』和『删除』。这两个功能图标你添加上。」）。
 *
 * 微信 macOS 的等价菜单是「发消息 / 标为星标朋友 / 删除联系人」；本应用当前没有星标概念，
 * 所以只做用户点名的两项：**发起聊天**（打开与他的会话）与**删除好友**。
 * 外观走 `style.css` 的 `.gosslan-menu*` 统一类（#4），不再自带一份样式。
 */
import { t } from "@/i18n";
import { MessageSquare, UserMinus } from "lucide-vue-next";
import ContextMenu from "@/components/ContextMenu.vue";

defineProps<{ x: number; y: number }>();
const emit = defineEmits<{
  (e: "close"): void;
  /** 发起聊天：打开与该好友的会话（父组件负责切换视图）。 */
  (e: "chat"): void;
  /** 删除好友（危险操作，父组件走二次确认）。 */
  (e: "confirm"): void;
}>();
</script>

<template>
  <ContextMenu :x="x" :y="y" :estimated-height="130" @close="emit('close')">
    <button role="menuitem" class="gosslan-menu-item" @click="emit('chat')">
      <MessageSquare />
      {{ t("conv.startChat") }}
    </button>
    <div class="gosslan-menu-sep" role="separator"></div>
    <button role="menuitem" class="gosslan-menu-item gosslan-menu-item--danger" @click="emit('confirm')">
      <UserMinus />
      {{ t("common.deleteFriend") }}
    </button>
    <div class="gosslan-menu-hint">{{ t("friend.delete.keepHistory") }}</div>
  </ContextMenu>
</template>
