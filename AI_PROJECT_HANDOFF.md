# Gosslan 项目全景与开发指南（AI Handoff）

> **本文件的用途**：为三类读者提供完整项目上下文——
> ① 用 AI 编程工具（Claude Code / CodeBuddy / Cursor 等）继续开发的人；
> ② 想通读源码理解设计的开发者；
> ③ fork 后想二次开发的人。
>
> 内容包括：全部功能、架构与代码导读、协议与加密状态机、工程约定、测试口径、
> 历史演进与未来设想。最后更新：**2026-09-08（v1.0.0）**。
>
> ⚠️ **AI 编程助手请先阅读 [AI_RULES.md](AI_RULES.md)**（工程宪法：不变量 / 禁止事项 / 强制流程），
> 再读本文件了解项目全貌。架构设计原因见 [docs/adr/](docs/adr/)。

---

## 0. ⚠️ 时效性声明（2026-09-13 核对，**先读这一节**）

本文主体的快照停在 **v1.0.0（2026-09-08）**，而仓库**实际已经在 `4.2.19`**，
主干是 **`next`** 分支（`main` 停在 v2.1.2，落后 36 个提交）。
也就是说：**下面 §2 的功能清单、§7 的历史时间线都不是当前全貌**，只能当"早期架构与设计理由"读。

当前状态请以这些为准（按可信度排序）：

| 想知道 | 去哪里看 |
|---|---|
| **V5 目标架构与路线图（当前主线）** | **`docs/architecture/README.md` + `docs/architecture/07-roadmap-v5.md`（ADR-0020~0025）** |
| 最近在干什么 / 每个改动为什么 | `CHANGELOG.md` 的 `[Unreleased]` + 最新的带日期小节（当前最新 **4.2.19**） |
| 蓝牙（BLE）传输的完整设计与平台边界 | **`docs/adr/0015-ble-transport.md`**（含 §7.7 Android 外设、§7.9 Windows central） |
| 蓝牙真机排查的过程与判据 | `docs/notes/ble-audit-2026-09-13.md`（架构图 / 根因 / Test A~F） |
| 多路径选路、中继授权、窗口架构 | `docs/adr/0014`（多路径选路）· `0016`（中继授权）· `0018`（窗口架构）· `0017`（外部线格式） |
| 版本号怎么升（强制流程） | `docs/VERSIONING.md` + `npm run version:check` |
| UI 规范 | `docs/design-guidelines.md` |

本文仍有价值的部分：§1 定位、§4 核心机制（E2EE 状态机 / 已读回执 / 前端消息管线）、
§5 工程约定、§6 测试口径。但**任何与代码冲突的地方，一律以代码 + CHANGELOG + ADR 为准**。

与本文已知的**具体过时点**（读到就跳过）：

- §3 目录树：`src-tauri/src/` 现在还有 `discovery/`、`mesh/`、`transport/`（`tcp.rs`/`bluetooth.rs`/
  `bluetooth_peripheral.rs`/`ble_android.rs`/`ble_framing.rs`）、`network/ble.rs`、`logging.rs`、
  `menu.rs`、`open_path.rs`、`user_dirs.rs`、`export.rs` 等（见 ADR-0014/0015/0018）。
- §8.2 第 4 条、§8.4 末条：说"BLE 只是接口契约、未接线"—— **已过时**。
  Android 与 macOS 的 central + peripheral 都已实现；**Windows 的 central 已于
  2026-09-13 接线（`7-g`）**，**Windows 的 peripheral 也已在同日补齐**
  （`26969a9`，WinRT `GattServiceProvider`；详见 ADR-0015 §7.10）。
- §9 命令速查：Windows 生产包现在有一条命令 **`npm run dist:win:test`**
  （`scripts/build-windows-release.ps1`：护栏 → `tauri build --features bluetooth --bundles nsis` → 产物 + SHA-256）。

---

## 1. 项目一句话定位

**Gosslan**（gossip + LAN）是一款**无中央服务器**的 P2P 局域网即时通讯应用
（Windows / macOS / Android 三端）：

- 设备间通过 **UDP 组播/广播发现 + TCP 点对点传输**直接通信，无注册登录、无账号体系
- 所有单聊/群聊消息**强制端到端加密**（X25519 + ChaCha20-Poly1305），中继无法查看
- 用**设备指纹**识别同一用户，数据**只存本机 SQLite**
- 界面高度模仿**飞书 / 钉钉**，交互优先「乐观更新 + 失败可感知」

- 仓库：`github.com/fwd001/gosslan`（public，默认分支 `main`）
- 当前版本：**1.0.0**（package.json / src-tauri/Cargo.toml / src-tauri/tauri.conf.json / Cargo.lock 四处一致）
- 应用标识：`com.gosslan.app`，ProductName：`Gosslan`，CSS 前缀 `--gosslan-*`
- 技术栈：**Tauri v2 + Rust 2021 + Vue 3 + TypeScript + Vite + Tailwind CSS + Headless UI + Lucide + Pinia + SQLite（rusqlite bundled）**
- 作者：wd.f（fuwedong），MIT 协议

---

## 2. 完整功能清单（v1.0.0 现状）

### 2.1 网络与消息核心

| 功能 | 说明 | 版本 |
|---|---|---|
| UDP 发现 | 广播 255.255.255.255 + 组播 239.255.42.99（:59991）；自适应周期（<100 节点 5s / ≥100 10s / ≥500 20s）+ 0–2s 抖动 | 0.1.0 / 0.2.0 |
| 按需探测 | 打开「添加好友」时才群发 `who_has`，在线节点单播回复 announce；启动不持续扫描 | 0.3.0 |
| TCP 传输 | 分帧 = 4 字节大端长度 + JSON（:59992）；device_id 字典序小者主动拨号 | 0.1.0 |
| Gossip 广播 | Epidemic 泛洪，Bloom Filter + LRU 去重，fanout + TTL 衰减；信封 SHA-256 message_id + Ed25519 签名 | 0.2.0 |
| E2EE | **恒开且不可关闭**（v0.11.0 起）：X25519 ECDH 派生密钥 + ChaCha20-Poly1305 AEAD；详见 §5 | 0.2.0→0.11.0 |
| 群聊 | 随机群密钥对称加密；群密钥用各成员公钥 ECDH 单独加密分发（`GroupKey`） | 0.2.0 |
| 离线补发 | 单聊消息写 `outbox`，群消息写 `group_outbox`，文件写 `file_outbox`；**Ack / GroupAck / FileCompleteAck 到达才删行**；对方上线建链/心跳触发对应 flush；接收方按 msg_id / transfer_id 幂等去重 | 0.4.0 / 0.7.0 / 1.0.0 |
| 已读回执 | 单聊 `ReadReceipt`、群聊 `GroupReadReceipt` 均携带 `last_read_msg_id`，发送方用消息 ID 换算自己的本地序号，不依赖双方墙上时钟；链路不可用时暂存 `pending_reads` / `pending_group_reads`，由建链/Hello/心跳补发 | 0.6.0 / 1.0.0 |
| 逻辑序号 | 每会话 `conversation_clocks` 维护 Lamport 风格逻辑序号 `seq`；消息按 `seq,id` 排序，群聊清空边界也按 `seq` 判断，完全不依赖墙上时钟 | 1.0.0 |
| 传输优先级 | 每条 TCP 连接有 bulk / priority 双队列；文件分片走 bulk，聊天/控制/回执走 priority，大文件传输不会再饿死普通消息 | 1.0.0 |
| 局域网默认开启 | 桌面端启动即自动联网（`start_from_prefs`，沿用 `bind_ip`，网卡失效时回落 0.0.0.0）；用户在设置页关闭后写 `settings.lan_enabled="0"`，重启保持关闭。移动端仍为手动开启 | 0.12.0 |
| 心跳探活 | 每 5s 向已建链节点发 Heartbeat，静默断连及时清理 | 0.4.0 |
| 大文件 | 直连流式分片 + `file_outbox` 断线补发；接收方校验 size/SHA-256 后回 `FileCompleteAck`，发送方确认成功才标 delivered。中继自动路由暂统一为「直连 + 离线队列」 | 0.2.0 / 1.0.0 |
| 共享目录 | 设置本地共享文件夹，好友点对点浏览目录树并下载（防目录穿越） | 0.1.0 |
| 多开 | `--instance N`：独立 DB / TCP 端口 / 指纹，UDP 共享（SO_REUSEADDR + unix 上 SO_REUSEPORT） | 0.3.0 / 0.4.1 |

### 2.2 聊天 UI / UX

| 功能 | 说明 |
|---|---|
| 三栏响应式布局 | PC 三栏（导航 / 会话列表 / 聊天区），移动端单栏滑动切换 |
| 消息类型 | 文本 / 代码（highlight.js 自动检测语言 + 折叠）/ 图片（粘贴板直发）/ 文件（内嵌进度条）/ 系统消息 |
| 长文本折叠 | >280 字符默认折叠 5 行，「展开全文 / 收起 / 复制」 |
| 消息回执图标 | 我发的消息**气泡左侧**挂状态：转圈=发送中，空心圆=已送达未读，**绿勾=已读**，红叉=失败（v0.10.0 移到左侧） |
| 消息时间 | 默认 `MM-DD HH:mm`，hover 切秒级 `YYYY-MM-DD HH:mm:ss`；同分钟合并消息仅 hover 显示 |
| 连续消息合并 | 同发送者 5 分钟内省略头像/昵称；≥5 分钟显示时间分割线；同分钟消息合并显示 |
| 虚拟滚动 | 自研 VirtualList：纵向滚动、scrollToIndex 任意定位、触顶分页加载历史（每页 100 条，上限 10 页）、未读定位（「以下是未读消息」分割线）、「回到最新」悬浮按钮 |
| 聊天样式 | 6 套配色预设（明暗双主题，对比度 ≥4.5:1）+ 3 档字号 + 消息合并开关；**跨设备同步**：对方按「我的配色」渲染我发的消息 |
| 复制 | 气泡 hover 复制按钮；**禁用**了文本右键自定义菜单（v0.10.0） |
| 删除聊天记录 | 会话列表项**右下角 X 按钮**（hover 浮现）→ 二次确认 Modal → 事务删除本地消息 + 会话行（幂等，不影响对方/好友关系） |
| 删除好友 | 联系人右键菜单；保留聊天记录，公钥随行移除，可重新添加 |
| 在线状态 | 好友头像绿点/灰点角标 + 离线置灰；聊天头部「对方在线/离线」 |
| 会话选中态 | 飞书式左侧主色指示条 + 名称高亮 |
| 系统通知 | 后台/非当前会话时原生通知；**点击通知唤起窗口并跳转到发送者会话**；好友申请等生命周期事件由 Rust 直接触发 |
| 桌面托盘 | **点 X 隐藏窗口驻留托盘**（不退出进程，后台继续收发）；托盘菜单「显示主窗口 / 退出」，**只有退出才结束进程**；单击托盘图标恢复；macOS Dock 点击恢复（`RunEvent::Reopen`） |
| E2EE 徽标 | 聊天窗口顶部恒显**绿锁**「端到端加密」 |
| 外观设置 | 主题色 / 字体（CSS 变量动态注入）/ 深色模式 / 网卡选择，即点即存 |
| 缓存管理 | 保留时长（3/7/30 天/永久）+ 磁盘配额自动清理 + VACUUM；「立即清理」 |

### 2.3 安全

| 功能 | 说明 |
|---|---|
| E2EE 恒开 | 单聊 X25519 静态 ECDH + ChaCha20-Poly1305；群聊群密钥；Gossip 信封 Ed25519 验签（防伪造/防篡改） |
| 公钥同步 | announce / FriendAccept / 好友建链路径自动同步公钥并持久化到 friends 表 |
| 公钥缺失兜底 | 发送时查 friends → peers → 主动 `who_has` 探测等 1.2s 重试 → 仍缺则报明确错误 |
| 解密失败兜底 | 收到 `enc1:` 解密失败（缺公钥/公钥已更新）→ 写入系统消息提示，**不静默丢消息** |
| 设备指纹 | 前缀 `gosslan-`（桌面 machine-uid，移动端 UUID/主机名兜底）；私钥持久化本地 SQLite，重启身份不变 |
| 隐私 | 数据只存本机；SQLite 不存 BLOB（图片/文件落 Cache 目录）；共享目录路径规范化校验 |

---

## 3. 目录结构与代码导读

```
gosslan/
├── AI_RULES.md              # ★ AI 工程宪法（不变量/禁止事项/开发流程）
├── AI_PROJECT_HANDOFF.md    # ★ 本文件（项目全景与代码导读）
├── CHANGELOG.md             # 版本历史
├── .github/workflows/
│   ├── build.yml            # Windows x64（NSIS exe）
│   ├── build-macos.yml      # macOS 通用包（universal dmg）
│   └── build-android.yml    # Android APK（4 ABI）
├── package.json             # npm 脚本
├── scripts/
│   ├── version.mjs          # 版本号统一维护（同步 4 处 + CHANGELOG [Unreleased] 落日志）
│   └── android/…            # Android 权限模板（CI 注入）
├── docs/
│   ├── AI_ENGINEERING_INDEX.md  # ★ 约束文档导航 + 文档/代码冲突处理规则
│   ├── protocol-invariants.md   # ★ 协议不变量明细 INV-P01~P18 + 测试矩阵
│   ├── acceptance/              # 版本验收标准（当前：1.0-release.md）
│   ├── adr/                     # 架构决策记录
│   │   ├── 0007-protocol-versioning.md
│   │   ├── 0008-state-machine-boundaries.md
│   │   ├── 0009-rust-typescript-contract.md
│   │   └── 0010-failure-injection-testing.md
│   └── templates/               # ADR.md / BUG_FIX.md
├── src/                     # Vue 3 前端
│   ├── types.ts             # 与 Rust serde 结构一一对应的类型
│   ├── api/index.ts         # Tauri invoke封装 + 全部事件监听（bindEvents）
│   ├── stores/
│   │   ├── useAppStore.ts   # 设备/主题/深色/聊天样式/对端样式/toast/响应式
│   │   └── useChatStore.ts  # 好友/会话/消息合并/发送/回执/文件传输/通知（核心 store）
│   ├── utils/
│   │   ├── messages.ts      # 纯函数：mergeMessages 去重排序 / 会话未读统计 / preview
│   │   ├── chatStyle.ts     # 聊天样式预设解析
│   │   └── color.ts / cn.ts
│   ├── layouts/ResponsiveLayout.vue
│   └── components/
│       ├── ChatWindow.vue        # 聊天区（头部徽标/消息列表/输入区）
│       ├── MessageItem.vue       # 单条消息（气泡/回执/时间/复制/折叠）
│       ├── ConversationList.vue  # 会话+联系人列表（删除会话 X / 右键删好友）
│       ├── VirtualList.vue       # 虚拟滚动（绝对定位 + 触顶加载）
│       ├── CodeBlock.vue / BaseModal.vue / SettingsPanel.vue
│       ├── AddFriendModal.vue / GroupCreateModal.vue / ShareDirectory.vue
│       └── NavRail.vue / TopologyBar.vue
└── src-tauri/src/
    ├── lib.rs               # Tauri Builder：插件/状态/托盘 setup/invoke_handler 注册
    ├── main.rs
    ├── commands.rs          # Tauri 命令层（40+ 命令：send_message / delete_conversation / …）
    ├── state.rs             # AppState：identity/db/peers/gossip/group_keys/outbox 等
    ├── db.rs                # SQLite 存储层（SCHEMA 常量 + 全部 CRUD + 事务）
    ├── device.rs            # 设备指纹（gosslan- 前缀；桌面 machine-uid，移动端兜底）
    ├── crypto.rs            # ★ E2EE 原语：Identity / shared_secret / seal / open / 签名验签
    ├── protocol.rs          # 线格式：UDP 包 / TCP 帧 / Message 枚举 / GossipEnvelope
    ├── gossip_engine.rs     # Bloom+LRU 去重 / fanout / 信封构建与验签
    ├── relay_manager.rs     # 大文件切片 + 并行分发 + 乱序重组
    ├── schema.sql           # 建表脚本（与 db.rs SCHEMA 保持一致，文档用）
    ├── tray.rs              # 桌面托盘（#[cfg(desktop)] 门控；CloseRequested→隐藏）
    ├── network/
    │   ├── mod.rs           # 网络启动/停止
    │   ├── discovery.rs     # UDP 广播+组播 + 自适应周期 + who_has 探测
    │   ├── transport.rs     # ★ TCP 消息处理主循环（gossip/直发/回执/群密钥/outbox 补发）
    │   └── file.rs          # 文件直传 + 共享目录枚举
    ├── transport/           # 双通道聚合抽象（Transport trait；lan 已实现，bluetooth 为接口契约）
    ├── relay/mesh_router.rs # 异构 Mesh 桥接 + TTL + RingBuffer（接口就绪待蓝牙接线）
    └── storage/cache_cleaner.rs
```

**关键端口/常量**：UDP `59991`（发现）、TCP `59992`（消息），定义在 `protocol.rs`。

---

## 4. 核心机制详解

### 4.1 发现与建链

- announce 携带 device_id / 昵称 / TCP 端口 / X25519 + Ed25519 公钥；收到即 upsert peers 表并同步 friends 表公钥。
- 每对节点由 device_id 字典序较小一方主动拨号，避免重复建链竞态。
- 在线状态：peers 表 + 心跳（5s）判定；前端 `friends.online` 由 peers-updated 事件驱动。

### 4.2 消息投递的三条路径（统一 msg_id 去重）

1. **直发** `ChatMessage`（TCP 帧，内容可能带 `enc1:` 前缀）
2. **Gossip 广播** `Gossip{envelope}`（加密载荷或明文 base64）
3. **outbox 补发**（对方上线建链 Hello / 心跳触发 `flush_outbox`）

三处共用 Gossip 信封的确定性 SHA-256 message_id；接收方 `message_exists` 按 msg_id 幂等去重，
收到重复只回 Ack 不重复入库。发送方一律写 outbox（Ack 才删），防半开 TCP 静默丢包。

### 4.3 E2EE 状态机（v0.11.0 恒开）

```
发送方 send_message:
  查对端 X25519 公钥: friends 表 → peers 表 → 主动 who_has 探测 + 等 1.2s → 再查
    ├─ 拿到 → shared = X25519(自己私钥, 对方公钥)
    │         Gossip 载荷 = base64(seal(shared, JSON{kind,content}))
    │         直发内容  = "enc1:" + base64(seal(shared, content))
    │         信封 encrypted = true（build_envelope 默认）
    └─ 没有 → Err("尚未获取 {id} 的公钥：对方可能离线或处于不同子网…")

接收方 handle ChatMessage:
  content 有 "enc1:" 前缀?
    ├─ 查发送方公钥: friends 表 → peers 表
    │   ├─ 解密成功 → 明文落库并回 Ack
    │   └─ 失败/缺公钥 → 不落库、不 Ack；outbox 保留，等待用最新公钥重封后补发
    └─ 无前缀 → 当前版本直接拒绝（E2EE 恒开）

接收方 handle Gossip:
  env.encrypted == false → base64 明文
  env.encrypted == true  → Chat: X25519(自己私钥, env.sender_pubkey) 解密
                            Group: group_id 群密钥对称解密
```

要点：
- **接收方解密只需要发送方公钥**（信封自带 `sender_pubkey`），与本机是否「开启加密」无关——这就是 v0.11.0 恒开后不存在兼容性问题的原因。
- 群密钥：创建群时生成随机 32B key，持久化 `settings` 表 `gk:<group_id>`；`GroupKey` 消息用各成员公钥 ECDH 加密分发；`distribute_group_key` 可补发（新成员上线）。
- 私钥以 base64 存 `settings` 表（`x25519_secret` / `ed25519_secret`），重启身份不变。

### 4.4 已读回执链路

1. 触发标记已读（三处）：打开会话 / 会话内收到新消息（防抖 600ms）/ 窗口重新可见
2. Rust 查该会话里「对方最近一条消息」的 `msg_id + seq` → 发 `ReadReceipt{last_read_msg_id, last_read_ts}`
3. 链路不可用（未建链 / 半开）⇒ 回执暂存 `pending_reads` / `pending_group_reads`，
   由建链 / Hello / 心跳补发 —— 与 outbox 补发同一批触发点
4. 对方收到 → 用 `last_read_msg_id` 换算出自己的本地 `seq`，把 ts ≤ 该序号的消息全部置 `read` → **绿勾**
5. 状态流转：`sending`（转圈）→ `delivered`（Ack 到达，空心圆）→ `read`（绿勾）；失败 = 红叉
6. **送达状态只前进不回退**：outbox 补发会带回迟到的重复 Ack，`set_message_status`
   与前端 `onMessageAcked` 都必须跳过已置 `read` 的记录

### 4.5 前端消息管线

- 收到消息 → `enqueueMessage` 批量队列 → rAF（窗口可见）/ setTimeout（不可见）冲刷 →
  主线程同步合并（O(n) Set 去重 + 排序；**刻意不用 Web Worker**——WKWebView 生产构建下 Worker 可能加载失败导致消息全部卡住，v0.5.1 教训）。
- 乐观发送：先上屏 `sending` 态（`tmp-*` msg_id）→ invoke 成功替换真实记录 → 失败置 `failed` + toast。
- **两条替换规则**（否则气泡永久停在「发送中」，而库里已是 delivered/read）：
  - 乐观记录还压在批量队列里时 `replaceMessage` 找不到目标 ⇒ 挂进 `pendingReplace`，
    由 `applyIncoming` 落地时经 `applyReplacements` 换成真实记录；
  - 会话重查（`loadMessages`）的快照可能早于 Ack / peer-read 落库 ⇒ 用
    `preserveDeliveryStatus` 与查询期间已推进的内存状态合并，不做无条件覆盖。
- 交互约定：**一切操作先假定成功、失败可感知**（删除会话/好友/发送均乐观 + 失败回滚）。

### 4.6 桌面托盘（tray.rs）

- `#[cfg(desktop)]` 门控（Android 编译不引入）；Cargo 需 `tauri` 的 `tray-icon` feature。
- `CloseRequested → api.prevent_close() + hide()`；托盘菜单「退出」才 `app.exit(0)`。
- **容错**：托盘初始化失败时不拦截关闭（保持默认退出），避免「窗口关不掉且无托盘可恢复」。
- macOS `RunEvent::Reopen`（Dock 点击、无可见窗口）同样恢复。

---

## 5. 工程约定（贡献者必读）

### 5.1 开发流程（作者明确要求）

**新功能一律：先计划 → 再设计测试 → 后实现 → 用测试验证。**
计划要简短列出任务分解与测试设计，不要直接开写代码。

### 5.2 版本与发布（每次交付缺一不可）

1. 变更先写进 `CHANGELOG.md` 的 `## [Unreleased]` 小节
2. `node scripts/version.mjs minor|patch`（新功能=minor，修复=patch；0.x 破坏性变更允许进 minor，日志标注 ⚠️）
   —— 自动同步 package.json / Cargo.toml / tauri.conf.json / package-lock，并把 `[Unreleased]` 落为带日期小节
3. `cargo check` 刷新 `Cargo.lock`（已提交，保证 CI 可复现）
4. `git tag vX.Y.Z && git push origin main && git push origin vX.Y.Z`
   —— tag 触发三端 workflow 并自动创建 GitHub Release（apk/dmg/exe 三件套）
5. push 后用 `git ls-remote origin main vX.Y.Z` 核对 main 与 tag 同 commit（沙箱 git 链式命令有假输出）

### 5.3 代码风格偏好

- 编译零错误底线；警告零容忍（新代码不得引入新警告）
- 原生 UI 组件优先，避免自定义实现；短命名；简洁注释（中文，解释「为什么」而非「是什么」）
- 第三方依赖引入前先评估体积、适配性、依赖污染成本；重构必须保持现有业务功能不变
- 前端类型与 Rust serde 结构一一对应（`src/types.ts`），camelCase invoke 参数自动转 snake_case

### 5.4 平台门控

- `#[cfg(desktop)]` / `#[cfg(mobile)]` 由 tauri-build 注入，放心使用（focus_window、托盘已验证）
- machine-uid 仅桌面目标（Cargo.toml target 门控）；Android 交叉编译检查见 §6

---

## 6. 测试与验证口径

| 层 | 命令 | 现状 |
|---|---|---|
| 前端纯函数 | `npm test`（node --test，需 Node ≥22） | 22/22 |
| 前端类型+构建 | `npm run build`（vue-tsc + vite build） | 通过 |
| Rust 单测 | `cd src-tauri && cargo test --lib` | 35/35（crypto/gossip/db/协议/分片） |
| 双端编译 | `cargo check` + `cargo check --target aarch64-linux-android` | 0 error |
| 协议级 E2E | `src-tauri/examples/e2e_peer.rs`（无 GUI 直连真实实例，15+ 断言） | 手动跑 |
| 全功能 dev 验证 | `scripts/e2e-dev.sh`（单机双实例，29 项断言） | 手动跑 |
| 桌面冒烟 | `npx tauri build --debug` → 启动 .app 验证托盘/启动日志 | 手动跑 |

Android 交叉编译检查（无需 Gradle，快速验证代码能否过 Android 编译）：

```bash
export NDK=$HOME/Library/Android/sdk/ndk/27.1.12297006
BIN=$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin
export CC_aarch64_linux_android=$BIN/aarch64-linux-android21-clang
export AR_aarch64_linux_android=$BIN/llvm-ar
cargo check --target aarch64-linux-android
```

---

## 7. 历史演进时间线（版本 → 核心内容）

| 版本 | 日期 | 核心内容 |
|---|---|---|
| 0.1.0 | 09-03 | 基础 P2P：UDP 发现、TCP 分帧、好友、群聊、文件直传、共享目录 |
| 0.2.0 | 09-04 | E2EE（X25519/Ed25519/ChaCha20）、Gossip、大文件切片中继、群密钥、响应式布局、改名 Lanct→Gosslan、UI 换 Tailwind+Headless UI+Lucide |
| 0.3.0 | 09-04 | 按需 who_has 探测、系统通知、Transport 抽象（lan/bluetooth）、Mesh 中继路由、缓存清理、`--instance` 多开 |
| 0.3.1 | 09-04 | **修复 10 个 Rust 编译错误**（CI 此前 100% 失败的根因）、macOS workflow、machine-uid Android 门控 |
| 0.4.x | 09-04/05 | 统一文件自动路由、离线 outbox 激活、心跳探活、分页加载、会话选中态、e2e_peer 协议级验证工具、UDP 阻塞 socket 修复、SO_REUSEPORT 多开修复、统一 msg_id |
| 0.5.x | 09-05 | 虚拟滚动定位/未读分割线、长文本折叠、聊天样式跨设备同步、删除好友、**移除 Web Worker（Mac 消息不刷新根因）**、发送失败 toast |
| 0.6.0 | 09-05 | 消息回执链路（Ack + ReadReceipt + 绿勾）、在线状态角标、乐观交互推广、输入区重排、**确立发布三件套约定** |
| 0.7.0 | 09-05 | 代码块升级（语言检测/折叠）、**outbox 可靠性修复（Mac→Windows 丢消息根因：一律入队 + Ack 才删）**、乐观时间戳钳制、rAF 后台滞留修复、协议对标文档 |
| 0.8.0 | 09-05 | E2EE 开关（默认关）+ 直发链路加密统一（`enc1:`）+ 信封 encrypted 标志、**设备指纹前缀改 `gosslan-`（破坏性）**、设置页指纹显示 |
| 0.9.0 | 09-05 | **系统托盘：关窗驻留、仅托盘退出**（tray.rs，cfg(desktop)） |
| 0.10.0 | 09-05 | **删除聊天记录**（X 按钮 + 二次确认）、回执移到气泡左侧、时间 MM-DD HH:mm、移除右键复制菜单、**E2EE 健壮性**（公钥探测重试 / 解密失败写系统消息 / E2EE 关时不查公钥） |
| 0.11.0 | 09-05 | **E2EE 恒开且不可关闭**：移除开关，绿锁恒显，设置页只留说明 |

---

## 8. 演进设想（未来方向）

> ⚠️ **以下均为 v1.0 后规划方向，不要在当前阶段主动实现。**
> 规划范围见 [AI_RULES.md](AI_RULES.md) 与 [docs/acceptance/1.0-release.md](docs/acceptance/1.0-release.md)；
> 这里仅保留摘要与产品层面的记录。

### 8.1 协议安全（P0–P1）

1. **Noise XX 会话**（`snow` crate）：当前静态 X25519 派生长期密钥，无前向保密；
   升级为建链握手派生会话密钥是安全收益最大的一步（TCP 通道先行，蓝牙直接复用会话层）
2. **好友指纹安全码 / QR 当面校验**：添加好友完成页展示指纹比对，防中间人（TOFU 增强）
3. **mDNS 第三发现通道**：覆盖跨子网 / 隔离广播域（BeeBEEP 实践）

### 8.2 传输扩展（P2）

4. **BLE 无配对通道**：`transport/bluetooth.rs` 接口契约已就位（btleplug），每设备同时
   GATT Central + Peripheral；需要通用分片协议（MTU 限制）+ 二进制帧头 + log₂(degree) fanout
5. **Store-and-Forward 多跳暂存**：`relay/mesh_router.rs` 的 RingBuffer 已就绪待接线
6. **通用分片协议**：消息级 (frag_id, seq, total)，蓝牙前置

### 8.3 产品方向

7. **QUIC 传输**：`transport.rs` 分帧读写两原语可替换为 `quinn`，消息层零改动
8. **服务端中继（可选）**：JSON 帧协议可跑 WebSocket，轻量中继打通跨网段 / 公网（电脑↔手机）
9. **E2EE 部分-加密场景**：恒开后旧版本（<0.10.0）对端收到 `enc1:` 会静默丢弃——如果仍需
   支持旧对端，考虑升级提示；另可做「密钥轮换」（设备重装后指纹不变但公钥变化的场景提示）
10. **账号体系（远期）**：device_id 之上叠加账号绑定与多设备同步（当前设计刻意无账号）
11. **UI 细化**：消息引用/回复、表情回应、群成员管理、深色模式下图片预览优化

### 8.4 已知限制（fork 者注意）

- 公钥缺失时无法发送（E2EE 恒开的固有代价）：从未上线过的好友无法收到消息；错误提示已做兜底
- `enc1:` 解密依赖「发送方公钥」，对方重装应用（私钥重建、公钥变化）后，我方需等对方重新
  announce 才能解密——期间消息会显示为系统提示（不丢失，但看不到内容）
- Android 侧 `transport/bluetooth.rs` 仅为接口契约，未接线
- Web Worker 已移除（WKWebView 兼容问题），大批量消息合并在主线程（实测微秒级，无忧）

---

## 9. 常用命令速查

```bash
# 开发
npm run dev                  # 仅前端 vite
npm run tauri dev            # 桌面调试（需 Rust）
npm run android:dev          # Android 真机调试

# 测试
npm test                                        # 前端 22 用例
cd src-tauri && cargo test --lib                # Rust 35 用例
cargo check --target aarch64-linux-android      # Android 编译检查（env 见 §6）

# 版本与发布（三件套约定，见 §5.2）
npm run version:show / patch / minor / major
git tag vX.Y.Z && git push origin main && git push origin vX.Y.Z

# 打包（★ 首选：一键出包，按当前平台自动决定打什么）
npm run dist                    # macOS ⇒ 安卓 + mac **并行**；Windows ⇒ 只出当前环境的 win 包
npm run dist -- --dry-run       # 只打印命令与环境
npm run dist -- --dmg           # mac 额外出 DMG（默认只出 .app + zip）
npm run dist -- --all-abis      # 安卓两个 ABI（默认只 arm64-v8a）
npm run dist -- --fat-lto       # 发布级 LTO（仓库默认配置；不加则用 thin LTO，编译快 2~3 倍）
npm run dist -- --serial        # 串行 + 共用旧 target 目录
# 「mac 上必须同时出安卓包和 mac 包、且并行；Windows 上只出当前环境的包」是用户硬要求，
# 实现在 scripts/package.mjs（头部注释逐条写了 5 个提速点）。

# 打包（单平台细粒度命令，CI / 特殊场景用）
npm run dist:win                # Windows NSIS（需在 Windows 上）
npm run dist:mac:app            # macOS .app + zip
npm run tauri -- build --target universal-apple-darwin   # macOS universal
npm run android:build:release   # Android release APK（走 build-android-releases.sh 的校验）
npm run dist:win:portable       # Windows 便携版 zip（Windows）
npm run multi:run               # 单机多开 3 实例模拟多节点（Windows）
npm run env:check / env:install # Windows 环境检查/安装（PowerShell）

# 协议级验证
cd src-tauri && cargo build --example e2e_peer
GOSSLAN_AUTOSTART=1 ./target/debug/gosslan &    # headless 启动实例
# 另起 e2e_peer 对连（详见 examples/e2e_peer.rs 顶部说明）
```

CI：push `main` / push `v*` tag / 手动触发。tag 额外发布 Release。
产物：Windows NSIS exe、macOS universal dmg、Android 4-ABI APK。

---

## 10. 本机开发环境备忘（原作者 macOS 环境，fork 者可跳过）

1. npm 命令在 WorkBuddy 沙箱需前缀：`env -u NODE_OPTIONS -u CODEBUDDY_BROKERED_FS_HOOK_ENABLED`
2. rustup stable（`source "$HOME/.cargo/env"`）；4 个 android target 已装
3. Android SDK：`~/Library/Android/sdk`，NDK 27.1（CI 用 26.3）；JDK 21（CI 用 17）
4. `src-tauri/gen/android/` 不入库（CI 每次 `tauri android init` 重新生成）
5. git remote 走 SSH；提交身份 fuwedong / fuwendong5@outlook.com
6. CI 状态匿名可查：`https://api.github.com/repos/fwd001/gosslan/actions/runs`（日志下载需权限）
7. 匿名 GitHub API 限流 60/h，超限时用 WebFetch 读网页版 releases 页

---

## 11. 文档索引

| 文档 | 内容 |
|---|---|
| `AI_RULES.md` | **★ AI 工程宪法**（41 章）：不变量 INV-001~008、任务分级 L1/L2/L3、协议/DB/加密规则、Bug 修复流程、冻结功能清单、Definition of Done |
| `docs/acceptance/1.0-release.md` | **★ 当前版本验收标准**：P0/P1 清单、开发策略、必跑验证命令 |
| `AI_PROJECT_HANDOFF.md` | **本文件**：给 AI 编程/源码阅读/fork 者的完整上下文 |
| `docs/protocol-invariants.md` | **协议不变量明细** INV-P01~P18 + 必覆盖测试矩阵 |
| `docs/AI_ENGINEERING_INDEX.md` | 约束文档导航 + 文档与代码冲突时的处理规则 |
| **`docs/architecture/`** | **★ V5 目标架构规范（README + 01–07）** |
| **`docs/adr/0020`–`0025`** | **★ V5 决策：分层/单模型/无感融合/中继/跨平台+iOS/仿真** |
| `README.md` | 项目门面：功能特性、快速开始、架构简介、**AI 约束文档索引** |
| `CHANGELOG.md` | 全部版本历史（每版 Added/Fixed/Changed 明细） |
| `docs/adr/` | **架构决策记录**：协议版本化 / 状态机边界 / Rust-TS 契约 / 故障注入测试 |
| `docs/templates/` | `BUG_FIX.md`（修复报告模板）、`ADR.md`（决策记录模板） |
| `src-tauri/src/schema.sql` | 数据库 Schema（与 db.rs SCHEMA 一致） |
