/**
 * 「提醒强度分端」这条链的形状判据（用户 2026-09-29 需求汇总五）。
 *
 * 判的是**接线**，不是算法（算法在 `notifyUrgency.test.ts` 里逐条判）：
 * 前端算出的那个 bool 要真的走到 IPC、真的被后端两条命令签名接住、真的只在 macOS
 * 那一支改变行为 —— 少一环就是"看着改了，实际什么都没变"。
 *
 * 为什么用静态读文件而不是跑起来：`request_user_attention` 是**系统级视觉副作用**，
 * node 侧没有可判的对象；而"参数漏接"在 Tauri 里是运行时 `invalid args`（用户点了没反应），
 * 编译期反而不报 —— 正是静态判据最划算的那一类。
 */
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";
import { test } from "node:test";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");
const store = read("../stores/useChatStore.ts");
const api = read("../api/index.ts");
const networkRs = read("../../src-tauri/src/commands/network.rs");
const trayRs = read("../../src-tauri/src/tray.rs");

test("紧急程度按『这批真正发出的通知』算，并原样传给后端", () => {
  // 入参必须是 `entries`（去抖合并后的那批），不是未读列表 / 不是整个会话历史：
  // 用后者会让一条很旧的紧急任务把每次新消息都点亮 Dock 弹跳。
  assert.match(
    store,
    /const urgent = batchIsUrgent\(entries\.map\(\(e\) => e\.last\)\);/,
    "urgent 必须由这批通知的最后一条算出",
  );
  assert.doesNotMatch(
    store,
    /batchIsUrgent\((messages|allMessages|conversations)/,
    "不许把整条消息列表喂给 batchIsUrgent（那样紧急标记永远清不掉）",
  );
});

test("IPC 两端参数名对齐：前端传的 key 就是后端形参名", () => {
  // Tauri 按**参数名**做序列化匹配：前端写 `{ urgency }` 而后端形参叫 `urgent`
  // 不会编译失败，只会在运行时 `invalid args` ⇒ 必须两头都钉住。
  assert.match(api, /requestAttention: \(urgent: boolean\) => invoke<void>\("request_attention", \{ urgent \}\)/);
  assert.ok(
    networkRs.includes("pub fn request_attention(app: tauri::AppHandle, urgent: bool)"),
    "桌面端命令必须收 urgent: bool",
  );
  assert.ok(
    networkRs.includes("pub fn request_attention(_app: tauri::AppHandle, _urgent: bool)"),
    "移动端命令也必须收这个参数（Android 少收一个参数 = 每次调用 invalid args）",
  );
});

test("分端规则只在后端一处：macOS 看 urgent，Windows / 其它维持闪烁", () => {
  // macOS 那一支必须真的把 urgent 用上；写成 `Some(Critical)` 就等于这条需求没做。
  assert.match(networkRs, /AttentionPlatform::MacOS => urgent\.then_some/);
  assert.match(networkRs, /AttentionPlatform::Windows \| AttentionPlatform::Other => \{/);
  // 只有一处决定"要不要打断"：前端不许自己判平台（isMac 分支会漂成第二份规则）。
  assert.equal(
    (store.match(/api\.requestAttention\(/g) ?? []).length,
    1,
    "调用点只许一处（两处就会有两种分端判断）",
  );
  assert.equal(
    (networkRs.match(/fn request_attention\(/g) ?? []).length,
    2,
    "桌面 + 移动各一条；第三份就是第二个家",
  );
});

test("macOS 菜单栏数字由未读总数驱动，0 条时清空", () => {
  assert.ok(
    trayRs.includes("tray.set_title(tray_title(unread).as_deref())"),
    "set_unread_badge 里必须真的改标题（只写 tooltip = 菜单栏上没有数字）",
  );
  assert.ok(
    trayRs.includes("tray.set_tooltip(Some(tray_tooltip(zh, unread)))"),
    "tooltip 那条不能因为新增数字而被删掉（Windows 侧靠它报条数）",
  );
  // 纯函数那一半：0 → None（清空），不是 Some("0")
  assert.match(trayRs, /0 => None,/);
});
