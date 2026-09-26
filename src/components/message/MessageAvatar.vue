<script setup lang="ts">
import { computed } from "vue";
import { avatarSeedFor } from "@/utils/avatarSeed";

/** 消息头像：连续消息合并时由父组件渲染等宽占位，组件本身不感知合并规则。 */
const props = defineProps<{
  /**
   * 种子用**设备 id / 群 id**，不是昵称：昵称可改（改一次换一张脸）、也能撞名
   * （两个人同昵称就同一张脸），而这张脸的要求是"稳定 + 不撞脸"。
   * 缺 id 时才退回昵称兜底 —— 调用点漏传由 designGuards 的 `:id` 判据拦。
   */
  id?: string | null;
  name: string;
  avatar?: string | null;
}>();

const seed = computed(() => avatarSeedFor(props.id || props.name));
</script>

<template>
  <div
    class="gosslan-avatar-box flex h-9 w-9 shrink-0 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)]"
    :style="{ backgroundColor: seed?.bg }"
  >
    <img alt="" draggable="false" v-if="avatar" :src="avatar" class="h-full w-full object-cover" />
    <span v-else class="gosslan-avatar-emoji" aria-hidden="true">{{ seed?.emoji }}</span>
  </div>
</template>
