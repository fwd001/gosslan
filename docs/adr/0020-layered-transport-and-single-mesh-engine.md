# ADR-0020: 分层传输 + 单串行 Mesh 引擎

- Status: Proposed（V5.0 方向）
- Date: 2026-09-14
- Owners: Gosslan
- Related:
  - 规范：`../architecture/01-layers-and-concurrency.md`、`../architecture/README.md`
  - 依赖：ADR-0021/0022/0024/0025
  - CHANGELOG: `[Unreleased]`
  - Protocol: 无（不改线格式）

---

## 1. Context

现状把**射频生命周期、链路策略、协议状态、可靠消息**全塞在 `network/ble.rs`（2173 行）与
`network/transport.rs`（8399 行）里，靠 `Arc<AppState>` 上约 40 个独立 Mutex 协调：
`state.rs:607-839`。没有单一 owner、没有 mailbox、没有统一状态转移日志。
蓝牙「时好时坏」难以定位与回归，根因在此。

参考 bitchat 的复盘（`docs/BLE-ARCHITECTURE-V3.md`）：把 8.3k 行 god object 拆成
**平台链路层 → 单串行 mesh 引擎 → 特性模块 → 能力协议边界**；并明确一条教训：
**纯策略抽取是胜利，但用 20–30 个闭包 environment 抽 handler 是失败的（状态与同步没跟着走）**。

---

## 2. Decision

1. 采用四层边界：**L2 链路适配器 / L4 单串行 MeshEngine / L5 App / L0 Persistence**（`01` §1）。
2. 引擎是**单串行状态域**，对外只经 `MeshEvent` / `MeshEffect`（`01` §3）。
3. 并发所有权三选一：Engine-confined / Link-confined / Lock-backed snapshot（`01` §2）。
4. **分阶段迁移**：3a 合并为 `Mutex<MeshState>` → 3b actor + DB effect → 3c 删过渡桥（`07`）。
5. 抽取 handler 时**必须连同状态所有权一起搬**，禁止闭包 environment（吸取 bitchat 教训）。

---

## 3. Detailed Design

见 `../architecture/01-layers-and-concurrency.md`：分层表、并发契约、同步边顺序、事件/效果模型、迁移步骤。

---

## 4. Invariants

```text
INV-NET-01  引擎外部不得直接锁引擎状态。
INV-NET-02  链路层不得 import protocol / db / UI 类型。
INV-NET-04  引擎不直接调用 DB / 射频，只产出 Effect。
INV-NET-05  同步边只允许 main→engine→link 方向。
```

---

## 5. Alternatives

**A. 维持现状（继续加 Mutex）**：短期最快，但漂移与死锁风险持续累积；蓝牙继续只能靠真机回归。
**B. 只抽纯策略，不动状态所有权（bitchat V2 路线）**：可部分改善可测性，
但状态仍散落 ⇒ 队列序死锁与「改了但问题不在那里」无法根治。
**C. 一次性重写（big bang）**：与仓库「每阶段可编译可回退」纪律冲突，LAN 回归面不可控。
**选 B 的增强版 + 分阶段 C**：先抽纯策略（已在做），再按 3a/3b/3c 迁移状态所有权。

---

## 6. Compatibility

- **线格式**：不变（C1）。
- **数据**：不变（C2）。
- **旧客户端**：本 ADR 不改协议，不适用。
- **迁移**：Phase 3a 行为零变化；3b 影子双跑。

---

## 7. Failure Modes

- 引擎单串行成为吞吐瓶颈 ⇒ 先度量（`06` §1）；BLE 吞吐远低于单串行上限，风险低。
- actor 化引入新死锁 ⇒ 同步边顺序 + 队列契约 grep 护栏。
- DB effect 丢失 ⇒ effect 需幂等 + worker 重试 + 与 outbox 语义一致。

---

## 8. Testing

- Phase 3a：现有 441 测试 + LAN 黄金回归；
- Phase 3b：影子双跑 diff = 0；≥3 个仿真场景；
- 结构护栏：引擎外部无 `Mutex<MeshState>` 引用。

---

## 9. Consequences

**Positive**：可确定性测试、可回退、平台解耦、蓝牙可回归。
**Negative**：大重构 + 学习成本；需先补 SLO/影子设施。
**Future cost**：特性模块注册机制需要设计（在 Phase 3b 之后）。

## 10. Revisit Conditions

若单串行成为实测瓶颈，或 Phase 3b 影子比对长期无法收敛，则重新评估（可退回 3a 的多锁但统一 `MeshState`）。

## 11. References

- `../architecture/01-layers-and-concurrency.md`、`../architecture/07-roadmap-v5.md`
- `docs/notes/ble-mesh-v4-plan-review-2026-09-14.md`
