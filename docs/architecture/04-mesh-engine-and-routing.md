# 04 · Mesh 引擎与路由（含网关与 BitChat 中继）

- Status: Normative
- 上游：`01`、`02`
- 决策 ADR：`../adr/0020-layered-transport-and-single-mesh-engine.md`、`../adr/0021-single-peer-connection-model.md`、`../adr/0022-ble-seamless-lan-join.md`、`../adr/0023-bitchat-dual-stack-relay.md`

---

## 0. 引擎职责（且仅这些）

线编解码 · 全局去重 · TTL · Peer/Connection 图 · relay 授权 · 目的判定 · 链路选择 · 转发 ·
可靠层（outbox/ACK/read/seq）· 特性分发。

**不负责**：UI、SQLite、E2EE 明文、好友关系、BLE GATT 实现、TCP socket 实现、射频策略（后者在 `05`）。

---

## 1. 处理流水线（唯一入口）

```text
LinkEvent::BytesIn
   ↓  线解码（Gosslan Message / OpaqueExternal / BitChat 不透明字节）
   ↓  验签 / 身份绑定（只对 Gosslan 帧）
   ↓  全局去重（frame_id / msg_id）
   ↓  TTL 递减 + clamp
   ↓  目的判定（me / directed peer / broadcast）
   ↓  链路选择（direct → pick_link；无直连 → 源路由；否则洪泛）
   ↓  转发 or 本地消费
   ↓  Effects（SendFrame / PersistMessage / WriteAck / EmitPeerEvent）
```

去重与 TTL **只有一份**（INV-NET-03）；这保证 A(BLE)→B→C(LAN) 的循环不会发生。

---

## 2. 路由策略（V5 分三档，逐档落地）

### 2.1 直连（已有）
- `pick_link`：活性过滤 + 路径优先级 **LAN > Routed > Bluetooth** + 稳定序打破平局；全部不健康退回第一条。
- `try_send`：按 `route_order` 顺序**依次尝试**，非阻塞第一轮 + 有界补试（同 peer 多链路 failover）。

### 2.2 广播 / 洪泛（已有，需加固）
- **源发**用 `exclude_source`，**不按 fanout 截断**（否则漏发群消息）。
- **转发**用有界 fanout（控制风暴）。
- ⚠️ 待修：① 转发候选应取「有链路的 peer」而非全部 `peers`；② 转发 `ttl-1` 必须 **clamp** 到 `max_ttl`。

### 2.3 源路由 + 网关（Phase 5 新增）
- ANNOUNCE 携带**直接邻居**（≤10 个 8 字节 peerID，跨 transport 取并集）。
- 拓扑边**必须双向确认**才可用于路由；条目 60s 过期。
- 定向帧可带源路由 `[中间跳…]`（不含 sender/recipient）；下一跳失败 ⇒ **回退洪泛**。
- **网关是自动角色**：任何同时有 BLE 与 LAN 链路的节点，都会按引擎转发规则把一条链路的帧送到另一条链路；
  无需声明、无需配置。

---

## 3. 邻居列表与「无感可见」

1. 引擎维护 `direct_neighbors(link)`，并按 **transport 并集**汇总（`union(all transports)`）。
2. announce/Presence 携带该并集（cap 10）⇒ LAN-only 节点也能从邻居表里「看到」BLE-only 节点存在。
3. 因此 BLE-only 节点在 LAN 侧表现为**普通 peer**（身份 = device_id），用户无感。

> 这条是「蓝牙无感接入局域网」的核心：**可见性靠邻居 gossip，可达性靠网关转发**，不靠用户切通道。

---

## 4. BitChat 透明中继（OpaqueExternal）

- **双栈 GATT**：外设同时注册 Gosslan 与 BitChat 的 service/char；central 同时匹配两个 service UUID。
- 收到的 BitChat 字节 → 包成 `Message::OpaqueExternal { id, ttl, payload }` → 现有流水线（去重 + TTL + fanout）。
- **不解密、不落库、不建用户/channel、不进 gossip、不进 UI**。
- 广告：legacy 31B 装不下 2 个 128 位 UUID ⇒ extended advertising / 第二 advertising set / scan response（逐平台确认）。
- **默认关闭 + 设置显式开关**（合规/身份/功耗）。

---

## 5. DB 边界（Effect 化）

现状：`network/transport.rs` 里 **105 处** `state.db.lock()`（同步 rusqlite）在热路径上。
V5：引擎**不直接**碰 DB，只产出 `Effect::PersistMessage/WriteAck/TouchConversation/SaveOutbox`，
由独立 persistence worker（`spawn_blocking`）消费，结果再以 `MeshEvent` 回灌。

这条是 actor 化的前提 —— 否则同步 DB 会顶住 mailbox（见 `01` §4、`07` Phase 3b）。

---

## 6. 不变量

```text
INV-NET-30  去重/TTL 在引擎，且跨 transport 只有一份。
INV-NET-31  源发广播不截断；转发才有界 fanout。
INV-NET-32  转发 TTL 必须 clamp 到 max_ttl。
INV-NET-33  非成员也要转发群消息（只跳过本地消费）。
INV-NET-34  定向帧到达目标后停止转发。
INV-NET-35  OpaqueExternal 不进入业务处理、不落库。
```
