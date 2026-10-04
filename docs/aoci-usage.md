# AOCI 仓库认知层使用手册（本仓实况）

> 适用范围：本仓库接入的 **AOCI-CODE**（`github.com/aoci-spec/aoci-code`）。
> 本手册只讲**怎么用**；接入的来龙去脉见 `CHANGELOG.md` 的 `chore(aoci)` 与 `fix(android)` 两条。
> ⚠️ 文中每个数字都挂了一条现算命令 —— 别抄本文的数字，跑一遍再说。

## 0. 一句话

AOCI 给本仓库维护一层**可版本化的仓库级认知**：`aoci.code.txt` 里每个受管理对象一条 Entry，
用符号标签 + F/R/A/S 说清「它的职责 / 强关系 / 对外契约 / 改它必须知道的非显然约束」。
它是**问了才答**的层：不读代码就答不出语义，写语义的必须是当前那个模型，AOCI 只负责签发批次、
绑哈希、拒越界。**它不是全自动守护进程**（详见 §2）。

## 1. 装在哪 / 哪些入库 / 哪些不入库

| 位置 | 内容 | 入库 | 换机器 |
|---|---|---|---|
| `~/.local/bin/aoci`（本机） | CLI + `aoci mcp` stdio server，v0.1.0-rc17 | — | 必须重装 |
| `aoci.txt` | Root manifest（Volumes v1，登记两个 Volume） | ✅ | 直接可用 |
| `aoci.meta.txt` | 标签字典 + FRAS 规则（**判据权威**） | ✅ | 直接可用 |
| `aoci.code.txt` | 全部 Code Entry | ✅ | 直接可用 |
| `AGENTS.md` | `<!-- aoci:begin -->` 那段使用合同 | ✅ | 直接可用 |
| `.aoci/{.gitignore,baseline.json,config.json}` | 指纹基线 + 治理配置 | ✅ | 直接可用 |
| `.aoci/{ledger.jsonl,governance/,transactions/,drafts/,verify_history/}` | 本机运行态 | ❌（`.aoci/.gitignore` 兜住） | 各机自产 |
| `.mcp.json` / `.codex/config.toml` / `.qoder/` | 宿主接入配置，**内嵌本机绝对路径** | ❌ | 每台重配 |

**换机器/新同事的两步**：装二进制 → 把 `aoci mcp` 写进你所用宿主的 MCP 配置（绝对路径）。
不带 `--repo` 时服务端从当前目录向上找仓库根，所以配一条全局条目对所有仓库都管用。

判「这条索引在我这台机器上到底认不认」：

```bash
aoci --repo . doctor          # 看「索引解析」那一行的条目数与警告数
aoci --repo . check           # 退 0 = 五净；这是提交前该看的一条
```

⚠️ **别拿 `/tmp` 下的副本证明"别人机器上用不了"**：macOS 的 `/tmp` 是 `/private/tmp` 的符号链接，
AOCI 对这种 Git 边界失败关闭（`business_source_manifest_invalid: safe_inventory_git_boundary_mismatch`）。
要验可移植性就 clone 到一个**非符号链接**的真实路径，`check` / `verify` / `guide` 三步都该绿。

## 2. 谁触发、什么时候触发

| 时机 | 动作 | 不做会怎样 |
|---|---|---|
| 开新会话、上下文里没有这份认知 | `aoci_rules` → `aoci_overview`（完整索引，分块续传） | 模型凭印象改核心链路 |
| **上下文被压缩过**（宿主注入的压缩摘要也算） | 用 `refresh_reasons=["context_compaction"]` + 新 `refresh_event_id` 重新跑一次完整 `aoci_overview` | 旧认知按合同算**不可靠**，不能拿来下结论 |
| 本次任务里受管理对象达到**最终稳定状态**后 | `aoci_maintain` 一次（不是每次中间提交都跑）→ 按批次写条目 → `aoci_update_entry` | 索引变陈旧，`aoci check` 退 1 |
| commit / push / build | **什么都不发生**：`.aoci/hooks` 是空的、`hook_strict=false`，门禁与 CI 里一行 aoci 都没接 | —— |

`automation.mode=auto`（`.aoci/config.json`，现读 `python3 -c "import json;print(json.load(open('.aoci/config.json'))['automation'])"`）
**只**意味着安全的常规写入不需要你在终端敲确认词，**不**意味着它会自己找活干。

## 3. MCP 工具速查（9 个，宿主里以 `aoci_*` 出现）

| 工具 | 什么时候用 |
|---|---|
| `aoci_rules` | 每次开工先拿当前版本的会话合同（口径会随版本变） |
| `aoci_overview` | 建立/恢复整份认知；`continuation_required` 时**必须**原样跟着 `next_cursor` 跑到 `completed=true`；局部不确定时可 `check_only=true` 只要紧凑事实 |
| `aoci_get_entries` | 只要指定对象/指定 scope 的条目，不要整份 |
| `aoci_search` | 按关键词找对象（找入口用，不替代 overview） |
| `aoci_maintain` | 收尾主入口：报漂移、**签发批次**（每批约 25–30 条，被 24KiB 运输预算卡的）；`intent="cognition_optimization"` + `object_refs=["code:<路径>",…]` 是**重写已应用条目**的唯一合法通道（要求仓库已 aligned） |
| `aoci_update_entry` | 整批提交：`code_batch_id` 必须等于 `code_plan.batch_id`（≠ `authoring_batch.batch_identity`），每项原样带 `source_sha256` 与 `candidate_id` |
| `aoci_remove_entry` | 人工策展裁决删条目 |
| `aoci_header` | 读正式 Meta/Root 精确原文（写条目前必须读，字典以它为准） |
| `aoci_report` | 证据不足时登记待办，**不猜写**（未 aligned 时返回 `volume_read_only`） |

## 4. CLI 速查（人读，全只读）

```bash
aoci --repo . check                 # 提交前聚合检查（结论 + S 覆盖）
aoci --repo . verify --json         # 治理状态（structure_valid / governance_aligned）
aoci --repo . doctor                # 环境与 agent 接入诊断
aoci --repo . source manifest       # 确定性业务源码身份清单（条目 R 该指谁，以它为权威集合）
aoci --repo . index agent guide --agent <你的宿主名> --json   # 终态证明（stage / complete / next_action）
aoci --repo . ui --detach --json    # 本地只读状态页；aoci ui --repo . --stop 关掉
```

两个坑：① **没有**顶层 `aoci guide` 命令，Guide 在 `aoci index agent guide` 且 `--agent` 必填；
② CLI 的写入通道（`index agent plan/stage`）在 Volumes v1 下直接拒（`error_code=config`），
**写条目只有 MCP 一条路**。

收尾三件套（每轮维护跑完都要过）：`verify` → `check` → `index agent guide`，
直到 `guide` 返回 `stage=aligned`、`complete=true`、`next_action=none`。

## 5. 一条 Entry 长什么样

```
basename.ext[AB C [D] E]: F:职责 | R:code:相对路径,… | A:对外契约 | S:非显然约束
```

- 标签四位（第三、四位可省）：**A** 层 ∈ `ACDEFIKLMOPRSTXZ`（无 U）；**B** 域 ∈ `ABCDEGHILMNOPQRSUVWZ`
  （**无 T/F/K/J**，B=Q 只给测试与质量基础设施本身）；**C** 重要度 1–9；**E** 规模 L>400 / M 200–400 /
  S 100–200 / T<100 行。字典的权威现读口＝MCP 工具 `aoci_header`（或 `aoci_maintain` 响应里的
  `authoring_meta` 字段）；**CLI 没有 `aoci header` 这条命令**（会报 `command_failed`）。
- **S 有两层配额，两都要过**：① token＝UTF-8 字节/3，上限 C9=200、C8=140、C5–C7=80、C1–C4=40；
  ② rune＝字符数，`C9-8≤600 C7-4≤200 C3-1≤50`。F≤160、R≤360/8 项、A≤400/6 项。
  上限的机器口径＝`aoci_maintain` 响应里的 `governance.budget.s`（别照抄本节）。
- 压不进配额时**合法出口是按证据提一档 C**，不许机械截断；没看过实现就写 `S:-` 与编造同样坏。
- `R` 只接受 `code:<仓库相对路径>` 的**被跟踪托管对象**：目录不算、未跟踪不算。复跑：
  逐条取 `]: ` 后第 2 段按 `,` 拆开，`os.path.exists` + 不是目录 + `git ls-files --` 非空。
- 任一候选越界 ⇒ `status=repair_required` 且 `formal_writes_started=false` ⇒ **零写入，整批改完整批重提**。

## 6. 当前实况（现算，别抄）

```bash
grep -c ': F:' aoci.code.txt                                    # Entry 条数
aoci --repo . index agent guide --agent qoder --json \
  | python3 -c 'import json,sys;d=json.load(sys.stdin);g=d["governance"];print(d["stage"],g["code_source_count"],g["code_entry_count"],g["budget"]["whole_index_tokens"],g["budget"]["max_tokens"])'
# 依次是：对齐状态 / 源码对象数 / 条目数 / 整索引 token / token 上限
```

判「某个文件为什么没有条目」：它若是空文件、二进制（前 8000 字节含 NUL）或超 1 MiB，
按合同**免条目**，`aoci check --json` 里落在 `code_drift.skipped`（原因码 `code_skipped`），不是漏写。
给这种对象补条目只有一条合法路：直接 `aoci_update_entry` 带它的 `source_sha256`。

## 7. 别这么做

- ❌ 手改 `aoci.code.txt`（正式字节由事务写；手改会被判漂移，且绕过哈希绑定）。
- ❌ 用路径/文件名/AST/依赖扫描/正则/模板拼语义 —— 语义必须来自模型对**真实证据**的阅读理解。
- ❌ 把 `aoci.txt` 之类内嵌本机绝对路径的宿主配置（`.mcp.json`、`.qoder/`）提交上去。
- ❌ 每次改完一个文件就 maintain 一次；本轮任务内等**最终稳定状态**再收尾一次。
- ❌ 为满足 `max_entries` 缩减索引覆盖或自己截批次 —— `remaining` 非零就在当前批 Apply 后重新
  `aoci_maintain`，从新 preimage 继续。
