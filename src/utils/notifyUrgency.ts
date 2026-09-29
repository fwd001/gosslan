/**
 * 「这一批通知值不值得让 Dock 图标弹跳 / 任务栏闪烁」的判据（用户 2026-09-29 需求汇总五：
 * 「Mac 端收到普通消息不要跳动 Dock 图标；图标跳动仅用于非常紧急或超高优先级消息」）。
 *
 * ## 为什么单独一个文件
 * 这个判断跨两层：Rust 侧要知道"这批紧不紧急"才知道该不该 `request_user_attention`，
 * 而**只有前端看得见消息内容**（后端那条命令只收到 title/body）。所以"紧不紧急"在前端算，
 * 算完只把一个 bool 传下去 —— 而这段算法本身必须能脱离界面被判，否则它就是一条没人验过的规则。
 *
 * ## 紧急只有两个来源（刻意不收第三个）
 * ① **群公告**（`kind === "announcement"`）：它是"必须让全员看到"的那一类，本来就是弹出来用的；
 * ② **紧急群任务**（`todo` / `todo_update` 且载荷 `priority === "high"`）：用户在 4.31.15
 *   要的就是"紧急"这一档，那一档如果和"常规"共用同一种提醒强度，这个档就没有意义。
 * 普通聊天、文件、投票、表情回应一律**不**算紧急 —— 判据写宽了就等于没写（Dock 还是会一直跳）。
 */
import type { MessageRecord } from "../types.ts";
import { parseTodo } from "./todos.ts";

/** 单条消息是否构成"值得打断"的紧急事项。 */
export function isUrgentNotice(rec: MessageRecord): boolean {
  if (rec.kind === "announcement") return true;
  if (rec.kind !== "todo" && rec.kind !== "todo_update") return false;
  // 载荷解不开就按"不紧急"处理：这条判据的失败方向必须是**少打扰**，
  // 而不是"脏数据把 Dock 跳动点亮"（那样用户会以为真来了紧急任务）。
  return parseTodo(rec)?.priority === "high";
}

/**
 * 一批通知里是否有紧急项。入参是"每个会话最后一条"那一份快照 —— 与去抖合并后
 * 真正发出去的那批一一对应（合并队列里同会话只保留最后一条，前面的已被那句"等 N 条"涵盖）。
 */
export function batchIsUrgent(entries: readonly MessageRecord[]): boolean {
  return entries.some(isUrgentNotice);
}
