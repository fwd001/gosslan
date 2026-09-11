<script setup lang="ts">
/**
 * 未读数量徽标（红底胶囊 + 数字）。**全应用唯一实现**。
 *
 * ## 为什么数字要加一点「光学补偿」（`pb-[1.5px]`）
 *
 * 圆的直径 16px（`h-4`），数字 11px。`items-center` 居中的是**行盒**，
 * 而行盒里的字形并不在行盒正中：字体的 ascent/descent 不对称，数字又没有下伸部，
 * 于是字形整体偏下。实测（2x 截图逐像素测量，导航栏与会话列表三处徽标结果一致）：
 *
 *   圆 32 设备像素 · 数字墨迹高 15–16 · **上间隙 10 / 下间隙 7**
 *   ⇒ 字形偏下 1.5 设备像素 = **0.75 CSS px**
 *
 * 用**布局补偿**而不是 `translateY`：给固定高度的盒子加 `padding-bottom: 1.5px`，
 * 会把 flex 居中的行盒上移 0.75px（`(16 − 1.5 − 11) / 2 = 1.75`，未补偿时是 2.5），
 * 正好抵消。选 padding 而非 transform，是因为放进 transform 的文本会被栅格化到
 * 变换后的空间，小字号容易发虚 —— 布局阶段就落在最终位置更干净。
 *
 * ⚠️ `pb-[1.5px]` 与 `leading-none` 是**一对**：补偿值是按「行盒高 = font-size」
 * 算出来的，去掉 `leading-none`（行高变 normal ≈ 13.2px）补偿量就不再成立。
 * 改徽标尺寸或字号时必须重新测量这两个值。
 *
 * ⚠️ 不要把定位（`absolute -right-1 -top-1` 之类）写进本组件：各处头像/图标大小
 * 不同，挂点位置本来就该由调用方决定。调用方通过 `class` 传入即可（Vue 会自动合并
 * 到根元素）。
 *
 * 唯一实现的理由：这里原本有 5 处手写副本，其中 2 处漏了 `leading-none`，
 * 导致同一种徽标在不同位置基线不一致。`designGuards.test.ts` 有护栏禁止再手写。
 */
const props = withDefaults(
  defineProps<{
    /** 未读/待处理数量 */
    count: number;
    /** 显示上限，超过显示 `{max}+`（默认 99） */
    max?: number;
  }>(),
  { max: 99 },
);
</script>

<template>
  <span
    class="flex h-4 min-w-4 items-center justify-center rounded-full bg-[var(--gosslan-danger)] px-1 pb-[1.5px] text-[11px] font-medium leading-none text-white"
  >
    {{ props.count > props.max ? `${props.max}+` : props.count }}
  </span>
</template>
