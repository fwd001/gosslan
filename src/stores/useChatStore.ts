import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { api, bindEvents } from "@/api";
import { applyIncomingToConversations, mergeMessages, previewText } from "@/utils/messages";
import { useAppStore } from "@/stores/useAppStore";
import {
  isPermissionGranted,
  onAction,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import type {
  Conversation,
  FileDoneInfo,
  FileProgress,
  Friend,
  Group,
  MessageRecord,
  Peer,
  PendingRequest,
  TopologyInfo,
  TransferInfo,
} from "@/types";

export const useChatStore = defineStore("chat", () => {
  const peers = ref<Peer[]>([]);
  const friends = ref<Friend[]>([]);
  const pendingRequests = ref<PendingRequest[]>([]);
  const conversations = ref<Conversation[]>([]);
  const groups = ref<Group[]>([]);
  const transfers = ref<TransferInfo[]>([]);
  const messages = ref<Record<string, MessageRecord[]>>({});
  const activeConv = ref<string | null>(null);
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
  let notifyPermission = false;
  let notifSeq = 1;
  const notifMap = new Map<number, string>();

  async function ensureNotifyPermission() {
    if (notifyPermission) return;
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    notifyPermission = granted;
  }

  function nicknameOf(id: string): string {
    const f = friends.value.find((x) => x.device_id === id);
    if (f) return f.nickname;
    const p = peers.value.find((x) => x.device_id === id);
    if (p) return p.nickname;
    return id;
  }

  const myDeviceId = computed(() => app.device?.device_id ?? "");

  function maybeNotify(rec: MessageRecord) {
    const myId = app.device?.device_id;
    if (!myId || rec.sender_id === myId) return;
    // 应用在前台且正查看该会话 → 不通知
    if (document.hasFocus() && activeConv.value === rec.conv_id) return;
    void ensureNotifyPermission().then(() => {
      if (!notifyPermission) return;
      const id = notifSeq++;
      notifMap.set(id, rec.conv_id);
      sendNotification({
        id,
        title: nicknameOf(rec.sender_id),
        body: previewText(rec),
        autoCancel: true,
        extra: { conv_id: rec.conv_id },
      });
    });
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

  async function applyIncoming(batch: MessageRecord[]) {
    if (batch.length === 0) return;
    const byConv = new Map<string, MessageRecord[]>();
    for (const m of batch) {
      const list = byConv.get(m.conv_id) ?? [];
      list.push(m);
      byConv.set(m.conv_id, list);
    }
    // 会话列表中不存在的会话（新好友 / 后端新创建）：本地合并不了，直接从后端拉取
    const knownIds = new Set(conversations.value.map((c) => c.id));
    const missing = [...byConv.keys()].filter((id) => !knownIds.has(id));
    for (const [convId, incoming] of byConv) {
      const existing = messages.value[convId] ?? [];
      messages.value[convId] = mergeMessages(existing, incoming);
    }
    if (missing.length > 0) {
      await refreshConversations();
    } else {
      conversations.value = applyIncomingToConversations(
        conversations.value,
        activeConv.value,
        byConv,
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
  }
  async function refreshTransfers() {
    transfers.value = await api.getTransfers();
  }
  async function refreshTopology() {
    topology.value = await api.getTopology();
  }

  /** 打开会话时的未读定位：记录第一条未读消息索引（-1 = 无未读，贴底显示）。 */
  const unreadJump = ref<{ convId: string; index: number } | null>(null);

  async function openConversation(id: string) {
    activeConv.value = id;
    unreadJump.value = null;
    // 会话行不存在（如新加好友还没发过消息）→ 后端补建，保证左侧列表有对应可高亮的项
    if (!conversations.value.some((c) => c.id === id) && !id.startsWith("group:")) {
      try {
        const conv = await api.ensureConversation(id);
        conversations.value = [conv, ...conversations.value];
      } catch {
        /* 忽略：不影响打开聊天 */
      }
    }
    // 打开前先记录未读数（markRead 会清零），用于「跳到第一条未读」定位
    const unreadBefore = conversations.value.find((c) => c.id === id)?.unread ?? 0;
    await loadMessages(id);
    if (unreadBefore > 0) {
      const list = messages.value[id] ?? [];
      const idx = list.length - Math.min(unreadBefore, list.length);
      if (idx >= 0 && idx < list.length) {
        unreadJump.value = { convId: id, index: idx };
      }
    }
    await api.markRead(id);
    const conv = conversations.value.find((c) => c.id === id);
    if (conv) conv.unread = 0;
  }

  // ---------------- 会话内消息分页：每会话最多缓存页数（防内存无限增长） ----------------
  const PAGE_SIZE = 100;
  const MAX_PAGES = 10;
  const pagesLoaded = new Map<string, number>();
  // 加载竞态守卫：快速切换会话时丢弃过期响应
  let loadSeq = 0;

  async function loadMessages(convId: string) {
    const seq = ++loadSeq;
    const list = await api.getMessages(convId, PAGE_SIZE, 0);
    if (seq !== loadSeq || activeConv.value !== convId) return;
    messages.value[convId] = list;
    pagesLoaded.set(convId, 1);
  }

  /** 向上翻页加载更早的历史消息（VirtualList 触顶时调用）。 */
  async function loadMoreMessages(convId: string) {
    const pages = pagesLoaded.get(convId) ?? 1;
    if (pages >= MAX_PAGES) return;
    const seq = loadSeq;
    const offset = pages * PAGE_SIZE;
    const older = await api.getMessages(convId, PAGE_SIZE, offset);
    if (seq !== loadSeq || older.length === 0) return;
    const existing = messages.value[convId] ?? [];
    messages.value[convId] = mergeMessages(existing, older);
    pagesLoaded.set(convId, pages + 1);
    // prepend 历史后，「第一条未读」的索引整体后移
    if (unreadJump.value?.convId === convId) {
      unreadJump.value = { ...unreadJump.value, index: unreadJump.value.index + older.length };
    }
  }

  /** 统一发送（单聊/群聊）。乐观上屏：先显示 sending，invoke 成功后替换为真实记录。 */
  async function send(convId: string, content: string, kind: string): Promise<MessageRecord> {
    const myId = app.device?.device_id ?? "";
    // 时间戳取「发送时刻」；并用会话内最新消息时间做下限钳制，
    // 避免设备间时钟偏差导致乐观消息排序到已收到消息之上。
    const lastTs = messages.value[convId]?.at(-1)?.ts ?? 0;
    const optimistic: MessageRecord = {
      id: -1,
      msg_id: `tmp-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      conv_id: convId,
      sender_id: myId,
      receiver_id: convId,
      kind: kind as MessageRecord["kind"],
      content,
      ts: Math.max(Date.now(), lastTs),
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
      replaceMessage(convId, optimistic.msg_id, rec);
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
    if (!list) return;
    const i = list.findIndex((m) => m.msg_id === msgId);
    if (i >= 0) {
      messages.value[convId] = [...list.slice(0, i), next, ...list.slice(i + 1)];
    }
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
  async function createGroup(name: string, members: string[]) {
    const g = await api.createGroup(name, members);
    await api.distributeGroupKey(g.id);
    await refreshGroups();
    await refreshConversations();
    return g;
  }

  /** 统一文件发送：后端自动路由（有直连走直连，无直连自动中继）。 */
  async function sendFileTo(convId: string, path: string) {
    if (convId.startsWith("group:")) return null;
    try {
      const id = await api.sendFileAuto(convId, path);
      void refreshTransfers();
      // 乐观上屏：拿到 transfer_id 即插入文件气泡（后端同 msg_id 的事件会被去重合并）；
      // 进度条随 file-progress 事件实时更新
      const name = path.split(/[\\/]/).pop() ?? "文件";
      enqueueMessage({
        id: -1,
        msg_id: `file-${id}`,
        conv_id: convId,
        sender_id: app.device?.device_id ?? "",
        receiver_id: convId,
        kind: "file",
        content: JSON.stringify({ name }),
        ts: Math.max(Date.now(), messages.value[convId]?.at(-1)?.ts ?? 0),
        status: "sending",
      });
      return id;
    } catch (e) {
      const app2 = useAppStore();
      app2.toast(`文件发送失败：${e}`, "error");
      return null;
    }
  }
  async function sendFileRelayTo(convId: string, path: string) {
    if (convId.startsWith("group:")) return null;
    return api.sendFileRelay(convId, path);
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
      }, 600);
    };
    bindEvents({
      onPeers: (p) => {
        peers.value = p;
        const onlineIds = new Set(p.map((x) => x.device_id));
        friends.value.forEach((f) => (f.online = onlineIds.has(f.device_id)));
        void refreshTopology();
      },
      onFriendRequest: (req) => {
        pendingRequests.value = [req, ...pendingRequests.value];
      },
      onFriendAccepted: async () => {
        await refreshFriends();
        await refreshConversations();
      },
      onFriendRejected: () => {},
      onFriendRemoved: async () => {
        await refreshFriends();
      },
      onMessage: (rec) => {
        enqueueMessage(rec);
        maybeNotify(rec);
        if (rec.kind === "file") void refreshTransfers();
        // 正在看这个会话且窗口可见 → 自动已读并回执
        if (rec.sender_id !== myDeviceId.value && rec.conv_id === activeConv.value) {
          debounceMarkRead(rec.conv_id);
        }
      },
      onMessageAcked: (msgId) => {
        // 对方收到（Ack）：sending → delivered（空圆框）
        for (const [convId, list] of Object.entries(messages.value)) {
          const i = list.findIndex((m) => m.msg_id === msgId);
          if (i >= 0) {
            messages.value[convId] = [
              ...list.slice(0, i),
              { ...list[i], status: "delivered" },
              ...list.slice(i + 1),
            ];
            break;
          }
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
      onFileProgress: (p) => {
        // 进度由事件载荷直接更新，不再全量刷新传输列表（避免大文件 IPC 风暴卡死界面）
        updateTransferProgress(p);
      },
      onFileDone: (d) => {
        onFileDone(d);
        void refreshTransfers();
      },
      onPeerStyle: (p) => {
        app.applyPeerStyle(p.device_id, p.style);
      },
    });
    // 注册系统通知点击回调：点击通知 → 唤起窗口 + 定位到发送者会话
    void onAction((n) => {
      const raw = n as { id?: unknown; extra?: Record<string, unknown> };
      const id = typeof raw.id === "number" ? raw.id : undefined;
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
    activeConv,
    topology,
    activeConversation,
    totalUnread,
    unreadJump,
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
    createGroup,
    sendFileTo,
    sendFileRelayTo,
    enqueueMessage,
  };
});
