import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { api, bindEvents } from "@/api";
import {
  applyIncomingToConversations,
  applyReplacements,
  furthestStatus,
  mergeMessages,
  messageMentionsName,
  preserveDeliveryStatus,
  previewText,
  selectCachedConversations,
  syncProfileFromPeers,
} from "@/utils/messages";
import { useAppStore } from "@/stores/useAppStore";
import { notificationBody } from "@/utils/notifications";
import { t } from "@/i18n";
import { shouldRunThrottled } from "@/utils/defer";
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
  Friend,
  Group,
  GroupReadInfo,
  MessageRecord,
  Peer,
  PendingRequest,
  TopologyInfo,
  TransferInfo,
} from "@/types";

/** 上次打开的会话（重启后恢复，纯前端 UI 状态，各端统一）。 */
const LAST_CONV_KEY = "gosslan.lastConv";

export const useChatStore = defineStore("chat", () => {
  const peers = ref<Peer[]>([]);
  const friends = ref<Friend[]>([]);
  const pendingRequests = ref<PendingRequest[]>([]);
  const conversations = ref<Conversation[]>([]);
  const groups = ref<Group[]>([]);
  const transfers = ref<TransferInfo[]>([]);
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
  const notifMap = new Map<number, string>();

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
  const notifyQueue = new Map<string, { count: number; last: MessageRecord }>();
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
      for (const { count, last } of entries) {
        // 窗口期间用户已切到该会话且前台 → 该会话跳过通知
        if (document.hasFocus() && activeConv.value === last.conv_id) continue;
        const title = nicknameOf(last.sender_id);
        const body = notifyBody(count, last);
        const convId = last.conv_id;
        if (app.isMobile) {
          // 移动端：plugin 通知（Android 有 actionPerformed 点击事件桥）
          const id = notifSeq++;
          notifMap.set(id, convId);
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
          // 桌面端：plugin 的 actionPerformed 事件桥仅在 iOS/Android 实现，
          // Windows/macOS 点击通知不会回调 onAction。桌面走 WebView 原生
          // Notification（plugin JS 端 sendNotification 底层同为此 API）：
          // Windows WebView2 下由系统通知中心显示，点击会激活宿主窗口并
          // 触发 onclick → focusWindow + openConversation。
          // macOS WKWebView 无此 API → 回退 plugin 通知（有提示、无点击，平台限制）。
          let n: Notification | null = null;
          try {
            n = new Notification(title, { body });
          } catch {
            n = null; // permission 异常等：回退 plugin，不让通知链静默失败
          }
          if (n) {
            n.onclick = () => {
              void handleNotificationClick(convId);
            };
          } else {
            const id = notifSeq++;
            notifMap.set(id, convId);
            void sendNotification({
              id,
              title,
              body,
              autoCancel: true,
              extra: { type: "chat", conv_id: convId },
            });
          }
        }
      }
    });
  }

  /**
   * 通知正文：按「显示消息内容」隐私开关决定是否带正文。
   * 关掉时只提示"收到新消息"（锁屏 / 通知中心不泄内容），标题仍保留发送者昵称。
   * 拼装逻辑在 utils/notifications.ts（纯函数、有单测），这里只喂入实时数据。
   */
  function notifyBody(count: number, last: MessageRecord): string {
    return notificationBody({
      showContent: app.notifyShowContent,
      count,
      sender: nicknameOf(last.sender_id),
      preview: previewText(last),
    });
  }

  function maybeNotify(rec: MessageRecord) {
    if (!app.notifyEnabled) return;
    const myId = app.device?.device_id;
    if (!myId || rec.sender_id === myId) return;
    // 应用在前台且正查看该会话 → 不通知（不进队列）
    if (document.hasFocus() && activeConv.value === rec.conv_id) return;
    queueNotification(rec);
  }

  async function handleNotificationClick(convId: string) {
    await api.focusWindow();
    await openConversation(convId);
    if (app.isMobile) app.mobileView = "chat";
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
  const pendingAcks = new Set<string>();

  function scheduleFlush() {
    if (flushScheduled) return;
    flushScheduled = true;
    const flush = () => {
      flushScheduled = false;
      const batch = pending;
      pending = [];
      void applyIncoming(batch);
    };
    // 后台/遮挡窗口的 requestAnimationFrame 会被浏览器暂停，导致消息滞留不渲染；
    // 窗口不可见时退回 setTimeout，保证任何状态下都能入列渲染。
    if (!document.hidden && typeof requestAnimationFrame === "function") {
      requestAnimationFrame(flush);
    } else {
      setTimeout(flush, 0);
    }
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
    const myName = app.device?.nickname ?? "";
    if (myName) {
      for (const [cid, fresh] of newByConv) {
        if (cid === activeConv.value || !cid.startsWith("group:")) continue;
        for (const rec of fresh) {
          if (rec.sender_id !== myDeviceId.value && messageMentionsName(rec, myName)) {
            mentionedConvs.value.add(cid);
            break;
          }
        }
      }
    }
    // 会话列表中不存在的会话（新好友 / 后端新创建）：本地合并不了，直接从后端拉取
    const knownIds = new Set(conversations.value.map((c) => c.id));
    const missing = [...byConv.keys()].filter((id) => !knownIds.has(id));
    for (const [convId, incoming] of byConv) {
      const existing = messages.value[convId] ?? [];
      messages.value[convId] = mergeMessages(existing, incoming);
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
    scheduleFlush();
  }

  // ---------------- 刷新 ----------------
  async function refreshPeers() {
    peers.value = await api.getPeers();
  }
  /** 按需探测：群发一次 who_has 后返回周围在线节点（添加好友时调用）。 */
  async function searchNearbyPeers() {
    peers.value = await api.searchNearbyPeers();
    const onlineIds = new Set(peers.value.map((x) => x.device_id));
    friends.value.forEach((f) => (f.online = onlineIds.has(f.device_id)));
    return peers.value;
  }
  async function refreshFriends() {
    friends.value = await api.getFriends();
  }
  async function refreshPending() {
    pendingRequests.value = await api.getPendingRequests();
  }
  async function refreshConversations() {
    conversations.value = await api.getConversations();
  }
  async function refreshGroups() {
    groups.value = await api.getGroups();
    const entries = await Promise.all(
      groups.value.map(async (group) => [group.id, await api.getGroupReads(group.id).catch(() => [])] as const),
    );
    const next: Record<string, Record<string, number>> = {};
    for (const [groupId, reads] of entries) {
      next[groupId] = Object.fromEntries(reads.map((read) => [read.reader_id, read.last_read_ts]));
    }
    groupReads.value = next;
  }

  function groupReaderIds(groupId: string, messageTs: number): string[] {
    const myId = app.device?.device_id;
    const members = new Set(groups.value.find((group) => group.id === groupId)?.members ?? []);
    return Object.entries(groupReads.value[groupId] ?? {})
      .filter(([readerId, lastReadTs]) => members.has(readerId) && readerId !== myId && lastReadTs >= messageTs)
      .map(([readerId]) => readerId);
  }
  async function refreshTransfers() {
    transfers.value = await api.getTransfers();
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
    topology.value = await api.getTopology();
  }

  /** 打开会话时的未读定位：记录第一条未读消息索引（-1 = 无未读，贴底显示）。 */
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
    const optimisticClearUnread = (convId: string) => {
      const conv = conversations.value.find((c) => c.id === convId);
      if (conv && conv.unread !== 0) conv.unread = 0;
    };
    optimisticClearUnread(id);
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
              conversations.value = [conv, ...conversations.value];
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
      const list = messages.value[id] ?? [];
      const idx = list.length - Math.min(unreadBefore, list.length);
      if (idx >= 0 && idx < list.length) {
        unreadJump.value = { convId: id, index: idx };
      } else {
        // 无可定位的未读（空列表等异常）→ 不跳，交给贴底兜底
        unreadJump.value = null;
      }
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
  // 加载竞态守卫：快速切换会话时丢弃过期响应
  let loadSeq = 0;

  // ---------------- 消息缓存上界：限制「同时缓存多少个会话」 ----------------
  //
  // 为什么需要：MAX_PAGES 只限制「一个会话能往上翻几页」，而 messages 是按会话累积的
  // 内存副本——聊天对象一多，几十个会话各留最多 1000 条，长期挂机内存会持续增长。
  //
  // 为什么可以安全淘汰非活跃会话：UI 只渲染活跃会话（ChatWindow 只读 activeConv 的
  // 消息列表），其它会话的缓存纯粹是内存副本，切回时由 loadMessages 从 SQLite 重新
  // 加载最新一页，因此丢弃不影响任何展示。
  //
  // 去重不受影响：后端 insert_message_if_new 只在「真的新建一行」时 emit
  // message-received（见 db.rs 注释），跨批次去重以它为权威；前端这份缓存只是二级保险。
  //
  // 仍未覆盖（已知遗留）：单个会话的实时新消息仍会不断追加，条数没有硬上界。
  // 未做是因为裁剪头部会改变 VirtualList 的滚动锚定（用户正向上翻阅时内容会跳动），
  // 需要与滚动状态联动，收益（每会话几 MB）不值这个回退风险。
  const MAX_CACHED_CONVS = 4;
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
    const seq = ++loadSeq;
    touchCacheOrder(convId);
    // 打开会话应先加载「最新一页」，而不是最旧一页；否则底部会停在第 100 条历史，
    // 最新消息与文件都要靠后续滚动才出现。
    let list: MessageRecord[];
    try {
      const total = await api.getMessageCount(convId);
      const offset = Math.max(0, total - PAGE_SIZE);
      list = await api.getMessages(convId, PAGE_SIZE, offset);
    } catch {
      // 读库失败也要把该会话标记为「已加载」：留 undefined 会让 ChatWindow 的加载骨架
      // 永远停在那里（UI 先行就得保证"必有终态"）。给空列表 = 回到"暂无消息"的正常态。
      if (messages.value[convId] === undefined) messages.value[convId] = [];
      return;
    }
    if (seq !== loadSeq || activeConv.value !== convId) return;
    // 快照可能取自 Ack / peer-read 落库之前：与查询期间已推进的内存状态合并，
    // 否则刚亮的绿勾会被这份旧快照退回「发送中」。
    const prev = messages.value[convId];
    messages.value[convId] = prev ? preserveDeliveryStatus(list, prev) : list;
    pagesLoaded.set(convId, 1);
  }

  /** 向上翻页加载更早的历史消息（VirtualList 触顶时调用）。 */
  async function loadMoreMessages(convId: string) {
    const pages = pagesLoaded.get(convId) ?? 1;
    if (pages >= MAX_PAGES) return;
    const seq = loadSeq;
    const total = await api.getMessageCount(convId);
    if (pages * PAGE_SIZE >= total) return;
    // 当前已加载最新 pages 页，继续向更早方向取一页。
    const offset = Math.max(0, total - (pages + 1) * PAGE_SIZE);
    const older = await api.getMessages(convId, PAGE_SIZE, offset);
    if (seq !== loadSeq || older.length === 0) return;
    const existing = messages.value[convId] ?? [];
    messages.value[convId] = mergeMessages(existing, older);
    pagesLoaded.set(convId, pages + 1);
    // prepend 历史后，「第一条未读」的索引整体后移（index=-1 占位态不参与）
    if (unreadJump.value?.convId === convId && unreadJump.value.index >= 0) {
      unreadJump.value = { ...unreadJump.value, index: unreadJump.value.index + older.length };
    }
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
    const prev = pendingRequests.value;
    pendingRequests.value = prev.filter((r) => r.from !== peerId);
    try {
      await api.respondFriendRequest(peerId, accept);
    } catch (e) {
      pendingRequests.value = prev; // 回滚
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
  async function sendFileRelayTo(convId: string, path: string) {
    if (convId.startsWith("group:")) return null;
    return api.sendFileRelay(convId, path);
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
    pendingAcks.clear();
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

  function updateTransferProgress(p: FileProgress) {
    const t = transfers.value.find((x) => x.id === p.transfer_id);
    if (t) t.progress = p.total > 0 ? p.received / p.total : 0;
  }
  function onFileDone(d: FileDoneInfo) {
    const t = transfers.value.find((x) => x.id === d.transfer_id);
    if (t) {
      t.status = "done";
      t.path = d.path;
      t.progress = 1;
    }
  }
  function onFileFailed(d: FileFailedInfo) {
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
    await Promise.all([
      refreshFriends(),
      refreshConversations(),
      refreshPending(),
      refreshGroups(),
      refreshTransfers(),
      refreshPeers(),
      refreshTopology(),
    ]);
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
    // 会话打开期间收到新消息：去抖标记已读（同时把已读回执发给对方 → 对方绿勾）
    let markReadTimer: ReturnType<typeof setTimeout> | null = null;
    const debounceMarkRead = (convId: string) => {
      if (markReadTimer) clearTimeout(markReadTimer);
      markReadTimer = setTimeout(() => {
        markReadTimer = null;
        if (activeConv.value !== convId || document.hidden) return;
        void api.markRead(convId).then(() => {
          const conv = conversations.value.find((c) => c.id === convId);
          if (conv) conv.unread = 0;
        });
      }, 300);
    };
    await bindEvents({
      onPeers: (p) => {
        peers.value = p;
        const onlineIds = new Set(p.map((x) => x.device_id));
        friends.value.forEach((f) => (f.online = onlineIds.has(f.device_id)));
        // 同步好友/单聊会话的昵称/头像（对方改名后立即生效）
        syncProfileFromPeers(friends.value, conversations.value, p);
        // 拓扑（节点数/中继数/平均 RTT/在线）变化很慢，而 peers-updated 最多 3/s；
        // 每个事件都发一次 IPC 纯属浪费（每次 IPC 都要跨进程 + 过主线程消息循环，
        // 攒起来就是"顿"）。这里节流到最多 1s 一次，另有 5s 定时器兜底。
        refreshTopologyThrottled();
      },
      onFriendRequest: (req) => {
        // 去重：同一设备多次申请只保留最新一条（过滤历史重复申请）
        pendingRequests.value = [
          req,
          ...pendingRequests.value.filter((r) => r.from !== req.from),
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
        if (!found) pendingAcks.add(msgId);
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
      onFileProgress: (p) => {
        // 进度由事件载荷直接更新，不再全量刷新传输列表（避免大文件 IPC 风暴卡死界面）
        updateTransferProgress(p);
      },
      onFileDone: (d) => {
        onFileDone(d);
        void refreshTransfers();
      },
      onFileFailed: (d) => {
        onFileFailed(d);
        void refreshTransfers();
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
    });
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
    void onAction((n) => {
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
          void api.markRead(convId).then(() => {
            const conv = conversations.value.find((c) => c.id === convId);
            if (conv) conv.unread = 0;
          });
        }
        return;
      }

      // 点击通知本体：无论能否解析出会话，先把窗口弹到前台
      // （最小化/隐藏/被遮挡时都恢复，unminimize+show+set_focus 幂等）
      void api.focusWindow();

      if (extraType === "friend_request") {
        // 好友申请通知：唤起窗口 + 切换到联系人视图
        if (id != null) notifMap.delete(id);
        void api.focusWindow().then(() => {
          window.dispatchEvent(new CustomEvent("navigate-to-contacts"));
        });
        return;
      }

      // 默认：聊天消息通知
      let convId = id != null ? notifMap.get(id) : undefined;
      if (!convId && raw.extra?.conv_id) convId = String(raw.extra.conv_id);
      if (id != null) notifMap.delete(id);
      if (convId) void handleNotificationClick(convId);
    });
    // 定时刷新拓扑
    setInterval(() => void refreshTopology(), 5000);
    // 窗口重新可见：补发当前会话已读回执 + 冲刷后台期间滞留的消息批次
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) return;
      if (pending.length) scheduleFlush();
      if (activeConv.value) {
        void api.markRead(activeConv.value).then(() => {
          const conv = conversations.value.find((c) => c.id === activeConv.value);
          if (conv) conv.unread = 0;
        });
      }
    });
  }

  return {
    peers,
    friends,
    pendingRequests,
    conversations,
    groups,
    transfers,
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
    refreshTransfers,
    refreshTopology,
    openConversation,
    loadMessages,
    loadMoreMessages,
    send,
    sendFriendRequest,
    respondRequest,
    removeFriend,
    deleteConversation,
    createGroup,
    renameGroup,
    addGroupMember,
    removeGroupMember,
    transferGroupCreator,
    leaveGroup,
    handleSelfRemovedFromGroup,
    sendFileTo,
    sendFileRelayTo,
    sendGroupFileTo,
    sendImage,
    clearAllData,
    enqueueMessage,
  };
});
