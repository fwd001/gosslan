import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { api, bindEvents } from "@/api";
import { applyIncomingToConversations, previewText } from "@/utils/messages";
import { useAppStore } from "@/stores/useAppStore";
import {
  isPermissionGranted,
  onAction,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import MessageWorker from "@/workers/message.worker?worker";
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

  // ---------------- Web Worker 合并 ----------------
  let worker: Worker | null = null;
  function getWorker(): Worker {
    if (!worker) worker = new MessageWorker();
    return worker;
  }
  function mergeInWorker(
    existing: MessageRecord[],
    incoming: MessageRecord[],
  ): Promise<MessageRecord[]> {
    return new Promise((resolve) => {
      const w = getWorker();
      const handler = (e: MessageEvent) => {
        if (e.data?.action === "merged") {
          w.removeEventListener("message", handler);
          resolve(e.data.payload as MessageRecord[]);
        }
      };
      w.addEventListener("message", handler);
      w.postMessage({ action: "merge", payload: { existing, incoming } });
    });
  }

  // ---------------- 密集广播批量队列 ----------------
  let pending: MessageRecord[] = [];
  let flushScheduled = false;
  function scheduleFlush() {
    if (flushScheduled) return;
    flushScheduled = true;
    requestAnimationFrame(() => {
      flushScheduled = false;
      const batch = pending;
      pending = [];
      void applyIncoming(batch);
    });
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
      messages.value[convId] = await mergeInWorker(existing, incoming);
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

  // 会话内消息分页：每会话最多缓存页数（防内存无限增长）
  const PAGE_SIZE = 300;
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
    messages.value[convId] = await mergeInWorker(existing, older);
    pagesLoaded.set(convId, pages + 1);
    // prepend 历史后，「第一条未读」的索引整体后移
    if (unreadJump.value?.convId === convId) {
      unreadJump.value = { ...unreadJump.value, index: unreadJump.value.index + older.length };
    }
  }

  /** 统一发送（单聊/群聊）。 */
  async function send(convId: string, content: string, kind: string): Promise<MessageRecord> {
    let rec: MessageRecord;
    if (convId.startsWith("group:")) {
      rec = await api.sendGroupMessage(convId.slice(6), content, kind);
    } else {
      rec = await api.sendMessage(convId, content, kind);
    }
    enqueueMessage(rec);
    return rec;
  }

  async function sendFriendRequest(peerId: string) {
    await api.sendFriendRequest(peerId);
  }
  async function respondRequest(peerId: string, accept: boolean) {
    await api.respondFriendRequest(peerId, accept);
    pendingRequests.value = pendingRequests.value.filter((r) => r.from !== peerId);
    if (accept) await refreshFriends();
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
      },
      onMessageAcked: () => {},
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
    createGroup,
    sendFileTo,
    sendFileRelayTo,
    enqueueMessage,
  };
});
