/**
 * 通道开关的**单一真相源** + 移动端「新的朋友」跳转守卫。
 *
 * ## 为什么需要（用户 2026-09-12 安卓实测）
 * - 「添加好友里把局域网打开，设置里还是关的」—— 同一个概念（局域网开没开）在界面上有**两份**
 *   前端表示：`channels[lan].enabled`（后端真实运行状态）与 `app.online`（另一份快照）。
 *   添加好友页改的是前者、设置页显示的是后者，两边就各说各话。
 * - 「点了『新的朋友』打不开界面」—— 移动端靠 `mobileView` 平移切换面板，
 *   申请页渲染在右侧主面板里；不切过去，用户还停在会话列表上，看起来就是"点了没反应"。
 *
 * 这两类退化**功能看起来都还在**（点第二下、退出去再进设置就能看到正确状态），
 * 所以只能靠静态守卫钉住调用路径。
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import assert from "node:assert/strict";
import { test } from "node:test";

const srcDir = join(import.meta.dirname, "..");
const read = (p: string) => readFileSync(join(srcDir, p), "utf8");

test("通道开关只走「一个快照 + 一个事件」（不再两份状态各自刷新）", () => {
  const store = read("stores/useAppStore.ts");
  const at = store.indexOf("async function setChannelEnabled");
  const body = store.slice(at, at + 700);
  assert.match(body, /await api\.setChannelEnabled\(channel, enabled\)/, "必须真的调后端");
  assert.match(
    body,
    /applyRuntimeSnapshot\(await api\.setChannelEnabled/,
    "必须直接应用后端返回的快照：命令返回值就是切换后的运行状态，再拉一次既有竞态又是多余 IPC",
  );
  assert.ok(
    !/refreshChannels\(\)|refreshNetworkStatus\(\)/.test(store),
    "『通道状态』与『网络状态』两半各自刷新的写法必须已经删除 —— 那正是同一件事两份状态",
  );
  assert.ok(
    !store.includes("getChannelStatus(") && !store.includes("getNetworkStatus("),
    "前端不得再调那两个半份命令（后端也已删除，只剩 getRuntimeSnapshot）",
  );
  assert.match(
    store,
    /function applyRuntimeSnapshot/,
    "通道状态 / 在线 / 绑定 IP 必须只有这一个写入入口",
  );
});

test("设置页的局域网开关必须与「添加好友」页走同一条路径、同一份状态", () => {
  const section = read("components/settings/NetworkSection.vue");
  assert.match(
    section,
    /app\.setChannelEnabled\("lan", target\)/,
    "设置页的局域网开关必须走 store 的 setChannelEnabled（与添加好友页同一条路径）",
  );
  assert.match(
    section,
    /:model-value="!!lanStatus\?\.enabled"/,
    "开关值必须取通道状态（唯一真相源），而不是 app.online 那份快照",
  );
  assert.ok(
    !section.includes(':model-value="app.online"'),
    "不得再用 app.online 当局域网开关值 —— 那正是两处不同步的根因",
  );
});

test("「添加好友」页的通道开关：走 store、用开关给的目标值、不用本地快照取反", () => {
  const modal = read("components/AddFriendModal.vue");
  assert.match(modal, /await app\.setChannelEnabled\(ch\.channel, next\)/, "必须走 store 的通道开关");
  assert.ok(
    !/await api\.setChannelEnabled/.test(modal),
    "不要绕过 store 直接调 api.setChannelEnabled（那样不会刷新共享状态）",
  );
  assert.match(
    modal,
    /@update:model-value="\(v: boolean\) => toggleChannel\(ch, v\)"/,
    "开关必须把**目标值**传下去：用 `!ch.enabled` 取反会在状态过期时反向操作",
  );
});

test("移动端「新的朋友」必须切到主面板（否则点了像没反应）", () => {
  const layout = read("layouts/ResponsiveLayout.vue");
  const open = layout.slice(
    layout.indexOf("function openRequests()"),
    layout.indexOf("function openRequests()") + 500,
  );
  assert.match(
    open,
    /if \(app\.isMobile\) app\.mobileView = "chat"/,
    "移动端打开申请页必须切到主面板（chat），否则用户还停在会话列表上",
  );
  const close = layout.slice(
    layout.indexOf("function closeRequests()"),
    layout.indexOf("function closeRequests()") + 500,
  );
  assert.match(close, /if \(app\.isMobile\) app\.mobileView = "list"/, "关闭时返回会话列表");
});

test("安卓文件选择：选择器的返回值必须先落地成真实路径再发送", () => {
  const chat = read("components/ChatWindow.vue");
  assert.match(
    chat,
    /const local = await api\.importPickedFile\(picked\);/,
    "`content://` URI 直接交给后端发送必然失败（std::fs 打不开 URI）—— 必须经 importPickedFile 落地",
  );
  assert.match(read("api/index.ts"), /importPickedFile:/, "api 层要暴露这个命令");
});
