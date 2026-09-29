/**
 * 1:1 表情回应的通路判据（用户 2026-09-29 需求汇总第三条：「群聊 + 1:1 都要」）。
 *
 * 为什么这一组判据住在前端测试里、却要读 Rust 源码：这条通路跨两种语言，
 * 而它坏起来的形状是**静默**的 —— 前端把按钮点亮了、消息进了 outbox，
 * 后端却因为"白名单里没有 reaction / 载荷键名对不上 / 老对端吃不下这个 kind"而拒掉或丢掉。
 * 这些接缝没有一层能在本机跑出来（要两个不同版本的实例），只能静态钉住。
 * 同形状的先例见 `channelState.test.ts`（钉提请注意的调用位置）与 `messageKinds.test.ts`
 * （直接读 Rust 常量比对文案）。
 */
import test from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { buildReactionPayload } from "./reactions.ts";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..");

function read(rel: string): string {
  return readFileSync(join(repoRoot, rel), "utf8");
}

const CHAT_RS = "src-tauri/src/commands/chat.rs";
const PROTO_RS = "src-tauri/src/protocol.rs";
const STORE_TS = "src/stores/useChatStore.ts";
const CW_VUE = "src/components/ChatWindow.vue";
const MI_VUE = "src/components/MessageItem.vue";

/** 取出某个函数/方法体的文本：从签名到它自己那行缩进的收尾 `}`（够用即可，不解析语法）。 */
function body(text: string, signature: string, closer: string): string {
  const at = text.indexOf(signature);
  assert.ok(at >= 0, `找不到签名：${signature}`);
  const end = text.indexOf(closer, at);
  assert.ok(end > at, `函数体没收尾：${signature}`);
  return text.slice(at, end);
}

test("后端 1:1 白名单收 reaction，且畸形载荷当场拒（不是发出去再折不出来）", () => {
  const c = read(CHAT_RS);
  const arm = body(c, '"reaction" => {', "    };");
  assert.ok(
    c.includes('"reaction" => {') && c.includes("MsgKind::Reaction"),
    "chat.rs 的类型白名单必须放行 reaction —— 缺这一条时前端按钮再亮也只是「点了没反应」",
  );
  assert.match(
    arm,
    /parse_reaction_payload\(&content\)\?/,
    "reaction 那一段必须先校验载荷（和 merge 一样），否则空 target 会变成一条永远折不出来的事件",
  );
  // 反面：不许把校验写成"只判 kind 存在"
  assert.ok(
    !/"reaction" =>\s*MsgKind::Reaction/.test(c),
    "不许出现不校验载荷就直接收 reaction 的写法（那是把校验挪到了 nonexistent 的下一层）",
  );
});

test("1:1 的门控走 dm_* 那一对判据，群侧仍走 kind_audience（两处问的不是同一件事）", () => {
  const c = read(CHAT_RS);
  assert.match(c, /dm_required_features\(&kind\)/, "chat.rs 必须问 1:1 那张表");
  assert.match(c, /dm_allowed_by_features\(&kind/, "chat.rs 的放行判定也必须走 1:1 那一对");
  // 正面：群内核仍然用受众三态（只报不拦），不许被顺手换成硬门控
  const w = read("src-tauri/src/commands/window.rs");
  assert.match(w, /kind_audience\(/, "群侧仍要走受众统计（不拦发送，只提示）");
  assert.ok(
    !w.includes("dm_allowed_by_features"),
    "群侧不许改用 1:1 的硬门控：群 kind 按自由字符串解析，老成员只是渲染退化，硬拦会当场少发一批消息",
  );
});

test("静默 kind 不许刷 1:1 会话预览 —— 守卫必须落在 touch_conversation 之前", () => {
  const c = read(CHAT_RS);
  const guard = c.indexOf("is_non_notifying_kind(&kind)");
  const touch = c.indexOf("db::touch_conversation(");
  const ensure = c.indexOf("db::ensure_conversation(");
  assert.ok(
    guard > 0 && touch > 0 && ensure > 0,
    "三处都应能找到（守卫、正常分支、静默分支）—— 少一处就是这条判据空转",
  );
  assert.ok(guard < touch, "分叉必须写在 touch_conversation **之前**，写在后面等于没挡");
  assert.ok(guard < ensure && ensure < touch, "静默分支要 ensure_conversation（会话行仍得存在）");
  // 群内核那条例子的出处（同一句话不许有两个家）
  assert.match(
    read("src-tauri/src/commands/window.rs"),
    /is_non_notifying_kind\(kind\)/,
    "群侧同源判据必须还在：两处各自演化就是这条判据最典型的坏法",
  );
});

test("回应载荷的键名与 Rust struct 逐字一致（跨语言，只有一个家）", () => {
  const proto = read(PROTO_RS);
  const structBody = body(proto, "pub struct ReactionPayload {", "\n}");
  const rustFields = [...structBody.matchAll(/pub\s+(\w+)\s*:/g)].map((m) => m[1]).sort();
  assert.deepEqual(rustFields, ["add", "emoji", "target"], "先确认 Rust 侧那三个字段还是这三个");

  const parsed = JSON.parse(buildReactionPayload("m1", "[赞]", true)) as Record<string, unknown>;
  assert.deepEqual(Object.keys(parsed).sort(), rustFields, "前端拼的 JSON 键名必须等于后端 struct 的字段名");
  assert.equal(parsed.target, "m1");
  assert.equal(parsed.emoji, "[赞]");
  assert.equal(parsed.add, true, "add 是布尔（取消 = false），不许写成字符串");
});

test("前端通路：store 有 1:1 那条发送，且 ChatWindow 不再对非群会话直接 return", () => {
  const s = read(STORE_TS);
  const fn = body(s, "async function sendDmReaction(", "  }\n");
  assert.match(fn, /api\.sendMessage\(/, "1:1 回应必须复用 send_message（不要再开一条命令，那是第二个家）");
  assert.match(fn, /"reaction"/, "kind 必须是 reaction");
  assert.match(fn, /buildReactionPayload\(/, "载荷形状要取自唯一那个家");
  assert.match(fn, /enqueueMessage\(rec\)/, "落库后的记录要回灌时间线，chip 才会立刻变");
  assert.match(s, /sendDmReaction,\n/, "必须 export，否则界面拿不到");

  const cw = read(CW_VUE);
  const toggle = body(cw, "function toggleReaction(", "\n}\n");
  assert.ok(
    !toggle.includes('if (!convId?.startsWith("group:")) return;'),
    "那条对非群会话的早退必须去掉 —— 它就是「1:1 点表情没反应」的直接原因",
  );
  assert.match(toggle, /chat\.sendReaction\(/, "群那条通路不能被我顺手改掉");
  assert.match(toggle, /chat\.sendDmReaction\(/, "非群要走 1:1 那条");
});

test("入口口径与「这是不是群」解耦，且自聊明确不给入口", () => {
  const cw = read(CW_VUE);
  assert.match(cw, /:can-react="canReact"/, "ChatWindow 必须把口径传给消息项，而不是让组件自己猜");
  assert.match(
    cw,
    /const canReact = computed\(\(\) => isGroup\.value \|\| \(isPeerFriend\.value && !isSelfChat\.value\)\)/,
    "口径 = 群 或 好友单聊，且**排除自聊**：后端自聊通路只认 text/code，让按钮亮着是往错方向支使用户",
  );

  const mi = read(MI_VUE);
  assert.match(mi, /canReact && !selectMode/, "笑脸入口问 canReact");
  assert.match(mi, /:interactive="canReact"/, "回应条能不能点也问同一个口径（问两次就会漂成两个答案）");
  assert.ok(
    !mi.includes('v-if="isGroup && !selectMode"'),
    "入口不许再钉在 isGroup 上：那是把产品口径写成分支类型",
  );
  // 正面：isGroup 本身还在别处使用（阅读者/发送者名），不许被一起删掉
  assert.match(mi, /isGroup/, "isGroup 这个 prop 仍应存在，只是不再管回应入口");
});
