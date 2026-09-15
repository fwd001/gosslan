# 01 · 分层边界与并发所有权

- Status: Normative
- 上游：`README.md`（P1–P7、C1–C6）

---

## 0. 目的

定义 V5 每一层的**唯一职责**与**状态所有权**。任何代码不得跨层越权；
这条是「蓝牙不稳」最根本的解药 —— 现状把射频生命周期、链路策略、协议状态、可靠消息全塞进
`network/ble.rs`（2173 行）与 `network/transport.rs`（8399 行），没有单一 owner。

---

## 1. 分层（L0–L5）

| 层 | 职责 | 拥有状态 | 禁止 |
|---|---|---|---|
| **L5 App/UI** | 渲染、用户命令、窗口 | 只读快照 | 直接锁引擎状态、直接碰射频 |
| **L4 MeshEngine** | 线编解码、去重、TTL、Peer/Connection 图、relay 授权、路由/网关、可靠层（outbox/ACK/read/seq）、特性分发 | **全部协议状态（单串行）** | 直接碰 DB、射频、UI |
| **L3 LinkRegistry** | `LinkId ↔ peer/endpoint/path_kind/health/backpressure` | 连接注册表 | 理解业务消息内容 |
| **L2 LinkAdapter（平台）** | BLE central/peripheral、TCP、UDP 发现 | 射频对象、写/通知缓冲、分片重组器 | 知道 peer/协议/DB |
| **L1 Platform/OS** | 系统蓝牙/网络栈 | — | — |
| **L0 Persistence** | SQLite 读写 | DB 连接 | 协议决策 |

> Discovery 不是独立一层：它是 L2 的一部分，只产出「候选/事件」，**不建引擎态**（P1）。

---

## 2. 并发所有权（三选一，不允许模糊）

| 域 | 谁拥有 | 通信方式 |
|---|---|---|
| **Engine-confined** | 单个专用任务串行跑引擎 | 外部只能经 mailbox 发 `MeshEvent` |
| **Link-confined** | 每个平台适配器自己的任务 | 向上 `emit(LinkEvent)`；向下收 `LinkCommand` |
| **Lock-backed snapshot** | UI 需要的只读视图 | 引擎变更时**整段发布**新快照，读者不加业务锁 |

**同步边顺序（构造上无死锁）**：

```text
main / test threads ──sync──▶ engine ──sync──▶ link
                                  └──async──▶ persistence worker
```

- 允许：main→engine、engine→link 的同步调用（有界、无重入）。
- **禁止**反向 sync-wait：link 只能 `async` 到 engine；任何东西都不 sync-dispatch 到 main。
- 引擎内部不得再拿第二把业务锁（单写者）；确需的只读缓存用 lock-backed 快照。

---

## 3. 事件 / 命令 / 效果

| 类型 | 方向 | 例子 |
|---|---|---|
| `MeshEvent` | 进引擎 | `LinkUp/LinkDown/BytesIn(link,bytes)/Writable(link)/Timer(kind)/Command(..)` |
| `MeshCommand` | 引擎→链路 | `Send(link,bytes)/Scan(policy)/Advertise(policy)/Disconnect(link)` |
| `MeshEffect` | 引擎→外部 | `SendFrame/ScheduleTimer/PersistMessage/WriteAck/EmitPeerEvent` |

引擎是**纯状态机**：`handle(event, now) -> Vec<Effect>`（sans-I/O）。
好处：① 可确定性仿真（`06`）；② 可属性测试（relay 风暴、分区恢复、去重健全性）；③ 无隐藏时钟。

---

## 4. 从现状迁移（不一次到位）

现状事实（2026-09-14 核对）：
- `state.rs:607-839` 约 40 个独立 Mutex 字段；
- 两套模型并存（`state::Peer/Link` 与 `mesh::Peer/Connection`，`mesh/manager.rs:10` 自述「仍是旁路」）；
- `network/transport.rs` 里 **105 处** `state.db.lock()`（同步 rusqlite）在热路径上。

迁移顺序（详见 `07-roadmap-v5.md`）：
1. **Phase 3a**：把 6 个 mesh 自有 Mutex（`mesh_router`/`gossip`/`peer_manager`/`peers`/`links`/`relay_policy`）合并为一个 `Mutex<MeshState>`，写死锁序；**行为零变化、可单独回退**。
2. **Phase 3b**：`Mutex<MeshState>` → 单任务 mailbox；DB 改 `Effect` + 独立 worker（`spawn_blocking`）。
3. **Phase 3c**：删除 `route_order` 的「按端点对齐 + 合成候选」过渡逻辑。

**迁移红线**：C1–C6（`README.md` §6）。LAN 路径必须始终可退回当前实现。

---

## 5. 不变量

```text
INV-NET-01  引擎外部不得直接锁引擎状态（只能发 MeshEvent）。
INV-NET-02  链路层不得 import protocol / db / UI 类型。
INV-NET-03  去重与 TTL 只有一份，位于引擎。
INV-NET-04  引擎不直接调用 DB / 射频，只产出 Effect。
INV-NET-05  同步边只允许 main→engine→link 方向。
```
