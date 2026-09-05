# Gosslan 协议设计与演进（参考 BeeBEEP / bitchat）

> 目标：对标成熟的开源去中心化聊天协议（BeeBEEP、bitchat），审视 Gosslan 在
> **安全（Security）/ 完整（Integrity）/ 及时（Timeliness）** 三个维度的差距，
> 并为「协议与传输解耦」「蓝牙无配对信道」给出演进路线。
> 最后更新：2026-09-05（v0.7.0）。

---

## 1. 现状盘点（Gosslan 已有能力）

| 维度 | 现状 |
|---|---|
| 发现 | UDP 广播 255.255.255.255 + 组播 239.255.42.99（:59991），自适应周期 + 抖动；按需 `who_has` 探测 |
| 传输 | TCP 分帧（4B 大端长度 + JSON，:59992）；建链规则 = device_id 字典序小者拨号 |
| 加密 | 单聊：X25519 静态 ECDH 派生密钥 + ChaCha20-Poly1305；群聊：群密钥对称加密（按成员公钥分发） |
| 认证 | Gossip 信封 Ed25519 签名（对 message_id），接收方验签 |
| 可靠性 | 消息级 Ack；outbox 离线队列（**Ack 才删行**，Hello/心跳触发补发）；接收方 msg_id 去重 |
| 回执 | 送达 Ack + 已读回执（ReadReceipt，合并式 last_read_ts，窗口聚焦时补发） |
| Mesh | Gossip 泛洪（Bloom+LRU 去重、fanout、TTL）；大文件切片并行分发；Transport trait 已抽象（lan/bluetooth） |

## 2. BeeBEEP 可借鉴点（15 年演进的 LAN 通讯录）

- **发现多通道冗余**：广播 + 组播 + mDNS（macOS Bonjour `_beebeep._tcp`）+ 手动添加主机。
  → Gosslan 可补 **mDNS** 作为第三发现通道（跨子网/隔离广播域场景），实现成本低。
- **协议帧结构化**：8 字节协议头（`BEE-CHAT`/`BEE-FILE`/`BEE-USER`）+ 消息 ID + 标志位 +
  UTC ISO 时间戳 + 数据。类型即头，解析器极稳。
  → Gosslan 的 JSON `type` tag 等价，但**二进制头更省**（移动端/蓝牙带宽敏感时再迁）。
- **每连接随机会话密钥**：BeeBEEP 为每条点对点连接生成随机 256 位 AES 密钥。
  → Gosslan 目前用长期静态 X25519 派生密钥（无前向保密），见 §4 差距。
- **文件传输独立端口 + 断点续传**（暂停/恢复/失败重收）。
  → Gosslan 文件与消息同端口分帧；大文件已有切片重组，可补**断点续传**（chunk 级 Ack 落盘）。
- **用户识别方式可选**（昵称 / 账号+域 / 账号）。
  → Gosslan 用设备指纹 + 密钥，天然更稳，无需此选项。

## 3. bitchat 可借鉴点（BLE Mesh + Noise 的现代范式）

- **Transport 抽象 + MessageRouter**：BLE mesh 与 Nostr 双传输实现同一接口，路由器择优。
  → Gosslan 的 `transport/` 模块（`Transport` trait + `TransportManager`）正是此结构，
  蓝牙通道接入后消息层零改动。**抽象方向正确，继续沿走。**
- **无配对 BLE Mesh**：每台设备同时做 GATT Central + Peripheral，受控泛洪：
  - TTL 初始 7，按连接度自适应（≥6 链路封顶 5，稀疏链全深度）
  - 去重 LRU 1000 条 / 5 分钟；fanout = log₂(degree) 且用 message_id 做种子（确定性选路）
  - 转发抖动 10–220ms（密集时更宽）
  → Gosslan Gossip 参数（fanout=4、TTL=6、Bloom+LRU）同量级；接入蓝牙时按连接度自适应即可。
- **Noise XX 会话**（Curve25519 + ChaCha20-Poly1305 + SHA-256）：双向认证 + 前向保密，
  静态密钥指纹 = 身份（SHA-256 前 8 字节 = peer ID）。
  → Gosslan 已有 X25519/Ed25519/ChaCha20 同族原语，**升级 Noise XX 是补前向保密的最短路径**
  （Rust 生态用 `snow` crate）。
- **送达 Ack + 已读回执 + 聚焦时发回执**：与 Gosslan v0.6.0 起的实现一致，方向得到印证。
- **Store-and-Forward**：持久 outbox + 机会式转发（spray-and-wait 复制预算）。
  → Gosslan outbox 已覆盖单跳；多跳暂存可复用 `relay/mesh_router.rs` 的有界 RingBuffer。
- **身份绑定**：收藏固定完整公钥；QR 当面扫码绑定 昵称↔指纹。
  → Gosslan 可在「添加好友」成功页加**安全码/QR 校验**（防中间人，TOFU 增强版）。

## 4. 差距分析（Security / Integrity / Timeliness）

### Security
| 差距 | 现状 | 演进 |
|---|---|---|
| 前向保密 | 静态 X25519 派生长期密钥，私钥泄露 = 历史全解 | **Noise XX 会话**（snow）：建链握手派生会话密钥，静态密钥仅做身份 |
| 身份校验 | 公钥随 announce 广播，无人工验证 | 好友添加完成页展示**指纹安全码 / QR**，当面核对防中间人 |
| 重放防护 | msg_id 去重（兼防重放） | 会话密钥时代改用 nonce 序号窗口（Noise 自带） |
| 元数据 | payload 长度即明文长度 | 关键帧填充至 256/512/1024 桶（bitchat 式），蓝牙低带宽时优先做 |

### Integrity
| 差距 | 现状 | 演进 |
|---|---|---|
| 直发链路丢包 | TCP 可靠 + Ack + outbox 补发（v0.7.0 已修「入队即删」缺陷） | 已达标；补发去重靠 msg_id，幂等 |
| 分片 | 大文件有切片（64–512KB）+ 乱序重组；普通消息单帧 | 蓝牙 MTU 限制需要**通用分片协议**（frag_id, seq, total），消息层透明 |
| 时钟偏差 | 依赖发送方时钟排序，设备间偏差会影响展示顺序 | 前端已做乐观时间戳钳制；协议层可在 Ack 中携带接收方时钟做偏移估计（低优先） |

### Timeliness
| 差距 | 现状 | 演进 |
|---|---|---|
| 离线补发时机 | Hello/心跳触发 flush | 已达标；可加「链路建立后 200ms 延迟批补」减少建链竞态重发 |
| 泛洪风暴 | 自适应周期 + fanout + TTL | 蓝牙接入时改 log₂(degree) fanout + 10–220ms 抖动 |
| 多跳暂存 | RingBuffer 有界队列（接口就绪未接线） | 蓝牙通道接入时启用 store-and-forward 复制预算 |

## 5. 蓝牙无配对信道落地路线（bitchat 式）

1. **传输实现**（`transport/bluetooth.rs`，feature `bluetooth` 引入 `btleplug`）：
   - 每设备同时注册 GATT Peripheral（广播服务 UUID + 写/通知特征）与 Central（扫描同 UUID）
   - 发现即连，无需配对；`available()` 改为探测适配器
2. **帧适配**：GATT 通知 MTU 有限 → 启用通用分片协议；二进制头替代 JSON（蓝牙带宽优先）
3. **路由接线**：`TransportManager.route()` 按负载与链路度分流（已有）；`mesh_router.rs`
   接入跨链路桥接 + store-and-forward
4. **加密升级先行**：先落 Noise XX 会话（TCP 通道先上），蓝牙通道直接复用会话层
5. **节能**：电池感知占空比（bitchat 三档模式）放到最后迭代

## 6. 演进优先级

1. **P0** Noise XX 会话（前向保密）—— 安全收益最大，TCP 通道先行
2. **P1** 好友指纹安全码/QR 校验 —— 防中间人，UI 成本低
3. **P1** mDNS 第三发现通道 —— 覆盖隔离广播域
4. **P2** 通用分片协议 + 二进制帧头 —— 蓝牙前置
5. **P2** BLE 无配对通道（btleplug）—— `Transport` trait 落地第二实现
6. **P3** 帧填充 / 时钟偏移估计 / 电池自适应

> 结论：Gosslan 的协议骨架（发现双通道、E2EE、Gossip、Ack+回执、outbox、Transport 抽象）
> 与 BeeBEEP/bitchat 的成熟实践同构；核心待补是**前向保密（Noise XX）**、**身份人工校验**、
> **蓝牙第二通道**。抽象边界已就位，演进不需要重写消息层。
