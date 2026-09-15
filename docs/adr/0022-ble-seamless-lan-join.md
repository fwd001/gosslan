# ADR-0022: BLE 无感加入去中心化局域网（网关 + LAN 零回归契约）

- Status: Proposed（V5.0 方向）
- Date: 2026-09-14
- Owners: Gosslan
- Related:
  - 规范：`../architecture/04-mesh-engine-and-routing.md`、`../architecture/README.md` §6/§7
  - 依赖：ADR-0020/0021/0024
  - CHANGELOG: `[Unreleased]`
  - Protocol: 邻居 gossip TLV 属线格式扩展，走 ADR-0007/INV-P13

---

## 1. Context

目标：**BLE-only 设备无感加入去中心化局域网**，同时**保证 LAN 现有全部聊天功能零回归**。
现状：BLE 与 LAN 都在消息层直接调 `handle_message`，跨 transport 的转发靠 `broadcast_gossip`/`handle_gossip`，
存在已知缺口：转发候选取全部 `peers`（含无链路）、转发 `ttl-1` 未 clamp（`transport.rs:4074-4098`）、
无源路由、无跨跳补发。用户不感知通道这一目标尚未正式定义。

---

## 2. Decision

1. **BLE 只是第三种 Link**，与 LAN 共用同一引擎、同一份去重/TTL、同一套 `Message`（P4）。
2. **跨 transport 转发由引擎统一做**：任何同时有两条不同 transport 链路的节点**自动成为网关**，
   不需要声明或配置。
3. **邻居列表跨 transport 取并集**并随 announce/Presence 传播（cap 10）⇒ LAN-only 用户能「看到」BLE-only 设备。
4. **直连优先**：`pick_link` 保持 LAN > Routed > BLE；无直连才走源路由/洪泛穿网关。
5. **UI 无感**：不要求用户选通道；最多显示「直连 / 中继」。
6. **LAN 零回归契约 C1–C6 作为本 ADR 的强制约束**（`README.md` §6）。

---

## 3. Detailed Design

见 `../architecture/04-mesh-engine-and-routing.md`（路由三档、邻居并集、网关语义）与 `05-radio-policy.md`。

---

## 4. Invariants

```text
C1  线格式不变（Message/GossipEnvelope/Hello 签名/E2EE/协议版本）。
C2  语义不变（outbox/ACK/已读/seq/文件/好友/群）。
C3  发现不变（UDP 广播/组播/who_has 与端口绑定）。
C4  关蓝牙 = 与今天逐字节一致。
C5  新引擎先影子双跑，diff=0 才切读写。
C6  新路径稳定前不删 state.links / state.peers / network::discovery。
INV-NET-30  去重/TTL 在引擎且跨 transport 唯一。
INV-NET-31  源发广播不截断；转发才有界 fanout。
INV-NET-33  非成员也转发群消息（只跳过本地消费）。
```

---

## 5. Alternatives

**A. 独立「蓝牙桥接模式」**（BLE 设备连一个专属网关）：需要用户配置、单点，且违背「无感」；拒绝。
**B. 给 BLE 单独一套协议/路由**：与「一份去重/TTL」冲突，易产生回环；拒绝。
**C. 网关显式声明/选举**：复杂且脆弱；改为**自动角色**（有双链路即网关）。

---

## 6. Compatibility

- 不改 LAN 线格式与语义（C1/C2）；
- 邻居 gossip TLV 是**新增可选**字段，未知端忽略；按 ADR-0007 版本化 + INV-P13 流程；
- 「不考虑旧版兼容」的用户裁定允许直接新增变体，但仍需长度/边界校验，畸形帧只丢该帧不断链。

---

## 7. Failure Modes

- **回环**：靠全局 dedup + TTL 防；INV-NET-30。
- **网关抖动**：网关断开 ⇒ 邻居表过期（60s）+ 洪泛兜底；不引入永久黑洞。
- **BLE-only 设备不可见**：若网关未转发 announce ⇒ 用户认为「蓝牙搜不到」；由仿真场景 7 覆盖。
- **LAN 回归**：C4/C5 兜底；关蓝牙对照测试。

---

## 8. Testing

- 仿真：场景 7「BLE↔LAN 跨 transport 中继」（`06` §2）；
- 真机：三机 mesh（Android BLE-only + macOS 双链路 + Windows LAN）A→C 可达；
- LAN 黄金回归：同 Wi-Fi 两机全功能；关蓝牙对照。

---

## 9. Consequences

**Positive**：真正无感；LAN 用户无感看到 BLE 用户；网关零配置。
**Negative**：多跳延迟与带宽损耗；需要源路由/补发才完整。

## 10. Revisit Conditions

若跨 transport 转发带来不可接受的功耗或带宽损耗，可引入「网关只在有流量时转发」的节流（不影响语义）。

## 11. References

- `../architecture/04-mesh-engine-and-routing.md`、`../architecture/README.md`
- `docs/notes/bitchat-comparison.md`
