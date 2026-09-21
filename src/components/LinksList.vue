<script setup lang="ts">
/**
 * 左栏「链接」视图：用户配置的外部链接列表 + 增 / 改 / 删（用户 2026-09-17）。
 *
 * 点某条链接 → 交给父级（`ResponsiveLayout`）在**独立窗口**里加载它
 * （窗口隔离、不授予远端页面任何命令权限，见 Rust `open_link_window`）。
 *
 * 数据只在主窗口用（左列），所以走独立命令而不是 `Settings` + `settings-changed`：
 * 后端能返回真实校验错误，用户不会"以为保存了"。校验口径在前端也有一份（`utils/externalLinks`）。
 */
import { onMounted, ref } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useBackLayer } from "@/composables/useBackLayer";
import MobilePageFrame from "@/components/MobilePageFrame.vue";
import { api } from "@/api";
import { t } from "@/i18n";
import { MAX_EXTERNAL_LINKS, MAX_LINK_NAME_CHARS, validateLinkInput } from "@/utils/externalLinks";
import { ExternalLink as ExternalLinkIcon, Pencil, Plus, Trash2 } from "lucide-vue-next";
import type { ExternalLink } from "@/types";

defineProps<{ opening?: boolean }>();
const emit = defineEmits<{
  (e: "open", link: ExternalLink): void;
  /** 移动端覆盖层返回（回到「我的」页）。桌面端内嵌时不会触发。 */
  (e: "back"): void;
}>();

const app = useAppStore();
const links = ref<ExternalLink[]>([]);
const loading = ref(true);
/** 当前草稿（`id === null` = 新增）。 */
const draft = ref<{ id: string | null; name: string; url: string } | null>(null);
const saving = ref(false);

async function load() {
  loading.value = true;
  try {
    links.value = await api.listExternalLinks();
  } catch (e) {
    app.toastError(e, t("links.loadFail"));
  } finally {
    loading.value = false;
  }
}
onMounted(load);

// 移动端链接页是覆盖层下钻页，系统返回键 / 框架返回键都走这里回到「我的」页。
useBackLayer(
  () => app.isMobile,
  () => emit("back"),
);

function startAdd() {
  draft.value = { id: null, name: "", url: "" };
}
function startEdit(l: ExternalLink) {
  draft.value = { id: l.id, name: l.name, url: l.url };
}
function cancelDraft() {
  draft.value = null;
}

async function saveDraft() {
  const d = draft.value;
  if (!d || saving.value) return;
  if (!d.id && links.value.length >= MAX_EXTERNAL_LINKS) {
    app.toast(t("links.err.tooMany", { n: MAX_EXTERNAL_LINKS }), "error");
    return;
  }
  const err = validateLinkInput(d, links.value, d.id ?? undefined);
  if (err) {
    app.toast(t(`links.err.${err}`, { n: MAX_LINK_NAME_CHARS }), "error");
    return;
  }
  saving.value = true;
  try {
    links.value = d.id
      ? await api.updateExternalLink(d.id, d.name, d.url)
      : await api.addExternalLink(d.name, d.url);
    draft.value = null;
  } catch (e) {
    app.toastError(e, t(d.id ? "links.updateFail" : "links.addFail"));
  } finally {
    saving.value = false;
  }
}

async function remove(l: ExternalLink) {
  try {
    links.value = await api.removeExternalLink(l.id);
  } catch (e) {
    app.toastError(e, t("links.removeFail"));
  }
}
</script>

<template>
  <!-- 统一框架头部：标题 + 新增按钮（右上角）由 MobilePageFrame 提供；
       移动端是覆盖层带返回键，桌面端内嵌在左列（无返回键，标题栏保留）。 -->
  <MobilePageFrame
    :mode="app.isMobile ? 'overlay' : 'inline'"
    :header="true"
    :show-back="app.isMobile"
    :title="t('links.title')"
    @back="emit('back')"
  >
    <template #actions>
      <button
        v-if="!draft"
        class="tap-safe flex h-8 w-8 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('links.add')"
        :aria-label="t('links.add')"
        @click="startAdd"
      >
        <Plus class="h-[18px] w-[18px]" />
      </button>
    </template>

    <div class="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
      <!-- 新增 / 编辑表单 -->
      <div
        v-if="draft"
        class="mb-2 space-y-2 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-[var(--gosslan-card)] p-2.5"
      >
        <input
          v-model="draft.name"
          type="text"
          :maxlength="MAX_LINK_NAME_CHARS"
          :placeholder="t('links.namePlaceholder')"
          class="w-full rounded-[var(--gosslan-radius-md)] border border-transparent bg-[var(--gosslan-bg)] px-3 py-1.5 text-[13px] outline-none transition focus:border-transparent"
          @keyup.enter="saveDraft"
        />
        <input
          v-model="draft.url"
          type="text"
          :placeholder="t('links.urlPlaceholder')"
          class="w-full rounded-[var(--gosslan-radius-md)] border border-transparent bg-[var(--gosslan-bg)] px-3 py-1.5 text-[13px] outline-none transition focus:border-transparent"
          @keyup.enter="saveDraft"
        />
        <div class="flex justify-end gap-2">
          <button
            class="tap-safe rounded-[var(--gosslan-radius-md)] px-3 py-1.5 text-[13px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
            @click="cancelDraft"
          >
            {{ t("common.cancel") }}
          </button>
          <button
            class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-primary)] px-3 py-1.5 text-[13px] text-white transition hover:bg-[var(--gosslan-primary-hover)] disabled:opacity-50"
            :disabled="saving"
            @click="saveDraft"
          >
            {{ t("common.save") }}
          </button>
        </div>
      </div>

      <p v-if="loading" class="px-2 py-3 text-xs text-[var(--gosslan-text-2)]">{{ t("links.loading") }}</p>
      <p
        v-else-if="!links.length && !draft"
        class="px-2 py-8 text-center text-xs text-[var(--gosslan-text-2)]"
      >
        {{ t("links.empty") }}
      </p>

      <!-- 链接行：点名称打开独立窗口；右侧编辑 / 删除常显（触屏友好）。
           行样式与聊天 / 通讯录 / 收藏列表**同款**：满宽不圆角 + `--gosslan-list-hover` 悬停
           + 行间内缩分隔线（用户 2026-09-20：「链接列表的也修一下」）。 -->
      <div
        v-for="l in links"
        :key="l.id"
        class="relative flex items-center gap-0.5 px-2 py-1.5 transition hover:bg-[var(--gosslan-list-hover)]"
      >
        <button
          class="tap-safe flex min-w-0 flex-1 items-center gap-2 rounded-[var(--gosslan-radius-sm)] px-1.5 py-1.5 text-left"
          :class="opening ? 'opacity-60' : ''"
          :aria-busy="opening"
          :title="l.url"
          @click="emit('open', l)"
        >
          <ExternalLinkIcon class="h-4 w-4 shrink-0 text-[var(--gosslan-text-2)]" />
          <span class="min-w-0 flex-1 truncate text-[13px] text-[var(--gosslan-text)]" :title="l.name">
            {{ l.name }}
          </span>
        </button>
        <button
          class="tap-safe flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          :title="t('links.edit')"
          :aria-label="t('links.edit')"
          @click="startEdit(l)"
        >
          <Pencil class="h-3.5 w-3.5" />
        </button>
        <button
          class="tap-safe flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-danger-soft)] hover:text-[var(--gosslan-danger-ink)]"
          :title="t('links.remove')"
          :aria-label="t('links.remove')"
          @click="remove(l)"
        >
          <Trash2 class="h-3.5 w-3.5" />
        </button>
        <!-- 行间内缩分隔线：从文本列起（px-2 + 按钮 px-1.5 + 16 图标 + gap-2 = 38px） -->
        <div class="pointer-events-none absolute bottom-0 left-[38px] right-0 h-px bg-[var(--gosslan-divider)]"></div>
      </div>
    </div>
  </MobilePageFrame>
</template>
