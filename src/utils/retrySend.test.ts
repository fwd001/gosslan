/**
 * 「重发」必须**原地复用**那条 failed 气泡，不能新建第二条。
 *
 * ## 缺陷现场（2026-10-03 审计）
 * `MessageItem.vue::retrySend` 走 `chat.send(...)`，而 `send()` 每次都新建一条
 * `msg_id = tmp-${Date.now()}-...` 的乐观记录并 `enqueueMessage`。于是：
 *
 * ```
 * 列表里：① 发送失败的原文（status=failed，msg_id=tmp-A）
 *          ② 重发中的原文（status=sending，msg_id=tmp-B）
 * ```
 *
 * 而 ① **永远不会被删**：
 * · 全库没有任何删除类函数（`messages.ts` 导出的 19 个里没有 remove/delete/purge）；
 * · `appendLocalOnly`（`utils/messages.ts`）把 `tmp-*` 视为"只存在于内存"，
 *   每次 `loadMessages` 都会把它**重新追加**到快照尾部。
 *
 * ⇒ 用户点一次「重发」看到两条一样的文字，且失败那条永不消失；切会话重开也一样。
 *
 * ## 为什么不复用同一 msg_id（INV-001 字面做不到）
 * 后端 msg_id = `SHA-256(sender_id + nonce + payload)`（`protocol.rs::compute_message_id`），
 * nonce 每条新消息都不同 ⇒ 重发**必然**是新 msg_id；且 `send_message` 的签名只有
 * `(friend_id, content, kind)`，**没有 msg_id 参数**可传。所以 INV-001 的
 * "重试复用同一 msg_id"在本项目落在**接收侧幂等**（`message_exists` 按 msg_id 去重），
 * 前端这一侧只能做成"旧气泡原地转 sending 再替换"。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dirname, "..");
const read = (p: string) => readFileSync(join(ROOT, p), "utf8");

test("★ 重发必须把旧 failed 气泡原地转回 sending，而不是新建一条", () => {
  const store = read("stores/useChatStore.ts");
  // send() 接受 retryOfMsgId
  assert.match(
    store,
    /async function send\([\s\S]*?retryOfMsgId\?: string,/,
    "send() 必须接受 retryOfMsgId —— 没有它就区分不出「首次发送」与「重发」。",
  );
  // 有那条"原地转回"的分支
  assert.match(
    store,
    /if \(retryOfMsgId\)[\s\S]*?findIndex\(\(m\) => m\.msg_id === retryOfMsgId\)/,
    "重发分支必须按 msg_id 找到那条 failed 气泡（findIndex），而不是新建。",
  );
  assert.match(
    store,
    /status: "sending" as const/,
    "重发必须把旧气泡的状态改回 sending（用户看到的是同一条消息在重试）。",
  );
  // 重发成功后替换的是**旧那条**，不是新的
  assert.match(
    store,
    /replaceMessage\(\s*convId,\s*retryOfMsgId,/,
    "成功后必须 replaceMessage(convId, retryOfMsgId, …) —— 替换旧气泡，" +
      "若换成 optimistic.msg_id 就等于什么都没做（旧 failed 仍在、新记录另开一条）。",
  );
  // 重发失败也要退回 failed，且退回的是同一条
  assert.match(
    store,
    /catch \(e\) \{\s*replaceMessage\(convId, retryOfMsgId, \{ \.\.\.reverted, status: "failed" \}\);/,
    "重发失败必须把**同一条**气泡退回 failed（reverted 是那份 sending 副本）。",
  );
});

test("★ retrySend 必须把旧 msg_id 传下去（否则上面的分支永远进不去）", () => {
  const item = read("components/MessageItem.vue");
  assert.match(
    item,
    /chat\.send\(msg\.conv_id, msg\.content, msg\.kind, undefined, msg\.msg_id\)/,
    "retrySend 必须传第 5 个参数 msg.msg_id —— 少了它 store 无法定位要复用的那条气泡，" +
      "会退回新建路径 ⇒ 界面上两条一样的文字。",
  );
});

test("★ 重发路径不许 enqueueMessage（那正是「新建第二条」的来源）", () => {
  const store = read("stores/useChatStore.ts");
  // enqueueMessage 只能在新建分支里出现一次，且必须落在 retryOfMsgId 分支之外
  const enqueueAt = store.indexOf("enqueueMessage(optimistic)");
  const retryBranchAt = store.indexOf("if (retryOfMsgId)");
  assert.ok(retryBranchAt > 0 && enqueueAt > retryBranchAt, "enqueueMessage 应在重发分支之后");
  const branch = store.slice(retryBranchAt, enqueueAt);
  assert.ok(
    !branch.includes("enqueueMessage"),
    "重发分支里绝不能出现 enqueueMessage —— 那会新建一条乐观记录，" +
      "旧 failed 又不会被删 ⇒ 两条一样的文字。",
  );
});

test("单聊/群聊的 invoke 分流只有一份（重发与首次发送共用）", () => {
  const store = read("stores/useChatStore.ts");
  // 只看 sendToBackend 那个函数体内部：分流必须恰好一次
  const at = store.indexOf("async function sendToBackend(");
  assert.ok(at > 0, "必须有 sendToBackend 这个共用实现");
  const body = store.slice(at, store.indexOf("\n  }", at));
  const dm = [...body.matchAll(/api\.sendMessage\(/g)].length;
  const grp = [...body.matchAll(/api\.sendGroupMessage\(/g)].length;
  assert.equal(dm, 1, `sendToBackend 里 api.sendMessage 出现 ${dm} 次，应恰好 1 次`);
  assert.equal(grp, 1, `sendToBackend 里 api.sendGroupMessage 出现 ${grp} 次，应恰好 1 次`);
  // 且 send() 的两条路径（新建 / 重发）都调它，不自己分流
  const calls = [...store.matchAll(/await sendToBackend\(/g)].length;
  assert.equal(
    calls,
    2,
    `send() 里有 ${calls} 处 await sendToBackend，应恰好 2 处（新建 + 重发）。` +
      `若某条路径自己写了 if (convId.startsWith("group:")) 分流，就是第二个家。`,
  );
});

/**
 * 钉住后端"msg_id 不可由客户端指定"这个前提。
 *
 * `send()` 里那句"⚠️ 为什么不能字面复用同一 msg_id"依赖它：若哪天后端给
 * `send_message` 加了可选 `msg_id` 参数，前端就可以走"真复用"这条路，
 * 本文件关于"必然是新 msg_id"的推理与注释都要重写。
 */
test("后端 send_message 不接受客户端指定 msg_id（前端「复用」只能靠旧气泡原地转）", async () => {
  const rust = readFileSync(join(ROOT, "..", "src-tauri/src/commands/chat.rs"), "utf8");
  const fn = rust.slice(rust.indexOf("pub async fn send_message("));
  const sig = fn.slice(0, fn.indexOf(") -> Result<MessageRecord, String>"));
  assert.ok(sig.length > 0, "没找到 send_message 的签名");
  assert.ok(
    !/\bmsg_id\b/.test(sig),
    "send_message 的签名里出现了 msg_id —— 若后端已支持客户端指定 msg_id，" +
      "前端应改成真复用（同 msg_id 重试），本文件的推理与 store 里的注释都要重写。",
  );
  // 且 msg_id 由 nonce + payload 派生（重发必得新 id）
  const proto = readFileSync(
    join(ROOT, "..", "src-tauri/src/protocol.rs"),
    "utf8",
  );
  assert.match(
    proto,
    /pub fn compute_message_id\(&mut self\)/,
    "msg_id 由 compute_message_id 计算（SHA-256 of sender_id + nonce + payload）—— " +
      "nonce 每条新消息不同 ⇒ 重发必得新 msg_id。",
  );
});
