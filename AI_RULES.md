# Gosslan AI Development Rules

> Version: 1.1（2026-09-20：定位、范围与跨版本兼容三节按现实重写）
> Status: Release track / Decentralized mesh chat & team collaboration
> Project type: Tauri v2 + Vue 3 + TypeScript + Rust + SQLite
> Primary goal: 稳定 · 速度 · 可达 · 安全 · 可解释（见 §1）
>
> This document is the primary engineering rule for AI-assisted development.
> When a task conflicts with this document, stop and resolve the conflict before coding.

---

# 1. Project Positioning

Gosslan 是一个**去中心化的 mesh 通讯工具**：以局域网为主，跨网段靠节点互相中继转发，
蓝牙用于近场加入。节点既互为中继也可以直连；没有服务器、没有账号、没有强制升级 ——
每台设备都是对等的一份。

对用户的目标只有一句：**像普通聊天/团队协作工具一样无感地使用**。
技术上的四个目标按优先级排列：

```text
稳定（不丢、不重、不炸、不静默失败）
    ↓
速度（大文件与聊天并发时都不互相饿死）
    ↓
可达（跨网段/蓝牙/离线补发都要真的能到）
    ↓
安全（身份锚定、E2EE、中继不越权）
    ↓
可解释（用户看得懂现在在发生什么：进度、失败原因、版本差异）
```

产品判断标准（用户反复强调的两条）：

* **点击即乐观响应**：任何操作在界面上必须立刻有反馈，慢的工作放后面做。
* **状态必须可感知**：发送方进度真实推进、`delivered` = 对方收完且合成完、
  `read` = 对方真的看到。"0% 却已读""一直转圈但其实成功"这类组合算缺陷，不算边界情况。

稳定性优先于新功能：不为了"架构上能支持"就实现未来特性（见 §3）。

---

# 2. v1.0 Scope

## 2.1 v1.0 Core Goals

The following are P0:

* LAN device discovery
* Friend request / accept
* Online / offline state
* Single chat
* Text messages
* Image messages
* File messages
* Chat history
* Unread messages
* Delivery state
* Read receipt
* Failed state
* Offline message persistence
* Automatic resend
* Message deduplication
* E2EE
* SQLite persistence
* Restart recovery
* Notification
* Basic Windows / macOS / Android stability
* Two-device real LAN communication

The following are P1:

* Shared directory
* Friend delete / re-add
* Tray behavior
* Basic stress testing
* UI stability improvements

## 2.2 已经扩出的范围（2026-09-20 记录，避免文档与代码互相打脸）

2.1 那份清单是"LAN Chat 稳定"时期的 v1.0 目标，它们仍然是 P0 地基。此后产品定位已扩大为
mesh（见 §1），下面这些**同属 v1.0 必做项**，不再算"未来特性"：

* 群聊与团队协作（群消息、群文件、群任务、删除/退群的一致性）
* 多路径并存与选路（LAN / 跨网段 Routed / 蓝牙），含大文件分片流的**保序**
* 中继转发与中继授权（用户能表达"别拿我当中转"，且设置真的管到数据面）
* 离线补发在三种队列（单聊 / 群 / 文件）上语义一致
* **跨版本优雅降级**（INV-P24：未知帧不拆链、未知内容不显示裸 JSON、新能力按版本门控）

验收口径见 `docs/acceptance/1.0-release.md`。

---

# 3. Explicitly Out of Scope（当前不做，别主动碰）

2026-09-20 重写。旧版本这一节把 Bluetooth / 跨子网 / Mesh 路由 / 中继优化列为"冻结"，
但代码早已越过（ADR-0014 多路径选路、ADR-0015 BLE 传输、ADR-0016 中继授权、
ADR-0017 opaque 外部帧，且三端 BLE 已发布）。**约束与代码相反时，每个新会话读到的第一课
都是"别做你正在做的事"**，那是无效约束。所以按当前定位改成下面这份真实清单。

**已经在范围内、不要再去"冻结"它**：蓝牙传输与近场加入、跨网段与中继转发、多路径选择与
保序、中继授权策略、群聊与团队协作、离线补发、文件断点续传。这些都是**当前产品本体**，
改进它们属于本职工作。

不要主动实现、重设计、扩展的：

* QUIC / 自建新传输栈
* mDNS 发现（现有 UDP 广播 + 手动端点已经够用）
* 账号体系、服务端注册、任何需要"有个后台"的东西
* Noise XX 之类的握手重做（现有 Hello 签名认证已经承担该职责；要动走 ADR）
* 500~1000 节点规模的专门优化、全局索引/FTS、多设备漫游、消息编辑
* 大改架构的重构（拆文件、挪模块属于允许的整理，不改变行为）
* 当前任务不需要的路线图特性 —— **不要因为架构上能支持就实现它**

保留扩展点是允许的（接口留白、注册表、capability 位），但不要为它写实现。

> 想碰上面任何一条：先停下问用户，别自己开工。

---

# 4. Engineering Philosophy

Priority order:

```text
Correctness
>
Reliability
>
Simplicity
>
Maintainability
>
Extensibility
>
Performance optimization
>
Future-proofing
```

Prefer:

```text
reuse existing code
>
small local fix
>
small abstraction
>
large refactor
```

Do not introduce complexity unless the current requirement actually needs it.

A theoretically elegant architecture is not automatically better.

For this project:

> A simple solution that is correct is better than a sophisticated solution that creates more failure paths.

---

# 5. AI Task Complexity

Not every task requires full architectural analysis.

## L1 — Trivial

Examples:

* UI text
* CSS
* layout
* icon
* simple component change
* small pure function
* obvious bug
* simple validation

Process:

```text
Locate → Modify → Test
```

Do not perform unnecessary architecture analysis.

---

## L2 — Normal Feature

Examples:

* normal chat feature
* notification behavior
* unread behavior
* UI interaction
* existing API integration
* small database query change

Process:

```text
Locate existing logic
→ Reuse existing implementation
→ Identify affected files
→ Make minimal change
→ Test
```

Do not redesign the system unless required.

---

## L3 — Core Reliability

Examples:

* protocol
* message lifecycle
* network transport
* discovery
* E2EE
* SQLite schema
* outbox
* ACK
* retry
* deduplication
* connection lifecycle

Process:

```text
Understand data flow
→ Identify invariants
→ Identify failure paths
→ Plan minimal change
→ Implement
→ Add regression test
→ Run relevant E2E tests
```

L3 tasks require strict engineering discipline.

---

## Important Rule

> Do not upgrade an L1/L2 task into an L3 architecture project.

Do not turn:

```text
"fix this button"
```

into:

```text
"let's redesign the state architecture".
```

Do not turn:

```text
"fix this message bug"
```

into:

```text
"let's rewrite the entire messaging subsystem".
```

---

# 6. Existing Code First

Before creating new logic:

1. Search the repository.
2. Find existing implementation.
3. Understand how it currently works.
4. Reuse it when possible.
5. Modify the existing path when appropriate.

Do not create a second implementation of an existing responsibility.

Examples:

Bad:

```text
Existing message send logic
+
new message send helper
+
new message service
```

Preferred:

```text
Existing message send logic
+
minimal modification
```

Only introduce a new abstraction when the existing structure genuinely prevents a correct implementation.

---

# 7. Source of Truth

Prefer the following order:

```text
Actual implementation
>
Existing tests
>
Protocol definitions
>
AI_RULES.md
>
ADR
>
Handoff documentation
>
README
>
Future roadmap
```

Documentation describes the system.

The actual code and tests determine current behavior.

If documentation and implementation disagree:

1. Do not blindly assume either one is correct.
2. Inspect the current data flow.
3. Determine intended behavior.
4. Fix documentation or implementation as appropriate.
5. Do not silently create a third behavior.

---

# 8. Core Message Invariants

These invariants are mandatory.

## Exceptions Must Be Registered

If a code path **must** deviate from an invariant, both of the following are required:

1. A marker at the code site: `// INV-EXCEPTION: INV-PXX — <why this exception is necessary>`
2. An entry in `docs/protocol-invariants.md` §22 (`INV-P22`): id + reason + origin

`scripts/check-invariant-exceptions.mjs` verifies both directions; either side missing fails CI.

An **unregistered exception is a bug** — not because the exception itself is wrong, but because
it is invisible to whoever reads the docs. The required-reading list points at
`protocol-invariants.md`, so a reader will see `insert_self_message` skipping the outbox and
"fix" it back — which would genuinely break the invariant that the exception was protecting
(the outbox row can never be acked, so `flush_outbox` would resend it on every heartbeat).

Exceptions are not a licence to break invariants freely. Each one must answer
"why is there no other way", and only invariants that are **actually** deviated from get
registered — padding the registry with invariants that were never violated defeats its purpose.

## INV-001 — Stable Message ID

Every logical message must have a stable `msg_id`.

Retries must reuse the same `msg_id`.

Do not create a new message ID merely because transmission failed.

---

## INV-002 — Idempotency

Receiving the same `msg_id` multiple times must not create duplicate logical messages.

This applies to:

* direct delivery
* retry
* reconnect
* outbox resend
* gossip
* duplicate packets

---

## INV-003 — Outbox Before Delivery

Reliable outgoing messages must follow:

```text
Create message
    ↓
Persist message
    ↓
Persist outbox
    ↓
Attempt network delivery
```

Do not rely solely on memory.

---

## INV-004 — ACK Means Persisted

A successful TCP write does NOT mean the message was delivered.

The sender may consider a message delivered only after the receiver confirms the appropriate persistence/processing point through the protocol ACK.

Conceptually:

```text
TCP send
≠
Delivered
```

---

## INV-005 — No Silent Message Loss

Encrypted or network messages must not disappear silently.

If processing fails:

```text
success
or
explicit failure / retry / diagnostic path
```

Never:

```text
catch error
→ ignore
→ pretend nothing happened
```

---

## INV-006 — UI State Must Reflect Reality

Do not leave a message permanently in:

```text
sending
```

when the system already knows that the operation failed.

Likewise:

```text
network temporarily unavailable
```

must not automatically mean:

```text
message permanently failed
```

---

## INV-007 — Device Identity Persists

The device identity must remain stable across application restarts.

Do not regenerate identity keys or device fingerprints on every launch.

---

## INV-008 — Persistence Is Part of Reliability

Important state must survive restart.

At minimum:

* device identity
* cryptographic identity
* friends
* messages
* message status
* outbox state where applicable

---

# 9. Message Lifecycle

The intended reliable flow is:

```text
User sends message
        ↓
Create msg_id
        ↓
Persist message
        ↓
Persist outbox
        ↓
Encrypt
        ↓
Send
        ↓
Receiver decrypts
        ↓
Receiver persists / deduplicates
        ↓
Receiver ACK
        ↓
Sender marks delivered
        ↓
Remove outbox
```

Do not bypass this flow for convenience.

---

# 10. Temporary Network Failure

Network failure is not automatically permanent message failure.

Example:

```text
A sends message
        ↓
B offline
        ↓
message remains in outbox
        ↓
B comes online
        ↓
connection restored
        ↓
message resent
        ↓
ACK received
        ↓
outbox removed
```

This behavior is a core requirement.

---

# 11. Protocol Rules

All protocol messages must use the existing protocol layer.

Do not create random protocol formats in unrelated modules.

Before modifying a protocol message, check:

1. Who sends it?
2. Who receives it?
3. Is it persisted?
4. Is it encrypted?
5. Is it idempotent?
6. Does it require ACK?
7. Does it interact with outbox?
8. Does reconnect/retry change its behavior?
9. Does it affect gossip or forwarding?
10. Does the TypeScript side depend on it?
11. **老版本遇到它会看到什么？**（连不上？静默丢？看得懂？）—— 答案必须是"忽略并继续"
    或"可解释地降级"，见 INV-P24
12. **它需要 `protocol_version` bump 吗？**加可选字段不需要；新增 variant / 改必填 /
    改语义都需要，且必须按 capability 门控（ADR-0007）

---

# 12. Cross-Version Protocol Compatibility

## IMPORTANT — 前提已在 2026-09-20 更正

本项目**已经在外发布**：GitHub Releases 上有带安装介质与 sha256 校验的正式包
（`v4.8.2` 2026-09-14 → `v4.20.0` 2026-09-18，覆盖 macOS / Windows / Android）。
它没有服务器、也没有强制升级通道 —— 所以"网里同时存在多个版本"是**当前事实**，
不是假设场景。一台 4.18 的手机和一台 4.22 的 Mac 在同一个局域网里说话，就是日常。

本节早期写的是"尚未发布、没有历史客户端需要兼容"，那个前提被仓库自己证伪了；
它曾把 `docs/adr/0007-protocol-versioning.md` 一直压在 Proposed 状态。

因此规则从"不要写兼容层"改成下面这条更准的说法：

> **兼容性靠"加法 + 门控"获得，不靠"两边都留一份"获得。**

允许的（也是首选的）兼容手段：

* 只**加可选字段**，读侧对缺失值有默认行为（老对端不发也能跑）
* 未知内容一律**降级并可解释**（INV-P24：未知帧不拆链、未知 kind 不显示裸 JSON）
* 新帧 / 新语义必须按对端 `protocol_version` 或 capability **门控**，不门控不许发

不允许的：

* 为"理论上可能存在的旧版本"写双份消息格式、版本适配器、迁移代码
* 一条 wire 变更同时"破坏老版本"又"没有任何提示"（静默连不上比功能缺失严重一个数量级）
* 把 legacy 分支当永久状态 —— 每个 legacy 分支必须在 ADR 里写明**退出条件**

判断要不要兼容，看的是"有没有已发布的包还可能在网里"，答案在 2026-09-20 之后是"有"。

---

## 12.1 Breaking Protocol Changes

允许破坏兼容，但必须先满足这三条（缺一不可）：

1. 写 ADR：谁会断、断了他看到什么、legacy 分支什么时候删；
2. `protocol_version` bump，且发送侧对新帧/新语义做 capability 门控；
3. **先让"能容忍 Unknown + 会报版本"的版本铺开**，之后才允许出现新帧类型
   （顺序颠倒 = 老设备直接连不上，见 INV-P24 的"现状"段）。

Examples:

```text
old message format
→ new message format
```

or:

```text
old protocol field
→ removed
```

or:

```text
old state machine
→ corrected state machine
```

上面三类例子都属于**破坏兼容**，因此先要满足 12.1 开头那三条门，再走下面的清单：

> Breaking does not mean careless.

For a protocol-breaking change:

1. Update sender.
2. Update receiver.
3. Update related tests.
4. Update E2E tests.
5. Update protocol documentation if necessary（`docs/protocol-invariants.md` / ADR）.
6. 只在"已确认没人在用"之后才删 legacy 分支，并在 ADR 里记下这个判断依据。
7. Verify the complete message flow.
8. 想清楚"老版本遇到它到底看到什么"：理想答案是"忽略并继续"，最差也不能是"连不上"。

---

# 13. Database Schema Rules

同上的前提修正也适用于 SQLite：**已经发布的包里装着真实用户数据**。

因此 schema 变更的口径是：

> **允许清理与破坏性改动，但已发布包装着真实用户数据 ⇒ 必须能升不能毁，且不许把老数据读错。**

具体到本项目已经在做的那套（不要再造第二套）：

* `user_version` 单调递增 + 分步迁移；**降级一律拒绝启动**（不猜、不删数据）——
  拒绝必须给用户可读解释（"本机数据来自更新版本，请升级后再打开"），不能只是一个错误。
* 迁移只做"结构搬运/存量清理"，不做"顺手改语义"；一次迁移一件事。
* 不为"从没发布过的中间态"写迁移（那是真正的假想兼容）；为"已发布包里可能有的状态"写，
  并且要在迁移测试里**造出那个旧状态**再验证。
* 读侧对历史形态要能容错（`protocol::display_kind` 是现成的例子：老 `video/audio` 行
  在**读出口**归一成 `file`，不回写数据）。

---

## Allowed

Examples:

```text
bad column
→ replace column

incorrect schema
→ redesign schema

duplicate state
→ consolidate state

temporary development table
→ remove table
```

when the change is justified.

---

## Still Required

A database change must:

* keep the current application consistent
* correctly initialize a fresh database
* correctly handle the current development database
* update related queries
* update tests
* avoid silent data corruption

If an existing development database can simply be recreated, say so explicitly.

Do not create ten layers of migration code to protect a database that has never shipped.

---

# 14. Compatibility Rule

Use this decision:

```text
Has this behavior been released to real users?
        │
        ├── YES → preserve compatibility
        │
        └── NO
             ↓
       Is breaking change useful?
             │
             ├── YES → allow breaking change
             │
             └── NO → keep existing behavior
```

The purpose of compatibility is to protect real users.

Do not create compatibility complexity without a real compatibility requirement.

---

# 15. State Machines

When modifying core state, understand the existing state machine.

Important states include:

### Message

```text
sending
  ↓
delivered
  ↓
read
```

Failure paths may include:

```text
sending
  ↓
retry
  ↓
delivered
```

or:

```text
sending
  ↓
failed
```

depending on the actual error.

---

### Connection

Conceptually:

```text
disconnected
→ connecting
→ connected
→ disconnected
```

Do not invent UI-only connection states that disagree with the Rust/network layer.

---

### File Transfer

Respect the existing:

```text
offer
→ accept
→ transfer
→ complete / failed
```

flow.

Do not create an independent file-transfer lifecycle in the frontend.

---

# 16. Rust / TypeScript Boundary

Rust is responsible for:

* network
* discovery
* encryption
* database
* protocol
* reliable delivery
* file transfer
* platform integration

TypeScript/Vue is responsible for:

* UI
* interaction
* presentation state
* user-facing error display
* frontend orchestration

Do not move core network/protocol logic into the frontend simply because it is easier to implement.

Do not create a second business implementation in TypeScript.

---

# 17. Frontend State Rules

Frontend stores should reflect backend reality.

Do not let the UI invent:

* delivery state
* connection state
* encryption state
* online state
* persistence state

Example:

Bad:

```text
send() succeeded
→ immediately show delivered
```

Preferred:

```text
send()
→ backend processes message
→ ACK
→ update delivered
```

Optimistic UI is allowed only when failure/rollback behavior is clearly defined.

---

# 18. Database Rules

Before modifying the database:

1. Find existing schema.
2. Find existing query functions.
3. Find all callers.
4. Check tests.
5. Determine whether the change is actually necessary.

Do not create duplicate storage for the same concept.

Bad:

```text
existing unread state
+
new unread state
```

Preferred:

```text
existing source of truth
+
correct it
```

---

# 19. Crypto Rules

E2EE is mandatory.

Never introduce:

```text
plaintext fallback
```

when encryption fails.

Never silently:

* disable encryption
* bypass encryption
* accept unverifiable keys
* ignore authentication failures

If cryptographic processing fails:

```text
explicit error
or
explicit retry path
```

not silent fallback.

Do not replace existing cryptographic primitives without a strong reason.

---

# 20. Error Handling

Never hide core errors.

Avoid:

```rust
let _ = something();
```

when the result matters.

Avoid:

```text
catch
→ ignore
```

especially for:

* network
* database
* encryption
* protocol
* file transfer
* persistence

Errors should either:

1. be handled correctly,
2. be returned,
3. be logged with useful context,
4. or be transformed into a meaningful user-visible state.

---

# 21. Bug Fix Workflow

For a bug:

```text
Symptom
↓
Reproduce
↓
Trace actual data flow
↓
Locate break point
↓
Find root cause
↓
Minimal fix
↓
Regression test
↓
Run relevant verification
```

Do not start with:

```text
"Let's refactor the architecture."
```

unless the architecture is actually the root cause.

---

# 22. Bug Fix Report

After fixing a bug, report only:

```text
Root Cause:
...

Fix:
...

Verification:
...
```

Do not produce a long theoretical explanation unless requested.

---

# 23. Regression Test Rule

If a bug is caused by a reproducible logic problem:

> Add a regression test when practical.

Do not modify an existing test only to make it pass.

Never weaken an assertion just because the implementation currently fails it.

If a test is genuinely incorrect:

1. Explain why.
2. Fix the test.
3. Fix the implementation if necessary.
4. Run the complete relevant test set.

---

# 24. Refactoring Rules

Do not combine:

```text
feature
+
large refactor
+
dependency upgrade
+
architecture redesign
```

in one task unless explicitly requested.

Prefer:

```text
small change
→ test
→ stable
→ next change
```

A refactor is justified only when it directly improves the current task or removes a demonstrated source of bugs.

Do not refactor merely because another architecture looks cleaner.

---

# 25. Dependency Rules

Do not introduce a new dependency unless it provides a meaningful benefit.

Before adding one:

* check whether an existing dependency already solves the problem
* check whether the standard library is sufficient
* consider build size
* consider platform compatibility
* consider Tauri/WebView compatibility
* consider maintenance cost

Do not add a dependency for a trivial helper function.

---

# 26. Platform Rules

Gosslan targets:

* Windows
* macOS
* Android

When modifying platform-sensitive code:

* do not assume desktop-only behavior
* do not assume Windows-only APIs
* do not assume macOS-only APIs
* do not introduce browser-only behavior into Tauri core logic
* consider Android limitations when modifying shared Rust code

However:

> Do not over-engineer cross-platform abstractions before a real platform problem exists.

---

# 27. Performance Rules

Do not optimize based on theory alone.

First establish:

```text
Is there an actual performance problem?
```

Then:

```text
Measure
→ identify bottleneck
→ make focused optimization
→ measure again
```

Do not introduce:

* caches
* worker systems
* queues
* complex schedulers
* advanced routing
* custom concurrency

just because they might be faster.

Correctness comes first.

---

# 28. Testing Levels

## L1

Simple change:

```text
targeted test
```

## L2

Feature:

```text
targeted tests
+
npm test / cargo test where relevant
```

## L3

Core system:

```text
unit tests
+
integration tests
+
E2E
+
build
```

For message/network changes, prefer real protocol-path testing over mocks.

---

# 29. v1.0 Minimum Verification

At minimum:

```bash
npm test
npm run build

cd src-tauri
cargo test --lib
cargo check
```

For network/protocol/message changes:

```bash
cd src-tauri
cargo build --example e2e_peer
```

and:

```bash
bash scripts/e2e-dev.sh
```

When possible, verify with two actual devices on the same LAN.

---

# 30. Real Device Acceptance

Automated tests are not enough. 跨版本、多路径、蓝牙与中继这类问题只在真机上暴露，
单测与构建全绿也可能整体不可用。

At least two real devices should verify:

```text
Discovery
↓
Friend
↓
A → B message
↓
B → A message
↓
100 messages
↓
Offline
↓
Outbox
↓
Reconnect
↓
Automatic resend
↓
ACK
↓
Read receipt
↓
Restart
↓
Continue chatting
↓
Image
↓
File
```

The final goal is:

> The chat works reliably in the real LAN environment.

---

# 31. Frozen Feature Rule

If a task is not required for current LAN Chat stability:

Do not proactively implement it.

If you notice a possible future improvement:

```text
Do not implement automatically.
```

Instead:

```text
Record it as a possible future improvement.
```

Do not let future requirements contaminate current implementation.

---

# 32. No Duplicate Logic

Before adding a function, ask:

```text
Does this responsibility already exist?
```

If yes:

```text
Reuse or modify it.
```

Do not create:

```text
sendMessage()
sendChatMessage()
sendReliableMessage()
sendP2PMessage()
sendNetworkMessage()
```

when they represent the same responsibility.

One clear path is preferred.

---

# 33. No Unnecessary Abstraction

Do not create:

```text
IMessageService
MessageServiceFactory
MessageTransportProvider
MessageRepositoryFactory
MessagePipelineCoordinator
```

unless there is a real current requirement for them.

The goal is understandable code.

Not maximum abstraction.

---

# 34. Change Scope

Every task should answer:

```text
What needs to change?
```

Prefer the smallest set of files that can correctly solve the problem.

Avoid unrelated:

* renaming
* formatting
* dependency upgrades
* directory restructuring
* code style rewrites

unless requested.

---

# 35. When AI Must Stop and Ask

Normally, continue implementing without unnecessary questions.

Stop and ask only when:

1. Requirement is genuinely ambiguous.
2. Two interpretations produce materially different behavior.
3. A security decision is required.
4. A destructive data operation is required.
5. A major architectural decision is unavoidable.
6. A new dependency is required but has meaningful trade-offs.
7. A breaking change affects a real released client.

Do not stop merely because:

```text
"there are several ways to implement this."
```

Choose the simplest correct implementation.

---

# 36. When AI Should NOT Ask

Do not ask for confirmation for:

* obvious UI fixes
* obvious bug fixes
* existing patterns
* straightforward refactors within the same module
* adding a regression test
* updating documentation to match implemented behavior
* small internal API changes
* pre-release protocol cleanup
* pre-release DB cleanup

Use engineering judgment.

---

# 37. Definition of Done

A task is complete when:

```text
Feature works
+
Relevant tests pass
+
No obvious regression
+
Existing invariants remain valid
+
No unnecessary architecture was introduced
```

For core networking tasks:

```text
Feature works
+
Unit tests pass
+
E2E passes
+
Message invariants remain valid
+
No silent failure path
```

Do not claim completion merely because:

```text
code compiles
```

---

# 38. Final Self-Check

Before reporting completion, ask:

### Scope

* Did I solve the requested problem?
* Did I implement anything that was not requested?

### Reuse

* Did I reuse existing logic?
* Did I accidentally create duplicate logic?

### Reliability

* Can the message be lost?
* Can it be duplicated?
* Can it remain stuck?
* Does retry behave correctly?
* Does reconnect behave correctly?

### Persistence

* Does restart preserve required state?

### Security

* Did I accidentally bypass E2EE?
* Did I introduce plaintext fallback?

### Testing

* Did I run the relevant tests?
* Did I add a regression test where appropriate?

### Complexity

* Did I make the solution more complicated than necessary?

If the answer to the last question is:

```text
Yes
```

simplify it before finishing.

---

# 39. Most Important Rule

When uncertain, follow this priority:

```text
Current user requirement
>
Current LAN Chat stability
>
Existing correct implementation
>
Core invariants
>
Simple maintainable solution
>
Future extensibility
>
Future roadmap
```

Never sacrifice a working current chat system to prepare for a feature that does not exist yet.

---

# 40. v1.0 Development Principle

The current phase is:

> **Stabilize first. Expand later.**

The AI should behave like an engineer maintaining a small, reliable LAN chat application.

Not like an architect trying to build the final distributed system in advance.

The desired behavior is:

```text
Understand enough
→
Reuse existing code
→
Make the smallest correct change
→
Test it
→
Move on
```

Not:

```text
Analyze everything
→
Redesign everything
→
Abstract everything
→
Implement future features
→
Create more complexity
```

---

# 41. v1.0 Success Criteria

v1.0 is successful when two real LAN devices can reliably:

```text
Discover each other
        ↓
Become friends
        ↓
Chat both directions
        ↓
Send text
        ↓
Send image
        ↓
Send file
        ↓
Receive ACK
        ↓
Read messages
        ↓
Go offline
        ↓
Queue messages
        ↓
Reconnect
        ↓
Automatically resend
        ↓
Avoid duplicates
        ↓
Restart application
        ↓
Continue chatting
```

without major crashes, message loss, duplicate messages, or state corruption.

That is the current definition of **v1.0**：mesh 上的聊天与协作既稳定又可解释。

---

# 42. UI / Design System Rules

Applies to **any change that touches UI** (样式、布局、交互态). Normative source:
**`docs/design-guidelines.md`** (based on Apple HIG / WWDC25 *Shape & Concentricity*).

**Default rule:** new features follow that document unless the task explicitly specifies otherwise.
Deviating requires stating the reason in the change description.

Hard rules (violations are review blockers):

1. **No literal radii.** Use `--gosslan-radius-*` tokens from `src/style.css`
   (`rounded-[var(--gosslan-radius-md)]`), never `rounded-lg` or `border-radius: 8px`.
2. **Concentric nesting:** inner radius = outer radius − padding, and inner must be **smaller**
   than outer. Never leave inner corners pinched or flared.
3. **The system owns the window shape.** Never put a radius on the window root container or on
   the title-bar window buttons — the OS (DWM / macOS) rounds the window, and it does **not**
   round when maximized. An app-drawn radius leaves a gap of `body` background at the corner.
4. **No hardcoded state colors.** hover / press / danger / warning go through semantic tokens
   (`--gosslan-hover`, `--gosslan-danger`, `--gosslan-danger-soft`, …), defined for both
   light and dark. Never `#e81123`, `hover:bg-red-500/10`, `hover:bg-amber-500/10`, etc.
5. **Every interactive element needs a press state** (a global rule already covers
   `button` / `[role=button]`; do not override it away).
6. **If you change layout dimensions or tokens**, also sync the two places that duplicate them:
   `index.html` (boot skeleton, which cannot use CSS variables) and
   `utils/previewMetrics.ts` / `utils/messageHeight.ts` (virtual-list height estimation).

