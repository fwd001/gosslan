我认真看了一遍你这个仓库，重点读了：

* `README.md`
* `AI_PROJECT_HANDOFF.md`
* `CHANGELOG.md`
* `docs/protocol-design.md`
* `docs/performance.md`
* `docs/overview.md`
* 项目目录和当前架构说明

项目本身的目标其实已经非常明确，而且已经不是一个“小 Demo”了，而是一个**带 P2P 网络、Gossip、离线队列、E2EE、群聊、大文件中继、SQLite、Tauri 三端、Mesh/Transport 抽象的复杂系统**。([GitHub][1])

我的核心判断是：

> **现在这个项目最大的问题已经不是“AI 不会写代c码”，而是“AI 缺少一个足够强的工程约束和状态模型”。**
>
> 如果继续让 AI 以“用户提一个需求 → AI 找几个文件 → 修改代码 → 跑一下测试”的模式开发，后面一定会越来越容易出现**修 A 坏 B、重复造轮子、协议不一致、状态机错乱、跨平台回归、文档和代码不一致**。

而且从你的 `CHANGELOG` 已经能看到这个趋势：0.5～0.11 连续出现消息不刷新、半开 TCP 丢消息、E2EE 公钥缺失、解密失败静默丢消息、后台 rAF 滞留等问题。([GitHub][2])

所以我建议你现在**不要马上继续堆功能**。

应该先给这个 AI 加一套：

> **“架构守门员 + 变更协议 + 状态机 + 回归测试 + AI 开发流程”**

下面我给你拆开。

---

# 一、先说我对 Gosslan 的整体理解

![Image](https://images.openai.com/static-rsc-4/4RZwRU-q4HbAR_cKeWPgEqXMP6-tKPDhNByMo7L3sZS1a3gZptPhe10pHUW5NqfnVgPJ8KINPtGDR0bzTKAxVn_IiLg8rVPuP_910ltjhHZsAHL5p3r_rGtJCscG9sJsW40iU4N3_iW3OR1z1J4wsjsrx3eAeSdd7ou0cWZ2cfdICVPuUIxWecIq4wPD-Icw?purpose=fullsize)

![Image](https://images.openai.com/static-rsc-4/kOJK1AdTHXz20tBsonNkPwGSZUQnLAv38x0uDoCY_v_KpVTTNCGyqcpKm-qZKOfM5HuM8QReEtkn7eomJgabz2jyegf_LEWpv-dm-mMIQZLFMgArzhl90hgyywxczdLlDqMjlqdKX5MPZeuNBYfA0nzZUaA41KZLy2_KEfinFmYWZWpCOphuT5oRno7gyjGb?purpose=fullsize)

![Image](https://images.openai.com/static-rsc-4/5WzqYBPpSs2_4bF0q-rJE9mKWL8vqpnAVWu_9_ktAyccnxM_7czcAUNp2MUE0raf4t2w5TI8tBZS9fK5dhXOKeFhtk6ahhZxedkYoH3tImmube-QjgmxjWCr3mLViuS76UH5Qw7Ng7WFFZjjmvhpXuTM7OjTxnZ0maVvht7Hw42rSug8zan9pLz7d0cRbrfL?purpose=fullsize)

![Image](https://images.openai.com/static-rsc-4/8wwCCE3mk86K_5Tn1kWZUyh295_jp9TGBRaT7PMGiWZzWCjLa52yEJXDFIN-l1WUxcVIrTZT8BjrRr-yRRXqKtN0d362-XtOmEynSK4JvVJByuqPMbhIzAhqoDMfa5HPimViqwcFSfZzrpPikqEhLR5ts4v887e3CRjemJZIcBUPtTj73g9o9iyo7Fau5UZA?purpose=fullsize)

![Image](https://images.openai.com/static-rsc-4/faV2OY4AYhgBToc1GH_lCbmTnb76ezi761yVpdqNHNXrMi3mAETgFcysy-StI7aC7iyLg6IlCaltw9JGfpY7skf1mzTdkwTOCDrR4Xoq3ebSLp-mZuV1dYd6LJixAjabdMR9UmvBr1siLCCgxtd3AJXpT9FLOvPhRdBWOe2jvA9_FofEbHHQKZAENT81Bc20?purpose=fullsize)

![Image](https://images.openai.com/static-rsc-4/TvIGB6WXYnSvNNo3aDux7b2IkMpxXUDu1vooUg8Czoy-IuSiCrFUA9KF5IVTd0-ZdKtV4wPT3wkzT-NP_brwfnQAuKR0QSeNwHqzwkX8tktZt4iL4D6qygFFcUSiwPmpM7-xYfdD6G2DP_f_dNXpbuv6YyouYLHtOipI3mfAdn-WGsb4mxldIaNK1k7Q0fvq?purpose=fullsize)

现在 Gosslan 实际上是 5 个系统叠在一起：

```text
                    Gosslan
                       │
        ┌──────────────┼──────────────┐
        │              │              │
      UI 层          应用层          网络层
        │              │              │
   Vue / Pinia     Chat / Friend    Discovery
   Components      Group / File     TCP / UDP
        │              │              │
        └──────────────┼──────────────┘
                       │
                  Protocol Layer
                       │
             ┌─────────┼─────────┐
             │         │         │
           E2EE      Gossip    Outbox
             │         │         │
             └─────────┼─────────┘
                       │
                    Storage
                       │
                    SQLite
```

而且未来还准备继续增加：

```text
LAN
 │
 ├── TCP
 │
 ├── Gossip
 │
 ├── Relay
 │
 ├── Bluetooth
 │
 ├── Mesh
 │
 ├── QUIC
 │
 └── Server Relay
```

这意味着它已经进入了一个很危险的阶段：

**复杂度开始指数级增长。**

因为现在一个看似简单的需求：

> “给消息增加一个状态”

实际上可能影响：

```text
Vue UI
 ↓
Pinia
 ↓
Tauri invoke
 ↓
Rust command
 ↓
protocol
 ↓
TCP
 ↓
Gossip
 ↓
outbox
 ↓
Ack
 ↓
SQLite
 ↓
peer event
 ↓
Vue
```

所以如果 AI 只从“当前页面”考虑问题，就特别容易产生 bug。

---

# 二、我认为现在最大的架构问题

## 1. `useChatStore.ts` 已经开始成为“上帝对象”

你自己的 Handoff 已经明确写了：

> `useChatStore.ts` 是好友 / 会话 / 消息合并 / 发送 / 回执 / 文件传输 / 通知的核心 store。([GitHub][3])

这是我最建议你后面重点治理的地方。

现在它承担：

```text
好友
会话
消息
消息发送
消息合并
消息回执
文件
通知
在线状态
未读
```

这对早期开发非常方便。

但是继续增加：

```text
引用回复
Reaction
群成员管理
消息编辑
消息撤回
文件断点续传
多设备
蓝牙
QUIC
```

之后，这个 Store 很容易变成：

```text
useChatStore.ts
    3000 lines
        ↓
    5000 lines
        ↓
    AI 不敢改
        ↓
    AI 再复制一份逻辑
        ↓
    bug
```

### 建议逐渐拆成

```text
stores/
├── useChatStore.ts
├── useConversationStore.ts
├── useMessageStore.ts
├── useFriendStore.ts
├── usePresenceStore.ts
├── useTransferStore.ts
└── useNotificationStore.ts
```

但是**不要现在一次性大重构**。

AI 后续开发的时候遵守：

> 新功能优先进入对应领域 Store，而不是继续无脑塞进 `useChatStore`。

---

# 三、第二个非常重要的问题：协议层必须“冻结”

你现在的协议非常复杂：

```text
Hello
Heartbeat
UserInfo
FriendRequest
FriendAccept
ChatMessage
GroupMessage
Ack
FileOffer
FileAccept
FileChunk
FileDone
ShareTreeRequest
ShareTreeResponse
ShareFileRequest
Gossip
RelayFileOffer
RelayChunk
GroupKey
ReadReceipt
...
```

而 `protocol.rs` 又是整个系统的核心。README 也明确把它作为线格式和消息枚举的中心。([GitHub][1])

这里千万不要让 AI：

> “需要一个新功能 → 在 Message enum 里面随便加东西。”

建议以后建立：

```text
protocol/
├── envelope.rs
├── message.rs
├── chat.rs
├── ack.rs
├── file.rs
├── group.rs
├── discovery.rs
└── version.rs
```

即便现在不重构目录，也要在开发规则中规定：

### 所有协议变更必须回答 7 个问题

```text
1. 谁发送？
2. 谁接收？
3. 是否允许重复？
4. 是否需要 ACK？
5. 是否进入 outbox？
6. 是否允许 Gossip？
7. 是否需要持久化？
```

例如：

```text
ChatMessage

发送：
Sender → Receiver

重复：
允许

去重：
msg_id

ACK：
必须

outbox：
必须

Gossip：
允许

持久化：
必须
```

AI 就不能随便搞。

---

# 四、第三个问题：现在最应该强化的是“状态机”

你已经有很多状态：

### 消息

```text
sending
   ↓
delivered
   ↓
read

       ↘
        failed
```

### Peer

```text
unknown
 ↓
discovered
 ↓
connecting
 ↓
connected
 ↓
heartbeat
 ↓
offline
```

### 文件

```text
created
 ↓
offered
 ↓
accepted
 ↓
transferring
 ↓
completed

       ↘
        failed
```

### 好友

```text
unknown
 ↓
request_sent
 ↓
accepted
 ↓
friend
```

### E2EE

```text
unknown_key
 ↓
discovering
 ↓
key_available
 ↓
encrypted
 ↓
decrypted

       ↘
        decrypt_failed
```

现在最容易让 AI 出 bug 的，就是**跨状态直接操作**。

例如：

```rust
if peer_exists {
    send_message()
}
```

实际上应该是：

```text
PeerState
  ↓
CanSend?
  ↓
KeyAvailable?
  ↓
ConnectionAvailable?
  ↓
OutboxPersisted?
  ↓
Transmit
  ↓
WaitAck
```

所以我要强烈建议你让 AI：

> **不要把业务逻辑写成大量 if/else，而要把关键业务明确成状态机。**

---

# 五、你这个项目最危险的不是 UI，而是“消息可靠性”

这个项目真正的生命线是：

```text
Message
 ↓
msg_id
 ↓
encrypt
 ↓
persist
 ↓
transport
 ↓
relay
 ↓
gossip
 ↓
ack
 ↓
dedupe
 ↓
read receipt
```

你已经在 0.7.0 修过一次非常典型的问题：

> TCP 看起来连接存在，但实际上是半开状态，消息发送成功返回，却没有真正到达；之前 `try_send Ok` 就删除 outbox，导致消息永久丢失。

后来才改成：

> **只有收到 ACK 才删除 outbox。**

这个修复非常关键。([GitHub][2])

这实际上暴露出一个原则：

# “发送成功 ≠ 消息送达”

AI 后面绝对不能破坏这个原则。

---

# 六、我建议你建立一个“不可破坏原则”

给 AI 明确规定：

## Gosslan Invariants

以后任何代码修改，都不能破坏这些：

### INV-001

```text
任何 ChatMessage 都必须拥有稳定 msg_id
```

### INV-002

```text
msg_id 在：

直发
Gossip
Relay
Outbox

必须保持一致。
```

### INV-003

```text
任何发送消息必须先进入 outbox。
```

### INV-004

```text
只有收到 ACK 才能从 outbox 删除。
```

### INV-005

```text
收到重复 msg_id：

不能重复入库
必须 ACK
```

### INV-006

```text
网络失败 ≠ 消息失败
```

### INV-007

```text
UI 乐观更新失败必须回滚或进入 failed 状态。
```

### INV-008

```text
任何加密消息不能因为解密失败而静默丢弃。
```

你现在其实已经有这些规则，只是它们散落在代码、Handoff 和 CHANGELOG 里面。([GitHub][3])

**应该把它们提升成“系统宪法”。**

---

# 七、还有一个非常重要的问题：E2EE 现在其实还有架构债

你自己文档里已经承认：

> 当前是静态 X25519 派生长期密钥，没有 Forward Secrecy，未来计划 Noise XX。([GitHub][3])

也就是说：

```text
现在：

Alice private key
       +
Bob public key
       ↓
长期 shared secret
```

这不是理想的聊天协议。

未来应该：

```text
Identity Key
     ↓
Noise XX Handshake
     ↓
Session Key
     ↓
Message Encryption
```

而不是：

```text
Identity Key
     ↓
所有消息
```

### 但是！

我反而建议：

> **现在不要让 AI 直接开始改 Noise。**

因为这是典型的：

```text
AI 改密码学
↓
协议变化
↓
旧消息
↓
公钥
↓
Gossip
↓
群聊
↓
outbox
↓
跨版本兼容
```

很容易炸掉整个项目。

应该先：

```text
Protocol v1
稳定
↓
建立完整测试
↓
Protocol v2
Noise
↓
兼容 v1
```

---

# 八、我尤其建议你建立“协议版本”

现在协议最好增加：

```text
protocol_version: 1
```

以后：

```text
v1
v2
v3
```

并明确：

```text
最低支持版本
当前版本
```

例如：

```text
Hello {
    protocol_version,
    app_version,
    device_id,
    capabilities
}
```

这样以后：

```text
老版本
    ↓
看到新消息
    ↓
不认识
    ↓
明确拒绝 / ignore
```

而不是：

```text
AI 新增 enum
↓
老客户端 JSON parse fail
↓
消息消失
```

---

# 九、另一个我很担心的地方：文档已经开始“追不上代码”

你现在已经有：

```text
README
AI_PROJECT_HANDOFF
CHANGELOG
protocol-design
performance
overview
schema.sql
```

这其实非常好。

但问题是：

> **AI 非常容易修改代码之后忘记同步所有文档。**

尤其是：

```text
AI_PROJECT_HANDOFF
README
CHANGELOG
protocol-design
schema.sql
```

里面都有重复信息。

所以以后应该规定：

### Source of Truth

```text
协议
→ Rust protocol.rs

数据库
→ db.rs

API
→ commands.rs

前端类型
→ types.ts

功能状态
→ tests

版本历史
→ CHANGELOG
```

而：

```text
README
AI_PROJECT_HANDOFF
docs/*.md
```

都是：

> **派生文档，而不是事实来源。**

这样 AI 就不会：

> “README 写着 XXX，所以我按照 README 修改。”

而应该：

> “先读代码和测试，再判断 README 是否过期。”

---

# 十、你现在最应该加一个东西：Architecture Decision Records

建议增加：

```text
docs/adr/
```

比如：

```text
docs/adr/
├── 0001-p2p-network.md
├── 0002-message-idempotency.md
├── 0003-outbox-ack.md
├── 0004-e2ee-static-x25519.md
├── 0005-tauri-frontend-boundary.md
├── 0006-transport-abstraction.md
└── 0007-protocol-versioning.md
```

每个 ADR 只回答：

```text
问题
为什么这么设计
考虑过什么方案
为什么没选
这个决定有什么代价
什么情况下允许推翻
```

这个对 AI 特别重要。

因为 AI 最大的问题之一就是：

> **它不知道“为什么不能这么写”。**

例如：

```text
为什么不用 Web Worker？
```

普通 AI：

> 性能更好，我给你加回来。

但你的项目历史已经证明：

> WKWebView/Tauri 生产构建下 Worker 曾经导致消息管线 Promise 永不 resolve，从而出现 Mac 消息不刷新。([GitHub][2])

如果没有 ADR，AI 以后很可能重新犯这个错误。

---

# 十一、我建议你给 AI 建一个“禁止事项”

比如：

```text
docs/AI_RULES.md
```

里面明确：

```text
禁止：

1. 未阅读相关模块就修改代码
2. 为解决 bug 复制一套逻辑
3. 绕过 protocol.rs
4. 绕过 outbox
5. 绕过 msg_id
6. 绕过 ACK
7. UI 自己模拟网络状态
8. Rust 和 TS 定义不同的数据结构
9. 修改数据库 schema 却不写 migration
10. 修改协议却不更新协议版本
11. 修改核心逻辑却不增加回归测试
12. 为了让测试通过而修改测试
13. 删除失败逻辑来“解决”失败
14. catch error 后静默忽略
15. 引入新的全局状态
16. 新增第三方库前不评估
17. 大规模重构和功能开发同时进行
```

这会比再写 1000 行需求文档更有价值。

---

# 十二、尤其要禁止 AI 的一种行为

这是 AI 项目里特别常见：

```text
用户：
消息偶尔不显示。

AI：
我发现 mergeMessages 有问题。

修改：
mergeMessages 改掉。

测试：
通过。

完成。
```

这是不够的。

你应该要求：

```text
现象
 ↓
复现条件
 ↓
根因
 ↓
影响范围
 ↓
修改点
 ↓
为什么不会破坏其他路径
 ↓
回归测试
```

例如：

```text
BUG：

Mac → Windows 偶发消息丢失

Root Cause：

半开 TCP 被认为 send 成功，
outbox 提前删除。

Fix：

ACK 前禁止删除 outbox。

Regression:

1. 正常 TCP
2. TCP 中途断开
3. 重连
4. 重复发送
5. Gossip + direct 同时到达
6. 重启后补发
```

这才是工程化。

---

# 十三、我建议你把测试升级成“故障注入测试”

你现在已经有：

```text
npm test
35 Rust tests
e2e_peer
e2e-dev.sh
```

这个基础其实不错。([GitHub][3])

但是对于 Gosslan 这种 P2P 软件：

> **正常情况下测试通过，价值其实没有那么大。**

真正应该测试的是：

```text
断网
丢包
重复包
乱序
延迟
半开 TCP
节点突然退出
节点重启
数据库损坏
公钥变化
对方离线
Gossip 重复
Relay 节点消失
文件中途断开
```

例如：

```text
Test: message survives TCP failure

Alice
 ↓
send
 ↓
outbox
 ↓
TCP connected
 ↓
TCP suddenly killed
 ↓
ACK missing
 ↓
outbox remains
 ↓
Bob reconnects
 ↓
flush_outbox
 ↓
Bob receives
 ↓
ACK
 ↓
Alice deletes outbox
```

这才是真正适合 Gosslan 的测试。

---

# 十四、还有一个架构建议：把“业务”和“传输”彻底分开

现在你已经有：

```text
transport/
├── lan.rs
└── bluetooth.rs
```

这是正确方向。([GitHub][3])

我建议以后必须坚持：

```text
MessageService
      │
      ▼
TransportManager
      │
 ┌────┼────┐
 ▼    ▼    ▼
 LAN  BLE  QUIC
```

而绝对不要：

```text
send_message()
   ↓
if bluetooth
   ...
else if lan
   ...
else if relay
   ...
```

否则以后一定会变成：

```rust
if bluetooth {
   ...
} else if relay {
   ...
} else if gossip {
   ...
} else if tcp {
   ...
}
```

最后没人敢改。

---

# 十五、我甚至建议把“消息服务”独立出来

最终结构可以逐渐变成：

```text
src-tauri/src/

domain/
├── message.rs
├── conversation.rs
├── friend.rs
├── group.rs
└── transfer.rs

application/
├── message_service.rs
├── friend_service.rs
├── group_service.rs
└── transfer_service.rs

protocol/
├── envelope.rs
├── message.rs
└── version.rs

transport/
├── manager.rs
├── lan.rs
├── bluetooth.rs
└── quic.rs

storage/
├── db.rs
├── message_repo.rs
├── friend_repo.rs
└── outbox_repo.rs

crypto/
├── identity.rs
├── session.rs
└── encryption.rs

network/
├── discovery.rs
├── gossip.rs
└── relay.rs
```

不用现在一次性改。

**让 AI 后续新代码按照这个方向增长。**

---

# 十六、最重要的：给你一个可以直接喂给 AI 的总提示词

这个我建议你直接放进：

```text
AI_PROJECT_RULES.md
```

或者：

```text
CLAUDE.md
AGENTS.md
.cursor/rules/
```

如果你用 Claude Code / Cursor / Codex / WorkBuddy 之类，都可以作为长期规则。

下面这个版本我给你写得比较“硬”。

# Gosslan AI 长期开发工程规则

你现在不是在开发一个普通 CRUD 项目。

Gosslan 是一个 Tauri v2 + Rust + Vue 3 + TypeScript 的跨平台 P2P 局域网即时通讯系统，核心包含：

* UDP 广播 / 组播发现
* TCP 点对点通信
* Gossip 消息传播
* E2EE
* ACK / ReadReceipt
* Outbox 离线补发
* SQLite 持久化
* 大文件分片 / Relay
* Mesh
* Transport 抽象
* Windows / macOS / Android

因此：

> 稳定性、协议一致性、数据可靠性和回归测试优先于开发速度。

---

## 一、最高优先级原则

任何需求都必须遵循：

```text
理解现有架构
    ↓
确认数据流
    ↓
确认状态机
    ↓
确认协议影响
    ↓
确认持久化影响
    ↓
设计测试
    ↓
实现
    ↓
运行测试
    ↓
检查回归
```

禁止：

```text
看到一个 UI 问题
→ 直接修改 UI

看到一个网络问题
→ 直接修改 TCP

看到一个状态问题
→ 新增一个 boolean
```

必须先找到真实的数据来源和状态流。

---

# 二、修改代码之前必须做分析

任何非 trivial 修改，必须先回答：

1. 当前功能在哪里实现？
2. 数据从哪里产生？
3. 数据经过哪些层？
4. 哪些地方消费这个数据？
5. 是否涉及 Rust ↔ TypeScript？
6. 是否涉及 protocol？
7. 是否涉及 SQLite？
8. 是否涉及网络？
9. 是否涉及消息状态机？
10. 是否影响其他平台？
11. 是否影响旧数据？
12. 是否需要回归测试？

如果无法回答这些问题，不允许直接修改。

---

# 三、Gosslan 核心不可破坏规则

## INV-001 Message ID

所有消息必须拥有稳定、确定性的 `msg_id`。

同一条消息经过：

* Direct
* Gossip
* Relay
* Outbox

时必须保持相同的 `msg_id`。

---

## INV-002 Idempotency

任何节点收到重复 `msg_id`：

* 不得重复插入数据库
* 不得重复显示 UI
* 必须能够安全 ACK

---

## INV-003 Outbox

任何需要可靠投递的消息：

```text
发送
→ 先进入 outbox
→ 尝试传输
→ 等待 ACK
→ 收到 ACK
→ 删除 outbox
```

禁止：

```text
send() 成功
→ 直接删除 outbox
```

因为 TCP send 成功不代表对方已经收到。

---

## INV-004 ACK

ACK 表示：

> 对端已经确认收到消息。

不得把：

* TCP 写成功
* socket connected
* send() 返回 Ok

当作最终送达。

---

## INV-005 加密

加密消息：

* 不允许静默丢弃
* 解密失败必须进入可观测错误状态
* 不得为了兼容 UI 而绕过加密
* 不得在多个地方实现不同加密逻辑

---

## INV-006 UI 乐观状态

UI 可以乐观更新，但：

```text
成功 → 保持
失败 → 明确 failed / rollback
```

禁止：

```text
catch error {}
```

禁止吞掉错误。

---

# 四、状态机优先

不要使用大量独立 boolean 表达复杂业务状态。

例如：

错误：

```ts
sending = true
delivered = false
failed = false
read = false
```

优先考虑明确状态：

```text
sending
delivered
read
failed
```

网络连接、文件传输、好友关系、E2EE 等同理。

---

# 五、Transport 与业务必须解耦

业务层不能直接依赖：

* TCP
* UDP
* Bluetooth
* QUIC

业务代码应该依赖：

```text
TransportManager / Transport trait
```

结构：

```text
Application
    ↓
Message Service
    ↓
Transport Manager
    ↓
LAN / Bluetooth / QUIC / Relay
```

禁止在业务代码中大量出现：

```text
if bluetooth
else if lan
else if relay
else if gossip
```

---

# 六、Protocol 是公共契约

任何协议修改必须同时检查：

* Rust enum / struct
* serde
* TypeScript types
* protocol version
* encode / decode
* sender
* receiver
* ACK
* Gossip
* Outbox
* E2EE
* 旧版本兼容

如果新增协议消息，必须说明：

```text
sender
receiver
idempotency
ack
persistence
gossip
encryption
```

---

# 七、数据库规则

SQLite 是事实来源之一。

任何 schema 修改必须考虑：

```text
旧数据库
新数据库
已有用户数据
升级路径
重复执行
事务
回滚
```

禁止直接修改 schema 后假设所有用户都是全新数据库。

---

# 八、Rust / TypeScript 数据结构必须一致

Rust serde model 与：

```text
src/types.ts
```

必须保持一致。

修改 Rust protocol / command / event 时：

必须同步检查 TypeScript 类型。

禁止：

```text
Rust 有字段
TS 没有

TS 假设字段存在
Rust 实际不存在
```

---

# 九、禁止复制业务逻辑

如果发现已有：

```text
sendMessage()
```

不得重新实现：

```text
sendChatMessage()
sendDirectMessage()
sendGossipMessage()
sendRetryMessage()
```

除非它们有明确不同的领域职责。

优先复用核心 service。

---

# 十、Bug 修复必须找到根因

所有 bug 修复必须记录：

```text
Bug:
复现步骤:
实际行为:
预期行为:
Root Cause:
影响范围:
Fix:
Regression Test:
```

禁止只修改表象。

例如：

错误：

```text
消息不显示
→ 强制 refresh UI
```

正确：

```text
消息不显示
→ 检查网络
→ 检查事件
→ 检查 store
→ 检查 merge
→ 检查 DB
→ 找到真实断点
```

---

# 十一、每个 Bug 必须增加回归测试

修复 bug 后必须增加至少一个可以重现原问题的测试。

优先级：

```text
纯函数测试
↓
Rust unit test
↓
protocol test
↓
E2E
↓
真实双设备
```

如果无法自动测试，必须说明原因。

---

# 十二、P2P 网络必须测试故障场景

不能只测试：

```text
A → B
```

必须逐渐覆盖：

```text
A → B
A → B 断网
A → B 重连
A → B 重复消息
A → B 消息乱序
A → B 延迟
A → B 半开 TCP
A → B 对方突然退出
A → B 对方重启
A → B ACK 丢失
A → B Gossip 重复
A → B Outbox 补发
A → Relay → B
```

---

# 十三、禁止为了测试通过而修改测试

测试失败时：

优先：

```text
检查实现
```

而不是：

```text
修改测试预期
```

除非需求确实发生变化，并明确说明原因。

---

# 十四、不要进行无必要的大重构

一个需求中：

```text
功能修改
+
架构重构
+
依赖升级
+
UI 重做
```

禁止同时进行。

优先：

```text
最小修改
→ 测试
→ 稳定
→ 后续独立重构
```

---

# 十五、第三方依赖必须谨慎

新增依赖之前必须说明：

```text
为什么需要
是否可以使用已有能力
包大小
跨平台支持
Tauri 支持
Android 支持
维护状态
是否会增加构建复杂度
```

能不用就不用。

---

# 十六、平台兼容

Gosslan 支持：

* Windows
* macOS
* Android

任何修改必须考虑：

```text
desktop
mobile
Windows
macOS
Android
```

平台相关代码必须明确使用：

```rust
#[cfg(desktop)]
#[cfg(mobile)]
```

禁止让 Android 引入仅桌面依赖。

---

# 十七、文档不是事实来源

事实来源优先级：

```text
代码
↓
测试
↓
协议定义
↓
数据库 schema
↓
ADR
↓
Handoff
↓
README
```

如果文档与代码冲突：

不要盲目相信文档。

先检查：

```text
代码
测试
CHANGELOG
```

然后再更新文档。

---

# 十八、重要架构决策必须记录 ADR

重大设计决策放入：

```text
docs/adr/
```

例如：

```text
0001-message-idempotency.md
0002-outbox-ack.md
0003-e2ee.md
0004-transport.md
0005-protocol-version.md
```

ADR 必须说明：

```text
问题
方案
选择原因
放弃方案
代价
未来什么情况下可以推翻
```

---

# 十九、每次开发任务必须采用以下流程

## Phase 1：理解

阅读：

* 相关代码
* 相关测试
* protocol
* storage
* 当前 CHANGELOG
* 相关 ADR

不要立即修改。

---

## Phase 2：影响分析

输出：

```text
Affected modules:
Affected protocol:
Affected database:
Affected UI:
Affected platforms:
Affected state machines:
Regression risks:
```

---

## Phase 3：方案

给出最小修改方案。

如果存在多个方案：

```text
方案 A
方案 B
方案 C
```

说明为什么选择当前方案。

---

## Phase 4：测试设计

先写：

```text
正常场景
失败场景
边界场景
重复场景
断网场景
跨平台场景
```

然后实现。

---

## Phase 5：编码

只修改必要文件。

禁止顺手：

* 重命名
* 大规模格式化
* 无关重构
* 升级依赖
* 改 UI 风格

---

## Phase 6：验证

至少执行：

```text
npm test
npm run build
cargo test --lib
cargo check
```

如果修改 Android：

```text
cargo check --target aarch64-linux-android
```

如果修改协议 / 网络：

必须尽可能运行：

```text
e2e_peer
e2e-dev.sh
```

---

## Phase 7：Review

完成后再次检查：

```text
是否破坏 msg_id？
是否破坏 outbox？
是否破坏 ACK？
是否破坏 E2EE？
是否产生重复消息？
是否破坏旧数据？
是否影响 Android？
是否影响 macOS？
是否产生新 warning？
是否需要更新文档？
是否需要 CHANGELOG？
是否需要 ADR？
```

---

# 二十、最终原则

永远遵守：

> 不确定时，不猜。

> 不理解现有状态机时，不改代码。

> 不知道数据从哪里来时，不新增状态。

> 不知道为什么这么设计时，先找 ADR / CHANGELOG / 测试。

> 修 Bug 时修根因，不修表象。

> 新功能不能破坏已有协议。

> 网络系统优先保证可靠性，再追求性能。

> 稳定性优先于代码数量。

> 宁可少做一个功能，也不要制造新的隐性状态。

> 每一个过去修过的 Bug，都应该变成未来不会再次发生的测试。

---

## AI 的最终工作目标

不是：

> “把用户这一次需求做出来。”

而是：

> “在不破坏现有系统不变量的前提下，以最小改动实现需求，并让这个 bug / 问题以后不再回来。”

这个我认为是你现在最值得直接放进项目里的东西。

---

# 十七、再给你一个“每次提需求前”的短 Prompt

上面那个是**长期规则**。

但是你每次让 AI 开发功能，我建议前面再加这个。

你现在要修改 Gosslan。

不要立即写代码。

先阅读并理解：

1. AI_PROJECT_HANDOFF.md
2. CHANGELOG.md
3. 相关 docs
4. 相关 Rust / Vue / TypeScript / SQLite 代码
5. 相关测试
6. docs/adr/ 中相关架构决策

然后先输出：

### 1. 现状分析

说明当前功能的数据流：

```text
UI
→ Store
→ Tauri Command
→ Rust Service
→ Protocol
→ Transport
→ Peer
→ Storage
→ Event
→ UI
```

只列实际涉及的链路。

### 2. 状态分析

列出这个需求涉及的状态机，以及可能受到影响的状态。

### 3. 架构影响

明确说明：

* 是否修改协议
* 是否修改数据库
* 是否修改 Rust
* 是否修改 TypeScript 类型
* 是否修改 Store
* 是否影响 Gossip
* 是否影响 Outbox
* 是否影响 ACK
* 是否影响 E2EE
* 是否影响 Windows
* 是否影响 macOS
* 是否影响 Android

### 4. 风险

列出至少 3 个最可能出现的回归问题。

特别检查：

* 重复消息
* 消息丢失
* 状态不同步
* 半开 TCP
* 离线补发
* 重启
* 公钥变化
* 跨平台

### 5. 实现方案

给出最小修改方案。

禁止无关重构。

### 6. 测试方案

先列测试，再编码。

至少包含：

* 正常场景
* 失败场景
* 边界场景
* 重复场景
* 网络异常场景
* 回归场景

---

确认方案没有破坏 Gosslan 的核心不变量后，再开始修改。

修改过程中：

* 不复制已有业务逻辑
* 不绕过 protocol
* 不绕过 msg_id
* 不绕过 outbox
* 不绕过 ACK
* 不吞错误
* 不新增没有必要的全局状态
* 不为了通过测试修改测试预期
* 不做无关重构

完成后必须：

1. 运行相关测试
2. 运行构建
3. 检查 Rust warning
4. 检查 TypeScript 类型
5. 检查跨平台影响
6. 检查文档是否需要更新
7. 检查 CHANGELOG 是否需要更新
8. 如果改变架构，新增/更新 ADR

最后输出：

```text
修改内容：
根因/设计原因：
涉及文件：
测试结果：
可能风险：
是否需要文档更新：
是否需要后续架构工作：
```

如果发现当前架构本身存在问题，不要为了快速完成需求而继续堆补丁。

应该先指出问题，并给出：

```text
最小修复方案
```

和：

```text
长期架构方案
```

由我决定是否重构。

---

# 十八、最后，我建议你现在不要继续让 AI 加功能

这是我看完这个项目之后最想提醒你的。

目前仓库在 `v0.11.0`，已经积累了很多能力，但版本历史实际上是**非常高密度地连续修 bug + 加功能**：0.5.x 开始就出现消息刷新、发送失败、虚拟滚动；0.7 又修半开 TCP、outbox；0.8～0.11 连续调整 E2EE；同时又加入托盘、跨平台等。([GitHub][2])

这非常像典型的：

```text
AI 开发速度
      ↑
      │
      │        ／
      │      ／
      │    ／
      │  ／
      │／____________
             代码复杂度
```

现在应该进入：

# “稳定化阶段”

我会建议你下一阶段按照这个顺序：

```text
                Gosslan Stabilization
                         │
        ┌────────────────┼────────────────┐
        ↓                ↓                ↓
   架构不变量        测试体系          文档体系
        │                │                │
   msg_id              E2E             ADR
   outbox              failure         AI_RULES
   ACK                  injection       Handoff
   E2EE                 network         CHANGELOG
        │                │                │
        └────────────────┼────────────────┘
                         ↓
                  Protocol v1 Freeze
                         ↓
                  稳定版本 0.12
                         ↓
                 再开始新功能
```

---

## 我尤其建议你做一个 `v0.12 Stabilization`

目标甚至可以明确写成：

> **0.12 不新增产品功能，只解决架构债、可靠性、测试和协议稳定性。**

重点做：

### P0

* 消息可靠性测试
* Outbox 测试
* ACK 测试
* Gossip 重复测试
* 网络断开/重连
* 重启恢复
* E2EE 失败路径
* 公钥变化
* DB 恢复

### P1

* Protocol version
* ADR
* `AI_RULES.md`
* Message Service 抽象
* Store 边界
* Transport 边界

### P2

* Noise XX
* Bluetooth
* QUIC
* mDNS

你现在文档里自己规划的 Noise XX、mDNS、BLE、QUIC 等确实都有价值，但**不要让 AI 同时推进这些方向**。项目文档也明确把它们列为后续演进方向，而不是当前稳定能力。([GitHub][3])

---

# 最后一个非常关键的判断

你这个项目其实**不缺功能设计文档**。

`AI_PROJECT_HANDOFF.md` 已经写得相当详细了，甚至包含功能清单、代码导读、协议状态机、工程规范和测试口径。([GitHub][3])

现在缺的是：

> **“AI 什么情况下绝对不能动、动之前必须证明什么、动完之后必须验证什么。”**

也就是从：

```text
AI Handoff
```

升级成：

```text
AI Engineering Constitution
```

我认为这会对你后面这个项目的稳定性提升非常明显。

另外，你现在已经有 `AI_PROJECT_HANDOFF.md`，所以我**不建议再造一个巨大无比的“AI 开发说明书”**。最好最终整理成这 4 个核心文件：

```text
/
├── AI_PROJECT_HANDOFF.md    # 项目全貌：是什么、现在有什么
├── AI_RULES.md               # AI 开发宪法：什么能做、什么不能做
├── docs/
│   └── adr/                  # 为什么这么设计
└── CHANGELOG.md              # 历史：过去为什么改
```

其中 **`AI_RULES.md` 是我认为现在最应该补上的。**

如果你愿意，我下一步甚至可以直接**按照你这个 Gosslan 仓库的实际代码结构，给你设计一套完整的 `AI_RULES.md + docs/adr/ + Protocol 不变量文档 + Bug 修复模板`，做到可以直接复制进 GitHub 仓库使用**，而不是泛泛的 AI 开发规范。[Gosslan GitHub 仓库](https://github.com/fwd001/gosslan?utm_source=chatgpt.com)

[1]: https://github.com/fwd001/gosslan.git "GitHub - fwd001/gosslan: Gosslan — 无服务器 P2P 局域网即时通讯（Tauri v2 + Vue 3 + TypeScript） · GitHub"
[2]: https://github.com/fwd001/gosslan/blob/main/CHANGELOG.md "gosslan/CHANGELOG.md at main · fwd001/gosslan · GitHub"
[3]: https://github.com/fwd001/gosslan/blob/main/AI_PROJECT_HANDOFF.md "gosslan/AI_PROJECT_HANDOFF.md at main · fwd001/gosslan · GitHub"
