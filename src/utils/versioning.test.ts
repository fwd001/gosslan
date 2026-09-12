/**
 * 版本号规则的单测（纯函数部分）。
 *
 * 规则本身写在 `docs/VERSIONING.md`，实现在 `scripts/semver.mjs`；
 * 这里钉住"什么算小/中/大"与"两个累加口径"的语义 —— 它们是版本号与发布台账的唯一依据，
 * 一旦悄悄改动，历史台账和门禁都会对不上。
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  accumulate,
  bumpVersion,
  classifyCommit,
  compareVersion,
  parseBumpTrailer,
  parseSubject,
  requiredLevel,
  TYPE_LEVEL,
  BUMP_TRAILER,
} from "../../scripts/semver.mjs";

test("解析 conventional commit（类型/范围/破坏性标记）", () => {
  assert.deepEqual(parseSubject("feat(settings): 加个开关"), {
    type: "feat",
    scope: "settings",
    breaking: false,
    summary: "加个开关",
  });
  assert.equal(parseSubject("refactor(window)!: 换架构").breaking, true);
  assert.equal(parseSubject("随手改改").type, "other");
});

test("定级：小 / 中 / 大（判据必须确定性）", () => {
  assert.equal(classifyCommit({ subject: "fix: 修个崩溃" }).level, "patch");
  assert.equal(classifyCommit({ subject: "docs: 补注释" }).level, "patch");
  assert.equal(classifyCommit({ subject: "feat: 新增跨网段连接 UI" }).level, "minor");
  assert.equal(classifyCommit({ subject: "perf: 列表滚动更快" }).level, "minor");
  assert.equal(classifyCommit({ subject: "feat: 换架构" }).level, "minor"); // 没有线索词 → 中
  assert.equal(
    classifyCommit({ subject: "refactor(窗口): 三个窗口各自一个入口", churn: 2398, files: 24 }).level,
    "major",
  );
  assert.equal(classifyCommit({ subject: "feat!: 不兼容的协议变更", churn: 10 }).level, "major");
  // 架构线索 + 小改动 → 不升级（避免"改个注释就大版本 +1"）
  assert.equal(
    classifyCommit({ subject: "feat: 协议注释微调", churn: 3, files: 1 }).level,
    "minor",
  );
});

test("版本累加：两个口径（逐提交 / 一次发布取最高档）", () => {
  assert.equal(bumpVersion("2.1.2", "patch"), "2.1.3");
  assert.equal(bumpVersion("2.1.2", "minor"), "2.2.0");
  assert.equal(bumpVersion("2.1.2", "major"), "3.0.0");
  // 逐提交字面累加（只用于台账）
  assert.equal(accumulate("2.1.2", ["patch", "major", "minor", "patch"]), "3.1.1");
  // 标准做法：一次发布取最高档
  assert.equal(requiredLevel(["patch", "minor", "patch"]), "minor");
  assert.equal(requiredLevel(["patch", "major", "minor"]), "major");
  assert.equal(requiredLevel([]), null);
});

test("版本比较（门禁用）", () => {
  assert.equal(compareVersion("3.0.0", "2.1.2"), 1);
  assert.equal(compareVersion("2.1.2", "2.1.2"), 0);
  assert.equal(compareVersion("2.1.2", "2.2.0"), -1);
});

test("提交信息里的 Version-Bump 声明", () => {
  assert.equal(parseBumpTrailer(`feat: x\n\n${BUMP_TRAILER}: minor`), "minor");
  assert.equal(parseBumpTrailer("feat: x"), null);
  assert.equal(parseBumpTrailer(`${BUMP_TRAILER}: huge`), null, "只认三档");
});

test("类型到级别的默认映射完整（新增类型必须显式表态）", () => {
  for (const [type, level] of Object.entries(TYPE_LEVEL)) {
    assert.ok(["patch", "minor", "major"].includes(level), `${type} → ${level} 非法`);
  }
  assert.equal(TYPE_LEVEL.feat, "minor");
  assert.equal(TYPE_LEVEL.fix, "patch");
});
