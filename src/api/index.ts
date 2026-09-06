// Tauri 后端调用封装（invoke 参数使用 camelCase，后端自动转换为 snake_case）。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppSettings,
  CacheInfo,
  ChannelStatus,
  CleanupReport,
  Conversation,
  DeviceInfo,
  FileDoneInfo,
  FileProgress,
  Friend,
  Group,
  SearchResult,
  InterfaceInfo,
  MessageRecord,
  NetworkStatus,
  Peer,
  PeerReadInfo,
  PendingRequest,
  ShareEntry,
  TopologyInfo,
  TransferInfo,
} from "@/types";

export const api = {
  getDeviceInfo: () => invoke<DeviceInfo>("get_device_info"),
  updateProfile: (nickname: string, avatar: string | null) =>
    invoke<DeviceInfo>("update_profile", { nickname, avatar }),
  listInterfaces: () => invoke<InterfaceInfo[]>("list_interfaces"),
  startNetwork: (bindIp: string) => invoke<void>("start_network", { bindIp }),
  stopNetwork: () => invoke<void>("stop_network"),
  getNetworkStatus: () => invoke<NetworkStatus>("get_network_status"),
  getTopology: () => invoke<TopologyInfo>("get_topology"),

  getPeers: () => invoke<Peer[]>("get_peers"),
  searchNearbyPeers: () => invoke<Peer[]>("search_nearby_peers"),
  focusWindow: () => invoke<void>("focus_window"),
  getFriends: () => invoke<Friend[]>("get_friends"),
  removeFriend: (peerId: string) => invoke<void>("remove_friend", { peerId }),
  getPendingRequests: () => invoke<PendingRequest[]>("get_pending_requests"),
  sendFriendRequest: (peerId: string) => invoke<void>("send_friend_request", { peerId }),
  respondFriendRequest: (peerId: string, accept: boolean) =>
    invoke<void>("respond_friend_request", { peerId, accept }),

  sendMessage: (friendId: string, content: string, kind: string) =>
    invoke<MessageRecord>("send_message", { friendId, content, kind }),
  getMessages: (convId: string, limit?: number, offset?: number) =>
    invoke<MessageRecord[]>("get_messages", { convId, limit, offset }),
  getConversations: () => invoke<Conversation[]>("get_conversations"),
  ensureConversation: (friendId: string) =>
    invoke<Conversation>("ensure_conversation", { friendId }),
  markRead: (convId: string) => invoke<void>("mark_read", { convId }),
  deleteConversation: (convId: string) =>
    invoke<void>("delete_conversation", { convId }),

  createGroup: (name: string, members: string[]) => invoke<Group>("create_group", { name, members }),
  distributeGroupKey: (groupId: string) => invoke<void>("distribute_group_key", { groupId }),
  getGroups: () => invoke<Group[]>("get_groups"),
  sendGroupMessage: (groupId: string, content: string, kind: string) =>
    invoke<MessageRecord>("send_group_message", { groupId, content, kind }),

  sendFile: (friendId: string, path: string) => invoke<string>("send_file", { friendId, path }),
  sendFileAuto: (friendId: string, path: string) =>
    invoke<string>("send_file_auto", { friendId, path }),
  sendFileRelay: (friendId: string, path: string) =>
    invoke<string>("send_file_relay", { friendId, path }),
  getTransfers: () => invoke<TransferInfo[]>("get_transfers"),

  setShareDir: (path: string) => invoke<void>("set_share_dir", { path }),
  getShareDir: () => invoke<string | null>("get_share_dir"),
  requestShareTree: (friendId: string) => invoke<ShareEntry[]>("request_share_tree", { friendId }),
  downloadSharedFile: (friendId: string, remotePath: string) =>
    invoke<string>("download_shared_file", { friendId, remotePath }),

  getChannelStatus: () => invoke<ChannelStatus[]>("get_channel_status"),
  setChannelEnabled: (channel: string, enabled: boolean) =>
    invoke<void>("set_channel_enabled", { channel, enabled }),
  getCacheInfo: () => invoke<CacheInfo>("get_cache_info"),
  setCachePolicy: (retentionDays: number | null, maxBytes: number | null) =>
    invoke<void>("set_cache_policy", { retentionDays, maxBytes }),
  cleanCacheNow: () => invoke<CleanupReport>("clean_cache_now"),

  getSettings: () => invoke<AppSettings>("get_settings"),
  saveSettings: (s: AppSettings) => invoke<void>("save_settings", { settings: s }),
  resetSettings: () => invoke<void>("reset_settings"),
  broadcastChatStyle: (style: string) => invoke<void>("broadcast_chat_style", { style }),
  searchMessages: (keyword: string) => invoke<SearchResult[]>("search_messages", { keyword }),
};

// ---------------- 事件监听 ----------------

export interface PeerStyleUpdate {
  device_id: string;
  style: string;
}

export type EventHandlers = {
  onPeers: (peers: Peer[]) => void;
  onFriendRequest: (req: PendingRequest) => void;
  onFriendAccepted: (id: string) => void;
  onFriendRejected: (id: string) => void;
  onFriendRemoved: (id: string) => void;
  onMessage: (rec: MessageRecord) => void;
  onMessageAcked: (msgId: string) => void;
  onPeerRead: (p: PeerReadInfo) => void;
  onFileProgress: (p: FileProgress) => void;
  onFileDone: (d: FileDoneInfo) => void;
  onPeerStyle: (p: PeerStyleUpdate) => void;
};

/** 注册所有后端事件监听，返回取消函数集合。 */
export async function bindEvents(h: EventHandlers): Promise<UnlistenFn[]> {
  const unlisteners = await Promise.all([
    listen<Peer[]>("peers-updated", (e) => h.onPeers(e.payload)),
    listen<PendingRequest>("friend-request", (e) => h.onFriendRequest(e.payload)),
    listen<string>("friend-accepted", (e) => h.onFriendAccepted(e.payload)),
    listen<string>("friend-rejected", (e) => h.onFriendRejected(e.payload)),
    listen<string>("friend-removed", (e) => h.onFriendRemoved(e.payload)),
    listen<MessageRecord>("message-received", (e) => h.onMessage(e.payload)),
    listen<string>("message-acked", (e) => h.onMessageAcked(e.payload)),
    listen<PeerReadInfo>("peer-read", (e) => h.onPeerRead(e.payload)),
    listen<FileProgress>("file-progress", (e) => h.onFileProgress(e.payload)),
    listen<FileDoneInfo>("file-done", (e) => h.onFileDone(e.payload)),
    listen<PeerStyleUpdate>("peer-style-updated", (e) => h.onPeerStyle(e.payload)),
  ]);
  return unlisteners;
}
