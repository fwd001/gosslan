// Tauri 后端调用封装（invoke 参数使用 camelCase，后端自动转换为 snake_case）。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppSettings, CacheInfo, ChannelStatus, ChatSearchGroup, CleanupReport, Conversation, DeviceInfo, DiscoveryDiag, ExportSummary, FileDoneInfo, FileFailedInfo, FileProgress, Friend, Group, GroupReadInfo, InterfaceCandidate, InterfaceInfo, LinkState, LogEntry, MessageRecord, NetworkStatus, Peer, PeerReadInfo, PendingRequest, RoutedEndpoint, SearchResult, ShareEntry, TopologyInfo, TransferInfo } from "@/types";

export const api = {
  /**
   * 监听"**另一个窗口**改了设置"（外观 / 语言 / 资料 / 目录 / 缓存策略）。
   *
   * 独立「设置」窗口与主窗口是两个 WebView、各有自己的 store —— 没有这个事件时，
   * 在设置窗口改语言/主题后主窗口不会变（用户实测反馈）。两个窗口都监听，返回取消函数。
   */
  onSettingsChanged: (cb: () => void) => listen("settings-changed", () => cb()),
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
  getConvLink: (convId: string) => invoke<LinkState | null>("get_conv_link", { convId }),
  getMessageCount: (convId: string) => invoke<number>("get_message_count", { convId }),
  getConversations: () => invoke<Conversation[]>("get_conversations"),
  ensureConversation: (friendId: string) =>
    invoke<Conversation>("ensure_conversation", { friendId }),
  markRead: (convId: string) => invoke<void>("mark_read", { convId }),
  deleteConversation: (convId: string) =>
    invoke<void>("delete_conversation", { convId }),

  createGroup: (name: string, members: string[]) => invoke<Group>("create_group", { name, members }),
  distributeGroupKey: (groupId: string) => invoke<void>("distribute_group_key", { groupId }),
  renameGroup: (groupId: string, name: string) =>
    invoke<void>("rename_group", { groupId, name }),
  groupAddMember: (groupId: string, deviceId: string) =>
    invoke<void>("group_add_member", { groupId, deviceId }),
  groupRemoveMember: (groupId: string, deviceId: string) =>
    invoke<void>("group_remove_member", { groupId, deviceId }),
  transferGroupCreator: (groupId: string, newCreator: string) =>
    invoke<void>("transfer_group_creator", { groupId, newCreator }),
  leaveGroup: (groupId: string) => invoke<void>("leave_group", { groupId }),
  getGroups: () => invoke<Group[]>("get_groups"),
  getGroupReads: (groupId: string) => invoke<GroupReadInfo[]>("get_group_reads", { groupId }),
  sendGroupMessage: (groupId: string, content: string, kind: string) =>
    invoke<MessageRecord>("send_group_message", { groupId, content, kind }),

  // 自绘标题栏：窗口控制
  windowMinimize: () => invoke<void>("window_minimize"),
  windowToggleMaximize: () => invoke<boolean>("window_toggle_maximize"),
  windowIsMaximized: () => invoke<boolean>("window_is_maximized"),
  windowToggleFullscreen: () => invoke<boolean>("window_toggle_fullscreen"),
  windowClose: () => invoke<void>("window_close"),

  sendFile: (friendId: string, path: string) => invoke<string>("send_file", { friendId, path }),
  sendFileAuto: (friendId: string, path: string) =>
    invoke<string>("send_file_auto", { friendId, path }),
  sendFileRelay: (friendId: string, path: string) =>
    invoke<string>("send_file_relay", { friendId, path }),
  sendGroupFile: (groupId: string, path: string) =>
    invoke<string>("send_group_file", { groupId, path }),
  saveOutgoingImage: (dataUrl: string) =>
    invoke<{ path: string; name: string; size: number }>("save_outgoing_image", { dataUrl }),
  deleteFile: (path: string) => invoke<void>("delete_file", { path }),
  /** 用系统默认应用打开本地文件：macOS 走 NSWorkspace（沙盒下 /usr/bin/open 被拦），
   *  Windows/Linux 走 opener。 */
  openFileNative: (path: string) => invoke<void>("open_file_native", { path }),
  /** macOS 窗口圆角：WebView 加载完成后调用（setup 阶段设会被 wry 替换 contentView 丢失）。 */
  applyMacosWindowShape: (dark: boolean) =>
    invoke<void>("apply_macos_window_shape", { dark }),
  getTransfers: () => invoke<TransferInfo[]>("get_transfers"),

  /** 读取附件预览原始字节（图片→Blob/objectURL，代码→TextDecoder）。超限后端 reject "TOO_LARGE"。
   *  注意：后端 raw bytes 在 macOS(WKWebView) 上经 JSON 序列化回传为 number[]，
   *  其余平台为 ArrayBuffer——调用方需按平台形状归一化成字节再使用。 */
  readFilePreview: (msgId: string, maxBytes: number) =>
    invoke<ArrayBuffer | number[]>("read_file_preview", { msgId, maxBytes }),

  /** 媒体是否仍在本机（未被「存储清理」删除）。仅"确定已删除"时返回 false，
   *  查不到消息（在途的乐观消息）返回 true——不能把在途消息误标成已清理。 */
  mediaPresent: (msgId: string) => invoke<boolean>("media_present", { msgId }),

  setShareDir: (path: string) => invoke<void>("set_share_dir", { path }),
  getShareDir: () => invoke<string | null>("get_share_dir"),
  getDownloadsDir: () => invoke<string>("get_downloads_dir"),
  setDownloadsDir: (path: string) => invoke<void>("set_downloads_dir", { path }),
  openDownloadsDir: () => invoke<void>("open_downloads_dir"),
  requestShareTree: (friendId: string) => invoke<ShareEntry[]>("request_share_tree", { friendId }),
  downloadSharedFile: (friendId: string, remotePath: string) =>
    invoke<string>("download_shared_file", { friendId, remotePath }),

  getChannelStatus: () => invoke<ChannelStatus[]>("get_channel_status"),
  setChannelEnabled: (channel: string, enabled: boolean) =>
    invoke<void>("set_channel_enabled", { channel, enabled }),
  /** 跨子网（Routed）端点：列表 / 添加 / 移除。添加只填地址即可（device_id 由握手学）。 */
  listRoutedEndpoints: () => invoke<RoutedEndpoint[]>("list_routed_endpoints"),
  addRoutedEndpoint: (address: string) =>
    invoke<RoutedEndpoint[]>("add_routed_endpoint", { deviceId: null, address }),
  removeRoutedEndpoint: (address: string) =>
    invoke<RoutedEndpoint[]>("remove_routed_endpoint", { address }),
  getCacheInfo: () => invoke<CacheInfo>("get_cache_info"),
  setCachePolicy: (retentionDays: number | null, maxBytes: number | null) =>
    invoke<void>("set_cache_policy", { retentionDays, maxBytes }),
  cleanCacheNow: () => invoke<CleanupReport>("clean_cache_now"),

  /** 导出全部聊天文字到指定文件。`utcOffsetMinutes` = -new Date().getTimezoneOffset()：
   *  Rust 侧不引入时区库，本地时间换算需要前端给出偏移。 */
  exportChatText: (destination: string, utcOffsetMinutes: number) =>
    invoke<ExportSummary>("export_chat_text", { destination, utcOffsetMinutes }),

  getSettings: () => invoke<AppSettings>("get_settings"),
  /**
   * 把**解析后**的界面语言推给后端重建 macOS 原生菜单栏（`src-tauri/src/menu.rs`）。
   * 非 macOS 平台是空实现（后端命令存在，直接 Ok），前端不必按平台分支。
   */
  setUiLanguage: (lang: string) => invoke<void>("set_ui_language", { lang }),
  saveSettings: (s: AppSettings) => invoke<void>("save_settings", { settings: s }),
  resetSettings: () => invoke<void>("reset_settings"),
  broadcastChatStyle: (style: string) => invoke<void>("broadcast_chat_style", { style }),
  searchMessages: (keyword: string) => invoke<SearchResult[]>("search_messages", { keyword }),
  clearAllData: () => invoke<void>("clear_all_data"),

  // 开发者诊断（隐藏面板用）
  getDiscoveryDiag: () => invoke<DiscoveryDiag>("get_discovery_diag"),
  getInterfaceCandidates: () => invoke<InterfaceCandidate[]>("get_interface_candidates"),

  // 运行日志（「运行日志」页 / 独立窗口用）
  /**
   * 搜索聊天记录（跨会话、按会话分组，支持「发送人 / 日期」筛选）。
   * 结果页用；会话列表里那点是 `searchMessages`（每会话只回一条摘要）。
   */
  searchChatHistory: (p: {
    keyword: string;
    senderId?: string | null;
    sinceMs?: number | null;
    untilMs?: number | null;
  }) =>
    invoke<ChatSearchGroup[]>("search_chat_history", {
      keyword: p.keyword,
      senderId: p.senderId ?? null,
      sinceMs: p.sinceMs ?? null,
      untilMs: p.untilMs ?? null,
    }),
  getLogs: () => invoke<LogEntry[]>("get_logs"),
  clearLogs: () => invoke<void>("clear_logs"),
  /** 桌面端：打开独立日志窗口；移动端不要调用（用页面跳转）。 */
  openLogWindow: () => invoke<void>("open_log_window"),
  closeLogWindow: () => invoke<void>("close_log_window"),
  openSettingsWindow: () => invoke<void>("open_settings_window"),
  closeSettingsWindow: () => invoke<void>("close_settings_window"),
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
  onFriendMessageBlocked: (id: string) => void;
  onMessage: (rec: MessageRecord) => void;
  onMessageAcked: (msgId: string) => void;
  onPeerRead: (p: PeerReadInfo) => void;
  onGroupRead: (p: GroupReadInfo) => void;
  onFileProgress: (p: FileProgress) => void;
  onFileDone: (d: FileDoneInfo) => void;
  onFileFailed: (d: FileFailedInfo) => void;
  onPeerStyle: (p: PeerStyleUpdate) => void;
  /** 群信息变更（群密钥建群 / 群改名 / 成员变更） */
  onGroupsUpdated: (groupId: string) => void;
  /** 自己被移出群（group_id） */
  onGroupMemberRemoved: (groupId: string) => void;
};

/** 注册所有后端事件监听，返回取消函数集合。 */
export async function bindEvents(h: EventHandlers): Promise<UnlistenFn[]> {
  const unlisteners = await Promise.all([
    listen<Peer[]>("peers-updated", (e) => h.onPeers(e.payload)),
    listen<PendingRequest>("friend-request", (e) => h.onFriendRequest(e.payload)),
    listen<string>("friend-accepted", (e) => h.onFriendAccepted(e.payload)),
    listen<string>("friend-rejected", (e) => h.onFriendRejected(e.payload)),
    listen<string>("friend-removed", (e) => h.onFriendRemoved(e.payload)),
    listen<string>("friend-message-blocked", (e) => h.onFriendMessageBlocked(e.payload)),
    listen<MessageRecord>("message-received", (e) => h.onMessage(e.payload)),
    listen<string>("message-acked", (e) => h.onMessageAcked(e.payload)),
    // 群消息的送达确认走**独立事件**（载荷 `{group_id, msg_id}`）；此前前端没接，
    // 群消息气泡的"已送达"只能等其它刷新才更新。
    listen<{ msg_id: string }>("group-message-acked", (e) => h.onMessageAcked(e.payload.msg_id)),
    listen<PeerReadInfo>("peer-read", (e) => h.onPeerRead(e.payload)),
    listen<GroupReadInfo>("group-read", (e) => h.onGroupRead(e.payload)),
    listen<FileProgress>("file-progress", (e) => h.onFileProgress(e.payload)),
    listen<FileDoneInfo>("file-done", (e) => h.onFileDone(e.payload)),
    listen<FileFailedInfo>("file-failed", (e) => h.onFileFailed(e.payload)),
    listen<PeerStyleUpdate>("peer-style-updated", (e) => h.onPeerStyle(e.payload)),
    listen<string>("groups-updated", (e) => h.onGroupsUpdated(e.payload)),
    listen<string>("group-member-removed", (e) => h.onGroupMemberRemoved(e.payload)),
  ]);
  return unlisteners;
}

// ---------------- 应用级动作（原生菜单 ↔ 键盘快捷键） ----------------
// 动作名与 emitAction 抽到 utils/appActions.ts：useShortcuts / shortcuts 这类纯逻辑也要用，
// 而纯逻辑会被 node:test 直接 import（Node 无法解析 @/ 别名）。这里继续 re-export，
// 保证既有的 `import { APP_ACTION } from "@/api"` 调用处无需改动。
import { APP_ACTION, emitAction } from "../utils/appActions";
export { APP_ACTION, emitAction };

/**
 * 监听 macOS 原生菜单栏的自定义项 → 转成应用级动作。
 * 菜单只在 macOS 建立（见 src-tauri/src/menu.rs），非 macOS 平台 listen 静默无事件。
 */
export async function bindMenuEvents(): Promise<UnlistenFn[]> {
  const to = (action: string) => () => emitAction(action);
  return Promise.all([
    listen("menu://settings", to(APP_ACTION.openSettings)),
    listen("menu://add-friend", to(APP_ACTION.addFriend)),
    listen("menu://search", to(APP_ACTION.focusSearch)),
    listen("menu://logs", to(APP_ACTION.openLogs)),
  ]);
}
