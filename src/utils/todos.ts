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
 * 授权口径与后端 `commands::may_change_todo` **必须一致**（后端是权威、这里是显示用的镜像）：
 * 改状态 = 创建者或被指派人；改标题/指派人/删除 = 创建者或群主；
 * **只动归档位** = 任何群成员（用户 2026-09-24）。两边都有各自的用例表。
 */
import type { MessageRecord } from "@/types";

/**
 * 任务状态取值 —— **与 Rust `protocol::TODO_STATUSES` 必须一致**，
 * 由 `messageKinds.test.ts` 读 `protocol.rs` 逐项比对（与 `WIRE_KINDS` 同一套跨语言契约）。
 */
export const TODO_STATUSES = ["todo", "doing", "overdue", "done"] as const;

export type TodoStatus = (typeof TODO_STATUSES)[number];

/**
 * 任务优先级三档的**唯一取值表**（与 Rust `TODO_PRIORITIES` 同序同值）。
 * ⚠️ 改这里必须同时改 Rust 那一处：`src/utils/messageKinds.test.ts` 会直接读 `protocol.rs`
 * 比对**表体与缺省值**（缺省值不一致更阴 —— 旧载荷没这一格时两侧各回落一份，
 * 同一条任务在两个成员界面上显示成不同档位）。
 */
export const TODO_PRIORITIES = ["high", "normal", "low"] as const;
export type TodoPriority = (typeof TODO_PRIORITIES)[number];
/** 缺省档（历史任务与旧版载荷读出来都是它，不是"没优先级"）。 */
export const TODO_PRIORITY_DEFAULT: TodoPriority = "normal";
/** 展示名：一句话说得清、卡片里放得下。 */
export const TODO_PRIORITY_LABEL_KEY: Record<TodoPriority, string> = {
  high: "todo.priority.high",
  normal: "todo.priority.normal",
  low: "todo.priority.low",
};
/** 只有「紧急」抢视觉权重；常规/不急刻意压在次级色，避免三档都变红海。 */
export const TODO_PRIORITY_CLASS: Record<TodoPriority, string> = {
  high: "font-medium text-[var(--gosslan-primary)]",
  normal: "text-[var(--gosslan-text-2)]",
  low: "text-[var(--gosslan-text-2)]",
};

function isTodoPriority(v: unknown): v is TodoPriority {
  return typeof v === "string" && (TODO_PRIORITIES as readonly string[]).includes(v);
}

/**
 * 任务**类型**三档的取值表（与 Rust `TODO_CATEGORIES` 同序同值，同一套跨语言守卫）。
 *
 * 三者**同列一张表**，靠 tag 区分（用户 2026-09-29：不要为类型另开列表、也不要筛选成互斥视图）。
 * 加第四种 = 这里与 Rust 各加一个值 + 下面 label/class 两张表补齐。
 * 名字用 `category` 而不是 `kind`：本仓的 `kind` 固定指线上消息类型，
 * 而 `parseTodo` 上一行就在判 `rec.kind`，同名会在同一段代码里撞车。
 */
export const TODO_CATEGORIES = ["task", "requirement", "bug"] as const;
export type TodoCategory = (typeof TODO_CATEGORIES)[number];
/** 缺省档（历史任务与旧版载荷读出来都是它，不是"没类型"）。 */
export const TODO_CATEGORY_DEFAULT: TodoCategory = "task";
export const TODO_CATEGORY_LABEL_KEY: Record<TodoCategory, string> = {
  task: "todo.category.task",
  requirement: "todo.category.requirement",
  bug: "todo.category.bug",
};
/**
 * 类型 → **胶囊徽标**配色（soft 底 + `*-ink` 字，与 `TODO_STATUS_PILL` 同一套家风）。
 *
 * 与优先级刻意分开：优先级是**文字**、只有「紧急」抢主色；类型三档都是角标。
 * 「缺陷」用 danger 是语义正当的（它确实是出错），与「延期」那次视觉走查的纠正不冲突 ——
 * 那条是"需要关注"、当时被错标成红，这里不重复那个误用。
 * ⚠️ 彩色文字一律走 `*-ink` 档（设计规范 §3.1）；这里只用 style.css 里**已存在**的 token。
 */
export const TODO_CATEGORY_PILL: Record<TodoCategory, string> = {
  task: "bg-[var(--gosslan-hover)] text-[var(--gosslan-text-2)]",
  requirement:
    "bg-[color-mix(in_srgb,var(--gosslan-primary)_14%,transparent)] text-[var(--gosslan-accent-ink)]",
  bug: "bg-[var(--gosslan-danger-soft)] text-[var(--gosslan-danger-ink)]",
};

function isTodoCategory(v: unknown): v is TodoCategory {
  return typeof v === "string" && (TODO_CATEGORIES as readonly string[]).includes(v);
}

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

/**
 * 状态 → 列表行的**左侧色条**（用户 2026-09-24 #37：不同状态的任务列表要看得出颜色）。
 *
 * 为什么是一条 2px 色条而不是"整行换底色"：底色要么太淡（等于没有）、要么把
 * `--gosslan-hover` 与选中态盖掉，而四种底色同屏会花。色条与状态胶囊同一族色
 * （`primary` / `warning` / `success`；「待办」是"还没开始"⇒ 中性灰，与离线点同一个 token），
 * 扫一眼就能分组，又不跟行内的其他颜色抢。
 * ⚠️ 只准用 token、别写死色值：深色模式下这四个 token 另有取值。
 */
export const TODO_STATUS_BAR: Record<TodoStatus, string> = {
  todo: "bg-[var(--gosslan-status-offline)]",
  doing: "bg-[var(--gosslan-primary)]",
  overdue: "bg-[var(--gosslan-warning)]",
  done: "bg-[var(--gosslan-success)]",
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
  /** 优先级（三档，缺省常规）。 */
  priority: TodoPriority;
  /** 类型（三档，缺省普通任务）。三者同列，界面上用胶囊 tag 区分。 */
  category: TodoCategory;
  /** 长文本描述（2026-09-17 优化）。 */
  description: string;
  /** 描述里附带的图片（仅元数据，真实字节走群文件管线）。 */
  images: TodoImage[];
  /** 是否**显式**归档（完成之后手动归档；用户 2026-09-17 起完成不再自动归档）。 */
  archived: boolean;
  /**
   * 群内固定编号（1 起、组内唯一、创建时定、编辑/归档不改）；0 = 无号
   * （旧版本对端建的任务，载荷里根本没有这个字段 ⇒ 不猜号、不补号）。
   * 显示为「#N」的唯一来源就是这里（`resolveTodoNumbers`），别再在别处按顺序数一遍。
   */
  number: number;
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
      // 未知/缺失一律回落「常规」：宁可给一档可读的默认，也不让这条任务的优先级变成空白
      priority: isTodoPriority(p.priority) ? p.priority : TODO_PRIORITY_DEFAULT,
      // 类型同口径：旧载荷没有这一格 ⇒ 读成「普通任务」，不是"没类型"
      category: isTodoCategory(p.category) ? p.category : TODO_CATEGORY_DEFAULT,
      deleted: p.deleted === true,
      description: typeof p.description === "string" ? p.description : "",
      images: Array.isArray(p.images)
        ? p.images.filter(
            (x): x is TodoImage =>
              !!x && typeof x === "object" && typeof (x as TodoImage).sha256 === "string",
          )
        : [],
      archived: p.archived === true,
      number: typeof p.number === "number" && p.number > 0 ? Math.trunc(p.number) : 0,
      doneAt: typeof p.done_at === "number" ? p.done_at : null,
      createdAt: null,
      seq: rec.seq,
      msgId: rec.msg_id,
    };
  } catch {
    return null;
  }
}

/** 折叠出当前全部任务（墓碑不列），按**最新定义的 seq 从大到小**（＝"最近被改过的在最前"）。
 *
 * ⚠️ 排序键**不是**创建时间：`seq` 来自 LWW 折叠后留下的那份**最新定义**，
 * 所以一条老任务只要被改一次就会排到新创建的任务前面。以前这句注释写的是"按创建版本从新到旧"，
 * 与本文件自己的 `newer()`/实现都不符（#154 第 10 条的另一半）—— 注释错了比没有注释更坏，
 * 因为它会让人以为看板顺序稳定，而实际顺序会随每次编辑跳动。
 * 要"按创建时间排"得改实现并配一条反向断言，那是产品决定，不在本轮收尾范围。
 *
 * 创建时间单独收集：它来自**创建那条记录**（`kind === "todo"`），而 LWW 折叠保留的是
 * 最新定义（往往是 `todo_update`）—— 两条是不同的记录，不能混在一张表里。 */
/**
 * 把"载荷里的号"折成"这一群当前显示的号"。
 *
 * 正常路径是恒等：创建时分配的号已经是组内唯一，这里原样给出。
 * 只有一种情况会动它——**两个成员在彼此离线的窗口里各自建任务，撞了同一个号**
 * （`number` 由各自本地分配，中间没有任何仲裁者，这是 P2P 下无法避免的一次窗口）。
 * 收敛办法是把规则写成"当前这批定义"的**纯函数**：按 (号, todo_id) 定序，
 * 号相同则 `todo_id` 小的保住原号、其余顺延 ⇒ 每个成员拿到同一批定义时必然算出同一套号，
 * 不需要额外同步帧，也不需要谁去改写谁的定义。
 *
 * 无号（0，旧版本对端建的）不参与编号，也不占号。
 *
 * ⚠️ 顺延只发生在撞号那一条上：一条已显示 #5 的任务不会因为"来了新任务"变成 #6。
 */
export function resolveTodoNumbers(defs: { todoId: string; number: number }[]): Map<string, number> {
  const numbered = defs.filter((d) => d.number > 0).sort((a, b) => a.number - b.number || (a.todoId < b.todoId ? -1 : 1));
  const out = new Map<string, number>();
  let prev = 0;
  for (const d of numbered) {
    const n = Math.max(d.number, prev + 1);
    out.set(d.todoId, n);
    prev = n;
  }
  return out;
}

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
  const live = [...defs.values()].filter((d) => !d.deleted);
  const numbers = resolveTodoNumbers(live);
  return live
    .sort((a, b) => {
      if (a.seq !== b.seq) return b.seq - a.seq; // 新的在前
      return a.msgId < b.msgId ? 1 : -1; // 同 seq 按 msg_id 比（与 newer 同规则）
    })
    .map(({ todoId, title, assignees, status, creator, priority, category, description, images, archived, doneAt }) => ({
      todoId,
      number: numbers.get(todoId) ?? 0,
      title,
      assignees,
      status,
      creator,
      priority,
      // ⚠️ 这一串是**白名单式**解构：新字段不写在这里就会在 parse 之后被静默丢掉
      // （能编译、门禁全绿、界面上没有那一格 —— 最难查的一种"接了一半"）。
      category,
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
 * 「与我相关且还活着」的任务 —— 蓝色徽标的**唯一**判据（用户 2026-09-26）。
 *
 * 口径原话是"除了完成和归档的其他数据都统计"，所以排除的只有两态：
 * - `status === "done"`（完成）；
 * - **归档**，且必须走 [`isEffectivelyArchived`] 那一份 —— 它同时含"手动归档"与
 *   "完成满 7 天自动归档"，在别处再写一遍 `item.archived` 就会与列表口径分叉。
 * 其余状态（待办 / 进行中 / 延期）一律计入。
 *
 * "与我相关" = 指派里有我 **或** 我是创建人（我发的任务还挂着，用户也要看得见）。
 * ⚠️ 会话列表与聊天头的任务图标**都必须调这一份**；两处各数一次就是两份真源。
 * `myId` 为空 ⇒ 直接返回空：身份还没就绪时不许点亮一个假数字。
 */
export function openTodosForMe<
  T extends Pick<TodoItem, "status" | "archived" | "doneAt" | "assignees" | "creator">,
>(items: T[], myId: string, now: number = Date.now()): T[] {
  if (!myId) return [];
  return items.filter(
    (t) =>
      t.status !== "done" &&
      !isEffectivelyArchived(t, now) &&
      (t.assignees.includes(myId) || t.creator === myId),
  );
}

/**
 * 我能不能改这条任务（**显示用**的镜像，后端 `commands::may_update_todo` 才是权威）。
 * 归档另有一档（比这里宽），见 [`canArchiveOrReopenTodo`]。
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
 * 我能不能**归档 / 还原**这条任务（用户 2026-09-24：「群里所有人都可以归档」，
 * 同日追加：「归档和被归档的数据还原，任何人都可以操作，其他权限不变」）。
 *
 * 对应后端 `commands::may_change_todo` 的「成员窄档」。后端是**两条**判据
 * （`archive_only_change` 只翻归档位 / `reopen_only_change` 只把状态从完成退回待办），
 * 但放行条件相同（是本群成员即可），所以这里合成一个判据 —— 两处各写一份迟早会漂。
 *
 * ⚠️ 它**只**覆盖归档与还原：改标题 / 描述 / 图片 / 指派人 / 完成状态仍走
 * [`canUpdateTodo`] 那两档。尤其"把一条没干完的任务标成完成"没有放宽 ——
 * 成员能收摊、也能把活重新拎回来，但不能替别人宣布干完了。
 * 归档只在「完成」态成立（后端明确拒绝未完成任务的归档请求），所以调用方还要自己判
 * `status === "done"` 才给按钮。
 */
export function canArchiveOrReopenTodo(
  item: Pick<TodoItem, "creator" | "assignees">,
  actor: string,
  groupCreator: string,
  memberIds: readonly string[],
): boolean {
  if (canUpdateTodo(item, actor, groupCreator, false)) return true;
  return !!actor && memberIds.includes(actor);
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

// ---------------- 看板 / 面板取的那一份源（#154-9） ----------------
//
// 为什么需要它：徽标一直走后端**全表读**（`api.getGroupTodoMessages`），而看板、任务面板、成员面板
// 折的是 `chat.messages[convId]` —— 那份消息缓存有两道上界（`PAGE_SIZE × MAX_PAGES = 1000 条/会话`、
// `MAX_CACHED_CONVS = 8` 个会话整块逐出）。于是「徽标还亮着，看板里却再也列不出这条任务」是可达状态，
// 用户没法归档一个看不见的任务。`refreshConversations` 的注释早就写明"用内存里已有的消息算不行"，
// 那句话当时只应用在徽标那一半 ⇒ 同一句判据两个家。下面这三个函数是那**一个家**的语义，
// store 只做持有，消费者（看板/面板/成员页）只读 `chat.groupTodoRows`。

/** 按会话存的任务相关行（后端全表读的结果）。
 *  **键存在 = 这个群读到过**（空数组 = 读了、确实没有任务）；键不存在 = 还没读过。
 *  这两件事必须分得开，否则"没有任务"与"还没数据"画成同一个样子，看板每次打开都要重读一遍。 */
export type TodoRowsByConv = Record<string, MessageRecord[]>;

/** 整表读的结果落进那份源：`convIds` 是这次读**点到**的全部会话（后端只回有任务的会话）。 */
export function putTodoRows(
  prev: TodoRowsByConv,
  convIds: string[],
  rows: MessageRecord[],
): TodoRowsByConv {
  const byConv: TodoRowsByConv = {};
  for (const r of rows) {
    const list = byConv[r.conv_id];
    if (list) list.push(r);
    else byConv[r.conv_id] = [r];
  }
  const next: TodoRowsByConv = { ...prev };
  for (const cid of convIds) next[cid] = byConv[cid] ?? [];
  return next;
}

/** 读到过没有（区分「没有任务」与「还没读过」）。 */
export function isTodoRowsLoaded(rows: TodoRowsByConv, convId: string): boolean {
  return Object.prototype.hasOwnProperty.call(rows, convId);
}

/** 取某个会话的任务行。 */
export function todoRowsFor(rows: TodoRowsByConv, convId: string): MessageRecord[] {
  return rows[convId] ?? [];
}

/**
 * 乐观更新（自己刚发的、或事件里收到的任务行）落在**同一份**源上。
 *
 * 两条规矩都要：
 *  - 只写**已加载**的会话：给没读过的会话建键 = 拿一份局部快照冒充全量，那正是这次要消灭的形状；
 *  - 同 `msg_id` 是刷新而不是追加：后端对同一条消息存在回填式重发（见 store 里 `applyIncoming` 的注释），
 *    追加会让一条任务被折两遍。
 */
export function mergeIncomingTodoRows(
  rows: TodoRowsByConv,
  incoming: MessageRecord[],
): TodoRowsByConv {
  const todo = incoming.filter((m) => parseTodo(m) !== null);
  if (todo.length === 0) return rows;
  const next: TodoRowsByConv = { ...rows };
  let changed = false;
  for (const m of todo) {
    if (!isTodoRowsLoaded(next, m.conv_id)) continue;
    const existing = next[m.conv_id];
    const i = existing.findIndex((x) => x.msg_id === m.msg_id);
    next[m.conv_id] = i >= 0 ? existing.map((x, j) => (j === i ? m : x)) : [...existing, m];
    changed = true;
  }
  return changed ? next : rows;
}
