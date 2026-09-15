# ADR-0025: 确定性仿真 + 蓝牙链路策略

- Status: Proposed（V5.0 方向）
- Date: 2026-09-14
- Owners: Gosslan
- Related:
  - 规范：`../architecture/05-radio-policy.md`、`../architecture/06-testing-slo-and-guardrails.md`
  - 依赖：ADR-0020、ADR-0024
  - CHANGELOG: `[Unreleased]`

---

## 1. Context

「蓝牙总是很不稳定」无法收敛的根本原因：**只能靠 2–3 台真机复现**，没有确定性回归、没有 SLO。
同时，连接预算/扫描占空比/冗余链路/自愈/功率等链路策略**缺失或为固定参数**，
而 bitchat 把它们做成了 ~60 个可单测的纯策略对象，并用 `SimulatedMesh` 把多节点测试做到 ~40ms。

---

## 2. Decision

1. 链路策略全部实现为**无 I/O 纯函数/小 struct**（`05`），参数照抄 bitchat 并经真机校准。
2. 建立 `SimulatedMesh`：**真实引擎 + 模拟链路 + 虚拟时钟**（`06` §2）。
3. 定义并采集 **SLO**（`06` §1）。
4. 每阶段绑定自动化护栏 + 真机矩阵（`06` §3/§4）。

---

## 3. Detailed Design

- 策略：`../architecture/05-radio-policy.md`（连接调度、占空比、冗余链路、自愈、功率、出站优先级、分片重组）；
- 仿真与 SLO：`../architecture/06-testing-slo-and-guardrails.md`。

---

## 4. Invariants

```text
INV-NET-40  链路策略是纯函数（无 I/O、可单测）。
INV-NET-41  BLE 拨号纳入全局并发上限。
INV-NET-50  每阶段必须有可自动化的验收。
INV-NET-51  仿真只跑生产引擎，不复制协议逻辑。
INV-NET-52  仿真收敛失败必须 fail。
```

---

## 5. Alternatives

**A. 只靠真机测试**：现状，无法回归；拒绝。
**B. 写一个独立模拟实现**：会与生产逻辑漂移，测试通过不代表线上正确；拒绝（仿真必须跑真实引擎）。
**C. 引入第三方 mesh 仿真框架**：YAGNI，且与本项目类型系统不匹配。

---

## 6. Compatibility

- 策略改变的是**射频节奏与链路管理**，不改线格式与消息语义；
- 所有策略带开关，可退回固定参数（INV-NET-43）。

---

## 7. Failure Modes

- 仿真保真不足 ⇒ 明确写入保真边界（射频/MTU/背压由真机覆盖）；
- 策略参数不适合真机 ⇒ 全部可配 + 真机 A/B；
- SLO 埋点影响性能 ⇒ 采样 + 异步上报。

---

## 8. Testing

本 ADR 本身就是测试策略（`06`）；验收：≥8 仿真场景 + 真机矩阵 + SLO 达标。

---

## 9. Consequences

**Positive**：蓝牙可回归、可量化、可优化到 bitchat 级流畅度。
**Negative**：需要先建仿真底座（Phase 2）+ 埋点（Phase 0）。

## 10. Revisit Conditions

当仿真与真机结果长期不一致时，优先修仿真保真度，而不是放弃仿真。

## 11. References

- `../architecture/05-radio-policy.md`、`../architecture/06-testing-slo-and-guardrails.md`
