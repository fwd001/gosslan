# Gosslan AI Engineering Index

## Required before coding

1. `AI_RULES.md`
2. `docs/acceptance/1.0-release.md` — current goal and acceptance bar
3. `AI_PROJECT_HANDOFF.md`
4. `docs/domains.data.mjs` — **领域图**：先确认「我改的是哪个领域、它的边界在哪」
5. `docs/migration-ledger.md` — **迁移台账**：这个关注点**有几个家、哪个在跑数据**
6. `docs/protocol-invariants.md` — when touching protocol / network / crypto / DB
7. `docs/design-guidelines.md` — when touching UI (圆角 / hover / 配色 / 窗口边界)
8. `docs/stability-roadmap.md` — **★★★ 稳定版工作地图（当前阶段）**：审计结论、10 条风险、26 条在途任务的
   A/B/C/D 重判与「保留/修复/优化/延后/删除」理由、自动化 vs 人工覆盖边界、8 条高风险旅程、执行顺序与每阶段
   退出判据。**做任何稳定性相关工作前，先在这里定位当前处在哪一格。**
9. `docs/ARCHITECTURE-REVIEW-2026-09-24.md` — **架构复审**：动 Transport / AppState / SQLite / useChatStore / 选路 / 文件传输这些核心链路之前必读。它回答了「为什么现在不做 Actor、不换库、不接 `transport/` 新栈」，并给出一次只改一个领域的 8 步路线与每步的验收。
   ↳ 第二阶段复审见 `docs/final-architecture-review.md`（2026-09-27）：基线现算值、Large File Audit、§32 那 11 问的四态结论、Release 判定。
10. `docs/ARCHITECTURE-MAP.html` — **架构与接口契约图**（浏览器直接打开，零依赖）：分层大图、
   IPC 命令的「输入 → 输出」规则表、事件契约、表的数据契约（**条数/表数别在这里抄第二份**：以图首行「取数」戳里写的口径现算）、8 条流程穿透、
   以及「已核出的漂移」清单。**要判断一个改动方向对不对 / 接口该不该新增，先看这张图**；
   图里的数字与函数名都是实跑 grep 抽的，改完架构后请同步更新它（它标了取数的 HEAD sha）。
11. Relevant ADR
12. Relevant tests
13. `CHANGELOG.md` history when touching a previously-fixed area

> ⚠️ **第 4、5 条为什么必须排这么前**（2026-09-16 加入）：本仓库处在一次**半完成的 ADR
> 迁移**中，同一个关注点常常有**两个家**（老的还在跑数据、新的部分接线）。不先确认
> 「哪个家在跑数据」就直接 grep 改代码，会改到那个**看起来更对但没在跑**的新家上 ——
> 症状不变或换个形态，于是出现「改完这个 bug 又冒那个」。
>
> 历史代价：`4.18.7→4.18.10` 连着四版修同一个 BLE 分片预算问题（三份实现）；
> macOS 与 Android 各自藏着一份独立实现，分别到 Phase 3 / Phase 4 才被发现。
>
> 由 `scripts/check-domain-map.mjs` 守门（路径存在、一文件不属两域、无文件漏归属、
> `enforce` 只能开在已收口的单家领域）。

## 其余在 `docs/` 里的文档（为什么不进上面的必读，各自写明）

上面那份"必读"是**每次动手前都要过**的清单，所以刻意不把所有文档塞进去（塞进去＝每次都变贵）。
但这几份此前在导航里**一个字都没出现**，其中前两份其实是要强制遵守的：

- `docs/VERSIONING.md` — **版本号强制规则**（每个提交的档位声明 + 同提交提版；判据 4 的对账口径）。
  动到提交/发版就必读，不在"读代码前"那一批里，所以之前没人能找到它。
- `docs/acceptance/stability-smoke-matrix.md` — **稳定版验证覆盖矩阵**（AUTOMATED / SIMULATED /
  MANUAL-HARDWARE）。它是受硬数字守卫的**活文档**之一（`check-doc-numbers.mjs` 的扫描范围里有它），
  判"某件事到底有没有被测过"以它为准，不许凭印象说 PASS。
- `docs/acceptance/native-experience-round-1.md` — **原生体验首轮建设报告**（总指令 §十一 那九项的落点）。
  读法：里面的数字都是定稿时现算的，**每条挂着复跑命令**；它明写"未验证"的四档平台与"未测量"清单，
  不许被摘出来当验收结论。任务清单在 `docs/stability-roadmap.md` §12.6.1（N1–N29，两种单元格形状，
  数分母要 `grep -cE "^\|\s*(\*\*)?N[0-9]"`，只按 `**N` 数会少 8 行）。
- `docs/version-ledger.md` — 分类台账，由 `npm run version:ledger` 生成，**别手改**（改了会被下次生成覆盖）。
- `docs/ARCHITECTURE-EXPLAINED.md` — 架构图解（新人版，写给不读代码的人）。⚠️ 它自己标了快照基线
  `v4.2.7 · 2026-09-12`，行号与模块清单**早已漂**；当"为什么这样分层"读，别当现状清单读。
- `docs/P1-image-out-of-sqlite-overview.md` — 一次**已落地**修复（图片不再以 base64 内联 SQLite）的
  交付说明。是历史，不是约束；**此前全仓零引用**（下面这条导航就是第一条指向它的话），
  留着只为"当时为什么这么改"可查。
- `docs/aoci-usage.md` — **AOCI 仓库认知层使用手册**（工具用法，不是代码约束）：认知层怎么读、什么时候必须 `aoci_maintain` 收尾、
  `S` 的两层配额与标签字典的现读口、CLI 只读命令与两个坑（CLI 写路径在 Volumes v1 下是拒的、`guide` 必带 `--agent`）。
  **§8「重新索引」**把这件事拆成三条路（补条目 / 刷新收录与基线 / 推倒重来）并列出本仓实测会被拒的六条命令与它们的报错原话——
  撞见 `init` 或 `scan` 报 `error_code=config` 时先读那一节，别在工具上找故障。
  约束本体在 `AGENTS.md` 的 AOCI 区块，这份只补"具体怎么操作"。
- `docs/large-file-split-plan.md` — **大文件拆分计划**（>3000 行的清点、transport.rs 的分节构成与 26 条护栏锚点风险、
  「行为不变」的恒等判据、下刀顺序）。当**在途工作地图**读：里面每条数字都挂现算命令，跑一遍再引。
  ⚠️ 它与 2026-09-25 复审那条「拆 store / handle_message 分册 暂缓」不冲突：那条反对的是改语义与拆 AppState，
  本计划只做文件级搬家 + 再导出（`AI_RULES.md:121` 允许）。
- `docs/strong-reminder-plan.md` — **聊天强提醒：需求登记 + 第 0 阶段设计**（2026-10-10 登记，**尚未实现**）。
  读法：它是「下一步该怎么做这件事」的设计契约，不是「这件事已经是什么样」的现状文档 ——
  现状一律以 `git show <tip>:<path>` 现读为准，本文所有现状句子都挂着复跑命令。
  内含一条**结构性缺口**（全仓身份是 `device_id`，没有"同一人多台设备"的状态同步机制 ⇒
  验收项「多设备确认合并」不能按现成能力写 PASS）与一条**必做实测**（旧端收到未知 kind 的降级形状），
  两者都还没有结论 ⇒ 动第 1 阶段之前先把这两格填掉。

## Templates

- `docs/templates/BUG_FIX.md`
- `docs/templates/ADR.md`

## ADRs

- `0007-protocol-versioning.md`（**Accepted 2026-09-20**：`protocol_version` + capability
  门控 + 灰度顺序 + 三项决策，是跨版本兼容的权威出处）
- `0008-state-machine-boundaries.md`
- `0009-rust-typescript-contract.md`
- `0010-failure-injection-testing.md`
- `0011-gossip-envelope-authentication.md`
- `0012-logical-sequence-ordering.md`
- `0013-transport-priority-queues.md`
- `0014-multi-path-connection-selection.md`
- `0015-ble-transport.md`
- `0016-relay-authorization.md`
- `0017-opaque-external-wire-frame.md`
- `0018-window-architecture.md`
- `0019-content-transfer.md`
- `0020-blind-circuit-relay.md`（**Accepted 2026-09-21**：可选自托管的公网哑管道中继。
  服务器代码**不在本仓库** → <https://github.com/fwd001/gosslan-relay-server>；
  本仓只有客户端侧的 `src-tauri/src/transport/relay_seal.rs`。
  ⚠️ 改到中继线格式要**同一轮改两个仓库**，两边各有一份规格文字，漂移没有测试提醒）
- `0021-device-id-is-generated-not-derived.md`（**Accepted 2026-09-22**：设备 ID 改为首启随机生成
  + 设备属性混合、此后只认持久化值。真机事故：克隆镜像的机器码 / 新机的默认主机名相同 ⇒ 两台设备
  同一个 id ⇒ `peers`/`links`/`friends` 互相顶 + Hello 密钥冲突硬拒 + 镜像规则在 id 相等时退化。
  含「已装设备不迁移、撞号那台换 id 并重加好友、旧会话只读」的迁移决定与自定义后缀的字符集约束）

> Earlier ADRs `0001`–`0006` (message idempotency, outbox+ACK, E2EE, transport, no-Web-Worker,
> device fingerprint) were removed; their normative content now lives in
> `docs/protocol-invariants.md` (INV-P01…PNN — the range and the count are printed by
> `node scripts/check-invariant-hooks.mjs`; never hand-copy them, this very cell drifted
> P24→P26→P27 in one day) and `AI_RULES.md` (INV-001…008).
> Do not re-create them as a second source of truth.

## Rule

If a document conflicts with executable code or tests, do not silently choose one. Report the conflict and determine whether the documentation or implementation is stale.
