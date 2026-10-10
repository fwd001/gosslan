# 聊天强提醒 —— 需求登记与第 0 阶段设计（只审计与设计，**不含实现**）

> 状态：**登记 + 第 0 阶段设计**。本文不写一行实现代码，也不改协议。
> 取证基线：提交 `1cdb42b`（= 当前 `origin/main`，版本 4.33.42）。
> 文中每一条"现状"都由 `git show 1cdb42b:<path>` 现读得出并挂复跑命令 —— 护栏注入期间工作树不可信，
> 所以这份审计从一开始就不读磁盘。凡"未实现 / 未验证 / 只有人工判据"都明写，**不许读成已完成**。
> 复跑全部现状：`git grep -n "<文中符号>" 1cdb42b -- src src-tauri/src`
>
> **这份文档刻意不进 `scripts/check-doc-citations.mjs` 的 `LIVE_DOCS` 名单** —— 它钉基线（`1cdb42b`），
> 属那份守卫文件头明写"刻意不管"的**带基线的历史快照**那一类：拿今天的事实源去判它必然随代码漂移误报，
> 而"为了避免红去放宽守卫"比没有守卫更糟。落笔时它被**一次性**用该守卫核过：
> 临时把自己那行加进 `LIVE_DOCS` 跑 `node scripts/check-doc-citations.mjs` ⇒ 裸退码 **0**、
> 「7 份活文档、40 处 `file:line` 引用全部落在真文件真行范围内」，随后把那一行撤回（名单回到 6 份）。
> 也就是说：**本文的行号引用在它自己的基线上是真行，但这份正确性不受任何常驻门禁背书。**

---

## 一、需求登记（用户 2026-10-10 原文要点，逐条编号以便将来对账）

**目标**：在保持现有聊天可靠性与去中心化特性的前提下，让用户可以明确提醒对方关注一条消息，
并获得**可信的送达与确认反馈**。参考飞书的消息触达与上下文定位、钉钉的重要事项提醒、QQ 的即时交互。

功能 1–9：私聊对已发消息发起（默认目标＝对方）｜群聊必须显式选对象（**第一版不默认全群**）｜
接收方有明显标识且点击跳到会话**与原消息**｜接收方可点「我知道了」｜发送方看待送达/已送达/已提醒/已确认/失败｜
声音可控、支持振动的平台给对应设置｜遵守系统通知权限/静音/勿扰｜未确认采用**有限重试 + 去重**、
禁止无限弹窗或反复响铃｜离线在现有通信条件允许时恢复投递（**不承诺**无网络或无推送时实时到达）。

状态语义（必须严格区分六态，见 §四）+ 三条硬不变量：
**传输层 Ack ≠ 用户确认**、**已读 ≠ 用户确认**、**确认知晓 ≠ 事情已完成**；
并且**不得修改普通消息现有的发送、送达、已读、失败语义**。

技术约束 1–9：优先复用现有消息 ID / Outbox / Ack / 去重 / 加密 / 可靠传输｜强提醒状态与普通消息状态分离｜
**只有现有架构确实无法满足**才加字段或协议消息｜任何 DB 或协议变更必须给兼容 + 迁移 + 回滚｜
必须处理离线恢复、重复投递、多设备确认、乱序同步、群成员权限变化、旧版本兼容｜
不得破坏 E2EE / 签名验证 / 消息去重 / 文件传输 / Gossip / 现有群同步｜
**不得擅自新增中心服务器、远程推送服务或第三方依赖**｜必须写清 Win/macOS/Android/iOS 的后台生命周期与通知能力差异｜
**不得把本地通知 API 调用成功误认为对方已收到提醒**。

暂不纳入第一版：电话/短信催办、默认全群、无限重复、复杂催办任务与统计报表、为强提醒重写消息或通知系统。

---

## 二、现状架构图（四条链 + 强提醒的挂点）

```
        ┌──────────────────────── 本机（一个 device_id ＝ 一个节点）────────────────────────┐
        │                                                                          │
 发起   │  MessageItem / 气泡长按菜单 ──┐                                          │
 ───────┼──────────────────────────────┼──────────────────────────────────────────┼───
        │                              ▼                                          │
        │                 commands/chat.rs（发送侧唯一入口）                       │
        │                   ├─ INV-P24 第 4 条「不门控不许发」(:65-73)              │
        │                   │    └─ protocol::dm_allowed_by_features              │
        │                   └─ db::insert_message_and_outbox (:200)               │
        │                        ├─ messages.status: sending→sent                 │
        │                        └─ outbox(msg_id UNIQUE, payload, created_at)    │
        └──────────────────────────────┬──────────────────────────────────────────┘
                                       │  链路可用时即时发；不可用时留存
                                       ▼
 投递   ┌── 可靠传输（**这条链已经满足需求 9 的"现有通信条件允许时"**）────────────────┐
        │ transport/outbox_flush.rs::flush_outbox  ← 建链 / Hello / 心跳三处触发      │
        │    只补发不删行；收到 Ack / GroupAck 才删对应行；msg_id 不变                 │
        │ transport/outbox_sweep.rs 每 30s：age 过 120s 且对端可达 ⇒ failed           │
        │                             7 天离线保留窗 ⇒ 到点才判死                     │
        │ 群侧：group_outbox UNIQUE(msg_id, peer_id) ＝ **按人排队**，30min 死线       │
        └──────────────────────────────┬────────────────────────────────────────────┘
                                       ▼
 送达   ┌── 接收侧 ──────────────────────────────────────────────────────────────┐
        │ dedup：messages.msg_id UNIQUE + INSERT OR IGNORE ＋ gossip Bloom/LRU    │
        │ Ack 帧 ⇒ set_message_status("delivered") + DELETE outbox 行              │
        │ ⚠️ 单调守卫：db/messages.rs:276 `AND status NOT IN ('read','delivered',…)`│
        └──────────────────────────────┬─────────────────────────────────────────┘
                                       ▼
 提醒   ┌── 通知/注意力链（强提醒**挂在这里，不另起炉灶**）────────────────────────┐
        │ useChatStore::queueNotification → 1500ms 去抖合并（同会话留最后一条）      │
        │   门 1 app.notifyEnabled 门 2 ensureNotifyPermission 门 3 可见即不打扰     │
        │   → notifyDesktop / sendNotification → requestAttention(urgent)          │
        │     ▲ urgent 的唯一判据 = src/utils/notifyUrgency.ts（**强提醒要成为它     │
        │       第三个被点名允许的来源**；文件注释写着"刻意不收第三个"）             │
        │   点击 → emit("notification-clicked") → focusWindow + openConversation    │
        │            └─（**载荷今天只带 conv_id**；msg 级定位链已存在但未接：        │
        │                locateMessageInConv → locateMessage（带翻页 + 高亮））      │
        └─────────────────────────────────────────────────────────────────────────┘

 确认   「我知道了」＝**全新的一段**：现有语义里没有任何"人对某条消息显式确认"的位置
        （`read` 是"看到过"，`delivered` 是"机器收到"）。它的形状应当照抄 §三 那三个现成家。
```

---

## 三、已经存在的可复用资产（这条决定了"要不要新协议消息"的答案）

★ **最重要的发现：这套架构里已经有三个"引用一条消息、带自己状态的小控制消息"的现成家**，
它们都不需要新协议消息、不需要新表：

| 家 | 形状 | 落库 | 可靠投递 |
|---|---|---|---|
| 表情回应 | `kind="reaction"` 的**静默消息**，载荷 `ReactionPayload{target,emoji,add}`（`protocol.rs:843`） | 无独立表，行住在 `messages`，前端 `src/utils/reactions.ts::foldReactions` 折叠 | 与群消息**完全相同**的 outbox + gossip 管道（`group_announcements.rs:163` 原话） |
| 置顶 | 同构 `PinPayload{target,pinned}`，LWW 寄存器 | 同上，前端 `src/utils/pins.ts` | 同上 |
| 撤回 | `RecallPayload{target}` + 权威 G-Set 表 `group_recalled_messages`（`db.rs:510`） | 并把被撤回那一行**原地物化**成 `kind="recalled"` | 同上 |

⇒ **强提醒照这个形状做就是"复用现有消息 ID / Outbox / Ack / 去重 / 加密 / 可靠传输"**（技术约束 1）：
一条 `kind="remind"`（发起）+ 一条 `kind="remind_ack"`（确认）的**静默消息**，载荷带 `{target, actors}`。
"有限重试 + 去重"由 `flush_outbox` + `INSERT OR IGNORE` + `msg_id = SHA-256(sender+nonce+payload)` 天然给；
"乱序 / 旧态覆盖"由 LWW 版本号元组 `(seq, msg_id)` 给（`reactions.ts:44-49` 那条"版本号必须是元组"的既有规矩）。

其它现成件：
- 会话级 Lamport 序号 `db/clocks.rs`（`get_clock/next_clock/observe_clock`）+ 签名覆盖的 `seq` ⇒ 排序不需要新机制。
- 每会话状态列：`messages.status` ∈ `sending|sent|delivered|read|failed`，前端排名 `DELIVERY_ORDER` + `furthestStatus`
  （`src/utils/messages.ts:66-69`）⇒ **强提醒不许写进这一列**（技术约束 2 + 状态语义那条禁令）。
- 能力协商：Hello 带 `content_features` 位图（`#[serde(default)]`、不参与签名），`DM_LEGACY_KINDS` 是**冻结清单、
  永远不许加东西**；1:1 发新 kind 需要对端声明 `CONTENT_FEATURE_FLEX_DM_KIND`（`MERGE` 位蕴含它），
  默认方向＝**不知道就当不支持**（宁可少发，也不要老端丢帧 + 断链）。

---

## 四、六态 → 写入点与证据来源（Phase 1 要把这张表变成代码 + 判据）

| 态 | 唯一写入点（谁点亮） | 证据必须来自 | 明写"不等于" |
|---|---|---|---|
| S0 普通消息已发送 | 现有 `set_message_status("sent")`，**一字不改** | 现有链路写成功 | 与强提醒无关 |
| S1 强提醒待送达 | 本机：remind 行 + outbox 行落库 | 本机 outbox 有这一行 | 对方收到 |
| S2 强提醒已送达 | **接收端** Ack（现有 Ack 帧路径） | 对端删掉自己那份 outbox 行这一事实 | 用户看到了 |
| S3 已触发提醒处理 | 接收端：走完通知那一段后**由接收端自己**记一笔并随回执带出 | 接收端本地记录 | ⚠️ **不等于** `notifyDesktop()` 调用成功（今天那条调用是 `void`，本就不可信）；也不等于已确认 |
| S4 用户已确认知晓 | 接收端**主动点「我知道了」** ⇒ `remind_ack` 帧 | 显式确认帧 | 不等于事情已完成 |
| S5 强提醒失败 | 重试预算耗尽 / 门控拒发 | 本机计数到上限 或 门控那条既有具名拒绝 | 对方拒绝 |

两条设计推论：
1. S3 与 S2 的差别必须落在**接收端的产物**上（否则就是"把本地 API 成功当成对方收到"那条禁令的复发）。
2. `messages.status` 那条单调守卫（`AND status NOT IN ('read','delivered',…)`）**只保护普通消息**；
   强提醒状态要走自己的 LWW（`(seq, msg_id)` 元组）才能满足"已确认不被旧状态覆盖"。

---

## 五、最小变更方案（A 为推荐；两条都不碰普通消息那四态）

| 变更点 | 路线 A（kind + 载荷，**推荐**） | 路线 B（新独立协议帧） |
|---|---|---|
| 发起/确认 | 2 个新 kind（`remind` / `remind_ack`），静默消息 | 2 个新 `Message::` 变体（`protocol.rs` 现 38 个变体） |
| 数据库 | **不加列、不加表、不迁移**（状态从 kind 行折叠出来，照 reactions/pins 的做法） | 同样可不加，但要新落点 |
| 受众 | 载荷 `actors: [device_id]` ⇒ 群"仅提醒指定成员"是**载荷层**的事 | 同 |
| 敏感文件 | 不碰 `protocol.rs` 的线协议定义 ⇒ 不进 L3 | **碰** `protocol.rs` ⇒ 直接 L3、标题必须 `[impact]`、按 `docs/adr/0017` 走 ADR |
| 旧端 | 群侧：kind 是自由字符串 ⇒ 老成员只是渲染退化，不丢帧 | 老端整帧解析失败风险 |
| 与既有纪律 | 符合「别扩散协议消息类型」 | 相反 |

一个 kind 的真实登记面（现算，不是估）：`git grep -c -E "vote" 1cdb42b -- 'src*'` ⇒ **7 个文件**。
所以 A 的改动半径是"每 kind 七处登记 + `notifyUrgency` 那一家加第三个被点名允许的来源 + 通知载荷补 `msg_id`"，
而不是"重写消息系统"（明确落在"暂不纳入"之外）。

**A 里有一条必须先实测再定**：未知 kind 的**降级形状**。前端 `messageKinds.ts::kindClass` 对未知 kind 一律 `bubble`
（注释理由是对普通内容"宁可多显示一条也不静默吞掉"），但**控制类载荷**被当 bubble 显示可能是一坨裸 JSON。
⇒ Phase 0→1 的唯一必做实测项：跨实例 harness 排一封自制 `remind` 帧给按旧词表解析的对端，
读**它自己的**日志与 DB（同 `--round=group-targets-lie` 那条管道），据此决定
"归 Silent + 补一条已知 kind 判据"还是"必须先升级对端才允许发"。**不在这里凭读码下结论。**

---

## 六、兼容策略

1. **1:1 门控**：新 kind 不在 `DM_LEGACY_KINDS` ⇒ 对端必须声明 `FLEX_DM_KIND` 才允许发；
   被挡下时复用现成那句"被门控挡下时给用户的那句话"，**不新造文案家**，也**不许**报成"对方版本旧"
   （INV-P24 已把"未知"与"版本旧"拆成两句话）。
2. **群侧**：受众预告机制（谁该收到）已有；`actors` 名单必须在**成员权限变化**后重新校验 ——
   被移出的人不该再收到提醒、也不该还能确认。现读的家：四类治理帧都在
   `network/transport/group_membership.rs`（`handle_group_rename:6` / `handle_group_member_removed:34` /
   `handle_group_creator_changed:96` / `handle_group_member_left:128`），且**都要求 `from == 本地记录的 creator`**；
   成员表本身是 `INSERT OR IGNORE`（**只增不减**，`db/groups.rs:63`），移除只有 `remove_group_member` 一条 DELETE。
   ⚠️ 已登记的既存边界：**"离线被移除的人不知道自己被移除"** ⇒ 提醒的受众校验不能假设对方已经知道。
3. **数据库**：`DB_VERSION = 11`，迁移是一张 `Migration{from,to,description,run}` **结构体数组**（不是 SQL 字符串），
   按 `PRAGMA user_version` 驱动；**加列**必须新编号一步，而**建表 / 建索引不需要 step**
   （`init()` 在版本分支之前跑 `execute_batch(SCHEMA)`）。失败处理是硬 `Err` ⇒ 应用拒绝启动，
   而**全仓没有 down-migration / 回滚**，唯一的"反向"是 `InitError::Downgrade` 直接拒开。
   ⇒ 所以"给兼容 + 迁移 + 回滚三件套"这句话在**本仓今天只有前两件能做**，回滚那一半只能靠"不改结构"来保证
   —— 这也是路线 A 刻意**不加列、不加表**的根本理由（不是省事，是"回滚"这件事在这套迁移机制里没有实现路径）。
4. **不引入中心服务器 / 远程推送 / 新依赖**（用户明写）。
   ★ 现读更正一处常见误解：**盲中继不是"只搬文件分片"** —— 它**永不解析 Gosslan 帧**（ADR-0020:60），
   链路一旦建立就跑既有 `connect_to_peer` 全流程（`relay.rs:3-4`），上面**什么帧都能走**（消息 / gossip / Ack 都行），
   且 ADR-0020:18 写的是"**无新帧、无新 kind、无 capability 位**"。
   ⇒ 强提醒的离线投递因此**不需要为中继做任何新工作**：它只要变成一条普通消息帧，就能搭上现有链路。
   也顺此说明：中继**不做 store-and-forward**（ADR-0020:67 明确否掉）⇒
   "对方离线时提醒先到哪儿"这个问题的答案只有本机的 outbox / group_outbox，**没有第三方代存**。
5. **架构地图**：本文**不改** `docs/ARCHITECTURE-MAP.html`（Phase 0 没有新 IPC 命令）。
   ⚠️ Phase 1 一旦新增命令或事件名，必须同批改图 + `mapContract.test.ts`（该图在 LIVE_DOCS 名单里）。

---

## 六之二、"旧状态不许覆盖新状态"已经有四个家，不许新开第五个

| 现成的家 | 规则 | 复跑 |
|---|---|---|
| 会话 Lamport 序号 | `next_clock = cur+1`、`observe_clock = MAX(local, observed)`；排序与清空边界**都以它为准，不用墙上时钟** | `grep -n "fn observe_clock" -A6 src-tauri/src/db/clocks.rs` |
| 清空边界 | `group_message_blocked_by_boundary`：逻辑序号 ≤ 清除边界 ⇒ 判成旧历史，**不得重新写入本机** | `grep -n "group_message_blocked_by_boundary" -A10 src-tauri/src/db/group_delete_boundary.rs` |
| 已读水位 | `upsert_group_read` / `pending_reads` / `pending_group_reads` 三处一律 `SET = MAX(...)` | `grep -n "MAX" src-tauri/src/db/read_receipts.rs` |
| 消息级 LWW | 折叠版本号必须是 **`(seq, msg_id)` 元组**（`reactions.ts:44-49`）；前端投递态 `furthestStatus` + `DELIVERY_ORDER` **只进不退** | `grep -n "DELIVERY_ORDER\|furthestStatus" src/utils/messages.ts` |

⇒ 强提醒的"已确认不被旧提醒/旧回执覆盖"应当**复用 `(seq, msg_id)` 那一套**，
而不是再发明一个"比较时间戳"的规则（墙上时钟在本仓已被明确判为不可信来源）。

## 六之三、重连补发是有固定顺序的（这决定提醒什么时候能出去）

现读：链路建好之后依次是
`flush_outbox → requeue_group_keys_for_peer → flush_pending_group_keys → flush_group_outbox →
flush_pending_reads → flush_pending_group_reads → flush_pending_files → flush_pending_group_files`
（触发点：建链 `dial.rs:613-622`、验过的 Hello `handle_identity.rs:86-96`、心跳 `:118-130`、BLE 两处），
而这个**顺序本身是被判据钉着的**（`lib_delivery_shape_tests.rs:190 group_keys_always_precede_group_messages`）。
超时回收每 30s 一次：单聊 120s 死线、群与文件 30min、离线保留窗 7 天（`db/offline_queue.rs:14-32`）。
⇒ 强提醒只要成为一条普通消息帧就自动进这条队列；**不许**为它另起一条"重连时发提醒"的定时器
（那正是"同一件事长第二个家"的形状，且会绕开那条顺序判据）。
⚠️ 另有一条既存且已登记的不对称：`pending_group_keys` **只在内存、不跨重启**
（`pending_keys.rs:5`）⇒ 别把"提醒能跨重启续投"当成理所当然：能跨重启的是 `outbox` / `group_outbox` /
`file_outbox` / `pending_reads` 这些**表里的行**。

## 六之四、E2EE 那一侧的准确说法（关系到提醒帧携带 `msg_id`）

- 群 = **一把共享对称密钥**（`settings['gk:<id>']` + `state.group_keys`），载荷 `crypto::seal_symmetric`；
  谁能解只取决于谁持有那把 key。新成员读不到历史**不是靠轮换**：`group_add_member` 明确**不轮换**
  （注释原话"新成员本来就没有旧密钥，转不转旧消息他都解不开"），轮换只在**移除成员**时发生。
- ⚠️ 一条会把结论掀翻的现读例外：进程内的**重放缓存**会在 10 分钟窗口内把最近 16 条群帧交给刚进来的人
  （`mesh/gossip_replay.rs:9-13,41-43`）。⇒ "晚进群的人拿不到旧内容"这句话**不绝对成立**；
  提醒帧只携带 `msg_id` 与受众、**不携带正文**，正是为了让这条边界不重要 —— 这一点要写进第 1 阶段的设计约束。
- 解密失败 ⇒ **既不落库也不 Ack**（`handle_messaging.rs:92-98`）⇒ 提醒帧的 S2「已送达」不会被一次解密失败点亮，
  这条既有行为正好是需求要的方向，直接依赖它。


---

## 七、★ 现有架构确实无法满足的三处（这三条就是"能不能加字段/协议消息"的判断依据）

| 需求 | 现状（现读证据） | 结论 |
|---|---|---|
| **多设备确认状态合并**（验收项） | 全仓以 `device_id` 为身份：`group_members(group_id, device_id)`、Ack/ReadReceipt 都判 `to != state.device_id`；**`userId` / `same_user` 零命中**，代码里唯一出现"多设备"的地方是日志前缀（`logging.rs:167`）。没有任何"同一个人的多台设备"之间的状态同步机制。 | **真缺口，且不是补一条能填的**。要么定义"同一人"的身份聚合口径（那是 INV 级的身份问题，本仓已定过"设备号只做现场核对、不派生用户身份"），要么第一版把这条验收降级为"每台设备各自确认、发起方看到**每台**的状态"。**这一条需要负责人拍板，我不自行决定。** |
| **强提醒声音可控**（功能 6 的声音那一半） | `notify-rust` 的 `.sound()` **从未被调用**；`new Audio` / `rodio` / 音频资产零命中；设置键只有 `notify_enabled` 与 `notify_show_content`。 | **新能力**（声音资产 + 跨平台播放出口 + 设置键），不是"把现有开关打开"。跨平台可行性**未证**。 |
| **通知点击定位原消息**（功能 3） | 消息级定位链早就通（`locateMessageInConv` → 翻页 → `scrollToIndex` → 高亮 1600ms），但通知载荷只带 `conv_id`；且**点击回调只在 Windows 实现**（`#[cfg(windows)] show_click_impl`），macOS/Linux 把 `on_click` 丢掉。 | 载荷补 `msg_id` 是小改动；**macOS/Linux 的点击通道是缺的**，那半必须实现后才能谈验证。 |

---

## 八、风险清单

| | 风险 | 今天凭什么这么说 |
|---|---|---|
| R1 | 把 S3 挂在本地通知调用成功上 ⇒ **假确认** | `notifyDesktop` 那一条是 `void` 调用 |
| R2 | 去抖合并把提醒吞进「等 N 条」 | `NOTIFY_DEBOUNCE_MS = 1500`、同会话只留最后一条 |
| R3 | 1:1 上老端整帧解析失败**并断链** | `protocol.rs` 里那段 v4.22.34 之前的考古结论 |
| R4 | 「刻意不收第三个」被写宽 ⇒ Dock 一直跳 | `notifyUrgency.ts` 文件注释原话 |
| R5 | Android/iOS 进程被回收后永远收不到 | Android 前台服务**只是 `scripts/android/AndroidManifest.xml:29-64` 的模板注释**，没接进 `gen/android`；iOS 工程从未初始化、无 `UIBackgroundModes` |
| R6 | 被移出群的人仍收到提醒 / 仍能确认 | 群治理四类事件在 `transport/group_membership.rs`，新帧受众必须跟着它校验 |
| R7 | 多设备合并 | 见 §七 第一条（**结构性缺口**） |
| R8 | 重复投递导致反复响铃 | 去重键是 `msg_id UNIQUE` + Bloom/LRU；提醒帧若每次重发**换新 msg_id**，去重当场失效 —— 这是设计约束不是假设 |
| R9 | 门禁与提交纪律 | `protocol.rs` 属敏感文件 ⇒ 触碰直接 L3 且标题必须 `[impact]`；动应用码必须同提交带 `Version-Bump:` + 五处版本号 |
| R10 | 系统勿扰读不到却承诺遵守 | 全仓只有给用户看的"专注助手/勿扰"文案，代码里没有读取；而 `request_attention` 的存在理由正是"不经过通知中心、不受该开关影响" ⇒ 强提醒用 Dock/闪烁时**天然绕过勿扰**，必须在界面与文档里说实话 |

---

## 九、测试矩阵（每平台分「自动化 / 真机 / 未验证」三档）

| 判据 | Windows | macOS | Android | iOS |
|---|---|---|---|---|
| 六态各自独立、传输 Ack 不点亮已确认（单元） | 自动化可覆盖 | 自动化（同一份） | 自动化（纯函数侧） | 无 CI ＝ **不可能已验证** |
| 群聊只提醒指定成员（跨进程） | 现有 `--round=group` 管道可加 | 同左（本机腿） | 真机 | 未验证 |
| 离线 → 重连补发（有界等待 + 对端自己的产物） | 现有 harness 形状可加 | 同左 | 真机 | 未验证 |
| 重复投递幂等（同 msg_id 只生效一次） | 自动化 | 自动化 | 真机 | 未验证 |
| 已确认不被旧状态覆盖（LWW `(seq,msg_id)`） | 自动化 | 自动化 | 真机 | 未验证 |
| 真系统通知出现且点击落到原消息 | **唯一有 `on_click` 的腿** ＋ Smoke-3 人工 | 需先补实现 ＋ 人工 | 人工 | 未验证 |
| 声音 / 振动 | 未证 | 未证（`navigator.vibrate` 不适用） | 权限已声明、后台链路未接 ⇒ 真机 | **结构性做不到**（WKWebView 不支持 `navigator.vibrate`） |
| 权限关闭 / 勿扰 / 后台限制 | 行为明确＝**不弹、不响、不承诺** | 同左 | 同左 | 未验证 |

既有回归层（不许降低）：快速层 / 全量层 / 本地跨进程层 / 发版前层 + 护栏非空转整跑（分母现算
`python3 scripts/verify-guards.py` 自己打印的「起跑前核对：N 条用例」）；
普通聊天、已读回执、E2EE、Outbox/Ack、文件传输、群同步的现有用例是**回归判据**，
强提醒任何一步都不许让其中任何一条变红或变松。

---

## 十、阶段划分与退出判据（每阶段独立实施 / 独立提交 / 独立验收）

- **第 0 阶段（本文）**：✅ 审计 + 设计交付完。**退出判据**＝架构图、最小变更方案、兼容策略、风险清单、测试矩阵
  都在，且 §七 那三处"确实不满足"有现读证据。⚠️ 尚欠一件实测（未知 kind 的降级形状）＋ 一条待拍板（多设备）。
- **第 1 阶段 可靠性核心**：提醒身份、六态、幂等、权限验证 + 单元测试。
  判据形状照 `reactions`/`pins` 那两家（**一次单点变异各红对应那一条**）。**不动界面。**
- **第 2 阶段 离线投递与确认**：复用 `flush_outbox` / `group_outbox`，补跨进程判据（离线恢复、重复投递、
  回执、群成员变化后的受众校验）。
- **第 3 阶段 用户交互与通知**：发起入口、接收标识、确认按钮、点击落到原消息（含 §七 那条 macOS/Linux 缺口）、
  `notifyUrgency` 加第三个被点名允许的来源。
- **第 4 阶段 回归验收**：真实通信链 + 四层门禁 + 每平台三档如实记录，**未验证就写未验证**。

不在任何阶段做：中心服务器、远程推送、第三方依赖、为强提醒重写消息或通知系统。
