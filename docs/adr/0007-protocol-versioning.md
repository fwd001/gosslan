# ADR-0007: Protocol Versioning

- Status: **Accepted**（2026-09-20 用户授权按最佳实践决定；见文末"实现决定"）
- Date: 2026-09-05（Proposed）／2026-09-20（Accepted）
- Related: `protocol.rs`, `src/types.ts`, `docs/protocol-invariants.md` **INV-P24**,
  `AI_RULES.md` §12（premise 已于 2026-09-20 更正）

## Context

Gosslan 的协议已经包含 discovery、ChatMessage、Ack、Gossip、GroupKey、文件传输等多种消息。

随着 Noise、BLE、QUIC、Mesh 等能力继续演进，协议字段和语义变化不可避免。

如果没有显式版本策略，AI 很容易直接修改 enum/serde 结构，导致旧客户端：

- 无法解析
- 静默丢消息
- 错误解释字段
- 破坏 ACK / outbox

## Decision

建立显式 protocol version。

协议版本用于表示：

> 线格式或消息语义是否发生兼容性影响。

不把 app version 当 protocol version。

任何破坏兼容性的 protocol change：

```text
protocol version bump
→ compatibility analysis
→ ADR
→ tests
```

## Rules

1. 新增可忽略字段优先保持向后兼容。
2. 删除/重命名字段必须视为 breaking change。
3. 修改字段语义必须视为 breaking change。
4. 新增 Message variant 必须定义旧客户端行为。
5. 未知消息不得导致 panic。
6. 发送方必须根据对端能力决定是否使用新消息。
7. 旧协议消息不能被错误解释成新语义。

## Consequences

协议演进速度会稍慢，但可以避免 AI 在未来通过“直接改 enum”制造跨版本隐性 bug。

## Revisit

当 Gosslan 建立成熟的 capability negotiation 后，可以把部分 version decision 下沉到 capability negotiation。

---

## 实现决定（2026-09-20，用户授权"按最佳实践决定，体验与扩展性优先"）

### 前提更正（这条决定成立的原因）

本 ADR 长期停在 Proposed，是因为 `AI_RULES.md` §12 写着"Gosslan 尚未发布、没有历史客户端需要兼容"。
该前提**已被仓库自身证伪**：GitHub Releases 上有带安装介质与 sha256 的正式发布
（`v4.8.2` 2026-09-14 → `v4.20.0` 2026-09-18，含 macOS/Windows/Android 三类包）。
装了 4.18 的手机和跑 4.22 的 Mac 在同一个网里说话，是当前的现实而不是假设。

### 决策 1：上 `protocol_version` + `app_version`（wire 兼容，可立刻做）

- `Hello` 增加两个**可选**字段：`protocol_version: Option<u32>`、`app_version: Option<String>`。
  老对端不发 ⇒ `None` ⇒ 按"最低版本"处理；新对端收到我们的字段则忽略（serde 默认忽略未知字段）。
  **因此这一步不断老版本互通**，与决策 3 不同。
- 两者分开存：`protocol_version` 是**兼容性判据**（决定是否允许用新帧/新语义），
  `app_version` 只用于**给人看**（诊断面板、"对方版本较新"提示）。绝不用 app version 做兼容判断
  —— 那是本 ADR Rule 里"不把 app version 当 protocol version"的落地。
- 本机当前 `PROTOCOL_VERSION: u32 = 1`。今天所有已发布版本都等于 1；任何破坏兼容的变更才 +1。
- 不引入版本区间/协商矩阵那套机制（YAGNI）：只做"我知道对端是几，能力我按几来"。

### 决策 2：群文件半途重投 **不加 attempt epoch**，改为"半途不重投"

`GroupFileChunk{seq}` 每次重投都从 0 重数，而群接收端严格 `seq != next_seq` 判死 ⇒
上一轮在途残片必然撞上新一轮的序号。加 epoch 是新帧/新字段 ⇒ 需要决策 1 先铺开，收益却要等到
"群文件支持断点续传"才体现。当前更诚实也更小的做法：

- **同一 `transfer_id` 的部分完成状态不得被重投复用**：接收端一旦判死就把该成员落到终态
  （v4.22.19 已落），发送端只补发 `pending`（从未开始）的成员；已失败成员由用户重发，
  重发产生**新的** `transfer_id` ⇒ 两侧状态天然干净，不存在混流。
- 将来若要做群文件续传（省流量的真实收益），再作为 capability 上线：
  `GroupFileOffer{ from_bytes }` + 接收端回 `received`，**门控条件是对端 protocol_version ≥ 2**。

### 决策 3：HKDF 域分离 + nonce 策略 **做，但走版本分支，不做硬切**

现状是 ECDH 原始共享密钥直接当密钥用、无域分离。这是真实缺陷，但改它会断与老版本的互通
（解不开对方的密文），而在无强制升级通道的 mesh 里"硬切"等于把老设备变成连不上的人。

- 实现为**按对端 protocol_version 选择派生方式**：双方都 ≥ 2 ⇒ `HKDF(shared, salt, "gosslan-chat-v2")`
  之类的域分离派生；任一方为 1 ⇒ 走 legacy 派生。密钥派生发生在**会话建立时**，
  所以分支是每次会话级的，不需要在每个帧里带版本位。
- legacy 分支必须有**退出条件**（写死在这里，免得它变成永久技术债）：当 releases 的下载/使用
  证据显示 version 1 线已无人在用时删除它，并把 `PROTOCOL_VERSION` 提升为最低要求，
  同时对低版本对端给出"需要升级"的明确提示（INV-P24 的拒绝路径），而不是静默失败。
- 与决策 1 同批实现最省：一次 bump 里把"能报版本 + 能用新派生"的骨架建好。

### 落地顺序（不可颠倒）

```text
① Unknown 兜底（未知帧不拆链）+ Hello 带 protocol_version/app_version + 诊断面板显示对端版本
   —— 全部 wire 兼容，老设备不受影响
② 前端渲染兜底（不显示裸 JSON）+ "对方版本较新"的可解释状态
③ 需要新帧/新语义的功能（群文件续传、HKDF v2 派生）：等 ① 在网里铺开后才允许上线，
   并且一律按 capability 门控，不门控不许发
```

第 ① 步没有做完之前，任何"新帧类型"都不许上线 —— 那会让老设备**直接连不上**（INV-P24 的
"为什么"一节记的就是这个后果）。

### 落地进度（2026-09-20）

```text
① 前半  容忍 Unknown            ✅ v4.22.27  Message::Unknown + decode_frame（握手首帧仍严格）
① 后半  Hello 报版本 + 面板显示  ✅ v4.22.28  protocol_version / app_version（可选、不进签名）
                                   → AppState::peer_versions → 诊断面板「版本互通」+ 降级日志
②      前端渲染兜底 + 可解释状态  ◐ v4.22.33 渲染兜底已落（未知 kind 绝不显示裸 JSON，
                                   判据 = `is_known_kind`/`isKnownKind` 查 WIRE_KINDS）；
                                   "对方版本较新"的用户可见状态 ⬜（数据源 V2 已就绪）
                                   ✅ 另一半 v4.22.34：`ChatMessage.kind` 改回 `String` —— 未知 kind 不再在帧层
                                   被丢掉，单聊与群聊两条路径都会显示占位
③      新帧 / HKDF v2 派生       ⬜（且必须等 ① 在网里铺开后才允许）
```

决策 1 的"这一步不断老版本互通"已经在测试里坐实：老格式 Hello（没有这两个字段）必须照样
解析成 `None`，而 Hello 遇到未知**字段**必须照单收下（新→老方向）—— 见
`hello_version_fields_roundtrip_and_old_peer_declares_nothing`。
反过来，"字段进了签名材料"这个会让老端验签失败的错误由守卫
`peer_version_is_declared_not_signed_and_reclaimed` 钉住，并由 `verify-guards.py`
的「Hello 必须声明本机版本」用例做非空转验证。

**这里刻意没做的事**（下一个 AI 别顺手加）：没有 `MIN_PROTOCOL_VERSION`、没有版本区间协商、
没有 capability 位图。今天没有任何一条按版本门控的消息，所以那些机制一个调用点都没有；
它们应随第一个真正需要门控的新帧一起出现（那时才知道要门控什么）。
`PROTOCOL_VERSION` 现在全网都是 1，它的价值只是"把版本号写进线格式，让将来能写门控"。

