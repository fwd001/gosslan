/**
 * 卡片类消息（待办 / 投票 / 群公告）的**可复制文字形态** —— 一份实现，多处共用。
 *
 * 为什么单独成模块：消息右键菜单（含移动端长按面板）与收藏页的「复制」必须是**同一份结果**
 * ——两处各写一份的话，同一条卡片在两处会复制出不同文字，甚至一边有入口一边没有
 * （用户 2026-09-21：「群任务也不能复制啊，收藏咋还有复制呢」）。
 *
 * 非卡片类、或载荷解析不出来、待办已被删除（墓碑）时返回 `null`，
 * 调用方据此回退到各自的正文复制 / 给出失败提示。
 *
 * ⚠️ 本模块必须保持**零 `@/` 依赖**（与 `appActions` / `nameGroup` 同一条规则）：
 * `cardText.test.ts` 用 `node --test` 直接 import 它，而 Node 解析不了 Vite 的别名。
 */
import { t } from "../i18n/index.ts";
import { parseTodo } from "./todos.ts";
import type { MessageRecord } from "@/types";

export function cardCopyText(kind: string, content: string): string | null {
  if (kind === "todo") {
    // parseTodo 只认 kind = todo/todo_update，且只读 content/seq/msg_id ⇒ 喂最小记录即可复用
    // 同一套解析与墓碑判定（⚠️ `kind` 不能漏，漏了会直接返回 null）。
    const d = parseTodo({
      kind: "todo",
      content,
      msg_id: "",
      seq: 0,
      ts: 0,
    } as MessageRecord);
    if (!d || d.deleted) return null;
    const title = d.title.trim();
    if (!title) return null;
    const desc = d.description.trim();
    const head = `${t("favorite.cardKind.todo")}：${title}`;
    return desc ? `${head}\n${desc}` : head;
  }

  let payload: Record<string, unknown>;
  try {
    const parsed = JSON.parse(content) as unknown;
    if (!parsed || typeof parsed !== "object") return null;
    payload = parsed as Record<string, unknown>;
  } catch {
    return null;
  }

  if (kind === "poll") {
    const question = typeof payload.question === "string" ? payload.question.trim() : "";
    if (!question) return null;
    const options = Array.isArray(payload.options)
      ? payload.options.filter((x): x is string => typeof x === "string")
      : [];
    return [`${t("favorite.cardKind.poll")}：${question}`, ...options].join("\n");
  }

  if (kind === "announcement") {
    const text = typeof payload.text === "string" ? payload.text.trim() : "";
    return text || null;
  }

  return null;
}
