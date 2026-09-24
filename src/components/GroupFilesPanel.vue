<script setup lang="ts">
/**
 * 群文件面板：把该群**全部**群文件汇总成一份清单（钉钉/飞书「群文件」的等价物）。
 *
 * 为什么需要它：群文件以前只能顺着聊天记录往回翻——一条 `gfile-` 消息滑上去就找不到了。
 * 而后端的 `group_files` 表本来就完整存着每个群文件的元数据（名称/大小/发送者/时间）
 * 与每个成员的投递状态，只是从来没有一个入口把它列出来。
 *
 * 数据来源：`list_group_files` 命令（读 `group_files` + `group_file_recipients` + 文件系统）。
 * ⚠️ 这张表**不随「删除聊天记录」清空**（同钉盘语义：群文件是群资产，不是聊天记录），
 *    所以本面板在清空聊天历史后依然完整。
 */
import { t } from "@/i18n";
import { computed, ref, watch } from "vue";
import { api } from "@/api";
import { useAppStore } from "@/stores/useAppStore";
import { useMemberProfile } from "@/composables/useMemberProfile";
import BaseModal from "@/components/BaseModal.vue";
import { fmtConversationTime } from "@/utils/time";
import { humanSize, rgba } from "@/utils/color";
import { FILE_KIND_COLORS, FILE_KIND_ICONS, fileExt, fileKindOf } from "@/utils/fileKind";
import { openLocalFile } from "@/utils/localFile";
import { Loader2, RotateCw } from "lucide-vue-next";
import type { GroupFileEntry } from "@/types";

const props = defineProps<{ open: boolean; groupId: string | null }>();
const emit = defineEmits<{ (e: "close"): void }>();

const app = useAppStore();
const { memberProfile, myId } = useMemberProfile();

const files = ref<GroupFileEntry[]>([]);
const loading = ref(false);
const failed = ref(false);

async function load() {
  if (!props.groupId) return;
  const gid = props.groupId;
  loading.value = true;
  failed.value = false;
  try {
    const list = await api.listGroupFiles(gid);
    // 慢响应过期守卫：期间换了群就不许把 A 群的清单写进 B 群的面板
    // （与 4.1-7 同一条链上的另一半 —— 只补 watch 不补这里，切群瞬间的旧响应仍会回填）。
    if (gid !== props.groupId) return;
    files.value = list;
  } catch {
    if (gid !== props.groupId) return;
    failed.value = true;
    files.value = [];
  } finally {
    if (gid === props.groupId) loading.value = false;
  }
}

// 每次打开都重拉：面板不在时可能刚收完一个文件（状态会变），
// 缓存一份旧快照会让用户看到"未下载"却其实早就到了。
watch(
  () => props.open,
  (v) => {
    if (v) void load();
  },
);

// ⚠️ 必须 watch groupId（审计阶段 4 · 4.1-7）：面板开着时 activeConv 会被**通知点击**
// 这类程序化路径换掉（`handleNotificationClick` → `openConversation`），而 `groupId` 变了
// 上面的 open 判据根本不会再触发 ⇒ 标题/总数/整张清单全是上一个群的。
// 这里清空而不是静默留着旧数据：宁可显示"加载中/空"，也不能让用户把 A 群的文件当成 B 群。
watch(
  () => props.groupId,
  (v, old) => {
    if (v === old) return;
    files.value = [];
    failed.value = false;
    if (v && props.open) void load();
  },
);

/** 每项的文件类型与配色 —— 与文件气泡同一套（utils/fileKind），保证同一种文件在
 *  列表和气泡里长得一样。 */
function kindIcon(name: string) {
  return FILE_KIND_ICONS[fileKindOf(name)];
}
function kindStyle(name: string) {
  const c = FILE_KIND_COLORS[fileKindOf(name)];
  const accent = app.dark ? c.dark : c.light;
  return { backgroundColor: rgba(accent, 0.12), color: accent };
}
function extLabel(name: string) {
  const e = fileExt(name);
  return e ? e.toUpperCase().slice(0, 4) : t("common.file");
}

/** 本机持有状态 → 可读文案。与 sender 无关：这是**我能不能打开它**。 */
const STATE_TEXT: Record<GroupFileEntry["local_state"], string> = {
  local: "group.files.stateLocal",
  receiving: "group.files.stateReceiving",
  remote: "group.files.stateRemote",
  failed: "group.files.stateFailed",
};

/** 发送者昵称。自己的文件也照常显示：面板里混着多人发的文件，逐行标出归属更好认。 */
function senderLabel(id: string): string {
  return memberProfile(id).name;
}

/**
 * 只有**别人发的**且本机没有的文件才谈得上「重新获取」。
 *
 * 自己发的文件本机没有被取到时，`request_content` 会以 `sender_id = 我自己`
 * 发起请求 —— 那是在向自己索要文件，没有任何端点会应答。所以自己的文件不提供该入口。
 */
function canRefetch(f: GroupFileEntry): boolean {
  return f.sender_id !== myId.value && (f.local_state === "remote" || f.local_state === "failed");
}

/** 我发出的文件显示「已送达 N/M」：群文件最常被追问的就是"大家都收到了吗"。 */
function deliveryText(f: GroupFileEntry): string | null {
  if (f.sender_id !== myId.value || f.total === 0) return null;
  return t("group.files.delivered", { n: f.delivered, total: f.total });
}

async function openFile(f: GroupFileEntry) {
  if (!f.local_path) return;
  // 平台差异（Android 改走另存为）统一在 utils/localFile 里，与消息气泡同一条路径。
  try {
    await openLocalFile(f.local_path, f.name);
  } catch (e) {
    app.toastError(e, t("msg.openFileFail"));
  }
}

/**
 * 未取到的文件：按 cid 向**原发送者**重新拉一份。
 *
 * 直接复用消息气泡既有的 `request_content`（ADR-0019「拥有即授权」）——
 * 群文件的 msg_id 恒为 `gfile-{transfer_id}`，因此面板只凭一行元数据就能构造出
 * 与气泡完全相同的请求，不需要为群文件另开一条重取路径。
 */
async function refetch(f: GroupFileEntry) {
  try {
    const ok = await api.requestContent(f.sender_id, `gfile-${f.transfer_id}`);
    app.toast(
      t(ok ? "group.files.refetching" : "group.files.refetchUnsupported"),
      ok ? "info" : "error",
    );
  } catch (e) {
    app.toastError(e, t("group.files.refetchFail"));
  }
}

const totalSize = computed(() => files.value.reduce((s, f) => s + f.size, 0));

/** `t()` 的 key 是静态字面量，才能在字典里被静态检查到；因此用查表而不是拼 key。 */
function stateText(state: GroupFileEntry["local_state"]): string {
  return t(STATE_TEXT[state]);
}
</script>

<template>
  <BaseModal
    :open="open"
    :title="files.length ? t('group.files.titleCount', { n: files.length }) : t('group.files.title')"
    width="max-w-lg"
    @close="emit('close')"
  >
    <div class="space-y-3">
      <div v-if="files.length" class="text-xs text-[var(--gosslan-text-2)]">
        {{ t("group.files.totalSize", { n: files.length, size: humanSize(totalSize) }) }}
      </div>

      <div v-if="loading" class="flex items-center justify-center gap-2 py-8 text-sm text-[var(--gosslan-text-2)]">
        <Loader2 class="h-4 w-4 animate-spin" />
        {{ t("group.files.loading") }}
      </div>

      <div v-else-if="failed" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">
        {{ t("group.files.loadFail") }}
      </div>

      <div v-else-if="files.length === 0" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">
        {{ t("group.files.empty") }}
      </div>

      <div v-else class="max-h-80 space-y-0.5 overflow-y-auto">
        <div
          v-for="f in files"
          :key="f.transfer_id"
          class="flex items-center gap-2.5 rounded-[var(--gosslan-radius-md)] px-2 py-2 transition"
          :class="f.local_path ? 'cursor-pointer hover:bg-[var(--gosslan-hover)]' : ''"
          :role="f.local_path ? 'button' : undefined"
          :tabindex="f.local_path ? 0 : undefined"
          :title="f.local_path ? t('msg.clickToOpen') : undefined"
          @click="openFile(f)"
          @keydown.enter.prevent="openFile(f)"
          @keydown.space.prevent="openFile(f)"
        >
          <div
            class="flex h-9 w-9 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-md)]"
            :style="kindStyle(f.name)"
          >
            <component :is="kindIcon(f.name)" class="h-[18px] w-[18px]" />
          </div>

          <div class="min-w-0 flex-1">
            <div class="truncate text-[13px] font-medium" :title="f.name">{{ f.name }}</div>
            <div class="mt-0.5 flex items-center gap-1 text-[11px] text-[var(--gosslan-text-2)]">
              <span>{{ extLabel(f.name) }}</span>
              <span>·</span>
              <span>{{ humanSize(f.size) }}</span>
              <span>·</span>
              <span class="truncate" :title="senderLabel(f.sender_id)">{{ senderLabel(f.sender_id) }}</span>
              <span>·</span>
              <span class="shrink-0">{{ fmtConversationTime(f.created_at) }}</span>
              <template v-if="deliveryText(f)">
                <span>·</span>
                <span class="shrink-0">{{ deliveryText(f) }}</span>
              </template>
            </div>
          </div>

          <!-- 右侧状态：已在本机 / 传输中 / 未取到（可点重取）/ 失败 -->
          <span
            v-if="f.local_state !== 'local'"
            class="flex shrink-0 items-center gap-1 text-[11px] text-[var(--gosslan-text-2)]"
          >
            <Loader2 v-if="f.local_state === 'receiving'" class="h-3 w-3 animate-spin" />
            {{ stateText(f.local_state) }}
          </span>
          <button
            v-if="canRefetch(f)"
            class="tap-safe flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
            :title="t('group.files.refetch')"
            :aria-label="t('group.files.refetch')"
            @click.stop="refetch(f)"
          >
            <RotateCw class="h-3.5 w-3.5" />
          </button>
        </div>
      </div>
    </div>
  </BaseModal>
</template>
