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
  /** 当前实际链路类型：`lan` / `routed` / `relay`（公网中转）/ `bluetooth`（无链路时缺省） */
  link?: string | null;
}

export interface Friend {
  device_id: string;
  nickname: string;
  avatar: string | null;
  device_type: string;
  /**
   * 对端 Gosslan 的应用版本（如 `"4.22.34"`）。缺省/`null` = 还没连过，**或对方是不报版本的
   * 老版本** —— 两者都不猜（INV-P24）。只用于给人看，不参与兼容判断。
   */
  peer_app_version?: string | null;
  /**
   * 对端**线格式**版本比本机高 ⇒ 界面给一句可解释提示。
   * 判定只在后端一处（`protocol::peer_protocol_is_newer`）；前端不许自己比数字，
   * 否则 `PROTOCOL_VERSION` 就有了第二份真相源。未声明版本的老老实例一定是 false。
   */
  peer_version_newer?: boolean;
  online: boolean;
}

export interface PendingRequest {
  from: string;
  from_nickname: string;
  from_avatar: string | null;
  ts: number;
}

export type MsgKind =
  | "text"
  | "code"
  | "image"
  | "file"
  | "system"
  | "reaction"
  | "recall"
  | "recalled"
  | "pin"
  | "announcement"
  | "announcement_delete"
  | "todo"
  | "todo_update"
  | "poll"
  | "poll_vote"
  /** 合并转发的聊天记录（微信式卡片）。Bubble 类：进时间线、计未读、可搜索。 */
  | "merge";

/** 表情回应的事件载荷（kind = "reaction" 时 content 的 JSON 形态）。 */
export interface ReactionPayload {
  /** 被回应的消息 msg_id */
  target: string;
  /** 表情 token，如 "[赞]" */
  emoji: string;
  /** true = 添加，false = 取消 */
  add: boolean;
}

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

/**
 * 一条收藏（微信式）。
 *
 * **独立于消息存储**：`content` 是收藏当时的快照，图片/文件的 `content.path` 已指向
 * 收藏副本（不是原消息所在的下载目录）—— 所以删会话、清缓存之后这条收藏仍然打得开。
 */
export interface FavoriteEntry {
  id: string;
  msg_id: string;
  conv_id: string;
  sender_id: string;
  kind: MsgKind;
  content: string;
  /** 原消息时间（列表里显示"这条内容是什么时候的"） */
  ts: number;
  /** 收藏时间（列表排序键，新的在前） */
  favorited_at: number;
  /** 收藏副本的绝对路径（仅 image/file，纯文本为 null） */
  media_path: string | null;
  media_size: number;
  /** 副本是否还在磁盘上（后端在列表查询时填）；false ⇒ 渲染「已清理」占位 */
  available: boolean;
}

export interface Conversation {
  id: string;
  kind: "single" | "group";
  name: string;
  avatar: string | null;
  last_msg: string | null;
  last_ts: number | null;
  unread: number;
  /** 本机置顶（纯本地偏好，不广播不同步）。列表排序时优先于 last_ts。 */
  pinned: boolean;
}

/** 群文件列表项（「群文件」面板）。本机持有状态与群投递进度是两回事。 */
export interface GroupFileEntry {
  transfer_id: string;
  name: string;
  size: number;
  sender_id: string;
  created_at: number;
  /** local = 本机可打开；receiving = 传输中；remote = 未取到；failed = 取失败可重试 */
  local_state: "local" | "receiving" | "remote" | "failed";
  /** 仅 local_state === "local" 时给出的本机真实路径 */
  local_path: string | null;
  /** 该文件对全群的投递进度（已完成成员数 / 成员总数） */
  delivered: number;
  total: number;
}

/** 会话的「当前链路」快照：最近一条消息走的链路 + 中间节点数。 */
export interface LinkState {
  /** `lan` / `routed`（跨网段·VPN 直达）/ `relay`（公网中转电路）/ `bluetooth`。 */
  path: "lan" | "routed" | "relay" | "bluetooth";
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
  /** 内容指纹（= cid，明文 sha256，ADR-0019）。合并转发卡片按它按需拉取；旧版本消息可能没有。 */
  sha256?: string;
}

/** 统一内容传输状态（ADR-0019）：前端气泡据此显示发送中/等待/重试/完成。 */export interface ContentTransfer {
  cid: string;
  peerId: string;
  groupId: string | null;
  name: string;
  size: number;
  direction: "send" | "receive";
  status: "queued" | "active" | "verifying" | "complete" | "incomplete" | "rejected";
  received: number;
  attempts: number;
  nextAttemptAt: number;
  lastError: string | null;
  path: string | null;
  createdAt: number;
  updatedAt: number;
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

/** 链路停滞状态翻转（与 Rust `state::FileStalledInfo` 同形）。
 *  `idle_ms` 是距最近一次**真正写出**分片的毫秒数，不是投进队列的时间。
 *  `reason` = 为什么停（缺省 = 普通链路静默，界面用既有的「网络停滞」文案）。 */
export interface FileStalledInfo {
  transfer_id: string;
  stalled: boolean;
  idle_ms: number;
  reason?: string;
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

/**
 * 群发「受众预告」（事件 `content-audience`，见 Rust 侧 `protocol::kind_audience_hint`）。
 *
 * 消息**已经发出去了** —— 这条只解释"群里有谁会把这条看成一段原始文本"，不是失败通知。
 * 文案整句由后端拼（判据与文案必须在同一处），前端不再自己组句子。
 */
export interface ContentAudience {
  conv_id: string;
  msg_id: string;
  hint: string;
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
  /** 已配置的「跨网段 / VPN」端点数（不是发现通道，配置在设置页；只给"有没有"，不给地址） */
  routedEndpoints: number;
  /** 公网中转状态（只有可公开的事实，**不含口令、不含服务器地址**） */
  relay: { enabled: boolean; connected: boolean };
}

export interface ChannelStatus {
  channel: "lan" | "bluetooth";
  enabled: boolean;
  available: boolean;
  running: boolean;
  peers: number;
  /**
   * **持久化的用户偏好**：与 running（此刻是否在跑）分开。
   *
   * 应用刚启动时通道必然没在跑，不能据此判断"用户关掉了"——自动拉起蓝牙必须看这个字段，
   * 否则退出重进会把用户明确关闭的蓝牙又打开（真机 2026-09-14）。
   */
  preferred: boolean;
}

/** 手动配置的跨子网端点（Tailscale / VPN / 跨网段）。
 *  只填地址即可，`device_id` 由握手时自动学到（后端 `RoutedEndpoint` 序列化而来）。 */
export interface RoutedEndpoint {
  device_id?: string;
  address: string;
}

/** 用户配置的外部链接（左栏「链接」视图 → 点开在独立窗口加载；后端 `ExternalLink` 序列化而来）。 */
export interface ExternalLink {
  /** 稳定主键（改名字/改网址都按它匹配）。 */
  id: string;
  name: string;
  url: string;
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

/**
 * 公网中转（盲管道）配置，与后端 `commands::RelayConfigView` 一一对应（ADR-0020）。
 *
 * `server` 是**后端规范化后**的 `ip:port`（省略端口时补 59993）—— 界面回显这个值，
 * 用户因此能立刻看到"实际会连哪儿"，而不是他手打的原始串。
 */
export interface RelayConfig {
  enabled: boolean;
  server: string;
  token: string;
}

/**
 * 一次"保存前真拨"的结论（后端 `check_relay_server`）。
 *
 * `kind` 是三档 **机器可读**串，与 Rust 侧 `RelayProbeKind::as_str` 同一套
 * （`INTEGRATION.md` §1.1：服务器的两种命运差两个数量级，加上纯本端可判的 TCP 失败）：
 * · `unreachable` 连不上（地址 / 端口 / 安全组）
 * · `rejected`    首行被立刻拒绝（口令 / 版本 / 格式，精确原因只在服务器日志里）
 * · `held`        首行已被接受、口令正确，只是对方这一轮没接入
 * 刻意**没有**第四种："对方没开中转"与"对方不在线"对哑管道是同一个观测，分不出来。
 */
export interface RelayProbe {
  kind: "unreachable" | "rejected" | "held";
  /** 探到的那个 `ip:port`（全不可达时是第一个候选）。与已存值不同时界面应回填。 */
  server: string;
  /** 试过哪些端口，逗号分隔 —— 供用户抄给部署服务器的人看。 */
  tried: string;
  /** 技术细节。**不含口令**。 */
  detail: string;
}

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

// ---------------- 网络诊断（隐藏开发者面板用） ----------------

/** 一条候选链路（网卡或蓝牙）。与 Rust `state::InterfaceCandidate` 逐字对应。 */
export interface InterfaceCandidate {
  /** `lan`（真实网卡）| `bluetooth`（BLE 链路） */
  kind: "lan" | "bluetooth" | string;
  name: string;
  ip: string;
  has_broadcast: boolean;
  broadcast: string | null;
  is_rfc1918: boolean;
  is_virtual: boolean;
  score: number;
  selected: boolean;
  /** 一句话状态（蓝牙行用；网卡行为空） */
  detail: string;
}

/** 蓝牙候选的失败退避条目 */
export interface BleBackoff {
  id: string;
  failures: number;
  remaining_ms: number;
}

/**
 * 蓝牙通道事实。
 *
 * 为什么单独一块：诊断数据原来全是局域网的，纯蓝牙用户看到的是 `mode = offline`
 * （用户 2026-09-13 反馈）。蓝牙是独立通道，必须有自己的状态。
 */
export interface BleDiag {
  feature_compiled: boolean;
  enabled: boolean;
  available: boolean;
  running: boolean;
  peers: number;
  /** 当前扫描节奏来源：`active`（前台/聚焦）| `idle`（后台/失焦） */
  activity: string;
  scan_window_ms: number;
  scan_interval_ms: number;
  last_scan_ts: number;
  last_scan_total: number;
  last_scan_matched: number;
  backoff: BleBackoff[];
  no_dial: number;
}

/**
 * 网络诊断快照。
 *
 * 局域网字段（mode / bound_ip / …）**只描述局域网**；蓝牙在 `bluetooth` 里独立描述。
 * 「最近事件」已合并进运行日志（见 `AppState::push_diag_event`），不再出现在这里。
 */
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
  bluetooth: BleDiag;
  /** 本机线格式版本（跨版本排查的基准行） */
  protocol_version: number;
  /** 本机应用版本，只给人看 */
  app_version: string;
  /** 各对端在 Hello 里声明的版本 */
  peer_versions: PeerVersionDiag[];
}

/** 诊断面板的一行「对端声明了什么版本」（ADR-0007 决策 1）。 */
export interface PeerVersionDiag {
  device_id: string;
  nickname: string;
  /** null = 对端是没报版本的老版本，不当成 0 也不当成 1 */
  protocol_version: number | null;
  app_version: string | null;
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
