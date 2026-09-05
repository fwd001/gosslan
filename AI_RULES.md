# Gosslan AI 工程宪法

> Version: 2.0
>
> 本文件是 Gosslan 所有 AI 编程助手的强制工程规则。
> 任何 AI 在修改代码前必须阅读本文件、`AI_PROJECT_HANDOFF.md` 以及与任务相关的 ADR。
>
> 核心目标不是“尽快完成需求”，而是：
>
> **在不破坏系统不变量的前提下，以最小改动实现需求，并让已经修复的问题不再回来。**

---

## 0. 适用范围

Gosslan 是 Tauri v2 + Rust + Vue 3 + TypeScript + Pinia + SQLite 的 P2P LAN 即时通讯系统，包含：

- UDP discovery
- TCP transport
- Gossip
- Outbox / ACK
- E2EE
- 群聊 / GroupKey
- 文件传输 / Relay / Mesh
- SQLite
- Windows / macOS / Android
- Transport abstraction

因此，本项目属于**分布式系统 + 跨平台客户端**。

任何看似局部的修改，都必须考虑：

```text
UI
 ↓
Pinia
 ↓
Tauri invoke / event
 ↓
Rust command
 ↓
domain / service
 ↓
protocol
 ↓
transport / network
 ↓
peer
 ↓
storage
 ↓
event
 ↓
UI
```

---

# 1. AI 启动协议

每次开始非 trivial 任务，必须按以下顺序工作：

```text
1. 读取 AI_RULES.md
2. 读取 AI_PROJECT_HANDOFF.md
3. 定位相关代码
4. 阅读相关测试
5. 阅读相关 ADR
6. 检查 CHANGELOG 中的历史教训
7. 输出影响分析
8. 设计测试
9. 最小实现
10. 验证
11. 自检不变量
12. 更新必要文档
```

禁止“先改再理解”。

---

# 2. Source of Truth

当文档与实现冲突时，优先级：

```text
运行时代码
> 测试
> protocol.rs / db.rs
> schema.sql
> ADR
> AI_PROJECT_HANDOFF.md
> README.md
> CHANGELOG.md
```

但如果发现冲突：

**不能默默选择一方。**

必须报告：

```text
发现冲突：
代码：
文档：
测试：
判断：
建议同步：
```

---

# 3. 系统不变量

完整定义见：

`docs/protocol-invariants.md`

以下规则不可被普通需求覆盖。

## INV-001 Message Identity

可靠消息必须拥有稳定 `msg_id`。

同一消息经过：

```text
Direct
Gossip
Relay
Outbox
Retry
```

不得重新生成业务意义上的新 `msg_id`。

---

## INV-002 Idempotency

收到重复消息：

```text
同 msg_id
→ 不重复入库
→ 不重复通知
→ 不重复显示
→ 必须安全处理 ACK
```

---

## INV-003 Outbox

可靠消息必须：

```text
Create
→ Persist outbox
→ Attempt transport
→ Wait ACK
→ ACK confirmed
→ Delete outbox
```

禁止：

```text
send() == Ok
→ delete outbox
```

TCP 写成功不代表对端已经收到并持久化。

---

## INV-004 ACK Semantics

ACK 的语义必须保持稳定：

> 接收方已经完成消息接收，并满足项目定义的持久化确认条件。

以下都不能直接等价于 ACK：

- socket connected
- TCP write succeeded
- `send()` returned `Ok`
- `try_send()` succeeded

---

## INV-005 Encryption

加密消息：

- 不得静默丢失
- 解密失败必须可观测
- 私钥不得进入前端
- 加密逻辑只能集中在 crypto 层
- 不允许为了 UI 正常显示而绕过加密

---

## INV-006 Network Failure

网络暂时不可用：

```text
Network failure != Message failure
```

可恢复网络错误应该进入：

```text
outbox / retry / reconnect
```

而不是直接永久 failed。

---

## INV-007 Device Identity

`device_id` 和身份密钥必须持久化。

重启：

```text
device_id 不变
identity 不变
```

身份发生变化必须视为显式迁移/重置，而不是普通启动行为。

---

## INV-008 UI State

前端可以乐观更新，但最终状态必须来自真实结果。

```text
optimistic
  ├── success → committed
  └── failure → failed / rollback
```

禁止 UI 自己猜测：

```text
online
delivered
read
encrypted
```

---

# 4. 协议修改规则

任何 `protocol.rs` 的消息、字段、序列化规则变更，必须回答：

```text
1. Sender
2. Receiver
3. Message identity
4. Idempotency
5. ACK
6. Outbox
7. Gossip
8. Encryption
9. Persistence
10. Version compatibility
11. Unknown-message behavior
12. Failure behavior
```

必须检查：

```text
src-tauri/src/protocol.rs
src/types.ts
sender
receiver
gossip_engine.rs
network/transport.rs
db.rs
tests
e2e_peer.rs
```

如果协议发生语义变化：

**必须新增或更新 ADR。**

---

# 5. 状态机规则

禁止用大量互相独立的 boolean 表达复杂生命周期。

优先：

```text
enum / union / explicit state
```

## Message

```text
local_created
→ queued
→ sending
→ delivered
→ read

queued / sending
→ failed_recoverable

sending
→ failed_permanent
```

## Peer

```text
unknown
→ discovered
→ connecting
→ connected
→ healthy
→ offline
```

## File

```text
created
→ offered
→ accepted
→ transferring
→ completed

offered / accepted / transferring
→ failed
```

## E2EE

```text
unknown_key
→ discovering
→ key_available
→ encrypting
→ encrypted
→ decrypted

decrypting
→ decrypt_failed
```

如果实际代码状态与此不同：

**以实际代码为准，并先说明差异。**

---

# 6. Transport 边界

业务层不得直接依赖：

```text
TCP
UDP
Bluetooth
QUIC
Relay
```

应通过 Transport 抽象。

目标结构：

```text
Application
  ↓
Message / File Service
  ↓
TransportManager
  ↓
LAN / Bluetooth / QUIC / Relay
```

禁止：

```rust
if bluetooth { ... }
else if lan { ... }
else if relay { ... }
```

除非代码处于 transport adapter 本身。

---

# 7. 前端边界

当前 `useChatStore.ts` 已承担较多职责。

新功能：

**不要继续无条件塞进 `useChatStore.ts`。**

优先按领域演进：

```text
useAppStore
useChatStore
useConversationStore
useMessageStore
useFriendStore
useTransferStore
useNotificationStore
```

但禁止为了“架构漂亮”一次性大重构。

原则：

```text
新增功能 → 正确边界
旧代码 → 保持稳定
重构 → 独立任务
```

---

# 8. Rust / TypeScript Contract

Rust serde 类型与：

```text
src/types.ts
```

必须保持一致。

修改任意一侧，都必须检查另一侧。

特别注意：

```text
camelCase ↔ snake_case
Option / nullable
enum variant
timestamp
byte encoding
```

如果可能，优先考虑生成类型或增加 contract test，避免长期手工漂移。

---

# 9. Database Rules

任何数据库修改必须考虑：

```text
旧数据库
→ migration
→ 新数据库
```

必须检查：

- 已有数据
- 默认值
- NULL
- index
- unique constraint
- transaction
- rollback
- 重复执行
- `db.rs`
- `schema.sql`

禁止：

```text
修改 schema
→ 假设用户都是新安装
```

---

# 10. Bug Fix Protocol

任何 Bug 修复必须遵循：

```text
复现
→ 定位
→ Root Cause
→ Regression Test
→ Fix
→ 全量验证
```

禁止：

```text
现象
→ 猜一个地方
→ 加 if
→ 测试绿
→ 结束
```

每个 Bug 必须使用：

`docs/templates/BUG_FIX.md`

---

# 11. 测试优先级

修改越靠近下面位置，测试要求越高：

```text
UI
↓
Store
↓
Tauri command
↓
Service
↓
Protocol
↓
Transport
↓
Storage
↓
Crypto
```

尤其是：

```text
protocol
network
crypto
outbox
db
identity
```

不得只做 UI 手工验证。

---

# 12. P2P 故障场景

涉及网络/消息的任务，必须考虑：

```text
正常发送
对方离线
发送中断网
ACK 丢失
TCP 半开
连接重建
重复消息
乱序
Gossip 重复
节点重启
应用重启
outbox 恢复
公钥暂时不存在
公钥变化
中继节点消失
```

---

# 13. 禁止事项

## P-01

未阅读相关模块就修改。

## P-02

复制已有业务逻辑解决 Bug。

## P-03

绕过 protocol。

## P-04

绕过 outbox。

## P-05

绕过 msg_id。

## P-06

绕过 ACK。

## P-07

前端伪造网络状态。

## P-08

Rust / TS 类型不一致。

## P-09

数据库无 migration 修改。

## P-10

协议修改不考虑兼容。

## P-11

核心 Bug 不增加回归测试。

## P-12

为了通过测试修改测试预期。

## P-13

catch 后静默吞错。

## P-14

新增全局状态而不分析生命周期。

## P-15

大重构与功能开发混在一个任务。

## P-16

无必要新增第三方依赖。

## P-17

无关格式化、重命名、升级依赖。

## P-18

重新引入已被 ADR 明确否决的实现。

## P-19

修改密码学原语/密钥派生但不更新协议版本和 ADR。

## P-20

为了消除 warning 删除错误处理。

---

# 14. 密码学规则

禁止：

- 自己实现密码算法
- 修改 key derivation 而不做协议兼容分析
- 将私钥暴露给前端
- 在 UI 层进行加密
- 在多个模块实现不同加密逻辑
- 把“加密失败”转成普通明文发送

密码学变更至少需要：

```text
Threat model
Compatibility
Key lifecycle
Old message behavior
New message behavior
Migration
Test vectors
```

---

# 15. 第三方依赖

新增依赖必须说明：

```text
为什么需要
已有依赖为什么不能完成
体积
维护状态
Windows
macOS
Android
Tauri v2
构建影响
安全影响
```

---

# 16. 最小变更原则

一次任务：

```text
只改完成需求所需的代码
```

禁止顺手：

- 重命名
- 大规模格式化
- 升级依赖
- 重构无关模块
- 改 UI 风格
- 改协议命名

如果发现架构问题：

```text
当前需求的最小修复
+
单独的长期重构建议
```

不要偷偷混在一起。

---

# 17. 强制开发流程

## Phase 1 — Understand

先读代码、测试、ADR。

## Phase 2 — Impact

输出：

```text
Affected files:
Affected protocol:
Affected DB:
Affected state machine:
Affected platforms:
Affected transport:
Affected crypto:
Regression risks:
```

## Phase 3 — Design

给出：

```text
最小方案
备选方案
选择原因
```

## Phase 4 — Test

先定义测试：

```text
normal
failure
boundary
duplicate
reconnect
restart
cross-platform
```

## Phase 5 — Implement

只改必要文件。

## Phase 6 — Verify

至少：

```bash
npm test
npm run build
cd src-tauri && cargo test --lib
cd src-tauri && cargo check
```

协议/网络任务尽可能：

```bash
cd src-tauri && cargo build --example e2e_peer
bash scripts/e2e-dev.sh
```

Android 相关：

```bash
cargo check --target aarch64-linux-android
```

## Phase 7 — Review

检查：

```text
INV-001 ~ INV-008
Protocol compatibility
DB compatibility
Rust/TS contract
Cross-platform
Warnings
Tests
CHANGELOG
ADR
```

---

# 18. AI 输出要求

完成任务后必须报告：

```text
## Summary
[做了什么]

## Root Cause
[如果是 Bug]

## Changed Files
[文件列表]

## Architecture Impact
[协议/DB/Transport/State/Platform]

## Tests
[执行了什么]

## Verification
[结果]

## Remaining Risks
[仍存在什么]

## Documentation
[更新了什么]

## Follow-up
[后续建议]
```

---

# 19. 最终原则

> 不确定时，不猜。

> 不理解数据流时，不改代码。

> 不知道状态来源时，不新增状态。

> 修 Bug 修根因，不修表象。

> 网络系统先保证可靠性，再追求性能。

> 稳定性优先于开发速度。

> 每一个历史 Bug，都应该转化成一个未来的回归测试。

> 每一个架构决定，都应该留下为什么。

> AI 的目标不是“完成这次任务”，而是“完成任务后让系统更难出错”。
