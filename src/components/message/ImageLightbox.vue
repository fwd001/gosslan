<script setup lang="ts">
/* focus-ring-ok：本组件的**容器**带 `outline-none`（`role="menu"` / `role="dialog"` +
   `tabindex="-1"`，只用于把焦点接进来），焦点指示由内部条目/控件承担；
   给整块容器画 2px 环在全屏遮罩/弹出菜单上只会变成噪声。
   ⇒ 按 `designGuards` ⑦ 的约定，用文件级逃生阀显式声明，而不是靠"没人发现"。 */
import { clampScale, pinchScale, swipeDirection } from "@/utils/lightboxGestures";
import { t } from "@/i18n";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { ChevronLeft, ChevronRight, Save, X } from "lucide-vue-next";
import { useAppStore } from "@/stores/useAppStore";
import { loadFilePreview } from "@/utils/filePreview";

/** 相册里的一张图：新格式走 readFilePreview（msg_id → blob URL），旧格式 data URL 直接用。 */
interface GalleryImage {
  msgId: string;
  name: string;
  dataSrc: string | null;
}

const props = defineProps<{
  images: GalleryImage[];
  index: number;
  open: boolean;
}>();
const emit = defineEmits<{
  (e: "close"): void;
  (e: "update:index", v: number): void;
}>();

const app = useAppStore();

const current = computed(() => props.images[props.index] ?? null);
const hasMultiple = computed(() => props.images.length > 1);

const src = ref<string>("");
const note = ref<string | null>(null);

/** 解析当前图片 src：旧格式直接用 dataSrc，新格式走 readFilePreview（带缓存，秒回）。 */
async function resolveCurrent() {
  const img = current.value;
  if (!img) {
    src.value = "";
    note.value = null;
    return;
  }
  if (img.dataSrc) {
    src.value = img.dataSrc;
    note.value = null;
    return;
  }
  const r = await loadFilePreview(img.msgId, "image", img.name);
  src.value = r.url ?? "";
  note.value = r.note ?? null;
}

watch(current, () => void resolveCurrent(), { immediate: true });

function go(delta: number) {
  if (props.images.length === 0) return;
  const next = (props.index + delta + props.images.length) % props.images.length;
  emit("update:index", next);
}

/** 保存当前图片：fetch 源(支持 dataURL 与 blob URL) → base64 → rust save_data_file 落盘 */
async function saveImage() {
  if (!src.value) return;
  try {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const { invoke } = await import("@tauri-apps/api/core");
    const destination = await save({ defaultPath: `${t("common.image")}-${Date.now()}.png` });
    if (!destination) return; // 用户取消
    const buf = new Uint8Array(await (await fetch(src.value)).arrayBuffer());
    let binary = "";
    const chunk = 0x8000;
    for (let i = 0; i < buf.length; i += chunk) {
      binary += String.fromCharCode(...buf.subarray(i, i + chunk));
    }
    await invoke("save_data_file", { base64Data: btoa(binary), destination });
    app.toast(t("msg.imageSaved"), "success");
  } catch (e) {
    app.toastError(e, t("msg.saveImageFail"));
  }
}

/**
 * 手势与缩放（HIG：查看大图要支持**双指缩放 + 左右滑动切图**，不是只有箭头按钮）。
 *
 * 三种手势共用一个指针表：
 *  · 单指 + 已放大 ⇒ 平移（看局部）；
 *  · 单指 + 原始尺寸 ⇒ 左右滑动切图（仅触屏；鼠标拖动不切图，避免误换图）；
 *  · 双指 ⇒ 按两指距离比例缩放（锚点不动，iOS 相册的手感）。
 * 判定阈值/主轴/边界全在 `utils/lightboxGestures.ts`（纯函数 + 单测）。
 */
const scale = ref(1);
const tx = ref(0);
const ty = ref(0);
let dragging = false;
let lastX = 0;
let lastY = 0;
/** 当前按下的指针（触屏才可能有多指）：id → 位置。 */
const pointers = new Map<number, { x: number; y: number }>();
/** 双指手势的起点：两指距离与当时的倍率。 */
let pinchStart: { distance: number; scale: number } | null = null;

function reset() {
  scale.value = 1;
  tx.value = 0;
  ty.value = 0;
  pointers.clear();
  pinchStart = null;
  dragging = false;
}

function onWheel(e: WheelEvent) {
  e.preventDefault();
  const next = clampScale(scale.value + (e.deltaY < 0 ? 0.2 : -0.2));
  if (next === 1) {
    tx.value = 0;
    ty.value = 0;
  }
  scale.value = next;
}

function twoPointerDistance(): number | null {
  if (pointers.size < 2) return null;
  const [a, b] = [...pointers.values()];
  if (!a || !b) return null;
  return Math.hypot(a.x - b.x, a.y - b.y);
}

function onPointerDown(e: PointerEvent) {
  pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  if (pointers.size === 2) {
    // 进入双指：记下起点，随后按距离比例缩放
    pinchStart = { distance: twoPointerDistance() ?? 0, scale: scale.value };
    dragging = false;
    return;
  }
  if (scale.value > 1) {
    dragging = true;
    lastX = e.clientX;
    lastY = e.clientY;
  }
}

function onPointerMove(e: PointerEvent) {
  const prev = pointers.get(e.pointerId);
  if (!prev) return;
  pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });

  // ① 双指缩放
  if (pointers.size >= 2 && pinchStart) {
    const d = twoPointerDistance();
    if (d !== null) scale.value = pinchScale(pinchStart.distance, d, pinchStart.scale);
    // 缩回原始尺寸时把位移归零，否则图会停在屏幕外
    if (scale.value <= 1) {
      tx.value = 0;
      ty.value = 0;
    }
    return;
  }
  // ② 单指：已放大 ⇒ 平移
  if (dragging) {
    tx.value += e.clientX - lastX;
    ty.value += e.clientY - lastY;
    lastX = e.clientX;
    lastY = e.clientY;
    return;
  }
  // ③ 单指（仅触屏）在原始尺寸下的横向滑动留给 pointerup 判定（这里只累积，不做事），
  //    避免边滑边切图（手指还没抬起来就换图会很突兀）。
}

function onPointerUp(e: PointerEvent) {
  const start = pointers.get(e.pointerId);
  pointers.delete(e.pointerId);
  if (pointers.size === 0) pinchStart = null;
  if (!start) return;
  // 触屏滑动切图：只有"原始尺寸 + 单指 + 触屏"才判（鼠标拖动不换图）
  if (e.pointerType === "touch" && !dragging && scale.value <= 1) {
    const dir = swipeDirection(
      e.clientX - start.x,
      e.clientY - start.y,
      scale.value,
      props.index,
      props.images.length,
    );
    if (dir !== null) go(dir);
  }
  dragging = false;
}

function onKey(e: KeyboardEvent) {
  if (!props.open) return;
  if (e.key === "Escape") {
    emit("close");
    return;
  }
  if (e.key === "ArrowLeft") {
    go(-1);
    return;
  }
  if (e.key === "ArrowRight") {
    go(1);
    return;
  }
}

// 打开 / 关闭 / 切图时复位缩放与位移
watch(
  () => [props.open, props.index] as const,
  () => reset(),
);

/**
 * 打开时把焦点移进浮层（HIG：模态浮层要有 dialog 语义且焦点在其中）。
 * 本组件没有引入 headlessui Dialog（它是自绘的全屏预览），所以至少做到：
 * `role="dialog" aria-modal="true"` + 打开即聚焦容器（Tab 之后在浮层内流转），
 * 关闭后把焦点还给触发元素。
 */
const dialogRef = ref<HTMLElement | null>(null);
let restoreFocus: HTMLElement | null = null;
watch(
  () => props.open,
  (open) => {
    if (open) {
      restoreFocus = document.activeElement as HTMLElement | null;
      void nextTick(() => dialogRef.value?.focus());
    } else {
      restoreFocus?.focus?.();
      restoreFocus = null;
    }
  },
);

onMounted(() => window.addEventListener("keydown", onKey));
onBeforeUnmount(() => window.removeEventListener("keydown", onKey));
</script>

<template>
  <Teleport to="body">
    <Transition
      enter-active-class="duration-150 ease-out"
      enter-from-class="opacity-0"
      leave-active-class="duration-100 ease-in"
      leave-to-class="opacity-0"
    >
      <div
        v-if="open"
        ref="dialogRef"
        tabindex="-1"
        class="glass fixed inset-0 z-[80] flex items-center justify-center outline-none"
        style="background: rgba(0, 0, 0, 0.45)"
        role="dialog"
        aria-modal="true"
        :aria-label="t('common.image')"
        @click="emit('close')"
        @wheel="onWheel"
      >
        <!-- 右上操作区：保存 + 关闭（与其它弹窗一致的样式） -->
        <!-- 移动端让开状态栏/挖孔（`safe-area-inset-top`）：用户 2026-09-12 实测
             「安卓端图片预览右上角的保存和叉叉与状态栏重叠」。桌面端 env() 为 0，视觉不变。 -->
        <div
          class="absolute right-4 z-10 flex items-center gap-1"
          :style="{ top: 'calc(env(safe-area-inset-top, 0px) + 1rem)' }"
        >
          <button
            class="flex h-9 items-center gap-1.5 rounded-full px-3 text-[13px] text-white/85 transition hover:bg-white/15"
            :title="t('common.saveImage')"
            @click.stop="saveImage"
          >
            <Save class="h-4 w-4" />
            {{ t("common.save") }}
          </button>
          <button
            class="tap-safe flex h-9 w-9 items-center justify-center rounded-full text-white/85 transition hover:bg-white/15"
            :title="t('common.closeEsc')" :aria-label="t('common.closeEsc')"
            @click.stop="emit('close')"
          >
            <X class="h-5 w-5" />
          </button>
        </div>

        <!-- 上一张 -->
        <button
          v-if="hasMultiple"
          class="absolute left-4 top-1/2 z-10 flex h-11 w-11 -translate-y-1/2 items-center justify-center rounded-full bg-white/10 text-white/90 transition hover:bg-white/20"
          :title="t('common.prev')" :aria-label="t('common.prev')"
          @click.stop="go(-1)"
        >
          <ChevronLeft class="h-6 w-6" />
        </button>

        <!-- 下一张 -->
        <button
          v-if="hasMultiple"
          class="absolute right-4 top-1/2 z-10 flex h-11 w-11 -translate-y-1/2 items-center justify-center rounded-full bg-white/10 text-white/90 transition hover:bg-white/20"
          :title="t('common.next')" :aria-label="t('common.next')"
          @click.stop="go(1)"
        >
          <ChevronRight class="h-6 w-6" />
        </button>

        <!-- 图片主体 -->
        <template v-if="src">
          <img :alt="current?.name || t('msg.imagePreview')"
            :src="src"
            class="max-h-[85vh] max-w-[88vw] touch-none select-none rounded-[var(--gosslan-radius-lg)] shadow-2xl"
            :style="{
              transform: `translate(${tx}px, ${ty}px) scale(${scale})`,
              cursor: scale > 1 ? (dragging ? 'grabbing' : 'grab') : 'zoom-in',
            }"
            draggable="false"
            @click.stop
            @dblclick="reset"
            @pointerdown="onPointerDown"
            @pointermove="onPointerMove"
            @pointerup="onPointerUp"
            @pointercancel="onPointerUp"
          />
        </template>
        <div
          v-else
          class="flex h-48 items-center justify-center px-8 text-sm text-white/70"
        >
          {{ note ?? t("msg.cannotPreview") }}
        </div>

        <!-- 底部提示：计数 + 操作说明 -->
        <div
          class="absolute left-1/2 -translate-x-1/2 rounded-full bg-black/45 px-3 py-1 text-xs text-white/85"
          :style="{ bottom: 'calc(env(safe-area-inset-bottom, 0px) + 1.25rem)' }"
        >
          <template v-if="hasMultiple">{{ t("msg.lightbox.multiHint", { i: index + 1, n: images.length }) }}</template>
          <template v-else>{{ t("msg.lightbox.singleHint") }}</template>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>
