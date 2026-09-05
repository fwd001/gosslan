# ADR-0010: Failure Injection for P2P Reliability

- Status: Proposed
- Date: 2026-09-05
- Related: `docs/protocol-invariants.md`, `e2e_peer.rs`, `scripts/e2e-dev.sh`

## Context

Gosslan 的主要风险来自异常网络，而不是 happy path。

历史问题已经证明：

```text
TCP half-open
→ send() succeeds
→ outbox removed
→ message lost
```

因此普通：

```text
A → B → success
```

测试不足以证明可靠性。

## Decision

逐步建立 failure injection tests。

至少覆盖：

```text
packet/message duplicate
ACK missing
TCP disconnect
TCP half-open
peer offline
peer restart
application restart
delayed delivery
Gossip duplicate
relay unavailable
key unavailable
key changed
```

## Expected Semantics

### ACK lost

```text
sender outbox remains
→ retry
→ receiver dedupe
→ receiver ACK
→ sender deletes outbox
```

### Receiver restart

```text
duplicate message
→ idempotent
→ one DB record
```

### Peer offline

```text
message remains queued
```

## Consequences

测试基础设施复杂度增加，但这是 P2P 软件稳定性的核心投入。

## Revisit

当协议级 failure injection 达到完整覆盖后，再考虑 chaos test / multi-node automated test。
