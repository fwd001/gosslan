/**
 * 强提醒的**折叠**（纯函数，便于单测）。
 *
 * 与 reactions/pins 同构：提醒在协议层是一条条独立静默消息（kind = remind / remind_ack），
 * 「当前是什么状态」由接收端按 `(seq, msg_id)` 元组折叠出来，不新增数据库列/表。
 *
 * 六态归属（见 docs/strong-reminder-plan.md §四）：
 * - S1 pending：remind 行 + outbox 在册（行状态 sending/sent）
 * - S2 delivered：接收端 Ack 已回（remind 行 delivered/read）
 * - S3 reminded：接收端自动回执 stage=alerted —— 只证明"对方设备已处理"，不是用户看到
 * - S4 confirmed：用户主动点「我知道了」stage=confirmed
 * - S5 failed：重试预算耗尽 / 门控拒发（行状态 failed 且无更高证据）
 *
 * 传输层 Ack ≠ 用户确认；已读 ≠ 用户确认。状态只进不退，旧事件不覆盖新状态。
 */
import type { MessageRecord, RemindPayload, RemindAckPayload, RemindAckStage } from "@/types";

export type ReminderPhase = "failed" | "pending" | "delivered" | "reminded" | "confirmed";

/** 状态秩：只进不退。failed=0：允许"失败后又收到更高证据"翻案（帧其实送到了）。 */
const RANK: Record<ReminderPhase, number> = {
  failed: 0,
  pending: 1,
  delivered: 2,
  reminded: 3,
  confirmed: 4,
};

function maxPhase(a: ReminderPhase, b: ReminderPhase): ReminderPhase {
  return RANK[a] >= RANK[b] ? a : b;
}

/** 解析发起载荷；非 remind 行或坏载荷返回 null（坏数据不打断渲染）。 */
export function parseRemind(rec: MessageRecord): RemindPayload | null {
  if (rec.kind !== "remind") return null;
  try {
    const p = JSON.parse(rec.content) as Partial<RemindPayload>;
    if (typeof p?.target !== "string" || !p.target.trim()) return null;
    if (p.actors != null && !Array.isArray(p.actors)) return null;
    const actors = (p.actors ?? []).filter((x): x is string => typeof x === "string");
    return { target: p.target, actors };
  } catch {
    return null;
  }
}

/** 解析回执载荷；非法/坏行返回 null。 */
export function parseRemindAck(rec: MessageRecord): RemindAckPayload | null {
  if (rec.kind !== "remind_ack") return null;
  try {
    const p = JSON.parse(rec.content) as Partial<RemindAckPayload>;
    if (typeof p?.target !== "string" || !p.target.trim()) return null;
    if (p.stage !== "alerted" && p.stage !== "confirmed") return null;
    return { target: p.target, stage: p.stage };
  } catch {
    return null;
  }
}

/** remind 行自身状态 → 投递相（S1/S2/S5）。 */
function deliveryPhase(status: MessageRecord["status"]): ReminderPhase {
  if (status === "failed") return "failed";
  if (status === "delivered" || status === "read") return "delivered";
  return "pending";
}

export interface ReminderState {
  /** remind 事件自身 msg_id */
  remindId: string;
  /** 被提醒的原消息 msg_id */
  target: string;
  /** 总览相（群聊＝最慢的一台） */
  phase: ReminderPhase;
  /** 群聊：每台目标设备各自一态（key=device_id）；1:1 也由回执发送方填一格 */
  perActor: Partial<Record<string, ReminderPhase>>;
}

/**
 * 折叠出「原消息 → 提醒状态」。
 *
 * ⚠️ 回执的版本号必须按 `(seq, msg_id)` 元组 LWW（理由同 foldReactions：离线两端可能
 * 拿到相同 seq，msg_id 是被信封签名的全局唯一 tie-break）。
 */
export function foldReminders(records: MessageRecord[]): Map<string, ReminderState> {
  // remindId → 发起行状态
  const roots = new Map<
    string,
    { target: string; base: ReminderPhase; actors: Map<string, ReminderPhase> }
  >();

  for (const rec of records) {
    const p = parseRemind(rec);
    if (!p) continue;
    const base = deliveryPhase(rec.status);
    const actors = new Map<string, ReminderPhase>();
    for (const a of p.actors) actors.set(a, base);
    roots.set(rec.msg_id, { target: p.target, base, actors });
  }

  // 回执寄存器：key = remindId\0actor\0stage → 赢的那版 (seq,msgId)
  const ackWinner = new Map<string, { seq: number; msgId: string }>();
  // 每次赢的回执按 (remindId, actor) 收集有效 stage
  const wonAcks = new Map<string, Map<string, Set<RemindAckStage>>>();

  for (const rec of records) {
    const p = parseRemindAck(rec);
    if (!p) continue;
    if (!roots.has(p.target)) continue; // 没有对应发起行的回执挂不住（忽略，不造状态）
    const actor = rec.sender_id;
    const key = `${p.target}\u0000${actor}\u0000${p.stage}`;
    const cur = ackWinner.get(key);
    const newer = !cur || rec.seq > cur.seq || (rec.seq === cur.seq && rec.msg_id > cur.msgId);
    if (!newer) continue;
    ackWinner.set(key, { seq: rec.seq, msgId: rec.msg_id });

    let byActor = wonAcks.get(p.target);
    if (!byActor) {
      byActor = new Map();
      wonAcks.set(p.target, byActor);
    }
    let stages = byActor.get(actor);
    if (!stages) {
      stages = new Set();
      byActor.set(actor, stages);
    }
    stages.add(p.stage);
  }

  const out = new Map<string, ReminderState>();
  for (const [remindId, root] of roots) {
    const perActor: Partial<Record<string, ReminderPhase>> = {};
    const acks = wonAcks.get(remindId);

    // 群：显式名单初始化；每台设备 = max(投递相, 该设备赢的回执相)
    for (const [actor, base] of root.actors) {
      let phase = base;
      const stages = acks?.get(actor);
      if (stages) {
        for (const s of stages) {
          phase = maxPhase(phase, s === "confirmed" ? "confirmed" : "reminded");
        }
      }
      perActor[actor] = phase;
    }

    // 1:1（名单为空）或名单外的回执发送方：补格子
    if (acks) {
      for (const [actor, stages] of acks) {
        let phase = root.actors.size === 0 ? root.base : (perActor[actor] ?? "pending");
        for (const s of stages) {
          phase = maxPhase(phase, s === "confirmed" ? "confirmed" : "reminded");
        }
        perActor[actor] = phase;
      }
    }

    const actorPhases = Object.values(perActor) as ReminderPhase[];
    let phase: ReminderPhase;
    if (root.actors.size === 0) {
      // 1:1：base 与回执取最大（失败可被翻案）
      phase = root.base;
      for (const p of actorPhases) phase = maxPhase(phase, p);
    } else if (actorPhases.length === 0) {
      phase = root.base;
    } else if (actorPhases.every((p) => p === "failed")) {
      // 群：**全部**设备失败才是失败 —— 只要有一台给出了更高证据，就不该对发起方
      // 报「发送失败」（提醒确实送到了一部分人；逐台状态仍可在 roster title 里看到）。
      phase = "failed";
    } else {
      // 群：总览＝最慢的一台，但 failed 格在"并非全败"时不参与最慢比较
      // （它代表"这台尚无任何证据"，不是一个可向用户展示的进展相）。
      phase = actorPhases
        .filter((p) => p !== "failed")
        .reduce((a, b) => (RANK[a] <= RANK[b] ? a : b));
    }

    out.set(root.target, { remindId, target: root.target, phase, perActor });
  }
  return out;
}

/**
 * 发起载荷 builder（字段名与 Rust `protocol::RemindPayload` 逐字一致，
 * 契约/解析两侧共用，不另造词表）。
 */
export function buildRemindPayload(target: string, actors: string[] = []): string {
  return JSON.stringify({ target, actors });
}

/** 回执载荷 builder（对应 Rust `RemindAckPayload`）。 */
export function buildRemindAckPayload(target: string, stage: RemindAckStage): string {
  return JSON.stringify({ target, stage });
}
