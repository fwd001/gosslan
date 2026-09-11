<script setup lang="ts">
import { t } from "@/i18n";
import { Copy, CornerUpLeft, Save, Share2 } from "lucide-vue-next";
import type { MsgKind } from "@/types";
import ContextMenu from "@/components/ContextMenu.vue";

defineProps<{
  x: number;
  y: number;
  kind: MsgKind;
}>();
const emit = defineEmits<{
  (e: "close"): void;
  (e: "copy-text"): void;
  (e: "copy-image"): void;
  (e: "save-image"): void;
  (e: "save-file"): void;
  (e: "copy-file"): void;
  (e: "quote"): void;
  (e: "forward"): void;
}>();

/** 转发支持：文本 / 代码 / 图片 / 文件（文件按本地路径重走传输链路；系统消息不提供）。 */
const forwardable = (k: MsgKind) => k === "text" || k === "code" || k === "image" || k === "file";

// 定位 / 点外部关闭 / Esc 全部交给统一外壳 `ContextMenu`（#4 全局统一样式）。
</script>

<template>
  <!-- 聊天气泡右键菜单（用户 2026-09-12 晚 #11：「聊天气泡的右键菜单也参考微信样式」）。
       外观与分组统一走 `.gosslan-menu*`：先「内容操作」（复制 / 保存），
       再分隔线，后「转发 / 引用」—— 与微信把"内容操作"和"消息流转"分组的习惯一致。
       本应用没有 翻译 / 搜一搜 / 收藏 / 多选 / 提醒 这些能力，就不放空条目。 -->
  <ContextMenu :x="x" :y="y" :estimated-height="260" @close="emit('close')">
    <template v-if="kind === 'text' || kind === 'code'">
      <button role="menuitem" class="gosslan-menu-item" @click="emit('copy-text')">
        <Copy />
        {{ t("common.copy") }}
      </button>
    </template>
    <template v-if="kind === 'image'">
      <button role="menuitem" class="gosslan-menu-item" @click="emit('copy-image')">
        <Copy />
        {{ t("common.copyImage") }}
      </button>
      <button role="menuitem" class="gosslan-menu-item" @click="emit('save-image')">
        <Save />
        {{ t("common.saveImage") }}
      </button>
    </template>
    <!-- 文件：保存（另存为）+ 复制（文件本体写 CF_HDROP，可在资源管理器/聊天框直接粘贴） -->
    <template v-if="kind === 'file'">
      <button role="menuitem" class="gosslan-menu-item" @click="emit('save-file')">
        <Save />
        {{ t("common.save") }}
      </button>
      <button role="menuitem" class="gosslan-menu-item" @click="emit('copy-file')">
        <Copy />
        {{ t("common.copyFile") }}
      </button>
    </template>

    <div class="gosslan-menu-sep" role="separator"></div>

    <button role="menuitem" class="gosslan-menu-item" @click="emit('quote')">
      <CornerUpLeft />
      {{ t("common.quote") }}
    </button>
    <button v-if="forwardable(kind)" class="gosslan-menu-item" @click="emit('forward')">
      <Share2 />
      {{ t("common.forward") }}
    </button>
  </ContextMenu>
</template>
