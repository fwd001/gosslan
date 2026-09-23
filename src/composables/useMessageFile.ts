import { t as $t } from "@/i18n";
import { computed, ref, toValue, watch, type MaybeRefOrGetter } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { api } from "@/api";
import { loadFilePreview } from "@/utils/filePreview";
import { parseFileMeta } from "@/utils/fileMeta";
import { openLocalFile, saveLocalFile } from "@/utils/localFile";
import { shouldProbePresence } from "@/utils/mediaAvailability";
import { isAndroid } from "@/utils/platform";
import { codeNeedsClamp } from "@/utils/previewMetrics";
import type { FileMeta, MessageRecord } from "@/types";

export type { FileMeta };

/** 文件消息的 msg_id 与 transfer_id 的映射：单聊 "file-{id}"，群文件 "gfile-{id}"。 */
function transferIdOf(msgId: string): string | null {
  if (msgId.startsWith("file-")) return msgId.slice(5);
  if (msgId.startsWith("gfile-")) return msgId.slice(6);
  return null;
}

/**
 * 文件/附件消息的派生状态：元信息、传输进度、本地预览、打开与另存。
 */
export function useMessageFile(
  message: MaybeRefOrGetter<MessageRecord>,
  sendState: MaybeRefOrGetter<string>,
) {
  const app = useAppStore();
  const chat = useChatStore();
  const msg = computed(() => toValue(message));

  const transfer = computed(() => {
    const id = transferIdOf(msg.value.msg_id);
    return id ? (chat.transfers.find((t) => t.id === id) ?? null) : null;
  });

  /** 乐观上屏的文件/图片气泡可能缺 size/path，用传输记录补齐。 */
  const fileMeta = computed<FileMeta | null>(() => {
    if (msg.value.kind !== "file" && msg.value.kind !== "image") return null;
    // 载荷解析走**唯一一份** `utils/fileMeta`（引用块也用同一份，见那里的说明）
    const raw = parseFileMeta(msg.value.content);
    if (!raw) return null;
    const t = transfer.value;
    return {
      name: raw.name || t?.name || $t("common.file"),
      path: raw.path || t?.path || "",
      size: raw.size || t?.size || 0,
      subtype: raw.subtype || (msg.value.kind === "image" ? "image" : "file"),
      sha256: raw.sha256,
    };
  });

  /** 进度 0~1；无记录（历史消息）返回 null 表示不显示进度条。 */
  const fileProgress = computed(() => {
    const t = transfer.value;
    if (!t || t.status === "done") return null;
    return t.progress;
  });
  const fileStatusText = computed(() => {
    const t = transfer.value;
    if (!t || t.status === "done") return null;
    // 链路停滞：后端按 writer **实发**判定（对端长时间没再收任何一片）。不显示出来的话，
    // 用户看到的是"进度条冻在 63%"，只能猜是软件死了 —— 而后端其实还在背压里等。
    // 停滞原因优先：「只有蓝牙链路、这个文件在等局域网」和「网络卡住了」是两件不同的事，
    // 说错会把用户引去查网络（而网络没问题）。后端没给原因时回退既有的通用文案。
    if (chat.isTransferStalled(t.id)) return chat.transferStallReason(t.id) ?? $t("send.stalled");
    const pct = Math.round((t.progress ?? 0) * 100);
    return t.direction === "send" ? $t("send.sendingPct", { pct }) : $t("send.receivingPct", { pct });
  });

  const attachmentUrl = ref<string | null>(null);
  const previewNote = ref<string | null>(null);
  /**
   * 本地媒体已被「存储清理」删除。
   * 用于把"坏图/打不开"换成明确的「已被清理」提示——否则用户会以为是对端发来的
   * 文件本身有问题，而不是本机为了腾空间删掉了它。
   * 只有后端能**确定**文件已删除时才为真（见 `api.mediaPresent` 的语义）。
   */
  const attachmentMissing = ref(false);

  /** 当 file/image 消息本地路径就绪、未失败、且为 image 时可预览。
   *  ⚠️ 刻意**不**为 `code` 子类型做内联预览：发送的文件就是文件（按文件卡片渲染），
   *  代码块只应来自「代码消息」（kind=code，输入框粘贴/发送的文本），而不是把 .js/.py
   *  这类文件内容拉出来渲染成代码块（用户反馈：复制 md 文件发送不应变成代码块）。 */
  const previewSubtype = computed<"image" | null>(() => {
    const meta = fileMeta.value;
    if ((msg.value.kind !== "file" && msg.value.kind !== "image") || !meta || !meta.path) return null;
    if (toValue(sendState) === "failed") return null;
    return meta.subtype === "image" ? "image" : null;
  });

  /**
   * 普通文件（subtype=file）没有"读预览"这条路径，因此拿不到可达性信号，
   * 需要单独问一次后端文件还在不在。图片附件由预览读取代劳，不重复探测。
   */
  const needsPresenceProbe = computed(() =>
    shouldProbePresence(msg.value.kind, fileMeta.value),
  );

  async function ensureAttachmentPreview() {
    const sub = previewSubtype.value;
    const meta = fileMeta.value;
    if (sub) {
      const r = await loadFilePreview(msg.value.msg_id, sub, meta?.name ?? "");
      // await 期间该气泡可能已不满足预览条件（切换/失败）→ 丢弃，避免贴到错误气泡。
      if (previewSubtype.value !== sub) return;
      attachmentUrl.value = r.url ?? null;
      previewNote.value = r.note ?? null;
      attachmentMissing.value = r.missing === true;
      return;
    }
    attachmentUrl.value = null;
    previewNote.value = null;
    attachmentMissing.value = false;
    if (!needsPresenceProbe.value) return;

    const msgId = msg.value.msg_id;
    const present = await api.mediaPresent(msgId);
    // 同上：期间消息可能已被替换（乐观 → 真实），只在仍指向同一条时落地。
    if (!needsPresenceProbe.value || msg.value.msg_id !== msgId) return;
    attachmentMissing.value = !present;
    if (!present) previewNote.value = $t("msg.cleaned");
  }

  // 显式读 path/name/subtype：Vue 对 watcher 返回的数组做浅比对，若源里不包含这些字段的读取，
  // optimistic({path:""})→real({path:"/…",name:"x"}) 替换时 msg_id+subtype 不变 → 不触发。
  watch(
    () =>
      [
        previewSubtype.value,
        needsPresenceProbe.value,
        msg.value.msg_id,
        fileMeta.value?.path,
        fileMeta.value?.name,
        // 传输状态：收到 Done 时（字节刚落盘）必须重读预览，否则在途时读到的
        // "仍在接收"会把图片钉死在"加载失败/被清理"（真机 2026-09-14）。
        transfer.value?.status,
      ] as const,
    () => void ensureAttachmentPreview(),
    { immediate: true },
  );

  /** 消息流里的代码：只来自 inline code 消息（kind=code，输入框粘贴/发送的文本）。
   *  文件消息不再渲染成代码块——发送的文件就按文件卡片显示。 */
  const streamCode = computed<string | null>(() =>
    msg.value.kind === "code" ? msg.value.content : null,
  );
  const streamCodeClamped = computed(() =>
    streamCode.value ? codeNeedsClamp(streamCode.value) : false,
  );

  /**
   * 文件是否已就绪可打开：本地路径非空**且文件确实还在**
   * （发送方＝源文件；接收方＝传输完成落盘，路径经 file_transfers 持久化）。
   * 只看路径是不够的——「存储清理」删掉文件后消息里的 path 仍然存在，
   * 这时必须显示「已被清理」，而不是让用户点开才发现打不开。
   */
  const fileReady = computed(() => !!fileMeta.value?.path && !attachmentMissing.value);

  /**
   * 这条文件消息是否本机发出。
   *
   * Android 上「自己发的文件」点一下必须**没有任何响应**（用户明确要求）；
   * 长按气泡弹出的菜单不受影响，仍可另存/复制。
   */
  const isMineFile = computed(
    () => !!app.device?.device_id && msg.value.sender_id === app.device.device_id,
  );
  /**
   * 文件卡片是否可点开。
   *
   * - Android：自己发的文件 ⇒ 不可点（点击无响应）；收到的文件 ⇒ 可点（弹另存为）。
   * - 桌面端：保持原样（点开本地文件），不做这个限制。
   * 与 fileReady 分开：fileReady 还管"下载按钮/已被清理"的显示，不能被这里连累。
   */
  const fileTappable = computed(() => !(isAndroid && isMineFile.value));

  async function openFile() {
    // 点自己发的文件：静默返回（连 toast 都不给，避免"看起来有反应"）。
    if (!fileTappable.value) return;
    const path = fileMeta.value?.path;
    const name = fileMeta.value?.name;
    if (!path || !name) {
      app.toast($t("msg.filePathUnavailable"), "error");
      return;
    }
    // 平台差异（Android 改走另存为）统一在 utils/localFile 里，与群文件面板同一条路径。
    try {
      await openLocalFile(path, name);
    } catch (e) {
      app.toastError(e, $t("msg.openFileFail"));
    }
  }

  async function saveAs() {
    const source = fileMeta.value?.path;
    const filename = fileMeta.value?.name;
    if (!source || !filename) {
      app.toast($t("msg.filePathUnavailable"), "error");
      return;
    }
    try {
      const r = await saveLocalFile(source, filename);
      if (r === "done") app.toast($t("msg.fileSaved"), "success");
    } catch (e) {
      app.toastError(e, $t("msg.saveFileFail"));
    }
  }

  return {
    transfer,
    fileMeta,
    fileReady,
    fileTappable,
    fileProgress,
    fileStatusText,
    attachmentUrl,
    attachmentMissing,
    previewNote,
    previewSubtype,
    streamCode,
    streamCodeClamped,
    openFile,
    saveAs,
  };
}
