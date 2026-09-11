# ADR-0014: Multi-Path Connection Selection

- Status: Accepted（2026-09-12 用户审核通过）
- Date: 2026-09-12
- Owners: Gosslan
- Related:
  - 设计文档：§19 Routing 与 Transport Selection / §24 Connection Failover / §34 Online 状态 / §35 Connection Health / §20 控制风暴
  - PR: 待定
  - CHANGELOG: `[Unreleased]`
  - Protocol: **无新协议字段**（选路是纯内部决策）

---

## 1. Context

Phase 6 把传输层从「Peer = Device = 一条 TCP」改成了 `links: HashMap<device_id, Vec<Link>>`，
`Link { endpoint, bulk, priority }` —— **多连接的结构已经具备**，一个 peer 可以同时持有
LAN 与 Routed 两条连接，断一条不影响另一条。但「**怎么挑**」这一层还停留在最朴素的形式。

现状（本 ADR 动笔时逐条核实的代码事实）：

| 事实 | 位置 | 后果 |
|---|---|---|
| `try_send` 顺序遍历该 peer 的连接列表，**首个成功即返回** | `network/transport.rs::try_send` | 顺序 = 建链顺序，不是「最快」 |
| `broadcast_gossip` 取 `links.get(peer).first()` | 同上 | 同上 |
| `try_send` 返回 `Ok` 只代表 **mpsc 入队成功**，不代表 TCP 写出成功 | 同上（代码注释已明示） | 「发成功」无法反映链路真实健康 |
| `ConnectionHealth { rtt_ms, last_seen_ms, consecutive_failures }` | `mesh/connection.rs` | 模型齐全，但**生产路径从未喂过数据** |
| `mark_seen` / `mark_failure` 仅由 `mesh/peer.rs` 内部方法与测试调用 | 全仓 grep 确认 | `Connection.health` 恒为 `default()` |
| `Peer.rtt_ms` 唯一生产调用点传 `None` | `network/discovery.rs` announce 分支 | 诊断面板 `avg_rtt_ms` 恒空 |
| `Message::Heartbeat` 是**单向**的，无 `Pong`；收到只 `touch_peer` + flush | `network/transport.rs` | **没有现成的 RTT 来源** |

也就是说：**现在做「挑最快」，挑的依据是空的** —— `online_state()` 若真被调用会恒返回
Offline。这与 Phase 2 review 抓到的 `upsert_connection` health 覆盖 bug 是**同一类陷阱**
（模型在、数据空），必须先补数据再谈策略。

---

## 2. Decision

我们决定：

```text
1. 先接「连接级活性」信号，后做选路。信号全部复用现有帧，零新协议。
2. 选路策略 = 路径优先级（LAN > Routed > Bluetooth）+ 活性过滤 + 稳定序打破平局。
   **不做 RTT 排序**。
3. 源发广播保持「每个 peer 选一条、但不裁剪 peer 集合」（§8.1 #5 红线不变）。
4. ensure_link 的「连通性」语义保持不变（见 P1-2 修正 `f69b917` 与
   `.workbuddy/mesh-task/mesh-architecture-evolution.md` §7.10）：
   多路径由**各 Transport 自己的驱动**负责产生（Routed 由配置驱动、BLE 由发现驱动），
   选路层只负责「已有多条时挑哪条」。
```

### 为什么不做 RTT

RTT 的唯一可行来源是给 `Heartbeat` 加回包（新 wire 变体），需要三端同步升级。
本项目已明确**不背 2.0 及更早版本的兼容包袱**，所以「兼容性」不是拒绝它的理由 ——
拒绝它的理由是**收益不足**：LAN 与 Tailscale 的延迟差几个数量级，路径优先级已经能
正确区分；同类型路径之间的延迟差异（如两块网卡）在当前产品形态下不是用户可感知的问题。
按 §8.1 #11（不为「未来可能支持」提前加抽象），现在不引入。

> 若将来真机数据显示「同路径多连接的选优」确有价值，**另立 ADR** 讨论 `Heartbeat` 回包方案。
> 届时的门槛是「收益 vs 三端同步升级成本」，不是「是否兼容旧版本」。

---

## 3. Detailed Design

### 3.1 健康信号（M3-0，先做，纯旁路）

数据来源**全部复用现有帧**：

| 事件 | 落点 | 语义 |
|---|---|---|
| 出站 `write_frame` 成功 | `writer_loop` → 写**出站**活性 | 该连接**能写出** |
| 入站读到任意一帧 | `reader_loop` → 写**入站**活性 | 该连接**能读到**（更强：对端活着） |
| 建链完成 | `register_connection` → 建链时**一次性播种**入站活性 | 「刚建好」必须算健康（否则会触发重拨，见下方注意 1） |
| 写失败 / 读循环退出 | `mark_connection_failure` / 现有 `unregister_connection` | 该连接不可用 |

✅ **M3-0b 已实施**（`fix: M3-0b 健康信号拆读写活性`）。以下为该补丁的设计依据：M3-0 落地时
`ConnectionHealth` **只有一个** `last_seen_ms`，写成功（`writer_loop`）与读成功（`reader_loop`）
写的是**同一个字段**，而 5s 心跳会给每条链路写成功 ⇒ **半开 TCP（对端已死、内核仍收写）
会永久「健康」**，上表「入站更强」在实现上不成立，本 ADR §7 要解决的正是这种链路。
M3-0b 把健康拆成 `last_write_seen_ms` / `last_read_seen_ms`（或等价地加 `last_inbound_ms`），
并让 `is_healthy` 要求**近期有入站观测**（建链时播种一次，见注意 1；此后只由 `reader_loop` 刷新）。
**M3-0b 已合入，M3-b 可以开工。**

阈值提醒（供 M3-b 调参，不构成缺陷）：`PeerManager::new(10_000, 3)` —— 读活性阈值 10s，
而双向心跳是 5s 一次 ⇒ 容错仅「一个心跳周期」（丢一拍还健康，丢两拍就判不健康）。
`online_state()` 目前**只有日志在读**（`[mesh] +conn` 的 `online=` 字段），所以这个偏紧的
阈值今天不会造成任何行为变化；但 M3-b 一旦用它做选路/在线判定，建议放宽到 ≥3 个心跳
周期（15s），否则网络抖动会被误判成链路故障并触发无谓换路。

**两条硬性注意（都是踩过的坑）**：

1. **建链时必须先播种一次入站活性**（`register_connection`）。否则「已建立但尚未收发」的连接
   会被判为不健康，`should_dial` 会反复重拨 —— 与 P1-2 修正叠加会重新制造重复连接。
   这是**唯一**允许在 `reader_loop` 之外写入站活性的地方；写成「写成功也刷新入站活性」
   就等于退回 M3-0b 之前的缺陷。
2. **同 endpoint `upsert_connection` 绝不能覆盖 health**（Phase 2 review 抓到的真 bug：
   `Connection::new` 的 health 恒为 `default()`，周期性 announce 会把已建链连接打回
   「从未成功」→ 在线恒 Offline）。已有两个护栏测试，M3-0 不得回退它们。

RTT 字段（`ConnectionHealth.rtt_ms`）本阶段**保持为 `None`**，并在文档中如实标注，
不假装有数据。

### 3.2 选路（M3-a 纯函数 → M3-b 接线）

```text
pick_link(candidates: &[LinkView], now_ms) -> Option<usize>

  1) 过滤：只保留「健康」的连接（**近期有入站观测** && 连续失败 <= 阈值；见 §3.1 M3-0b）
  2) 排序：路径优先级 LAN > Routed > Bluetooth
  3) 打破平局：建链顺序（稳定、可复现，不引入随机性）
  4) 全不健康时：**退回首个连接**（保持可用，不报错）—— 与今天的兜底行为一致
```

- **不做** fanout 截断：这是「同一 peer 内选一条」，不是「裁剪 peer 集合」。
- `try_send`：按策略取首条 → 失败（channel 关闭）→ 依次尝试次优 → 全失败才 `Err`。
- `broadcast_gossip`：对每个 peer 用同一策略选一条（当前是 `.first()`）。
- 可观测：新增 `[mesh] route peer=X picked=<path> cands=N` 一行日志（链路状态是内存态，
  日志是唯一可观测手段，§45）。

### 3.3 分步落地（每步 1 commit，可独立回退）

| 步 | 内容 | 行为变化 |
|---|---|---|
| M3-0 | 健康信号接入（§3.1） | 无（只写不读）—— ✅ `03ae61a` |
| M3-a | `pick_link` 纯函数 + 单测 | 无（**落地时连日志都没接**；§3.3 原写「只用于日志上报」不准确）—— ✅ `af9fca5` |
| **M3-0b** | **健康拆读写活性**（`last_write_seen_ms` / `last_read_seen_ms`，`is_healthy` 要求近期入站）+ 建链播种 | 无（纯旁路：`online_state()` 仍只有日志在读；但修复了「半开链路永久健康」）—— ✅ **已完成** |
| M3-b | `try_send` 接线 + 半开链路回退广播（P1-1 遗留项）；**同时**把「持 `links` 锁跨 `await` 阻塞发送」改为锁内快照、锁外发送 | 有（顺序可能改变）—— ✅ **已完成**（`2f86b5b`；另 `f334cb6` 先修了锁与信道满挂起） |
| M3-c | ~~失败判据（连续失败摘链路）~~ → **改由读活性超时拆除实现**（`f0bb87c`，M3#6） | 有（边界收敛）—— ⚠️ 见下方说明 |
| M3-d | `broadcast_gossip` 接线 | 有 |
| M3-e | 可观测性（route 日志 + conv_link 语义校正） | 无 |

> ⚠️ **M3-c 的失败判据改为「读活性超时拆除」**（2026-09-12 复核结论 + 实现）：
> 原计划的「连续失败 N 次 → 摘链路」在本设计里**不可达** —— `writer_loop` 首次写失败即
> `break`（连接报废，不会累积计数），而每次成功读写又把计数清零 ⇒
> `consecutive_failures` 只可能是 0 或 1，`is_healthy` 的失败分支永不触发。
> 真正需要解决的是**半开链路**（对端消失、本机内核仍收写）：它既不会写失败、也不会读到帧。
> 因此判据改为**读活性超时**：watchdog 每 5s 检查，读活性跨过 3× 健康超时（45s）即
> 精确取消该连接的读写任务并移出链路表，交由发现层重拨（`f0bb87c`）。
> `max_failures` 参数暂时保留（未来若要引入「主动探测失败」等显式失败信号再用），
> 但**不再作为拆除依据** —— 不要在它上面继续加逻辑。

> ⚠️ 复核补充（2026-09-12）：`try_send` 现在的「failover」只对**信道关闭**生效 ——
> `tx.send().await` 在信道满时是**挂起**而非 `Err`，且此时 `state.links` 全局锁被持有。
> M3-b 必须一并收敛（锁内快照 `Vec<Link>` 克隆 → 锁外发送；必要时改用非阻塞 `try_send`
> 并对 `Full` 走下一跳）。`broadcast_gossip`(`:113`+`:128`) 与心跳循环同病。

---

## 4. Invariants

- **INV-P20**：priority 队列优先于 bulk（`is_bulk_message` 分流 + `biased` select）**不得被破坏**。
- **设计 §34**：任一 Connection 健康 ⇒ Online。
- **P-A02**：Peer 与 Connection 分离（一个 Peer 多条 Connection）。
- **P-A05**：需要 relay 的数据必经 MeshRouter。
- **§8.1 #5**：**源发广播禁止 fanout 截断**（会漏发群消息）。

---

## 5. Alternatives

| 方案 | 判断 |
|---|---|
| A. 给 `Heartbeat` 加回包，按 RTT 排序 | 收益不足，且需新 wire 变体 + 三端同步升级。记为**后续可选**，需新 ADR |
| B. 维持「首个成功即返回」不改 | 无法 failover；且首个连接若是僵尸（mpsc 入队成功但 TCP 已死），消息会被**持续投进死路**。这是本 ADR 要解决的问题本身 |
| C. 主动带宽探测 / 拥塞感知选路 | YAGNI（§8.1 #11） |
| D. 每个 peer 只保留一条连接（回到单链） | 会丢掉 Phase 6 的全部成果（LAN + Routed 共存、断一条仍在线），明确否决 |

---

## 6. Compatibility

- **无协议变化**：不新增/修改任何 wire 字段。
- 用户已明确本版本可不兼容 2.0 及更早版本，但**本 ADR 不需要用到该许可** ——
  选路是纯内部决策。
- 旧版本与本版本在同一网段混跑时行为不受影响。

---

## 7. Failure Modes

| 失效场景 | 表现 | 缓解 |
|---|---|---|
| 半开 TCP：写「成功」但对方已消失 | 健康信号仍刷新 → 选路继续选它 | **M3-0b 之后**：写出不再刷新入站活性 ⇒ 该链路在一个阈值内自然被判不健康；在此之前（M3-0 现状）**无缓解** —— 这是必须补 M3-0b 的原因 |
| 全部连接都不健康 | 选路无候选 | 退回「首个连接」而非报错（保持可用） |
| 健康阈值过紧 | 正常的 Tailscale 高延迟连接被判不健康 → 频繁切换 | 阈值参照既有 `RELAY_PEER_TIMEOUT_SECS = 45s` / 心跳 5s 的量级标定，并留 2–3 个心跳周期余量 |
| 健康阈值过松 | 僵尸连接长期被选中 | 由 §3.1 的**入站**活性提供更强证据（能读到帧才是真活）；前提是 M3-0b 已拆读写 |
| 流量拥塞导致信道满 | `try_send` 的 `send().await` **挂起**（非 Err）→ 既不降级下一条，又持锁阻塞全表 | M3-b 一并收敛：锁内快照、锁外发送；必要时改用非阻塞 `try_send` 并对 `Full` 走下一跳 |
| 与 P1-2 修正叠加 | 选路切换触发重拨 → 重复连接 | `ensure_link` 已有 `has_any_link` 短路；建链时的播种避免误判不健康 |

---

## 8. Testing

- **单测**：`pick_link` 纯函数（健康过滤 / 路径优先级 / 平局稳定序 / 全不健康回退）。
  **M3-0b 追加**：「只写不读的连接不算健康」「建链播种后算健康」「读一帧后刷新」。
- **决定性判据（扩展 `examples/dual_link.rs`）**：它现在只验「建立了 2 条 + 断开一条后
  另一条仍能收到帧」，判据范围是**帧级**（示例没有监听端 ⇒ 实例只能经我们发起的连接送帧，
  能在第二条读到帧即该链路仍是活跃链路）。**它证明不了"选路切换"**。
- **消息级判据（已实现在 `examples/dual_link.rs`，⚠️ 待真机/本机 E2E 终端实跑）**：
  关键在于「让实例**主动发一条定向消息**，才算真的走了 `try_send`」，而实例只给好友发消息。
  最终方案**不需要好友、也不需要改库** —— 利用**非好友单聊**这一合法触发点：
  实例在 `is_friend` 判定失败时会用 `try_send` 回一条 `Message::FriendMessageBlocked`
  （`network/transport.rs` 非好友分支，且发生在**解密之前**，内容可以是垃圾）。
  判据三步（含对照）：
  1. 两条链路建立后发一条单聊 → **只有被选中的那条**收到 `FriendMessageBlocked`；
     **对照**：另一条在短窗口内**不得**收到（否则是播发而非定向，判据不成立）；
  2. **切断被选中那条**；
  3. 在幸存链路上再发一条单聊 → 必须**仍收到** `FriendMessageBlocked` ⇒ 真 failover。
  实跑方式（需可写应用数据目录）：`GOSSLAN_AUTOSTART=1 ./target/debug/gosslan --instance 1 &`
  然后 `cargo run --example dual_link -- 60002`。
  ⚠️ **诚实标注**：该判据的 PASS 尚未取得 —— 开发沙箱里实例**起不来**
  （实测 `unable to open database file`：App 数据目录不可写），只验证到「编译通过 + 逻辑完整」。
  另外本示例两条链路都是 LAN（loopback 与私网地址同属 `PathKind::Lan`），
  所以它验的是 **failover**；**优先级 LAN > Routed** 由 `route_order_*` 单测覆盖。

---

## 9. Consequences

**Positive**
- 健康信号**已落库到运行时**（M3-0：`ConnectionHealth` 终于有真实数据）。
  ⚠️ 但**「在线」判定尚未升级为连接级事实** —— `online_state()` 目前在生产路径
  只用于 `[mesh] +conn` 的 `online=` 日志字段，UI 的在线仍是旧启发式
  （`peers` 表 + `RELAY_PEER_TIMEOUT_SECS`，见 `commands.rs` 的 friend online 判定）。
  该项要等 M3-c/M3-e 才真正兑现。
- 为后续 BLE（Phase 7）接入铺好路：BLE 与 LAN 并存时，选路层不需要为每个 Transport 写特例。
- 半开链路造成的「消息投进死路」有了**收敛路径的设计**（读出问题 → 降级），
  但**该路径要等 M3-0b 拆开读写活性之后才成立**。

**Negative / 代价（明确承认）**
- 引入选路后，消息可能**换路**：这会让「同一对节点两次投递走不同路径」成为常态，
  链路徽标（`conv_link`）需要语义校正，排障时要结合 `[mesh] route` 日志。
- 不做 RTT 意味着**无法在同类型路径之间选优**（例如两块网卡）。承认这个上限。

**Risks**
- `try_send` 是网络核心热路径，选路写错会整体影响收发 → 因此 M3-a（纯函数）与
  M3-b（接线）必须拆成两个 commit，且 M3-a 阶段**行为零变化**，便于二分定位。

**落地前提**
- M3-0 之前必须先合入 P1-2 的镜像重复连接修正（已合入 `f69b917`），
  否则选路判据会被镜像重复连接污染成假阳性。
