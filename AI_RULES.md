# Gosslan AI 工程宪法

> **定位**：本文件是所有 AI 编程助手在 Gosslan 项目上的**强制约束**。
> 任何代码修改前必须先阅读本文件，修改后必须按本文件验证。
> 本文件优先级高于用户需求中的隐含假设——如果需求与不变量冲突，AI 必须先指出冲突。

---

## 一、项目本质

Gosslan 不是 CRUD 应用。它是一个**带 P2P 网络、Gossip 传播、离线队列、E2EE、群聊、大文件中继、SQLite 持久化、Tauri 三端（Windows/macOS/Android）、Mesh/Transport 抽象**的复杂分布式系统。

一个看似简单的需求（如"给消息加一个状态"）实际影响链路：

```
Vue UI → Pinia Store → Tauri invoke → Rust Command → Protocol → Transport → Peer → Storage → Event → Vue
```

因此：**稳定性、协议一致性、数据可靠性和回归测试优先于开发速度。**

---

## 二、核心不变量（Invariants）

以下规则是系统的"宪法"，**任何代码修改都不得破坏**。

### INV-001 消息 ID 稳定性

所有消息必须拥有稳定、确定性的 `msg_id`（Gossip 信封的 SHA-256 message_id）。
同一条消息经过直发、Gossip、Relay、Outbox 四条路径时，`msg_id` 必须保持一致。

### INV-002 幂等去重

任何节点收到重复 `msg_id`：
- 不得重复插入数据库
- 不得重复显示 UI
- 必须能够安全回 Ack

### INV-003 Outbox 保障

任何需要可靠投递的消息必须遵循：

```
发送 → 先写入 outbox → 尝试传输 → 等待 Ack → 收到 Ack → 才从 outbox 删除
```

**禁止**：`send()` 返回 Ok 就删除 outbox。TCP send 成功 ≠ 对方已收到（半开连接教训，v0.7.0）。

### INV-004 Ack 语义

Ack 表示：**对端已确认收到消息并持久化**。

不得把以下情况当作"送达"：
- TCP 写成功
- socket connected
- `send()` 返回 Ok
- try_send 成功

### INV-005 加密消息不可静默丢弃

- 解密失败的消息**不得静默丢弃**
- 必须写入系统消息提示（用户可见）
- 不得为了兼容 UI 而绕过加密
- 不得在多个地方实现不同的加密/解密逻辑

### INV-006 UI 乐观状态可回滚

UI 可以乐观更新，但：
- 成功 → 保持
- 失败 → 必须明确回滚或进入 `failed` 状态 + toast 提示

**禁止**：`catch(error) {}` 吞掉错误。

### INV-007 网络失败 ≠ 消息失败

网络暂时不可达时，消息应留在 outbox 等待补发，而非标记为永久失败。
只有明确不可恢复的错误（如对方公钥永远无法获取）才标记 failed。

### INV-008 设备指纹持久性

设备重启后 `device_id` 不得变化（私钥持久化本地 SQLite）。
指纹变化 = 身份丢失 = 所有好友关系断裂。

---

## 三、禁止事项

### 开发行为禁止

| # | 禁止 | 原因 |
|---|---|---|
| P-01 | 未阅读相关模块就修改代码 | 数据流跨 12 层，盲改必出 bug |
| P-02 | 为修 bug 复制一套逻辑 | 双份逻辑 = 未来不一致 |
| P-03 | 绕过 `protocol.rs` 自定义消息格式 | 协议是公共契约 |
| P-04 | 绕过 outbox 直接发送 | 违反 INV-003 |
| P-05 | 绕过 msg_id 生成逻辑 | 违反 INV-001/002 |
| P-06 | 绕过 Ack 确认机制 | 违反 INV-004 |
| P-07 | UI 自己模拟网络状态 | 前端不得臆造在线/送达 |
| P-08 | Rust 和 TS 定义不一致的数据结构 | `types.ts` 必须与 serde 对齐 |
| P-09 | 修改 DB schema 却不写 migration | 用户已有数据不可丢 |
| P-10 | 修改协议却不考虑版本兼容 | 旧客户端不能 parse fail |
| P-11 | 修改核心逻辑却不增加回归测试 | 过去每个 bug 都该变成测试 |
| P-12 | 为了让测试通过而修改测试预期 | 除非需求确实变了 |
| P-13 | 删除失败处理逻辑来"解决"失败 | 掩盖问题 |
| P-14 | catch error 后静默忽略 | 违反 INV-006 |
| P-15 | 引入新的全局状态而不评估影响 | AppState 已经够复杂 |
| P-16 | 新增第三方库前不评估 | 必须说明必要性、体积、跨平台 |
| P-17 | 大规模重构和功能开发同时进行 | 一次只做一件事 |
| P-18 | 引入 Web Worker 处理消息管线 | WKWebView 生产构建下 Worker 可能加载失败（v0.5.1 教训） |
| P-19 | 在业务代码中 if/else 分发传输类型 | 必须走 TransportManager 抽象 |
| P-20 | 顺手重命名/格式化/升级依赖/改 UI 风格 | 只修改必要文件 |

### 密码学禁止

| # | 禁止 | 原因 |
|---|---|---|
| C-01 | 自行实现加密算法 | 必须用 `crypto.rs` 封装的原语 |
| C-02 | 修改密钥派生逻辑而不升级协议版本 | 会导致旧消息不可解密 |
| C-03 | 在非加密路径处理密文 | `enc1:` 前缀是唯一判断标准 |
| C-04 | 将私钥暴露到前端 | 私钥只存 Rust 侧 SQLite |

---

## 四、开发工作流（强制）

每次开发任务必须按以下阶段执行，**不得跳过**：

### Phase 1：理解

阅读：
- 相关 Rust / Vue / TypeScript 代码
- 相关测试
- `protocol.rs` 中涉及的消息类型
- `db.rs` / `schema.sql` 中涉及的表
- CHANGELOG 中相关历史
- `docs/adr/` 中相关架构决策

**不要立即修改。**

### Phase 2：影响分析

输出（可以是内部思考，但必须覆盖）：

```
Affected modules:       [列出涉及的 src-tauri/src/ 和 src/ 文件]
Affected protocol:      [是否新增/修改 Message 枚举]
Affected database:      [是否修改 schema]
Affected UI:            [涉及哪些组件]
Affected platforms:     [Windows / macOS / Android 是否都受影响]
Affected state machines:[消息/连接/文件/好友/E2EE 哪个状态机]
Regression risks:       [至少 3 个最可能的回归问题]
```

### Phase 3：方案

给出**最小修改方案**。如果有多个方案，说明为什么选择当前方案。

### Phase 4：测试设计

先设计测试，再编码。至少覆盖：
- 正常场景
- 失败场景（网络断开、对方离线）
- 边界场景（重复消息、空内容）
- 跨平台影响

### Phase 5：编码

只修改必要文件。禁止顺手做无关变更。

### Phase 6：验证

至少执行：

```bash
npm test                          # 前端纯函数测试
npm run build                     # TypeScript 类型检查 + Vite 构建
cd src-tauri && cargo test --lib  # Rust 单测
cd src-tauri && cargo check       # Rust 编译检查
```

如果修改协议/网络，尽可能运行：

```bash
cd src-tauri && cargo build --example e2e_peer  # 协议级 E2E
bash scripts/e2e-dev.sh                          # 单机双实例全功能验证
```

如果修改涉及 Android：

```bash
cargo check --target aarch64-linux-android
```

### Phase 7：自检清单

完成后逐条检查：

- [ ] 是否破坏 msg_id 一致性？（INV-001）
- [ ] 是否破坏幂等去重？（INV-002）
- [ ] 是否破坏 outbox 保障？（INV-003）
- [ ] 是否破坏 Ack 语义？（INV-004）
- [ ] 是否可能静默丢弃加密消息？（INV-005）
- [ ] 是否吞掉了错误？（INV-006）
- [ ] 是否破坏旧数据兼容？
- [ ] 是否影响 Android 编译？
- [ ] 是否产生新的 Rust warning？
- [ ] 是否需要更新 `CHANGELOG.md`？
- [ ] 是否需要新增/更新 ADR？
- [ ] Rust serde 与 `src/types.ts` 是否对齐？

---

## 五、协议变更规则

任何对 `protocol.rs` Message 枚举的修改（新增/修改/删除变体）必须回答以下 7 个问题：

```
1. 谁发送？（sender）
2. 谁接收？（receiver）
3. 是否允许重复？（idempotency）
4. 是否需要 Ack？（ack）
5. 是否进入 outbox？（persistence）
6. 是否允许 Gossip 传播？（gossip）
7. 是否需要加密？（encryption）
```

示例（ChatMessage）：

```
sender:      Sender → Receiver
idempotency: 允许重复到达，msg_id 去重
ack:         必须
outbox:      必须（Ack 才删）
gossip:      允许
encryption:  必须（E2EE 恒开）
```

协议变更还必须检查：
- Rust enum/struct 定义
- serde 序列化/反序列化
- TypeScript `src/types.ts` 对应类型
- 发送方和接收方处理逻辑
- 旧版本兼容性（未知消息类型应 ignore 而非 panic）

---

## 六、状态机规范

### 消息状态

```
sending → delivered → read
   ↘ failed
```

### 连接状态

```
unknown → discovered → connecting → connected → heartbeat → offline
```

### 文件传输状态

```
created → offered → accepted → transferring → completed
                                      ↘ failed
```

### 好友关系状态

```
unknown → request_sent → accepted → friend
```

### E2EE 状态

```
unknown_key → discovering → key_available → encrypted → decrypted
                                            ↘ decrypt_failed (写系统消息)
```

**要求**：不要用多个独立 boolean 表达状态。优先使用明确的枚举/联合类型。
**禁止**：跨状态直接操作（如 peer_exists 就 send_message，必须经过完整状态检查链）。

---

## 七、Bug 修复规范

所有 bug 修复必须记录（commit message 或 CHANGELOG）：

```
Bug:          [现象描述]
复现步骤:      [如何触发]
Root Cause:   [根因，不是表象]
影响范围:      [涉及哪些模块/平台]
Fix:          [修改内容]
Regression:   [新增了什么测试来防止回归]
```

**禁止只修表象**。例如：

- 错误：消息不显示 → 强制 refresh UI
- 正确：消息不显示 → 检查事件 → 检查 store → 检查 merge → 检查 DB → 找到真实断点

每个 bug 修复后必须增加至少一个回归测试。优先级：

```
Rust unit test > protocol test > E2E > 真实双设备
```

如果无法自动测试，必须在 commit message 中说明原因。

---

## 八、架构边界

### Source of Truth（事实来源优先级）

```
代码 > 测试 > protocol.rs > db.rs/schema.sql > ADR > AI_PROJECT_HANDOFF > README > CHANGELOG
```

如果文档与代码冲突：**先读代码和测试，再判断文档是否过期**。

### Transport 与业务解耦

```
Application → MessageService → TransportManager → LAN / Bluetooth / QUIC / Relay
```

**禁止**在业务代码中出现：

```rust
if bluetooth { ... }
else if lan { ... }
else if relay { ... }
```

### 前端 Store 边界

当前 `useChatStore.ts` 承担了过多职责。后续新功能应优先进入对应领域 Store：

```
stores/
├── useAppStore.ts          # 设备/主题/响应式
├── useChatStore.ts         # 核心聊天（不再膨胀）
├── [未来] useConversationStore.ts
├── [未来] useMessageStore.ts
├── [未来] useFriendStore.ts
├── [未来] useTransferStore.ts
└── [未来] useNotificationStore.ts
```

**不要现在一次性重构**，但新功能不要继续无脑塞进 useChatStore。

### Rust/TypeScript 数据对齐

Rust serde model 与 `src/types.ts` 必须保持一致。修改任一侧时，必须同步检查另一侧。

---

## 九、数据库规则

SQLite 是事实来源之一。任何 schema 修改必须考虑：

1. 旧数据库升级路径（ALTER TABLE / 新列默认值）
2. 已有用户数据不丢失
3. 重复执行安全（IF NOT EXISTS）
4. 事务完整性
5. `schema.sql` 与 `db.rs` SCHEMA 常量同步更新

---

## 十、平台兼容

Gosslan 支持 Windows / macOS / Android。任何修改必须考虑三端。

- 平台相关代码使用 `#[cfg(desktop)]` / `#[cfg(mobile)]`
- 禁止让 Android 引入仅桌面依赖（如 machine-uid、tray-icon）
- 前端代码不得使用桌面专属 API 而不做条件判断

---

## 十一、第三方依赖

新增依赖前必须说明：

```
为什么需要:
是否可以用已有能力替代:
包大小影响:
跨平台支持（Windows/macOS/Android）:
Tauri v2 兼容性:
维护状态（最近更新时间）:
是否增加构建复杂度:
```

**能不用就不用。**

---

## 十二、文档同步

每次代码变更后检查是否需要更新：

| 变更类型 | 需更新文档 |
|---|---|
| 新增功能 | CHANGELOG `[Unreleased]` + README 功能表 |
| 协议变更 | CHANGELOG + `docs/protocol-design.md` + 新 ADR |
| Schema 变更 | CHANGELOG + `schema.sql` |
| 架构决策 | 新 `docs/adr/XXXX-title.md` |
| Bug 修复 | CHANGELOG `[Unreleased]` |
| 破坏性变更 | CHANGELOG（标注 ⚠️）+ README + HANDOFF |

---

## 十三、最终原则

> 不确定时，不猜。

> 不理解现有状态机时，不改代码。

> 不知道数据从哪里来时，不新增状态。

> 不知道为什么这么设计时，先找 ADR / CHANGELOG / 测试。

> 修 Bug 时修根因，不修表象。

> 新功能不能破坏已有协议和不变量。

> 网络系统优先保证可靠性，再追求性能。

> 稳定性优先于代码数量。

> 宁可少做一个功能，也不要制造新的隐性状态。

> 每一个过去修过的 Bug，都应该变成未来不会再次发生的测试。

---

## AI 的最终工作目标

不是：

> "把用户这一次需求做出来。"

而是：

> "在不破坏现有系统不变量的前提下，以最小改动实现需求，并让这个 bug / 问题以后不再回来。"

---

## 附录：相关文件索引

| 文件 | 用途 |
|---|---|
| `AI_RULES.md`（本文件） | AI 开发宪法：什么能做、什么不能做 |
| `AI_PROJECT_HANDOFF.md` | 项目全貌：是什么、现在有什么、代码导读 |
| `CHANGELOG.md` | 版本历史：过去为什么改 |
| `docs/adr/` | 架构决策记录：为什么这么设计 |
| `docs/protocol-design.md` | 协议对标与演进路线 |
| `docs/performance.md` | 大规模节点性能设计 |
| `docs/setup-windows.md` | Windows/Android 环境配置 |
