# ADR-0014: Multi-Path Connection Selection

- Status: Proposed（待用户审核后转 Accepted）
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
4. ensure_link 的「连通性」语义保持不变（见 ADR-0015 / P1-2 修正）：
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
| 出站 `write_frame` 成功 | `writer_loop` → `mark_connection_seen(peer, endpoint, now, None)` | 该连接**能写出** |
| 入站读到任意一帧 | `reader_loop` → 同上 | 该连接**能读到**（更强：对端活着） |
| 建链完成 | `register_connection` → 先 `mark_seen` 一次 | 「刚建好」必须算健康 |
| 写失败 / 读循环退出 | `mark_connection_failure` / 现有 `unregister_connection` | 该连接不可用 |

**两条硬性注意（都是踩过的坑）**：

1. **建链时必须先 `mark_seen` 一次**。否则「已建立但尚未收发」的连接会被判为不健康，
   `should_dial` 会反复重拨 —— 与 P1-2 修正叠加会重新制造重复连接。
2. **同 endpoint `upsert_connection` 绝不能覆盖 health**（Phase 2 review 抓到的真 bug：
   `Connection::new` 的 health 恒为 `default()`，周期性 announce 会把已建链连接打回
   「从未成功」→ 在线恒 Offline）。已有两个护栏测试，M3-0 不得回退它们。

RTT 字段（`ConnectionHealth.rtt_ms`）本阶段**保持为 `None`**，并在文档中如实标注，
不假装有数据。

### 3.2 选路（M3-a 纯函数 → M3-b 接线）

```text
pick_link(candidates: &[LinkView], now_ms) -> Option<usize>

  1) 过滤：只保留「健康」的连接（last_seen 在阈值内 && 连续失败 <= 阈值）
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
| M3-0 | 健康信号接入（§3.1） | 无（只写不读） |
| M3-a | `pick_link` 纯函数 + 单测 | 无（只用于日志上报选中哪条） |
| M3-b | `try_send` 接线 + 半开链路回退广播（P1-1 遗留项） | 有（顺序可能改变） |
| M3-c | 失败判据（连续失败摘链路） | 有（边界收敛） |
| M3-d | `broadcast_gossip` 接线 | 有 |
| M3-e | 可观测性（route 日志 + conv_link 语义校正） | 无 |

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
| 半开 TCP：写「成功」但对方已消失 | 健康信号仍刷新 → 选路继续选它 | 心跳写出最终会失败 → 摘链路；后续可按连续失败阈值提前收敛 |
| 全部连接都不健康 | 选路无候选 | 退回「首个连接」而非报错（保持可用） |
| 健康阈值过紧 | 正常的 Tailscale 高延迟连接被判不健康 → 频繁切换 | 阈值参照既有 `RELAY_PEER_TIMEOUT_SECS = 45s` / 心跳 5s 的量级标定，并留 2–3 个心跳周期余量 |
| 健康阈值过松 | 僵尸连接长期被选中 | 由 §3.1 的入站 `mark_seen` 提供更强证据（能读到帧才是真活） |
| 与 P1-2 修正叠加 | 选路切换触发重拨 → 重复连接 | `ensure_link` 已有 `has_any_link` 短路；M3-0 的 `mark_seen` 在建链时打点，避免误判不健康 |

---

## 8. Testing

- **单测**：`pick_link` 纯函数（健康过滤 / 路径优先级 / 平局稳定序 / 全不健康回退）。
- **决定性判据（扩展 `examples/dual_link.rs`）**：当前它只验「建立了 2 条」，
  **不验「切换」**。M3-b 必须扩展为：同一 peer 两条连接 → **主动切断被选中的那条**
  → 消息仍能送达（真 failover）。
- **护栏非空转**：临时把策略改成恒 `first()` → failover 判据必须 FAIL。
- **回归**：`cargo test --lib` 全绿 / 0 warning；`npm test`；`bash scripts/e2e-dev.sh` 失败数恒 0。
- **真机**：一台设备同时有 LAN + Routed 两条连接 → 日志显示按策略选中 → 拔网线后
  Routed 接管、聊天不中断。

---

## 9. Consequences

**Positive**
- 「在线」从启发式（`last_seen` + 45s 超时）升级为**连接级事实**（`ConnectionHealth` 终于有数据）。
- 为后续 BLE（Phase 7）接入铺好路：BLE 与 LAN 并存时，选路层不需要为每个 Transport 写特例。
- 半开链路造成的「消息投进死路」有明确收敛路径。

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
