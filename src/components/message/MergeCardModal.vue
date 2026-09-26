<script setup lang="ts">
/**
 * 合并转发的详情（点卡片打开）：把卡片里的 N 条按「发送者 + 内容」列出来。
 *
 * ## 图片的两种形态
 * 卡片载荷里的图片只带元信息（`{name,size,subtype,sha256}`），其中 `sha256` = cid（内容指纹）。
 * 打开详情时先按 cid 读**本机**字节（`read_content_preview`）：可能已经经传输落盘、或本机
 * 本来就是原始发送方 —— 有就直接显示大图。没有则给一个「拉取」按钮，按 cid 向**卡片发送者**
 * 请求（`request_content_by_cid`，ADR-0019 Phase 3，拥有即授权）。图片本体因此**不随卡片
 * 一帧发过去**（帧大小不变），只在需要时按需回源。
 *
 * ## 为什么媒体默认仍是占位
 * 不带 cid 的旧载荷 / 文件（非图片）没有回源钥匙，只能显示 `[图片] 名字` / `[文件] 名字`。
 */
import { computed, onBeforeUnmount, reactive, watch } from "vue";
import { Loader2 } from "lucide-vue-next";
import { t } from "@/i18n";
import BaseModal from "@/components/BaseModal.vue";
import { useImagePreviewStore } from "@/stores/useImagePreview";
import { api } from "@/api";
import { imageMime } from "@/utils/filePreview";
import { fmtConversationTime } from "@/utils/time";
import { mediaCid, mediaName, mergeItemLine, parseMergePayload, type MergedItem } from "@/utils/mergeCard";

const props = defineProps<{
  open: boolean;
  content: string;
  /** 卡片发送者（按需拉取的对端）。 */
  senderId?: string;
}>();
const emit = defineEmits<{ (e: "close"): void }>();

const parsed = computed(() => parseMergePayload(props.content));
const items = computed(() => parsed.value?.items ?? []);
/** 只对**带 cid** 的图片提示媒体不随卡片传输（不带 cid 的老卡片才需要解释占位）。 */
const hasMediaWithoutCid = computed(() =>
  items.value.some((i) => (i.kind === "image" || i.kind === "file") && !mediaCid(i)),
);

const IMAGE_MAX_BYTES = 15 * 1024 * 1024;

type ImagePhase = "loading" | "ready" | "absent" | "pulling" | "failed";
interface ImageSlot {
  phase: ImagePhase;
  url?: string;
  note?: string;
}
const imageSlots = reactive<Record<number, ImageSlot>>({});

/** 已就绪的图片 → 相册数组（`dataSrc` 直接塞 objectURL，只在本文档有效，见下面 MERGE_SOURCE 的说明）。 */
interface GalleryItem {
  msgId: string;
  name: string;
  cid?: string;
  dataSrc: string | null;
}
const gallery = computed<GalleryItem[]>(() => {
  const out: GalleryItem[] = [];
  items.value.forEach((it, i) => {
    if (it.kind !== "image") return;
    const slot = imageSlots[i];
    if (slot?.phase !== "ready") return;
    const cid = mediaCid(it);
    // ⚠️ 优先给 **cid**，别给 objectURL：`blob:` 只在本文档有效，交给独立预览窗口就是破图
    // （用户 2026-09-24 #40 要求"任何界面都能调这个预览"）。合并卡片的图本来就来自
    // content store，cid 是它的稳定身份；只有拿不到 cid 的旧载荷才退回 `slot.url`，
    // 那种情况由 store 的 `deliverableToWindow` 把整份相册留在应用内覆盖层。
    if (cid) out.push({ msgId: `merge-${i}`, name: mediaName(it), cid, dataSrc: null });
    else if (slot.url) out.push({ msgId: `merge-${i}`, name: mediaName(it), dataSrc: slot.url });
  });
  return out;
});
/**
 * 预览用的是全局那一份实例（用户 2026-09-24 #40），所以这个来源要有个 key。
 *
 * ⚠️ 合并卡片这里的图是 **objectURL**（blob:），只在本文档有效，而且卡片一关就被
 * `URL.revokeObjectURL` 回收 ⇒ 预览绝不能比它活得久。所以下面两处回收之前
 * 必须先 `closeIfFrom(MERGE_SOURCE)`：这与"预览长在本组件里时随组件一起消失"
 * 是同一个保证，只是换了实现位置。
 * （将来把预览镜像到独立窗口时，blob: 是跨不过文档的 —— 那条路必须先换成 data URL。）
 */
const MERGE_SOURCE = "merge:card";
const preview = useImagePreviewStore();

function openLightbox(i: number) {
  const gi = gallery.value.findIndex((g) => g.msgId === `merge-${i}`);
  if (gi >= 0) preview.openGallery(gallery.value, gi, MERGE_SOURCE);
}

let objectUrls: string[] = [];
let closed = false;

function releaseUrl(url?: string) {
  if (url) URL.revokeObjectURL(url);
}

async function probe(i: number, item: MergedItem) {
  const cid = mediaCid(item);
  if (!cid) return;
  imageSlots[i] = { phase: "loading" };
  try {
    const raw = await api.readContentPreview(cid, IMAGE_MAX_BYTES);
    const bytes = new Uint8Array(raw);
    const url = URL.createObjectURL(new Blob([bytes], { type: imageMime(mediaName(item)) }));
    objectUrls.push(url);
    if (closed || items.value[i] !== item) {
      releaseUrl(url);
      return;
    }
    imageSlots[i] = { phase: "ready", url };
  } catch (e) {
    const msg = String(e);
    imageSlots[i] = {
      phase: msg.includes("TOO_LARGE") ? "failed" : "absent",
      note: msg.includes("TOO_LARGE") ? t("merge.tooLarge") : undefined,
    };
  }
}

function sleep(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}

async function pull(i: number, item: MergedItem) {
  const cid = mediaCid(item);
  if (!props.senderId || !cid) return;
  imageSlots[i] = { phase: "pulling" };
  try {
    const ok = await api.requestContentByCid(props.senderId, cid, mediaName(item), 0);
    if (!ok) {
      imageSlots[i] = { phase: "failed", note: t("merge.pullUnsupported") };
      return;
    }
  } catch {
    imageSlots[i] = { phase: "failed", note: t("merge.pullFail") };
    return;
  }
  // 字节落盘是异步的（offer→分片→done），轮询读侧直到成功或超时。
  for (let k = 0; k < 40; k++) {
    await sleep(1500);
    if (imageSlots[i]?.phase !== "pulling" || closed) return;
    try {
      const raw = await api.readContentPreview(cid, IMAGE_MAX_BYTES);
      const bytes = new Uint8Array(raw);
      const url = URL.createObjectURL(new Blob([bytes], { type: imageMime(mediaName(item)) }));
      objectUrls.push(url);
      imageSlots[i] = { phase: "ready", url };
      return;
    } catch {
      /* 还没落盘，继续等 */
    }
  }
  imageSlots[i] = { phase: "failed", note: t("merge.pullFail") };
}

watch(
  [() => props.open, () => props.content],
  () => {
    // 先收预览再回收 URL（顺序反了就会先看到一次破图闪烁）
    preview.closeIfFrom(MERGE_SOURCE);
    objectUrls.forEach(releaseUrl);
    objectUrls = [];
    for (const k of Object.keys(imageSlots)) delete imageSlots[Number(k)];
    closed = false;
    if (props.open) {
      items.value.forEach((it, i) => {
        if (it.kind === "image" && mediaCid(it)) void probe(i, it);
      });
    }
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  closed = true;
  preview.closeIfFrom(MERGE_SOURCE);
  objectUrls.forEach(releaseUrl);
  objectUrls = [];
});
</script>

<template>
  <BaseModal
    :open="open"
    :title="parsed?.title || t('merge.title')"
    width="max-w-md"
    @close="emit('close')"
  >
    <div class="space-y-3">
      <div class="text-xs text-[var(--gosslan-text-2)]">
        {{ t("merge.count", { n: items.length }) }}
      </div>
      <div
        v-if="hasMediaWithoutCid"
        class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-hover)] px-3 py-2 text-xs leading-relaxed text-[var(--gosslan-text-2)]"
      >
        {{ t("merge.mediaNotIncluded") }}
      </div>

      <div class="max-h-80 space-y-2 overflow-y-auto">
        <div v-for="(it, i) in items" :key="i" class="space-y-0.5">
          <div class="flex items-baseline gap-1.5 text-[11px] text-[var(--gosslan-text-2)]">
            <span class="truncate font-medium" :title="it.sender">{{ it.sender }}</span>
            <span v-if="it.ts" class="shrink-0">{{ fmtConversationTime(it.ts) }}</span>
          </div>

          <!-- 图片：有 cid 就按需回源渲染大图；没有就退回占位行 -->
          <template v-if="it.kind === 'image' && mediaCid(it)">
            <button
              v-if="imageSlots[i]?.phase === 'ready' && imageSlots[i]?.url"
              type="button"
              class="block w-full rounded-[var(--gosslan-radius-md)]"
              :aria-label="t('merge.viewImage')"
              @click="openLightbox(i)"
            >
              <img
                :src="imageSlots[i]?.url"
                class="max-h-64 w-full rounded-[var(--gosslan-radius-md)] object-contain"
                :alt="t('msg.imageMessage')"
              />
            </button>
            <div
              v-else-if="imageSlots[i]?.phase === 'loading'"
              class="flex h-16 items-center justify-center rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-hover)] text-[var(--gosslan-text-2)]"
            >
              <Loader2 class="h-4 w-4 animate-spin" aria-hidden="true" />
            </div>
            <div
              v-else
              class="flex items-center justify-between gap-2 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-hover)] px-3 py-2"
            >
              <span class="min-w-0 flex-1 truncate text-[13px] text-[var(--gosslan-text)]" :title="mediaName(it)">
                {{ mergeItemLine(it) }}
              </span>
              <button
                v-if="imageSlots[i]?.phase !== 'pulling'"
                class="tap-safe shrink-0 rounded-[var(--gosslan-radius-sm)] px-2 py-1 text-xs text-[var(--gosslan-primary)] transition hover:bg-[var(--gosslan-hover)]"
                @click="pull(i, it)"
              >
                {{ imageSlots[i]?.phase === 'failed' ? t("merge.retry") : t("merge.pull") }}
              </button>
              <span v-else class="flex shrink-0 items-center gap-1 text-xs text-[var(--gosslan-text-2)]">
                <Loader2 class="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
                {{ t("merge.pulling") }}
              </span>
            </div>
            <div v-if="imageSlots[i]?.note" class="text-[11px] text-[var(--gosslan-danger-ink)]">
              {{ imageSlots[i]?.note }}
            </div>
          </template>

          <!-- 文本/代码给**全文**（详情页再截断就没有意义了）；媒体走占位行 -->
          <template v-else-if="it.kind === 'text' || it.kind === 'code'">
            <div class="break-words whitespace-pre-wrap text-[13px] text-[var(--gosslan-text)]">{{ it.content }}</div>
          </template>
          <template v-else>
            <div class="break-words whitespace-pre-wrap text-[13px] text-[var(--gosslan-text)]">{{ mergeItemLine(it) }}</div>
          </template>
        </div>
      </div>
    </div>
  </BaseModal>

</template>
