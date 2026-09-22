/**
 * **一条链路的"连接信息"该怎么展示**（单一事实来源）。
 *
 * ## 为什么需要它（用户 2026-09-13 提出）
 *
 * 界面上原来是"有 IP 就显示 IP、没有就显示 —"，于是**蓝牙链路**也会显示一行
 * `IP 地址：—`，而 `设备类型` 直接显示后端的 `desktop` / `mobile` 英文原值。用户的要求是：
 * **不同链路进来的设备，标注的信息应当不一样** —— 蓝牙根本没有 IP，不该拿一个空行敷衍。
 *
 * ## 约定（各链路各说各的事实）
 *
 * | 链路 | 连接方式 | 地址行 | 例子 |
 * |---|---|---|---|
 * | 蓝牙直连 | 蓝牙直连（近距离） | **不显示** —— 蓝牙链路上没有 IP 这个概念 | — |
 * | 同一局域网 | 同一局域网 | `ip:port` | `192.168.31.32:59992` |
 * | 跨网段/VPN | 跨网段 / VPN（对端真实 IP 直达） | `ip:port` | `100.101.221.60:59992` |
 * | 公网中转 | 公网中转（经自备服务器的密封电路） | **不显示** —— 只有服务器地址，不是对端地址 | — |
 * | 经 N 跳转发 | 经 {n} 跳中继（mesh 多跳，无直连链路） | 不显示 | — |
 * | 已发现未建链 | 已发现（未建链） | 不显示 | — |
 *
 * 判据全部是纯函数 ⇒ 可单测、可护栏（这类"显示错了"的退化不会报错，只会误导用户）。
 */

/**
 * 与后端 `PathKind::as_str()` 对齐（`lan` / `routed` / `relay` / `bluetooth`）。
 *
 * ⚠️ `relay` 是**公网中转服务器的直连密封电路**（后端 `PathKind::Relay`），
 * 与 `hop > 0` 的「经 N 跳 mesh 转发」是两回事 —— 后者没有直连链路、只有跳数。
 */
export type PeerLink = "bluetooth" | "lan" | "routed" | "relay" | string | null | undefined;

export interface PeerInfoInput {
  /** 后端填的**真实链路类型**（`Peer.link`）；null/缺失 = 只有发现、没有链路 */
  link?: PeerLink;
  ip?: string | null;
  tcp_port?: number | null;
  /** 中继跳数（0 = 直连；>0 = 经中继）。缺失按 0 处理。 */
  hop?: number | null;
  online?: boolean;
  /** 后端 `device_type`：`desktop` / `mobile` / 空串（旧端或未知） */
  device_type?: string | null;
}

/** 连接方式那一行的 i18n key（用 `t()` 渲染）。 */
export function linkLabelKey(info: PeerInfoInput): string {
  const hop = info.hop ?? 0;
  if (info.link === "bluetooth") return "peer.link.bluetooth";
  // hop>0 = 经多个中间节点 mesh 转发（无直连链路），与"公网中转服务器直连电路"是两回事，先判它。
  if (hop > 0) return "peer.link.relay";
  // 公网中转服务器的密封电路（后端 PathKind::Relay）：与"跨网段/VPN 对端 IP 直达"区分开。
  if (info.link === "relay") return "peer.link.relayServer";
  if (info.link === "routed") return "peer.link.routed";
  if (info.link === "lan") return "peer.link.lan";
  if (info.online) return "peer.link.lan";
  // 有节点、没链路：**必须说清"还没连上"**，否则用户会以为已经可用（真机踩过）
  return "peer.link.discovered";
}

/** 连接方式的展示参数（中继跳数要填进模板）。 */
export function linkLabelParams(info: PeerInfoInput): Record<string, string | number> {
  return { n: info.hop ?? 0 };
}

/**
 * 连接图标的标识（**与 `linkLabelKey` 同源判据、同一顺序**，只是输出图标名而不是文案 key）。
 *
 * 为什么要它：图标选择此前只内联在 `ChatHeader.linkIcon` 一处，好友列表想显示同款图标就得
 * 再抄一遍 —— 用户 2026-09-22 要求"四种通道的图标各界面全局统一"，所以把判据收在这里，
 * 各组件只负责把图标名映射到 lucide 组件（见 `components/conversation/LinkIcon.vue`）。
 *
 * 五种真实链路 + 一种"无链路"：
 * `relay`=经 N 跳 mesh 转发（Share2）、`relayServer`=公网中转直连电路（Globe）、
 * `routed`=跨网段/VPN（Network）、`bluetooth`=蓝牙（Bluetooth）、`lan`=局域网（Router）、
 * `discovered`=发现了但没建链（**调用方不应为它画图标**，画了就是骗）。
 */
export type LinkIconName =
  | "lan"
  | "routed"
  | "relayServer"
  | "relay"
  | "bluetooth"
  | "discovered";

export function linkIconName(info: PeerInfoInput): LinkIconName {
  const hop = info.hop ?? 0;
  if (info.link === "bluetooth") return "bluetooth";
  if (hop > 0) return "relay";
  if (info.link === "relay") return "relayServer";
  if (info.link === "routed") return "routed";
  if (info.link === "lan") return "lan";
  if (info.online) return "lan";
  return "discovered";
}

/**
 * 该不该显示"地址"这一行。
 *
 * **蓝牙链路一律不显示**：蓝牙上没有 IP，显示 `IP 地址：—` 只会让人以为"信息缺失"。
 * **公网中转链路也不显示**：那条电路的 endpoint 是**中转服务器地址**，不是对端地址，
 * 显示出来等于把服务器 IP 冒充成对方 IP（写了就是骗）。
 * mesh 多跳（hop>0）同样不显示直连地址（我们只有跳数，没有中继节点地址）。
 */
export function shouldShowAddress(info: PeerInfoInput): boolean {
  if (info.link === "bluetooth") return false;
  if (info.link === "relay") return false;
  if ((info.hop ?? 0) > 0) return false;
  return !!info.ip;
}

/** 地址文本（`ip:port`），没有地址返回 null。 */
export function addressText(info: PeerInfoInput): string | null {
  if (!shouldShowAddress(info)) return null;
  const ip = info.ip as string;
  return info.tcp_port ? `${ip}:${info.tcp_port}` : ip;
}

/** 设备类型的 i18n key：后端只给 `desktop` / `mobile`，其余一律"未知设备"。 */
export function deviceTypeKey(deviceType: string | null | undefined): string {
  const v = (deviceType ?? "").trim().toLowerCase();
  if (v === "desktop") return "peer.device.desktop";
  if (v === "mobile") return "peer.device.mobile";
  return "peer.device.unknown";
}
