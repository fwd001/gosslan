# Gosslan Protocol & Reliability Invariants

> Version: 1.0
>
> 本文件定义 Gosslan 网络、消息、ACK、Outbox、Gossip、E2EE、身份和持久化必须长期保持的系统不变量。
>
> 修改协议或网络核心代码时必须阅读。

---

## 1. Message Identity

### INV-P01 — Stable Message ID

可靠消息必须具有稳定的业务 `msg_id`。

同一消息经过：

```text
Direct
Gossip
Relay
Retry
Outbox
Reconnect
```

不得因为传输路径变化而变成不同业务消息。

### 验证

```text
A → B direct
A → Gossip → B
A → outbox → B
```

最终 B 必须只产生一个业务消息。

---

## 2. Idempotency

### INV-P02 — Duplicate Delivery Is Safe

同一 `msg_id` 重复到达：

```text
DB: 不重复
UI: 不重复
Notification: 不重复
Read state: 不回退
ACK: 可重复发送
```

### 推荐测试

```text
receive(msg)
receive(msg)
receive(msg)

assert database_count(msg_id) == 1
assert ui_count(msg_id) == 1
```

---

## 3. ACK

### INV-P03 — ACK Means Durable Receive

ACK 只有在接收方完成项目定义的“接收确认”后才能发送。

不能把：

```text
socket write
```

等价为：

```text
message delivered
```

### 发送方

```text
queued
→ sending
→ waiting_ack
→ delivered
```

### 无 ACK

```text
waiting_ack
→ reconnect / retry
```

而不是立即：

```text
failed
```

---

## 4. Outbox

### INV-P04 — Outbox Is Reliability Boundary

发送可靠消息：

```text
BEGIN
  insert message
  insert outbox
COMMIT
```

然后才允许：

```text
network attempt
```

ACK 到达：

```text
delete outbox
```

### 绝对禁止

```text
send() == Ok
→ delete outbox
```

---

## 5. Crash Safety

### INV-P05 — Process Crash Must Not Lose Queued Message

以下场景：

```text
write outbox
process crash
restart
```

重启后必须仍能发现：

```text
pending outbox
```

并尝试补发。

---

## 6. Retry

### INV-P06 — Retry Must Be Idempotent

重试不得生成新的业务消息 ID。

```text
retry(msg_id)
```

不是：

```text
create_new_message()
```

### 界面上的「重发」是**已知例外**（2026-09-25 记）

按 `msg_id` 重投那套实现（`resend_message`）已经删除 —— 前端从未调用过它，
界面上的「重发」实际是 `chat.send` **重发一条新消息**（新 `msg_id`）。
所以今天这条不变量在 UI 路径上并不成立，把它写清楚比留一个隐形接口诚实。

如果将来真要做「按原 id 重投」，那两条用真事故换来的规则必须连实现一起回来
（原本是 `lib.rs` 里的两条源码护栏，跟着被删的命令一起走了）：

1. **先密封再入队**：`messages` 存的是**明文**，直接把库内 content 上线就没有 `enc1:` 前缀 ⇒
   接收端 `open_direct_content` 拒收（不落库、不 Ack）⇒ 重发静默变 no-op，
   稍后又被清扫器判 failed（用户看到「点重发没反应」）。`ts`/`seq` 同理必须沿用原记录：
   `seq: 0` 会让接收端把重发消息排到会话最前（排序按 seq，INV-P09），两端顺序分裂。
   —— 两侧都能「正常加密」，普通单测测不出来。
2. **所有可失败步骤通过之后才置 `sending`**：旧顺序先 `set_message_status("sending")`
   再判群聊 / 取公钥 / 密钥交换 / 加密，失败路径不回滚 ⇒ 状态永久卡 `sending`，
   而重发入口的守卫（`"sending" | "sent" => Err`）又把它挡死 ⇒ 这条消息**永远发不出去**，
   outbox 也从未写入。触发条件是「对任何失败的群消息点重发」，必现。

---

## 7. Gossip

### INV-P07 — Gossip Must Converge

Gossip 消息允许多路径到达。

必须：

```text
TTL bounded
dedupe bounded
fanout bounded
```

禁止无限传播。

---

## 8. Gossip + Direct

### INV-P08 — Direct and Gossip Are Same Logical Message

如果：

```text
Direct(ChatMessage)
```

和：

```text
Gossip(ChatMessage)
```

同时到达：

```text
one DB row
one UI message
one logical delivery
```

---

## 9. Ordering

### INV-P09 — Transport Ordering Is Not Application Ordering

不能假设：

```text
TCP order == global message order
```

因为 Gosslan 同时存在：

```text
Direct
Gossip
Outbox
Relay
```

最终排序必须使用项目定义的消息时间/序列/稳定排序策略。

不得用“最后收到的”作为业务真相。

---

## 10. E2EE

### INV-P10 — Ciphertext Must Not Become Plaintext Silently

如果：

```text
ciphertext
+
missing key
```

必须：

```text
observable failure
```

不得：

```text
decrypt failed
→ show plaintext
```

也不得：

```text
decrypt failed
→ silently drop
```

---

## 11. Key Changes

### INV-P11 — Public Key Change Is Security-Relevant

如果发现：

```text
device_id 相同
public_key 不同
```

不能静默覆盖后继续发送。

必须明确决定：

```text
accept
reject
re-key
warn
```

当前实现若尚未具备完整 key rotation，应记录为已知限制，而不是让 AI 自行决定。

### 现在的决定（2026-09-13 实装，用户真机反馈后）

| 绑定来源 | 公钥不一致时 | 理由 |
|---|---|---|
| **好友表**（`friends.ed25519_pubkey`，用户显式建立的关系） | **硬拒** Hello + 在聊天里插一条系统提示 | 这是唯一一条"用户亲自建立过"的信任；静默换钥等于把中间人攻击变成默认行为 |
| **内存 `peers` 表**（广播里学来的公钥，**未经验签**） | **同样硬拒**，但删好友 / 收到 `FriendRemove` 时会**解除绑定**（`forget_peer_identity`） | 广播公钥本来就没有任何可信度，它不该变成"永久枷锁" |

**用户可行动路径**（提示里必须写出来，否则等于把用户扔在原地）：
`删掉该好友 → 重新添加`（聊天记录保留、**不需要重启**）。

### 反过来的一面：这把锚点**是被谁写进去的**（2026-09-22 实装）

上表管"绑好之后不一致怎么办"，这条管"绑下去的那一刻可不可信"。`update_friend_pubkeys`
只填空、首写者永久胜出 ⇒ **写入侧的信任等级决定锚点本身**，而这此前是不对称的：
`upsert_peer` 要求 `peers.keys_verified`（只有验签通过的 Hello 能打上），而三条"成为好友"
的路径（直连 `FriendAccept` / 跨跳 Gossip `FriendAccept` / 本机点同意）读**同一张 `peers` 表**
却不过这道闸 ⇒ 一次伪造的 UDP announce 抢先把 `friends.ed25519_pubkey` 填上，就是永久的。

- 现在的规则（判据只有一份：`peer_keys_trusted` + `acceptable_friend_keys`）：
  **`x25519`（加密钥匙）照旧早绑** —— 晚了就是"首次加密发送失败"，而 Gossip 那处补齐以
  "这一封能解密"为持有证明；**`ed25519`（身份锚点）只认被证明过的来源**。
- 为什么要收紧写入侧：锚点此后有三个消费者 —— Hello 验签（INV-P21）、安全码、
  **公网中继的准入判据**（`list_bound_friend_identities` 只看它非空，ADR-0020 称那把钥匙
  为"整个设计的支点"）。绑错的后果从"消息被加密给攻击者"扩到"我们主动跨公网给对方建电路"。
- 留 NULL 不是死路，但**自愈的机制只有一处**：验签通过的 Hello 最终都会落进
  `handle_message` 的 Hello 分支，那里在 `upsert_peer` **之后**调 `mark_peer_keys_verified`
  （TCP 入站 / 出站拨号 / BLE 三条 transport 共用这一个写入点）。
  ⚠️ 别把这句话理解成"`upsert_peer` 会自己打标"——它不会，它新建条目时**恒标
  `keys_verified: false`**（`announce` 与 Hello 共用同一个函数，必须保守）；握手处那三次打标
  也只覆盖"`peers` 条目已经由 announce 建好"的情形，条目还不存在时那次调用是空操作。
  2026-09-22 补上 Hello 分支这一处之前，出站拨号与蓝牙**第一次**连上的好友整个会话都绑不上
  锚点（安全码算不出、中继永不准入，且没有任何报错）—— 收紧来源必须同时把打标点补齐，
  否则就是把安全改动做成可用性回退。次序也是判据的一部分：打标在 `upsert_peer` 之前 = 空操作。
- 守卫：`friend_identity_anchor_has_one_binding_rule`（三处 accept 都走同一 helper、
  `upsert_peer` 那道闸不许拆、Gossip 那处只准绑 x25519、**打标点的数量与"在 `upsert_peer`
  之后"这条次序**）+
  `accept_binds_encryption_key_but_defers_unverified_anchor`（两列的差别与 NULL 自愈）。
  两条非空转用例登记在 `scripts/verify-guards.py`：删掉打标点、以及把打标挪到 `upsert_peer`
  之前（换序不改计数，只有次序断言会响）。

### 为什么原来"必须重启"（真机 2026-09-13，已修）

对方重装应用 → 换了密钥 → 用户按提示删好友 → `friends` 表那一行确实没了，
但 `verify_hello` 的绑定还有**第二条腿**（内存 `peers` 表里那条广播学来的旧公钥），
于是 Hello 继续被硬拒 ⇒ 消息与好友请求都进不来；**只有重启**（内存清空）才回落到 TOFU。
修法：**解除关系就解除身份绑定**（`network::transport::forget_peer_identity`，
在 `remove_friend` 与 `FriendRemove` 两条路径上都调；只清身份，不动链路）。

### 待办（更好的方案，尚未实装）

1. **非好友**的内存绑定降级为"提示"：Hello 是自签名握手、比未验签的广播强，
   非好友时可以按验签结果**替换**并留痕（好友仍硬拒）。
2. **重新配对对话框**：把旧/新指纹并排给用户看（`3F2A… → 91C4…`），
   一键"信任新身份"（等价于现在的"删好友再添加"，但少两步且不丢关系元数据）。

---

## 12. Identity

### INV-P12 — Device Identity Persistence

重启不能随机产生新身份。

身份数据：

```text
device_id
identity private key
identity public key
```

必须保持一致，除非用户显式重置身份。

---

## 13. Protocol Version

### INV-P13 — Protocol Evolution Must Be Explicit

协议发生不兼容变化时必须：

```text
version bump
+
compatibility decision
+
ADR
```

禁止：

```text
旧客户端收到新消息
→ panic
→ crash
→ silently corrupt state
```

未知扩展应在协议允许的情况下安全忽略。

---

## 14. Persistence

### INV-P14 — DB Is Not Cache

消息、好友、身份、outbox 等持久化数据不能只存在：

```text
Pinia
memory
peer map
```

如果数据定义为 durable：

```text
SQLite must be source of truth
```

---

## 15. Event / UI

### INV-P15 — Event Is Notification, Not Durable Storage

Tauri event：

```text
Rust → Vue
```

是通知机制，不是数据库。

如果事件丢失：

```text
UI should be able to rehydrate from DB / command
```

不能因为一次 event 丢失而导致永久状态丢失。

---

## 16. Error Handling

### INV-P16 — Errors Must Have Semantics

错误至少区分：

```text
recoverable
retryable
permanent
user_action_required
security_related
```

禁止统一：

```text
Err → failed
```

---

## 17. File Transfer

### INV-P17 — File Chunks Must Be Verifiable

文件分片必须能够：

```text
identify
order
dedupe
validate
reassemble
```

不能仅依赖：

```text
arrival order
```

**续推论（2026-09-22 跨网真机：160MB 永远停在 0%）**：`order` 这条对**续传**同样成立，而且
它有两个方向，只做一半必然出错 ——

1. **接收端是"我有什么"的唯一权威，每一次 `FileOffer` 都必须把真实位置答出去。**
   发送端的重试一律从 `from_bytes = 0` 起步（`flush_pending_files` 走的是不带位置的
   `send_file_from_path`），它只能靠 `FileReject.received` 知道该从哪儿续。
   判据只许一份：`file::decide_offer`（`has_active` 时用内存 `received`，否则用 `.part` 长度）。
   历史上这里是两套规矩 —— 没有活跃接收器才比对前缀、有活跃接收器一律裸 `Accept` ⇒
   接收端把已收前缀当"迟到的重复片"静默丢掉，发送端**每一轮都重传一遍已收部分**，
   慢链路上永不收敛，而且全程不报错。
2. **段号必须与发送端的"按段从 `seq = 0` 重编"对齐，但只在续传段归零。**
   归零少了 ⇒ 新数据被判重复片丢掉（差一截、不报错）；归零无条件做 ⇒ 上一轮 attempt
   还在排空的分片（队列 1024 槽 ≈ 262MB，丢 future 不排空队列）会被判成「跳号」
   ⇒ `Err(文件分片顺序错误)` ⇒ 整单死。两个方向都有单测钉住（`decide_offer` 三条用例 +
   `verify-guards.py` 两条变异用例）。

---

## 18. Security Boundary

### INV-P18 — Private Key Never Crosses UI Boundary

禁止：

```text
Rust private key
→ Tauri invoke result
→ JS
```

前端只能获得完成 UI 所需的公开信息或状态。

---

## 19. Logical Ordering

### INV-P19 — Per-Conversation Logical Sequence

同一会话内的消息排序使用本地维护的逻辑序号 `seq`，不使用墙上时钟，也不猜测对端时钟。

- 发送：`seq = local_clock + 1`，并持久化 `conversation_clocks`。
- 接收：`seq = max(1, received_seq)`，并推进本地时钟。
- 排序：`seq ASC, id ASC`。
- 群聊清空边界：记录清空时的 `seq`，`seq <= boundary` 的旧消息不再回灌。

禁止：

```text
sender_wall_clock 直接决定消息顺序
receiver 纠正/猜测 sender 时钟
```

---

## 20. Transport Priority

### INV-P20 — Chat Must Not Be Starved By Bulk Transfers

每条 TCP 连接同时维护：

```text
priority 通道：聊天 / Gossip / Ack / 回执 / 好友群控制 / 心跳 / Hello / 文件握手
bulk 通道：FileChunk / GroupFileChunk / RelayChunk / FileDone / GroupFileDone
```

文件终止帧必须和分片同属 bulk 通道，保证 `Offer → Chunk... → Done` 的协议顺序不被破坏。

禁止：

```text
大文件分片占满唯一队列，导致聊天消息长时间排队
```

---

## 21. Handshake Identity

### INV-P21 — Peer Identity Authenticated Before Link

TCP 链路建立**之前**，必须先用密码学手段确认对端确实是 `peer_id` 本人。

`Hello` 必须携带：

```text
nonce  : 每次握手新生成的随机串（防重放）
sig    : Ed25519 签名，覆盖 device_id | tcp_port | nonce | x25519_pubkey | ed25519_pubkey
```

接收方校验规则：

```text
friends / peers 中已绑定该 device_id 的 Ed25519 公钥？
    ├── 是 → 自报公钥必须等于绑定公钥，且 sig 必须用该公钥验证通过
    └── 否 → TOFU：sig 必须用 Hello 自报的 ed25519_pubkey 验证通过
校验失败 → 直接丢弃连接，且不得把 peer_id 写入 links / priority_links
```

原因：`handle_message` 中大量**明文控制消息**（`GroupMemberRemoved` / `GroupRename` /
`FriendRemove` / `FriendAccept` / `ReadReceipt` / `Heartbeat` / `UserInfo`）只做
`from == peer_id` 绑定校验。若 `peer_id` 可冒充，这些消息即可被伪造 —— 例如冒用群主
`device_id` 发送 `GroupMemberRemoved` 可让受害者本地删除整个群。

禁止：

```text
未经验签就把 Hello 自报的 device_id 当作链路身份
为「兼容旧版本」保留无签名的 Hello 分支
```

---

## 22. Invariant Exceptions

### INV-P22 — Exceptions Must Be Registered

上面 21 节的不变量默认是**无条件**的。若某条代码路径**必须**偏离它，必须**同时**做两件事：

```text
① 在代码处留标记：  // INV-EXCEPTION: INV-PXX — <为什么必须破例>
② 在本节登记：      不变量 id + 理由 + 引入出处
```

两件事缺一不可，由 `scripts/check-invariant-exceptions.mjs` **双向**校验：
代码标了而本节没登记 ⇒ FAIL；本节登记了而代码没标 ⇒ FAIL。

### 为什么必须有这一节（2026-09-16）

「和自己聊天」（提交 `7c03341`）是一处正当的例外：收发双方都是本机，所以它不进 outbox、
不广播、不做 E2EE。**这些决定都是对的，理由也写得很清楚** —— 落在三个地方：

| 落点 | 性质 |
|---|---|
| `src-tauri/src/commands.rs` 里 `insert_self_message` 的文档注释 | 局部注释 |
| `scripts/verify-guards.py` 的「自聊消息必须留在本地」用例 | 护栏 |
| `src/utils/selfChat.ts` 的文件头 | 局部注释 |

而 AI 的必读清单（`docs/AI_ENGINEERING_INDEX.md`）只指向**本文件**与 `AI_RULES.md` ——
**这两份都没提这个例外**。于是产生一条非常具体的误修路径：

```text
AI 读到 INV-P04「发送可靠消息 → insert message + insert outbox」
        ↓
看到 insert_self_message 只调了 insert_message、没有 outbox
        ↓
按文档判定这是 bug，并按文档「修」它
        ↓
自聊消息进入 outbox ⇒ 永远等不到对端 Ack ⇒ flush_outbox 每次心跳都重发
        ↓
「outbox 必然排空」被真的破掉 —— 而这次回归是「照文档修」造成的
```

**结论：局部注释不能替代规范文档。** 同一条知识，写在实现旁边对读实现的人有用、
对读不变量的人没用；而 AI 读的是不变量。

同理，**例外不是"不变量可以随便破"的借口**：每条例外都必须能回答「为什么没有别的做法」。
本节只登记**确实偏离了**的不变量 —— 例如自聊不广播、不做 E2EE 就**不是**例外，
因为 INV-P07 只要求 TTL/去重/扇出有界、INV-P10 只管"解密失败不得静默退明文"，
两者都没被违反，硬登记进来反而是把例外机制用坏。

<!-- BEGIN EXCEPTION REGISTRY -->

| 不变量 | 例外 | 理由 | 引入 |
|---|---|---|---|
| INV-P03 | 自聊不经过 `queued → sending → waiting_ack → delivered`，落库即终态 `read` | 本机既是发送方也是接收方，不存在"在途"阶段，没有可等待的 ACK | `7c03341`（2026-09-16） |
| INV-P04 | `insert_self_message` 走 `db::insert_message`，**不**写 outbox | outbox 的唯一出队条件是收到对端 Ack。自聊没有收件人、永远等不到 Ack ⇒ 那一行永远留在库里，被每次心跳/建链的 `flush_outbox` 反复重发，反而把「outbox 必然排空」破掉 | `7c03341`（2026-09-16） |

<!-- END EXCEPTION REGISTRY -->

### 自聊的完整边界（一处理由，多处套用）

```text
msg_id     前缀 self-（日志/排障时一眼与网络消息的哈希 id 区分）
conv_id    = sender_id = receiver_id = 本机 device_id
status     落库即 read（见上表 INV-P03 例外）
outbox     不写（见上表 INV-P04 例外）
gossip     不发（非例外：INV-P07 不要求广播；且 sender == 自己 会在 handle_gossip 早退）
E2EE       不做（非例外：INV-P10 管的是"解密失败不得退明文"；自聊没有密文）
内容类型   仅 text / code（用户 2026-09-16 明确「先只支持文本」）
unread_inc 0（自己发的不该让自己有未读）
```

禁止：

```text
新增一条偏离不变量的代码路径而不登记例外
用「这是特例」代替理由 —— 每条例外都要能说出为什么没有别的做法
把没被违反的不变量也登记进来充数（会让例外机制失去意义）
```

---

# 22. Invariant Change Procedure

如果一个新需求必须违反现有 invariant：

AI 不得直接修改。

必须先：

```text
1. 指出冲突
2. 说明为什么当前 invariant 不再成立
3. 给出新 invariant
4. 说明迁移策略
5. 增加回归测试
6. 新增/更新 ADR
7. 再修改实现
```

---

## 23. BLE Fragmentation Budget

### INV-P23 — One Budget, One Place

BLE 上「一片能装多少字节」这件事，三个平台有三种输入：

```text
central 侧       btleplug 协商出的 ATT MTU          → 要减 ATT 头（3）
外设侧 macOS     CoreBluetooth maximumUpdateValueLength
外设侧 Windows   WinRT MaxNotificationSize          → 本身已是载荷，不减
外设侧 Android   等价协商结果
```

**输入语义可以不同，但常量与换算必须只有一份**：唯一定义点在
`transport/ble_framing.rs`，由 `scripts/check-ble-constants.mjs` 守门（判据 A：六个规范名字
各有且仅有一处定义；判据 B：BLE 领域内不许用匿名常量重述受保护字面量）。

约束：

```text
每片载荷 = min(该链路的载荷预算, GATT_MAX_ATTR_LEN = 512)
    central 侧：  载荷预算 = 协商 MTU − ATT_HEADER_LEN(3)
    peripheral 侧：载荷预算 = 对端声明的通知长度（**不再减** ATT 头）
非法/过小输入（装不下 6 字节分片头）→ 退回 DEFAULT_PAYLOAD_BUDGET(20)，**绝不返回 0**

单条消息         ≤ MAX_BLE_MESSAGE_BYTES (512 KiB)
单条消息的分片数 ≤ MAX_BLE_CHUNKS_PER_MESSAGE (8192)
同时进行的未完成消息 ≤ MAX_INFLIGHT_MESSAGES (8)
未完成消息 TTL    = PARTIAL_TTL_MS (30s)
```

**两侧必须能互相推回去**（`both_sides_agree_on_the_same_link_budget`）：

```text
协商 MTU m --att_payload_budget--> 载荷 b --告知对端--> notify_payload_budget(b) == b
```

末尾那个 `== b` 成立**正是因为外设侧不再减 ATT 头**。若有人给外设侧也减一次，
这条立刻红 —— 而真机症状只是「某台设备收不到消息」，没有这条测试极难定位。

### 为什么单列成一条不变量

CHANGELOG `4.18.7 → 4.18.10` **连着四个版本**修同一个分片预算问题：

| 版本 | 标题 |
|---|---|
| 4.18.7 | 分片预算没减 ATT 头 ⇒ 多分片帧写不出去（好友申请永远发不出） |
| 4.18.8 | 写入失败日志补上帧长（**上一版修复生效但不够**） |
| 4.18.9 | 每片 514 字节 > AOSP 硬上限 512 ⇒ 多分片帧永远发不出去 |
| 4.18.10 | 外设启动失败的原因被丢掉 |

根因不是某一行写错，而是**同一个概念在多个地方各算一遍**，且**数值恰好一致所以不报错**。
2026-09-16 收敛之前，macOS 外设侧自己留着 `const DEFAULT = 20` / `const MAX = 512`
（匿名、靠注释解释语义），Windows 侧走共享函数 —— 于是 `transport/bluetooth.rs` 里那句
「**外设侧用的是同一个函数**」只对 Windows 成立。

禁止：

```text
在 ble_framing.rs 之外重新定义 GATT_MAX_ATTR_LEN / ATT_HEADER_LEN /
    BLE_DEFAULT_MTU / DEFAULT_PAYLOAD_BUDGET
在 ble_framing.rs 之外重新实现 att_payload_budget / notify_payload_budget
给外设侧再减一次 ATT 头（会与 central 侧推出两个不同的数）
让任一换算返回 0
```

---

## 24. Cross-Version Graceful Degradation

### INV-P24 — 版本不同必须降级，不许炸也不许装

Gosslan 是**没有服务器、没有强制升级通道**的 mesh：网里同时存在多个版本是常态。
一旦"新旧共存"这件事没被当成约束，最坏结果不是"某个功能不可用"，而是
**老设备跟新设备彻底连不上**（今天就是这样，见下面"现状"）。

四条硬判据：

```text
1. 未知帧不得成为连接错误            —— 收到不认识 type 的帧 ⇒ 忽略 + 节流记日志，
                                        绝不 panic、绝不拆链、绝不因此丢 ACK 语义
2. 未知内容不得成为裸 JSON            —— 渲染层必须有兜底：识别不了的 kind/载荷
                                        ⇒「不支持的消息类型，升级后可查看」+ 可展开原文
3. 新字段必须可缺省                  —— 加字段 = 老对端不发它 ⇒ 读侧必须有默认值。
                                        把字段改成必填 = breaking，必须走 ADR-0007
4. 新能力必须先协商再用              —— 发送方只在对端声明支持（protocol_version /
                                        capability）时才允许发新帧或新语义；
                                        门控失败 ⇒ 退回旧行为，而不是"发了再说"
```

**灰度顺序（不可颠倒）**：

```text
第一步：全网铺开"能识别 Unknown + 会报告自己版本"的版本
   ↓（这一步没有它，后面所有新帧都会拆老链路）
第二步：才允许引入新帧 / 新语义
   ↓
第三步：某个旧版本线确认没人用了，才删它的 legacy 分支（删也要写进 ADR）
```

对端版本**高于**本机时，用户看到的必须是可解释的状态（会话/联系人详情一条说明 +
隐藏诊断面板里的对端版本），不是错误弹窗、不是静默失败、不是转圈。
对端版本**低于**本机最低支持版本时：拒绝建链并给出"对方需要升级"的具体对象是谁 ——
不共享密钥、不半连不猜。

**为什么这条曾经是硬规则而不是洁癖**：`Message` 是
`#[serde(tag = "type")]` 且原本**没有**兜底变体；`read_frame` 把反序列化失败
转成 `io::Error(InvalidData)`，reader 循环按连接错误处理 ⇒ 拆链 ⇒ 重连后同一帧再拆，
形成无限循环。也就是说：在 v4.22.27 之前上线任何"新帧类型"，老设备与它**连不上**，
而不是"看不懂这一条"。`ADR-0007` 记录的正是把它从 Proposed 落到实处的决定。

**落地进度（别重复造，也别拿旧事实当现状）**：

```text
第 1 条 未知帧降级        ✅ v4.22.27  decode_frame ⇒ Message::Unknown，链路保持
                            守卫 unknown_wire_frame_is_tolerated_after_auth
第 4 条 的前提：会报版本   ✅ v4.22.28  Hello 带可选 protocol_version / app_version
                            （不进签名材料）；写入点只有一个 = 验签后的 Hello，
                            TCP 与 BLE 共用；节点离线时与 peer_content_features 同点回收
第 2 条 未知内容不显示裸 JSON ✅ v4.22.33  判据统一走 `is_known_kind` / `isKnownKind`
                            （只能查 `WIRE_KINDS`，不许维护第二份清单；`kindClass` 对未知值
                            回落 Bubble，用它判"认识"会永远判成认识）
                            覆盖：会话列表与通知文案（Rust `preview_text` + 前端 `previewText`，
                            两侧文案由 messageKinds.test.ts 机器比对）、时间线气泡
                            （`UnsupportedKindBubble`：可解释说明 + 主动展开才给原文）、
                            引用片段、搜索命中行、收藏列表、虚拟列表高度估算。
                            ✅ 另一半 v4.22.34：`ChatMessage.kind` 从 `MsgKind` 改回 `String`。
                            在此之前**单聊根本走不到兜底** —— 未知 kind 会让整帧解析失败并被
                            Unknown 降级丢弃（消息连库都进不了，双方都看不见「看不懂」）。
                            现在单聊与群聊两条路径都会落到占位气泡上。
第 4 条 门控本身           ✅ v4.22.39  机制 = Hello 的 capability 位（**不是** protocol_version：
                            V1 期间新增的 kind 已经证明"V1 内部并不单调"，版本号当能力清单用会骗人）
                              · 唯一判据 `protocol::kind_allowed_by_features` + 唯一映射表
                                `kind_required_feature`（源守卫各判"只许一处"）
                              · 发送侧接线在 `commands/chat.rs::send_message`，且刻意排在
                                公钥探测之前（不该为一个根本不会发的东西白探测 1.2s）
                              · 未知即不支持：从没交换过 Hello / 已离线 ⇒ 位图按 0 处理（**决策**方向）
                                但"不知道"与"它自己声明过缺位"分两句说：入口 `kind_blocked_hint`
                                收 `Option<u32>`（缺条目=未知）。两张表都是内存态、重启即空、
                                离线即被 sweep ⇒ 缺条目通常只代表对方此刻不在线，一律说成
                                "版本较旧"是对用户作假指控（他会去催升级，而要做的只是等上线）
                            第一个被门控的东西不是假想的：`kind:"merge"` 是 V1 期间（9b26006）
                            才加的，v4.8.2 / v4.18.10 / v4.20.0 的 `MsgKind` 里没有它 ⇒
                            那些实例收到会**整帧丢掉**（第 2 条只修了我们这一侧），
                            发送方只会看到"发送失败"。
                            ⬜ 残留（见待办）：群聊发送口**刻意不门控**（成员能力不齐、且 gossip 是
                            fire-and-forget）—— 老成员那边依旧只退化成原始文本，v4.24.0 起发送方
                            会收到一条受众预告（只提示、不拦），所以不再是"静默看不见"；
                            离线对端"未知即不支持"的代价是发不出去而不是延后判断 ——
                            要变成 outbox flush 期降级
                            （那时才第一次真的知道对方能力）需要载荷可改写，是另一件事。
```

用户可见状态也已就位（v4.22.35）：判定只在 `protocol::peer_protocol_is_newer` 一处，
结论随好友记录下发（`Friend::peer_version_newer`）—— 单聊头部一个不抢戏的标记、
联系人详情一整句"对方的 Gosslan 比本机新，部分新类型消息需升级本机后才能查看"。
`None`（老实例没报版本）一律判"不高"：既不猜成 0 也不猜成 1，所以不会满屏误报。

诊断入口同样在：隐藏诊断面板「版本互通」格显示本机协议/应用版本 + 每个已知对端的**声明**
（老端如实显示"未声明"，不替它猜版本号）；`忽略未知帧类型` 那条日志同样带上对端声明，
"谁版本高"从此有据可查而不是靠倒推。

同一条诉求在**本地数据**那一侧（用户原话里的"数据库版本升级"）已于 v4.22.36 闭合：
降级仍然拒绝启动（红线不变），但拒绝变成了一句可读、可行动的原生提示，且判定挪到了
任何写操作之前。规则本文归 `AI_RULES.md` §13，这里只记进度，不重复写第二份。

不适用（别过度实现）：只加字段、不改语义、且老代码会忽略未知字段的变更 ⇒ 兼容，
不需要 `protocol_version` bump，也不需要 legacy 分支。

---

## 25. Lock Scope

### INV-P25 — 发射事件不得持有 db 锁

**规则**：`AppState.db` 那把 `Mutex<Connection>` 的**存活期内不许出现任何 `emit`**。
写库要用 `{ … }` 圈住（或 `drop(dbc);`），事件放到圈外再发。

**为什么是硬规矩而不风格**：生产环境只有一条 SQLite 连接，前端每个 IPC 命令都抢同一把锁。
锁内 emit 的完整链条是：后端发事件 → WebView 监听器立刻回一次 IPC → 那次 IPC 阻塞在
还没释放的锁上 → **用户看到"出错那一刻界面整个冻一下"**。事件越是紧跟着一次刷新，
冻得越明显，所以文件失败路径（`file-failed` ⇒ 前端立刻 `list_transfers`）是重灾区。

**判据在哪跑**：`scripts/check-lock-scope.mjs`（`npm run verify` 快速层，CI frontend 组）。
它扫 292 个取锁点，分三类判：guard 绑定（活到块结束或 `drop`）、`if let` 条件与实参位置的
语句级临时量（活到本语句结束）、以及"判不出作用域"——第三类**直接红**，因为一个会因看不懂
而静默放过的守卫比没有守卫更坏。判据本身带 7 段夹具自证（`--self-test`）。

**边界（这条规矩不管什么）**：

* 只管 db 那把锁。`peers` / `file_receivers` / `relay` 等 Mutex 没有被前端 IPC 抢，
  把它们一起管需要逐个论证"谁在抢它"，那是另一件事。
* 跨函数的 emit 看不见。缓解靠命名纪律：**会 emit 的一律叫 `emit*`**（现状如此，
  `emit_failed` 是唯一 helper）。
* 条件式临时量按 `edition = "2021"` 判（析构在块之前）。**升 2024 时这一半判据必须改成块级**，
  否则假绿 —— 这是脚本头部判据 B 里那条警告的文档版。

**为什么 2026-09-25 才立**：纪律本身早就写在 `network/transport.rs:90`，但那天补接收侧静默回收时，
`fail_taken_receive` 仍然写成"写库 + emit 连着两行"——同一个函数体里两条相邻语句，逐行 review
谁都看不出毛病，只有全局扫才看得见。同一批还扫出中继收文件失败分支 3 处同样的形状。
**注释管不住新写的分支**，所以从这天起它是一条有机器判据的不变量。

---

## 26. Terminal State

### INV-P26 — 有磁盘证据的状态不可被降级

`file_transfers.status` 今天有 39 个写入点（active 12 / failed 12 / done 6 / pending 3 / sent / cancelled），
所以"谁能写什么"必须由**写入口**保证，而不是靠每个调用点自觉。

规则只有一条：**`done` 不许被改写成别的状态**。状态、进度、路径三个字段走的是同一条
`ON CONFLICT DO UPDATE`，闸门加在语句上就一起生效（`db/file_transfer.rs::upsert_transfer`）。
队列判死走 `mark_queued_transfer_failed`，合法集合同样是"非 done"。

为什么只有 `done`：它是唯一带**磁盘证据**的状态（长度对 + sha256 对 + `sync_all()` 之后才 rename）。
`sent` 只证明字节写出去了、没有送达证据；`failed` / `cancelled` 更不是终点 ——

> **`failed` 必须还能改回 `active`**：`retry_incomplete_content` 复用同一个 `transfer_id`
> 发 `ContentRequest`（`network/transport.rs:2644`）。把 `failed` 一起钉死的后果是
> "一判死就永远停在失败，而字节其实还在流" —— 那恰好是本条要修的缺陷的反面。
> 反向判据：`cascade_tests::a_failed_row_can_be_reactivated_by_the_next_attempt`。

配套的两条口径：

* **同一个关注点只有一个家**：除 `db/file_transfer.rs` 之外不许再出现直接
  `UPDATE file_transfers SET status`（守卫 `cascade_tests::terminal_status_writes_have_one_home`）。
  有第二个家时，"改一个忘一个"是常态 —— 本仓 §9 那族平行实现反复就是这个形状。
* **两个失败口径不许合并**：`file_outbox.status` 的 `failed` = **自动**判死（超时 / 重试耗尽 /
  接收方不可达），`cancelled` = 用户主动取消。三条队列查询只认 `pending` / `sending`，
  所以两者在功能上等价 —— 但台账不等价，混用之后"这一单为什么失败"就查不动了。
  判据：`cascade_tests::cancelled_file_outbox_rows_are_never_requeued` +
  `a_user_cancel_is_not_recorded_as_a_failure`。
* **emit 由写库结果门控**（与 INV-P15 / 审计 A3 同一口径）：已经 `done` 的行没被改动，
  就不该为它发 `file-failed`。为一个躺在下载目录里能打开的文件弹"传输失败"是谎话。

**非空转证明**：两条 `verify-guards` 变异用例 —— 摘掉 `WHERE status <> 'done'` ⇒ 正向判据红；
照本条落地前的建议把集合写成 `NOT IN ('done','failed','cancelled')` ⇒ 反向判据红。

---

# 27. Required Test Matrix

核心消息功能至少覆盖：

| Scenario | Expected |
|---|---|
| Direct | 1 message |
| Gossip | 1 message |
| Direct + Gossip | 1 message |
| Duplicate | 1 DB row |
| ACK lost | Outbox remains |
| TCP half-open | Outbox remains |
| Peer offline | Outbox remains |
| Reconnect | Message delivered |
| App restart | Outbox recovered |
| Receiver restart | Duplicate safe |
| Key missing | Visible failure |
| Key changed | Security-relevant handling |
| Gossip TTL exhausted | Stop forwarding |
| Big file + chat burst | Chat delivered promptly |
| File chunk order | Chunk N before Done |
| Group clear boundary | seq <= boundary blocked |
| Clock skew | Ordering/read state unaffected |
| BLE chunk budget（两侧） | `peripheral` 与 `central` 推出同一个数（INV-P23） |
| 收到未知 type 的帧（INV-P24） | 忽略 + 记日志，**链路保持**、其它消息照常 |
| 老版本对端 + 新帧能力 | 发送方按对端版本门控 ⇒ 退回旧行为，不产生无法解析的帧 |
| 对端版本更高 | 界面给出可解释状态，不是错误弹窗/裸 JSON/永久转圈 |
| 老端 Hello（不带版本字段） | 照常建链；面板/日志显示「未声明」，不猜成版本 1 |
| 晚到的失败判定落在已 done 的传输行（INV-P26） | 状态/进度/路径三样都不许变，且不发 `file-failed` |
| 同一 transfer_id 的新一轮续传落在 failed 行（INV-P26 反向） | 必须能改回 active，否则断点续传停在旧终态 |
| 锁内 emit（INV-P25） | `check-lock-scope.mjs` 扫全部取锁点为 0；判不出作用域的那一处**直接红**，不算通过 |
