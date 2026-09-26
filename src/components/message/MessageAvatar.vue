<script setup lang="ts">
import { avatarInitial, avatarInitialLen, nameToColor } from "@/utils/color";
/** 消息头像：连续消息合并时由父组件渲染等宽占位，组件本身不感知合并规则。 */
defineProps<{
  name: string;
  avatar?: string | null;
}>();
</script>

<template>
  <div
    class="gosslan-avatar-box flex h-9 w-9 shrink-0 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
    :style="{ backgroundColor: nameToColor(name) }"
  >
    <img alt="" draggable="false" v-if="avatar" :src="avatar" class="h-full w-full object-cover" />
    <span
      v-else
      class="gosslan-avatar-initial text-sm font-medium" aria-hidden="true"
      :data-len="avatarInitialLen(name)"
      >{{ avatarInitial(name) }}</span
    >
  </div>
</template>
