# Gosslan 稳定版工作地图（stability-roadmap）

> 本文件是「最终稳定版」阶段的**唯一工作地图**。它的用途不是描述愿景，而是回答三个问题：
> **现在真正被机器证明到哪一步／接下来按什么顺序做／每一项做完的判据是什么。**
>
> 建立于 2026-09-25（总指令§二第一阶段产物）。**本阶段只审计，不改生产代码。**
> 任何与总指令冲突的判断，以总指令为准；任何与本文件数字冲突的**实测结果**，以实测为准并回来改本文件。

---

## 0. 判定口径

**Definition of Done（覆盖一切）**：不是"测试通过"，而是
「用户实际操作路径、边界、异常恢复、跨实例通信、数据一致性、UI 反馈、文档契约
全部能被机器反复验证，并长期保持通过」。

**任务分级**：A 必须修复（用户可感知故障／数据或消息丢失／状态错误／卡死／边界破坏）
／ B 应该优化（结构性风险，暂无现场故障，但做了能明确降低回归概率）
／ C 可以延后（美化、命名、非必要抽象）／ D 不应该做（无需求来源、无证据的大重构、无关依赖升级、重写已稳定机制）。

**处置动作**：保留 / 修复 / 优化 / 延后 / 删除。

**覆盖标注（不允许"没测"冒充 PASS）**：
`AUTOMATED`＝CI 或本地门禁里真的跑；`SIMULATED`＝进程内夹具/假对端跑过，但不是真实进程；
`MANUAL-HARDWARE`＝只能真机或人眼看，明确写进 §6 Smoke 清单。

**任务状态机（§二十一，每条任务只能处在一态，禁止"改完码→DONE"）**：
`DISCOVERED → AUDITED → PLANNED → IMPLEMENTING → UNIT_VERIFIED → INTEGRATION_VERIFIED → E2E_VERIFIED → DOCS_SYNCED → GUARDS_LOCKED → DONE`

---

## 1. Current Architecture Assessment（现状架构判定）

**结论：架构方向正确、层次边界已经收口，且这一层不是当前风险来源。风险集中在"证明的形状"——
现有自动化几乎全部是**静态源码断言 + 进程内单元**，没有一层真的把应用当应用跑起来。**

### 1.1 分层与依赖方向（实测）

`UI → src/api 门面 → Commands → Domain → Persistence/Network` 成立，且**是机器守着的**：

| 判据 | 守在哪 | 现状 |
|---|---|---|
| 前端只能走 `src/api`（出现 `@tauri-apps/api/core` 即红） | `src/api/events.test.ts` | 绿 |
| 注册表 ↔ 契约图 IPC 表双向等值、两侧无重复 | `src/utils/mapContract.test.ts` | 绿，实测 **129** 条 |
| 门面包装必须被引用；注册表 ↔ 前端调用面逐条对账；死命令必须为 0 | `mapContract.test.ts` / `events.test.ts` | 绿（本轮已删 5 条零引用命令） |
| 发射事件不得持 DB 锁（INV-P25） | `scripts/check-lock-scope.mjs`（296 个取锁点自动扫） | 绿 |
| 领域图必须认领每个源文件、一文件一域、`activeHome` 字段齐 | `check-domain-map.mjs` 判据 A–F | 绿 |
| Rust 跨域 `use crate::…` 必须在 `consumes` 里 | `check-domain-deps.mjs` 判据 G–I | 绿（**只扫 Rust，前端不扫**） |
| 不变量例外必须登记 | `check-invariant-exceptions.mjs` | 绿（现 2 条：INV-P03/P04） |

### 1.2 规模（本轮逐个复算，不引用旧文档）

| 事实 | 数字 | 怎么量的 |
|---|---|---|
| 后端注册命令 | **129** | `lib.rs` `generate_handler!` 条目数 |
| 前端测试用例 | **606** | `node --test --test-reporter=tap` 的 `^ok` 计数 |
| Rust 用例基线 macOS | **690** | `src-tauri/test-baseline.macos.txt` 行数 |
| Rust 用例基线 Windows | **487** | 同上 windows（**落后约 200 条，见 S-2**） |
| 护栏非空转用例 | **185** | `verify-guards.py` 的 `Case(` 计数；#27 那 4 条已跑完注入验证并计入 |
| 登记不变量 | **26** | `protocol-invariants.md` 的 `^### INV-P` |
| 其中**有具名验证钩子** | **6 条**（P11/P22/P23/P24/P25/P26）+ P17 半条 | 逐节核对 |
| 门禁层 | 快速 10 步 / 全量 16 步 / CI 3 job | `scripts/verify.mjs` 步骤表 |
| 真实多实例 E2E | **0** | 见 §5.3 |

### 1.3 结构上的既成事实（保留，不要动）

- `include!` 分册使模块命名空间扁平 ⇒ 守卫必须读**手工聚合视图**（`all_commands_src()` / `all_db_src()` / `transport_src_for_guards()`）。已有"漏登记=假绿"的教训与对应对账守卫，**新增分册必须同步三处登记**。
- 单全局 `Mutex<Connection>` + 「emit 出锁」是已经落地的取舍，不是待改的债。
- 文件链：`.part` 流式 + attempt epoch + `chunk_size` 上线协商（v4.29.41 收口 P4）+ 终态唯一出口 `db::finalize_file_failure`。
- 常驻窗口 = 关即隐藏 ⇒ 自愈契约是「重新可见/重获焦点必拉数据」，已有判据现场读 Rust 常量而非点名。

---

## 2. Current Stability Risks（当前稳定性风险，按危害排序）

> 每条都给「今天有没有机器证明」。没有的，就是本阶段要补的。

**R1 用户旅程完全无自动化证明（最高）**
发消息/收文件/群同步/重启恢复这些链路，单测覆盖的是**函数**，护栏覆盖的是**文本形状**。
没有任何一层验证过"点一下 → 对面真的出现且只有一条"。
现状证明：`AUTOMATED`（单元级）／`MANUAL-HARDWARE`（旅程级）。缺：§7 全部 8 条旅程。

**R2 CI 从不启动应用**
`verify.yml` 只做静态检查+单测+编译；`build*.yml` 只打包。三个 example（`e2e_peer`/`dual_link`/`mirror_dial`）
需要人手工起实例，且**不在任何门禁里**。⇒ 一次跨进程回归都不会自己响。

**R3 26 条不变量里 20 条没有具名钩子**
P01 唯一 msg_id、P02 重复投递安全、P03/P04 outbox 边界、P14 DB 是事实源、P15 event 只是通知、
P19 seq 权威、P20 聊天不被大文件饿死、P21 建链前先验身份等，**只有 prose 的「验证」段或"推荐测试"伪码**。
⇒ 总指令§三列的"物理定律"里，大多数定律目前没有锁。这是"未来 AI 改坏而 CI 不响"的直接入口。

**R4 文档硬数字与代码大面积漂移（本文件实测）**

| 位置 | 写的 | 真的 |
|---|---|---|
| `ARCHITECTURE-MAP.html:1404` | `DB_VERSION = 8` | **9**（`db.rs:20`） |
| `ARCHITECTURE-MAP.html:229/261/298/413/1459` | 133 条 IPC | **129** |
| `ARCHITECTURE-MAP.html:275` | 29 个事件 | EVENTS 表 **28** 行 |
| `ARCHITECTURE-MAP.html:1436` | 181 条护栏 | 185（工作树） |
| `README.md:33/271` | 138 条 IPC / 123 条护栏 | 129 / 185 |
| `docs/AI_ENGINEERING_INDEX.md:14` | 138 条 | 129 |
| `protocol-invariants.md:632` | 「上面 21 节」 | 26 节 |
| `README.md:28`、`AI_ENGINEERING_INDEX.md:64` | INV-P01~P24 | P01~P26 |
| `ARCHITECTURE-EXPLAINED.md:174/381` | 16 张表 | 19 |
| `migration-ledger.md:43` | transport.rs 8836 行 | **10182** |
| `migration-ledger.md:49` | file_relay 86 行 | **504** |
| `migration-ledger.md:57` vs `:107` | 43 个 vs 13 个文件（自相矛盾） | 22 |

⇒ 根因不是"没人记得改"，是**契约图只有 CMDS 第一列被机器核对，其余全靠人诚实**
（`mapContract.test.ts` 头部自己承认）。修法见 §8 阶段 B：**能算出来的数字一律不许手写**。

**R5 `schema.sql` 已经不是事实源**
实测：`src-tauri/src/schema.sql` 14 张表，`db.rs` SCHEMA 18 张（+`content/store.rs` 的 `content_transfers` = 19），
差集 = `favorites` / `group_files` / `group_file_recipients` / `group_recalled_messages`。
没有任何判据读这个文件。⇒ 两条路都合法：要么生成它，要么删它并在文档里声明"唯一真源是 `db.rs::SCHEMA`"。
**当前它是个会误导人的假契约**（§8 B-3 处理）。

**R6 事件 payload 无契约**
`events.test.ts` 双向核对**事件名**，但 28 个事件里只有 3 个（`settings-changed`/`runtime-changed`/`data-cleared`）
有逐字段断言。TS 侧 `listen<T>` 的 `T` 与 Rust serde 结构**零比对**，字段改名/漏字段/类型变化都不红。
ADR-0009（Rust 生成 TS 类型）至今 `Status: Proposed`。

**R7 多实例能力不足以直接支撑双实例 E2E（隔离有洞）**
实测 `state.rs`：隔离了 **DB 文件**（`gosslan-N.db`）、**日志**（`gosslan-N.log`）、**device_id**（`-iN` 后缀）、
**TCP 端口**（59992+N×10）、UDP 59991 共享 SO_REUSEPORT（这正是同机两实例能互相发现的原因，**保留**）。
**没隔离**：`app_data_dir` 本身 ⇒ `downloads`/`cache`/`favorites` 三目录**全实例共用**。
⇒ 双实例收发文件会写进同一个目录，"同名文件/`.part` 已存在"这类用例的判定会被污染。
好消息：接收目录走 `user_dirs::load(&conn, RECEIVE)`，即**每实例 DB 里的设置** ⇒ 不改生产码就能隔离。

**R8 Windows 那条腿的清单守卫半失效**
Windows 基线 487 条 vs macOS 690，差约 200 条；`check-test-manifest` 对"多出来"只 warn。
⇒ Windows 上"绿"不代表同等覆盖。只能在 Windows 环境 `--update` 收。

**R9（本轮已收口）一处未提交护栏**
`verify-guards.py` 里 #27 那 4 条（`mobile-layout` 族）已跑非空转验证：四条全部「改坏即 FAIL、恢复即 PASS」，
注入残留 grep 为 0 ⇒ 护栏 181→**185**。教训保留：**未验证的守卫比没有守卫更危险**，
它让人以为那条判据真的被守着。

**R10 两份文档对同一件事说法相反（账要清）**
① `HANDOFF §0` 说 #27 "正在查机制"，CHANGELOG 4.29.45 说两条成因已修；
② `HANDOFF §4-A` 说 "P3 八条下一批从这里开始"，同文档 §2 说批次 i 已逐条复核完；
③ `HANDOFF §13` 说"某群首次开任务窗口仍付一次 WebView 创建、等用户拍板"，而 `736e7c5` 已改成全局固定 `label=tasks`；
④ `review` 文档仍把 P1 标 ⬜，但 `8d5be3a`/`159d14b` 已落地。
⑤ `acceptance/1.0-release.md` 第 21 条"收到未知帧 ⇒ 今天会拆链，属必修"已被 `4dfe93f` 推翻，文档未更新，
且该文件 22 条 P0 **一个 checkbox 都没有**。

---

## 3. Existing Tasks Audit（现有任务逐条重判）

> 来源：`HANDOFF.md`（§0/§3/§4/§9/§12/§13）、`docs/ARCHITECTURE-REVIEW-2026-09-24.md`（P1–P12 + 8 步路线）、
> `docs/acceptance/1.0-release.md`、`docs/adr/0010`、本地任务号 `#NN`（**注意：仓库没有 GitHub issue，
> `#NN` 全是本地任务 id**）、CHANGELOG。共 **26 条真开着的** + 下述新增。
> **不机械接受**：判定列是本轮重新给的，与原计划不一致的地方都写明理由。

### 3.1 A 类——必须修复（进计划）

| id | 一句话 | 判定 | 处置 | 理由 |
|---|---|---|---|---|
| **A-1** | 双实例 E2E harness（启 A/B、等 ready、建友、通信、断言双方 DB+日志+UI、清理、出报告） | **A** | 新建（原任务列表里没有，是总指令§四的核心） | R1/R2 的唯一根治；§7 全部旅程都压在它身上 |
| **A-2** | 故障注入层（断链/杀进程/重启/重复帧/乱序/旧 attempt/错 hash/错 size/`.part` 已存在/DB 锁竞争） | **A** | 新建，`adr/0010` 至今 Proposed ⇒ 从"设计"转"实施" | §八要求；文件链历史上 4 个真机 RCA 全在这一族 |
| **A-3** | 文件传输稳定性实验室（尺寸×故障×恢复的矩阵化永久回归） | **A** | 新建（原 `#25` 判"两段已完成"即收口 ⇒ **改判未完成**） | `#25` 只做到进程内；§七要的是真进程+真磁盘。RCA 记录的那几次 600MB 失败今天仍只能人肉测 |
| **A-4** | `#25-2` 多文件同发 / 大文件+建群并发的自动化回归 | **A** | 保留并并入 A-3 | 曾实测互相拖累（RC4），无回归 = 会再犯 |
| **A-5** | `#25-3` 两端重启后 `.part` 前缀与内存接收器不一致（160MB 根因形状） | **A** | 保留并并入 A-3 | 同上，且"重启恢复"是 DoD 明列项 |
| **A-6** | `#47` Windows 基线落后约 200 条 ⇒ 该腿清单守卫失效 | **A** | 保留，**延后到有 Windows 环境时**（技术上只能在 Windows 上 dump） | 不是"可以不管"，是"环境阻塞"；§8 阶段 C 排它 |
| ~~A-7~~ | R9：那 4 条未提交用例的注入验证 | — | **已完成**（4/4 改坏即红，残留 0） | —— |
| **A-8** | R5：`schema.sql` 与真源不一致且无人核对 | **A** | 修复（生成或删+声明唯一真源，二选一） | 它是"数据契约"的地基，错了会带错一批结论 |

### 3.2 B 类——应该优化（能明确降低回归概率才做）

| id | 一句话 | 判定 | 处置 | 理由 |
|---|---|---|---|---|
| **B-1** | 20 条无具名钩子的不变量补钩子（优先 P01/P02/P03/P04/P14/P15/P19/P20/P21） | **B** | 优化，**排在 A-1/A-2 之后** | 没有跨进程 harness 时，部分只能做成"顺序/形状"护栏；有 harness 后能做成行为护栏 ⇒ 顺序错了会白做 |
| **B-2** | R6：事件 payload 契约（先覆盖有真实消费者的 ~10 个，不做全量 codegen） | **B** | 优化 | ADR-0009 全量生成 TS 类型属大工程，收益待证；先做"逐字段断言"扩面，成本可控 |
| **B-3** | R4：文档硬数字改成**由门禁自己算并写回**，或加"文档数字 == 实算"判据 | **B** | 修复 + 优化 | 教训已记过：写死条数一定腐烂，一律交给执行者打印 |
| **B-4** | R7：双实例 receive/cache 目录隔离 | **B** | 修复（作为 A-1 的前置，写进 harness 而非改生产码） | 不隔离会让 A-3 的"同名文件"类用例判定失真 |
| **B-5** | §4-A③ `useAppStore` 4 处直写未收，`storeContract` 禁令只扫 `useChatStore` | **B** | 保留（小步） | 门面唯一性的已知缺口，成本低 |
| **B-6** | §4-A② 群消息卡片跨窗口同步（需向自己窗口 emit） | **B** | **降级为 C**（改判） | 属"新增一条链路"，§「暂不新增功能」；且它是跨窗口一致性问题，应先由 A-1 证明"到底错在哪一步" |
| **B-7** | `review-step-3-1` Persistence 只读连接（`db_read`） | **B** | **保留为待数据决策，现在不做** | §十二明写"先建锁竞争测试，再按真实数据决定" ⇒ 前置是 A-2 的 DB 锁竞争用例 |
| **B-8** | `review-step-3-2` `list_group_files` 1+3N 与 `refreshGroups` N+1 收批量 | **B** | 延后（性能类，无现场故障） | 先有 §7 旅程判据，再谈优化 |
| **B-9** | `review-step-3-4` `export_chat_text` 等待时间上界测试 | **B** | 保留，并入 A-2 | 现在是"无上界可测"，属 harness 能力 |
| **B-10** | §4-C clippy `--all-targets` 约 25 条测试代码告警 | **B** | 延后（口径外） | 门禁是 lib-only；改口径要连同 CI 一起想清楚 |
| **B-11** | `verify:all` 与分层入口（§十五） | **B** | 优化，**排在 A-1/A-2 落地之后** | 现在只有 `verify`/`verify:full`，且 CI 用同一步骤表 ⇒ 一致性已达标；缺的是 `e2e`/`multi-instance`/`fault-injection` 这三层**还不存在**，先造层再造入口 |

### 3.3 C 类——延后（无稳定收益或前置未成立）

| id | 一句话 | 处置 | 理由 |
|---|---|---|---|
| **C-1** | `review-step-7-1` `handle_message` 按域分册 | 延后 | 纯搬家；实测第 7 步主要收益已拿到（冷加载单 IPC、对账三段）。分册本身不减少任何一类故障 |
| **C-2** | `#49`/`review-step-7-4` 拆 `useChatStore`（2168 行） | 延后 | §十一原话"先消除状态串味，再考虑文件大小"。串味由 B-5 与 storeContract 处理，行数不是风险 |
| **C-3** | `review-step-6-2` 次级键选路 | 延后 | 文档自己判"建议先不做，等一条可复现证据" |
| **C-4** | `review-step-6-3` 选路断言（LAN-only 顺序零变化 + 全排列） | 保留为 C | 系于 C-3；等真做选路时再要 |
| **C-5** | `#26` 设备身份一键重生成 + 自定义后缀 | 延后（**功能类**） | §「暂不新增用户功能」；且其子问题"链路上收到与自己相同 device_id 会怎样"应作为 **A-2 的一条断言**先查清，不必先做 UI |
| **C-6** | `#32` 默认头像 / `#35` 原生感 / `#36` Win 内置 WebView2 | 延后 | 全部是新增功能/打包形态，不属稳定版范围。#36 有真实故障面（企业版无 WebView2），但属发布策略不是代码稳定性 |
| **C-7** | §4-A① 未读与投递摘要去重只覆盖 LRU 4 个会话 | 延后 | 原注释自己写"不确定就别动"；A-1 起来后用真实多会话旅程先看有没有现场 |
| **C-8** | `#30` 移动端群已读"不见了" | **保持 AUDITED，不许猜修** | 用户已真机确认"已读在"；根因未定位，缺取证手段 ⇒ 转 §6 Smoke，等一次真机取证 |

### 3.4 D 类——不应该做（本轮明确写死，防"顺手扩大"）

| id | 内容 | 处置 | 理由 |
|---|---|---|---|
| **D-1** | 引入 Actor / task-per-connection 框架；接 `transport/` 那套新栈 | 删除（计划层面） | review「明确不做」原话；新栈那部分死代码已删（`b64c0ad`） |
| **D-2** | 协议词表改造 / 换数据库 / 把 SQLite 换成别的 | 删除 | 无证据、无收益、风险最高 |
| **D-3** | 把"真 socket IO 失败"降级成普通文件错误（忍着复用坏连接） | 删除 | 坏连接必须判死，隔离靠 endpoint 维度 |
| **D-4** | 没有现成信号支撑的"智能选路" | 删除 | 先接线已有拥塞时间戳，且必须先证明 LAN-only 顺序零变化 |
| **D-5** | `#18` A1-L2 第二段回执链 | 删除 | 发送方没有 UI 消费者；已判"不做"，不要再捡 |
| **D-6** | `#25-4` 100MB/600MB 真量级单测 | 删除（改由 A-3 以受控尺寸 + 断点续传语义覆盖） | 单测里跑真 600MB 只买到 CI 时长，不买到判据 |
| **D-7** | 未读改成后端给值 | 删除 | 实测前提不成立（`utils/messages.ts:328` 已判定） |
| **D-8** | `messages(ts)` 索引 | 删除 | `EXPLAIN QUERY PLAN` 实测无收益 |
| **D-9** | `nicknameOf` / `groupReaderIds` 性能改造 | 删除 | 实测 0.066ms/帧，忽略级 |
| **D-10** | F5③ gossip 解密前去重 | 删除（保持现状） | 刻意不改：会造成广播放大 |
| **D-11** | `AI_PROJECT_HANDOFF §8` 的 Noise XX / mDNS / QUIC / 服务端中继 / 账号体系 | 删除（本阶段） | 文档自己写"v1.0 后规划，不要在当前阶段主动实现" |
| **D-12** | ADR-0009 全量"从 Rust 生成 TS 类型" | 延后为 B-2 的窄版本 | 全量 codegen 与"给 ~10 个事件补逐字段断言"不是一回事，后者够用 |

### 3.5 账目矛盾必须先清（不改代码，只改文档）

`HANDOFF §0` 状态表有 11 行仍写"待推/待提"，而版本号已涨到 4.29.45、远端 tip = `aaf238d` ⇒ 表过期。
连同 §3.3 列的 5 处相互矛盾（R10），一并作为 §8 阶段 B 的第一项（AUDITED→DOCS_SYNCED）。

---

## 4. Completed / Invalid / Duplicate / Missing

### 4.1 已完成——**不要重做**（有 sha 或具名测试才列）

0-A1 守卫登记 `69ec7a7`｜0-A2 删三处平行实现 `b64c0ad`｜0-A3 IPC 收缝 + 删 5 条零引用命令
`417ba37`/`69db4b6`/`a021861`/`b1b1cbb`/`3b32f04`/`1c8541c`｜0-B 四条热查询索引 + v8→v9 `87836a4`/`ccdf21e`｜
P1 断链按"该 peer 全链路丢失"门控 + 停滞兜底 `8d5be3a`/`159d14b`｜P2 写失败按错误来源分流 `4dfe93f`/`c56127a`｜
P3 链路队列按字节封顶 `33bf933`｜P4 中继接收改流式 + `chunk_size` 协商 `274d42e`(v4.29.41)｜
P7 四刀（文件终态唯一出口）｜锁内 emit 清零 + INV-P25 判据 `6d8defb`｜第 5 步-1 常驻窗口自愈｜
第 5 步-3 未读过渡竞态｜`#46` `is_fresh` 恒为假（`fresh_schema_alone_has_exactly_the_migrated_shape`）｜
第 7 步-1 切会话冷加载收成一次 IPC（`get_latest_messages`，判据在 `storeContract.test.ts`）｜
`#25` 段 1/段 2 + emit 抑制行为测试 `fe01723`/`f4b3d3f`/`d18db7a`｜F1′ attempt epoch `2ebfe33`｜
F2 大文件不上 BLE(16MiB)｜F4 fsync 出锁｜F5 群同步①②④｜`#40` 桌面全局预览窗 `7f1664d`/`bd93f58`｜
`#41` 任务窗口固定 label + 预热 `736e7c5`｜`#47` 致命类跨平台可拦 `158eda0`/`aaf238d`｜发版脚本同步 Cargo.lock `26f82ee`｜
`#27` 移动端首屏两条成因 `48345bf`（⚠️ 见 §6，真机未证）。

### 4.2 Invalid——**任务本身不成立**（判据被实测推翻，直接删）

- `P3 八条`：逐条复核后 **6 条是幻影**（其中 2 条"未接线"实为已接线但命名不同形状）⇒ 别再照单开做。
- `第 6 步-1` "全链路不健康要返回 None"：前提不成立，已删。
- `acceptance/1.0-release.md` 第 21 条（未知帧会拆链 ⇒ 必修）：已被 P2 修复推翻，**条目本身作废**，文档待更新。
- `review-step-2-3`（2GB offer 不分配全量内存的测试）：P4 已把接收改流式，**这条的形状已被更强的实现取代**；改写成 A-3 的一条断言即可，不单独留任务。

### 4.3 Duplicate——**同一根因被记成多条**（合并）

`§4-A②` ＋ `#30` ＋ 第 5 步系列 ⇒ 都是"跨窗口/常驻窗口状态新鲜度"，归 **J6**。
`#25-2` ＋ `#25-3` ＋ `§4-A P3八条剩余` ＋ `review-step-2-3` ＋ `adr/0010` 的 11 个场景 ⇒ 全归 **A-3 文件实验室** + **A-2 故障注入** 两张矩阵。
`#47` ＋ CHANGELOG 4.29.42/4.29.43 里两条"基线数字不对" ⇒ 同一件"Windows 腿弱"，归 **A-6**。

### 4.4 Missing——**没有任何地方记过，但按 §19 必须有**

1. **M-1 契约漂移检测**（§十三）：现在文档数字漂移是既成事实（R4），但**没有任何判据**读它 ⇒ 需要"文档 == 实算"的门。
2. **M-2 测试报告产物**（§十六）：`test-results/run-<ts>/`、`summary.json/html`、实例日志、DB 快照、截图 ⇒ 仓库里**一个都不存在**。
3. **M-3 Trace 贯穿**：`msg_id`/`transfer_id` 目前散在日志文本里，没有"一条消息跨层可串"的判据。
4. **M-4 数据生命周期 E2E**：旧库→迁移→重启→清空→重初始化，今天只有迁移单测，没有"进程重启后仍正确"。
5. **M-5 平台/硬件 Smoke 清单**：`docs/acceptance/1.0-release.md` 有 22 条 P0 但**零 checkbox、零状态** ⇒ 不是清单，是一份愿望列表。
6. **M-6 `verify:all` 的 e2e/multi-instance/fault-injection 三层**：§十五列的入口里有 **3 层今天不存在**（不是命名问题，是没有被测物）。
7. **M-7 前端事件反向通道的架构判据**：`check-domain-deps` 明确不扫 `src/**` ⇒ 前端依赖方向只有 api 门面一条守得住。

---

## 5. Automation Coverage（自动化覆盖现状）

### 5.1 四层现状

| 层 | 内容 | 数量 | 查得到什么 | **查不到什么** |
|---|---|---|---|---|
| L1 行为单测（前端） | `node --test`，60 个 `.test.ts` | **606** | 纯函数/状态机/合并/解析的正确性 | 组件真实渲染、浏览器布局、跨窗口 |
| L2 行为单测（Rust） | `cargo test --features bluetooth` | **690**(mac)/487(win) | 协议/分片/队列/DB/选路/清理的进程内语义 | 真 socket、真进程、真重启 |
| L3 契约对账 | api/events/domain/lock/invariant 等 6 个脚本 + 3 个 test | 20 余条判据 | 名字/归属/注册/取锁形状/双向存在 | 类型与 payload 形状、UI 可达性 |
| L4 护栏非空转 | `verify-guards.py` | 181(提交)/185(树) | "把正确代码改坏必须红" | 反向（无对象时必须非 0）类判据 |
| **L5 跨进程 E2E** | — | **0** | — | **一切用户可感知的东西** |

### 5.2 功能区覆盖矩阵（17 区 × 4 类证明）

`●`=真行为断言 `○`=只有文本/形状护栏 `–`=没有 · E2E 列全为 – 即 §7 的欠账

| 功能区 | 单元 | 契约 | 护栏 | E2E |
|---|---|---|---|---|
| 文本消息 | ● | ● | ● | **–** |
| 文件传输 | ● | ● | ● | **–** |
| 图片/预览 | ○ | ● | ○ | **–** |
| 群聊/群同步 | ● | – | ● | **–** |
| 任务(群 todo) | ● | ○ | ● | **–** |
| 公告 | ○ | – | ○ | **–** |
| 收藏 | ● | ○ | ○ | **–** |
| 好友/请求 | ● | ○ | ● | **–** |
| 已读/未读 | ● | ● | ● | **–** |
| 通知 | ○ | – | ○ | **–** |
| 搜索 | ● | ○ | – | **–** |
| 共享目录 | ● | ○ | ● | **–** |
| 设置 | ● | ● | ● | **–** |
| 窗口生命周期 | ○ | ○ | ● | **–** |
| 网络通道(LAN/routed/relay/BLE) | ● | ● | ● | **–** |
| 内容补拉 | ● | ○ | ○ | **–** |
| 数据清理/迁移 | ● | ● | ● | **–** |

⇒ **结论很直白：单测与护栏这一层密度不低，E2E 这一层是零。** 所以§四把它定成"整个计划中最重要的一项"是对的。

### 5.3 明确不存在（不要以为有）

- 无任何 CI job 启动应用；无任何跨进程通信测试。
- 无 Playwright / WebdriverIO / tauri-driver / wdio 配置，无 `test-results/` 目录。
- 3 个 example（`e2e_peer`/`dual_link`/`mirror_dial`）手工、需要活实例、**不在门禁里**。
- 已有手工多实例脚本：`scripts/run-multi-instance.ps1`（Win）、`t3-presence-relay.sh`、`t2-learn-id.sh`、`e2e-dev.sh`、`t4-mirror-dial.sh` ⇒ **可当 harness 的行为参考，但它们不是测试**（无断言、无报告、无门禁）。
- ADR-0010 故障注入：`Status: Proposed`，代码里只有零散同名用例。
- ADR-0009 类型契约：`Status: Proposed`，未落地。

---

## 6. Manual Coverage（必须承认只能人工的部分）

> 现状问题不是"有手工项"，而是**手工项没有清单、没有状态**（`acceptance/1.0-release.md` 22 条零 checkbox）。
> §8 阶段 A 要把它变成带 `AUTOMATED / SIMULATED / MANUAL-HARDWARE` 标注的 Smoke 矩阵。
> 下面这批**近期只能由用户真机验收**（本环境无屏幕录制/辅助访问权限，起得来窗口但我看不到）：

| 项 | 为什么不能自动 | 标注 |
|---|---|---|
| 移动端首屏（#27）真机首帧布局、转屏 | 需要真机 + 权限弹框时序 | MANUAL-HARDWARE |
| BLE 真实收发（三端） | 无 BLE 硬件、无对端 peripheral | MANUAL-HARDWARE |
| 系统通知点击跳转、桌面角标 | OS 通知中心不可读回 | MANUAL-HARDWARE |
| 沙盒目录书签权限（macOS）/ 厂商后台限制 | 系统与厂商行为 | MANUAL-HARDWARE |
| relay 大文件真链路（含断网/切换） | 依赖外部中继服务器与真网 | 待 A-2/A-3 转 SIMULATED |
| 两台以上真设备联调（acceptance 第 15 条） | 物理设备 | MANUAL-HARDWARE（A-1 落地后同机双实例可转 AUTOMATED 覆盖大部分） |
| 群已读"不见了"(#30) | 根因未定位、缺取证 | MANUAL-HARDWARE |

---

## 7. High-Risk User Journeys（高风险旅程 = A-1 harness 的第一批用例）

排序依据：**曾出过真故障 > 状态机最复杂 > 涉及磁盘 > 涉及跨进程 > 其他**。
每条写"必须断言什么"，因为 harness 的价值全在断言清单，不在启动进程。

| # | 旅程 | 为什么最高风险 | 必须断言 | 今天能否自动 |
|---|---|---|---|---|
| **J1** | 文本消息 A→B 全程 | 它是所有语义的地基；RC3 曾出现"接收失败但发送端显示成功" | A 出现 sending→sent；B **恰好一条**、内容正确；`msg_id` 唯一；A 最终 delivered/read；outbox 清空；**重启后仍正确**；无重复通知 | 否 → A-1 后 **AUTOMATED** |
| **J2** | 文件 A→B（小/中/大 × 尺寸阶梯） | 历史 4 个 RCA 全在此；`.part`+rename+hash+attempt 四层状态 | 只有 rename 后 B 目录有文件；进度单调不超 100%；hash 不符必拒且不 rename；`.part` 收完即清；重复 FileDone 幂等 | 否 → A-2/A-3 后 AUTOMATED |
| **J3** | 传输中断链/杀进程/重启后续传 | 跨链路重试 + Gap 判死曾是 600MB 必死根因 | 断链只影响该端点；旧 attempt 帧必须无效；重启后前缀一致；终态不可被降级（INV-P26） | 否 → A-2 后 AUTOMATED |
| **J4** | 群聊：离线成员→上线后补齐 + gossip 收敛 | RC2「永久不同步而单聊正常」的机制最复杂 | 成员重连后收敛到同一集合；撤回按 G-Set 语义不复活；seq 排序权威；不产生重复应用 | 否 → A-1 后 SIMULATED，两台真机 MANUAL |
| **J5** | 首次启动（新库→骨架→首页→身份） | `is_fresh` 曾恒为假；#27 首屏误判桌面；建窗不能阻塞主线程 | 新库只付一次 schema 成本（不打假"正在迁移"）；首帧判移动版；骨架必关；窗口可见 | 部分可 AUTOMATED（DB/日志），视觉仍 MANUAL |
| **J6** | 窗口：隐藏→再聚焦→数据新鲜 | 常驻=不重载；三窗口共用骨架 | 重获焦点必重拉；任务窗换群；预览换内容；无白屏骨架 | 结构部分 AUTOMATED，视觉 MANUAL |
| **J7** | 数据生命周期：退出→重启→迁移→清空→重初始化 | 唯一裁决/终态契约/级联删除的交叉区 | 无幽灵状态；清空后身份仍可用；迁移中途失败可恢复；`failed` 只有一个裁决者 | 否 → A-1 后 AUTOMATED |
| **J8** | 中继：直连失败→relay，且 token 不进日志 | 安全边界 + 唯一"外部服务器"依赖 | 旧版本 offer 明确拒收并给理由；日志/事件里**只有 `token_len`**；中继不改变 hash 语义 | 日志断言 AUTOMATED，链路 SIMULATED |

---

## 8. Recommended Execution Order（执行顺序与退出判据）

> 原则：**先造证明，再动核心**（§二十二 3）。所以顺序不是"先修最险的代码"，
> 而是"先让最险的代码**能被自动证明**"。当前**没有任何 A 类生产代码缺陷是已知的**
> （P1–P7 均已收口），因此阶段 A/B 全是基础设施与账目，不改生产链路——这正好符合§二"先审计不要乱改"。

### 阶段 A — 证明基础设施（最高优先，不改生产逻辑）

1. **A 账**：把 §2/§3.5/§4 的 5 处文档矛盾、`acceptance/1.0-release.md` 的 Smoke 矩阵、
   以及本文件数字一次性对齐 ⇒ 产出带 `AUTOMATED/SIMULATED/MANUAL-HARDWARE` 标注的清单。（**纯文档**）
2. **收口 R9**：4 条未提交护栏跑注入验证，红则留、不红则撤。
3. **A-1 harness v0（里程碑即"能跑通一条"）**：同机双实例启动 → 等 ready（读各自 DB/日志，超时判 FAIL 不退 0）
   → 互加好友 → 等在线 → **跑 J1 全断言** → 清理 → 出 `test-results/run-<ts>/{summary.json,summary.html,instance-*.log,sqlite-*/}`。
   前置 B-4：每实例 `receive_dir`/`cache` 走自己 DB 的设置，物理隔离。
   **退出判据**：连跑 3 次全绿、单次 ≤5 分钟、残留 0 个进程/0 个临时目录、失败时能报出"第几步、预期 vs 实际、msg_id"。
4. **CI 接一层**：`verify.yml` 加 `e2e` job（macOS + Windows 两条腿，§二十优先 Windows x64），**必须调同一套入口**（§十五）。

### 阶段 B — 把"证明"扩到面

5. **A-2 故障注入层**：断链 / SIGKILL / 重启 / 重复帧 / 乱序 / 旧 attempt / 错 hash / 错 size / `.part` 已存在 / DB 锁竞争（§八清单逐条落），
   每条同时是 **ADR-0010 的实施**（顺手把那份 Proposed ADR 关掉）。
6. **A-3 文件实验室**：尺寸阶梯 × 上述故障，覆盖 J2/J3，并把 §4.1 里 F1/F2/F4/F5 的每次历史修复各钉成一条永久回归。
7. **J4/J6/J7**：群收敛、窗口新鲜度、数据生命周期。
8. **M-1 契约漂移门**：文档数字改为实算/写回；`schema.sql`（A-8）一并处理。

### 阶段 C — 把既有证明补强（B 类）

9. **B-1** 20 条不变量补钩子（有 harness 后优先做行为版）。
10. **B-2** 事件 payload 逐字段断言扩面（~10 个有真实消费者的事件）。
11. **A-6** Windows 基线在有 Windows 环境时 `--update` 收口。
12. **B-5 / B-9 / B-11** 小步补齐。

### 阶段 D — 由数据决定要不要动 DB / 前端结构

13. **B-7** 只读连接：仅在 §5 的 DB 锁竞争用例给出"确实阻塞 UI"的数字之后才做。
14. **C-1 / C-2** 分册与拆 store：仍延后，除非 A 阶段暴露出"因为文件大才查不出"的具体故障。

### 阶段 E — 稳定版终审（§第五步）

15. 逐项回答 §19 七组验收 + §13 十四个审计维度；只有全部达到标准才宣布完成。

**每阶段都走§十七小步闭环**：发现→根因→最小修改→测试→代码→局部测试→guard→E2E→文档→完整验证→提交。

---

## 9. 进度状态机（本地图当前快照）

| 任务 | 状态 | 缺什么才能前进 |
|---|---|---|
| 本文件（第一阶段审计） | **DONE**（AUDITED→DOCS_SYNCED 于本轮完成；无生产码） | — |
| A 账 / Smoke 矩阵 | PLANNED | 落文档，不需环境 |
| R9 四条护栏 | **DONE**（UNIT_VERIFIED + GUARDS_LOCKED：4/4 注入验证通过） | — |
| A-1 harness v0 | DISCOVERED | 设计（第二步）+ B-4 隔离方案 |
| A-2 故障注入 | DISCOVERED | A-1 先立 |
| A-3 文件实验室 | DISCOVERED | A-1 + A-2 |
| A-8 `schema.sql` | AUDITED | 决定生成还是退役（属"数据契约"，需用户点头） |
| A-6 Windows 基线 | BLOCKED(环境) | 需要 Windows 环境 |
| B-1 不变量补钩子 | DISCOVERED | A-1/A-2 提供行为断言载体 |
| C-1/C-2 分册/拆 store | 延后 | 需要 A 阶段暴露具体故障 |
| D 类 12 项 | 关闭 | 不再重开，除非有新证据 |

---

## 10. 复算方式（每个数字怎么来的——下一个人不必猜）

```bash
# 注册命令数
node -e 'const s=require("fs").readFileSync("src-tauri/src/lib.rs","utf8");
 const m=s.match(/generate_handler!\s*\[([\s\S]*?)\n        \]/);
 console.log([...m[1].matchAll(/commands::([a-z0-9_]+)/g)].length)'
# 前端用例数
node --test --experimental-strip-types --test-reporter=tap <package.json scripts.test 里那批文件> | grep -c "^ok "
# Rust 用例数
wc -l src-tauri/test-baseline.macos.txt src-tauri/test-baseline.windows.txt
# 护栏用例数 / 不变量数
grep -c "^    Case(" scripts/verify-guards.py
grep -c "^### INV-P" docs/protocol-invariants.md
# schema 差集（本文件 R5 的来源）
grep -oE "CREATE TABLE IF NOT EXISTS ([a-z_]+)" src-tauri/src/schema.sql | awk '{print $NF}' | sort
grep -oE "CREATE TABLE IF NOT EXISTS ([a-z_]+)" src-tauri/src/db.rs       | awk '{print $NF}' | sort
# 真源版本
grep -n "pub const DB_VERSION" src-tauri/src/db.rs
```

**纪律（本轮又验证了一次）**：文档里任何"N 条/N 张/N 行"都不许手抄；
子 agent 报的"实测值"必须自己复跑一遍才算数（本轮 L2 数字 `651 vs 基线 690` 的差异，
就是"数函数名"与"数 harness 清单"两种口径 —— 取后者，因为门禁用的是后者）。
