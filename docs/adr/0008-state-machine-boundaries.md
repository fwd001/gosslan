# ADR-0008: Explicit State Machine Boundaries

- Status: Proposed
- Date: 2026-09-05
- Related: `AI_RULES.md`, `docs/protocol-invariants.md`

## Context

Gosslan 同时存在消息、Peer、文件、好友、E2EE 等生命周期。

使用大量 boolean 会产生非法组合，例如：

```text
sending=true
delivered=true
failed=true
```

AI 很容易通过增加一个 boolean 快速解决需求，长期造成状态爆炸。

## Decision

新的复杂生命周期优先使用明确的状态枚举/联合类型。

状态转换必须通过明确的 transition。

```text
Current State
→ Event
→ Valid Transition
→ New State
```

## Rules

1. 不新增与现有状态重复表达的 boolean。
2. 不允许任意模块直接修改核心状态。
3. 状态转换必须定义非法转换。
4. 网络状态不能直接决定业务状态。
5. UI 状态不能成为 Rust 业务状态的事实来源。

## Consequences

代码会比简单 boolean 略多，但状态边界清晰，AI 更难制造非法状态组合。

## Revisit

当现有状态机被证明无法覆盖新领域时，新增独立状态机，而不是扩大旧状态机。
