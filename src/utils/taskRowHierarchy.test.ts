import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { zhCN, enUS } from "../i18n/locales.ts";

/**
 * 群任务列表那一行的**信息层级**（用户 2026-09-29 需求汇总七：默认落在「给我的」；
 * 行里要一眼看清「谁发起、指派给谁」，其余文字弱化）。
 *
 * 判的是接线而不是观感：这台机器锁屏，截图与无障碍两条通道都拿不到 ⇒ **视觉结果没有运行时证据**，
 * 下面这几条只能保证"改成的形状不会被好心改回去"。
 */
const SRC = readFileSync(join(import.meta.dirname, "..", "components", "GroupTasksBoard.vue"), "utf8");

test("默认 Tab 是「给我的」，而四档本身没被动过", () => {
  assert.match(SRC, /const filter = ref<TodoFilter>\("mine"\);/, "默认档位不是 mine ⇒ 用户点名的『一进来就选中全部』还在");
  // 正面：证明 `mine` 这一档在这个文件里**真的存在且被消费**，否则上面那条会因为改名而空转
  assert.match(SRC, /\{ key: "mine", labelKey: "todo\.filter\.mine" \}/);
  assert.match(SRC, /filter\.value = "mine";/, "关面板时的复位没跟着改 ⇒ 关一次再开就回到 all，默认那一句只在第一次成立");
  // 「新建后如果正看着已归档就切回全部」那一条是**可见性**判断，不该跟着改成 mine（新建的任务未必分给我）
  assert.match(SRC, /if \(filter\.value === "archived"\) filter\.value = "all";/);
});

/**
 * 行内层级：**名字用主文本色 + 字重，标签用次级色**，活动行与已归档行同一套写法。
 *
 * 旧写法是整句「由 X 发起 · 指派 Y」全行同一个 11px 次级色，于是"谁发起、指派给谁"
 * 这两件用户要一眼读到的事和分隔符同权。判据两头都钉：
 *   正面 = 四处"名字提到主文本色"（两块行各 2 处）+ 两个标签各用两次；
 *   反面 = 行内不许再出现整句式 `t("todo.creator")` / `t("todo.assigneesInline")`
 *          （那两个键仍归任务详情弹窗用 —— 那里字号够，读得出整句）。
 */
test("发起人与指派人的名字比标签更抢眼，且两档列表同一套", () => {
  // 分母是"我排进去的那四处"，不是"这个文件里有几处 font-medium 主色"：
  // 整串 `<span class="font-medium ...">` 才唯一对应"名字那一格"，否则分组头/标题/选中态会混进来
  const strong = SRC.match(/<span class="font-medium text-\[var\(--gosslan-text\)\]">/g) ?? [];
  assert.equal(strong.length, 4, `行内"名字提到主文本色"该有 4 处（活动 + 已归档各两处），实测 ${strong.length}`);
  const labels = (SRC.match(/t\("todo\.rowCreator"\)/g) ?? []).length;
  const labels2 = (SRC.match(/t\("todo\.rowAssignee"\)/g) ?? []).length;
  assert.deepEqual([labels, labels2], [2, 2], "两个标签必须两档列表都在：少一个是缺信息不是弱化");
  assert.equal(SRC.includes('t("todo.creator"'), false, "行里还留着整句「由 X 发起」⇒ 层级没改过来");
  assert.equal(SRC.includes('t("todo.assigneesInline"'), false, "行里还留着整句「指派 X」⇒ 层级没改过来");
});

/** 中英两份都得有那两个标签键（字典那条 key 集合一致性判据也管，但它不知道"值该长什么样"）。 */
test("行内标签在中英两份里都存在且非空", () => {
  for (const [name, dict] of [["zh-CN", zhCN], ["en-US", enUS]] as const) {
    for (const k of ["todo.rowCreator", "todo.rowAssignee"]) {
      assert.ok((dict[k] ?? "").trim().length > 0, `${name} 缺 ${k} ⇒ 界面上那一格会退化成键名本身`);
    }
  }
});
