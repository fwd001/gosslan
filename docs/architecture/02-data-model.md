# 02 · 数据模型（唯一一套）

- Status: Normative
- 上游：`README.md` P2/P4；`01-layers-and-concurrency.md`
- 决策 ADR：`../adr/0021-single-peer-connection-model.md`

---

## 0. 为什么必须收敛

现状**同时存在两套模型**，靠端点对齐手工缝在一起 —— 这是「状态漂移」的结构性来源：

| 模型 | 位置 | 用途 |
|---|---|---|
| `state::Peer` + `state::Link` | `state.rs:154/187` | **运行时**真正用的表 |
| `mesh::Peer` + `mesh::Connection` | `mesh/peer.rs`/`connection.rs` | mesh 层，自述「Phase 2 仍是旁路、不接管 state」（`mesh/manager.rs:10`） |

`network/transport.rs:113-116` 明确写着「mesh 层（Connection）与传输层（Link）是**两套链路表**，且不保证 1:1 同序」，
于是 `route_order`（`transport.rs:125-162`）用端点对齐，还要为「登记窗口」合成假健康候选。

**V5 决定：收敛为唯一模型**，不允许两条并存。

---

## 1. 规范模型

```text
NodeId        = device_id（String，例 gosslan-718562a258f7cf55）

Peer {
  node_id      : NodeId
  identity     : { x25519_pubkey?, ed25519_pubkey? }   // 只补空、不覆盖（INV-P11）
  profile      : { nickname, avatar?, device_type? }
  capabilities : Set<Capability>                        // 跨通道并集，见 04
  connections  : Map<ConnId, Connection>
  state        : { online, last_seen, rtt_ms? }
}

Connection {
  conn_id      : ConnId                                // 稳定于 (peer, endpoint) 世代
  peer_id      : NodeId
  endpoint     : Endpoint
  path_kind    : PathKind                              // Lan | Routed | Bluetooth
  health       : ConnectionHealth
  capability   : Capability                            // 这条连接能承载什么
  link         : Option<LinkHandle>                    // 传输句柄（无链路=仅发现）
  backpressure : { bulk, priority }                    // mpsc Sender，见 05
}

Endpoint = Tcp(SocketAddr) | Routed(SocketAddr) | Ble(BleEndpoint)
LinkHandle = { bulk: Sender<Message>, priority: Sender<Message>, cancel: watch::Sender<bool> }
ConnectionHealth = { last_read_seen, last_write_seen, consecutive_failures }
```

---

## 2. 收敛决策（怎么做）

1. **采用 `mesh::Peer`/`mesh::Connection` 作为唯一模型**（它是为多连接设计的）。
2. `state::Link` 的传输字段（`bulk`/`priority`/`cancel`/`path_kind`）**并入 `Connection`** 作为 `LinkHandle`；`state::Link` 类型随之删除。
3. `state::peers`（供 UI/命令层读取的展示字段）改为**引擎产出的快照**，不再是权威状态：`PeerView { device_id, nickname, avatar?, device_type?, link, path_kind, online, rtt_ms? }`。
4. `route_order` 的端点对齐 + 合成候选是**过渡契约**，Phase 3c 删除（`01` §4）。

### 字段映射（迁移时对照）

| 旧（`state.rs`） | 新 |
|---|---|
| `Peer.device_id/nickname/avatar/device_type` | `Peer.node_id` + `Peer.profile` |
| `Peer.x25519_pubkey/ed25519_pubkey` | `Peer.identity` |
| `Peer.online/rtt_ms/last_seen` | `Peer.state` |
| `Peer.link`（Option<String>） | 快照 `PeerView.path_kind` |
| `Link.endpoint/path_kind` | `Connection.endpoint/path_kind` |
| `Link.bulk/priority/cancel` | `Connection.link`（`LinkHandle`） |
| `mesh::Connection.health` | 保留（已经是权威） |

---

## 3. 健康模型（多路径 failover 的基础）

- **读活性是唯一「对端活着」的证据**：`last_read_seen` 只在读到对端帧时刷新（`mark_read_seen`）。
- **写成功不刷新读活性**（半开 TCP 上也会写成功）。
- 建链时播种一次读活性（`seed_read_seen`），此后只由读循环刷新。
- 不健康判定：`now - last_read_seen > health_timeout` **或** `consecutive_failures > max_failures`。
- **同 endpoint 的 upsert 幂等，绝不能覆盖 `health`**（否则该连接永远「从未成功」→ 永远离线）。

---

## 4. 身份

- **身份只由双向 `Hello` 验签建立**；IP 与蓝牙地址都**不是**身份（ADR-0015 P-A01）。
- 公钥冲突不静默覆盖（INV-P11）。
- 重新配对需知情同意（见 `../protocol-invariants.md`）。

---

## 5. 不变量

```text
INV-NET-10  同 device_id 只有一个 Peer。
INV-NET-11  同 endpoint 的 upsert 幂等，且绝不覆盖 health。
INV-NET-12  Peer 在线 = 任一 Connection 健康（不看 transport 数量）。
INV-NET-13  Connection 的 path_kind 由「来路」决定，不能从 IP 段反推。
```
