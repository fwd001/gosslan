# ADR-0004: Transport 抽象层（业务与传输解耦）

## 状态

已采纳（v0.3.0）

## 背景

Gosslan 当前通过 TCP 传输消息，未来计划支持蓝牙（BLE）、QUIC、服务端中继等多种传输方式。如果业务代码直接依赖具体传输实现，每增加一种传输就需要修改所有消息发送/接收逻辑。

## 决策

引入 `transport/` 模块，定义 `Transport` trait + `TransportManager`：

```
src-tauri/src/transport/
├── mod.rs        # Transport trait + TransportManager + 智能分流
├── lan.rs        # 局域网通道（已实现，适配现有 TCP 网络层）
└── bluetooth.rs  # 蓝牙通道（接口契约，待 btleplug 接线）
```

业务层通过 TransportManager 发送，Manager 根据链路可用性和负载智能选择通道。

另有 `relay/mesh_router.rs` 提供异构 Mesh 桥接（局域网 ↔ 蓝牙跨链路转发）+ TTL 衰减 + 有界 RingBuffer 限流。

## 考虑过的方案

1. **直接 if/else 分发**：`if bluetooth { ... } else if lan { ... }` → 随通道增加变成不可维护的巨型分支 → 放弃
2. **策略模式（运行时切换）**：本质与 Transport trait 相同，Rust 的 trait object 天然支持 → 采纳
3. **Actor 模型（每通道一个 Actor）**：过度设计，当前通道数 ≤ 3 → 暂不需要

## 代价

- 多一层抽象，调试时需要跟踪 TransportManager 的路由决策
- 蓝牙通道目前仅为接口契约（`available()` 返回 false），实际未接线
- TransportManager 的分流逻辑需要随通道增加而演进

## 推翻条件

如果最终只有 TCP 一种传输且蓝牙/QUIC 计划取消，可以简化移除抽象层。但根据路线图，多通道是确定方向。
