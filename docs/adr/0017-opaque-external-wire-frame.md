# ADR-0017: BitChat 中继的线格式（OpaqueExternal 帧变体）

- Status: **Accepted**（2026-09-12 用户裁决：**本版不考虑旧版本兼容**，直接按最优设计做）。
  因此本文原设计的"能力门控 + 版本化双读"**不再需要**（见文末「决策更新」）；
  实现（Phase 8）待做，验收只三件事：收得到、去得掉重、TTL 递减后转发
- Date: 2026-09-12
- Owners: Gosslan
- Related:
  - 计划：`.workbuddy/mesh-task/P2-P3-开发计划.md` §4（P3 / Phase 8）
  - 设计：§13/§14（帧种类必须分开）、§29（外部帧走同一条流水线）、§8.1 #11（YAGNI）
  - 协议：ADR-0007（协议版本化与双读）
  - CHANGELOG: `[Unreleased]`
  - 代码现状：`mesh::MeshFrameKind::OpaqueExternal` 已就绪（**抽象层**），
    但 `protocol::Message` 线格式**没有**对应的帧变体

---

## 1. Context

Phase 8 的目标只有三条（其余一律不做）：**收得到 · 去得掉重 · TTL 递减后转发出去**。
也就是说 Gosslan 只当 BitChat 的中继：不解密、不落库、不建 BitChat 用户/channel/UI。

现状：
- **抽象层已备好**：`MeshFrameKind::OpaqueExternal` + `MeshRouter::on_receive` 的流水线
  对不透明载荷一视同仁（有单测 `opaque_external_frame_follows_same_pipeline`）；
- **线格式没有入口**：`Message` 是 `#[serde(tag = "type")]` 的枚举，只有
  Hello / Heartbeat / ChatMessage / Gossip / 文件分片等**业务**变体。
  想要"收到一个 BitChat 包"，就必须新增一个线格式变体。

## 2. Problem（为什么不能"顺手加一个变体"）

旧端（今天的构建）收到未知的 `type` 时，`serde_json::from_slice::<Message>` **失败**
⇒ `read_frame` 返回 `InvalidData` ⇒ `reader_loop` 视为坏帧并**断开整条连接**。
后果不是"忽略一个包"，而是**混版本拓扑直接断链**：
三台设备里只要有一台没升级，升级方一转发 BitChat 包就会把与它的连接打断
（用户 2026-09-12 明确要求"现有局域网聊天不能搞坏"）。

另外 `Hello` 的**签名串**（`hello_signing_bytes`，前缀 `gosslan-hello-v1`）覆盖
device_id / tcp_port / nonce / 双公钥。若把"我支持 OpaqueExternal"放进签名串，
旧端验签会失败 —— 同样断链。所以能力声明必须**不参与签名**（像 `device_type` 那样）。

## 3. Options

### A. 能力门控的新变体（推荐，但**与 BLE 同批落地**）
1. `Hello` 增加**不参与签名**的字段 `caps: Vec<String>`（旧端按 serde 默认忽略未知字段
   —— 需在实现时验证 `Message` 未开 `deny_unknown_fields`，当前确实未开）；
   新端在 caps 里声明 `"opaque_external"`。
2. 只在**对端声明了该能力**时，才向它发送新变体 `Message::OpaqueExternal { id, ttl, payload }`
   （`payload` 为 base64 的原样字节）。
3. 能力是**远端自报**、只用于"避免打断旧端"，所以：拿不到声明 ⇒ 不发（fail-safe），
   绝不因为声明了就信任其内容（内容对 Gosslan 永远是不透明字节）。
4. 新变体**不参与** `compute_message_id()` 体系（它有自己的 `id`，用于去重）。

### B. 把 Phase 8 推到 BLE 落地之后（**本文推荐**）
理由：Phase 8 的**唯一现实输入是 BLE**（BitChat 是 BLE mesh 应用）。BLE 传输层还没实现
（ADR-0015 的 7-e），现在做 Phase 8 等于：先改线格式，却没有任何真实流量能验证它，
还要额外承担 §2 的兼容风险。项目红线 §8.1 #11（YAGNI：不为未来提前加抽象）也支持等待。

### C. 直接升协议版本、不做能力门控（**拒绝**）
"所有节点同步升级"在用户的自有 3 台设备上看似可行，但：① 一旦有一台旧版就断链；
② 违反 ADR-0007 的双读约定；③ 与用户"现有功能必须稳定可用"的要求冲突。

## 4. Decision（建议）

1. **采纳 B**：Phase 8 与 BLE 驱动（7-e）**同批**实施 —— 那时才有真实 BitChat 流量
   可验证"收得到/去重/TTL 转发"三条判据，也才能在三平台真机上验证混版本行为。
2. 同批实施时**按 A 落地**：能力门控 + fail-safe 不发 + 能力不参与签名。
3. 无论何时实施，**必须先补一条测试**：把"旧端遇到未知变体会断链"这个事实钉住
   （构造未知 `type` 的 JSON → 断言 `read_frame` 返回 `Err`），
   这样 A 的"只在对方声明能力后才发"才有非空转的护栏。

## 5. Consequence

- ✅ 现在不动线格式 ⇒ 用户的 3 台设备（可能含旧构建）零风险。
- ✅ Phase 8 的实现顺序变成"BLE 驱动 → 能力声明 → OpaqueExternal 转发"，
  每步都能真机验证，不需要"盲改协议"。
- ⚠️ 若用户希望**先**把 Phase 8 的帧转发逻辑（去重 + TTL）做出来，可行 ——
  但只能在没有真实流量、且不触碰线格式的前提下做（例如只做纯函数级的
  `opaque_forward_decision`），价值有限；本文建议不做。
- ⚠️ 若将来 BitChat 之外的第二种外部协议出现，**不要**再加一个变体：
  统一用 `OpaqueExternal` + 载荷里的协议标识（本文的实现注释会写明）。

---

## 决策更新（2026-09-12，用户裁定）

用户明确：**这一版不考虑旧版兼容**，按最优、体验最好的设计来做。因此本 ADR 里为"旧端"
设计的能力门控 + 版本化双读**不再需要**：

- 可以直接给 `Message` 增加新的帧类型（例如不透明外部帧 / BitChat 中继帧），
  不必再保证"未知 `type` 不被断链"；
- 也**不需要**新增能力位或双读窗口 —— 同版本客户端之间互相认识即可；
- 仍然保留一条底线：**新类型必须自带长度/边界校验**，畸形帧只丢该帧、不断链
  （这是健壮性，不是兼容性）。

⚠️ 影响面：升级说明里要写清"必须所有设备一起升级到同一版本"。
