<script setup lang="ts">
/**
 * 待办描述图片缩略图：按 sha256（= content store cid）找回本机已落盘的字节并渲染。
 *
 * 字节通过 `read_content_preview` 读回（与聊天图片同一套安全边界：只允许读 downloads 下文件、
 * 或本机自己发出的内容），转成 objectURL 显示。
 *
 * ⚠️ **字节是"后到"的**：任务定义（带图片元数据）先到，真实文件走群文件管线后到 ——
 * 所以第一次读不到很正常。这里用**退避重试**把后到的字节追回来（1.5s 起步、最长 30s），
 * 而不是失败一次就永远显示占位（用户 2026-09-17：「群任务的图片大概率打不开」）。
 * `loadContentPreview` 也配合改成"失败不缓存"。
 *
 * ⚠️⚠️ **本组件绝不 `revokeObjectURL`**（这是 2026-09-21「任务详情里的图片大概率加载失败」
 * 的真根因，别再好心加回来）：`loadContentPreview` 返回的 objectURL 是**模块级缓存里
 * 大家共用的同一个字符串**（同一 cid 在聊天时间线的任务卡、看板表单、任务详情里同时存在）。
 * 此前本组件在**卸载/换图**时 revoke 掉它，于是任何一处卸载（虚拟列表回收一行、关掉一次
 * 详情弹窗）都会把所有其它视图的图一起打回裂图；更糟的是缓存里那个 URL **已经死了却仍被
 * 命中**，`loadContentPreview` 直接返回它 ⇒ 连退避重试也救不回来，只能重启应用。
 * URL 的生命周期归缓存所有；要回收内存请走缓存自己的出口，不要由消费者就地 revoke。
 */
import { onBeforeUnmount, ref, watch } from "vue";
import { api } from "@/api";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { loadContentPreview, type PreviewResult } from "@/utils/filePreview";
import type { TodoImage } from "@/utils/todos";
import { t } from "@/i18n";

const props = withDefaults(
  defineProps<{
    image: TodoImage;
    /**
     * 可点开大图。**默认 false = 纯展示** ⇒ 调用点漏给这一个 prop 的表现不是报错，而是
     * "图渲染出来了但点不动"：编译过、类型过、单测过，只有真人去点才发现。
     * 本仓真发生过两处：收藏那处（已修 `a077850`）、聊天时间线的任务卡（同一轮漏掉的那一面）。
     * ⇒ 每个调用点必须同时给 `clickable` 与 `@open=`，由 `designGuards` 那条**按形状数调用点**
     * 的判据兜着，不靠人记得数过。
     */
    clickable?: boolean;
  }>(),
  { clickable: false },
);
const emit = defineEmits<{ (e: "open"): void }>();

const app = useAppStore();
const chat = useChatStore();

const url = ref<string | null>(null);
const loading = ref(true);
const failed = ref(false);

const RETRY_BASE_MS = 1500;
const RETRY_MAX_MS = 30_000;

let timer: number | null = null;
let delay = RETRY_BASE_MS;

function cancelRetry() {
  if (timer !== null) {
    window.clearTimeout(timer);
    timer = null;
  }
}

function scheduleRetry(cid: string, name: string) {
  cancelRetry();
  timer = window.setTimeout(() => {
    timer = null;
    void load(cid, name);
  }, delay);
  delay = Math.min(delay * 2, RETRY_MAX_MS);
}

/**
 * 本机没有这份字节时，**主动**按 cid 向对端要一份 —— 而不是只等退避重试把别人推来的字节撞上。
 *
 * 为什么必须主动：群文件/任务图片的收件人登记是**发送那一刻的成员快照**
 * （`commands/group_announcements.rs` 逐成员写 `group_file_recipients`），后加入群的人没有那一行，
 * 于是发送端的 `flush_pending_group_files` 永远不会向他派货；他这边再怎么重试也读不到字节。
 * 拉取这条路的权限在**对端**判：`ContentRequest` 走"拥有即授权" + 当前群名册校验
 * （`network/transport.rs`），所以问谁都可以、不在群里的对端会自己拒 —— 本机不做授权假设。
 */
async function pullFromPeers(cid: string, name: string) {
  const me = app.device?.device_id;
  const online = chat.peers.map((x) => x.device_id).filter((x) => x && x !== me);
  const known = chat.friends.map((x) => x.device_id).filter((x) => x && x !== me);
  const peer = online[0] ?? known[0];
  if (!peer) return; // 一个对端都不认识 ⇒ 只能继续等，不报错
  try {
    await api.requestContentByCid(peer, cid, name, props.image.size ?? 0);
  } catch {
    /* 拉不到就等下一轮退避；这里不弹提示，避免每个缩略图各刷一条 */
  }
}

async function load(cid: string, name: string) {
  loading.value = true;
  failed.value = false;
  try {
    const r: PreviewResult = await loadContentPreview(cid, name);
    if (r.url) {
      url.value = r.url;
      loading.value = false;
      return; // 成功 ⇒ 不再重试
    }
    failed.value = true;
  } catch {
    failed.value = true;
  }
  loading.value = false;
  void pullFromPeers(cid, name);
  scheduleRetry(cid, name);
}

watch(
  () => props.image.sha256,
  (cid) => {
    // 只清引用，**不 revoke**（URL 归缓存所有，见文件头说明）
    url.value = null;
    delay = RETRY_BASE_MS;
    cancelRetry();
    if (cid) void load(cid, props.image.name);
  },
  { immediate: true },
);

onBeforeUnmount(cancelRetry);

function open() {
  // ⚠️ **不看 `url`**（用户 2026-09-29：「从任务列表进入某个任务后，查看图片时可能打不开图片预览」）：
  // 缩略图和大图是在**两个不同文档**里各读一次的（主窗口这份 vs 独立预览窗口那份，各自的
  // objectURL 缓存互不相干）。所以「本机其实有这份字节、预览窗口读得到，只有这里的读取还没成功」
  // 是一个真实且常见的状态 —— 旧写法把可点性与 `url` 绑在一起，那个状态下点击就**完全没反应**。
  // 现在可点性只由 `clickable` 决定；字节真取不到时，预览自己有「无法预览/已被清理」那一格，
  // 用户看到的是那句说明，而不是"点了没反应"。
  if (props.clickable) emit("open");
}
</script>

<template>
  <div
    class="relative h-20 w-20 shrink-0 overflow-hidden rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-[var(--gosslan-bg)] transition"
    :class="clickable ? 'cursor-pointer hover:opacity-90' : ''"
    :title="clickable ? t('todo.viewImage') : url ? image.name : t('todo.imageSyncing')"
    :role="clickable ? 'button' : undefined"
    :tabindex="clickable ? 0 : undefined"
    :aria-label="clickable ? t('todo.viewImage') : undefined"
    @click="open"
    @keydown.enter.prevent="open"
    @keydown.space.prevent="open"
  >
    <img
      v-if="url"
      :src="url"
      :alt="t('msg.imageMessage')"
      class="h-full w-full object-cover"
      loading="lazy"
    />
    <div
      v-else
      class="flex h-full w-full items-center justify-center px-1 text-center text-[11px] text-[var(--gosslan-text-2)]"
    >
      {{ loading ? "…" : t("todo.imageSyncing") }}
    </div>
  </div>
</template>
