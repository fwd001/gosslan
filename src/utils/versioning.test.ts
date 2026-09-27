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
  declaresBump,
  filterUnpushed,
  isAppCodePath,
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
  // 向后兼容的大改动**不是** major：规模/线索词不参与定档（SemVer 2.0.0）
  assert.equal(
    classifyCommit({ subject: "refactor(窗口): 三个窗口各自一个入口", churn: 2398, files: 24 }).level,
    "patch",
  );
  assert.equal(
    classifyCommit({ subject: "feat: 引入统一 Mesh 网络模型", churn: 3000, files: 40 }).level,
    "minor",
  );
  assert.equal(classifyCommit({ subject: "feat!: 不兼容的协议变更", churn: 10 }).level, "major");
  // Conventional Commits 的另一种等价声明：正文 BREAKING CHANGE: footer
  assert.equal(
    classifyCommit({
      subject: "feat: 换数据模型",
      message: "feat: 换数据模型\n\nBREAKING CHANGE: 旧库不兼容",
      churn: 10,
    }).level,
    "major",
  );
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

test("声明门禁只看未推送提交（#123：历史 151 条不该天天响）", () => {
  const rows = [{ short: "aaaa111" }, { short: "bbb2222" }, { short: "ccc3333" }];
  // 已推送的那条要被排除
  assert.deepEqual(
    filterUnpushed(rows, new Set(["bbb2222"])).map((r) => r.short),
    ["aaaa111", "ccc3333"],
  );
  // 没有 upstream（pushedSet 为空）⇒ 回退成全范围，不许把门禁静默变成"什么都不判"
  assert.equal(filterUnpushed(rows, new Set()).length, 3);
  assert.deepEqual(filterUnpushed([], new Set(["bbb2222"])), []);
});

test("零影响声明用现有 [plan] 标记，其余提交必须写 Version-Bump（#123）", () => {
  // 文档/判据类提交：标题带 [plan] ⇒ 视为已声明"不动版本"
  assert.equal(declaresBump({ subject: "docs(x): 改口 [plan]", message: "docs(x): 改口 [plan]", level: "patch" }), true);
  // 代码类提交：必须写 trailer，且档位要和自己被定级的一致
  assert.equal(
    declaresBump({ subject: "fix(y): 修崩溃", message: "fix(y): 修崩溃\n\nVersion-Bump: patch", level: "patch" }),
    true,
  );
  assert.equal(declaresBump({ subject: "fix(y): 修崩溃", message: "fix(y): 修崩溃", level: "patch" }), false);
  assert.equal(
    declaresBump({ subject: "feat(y): 新增", message: "feat(y): 新增\n\nVersion-Bump: patch", level: "minor" }),
    false, // 被定级为 minor，声明 patch ⇒ 仍算没声明对
  );
});

test("测试文件不算应用代码（src/*.test.ts 是给门禁跑的，不发出去）", () => {
  // 口径：只有"会被打包发出去的应用码"才算 —— 前端测试与 Rust 测试都不算。
  assert.equal(isAppCodePath("src/utils/buildConfig.test.ts"), false);
  assert.equal(isAppCodePath("src/style.css"), true);
  assert.equal(isAppCodePath("src-tauri/src/db/settings.rs"), true);
  assert.equal(isAppCodePath("src-tauri/src/db/migration_tests.rs"), false);
  assert.equal(isAppCodePath("docs/VERSIONING.md"), false);
});

test("[plan] 只能豁免**不动应用代码**的提交（否则声明门禁就是装饰）", () => {
  // 不动 src/ 与 src-tauri/src/ ⇒ [plan] 视为零影响
  assert.equal(
    declaresBump({ subject: "docs(x): 改口 [plan]", message: "docs(x): 改口 [plan]", level: "patch", touchesCode: false }),
    true,
  );
  // 动了应用代码 ⇒ 光写 [plan] 不算，必须有 Version-Bump 且档位自洽
  assert.equal(
    declaresBump({ subject: "fix(x): 修崩溃 [plan]", message: "fix(x): 修崩溃 [plan]", level: "patch", touchesCode: true }),
    false,
  );
  assert.equal(
    declaresBump({ subject: "fix(x): 修崩溃", message: "fix(x): 修崩溃\n\nVersion-Bump: patch", level: "patch", touchesCode: true }),
    true,
  );
});
