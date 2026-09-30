import { test } from "node:test";
import assert from "node:assert/strict";
import { evaluate, defaultCollectors } from "./sign-readiness.mjs";

/**
 * `scripts/sign-readiness.mjs` 的判定层（采集器整个注掉，只判"三态 → 就绪/缺口"这条规则）。
 *
 * 为什么这份判据值得存在：这个脚本的存在意义就是**别让"没判到"被读成"可以发"**
 * （§十那条老规矩在发版判定上的第二次落地）。所以核心不是"缺了几项"，
 * 而是 `unknown` 这一态绝不能通往 ready=true。
 */
const fact = (mac, notary, win) => ({
  macIdentity: { state: mac, evidence: "stub" },
  notarizeCreds: { state: notary, evidence: "stub" },
  winCert: { state: win, evidence: "stub" },
});

test("三项都确认有 ⇒ 就绪，且缺口为空", () => {
  const r = evaluate(fact("yes", "yes", "yes"));
  assert.equal(r.ready, true);
  assert.deepEqual(r.missing, []);
  assert.deepEqual(r.unknown, []);
});

test("有签名身份但没公证 ⇒ 不就绪，缺的就是 mac-notarize（Apple 只签名不公证照样被 Gatekeeper 拦）", () => {
  const r = evaluate(fact("yes", "no", "yes"));
  assert.equal(r.ready, false);
  assert.deepEqual(r.missing, ["mac-notarize"]);
});

test("有公证凭据但没签名身份 ⇒ 不就绪，缺 mac-signing（两半不互相顶替）", () => {
  const r = evaluate(fact("no", "yes", "yes"));
  assert.equal(r.ready, false);
  assert.deepEqual(r.missing, ["mac-signing"]);
});

test("只有 Windows 缺 ⇒ 不就绪，缺口只列 win-signing", () => {
  const r = evaluate(fact("yes", "yes", "no"));
  assert.equal(r.ready, false);
  assert.deepEqual(r.missing, ["win-signing"]);
});

test("全空 ⇒ 三项都进缺口（顺序稳定，报告里逐项可追）", () => {
  const r = evaluate(fact("no", "no", "no"));
  assert.equal(r.ready, false);
  assert.deepEqual(r.missing, ["mac-signing", "mac-notarize", "win-signing"]);
});

test("★ unknown 不算就绪：一台问不出来的机器不许把发版判成通过", () => {
  const r = evaluate(fact("yes", "unknown", "yes"));
  assert.equal(r.ready, false, "unknown 被当成就绪了 ⇒ 这份报告就是假绿灯");
  assert.deepEqual(r.missing, ["mac-notarize"]);
  assert.deepEqual(r.unknown, ["mac-notarize"], "unknown 要单独列出来：它说的是「这台机器问不出来」");
});

test("每项都带着「缺的是什么资产 + 接在哪一步」（不然下一个人还得重新问一遍）", () => {
  const r = evaluate(fact("no", "unknown", "no"));
  for (const i of r.items) {
    assert.ok(i.owner && i.owner.length > 6, `${i.key} 没说缺的是哪样资产`);
    assert.ok(i.whereToWire.includes("workflow"), `${i.key} 没说接在哪一步`);
  }
});

test("采集器：非 mac 平台时签名身份是「问不出来」，不是「没有证书」", () => {
  const c = defaultCollectors("win32");
  const got = c.macIdentity();
  assert.equal(got.state, "unknown", "把「这台机器问不出来」报成「没有证书」会把他带去重买证书");
  assert.match(got.evidence, /不是 mac/);
});
