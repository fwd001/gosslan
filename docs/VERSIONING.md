# 版本号规则（从 2026-09-12 起强制执行）

> 结论先说：**未发布的一批提交里最高档是什么，这次发布就提升哪一位**；
> 每个提交必须在提交信息里声明自己的档位（`Version-Bump:`），由 `npm run version:check` 把门。
> 依据：SemVer 的"一次发布取最高档" + conventional commits 的类型约定。

## 1. 三档判据（小 / 中 / 大）

| 档位 | 版本变化 | 什么算 | 例子 |
|---|---|---|---|
| **小**（patch） | `x.y.Z+1` | 缺陷修复；非功能性改动（docs / test / chore / build / ci / style）；小幅无行为变化的优化 | `fix(settings): 主题点了又跳回去`、`docs: 补注释`、`test: 补护栏` |
| **中**（minor） | `x.Y+1.0` | 一个完整的新能力，用户可感知的改进（`feat`、`perf`）；向后兼容 | `feat: 跨网段连接 UI`、`perf(logs): 隐藏时不再每 2s 拉日志` |
| **大**（major） | `X+1.0.0` | 架构级/协议级改动、新平台支持、破坏性变更（提交带 `!` 或 `BREAKING CHANGE`） | `refactor(window): 三个窗口各自一个入口`、`feat: 引入统一 Mesh 网络模型`、`feat: Android 外设角色` |

判据是**确定性**的（同输入必同档），实现在 `scripts/semver.mjs` 的 `classifyCommit()`：

1. 提交带 `!` ⇒ **大**；
2. `feat/refactor/perf/build` + 命中架构/协议线索词（协议、架构、ADR、窗口、蓝牙、BLE、mesh、中继、传输、不兼容…）+ 改动 ≥150 行 ⇒ **大**；
3. `feat` ⇒ **中**；`perf` ⇒ **中**；
4. 其余（fix / docs / test / chore / build / ci / style / 未分类）⇒ **小**。

## 2. 怎么"累加"：两个口径，别混用

- **逐提交累加**（字面执行"每次提交进一位"）：`npm run version:classify` / 台账里的"累计版本"列。
  仅用于**审计**。把这套规则套到历史 backlog（184 个提交、其中 23 个大）会算出 `25.1.2` ——
  它既不表达"这次发布有多大"，也和安装包/后端的版本语义脱节，所以**不用它定版本**。
- **一次发布取最高档**（SemVer 标准做法，**实际采用**）：`requiredLevel()` + `version:release`。
  同一批 backlog 的最高档是 `major` ⇒ 本次发布 `2.1.2 → 3.0.0`。

## 3. 日常怎么做（每个提交都要做）

1. 按上表给自己的改动定档；
2. 提交信息末尾加一行 **`Version-Bump: patch|minor|major`**（这就是"版本号提升约束"的落点：
   它让每个提交都被记账，而不是靠人记得去改版本）；
3. 提交前跑 **`npm run version:check`**：它会
   - 从**上一次版本提升提交**起，收集所有提交并计算最高档；
   - 若 `当前版本 < bump(当前版本, 最高档)` ⇒ 失败，提示你跑 `npm run version:release`；
   - 若某个提交缺 `Version-Bump:` 或写错档位 ⇒ 失败并逐条指出。
4. 攒够一批（或要发版时）跑 **`npm run version:release`**：按最高档一次性提升
   `package.json` / `Cargo.toml` / `tauri.conf.json` / `package-lock.json` 并落 CHANGELOG 版本小节。

## 4. 现在的台账与数字

- 全量分类台账：`docs/version-ledger.md`（184 个提交：**23 大 / 45 中 / 116 小**，由 `npm run version:ledger` 生成）；
- 逐提交字面累加：`2.1.2 → 25.1.2`（仅审计口径）；
- **本次实际发布：`2.1.2 → 3.0.0`**（最高档 = 大：多窗口架构重做、Mesh/中继协议分层、Android BLE 外设支持等）。
