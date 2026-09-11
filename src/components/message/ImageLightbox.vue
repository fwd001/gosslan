<script setup lang="ts">
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

/** 缩放（滚轮，1~5 倍）与拖拽平移（放大后可拖动查看局部），双击复位。 */
const scale = ref(1);
const tx = ref(0);
const ty = ref(0);
let dragging = false;
let lastX = 0;
let lastY = 0;

function reset() {
  scale.value = 1;
  tx.value = 0;
  ty.value = 0;
}

function onWheel(e: WheelEvent) {
  e.preventDefault();
  const next = Math.min(5, Math.max(1, scale.value + (e.deltaY < 0 ? 0.2 : -0.2)));
  if (next === 1) {
    tx.value = 0;
    ty.value = 0;
  }
  scale.value = next;
}

function onPointerDown(e: PointerEvent) {
  if (scale.value <= 1) return;
  dragging = true;
  lastX = e.clientX;
  lastY = e.clientY;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}
function onPointerMove(e: PointerEvent) {
  if (!dragging) return;
  tx.value += e.clientX - lastX;
  ty.value += e.clientY - lastY;
  lastX = e.clientX;
  lastY = e.clientY;
}
function onPointerUp() {
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
        <div class="absolute right-4 top-4 z-10 flex items-center gap-1">
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
            class="max-h-[85vh] max-w-[88vw] select-none rounded-[var(--gosslan-radius-lg)] shadow-2xl"
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
        <div class="absolute bottom-5 left-1/2 -translate-x-1/2 rounded-full bg-black/45 px-3 py-1 text-xs text-white/85">
          <template v-if="hasMultiple">{{ t("msg.lightbox.multiHint", { i: index + 1, n: images.length }) }}</template>
          <template v-else>{{ t("msg.lightbox.singleHint") }}</template>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>
