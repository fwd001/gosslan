<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { api } from "@/api";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "@/stores/useAppStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import SettingsRow from "@/components/settings/SettingsRow.vue";
import { formatBytes } from "@/utils/format";
import { Download, FolderOpen, Trash2 } from "lucide-vue-next";
import { t } from "@/i18n";
import type { CacheInfo } from "@/types";

const props = defineProps<{ active: boolean; reloadToken?: number }>();

const app = useAppStore();
const cacheInfo = ref<CacheInfo | null>(null);
/** 0 = 永久 / 无限制 */
const retentionDays = ref(0);
const maxQuotaMb = ref(0);
const cleaning = ref(false);
/** 导出进行中（读库 + 渲染可能耗时，期间禁用按钮防重复触发）。 */
const exporting = ref(false);
/** 文件接收目录（接收的图片/文件落盘于此，未手动另存前都在这里）。 */
const downloadsDir = ref("");

/** 回显赋值不应触发「改动即保存」。 */
let suppressAutoSave = false;

const quotas = [
  { value: 0, label: "settings.storage.quota.unlimited" },
  { value: 256, label: "256 MB" },
  { value: 512, label: "512 MB" },
  { value: 1024, label: "1 GB" },
  { value: 2048, label: "2 GB" },
];
// 旧版本可能存了预设之外的数值，补一个回显选项避免下拉框空白
const quotaOptions = computed(() => {
  if (maxQuotaMb.value > 0 && !quotas.some((q) => q.value === maxQuotaMb.value)) {
    return [...quotas, { value: maxQuotaMb.value, label: `${maxQuotaMb.value} MB` }];
  }
  return quotas;
});

async function loadCache() {
  cacheInfo.value = await api.getCacheInfo();
  suppressAutoSave = true;
  retentionDays.value = cacheInfo.value?.retention_days ?? 0;
  maxQuotaMb.value = Math.round((cacheInfo.value?.max_bytes ?? 0) / 1048576);
  // 等 watch 同步跳过这一轮由「回显赋值」触发的回调
  setTimeout(() => (suppressAutoSave = false), 0);
}

async function loadDownloadsDir() {
  try {
    downloadsDir.value = await api.getDownloadsDir();
  } catch {
    downloadsDir.value = "";
  }
}

async function changeDownloadsDir() {
  const picked = await openDialog({ directory: true });
  if (typeof picked !== "string") return;
  try {
    await api.setDownloadsDir(picked);
    downloadsDir.value = picked;
    app.toast(t("settings.storage.toast.dirUpdated"), "success");
  } catch (e) {
    app.toastError(e, t("settings.storage.toast.dirFail"));
  }
}

async function openDownloadsDir() {
  try {
    await api.openDownloadsDir();
  } catch (e) {
    app.toastError(e, t("settings.storage.toast.openFail"));
  }
}

watch([retentionDays, maxQuotaMb], (_nv, ov) => {
  if (suppressAutoSave) return;
  // 开启「自动删除」前必须让用户明确知道代价：被清理的图片/文件在历史消息里会打不开。
  // 默认（永久 + 无限制）不会走到这里，所以不改默认行为、也不影响任何人。
  if (retentionDays.value > 0 || maxQuotaMb.value > 0) {
    const keep = retentionDays.value > 0 ? t("settings.storage.confirm.keepDays", { n: retentionDays.value }) : t("settings.storage.confirm.keepForever");
    const cap = maxQuotaMb.value > 0 ? t("settings.storage.confirm.cap", { n: maxQuotaMb.value }) : t("settings.storage.confirm.capUnlimited");
    const ok = window.confirm(t("settings.storage.confirm.body", { keep, cap }));
    if (!ok) {
      // 用户取消：把两个下拉回滚到改动前的值
      const prev = ov as [number, number];
      suppressAutoSave = true;
      retentionDays.value = prev[0];
      maxQuotaMb.value = prev[1];
      setTimeout(() => (suppressAutoSave = false), 0);
      return;
    }
  }
  void applyCachePolicy(true);
});

async function applyCachePolicy(silent = false) {
  await api.setCachePolicy(
    retentionDays.value === 0 ? null : retentionDays.value,
    maxQuotaMb.value === 0 ? null : maxQuotaMb.value * 1048576,
  );
  if (!silent) app.toast(t("settings.storage.toast.saved"), "success");
  await loadCache();
}

/**
 * 导出全部聊天文字为单个 Markdown 文件。
 *
 * 这是磁盘吃紧 / 换机前**唯一的自救手段**：存储清理只会删媒体，但库本身如果出问题，
 * 没有导出入口就只能看着聊天记录丢。只导文字（媒体仅留文件名），所以不需要新依赖。
 */
async function exportChat() {
  const now = new Date();
  const stamp = `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, "0")}${String(
    now.getDate(),
  ).padStart(2, "0")}`;
  let destination: string | null;
  try {
    destination = await saveDialog({ defaultPath: `gosslan-${t("settings.storage.export.filename")}-${stamp}.md` });
  } catch (e) {
    app.toastError(e, t("settings.storage.toast.exportDialogFail"));
    return;
  }
  if (!destination) return; // 用户取消

  exporting.value = true;
  try {
    // 时区偏移交给前端给（Rust 侧不引入时区库），getTimezoneOffset 的符号与
    // "本地 = UTC + 偏移" 相反，因此取负。
    const r = await api.exportChatText(destination, -now.getTimezoneOffset());
    app.toast(t("settings.storage.toast.exported", { conversations: r.conversations, messages: r.messages }), "success");
  } catch (e) {
    app.toastError(e, t("settings.storage.toast.exportFail"));
  } finally {
    exporting.value = false;
  }
}

async function cleanNow() {
  cleaning.value = true;
  try {
    const r = await api.cleanCacheNow();
    // 结果要说清"清了什么"：0 个时明确告诉用户当前设置下无需清理，
    // 而不是丢一句"删除 0 个文件"让人不知道点了什么。
    if (r.removed === 0) {
      app.toast(t("settings.storage.toast.nothing"), "info");
    } else {
      app.toast(t("settings.storage.toast.cleaned", { n: r.removed, bytes: formatBytes(r.freed_bytes) }), "success");
    }
  } catch (e) {
    app.toastError(e, t("settings.storage.toast.cleanFail"));
  } finally {
    cleaning.value = false;
    await loadCache();
  }
}

watch(
  () => [props.active, props.reloadToken],
  () => {
    if (props.active) {
      void loadCache();
      void loadDownloadsDir();
    }
  },
  { immediate: true },
);
</script>

<template>
  <SettingsGroup
    :title="t('settings.group.storage')"
    :footer="t('settings.group.storage.footer')"
  >
    <SettingsRow
      :label="t('settings.storage.retention')"
      :description="t('settings.storage.retention.desc')"
    >
      <span class="gosslan-select-wrap">
        <select v-model="retentionDays" class="gosslan-select">
          <option :value="0">{{ t("settings.storage.retention.forever") }}</option>
          <option :value="3">{{ t("settings.storage.retention.days", { n: 3 }) }}</option>
          <option :value="7">{{ t("settings.storage.retention.days", { n: 7 }) }}</option>
          <option :value="30">{{ t("settings.storage.retention.days", { n: 30 }) }}</option>
        </select>
      </span>
    </SettingsRow>

    <SettingsRow
      :label="t('settings.storage.quota')"
      :description="t('settings.storage.quota.desc')"
    >
      <span class="gosslan-select-wrap">
        <select v-model.number="maxQuotaMb" class="gosslan-select">
          <option v-for="q in quotaOptions" :key="q.value" :value="q.value">{{ t(q.label) }}</option>
        </select>
      </span>
    </SettingsRow>

    <SettingsRow
      :label="t('settings.storage.dir')"
      :description="t('settings.storage.dir.desc')"
    >
      <div class="flex min-w-0 flex-col items-end gap-1">
        <span class="max-w-[240px] truncate text-[11px] text-[var(--gosslan-text-2)]" :title="downloadsDir || t('settings.storage.dir.default')">
          {{ downloadsDir || t("settings.storage.dir.default") }}
        </span>
        <div class="flex items-center gap-1.5">
          <button
            class="flex items-center gap-1 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-2.5 py-1 text-xs transition hover:bg-[var(--gosslan-hover)]"
            :title="t('settings.storage.dir.open.title')"
            @click="openDownloadsDir"
          >
            <FolderOpen class="h-3.5 w-3.5" />
            {{ t("settings.storage.dir.open") }}
          </button>
          <button
            class="rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-2.5 py-1 text-xs transition hover:bg-[var(--gosslan-hover)]"
            @click="changeDownloadsDir"
          >
            {{ t("settings.storage.dir.change") }}
          </button>
        </div>
      </div>
    </SettingsRow>

    <SettingsRow
      :label="t('settings.storage.export')"
      :description="t('settings.storage.export.desc')"
    >
      <button
        class="flex shrink-0 items-center gap-1.5 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-3 py-1.5 text-xs transition hover:bg-[var(--gosslan-hover)] disabled:opacity-50"
        :disabled="exporting"
        :title="t('settings.storage.export.title')"
        @click="exportChat"
      >
        <Download class="h-3.5 w-3.5" />
        {{ exporting ? t("settings.storage.export.exporting") : t("settings.storage.export.btn") }}
      </button>
    </SettingsRow>

    <div class="flex items-center justify-between gap-3 px-4 py-3">
      <span class="min-w-0 text-xs leading-5 text-[var(--gosslan-text-2)]">
        {{ t("settings.storage.stats.media") }} <b>{{ cacheInfo?.media_count ?? 0 }}</b> {{ t("settings.storage.stats.unit") }} ·
        <b>{{ formatBytes(cacheInfo?.media_bytes ?? 0) }}</b><br />
        {{ t("settings.storage.stats.db") }} <b>{{ formatBytes(cacheInfo?.db_bytes ?? 0) }}</b>
      </span>
      <button
        class="flex shrink-0 items-center gap-1.5 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-3 py-1.5 text-xs transition hover:bg-[var(--gosslan-hover)] disabled:opacity-50"
        :disabled="cleaning"
        :title="t('settings.storage.clean.title')"
        @click="cleanNow"
      >
        <Trash2 class="h-3.5 w-3.5" />
        {{ t("settings.storage.clean.btn") }}
      </button>
    </div>
  </SettingsGroup>
</template>
