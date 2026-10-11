/**
 * 强提醒「接收 → 提醒 → 点击定位」链路的形状判据（第 3 阶段）。
 *
 * 算法（折叠/紧急判定）在 reminders.test.ts / notifyUrgency.test.ts 里逐条判；
 * 这里判的是**接线**：受众门控、S3 回执与本地通知解耦、抑制矩阵、移动端通道与
 * 权限、点击载荷把 msg_id 带到定位链。系统级副作用（真通知/振动/Dock）在 node 侧
 * 跑不起来，静态读源码是本仓对这类链路的标准做法（见 channelState.test.ts）。
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import assert from "node:assert/strict";
import { test } from "node:test";

const srcDir = join(import.meta.dirname, "..");
const read = (p: string) => readFileSync(join(srcDir, p), "utf8");

test("onMessage：remind 走专门通道，其它消息才走普通通知", () => {
  const store = read("stores/useChatStore.ts");
  assert.match(
    store,
    /if \(rec\.kind === "remind"\) handleIncomingRemind\(rec\);\s*\n\s*else maybeNotify\(rec\);/,
    "remind 必须走 handleIncomingRemind；remind_ack/普通消息走 maybeNotify（ack 绝不能弹通知）",
  );
});

test("接收门控：群聊先判 actors 名单，再以当前成员表复核（R6 被移出群不提醒）", () => {
  const store = read("stores/useChatStore.ts");
  const at = store.indexOf("function handleIncomingRemind");
  assert.ok(at > 0, "handleIncomingRemind 必须存在");
  const body = store.slice(at, at + 1600);
  assert.match(body, /payload\.actors\.includes\(myId\)/, "不在受众名单里直接返回");
  assert.match(
    body,
    /g\.id === gid && g\.members\.includes\(myId\)/,
    "必须以**当前**群成员表复核（发起后被移出的人不该再被提醒）",
  );
  // 两道门都必须在去重/回执之前：不该回 S3、不该占 remindedIds
  const memberGate = body.indexOf("g.members.includes(myId)");
  const dedup = body.indexOf("remindedIds.add");
  const ack = body.indexOf('"alerted"');
  assert.ok(memberGate > 0 && memberGate < dedup && dedup < ack, "门控 → 去重 → S3 回执 的顺序");
});

test("S3 自动回执与本地通知是否弹出解耦（R1：通知 API 成功不是对方已收到）", () => {
  const store = read("stores/useChatStore.ts");
  const at = store.indexOf("function handleIncomingRemind");
  const body = store.slice(at, at + 2600);
  // 回执在所有本地通知/振动逻辑之前
  const ack = body.indexOf("buildRemindAckPayload(rec.msg_id, \"alerted\")");
  const vibrate = body.indexOf("const vibrate");
  const notify = body.indexOf("notifyDesktop");
  assert.ok(ack > 0 && ack < vibrate && ack < notify, "S3 回执必须先于且独立于本地提醒");
});

test("抑制矩阵：总开关关着 / 前台正查看该会话 → 不振不弹；判定含 document.hidden", () => {
  const store = read("stores/useChatStore.ts");
  const at = store.indexOf("function handleIncomingRemind");
  const body = store.slice(at, at + 2000);
  assert.match(
    body,
    /if \(!app\.notifyEnabled \|\| viewing\) return;/,
    "总开关与查看抑制必须在振动/通知之前短路",
  );
  assert.match(
    body,
    /!document\.hidden && document\.hasFocus\(\) && activeConv\.value === rec\.conv_id/,
    "viewing 必须同时判 hidden（最小化/隐藏时 hasFocus 可能仍为真）",
  );
});

test("移动端：权限门控 + 高优先级通道 + 点击载荷带 msg_id", () => {
  const store = read("stores/useChatStore.ts");
  const at = store.indexOf("function handleIncomingRemind");
  const body = store.slice(at, at + 3600);
  assert.match(body, /app\.ensureNotifyPermission\(\)\.then\(\(granted\)/, "发通知前必须过运行时权限");
  assert.match(
    body,
    /remindChannelReady \? \{ channelId: REMIND_CHANNEL_ID \} : \{\}/,
    "通道就绪才挂 channelId（不存在的通道会让通知整条不发）",
  );
  assert.match(
    body,
    /extra: \{ type: "chat", conv_id: rec\.conv_id, msg_id: payload\.target \}/,
    "移动端通知 extra 必须带原消息 msg_id",
  );
  // 通道在 init 的移动端块里建好，设置变化时重建
  const init = read("stores/useChatStore.ts");
  assert.match(init, /void ensureRemindChannel\(false\);/, "init 必须先建通道");
  assert.match(init, /ensureRemindChannel\(true\)/, "声音/振动设置变化必须 remove+create 重建");
});

test("桌面端：通知命令带 msg_id/声音/按钮文案，且只有 sent=true 才请求注意", () => {
  const store = read("stores/useChatStore.ts");
  const at = store.indexOf("function handleIncomingRemind");
  const body = store.slice(at, at + 3600);
  assert.match(
    body,
    /api\s*\.\s*notifyDesktop\(\s*title,\s*body,\s*rec\.conv_id,\s*\{\s*msgId:\s*payload\.target,\s*sound:\s*app\.remindSound,\s*actionLabel:\s*t\("notification\.view"\),?\s*\}\s*\)/s,
    "桌面通知必须以 opts 带原消息 msg_id、声音开关与本地化按钮文案",
  );
  assert.match(body, /if \(sent\) requestAttentionOnce\(true\);/, "通知没真正发出不许闪 Dock/任务栏");
});

test("点击路由：桌面事件与移动端 actionPerformed 都把 msg_id 送进定位链", () => {
  const store = read("stores/useChatStore.ts");
  assert.match(
    store,
    /routeNotificationClick\(p\.type, p\.conv_id, p\.msg_id\)/,
    "桌面 notification-clicked 事件必须透传 msg_id",
  );
  assert.match(
    store,
    /raw\.extra\?\.msg_id != null \? String\(raw\.extra\.msg_id\)/,
    "移动端点击必须从 extra 取 msg_id",
  );
  assert.match(
    store,
    /const r = await locateMessageInConv\(convId, msgId\);/,
    "打开会话后必须继续翻页定位原消息",
  );
});

test("后端：notify_desktop 以 opts 收 msg_id/action_label，点击载荷带 msg_id；无按钮文案时按语言补", () => {
  const network = read("../src-tauri/src/commands/network.rs");
  assert.match(network, /pub msg_id: Option<String>/, "opts 必须收 msg_id");
  assert.match(network, /pub action_label: Option<String>/, "opts 必须收 action_label");
  assert.match(network, /opts: Option<DesktopNotificationOpts>/, "可选项必须收进 opts 结构");
  assert.match(
    network,
    /crate::db::get_setting\(&dbc, "language"\)/,
    "前端没给按钮文案时按语言设置补（macOS 点击捕获的承重件）",
  );
  const notif = read("../src-tauri/src/notifications.rs");
  assert.match(notif, /pub msg_id: Option<String>/, "点击事件载荷必须带 msg_id");
  // macOS 实现必须同时认 Default（点正文）与 Action("open")（点按钮）
  const macAt = notif.indexOf('#[cfg(target_os = "macos")]\nfn show_click_impl');
  assert.ok(macAt > 0, "macOS 必须有自己的 show_click_impl（此前点击通道整条缺失）");
  const macBody = notif.slice(macAt, macAt + 2000);
  assert.match(macBody, /is_default_action\(\)/, "点正文必须触发");
  assert.match(macBody, /Action\(k\) if k == "open"/, "点 action 按钮也必须触发");
  assert.match(
    notif,
    /NSUserNotificationDefaultSoundName/,
    "macOS 默认声音必须用完整常量（传 default 会被当成不存在的声音文件）",
  );
});

test("MessageItem：发起入口要求消息已成功发出；接收标识与确认按钮齐备", () => {
  const item = read("components/MessageItem.vue");
  assert.match(
    item,
    /REMINDABLE_STATUS\s*=\s*new Set\(\["sent", "delivered", "read"\]\)/,
    "sending/failed/cancelled 的消息不能发起强提醒（会指向对方没收到的幽灵消息）",
  );
  assert.match(item, /:can-remind="canRemindEntry"/, "右键菜单入口必须接上");
  assert.match(item, /@remind="doRemind"/, "菜单事件必须接上 doRemind");
  assert.match(item, /t\("remind\.gotIt"\)/, "必须有「我知道了」按钮");
  assert.match(item, /doConfirmReminder/, "确认按钮必须接 store 的确认动作");
});

test("ChatWindow：会话层折叠一次再分发（与 reactionMap 同构，不做 O(n²)）", () => {
  const win = read("components/ChatWindow.vue");
  assert.match(win, /foldReminders\(chat\.messages\[convId\] \?\? \[\]\)/);
  assert.match(win, /:reminder="reminderMap\.get\(item\.msg_id\) \?\? null"/);
});

test("群受众弹窗：排除自己、至少 1 人才能确认", () => {
  const dlg = read("components/RemindAudienceDialog.vue");
  assert.match(dlg, /filter\(\(id\) => id !== myId\.value\)/, "候选必须排除自己");
  assert.match(dlg, /:disabled="selected\.size === 0"/, "零选择不能确认（第一版不默认全群）");
});
