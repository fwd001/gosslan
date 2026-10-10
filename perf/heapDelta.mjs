#!/usr/bin/env node
/**
 * 量具八：堆快照（.heapsnapshot）的**离线归并器** —— roadmap N23 那一格缺的就是这一半。
 *
 * ## 为什么值得单独一个文件
 * N23 登记的是「内存占用未测量 ⇒ 不写百分比也不写『泄漏』」。量具四（`perf/run.mjs` 的 mem 档）
 * 能给出 `usedJSHeapSize / JSEventListeners / Nodes` 这些**标量曲线**，能判"有没有上行台阶"，
 * 但**判不出长了什么** —— 归因要 V8 堆快照，而一份快照是 20~80MB 的 JSON + 定长整型数组，
 * 手点 DevTools 既不可复现也没法进门禁式对账。这个文件做的事就一件：把快照里
 * 「同类节点的 self_size 聚合」与「两份快照之间的差」用**可复跑**的方式算出来。
 *
 * ## 它是量具，不是门禁
 * 与 perf/ 其余那几台同口径（`npm test` 里没有 perf 条目，CI 也不跑这一层）：
 * 读数用来定位，不用来判发版。**数字不许抄进文档**，要写就写命令。
 *
 * ## 用法
 *     node perf/heapDelta.mjs --selfcheck                 # 自带对账（无浏览器、无网络）
 *     node perf/heapDelta.mjs a.heapsnapshot              # 单份：按类型/name 看谁最占
 *     node perf/heapDelta.mjs before.heapsnapshot after.heapsnapshot   # 两份：逐格算差
 *   选项：`--top=15`（打印几行）、`--group=type|name|type+name`（按什么归并，默认 type+name）
 *
 * ## 快照怎么来（这一半要 CDP，本文件不碰浏览器）
 * 走 `HeapProfiler.takeHeapSnapshot`（分片 `addHeapSnapshotChunk` 拼盘），或 DevTools 手动 Memory →
 * Take heap snapshot 存盘。两种都得先把探针的浏览器拉起来 —— 见 perf/README.md 量具八那节的"今天卡在哪"。
 *
 * ## 它读不到什么（诚实边界，别拿它当结论）
 * ① V8 的 `self_size` **不含**字符串/ArrayBuffer 的外部内存（`externalCommands` 那部分要另读）；
 * ② 归并到 `name` 时闭包/匿名函数会是空串，那一格只能按类型看；
 * ③ 快照里"从 GC root 到它"的完整路径要跑一次全图 BFS（几百万节点、内存 3~5× 文件），
 *    这里刻意不做 —— 现在只出**谁变多了**，不出**谁拽着它不放**，那是下一步、且要先定标；
 * ④ detachedness 只统计 V8 标了的那一档，不等于"界面还在但内存里 detached"。
 */

import { readFileSync } from "node:fs";

const ARGV = process.argv.slice(2);
const OPT = (name, dflt) => {
  const hit = ARGV.find((a) => a.startsWith(`--${name}=`));
  return hit ? hit.slice(name.length + 3) : dflt;
};
const POS = ARGV.filter((a) => !a.startsWith("--"));

/** 快照的定长数组按 meta 里的字段表解释；字段名变了就应当**当场炸**，不要静默算出错数。 */
function fieldIndex(meta, want) {
  const i = meta.node_fields.indexOf(want);
  if (i < 0) throw new Error(`快照里没有节点字段「${want}」（node_fields=${meta.node_fields.join(",")}）⇒ V8 格式变了，先修这把尺子`);
  return i;
}

/** 解析一份快照的**聚合表**（不建整图，所以内存 ≈ 文件本身，不是 3~5×）。 */
export function summarize(json, group = "type+name") {
  const snap = typeof json === "string" ? JSON.parse(json) : json;
  const meta = snap?.snapshot?.meta;
  const nodes = snap?.nodes;
  if (!meta || !Array.isArray(nodes)) {
    throw new Error("不是 .heapsnapshot（缺 snapshot.meta 或 nodes）");
  }
  const stride = meta.node_fields.length;
  if (nodes.length % stride !== 0) {
    throw new Error(`nodes 长度 ${nodes.length} 不是字段数 ${stride} 的整数倍 ⇒ 文件被截断了，这份读数不可信`);
  }
  const typeNames = meta.node_types[0];
  const strings = snap.strings ?? [];
  const iType = fieldIndex(meta, "type");
  const iName = fieldIndex(meta, "name");
  const iSize = fieldIndex(meta, "self_size");
  // detachedness / edge_count 在新旧 V8 里不一定有 —— 这两个是"有就用"，不能拿它们当必要字段。
  const detachedCol = meta.node_fields.includes("detachedness")
    ? meta.node_fields.indexOf("detachedness") : -1;

  const agg = new Map();
  let total = 0;
  let count = 0;
  let detachedCount = 0;
  let detachedSize = 0;
  for (let n = 0; n < nodes.length; n += stride) {
    const type = typeNames[nodes[n + iType]] ?? `#${nodes[n + iType]}`;
    const raw = strings[nodes[n + iName]] ?? "";
    const size = nodes[n + iSize];
    total += size;
    count += 1;
    if (detachedCol >= 0 && nodes[n + detachedCol] === 1) {
      detachedCount += 1;
      detachedSize += size;
    }
    let key;
    if (group === "type") key = type;
    else if (group === "name") key = raw || "(无名)";
    else key = `${type} ${raw}`;
    const cur = agg.get(key);
    if (cur) {
      cur.size += size;
      cur.count += 1;
    } else {
      agg.set(key, { key, type, name: raw, size, count: 1 });
    }
  }
  const rows = [...agg.values()].sort((a, b) => b.size - a.size || a.key.localeCompare(b.key));
  return { rows, total, count, detached: { count: detachedCount, size: detachedSize } };
}

/** 两份快照的逐格差（按 key 对齐；只有一边有的那份按 0 算，但**标出来**）。 */
export function diff(before, after) {
  const a = new Map(before.rows.map((r) => [r.key, r]));
  const b = new Map(after.rows.map((r) => [r.key, r]));
  const rows = [];
  for (const key of new Set([...a.keys(), ...b.keys()])) {
    const s = (a.get(key)?.size ?? 0);
    const t = (b.get(key)?.size ?? 0);
    const c = (a.get(key)?.count ?? 0);
    const d = (b.get(key)?.count ?? 0);
    rows.push({ key, before: s, after: t, delta: t - s, countDelta: d - c, newKey: !a.has(key), gone: !b.has(key) });
  }
  rows.sort((x, y) => Math.abs(y.delta) - Math.abs(x.delta) || x.key.localeCompare(y.key));
  return { rows, totalDelta: after.total - before.total, countDelta: after.count - before.count };
}

const fmt = (n) => (n >= 1024 * 1024 ? `${(n / 1024 / 1024).toFixed(2)} MB`
  : n >= 1024 ? `${(n / 1024).toFixed(1)} KB` : `${n} B`);

function report(sum, d, top) {
  console.log(`节点 ${sum.count} 个 / self_size 合计 ${fmt(sum.total)}`
    + (sum.detached.count ? `；V8 标了 detached 的 ${sum.detached.count} 个 / ${fmt(sum.detached.size)}` : ""));
  if (d) {
    console.log(`两份之间：净 ${fmt(Math.abs(d.totalDelta))}（${d.totalDelta >= 0 ? "变大" : "变小"}）`
      + `，节点数 ${d.countDelta >= 0 ? "+" : ""}${d.countDelta}`);
  }
  const head = d ? d.rows.slice(0, top) : sum.rows.slice(0, top);
  console.log(`\n按「${OPT("group", "type+name")}」归并的前 ${head.length} 格${d ? "（按差绝对值排）" : "（按 self_size 排）"}：`);
  for (const r of head) {
    if (d) {
      const flag = r.newKey ? " 新增" : r.gone ? " 消失" : "";
      console.log(`  ${r.delta >= 0 ? "+" : "-"}${fmt(Math.abs(r.delta)).padStart(9)} `
        + `（${String(r.countDelta).padStart(6)} 个节点）${flag}  ${r.key.slice(0, 74)}`);
    } else {
      console.log(`  ${fmt(r.size).padStart(9)} （${String(r.count).padStart(6)} 个节点）  ${r.key.slice(0, 74)}`);
    }
  }
}

/** 读一个文件（可能是 JSON 字符串本身，selfcheck 用得到）。 */
function load(path) {
  return summarize(JSON.parse(readFileSync(path, "utf8")), OPT("group", "type+name"));
}

// ---------------- 自带对账（无浏览器） ----------------

/** 手写一份最小合法快照：字段顺序与 V8 一致（type,name,id,self_size,edge_count,detachedness）。 */
function fakeSnapshot(spec) {
  const strings = spec.strings;
  const nodes = [];
  for (const n of spec.nodes) {
    nodes.push(n.type, strings.indexOf(n.name), n.id, n.size, n.edges ?? 0, n.detached ?? 0);
  }
  return {
    snapshot: {
      meta: {
        node_fields: ["type", "name", "id", "self_size", "edge_count", "detachedness"],
        // ⚠️ V8 的顺序是：node_types[0] = **type 字段的枚举名**（不是"每个字段的种类"），
        //    我这把尺子按那条惯例读；第一版把种类数组写在前头 ⇒ selfcheck 当场把期望值照出来，
        //    红的是样本不是解析器。
        node_types: [["array", "object", "closure"], []],
        edge_fields: ["type", "name_or_index", "to_node"],
        edge_types: [["context", "element"], []],
      },
      node_count: spec.nodes.length,
      edge_count: 0,
    },
    nodes,
    edges: [],
    strings,
  };
}

async function selfCheck() {
  const { deepStrictEqual, ok, throws } = await import("node:assert/strict");
  let n = 0;
  const T = (name, fn) => { fn(); n += 1; console.log(`  ✔ ${name}`); };
  const A = fakeSnapshot({
    strings: ["root", "convCache", "outer"],
    nodes: [
      { type: 1, name: "root", id: 1, size: 100, edges: 2 },
      { type: 1, name: "convCache", id: 3, size: 900, edges: 0 },
      { type: 2, name: "convCache", id: 5, size: 80, edges: 0, detached: 1 },
    ],
  });
  const B = fakeSnapshot({
    strings: ["root", "convCache", "outer"],
    nodes: [
      { type: 1, name: "root", id: 1, size: 100, edges: 2 },
      { type: 1, name: "convCache", id: 3, size: 5900, edges: 0 },
      { type: 0, name: "outer", id: 7, size: 64, edges: 0 },
    ],
  });

  T("正向：按 type+name 归并，convCache 两档不合并（类型不同就是两格）", () => {
    const s = summarize(A);
    deepStrictEqual(s.total, 1080);
    deepStrictEqual(s.count, 3);
    deepStrictEqual(s.rows.map((r) => [r.key, r.size, r.count]),
      [["object convCache", 900, 1], ["object root", 100, 1], ["closure convCache", 80, 1]]);
  });
  T("正向：detachedness 那一档被单独统计（有列才统计，没列也不能报错）", () => {
    deepStrictEqual(summarize(A).detached, { count: 1, size: 80 });
    const noCol = structuredClone(A);
    noCol.snapshot.meta.node_fields = ["type", "name", "id", "self_size", "edge_count"];
    noCol.nodes = [1, 0, 1, 100, 2, 1, 1, 3, 900, 0, 2, 2, 5, 80, 0];
    deepStrictEqual(summarize(noCol).detached, { count: 0, size: 0 });
  });
  T("正向：两份相减按 key 对齐，新增/消失都标出来", () => {
    const d = diff(summarize(A), summarize(B));
    deepStrictEqual(d.totalDelta, 6064 - 1080);
    deepStrictEqual(d.countDelta, 0);
    const byKey = new Map(d.rows.map((r) => [r.key, r]));
    deepStrictEqual([byKey.get("closure convCache").delta, byKey.get("closure convCache").gone], [-80, true]);
    ok(byKey.get("array outer").newKey, "array outer 是新增格");
    deepStrictEqual(byKey.get("object root").delta, 0, "没变的那格差必须正好 0");
    // ⚠️ 排序键是**差的绝对值**：-80 那格排在 +64 前面（我第一版把期望写成按带符号大小排，
    //    被这条 selfcheck 当场照出来 ⇒ 红的是期望值不是解析器）。"变小最多"同样是归因要看的信号。
    deepStrictEqual(d.rows.map((r) => r.key).slice(0, 3),
      ["object convCache", "closure convCache", "array outer"],
      "必须按差绝对值排（+5000 → -80 → +64）");
  });
  T("反例：nodes 长度不是字段数整数倍 ⇒ 当场抛，不能静默算出错数", () => {
    const broken = structuredClone(A);
    broken.nodes = broken.nodes.slice(0, broken.nodes.length - 2);
    throws(() => summarize(broken), /被截断/);
  });
  T("反例：字段名换了（V8 格式变）⇒ 抛「先修这把尺子」而不是按老格式硬算", () => {
    const moved = structuredClone(A);
    moved.snapshot.meta.node_fields = moved.snapshot.meta.node_fields.map((f) => (f === "self_size" ? "size_self" : f));
    throws(() => summarize(moved), /先修这把尺子/);
  });
  T("反例：根本不是 .heapsnapshot ⇒ 抛缺字段", () => {
    throws(() => summarize({ nodes: [1, 2, 3] }), /不是 .heapsnapshot/);
  });

  console.log(`\n✅ heapDelta 自带对账 ${n} 条通过（手写最小快照，不依赖浏览器与真机）`);
}

// ---------------- 入口 ----------------

if (ARGV.includes("--selfcheck")) {
  await selfCheck();
} else {
  if (POS.length === 0) {
    console.error("用法：node perf/heapDelta.mjs <a.heapsnapshot> [b.heapsnapshot] [--top=15] [--group=type|name|type+name]\n     node perf/heapDelta.mjs --selfcheck");
    process.exit(2);
  }
  const top = Number(OPT("top", "15"));
  const a = load(POS[0]);
  if (POS.length === 1) {
    report(a, null, top);
  } else {
    report(a, diff(a, load(POS[1])), top);
  }
}
