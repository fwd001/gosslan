# 08 · 实现映射（Phase → 文件 / 符号 / 测试）

- Status: Normative
- 上游：`07-roadmap-v5.md`、`01`–`06`
- 目的：把路线图落成**逐文件、逐符号、逐测试**的可执行清单；执行时照做即可。

> 行号会随开发漂移，**按符号名定位**；本文件中的行号仅作 2026-09-14 的定位提示。

---

## 1. 现状锚点（2026-09-14 核对）

| 领域 | 符号 | 位置 |
|---|---|---|
| BLE 运行时 | `scan_loop` | `network/ble.rs:375` |
| | `dial_and_register` / `finish_dial` | `ble.rs:715` / `:804` |
| | `read_hello_frame` | `ble.rs:1055` |
| | `ble_writer_loop` / `ble_reader_loop` | `ble.rs:1227` / `:1364` |
| | `start_peripheral` / `peripheral_accept_loop` | `ble.rs:1496` / `:1536` |
| | `teardown_link` | `ble.rs:326` |
| BLE 纯策略 | `should_dial_ble` / `peripheral_route_action` / `preamble_action` | `ble.rs:653` / `:685` / `:1117` |
| | `ble_dial_backoff_ms` / `scan_interval` | `ble.rs:592` / `:137` |
| 链路端口（已存在） | `FrameSink` / `FrameSource` | `ble.rs:1131` / `:1160` |
| 传输/路由 | `route_order` / `send_over_order` / `try_send` | `transport.rs:125` / `:210` / `:244` |
| | `broadcast_gossip` | `transport.rs:290` |
| | `register_connection` / `unregister_connection` | `transport.rs:1712` / `:1764` |
| | `link_snapshot` / `ensure_link` / `connect_to_peer` | `transport.rs:1875` / `:2063` / `:2144` |
| | `handle_message` / `handle_gossip` | `transport.rs:2418` / `:3857` |
| | `Message::OpaqueExternal` 分支 | `transport.rs:2572` |
| | `flush_outbox` | `transport.rs:6330` |
| mesh | `MeshRouter::on_receive` / `select_outgoing` / `exclude_source` | `mesh/router.rs:185` / `:231` / `:251` |
| | `pick_link` | `mesh/selection.rs:44` |
| | `decide_forward` | `mesh/relay_policy.rs:155` |
| state | `AppState` | `state.rs:607` |
| | `MAX_CONCURRENT_DIALS` / `MAX_INBOUND_CONNECTIONS` | `state.rs:568` / `:571` |
| 死代码 | `Transport` trait / `TransportManager::route` / `route_payload` | `transport/mod.rs:52` / `:113` / `:165` |
| | `RelayManager` 字段 | `state.rs:728` |

---

## 2. Phase 0 — 减负 + 度量

| 动作 | 文件 / 符号 | 测试 / 护栏 |
|---|---|---|
| 新增纯策略骨架 | `transport/ble/policy/{mod,scheduler,duty,power}.rs` | 每个策略一条纯函数单测 |
| 删除死抽象 | `transport/mod.rs`：`Transport` trait、`TransportManager`、`route`/`route_payload` | 源 grep：不再出现 `TransportManager::route` |
| 删除死代码（确认后） | `state.rs:728` `RelayManager` 及其引用 | `cargo check` 0 warning |
| SLO 埋点 | `state.rs` 的 `BleDiag`/`BleScanStats` 扩展；`network/ble.rs` 事件点 | 诊断面板可读出 `06` §1 指标 |

**验收**：行为零变化（现有测试全绿）。**回退**：单提交 revert。

---

## 3. Phase 1 — BLE 内部止血（纯 BLE）

| 动作 | 文件 / 符号 | 测试 / 护栏 |
|---|---|---|
| 连接调度器接线 | `ble.rs:scan_loop`（`:375`）→ 候选入队；`dial_and_register`（`:715`）取 `dial_permits` | 打分/退避/上限单测；真机 Test A–F |
| 坏设备 blocklist | 新增 `transport/ble/policy/blocklist.rs`；`scan_loop`/握手处调用 | TTL/解封单测；`INV-NET-42` |
| 扫描/广播自愈 | `ble.rs:start`/`scan_loop`；三端 peripheral 的启动路径 | watchdog 状态机单测 |
| 出站优先级写队列 | 改 `ble.rs:ble_writer_loop`（`:1227`）：每片之间取最高优先级；`BleWriter::send_frame` 暴露逐片 | 优先级/字节上限单测；真机 A/B |
| TTL clamp + fanout 过滤 | `transport.rs` 转发处（`:4098` 附近）`ttl` clamp；fanout 候选改「有链路的 peer」 | `INV-NET-31/32` 单测 |
| `start()` 失败仍尝试外设 | `ble.rs:170`（`:179` 的 `?` 早退） | 单测/日志 |

**回退**：每项独立特性开关（`INV-NET-43`）。**风险**：中。**LAN**：不涉及。

---

## 4. Phase 2 — 链路端口 + 仿真底座

| 动作 | 文件 / 符号 | 测试 / 护栏 |
|---|---|---|
| 定义端口 | 新增 `transport/ble/link/{mod,event,command}.rs`（`LinkId`/`LinkEvent`/`LinkCommand`） | 类型层单测 |
| 包装既有帧端口 | `ble.rs` 的 `FrameSink`/`FrameSource` 适配到 `LinkCommand::Send`/`LinkEvent::BytesIn` | 往返单测 |
| 平台适配器收口 | `transport/ble/link/{macos,android,windows}.rs` 包装 `bluetooth*.rs`/`ble_android.rs` | `cargo check --features bluetooth` 0 warning |
| LAN 适配器 | 新增 `transport/ble/link/lan.rs`（包装 `transport/tcp.rs`，**语义冻结**） | LAN 黄金回归 |
| 平台 cfg 收敛 | `ble.rs` 的 `#[cfg]` 移到 `link/mod.rs` 工厂 | 源护栏 |
| 模拟链路 | 新增 `transport/ble/link/sim.rs` + 手动时钟 | 「两节点握手 + 一条消息」仿真 |

**结构护栏**：`link` 模块不得 import `protocol`/`db`/UI（`INV-NET-02/20`）。**回退**：双路径可切。

---

## 5. Phase 3a — mesh 状态合并（行为零变化）

| 动作 | 文件 / 符号 |
|---|---|
| 新增 `MeshState` | `mesh/state.rs`：聚合 `mesh_router`/`gossip`/`peer_manager`/`peers`/`links`/`relay_policy` |
| 替换字段 | `state.rs:607` 的 6 个 `Mutex<…>` → 一个 `Mutex<MeshState>` |
| 更新调用点 | `transport.rs` / `ble.rs` / `discovery.rs` / `commands.rs` / `file.rs` 的锁点 |
| 锁序契约 | 写入 `mesh/state.rs` 头部注释；grep 护栏：同一函数不得拿两次 |

**验收**：LAN 全量回归 + 真机 LAN 黄金回归。**风险**：中。

---

## 6. Phase 3b — MeshEngine actor + DB effect

| 动作 | 文件 / 符号 |
|---|---|
| 新增引擎 | `mesh/engine.rs`：单任务 mailbox，`handle(MeshEvent) -> Vec<MeshEffect>` |
| 新增效果 | `mesh/effect.rs`：`SendFrame/PersistMessage/WriteAck/TouchConversation/...` |
| 新增 DB worker | `mesh/persist.rs`（`spawn_blocking`，消费 effect，回灌 `MeshEvent`） |
| 迁入逻辑 | `transport.rs:handle_message`（`:2418`）、`handle_gossip`（`:3857`）、`try_send`（`:244`）、`broadcast_gossip`（`:290`） |
| 去 DB 锁 | `transport.rs` 的 **105 处** `state.db.lock()` → effect |
| UI 快照 | `PeerView`（`02` §2），`commands.rs` 读快照 |

**验收**：影子双跑 diff = 0 后切读写；`INV-NET-01/04`。**风险**：**高**。

---

## 7. Phase 3c / 4 / 5 / 6 / 7 / 8

| Phase | 关键文件 / 符号 |
|---|---|
| 3c | 删除 `transport.rs:route_order`（`:125`）的合成候选；删除 `state::Link` 类型 |
| 4 | 新增 `tests/simulation/`（`SimulatedMesh` + 8 场景，见 `06` §2） |
| 5 | `protocol.rs` 加邻居 TLV；新增 `mesh/topology.rs`；`transport.rs:handle_gossip` 的 fanout（`:4074` 附近）改用有链路 peer |
| 6 | 三端 peripheral 注册 BitChat UUID；`transport/bluetooth.rs` central 双匹配；`Message::OpaqueExternal`（`:2572`）生产端 |
| 7 | 新增 `src-tauri/gen/apple`；`transport/ble/link/ios.rs`；Info.plist/后台模式 |
| 8 | `transport/ble/policy/power.rs` 接线到 `scan_interval`（`:137`）与 announce |

---

## 8. 符号迁移表（旧 → 新）

| 旧 | 新 | 备注 |
|---|---|---|
| `state::Peer` | `mesh::Peer` | 唯一模型（ADR-0021） |
| `state::Link` | `mesh::Connection`（含 `LinkHandle`） | 类型删除 |
| `state::peers` | 引擎 `PeerView` 快照 | 不再权威 |
| `mesh::PeerManager` | `MeshState.peers` | 并入 |
| `route_order` 端点对齐 | `pick_link` per connection | 过渡后删除 |
| `Transport` trait / `TransportManager::route` | `LinkEvent`/`LinkCommand` | 删除 |

---

## 9. 风险热点（改动前先读）

1. **`handle_message`（`:2418`）与 `handle_gossip`（`:3857`）是 LAN+BLE 共用** —— 改它们就是改 LAN；
   必须影子双跑 + LAN 黄金回归（C1–C6）。
2. **105 处 `state.db.lock()`** —— actor 化必须先做 effect 化，否则同步 DB 顶死 mailbox。
3. **`OpaqueExternal`（`:2572`）已就绪但无生产者** —— Phase 6 只补生产端，不动流水线。
4. **`relay_manager` / `TransportManager` 死代码** —— 删除前先 `cargo check` 确认无引用。
5. **两套模型并存** —— 任何新代码不得再写 `state::Link`（`INV-NET-10`）。
