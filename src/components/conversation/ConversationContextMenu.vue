<script setup lang="ts">
/**
 * 会话行右键菜单（用户 2026-09-12 晚 #3 / #10：
 * 「取消叉叉，改为统一的右键删除操作。即每一个选项都做成单击右键弹出菜单，再点击『删除』」
 * 「消息列表的删除的右单击右键删除的这个弹出菜单样式参考微信」）。
 *
 * 微信 macOS 的会话菜单含置顶/免打扰/独立窗口等（本应用暂无这些能力），
 * 因此只做真实存在的一项：**删除聊天记录**（危险项放最后并标红，与微信一致），
 * 下方给一句说明（只删本机、不影响对方），避免误删恐慌。
 * 外观走 `style.css` 的 `.gosslan-menu*` 统一类（#4）。
 */
import { t } from "@/i18n";
import { Trash2 } from "lucide-vue-next";
import ContextMenu from "@/components/ContextMenu.vue";

defineProps<{ x: number; y: number }>();
const emit = defineEmits<{
  (e: "close"): void;
  (e: "delete"): void;
}>();
</script>

<template>
  <ContextMenu :x="x" :y="y" :estimated-height="90" @close="emit('close')">
    <button role="menuitem" class="gosslan-menu-item gosslan-menu-item--danger" @click="emit('delete')">
      <Trash2 />
      {{ t("conv.delete") }}
    </button>
    <div class="gosslan-menu-hint">{{ t("conv.delete.hint") }}</div>
  </ContextMenu>
</template>
