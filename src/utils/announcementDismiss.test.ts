/**
 * 群公告「看完可临时关闭，公告更新后再出现」的判据（用户 2026-09-29 需求汇总八）
 * + 「群管理只保留一个清晰入口」的形状判据（同一条需求汇总四）。
 *
 * 前三条判纯函数（收起 / 换一条 / 按群分开 / 脏数据），后两条钉**接线**：
 * 横幅必须走"可见性"那一个家、收起必须落盘、头部不许再留第二枚改名入口。
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  ANNOUNCE_SEEN_KEY,
  isAnnounceDismissed,
  readAnnounceSeen,
  withAnnounceDismissed,
  writeAnnounceSeen,
} from "./announcementDismiss.ts";

/** 最小假存储：不依赖 node 有没有 localStorage。 */
function memStore(init: Record<string, string> = {}) {
  const data = new Map(Object.entries(init));
  return {
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => void data.set(k, v),
    dump: () => Object.fromEntries(data),
  };
}

test("收起的是『这一条』：同一条算已看，群主换新一条就不算", () => {
  let seen = readAnnounceSeen(memStore());
  assert.equal(isAnnounceDismissed(seen, "group:a", "m1"), false, "没收起过必须还出现");
  seen = withAnnounceDismissed(seen, "group:a", "m1");
  assert.equal(isAnnounceDismissed(seen, "group:a", "m1"), true);
  // 公告是覆盖式的：改一次 = 新 msgId ⇒ 横幅必须回来（这条是"直到公告更新/更换"的全部内容）
  assert.equal(isAnnounceDismissed(seen, "group:a", "m2"), false, "换了另一条还压着 = 那句需求没做到");
});

test("按群分开：A 群的收起不许影响 B 群", () => {
  const seen = withAnnounceDismissed({}, "group:a", "m1");
  assert.equal(isAnnounceDismissed(seen, "group:b", "m1"), false);
});

test("读写都往同一个键；脏数据 / 存储不可用一律退回空表", () => {
  const store = memStore();
  writeAnnounceSeen(store, withAnnounceDismissed({}, "group:a", "m1"));
  assert.equal(typeof store.dump()[ANNOUNCE_SEEN_KEY], "string", "写盘用的就是那个键");
  assert.deepEqual(readAnnounceSeen(memStore()), {}, "空的存储 ⇒ 空表");
  assert.deepEqual(readAnnounceSeen(null), {}, "存储不可用（null）也不许抛");
  for (const bad of ["{", "null", "[1,2]", '{"group:a":123}', '"字符串"']) {
    assert.deepEqual(
      readAnnounceSeen(memStore({ [ANNOUNCE_SEEN_KEY]: bad })),
      {},
      `脏值 ${bad} 必须退回空表（宁可可再看一次，不许把横幅判没）`,
    );
  }
});

test("收起不改入参（那份是 Vue 的 ref，就地改会让旧值跟着变）", () => {
  const base = { "group:a": "m1" };
  withAnnounceDismissed(base, "group:b", "m9");
  assert.deepEqual(base, { "group:a": "m1" });
});

const chatWindow = () => readFileSync(new URL("../components/ChatWindow.vue", import.meta.url), "utf8");
const chatHeader = () => readFileSync(new URL("../components/chat/ChatHeader.vue", import.meta.url), "utf8");
const panel = () => readFileSync(new URL("../components/GroupMemberPanel.vue", import.meta.url), "utf8");

test("横幅只经『可见性』那一个家，收起按钮存在且真的落盘", () => {
  const src = chatWindow();
  assert.match(src, /v-if="isGroup && announcementVisible"/, "横幅必须走可见性判定，不是裸公告");
  assert.doesNotMatch(src, /v-if="isGroup && announcement"/, "绕过判定的那条 v-if 是第二个家");
  assert.match(src, /writeAnnounceSeen\(localStorage, next\)/, "收起必须写盘，否则重启就回来了");
  assert.match(src, /const announceSeen = ref<AnnounceSeenMap>\(readAnnounceSeen\(localStorage\)\)/);
});

test("群管理在聊天头部只剩一个入口；改名能力收在弹窗里", () => {
  const header = chatHeader();
  assert.doesNotMatch(header, /emit\("rename"\)|emit\('rename'\)/, "第二枚改名入口必须消失");
  assert.doesNotMatch(header, /Pencil/, "改名的图标也不再出现在头部");
  assert.doesNotMatch(chatWindow(), /@rename=|:can-rename=/, "调用方那份重复绑定也要一起拆掉");
  // 能力没丢：弹窗里仍有改名输入 + 保存
  assert.match(panel(), /@click="saveName"/);
  assert.match(panel(), /t\("group\.rename\.title"\)/);
});

test("群详情里普通成员看得到公告正文（以前整段挂在 isOwner 下）", () => {
  const src = panel();
  assert.match(src, /v-else-if="currentAnnouncement\?\.text"/, "非群主必须有一条读正文的路");
  assert.match(src, /t\("group\.announceEmpty"\)/, "没有公告时要说『暂无』，不是空白一格");
  assert.match(src, /t\('group\.manageTitle'/, "标题要说清这一格管的是什么，不只『成员 N』");
});
