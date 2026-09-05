# ADR-0007: Protocol Versioning

- Status: Proposed
- Date: 2026-09-05
- Related: `protocol.rs`, `src/types.ts`, `docs/protocol-invariants.md`

## Context

Gosslan 的协议已经包含 discovery、ChatMessage、Ack、Gossip、GroupKey、文件传输等多种消息。

随着 Noise、BLE、QUIC、Mesh 等能力继续演进，协议字段和语义变化不可避免。

如果没有显式版本策略，AI 很容易直接修改 enum/serde 结构，导致旧客户端：

- 无法解析
- 静默丢消息
- 错误解释字段
- 破坏 ACK / outbox

## Decision

建立显式 protocol version。

协议版本用于表示：

> 线格式或消息语义是否发生兼容性影响。

不把 app version 当 protocol version。

任何破坏兼容性的 protocol change：

```text
protocol version bump
→ compatibility analysis
→ ADR
→ tests
```

## Rules

1. 新增可忽略字段优先保持向后兼容。
2. 删除/重命名字段必须视为 breaking change。
3. 修改字段语义必须视为 breaking change。
4. 新增 Message variant 必须定义旧客户端行为。
5. 未知消息不得导致 panic。
6. 发送方必须根据对端能力决定是否使用新消息。
7. 旧协议消息不能被错误解释成新语义。

## Consequences

协议演进速度会稍慢，但可以避免 AI 在未来通过“直接改 enum”制造跨版本隐性 bug。

## Revisit

当 Gosslan 建立成熟的 capability negotiation 后，可以把部分 version decision 下沉到 capability negotiation。
