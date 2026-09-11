<script setup lang="ts">
/**
 * 「重置与数据」设置分区：恢复默认 + 清除聊天数据（破坏性，二次确认）。
 *
 * 抽出的原因同 `GeneralSection`：三种外壳（独立窗口 / 移动端整页 / 弹窗）共用同一份标记。
 *
 * 恢复默认后需要让各分区重新拉数据 —— 通过 `restored` 事件上抛，
 * 由外壳 bump 自己的 `reloadToken`（原先这一段是内联在 SettingsPanel 里的局部逻辑）。
 */
import { ref } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import BaseModal from "@/components/BaseModal.vue";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import { RotateCcw, Trash2 } from "lucide-vue-next";
import { t } from "@/i18n";

const emit = defineEmits<{ (e: "restored"): void }>();

const app = useAppStore();
const chat = useChatStore();

/**
 * 恢复默认：外观 / 网卡 / 缓存策略回到默认值（不动好友与聊天数据）。
 *
 * ⚠️ 走二次确认（HIG：让用户容易从错误中恢复）。它不只是"改外观"：
 * `resetDefaults()` 还会把**昵称恢复默认、头像清空并广播给已连接的好友**，
 * 一次点击就执行不合适 —— 旁边的「清除聊天数据」本来就有确认，这里不该更宽松。
 */
const confirmRestore = ref(false);

async function restoreDefaults() {
  confirmRestore.value = false;
  await app.resetDefaults();
  emit("restored");
  app.toast(t("settings.toast.defaultsRestored"), "success");
}

/** 清除聊天数据：二次确认走**应用内弹窗**。
 *  ⚠️ 原实现用 `window.confirm` —— 那是 WebView 的系统对话框，样式与 App 完全脱节，
 *  在无边框窗口里尤其突兀；HIG 也要求破坏性操作使用与 App 一致的对话样式并讲清后果。 */
const clearConfirmOpen = ref(false);

async function doClearAllData() {
  clearConfirmOpen.value = false;
  try {
    await chat.clearAllData();
    await chat.refreshFriends();
    await chat.refreshPending();
    app.toast(t("settings.toast.chatCleared"), "success");
  } catch (e) {
    app.toastError(e, t("settings.toast.clearFail"));
  }
}
</script>

<template>
  <div class="space-y-5">
    <SettingsGroup :title="t('settings.group.reset')">
      <button
        class="flex w-full items-center justify-center gap-2 px-4 py-3 text-sm text-[var(--gosslan-text)] transition hover:bg-[var(--gosslan-hover)]"
        @click="confirmRestore = true"
      >
        <RotateCcw class="h-4 w-4" />
        {{ t("settings.reset.restore") }}
      </button>
      <div class="ml-4 h-px bg-[var(--gosslan-divider)]" />
      <button
        class="flex w-full items-center justify-center gap-2 px-4 py-3 text-sm text-[var(--gosslan-danger-ink)] transition hover:bg-[var(--gosslan-danger-soft)] dark:hover:bg-[var(--gosslan-danger-soft)]"
        @click="clearConfirmOpen = true"
      >
        <Trash2 class="h-4 w-4" />
        {{ t("settings.reset.clearChat") }}
      </button>
    </SettingsGroup>
    <p class="px-1 text-center text-[11px] leading-relaxed text-[var(--gosslan-text-2)]">
      {{ t("settings.reset.footnote") }}
    </p>

    <!-- 恢复默认：会连带重置昵称/头像并广播给好友，必须确认 -->
    <BaseModal
      :open="confirmRestore"
      :title="t('settings.restore.title')"
      @close="confirmRestore = false"
    >
      <div class="space-y-3">
        <p class="text-sm text-[var(--gosslan-text)]">{{ t("settings.restore.warning") }}</p>
        <ul class="space-y-1 text-xs text-[var(--gosslan-text-2)]">
          <li>· {{ t("settings.restore.item.profile") }}</li>
          <li>· {{ t("settings.restore.item.appearance") }}</li>
          <li>· {{ t("settings.restore.item.network") }}</li>
        </ul>
        <p class="text-xs text-[var(--gosslan-text-2)]">{{ t("settings.restore.note") }}</p>
        <div class="flex justify-end gap-2 pt-2">
          <button
            class="rounded-[var(--gosslan-radius-md)] px-4 py-1.5 text-sm transition hover:bg-[var(--gosslan-hover)]"
            @click="confirmRestore = false"
          >{{ t("common.cancel") }}</button>
          <button
            class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-4 py-1.5 text-sm text-white transition hover:bg-[var(--gosslan-danger)]"
            @click="restoreDefaults"
          >{{ t("settings.reset.restore") }}</button>
        </div>
      </div>
    </BaseModal>

    <!-- 清除聊天数据：破坏性操作，逐条讲清"删什么 / 不删什么"，再给红色确认键 -->
    <BaseModal :open="clearConfirmOpen" :title="t('settings.clear.title')" @close="clearConfirmOpen = false">
      <div class="space-y-3">
        <p class="text-sm text-[var(--gosslan-text)]">{{ t("settings.clear.warning") }}</p>
        <ul class="space-y-1 text-xs text-[var(--gosslan-text-2)]">
          <li>· {{ t("settings.clear.item.messages") }}</li>
          <li>· {{ t("settings.clear.item.transfers") }}</li>
          <li>· {{ t("settings.clear.item.cache") }}</li>
          <li>· {{ t("settings.clear.item.groups") }}</li>
        </ul>
        <p class="text-sm text-[var(--gosslan-text)]">{{ t("settings.clear.unaffected") }}</p>
        <ul class="space-y-1 text-xs text-[var(--gosslan-text-2)]">
          <li>· {{ t("settings.clear.item.friends") }}</li>
          <li>· {{ t("settings.clear.item.identity") }}</li>
          <li>· {{ t("settings.clear.item.profile") }}</li>
          <li>· {{ t("settings.clear.item.otherDevices") }}</li>
        </ul>
        <p class="text-xs text-[var(--gosslan-text-2)]">{{ t("settings.clear.note") }}</p>
        <div class="flex justify-end gap-2 pt-2">
          <button
            class="rounded-[var(--gosslan-radius-md)] px-4 py-1.5 text-sm transition hover:bg-[var(--gosslan-hover)]"
            @click="clearConfirmOpen = false"
          >{{ t("common.cancel") }}</button>
          <button
            class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-4 py-1.5 text-sm text-white transition hover:bg-[var(--gosslan-danger)]"
            @click="doClearAllData"
          >{{ t("common.clear") }}</button>
        </div>
      </div>
    </BaseModal>
  </div>
</template>
