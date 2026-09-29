/**
 * 表情回应的**折叠**（纯函数，便于单测）。
 *
 * 回应在协议层是一条条独立消息（`kind = "reaction"`），而不是「给消息加一个可变字段」——
 * 因为 `message_id` 是 `SHA-256(sender_id + nonce + payload)`，同一条业务消息不可能带
 * 不同 content 重发。所以「当前有哪些回应」是**折叠出来的派生态**，不是存下来的状态。
 *
 * 收敛规则：每个 `(target, actor, emoji)` 三元组是一个 LWW 寄存器、值为 `add`。
 * 每人只写自己那一格 ⇒ 不存在丢更新；版本号用 `(seq, msg_id)` ⇒ 任意到达顺序收敛。
 */
import type { MessageRecord, ReactionPayload } from "@/types";

export interface ReactionChip {
  /** 表情 token，如 "[赞]" */
  emoji: string;
  count: number;
  /** 我有没有参与（用于高亮自己点过的） */
  mine: boolean;
  /**
   * 参与者 device_id，**按追加先后**（末位 = 最新点这个表情的人）。
   *
   * ⚠️ 不是字典序、也不是首次出现顺序：见 `foldReactions` 里那段"排序键只能用
   * 已经参与 LWW 的那对值"。名单要回答的是"谁最新追加"，顺序一错信息就归零。
   */
  actors: string[];
  /** 最新追加这个表情的人（= `actors` 末位；空字符串 = 没人）。单独给一个字段是为了让调用点不靠"取最后一个"来表意。 */
  latest: string;
}

/** 解析回应载荷；畸形或非回应消息返回 null（不抛错，坏数据不该打断渲染）。 */
export function parseReaction(rec: MessageRecord): ReactionPayload | null {
  if (rec.kind !== "reaction") return null;
  try {
    const p = JSON.parse(rec.content) as Partial<ReactionPayload>;
    if (typeof p?.target !== "string" || !p.target) return null;
    if (typeof p?.emoji !== "string" || !p.emoji) return null;
    if (typeof p?.add !== "boolean") return null;
    return { target: p.target, emoji: p.emoji, add: p.add };
  } catch {
    return null;
  }
}

/**
 * 折叠出「每条消息 → 它的回应 chips」。
 *
 * ⚠️ **版本号必须是 `(seq, msg_id)` 元组**，不能只看 `seq`：seq 是 Lamport 逻辑时钟，
 * `next_clock` 是 local+1、`observe_clock` 是 max(local, observed) —— 两端各自离线时
 * 都可能停在同一个 seq，重连后各发一条**都拿到同一个 seq**。只看 seq 的话，不同副本
 * 会算出不同结果（这就是「同一件事在我这儿和在你那儿显示不一样」的根因）。
 * msg_id 全局唯一且被信封签名，各副本一致，用它做 tie-break 才能收敛。
 */
export function foldReactions(
  records: MessageRecord[],
  myDeviceId: string,
): Map<string, ReactionChip[]> {
  // key = target \0 actor \0 emoji → 当前生效的版本与 add
  const latest = new Map<string, { seq: number; msgId: string; add: boolean; target: string; emoji: string; actor: string }>();

  for (const rec of records) {
    const p = parseReaction(rec);
    if (!p) continue;
    const actor = rec.sender_id;
    const key = `${p.target}\u0000${actor}\u0000${p.emoji}`;
    const cur = latest.get(key);
    // 元组序比较：先 seq，再 msg_id
    const newer =
      !cur || rec.seq > cur.seq || (rec.seq === cur.seq && rec.msg_id > cur.msgId);
    if (newer) {
      latest.set(key, {
        seq: rec.seq,
        msgId: rec.msg_id,
        add: p.add,
        target: p.target,
        emoji: p.emoji,
        actor,
      });
    }
  }

  // target → emoji → 追加记录（**按生效那条的 (seq, msg_id) 排序**）
  //
  // 这里刻意**不按 device_id 字典序**：用户 2026-09-29 要的是"看得出谁最新追加"，
  // 而字典序把"最后点的那个人"放到随机位置 —— 名单就退化成一份没有信息的名单。
  // 首次出现顺序也不行（那是 `latest` 迭代顺序，跨两端不保证一致），所以排序键
  // 只能用**已经参与 LWW 的那对值**：它在所有副本上一致（见上面那段 tie-break 说明）。
  type Entry = { actor: string; seq: number; msgId: string };
  const byTarget = new Map<string, Map<string, Entry[]>>();
  for (const v of latest.values()) {
    if (!v.add) continue; // 取消的不计入
    let byEmoji = byTarget.get(v.target);
    if (!byEmoji) {
      byEmoji = new Map();
      byTarget.set(v.target, byEmoji);
    }
    const actors = byEmoji.get(v.emoji) ?? [];
    actors.push({ actor: v.actor, seq: v.seq, msgId: v.msgId });
    byEmoji.set(v.emoji, actors);
  }

  const out = new Map<string, ReactionChip[]>();
  for (const [target, byEmoji] of byTarget) {
    const chips: ReactionChip[] = [];
    for (const [emoji, entries] of byEmoji) {
      entries.sort((a, b) => a.seq - b.seq || (a.msgId < b.msgId ? -1 : a.msgId > b.msgId ? 1 : 0));
      const actors = entries.map((e) => e.actor);
      chips.push({
        emoji,
        count: actors.length,
        mine: actors.includes(myDeviceId),
        actors,
        // 追加顺序里的最后一个 = **最新点这个表情的人**；同一个人取消后再点，
        // 生效那条的 seq 是新的 ⇒ 他会重新排到最后（这才叫"最新追加"）。
        latest: actors[actors.length - 1] ?? "",
      });
    }
    out.set(target, chips);
  }
  return out;
}

/** 名单一次最多列出几个人，剩下的折成「+N」（用户：人多要显示 +N，不许把气泡撑爆）。 */
/**
 * 1:1 回应的线上形状（群侧由 Rust 的 `send_group_reaction` 自己拼，因为那条命令收的是
 * 分开的参数）。字段名必须与 Rust `protocol::ReactionPayload` 逐字一致 ——
 * 后端会解析并校验它（`parse_reaction_payload`），对不上就是当场报错。
 * `dmReaction.test.ts` 直接读那份 struct 比对键名，防两侧漂成两个词。
 */
export function buildReactionPayload(target: string, emoji: string, add: boolean): string {
  return JSON.stringify({ target, emoji, add });
}

export const ROSTER_VISIBLE = 3;

/** 把 actor 列表折成"前 N 个名字 + 还有几人"。名字由调用方给（设备 id 对用户没有意义）。 */
export function summarizeActors(
  actors: readonly string[],
  nameOf: (id: string) => string,
  limit: number = ROSTER_VISIBLE,
): { shown: string[]; hidden: number } {
  const cap = Math.max(1, limit);
  return {
    shown: actors.slice(0, cap).map(nameOf),
    hidden: Math.max(0, actors.length - cap),
  };
}

/** 某条消息上「我」对某表情是否已点过（用于点击时决定 add 还是 remove）。 */
export function hasMyReaction(
  records: MessageRecord[],
  target: string,
  emoji: string,
  myDeviceId: string,
): boolean {
  return (foldReactions(records, myDeviceId).get(target) ?? []).some(
    (c) => c.emoji === emoji && c.mine,
  );
}
