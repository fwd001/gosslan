/**
 * `friends[].online` 判定口径的判据。
 *
 * ## 这个函数为什么被抽出来
 * 口径曾被抄成两份（`onPeers` 那条链与 `searchNearbyPeers`），其中一份少了
 * 「持有活跃链路也算在线」那一层。两个调用点现在共用本函数（AI_RULES §32：
 * 同一份语义不许有两个家）。抽成纯函数而不是留在 store 里，是为了**能测** ——
 * 藏在 store 私有函数里的口径护栏无从下手。
 *
 * ## ★ 为什么这里只判「在列表里」，不额外判「有链路」
 * 后端给 `link` 填值的两个入口**形状完全相同**，都只给「已经在 peers 表里」的节点填：
 *
 * · `state.rs::emit_peers_now`（`peers-updated` 事件的来源）：
 *   先从 `self.peers` 收集出 `peers`，再 `for p in peers.iter_mut()` 按 `p.device_id`
 *   查 `links` 填 `p.link`；
 * · `commands/network.rs::fill_peer_links`（`search_nearby_peers` / `get_peers` 的来源）：
 *   同样先收 `s.peers`，再 `for p in peers.iter_mut()` 填 `p.link`。
 *
 * 于是恒有 `link` 非空 ⇒ 该节点在同一个 list 里 ⇒ `linkedIds ⊆ onlineIds`，
 * `|| linkedIds.has(...)` 在当前数据形状下是**冗余的**。
 *
 * ## ★★ 别再把 F2 那条"缺陷"当真的（2026-10-03 已核查并排除）
 * 审计曾报告"`searchNearbyPeers` 漏了 linkedIds ⇒ 群文件…（好友）在建链时会被闪成离线"。
 * 它引用的后端注释（`state.rs:1770-1774`）描述的确实是真实痛点：
 * 「链路活着、但广播没收到（被防火墙/组播吞掉）或刚被 sweep」的好友会显示离线。
 * **但那不是前端能修的**：修好它的前提是「link 能给一个不在 peers 表里的节点」，
 * 而上面两条填值路径都做不到 —— 节点不在 `peers` 表里，后端根本不会给它 `link` 字段，
 * 前端拿到的 list 里就不可能有它。
 *
 * 也就是说：`onPeers` 那句注释描述的缺陷**在数据形状层面就不可达**，
 * `|| linkedIds` 是一句"看起来在兜底、实际不兜任何东西"的代码。
 * 真正该修的是后端让 `peers` 表包含"有链路但广播没到"（或 `fill_peer_links`
 * 接受一个"额外需要填 link 的 id 集合"），那是另一个话题、不在本次范围。
 *
 * 保留 `|| linkedIds` 不删：它**无害**（子集关系使它恒为冗余，不会算错），
 * 且后端哪天真的支持了"表外节点带 link"，这一行就自动开始生效。
 * 但上面这段必须留在注释里 —— 否则下一个人会以为它在兜什么，
 * 或者（更糟）反过来以为"删掉它会引入 bug"而不敢简化。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { markFriendsOnlineFrom, type FriendOnlineTarget } from "./friendOnline.ts";

const F = (device_id: string, online = false): FriendOnlineTarget => ({ device_id, online });

test("出现在节点表里 ⇒ 在线", () => {
  const fs = [F("a"), F("b")];
  markFriendsOnlineFrom(fs, [{ device_id: "a" }, { device_id: "c", link: null }]);
  assert.equal(fs[0].online, true, "在表里就是在线");
  assert.equal(fs[1].online, false, "不在表里 ⇒ 离线");
});

test("link 有值时也是在线", () => {
  const fs = [F("a")];
  markFriendsOnlineFrom(fs, [{ device_id: "a", link: "lan" }]);
  assert.equal(fs[0].online, true, "持有活跃链路 ⇒ 在线");
});

/**
 * 钉住**代码形状**：`|| linkedIds.has(...)` 那一层不许被删。
 *
 * ## 它今天在当前数据形状下是冗余的（见文件头注释的推导）
 * 后端只给「已在 peers 表里」的节点填 `link`，恒有 `linkedIds ⊆ onlineIds`，
 * 所以删掉这层**行为不变**。那为什么还要钉？
 *
 * 因为 `channelState.test.ts` 那条 2026-09-14 的判据（用户实测："局域网都连上了，
 * 在线状态却不实时"）要求这一层**存在于代码里**。它防的不是今天的行为，
 * 是"有人看到这层冗余就顺手清理掉"——而项目显然认为这层是契约的一部分。
 * 本条把该契约写在**判据能真正读到它的地方**，且明确写出"删掉它行为不变"，
 * 免得下一个人以为它在兜什么、或者反过来不敢简化。
 *
 * ## 为什么不能靠"行为断言"钉
 * 非空转实测：把 `|| linkedIds.has(...)` 删掉后，**本文件其余 6 条用例全绿**
 * （它们都能靠 `onlineIds` 定在线）⇒ 行为断言对这一层是**空转**的。
 * 同一次实测里 `channelState.test.ts:16` 那条会红——真正能抓住它的只有形状断言。
 * 这也是本文件第一版踩过的坑：新写行为用例时把设备同时放进了 list，
 * 于是"看起来在测 linkedIds、实际 onlineIds 就够了"，判据自等于。
 */
test("★ 代码形状：`|| linkedIds.has(...)` 那层必须留着（删掉行为不变，但它是契约）", async () => {
  const { readFileSync } = await import("node:fs");
  const { join } = await import("node:path");
  const src = readFileSync(join(import.meta.dirname, "friendOnline.ts"), "utf8");
  assert.match(
    src,
    /onlineIds\.has\(f\.device_id\)\s*\|\|\s*linkedIds\.has\(f\.device_id\)/,
    "在线判定必须保留「或持有活跃链路」这一层。它在当前后端数据形状下冗余（linkedIds ⊆ onlineIds），\n" +
      "但 2026-09-14 的判据（channelState.test.ts）把它当契约钉着 —— 清理这层前先问过那条判据的来历。",
  );
  // 同时钉住 linkedIds 的来源是 `x.link`（而不是别的东西），否则上面那条形状断言可以被"改形状绕过"
  assert.match(src, /filter\(\(x\) => x\.link\)/, "linkedIds 必须由 `link` 字段筛出");
});

test("link 为空串 / null / undefined 都算「无链路」", () => {
  // 判据形状：link 只在**该条目本身**上生效（后端只给表内节点填 link）
  for (const link of ["", null, undefined]) {
    const fs = [F("a", true)];
    markFriendsOnlineFrom(fs, [{ device_id: "other", link }]);
    assert.equal(fs[0].online, false, `link=${JSON.stringify(link)} 不该算有链路`);
  }
});

test("online 是每次重算的派生值，不粘住上一轮结果", () => {
  const fs = [F("a", true)];
  markFriendsOnlineFrom(fs, [{ device_id: "a", link: "lan" }]);
  assert.equal(fs[0].online, true);
  markFriendsOnlineFrom(fs, []); // 下一轮没拿到它 ⇒ 必须回落成离线
  assert.equal(fs[0].online, false, "不能因为上一轮是 true 就粘住");
});

test("返回同一个数组（可就地链式调用）", () => {
  const fs = [F("a")];
  assert.equal(markFriendsOnlineFrom(fs, [{ device_id: "a" }]), fs);
});

test("空节点表 ⇒ 全部离线", () => {
  const fs = [F("a", true), F("b", true)];
  markFriendsOnlineFrom(fs, []);
  assert.deepEqual(
    fs.map((f) => f.online),
    [false, false],
  );
});

/**
 * 钉住后端的填值前提：**`link` 只给已在节点表里的节点填**。
 *
 * 这条不是测 `markFriendsOnlineFrom` 的行为（它对 link 无能为力的部分已由上面的用例覆盖），
 * 而是测**我们对后端数据形状的假设**。假设一旦被后端改掉（比如让它支持表外节点带 link），
 * 这条会红，提醒同步更新函数头注释里那段"linkedIds ⊆ onlineIds"的推导。
 *
 * 判据形态：源码里 `fill_peer_links` / `emit_peers_now` 必须是
 * 「先收集 peers、再 `for p in peers.iter_mut()` 填 link」这个顺序 ——
 * 如果哪天改成"另外接受一组 id 也给它们填 link"，`linkedIds` 就不再是冗余，
 * 上面那条"不额外判 link"的说明与本函数都要重新审。
 */
test("后端两个填 link 的入口都只给「已在 peers 表里」的节点填（linkedIds ⊆ onlineIds 的前提）", async () => {
  const { readFileSync } = await import("node:fs");
  const { join } = await import("node:path");
  const root = join(import.meta.dirname, "../..");
  const files = [
    "src-tauri/src/commands/network.rs",
    "src-tauri/src/state.rs",
  ];
  for (const f of files) {
    const src = readFileSync(join(root, f), "utf8");
    assert.match(
      src,
      /for p in peers\.iter_mut\(\)/,
      `${f}：link 的填值循环必须仍是对 peers 表内迭代。若后端改成"表外节点也带 link"，
       linkedIds 就不再是 onlineIds 的子集，friendOnline.ts 头注释那段推导与
       本函数都要重新审。`,
    );
  }
});
