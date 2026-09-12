// 与 Rust 后端 serde 结构一一对应的前端类型

export interface DeviceInfo {
  device_id: string;
  nickname: string;
  avatar: string | null;
  device_type: string;
  tcp_port: number;
  online: boolean;
  x25519_pubkey: string;
  ed25519_pubkey: string;
}

export interface Peer {
  device_id: string;
  nickname: string;
  avatar: string | null;
  device_type: string;
  ip: string;
  tcp_port: number;
  last_seen: number;
  rtt_ms: number | null;
  x25519_pubkey: string | null;
  ed25519_pubkey: string | null;
  /** 首次发现该节点的时间戳 */
  first_seen: number | null;
  /** 当前实际链路类型：`lan` / `routed` / `bluetooth`（无链路时缺省） */
  link?: string | null;
}

export interface Friend {
  device_id: string;
  nickname: string;
  avatar: string | null;
  device_type: string;
  online: boolean;
}

export interface PendingRequest {
  from: string;
  from_nickname: string;
  from_avatar: string | null;
  ts: number;
}

export type MsgKind = "text" | "code" | "image" | "file" | "system";

export interface MessageRecord {
  id: number;
  msg_id: string;
  conv_id: string;
  sender_id: string;
  receiver_id: string;
  kind: MsgKind;
  content: string;
  ts: number;
  /** 每会话逻辑序号（Lamport），前端与后端都以它排序，而非墙上时钟。 */
  seq: number;
  status: string;
}

export interface Conversation {
  id: string;
  kind: "single" | "group";
  name: string;
  avatar: string | null;
  last_msg: string | null;
  last_ts: number | null;
  unread: number;
}

/** 会话的「当前链路」快照：最近一条消息走的链路 + 中间节点数。 */
export interface LinkState {
  path: "lan" | "routed" | "bluetooth";
  hop: number;
}

export interface Group {
  id: string;
  name: string;
  creator: string;
  members: string[];
}

export interface InterfaceInfo {
  name: string;
  ip: string;
}

export interface ShareEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
}

/** 文件消息 content 的 JSON 载荷（附件名 / 本地路径 / 大小 / 子类型）。 */
export interface FileMeta {
  name: string;
  path: string;
  size: number;
  /** 后端按扩展名分类的附件子类型；历史消息缺省按 file 处理。 */
  subtype: string;
}

export interface TransferInfo {
  id: string;
  peer_id: string;
  name: string;
  size: number;
  direction: "send" | "receive";
  status: string;
  path: string | null;
  progress: number;
}

export interface FileProgress {
  transfer_id: string;
  received: number;
  total: number;
}

export interface FileDoneInfo {
  transfer_id: string;
  name: string;
  size: number;
  path: string;
}

export interface FileFailedInfo {
  transfer_id: string;
  reason: string;
}

/** 对方已读回执（peer-read 事件载荷） */
export interface PeerReadInfo {
  peer_id: string;
  last_read_ts: number;
}

/** 群成员级已读回执。reader_id 读到 last_read_ts 为止。 */
export interface GroupReadInfo {
  group_id: string;
  reader_id: string;
  last_read_ts: number;
}

export interface TopologyInfo {
  node_count: number;
  relay_count: number;
  avg_rtt_ms: number | null;
  online: boolean;
}

/** 传输通道状态（局域网 / 蓝牙） */
/**
 * 运行状态的**唯一快照**（与 Rust 的 `RuntimeSnapshot` 逐字对应，用户要求的第 ② 项）。
 *
 * 以前"局域网开着没有"有**两份**前端状态（`channels[lan].enabled` 与 `online`），
 * 各自被不同命令+事件维护 ⇒ 必然出现"外面开了、里面还是关的"。
 * 现在前端只认这一份：`api.getRuntimeSnapshot()` / `runtime-changed` 事件载荷。
 */
export interface RuntimeSnapshot {
  channels: ChannelStatus[];
  online: boolean;
  boundIp: string | null;
  /** 蓝牙里"通道状态装不下"的事实（例如本次构建是否编译了蓝牙特性） */
  ble: { featureCompiled: boolean };
  /** 在线节点数（完整列表仍走 `peers-updated`，避免每次开关都搬全表） */
  peerCount: number;
  /** **我自己的在线状态**：任一通道在跑 = 在线；两个都关才是离线（用户 2026-09-12 定的规则） */
  present: boolean;
}

export interface ChannelStatus {
  channel: "lan" | "bluetooth";
  enabled: boolean;
  available: boolean;
  running: boolean;
  peers: number;
}

/** 手动配置的跨子网端点（Tailscale / VPN / 跨网段）。
 *  只填地址即可，`device_id` 由握手时自动学到（后端 `RoutedEndpoint` 序列化而来）。 */
export interface RoutedEndpoint {
  device_id?: string;
  address: string;
}

/** 应用偏好设置（外观 / 网卡选择 / 聊天样式，持久化到本地 SQLite）。
 *  E2EE 自 v0.11.0 起恒开且不可关闭，不再作为设置项。 */
export interface AppSettings {
  themeColor: string | null;
  fontFamily: string | null;
  darkMode: boolean | null;
  /** 外观模式："system"（跟随系统，默认）| "light" | "dark"。
   *  `darkMode` 是**解析后的结果**（跟随系统时由前端按系统偏好解析后回写），
   *  本字段才是**用户意图**。旧记录没有该字段 → 视为 "system"。 */
  appearanceMode: string | null;
  /** 桌面通知开关（null 视为开启）。 */
  notifyEnabled: boolean | null;
  /** 通知是否显示消息正文（null 视为显示；关掉后只提示"收到新消息"，保护锁屏隐私）。 */
  notifyShowContent: boolean | null;
  /** 语言偏好："system"（跟随系统，默认）| "zh-CN" | "en-US"（null 视为跟随系统）。 */
  language: string | null;
  bindIp: string | null;
  /** 聊天显示样式 JSON：{"preset":"theme","fontSize":"md"} */
  chatStyle: string | null;
  /** 对端样式表 JSON（device_id -> style JSON，后端收 ChatStyle 消息时写入） */
  peerStyles: string | null;
  /** 中继授权策略："off" | "friends" | "allowlist" | "all"（null 视为 "all"）。 */
  relayPolicy: string | null;
  /** 中继白名单 JSON 字符串数组（`allowlist` 策略用）。 */
  relayAllowlist: string | null;
}

/** 中继授权策略（与后端 `mesh::relay_policy::RelayPolicy` 一一对应）。 */
export type RelayPolicy = "off" | "friends" | "allowlist" | "all";

/** 缓存目录占用与策略 */
export interface CacheInfo {
  /** 已接收的图片 / 文件（落在「文件存储目录」）：文件数与合计占用 */
  media_count: number;
  media_bytes: number;
  /** 聊天记录数据库占用（含 -wal/-shm） */
  db_bytes: number;
  retention_days: number | null;
  max_bytes: number | null;
}

/** 缓存清理结果 */
export interface CleanupReport {
  removed: number;
  freed_bytes: number;
}

/** 聊天记录导出结果 */
export interface ExportSummary {
  /** 实际写入的会话数（只统计有消息的会话） */
  conversations: number;
  /** 实际写入的消息条数 */
  messages: number;
  /** 落盘路径 */
  path: string;
}

/** 搜索结果 */
export interface SearchResult {
  conv_id: string;
  name: string;
  match_content: string;
  match_ts: number;
  /** 命中消息的 msg_id —— 用于「跳到那一条」，见 useChatStore.locateMessageInConv。 */
  match_msg_id: string;
}

/** 「搜索聊天记录」结果页的一条命中消息。 */
export interface ChatSearchMessage {
  msg_id: string;
  sender_id: string;
  sender_name: string;
  kind: string;
  content: string;
  ts: number;
}

/** 按会话分组的检索结果（左栏一个会话一行，右栏是它的命中消息）。 */
export interface ChatSearchGroup {
  conv_id: string;
  name: string;
  kind: string;
  avatar: string | null;
  /** 当前筛选条件下该会话的命中总数（微信式「共 N 条相关聊天记录」）。 */
  total: number;
  latest_ts: number;
  messages: ChatSearchMessage[];
}

// ---------------- Discovery 诊断（隐藏开发者面板用） ----------------

export interface InterfaceCandidate {
  name: string;
  ip: string;
  has_broadcast: boolean;
  broadcast: string | null;
  is_rfc1918: boolean;
  is_virtual: boolean;
  score: number;
  selected: boolean;
}

export interface DiscoveryEvent {
  ts: number;
  kind: string;
  detail: string;
}

export interface DiscoveryDiag {
  mode: string;
  bound_ip: string;
  selected_interface: string;
  selected_ip: string;
  tcp_listen: string;
  udp_port: number;
  broadcast_target: string;
  multicast_group: string;
  multicast_join_result: string;
  multicast_if_result: string;
  last_broadcast_send: number;
  last_multicast_send: number;
  last_announce_recv: number;
  candidates: InterfaceCandidate[];
  recent_events: DiscoveryEvent[];
}

// ---------------- 运行日志（「运行日志」页用） ----------------

export type LogLevel = "info" | "warn" | "error";

export interface LogEntry {
  /** Unix 毫秒时间戳 */
  ts: number;
  level: LogLevel;
  /** 子系统名（transport / lan / routed / mesh / friend …） */
  target: string;
  message: string;
}
