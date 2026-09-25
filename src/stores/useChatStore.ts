import { acceptHMRUpdate, defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import { api, bindEvents } from "@/api";
import {
  applyConversationSnapshot,
  applyIncomingToConversations,
  applyReplacements,
  appendLocalOnly,
  furthestStatus,
  mergeMessages,
  messageMentionsAll,
  messageMentionsName,
  pickMediaContent,
  preserveDeliveryStatus,
  previewText,
  pruneUnreadClears,
  selectCachedConversations,
  sortConversations,
  syncProfileFromPeers,
  unreadAnchorIndex,
} from "@/utils/messages";
import { useAppStore } from "@/stores/useAppStore";
import { trimOldest } from "@/utils/bounded";
import { createInitScope, type InitScope } from "@/utils/initScope";
import { StaleGuard } from "@/utils/staleGuard";
import { actionableRequests } from "@/utils/friendRequests";
import { mergeNoticesInto, notificationBody, type QueuedNotice } from "@/utils/notifications";
import { isRenderedInTimeline, countsTowardUnread } from "@/utils/messageKinds";
import { todoCompletedForCreator, todoMentionsMe, type TodoImage } from "@/utils/todos";
import { invalidateFilePreview } from "@/utils/filePreview";
import { t } from "@/i18n";
import { shouldRunThrottled } from "@/utils/defer";
import { mergePeerList } from "@/utils/peerMerge";
import {
  onAction,
  registerActionTypes,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import type {
  Conversation,
  FileDoneInfo,
  FileFailedInfo,
  FileProgress,
  FileStalledInfo,
  FavoriteEntry,
  Friend,
  Group,
  GroupReadInfo,
  MessageRecord,
  Peer,
  PendingRequest,
  TopologyInfo,
  ContentTransfer,
  TransferInfo,
} from "@/types";

/** 上次打开的会话（重启后恢复，纯前端 UI 状态，各端统一）。 */
const LAST_CONV_KEY = "gosslan.lastConv";

/**
 * 上一轮 `init()` 的注册容器。
 *
 * 必须是**模块作用域**而不是 store 状态：`acceptHMRUpdate` 会换一个全新的 store
 * 实例（新实例的 `setup` 里没有任何旧实例注册的痕迹），而旧实例的 `listen()`
 * 仍挂在 Tauri 事件总线上 —— 只有模块级变量能让"新一轮 init"找到并拆掉"上一轮"。
 * 不拆的后果不是内存泄漏那么温和：每个事件回调会跑**两遍**（消息重复入账、
 * 通知翻倍、markRead 双发），且第一遍的闭包绑的是已被丢弃的 state。
 */
let chatInitScope: InitScope | null = null;

export const useChatStore = defineStore("chat", () => {
  const peers = ref<Peer[]>([]);
  const friends = ref<Friend[]>([]);
  /** 后端给的**原始**好友申请列表（可能含已经过期的：对方已同意 / 我已把他加上了）。 */
  const rawPendingRequests = ref<PendingRequest[]>([]);
  /**
   * 展示用的好友申请列表：**已经在好友列表里的申请自动消失**。
   *
   * 用户 2026-09-12 真机实测的规则：「如果双方已经互相是好友了，另一个人点进『新朋友』列表，
   * 那条好友申请就应该自动清除掉」。
   *
   * 做成 computed 而不是在各处手动删，有两个原因：
   *   ① 四个地方都读 `chat.pendingRequests`（会话列表红点、通讯录「新的朋友」、
   *      窄导航徽标、添加好友页的「同意/拒绝」行）—— 一处过滤，四处同时生效；
   *   ② 无论这条申请是**怎么**被解决的（我同意、对方同意、重启后重新拉取），
   *      只要 `friends` 里有这个人，那一行就立刻消失，不依赖某条回执消息有没有送达。
   */
  const pendingRequests = computed(() =>
    actionableRequests(
      rawPendingRequests.value,
      friends.value.map((f) => f.device_id),
    ),
  );
  const conversations = ref<Conversation[]>([]);
  const groups = ref<Group[]>([]);
  const transfers = ref<TransferInfo[]>([]);
  /** 统一内容状态（ADR-0019）：未完成/失败的气泡据此显示「点击重试」。 */
  const contentTransfers = ref<ContentTransfer[]>([]);
  const messages = ref<Record<string, MessageRecord[]>>({});
  // group_id -> reader_id -> reader 已读到的最大时间戳
  const groupReads = ref<Record<string, Record<string, number>>>({});
  const activeConv = ref<string | null>(null);
  // 群聊里有人 @ 我且未读的会话 id 集合 → 列表摘要前显示 [有人@我]。
  // 纯前端运行时标志（不落库）：打开会话即清除，重启后随未读一起消失。
  const mentionedConvs = ref(new Set<string>());
  const topology = ref<TopologyInfo>({
    node_count: 0,
    relay_count: 0,
    avg_rtt_ms: null,
    online: false,
  });

  const activeConversation = computed(
    () => conversations.value.find((c) => c.id === activeConv.value) ?? null,
  );
  const totalUnread = computed(() => conversations.value.reduce((s, c) => s + c.unread, 0));

  // ---------------- 系统通知（后台 / 非当前会话才触发） ----------------
  const app = useAppStore();
  let notifSeq = 1;
  // ⚠️ 只加不删（审计阶段 4 · 4.2 顺带发现）：删除点只有"通知被点击"和"动作按钮命中"两处，
  // 用户直接把通知划掉时条目永久留着，而 `notifSeq` 单调递增 ⇒ 每条通知必进一份。
  // 比 pendingAcks 更确定性地增长，所以同样上闸；上限 128 远大于一屏可见的通知数。
  const notifMap = new Map<number, string>();
  const NOTIF_MAP_MAX = 128;

  function nicknameOf(id: string): string {
    const f = friends.value.find((x) => x.device_id === id);
    if (f) return f.nickname;
    const p = peers.value.find((x) => x.device_id === id);
    if (p) return p.nickname;
    return id;
  }

  const myDeviceId = computed(() => app.device?.device_id ?? "");

  // ---------------- 通知：抑制 + 短窗口合并 ----------------
  // 抑制矩阵：前台且正查看该会话 → 不通知；其他会话 / 后台 → 进入合并队列。
  // 合并：首条消息开 1.5s 窗口，窗口内同会话只累积；窗口结束按会话各发一条
  // （count > 1 显示「…等 N 条新消息」）。窗口结束前用户已切到该会话 → 跳过。
  const NOTIFY_DEBOUNCE_MS = 1500;
  const notifyQueue = new Map<string, QueuedNotice>();
  let notifyTimer: number | null = null;

  function queueNotification(rec: MessageRecord) {
    const q = notifyQueue.get(rec.conv_id);
    if (q) {
      q.count += 1;
      q.last = rec;
    } else {
      notifyQueue.set(rec.conv_id, { count: 1, last: rec });
    }
    if (notifyTimer !== null) return;
    notifyTimer = window.setTimeout(() => {
      notifyTimer = null;
      flushNotifications();
    }, NOTIFY_DEBOUNCE_MS);
  }

  function flushNotifications() {
    const entries = [...notifyQueue.values()];
    notifyQueue.clear();
    // 用户关了通知 → 一条都不发；权限在这条之后才去查（关着就不该弹权限）
    if (!app.notifyEnabled || entries.length === 0) return;
    void app.ensureNotifyPermission().then((granted) => {
      if (!granted) return;
      // 只要这一批里真的弹出了通知，就顺手提请注意（Windows 闪任务栏 / macOS 弹跳 Dock）。
      // 放在这里而不是 maybeNotify：闪烁必须与**实际发出的通知**同进同出 —— 通知被去抖
      // 合并、被"正在看该会话"跳过时，用户不该被闪。循环后统一触发一次，
      // 同一批多个会话也只闪一次（逐条调用是同一次视觉事件的重复请求）。
      let anyReminded = false;
      for (const { count, last } of entries) {
        // 窗口期间用户已切到该会话且**可见**前台 → 该会话跳过通知
        if (!document.hidden && document.hasFocus() && activeConv.value === last.conv_id) continue;
        const title = nicknameOf(last.sender_id);
        const body = notifyBody(count, last);
        const convId = last.conv_id;
        anyReminded = true;
        if (app.isMobile) {
          // 移动端：plugin 通知（Android 有 actionPerformed 点击事件桥）。
          // 注意 `sendNotification` 是 fire-and-forget（返回 void，不是 Promise）⇒ 这里
          // 没有可 catch 的失败信号，清单 4.2 说的"无 catch 丢批"其实不成立；真正会 reject
          // 的是上面那次权限查询，它的 catch 在本函数末尾。
          const id = notifSeq++;
          notifMap.set(id, convId);
          trimOldest(notifMap, NOTIF_MAP_MAX);
          void sendNotification({
            id,
            title,
            body,
            autoCancel: true,
            // 「标记已读」动作按钮：见 init 里 registerActionTypes；桌面端 Web Notification 不支持按钮
            actionTypeId: "chat",
            extra: { type: "chat", conv_id: convId },
          });
        } else {
          // 桌面端：统一走 notify_desktop 命令（Rust → notify-rust）。
          //
          // 为什么不再用 WebView 原生 Notification：Tauri 的 notification 插件会把
          // window.Notification 换成“转发到 plugin:notification|notify”的实现，所以
          // 这里的 onclick 永远不会触发（点击无法定位会话）；而且那条链路把真正的
          // toast 错误 spawn 掉丢了 —— Windows 上“收不到通知”完全没有线索。
          // 现在由后端发送：失败会返回并记日志，设置页还能“发送测试通知”自检。
          // `convId` 是**点击定位**的依据：点击发生在后端，前端补不了这个信息。
          void api.notifyDesktop(title, body, convId).catch(() => {
            /* 通知失败不影响聊天本身；后端已记日志 */
          });
        }
      }
      if (anyReminded && !app.isMobile) {
        void api.requestAttention().catch(() => {
          /* 个别 Linux 桌面环境不支持闪烁，忽略即可（通知本身已经发出） */
        });
      }
    }).catch((e) => {
      // ⚠️ 本批修的正是这里：`flushNotifications` **先 `clear()` 再 await 权限**，
      // 而这条链原先没有 catch ⇒ `isPermissionGranted()` / `requestPermission()` 任一 IPC
      // reject，这批通知就彻底消失（用户少收一条，毫无线索，还附赠一条 unhandled rejection）。
      // 放回队列是安全的：失败点在**任何一条通知发出之前**，不会重复提醒。
      console.warn("[notify] 权限查询失败，这批通知退回队列", e);
      mergeNoticesInto(notifyQueue, entries);
    });
  }

  // ---------------- 未读外显（托盘红点 / 任务栏角标 / Dock 数字） ----------------
  // 为什么不跟通知一起做：通知回答"刚来了一条新消息"，角标回答"还有多少条没读"。
  // 标记已读、切会话、删会话都会改未读数，却不该弹任何通知 —— 所以这条链路跟着
  // `totalUnread` 走，而不跟着消息走。
  //
  // 去抖 300ms：消息洪水、或"一键已读"会让 totalUnread 连续跳好几次，而 Windows 上换
  // 托盘图标是**肉眼可见的一下**（图标被重建）；只推最后一次就够了。
  const BADGE_DEBOUNCE_MS = 300;
  let badgeTimer: number | null = null;
  let lastBadgeCount = -1;

  function pushUnreadBadge() {
    if (app.isMobile) return; // 移动端没有托盘可改（命令本身也是空实现，省一次 IPC）
    const n = totalUnread.value;
    if (n === lastBadgeCount) return; // 值没变就不去动系统图标
    lastBadgeCount = n;
    void api.setUnreadBadge(n).catch(() => {
      /* 角标失败不影响聊天（个别 Linux 桌面环境不支持） */
    });
  }

  watch(totalUnread, () => {
    if (badgeTimer !== null) window.clearTimeout(badgeTimer);
    badgeTimer = window.setTimeout(() => {
      badgeTimer = null;
      pushUnreadBadge();
    }, BADGE_DEBOUNCE_MS);
  });

  /**
   * 通知正文：按「显示消息内容」隐私开关决定是否带正文。
   * 关掉时只提示"收到新消息"（锁屏 / 通知中心不泄内容），标题仍保留发送者昵称。
   * 拼装逻辑在 utils/notifications.ts（纯函数、有单测），这里只喂入实时数据。
   */
  function notifyBody(count: number, last: MessageRecord): string {
    let preview = previewText(last);
    const doneTitle = todoCompletedForCreator(last, myDeviceId.value);
    if (doneTitle !== null) {
      // 我创建的任务被完成：通知里明确说「已完成」，而不是笼统的「[任务] 标题」。
      preview = doneTitle
        ? t("todo.completedNotice", { title: doneTitle })
        : t("todo.completedNoticeNoTitle");
    } else if ((last.kind === "todo" || last.kind === "todo_update") && todoMentionsMe(last, myDeviceId.value)) {
      // 任务 @ 我：预览前缀「@你」，让锁屏/通知中心一眼看出是点名我
      // （与「有人@我」徽标同口径：被指派人 = 被 @）。
      preview = `${t("todo.mentionYou")} ${preview}`;
    }
    return notificationBody({
      showContent: app.notifyShowContent,
      count,
      sender: nicknameOf(last.sender_id),
      preview,
    });
  }

  /** 已提示过的「任务完成」msg_id：同一事件经多条投递路径反复 emit 时不重复提示创建人。 */
  const notifiedTodoDone = new Set<string>();
  const NOTIFIED_TODO_DONE_MAX = 500;

  function maybeNotify(rec: MessageRecord) {
    const myId = app.device?.device_id;
    if (!myId || rec.sender_id === myId) return;

    // 任务被完成 → 给**创建人**提示。`todo_update` 本是静默事件（不打扰全群），
    // 但"我派的活被干完了"对创建人是个该知道的变化，单独放行；其余静默事件照旧不弹。
    // 按 msg_id 去重：同一事件经多条投递路径反复 emit 时不重复提示创建人。
    const doneTitle = todoCompletedForCreator(rec, myId);
    if (doneTitle !== null && !notifiedTodoDone.has(rec.msg_id)) {
      notifiedTodoDone.add(rec.msg_id);
      while (notifiedTodoDone.size > NOTIFIED_TODO_DONE_MAX) {
        const oldest = notifiedTodoDone.values().next().value;
        if (oldest === undefined) break;
        notifiedTodoDone.delete(oldest);
      }
      // 正看着该会话：系统通知会被抑制，改用应用内 toast —— 否则创建人"什么都看不到"。
      if (!document.hidden && document.hasFocus() && activeConv.value === rec.conv_id) {
        app.toast(
          doneTitle ? t("todo.completedNotice", { title: doneTitle }) : t("todo.completedNoticeNoTitle"),
          "info",
        );
        return;
      }
      if (!app.notifyEnabled) return;
      queueNotification(rec);
      return;
    }

    // 「不打扰」一族（表情回应/撤回 = 静默事件，`system` = 经广播来的成员变更与下载请求）
    // 一律不弹通知：它们不是"内容"。判据与后端 `is_non_notifying_kind` 同一份
    // （`utils/messageKinds::countsTowardUnread`），与未读记账那处（`utils/messages.ts`）也同一份。
    // 系统消息漏这一条的真实后果："X 加入了群聊"给群里每个人推一条系统通知，
    // 而后端明写它不打扰（`protocol.rs::is_non_notifying_kind` 就是这个理由）。
    if (!app.notifyEnabled) return;
    if (!countsTowardUnread(rec.kind)) return;
    // 应用在前台且正查看该会话 → 不通知（不进队列）。
    // 必须同时判 !document.hidden：窗口被隐藏/最小化到托盘时，WebView 的
    // document.hasFocus() 仍可能是 true，只看它会漏掉真正该提醒的消息。
    if (!document.hidden && document.hasFocus() && activeConv.value === rec.conv_id) return;
    queueNotification(rec);
  }

  async function handleNotificationClick(convId: string) {
    await api.focusWindow();
    // 翻页排在加载之前（用户 2026-09-24 #29）：`focusWindow` 必须先行（窗口还在托盘里时
    // 切，用户会在浮出瞬间看到一次跳变），但 `openConversation` 不能等 —— 它内部是
    // 同步写下 `activeConv` + 骨架 + 两轮 IPC 拉消息，await 它等于让移动端
    // 「点通知」白等一次读库才翻页。
    if (app.isMobile) app.mobileView = "chat";
    await openConversation(convId);
  }

  /**
   * 通知点击的**统一路由**：桌面端（后端 `notification-clicked` 事件）与移动端
   * （插件的 `actionPerformed`）共用一份 —— 两条路径的载荷同形（`type` / `conv_id`），
   * 各写一份就一定会漂移（"桌面点得动、手机点不动"这类只在某一个平台现形的缺陷）。
   */
  function routeNotificationClick(type: string | undefined, convId: string | undefined) {
    if (type === "friend_request") {
      // 唤起窗口后再切视图：窗口还在托盘里时切，用户会在浮出瞬间看到一次跳变。
      void api.focusWindow().then(() => {
        window.dispatchEvent(new CustomEvent("navigate-to-contacts"));
      });
      return;
    }
    if (convId) void handleNotificationClick(convId);
  }

  // ---------------- 消息合并（同步） ----------------
  // 说明：曾用 Web Worker 后台合并，但 Tauri 生产构建（WKWebView 自定义协议）下
  // Worker 可能加载失败——mergeInWorker 的 Promise 永不 resolve，导致发送/接收的
  // 消息全部卡在合并步骤不刷新（需重开会话走查库路径才能恢复）。
  // 合并本身是 O(n) Set 去重 + 排序（单会话缓存 ≤300 条，微秒级），不值得为它
  // 冒 Worker 失效风险，改为主线程同步合并；rAF 批量节流保留。

  // ---------------- 密集广播批量队列 ----------------
  let pending: MessageRecord[] = [];
  let flushScheduled = false;
  // 乐观记录还在队列里未落地时到达的「真实记录」：tmp msg_id → 后端记录
  const pendingReplace = new Map<string, MessageRecord>();
  // Ack 先于乐观→真实替换到达：msg_id 已知但 store 里还没有该条目。
  // ⚠️ 必须有容量上限（审计阶段 4 · 4.2）：`found` 只扫 `messages.value`（LRU 保留 4 个会话），
  // 所以「会话被淘汰 / 文件回执 / outbox 补投」这三类 Ack 永远命中不了消费点，条目
  // 只会加不会删 ⇒ 跟着运行时长单调增长。这里用 FIFO 兜住最坏情况；真正的删除
  // 仍由 `send()` 消费时做（下面 `pendingAcks.delete`），淘汰只清那些本来就没人要的。
  const pendingAcks = new Set<string>();
  const PENDING_ACKS_MAX = 512;

  function scheduleFlush() {
    if (flushScheduled) return;
    flushScheduled = true;
    // 后台/遮挡窗口的 requestAnimationFrame 会被浏览器暂停，导致消息滞留不渲染；
    // 窗口不可见时退回 setTimeout，保证任何状态下都能入列渲染。
    if (!document.hidden && typeof requestAnimationFrame === "function") {
      requestAnimationFrame(flushNow);
      // 安全网（2026-09-23 审计 1.7）：rAF 回调可能在个别平台行为下丢失（既不投递
      // 也不报错），flushScheduled 一旦卡在 true，后续所有 enqueueMessage 都被
      // 开头的守卫挡回 —— 消息永久不渲染且无恢复手段、pending 无上限堆积。
      // 1s 后仍未落地就强制冲刷；rAF 正常时这里是幂等 no-op（flag 已被置回）。
      setTimeout(() => {
        if (flushScheduled) flushNow();
      }, 1000);
    } else {
      setTimeout(flushNow, 0);
    }
  }

  /** 立即冲刷批量队列（幂等：无挂起项时只复位 flag）。 */
  function flushNow() {
    flushScheduled = false;
    if (pending.length === 0) return;
    const batch = pending;
    pending = [];
    void applyIncoming(batch);
  }

  async function applyIncoming(raw: MessageRecord[]) {
    if (raw.length === 0) return;
    // 快照当前各条目的送达状态，供 applyReplacements 做 furthestStatus 比较
    const curStatus = new Map<string, string>();
    for (const list of Object.values(messages.value)) {
      for (const m of list) curStatus.set(m.msg_id, m.status);
    }
    const batch = applyReplacements(raw, pendingReplace, curStatus);
    const byConv = new Map<string, MessageRecord[]>();
    for (const m of batch) {
      const list = byConv.get(m.conv_id) ?? [];
      list.push(m);
      byConv.set(m.conv_id, list);
    }
    // 未读增量只统计「本地真正新增」的消息：后端对同一条业务消息可能经多条投递路径
    // （直连 / outbox 补发 / Gossip 转发）反复 emit `message-received`，若按整批长度累加，
    // 重复投递会一次次 +1，导致「发几条、数字却几十」——未读数虚高的根因。
    // 按 msg_id 与内存已有消息比对后，再据此累加 unread。
    const existingIdsByConv = new Map<string, Set<string>>();
    for (const [cid, list] of Object.entries(messages.value)) {
      existingIdsByConv.set(cid, new Set(list.map((m) => m.msg_id)));
    }
    const newByConv = new Map<string, MessageRecord[]>();
    for (const [cid, list] of byConv) {
      const known = existingIdsByConv.get(cid) ?? new Set<string>();
      const fresh: MessageRecord[] = [];
      for (const item of list) {
        if (known.has(item.msg_id)) continue; // 重复投递：不计入未读
        known.add(item.msg_id);
        fresh.push(item);
      }
      if (fresh.length) newByConv.set(cid, fresh);
    }
    // 被 @ 检测（微信式 [有人@我]）：仅群聊、非自己发的、且当前没开着这个会话。
    // 与未读同源（本地真正新增的消息），重复投递不会反复触发。
    // 「@所有人」对每个成员都等同于被点名，与点名走同一条判定入口。
    const myName = app.device?.nickname ?? "";
    const myId = myDeviceId.value;
    for (const [cid, fresh] of newByConv) {
      if (cid === activeConv.value || !cid.startsWith("group:")) continue;
      for (const rec of fresh) {
        if (rec.sender_id === myId) continue;
        const named = myName ? messageMentionsName(rec, myName) : false;
        // 任务被指派给我 ═ 被 @：显式 @ 指派的人（与 item 4 同口径），
        // 走微信式 [有人@我] 红点，且不会被重复投递反复触发（与上面同源 fresh）。
        if (named || messageMentionsAll(rec) || todoMentionsMe(rec, myId)) {
          mentionedConvs.value.add(cid);
          break;
        }
      }
    }
    // 会话列表中不存在的会话（新好友 / 后端新创建）：本地合并不了，直接从后端拉取
    const knownIds = new Set(conversations.value.map((c) => c.id));
    const missing = [...byConv.keys()].filter((id) => !knownIds.has(id));
    for (const [convId, incoming] of byConv) {
      // ⚠️ 这里曾是「已有 msg_id 一律丢弃」——但后端对同一条消息存在**回填式重发**：
      // 群文件 Done 之后会再 emit 一条带本地 path 的 gfile 记录（transport 完成回填段），
      // 丢弃它等于气泡永远停在无 path 形态 → 群图片预览打不开（真机 2026-09-19）。
      // 改成 upsert：同 msg_id 以本轮记录刷新内容（DB 是真相），送达状态仍只前进不回退。
      const existing = messages.value[convId] ?? [];
      const indexOf = new Map(existing.map((m, i) => [m.msg_id, i] as const));
      const next = [...existing];
      for (const m of incoming) {
        const i = indexOf.get(m.msg_id);
        if (i === undefined) {
          indexOf.set(m.msg_id, next.length);
          next.push(m);
        } else {
          const prev = next[i];
          // content 不能一律取新的：群文件的 path 是收完才回填的，回填前的后到记录
          // 会把已回填的 path 擦掉 ⇒ 预览请求根本不发出去（空白气泡）。见 pickMediaContent。
          next[i] = {
            ...m,
            content: pickMediaContent(prev, m),
            status: furthestStatus(prev.status, m.status),
          };
        }
      }
      messages.value[convId] = mergeMessages([], next);
      touchCacheOrder(convId);
    }
    // 收完一批就收缩一次缓存（本轮可能让若干非活跃会话的缓存变冷）
    enforceMessageCacheBound();
    if (missing.length > 0) {
      // 新会话：以 DB 为准拉全量（DB 已按新消息 +1），前端不再自行叠加
      await refreshConversations();
    } else {
      conversations.value = applyIncomingToConversations(
        conversations.value,
        activeConv.value,
        newByConv,
      );
    }
  }

  function enqueueMessage(rec: MessageRecord) {
    pending.push(rec);
    // 批量队列软上限（审计 1.7）：正常情况下 rAF 一帧内就会清空；堆积到这个量级
    // 说明 flush 调度已经失灵（或事件风暴），必须留痕 —— 无告警时它只是安静地吃内存。
    if (pending.length === 5000) {
      console.error("[chat] 消息批量队列堆积 5000 条：flush 调度疑似失灵", {
        flushScheduled,
        hidden: document.hidden,
      });
    }
    scheduleFlush();
  }

  // ---------------- 刷新 ----------------
  /**
   * 后发先至守卫（审计阶段 4 · 4.2）。下面这些 `refresh*` 全是 `x.value = await api.foo()`，
   * 而调用点大量是 `void refreshX()`（群操作成对调两个、好友申请通过、gossip 更新…），
   * IPC 又没有顺序保证 ⇒ 旧快照完全可能后回来，把未读、排序、在线状态整体回退。
   * 同文件的 `refreshTransfers` 已经为这个坑单独修过一次（那次是"进度条永久钉在 0%"），
   * 这里把同一个不变量收成一份实现，并按**被写的状态**分 key —— 不按函数分：
   * `refreshPeers` 与 `searchNearbyPeers` 写的是同一个 `peers`，必须互相作废。
   */
  const refreshGuard = new StaleGuard();

  async function refreshPeers() {
    const tok = refreshGuard.begin("peers");
    const list = await api.getPeers();
    if (!refreshGuard.isCurrent("peers", tok)) return;
    // 整表直写会换掉数组与里面每个对象 ⇒ 凡读过 peers 的渲染全部失效（见 utils/peerMerge）。
    const merged = mergePeerList(peers.value, list);
    if (merged) peers.value = merged;
  }
  /** 按需探测：群发一次 who_has 后返回周围在线节点（添加好友时调用）。 */
  async function searchNearbyPeers() {
    const tok = refreshGuard.begin("peers");
    const list = await api.searchNearbyPeers();
    // 被更新的一次探测抢走时：不写 peers、也不标注 online（online 是本结果的派生值，
    // 用旧探测结果标会把更新的状态盖回去），但**照常返回**给调用方它要的那份列表。
    if (!refreshGuard.isCurrent("peers", tok)) return peers.value;
    const merged = mergePeerList(peers.value, list);
    if (merged) peers.value = merged;
    const onlineIds = new Set(list.map((x) => x.device_id));
    friends.value.forEach((f) => (f.online = onlineIds.has(f.device_id)));
    return peers.value;
  }
  async function refreshFriends() {
    const tok = refreshGuard.begin("friends");
    const list = await api.getFriends();
    if (!refreshGuard.isCurrent("friends", tok)) return;
    friends.value = list;
  }
  async function refreshPending() {
    const tok = refreshGuard.begin("pending");
    const list = await api.getPendingRequests();
    if (!refreshGuard.isCurrent("pending", tok)) return;
    rawPendingRequests.value = list;
  }
  /**
   * 「这条会话我在本地已经判过已读」的水位（convId → 时刻）。
   *
   * 存在的理由见 `utils/messages.ts::applyConversationSnapshot`：`StaleGuard` 只挡
   * "后发先至"，挡不住"同一份请求、数据本身是旧的" —— 乐观清零之后再落一份清零前的
   * 快照，红点就会自己亮回来。
   */
  const unreadClearedAt = new Map<string, number>();
  /** 水位窗：超过这个年纪的水位一定不会再被任何在飞快照引用，丢掉以免 Map 无界增长。 */
  const UNREAD_CLEAR_TTL_MS = 30_000;

  /**
   * **全 store 唯一的"本地把未读清零"入口**：改内存 + 打水位两件事必须同时发生，
   * 所以不许在别处再写一遍 `conv.unread = 0`（判据：`windowEntries` 之外的
   * `storeContract` 结构守卫 —— 见该文件里那条 "只有一个家"）。
   */
  function clearUnreadLocally(convId: string) {
    const conv = conversations.value.find((c) => c.id === convId);
    if (conv && conv.unread !== 0) conv.unread = 0;
    // 水位打在**发起前**的这一刻：晚于快照发起 ⇒ 那份快照里的数字是旧的，不许点亮红点
    unreadClearedAt.set(convId, Date.now());
  }

  async function refreshConversations() {
    const tok = refreshGuard.begin("conversations");
    // 发起时刻：这是"快照里的数据至少有多新"的下界（单连接单锁 ⇒ 读一定发生在发起之后）
    const issuedAt = Date.now();
    const list = await api.getConversations();
    // ⚠️ 这条最不是理论问题：`openConversation` 会**乐观清零**未读，而旧快照带着清零前的
    // `unread` ⇒ 红点自己亮回来、列表顺序也跟着回退（用户看到的"我没点它怎么又红了"）。
    if (!refreshGuard.isCurrent("conversations", tok)) return;
    conversations.value = applyConversationSnapshot(
      conversations.value,
      list,
      unreadClearedAt,
      issuedAt,
    );
    pruneUnreadClears(unreadClearedAt, Date.now(), UNREAD_CLEAR_TTL_MS);
  }
  async function refreshGroups() {
    const tok = refreshGuard.begin("groups");
    const list = await api.getGroups();
    if (!refreshGuard.isCurrent("groups", tok)) return;
    groups.value = list;
    const entries = await Promise.all(
      list.map(async (group) => [group.id, await api.getGroupReads(group.id).catch(() => [])] as const),
    );
    const next: Record<string, Record<string, number>> = {};
    for (const [groupId, reads] of entries) {
      next[groupId] = Object.fromEntries(reads.map((read) => [read.reader_id, read.last_read_ts]));
    }
    // 读名单是**第二次 await** 之后才写的，必须再过一次闸：把已退群成员的绿勾写回来，
    // 比群列表旧一帧难看得多。
    if (!refreshGuard.isCurrent("groups", tok)) return;
    groupReads.value = next;
  }

  /**
   * 「另一个窗口清空了聊天数据」→ 本窗口先**清空本地视图**，再把"还在的东西"重拉一遍。
   *
   * 为什么不能只重拉：会话/群/转移单来自后端（重拉就会变空），但 `messages`、
   * `rawPendingRequests`、`activeConv` 是**本地态** —— 不清的话主界面仍然挂着
   * 已被删除的会话内容与已经处理完的申请。用户实测（Mac 4.1.10）：在设置里清了
   * 「缓存 / 目录 / 聊天记录」，主界面一点反应都没有，看起来像没清掉。
   *
   * 好友**不清**：`clear_all_data` 不动好友表（重置数据不等于断交），所以这里同样保留。
   */
  async function resetAfterDataCleared() {
    messages.value = {};
    conversations.value = [];
    groups.value = [];
    groupReads.value = {};
    rawPendingRequests.value = [];
    transfers.value = [];
    // 收藏也随「清除数据」一并清空（后端删了行与副本）：不清的话，面板开着时仍显示
    // 已删条目，点开只会报"副本已不在本机"。
    favorites.value = [];
    activeConv.value = null;
    pending = [];  // 后台滞留待冲刷的消息批次（`let pending`，见上）
    // 这三张表存的都是"内存里的过渡态"：库已经清了，留着它们会让后续 flush 给已经不存在的
    // 记录重建条目（审计阶段 4 · 4.3）。`clearAllData` 走的是另一条路径，两边必须同口径。
    pendingReplace.clear();
    pendingAcks.clear();
    notifMap.clear();
    await Promise.all([
      refreshConversations(),
      refreshGroups(),
      refreshTransfers(),
      refreshPending(),
      refreshFriends(),
    ]);
  }

  function groupReaderIds(groupId: string, messageTs: number): string[] {
    const myId = app.device?.device_id;
    const members = new Set(groups.value.find((group) => group.id === groupId)?.members ?? []);
    return Object.entries(groupReads.value[groupId] ?? {})
      .filter(([readerId, lastReadTs]) => members.has(readerId) && readerId !== myId && lastReadTs >= messageTs)
      .map(([readerId]) => readerId);
  }
  /** 并发触发时只让**最新那次**写回结果（迁入 `refreshGuard`，与上面几个共用同一份实现）。 */
  async function refreshTransfers() {
    const req = refreshGuard.begin("transfers");
    const [list, contents] = await Promise.all([
      api.getTransfers(),
      api.getContentTransfers().catch(() => []),
    ]);
    // 后发先至的旧快照必须丢掉：多选发送时每个文件都会 `void refreshTransfers()`，
    // 旧快照里**没有**刚建的那条 transfer ⇒ 覆盖回来后，后续 file-progress 全部落空
    // （见 updateTransferProgress），进度条永久钉在「发送中 0%」——而对端早就收完已读。
    if (!refreshGuard.isCurrent("transfers", req)) return;
    transfers.value = list;
    // 顺带刷新统一内容状态：未完成 / 校验失败的气泡据此显示「点击重试」。
    contentTransfers.value = contents;
  }
  /** 进度/done 事件早于 transfer 行进内存时的一次性补拉（防抖，不放大 IPC）。 */
  let transfersRepairTimer: ReturnType<typeof setTimeout> | null = null;
  function scheduleTransfersRepair() {
    if (transfersRepairTimer) return;
    transfersRepairTimer = setTimeout(() => {
      transfersRepairTimer = null;
      void refreshTransfers();
    }, 400);
  }
  /** 上一次真正拉取拓扑的时间（`refreshTopologyThrottled` 用）。 */
  let lastTopologyAt = 0;

  /** 由高频事件触发的拓扑刷新：最多 1s 一次（判据是纯函数，见 `utils/defer`）。 */
  function refreshTopologyThrottled() {
    const now = Date.now();
    if (!shouldRunThrottled(now, lastTopologyAt, 1000)) return;
    lastTopologyAt = now;
    void refreshTopology();
  }

  async function refreshTopology() {
    const tok = refreshGuard.begin("topology");
    const list = await api.getTopology();
    if (!refreshGuard.isCurrent("topology", tok)) return;
    topology.value = list;
  }

  /**
   * 打开会话时的未读定位：第一条未读在**已渲染时间线**里的下标
   * （-1 = 有未读但还没算出来，交给贴底兜底）。
   *
   * ⚠️ 坐标系是 `ChatWindow.messages`（过滤后的列表），不是 `messages[convId]` 原始列表 ——
   * 分割线和 `scrollToIndex` 都在那个列表里消费这个数。换算收在
   * `utils/messages.ts:unreadAnchorIndex()`，过滤判据与 ChatWindow 共用
   * `isRenderedInTimeline`，不留第二份真相源（审计阶段 4 · 4.1-1）。
   */
  const unreadJump = ref<{ convId: string; index: number } | null>(null);

  /**
   * 「跳到某条消息」请求（搜索结果点击时发起，由 ChatWindow 消费）。
   * 与 unreadJump **分开**是刻意的：unreadJump 会画「以下是未读消息」分割线，
   * 而"搜索定位"只是滚过去 + 高亮，语义不同，复用会画错东西。
   */
  const locateRequest = ref<{ convId: string; msgId: string } | null>(null);

  function clearLocateRequest() {
    locateRequest.value = null;
  }

  /**
   * 打开会话并定位到指定消息（搜索结果点击）。
   *
   * 命中可能**早于已加载窗口**（默认只加载最近 100 条）→ 逐页往前找，
   * 上限沿用 MAX_PAGES（与用户手动上翻一致，不会为一句话翻遍整库）。
   *
   * 返回三态而非布尔：翻页中途出错与"翻到顶也没找到"是**两回事**，
   * 调用方要给不同的话（HIG：异步路径必须有终态，且不能说错原因）。
   */
  async function locateMessageInConv(
    convId: string,
    msgId: string,
  ): Promise<"found" | "not-found" | "error"> {
    await openConversation(convId);
    // 用户明确说"我要看这条"，就不要再去跳未读分割线（两者会互相抢滚动位置）
    unreadJump.value = null;
    try {
      for (let guard = 0; guard <= MAX_PAGES; guard++) {
        const list = messages.value[convId] ?? [];
        if (list.some((m) => m.msg_id === msgId)) {
          locateRequest.value = { convId, msgId };
          return "found";
        }
        const before = list.length;
        await loadMoreMessages(convId);
        // 长度没变 = 已翻到顶或已达页数上限 → 再循环无意义
        if ((messages.value[convId]?.length ?? 0) === before) break;
      }
    } catch (e) {
      // 不静默吞掉：留排查线索，并把"出错"这一态如实返回给调用方
      console.warn("[gosslan] 定位搜索命中消息失败", e);
      return "error";
    }
    return "not-found";
  }

  async function openConversation(id: string) {
    activeConv.value = id;
    // 记住上次会话：重启后恢复（HIG State Restoration；localStorage 只作 UI 状态，
    // 失败不影响打开聊天）。
    try {
      localStorage.setItem(LAST_CONV_KEY, id);
    } catch {
      /* localStorage 不可用则跳过，不影响本次打开 */
    }
    // 标记为最近使用，并在加载完成后收缩缓存（活跃会话始终保留）
    touchCacheOrder(id);
    // 打开即视为看到 → [有人@我] 标志随之清除
    mentionedConvs.value.delete(id);
    // 打开前先记录未读数（markRead 会清零），用于「跳到第一条未读」定位。
    // 有未读时提前占位（index=-1 = 加载中、索引未知）：让 ChatWindow 与
    // autoScrollOnSwap 在 loadMessages 完成前就知道「要跳未读」，避免先贴底再
    // 跳未读造成闪烁。
    const unreadBefore = conversations.value.find((c) => c.id === id)?.unread ?? 0;
    unreadJump.value = unreadBefore > 0 ? { convId: id, index: -1 } : null;
    // 未读清零走**乐观更新**（用户 2026-09-12 要求「所有异步操作尽量乐观更新」）：
    // 打开会话即视为已读，本地立刻清零，别让红点在 await 期间继续显示。
    clearUnreadLocally(id);
    // 会话行不存在（如新加好友还没发过消息）→ 后端补建，保证左侧列表有对应可高亮的项。
    // ⚠️ **不阻塞消息加载**：补建会话行与 loadMessages 互不依赖，串行 await 会让
    // 「切到一个全新会话」白等一次 IPC（骨架已经渲染，但内容迟迟不来）。
    // 因此并行发起，回来后若仍未出现再补进列表。
    const ensureConv = conversations.value.some((c) => c.id === id) || id.startsWith("group:")
      ? Promise.resolve()
      : api
          .ensureConversation(id)
          .then((conv) => {
            if (!conversations.value.some((c) => c.id === id)) {
              conversations.value = sortConversations([conv, ...conversations.value]);
            }
          })
          .catch(() => {
            /* 忽略：不影响打开聊天 */
          });
    // ReadReceipt 与消息加载并行：让对方尽早看到绿勾，且不拖慢本端渲染。
    // 原先这里发了两次 markRead（一次 void、末尾再一次 await），属重复 IPC，一并去掉。
    const readReceipt = api.markRead(id).catch(() => {
      /* 回执失败不回滚已读：本地确实已经看到了 */
    });
    await Promise.all([loadMessages(id), ensureConv]);
    await readReceipt;
    if (unreadBefore > 0) {
      // ⚠️ 换算必须发生在**过滤后的列表**里：`unread` 是后端计数，而分割线落在
      // ChatWindow 渲染出来的那些行上（判据两边共用 `isRenderedInTimeline`，
      // 见 utils/messageKinds）。旧写法在原始列表里取下标、到过滤后列表里用，
      // 而"占下标不占未读"（静默/`system`）与"占未读不占下标"（`announcement`/`poll`）
      // 两个方向的偏差同时存在 ⇒ 分割线画错消息、`scrollToIndex` 跳错位置。
      const rendered = (messages.value[id] ?? []).filter((m) => isRenderedInTimeline(m.kind));
      const idx = unreadAnchorIndex(rendered, unreadBefore);
      unreadJump.value = idx >= 0 ? { convId: id, index: idx } : null;
    }
    // 未读已在上面乐观清零；这里只做一次兜底（若期间又来了新消息把 unread 加回去，
    // 说明是"打开之后"到达的，此时不该再清）。
    // 切会话后收缩一次：刚被切走的会话若已冷，就可释放其内存副本
    enforceMessageCacheBound();
  }

  // ---------------- 会话内消息分页：每会话最多缓存页数（防内存无限增长） ----------------
  const PAGE_SIZE = 100;
  const MAX_PAGES = 10;
  const pagesLoaded = new Map<string, number>();
  /**
   * 正在翻历史页的会话 → 那一次的 Promise。同一会话同时只允许一个在飞，
   * 但**后来者必须并入这一次**（见 `loadMoreMessages` 的 ⚠️）而不是被直接弹回。
   */
  const loadingMore = new Map<string, Promise<void>>();
  /**
   * 已知"这个会话已翻到最早一页"的会话：不再为它付任何翻页 IPC。
   * ⚠️ 必须在每次 `loadMessages` 重新加载该会话时作废 —— 重新打开只取最新一页，
   * 若沿用旧标记，500 条的会话就再也翻不到第 101 条以前（"翻不动历史"就是这么来的）。
   */
  const historyTops = new Set<string>();
  // 加载竞态守卫：快速切换会话时丢弃过期响应。**按会话分桶**：
  // 全局单计数会让「非活跃会话的重查」把此刻飞行中的活跃会话加载/翻页
  // 一并判为过期（onMessageStatusChanged 会对任意含该 msg_id 的会话触发
  // loadMessages）→ 活跃会话 messages[convId] 永远 undefined，骨架永久转圈
  // （2026-09-23 审计 1.4）。会话淘汰时一并清理（见 enforceMessageCacheBound）。
  const loadSeqs = new Map<string, number>();

  // ---------------- 消息缓存上界：限制「同时缓存多少个会话」 ----------------
  //
  // 为什么需要：MAX_PAGES 只限制「一个会话能往上翻几页」，而 messages 是按会话累积的
  // 内存副本——聊天对象一多，几十个会话各留最多 1000 条，长期挂机内存会持续增长。
  //
  // 为什么可以安全淘汰非活跃会话：UI 只渲染活跃会话（ChatWindow 只读 activeConv 的
  // 消息列表），其它会话的缓存纯粹是内存副本，切回时由 loadMessages 从 SQLite 重新
  // 加载最新一页，因此丢弃不影响任何展示。
  //
  // 为什么从 4 提到 8（用户 2026-09-24 #29「切换会话要瞬间响应」）：这条上界直接决定
  // 「切过去是**当场有内容**还是**先看到骨架**」—— 骨架的判据就是
  // `messages[convId] === undefined`。命中缓存 = 立刻有上一轮的内容可画（后台照样重查一页
  // 把发送状态/撤回对齐，只是那期间不空白）；被淘汰 = 冷加载**只能等那一轮 IPC**
  // （`get_latest_messages`，一次拿完，仍要排队过后端那把全局 `Mutex<Connection>`）。
  // 4 个槽位意味着常聊 5 个人时按 A→B→C→D→E→A 转一圈，
  // **回到 A 的那一下必然**是冷的 —— 淘汰是纯 LRU 计数、与"多久没打开"无关，
  // 所以这个退化是确定会发生，不是偶发。
  // 代价核过账：每会话上界 = MAX_PAGES × PAGE_SIZE = 1000 条，一条记录（msg_id/conv_id/
  // kind/content/sender/ts/link/status 等十余字段，正文通常几十字）量级 0.5–1 KB
  // ⇒ 8 个会话最坏 4–8 MB 的二级内存副本；渲染侧仍只有活跃会话那一列（VirtualList
  // 只画视口内的行），所以放大的是内存不是帧开销。
  //
  // 去重不受影响：后端 insert_message_if_new 只在「真的新建一行」时 emit
  // message-received（见 db.rs 注释），跨批次去重以它为权威；前端这份缓存只是二级保险。
  //
  // 仍未覆盖（已知遗留）：单个会话的实时新消息仍会不断追加，条数没有硬上界。
  // 未做是因为裁剪头部会改变 VirtualList 的滚动锚定（用户正向上翻阅时内容会跳动），
  // 需要与滚动状态联动，收益（每会话几 MB）不值这个回退风险。
  const MAX_CACHED_CONVS = 8;
  /** 会话缓存的 LRU 顺序（最近使用的在末尾） */
  const cacheOrder: string[] = [];

  function touchCacheOrder(convId: string) {
    const i = cacheOrder.indexOf(convId);
    if (i >= 0) cacheOrder.splice(i, 1);
    cacheOrder.push(convId);
  }

  /** 把消息缓存收缩到 MAX_CACHED_CONVS 个会话（保留活跃会话 + 最近使用的若干个）。 */
  function enforceMessageCacheBound() {
    const keys = Object.keys(messages.value);
    if (keys.length <= MAX_CACHED_CONVS) return;
    const keep = selectCachedConversations(cacheOrder, activeConv.value, MAX_CACHED_CONVS);
    for (const cid of keys) {
      if (keep.has(cid)) continue;
      delete messages.value[cid];
      // 分页簿记一并清掉：切回该会话时 loadMessages 会重新从 DB 取最新一页
      pagesLoaded.delete(cid);
      loadSeqs.delete(cid);
      // 同一本账：不清"到顶"结论的话，该会话重开后翻不动历史。
      // **不**跟着删 loadingMore —— 那条 Promise 自己会落地并摘除；在这里删会
      // 让"淘汰后新发起的一次"与仍在飞的旧的一次并行拉页，制造新的重复请求。
      historyTops.delete(cid);
    }
    // 同步收缩 LRU，避免它自身无限增长
    for (let i = cacheOrder.length - 1; i >= 0; i--) {
      if (!Object.prototype.hasOwnProperty.call(messages.value, cacheOrder[i])) {
        cacheOrder.splice(i, 1);
      }
    }
    // 未读定位指向已被淘汰的会话 → 清除（正常指向活跃会话，几乎不会走到）
    if (
      unreadJump.value &&
      !Object.prototype.hasOwnProperty.call(messages.value, unreadJump.value.convId)
    ) {
      unreadJump.value = null;
    }
  }

  async function loadMessages(convId: string) {
    const seq = (loadSeqs.get(convId) ?? 0) + 1;
    loadSeqs.set(convId, seq);
    touchCacheOrder(convId);
    // ⚠️ 必须在**函数开头**作废"已翻到顶"这个结论，不能只在成功路径末尾清：
    // IPC 失败会走 catch 提前 return、seq 被后来者抢走也会提前 return —— 那些路径下
    // 内存列表同样是"只有最新一页"，若 historyTops 还留着 true，`loadMoreMessages`
    // 就被永久挡死，而它正是这种空列表下唯一的自愈通道（2026-09-23 评审抓出）。
    historyTops.delete(convId);
    // 打开会话应先加载「最新一页」，而不是最旧一页；否则底部会停在第 100 条历史，
    // 最新消息与文件都要靠后续滚动才出现。
    let list: MessageRecord[];
    try {
      // 一次 IPC 拿最新一页（后端返回正序，前端整表覆盖不必再倒一遍）。
      // 原先是「先 `COUNT(*)` 算 offset、再按 offset 取页」两轮**串行** —— 每轮都要排队过
      // 全局那把 db 锁，而 count 这一轮在冷加载里除了算 offset 没有别的用处。
      // 判据：`storeContract` 钉住"冷加载只发一次消息页 IPC"，Rust 侧钉住
      // 「一次取尾 == 两轮按 offset 取，逐行同序」。
      list = await api.getLatestMessages(convId, PAGE_SIZE);
    } catch {
      // 读库失败也要把该会话标记为「已加载」：留 undefined 会让 ChatWindow 的加载骨架
      // 永远停在那里（UI 先行就得保证"必有终态"）。给空列表 = 回到"暂无消息"的正常态。
      if (messages.value[convId] === undefined) messages.value[convId] = [];
      return;
    }
    if (seq !== (loadSeqs.get(convId) ?? 0) || activeConv.value !== convId) return;
    // 快照可能取自 Ack / peer-read 落库之前：与查询期间已推进的内存状态合并，
    // 否则刚亮的绿勾会被这份旧快照退回「发送中」。
    const prev = messages.value[convId];
    // 乐观气泡（tmp-*）/ 文件失败占位（file-failed-*）只存在于内存：快照整表
    // 覆盖会把它们吞掉 → 已发出的消息"凭空消失"，用户以为失败而重发（审计 1.3）。
    messages.value[convId] = prev
      ? appendLocalOnly(preserveDeliveryStatus(list, prev), prev)
      : list;
    pagesLoaded.set(convId, 1);
  }

  /** 向上翻页加载更早的历史消息（VirtualList 触顶时调用）。 */
  async function loadMoreMessages(convId: string): Promise<void> {
    // 两道闸门都必须有（审计阶段 4 · 4.1-2）：`VirtualList` 在"停在距顶 60px 内"期间
    // 每一次重测都会 emit `loadMore`（滚动 / resize / applyJump 轮询 / scrollToIndex /
    // items 变化 / 总高变化共 6 个入口），所以本函数会被**每帧**调用。
    //   · loadingMore：没有它，并发的两次调用都过得了下面的页数判据、各自 `getMessages`
    //     拉一页，却把 `pagesLoaded` 写成同一个值 ⇒ "前进一页、拉了两页数据"。
    //   · historyTops：没有它，已经翻到最早一页的会话（绝大多数会话都不满 MAX_PAGES）
    //     每帧都要白付一次 `getMessageCount` IPC。
    // 刻意**不做**边沿触发式的一次性开关：那样用户停在顶部时只会翻出一页，必须"往下滚
    // 一点再滚回来"才能继续翻 —— 那是拿一个真实可用的行为去换一个噪声，不划算。
    if (historyTops.has(convId)) return;
    // ⚠️ 单飞必须是**并入在飞的那一次**，不是"看见有人在飞就直接返回"。
    // `locateMessage` / `locateMessageInConv` 的循环写的是 `await loadMoreMessages()`
    // 然后"长度没变 ⇒ 没有更早历史了"；直接返回会让它们把"别人正在翻"误判成"翻到头了"，
    // 表现就是点引用/搜索命中时误报「原消息在更早的历史里」并拒绝定位（2026-09-23 评审
    // 抓出的回归 —— 加闸门时只想了防重复拉页，没想await方的语义）。
    const running = loadingMore.get(convId);
    if (running) return running;
    const pages = pagesLoaded.get(convId) ?? 1;
    if (pages >= MAX_PAGES) return;
    const page = loadMorePage(convId, pages);
    loadingMore.set(convId, page);
    // 挂在链上而不是原 promise 上：这样 `page` 始终有处理器，reject 时不会变成
    // unhandled rejection，而**所有** await 方仍各自拿到同一次结果。
    return page.finally(() => {
      loadingMore.delete(convId);
    });
  }

  /** 真正拉一页。单飞与容量闸都在 `loadMoreMessages` 那一层，这里只管一次 IPC。 */
  async function loadMorePage(convId: string, pages: number): Promise<void> {
    const seq = loadSeqs.get(convId) ?? 0;
    const total = await api.getMessageCount(convId);
    if (seq !== (loadSeqs.get(convId) ?? 0)) return;
    if (pages * PAGE_SIZE >= total) {
      historyTops.add(convId);
      return;
    }
    // 当前已加载最新 pages 页，继续向更早方向取一页。
    const offset = Math.max(0, total - (pages + 1) * PAGE_SIZE);
    const older = await api.getMessages(convId, PAGE_SIZE, offset);
    if (seq !== (loadSeqs.get(convId) ?? 0) || older.length === 0) return;
    const existing = messages.value[convId] ?? [];
    messages.value[convId] = mergeMessages(existing, older);
    pagesLoaded.set(convId, pages + 1);
    // 拿不满一页就是到头了，不必再问一次 `getMessageCount`
    if (older.length < PAGE_SIZE) historyTops.add(convId);
    // prepend 历史后，「第一条未读」的下标整体后移。位移量既不是 `older.length`（原实现，
    // 把不渲染的静默行也算进去 ⇒ 每翻一页再累积一次误差，审计 4.1-1 的第二处实例），
    // 也不是"这一页里会渲染的条数"（2026-09-23 评审抓出）—— `mergeMessages` 会按 msg_id
    // 去重，而重叠是常态：会话总数落在 101~199 之间时第二页请求的 offset 仍是 0，
    // 返回的整页与内存里的最新一页大面积重叠；翻页期间来了新消息同样重叠。
    // 只有"真正新插进列表的那些**会渲染**的行"才是该平移的量。
    // index=-1 是"还在加载"的占位态，不参与位移。
    if (unreadJump.value?.convId === convId && unreadJump.value.index >= 0) {
      const known = new Set(existing.map((m) => m.msg_id));
      let shift = 0;
      for (const m of older) {
        if (!known.has(m.msg_id) && isRenderedInTimeline(m.kind)) shift += 1;
      }
      if (shift > 0) {
        unreadJump.value = { ...unreadJump.value, index: unreadJump.value.index + shift };
      }
    }
  }

  // ---------------- 独立「群任务」窗口用的读路径 ----------------
  // ⚠️ 这两个函数是给**独立窗口**（`todos.html`）用的：那个窗口有自己的 store 实例，
  // 只服务一个群。绝不能在那里调 `init()`（会注册第二套事件监听 → 重复通知/未读/回执）
  // 或 `openConversation()`（会发群已读回执、写与主窗口共享的 localStorage）。

  /**
   * 「这个群的任务首屏取完了没有」—— 看板该转圈还是该显示内容的唯一依据。
   *
   * 为什么必须有（用户 2026-09-24：「点查看任务，弹窗弹出很慢，还以为没点上」）：群任务窗口
   * 原先把 `loadGroupTodos` 排在**挂载之前**，那段时间整扇窗是一块骨架灰屏（看不出在加载什么），
   * 而连点又被启动器的单飞/防抖吃掉 ⇒ 表现就是"点了没反应，过一会才蹦出来"。
   * 改成先挂载、数据后台取之后，没有这个标记的话首帧会理直气壮地显示「暂无任务」——
   * 那不是快了一点，那是**假空态**，比慢更糟。
   *
   * ⚠️ 语义是"这一次取数**结束了**"（成功或失败），不是"成功过"：失败也置位，否则
   * 一次 DB 出错就把窗口永久留在转圈上，而那是一个连点都救不回来的状态。
   * 失败由调用方 toast 出来（`src/entries/todos.ts`），本标记只负责"别一直转"。
   */
  const groupTodosSettled = ref<Record<string, true>>({});

  /** 该群的任务首屏是否已取完（未开始与进行中都返回 false ⇒ 看板显示加载态）。 */
  function todosLoadedOnce(groupId: string): boolean {
    return groupTodosSettled.value[groupId] === true;
  }

  /**
   * 群任务窗口的读路径：加载**一个群**的消息 + 解析成员名要用的数据。
   *
   * ⚠️ 不变量：调用方（群任务窗口）的 store 实例**只服务这一个会话**，所以这里
   * 把 `activeConv` 设成它（`loadMessages` 有 `activeConv === convId` 守卫）。
   */
  async function loadGroupTodos(groupId: string): Promise<void> {
    const convId = `group:${groupId}`;
    try {
      // 成员名/群名/头像来自 groups + friends + peers；conversations 让窗口内的
      // `enqueueMessage`（创建/更新任务后本地合并）不必再补一次拉取。
      await Promise.all([refreshGroups(), refreshFriends(), refreshPeers(), refreshConversations()]);
      activeConv.value = convId;
      await loadMessages(convId);
    } finally {
      groupTodosSettled.value[groupId] = true;
    }
  }

  let groupTodosUnlisten: (() => void) | null = null;
  let groupTodosTimer: number | null = null;

  /**
   * 只订阅**本会话**的 `message-received`，让窗口里的任务随远端改动实时刷新
   * （突发消息合并到 150ms 一次重拉）。返回取消函数。
   */
  async function watchGroupTodos(groupId: string): Promise<() => void> {
    const convId = `group:${groupId}`;
    groupTodosUnlisten?.();
    const unlisten = await api.onMessageReceived((rec) => {
      if (rec.conv_id !== convId) return;
      if (groupTodosTimer !== null) window.clearTimeout(groupTodosTimer);
      groupTodosTimer = window.setTimeout(() => {
        groupTodosTimer = null;
        void loadMessages(convId);
      }, 150);
    });
    groupTodosUnlisten = unlisten;
    return () => {
      if (groupTodosTimer !== null) {
        window.clearTimeout(groupTodosTimer);
        groupTodosTimer = null;
      }
      unlisten();
      groupTodosUnlisten = null;
    };
  }

  /** 统一发送（单聊/群聊）。乐观上屏：先显示 sending，invoke 成功后替换为真实记录。 */
  async function send(convId: string, content: string, kind: string): Promise<MessageRecord> {
    const myId = app.device?.device_id ?? "";
    // 乐观消息先占一个很大的逻辑序号，保证它出现在会话底部；后端返回真实记录后会替换为权威 seq。
    const optimistic: MessageRecord = {
      id: -1,
      msg_id: `tmp-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      conv_id: convId,
      sender_id: myId,
      receiver_id: convId,
      kind: kind as MessageRecord["kind"],
      content,
      ts: Date.now(),
      seq: Number.MAX_SAFE_INTEGER,
      status: "sending",
    };
    enqueueMessage(optimistic);
    try {
      let rec: MessageRecord;
      if (convId.startsWith("group:")) {
        rec = await api.sendGroupMessage(convId.slice(6), content, kind);
      } else {
        rec = await api.sendMessage(convId, content, kind);
      }
      // 如果 Ack 已在 await 期间到达：直接把 real record 提升到 delivered，
      // 然后一次性替换 optimistic → real（不再二次 replaceMessage）。
      const acked = pendingAcks.delete(rec.msg_id);
      const next = acked
        ? { ...rec, status: furthestStatus(rec.status, "delivered") }
        : rec;
      replaceMessage(convId, optimistic.msg_id, next);
      return rec;
    } catch (e) {
      // 发送失败：乐观消息标记为失败态（保留内容，用户可重发）
      replaceMessage(convId, optimistic.msg_id, { ...optimistic, status: "failed" });
      throw e;
    }
  }

  /** 替换会话内指定 msg_id 的消息（乐观记录 → 真实记录 / 状态变更）。 */
  function replaceMessage(convId: string, msgId: string, next: MessageRecord) {
    const list = messages.value[convId];
    if (list) {
      const i = list.findIndex((m) => m.msg_id === msgId);
      if (i >= 0) {
        // 取两者中更靠后的状态：pendingAcks 消费或 pendingReplace 批量替换
        // 不应把已达 read 的记录退回 delivered（Ack 晚到 / peer-read 竞态）。
        const merged = { ...next, status: furthestStatus(list[i].status, next.status) };
        messages.value[convId] = [...list.slice(0, i), merged, ...list.slice(i + 1)];
        return;
      }
    }
    // 乐观记录还压在批量队列里（invoke 先于 rAF 落地返回）：挂起等批次落地时替换。
    // 直接丢弃真实记录 = 气泡永远停在「发送中」，而后端库里早已是 delivered/read。
    if (pending.some((m) => m.msg_id === msgId)) pendingReplace.set(msgId, next);
  }

  async function sendFriendRequest(peerId: string) {
    await api.sendFriendRequest(peerId);
  }
  /** 乐观交互：立即移出申请列表，失败回滚（调用方负责 toast）。 */
  async function respondRequest(peerId: string, accept: boolean) {
    const prev = rawPendingRequests.value;
    rawPendingRequests.value = prev.filter((r) => r.from !== peerId);
    try {
      await api.respondFriendRequest(peerId, accept);
    } catch (e) {
      rawPendingRequests.value = prev; // 回滚
      throw e;
    }
    if (accept) {
      await refreshFriends();
      await refreshConversations();
    }
  }

  /** 乐观交互：立即从联系人移除，失败回滚。保留聊天记录（后端行为）。 */
  async function removeFriend(peerId: string) {
    const prev = friends.value;
    friends.value = prev.filter((f) => f.device_id !== peerId);
    try {
      await api.removeFriend(peerId);
    } catch (e) {
      friends.value = prev; // 回滚
      throw e;
    }
  }

  /**
   * 删除一个会话及其全部本地消息。
   * - 乐观移除：先从会话列表与内存缓存中抹除，后端失败回滚；
   * - 若被删的会话正是当前打开的会话，需让父组件（ConversationList/ChatWindow）
   *   检测 activeConv 是否仍存在并切到 null，避免打开空窗口。
   * - 仅本地清理，不影响对方聊天记录（前端弹窗二次确认）。
   */
  async function deleteConversation(convId: string) {
    const prevConvs = conversations.value;
    const prevMessages = messages.value[convId];
    conversations.value = conversations.value.filter((c) => c.id !== convId);
    mentionedConvs.value.delete(convId);
    // 清空该会话的内存消息缓存，避免下次打开时短暂闪烁旧数据
    const nextMessages = { ...messages.value };
    delete nextMessages[convId];
    messages.value = nextMessages;
    pagesLoaded.delete(convId);
    try {
      await api.deleteConversation(convId);
    } catch (e) {
      // 回滚：恢复会话行与消息缓存
      conversations.value = prevConvs;
      if (prevMessages !== undefined) {
        messages.value = { ...messages.value, [convId]: prevMessages };
      }
      throw e;
    }
    // 若被删会话就是当前打开的会话，置空（调用方负责切换 UI）
    if (activeConv.value === convId) {
      activeConv.value = null;
      unreadJump.value = null;
    }
  }

  /**
   * 置顶/取消置顶会话（纯本地偏好，不广播不同步）。
   * 乐观更新 + 失败回滚：置顶是高频轻操作，等一次 IPC 往返才动列表会明显发顿。
   */
  async function setConversationPinned(convId: string, pinned: boolean) {
    const prevConvs = conversations.value;
    conversations.value = sortConversations(
      conversations.value.map((c) => (c.id === convId ? { ...c, pinned } : c)),
    );
    try {
      await api.setConversationPinned(convId, pinned);
    } catch (e) {
      conversations.value = prevConvs;
      throw e;
    }
  }

  /**
   * 撤回自己发的一条群消息（仅原作者，窗口 2 分钟）。
   *
   * 不做乐观更新：先在服务端成功（事件已发出）才改本地 —— 顺序反了会出现
   * 「本地显示已撤回、对端根本没收到」。事件回来会走 `onMessageRecalled` 统一改本地形态。
   */
  async function recallMessage(groupId: string, msgId: string) {
    await api.recallGroupMessage(groupId, msgId);
  }

  /** 发布群公告（仅群主）。正常入时间线（计未读、可通知），只是不随清空历史消失。 */
  async function publishAnnouncement(groupId: string, text: string) {
    const rec = await api.sendGroupAnnouncement(groupId, text);
    enqueueMessage(rec);
    await refreshAnnouncements();
  }

  // ---------------- 群公告（会话列表 📢 标记的数据源） ----------------
  // 为什么走一条专门的查询：会话列表要覆盖**全部**群，而 `chat.messages` 是 ~4 个会话的
  // LRU 缓存 —— 从它折叠公告覆盖不了大多数群。后端 `list_active_group_announcements`
  // 一条 SQL 全量折叠（公告 + 墓碑），前端缓存成 map。

  /** 当前生效的群公告：key = group_id。 */
  const activeAnnouncements = ref(new Map<string, { text: string; msgId: string }>());

  /** 全量刷新（一条 SQL，代价低）。 */
  async function refreshAnnouncements() {
    try {
      const list = await api.listActiveGroupAnnouncements();
      const next = new Map<string, { text: string; msgId: string }>();
      for (const a of list) next.set(a.groupId, { text: a.text, msgId: a.msgId });
      activeAnnouncements.value = next;
    } catch {
      /* 拉取失败保持旧值（下一条公告事件会再触发刷新） */
    }
  }

  /** 删除群公告（仅群主）：发墓碑 → 入 store（本端横幅即刻消失）→ 刷新标记。 */
  async function deleteAnnouncement(groupId: string, annId: string) {
    const rec = await api.deleteGroupAnnouncement(groupId, annId);
    enqueueMessage(rec);
    await refreshAnnouncements();
  }

  // ---------------- 群任务（Card kind，不在时间线上渲染，只在任务面板里折叠展示） ----------------
  // 为什么都要 `enqueueMessage(rec)`：面板里的列表是 `foldTodos(该会话的消息)` 折出来的，
  // 事件不进 store 就折不出来 —— 界面要等下次重新拉全量（= 重进会话）才刷新
  // （与置顶/公告同一条理由）。

  /** 新建一条群任务（任意成员）。description / images 可选。返回新建的消息记录（含 todo_id）。 */
  async function createTodo(
    groupId: string,
    title: string,
    assignees: string[],
    description?: string,
    images?: TodoImage[],
  ): Promise<MessageRecord> {
    const rec = await api.sendGroupTodo(groupId, title, assignees, description, images);
    enqueueMessage(rec);
    return rec;
  }

  /**
   * 更新一条群任务：改状态 / 改标题与指派人 / 删除。
   *
   * 前端把**整份定义**发过去（标题、指派人、状态一起）：定义层是"同 `todo_id` 取最新一份"
   * 的 LWW 寄存器，只发改动字段会让没带的字段被清空。`patch` 缺省沿用传进来的 `item`
   * （调用方从折出来的 `TodoItem` 里取值，本身就是最新的）。
   */
  async function updateTodo(
    groupId: string,
    item: { todoId: string; title: string; assignees: string[]; status: string; description?: string; images?: TodoImage[] },
    patch: Partial<{
      title: string;
      assignees: string[];
      status: string;
      deleted: boolean;
      description: string;
      images: TodoImage[];
      /** 显式归档：`true` = 手动归档（完成之后）。不传 = 保留库中原值。 */
      archived: boolean;
    }> = {},
  ) {
    const rec = await api.updateGroupTodo(groupId, item.todoId, {
      title: patch.title ?? item.title,
      assignees: patch.assignees ?? item.assignees,
      status: patch.status ?? item.status,
      deleted: patch.deleted ?? false,
      description: patch.description ?? item.description,
      images: patch.images ?? item.images,
      archived: patch.archived,
    });
    enqueueMessage(rec);
  }

  /** 置顶/取消置顶一条群消息（任意成员；静默事件，由置顶条体现）。 */
  async function pinMessage(groupId: string, msgId: string, pinned: boolean) {
    // ⚠️ **必须 enqueue 进 store**：置顶在界面上的呈现（顶部的置顶条）
    // 是 `foldPinned(该会话的全部消息)` 折叠出来的 —— 事件不进 store，
    // 折叠就看不到它，界面要等下次重新拉全量（= 重进会话）才刷新。
    const rec = await api.pinGroupMessage(groupId, msgId, pinned);
    enqueueMessage(rec);
  }

  // ---------------- 收藏 ----------------
  // 为什么放 store 而不是收藏面板的本地 ref：收藏**跨会话、全局可见**（面板只是一个视图），
  // 且"收藏 / 取消收藏"从消息菜单与面板两处发起 —— 状态藏在面板里，另一处就看不到变化。
  const favorites = ref<FavoriteEntry[]>([]);

  async function refreshFavorites() {
    // 「收藏 / 取消收藏」从消息菜单与面板两处发起 ⇒ 不加闸的话，先发起的后回来会把
    // 刚收藏的那条从面板里抹掉（用户看到"点了星号又没了"）。
    const tok = refreshGuard.begin("favorites");
    const list = await api.listFavorites();
    if (!refreshGuard.isCurrent("favorites", tok)) return;
    favorites.value = list;
  }

  /**
   * 收藏一条消息。返回 `true` = 这次是**新增**，`false` = 早就在收藏里了。
   *
   * 内容与媒体副本一律由后端决定（前端只给 msg_id/conv_id，详见 `add_favorite` 的命令说明）。
   * 判断"是否新增"用**条目 id 是否已在列表里**而不是条数变化：列表可能压根还没加载过
   * （length 为 0），拿长度比会把重复收藏误报成新增。
   */
  async function addFavorite(msgId: string, convId: string): Promise<boolean> {
    const rec = await api.addFavorite(msgId, convId);
    if (favorites.value.some((f) => f.id === rec.id)) return false;
    favorites.value = [rec, ...favorites.value];
    return true;
  }

  /** 取消收藏。幂等：已经不在了也不报错（可能另一个窗口刚删过）。 */
  async function removeFavorite(id: string) {
    await api.removeFavorite(id);
    favorites.value = favorites.value.filter((f) => f.id !== id);
  }

  /**
   * 批量收藏（多选 → 收藏）。逐条调后端，最后刷一次列表。
   *
   * 为什么串行而不是 `Promise.all`：图片/文件类收藏每条都要把媒体**复制一份**到收藏目录，
   * 并发 100 个文件拷贝只会把磁盘和 db 锁打满；串行慢一点但稳，且每条的结果可数。
   * 单条失败**不中断整批**（已成功的不该被回滚），但失败条数如实返回给调用方报出来。
   */
  async function addFavorites(
    msgIds: string[],
    convId: string,
  ): Promise<{ added: number; already: number; failed: number }> {
    let added = 0;
    let already = 0;
    let failed = 0;
    for (const id of msgIds) {
      try {
        if (await addFavorite(id, convId)) added += 1;
        else already += 1;
      } catch {
        failed += 1;
      }
    }
    await refreshFavorites();
    return { added, already, failed };
  }

  /**
   * 本地删除若干条消息（多选 → 删除）。
   *
   * 乐观移除 + 失败回滚：删除必须"立刻见效"，等 IPC 回来才更新列表会有明显停顿；
   * 而失败时（例如后端按 kind 拒掉了非聊天内容）要把列表恢复原状 —— 不能让界面停在
   * "看起来删掉了、一刷新又回来"的状态。
   *
   * 会话摘要与未读由**后端重算**（`delete_messages`），这里删完拉一次会话列表即可：
   * 前端再实现一套"重算摘要"就是把同一口径复制到第二个地方。
   */
  async function deleteMessages(convId: string, msgIds: string[]): Promise<number> {
    if (msgIds.length === 0) return 0;
    const prevMsgs = messages.value[convId] ?? [];
    const prevConvs = conversations.value;
    const set = new Set(msgIds);
    messages.value = { ...messages.value, [convId]: prevMsgs.filter((m) => !set.has(m.msg_id)) };
    try {
      const n = await api.deleteMessages(msgIds);
      await refreshConversations();
      return n;
    } catch (e) {
      messages.value = { ...messages.value, [convId]: prevMsgs };
      conversations.value = prevConvs;
      throw e;
    }
  }

  /**
   * 发一条表情回应（群聊）。
   *
   * 走与普通消息**完全相同**的可靠管道（E2EE + outbox + GroupAck + 去重 + 离线补发），
   * 只是接收端会按 kind 归类为静默事件。本地**不做乐观上屏**：回应是幂等的状态事件，
   * 折叠逻辑已经能正确处理重复，等服务端回执再合并反而更简单、也不会出现
   * "点了没反应但本地已高亮"的错觉。
   */
  async function sendReaction(groupId: string, target: string, emoji: string, add: boolean) {
    const rec = await api.sendGroupReaction(groupId, target, emoji, add);
    enqueueMessage(rec);
  }

  async function createGroup(name: string, members: string[]) {
    const g = await api.createGroup(name, members);
    await api.distributeGroupKey(g.id);
    await refreshGroups();
    await refreshConversations();
    return g;
  }

  /** 重命名群（仅群主）。乐观更新会话标题，失败回滚。 */
  async function renameGroup(groupId: string, name: string) {
    const convId = `group:${groupId}`;
    const prevConvs = conversations.value;
    conversations.value = conversations.value.map((c) => (c.id === convId ? { ...c, name } : c));
    try {
      await api.renameGroup(groupId, name);
      await refreshGroups();
      await refreshConversations();
    } catch (e) {
      conversations.value = prevConvs;
      throw e;
    }
  }

  /**
   * 群操作的**乐观更新**骨架（用户 2026-09-12 要求「所有异步操作尽量乐观更新」）。
   *
   * 原先四个群操作都是「await 后端 → await refreshGroups → await refreshConversations」：
   * 点一下要等 **3 个 IPC 往返**才看到变化（成员列表、群主标识都不动），
   * 与「不阻断渲染」的要求相反。这里统一成与 `renameGroup` 相同的范式：
   * **先本地改（可感知即时）→ 调后端 → 失败回滚 + 抛错**（调用方负责 toast）。
   * 后端仍是权威：成功后会 refresh 一次收敛（成员/会话行以后端为准）。
   */
  function withGroupRollback<T>(
    mutate: () => () => void,
    call: () => Promise<T>,
  ): Promise<T> {
    const rollback = mutate();
    return call().catch((e) => {
      rollback();
      throw e;
    });
  }

  /** 加人入群（群主）。 */
  async function addGroupMember(groupId: string, deviceId: string) {
    await withGroupRollback(
      () => {
        const prev = groups.value;
        groups.value = groups.value.map((g) =>
          g.id === groupId && !g.members.includes(deviceId)
            ? { ...g, members: [...g.members, deviceId] }
            : g,
        );
        return () => {
          groups.value = prev;
        };
      },
      () => api.groupAddMember(groupId, deviceId),
    );
    await refreshGroups();
    await refreshConversations();
  }

  /** 移除成员（群主）。 */
  async function removeGroupMember(groupId: string, deviceId: string) {
    await withGroupRollback(
      () => {
        const prev = groups.value;
        groups.value = groups.value.map((g) =>
          g.id === groupId ? { ...g, members: g.members.filter((m) => m !== deviceId) } : g,
        );
        return () => {
          groups.value = prev;
        };
      },
      () => api.groupRemoveMember(groupId, deviceId),
    );
    await refreshGroups();
    await refreshConversations();
  }

  /** 转让群主（仅当前群主）。后端会向全体成员广播新群主。 */
  async function transferGroupCreator(groupId: string, newCreator: string) {
    await withGroupRollback(
      () => {
        const prev = groups.value;
        groups.value = groups.value.map((g) =>
          g.id === groupId ? { ...g, creator: newCreator } : g,
        );
        return () => {
          groups.value = prev;
        };
      },
      () => api.transferGroupCreator(groupId, newCreator),
    );
    await refreshGroups();
  }

  /** 退出群聊（群主须先转让）。退出后复用「被移出群」的本地清理路径。 */
  async function leaveGroup(groupId: string) {
    const convId = `group:${groupId}`;
    const prevGroups = groups.value;
    const prevConvs = conversations.value;
    const prevActive = activeConv.value;
    const prevMessages = messages.value[convId];
    // 乐观：立刻把群从本地移除并关掉会话（用户点了"退出"就该马上看到结果）
    groups.value = groups.value.filter((g) => g.id !== groupId);
    conversations.value = conversations.value.filter((c) => c.id !== convId);
    if (activeConv.value === convId) {
      activeConv.value = null;
      unreadJump.value = null;
    }
    try {
      await api.leaveGroup(groupId);
    } catch (e) {
      // 回滚：退群失败（例如群主未转让）时把群与会话原样放回
      groups.value = prevGroups;
      conversations.value = prevConvs;
      activeConv.value = prevActive;
      if (prevMessages !== undefined) messages.value = { ...messages.value, [convId]: prevMessages };
      throw e;
    }
    await handleSelfRemovedFromGroup(groupId);
  }

  /** 自己被移出群：关闭该会话并刷新。 */
  async function handleSelfRemovedFromGroup(groupId: string) {
    const convId = `group:${groupId}`;
    if (activeConv.value === convId) {
      activeConv.value = null;
      unreadJump.value = null;
    }
    delete messages.value[convId];
    await refreshGroups();
    await refreshConversations();
  }

  /** 统一文件发送：后端自动路由（有直连走直连，无直连自动中继）。 */
  async function sendFileTo(convId: string, path: string) {
    if (convId.startsWith("group:")) return null;
    const name = path.split(/[\\/]/).pop() ?? t("common.file");
    try {
      const id = await api.sendFileAuto(convId, path);
      void refreshTransfers();
      // 后端 send_file 会在返回前同步 emit `message-received`（携带完整的
      // name/size/subtype/path 记录），前端不再手工拼一个只有 name 的乐观气泡，
      // 避免与真实记录因同 msg_id 去重竞态而丢失元数据。
      return id;
    } catch (e) {
      // API 失败：插入明确 failed 状态的文件消息，不让消息永久停在 sending
      enqueueMessage({
        id: -1,
        msg_id: `file-failed-${Date.now()}`,
        conv_id: convId,
        sender_id: app.device?.device_id ?? "",
        receiver_id: convId,
        kind: "file",
        content: JSON.stringify({ name }),
        ts: Date.now(),
        seq: Number.MAX_SAFE_INTEGER,
        status: "failed",
      });
      app.toastError(e, t("send.fileFail"));
      return null;
    }
  }

  /** 清除聊天数据：后端清库（含群聊删除边界）后重置本 store 全部会话状态。
   *  activeConv 必须置 null——否则左侧无选中而右侧仍显示失效 ChatWindow。 */
  async function clearAllData() {
    await api.clearAllData();
    messages.value = {};
    conversations.value = [];
    groups.value = [];
    groupReads.value = {};
    activeConv.value = null;
    mentionedConvs.value = new Set();
    // 与 `resetAfterDataCleared` **同一组**（那边清的是同三张表）：后端把库清了，
    // 这些"等落地/等点击"的过渡态再留着就是给已不存在的记录重建条目。
    // 少清任何一张，两条清除路径就会走出两种不同的残留表现。
    pendingAcks.clear();
    pendingReplace.clear();
    notifMap.clear();
    void refreshTransfers();
  }


  /** 群文件发送：走群文件链路（Offer → Chunk → Done → CompleteAck）。
   *  群文件气泡/进度展示留待后续阶段，本封装只负责触发后端传输。 */
  async function sendGroupFileTo(groupId: string, path: string) {
    try {
      const id = await api.sendGroupFile(groupId, path);
      void refreshTransfers();
      return id;
    } catch (e) {
      app.toastError(e, t("send.groupFileFail"));
      return null;
    }
  }

  /** 粘贴图片发送：data URL → 本地文件 → 文件传输（单聊/群聊复用同一条可靠链路）。
   *  发送初始化失败时删除已保存的孤儿图片。 */
  async function sendImage(convId: string, dataUrl: string) {
    const { path } = await api.saveOutgoingImage(dataUrl);
    try {
      if (convId.startsWith("group:")) {
        const id = await api.sendGroupFile(convId.slice(6), path);
        void refreshTransfers();
        return id;
      }
      const id = await api.sendFileAuto(convId, path);
      void refreshTransfers();
      return id;
    } catch (e) {
      // 初始化失败：清理孤儿图片，避免 downloads 目录堆积垃圾
      await api.deleteFile(path).catch(() => {});
      app.toastError(e, t("send.imageFail"));
      return null;
    }
  }

  /**
   * 处于「停滞/等待」的 transfer → 原因文案（按 writer 实发判定，后端只在状态翻转时发一次事件）。
   *
   * 为什么单独存而不下到 `transfers[]` 行上：`refreshTransfers()` 会用后端快照**整体替换**
   * 那个数组，行上的临时标记会被无声冲掉；停滞是"这次尝试"的状态，必须活得比快照久。
   *
   * 值就是原因（`undefined` = 普通的链路静默，界面回退到既有的「网络停滞」文案）。
   * 用 Map 而不是 Set：只有蓝牙链路时大文件会"保持 pending 等 LAN"（后端
   * `refuse_reason_for_best_link`），那句原因必须落在同一个集合里 —— 分两处存就一定会有
   * 一边忘了清（表现为"停滞"标签一直挂着）。
   */
  const stalledTransfers = ref<Map<string, string | undefined>>(new Map());
  function isTransferStalled(id: string) {
    return stalledTransfers.value.has(id);
  }
  function transferStallReason(id: string): string | undefined {
    return stalledTransfers.value.get(id);
  }
  function setTransferStalled(id: string, stalled: boolean, reason?: string) {
    const next = reason ?? undefined;
    // 幂等：后端在每次心跳 flush 都会重发这条（"只有蓝牙"是持续状态），值没变就不触发响应式
    if (stalledTransfers.value.has(id) === stalled && (!stalled || stalledTransfers.value.get(id) === next))
      return;
    const m = new Map(stalledTransfers.value);
    if (stalled) m.set(id, next);
    else m.delete(id);
    stalledTransfers.value = m;
  }
  function onFileStalled(p: FileStalledInfo) {
    setTransferStalled(p.transfer_id, p.stalled, p.reason ?? undefined);
  }

  function updateTransferProgress(p: FileProgress) {
    const t = transfers.value.find((x) => x.id === p.transfer_id);
    if (t) {
      t.progress = p.total > 0 ? p.received / p.total : 0;
      return;
    }
    // 找不到行**不能静默丢**：进度是节流发的（250ms 一次），丢掉一次就可能再也没有下一次。
    scheduleTransfersRepair();
  }
  function onFileDone(d: FileDoneInfo) {
    // 终态一律收回「网络停滞」提示：后端只在翻转时发事件，漏一条就会让下次重试显示旧状态。
    setTransferStalled(d.transfer_id, false);
    const t = transfers.value.find((x) => x.id === d.transfer_id);
    if (t) {
      t.status = "done";
      t.path = d.path;
      t.progress = 1;
    } else {
      scheduleTransfersRepair();
    }
    // 字节刚落盘：让这条消息的预览缓存失效 —— 收到图片时可能"消息先到、字节后到"，
    // 在途读预览会得到"仍在接收"；不失效就不会重读，图片只能靠重发才出来。
    invalidateFilePreview(`file-${d.transfer_id}`);
    invalidateFilePreview(`gfile-${d.transfer_id}`);
  }
  function onFileFailed(d: FileFailedInfo) {
    setTransferStalled(d.transfer_id, false);
    const msgId = `file-${d.transfer_id}`;
    for (const [convId, list] of Object.entries(messages.value)) {
      const i = list.findIndex((m) => m.msg_id === msgId);
      if (i >= 0) {
        messages.value[convId] = [
          ...list.slice(0, i),
          { ...list[i], status: "failed" },
          ...list.slice(i + 1),
        ];
        break;
      }
    }
    app.toast(`${t("send.fileFail")}：${d.reason}`, "error");
  }

  /**
   * 收到撤回事件：把本地那一行改成「已撤回」形态。
   *
   * 与后端的物化视图保持一致：`kind` 改 `recalled`、`content` 清空 ——
   * 这样搜索、预览、复制等所有读 content 的地方**一处都不用改**就自动正确。
   */
  function onMessageRecalled(msgId: string) {
    for (const [convId, list] of Object.entries(messages.value)) {
      const idx = list.findIndex((m) => m.msg_id === msgId);
      if (idx < 0) continue;
      if (list[idx].kind === "recalled") return; // 幂等：重复事件不再改动
      const next = [...list];
      next[idx] = { ...next[idx], kind: "recalled", content: "" };
      messages.value = { ...messages.value, [convId]: next };
      // 会话列表的预览若正是这条，也要跟着清掉（否则左侧仍显示已被撤回的正文）
      const conv = conversations.value.find((c) => c.id === convId);
      if (conv && conv.last_msg && list[idx].content) {
        const preview = previewText(list[idx]);
        if (conv.last_msg === preview || conv.last_msg === preview.slice(0, 30)) {
          conversations.value = conversations.value.map((c) =>
            c.id === convId ? { ...c, last_msg: t("msg.recalled") } : c,
          );
        }
      }
      return;
    }
  }

  function onGroupRead(p: GroupReadInfo) {
    const readers = groupReads.value[p.group_id] ?? {};
    const current = readers[p.reader_id] ?? 0;
    if (p.last_read_ts <= current) return;
    groupReads.value = {
      ...groupReads.value,
      [p.group_id]: { ...readers, [p.reader_id]: p.last_read_ts },
    };
  }

  async function init() {
    // 重复初始化守卫：先拆掉上一轮注册的资源，再注册这一轮的。
    // 触发场景是 HMR（App.vue 热替换 → onMounted 再跑一次 init）与任何将来的
    // 二次调用；生产冷启动只有一轮，拆的是 `null`。
    chatInitScope?.dispose();
    const scope = createInitScope();
    chatInitScope = scope;
    // ⚠️ 事件绑定必须先于一切初始刷新（2026-09-23 审计 1.5）：Tauri `listen`
    // 不回放历史事件 —— 排在刷新之后绑定，启动窗口期（"打开就收消息"是局域网
    // 高频场景）emit 的系统通知/新消息/在线状态会**永久丢失**，且无任何纠正路径。
    // bindEvents 内部是并行 listen，调用瞬间 IPC 即发出，不等往返即可开始拉数据。
    //
    // 事件绑定自身也要兜底（审计 1.6）：不 catch 的话它 reject 会拖着 init 一起
    // reject —— 界面看起来正常，却永远收不到任何事件（"活着但功能全死"）。
    // 会话打开期间收到新消息：去抖标记已读（同时把已读回执发给对方 → 对方绿勾）
    let markReadTimer: ReturnType<typeof setTimeout> | null = null;
    const debounceMarkRead = (convId: string) => {
      if (markReadTimer) clearTimeout(markReadTimer);
      markReadTimer = setTimeout(() => {
        markReadTimer = null;
        // ⚠️ 三个条件缺一不可：得是这个会话、应用在前台、**而且聊天视图真的可见**
        // （移动端可能正盖着设置/新的朋友等整页浮层 —— 那时用户根本没看到这条消息，
        //  判已读等于替用户撒谎、还会把回执发回去。用户 2026-09-12 实测报告）。
        if (activeConv.value !== convId || document.hidden || !app.chatVisible) return;
        void api.markRead(convId).then(() => clearUnreadLocally(convId));
      }, 300);
    };
    // 去抖里还排着一次 markRead：拆掉它，否则一个绑在废弃实例上的已读回执会在
    // 300ms 后发出去（用户已经切走，等于替他判了已读）。
    scope.onDispose(() => {
      if (markReadTimer) clearTimeout(markReadTimer);
    });
    bindEvents({
      onPeers: (p) => {
        // 这个事件最多每秒 3 次：不比对就直接换表，等于每 333ms 把整屏消息行重画一遍
        // （消息行模板里的 nicknameOf 读的就是 peers）。没变就不写。
        const merged = mergePeerList(peers.value, p);
        if (merged) peers.value = merged;
        const onlineIds = new Set(p.map((x) => x.device_id));
        // 有活跃链路的节点即使在广播里缺席（局域网丢广播 / 刚被 sweep）也算在线，
        // 与后端 get_friends 的 friend_is_online 口径一致 ——
        // 否则会「局域网明明连上了，在线状态却不实时/显示离线」。
        const linkedIds = new Set(p.filter((x) => x.link).map((x) => x.device_id));
        friends.value.forEach(
          (f) => (f.online = onlineIds.has(f.device_id) || linkedIds.has(f.device_id)),
        );
        // 同步好友/单聊会话的昵称/头像（对方改名后立即生效）
        syncProfileFromPeers(friends.value, conversations.value, p);
        // 拓扑（节点数/中继数/平均 RTT/在线）变化很慢，而 peers-updated 最多 3/s；
        // 每个事件都发一次 IPC 纯属浪费（每次 IPC 都要跨进程 + 过主线程消息循环，
        // 攒起来就是"顿"）。这里节流到最多 1s 一次，另有 5s 定时器兜底。
        refreshTopologyThrottled();
      },
      onFriendRequest: (req) => {
        // 去重：同一设备多次申请只保留最新一条（过滤历史重复申请）
        rawPendingRequests.value = [
          req,
          ...rawPendingRequests.value.filter((r) => r.from !== req.from),
        ];
      },
      onFriendAccepted: async () => {
        await refreshFriends();
        await refreshConversations();
      },
      onFriendRejected: () => {},
      onFriendRemoved: async () => {
        await refreshFriends();
      },
      onFriendMessageBlocked: () => {
        app.toast(t("send.notFriend"), "error");
      },
      onMessage: (rec) => {
        enqueueMessage(rec);
        maybeNotify(rec);
        // 公告发布/删除：刷新「会话列表 📢 标记」的数据源（一条 SQL 全量折叠，代价低）。
        if (rec.kind === "announcement" || rec.kind === "announcement_delete") {
          void refreshAnnouncements();
        }
        if (rec.kind === "file" || rec.kind === "image") void refreshTransfers();
        // 正在看这个会话且窗口可见 → 自动已读并回执
        if (rec.sender_id !== myDeviceId.value && rec.conv_id === activeConv.value) {
          debounceMarkRead(rec.conv_id);
        }
      },
      onMessageAcked: (msgId) => {
        // 对方收到（Ack）：sending → delivered（空圆框）。经 outbox 补发的重试 Ack
        // 可能晚于 peer-read 到达，只允许前进不允许把绿勾退回空圆框。
        let found = false;
        for (const [convId, list] of Object.entries(messages.value)) {
          const i = list.findIndex((m) => m.msg_id === msgId);
          if (i >= 0) {
            messages.value[convId] = [
              ...list.slice(0, i),
              { ...list[i], status: furthestStatus(list[i].status, "delivered") },
              ...list.slice(i + 1),
            ];
            found = true;
            break;
          }
        }
        // Ack 先于乐观→真实替换到达（store 里找不到 real msg_id）：
        // 暂存 Ack，send() 拿到真实记录后立即消费。
        if (!found) {
          pendingAcks.add(msgId);
          trimOldest(pendingAcks, PENDING_ACKS_MAX);
        }
      },
      onPeerRead: (p) => {
        // 对方已读到 last_read_ts：我发出的、ts ≤ 该值的消息 → read（绿勾）
        for (const [convId, list] of Object.entries(messages.value)) {
          if (convId !== p.peer_id) continue;
          let changed = false;
          const next = list.map((m) => {
            if (m.sender_id === myDeviceId.value && m.status !== "read" && m.ts <= p.last_read_ts) {
              changed = true;
              return { ...m, status: "read" as const };
            }
            return m;
          });
          if (changed) messages.value[convId] = next;
        }
      },
      onGroupRead,
      onMessageRecalled,
      onFileProgress: (p) => {
        // 进度由事件载荷直接更新，不再全量刷新传输列表（避免大文件 IPC 风暴卡死界面）
        updateTransferProgress(p);
      },
      // 链路停滞（对端长时间没再收任何一片）：气泡文案要从「发送中 63%」变成
      // 「网络停滞」，否则用户只会觉得软件卡死 —— 后端在背压里等，前端什么都不知道。
      onFileStalled,
      onFileDone: (d) => {
        onFileDone(d);
        void refreshTransfers();
      },
      onFileFailed: (d) => {
        onFileFailed(d);
        void refreshTransfers();
      },
      onFileCancelled: (transferId) => {
        // 后端 cancel_file_transfer emit 的：用户手动取消了一条文件发送
        // 前端把这条消息标记 failed（如果还在发送中）+ 刷新列表
        for (const [, msgs] of Object.entries(messages.value)) {
          const msg = msgs.find(m =>
            m.msg_id === `file-${transferId}` || m.msg_id === `gfile-${transferId}`
          );
          if (msg) {
            msg.status = "failed";
            app.toast(t("msg.canceled"), "info");
          }
        }
        void refreshTransfers();
      },
      onMessageStatusChanged: (msgId) => {
        // 后端 emit 的状态变更事件 —— 我们直接从数据库拉最新状态覆盖本地
        // 目前只在 cancel_file_transfer 里发（cancel 后 mark failed，前端同步一下）
        // 只处理**活跃**会话：loadMessages 的结果对非活跃会话必被 activeConv
        // 守卫丢弃（两次 IPC 纯浪费），而且切换回来时 openConversation 会重查。
        // （修复前它还会顺带把全局 loadSeq +1，取消活跃会话飞行中的加载 →
        //   骨架永久转圈，见 2026-09-23 审计 1.4。）
        const convId = activeConv.value;
        if (convId && messages.value[convId]?.some((m) => m.msg_id === msgId)) {
          void loadMessages(convId);
        }
      },
      onPeerStyle: (p) => {
        app.applyPeerStyle(p.device_id, p.style);
      },
      onGroupsUpdated: () => {
        // 群密钥建群 / 群改名 / 成员变更：本地群表与会话标题可能都变了
        void refreshGroups();
        void refreshConversations();
      },
      onGroupMemberRemoved: (groupId) => {
        // 被移出群：后端已删本地群，前端关闭会话 + 刷新
        void handleSelfRemovedFromGroup(groupId);
      },
      onContentAudience: (p) => {
        // 消息已经发出去了，这里只是解释「群里有成员版本较旧，会把这条看成一段原始文本」。
        // 整句由后端拼（判据与文案在同一处），所以这里不再走 i18n、也不再自己组句子。
        app.toast(p.hint, "info");
      },
      onDataCleared: () => {
        void resetAfterDataCleared();
      },
      // 桌面端「点了系统通知」：后端已经唤起主窗口，这里只切界面（见 routeNotificationClick）。
      onNotificationClicked: (p) => {
        routeNotificationClick(p.type, p.conv_id);
      },
    }).then((fns) => {
      for (const f of fns) scope.onDispose(f);
    }).catch((e) => {
      // 见 init 开头的说明：不兜底的话 init 会 reject，而骨架照常撤除 ——
      // 用户看到一个"正常"的界面，却永远收不到任何事件。必须留痕 + 可感知。
      console.error("[chat] bindEvents 失败：实时事件不可用", e);
      app.toast(t("chat.eventBindFail"), "error");
    });

    // 初始刷新：每个独立兜底（2026-09-23 审计 1.6）—— 任一失败只影响该数据源
    // （对应事件/下次刷新会补上），不得阻断其他刷新，更不得阻断已注册的监听。
    const names = [
      "refreshFriends",
      "refreshConversations",
      "refreshPending",
      "refreshGroups",
      "refreshTransfers",
      "refreshPeers",
      "refreshTopology",
      "refreshAnnouncements",
    ] as const;
    const results = await Promise.allSettled([
      refreshFriends(),
      refreshConversations(),
      refreshPending(),
      refreshGroups(),
      refreshTransfers(),
      refreshPeers(),
      refreshTopology(),
      refreshAnnouncements(),
    ]);
    results.forEach((r, i) => {
      if (r.status === "rejected") {
        console.error(`[chat] 初始刷新 ${names[i]} 失败`, r.reason);
      }
    });

    // 恢复上次打开的会话（若仍存在）。HIG State Restoration：重启后回到上次离开的地方。
    // 用 void 触发：不阻塞 init，也避免其异步失败拖垮启动。
    try {
      const last = localStorage.getItem(LAST_CONV_KEY);
      if (last && conversations.value.some((c) => c.id === last)) {
        void openConversation(last);
      }
    } catch {
      /* localStorage 不可用则跳过恢复 */
    }

    // 移动端注册通知动作类别（「标记已读」按钮）。桌面端无此能力（Web Notification 不支持按钮），
    // 命令也不存在，故只对移动端调用。语言切换后按钮文案不随动（原生注册一次），可接受。
    if (app.isMobile) {
      void registerActionTypes([
        {
          id: "chat",
          actions: [{ id: "mark-read", title: t("notification.markRead") }],
        },
      ]).catch(() => {
        /* 注册失败不影响通知主体（只是没有动作按钮） */
      });
    }
    // 注册系统通知点击回调：点击通知 → 唤起窗口 + 定位到发送者会话
    onAction((n) => {
      // 兼容不同平台回调形状：对象 { id, actionId, extra } 或裸 id（number/string）
      const raw = (typeof n === "object" && n !== null
        ? n
        : { id: n }) as {
        id?: unknown;
        actionId?: string;
        extra?: Record<string, unknown>;
      };
      const id = typeof raw.id === "number" ? raw.id : undefined;
      const extraType = raw.extra?.type as string | undefined;

      // 「标记已读」动作按钮：不唤起窗口，只把该会话标为已读（发已读回执 + 清未读角标）
      if (raw.actionId === "mark-read") {
        let convId = id != null ? notifMap.get(id) : undefined;
        if (!convId && raw.extra?.conv_id) convId = String(raw.extra.conv_id);
        if (id != null) notifMap.delete(id);
        if (convId) {
          void api.markRead(convId).then(() => clearUnreadLocally(convId));
        }
        return;
      }

      // 点击通知本体：无论能否解析出会话/类型，先把窗口弹到前台
      // （最小化/隐藏/被遮挡时都恢复，unminimize+show+set_focus 幂等）
      void api.focusWindow();

      let convId = id != null ? notifMap.get(id) : undefined;
      if (!convId && raw.extra?.conv_id) convId = String(raw.extra.conv_id);
      if (id != null) notifMap.delete(id);
      routeNotificationClick(extraType, convId);
    }).then((listener) => {
      // `onAction` 返回的是 PluginListener 对象（要显式 `unregister()`），
      // 之前那句 `void onAction(...)` 把返回值直接丢了 —— 回调永久挂在插件上。
      scope.onDispose(() => {
        listener.unregister().catch(() => {
          /* 插件 IPC 抖动，不牵连其它卸载器 */
        });
      });
    }).catch((e) => {
      // 注册失败只影响"点通知跳转"，不影响其余事件；留痕而不是静默吞掉。
      console.error("[chat] 通知点击回调注册失败：点系统通知不会定位到会话", e);
    });
    // 定时刷新拓扑
    const topoTimer = setInterval(() => void refreshTopology(), 5000);
    scope.onDispose(() => clearInterval(topoTimer));
    // 窗口重新可见：补发当前会话已读回执 + 冲刷后台期间滞留的消息批次
    // （具名函数 + 成对 removeEventListener：匿名 handler 摘不掉，重复 init 会叠加）
    const onVisibility = () => {
      if (document.hidden) return;
      // 强制冲刷而不是 scheduleFlush()（2026-09-23 审计 1.7）：后者会被
      // `if (flushScheduled) return` 挡回 —— rAF 在后台被暂停/丢失时 flag 卡在
      // true，兜底就永远是死代码。flushNow 幂等且立即生效。
      if (pending.length) flushNow();
      // **自愈**：会话表与传输表是"事件只带 id、前端就地改内存"那一类事件的真相源
      //（`message-acked` / `message-failed` / `peer-read` / `file-*` / `file-cancelled`）。
      // 事件在后台丢了、或者窗口隐藏期间根本没投递到 ⇒ 不重拉就永久错下去：
      // 未读数、"已送达"勾、传输进度都可能停在一个从来没发生过的状态上。
      // 两件各自独立兜底（审计 1.6 同一条理由）：一边失败不影响另一边。
      void refreshConversations().catch((e) => {
        console.error("[chat] 回到前台重拉会话失败（未读数可能停留）", e);
      });
      void refreshTransfers().catch((e) => {
        console.error("[chat] 回到前台重拉传输表失败（进度可能停留）", e);
      });
      // 同 `debounceMarkRead`：回到前台也要确认"聊天视图真的可见"才补发已读回执
      if (activeConv.value && app.chatVisible) {
        // 刻意先把 id 取进局部变量：await 期间用户可能已经切走，回调里再读
        // `activeConv.value` 会把**新**会话的红点清掉而根本没给它发已读。
        const convId = activeConv.value;
        void api.markRead(convId).then(() => clearUnreadLocally(convId));
      }
    };
    document.addEventListener("visibilitychange", onVisibility);
    scope.onDispose(() => document.removeEventListener("visibilitychange", onVisibility));
  }

  return {
    peers,
    friends,
    pendingRequests,
    conversations,
    groups,
    transfers,
    contentTransfers,
    messages,
    groupReads,
    groupReaderIds,
    activeConv,
    mentionedConvs,
    topology,
    activeConversation,
    totalUnread,
    unreadJump,
    locateRequest,
    clearLocateRequest,
    locateMessageInConv,
    nicknameOf,
    init,
    refreshPeers,
    searchNearbyPeers,
    refreshFriends,
    refreshPending,
    refreshConversations,
    refreshGroups,
    resetAfterDataCleared,
    refreshTransfers,
    stalledTransfers,
    isTransferStalled,
    transferStallReason,
    refreshTopology,
    favorites,
    refreshFavorites,
    addFavorite,
    addFavorites,
    removeFavorite,
    deleteMessages,
    openConversation,
    loadMessages,
    loadMoreMessages,
    loadGroupTodos,
    todosLoadedOnce,
    watchGroupTodos,
    send,
    sendFriendRequest,
    respondRequest,
    removeFriend,
    deleteConversation,
    setConversationPinned,
    sendReaction,
    recallMessage,
    pinMessage,
    publishAnnouncement,
    deleteAnnouncement,
    refreshAnnouncements,
    activeAnnouncements,
    createTodo,
    updateTodo,
    createGroup,
    renameGroup,
    addGroupMember,
    removeGroupMember,
    transferGroupCreator,
    leaveGroup,
    handleSelfRemovedFromGroup,
    sendFileTo,
    sendGroupFileTo,
    sendImage,
    clearAllData,
    enqueueMessage,
  };
});

// Vite HMR：**改了 store 必须让新 store 生效**。
//
// 踩坑（用户实测"点设置卡、过一会儿弹出好几个设置、主题延迟切换"）：Pinia 的 store 是
// 缓存过的单例，**不接 HMR 就一直是旧实例** —— 我这一轮给 store 新增了 `channels` /
// `channels`/`setChannelEnabled`，而用户长时间运行的 dev 会话里还是旧 store ⇒ 设置页里
// `app.setChannelEnabled is not a function`、`channels.value.find` 抛错 ⇒ **整页渲染卡死**
// （一个分区渲染抛错，Vue 之后再也 patch 不动这个页面）。加上这一行之后，
// 以后改 store 都不必重启 dev。
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useChatStore, import.meta.hot));
