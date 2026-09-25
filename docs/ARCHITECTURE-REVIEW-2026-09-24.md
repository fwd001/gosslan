# Gosslan 架构复审（第一性原理级）· 2026-09-24

基线：`origin/main = 94fec6d` = **v4.29.33**。配套阅读：`docs/ARCHITECTURE-MAP.html`（契约图）、
`docs/domains.data.mjs`（领域图）、`docs/migration-ledger.md`（迁移台账）。

约束（不变）：**稳定 > 流畅 > 正确 > 原生体验 > 性能 > 可维护性 > 扩展性 > 新功能**；
本轮**不改任何代码**，只给结论与路线。所有 file:line 都是本次实跑读到的，不是转述。

---

## 0. 一句话结论

现在的架构**没有"错层"**：UI → Store → IPC → Command → Domain → Persistence/Transport 的分层是成立的，
`mesh/` 甚至是全仓最成型的一块。真正的问题集中在**三件事没做完**：

1. **"断一条连接 ≠ 这个 peer 下线"这条规则只贯彻了一半** —— links/在线 已经改了（6b），
   文件接收状态没改（`transport.rs:1548`）。⇒ 「大文件失败拖死聊天」的机制**仍然存在**，
   只是那几个具体触发器（chunk 尺寸、链路固定、attempt epoch）被逐个修掉了。
2. **一条全局 SQLite 连接 + 一把全局锁，是读路径也要抢的** —— 代码自己写了这件事
   （`commands/favorites.rs:404-406`：等锁的 async 任务占住工作线程 ⇒ "界面看起来就是死的"），
   但**读命令至今没有第二条连接**（`db.rs:533` 是唯一生产 `Connection::open`）。
3. **事件被当成事实用** —— 一半以上的状态推进是"只带 id 的事件 + 前端就地改本地状态"，
   而 `Tauri listen 不回放历史事件`（`useChatStore.ts:1731-1734` 原话：永久丢失且无纠正路径）。

这三条都属于"边界没重新定义完"，不属于"边界画错了要推翻"。所以路线是**补完 + 收缩 + 减法**，
不是重写。

---

## A. 最值得解决的结构性问题（按优先级，12 条）

> 每条给：**影响 / 概率 / 改造风险 / 用户体验影响**，以及为什么排在这个位置。

| # | 问题 | 影响 | 概率 | 改造风险 | 用户体验 | 优先级 |
|---|---|---|---|---|---|---|
| P1 | 一条连接断开 ⇒ 该 peer 全部文件接收判死（未按端点隔离） | 高 | **高**（多链路用户每次链路抖动） | 低 | 文件"莫名其妙失败"，而网络其实是通的 | **1** |
| P2 | **TCP writer 把"本地编码/成帧失败"和"socket 真 IO 失败"混成一类** ⇒ 一个自己造出来的坏帧会拆掉整条连接 | 高 | 中（历史上真发生过） | 中 | 传大文件时聊天卡顿/掉线 | **2** |
| P3 | 发送缓冲按**帧数**不按**字节数**限流（1024 × ~350KB ≈ 260–350 MB/链路） | 高 | 中（大文件 + 慢对端） | 低 | 移动端 OOM / 整机变慢 / 进度假快 | **3** |
| P4 | 中继借用接收把**整个文件读进内存**（直连路径是流式的） | 高 | 中（无 LAN 时传大图） | 中 | 手机上收大文件直接崩 | **4** |
| P5 | 单连接单锁：读也要抢锁；三处 N+1/全库形状 + 四条缺索引 | 高 | **高**（每天都在发生） | 低（索引）/ 中（读连接） | 点什么都慢半拍；导出/清理时全 App 冻 | **5** |
| P6 | 事件即事实：错过就永久错；只有设置窗口有重拉兜底 | 高 | **高**（常驻窗口 + 冷启动） | 低 | "已读没变"/"未读数不对"/"气泡一直转圈" | **6** |
| P7 | 传输状态机没有"谁能覆盖谁"的契约（`file_transfers` 无终态守卫、发送侧 `content_transfers` 永不 complete、取消写 `failed`） | 中-高 | 中 | 低 | 同一次传输两处显示不同结果 | **7** |
| P8 | Transport 仍是业务层：`handle_message` 1566 行 / 38 arms / 38 处 `db::` / 17 处 `emit`；4 处**锁内 emit**违反自身约定 | 中 | 高（每次改都踩） | **高**（不能大动） | 间接：修一个 bug 冒另一个 | **9** |
| P9 | 选路只有 rank + 二值健康；拥塞机制**建好没接线**；全不健康时盲发下标 0 | 中 | 中 | 中 | 死链路上白等 45s；BLE 被选中传大文件 | **10** |
| P10 | 死实现 / 平行实现 / **守卫用的"源码全集"靠人记得登记**（漏登记不会红，只会静默假绿） | 中 | 高 | **极低**（纯减法） | 间接：误导下一次修改 | **8** |
| P11 | 每行渲染做线性扫（`nicknameOf` / `groupReaderIds`）+ `peers` 每 300ms 整体替换 | 中 | 高（长群聊） | 低 | 滚动掉帧、切会话后列表一卡 | **11** |
| P12 | 切会话内容要等两次**串行** IPC；缓存内会话显示旧快照且无"可能过期"提示 | 低-中 | 高 | 低 | "点了要等一下才出内容" | **12** |

排序理由（对照 §11 的顺序）：P1–P4 是**故障隔离与稳定性**（同一根因家族：连接/缓冲/内存的生命周期边界）；
P5–P6 是**用户响应速度与状态正确性**；P7 是**正确性**（"显示成功必须真成功"的延伸）；
P10 排在 P8 之前是因为它**风险极低、收益立刻可见**，而且它是 P8 能否安全动手的前提；
P8/P9 是**架构边界**，必须建立在 P1–P7 的回归测试之上才动；P11/P12 是**流畅度末梢**。

---

## B. 根因（每条都到 file:line / 锁 / 队列 / 连接）

### P1 文件接收按 peer 杀，不按 connection 杀

```
reader_loop 退出（**每一条**连接的读半）
  └─ transport.rs:1548  file::fail_receives_for_peer(&state, &peer_id)   ← 只按 peer_id 过滤
  └─ transport.rs:1550-1561  群文件接收同样按 peer_id 全杀
  ──────── 下面才是"断一条 ≠ 下线"的正确逻辑 ────────
  └─ transport.rs:1563-1583  links[peer].retain(|l| !l.low.same_channel(&link_tx))
  └─ transport.rs:1594       只有空了才 links.remove(&peer_id)
  └─ transport.rs:1605-1607  mark_peer_offline 只在最后一个连接死时调用
```

`transport.rs:1563-1565` 的注释把 6b 的语义写得很清楚（"同一 peer 可能还连着别的端点（LAN + Tailscale），
断一条 ≠ peer 下线"），`state.rs` 的 `links: HashMap<String, Vec<Link>>` 也支持多连接；
但 `FileReceiver`（`state.rs:700-718`）**只有 `peer_id`，没有端点/链路身份**，
所以清理函数**无法**按连接收窄 —— 这不是写错，是**数据结构里没有那个信息**。

后果：LAN 与 relay 并存时，relay 电路被 `drop_relay_circuits`/配置变更回收，
正在 LAN 上收的 600MB 文件立刻判死（`fail_receive` ⇒ 删 `.part`? 不删，见 `file.rs:1930-1971`，
但状态已经是 failed）。

### P2 本地编码/成帧失败与 socket IO 失败被混成一类（只在 TCP writer 这一处）

**先把规则说对**（这一条我第一版写歪了，纠正如下）：

```
业务级失败（序列化不出来 / 帧超 MAX_FRAME / BLE 分片非法 / 入队被拒）
    ⇒ 这一帧、这一个 transfer 失败。链路没坏，不许拆链。
真 socket IO 失败（write_all / flush 返回 Err）
    ⇒ 这个 endpoint 已经写不出去了，必须判死并按 endpoint 隔离，
      绝不能"为了保护文件传输"继续复用一条已经失败的连接。
连接失败 ≠ peer 失败 ≠ 该 peer 的所有文件失败   ← 三条都要成立
```

**代码现状：这条规则在三个地方已经建立了，只有第四个地方没有分。**

| 位置 | 是否区分 | 证据 |
|---|---|---|
| 字节层写 | **已区分** | `transport/tcp.rs:38-44`：载荷为空 / 超 `MAX_FRAME` / 超 u32 ⇒ 返回 `ErrorKind::InvalidData`，**在碰 socket 之前**就失败 |
| BLE 写 | **已区分** | `network/ble.rs:1418-1420`：`// 「帧无法分片」是**这一帧**的问题（太大/MTU 异常），重试无意义` ⇒ 直接返回该帧错误，不重试、不拆链 |
| 读侧 | **已区分** | `outbound.rs:24-38` 的 `decode_frame` 文档：未知 type ⇒ `Message::Unknown`，**忽略这一条、链路保持**；已知 type 但字段畸形 ⇒ `Err`。注释里写的正是"旧路径把失败转成 io::Error ⇒ reader 当连接错误 ⇒ 拆链 ⇒ 重连后同一帧再拆"这个 bug |
| **TCP writer** | **没区分** | `transport.rs:1469-1471` 只有 `if res.is_ok() { Ok } else { Failed }`；`Failed` 分支 `:1496-1498` 一律 `mark_conn_failure(peer, endpoint)` + `break` ⇒ 拆掉整条写半 |

⇒ 读侧已经付过一次学费并修好了，**写侧是同一类缺陷的残留**。今天一个 `serde_json` 编不出来、
或载荷长度非法的帧，会被当成"对端写不出去"，代价是整条连接 + 该 peer 的接收侧（见 P1）。

`file.rs:62-73` 记录的那次 BLE 事故（256 KiB 分片需要 18725 个 BLE 分片 > `MAX_BLE_CHUNKS_PER_MESSAGE=8192`
⇒ `fragment()` 返回 `None` ⇒ 被当写失败 ⇒ 拆链）当时修的是**上游**（让分片尺寸匹配链路，对的），
**没有修下游的归类规则**。所以规则仍然是："任何 `write_frame` 返回 Err = 这条连接死了"。

另外：`mesh/connection.rs:105-110` 的 `is_healthy` 现在只看 `last_read_seen_ms`
（写活性刻意不参与，`transport.rs:1302-1306` 说明了理由：半开 TCP 上写会一直"成功"），
`consecutive_failures > max_failures` 那条分支在生产里几乎走不到（`connection.rs:96-101` 自己承认）
⇒ **真正会杀掉链路的只有 writer 的这个 `break`**，所以它的归类必须准确：
误判一次 = 白拆一条好链路；漏判一次 = 往死链路上一直写。

### P3 缓冲按帧数限流

```
transport.rs:1196-1198   每连接三条 mpsc：high / normal / low，容量各 1024
protocol.rs:30           FILE_CHUNK = 256 * 1024（明文）
file.rs:1043-1045        seal_symmetric(+28B) → base64(×4/3) → Message::FileChunk{data: String}
outbound.rs:340-355      FileChunk/GroupFileChunk 一律进 link.low
outbound.rs:332-339      队列满时 send_on_link **无限等**（每 5s 跑一次 stall_tick）
```

⇒ 单链路 low 队列最坏可堆 `1024 × ~350 KB ≈ 350 MB`（代码注释自己用的是 262 MB 明文口径，
`file.rs:1078`、`outbound.rs:290-292`）。多连接、群文件多收件人时按连接翻倍。
"背压"是有的（发送任务会原地等），但**背压的位置在错误的地方**：真正的缓冲已经先分配完了才排队。

### P4 中继借用接收 = 全量进内存

```
file_relay.rs:37   Reassembly { chunks: HashMap<u32, Vec<u8>> }        ← 整个文件的每一片都留在内存
transport.rs:4339  尺寸闸门只有一句：size > i64::MAX as u64              ← 等于没有闸门
transport.rs:4441-4446  add_chunk(...) 完成时返回 (name, expected_size, full: Vec<u8>)   ← 再复制一份整文件
transport.rs:4453 / 4458  full.len() 比对 + sha2::Sha256::digest(&full)
transport.rs:4511  save_received_bytes(state, &name, &full)
```

对照直连接收路径：`file.rs:1575` 建 `{transfer_id}.part`、`write_chunk` 边收边写、
`finish_receiver_into` 用**增量** hasher（`file.rs:2030+`）—— 那条是对的。
⇒ **同一个关注点的第二条实现忘了带上第一条已经建立的流式纪律**，这正是 §9 说的"平行实现"的代价。

**2026-09-25 补两条量化**（原来只写了"全量进内存"，没写清倍数和为什么不能就地照抄直连）：
① 峰值不是 1× 而是 **≈2×**（`chunks` 全量驻留 + 完成时再 `extend_from_slice` 组装一份 `out`）；
② 直连那套流式纪律**搬不过来**：`.part` 的偏移靠 `seq × chunk_size`，而 `RelayFileOffer` 没有
`chunk_size` 字段（`protocol.rs:1246`），发送方那个数还在 4 KiB / 64 KiB 之间按链路挑
（`file.rs:686-698`）⇒ 三个可选形状与各自代价见「第 2 步 · 2」，**待用户拍板**。

### P5 单连接单锁：读也要抢

事实清单（全部实测）：

* 生产环境只有**一条**连接：`db.rs:533`（其余 `Connection::open*` 全在 `#[cfg(test)]` 与 `examples/`）。
  没有 `busy_timeout`。301 处 `db.lock()`，分布在 31 个文件；**43 个文件直接调 `db::`**
  （领域图写"13 个"，已过期）。
* 一次持锁里的 N+1：`commands/files.rs:172-189` `list_group_files` = `1 + 3N` 条语句
  （每个文件 3 次查询：delivery summary / recipient status / transfer path），磁盘 stat 已挪到锁外（对）。
* 跨 IPC 的 N+1：`useChatStore.ts:554-561` `refreshGroups` = `1 + N` 次 IPC，
  每次 `get_group_reads` 单独上锁（`commands/groups.rs:146-158`）。而它被
  `createGroup / addGroupMember / removeGroupMember / transferGroupCreator / leaveGroup / loadGroupTodos`
  同步 await。
* 一次持锁读全库：`commands/favorites.rs:377-380` → `export::collect_sections`（`export.rs:229-266`
  逐会话 `get_messages(conn, id, -1, 0)`）。**代码自己标了这是已知限制并给出理由**
  （`favorites.rs:368-370`：单连接下读必须持锁，分批读要读连接/连接池，属架构级改动）。
* 锁内长操作：`commands/channel.rs:458-460` 与 `:366-368` 的 `VACUUM`（秒~分钟级，
  只在真删了文件时做 —— 这个纪律是对的）。
* 缺索引（建表 vs 实际查询）—— **✅ 0-B 已于 2026-09-25 补完前四条**（第 5 条 `messages(ts)` 判过不做），
  最终形状与实测见「0-B 已落地」那张表：
  | 查询 | 位置 | 现有索引 | 结果 |
  |---|---|---|---|
  | `WHERE msg_id=?1` on `group_recalled_messages` | `db/recalls.rs:22-31`（在 `gossip.rs:796` 每条群收件路径上） | PK `(conv_id, msg_id)`（`db.rs:375-380`） | **全表扫**，且这张表是只增 G-Set |
  | `ORDER BY created_at DESC` on `file_transfers` | `db/file_transfer.rs:75-78` | **零索引**（`db.rs:429-439`，只有 PK id） | 全表扫 + 临时排序 |
  | `WHERE peer_id AND status ORDER BY updated_at` on `content_transfers` | `content/store.rs:149-160` | PK `(cid,peer_id,direction)` + `idx(status)`（`store.rs:32-34`） | peer_id 不是任何索引前缀 |
  | `WHERE peer_id+status+next_attempt_at<=?` on `file_outbox` | `db/file_offline.rs:26-28` | `(peer_id,status)`（`db.rs:455`） | `next_attempt_at` 未覆盖 |
  | `ORDER BY ts DESC` 跨会话搜索 | `db/messages.rs:317-333` | `(conv_id,ts)`（`db.rs:362`） | 无 `messages(ts)` |
* 锁内 emit（把"锁内慢活"请回来）—— **✅ 已修，2026-09-25**：本行当时记的 4 处行号已失效，
  实测 5 处（含 P1 新引入的一处），处置见「第 3 步 · 3」
  —— 而**同一个文件 `:89-90` 写着相反的规定**："锁只圈住写库，emit 一律出锁再做：
  在 db 锁内 emit 会把「锁内慢活」请回来（前端收到 file-failed 后的下一次 IPC 要抢同一把锁）"。
  正确写法在同文件 `:3108-3110`、`commands/group_file_dispatch.rs:181-201` 就有。

### P6 事件即事实

| 事件 | 载荷 | 前端处理 | 错过之后 |
|---|---|---|---|
| `message-acked` | 只有 `msg_id` | `useChatStore.ts:1808-1829` 就地改 status | 只有重开会话（`openConversation` 重读）才纠正 |
| `message-failed` | 只有 `msg_id` | `:1884-1887`，**仅当该会话正活跃且行在缓存里**才更新 | 后台会话永久停在 sending |
| `message-recalled` | 只有 `msg_id` | `:1692-1712` | G-Set 是权威（`db/recalls.rs:6`），重开自愈；开着时是错的 |
| `peer-read` | 结构化但**就地改** | `:1831-1845` | 同上 |
| `file-cancelled` | `transfer_id` | `:1863-1876` | 状态刷新覆盖，标签不覆盖 |
| `file-progress` | 结构化 | `:1644-1652` + `scheduleTransfersRepair`（`:632-638`） | **有修复路径**（唯一一处） |

窗口重拉覆盖（实测）：

| 窗口 | 首次挂载 | 重新获得焦点 | data-cleared | 错过事件 |
|---|---|---|---|---|
| 主窗口 | `app.init`+`chat.init`（8 个 refresh） | **无重拉**（`visibilitychange` 只做 flush+markRead，`:2039`） | 有（`:582-606`） | **无** |
| 设置 | 有 | **有**（`entries/settings.ts:25` → `refreshEnvironment`，且被 `windowEntries.test.ts:235` 钉住） | **无**（禁止 bindEvents） | 无（关即销毁，重开即新） |
| 日志 | 有 | 2s 轮询 ⇒ 天然自愈 | 无 | 自愈 |
| 群任务 | 有（`getGroupTodosContext`+`loadGroupTodos`） | **无**（全仓 `onFocusChanged` 只有 settings 与 TitleBar 两处） | 无 | **无**（错过 `group-todos-target` 就看不到换群） |
| 预览 | 有（pull + 订阅） | 无 | 无 | "当前值"语义缓解（`state.rs:964-969`） |

未读是**两份实现**：后端 `conversations.unread`（`db/conversations.rs:37`）与前端
`conv.unread += msgs.length`（`utils/messages.ts:328`），一致性靠把后端的
`protocol::is_non_notifying_kind`（`protocol.rs:389`）复制一份成 `countsTowardUnread`
（`utils/messageKinds.ts:108`）维持；`messages.ts:314` 已经写下失败模式：
"后端 DB 里未读是 0，两边从此不一致"。

### P7 传输状态机没有覆盖契约

| 阶段 | `messages` | `file_transfers` | `file_outbox` | `content_transfers` |
|---|---|---|---|---|
| 排队 | `sent` | `pending` | `pending` | **无行** |
| 发送中 | `sent` | `active` | `sending`+attempts | `active` |
| 停滞 | `sent` | `active`（**无停滞字段**） | `sending` | `active` |
| 完成 | `delivered` | `done` | **行已删** | **仍是 `active`** |
| 取消 | `cancelled` | `cancelled` | **`failed`（不是 cancelled）** | 不动 |

* `db/file_transfer.rs:21-27` 的 upsert 是**无条件** `DO UPDATE SET status=excluded.status`，
  而 `db/messages.rs:172-179` 有 `AND status NOT IN ('read','delivered','recalled')` 的单调终态守卫。
  ⇒ 终态保护被下放到每个调用方自己记得过滤（`mark_transfer_failed_if_active` 只动 `active` 就是证据），
  所以 `cancel_file_transfer`（`commands/files.rs:462-469`）可以把已 `done` 的行改回 `cancelled`。
* 发送侧 `content::store::mark_complete`（`content/store.rs:218`）**零调用点**（全仓 grep 只有定义）
  ⇒ 发送方向的行永远停在 `active`，靠 `transport.rs:2554` 的运行时过滤器不让它进重试候选。
* `commands/files.rs:278` 的 `fail_file_job` 把破坏性写 `mark_file_outbox_failed` 排在**第一**，
  而 `lib.rs:2807-2824` 的护栏给 `finalize_expired_file` 钉的是**最后**（理由：
  `list_expired_file_outbox` 只扫 `pending/sending`）。两处顺序规则相反，且 `fail_file_job` 里
  后续 `file_transfers` 的 UPDATE 是 `.ok()` 吞掉的 ⇒ 失败就再没人扫它。

### P8 Transport 是业务层（量化）

`network/transport.rs` 9519 行 + 3 个 `include!`（`outbound.rs` 501 / `relay.rs` 845 / `gossip.rs` 900）
≈ 展开 11765 行。`handle_message` = `:2611-4176`，**1566 行 / 38 个 match arm / 无 `_ =>`**，
内部：**38 处 `db::`（19 个不同函数）+ 17 处 `emit`**。整个 transport.rs：173 处 `db::`（54 个函数）、
39 处 `emit`、**0 处 `emit_filter`**、**0 处 `try_lock`**。
它同时是：帧分发器 / 消息与传输状态机 / 队列回收器（`:3286` 直接写裸 SQL `DELETE FROM outbox`）/
好友与群成员策略 / 去重 / attempt epoch / Presence / 已读 / 文件协商 / 事件源。

### P9 选路

`mesh/selection.rs:49-74` 的输入只有两个：`is_healthy`（`connection.rs:105-110`，
= `last_read_seen_ms` 在 15s 内 **且** `consecutive_failures ≤ 3`）与 `path_rank`（`:36-43`）。
**存在但没被选路使用的信号**：`rtt_ms`（`connection.rs:48`；生产里两个 seen 标记都传 `None`
—— `transport.rs:1299`、`:1308` ⇒ 永远为空）、`consecutive_failures` 的梯度、
`ChannelKind` 拥塞时间戳（`connection.rs:119-148` + `peer.rs:187-219` + `manager.rs:148-169`
**整套机制在 `mesh/` 之外零调用者**）、队列长度/容量（1024 固定，无人读 `len()`）、
`peer_content_features`（只用于能力门禁）、`first_seen`（只用于拨号）。
`selection.rs:73`：**全部不健康时返回下标 0** ⇒ 继续往死链路写，等 3×15s=45s 的 watchdog 收尸
（`transport.rs:305-353`）。
`refuse_reason_for_best_link`（`file.rs:101-111`，BLE 且 >16 MiB）只在 `commands/files.rs:348`
用一次并 `continue` 保持 pending ⇒ **它不会改选路，只会拒绝**，群文件路径（`group_file_dispatch.rs:104`）
根本不查它。

### P10 死实现与"第二份登记清单"（实测逐条）

| 东西 | 状态 | 证据 |
|---|---|---|
| ✅ `transport/mod.rs` 的 `Transport` trait / `route_payload` / `LARGE_PAYLOAD_THRESHOLD` / `Channel`（**0-A2 已删除**） | 曾传递性死亡 | `TransportManager::route` 是 `#[allow(dead_code)]`（`:114`）且零生产调用；`route_payload`（`:167`）唯一非测试调用者就是那个 `route` |
| `transport/bluetooth.rs` | 占位 | 文件头 `:8` 自陈 placeholder，`running` 恒 false |
| ✅ `discovery/{trait,manager,lan}.rs`（**0-A2 已删除**） | 曾死亡 | `DiscoveryManager`/`announce_to_candidate` 只出现在自身测试；真跑的是 `network/discovery.rs:311`（由 `network/mod.rs:55` spawn） |
| `discovery/routed.rs` | **只剩活的那一半** | `parse_endpoints`/`ROUTED_ENDPOINTS_KEY` 活着（保留）；未接线的 `RoutedDiscovery` 连同 4 条自身测试已于 0-A2 删除。文件头现在直接写明「这里只负责把配置读出来，不负责发现与拨号」 |
| ✅ `file_relay.rs` **发送侧**整组 API（**0-A2 已删除**，文件 299 → 86 行） | 曾死亡 | `split_bytes/slice_file/slice_file_with/register_send/next_chunk/is_send_done/plan_distribution/ack_chunk/finish_send/progress` 全仓零外部调用 |
| `MeshRouter::select_outgoing` | 死亡（**故意的**） | `outbound.rs:428` 注释说明不用它做群洪泛；只有 `on_receive`/`exclude_source` 活着 |
| ✅ `send_file` / `send_file_relay` 命令 | 前端不可达 | 前端走 `send_file_auto`（`api/index.ts:203`）；`useChatStore.ts:1553 sendFileRelayTo` 零调用者。**2026-09-25 判定：不属本次删除范围**（它们是"门面有包装、界面没人调"这一层，比那 5 条深一级）⇒ 已单独记进契约图漂移清单等拍板 |
| ✅ 5 条注册命令（**2026-09-25 已删除**） | 零引用 | `recall_message / resend_message / cancel_send / send_group_poll / cast_group_poll_vote`（撤回真实走 `recall_group_message`）。注册表 138 → 133 = 前端调用面；两条 `resend_*` 护栏的不变量改记 `docs/protocol-invariants.md` §6 |
| ✅ 投票 | 读写不对称 | 渲染侧活（`utils/cardText.ts:46`、`types.ts:71`），**没有任何 UI 能创建或投票** ⇒ 两条发起命令已删，词表与渲染保留（老消息与对端新版本仍要能显示） |
| `lib.rs:561-615 all_commands_src()` | **靠人记得登记**（今天恰好是全的） | 实测三份视图的分册数：commands 24 / db 16 / transport 3，**都等于各自的 `include!` 闭包减去 `*_tests.rs`**。所以这不是"已经漏了"，而是"漏了不会红"：它已经漏过两次（4.25.0 接线中继时 `commands/relay.rs` 与 `transport/relay.rs` 都只登记了 `include!` 与领域图，现场注释在 `lib.rs:611-613`、`network/mod.rs:229-231`），而漏登记的后果是**静默假绿** —— 以"全部命令面"为判据的守卫扫不到那个分册，永远通过 |
| `useChatStore` 的 7 个导出 | 冗余导出（内部仍在用） | `groupReads/resetAfterDataCleared/refreshTopology/refreshAnnouncements/handleSelfRemovedFromGroup/sendFileRelayTo/enqueueMessage` 外部零引用 |
| 5 个 `src/utils/*.ts` | 只被自己的测试引用 | `a11yLabels/cn/designGuards/templateBranches/tokenContrast`（`cn(` 从未被调用） |
| ✅ `db.rs` 的 `idx_messages_conv_seq`、`idx_outbox_msg_id` | ~~只在迁移里建 ⇒ 新库缺~~ **判断有误**（0-B 实测） | 全新库其实**会**跑完整条迁移链（`is_fresh` 恒为假，见上表 ①），所以新库一直有这两条索引。<b>真实问题在别处</b>：`idx_outbox_msg_id` 与 `outbox.msg_id` 的内联 UNIQUE 是同一件事的两份 ⇒ 已由 v8→v9 删掉；`idx_messages_conv_seq` **只能**留在 v2→v3（它建在迁移才加的列上，挪进 SCHEMA 会让老库开不起来）。**2026-09-25 已按这条结论落地**：它改由 `db.rs::ensure_post_schema_shape`（两条启动分支都经过）补，于是新库不再靠重放迁移拿索引 |

### P11 / P12 前端末梢

* `useChatStore.ts:124-130 nicknameOf` = 两次 `find` 线性扫，而它在模板里被逐行调用
  （`ChatWindow.vue:1195`），`MessageItem.vue:264/868` 也调 ⇒ 一屏 × 好友数 × peers 数。
* `groupReaderIds`（`:608-613`）每行返回**新数组** ⇒ 那个 prop 永远不相等，行组件无法跳过。
* `peers.value = p` 每 300ms 整体替换（`:1760-1775`）⇒ 所有订阅 `peers` 的行一起失效。
* 切会话：`activeConv` 同步置位（`:716`）+ 骨架（`ChatWindow.vue:1160-1173`）是对的，
  但内容要等 `loadMessages` 里**两次串行 IPC**（`:883 getMessageCount` → `:885 getMessages`）；
  `MAX_CACHED_CONVS = 8`（`:829`）内的会话直接渲染旧快照、**没有加载指示**；
  `:889` 读库失败给 `[]` ⇒ "暂无消息"成为假终态（同文件 `:886-887` 注释解释了这个取舍）。

---

### 0-A2 做完后新查出的一条（不在原 P 列表里）

`get_topology` 的 `relay_count` 原先取 `RelayManager::active_sends()`，而 `senders` 这个 map
**只有 `register_send` 会写**、`register_send` 零生产调用点 ⇒ **顶栏「N 中继」永远是 0**。
这是「死实现」最典型的伤害方式：它不是白占几十行，而是**给一个活着的 UI 字段供了一个恒定值**，
于是所有人从界面上读到「没有中继连接」，而真实情况可能是三条。
修法：`relay_count` 改为数 `path_kind == Relay` 的活跃链路；判据与 `relay.connected` 同源
（新增 `state::link_is_relay_circuit` + `relay_circuit_count`，两处消费者共用，不留第三份口径）。
`get_topology` 是**同步**命令而 `links` 是 `tokio::Mutex` ⇒ 用 `try_lock`、抢不到报 0
（顶栏数字不值得为它阻塞工作线程，同「窗口路径装饰性读必须 try_lock」那条规矩）。

## C. 推荐的最终架构方向

### 保持（这些是对的，别动）

1. **一窗一文档一入口 + 固定 label + 常驻/预热**（`boot.ts`、`commands/logs.rs`）。
   跨窗口投递"pending + 仅复用时 emit + 挂载时自取"的形状是对的。
2. **三条优先级队列 + `biased` 严格优先**（`transport.rs:1445-1455`）。实测它**确实**保住了聊天：
   high/normal 不会被 low 饿到。要修的是"low 自己会被无限饿"和"缓冲按帧数"，不是优先级模型。
3. **文件流钉一条链路 + attempt epoch + 单一裁决函数**（`resolve_stream_link`、`decide_offer`、
   `stall_verdict`、`send_retry_verdict`）。这是 600MB 那批故障换来的，方向正确。
4. **`mesh/` 作为纯模型层**（path/endpoint/connection/peer/manager/router/selection/relay_policy 各自独立）。
5. **SQLite 不存 BLOB、二进制全落盘、`.part` 以 transfer_id 为键、rename 才算完成**。
6. **能力位门禁 + "未知 ≠ 版本旧"**；中继哑管道 + 每日旋转通道摘要 + token 不外泄。
7. **护栏体系**（源码守卫 + 变异非空转 + Change Budget + 领域图/依赖方向）。本次复审能落到 file:line
   全靠它。

### 收缩（把职责从 Transport 里拿出去，而不是加新层）

* **收掉"本地可判定的编码/成帧失败"与"socket IO 失败"的混淆**：`write_frame` 的错误要带类型
  （`Encode`/`Framing` vs `Io`），writer 按类型分流 —— **IO 失败仍然判该 endpoint 死**（不许降级成
  普通文件错误去复用坏连接），编码/成帧失败只结束这一帧/这一个 transfer 并记 error 级日志（那是我们
  自己的 bug，不能静默）。这不是新规则：字节层（`transport/tcp.rs:38-44`）、BLE 驱动
  （`network/ble.rs:1418-1420`）、读侧 `decode_frame`（`outbound.rs:24-38`）都已经各自区分了，
  **只有 TCP writer 没有**（`transport.rs:1469-1471`）。
* **收掉"按 peer 杀状态"**：所有 `state.*` 里以 `peer_id` 为键的**进行中**集合
  （`file_receivers` / `group_file_receivers` / `pending_file_accept` / `pending_file_complete`）
  统一带上端点身份，清理时按 `(peer, endpoint)`。
  ⇒ 目标三条同时成立：**连接失败 ≠ peer 失败 ≠ 该 peer 的所有文件失败**。
* **收掉锁内 emit**：`transport.rs` 里那 4 处改成"写库 → 放锁 → emit"，并把这条规则写成护栏
  （现在有 `blocking_commands_run_off_the_main_thread`，但没有"锁内不得 emit"的通用守卫）。
* **收掉尺寸/缓冲的"帧数思维"**：所有面向数据的缓冲用**字节预算**表达。

### 拆分（只做"不改语义的分册"，一次一个域）

* `handle_message` 1566 行 → 按域拆成 `inbound/{chat,file,group,friend,control}.rs`
  （沿用现有 `include!` 机制，**函数签名与调用顺序一字不改**）。
  目的不是好看，是让"改文件路径"不再需要打开 1566 行、也让护栏能按域定位。
* `useChatStore`（2128 行 / 73 导出）→ 按 **Message / Conversation / Group / Transfer / Task / Notification**
  分 store，**但排在 P1–P7 之后**：现在拆会把"事件即事实"的耦合固化到新的边界里。
  先做 P11 的三处微改（O(1) 查表 + peers 增量），它们与拆 store 无关且立刻有效。

### 解耦

* **Persistence**：加**一条只读连接**（或每 worker 一条 read connection），把所有 `get_/list_/search_`
  类命令从写锁上摘下来。这一步解锁三件事：导出不卡消息落库、`list_group_files` 的 1+3N 不再阻塞、
  `VACUUM` 期间的 UI 冻结窗口大幅缩短。`favorites.rs:368-370` 已经把这写成"架构级改动"，
  现在给它一个明确的边界：**写仍单连接单锁，读走第二连接（WAL 天然支持）**。
* **Event vs Fact**：确立一条规则 —— **事件的载荷必须能让接收方"直接应用或重新获取"**。
  具体：只带 id 的事件补成"带最小可应用状态"（如 `message-acked` 带 `{msg_id, status}`），
  或给每个常驻窗口加"重新可见 ⇒ 重拉"的兜底。**两者选一即可，但每个窗口必须有其中一个。**
* **未读**：把"什么算未读"从两份实现收成一份 —— 后端在 `message-received`/`conversations` 载荷里
  直接给 `unread` 值，前端只做显示，不再自己累加。

### 延后（明确不做）

* 不引入 actor/task-per-connection 的新运行时框架；不把 `transport/` 那套"新栈"接起来（**删**它）。
* 不把 SQLite 换成别的、不做分库、不上异步 IO（rusqlite 同步 + 读连接已够用）。
* 不做"智能选路"的新信号采集（RTT 探测、队列深度上报）—— 先把**已有**的拥塞时间戳接进选路，
  且必须证明 LAN-only 场景零额外探测、零抖动。
* 不动 `protocol.rs` 的帧词表与 `crypto.rs`（敏感文件，且当前没有证据说它们错）。
* 不新增任何业务功能（含投票：要么补 UI 要么删接口，不做第三份）。

---

## D. 安全改造路线（每步只动一个领域）

> 铁律：**一次一个领域；每步 修改 → 新增能红的测试 → `npm run verify` → 需要时 `verify:full` →
> CHANGELOG → 版本 → 下一步。** 禁止一次同时动 Transport + DB + Store + UI + Protocol。
> 顺序刻意让**风险最低、可回滚性最高**的先走。

### 第 0 步 · 减法与增量（**两组，分开验证；不统称"零运行风险"**）

**0-A 纯删除 —— 低风险。失败模式是"编译不过 / 守卫变红"，当场暴露，运行时行为不变。**

| 动作 | 领域 | 验收 |
|---|---|---|
| ~~0-A1~~ **已完成**（`CHANGELOG` 的 2026-09-24 Test 小节）：新增守卫 `guard_source_views_register_every_include_subfile`，从入口文件递归展开 `include!` 得到编译器的真实集合，与守卫登记清单**双向比**（少登记=假绿、多登记=假红，两个方向都红），每个用例配一枚自己的 canary | 护栏 | 三条变异证明全部「改坏即 FAIL、恢复即 PASS」并已登记进 `verify-guards.py`。**这一步排在最前**：它决定第 1–7 步所有源码守卫的证据是否可信 |
| ✅ **0-A2 已完成** | 死实现 | 删了 `discovery/{trait,manager,lan}.rs` + `RoutedDiscovery`；`transport/mod.rs` 的 `Transport`/`route`/`route_payload`/`Channel`/`LARGE_PAYLOAD_THRESHOLD`（并把 `lan.rs`/`bluetooth.rs` 的 trait impl 收成固有 impl）；`file_relay.rs` 发送侧整组。`MeshRouter::select_outgoing` **不标 deprecated、也不加 `#[allow(dead_code)]`**，而是写清「生产不走 + 为什么」（那个 allow 会静音编译器本会给的提示，正是 `transport/bluetooth.rs` 头部注释警告过的机制）。同步了 `domains.data.mjs` notes、`migration-ledger.md` 第 1/4/8/9 行与统计与 §2、契约图对应条目 |
| ✅ **0-A3 已完成（含 2026-09-25 的删除拍板）** | IPC 契约 | 10 处旁路 invoke 全收进 `src/api` 门面并补 6 条包装（`copy_file`/`copy_file_to_clipboard`/`save_data_file`/`read_clipboard_file_paths`/`get_group_file_delivery_summary`/`log_frontend_error`）；两条新守卫（门面唯一性 · 注册表逐条对账）；那 5 条零引用命令**已连实现删除** ⇒ `DEAD_COMMANDS` 白名单为空、`generate_handler!` 133 条与前端调用面差集为空。⚠️ 顺带暴露更深一级：**5 条包装没有 UI 调用点**（`send_file`/`send_file_relay`/`search_messages`/`get_interface_candidates`/`close_log_window`），对账守卫只比"注册↔门面"，看不见这层 ⇒ 待拍板 |

**0-B 数据库索引 —— 低风险但**不是**零风险：走迁移路径、有写放大、影响启动耗时，必须单独验证。**

| 动作 | 领域 | 验收（四条全做完才算完成） |
|---|---|---|
| 0-B1 补 4 条索引：`group_recalled_messages(msg_id)`、`file_transfers(created_at)`、`content_transfers(peer_id, status)`、`file_outbox(status, next_attempt_at)` | Persistence | ① **迁移**：老库 v8→v9 建索引成功、新库幂等、**降级守卫仍拒绝高版本库**；② **`EXPLAIN QUERY PLAN` 写成可跑断言**（这 4 条查询不再出现 `SCAN`），不是人眼看一次；③ **写性能回归**：`file_transfers`/`content_transfers` 是高频 upsert（进度每 250ms 一次），加索引有写放大 ⇒ 批量插入计时 before/after 对比，确认没把写路径拖慢；④ 索引进 `SCHEMA` 常量还是走 `MIGRATIONS`，**两处不能都写也不能都不写** |
| 0-B2 把 `idx_messages_conv_seq` / `idx_outbox_msg_id` 并回 `SCHEMA` 常量（修"全新库跳过迁移 ⇒ 缺索引"） | Persistence | 新库首启即有索引、老库幂等；顺带判定 `outbox.msg_id` 的重复唯一约束（内联 UNIQUE + 迁移建的唯一索引）要不要一并收掉 |

> 为什么必须拆开：0-A 的失败在**编译期**露头，0-B 的失败在**真机运行期**露头（老库迁移卡住、写路径变慢），
> 而后者正是本项目排在第一位的东西。两者混在一次提交里 = 出问题时分不清是谁。

**✅ 0-B 已落地（2026-09-25），四条验收逐条对完，并且推翻了两处原判断：**

| 验收 | 结果 |
|---|---|
| ① 迁移 / 新库幂等 / 降级守卫 | 新库幂等 ✓（`fresh_db_has_every_hot_query_index`）、老库补齐 ✓（`schema_alone_repairs_a_current_database`，版本保持最新 ⇒ 只可能是 SCHEMA 干的）、降级守卫仍拒绝 ✓（既有两条 `downgrade_*` 用例全绿，它们按 `DB_VERSION` 动态取值所以随版本 8→9 自动继续生效）。**顺带查出 `is_fresh` 恒为假**：`pre_table_count` 在 `execute_batch(SCHEMA)` 之后才数 ⇒ 新库会重放整条迁移链，首次启动因此打出一串**假的**「正在迁移 v1→v2…」日志。原判断「新库缺 `idx_messages_conv_seq`」因此**不成立**（它一直在，靠的是这个巧合）。**⇒ 2026-09-25 #46 已修**：数表挪到 SCHEMA 之前 + 新增 `ensure_post_schema_shape` 收那笔差额。恒等判据 `fresh_schema_alone_has_exactly_the_migrated_shape` 第一次跑就红在 `index idx_messages_conv_seq` 这一项 —— 正好证明「这个巧合」当时是**承重的**：直接跳过迁移会让新安装的那条热查询退回全表扫。 |
| ② `EXPLAIN QUERY PLAN` 写成可跑断言 | ✓ `hot_queries_use_their_indexes` 把 5 条生产 SQL 本体钉成计划断言（判"走没走索引"，不判"索引在不在"）。 |
| ③ 写性能回归 before/after | ✓ 实测（2000 行 × 21 次真实 upsert，A/B **换序各跑一轮**以排除缓存预热偏差）：`file_transfers` 883/890 → 903/911ms（噪声内）；`content_transfers` 1939/1963 → 2282/2290ms（**+17%**，与顺序无关）⇒ 折算**每个进度 tick 多约 8µs**（tick 间隔 250ms）。库文件 618,496 → 724,992 字节（+53 B/行）。结论：接受。<br>⚠️ 没有把耗时写成断言（CI 上必飘），钉的是**每表索引数量封顶**（`hot_tables_carry_exactly_the_intended_indexes`）—— 写放大的确定性代理。`dbstat` 在本构建里读不到（实测报错），空间成本改用文件大小量。 |
| ④ "两处不能都写也不能都不写" | 规则**成立但边界不是原来说的那个**：形状归 SCHEMA 的前提是"这一列已经存在"。把 `idx_messages_conv_seq` 并进 SCHEMA 后三个老库升级用例全红 —— SCHEMA 跑在迁移**之前**，而 `seq` 是 v2→v3 才 ADD 的列 ⇒ 老库 `no such column: seq`，`init()` 直接失败 = **用户打不开自己的数据库**。所以最终形状是：4 条新索引进 SCHEMA（`content_transfers` 那条进它自己的 `ensure_schema`），**"删"被取代的索引**（`idx_outbox_msg_id`、旧 `idx_file_outbox_peer`）才是迁移 v8→v9 的活；`idx_messages_conv_seq` 留在 v2→v3 里（建列之后）。判据测试：`index_on_a_migration_added_column_must_not_live_in_schema`。 |
| ⑤（追加）0-B2 的重复唯一约束 | 已收掉：`outbox.msg_id` 的权威是**内联 UNIQUE**，`idx_outbox_msg_id` 是热表上白付的一份写放大 ⇒ 迁移删掉，并且测试证明删后重复插入仍被挡（`superseded_indexes_are_dropped_and_uniquity_survives`，同时证明第二次启动 SCHEMA 不会把它造回来）。 |

新增用例 6 条（Rust 基线 661 → **667**）。**没做**的一件事：`ORDER BY ts DESC` 跨会话搜索缺 `messages(ts)`
（P5 表里的第 5 行）—— 它不在 0-B1 的四条里，且跨会话搜索走 `search_chat_history`，量级未测就先不加索引。


### 第 1 步 · 连接生命周期与故障隔离（**P1 + P2**，一个 PR 家族）

> **进度（2026-09-25）**：**P2 已落地**（下面第 1、3、4 条 + 一条双变异用例；`write_frame`
> 改成返回 `WriteError::{Local, Socket}`，`Local` 丢帧 + error 留痕但**链路保留**，
> `Socket` 行为一字未变仍然判死拆链）。
> **P1 未动**，卡在一个必须先答的前提上：**延后清理之后，谁回收喂不到分片的接收器？**
> 现状是既没有接收侧 idle TTL，而 `sweep_stale_parts` 反过来把内存里的接收器当"活跃"**永久保护**
> （它只删 24h 以上、且不在表里的 `.part`）。直接把第 2 条改掉 = 把"杀错人"换成"泄漏"。
> 两个候选：给 `FileReceiver` 加 `last_chunk_ms` + 复用某个周期任务做 TTL；
> 或者只在 `peer_now_offline` 为真时清（其余交给发送侧的 stall/outbox 重连续传）。
> 判完再动 —— 这一条不许和 P2 混在一个 commit 里。

1. ✅ 先写**能红的测试**（改成"分类用真链路证、接线用源码护栏证"，因为 `writer_loop` 吃
   `Arc<AppState>` 造不出来）：`oversize_frame_is_a_local_failure_and_writes_nothing`
   （断言超长帧归 `Local` **且对端一个字节都没读到**）、
   `socket_write_failure_is_classified_as_socket`（反向：真 IO 失败仍归 `Socket`）、
   `writer_loop_splits_local_from_socket_failure_exactly_once`（判死点全函数只有一处且不在 `Local` 分支）。
   另登记 `verify-guards.py` 双变异：折回旧的 `res.is_ok()` 一把抓必须红，把 `Socket` 也放过必须红。
2. ⬜ `FileReceiver`/群接收器加端点身份；`fail_receives_for_peer` → `fail_receives_for_endpoint(state, peer, endpoint_key)`；
   调用点从 `:1548` 移到 `:1563-1583` 的"确认这条连接确实没了"之后。（**行号已复核为 2026-09-25 实测**：
   `file::fail_receives_for_peer` 唯一调用点在 `transport.rs:1548`，紧跟其后的群接收器清理 `:1551-1562`
   是**同一形状的 peer-wide 清理**，两处要一起改；而"只删这一条连接"的正确逻辑在 `:1563-1603`，
   注释里已经写明「断一条 ≠ peer 下线」—— 也就是说文件那两处与它**自相矛盾**。）
3. ✅ **写失败按错误来源分流**（不是按帧类型分流！）：`write_frame` 返回带类型的错误，writer 按类型决定 ——
   * `Encode`/`Framing`（序列化失败、载荷超 `MAX_FRAME`、BLE 无法分片、入队被拒）⇒ **只结束这一帧 / 这一个
     transfer**，链路保留，并记 **error** 级日志（那是我们自己的 bug，绝不能静默降级成"文件失败"就完事）。
   * **真 socket IO 失败（`write_all` / `flush` 返回 Err）⇒ 仍然判该 endpoint 失效**：`mark_conn_failure`
     + 拆这条写半。**不允许"为了保护文件传输"继续复用一条已经写不出去的连接。**
   实现上这是**补齐一致性**，不是新发明：`transport/tcp.rs:38-44`（本地非法载荷 → `InvalidData`）、
   `network/ble.rs:1418-1420`（"帧无法分片"是这一帧的问题，不重试不拆链）、读侧 `decode_frame`
   （`outbound.rs:24-38`，未知 type ⇒ 忽略这一条、链路保持）三处都已经是这个规则，
   **只有 TCP writer 还停在 `res.is_ok()` 一把抓**（`transport.rs:1469-1471`、`1496-1498`）
   —— ✅ **这一处已改完**（2026-09-25）。落地形态比原计划更进一步：不是让调用方嗅 `io::ErrorKind`，
   而是 `write_frame` 直接返回 `WriteError::{Local, Socket}`，**因为只有产出错误的那一层
   知道自己有没有碰过 socket**（`tcp::write_bytes` 的长度校验发生在写之前，`write_all` 之后才失败
   的一定是 socket）。
4. 三条不变量都要有测试：**连接失败 ≠ peer 失败 ≠ 该 peer 的所有文件失败**（⬜ 这条属于 P1，
   还没做）；✅ 外加一条反向测试：**真 IO 失败必须仍然导致该 endpoint 判死**
   （`socket_write_failure_is_classified_as_socket` + 源码护栏里"判死点只有一处、且不在 Local 分支"）。
5. 回归：`verify:full` + 真机（用户）双链路场景。**回滚**：改动集中在清理函数签名与错误类型枚举，可单独 revert。

### 第 2 步 · 缓冲与内存（**P3 + P4**）

> **进度（2026-09-25）**：**P3 已落地**（1. 那一条，只改容量语义、优先级模型一字未动）。
> **P4 未动**，因为它的两个方案语义相反、要你拍板（见 ②下面那条）。

1. ✅ low 队列改成**字节预算**：`LINK_QUEUE_BYTE_BUDGET = 8 MB`，`low_queue_slots(chunk)` 折算槽数
   （LAN 24 槽 / BLE 仍 1024 / 下限 8 防 `channel(0)` panic），四个建链点统一 `link_channels`。
   实测依据：一片 LAN 分块上线 = `base64(256 KiB + 12 + 16) ≈ 341 KB` ⇒ 旧形状最坏 ~350 MB/链路。
   ⚠️ 没测吞吐影响（不猜，交给真机）。
2. ⬜ 中继借用接收的全量内存怎么收 —— **待拍板**。2026-09-25 把三件事查实了，选项因此从两个变成三个：

   **实测的现状代价**：收 F 字节要 **≈2F 堆内存** —— `chunks: HashMap<u32, Vec<u8>>` 先把整文件
   全量驻留，`add_chunk` 完成时**再组装一份** `out`（`file_relay.rs:104-112`）。尺寸闸今天只有
   `size > i64::MAX`（`transport.rs:4409`）= 8 EiB，等于没有。手机收一张经邻居借用的大图就是 OOM。

   **决定性约束（原本没记）**：`RelayFileOffer` 只带 `size` 与 `total_chunks`，**没有分片尺寸**
   （`protocol.rs:1246-1255`），而发送方是按"邻居里有没有 BLE"在 `BLE_FILE_CHUNK`(4 KiB) 与
   `MIN_CHUNK_SIZE`(64 KiB) 之间挑的（`file.rs:686-698`）⇒ 接收方**推不出** `seq × chunk_size`
   这个偏移（`size.div_ceil(total_chunks)` 与发送方那个数没有恒等关系），所以"直接复用
   `begin_receiver` 流式写 `.part`"**不是零协议变更**。

   | 选项 | 内存 | 磁盘 | 协议 | 对用户可见 |
   |---|---|---|---|---|
   | **A 真流式 `.part`（按 seq seek）** | ≈0 | 1× | **要加 `chunk_size` 字段** ⇒ INV-P24 版本门控 + 老端回落分支（两条路径并存） | 大文件能收，静默可续传 |
   | **B `RELAY_RECEIVE_MAX_BYTES` 尺寸闸** | ≤N | 不变 | 不动 | **明说"经邻居借用只能收 ≤N MB"**（功能收缩） |
   | **C 落盘暂存 + 完成时按 seq 重装**（内存只留 `seq→offset` 索引） | ≈一个分片 | **完成期 2×**（暂存 + 成品） | 不动 | 大文件能收，代价是慢一点、可能先撞磁盘 |

   我倾向 **A**：它是唯一"两种资源都不放大"的形状，而它缺的就是那一个字段；B/C 都是拿一种
   资源换另一种。但 A 触协议、要留老端回落 ⇒ 是这七步里最大的一刀，且"C 比 A 小"这个直觉
   在磁盘那一栏上不成立。**这一条我不替他选**：三个方案的用户可见行为各不相同。
3. ⬜ 测试：2 GB 声明尺寸下 `RelayFileOffer` 不分配全量内存（可用"只发 offer + 一片"的构造测出分配行为）。

### 第 3 步 · Persistence 读路径（**P5 剩余**）

1. `db.rs` 增加只读连接（`open` 同一路径 + `query_only`），`AppState` 加 `db_read`；
   把所有**纯读命令**逐个切过去（一次一批：先 `list_*`/`get_*` 的窗口路径，再 `search_*`，最后 `export`）。
2. `list_group_files` 的 1+3N 收成 3 条批量查询（`WHERE transfer_id IN (...)`）；
   `refreshGroups` 的 N+1 IPC 收成一条 `get_group_reads_batch`。
3. ✅ **锁内 emit（2026-09-25 已落地，第 3 步里唯一不需要拍板的一刀）**：报告写的"4 处"实测是
   **5 处**，且那 4 个行号已被 9-25 这两天 P1/P2 的移动改废 —— 重新逐处读码定位后得到：
   `file.rs::fail_taken_receive`（**P1 自己新引入的那一处**）、`transport.rs` 群 Ack 清 outbox 一处、
   中继收文件失败分支三处。五处都改成「`{ }` 圈住写库、emit 出锁」，写序与终态一字未动。
   通用护栏没有做成"第五处到第五处"的点守卫，而是新脚本 `scripts/check-lock-scope.mjs`
   （挂进 `npm run verify` 快速层）：实测 **292 个取锁点全部可判定**（281 guard 绑定 +
   11 语句级临时量），判不出作用域的直接算红。不变量本文 = **INV-P25**（§25，原「必测矩阵」
   顺延为 §26），非空转由 7 段夹具自证 + 一条 `verify-guards` 变异用例（把 emit 注回锁内）钉住。
4. 测试：`export_chat_text` 期间另一线程做一次 `insert_message`，断言等待时间上界（现在无上界可测）。

### 第 4 步 · 状态机契约（**P7**）—— ✅ 已收口 2026-09-25（四刀，详见下面每条的实测更正）

1. ✅ **已落地 2026-09-25，但本条原本给的写法被证伪**：39 个写入点（实测 active 12 /
   failed 12 / done 6 / pending 3 / sent / cancelled）先枚举完才发现 ——
   `NOT IN ('done','failed','cancelled')` 会**打断断点续传**：`retry_incomplete_content`
   复用同一个 `transfer_id` 发 `ContentRequest`（`transport.rs:2644`），把 `failed` 钉死
   就等于"一判死永远停在失败、而字节还在流"。真正不可降级的只有 `done`（唯一有磁盘证据的
   状态）。闸门落在 `upsert_transfer` 与新的 `mark_queued_transfer_failed` 两处 SQL 上，
   判据一正一反（反向那条今天绿、存在的意义就是让下一次"顺手扩大集合"变红），
   另有一条 `terminal_status_writes_have_one_home` 挡住第二个家。本文 = **INV-P26**（§26，
   原「必测矩阵」顺延 §27）。
2. ✅ 判定为**判断有误**（2026-09-25 枚举证伪）：`mark_complete` 全仓零调用点，而发送侧
   一直在记完成 —— 走的是 `record_local`（实测 `Direction::Send` 3 处 / `Receive` 3 处）。
   所以这不是"漏接线"，是一条长得像正主的岔路 ⇒ 按 0-A2 的纪律直接删。
3. ✅ `file_outbox` 取消写 `cancelled` 而不是 `failed`（2026-09-25）：`cancel_file_transfer` 的
   注释本来就写着这个口径，代码调的却是 `mark_file_outbox_failed` —— 注释与代码相反。
   新增 `mark_file_outbox_cancelled`；`mark_queued_transfer_failed`（自动判死）仍写 failed，
   两个口径不许合并。三条队列查询只认 pending/sending ⇒ 功能等价、台账不等价，
   所以先用 `cancelled_file_outbox_rows_are_never_requeued` 证明"写进去就是永久出局"再引入。
4. ✅ `fail_file_job` 的**写序**已对齐（2026-09-25 第三刀）：把行踢出重试集合的那一步挪到所有
   面向用户的写之后，`cancel_file_transfer` 同病同治。判据从"点名 `finalize_expired_file`"
   升级成**自动扫全部收尾路径**（结构式，不靠清单），首次跑就自己找出那两处没被点名的违规。
   ⚠️ 上一刀的 emit 抑制**仍无行为级测试**（函数吃 `&AppState`，本仓造不出来），只有 SQL 层
   判据 + 读码保证。
5. ✅ 三份收尾**已合并成一个出口**（2026-09-25 第四刀）：`db::finalize_file_failure(conn, id, end)`，
   `end ∈ {Expired, GiveUp, Cancelled}`。合并时又收掉两件事：清扫器缺 `done` 闸门（红测试实测到）、
   清扫器多写一句 `gfile-`（潜伏缺陷，今天撞不到：群文件不入 `file_outbox`，且 `set_message_status`
   拒绝把 delivered/read 改回失败）。口径差别收敛成一处 `if cancelled`，两个方向各有判据。
   ⚠️ 合并的取舍记在函数注释里：台账那笔走 `upsert_transfer` 而不是"回报改了几行"的助手，
   因为后者在台账行不存在时会让关行条件永不成立 ⇒ 清扫器空转。因此 `mark_queued_transfer_failed`
   变成零调用点，一并删（留着就是第二个家）。
   合并的形状是一个 `db::finalize_file_failure(dbc, transfer_id, kind) -> bool` owning 全部四笔写，
   但它必须先回答"群气泡 `gfile-` 该不该被单个收件人的失败改写"—— 今天 `fail_file_job`
   **只写 `file-`**，看着像缺口，其实很可能是有意的（N 个收件人共用一条气泡）。
   语义没确认之前不动，这是本条存在的原因。

### 第 5 步 · 事件与自愈（**P6**）—— 1 与 4 已落地 2026-09-25，且第 4 条的前提被推翻后重做过

1. 规则化：**每个常驻窗口必须有"重新可见 ⇒ 重拉"或"事件带可应用状态"**。
   先补最省的两个：主窗口 `visibilitychange`/`focus` ⇒ `refreshConversations + refreshTransfers`；
   群任务窗口 `onFocusChanged` ⇒ 重拉当前群。
2. 只带 id 的事件补最小载荷（`message-acked{status}`、`message-failed{reason}`、`file-cancelled{status}`）。
3. ✅ **已落地 2026-09-25，但这条的前提被核伪**：后端 `mark_read` 就是 `UPDATE … SET unread = 0`，
   `totalUnread` 也只由 `conversations[].unread` 求和 ⇒ 真相源本来就一个，不需要"后端给值"。
   真漏的是**过渡竞态**：乐观清零之后，一份**清零前发起**的快照落地会把红点点亮回来
   （`StaleGuard` 挡不住 —— 请求确实是最新那次，数据是旧的）。现在由
   `applyConversationSnapshot` 只豁免 `unread` 一个字段解决，本地清零收敛成唯一入口
   `clearUnreadLocally`（改内存与打水位同时发生），并修掉一处"回调里才读 activeConv"的闭包错误。
4. ✅ 测试：结构判据已加，但**"只钉了设置窗口"这件事比原先记的更糟** ——
   `AUX_WINDOWS_RESIDENT` 早就是 `false`（设置/日志关闭即销毁），那条守卫钉的是一扇
   已经不常驻的窗口，而真正常驻的 main / tasks 一个都没被覆盖。新判据改为**现场从 Rust 读
   `AUX_*_RESIDENT`** 并强制登记，形状与第 4 步那条自动扫同构（点名式判据会因为翻一个常量
   而静默失效，这是本仓第三次撞上同一件事）。

### 第 6 步 · 选路（**P9**，必须最后做）

> **进度（2026-09-25）**：第 1 条**判为不成立，已删**；第 2/3 条照旧，但落点变了。

1. ❌ **"全不健康返回下标 0 ⇒ 不往已知死链路写" —— 这条把 `pick_link` 当成了最终决策点，而它不是。**
   实测调用链：`try_send` / `send_*` 用的是 `route_order(...)`（`outbound.rs:142-178`），
   它拿 `pick_link` 的下标**只做一件事** —— 决定谁排在尝试顺序的第一位，
   **其余链路全部跟在后面做 failover**（`order.push(best)` 之后 `for i in 0..len` 逐个补进去）。
   所以按原建议把 `pick_link` 改成返回 `None`，`route_order` 会走 `else { return Vec::new() }`
   ⇒ **空顺序 = 一条都不试**，"全部不健康"从"照旧逐条试"变成"直接发不出去"，
   恰好是 ADR-0014 §3.2-4「保持可用优于报错」和既有测试
   `route_order_keeps_all_links_when_none_healthy` 一起守着的那条行为的**反面**。
   ⇒ 结论：不改返回值。真要改善"都不健康时先试哪条"，那是第 2 条（次级键）的范围。
2. ⏸ **建议先不做**（第 1 条塌了之后，这一条的收益没有证据了）。可用来排序的信号确实齐了
   （`ConnectionHealth`：`rtt_ms` / `last_read_seen_ms` / `consecutive_failures` /
   `last_prio_congestion_ms` / `last_bulk_congestion_ms`，注意拥塞要按**消息类别**选键 ——
   控制消息看 prio、文件分片看 bulk），但改的是发送热路径的**决策顺序**，而今天它唯一的可疑行为是
   "同 rank 内先试插入序第一条"：链路的 rank 已经把 LAN/VPN/中转/蓝牙分开，且 **P3 已把每条 low
   队列按 8 MB 封顶** —— 原本"砸进拥塞链路会无限堆内存"那条病理已经被 P3 拿掉。
   ⇒ 要等一个具体证据再做（例如同 peer 双 LAN 链路实测出现"一条堵死另一条空闲、消息白等"），
   不要为了"看起来更聪明"改热路径。
3. ⬜ 断言必须包含：**LAN-only 场景下候选顺序与今天完全一致、零额外探测**（否则不合并）；
   另加一条：`route_order` 的输出**永远是 `links` 的一个全排列**（不许因为健康度变短），
   这条能把第 1 条那种"改返回值"的错法直接挡在测试里。

### 第 7 步 · 边界与前端（**P8 + P11 + P12**）

1. `handle_message` 按域分册（纯搬家，零语义变化），每册一次提交；
2. 前端三处 O(1) 微改（`nicknameOf` → Map computed、`groupReaderIds` → 索引化、`peers` 增量合并）；
3. 切会话两次 IPC 合成一次；缓存会话加"可能过期"指示；
4. **最后**才评估 `useChatStore` 拆分（此时前 6 步已经把跨域耦合拆开了，边界才看得清）。

---

## E. 与未完成任务的合并（后续计划）

| 任务 | 与新计划的关系 | 合并到 |
|---|---|---|
| **#25 文件传输链自动化回归**（进行中） | **就是第 1/2/4 步的证明手段**。原计划的四项（多文件同发 / 大文件+建群并发 / 两端重启 / Ack 丢失）里，"大文件+建群并发"与"两端重启"直接服务 P1/P2/P7 ⇒ **改序：先写 P1 的红测试，再补这两项** | 第 1、2、4 步 |
| **#30 移动端群已读"不见了"** | 与 **P6 同根**（只带 id 的事件 + 就地改 + 无重拉兜底）。按第 5 步做就顺手解掉，不必单独再查一次 | 第 5 步 |
| **#27 移动端首屏样式崩** | 独立缺陷，但**P4（移动端全量内存）与它同属移动端稳定性**，建议排在第 2 步之后一起做真机取证 | 第 2 步之后 |
| **#26 设备身份撞号** | 与结构改造无耦合（identity 域边界本来就清楚） | 独立小批，随时可做 |
| **#18 A1-L2 回执链** | **P6 走"事件带可应用状态 + 重拉兜底"之后，回执链的必要性下降** ⇒ 维持"默认不做"，等第 5 步落地再判 | 第 5 步之后重判 |
| **#32 默认头像 / #35 原生感 / #36 Win 内置 WebView2** | 全是新增/打包类，按 §C"延后"与优先级顺序排在结构改造之后 | 第 7 步之后 |
| **架构图查出的 13 条漂移** | 其中 8 条（死接口、登记清单无守卫、索引缺失、注释与代码相反、领域图数字过期）就是**第 0 步**的内容 | 第 0 步 |

### 建议的下一刀

**0-A（纯删除，编译期暴露问题）→ 0-B（索引，单独跑迁移与写性能回归）→ 第 1 步（P1+P2 故障隔离）**。
0-A 与 0-B **不要合并成一次提交**：前者失败在编译期、后者失败在真机运行期，混在一起出问题分不清是谁。
~~0-A1~~ **已做完**：三份守卫视图今天实测都是全的，所以这一步的价值不是"补漏"，而是**让下一次漏登记必须红**
（历史上它漏过两次，且漏的时候是静默假绿）。第 1–7 步所有源码守卫的"绿"都建立在这份证据链上。

### 明确不做（本轮已定，防止后面反复）

* **不引入 Actor / task-per-connection 框架**，不接 `transport/` 那套新栈（**删**它），
  不做协议词表改造，不换数据库。
* **不把"真 socket IO 失败"降级成普通文件错误** —— 坏连接必须判死，隔离靠 endpoint 维度而不是"忍着复用"。
* 不做没有现成信号支撑的"智能选路"（先接线已有拥塞时间戳，且必须证明 LAN-only 顺序零变化）。

### 修订记录

* **2026-09-24 用户纠正 1**：P2 的修法第一版写成"FileChunk 写失败只结束传输、不再判连接失败"——
  错。正确规则是**按错误来源分流**：本地编码/成帧失败不拆链，**真 socket IO 失败仍判该 endpoint 死**。
  已改写 §A(P2) / §B(P2) / §C(收缩) / §D(第 1 步第 3–4 条)。
  附带收获：按这个口径重读代码，发现该规则在 `tcp.rs:38-44`、`ble.rs:1418-1420`、`decode_frame`
  （`outbound.rs:24-38`）三处**已经建立**，只有 TCP writer 没跟上 —— 于是 P2 从"新规则"变成
  "补齐一处一致性"，改造风险与论证成本都下降。
* **2026-09-24 用户纠正 2**：第 0 步"风险≈0"的说法过松。已拆成 0-A（删除类，低风险）与
  0-B（索引类，低风险但非零：迁移 + 写放大 + 启动耗时），并各给独立验收；两者不得合并提交。
