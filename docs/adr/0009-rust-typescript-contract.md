# ADR-0009: Rust-TypeScript Contract Boundary

- Status: Proposed
- Date: 2026-09-05
- Related: `src/types.ts`, `src/api/index.ts`, `src-tauri/src/commands.rs`

## Context

Gosslan 的前端通过 Tauri invoke/event 与 Rust 通信。

Rust serde 类型与 TypeScript 类型如果漂移，会产生：

- runtime undefined
- enum mismatch
- 参数命名错误
- 事件 payload 错误
- UI 静默失败

## Decision

Rust 与 TypeScript 之间视为明确的 API Contract。

任何 command/event/protocol 数据结构变化：

```text
Rust
↕
TS
↕
Tests
```

必须同步检查。

## Preferred Direction

未来可以考虑从 Rust schema 自动生成 TypeScript 类型，减少手工维护。

在生成机制落地前，至少建立 contract tests。

## Rules

1. Rust serde 是运行时契约来源。
2. TS 类型不得自行发明 Rust 不存在的字段。
3. camelCase/snake_case 转换必须显式确认。
4. event payload 同样属于 contract。
5. breaking contract change 必须更新测试和 ADR。

## Consequences

短期会增加一点类型同步成本，但能显著减少跨边界 bug。

## Revisit

当项目具备稳定 codegen pipeline 后，可将手工 `src/types.ts` 逐步替换为生成文件。
