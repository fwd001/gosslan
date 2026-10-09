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
| `~/.local/bin/aoci`（本机） | CLI + `aoci mcp` stdio server，v0.1.0-rc18 | — | 必须重装 |
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
**写条目只有 MCP 一条路**。要「重新索引」先看 §8 —— 那组会被拒的命令有一整张对照表。

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

## 8. 重新索引：走哪条路、会撞到什么、怎么处理

> 本节每条命令与每句报错原话都在 **v0.1.0-rc18 + 本仓现状**下实跑过（2026-10-08）。数字会漂，跑一遍再说。

「重新索引」其实是三件不同的事。先认清要哪一件，再敲命令——代价和可回退性完全不同。

| 你要的 | 走这条 | 代价 | 能不能回退 |
|---|---|---|---|
| 索引跟上这次改动（日常，几乎总是这件） | MCP `aoci_maintain` 签批次 → `aoci_update_entry` 整批提交（见 §2、§3） | 只重写变动的那几条 Entry | ✅ 正式字节全在 Git 里，`git checkout` 就回去 |
| 收录范围或基线指纹要刷新（改了 exclude、大批增删文件、换了二进制大版本） | 只读用 `aoci scope preview`；真要落盘是 `aoci scope plan` → `aoci scope apply`（一笔事务，配 `scope resume`、`scope rollback`） | 整批收录身份一起动 | ✅ 事务留精确前像 |
| 全部条目推倒重写 | **本仓现在没有可用的「一键全量重建」入口**——§8.2 那张表就是实测被拒的清单。要么按第一行逐批 maintain，要么在仓库外新建一份走 Fresh Bootstrap | 最贵，且要人工批准 | — |

### 8.1 动手前先跑三条只读检查

```bash
aoci --repo . status                 # 条目数 / 基线文件数 / 基线更新时间
aoci --repo . check --json | python3 -c 'import json,sys,collections;f=json.load(sys.stdin)["findings"];c=collections.Counter(x["code"] for x in f);print("  ".join(f"{k}={v}" for k,v in c.most_common()))'
find .aoci/transactions -mindepth 1 -maxdepth 1 -type d ! -name history -exec sh -c 'test -f "$1/result-applied.json" || echo "PENDING: $1"' _ {} \;
```

第三条**没有输出**才继续。它报 `PENDING: <目录>` 说明有一笔没走完的事务——先 `aoci scope status`，
再决定 `scope resume` 还是 `scope rollback`，**别**在上面再叠一笔新事务。
⚠️ **这条 find 是我按目录形状搭的量具，不是工具给的权威**：rc18 的 `aoci scope status --json` 顶层并没有
"未决事务"这一格（现读它的键能证实；里面那个 `observed_pending_review` 是人工复核计数，另一回事）。
判据取的是"走完的事务目录里有 `result-applied.json`（内含 `status: applied`）"——这半句是实读；
"没走完时这个文件不存在"是按文件名与目录里另有 `staging/` 前像**推**的（我没在本仓造出过未走完的事务）。
真报出 `PENDING` 时以 `aoci scope status` / `scope resume` 的输出为准，别只信这条 find。
顺带一条实测：`aoci scope status --transaction <那笔已应用事务的 id>` 会退 2 报 `managed_scope_transaction_invalid`
——那是"已应用的事务不按 id 查"，不是故障。
那条 find 排掉 `.aoci/transactions/history/`：那里装的是每道工序的前后像绑定件（现读一个文件的键：
`pre_index_sha256`、`post_index_sha256`、`baseline_pre_sha256`、`assets`、`guards`），不是待决状态。

### 8.2 会被拒的命令——拒的是「这条路不适用于本仓」，不是工具坏了

本仓是 Volumes v1 布局、基线已建成。下面逐条在 rc18 上实跑过：

| 命令 | 你会看到的 | 该怎么办 |
|---|---|---|
| `aoci init …` | 退 3、`error_code=config`、「该命令或兼容写入路径不支持修改Volumes v1正式认知」 | 不靠 init 也接得上：`aoci doctor` 直接报「Qoder CLI MCP（`.mcp.json`，与 Claude Code 共用）：已配置」。真判据见 §8.6 |
| `aoci scan` | 退 3、「基线已存在: 重建将以当前磁盘状态整体覆盖(未处理的漂移会被洗白)。确认请加 --force」 | **别顺手加 `--force`**，那句括号是真话。先用 §8.1 的量具读现状 |
| `aoci scan --dry-run` | **也退 3**、「已有Managed Scope Baseline不能通过scan --force重建；请使用scope preview/apply」 | 在这个仓 `--dry-run` 不是安全的只读 scan；只读收录权威只有 `aoci scope preview` |
| `aoci scan --force` | 会整体覆盖基线指纹 | 只有你确实要「承认当前磁盘状态为新起点、放弃未处理漂移」时才用，且先把 `aoci.*` 与 `.aoci/baseline.json` 提交存好 |
| `aoci index inventory`、`index update`、`index score`、`index agent plan`、`status --deep` | 全是退 3、同一条 `config` 原话 | 这一整组是 Legacy 布局的工序，Volumes v1 上不通。等价物：`scope preview`、`check --json`、`index agent guide --agent <宿主名>` |
| `aoci baseline scope plan` | 退 2、`error_code=baseline_scope_invalid`、「baseline_scope_managed_scope_unsupported」 | 这条也不是本仓的路（`aoci baseline --help` 底下只有 `scope` 一个子命令）。**别把它当 `scan` 的替身** |

⚠️ `aoci index entries check` 报的是另一句：「未找到Entries草稿；请先运行aoci index build: 草稿区内没有符合条件的 run」
——那是草稿区空着的正常提示，不属于上面那条 `config` 拒绝。但起草这一步要端点：`aoci index build --help`
原话是"对目标文件调用用户配置端点起草单行Entry"，而本仓 `ai.enabled=False`
（现读 `python3 -c "import json;print(json.load(open('.aoci/config.json'))['ai']['enabled'])"`，返回 False；
`aoci ai status` 也能看，`aoci ai setup` 才是开它的口）。**我没有实跑过 `index build`**，所以这句是
「帮助 + 配置现读」两条拼出来的：真要启用得先配端点，否则条目只能由宿主里的模型写。

### 8.3 finding 分五类，只有两类是活

`aoci --repo . check --json` 的 `findings[]` 按 `code` 分类（2026-10-08 实跑：`code_skipped=282`、
`code_stale=19`、`code_missing=3`、`code_unbaselined=3`、`observed_pending=1`，合计 308）：

| code | 是什么 | 谁处理 |
|---|---|---|
| `code_skipped` | 合同**免条目**：二进制（前 8000 字节含 NUL）或超 1 MiB。分 cause 现读 `binary` 与 `oversize` | 不用处理，也别给它补条目（判据见 §6） |
| `code_stale` | 已收录、但源码变了 ⇒ 旧条目过期 | 重写这些文件的 Entry（§8 第一行那条路） |
| `code_missing` | 磁盘有、索引没有 ⇒ 新文件 | 新建条目 |
| `code_unbaselined` | 没进基线指纹的新文件 | 与 `code_missing` 落在同一批文件时，随这次维护批次一起解决 |
| `observed_pending` | **人工复核门**：机器不替你说「我看过并认可了」 | 由**负责人**跑 `aoci scope acknowledge`；Agent 不该自己跑它 |

`next_action=blocked` 时先分类再动手——一堆 `code_skipped` 能把「N 项 finding」撑得很大却没人要干活。
另注：`aoci scope preview` 的 `drift.*` 与 `check` 的 `findings` 口径不同，同一个文件在两处可能归成不同类
（实测 `docs/notes/changelog-archive.md` 在 preview 的 `unbaselined` 里、在 check 里是 `code_skipped/oversize`），
**提交门禁认 `check`**。

### 8.4 写条目被拒时看这张（配额与格式的细节见 §5）

| 你会看到的 | 原因 | 出口 |
|---|---|---|
| `entry_field_budget_exceeded` | S 段超了该 C 档的 token 预算（字节/3） | 按证据把 C 提一档，或压缩语义；**别机械截断** |
| `object_tag_dictionary_violation` | 标签用了字典里没有的位（例如 B=T） | 现读字典：MCP `aoci_header` |
| `fras_structure_invalid` 且 `canonical_object_line=false` | 走 CLI `aoci update-entry` 时 `--entry` 只给了 `F:` 与 `R:` 那几段，没给完整规范对象行 | CLI 要交整行：以 `basename[TAG]: ` 开头，四段按竖线分隔，另带 `--source-sha256` |
| `code_candidate_source_sha256_mismatch` | 自己算的哈希与机器签发的绑定不是同一串字节 | 用 `aoci_maintain` 响应里给的那个值，**永远不要自己 hash** |
| 「重复批次: 正式索引零写入」（退 0 但没写） | 同文本重传会被拒 | 过期条目**不能靠原文重绑**：要么真改内容，要么等机器重新签发候选 |
| `status=repair_required` 且 `formal_writes_started=false` | 批里任一候选越界 ⇒ 整批零写入 | 改完**整批重提**，不要拆开逐条 |
| `impact_candidate_fras_invalid`（退 3） | F/R/A 原样、只把 S 整段换掉且改动幅度过大 | 优先在原文上**追加**新事实，而不是整段替换 |

### 8.5 什么才算「重新索引做完了」

```bash
aoci --repo . verify --json                                # structure_valid 与 governance_aligned 都要 true
aoci --repo . check                                        # 退 0 = 五净
aoci --repo . index agent guide --agent qoder --json       # stage=aligned、complete=true、next_action=none
```

`structure_valid=true` 而 `governance_aligned=false` 的含义是「形状对、但还没收尾」，不是命令坏了
（2026-10-08 本仓就是这一态：`verify` 退 1、两个布尔一真一假）。

### 8.6 换了二进制大版本以后（例如 rc17 → rc18）

先证明「新二进制读得懂旧索引，且收录范围没变」，再谈维护：

```bash
aoci --repo . status                       # 能报出条目数与基线文件数 = 读得懂旧资产
python3 - <<'PY'                           # 收录权威有没有漂
import json, subprocess
b = json.load(open('.aoci/baseline.json'))['managed_scope']
p = json.loads(subprocess.run(['aoci', '--repo', '.', 'scope', 'preview', '--json'],
                              capture_output=True, text=True).stdout)
print('policy', b['policy_identity'] == p['desired_policy_identity'],
      'budget', b['budget_policy_identity'] == p['desired_budget_identity'])
PY
```

两个都 `True` ⇒ 升级只换工具、不动认知（rc17→rc18 实测就是这样）。出现 `False` ⇒ 走 §8 第二行的事务，
**别**直接 `scan --force`。

MCP 那条 server 重启后能不能起，不用赌——照抄 `.mcp.json` 里那条命令做一次握手：

```bash
( printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}' \
                  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
                  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
  sleep 8 ) \
  | ~/.local/bin/aoci --repo "$PWD" mcp 2>/dev/null \
  | python3 -c 'import json,sys
for l in sys.stdin:
    l=l.strip()
    if not l: continue
    d=json.loads(l)
    if d.get("id")==1: print("server:", d["result"]["serverInfo"])
    if d.get("id")==2: print("tools:", len(d["result"]["tools"]), sorted(t["name"] for t in d["result"]["tools"]))'
```

期望：`server` 的版本号等于你刚装的二进制、`tools` 数到 9 个 `aoci_*`（§3 那张表）。
⚠️ **那个 `sleep 8` 是必需的，不是等 CI**：server 起来要先把整仓收录跑一遍，stdin 提前 EOF 会得到
`error_code=command_failed`、`保留的机器事实：EOF`、零输出——本仓实测 `sleep 2` 会间歇性这样，
**别把它读成「MCP 配置坏了」**。宿主里没出现 `aoci_*` 就先跑这段，再怀疑工具。

### 8.7 三个「静默没干活」的坑

- ❌ 把 `aoci.txt`、`aoci.meta.txt`、`aoci.code.txt`、`AGENTS.md` 写进 `.gitignore` 或 `.git/info/exclude`。
  收录按 Git 的忽略权威取文件，**被忽略的会被静默跳过，索引永远建不起来**。现读：
  `git check-ignore -v aoci.txt aoci.meta.txt aoci.code.txt AGENTS.md`——有输出就是踩了。
  init 自己写的宿主项（`.mcp.json`、`.codex/config.toml`）保持原样，那些内嵌本机绝对路径，本来就不该入库。
- ❌ 在 `/tmp` 下 clone 一份来验证可移植性：macOS 的 `/tmp` 是符号链接，AOCI 对这种 Git 边界失败关闭（见 §1）。
- ❌ 看到 `aoci check` 退 1 就判「索引坏了」：先按 §8.3 数那五类各有多少，再看 `observed_pending`
  是不是在等**你**复核——那一格机器永远等不出结果。
