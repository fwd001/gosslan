<script setup lang="ts">
/**
 * 头部返回箭头（按平台适配）—— 应用内所有「返回 / 返回上一级」箭头的唯一实现。
 *
 * 背景（用户 2026-09-20）：「每个独立转场的页面……头部标题的返回箭头也不一样，
 * 箭头统一用安卓风格，或者按平台适配也行，全部排查一下。」
 * 此前有五种画法各写各的：MobilePageFrame / BaseModal 用细线 chevron，
 * ChatHeader / FriendProfile 用 lucide `ArrowLeft`，收藏详情还自绘了 stroke-width=2
 * 的圆角 chevron —— 同一件事三四种造型。
 *
 * ⇒ 按平台给原生观感（用户 2026-09-20 定稿）：
 *   · iOS：细线 chevron（`<`），即 iOS 的导航返回造型；
 *   · 其余平台（Android / 桌面）：Material 的左向箭头（`←`，带尾）。
 *
 * 只画箭头本身，不带按钮：热区/圆角/间距由各头部自己给（它们略有差异，
 * 但箭头造型从此只有这一份）。
 */
import { ArrowLeft } from "lucide-vue-next";
import { isIOS } from "@/utils/platform";
</script>

<template>
  <!-- iOS 用 chevron；其余平台（含桌面）用 Material 左箭头。 -->
  <svg
    v-if="isIOS"
    viewBox="0 0 24 24"
    class="h-5 w-5"
    fill="none"
    stroke="currentColor"
    stroke-width="1.9"
  >
    <path d="m15 18-6-6 6-6" />
  </svg>
  <ArrowLeft v-else class="h-5 w-5" />
</template>
