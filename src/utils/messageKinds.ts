/**
 * 消息 kind 语义分类 —— **前端侧的唯一判定点**，与 Rust 的 `protocol::WIRE_KINDS`
 * 一一对应（`messageKinds.test.ts` 会读 `protocol.rs` 源码逐项比对，防止两边漂移）。
 *
 * 为什么前端也要有一份：消息落库、未读累计、通知、预览这些判定横跨 Rust/TS 两侧
 * （Rust 决定落库与 IPC，TS 决定渲染与通知）。这是 FFI 边界导致的**必要**重复，
 * 不是「第二套实现」——契约测试就是它的保险。
 */

export type KindClass = "bubble" | "silent" | "card";

/**
 * kind → 分类。
 *
 * **未知 kind 一律按 `bubble`**：与 Rust 侧回退到 `Bubble` 同语义 ——
 * 宁可多显示一条，也不要把不认识的内容静默吞掉（对端版本更新时不丢消息）。
 */
export function kindClass(kind: string): KindClass {
  if (SILENT_KINDS.includes(kind)) return "silent";
  if (CARD_KINDS.includes(kind)) return "card";
  return "bubble";
}

/** 静默事件：不进时间线、不计未读、不改预览、不弹通知。 */
export function isSilentKind(kind: string): boolean {
  return kindClass(kind) === "silent";
}

/**
 * 静默种类清单 —— **与 Rust 的 `WIRE_KINDS` 必须一致**，由契约测试锁死。
 * 这里显式列出（而不是从 `kindClass` 反推）是为了让测试能逐项比对。
 */
export const SILENT_KINDS: readonly string[] = [
  "reaction",
  "recall",
  "pin",
  "announcement_delete",
  // 任务的**改动**（改状态/改标题/删除）是状态微调：不记未读、不弹通知。
  // 任务的**创建**（`todo`）反过来是 Card —— 被指派的人得知道自己被派了活。
  "todo_update",
  "poll_vote",
];

/**
 * 群级沉淀物：**进时间线**（该计未读、该通知），但**不属于"聊天历史"** ——
 * 清空聊天记录不得删、清空边界不得拦。这两点是它与 bubble 的全部差别。
 */
export const CARD_KINDS: readonly string[] = ["announcement", "todo", "poll"];

/**
 * 提示行（微信式居中灰字）：**进时间线**，但**不是一条消息** —— 没有头像、没有气泡，
 * 不可右键/长按/复制/回应。这是它与 bubble 的全部差别。
 *
 * 为什么单独一个判定点：这条性质横跨三处渲染逻辑（`MessageItem` 的模板分支、
 * `messageHeight` 的高度估算、以及"要不要有昵称行"），三处各写一份 `kind === "system"`
 * 就一定会漂移 —— 高度估多估少会让虚拟列表的行互相遮挡。
 */
export const TIP_KINDS: readonly string[] = ["system", "recalled"];

export function isTipKind(kind: string): boolean {
  return TIP_KINDS.includes(kind);
}

/**
 * 是否可进「多选」（批量转发 / 收藏 / 删除）。
 *
 * 待办（`todo`）是**群级沉淀物**（卡种），不是普通聊天消息：它本就不可单独转发/收藏
 * （见各处的 `forwardable`），批量删除走的也是 `deleteMessages`，而待办的正确删除入口是
 * `updateTodo(deleted:true)` —— 所以待办卡片不进消息级批量操作（菜单里不给「多选」入口，
 * 多选模式下也不出勾选框）。
 */
export function isMultiSelectable(kind: string): boolean {
  return kind !== "todo";
}
