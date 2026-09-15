# Gosslan AI Engineering Index

## Required before coding

1. `AI_RULES.md` — 工程宪法
2. `docs/architecture/README.md` — **★ V5 目标架构规范（规范）**
3. `docs/architecture/07-roadmap-v5.md` — **★ V5 实施路线（分阶段）**
4. `docs/acceptance/1.0-release.md` — 当前验收基线
5. `AI_PROJECT_HANDOFF.md` — 全局上下文（注意其时效性声明）
6. `docs/protocol-invariants.md` — 改协议 / 网络 / 加密 / DB 时必读
7. `docs/design-guidelines.md` — 改 UI 时必读
8. Relevant ADR（见下）
9. Relevant tests
10. `CHANGELOG.md` history（改到旧修复区域时）

## V5 Architecture（规范，逐步落地）

| 文件 | 内容 |
|---|---|
| `docs/architecture/00-v5-one-pager.md` | **一页纸批准版**（先看这个） |
| `docs/architecture/README.md` | 原则 P1–P7、LAN 零回归契约 C1–C6、BLE 无感融合概览、索引 |
| `docs/architecture/01-layers-and-concurrency.md` | 分层 + 并发所有权 + 事件/效果 |
| `docs/architecture/02-data-model.md` | 唯一 Peer/Connection 模型（收敛两套旧模型） |
| `docs/architecture/03-link-layer-and-platforms.md` | LinkEvent/LinkCommand + 四平台（含 iOS） |
| `docs/architecture/04-mesh-engine-and-routing.md` | 引擎、路由/网关、BitChat 中继、DB effect |
| `docs/architecture/05-radio-policy.md` | 连接预算/扫描占空比/冗余链路/自愈/功率/背压 |
| `docs/architecture/06-testing-slo-and-guardrails.md` | SLO、确定性仿真、护栏、真机矩阵 |
| `docs/architecture/07-roadmap-v5.md` | V5.0 分阶段实施（交付/验收/回退） |
| `docs/architecture/08-implementation-map.md` | 实现映射：Phase → 文件/符号/测试/回退 |
| `docs/architecture/09-decisions-and-readiness.md` | 开工门禁：待拍板决策 + 就绪清单 |

> `.workbuddy/mesh-task/` 里的旧设计总纲**不入库**，上述 `docs/architecture/` 才是提交版规范。

## Templates

- `docs/templates/BUG_FIX.md`
- `docs/templates/ADR.md`

## ADRs

| ADR | 主题 | 状态 |
|---|---|---|
| `0007-protocol-versioning.md` | 协议版本化与双读 | Accepted |
| `0008-state-machine-boundaries.md` | 状态机边界 | Accepted |
| `0009-rust-typescript-contract.md` | Rust↔TS 契约 | Accepted |
| `0010-failure-injection-testing.md` | 故障注入测试 | Accepted |
| `0011-gossip-envelope-authentication.md` | Gossip 信封认证 | Accepted |
| `0012-logical-sequence-ordering.md` | 逻辑序号排序 | Accepted |
| `0013-transport-priority-queues.md` | 传收优先级双队列 | Accepted |
| `0014-multi-path-connection-selection.md` | 多路径选路（LAN>Routed>BLE） | Accepted |
| `0015-ble-transport.md` | BLE 传输（三端 + 外设） | Accepted，**由 0022/0024 修订方向** |
| `0016-relay-authorization.md` | 中继授权 | Accepted |
| `0017-opaque-external-wire-frame.md` | 外部不透明帧 | Accepted，**生产端见 0023** |
| `0018-window-architecture.md` | 窗口架构 | Accepted |
| `0019-content-transfer.md` | 统一内容传输 | Accepted |
| `0020-layered-transport-and-single-mesh-engine.md` | 分层 + 单串行 mesh 引擎 | **Proposed（V5）** |
| `0021-single-peer-connection-model.md` | 收敛唯一 Peer/Connection 模型 | **Proposed（V5）** |
| `0022-ble-seamless-lan-join.md` | BLE 无感加入 LAN + 网关 + LAN 契约 | **Proposed（V5）** |
| `0023-bitchat-dual-stack-relay.md` | BitChat 双栈透明中继 | **Proposed（V5）** |
| `0024-cross-platform-link-layer-and-ios.md` | 跨平台链路层 + iOS | **Proposed（V5）** |
| `0025-deterministic-mesh-simulation.md` | 确定性仿真 + 蓝牙链路策略 | **Proposed（V5）** |

> Earlier ADRs `0001`–`0006` were removed; their normative content now lives in
> `docs/protocol-invariants.md` (INV-P01…P20) and `AI_RULES.md` (INV-001…008).
> Do not re-create them as a second source of truth.

## Rule

If a document conflicts with executable code or tests, do not silently choose one. Report the conflict and determine whether the documentation or implementation is stale.
