import type { Conversation, MessageRecord } from "@/types";
import { MENTION_AFTER, MENTION_BEFORE, escapeRe } from "./linkify.ts";
import { isKnownKind, isSilentKind, UNSUPPORTED_KIND_LABEL } from "./messageKinds.ts";
import { mergeSummary } from "./mergeCard.ts";

/**
 * 合并去重并排序消息列表 —— Gossip 密集广播防重复的核心纯函数。
 * 去重键：msg_id（后端以 SHA-256 生成的 message_id 保证全网唯一）。
 * 排序：时间戳升序，同时间戳按 id 升序，保证稳定。
 */
export function mergeMessages(
  existing: MessageRecord[],
  incoming: MessageRecord[],
): MessageRecord[] {
  const seen = new Set<string>(existing.map((m) => m.msg_id));
  const merged: MessageRecord[] = [...existing];
  for (const m of incoming) {
    if (!seen.has(m.msg_id)) {
      seen.add(m.msg_id);
      merged.push(m);
    }
  }
  // 排序以每会话逻辑序号为主，id 兜底，完全不依赖墙上时钟。
  merged.sort((a, b) => a.seq - b.seq || a.id - b.id);
  return merged;
}

/** 送达状态推进序：只会前进，不会后退。 */
const DELIVERY_ORDER = ["sent", "delivered", "read"];

/** 两个送达状态中更靠后的一个；不在推进序上的状态（sending / failed）保持原样。 */
export function furthestStatus(status: string, ahead: string): string {
  return DELIVERY_ORDER.indexOf(ahead) > DELIVERY_ORDER.indexOf(status) ? ahead : status;
}

/** 媒体消息 `content` 里的本地落盘路径；解析失败按「无路径」处理。 */
function mediaPathOf(content: string): string {
  try {
    const v = JSON.parse(content) as { path?: unknown };
    return typeof v.path === "string" ? v.path : "";
  } catch {
    return "";
  }
}

/**
 * 同一条媒体消息的两份记录，该以谁的 `content` 为准 —— **带 path 的那份优先**。
 *
 * 为什么不能一律"取新的"：群文件的 `path` 是**收完才回填**的（Offer 阶段先落库无 path
 * 的内容，Done 时再 emit/UPDATE 一次带 path 的记录）。两条后到的记录都可能是"回填之前"
 * 的形态：`loadMessages` 的 DB 快照、以及任何比它更早取数的 emit。一律取新 ⇒ 已回填的
 * path 被冲掉，而 `useMessageFile` 在 `path` 为空时**根本不去请求预览** ⇒ 空白气泡。
 * 真机形状：群里连发 9-10 张总有 1-2 张预览不出，单独发同一张必成功。
 */
export function pickMediaContent(
  prev: MessageRecord | undefined,
  incoming: MessageRecord,
): string {
  if (!prev) return incoming.content;
  if (incoming.kind !== "file" && incoming.kind !== "image") return incoming.content;
  if (mediaPathOf(incoming.content)) return incoming.content;
  return mediaPathOf(prev.content) ? prev.content : incoming.content;
}

/**
 * 会话重查（getMessages）的快照可能取自「对方已读 / Ack 落库之前」，直接覆盖会把
 * 界面上已推进的状态退回「发送中」→ 合并时保留两者中更靠后的状态。
 * 媒体行的 `path` 同理：回填后的记录不能被回填前的快照擦掉（见 `pickMediaContent`）。
 */
export function preserveDeliveryStatus(
  fresh: MessageRecord[],
  local: MessageRecord[],
): MessageRecord[] {
  if (local.length === 0) return fresh;
  const known = new Map(local.map((m) => [m.msg_id, m]));
  return fresh.map((m) => {
    const prev = known.get(m.msg_id);
    const best = prev ? furthestStatus(m.status, prev.status) : m.status;
    const content = pickMediaContent(prev, m);
    if (best === m.status && content === m.content) return m;
    return { ...m, status: best, content };
  });
}

/**
 * `loadMessages` 的 DB 快照只含库里已有的行；两类记录**只存在于内存**：
 * - `tmp-*`：send() 的乐观气泡（invoke 在途 / 已失败待重发）
 * - `file-failed-*`：sendFileTo 失败时插入的 failed 占位
 *
 * 整表覆盖会把它们吞掉 —— 用户看到"刚发的消息凭空消失"，以为失败而重发
 * （重复消息）；invoke 返回后 replaceMessage 又找不到列表项 → 真实记录
 * 也静默丢失（后端不回声 message-received，无第二路径补回）。
 * 把这些本地独有记录按原顺序追加到快照尾部（它们的 seq 是 MAX_SAFE_INTEGER，
 * 理应排在最新一页之后）。见 2026-09-23 审计 1.3。
 */
export function appendLocalOnly(
  fresh: MessageRecord[],
  local: MessageRecord[],
): MessageRecord[] {
  if (local.length === 0) return fresh;
  const freshIds = new Set(fresh.map((m) => m.msg_id));
  const localOnly = local.filter(
    (m) =>
      !freshIds.has(m.msg_id) &&
      (m.msg_id.startsWith("tmp-") || m.msg_id.startsWith("file-failed-")),
  );
  return localOnly.length ? [...fresh, ...localOnly] : fresh;
}

/**
 * 乐观记录（`tmp-*` msg_id）经 rAF 批量队列落地，而 invoke 可能先返回真实记录；
 * 此时按 msg_id 就地替换会落空，真实记录一旦被丢弃气泡就永久停在「发送中」。
 * 挂起的替换在批次落地这一唯一入口处完成。
 *
 * `currentStatuses` 是批次落地前各 msg_id 的状态快照；替换时用 furthestStatus
 * 与之比较，防止已推进到 read 的记录被一个 stale 的 pendingReplace 退回 delivered。
 */
export function applyReplacements(
  batch: MessageRecord[],
  replacements: Map<string, MessageRecord>,
  currentStatuses?: Map<string, string>,
): MessageRecord[] {
  if (replacements.size === 0) return batch;
  return batch.map((m) => {
    const next = replacements.get(m.msg_id);
    if (!next) return m;
    replacements.delete(m.msg_id);
    if (currentStatuses) {
      const cur = currentStatuses.get(m.msg_id);
      if (cur) return { ...next, status: furthestStatus(cur, next.status) };
    }
    return next;
  });
}

/**
 * 从 peers 列表同步好友昵称/头像到 friends 和单聊 conversations。
 * 纯函数，便于测试；调用方为 useChatStore.onPeers。
 */
export function syncProfileFromPeers(
  friends: { device_id: string; nickname: string; avatar: string | null }[],
  conversations: { id: string; kind: string; name: string; avatar: string | null }[],
  peers: { device_id: string; nickname: string; avatar: string | null }[],
): void {
  const peerMap = new Map(peers.map((p) => [p.device_id, p]));
  for (const f of friends) {
    const peer = peerMap.get(f.device_id);
    if (peer) {
      f.nickname = peer.nickname;
      f.avatar = peer.avatar;
    }
  }
  for (const c of conversations) {
    if (c.kind !== "single") continue;
    const peer = peerMap.get(c.id);
    if (peer) {
      c.name = peer.nickname;
      c.avatar = peer.avatar;
    }
  }
}

/** 消息摘要（会话列表展示）。 */
export function previewText(rec: MessageRecord): string {
  switch (rec.kind) {
    case "file":
      return "[文件]";
    case "image":
      return "[图片]";
    case "code":
      return "[代码]";
    // 合并转发：卡片是 JSON，截前 30 字符会得到 '{"title":"群聊的聊天记录"' 这种东西。
    case "merge":
      return mergeSummary(rec.content);
    case "todo":
    case "todo_update": {
      // 待办载荷是 JSON，直接吐出来就是「聊天列表/通知显示了一串 JSON」的毛病（用户 2026-09-17）。
      // 这里只取标题，回落到「[任务]」，让列表与通知都干净。
      try {
        const p = JSON.parse(rec.content) as { title?: unknown };
        if (typeof p.title === "string" && p.title) return `[任务] ${p.title}`;
      } catch {
        /* 落到回落值 */
      }
      return "[任务]";
    }
    case "poll":
    case "poll_vote": {
      // 投票载荷是 JSON：取问题，回落「[投票]」。
      try {
        const p = JSON.parse(rec.content) as { question?: unknown };
        if (typeof p.question === "string" && p.question) return `[投票] ${p.question}`;
      } catch {
        /* 落到回落值 */
      }
      return "[投票]";
    }
    case "announcement": {
      // 公告正文本身就是给用户看的 ⇒ 取正文；删公告是墓碑事件，只显示「[公告]」。
      try {
        const p = JSON.parse(rec.content) as { text?: unknown };
        if (typeof p.text === "string" && p.text) return `[公告] ${p.text}`;
      } catch {
        /* 落到回落值 */
      }
      return "[公告]";
    }
    case "announcement_delete":
      return "[公告]";
    // 以下都是**静默类**，照理到不了预览（两侧都按 non-notifying 过滤）；这里兜一层，
    // 防"某一侧的过滤条件日后变了"再把 JSON 露出去。与 Rust `protocol::preview_text` 一致。
    case "reaction":
      return "[回应]";
    case "recall":
    case "recalled":
      return "[撤回]";
    case "pin":
      return "[置顶]";
    default:
      // ⚠️ 表里**没有**的 kind（对端版本比本机新）绝不能原样截断 —— 那是"界面上出现一串
      // JSON 字符串"的最后一环（INV-P24 第 2 条）。判据与文案都与 Rust `preview_text` 同源，
      // 由 `messageKinds.test.ts` 机器比对。已知但无专门文案的（text / system）照旧截断。
      if (!isKnownKind(rec.kind)) return UNSUPPORTED_KIND_LABEL;
      return rec.content.slice(0, 30);
  }
}

/**
 * 该消息是否 @ 了指定昵称（仅文本消息参与判断）。
 * 边界语义与 linkify 的 @提及高亮**共用** MENTION_BEFORE / MENTION_AFTER 两个常量，
 * 不允许在这里另写一份：两套边界一旦分叉，表现就是「气泡里高亮成蓝块、却没有红点和通知」。
 * 名字后的边界保证 @张三 不会误吞 @张三丰；前导边界见 MENTION_BEFORE（`]` 也算）。
 */
export function messageMentionsName(rec: MessageRecord, name: string): boolean {
  if (rec.kind !== "text") return false;
  const n = name.trim();
  if (!n) return false;
  return new RegExp(`${MENTION_BEFORE}@${escapeRe(n)}${MENTION_AFTER}`).test(rec.content);
}

/**
 * 「@所有人」在正文里的字面形式。
 *
 * 恒为中文三个字，**不随发送方界面语言变化**：它是一条落到消息正文里、要靠字面匹配
 * 才认得出的文本，而不是本地化文案。若按界面语言发成 `@All`，中文界面的成员就认不出来，
 * 红点与高亮会一起失效。
 */
export const MENTION_ALL_TOKEN = "所有人";

/**
 * 预编译的 @所有人 判定正则。模板是常量，没有理由每收到一条群消息就重新编译一次
 * （消息摄入是热路径）。无 `g` 标志 ⇒ `.test()` 不带 lastIndex 状态，可安全复用。
 */
const MENTION_ALL_RE = new RegExp(`${MENTION_BEFORE}@${MENTION_ALL_TOKEN}${MENTION_AFTER}`);

/**
 * 该消息是否 @ 了所有人。
 *
 * 与 `messageMentionsName` 同源：边界规则完全一致（@ 前须行首/空白，后须空白/标点/行尾），
 * 所以 `@所有人甲乙` 不会误命中。仅文本消息参与判断，与既有 @提及 口径一致。
 */
export function messageMentionsAll(rec: MessageRecord): boolean {
  if (rec.kind !== "text") return false;
  return MENTION_ALL_RE.test(rec.content);
}

/**
 * 计算「消息缓存该保留哪些会话」：活跃会话必留，其余按 LRU 保留最近使用的至多 `maxConvs` 个。
 *
 * 纯函数，供 useChatStore 的消息缓存上界使用；调用方负责删除未保留的键。
 * 单独抽出来的原因：这是「会不会把用户正在看的会话淘汰掉」的唯一判定点，必须有单测锁定。
 */
export function selectCachedConversations(
  lruOrder: string[],
  activeId: string | null | undefined,
  maxConvs: number,
): Set<string> {
  const keep = new Set<string>(lruOrder.slice(-maxConvs));
  // 活跃会话可能不在 LRU 里（例如从系统通知直接打开、尚未 touch 过）→ 必须补上
  if (activeId) keep.add(activeId);
  return keep;
}

/**
 * 会话列表排序：**置顶优先，其次按最后消息时间倒序**。
 *
 * 这是全应用唯一的会话排序口径 —— 列表渲染、新消息合入、切换置顶都必须走它。
 * 分散成多处 `sort` 会让「置顶了但被新消息挤下去」这类不一致在不同路径上分别出现。
 */
export function sortConversations(list: Conversation[]): Conversation[] {
  return [...list].sort((a, b) => {
    if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
    return (b.last_ts ?? 0) - (a.last_ts ?? 0);
  });
}

/**
 * 将一批新消息应用到会话列表：更新 last_msg/last_ts、累计未读（活跃会话不计），
 * 再按 `sortConversations` 重排。纯函数，便于测试与复用。
 */
export function applyIncomingToConversations(
  conversations: Conversation[],
  activeConvId: string | null,
  incomingByConv: Map<string, MessageRecord[]>,
): Conversation[] {
  const next: Conversation[] = conversations.map((c) => ({ ...c }));
  for (const [convId, rawMsgs] of incomingByConv) {
    // ⚠️ 静默类不参与未读与预览 —— 与后端 `is_non_notifying_kind` 同一口径。
    // 漏掉它的后果：别人回个表情，你的会话列表未读 +1、预览变成一段 JSON、
    // 会话还被顶到最前（后端 DB 里未读是 0，两边从此不一致）。
    const msgs = rawMsgs.filter((m) => !isSilentKind(m.kind));
    if (msgs.length === 0) continue;
    const last = msgs[msgs.length - 1];
    const conv = next.find((c) => c.id === convId);
    if (!conv) continue;
    conv.last_msg = previewText(last);
    conv.last_ts = last.ts;
    if (convId !== activeConvId) conv.unread += msgs.length;
  }
  return sortConversations(next);
}
