// 与 Rust 后端 serde 结构一一对应的前端类型

export interface DeviceInfo {
  device_id: string;
  nickname: string;
  avatar: string | null;
  tcp_port: number;
  online: boolean;
  x25519_pubkey: string;
  ed25519_pubkey: string;
}

export interface Peer {
  device_id: string;
  nickname: string;
  avatar: string | null;
  ip: string;
  tcp_port: number;
  last_seen: number;
  rtt_ms: number | null;
  x25519_pubkey: string | null;
  ed25519_pubkey: string | null;
  connected_since: number | null;
}

export interface Friend {
  device_id: string;
  nickname: string;
  avatar: string | null;
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

export interface NetworkStatus {
  online: boolean;
  bound_ip: string | null;
}

export interface TopologyInfo {
  node_count: number;
  relay_count: number;
  avg_rtt_ms: number | null;
  online: boolean;
}

/** 传输通道状态（局域网 / 蓝牙） */
export interface ChannelStatus {
  channel: "lan" | "bluetooth";
  enabled: boolean;
  available: boolean;
  running: boolean;
  peers: number;
}

/** 缓存目录占用与策略 */
export interface CacheInfo {
  file_count: number;
  total_bytes: number;
  retention_days: number | null;
  max_bytes: number | null;
}

/** 缓存清理结果 */
export interface CleanupReport {
  removed: number;
  freed_bytes: number;
}

/** 图片消息（粘贴板发图）的内容载荷 */
export interface ImagePayload {
  dataUrl: string;
}
