/**
 * 回应名单的判据（用户 2026-09-29 需求汇总三：「能看到谁加的、谁最新追加」「同一表情
 * 多人聚合、人多显示 +N」「PC 悬停、移动端点击/长按看名单」「不喧宾夺主」）。
 *
 * ## 为什么"顺序"这件事值得单开一条判据
 * `foldReactions` 早就算出了 `actors`，但它以前是 **device_id 字典序** —— 那是"看着像
 * 有序"的实际无序：把"最后点的那个人"放到名单里任意位置，"谁最新追加"这一格信息就归零，
 * 而渲染层无论怎么写都救不回来。所以顺序必须钉在**折叠层**，而不是留给组件去猜。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { foldReactions, ROSTER_VISIBLE, summarizeActors } from "./reactions.ts";
import type { MessageRecord } from "../types";

function rx(
  msg_id: string,
  sender: string,
  emoji: string,
  add: boolean,
  seq: number,
): MessageRecord {
  return {
    id: 0,
    msg_id,
    conv_id: "group:g1",
    sender_id: sender,
    receiver_id: "g1",
    kind: "reaction",
    content: JSON.stringify({ target: "m1", emoji, add }),
    ts: 0,
    seq,
    status: "delivered",
  };
}

const chip = (records: MessageRecord[]) => foldReactions(records, "me").get("m1")?.[0];

/** 故意让字典序与追加序**相反**：`zzz` 先点、`aaa` 后点。 */
test("名单按追加先后排（末位 = 最新点的人），不是 device_id 字典序", () => {
  const c = chip([
    rx("r1", "zzz", "[赞]", true, 1),
    rx("r2", "mmm", "[赞]", true, 2),
    rx("r3", "aaa", "[赞]", true, 3),
  ])!;
  assert.deepEqual(c.actors, ["zzz", "mmm", "aaa"], "必须是追加顺序；退回 .sort() 就变成 aaa/mmm/zzz");
  assert.equal(c.latest, "aaa", "latest 要单独给字段，不许调用点靠「取最后一个」表意");
  assert.equal(c.count, 3, "多人同表情仍然只聚合成一枚胶囊");
});

test("同一个人取消后再点 ⇒ 他重新排到最后（'最新追加'是动态的）", () => {
  const c = chip([
    rx("r1", "a", "[赞]", true, 1),
    rx("r2", "b", "[赞]", true, 2),
    rx("r3", "a", "[赞]", false, 3), // a 取消
    rx("r4", "a", "[赞]", true, 4), // a 又点上：生效那条的 seq 是 4
  ])!;
  assert.deepEqual(c.actors, ["b", "a"]);
  assert.equal(c.latest, "a");
});

test("seq 相同也要给出确定顺序（跨副本不能各排各的）", () => {
  const one = chip([rx("r9", "b", "[赞]", true, 5), rx("r2", "a", "[赞]", true, 5)])!;
  assert.deepEqual(one.actors, ["a", "b"], "同 seq 时按 msg_id 决胜 —— 与 LWW 的 tie-break 同一把尺");
});

test("summarizeActors：超出上限折成 +N，名字由调用方给", () => {
  const five = ["a", "b", "c", "d", "e"];
  const nameOf = (id: string) => id.toUpperCase();
  assert.deepEqual(summarizeActors(["a"], nameOf), { shown: ["A"], hidden: 0 });
  const s = summarizeActors(five, nameOf, ROSTER_VISIBLE);
  assert.deepEqual(s.shown, ["A", "B", "C"]);
  assert.equal(s.hidden, 2);
  // 上限被配成 0/负数时不许变成"名单空着但说还有 5 人"
  assert.equal(summarizeActors(five, nameOf, 0).shown.length, 1);
});

const bar = () =>
  readFileSync(new URL("../components/message/MessageReactionBar.vue", import.meta.url), "utf8");

test("揭示方式两端都有出口：hover 只在有指针的设备启用", () => {
  const src = bar();
  assert.match(src, /@mouseenter="canHover && reveal\(c\.emoji\)"/, "悬停要有 canHover 守卫");
  assert.match(src, /window\.matchMedia\("\(hover: hover\)"\)/, "守卫的判据是设备能力，不是窗口宽度");
  // 键盘必须走得到同一条路（只给 mouse 的浮层对键盘用户等于没有）
  assert.match(src, /@focus="canHover && reveal\(c\.emoji\)"/);
  assert.match(src, /@keydown\.esc="hide\(c\.emoji\)"/, "开着要有关闭出口");
  assert.match(src, /aria-expanded="openEmoji === c\.emoji"/);
});

test("名单是读不是写：它自己不许发回应；长按之后那次 click 必须吃掉", () => {
  const src = bar();
  // 只有按钮本身 emit toggle；tooltip 那一层必须一个 emit 都没有
  const panel = src.slice(src.indexOf("role=\"tooltip\""), src.indexOf("</template>"));
  assert.doesNotMatch(panel, /emit\(/, "点名单不该把回应切掉");
  assert.match(src, /if \(!props\.interactive \|\| canHover\) return;/, "长按只在'能点且无指针'的设备上抢这个手势");
  assert.match(src, /if \(heldEmoji === c\.emoji\) \{/, "长按触发后吞掉随后的 click");
  assert.match(src, /if \(!props\.interactive\) \{\s*\n?\s*togglePanel\(c\.emoji\);/, "不可点的那一侧（现在只有 1:1）点 = 看名单");
  assert.match(src, /onUnmounted\(clearPress\);/, "定时器不能留在卸载后的组件里");
});
