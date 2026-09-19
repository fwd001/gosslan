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
import { readCommandsSrc } from "../../scripts/rustSrc.ts";

const srcDir = join(import.meta.dirname, "..");
const read = (p: string) => readFileSync(join(srcDir, p), "utf8");

test("通道开关只走「一个快照 + 一个事件」（不再两份状态各自刷新）", () => {
  const store = read("stores/useAppStore.ts");
  const at = store.indexOf("async function setChannelEnabled");
  const body = store.slice(at, at + 2400);
  assert.match(body, /await api\.setChannelEnabled\(channel, enabled\)/, "必须真的调后端");
  assert.match(
    body,
    /applyRuntimeSnapshot\(await api\.setChannelEnabled\(channel, enabled\)\)/,
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

/**
 * 通道开关必须**乐观更新**（用户 2026-09-13：「所有的这种操作都是以乐观更新优先响应用户需求，
 * 然后再去底层执行。如果执行失败的话，上层 UI 可以去 loading，然后提示，最后回退数据」）。
 *
 * 为什么这条必须在开关上钉死：蓝牙启停**不是毫秒级**的 ——
 * `ble::start` 要等 CoreBluetooth 回报状态（`peripheral::STATE_WAIT = 3s`）、
 * `ble::stop` 要等扫描任务退出（`STOP_TIMEOUT = 2s`）。所以"等 `await` 回来才动开关"
 * 就等于让用户干等 2~3 秒（用户实测："点了一下，过了好一会儿才会关/才会开"）。
 *
 * 顺序必须是：**先按用户意图改状态 → 再 await 后端 → 成功用权威快照收尾 / 失败回退并抛错**，
 * 期间挂 pending（UI 可以 loading）。下面按源码顺序断言，写反了就会失败。
 */
test("通道开关必须乐观更新：先按用户意图切、再执行、失败回退", () => {
  const store = read("stores/useAppStore.ts");
  for (const [name, signature] of [
    ["setChannelEnabled", "async function setChannelEnabled"],
    ["startNetwork", "async function startNetwork"],
    ["stopNetwork", "async function stopNetwork"],
  ] as const) {
    const at = store.indexOf(signature);
    assert.ok(at > 0, `找不到 ${signature}（护栏需要同步更新）`);
    const body = store.slice(at, at + 2200);
    const awaitAt = body.indexOf("await api.");
    assert.ok(awaitAt > 0, `${name} 必须真的调后端`);
    // ① 乐观：await 之前就已经按用户意图改了状态，并挂上 pending
    const optimistic = body.indexOf("markChannelPending(");
    assert.ok(
      optimistic > 0 && optimistic < awaitAt,
      `${name} 必须在 await 之前先乐观更新（否则开关要等后端 2~3s 才动）`,
    );
    assert.match(
      body.slice(0, awaitAt),
      /channels\.value = /,
      `${name} 的乐观更新必须真的改通道状态（开关取值来自 channels）`,
    );
    // ② 失败回退：catch 里把状态还原，并把错误抛给调用方 toast
    assert.match(body, /catch[\s\S]*?throw e/, `${name} 失败必须回退并抛出（由调用方 toast）`);
    // ③ pending 必须清掉（成功失败都要），否则转圈会一直转
    assert.match(body, /finally[\s\S]*?markChannelPending\([^)]*false\)/, `${name} 必须清掉 pending`);
  }
  // 取值来自 pending 的 UI 提示（不在 store 里把开关值改成 pending 态）
  const store2 = read("stores/useAppStore.ts");
  assert.match(
    store2,
    /const isChannelPending = /,
    "pending 必须由 store 暴露出去（UI 只拿它做 loading，不参与开关取值）",
  );
  const section = read("components/settings/NetworkSection.vue");
  assert.match(
    section,
    /:pending="app\.isChannelPending\('bluetooth'\)"/,
    "设置页的蓝牙开关必须接上 pending（乐观更新期间显示 loading）",
  );
  assert.match(
    section,
    /:pending="app\.isChannelPending\('lan'\)"/,
    "设置页的局域网开关同样要 loading",
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
    /await api\.importPickedFile\(pickedList\[i\]\)/,
    "`content://` URI 直接交给后端发送必然失败（std::fs 打不开 URI）—— 必须经 importPickedFile 落地",
  );
  assert.match(read("api/index.ts"), /importPickedFile:/, "api 层要暴露这个命令");
});

test("已读回执只在「聊天视图真的可见」时才发（用户 2026-09-12 实测）", () => {
  const store = read("stores/useChatStore.ts");
  // 判据必须同时看：是这个会话 + 应用在前台 + **聊天视图可见**
  assert.match(
    store,
    /activeConv\.value !== convId \|\| document\.hidden \|\| !app\.chatVisible/,
    "去抖标记已读必须同时判「聊天视图可见」——否则用户在设置页/新的朋友页时收到消息也会被标已读",
  );
  assert.match(
    store,
    /if \(activeConv\.value && app\.chatVisible\)/,
    "回到前台补发已读回执同样要判可见性",
  );
  const app = read("stores/useAppStore.ts");
  assert.match(app, /const chatVisible = computed\(/, "可见性判据必须在 store 里唯一实现");
  assert.match(app, /mobileChatObscured/, "必须有『移动端被整页浮层盖住』这个状态");
  const layout = read("layouts/ResponsiveLayout.vue");
  assert.match(
    layout,
    /app\.setMobileChatObscured\(/,
    "只有布局层知道设置/日志/新的朋友/资料页开没开 ⇒ 必须由它同步给 store",
  );
});

test("「蓝牙直连」只能由后端链路类型判定（不许用『没有 IP』反推）", () => {
  const modal = read("components/AddFriendModal.vue");
  assert.ok(
    !/p\.ip \|\| t\("friend\.add\.viaBluetooth"\)/.test(modal),
    "不得再用 `p.ip || 蓝牙直连` 反推 —— Tailscale 同网段（Routed）的设备会被误标（用户实测）",
  );
  // 判据已抽到 utils/peerConnectionInfo.ts（资料页与添加好友页共用同一份），
  // 所以这里断言"调用那份判据"，而**不是**在组件里再写一遍 `p.link === "bluetooth"`。
  assert.match(modal, /function peerAddress\(/, "地址/链路文案必须收在一个函数里判定");
  assert.match(modal, /linkLabelKey\(/, "链路文案必须走 peerConnectionInfo 的唯一判据");
  assert.match(modal, /addressText\(/, "地址文案同样要走唯一判据（蓝牙/中继无地址）");
  const info = read("utils/peerConnectionInfo.ts");
  assert.match(
    info,
    /info\.link === "bluetooth"/,
    "只有后端说 bluetooth 才算蓝牙直连（不许用『没有 IP』反推）",
  );
  assert.match(
    info,
    /if \(info\.link === "bluetooth"\) return false;/,
    "蓝牙链路不显示 IP 行（蓝牙上没有 IP 概念）",
  );
  // 后端：命令返回时必须把链路类型填上（事件推送里没有它）
  const commands = readCommandsSrc();
  assert.match(commands, /async fn fill_peer_links\(/, "必须有唯一的『补链路类型』实现");
  assert.match(commands, /best_link_kind/, "链路类型按 LAN > Routed > Bluetooth 的优先级取");
});

/**
 * 自动拉起蓝牙**必须尊重用户的关闭偏好**（用户 2026-09-14 桌面实测：
 * 设置里关掉蓝牙，退出重进又被打开）。
 *
 * 根因：ensureBluetoothOn 原来用 channels[bluetooth].enabled（= 运行时是否在跑）判，
 * 而启动瞬间必然没在跑 ⇒ 把"用户明确关掉"当成"还没启动"，重新打开。
 * 判据：启用调用之前必须先判持久化偏好 preferred，且为 false 时直接返回。
 */
test("自动拉起蓝牙必须尊重用户的关闭偏好（不能只看运行时是否在跑）", () => {
  const store = read("stores/useAppStore.ts");
  const at = store.indexOf("async function ensureBluetoothOn");
  assert.ok(at > 0, "找不到 ensureBluetoothOn（护栏需要同步更新）");
  const body = store.slice(at, at + 1600);
  assert.match(
    body,
    /if \(!ch\.preferred\) return;/,
    "必须在拉起前判持久化偏好 preferred；用户明确关掉时不得自动打开",
  );
  const prefAt = body.indexOf("!ch.preferred");
  const enableAt = body.indexOf('setChannelEnabled("bluetooth", true)');
  assert.ok(prefAt > 0 && enableAt > prefAt, "偏好判据必须在启用调用之前（顺序反了等于没判）");
  assert.ok(
    !/\?\.enabled\) return;/.test(body),
    "不得再用 enabled（运行时是否在跑）当自动拉起的判据 —— 那正是本 bug 的根因",
  );
});

/**
 * 桌面系统通知**必须走后端命令**，不能再依赖被插件替换掉的 window.Notification
 * （用户 2026-09-14：Windows 同事收不到任何系统通知）。
 *
 * Tauri 的 notification 插件会把 window.Notification 换成转发到
 * plugin:notification|notify 的实现 —— 那条链路的 onclick 永远不触发，而且把真正的
 * toast 错误 spawn 掉丢了。现在统一 api.notifyDesktop：失败会返回并记日志，
 * 设置页还有「发送测试通知」自检。
 */
test("桌面通知必须走后端 notify_desktop（不再依赖被插件替换的 window.Notification）", () => {
  const store = read("stores/useChatStore.ts");
  // 第三个参数是 conv_id：**点通知要定位到会话**就得让后端知道这条通知属于谁
  // （点击是后端 notify-rust 的 handle 捕获的，前端补不了这个信息）。
  assert.match(
    store,
    /api\.notifyDesktop\(title, body, convId\)/,
    "桌面分支必须调后端 notify_desktop 命令，并把 conv_id 一起传过去",
  );
  assert.ok(
    !/new Notification\(/.test(store),
    "不得再用 WebView 原生 Notification（插件已把 window.Notification 换成另一套实现）",
  );
  assert.match(
    store,
    /if \(!document\.hidden && document\.hasFocus\(\) && activeConv\.value === rec\.conv_id\) return;/,
    "maybeNotify 必须同时判 !document.hidden（隐藏/最小化时 hasFocus 仍可能为 true）",
  );
  assert.match(read("api/index.ts"), /notifyDesktop:/, "api 层要暴露 notify_desktop");
  assert.match(
    read("components/settings/NotificationSection.vue"),
    /sendTestNotification/,
    "设置页必须有「发送测试通知」入口（Windows 静默失败时唯一的自检手段）",
  );
});

/**
 * 新消息提请注意（Windows 闪任务栏 / macOS 弹跳 Dock）。
 *
 * 为什么要静态钉住**调用位置**：它和系统通知是"同一件事的两种表达"——
 * 放在 `maybeNotify`（收到消息就调）会让"通知被 1.5s 去抖合并掉""用户正看着该会话被跳过"
 * 这些情况下**照样闪**，表现就是"没弹任何通知，任务栏却在闪"。
 * 只有放在 `flushNotifications` 的发送循环之后才与通知同进同出。
 */
test("提请注意必须与实际发出的通知同进同出（放 flushNotifications，不放 maybeNotify）", () => {
  const store = read("stores/useChatStore.ts");
  const flushAt = store.indexOf("function flushNotifications");
  const notifyAt = store.indexOf("function maybeNotify");
  const callAt = store.indexOf("api.requestAttention()");
  assert.ok(flushAt > 0 && notifyAt > 0 && callAt > 0, "store 里这三处都应能找到（函数被改名了？）");
  assert.ok(
    flushAt < callAt && callAt < notifyAt,
    "api.requestAttention() 必须写在 flushNotifications 里（且在 maybeNotify 之前），否则会脱离通知单独闪烁",
  );
  assert.match(
    store,
    /if \(anyReminded && !app\.isMobile\)/,
    "只在真的发出了通知时才闪，且移动端不调（没有任务栏可闪）",
  );
  assert.match(read("api/index.ts"), /requestAttention:/, "api 层要暴露 request_attention");
  // 后端：命令必须注册进 generate_handler!，否则前端调用返回 "Command not found"
  // （Android 侧的桩由 src-tauri 的 every_handler_command_exists_for_mobile 守卫）。
  const libRs = readFileSync(join(srcDir, "..", "src-tauri", "src", "lib.rs"), "utf8");
  assert.match(libRs, /commands::request_attention,/, "后端命令必须注册进 generate_handler!");
});

/**
 * 未读外显（托盘红点 / Windows 任务栏按钮角标 / macOS Dock 数字）。
 *
 * 为什么钉住"跟 `totalUnread` 走 + 去抖"：
 * - 跟**消息事件**走 ⇒ 标记已读、切会话、删会话时角标不更新，出现"红点清了、Dock 上还挂着 3"；
 * - 不去抖 ⇒ 消息洪水或"一键已读"会连续换托盘图标，Windows 上每次都是**肉眼可见的一下**。
 */
test("未读角标必须跟未读总数走、去抖后再推给后端（不是跟消息事件走）", () => {
  const store = read("stores/useChatStore.ts");
  assert.match(store, /watch\(totalUnread,/, "必须 watch totalUnread，而不是在收消息时改角标");
  assert.match(store, /api\.setUnreadBadge\(n\)/, "必须把**未读总数**推给后端");
  assert.match(store, /BADGE_DEBOUNCE_MS/, "必须有去抖（连续变化只推最后一次）");
  assert.match(store, /if \(app\.isMobile\) return;/, "移动端不走这条链路（没有托盘可改）");
  assert.match(read("api/index.ts"), /setUnreadBadge:/, "api 层要暴露 set_unread_badge");
  const libRs = readFileSync(join(srcDir, "..", "src-tauri", "src", "lib.rs"), "utf8");
  assert.match(libRs, /commands::set_unread_badge,/, "后端命令必须注册进 generate_handler!");
});

/**
 * 托盘红点必须**整块**挂在 `#[cfg(not(target_os = "macos"))]` 之下。
 *
 * 为什么这条值得占一个守卫位：红点在 macOS 上不参与渲染（那边走 Dock 数字角标），
 * 所以这份实现（色常量 / 几何常量 / 逐像素绘制 / 抗锯齿混合）在 macOS 上是**死代码** ——
 * 而 CI 的 macOS 腿跑 `cargo clippy -- -D warnings`，死代码直接判失败。
 *
 * 这个坑 2026-09-17 真踩过：Windows 上 clippy 干净、`cargo test` 全绿，只有 macOS 腿红
 * （PR #18 首轮 CI 就是这样）。修法是把它们整个收进带 cfg 的 `mod dot` ——
 * 分散地给每个 const/fn 各挂一次 cfg 迟早会漏一个，所以守卫钉的是"整块"。
 */
test("托盘红点的实现必须整块 cfg 到非 macOS（否则 macOS 腿 clippy 判死代码）", () => {
  const tray = readFileSync(join(srcDir, "..", "src-tauri", "src", "tray.rs"), "utf8");
  assert.match(
    tray,
    /#\[cfg\(not\(target_os = "macos"\)\)\]\nmod dot \{/,
    '红点实现必须整块收进 #[cfg(not(target_os = "macos"))] mod dot（见该模块的文档注释）',
  );
  const modStart = tray.indexOf("mod dot {");
  for (const name of ["const UNREAD_RED", "const DOT_R_RATIO", "fn with_unread_dot"]) {
    assert.ok(
      tray.indexOf(name) > modStart,
      `${name} 必须写在 mod dot 内（散在模块外 = macOS 上的死代码 ⇒ clippy 红）`,
    );
  }
});

/**
 * 链路徽标与在线状态必须**实时**（用户 2026-09-14：两边全在局域网，却显示「已桥接」，
 * 且好友在线状态不实时）。
 *
 * 两条根因都在这条测试里钉死：
 * 1. 前端只按"在不在节点表"判在线 ⇒ 有活跃链路但广播没收到的好友被显示离线；
 * 2. 聊天头的链路状态只按"消息条数"刷新 ⇒ 链路切回直连后仍显示「桥接」。
 */
test("链路徽标/在线状态必须实时（不能只看节点表或消息快照）", () => {
  const chat = read("stores/useChatStore.ts");
  assert.match(
    chat,
    /linkedIds\.has\(f\.device_id\)/,
    "在线必须包含「有活跃链路」的节点（与后端 friend_is_online 同口径）",
  );
  assert.match(chat, /filter\(\(x\) => x\.link\)/, "peers-updated 的 link 字段必须被用上");
  const win = read("components/ChatWindow.vue");
  assert.match(
    win,
    /const peerLink = chat\.peers\.find/,
    "聊天头的链路状态必须在活跃对端链路变化时刷新",
  );
});

/**
 * 收到的图片必须能在**字节落盘后**自动加载出来（用户 2026-09-14：群里收图时好时坏，
 * 点几次 / 等一会儿 / 重发才出来）。
 *
 * 判据：在途读预览得到的失败**不能永久缓存**；传输 Done 时必须让该消息的预览缓存失效，
 * 气泡据此重读。
 */
test("图片预览：在途失败不缓存 + 传输完成时失效重读", () => {
  const fp = read("utils/filePreview.ts");
  assert.match(fp, /export function invalidateFilePreview\(/, "必须提供预览缓存失效入口");
  assert.match(
    fp,
    /if \(r\.missing \|\| r\.note === "文件过大，无法预览"\) cache\.set\(msgId, r\)/,
    "只有确定性失败才缓存 —— 在途失败缓存了就永远不会重读",
  );
  const store = read("stores/useChatStore.ts");
  assert.ok(
    (store.match(/invalidateFilePreview\(/g) ?? []).length >= 2,
    "FileDone 必须让 file-/gfile- 两条消息的预览缓存失效",
  );
  const mf = read("composables/useMessageFile.ts");
  assert.match(mf, /transfer\.value\?\.status/, "预览必须随传输状态变化重读");
});

/**
 * 内容拉取（ADR-0019 Phase 3）：能力协商 + 点击重取。
 *
 * 判据：api 暴露 request_content；图片气泡失败可触发重取；前端真的调后端命令。
 */
test("内容拉取：点击重取必须接通后端 request_content", () => {
  assert.match(read("api/index.ts"), /requestContent:/, "api 必须暴露 request_content");
  const item = read("components/MessageItem.vue");
  assert.match(item, /@refetch="refetchContent"/, "图片气泡失败必须能触发重取");
  assert.match(item, /invoke<boolean>\("request_content"/, "必须真的调后端命令");
});

/** 统一内容状态（ADR-0019 Phase 1）：未完成/失败的文件卡片必须能给「重新获取」。 */
test("统一内容状态：未完成/失败的文件必须能给「重新获取」", () => {
  assert.match(read("api/index.ts"), /getContentTransfers:/, "api 必须暴露 get_content_transfers");
  assert.match(read("stores/useChatStore.ts"), /contentTransfers/, "store 必须持有统一内容状态");
  const bubble = read("components/message/MessageFileBubble.vue");
  assert.match(bubble, /emit\('refetch'\)/, "未完成时按钮必须触发 refetch");
  const item = read("components/MessageItem.vue");
  assert.match(item, /:content-retry=/, "MessageItem 必须把统一状态传给文件卡片");
});
