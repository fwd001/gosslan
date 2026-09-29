import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

/**
 * 群任务「添加图片」那一块的交互契约（用户 2026-09-29 逐句定下来的那几件）。
 *
 * 为什么用源码形状判而不是跑 UI：这里的三条都落在 `GroupTasksBoard.vue` 的事件接线上，
 * 而单测环境没有 WebView（拿不到 canvas、也发不出真 paste 事件）。形状判据挡的是
 * "以后有人好心把它改回去"这一类回归 —— 那正是这几条的失效方式。
 *
 * ⚠️ 每条**取反**的判据都配了一条**正面**判据（同一份源码里必须存在什么）：
 * 只写"不许有 X"的话，把 X 和它旁边的接线一起删掉就"通过"了。
 */
const SRC = readFileSync(
  join(import.meta.dirname, "..", "components", "GroupTasksBoard.vue"),
  "utf8",
);

test("点击「添加图片」不再直接弹文件选择器（桌面端只做聚焦）", () => {
  // 反面：模板里任何一处把选图入口接回按钮，就是用户这次明确要改掉的行为
  const direct = [...SRC.matchAll(/@click="addImage"/g)];
  assert.equal(direct.length, 0, `仍有 ${direct.length} 处 @click="addImage"：点击会展开文件列表`);
  // 正面：证明"点击有去处"在这份源码里能为真（否则上面那条会因为整块被删而空转）
  const armed = [...SRC.matchAll(/@click="armImageZone"/g)];
  assert.ok(armed.length >= 1, `一处 @click="armImageZone" 都没有 ⇒ 点击什么都不做，粘贴区没入口`);
});

/**
 * Android 没有剪贴板图片这回事（WebView 的 paste 拿不到位图），点击是**唯一**的加图方式。
 * 把桌面改成"点击只聚焦"时，最容易顺手牺牲的就是这一半 —— 表现是 Android 上再也加不了图，
 * 而这在这台机器上跑不出来。
 */
test("Android 那一半没被顺手删掉：点击仍然走选图", () => {
  const at = SRC.indexOf("function armImageZone()");
  assert.ok(at > 0, "找不到 armImageZone() —— 上面那条正面判据的落点没了，判据要跟着改");
  const body = SRC.slice(at, SRC.indexOf("\n}", at));
  assert.match(body, /isAndroid/, "armImageZone 里没有按平台分支");
  assert.match(body, /void addImage\(\)/, "Android 分支必须仍然调 addImage()，否则移动端没有加图入口");
});

/**
 * 粘贴进来的图**原样落盘**：不允许在 JS 侧过一遍 canvas（用户 2026-09-29：「图片应该是原始的」）。
 *
 * 过 canvas 必然重编码 —— 那是这条链上唯一会自己造成"糊"的一步（传输本身按 sha256 逐字节校验，
 * 显示层读的是同一份文件字节）。
 */
test("粘贴路径不得重编码图片（不建 canvas、不用 toDataURL）", () => {
  assert.equal(SRC.includes("toDataURL"), false, "任务图片路径里出现了 toDataURL ⇒ 会被重编码");
  assert.equal(
    SRC.includes('createElement("canvas")'),
    false,
    "任务图片路径里出现了画布 ⇒ 高清原图会被降采样",
  );
  // 正面：剪贴板字节必须原样交给后端那条 raw IPC
  assert.match(
    SRC,
    /saveTodoImageBytes\(new Uint8Array\(buf\)\)/,
    "粘贴字节没有直传后端 ⇒ 这条链换了形状，判据要重新对",
  );
});

/**
 * 拖入**要加图**（用户 2026-09-29 汇总：「点击后支持：拖拽图片进入、Ctrl+V 粘贴」），
 * 而**抑制也必须留着**。
 *
 * 为什么这条值得钉：草稿弹窗叠在聊天窗口上，两处监**同一路** webview 拖放事件。
 * 只留"加图"、不留 `app.boardDropActive`，落在图片区那一下会**顺路把文件当聊天附件发出去**
 * （不可撤回）；反过来整段删掉订阅，用户要的"拖进来"就没了。两头各判一句。
 */
test("拖放：落进图片区要加进任务，同时把聊天那侧的接收让掉", () => {
  assert.match(
    SRC,
    /if \(hit && p\.paths\.length\) void addImageFromPaths\(p\.paths\);/,
    "drop 不再往任务里加图 ⇒ 用户要的「支持拖拽图片进入」被丢了",
  );
  assert.match(SRC, /onDragDropEvent/, "拖放订阅被整段删了 ⇒ 文件会被聊天那侧收走并发出去");
  assert.match(
    SRC,
    /app\.boardDropActive = hit;/,
    "抑制逻辑没了 ⇒ 同一份文件会同时被聊天那侧发出去",
  );
});
