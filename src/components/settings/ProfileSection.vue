<script setup lang="ts">
import { ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import { avatarInitial, nameToColor } from "@/utils/color";
import { Camera } from "lucide-vue-next";
import { t } from "@/i18n";

const props = defineProps<{ active: boolean; reloadToken?: number }>();

const app = useAppStore();
const chat = useChatStore();

const nickname = ref("");
const avatar = ref<string | null>(null);
const avatarInput = ref<HTMLInputElement | null>(null);

function syncFromDevice() {
  nickname.value = app.device?.nickname ?? "";
  avatar.value = app.device?.avatar ?? null;
}
watch(() => [props.active, props.reloadToken], () => {
  if (props.active) syncFromDevice();
}, { immediate: true });

/** 昵称：失焦或回车即保存（即点即存，无「保存」按钮）。 */
async function saveProfileNow() {
  const name = nickname.value.trim();
  if (!name) {
    app.toast(t("settings.profile.toast.nicknameEmpty"), "error");
    nickname.value = app.device?.nickname ?? "";
    return;
  }
  if (name === app.device?.nickname && avatar.value === app.device?.avatar) return;
  await app.updateProfile(name, avatar.value);
  await chat.refreshFriends();
  app.toast(t("settings.profile.toast.saved"), "success");
}

function onNicknameKeydown(e: KeyboardEvent) {
  if (e.key === "Enter") {
    e.preventDefault();
    (e.target as HTMLInputElement).blur();
  }
}

/** 头像文件大小上限（2MB，原始文件、预处理前校验）。 */
const MAX_AVATAR_FILE_SIZE = 2 * 1024 * 1024;
/** 头像输出边长：中心裁剪成正方形后缩放到该尺寸（不放大），PNG 体积小且清晰。 */
const AVATAR_SIZE = 512;

async function onAvatarChange(e: Event) {
  const input = e.target as HTMLInputElement;
  const f = input.files?.[0];
  input.value = ""; // 清空，允许重复选择同一文件
  if (!f) return;
  if (!f.type.startsWith("image/")) {
    app.toast(t("settings.profile.toast.notImage"), "error");
    return;
  }
  if (f.size > MAX_AVATAR_FILE_SIZE) {
    app.toast(t("settings.profile.toast.avatarTooLarge"), "error");
    return;
  }
  try {
    avatar.value = await processAvatar(f);
    await saveProfileNow();
  } catch {
    app.toast(t("settings.profile.toast.avatarFail"), "error");
  }
}

/**
 * 中心裁剪为正方形 → 缩放到 AVATAR_SIZE（不放大）→ 导出 PNG data URL（无损）。
 * 裁剪规则：上下长取宽度、左右长取高度，即以较短边为基准、居中裁成正方形。
 */
function processAvatar(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(file);
    const img = new Image();
    img.onload = () => {
      try {
        const side = Math.min(img.naturalWidth, img.naturalHeight);
        const sx = (img.naturalWidth - side) / 2;
        const sy = (img.naturalHeight - side) / 2;
        const out = Math.min(side, AVATAR_SIZE);
        const canvas = document.createElement("canvas");
        canvas.width = out;
        canvas.height = out;
        const ctx = canvas.getContext("2d");
        if (!ctx) throw new Error("no 2d context");
        ctx.drawImage(img, sx, sy, side, side, 0, 0, out, out);
        URL.revokeObjectURL(url);
        resolve(canvas.toDataURL("image/png"));
      } catch (err) {
        URL.revokeObjectURL(url);
        reject(err);
      }
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("image load failed"));
    };
    img.src = url;
  });
}
</script>

<template>
  <SettingsGroup :title="t('settings.group.profile')">
    <div class="flex items-center gap-4 p-4">
      <!-- 头像：点击更换 -->
      <button
        class="group relative h-16 w-16 shrink-0 overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
        :style="{ backgroundColor: nameToColor(nickname) }"
        :title="t('settings.profile.changeAvatar')"
        :aria-label="t('settings.profile.changeAvatar')"
        @click="avatarInput?.click()"
      >
        <img alt="" v-if="avatar" :src="avatar" class="h-full w-full object-cover" />
        <span v-else class="text-2xl font-semibold">{{ avatarInitial(nickname) }}</span>
        <span
          class="hover-reveal-op absolute inset-0 flex items-center justify-center bg-black/40 opacity-0 transition group-hover:opacity-100"
        >
          <Camera class="h-5 w-5" />
        </span>
      </button>

      <div class="min-w-0 flex-1">
        <input
          v-model="nickname"
          maxlength="40"
          class="w-full rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none transition focus:ring-2 focus:ring-primary"
          :placeholder="t('settings.profile.nickname.placeholder')"
          @blur="saveProfileNow"
          @keydown="onNicknameKeydown"
        />
        <div class="mt-2.5 flex items-center justify-between">
          <button
            class="rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-3 py-1.5 text-xs transition hover:bg-[var(--gosslan-hover)]"
            @click="avatarInput?.click()"
          >
            {{ t("settings.profile.changeAvatar") }}
          </button>
          <span class="flex items-center gap-1.5 text-xs text-[var(--gosslan-text-2)]">
            <span class="h-2 w-2 rounded-full" :class="app.online ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-status-offline)]'"></span>
            {{ app.online ? t("settings.profile.online") : t("settings.profile.offline") }}
          </span>
        </div>
      </div>
    </div>
    <input ref="avatarInput" type="file" accept="image/*" class="hidden" @change="onAvatarChange" />
  </SettingsGroup>
</template>
