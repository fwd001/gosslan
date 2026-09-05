# ADR-0002: Outbox + Ack 可靠性保障

## 状态

已采纳（v0.7.0，原设计 v0.4.0）

## 背景

v0.7.0 之前，`send_message` 仅在"无直连链路"时才写 outbox，链路存在时直接 try_send；try_send 返回 Ok 即删除 outbox 行。在 Mac→Windows 场景下，TCP 连接看似存在但实际已半开（对端网络切换/休眠），send 成功返回但消息永远不到达，outbox 已删 → 消息永久丢失。

## 决策

1. **一律写 outbox**：`send_message` 不再判断链路是否存在，所有消息先 INSERT OR IGNORE 到 outbox 表
2. **Ack 才删行**：outbox 行仅在收到对方 Ack 时删除
3. **flush_outbox 只补发不删**：Hello/心跳触发时遍历 outbox 重新发送，但不删除任何行
4. **接收方 msg_id 幂等**：重复到达的消息只回 Ack 不重复入库

## 考虑过的方案

1. **TCP keepalive 检测半开**：系统级 keepalive 默认 2 小时，太慢；自定义 keepalive 增加复杂度且仍无法保证 → 放弃
2. **发送超时后标记 failed**：不能区分"慢"和"丢" → 放弃
3. **应用层心跳 + 超时删连接**：已有（5s 心跳），但心跳只能清理连接，无法恢复已发消息 → 配合 outbox 使用

## 代价

- outbox 表在长期离线时会积累大量行（通过 Ack 正常清理，极端情况需要定期 GC）
- 每条消息多一次 SQLite INSERT（实测微秒级，可接受）
- flush_outbox 可能造成重复发送（依赖接收方 msg_id 去重，INV-002）

## 推翻条件

如果引入消息队列中间件（如 NATS）替代 SQLite outbox，可以重新评估。当前 P2P 无服务器架构下，本地 outbox 是唯一可靠选择。
