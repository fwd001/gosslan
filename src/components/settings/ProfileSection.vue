<script setup lang="ts">
import { ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import { avatarInitial, avatarInitialLen, nameToColor } from "@/utils/color";
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
/**
 * 设备信息是**异步**到位的（`app.init()` / `updateProfile()` / 「恢复默认」都会改它），
 * 只在 active/reloadToken 变化时同步会漏掉这些时刻 —— 独立设置窗口一开场
 * 就会把空昵称、null 头像写进 ref，之后再不更新（用户看到"名字和头像不对劲"）。
 * 用户**正在输入**时 device 不会变（保存后才变），因此这里不会覆盖未保存的编辑。
 */
watch(() => [app.device?.nickname, app.device?.avatar], () => {
  if (props.active) syncFromDevice();
});

/**
 * 落库并让 store 成为唯一真相：`app.updateProfile` 的返回值就是新的 `device`，
 * 上面那条 device 字段的 watch 会把两个 ref 重同步成**后端确认过的值**（含截断/改名）。
 * "没变化就不发"这条保留：省一次广播，也避免每次失焦都给一条"已保存"。
 */
async function persistProfile(name: string) {
  if (name === app.device?.nickname && avatar.value === app.device?.avatar) return;
  await app.updateProfile(name, avatar.value);
  await chat.refreshFriends();
  app.toast(t("settings.profile.toast.saved"), "success");
}

/** 昵称：失焦或回车即保存（即点即存，无「保存」按钮）。 */
async function saveProfileNow() {
  const name = nickname.value.trim();
  if (!name) {
    app.toast(t("settings.profile.toast.nicknameEmpty"), "error");
    nickname.value = app.device?.nickname ?? "";
    return;
  }
  await persistProfile(name);
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
    const next = await processAvatar(f);
    const prev = avatar.value;
    avatar.value = next; // 乐观：先让用户看见换成了这张
    try {
      // ⚠️ **不复用 `saveProfileNow()`**（用户 #24 的根因）：那条对"昵称为空"是提前 return，
      // 于是"输入框恰好被清空 + 点头像"会让这次上传**被静默丢掉** —— 页面显示新头像、
      // 后端里还是旧的，切个页就弹回去，正是"显示成功但没真成功"。
      // 昵称为空是**另一个字段的未保存编辑**，头像该以库里现有的昵称为准一起提交。
      const name = nickname.value.trim() || app.device?.nickname || "";
      if (!name) {
        // 连库里都没有昵称（设备信息还没到位）⇒ 明确拒绝，不发一个空昵称出去
        avatar.value = prev;
        app.toast(t("settings.profile.toast.nicknameEmpty"), "error");
        return;
      }
      await persistProfile(name);
    } catch (e) {
      // 保存失败必须**回滚**：不回滚的话页面上挂着的是一个从没落库的头像
      avatar.value = prev;
      app.toastError(e, t("settings.profile.toast.avatarFail"));
    }
  } catch {
    // 图片本身读不出来（解码/画布失败）：这时还没动过 avatar，不需要回滚
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
        class="gosslan-avatar-box group relative h-16 w-16 shrink-0 overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
        :style="{ backgroundColor: nameToColor(nickname) }"
        :title="t('settings.profile.changeAvatar')"
        :aria-label="t('settings.profile.changeAvatar')"
        @click="avatarInput?.click()"
      >
        <img alt="" v-if="avatar" :src="avatar" class="h-full w-full object-cover" />
        <span
          v-else
          class="gosslan-avatar-initial text-2xl font-semibold"
          :data-len="avatarInitialLen(nickname)"
          >{{ avatarInitial(nickname) }}</span
        >
        <span
          class="hover-reveal-op absolute inset-0 flex items-center justify-center bg-[var(--gosslan-danger)] opacity-0 transition group-hover:opacity-100"
        >
          <Camera class="h-5 w-5" />
        </span>
      </button>

      <div class="min-w-0 flex-1">
        <!-- 焦点提示**透明**（与应用其它字段同一套：`focus:border-transparent`）。
             `border border-transparent` 是常驻的 1px 边框（静止时透明）——不能等聚焦时才加边框，
             否则聚焦那一下字段尺寸会变、里面的文字跟着跳。
             用户 2026-09-16 明确不要聚焦色，原本的 `focus:ring-2 focus:ring-primary` 实心方框、
             以及全局焦点环那条外圈方框都已去掉。 -->
        <input
          v-model="nickname"
          maxlength="40"
          class="w-full rounded-[var(--gosslan-radius-md)] border border-transparent bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none transition focus:border-transparent"
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
            <span class="h-2 w-2 rounded-full" :class="app.present ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-status-offline)]'"></span>
            {{ app.present ? t("settings.profile.online") : t("settings.profile.offline") }}
          </span>
        </div>
      </div>
    </div>
    <input ref="avatarInput" type="file" accept="image/*" class="hidden" @change="onAvatarChange" />
  </SettingsGroup>
</template>
