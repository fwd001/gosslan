/**
 * 群任务的**折叠**（纯函数，便于单测）。
 *
 * 一条任务 = 定义层里的一个 **LWW 寄存器**：`kind = "todo"`，按 `todo_id` 合并，
 * 版本 = `(seq, msg_id)`。改标题 / 换指派人 / 改状态 / 删除都是"重新发一份定义"，
 * 所以折叠只需要"同 `todo_id` 取最新那一份"。
 *
 * **状态是任务级的单值**（用户 2026-09-16 定的四态：待办 / 进行中 / 延期 / 完成，
 * 手动选、不做截止时间）—— 不是"每人各自一格完成"。
 * 为什么单值用 LWW 就够：一条任务"当前处于什么阶段"本来就是单值语义，并发改动时后写者胜
 * 是期望行为（看板类工具都这样）；真正会丢更新的模型是"每人一格的完成标记"，而这里不做它。
 *
 * 授权口径与后端 `commands::may_update_todo` **必须一致**（后端是权威、这里是显示用的镜像）：
 * 改状态 = 创建者或被指派人；改标题/指派人/删除 = 创建者或群主。两边都有各自的用例表。
 */
import type { MessageRecord } from "@/types";

/**
 * 任务状态取值 —— **与 Rust `protocol::TODO_STATUSES` 必须一致**，
 * 由 `messageKinds.test.ts` 读 `protocol.rs` 逐项比对（与 `WIRE_KINDS` 同一套跨语言契约）。
 */
export const TODO_STATUSES = ["todo", "doing", "overdue", "done"] as const;

export type TodoStatus = (typeof TODO_STATUSES)[number];

/** 新建任务的缺省状态（与 Rust `default_todo_status()` 同值）。 */
export const TODO_STATUS_DEFAULT: TodoStatus = "todo";

/**
 * 状态 → i18n key。放在这里而不是各面板里各写一份：任务状态在**两个**地方出现
 * （群任务面板、群成员面板的任务分区），两处各写一份就会漂移
 * （典型症状：一个面板写「进行中」、另一个写「处理中」）。
 */
export const TODO_STATUS_LABEL_KEY: Record<TodoStatus, string> = {
  todo: "todo.status.todo",
  doing: "todo.status.doing",
  overdue: "todo.status.overdue",
  done: "todo.status.done",
};

/**
 * 状态 → 文字颜色。彩色**文字**一律走 `*-ink` 档（设计规范 §3.1 的硬约束）。
 *
 * ⚠️ 「延期」用 **warning（橙）**：它是"需要关注"而不是"出错"。此前这里是 `danger-ink`（红），
 * 而群任务看板自己抄了一份橙 —— 同一个状态在两个面板里两种颜色（用户 2026-09-17 视觉走查发现）。
 * 现在三个消费者（本文件、看板、任务卡气泡）共用下面这份映射，口径只有一处。
 */
export const TODO_STATUS_CLASS: Record<TodoStatus, string> = {
  todo: "text-[var(--gosslan-text-2)]",
  doing: "text-[var(--gosslan-primary)]",
  overdue: "text-[var(--gosslan-warning-ink)]",
  done: "text-[var(--gosslan-success-ink)]",
};

/**
 * 状态 → **胶囊徽标**配色（soft 底 + `*-ink` 字）—— 与应用的「胶囊徽标」同一套。
 *
 * 看板与任务卡气泡共用（此前各自抄了一份）：`doing` 的主题色浅底用 `color-mix` 派生，
 * 跟随用户自定义主题色，不写死色值。
 */
export const TODO_STATUS_PILL: Record<TodoStatus, string> = {
  todo: "bg-[var(--gosslan-hover)] text-[var(--gosslan-text-2)]",
  doing: "bg-[color-mix(in_srgb,var(--gosslan-primary)_14%,transparent)] text-[var(--gosslan-accent-ink)]",
  overdue: "bg-[var(--gosslan-warning-soft)] text-[var(--gosslan-warning-ink)]",
  done: "bg-[var(--gosslan-success-soft)] text-[var(--gosslan-success-ink)]",
};

export function isTodoStatus(v: unknown): v is TodoStatus {
  return typeof v === "string" && (TODO_STATUSES as readonly string[]).includes(v);
}

export interface TodoImage {
  /** 跨端唯一 id（= 文件 sha256 / cid），落盘文件名与去重键。 */
  id: string;
  name: string;
  size: number;
  sha256: string;
  subtype: string;
}

export interface TodoItem {
  todoId: string;
  title: string;
  assignees: string[];
  status: TodoStatus;
  creator: string;
  /** 长文本描述（2026-09-17 优化）。 */
  description: string;
  /** 描述里附带的图片（仅元数据，真实字节走群文件管线）。 */
  images: TodoImage[];
  /** 是否**显式**归档（完成之后手动归档；用户 2026-09-17 起完成不再自动归档）。 */
  archived: boolean;
  /** 状态变为「完成」的权威时间戳（ms）；未完成/旧载荷为 null。 */
  doneAt: number | null;
  /** 创建时间（创建那条 `todo` 消息的 ts）；由 `foldTodos` 从创建记录回填，旧数据可能为 null。 */
  createdAt: number | null;
}

interface TodoDef extends TodoItem {
  deleted: boolean;
  seq: number;
  msgId: string;
}

/** 完成态超过该天数后自动归档（前端计算，见 `isEffectivelyArchived`）。 */
export const TODO_AUTO_ARCHIVE_DAYS = 7;

/** 版本序比较：**(seq, msg_id) 元组**。只看 seq 会让不同副本算出不同结果
 *  （seq 是 Lamport 时钟，两端离线后各发一条都可能拿到同一个 seq）。
 *  ⚠️ 与 Rust `commands::latest_todo_def` 的 `ORDER BY seq DESC, msg_id DESC` 同规则。 */
function newer(seq: number, msgId: string, cur: { seq: number; msgId: string } | undefined): boolean {
  return !cur || seq > cur.seq || (seq === cur.seq && msgId > cur.msgId);
}

export function parseTodo(rec: MessageRecord): TodoDef | null {
  // 两种 kind 同构：`todo` = 创建（Card，会通知），`todo_update` = 改状态/改标题/删除
  // （Silent，不打扰全群）。它们同属一条 LWW 序列，所以折叠时一视同仁。
  if (rec.kind !== "todo" && rec.kind !== "todo_update") return null;
  try {
    const p = JSON.parse(rec.content) as Record<string, unknown>;
    if (typeof p.todo_id !== "string" || !p.todo_id) return null;
    return {
      todoId: p.todo_id,
      title: typeof p.title === "string" ? p.title : "",
      assignees: Array.isArray(p.assignees)
        ? p.assignees.filter((x): x is string => typeof x === "string")
        : [],
      // 未知/缺失状态一律回落「待办」：宁可显示成一条待办，也不要让这条任务从列表里消失
      status: isTodoStatus(p.status) ? p.status : TODO_STATUS_DEFAULT,
      creator: typeof p.creator === "string" ? p.creator : "",
      deleted: p.deleted === true,
      description: typeof p.description === "string" ? p.description : "",
      images: Array.isArray(p.images)
        ? p.images.filter(
            (x): x is TodoImage =>
              !!x && typeof x === "object" && typeof (x as TodoImage).sha256 === "string",
          )
        : [],
      archived: p.archived === true,
      doneAt: typeof p.done_at === "number" ? p.done_at : null,
      createdAt: null,
      seq: rec.seq,
      msgId: rec.msg_id,
    };
  } catch {
    return null;
  }
}

/** 折叠出当前全部任务（墓碑不列），按**创建版本从新到旧**。
 *
 * 创建时间单独收集：它来自**创建那条记录**（`kind === "todo"`），而 LWW 折叠保留的是
 * 最新定义（往往是 `todo_update`）—— 两条是不同的记录，不能混在一张表里。 */
export function foldTodos(records: MessageRecord[]): TodoItem[] {
  const defs = new Map<string, TodoDef>();
  const created = new Map<string, number>();
  for (const rec of records) {
    const d = parseTodo(rec);
    if (!d) continue;
    if (rec.kind === "todo") {
      // 防御性保留较早的 ts（同一 todo_id 只应有一条创建记录）。
      const prev = created.get(d.todoId);
      if (prev === undefined || rec.ts < prev) created.set(d.todoId, rec.ts);
    }
    if (newer(d.seq, d.msgId, defs.get(d.todoId))) defs.set(d.todoId, d);
  }
  return [...defs.values()]
    .filter((d) => !d.deleted)
    .sort((a, b) => {
      if (a.seq !== b.seq) return b.seq - a.seq; // 新的在前
      return a.msgId < b.msgId ? 1 : -1; // 同 seq 按 msg_id 比（与 newer 同规则）
    })
    .map(({ todoId, title, assignees, status, creator, description, images, archived, doneAt }) => ({
      todoId,
      title,
      assignees,
      status,
      creator,
      description,
      images,
      archived,
      doneAt,
      createdAt: created.get(todoId) ?? null,
    }));
}

/**
 * 一条任务**实际上**是否已归档（用于列表过滤）。
 *
 * 两种情况（用户 2026-09-17：完成之后手动归档）：
 * - 显式 `archived` 为真 ⇒ 已归档（用户在完成之后手动点的）；
 * - 或「完成且超过 `TODO_AUTO_ARCHIVE_DAYS` 天」⇒ 自动归档（没手动归档的兜底）。
 *
 * 自动归档是**前端计算**的（`doneAt` 由后端在**首次**完成时填权威时间戳、重复保存不改），
 * 不另存字段、也不依赖定时任务，跨端结果一致。
 */
export function isEffectivelyArchived(
  item: Pick<TodoItem, "status" | "archived" | "doneAt">,
  now: number = Date.now(),
): boolean {
  if (item.archived) return true;
  if (item.status === "done" && item.doneAt != null) {
    return now - item.doneAt >= TODO_AUTO_ARCHIVE_DAYS * 86400000;
  }
  return false;
}

/**
 * 我能不能改这条任务（**显示用**的镜像，后端 `commands::may_update_todo` 才是权威）。
 *
 * | 改动 | 允许谁 |
 * |---|---|
 * | 改标题 / 删除（结构） | 创建者 **或** 群主 |
 * | 其余改动（描述 / 图片 / 指派人 / 状态） | 创建者 **或** 群主 **或** 当前被指派人 |
 *
 * 群主**什么都能改**（用户 2026-09-20：「群主不能编辑群任务」—— 此前群主的改动被落到
 * 「只改状态」那一档，描述 / 图片 / 状态都会被拒）。
 *
 * 判据与后端 `may_update_todo` 必须一致（后端是权威、这里是显示用的镜像）。
 */
export function canUpdateTodo(
  item: Pick<TodoItem, "creator" | "assignees">,
  actor: string,
  groupCreator: string,
  structural: boolean,
): boolean {
  // 创建者 / 群主：改什么都行。
  if (item.creator === actor || groupCreator === actor) return true;
  // 被指派人：能改描述 / 图片 / 指派人 / 状态，但不能改结构（标题 / 删除）。
  if (structural) return false;
  return item.assignees.includes(actor);
}

/**
 * 我能不能**改指派人**（用户 2026-09-17：被 @ 的人也能转派/加人）。
 * 创建者 / 群主 / 当前被指派人 都行；与后端 `edits_assignees` 分支一致。
 */
export function canEditAssignees(
  item: Pick<TodoItem, "creator" | "assignees">,
  actor: string,
  groupCreator: string,
): boolean {
  if (item.creator === actor) return true;
  if (groupCreator === actor) return true;
  return item.assignees.includes(actor);
}

/**
 * 该任务是否点名了我（被指派人之一）—— 用于「有人@我」徽标与通知。
 *
 * 显式 @ 指派的人（用户 2026-09-17 定的口径）：被指派 ═ 被 @。指派人是 device id，
 * 所以直接拿我的 `device_id` 去比对 `assignees`，不走昵称匹配
 * （昵称可重复、可改，device id 才是稳定身份）。
 */
export function todoMentionsMe(rec: MessageRecord, myId: string): boolean {
  if (!myId) return false;
  const d = parseTodo(rec);
  if (!d) return false;
  return d.assignees.includes(myId);
}

/**
 * 这条记录是否是「**我创建的任务被完成了**」—— 用于给创建人提示（用户 2026-09-17）。
 *
 * 返回任务标题（标题为空串时仍返回空串，调用方自行回落文案）；不是该情形返回 `null`。
 *
 * 为什么单独放行：改状态走的是 `todo_update`（Silent，不打扰全群），但"我派的活被干完了"
 * 对**创建人**是个该知道的变化 —— 只在创建人这里破例，其他人仍然一条通知都不多。
 * 只认「完成」这一种 status，改标题/改指派人/删除都不提示。
 */
export function todoCompletedForCreator(rec: MessageRecord, myId: string): string | null {
  if (!myId || rec.kind !== "todo_update") return null;
  const d = parseTodo(rec);
  if (!d || d.deleted || d.status !== "done" || d.creator !== myId) return null;
  return d.title;
}
