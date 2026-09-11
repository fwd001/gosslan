<script setup lang="ts">
/**
 * 「文件与共享」分区（iOS 概念：文件位置 / App 数据位置）。
 *
 * 把"两个目录"收在一处，因为它们回答的是同一个问题：**文件放哪儿、给谁看**
 *   · 接收文件目录：好友发来的图片/文件落盘位置（另存前都在这里）；
 *   · 共享目录：我开放给好友浏览的文件夹（对方在「共享目录」页只能看到它）。
 * 之前两者被拆到「存储」和「通用」下，且「通用」的导航项写着「通知」——
 * 用户点开"通知"看到共享目录，自然是分类错误（2026-09-12 反馈）。
 *
 * 与「存储」的分工：这里管**位置**，「存储」（`StorageSection`）管**占用与清理**。
 * 观察 `reloadToken` 是为了在「恢复默认」之后重新回显路径（与其它分区同一约定）。
 */
import { ref, watch } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { api } from "@/api";
import { useAppStore } from "@/stores/useAppStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import SettingsRow from "@/components/settings/SettingsRow.vue";
import { FolderOpen } from "lucide-vue-next";
import { t } from "@/i18n";

const props = defineProps<{ active: boolean; reloadToken?: number }>();

const app = useAppStore();
/** 接收文件目录（接收的图片/文件落盘于此，未手动另存前都在这里）。 */
const downloadsDir = ref("");

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
    app.toastError(e, t("settings.storage.toast.dirOpenFail"));
  }
}

async function pickShareDir() {
  const picked = await openDialog({ directory: true });
  if (typeof picked === "string") {
    try {
      await app.setShareDir(picked);
      app.toast(t("settings.toast.shareSet"), "success");
    } catch (e) {
      app.toastError(e, t("settings.toast.shareFail"));
    }
  }
}

watch(
  () => [props.active, props.reloadToken],
  async () => {
    if (!props.active) return;
    await loadDownloadsDir();
  },
  { immediate: true },
);
</script>

<template>
  <div class="space-y-5">
    <!-- 接收文件目录 -->
    <SettingsGroup :title="t('settings.group.files')" :footer="t('settings.group.files.footer')">
      <SettingsRow :label="t('settings.storage.dir')" :description="t('settings.storage.dir.desc')">
        <div class="flex min-w-0 flex-col items-end gap-1">
          <span
            class="max-w-[240px] truncate text-[11px] text-[var(--gosslan-text-2)]"
            :title="downloadsDir || t('settings.storage.dir.default')"
          >
            {{ downloadsDir || t("settings.storage.dir.default") }}
          </span>
          <div class="flex items-center gap-1.5">
            <button
              class="tap-safe flex items-center gap-1 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-2.5 py-1 text-xs transition hover:bg-[var(--gosslan-hover)]"
              :title="t('settings.storage.dir.open.title')"
              @click="openDownloadsDir"
            >
              <FolderOpen class="h-3.5 w-3.5" />
              {{ t("settings.storage.dir.open") }}
            </button>
            <button
              class="tap-safe rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-2.5 py-1 text-xs transition hover:bg-[var(--gosslan-hover)]"
              @click="changeDownloadsDir"
            >
              {{ t("settings.storage.dir.change") }}
            </button>
          </div>
        </div>
      </SettingsRow>

      <!-- 共享目录 -->
      <SettingsRow :label="t('settings.share.folder')" :description="app.shareDir || t('common.notSet')" last>
        <button
          class="tap-safe flex items-center gap-1.5 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-3 py-1.5 text-xs transition hover:bg-[var(--gosslan-hover)]"
          @click="pickShareDir"
        >
          <FolderOpen class="h-3.5 w-3.5" />
          {{ t("common.chooseFolder") }}
        </button>
      </SettingsRow>
    </SettingsGroup>
  </div>
</template>
