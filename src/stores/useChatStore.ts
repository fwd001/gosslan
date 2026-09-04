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
    for (const [convId, incoming] of byConv) {
      const existing = messages.value[convId] ?? [];
      messages.value[convId] = await mergeInWorker(existing, incoming);
    }
    conversations.value = applyIncomingToConversations(
      conversations.value,
      activeConv.value,
      byConv,
    );
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

  async function openConversation(id: string) {
    activeConv.value = id;
    await loadMessages(id);
    await api.markRead(id);
    const conv = conversations.value.find((c) => c.id === id);
    if (conv) conv.unread = 0;
  }

  async function loadMessages(convId: string) {
    messages.value[convId] = await api.getMessages(convId, 300, 0);
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

  async function sendFileTo(convId: string, path: string) {
    if (convId.startsWith("group:")) return null;
    return api.sendFile(convId, path);
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
      onMessage: (rec) => {
        enqueueMessage(rec);
        maybeNotify(rec);
        if (rec.kind === "file") void refreshTransfers();
      },
      onMessageAcked: () => {},
      onFileProgress: (p) => {
        updateTransferProgress(p);
        void refreshTransfers();
      },
      onFileDone: (d) => {
        onFileDone(d);
        void refreshTransfers();
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
    send,
    sendFriendRequest,
    respondRequest,
    createGroup,
    sendFileTo,
    sendFileRelayTo,
    enqueueMessage,
  };
});
