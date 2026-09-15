# ADR-0021: 收敛为唯一一套 Peer / Connection 模型

- Status: Proposed（V5.0 方向）
- Date: 2026-09-14
- Owners: Gosslan
- Related:
  - 规范：`../architecture/02-data-model.md`
  - 依赖：ADR-0020
  - CHANGELOG: `[Unreleased]`

---

## 1. Context

当前**同时存在两套模型**：
- `state::Peer` + `state::Link`（`state.rs:154/187`）—— 运行时真正用的表；
- `mesh::Peer` + `mesh::Connection` —— mesh 层，`mesh/manager.rs:10` 自述「Phase 2 仍是旁路、不接管 state」。

`network/transport.rs:113-116` 明确写「两套链路表不保证 1:1 同序」，于是 `route_order`（`:125-162`）用端点对齐，
并为「登记窗口」合成假健康候选（`:142-145`）。这是状态漂移与 failover 误判的结构性来源。

---

## 2. Decision

1. **采用 `mesh::Peer`/`mesh::Connection` 作为唯一模型**。
2. `state::Link` 的传输字段并入 `Connection.link`（`LinkHandle`），删除 `state::Link`。
3. `state::peers` 降级为**引擎产出的只读快照** `PeerView`，不再是权威状态。
4. 过渡期保留 `route_order` 作为显式 bridge，Phase 3c 删除。

---

## 3. Detailed Design

规范类型、字段映射、健康模型、身份规则见 `../architecture/02-data-model.md`。

---

## 4. Invariants

```text
INV-NET-10  同 device_id 只有一个 Peer。
INV-NET-11  同 endpoint 的 upsert 幂等，且绝不覆盖 health。
INV-NET-12  Peer 在线 = 任一 Connection 健康。
INV-NET-13  path_kind 由来路决定，不能从 IP 段反推。
```

---

## 5. Alternatives

**A. 保留 `state::Peer/Link` 为唯一模型，删 `mesh::`**：改动面更小，但 `state::Link` 是单连接结构，
无法表达「一个 peer 多连接」而不重新引入平行结构。
**B. 长期共存 + 同步**：即现状，已证明会产生漂移。
**选 A1（用 `mesh::`）**：它本就是为多连接设计的，逻辑更完整。

---

## 6. Compatibility

- 线格式/数据不变；
- 影响面在进程内类型（`state.rs`/`transport.rs`/`ble.rs`/`commands.rs` 读取点），需全量替换；
- `PeerView` 保持前端字段不变（`lib.rs`/`commands.rs` 的序列化形状）。

---

## 7. Failure Modes

- 替换中漏掉某读取点 ⇒ 用编译器 + 结构护栏兜底；
- health 被覆盖 ⇒ INV-NET-11 单测；
- `PeerView` 字段漂移导致前端异常 ⇒ 契约测试（ADR-0009）。

---

## 8. Testing

- `mesh::manager` 现有单测保留；
- 新增「同 endpoint 二次 upsert 不覆盖 health」；
- 结构护栏：全仓库不再出现 `state::Link` 类型。

---

## 9. Consequences

**Positive**：一套真相、failover 可靠、可仿真。
**Negative**：跨模块类型替换工作量大。

## 10. Revisit Conditions

若 `mesh::Connection` 无法表达某平台特有的传输字段，先扩展它，而不是另起一套。

## 11. References

- `../architecture/02-data-model.md`
