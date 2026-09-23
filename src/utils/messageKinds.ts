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
 * 时间线内容类 kind —— **与 Rust `WIRE_KINDS` 里 class 为 `Bubble` 的那些一致**（契约测试比对）。
 *
 * 为什么单独列一份而不是从 `kindClass` 反推：`kindClass` 对**未知** kind 也返回 `bubble`
 * （故意的，宁可多显示一条也不要静默吞掉对端的新内容），反推会得到一张"什么都在里面"的假表，
 * 于是"本机认不认识"这个判据就没了。
 */
export const BUBBLE_KINDS: readonly string[] = [
  "text",
  "code",
  "image",
  "file",
  "system",
  "recalled",
  "merge",
];

/**
 * 这个 kind 本机**认识**吗（INV-P24 第 2 条的判据来源）。
 *
 * 与 `kindClass` 必须分开用：分类回答"怎么对待它"，本判据回答"会不会显示成看不懂的东西"。
 * 渲染层据此决定走专门卡片、纯文本，还是「不支持的消息类型」占位 —— 缺了这一步，
 * 对端版本比本机新时用户看到的就是载荷原文（一串 JSON）。
 */
export function isKnownKind(kind: string): boolean {
  return (
    SILENT_KINDS.includes(kind) ||
    CARD_KINDS.includes(kind) ||
    BUBBLE_KINDS.includes(kind)
  );
}

/**
 * 这个 kind 会出现在**聊天时间线**里吗（`ChatWindow.vue` 的 `messages` 过滤判据）。
 *
 * 必须与 ChatWindow 共用一份而不是各写一遍：未读分割线的锚点是在 store 里算的
 * （store 才知道 `conversation.unread`），却要拿去**过滤后的列表**里当下标用。
 * 两边判据一旦漂移（比如这里加了 `announcement`、那边没加），表现就是分割线画错消息，
 * 而且**方向随消息种类变化**——静默行占下标不占未读、card 行占未读不占下标。
 */
export function isRenderedInTimeline(kind: string): boolean {
  return kindClass(kind) === "bubble" || kind === "todo";
}

/**
 * 这个 kind **计入未读/触发通知**吗 —— 与 Rust `is_non_notifying_kind`
 * （`src-tauri/src/protocol.rs:377-379`，= `is_silent_kind(kind) || kind == "system"`）同口径。
 *
 * 为什么不能直接用 `!isSilentKind(kind)`：`system` 归 **bubble**（要显示在时间线里，
 * 例如「对方改了昵称」「下载了你的文件」），但后端明确**不给它记未读**。两者相差的正好是
 * "渲染集合 \ 计未读集合"，所以凡是要把 `conversation.unread` 换算成时间线里某个位置的
 * 判据（未读分割线的锚点就是这一个），都必须用本函数而不是 `isSilentKind` 的反面。
 *
 * 的两个消费点（判据只留这一份）：未读记账与会话预览
 * （`utils/messages.ts::applyIncomingToConversations`）、是否弹系统通知
 * （`useChatStore` 的通知闸门）。2026-09-23 审计 4.2 复核时发现这两处都只滤 `isSilentKind`
 * ⇒ 后端不记账/不打扰的系统消息在前端会 +1 并弹通知；现已统一收敛到本函数。
 */
export function countsTowardUnread(kind: string): boolean {
  return !isSilentKind(kind) && kind !== "system";
}

/**
 * 未知 kind 的占位文案。**必须与 Rust 的 `UNSUPPORTED_PREVIEW_LABEL` 一字不差** ——
 * 会话列表（Rust 算）与气泡（前端算）说的必须是同一句话，`messageKinds.test.ts` 直接
 * 读 `protocol.rs` 比对这个字面量。
 */
export const UNSUPPORTED_KIND_LABEL = "[不支持的消息]";

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

/**
 * 能否「转发」——**唯一一份判据**（消息右键菜单、长按面板、收藏详情操作条都从这里取）。
 *
 * 为什么必须收成一份（AI_RULES §32）：这三处的动作集合要**一致**，此前各写一份的结果是
 * 「菜单里没有转发、收藏页却有」——用户 2026-09-21 报的正是这个（「收藏底下的操作和消息的
 * 右键也不一样」），而且点下去必然失败：两条发送命令的 kind 白名单只收
 * text/code(/file)/merge，卡片类会被直接拒。
 *
 * 合并转发卡片（merge）本身可以再转（微信允许"转发聊天记录"），它是自包含内容。
 */
export function isForwardableKind(kind: string): boolean {
  return kind === "text" || kind === "code" || kind === "image" || kind === "file" || kind === "merge";
}

/**
 * 能否「收藏」= 转发那几类 + **卡片类**（待办 / 投票 / 群公告）。
 *
 * 收藏走 `addFavorite(msgId)`、由**后端复制内容**，不重发、无副作用 ⇒ 卡片收藏是安全的
 * （收藏夹已按卡片补了渲染分支：列表行 `cardText`、详情分支）。
 * ⚠️ 转发**不含**卡片：那是个有副作用的动作（要做"把任务转给另一个群"得走专用发送路径）。
 */
export function isFavoritableKind(kind: string): boolean {
  return isForwardableKind(kind) || kindClass(kind) === "card";
}
