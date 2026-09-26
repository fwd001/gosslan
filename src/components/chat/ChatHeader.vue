<script setup lang="ts">
import {
  ArrowUpCircle,
  FolderOpen,
  ListChecks,
  Loader2,
  Monitor,
  Pencil,
  Smartphone,
  Users,
} from "lucide-vue-next";
import BackArrow from "@/components/ui/BackArrow.vue";
import UnreadBadge from "@/components/UnreadBadge.vue";
import { computed } from "vue";
import LinkIcon from "@/components/ui/LinkIcon.vue";
import { linkIconName, type LinkIconName } from "@/utils/peerConnectionInfo";
import type { Conversation, LinkState } from "@/types";
import { t } from "@/i18n";

const props = defineProps<{
  conv: Conversation | null;
  isGroup: boolean;
  online: boolean;
  memberCount: number;
  /** 单聊对方的设备类型（"desktop" / "mobile"，空串 = 未知/不显示）。 */
  deviceType?: string;
  /**
   * 对方 Gosslan 的线格式版本比本机高（INV-P24 的"可解释状态"）。
   *
   * 判定**不在这里做**：结论由后端 `protocol::peer_protocol_is_newer` 算好随好友记录带下来。
   * 头部只放一个不抢戏的标记（说明在联系人详情里整句写），对方没报版本时一定是 false。
   */
  peerVersionNewer?: boolean;
  /** 会话当前链路（最近一条消息的链路 + 跳数）。单聊显示。 */
  linkState?: LinkState | null;
  /** 仅群主可改名。 */
  canRename: boolean;
  /** 移动端：显示返回列表的箭头。 */
  showBack?: boolean;
  /**
   * 「外面还有多少条没看」= 未读总数（用户 2026-09-24：进聊天页后 TabBar 隐藏，
   * 返回箭头上要提示别的新消息）。
   *
   * 数字**不在头部算**：调用方传 `chat.totalUnread`，与底部 TabBar、桌面导航栏、
   * 托盘/Dock 角标共用同一份判据（`channelState.test.ts` 钉过这条）。
   */
  unreadTotal?: number;
  /** 群任务窗口正在打开（按钮 pending 反馈；桌面端开独立窗口时才可能为真）。 */
  tasksOpening?: boolean;
  /** 「与我相关的未完成任务」数（蓝色徽标）。0 就不画；判定在 store 的 `openTodoByConv`。 */
  openTasks?: number;
}>();
const emit = defineEmits<{
  (e: "back"): void;
  (e: "open-members"): void;
  (e: "open-files"): void;
  (e: "open-tasks"): void;
  (e: "rename"): void;
  (e: "open-share"): void;
}>();

const unreadOutside = computed(() => props.unreadTotal ?? 0);
/** 蓝色徽标的数字。与会话列表那枚同源（`chat.openTodoByConv`），不在这再算一遍。 */
const openTasks = computed(() => props.openTasks ?? 0);
const backLabel = computed(() =>
  unreadOutside.value > 0
    ? t("chat.header.backUnread", { n: unreadOutside.value })
    : t("chat.header.back"),
);

/** 连接图标名走 `peerConnectionInfo` 的唯一判据（与好友列表/资料页同源）；文案仍用聊天头自己那套 key。 */
function linkIcon(path: string, hop: number): { icon: LinkIconName; label: string } {
  const icon = linkIconName({ link: path, hop });
  let label: string;
  if (hop > 0) label = t("chat.header.linkRelay", { n: hop });
  else if (path === "bluetooth") label = t("chat.header.linkBluetooth");
  else if (path === "relay") label = t("chat.header.linkRelayServer");
  else if (path === "routed") label = t("chat.header.linkRouted");
  else label = t("chat.header.linkLan");
  return { icon, label };
}
</script>

<template>
  <!-- 微信 4.0 头部：~56px 浅灰底，与消息区无缝衔接，无分割线；右侧 通话 / 更多 / 共享 -->
  <div
    class="flex shrink-0 items-center justify-between border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-chat)] px-4"
    :style="{ height: 'var(--gosslan-header-h)' }"
  >
    <div class="flex min-w-0 items-center gap-1.5">
      <button
        v-if="showBack"
        class="tap-safe relative -ml-2 mr-1 flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="backLabel"
        :aria-label="backLabel"
        @click="emit('back')"
      >
        <BackArrow />
        <!-- 徽标用全应用唯一的 `UnreadBadge`（designGuards 禁止再手写一份），
             挂点由调用方给 —— 组件自己的注释明令不要把定位写进去。 -->
        <UnreadBadge v-if="unreadOutside > 0" :count="unreadOutside" class="absolute -right-1.5 -top-1" />
      </button>
      <span class="truncate text-[15px] font-medium leading-6" :title="conv?.name || t('chat.header.conversation')">{{ conv?.name || t("chat.header.conversation") }}<template v-if="isGroup && memberCount > 0"> ({{ memberCount }})</template></span>
      <span
        v-if="!isGroup && deviceType"
        class="inline-flex shrink-0 items-center text-[var(--gosslan-text-2)]"
        :title="deviceType === 'mobile' ? t('chat.header.deviceMobile') : t('chat.header.deviceDesktop')"
        :aria-label="deviceType === 'mobile' ? t('chat.header.deviceMobile') : t('chat.header.deviceDesktop')"
      >
        <Smartphone v-if="deviceType === 'mobile'" class="h-4 w-4" />
        <Monitor v-else class="h-4 w-4" />
      </span>
      <!-- 对方版本比本机新：头部只给一个标记（完整说明在联系人详情），
           真正看不懂的那条消息由 `UnsupportedKindBubble` 就地解释 —— 三处口径一致。 -->
      <span
        v-if="!isGroup && peerVersionNewer"
        class="inline-flex shrink-0 items-center text-[var(--gosslan-warning-ink)]"
        :title="t('peer.newerShort')"
        :aria-label="t('peer.newerShort')"
      >
        <ArrowUpCircle class="h-4 w-4" />
      </span>
      <!-- 链路徽标只在**对方在线**时显示。
           用户 2026-09-12 反馈：「现在这个用户是离线的，但是聊天框后面居然有一个『桥接 1』
           的图标，这个是错误的。」根因：`conv_link` 是"最近一条消息走的路径"的**快照**，
           由发送侧乐观写入（无直连时记 hop=1），而对方离线后这个快照并不会自动消失 ⇒
           徽标把"历史路径"读成了"当前链路"。语义上它是**当前可达路径**，
           所以必须与 `online` 绑定（离线时路径不存在，什么都不该显示）。 -->
      <span
        v-if="!isGroup && online && linkState"
        class="inline-flex shrink-0 items-center gap-0.5 text-[var(--gosslan-text-2)]"
        :title="linkIcon(linkState.path, linkState.hop).label"
        :aria-label="linkIcon(linkState.path, linkState.hop).label"
      >
        <LinkIcon :name="linkIcon(linkState.path, linkState.hop).icon" class="h-4 w-4" />
        <span v-if="linkState.hop > 0" class="text-[11px] font-medium leading-none">{{ linkState.hop }}</span>
      </span>
    </div>
    <div class="flex shrink-0 items-center gap-0.5">
      <!-- 只保留有真实功能的入口；不放没有实现的功能按钮（音视频通话/更多已移除） -->
      <button
        v-if="isGroup"
        class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('chat.header.members')" :aria-label="t('chat.header.members')"
        @click="emit('open-members')"
      >
        <Users class="h-[18px] w-[18px]" />
      </button>
      <!-- 群文件：把该群共享过的文件汇总成一份清单（此前只能顺着聊天记录往回翻） -->
      <button
        v-if="isGroup"
        class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('chat.header.files')" :aria-label="t('chat.header.files')"
        @click="emit('open-files')"
      >
        <FolderOpen class="h-[18px] w-[18px]" />
      </button>
      <!-- 群任务：桌面端开独立窗口（每群一个），移动端开应用内面板。任意成员都能看，
           能改什么由面板按权限决定。 -->
      <button
        v-if="isGroup"
        class="tap-safe relative flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :aria-busy="tasksOpening"
        :title="t('chat.header.tasks')"
        :aria-label="openTasks > 0 ? t('todo.openForMe', { n: openTasks }) : t('chat.header.tasks')"
        @click="emit('open-tasks')"
      >
        <!-- 正在开独立窗口时**换成转圈**，不是把图标调暗：`opacity-60` 在一个 18px 的图标上
             几乎看不出来，而用户等的是"这一扇窗什么时候出来"（用户 2026-09-24：
             "弹窗弹出很慢，我还以为没点了，点了好几下一会才弹出来"）。
             连点会被启动器的单飞/防抖吃掉，所以"看起来没反应"必须靠这里补上。 -->
        <Loader2 v-if="tasksOpening" class="h-[18px] w-[18px] animate-spin" />
        <ListChecks v-else class="h-[18px] w-[18px]" />
        <!-- 与我相关的未完成任务（蓝）。数字同时进 aria-label：只靠颜色区分"有活没干完"
             对读屏用户等于没有提醒。 -->
        <UnreadBadge v-if="openTasks > 0" :count="openTasks" tone="info" class="absolute -right-1.5 -top-1" />
      </button>
      <button
        v-if="isGroup && canRename"
        class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('chat.header.rename')" :aria-label="t('chat.header.rename')"
        @click="emit('rename')"
      >
        <Pencil class="h-[17px] w-[17px]" />
      </button>
      <button
        v-if="!isGroup"
        class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('chat.header.share')" :aria-label="t('chat.header.share')"
        @click="emit('open-share')"
      >
        <FolderOpen class="h-[18px] w-[18px]" />
      </button>
    </div>
  </div>
</template>
