# Gosslan · 无服务器 P2P 局域网即时通讯

一个**没有中央服务器**的局域网即时通讯软件，界面高度模仿飞书 / 钉钉。
数据**只存本机**（SQLite），节点间通过 **UDP 组播/广播发现 + TCP 点对点传输**直接通信，
**端到端加密强制开启**，无需注册登录，用**设备指纹**识别同一用户，连上局域网即自动同步用户资料。
支持 **Windows / macOS / Android** 三端。

> 技术栈：Tauri v2 · Rust · Vue 3 (Composition API) · TypeScript · Vite · **Tailwind CSS · Headless UI · Lucide 图标** · SQLite

---

## 🤖 AI 开发必读（约束文档索引）

> **任何 AI 编程助手在修改本项目代码前，必须先阅读以下文档。**
> 这些文档定义了项目的工程宪法、不变量、禁止事项和开发流程。

| 优先级 | 文档 | 内容 |
|---|---|---|
| **★★★ 必读** | [AI_RULES.md](AI_RULES.md) | **AI 工程宪法**：核心不变量（INV-001~008）、禁止事项（P-01~P-20）、强制开发流程（7 阶段）、协议变更规则、状态机规范、Bug 修复规范 |
| **★★★ 必读** | [AI_PROJECT_HANDOFF.md](AI_PROJECT_HANDOFF.md) | **项目全景**：完整功能清单、架构与代码导读、E2EE 状态机、工程约定、测试口径、演进设想 |
| **★★ 参考** | [docs/adr/](docs/adr/) | **架构决策记录**：为什么这么设计（消息幂等、Outbox+Ack、E2EE 选型、Transport 抽象、禁用 Web Worker、设备指纹） |
| **★★ 参考** | [CHANGELOG.md](CHANGELOG.md) | **版本历史**：过去每个版本改了什么、为什么改（含所有已修 bug 的根因） |
| **★ 按需** | [docs/protocol-design.md](docs/protocol-design.md) | 协议对标分析（BeeBEEP/bitchat）与演进路线 |
| **★ 按需** | [docs/performance.md](docs/performance.md) | 500–1000 节点性能设计 |
| **★ 按需** | [docs/setup-windows.md](docs/setup-windows.md) | Windows/Android 环境配置与打包 |

**阅读顺序建议**：`AI_RULES.md`（了解约束）→ `AI_PROJECT_HANDOFF.md`（了解项目）→ 相关 ADR（了解设计原因）→ 代码。

---

## ✨ 功能特性

| 模块 | 能力 |
| --- | --- |
| **界面** | 三栏布局（左导航 / 中会话列表 / 右聊天区），**响应式**（PC 三栏 + 移动端单栏滑动切换），自定义主题色与字体（CSS 变量动态注入），**深色模式**，网卡选择下拉框 |
| **发现与好友** | UDP **广播 + 组播**双通道；**按需 `who_has` 探测**（打开「添加好友」才扫描，启动不持续扫描）；昵称/IP 实时过滤、已是好友显示禁用态；发送申请 → 对方弹窗确认 → 双方互存本地 SQLite；好友头像在线/离线角标，右键删除好友（保留聊天记录） |
| **E2EE（强制）** | **恒开且不可关闭**：**X25519**（ECDH 密钥交换）+ **Ed25519**（签名/身份校验）+ **ChaCha20-Poly1305**（AEAD 加密）；聊天窗口顶部绿锁徽标；公钥自动同步 + 缺失时自动探测，解密失败以系统消息提示（不丢消息） |
| **消息分发** | **Gossip 广播**（Epidemic 泛洪，Bloom Filter + LRU 去重，TTL 衰减）；单聊点对点加密、群聊群密钥加密 |
| **消息可靠性** | 直发 + Gossip + 离线补发三路径统一 msg_id 幂等去重；消息**一律写离线队列**、Ack 到达才删行，防半开连接静默丢包 |
| **消息回执** | 转圈=发送中 → 空心圆=已送达未读 → **绿勾=已读**（挂在我方气泡左侧）；失败红叉；打开会话/窗口聚焦自动回执 |
| **离线兜底** | 对方离线时消息自动暂存，上线建链后**自动补发** |
| **大文件** | **BitTorrent 式切片中继**：64KB~512KB 分片，并行分发到空闲节点二次转发，接收方乱序重组；直连/中继**自动路由** |
| **消息** | 文本 / 长文本（>280 字折叠）/ 代码（`highlight.js` 高亮 + 折叠）/ 图片（粘贴板直发）/ 文件（内嵌进度条）/ 系统消息；气泡悬停一键复制 |
| **聊天体验** | 虚拟滚动 + 触顶分页加载历史 + 未读定位分割线 +「回到最新」；连续消息合并与时间分割线；消息时间 `MM-DD HH:mm`（悬停看秒级）；6 套聊天配色预设 + 字号（**跨设备同步**：对方按我的配色渲染我的消息）；长文本折叠；**删除聊天记录**（列表项右下角 ×，二次确认） |
| **通知** | 应用处于**后台或非当前会话**时触发系统原生通知，**点击通知唤起窗口、跳转到发送者会话并清零未读**（Windows / macOS / Android） |
| **系统托盘** | 点击窗口「×」**最小化到托盘**（后台继续收发消息与通知），托盘菜单「显示主窗口 / 退出」——**只有选择退出才真正结束进程**（桌面端） |
| **双通道** | 局域网 + 蓝牙**双通道聚合**架构：`Transport` 抽象接口、独立开关、按负载智能分流（蓝牙通道接口就绪待接线） |
| **中继路由** | 异构 **Mesh 桥接**（局域网 ↔ 蓝牙跨链路转发）+ TTL 衰减 + 有界 RingBuffer 限流，节点降压保护 |
| **存储** | SQLite 只存文本 / 密钥 / 关系（**不存 BLOB**），图片/文件落 `Cache` 目录懒加载；**自动缓存清理**（3/7/30 天/永久 + 磁盘配额）+ VACUUM 整理 |
| **共享目录** | 设置本地共享文件夹，好友点对点浏览目录树并下载文件（防目录穿越） |
| **多开压测** | `--instance N` 单机多开（独立数据库/TCP 端口/指纹，UDP 共享），模拟多节点 Mesh |

---

## 📁 项目结构

```
gosslan/
├── index.html
├── package.json
├── vite.config.ts
├── tailwind.config.js        # darkMode: class + 主题色变量
├── AI_RULES.md               # ★ AI 工程宪法（不变量/禁止事项/开发流程）
├── AI_PROJECT_HANDOFF.md     # ★ 项目全景与代码导读
├── CHANGELOG.md              # 版本历史
├── docs/
│   ├── adr/                  # 架构决策记录（为什么这么设计）
│   ├── protocol-design.md    # 协议对标与演进路线
│   ├── performance.md        # 大规模节点性能设计
│   └── setup-windows.md      # Windows/Android 环境配置
├── src/                      # Vue 3 前端
│   ├── main.ts
│   ├── App.vue
│   ├── style.css             # CSS 变量（亮/暗）+ Tailwind
│   ├── types.ts
│   ├── api/index.ts          # Tauri invoke 封装 + 事件监听
│   ├── utils/{cn.ts,color.ts}
│   ├── stores/
│   │   ├── useAppStore.ts    # 设备 / 主题 / 深色 / 聊天样式 / 响应式
│   │   └── useChatStore.ts   # 好友 / 会话 / 消息队列 / 回执 / 通知
│   ├── layouts/ResponsiveLayout.vue # 三栏 + 移动端单栏
│   └── components/
│       ├── NavRail.vue / ConversationList.vue / ChatWindow.vue
│       ├── MessageItem.vue / CodeBlock.vue / VirtualList.vue
│       ├── TopologyBar.vue / BaseModal.vue
│       └── ShareDirectory.vue / SettingsPanel.vue / AddFriendModal.vue / GroupCreateModal.vue
└── src-tauri/                # Rust 后端
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── capabilities/default.json
    └── src/
        ├── main.rs / lib.rs
        ├── tray.rs           # 桌面托盘：关窗驻留、仅托盘退出（#[cfg(desktop)]）
        ├── device.rs         # 设备指纹（gosslan- 前缀；桌面 machine-uid）
        ├── crypto.rs         # E2EE：X25519 + Ed25519 + ChaCha20-Poly1305
        ├── gossip_engine.rs  # Gossip 广播 + Bloom/LRU 去重 + 扇出
        ├── relay_manager.rs  # 大文件切片 + 并行分发 + 重组
        ├── db.rs             # SQLite 存储层 + Schema
        ├── protocol.rs       # 线格式（UDP 包 / TCP 帧 / 消息枚举 / Gossip 信封）
        ├── state.rs          # AppState 全局状态
        ├── schema.sql        # 创表脚本（文档用）
        ├── commands.rs       # Tauri 命令层
        ├── network/
        │   ├── mod.rs        # 网络启动 / 停止
        │   ├── discovery.rs  # UDP 广播+组播发现 + 多网卡选择 + 按需探测
        │   ├── transport.rs  # TCP 传输 + Gossip/中继/群密钥分发
        │   └── file.rs       # 文件直传 + 共享目录枚举
        ├── transport/        # 双通道聚合传输抽象
        │   ├── mod.rs        # Transport trait + TransportManager + 分流
        │   ├── lan.rs        # 局域网通道适配
        │   └── bluetooth.rs  # 蓝牙通道（BLE/RFCOMM 接口契约）
        ├── relay/
        │   └── mesh_router.rs # 异构 Mesh 桥接 + TTL + RingBuffer
        └── storage/
            └── cache_cleaner.rs # 缓存清理 + VACUUM
```

---

## 🚀 快速开始

### 环境要求

- **Node.js ≥ 18**（推荐 20+）
- **Rust ≥ 1.77**（[rustup](https://rustup.rs) 安装，MSVC 工具链）
- **Windows**：需安装 [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) 与 [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)（Win10/11 已内置）
- **Linux**：`sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev libgtk-3-dev`
- **macOS**：Xcode Command Line Tools

### 安装依赖

```bash
npm install
```

### 生成图标（首次打包前执行一次）

```bash
npm run tauri icon src-tauri/icons/icon.png
```

### 开发调试

```bash
npm run tauri dev
```

> 局域网联调建议使用两台真实电脑 / 虚拟机，且处于**同一网段**。双方启动网络后会自动互相发现。

### 跨平台（Windows + Android）环境配置与多开

完整的环境安装步骤、Android 权限清单与打包命令见 **[docs/setup-windows.md](docs/setup-windows.md)**，常用命令：

```bash
npm run env:check          # 一键检查编译环境（Rust/MSVC/JDK/SDK/NDK/target，Windows）
npm run env:install        # 一键安装（MSVC + Rust + JDK17 + 镜像 + target，需管理员，Windows）

npm run android:init       # 生成 Android 工程（首次）
npm run android:build      # Release APK（4 ABI，用于分发压测）
npm run android:build:debug    # Debug APK（本机 + 手机直连调试）

npm run dist:win:portable  # Windows 便携版 → gosslan_<版本>_x64-portable.zip（Windows）
npm run multi:run          # 同机多开 3 个实例模拟多节点（--instance N，Windows）
```

- **多开原理**：`--instance N` 让每个实例使用独立数据库、独立 TCP 端口、独立设备指纹；UDP 端口共享（`SO_REUSEADDR`，unix 上叠加 `SO_REUSEPORT`），从而在单机模拟多节点 Mesh 与离线补发。
- **Android 权限**：局域网（`CHANGE_WIFI_MULTICAST_STATE` 等）+ 蓝牙（`BLUETOOTH_SCAN/CONNECT/ADVERTISE` 等）+ 前台服务，模板见 `scripts/android/AndroidManifest.xml`，CI 自动注入。

### 生产打包（Windows x64）

先按改动量维护版本号（一次 bump 同步 `package.json` / `Cargo.toml` / `tauri.conf.json` 三处）：

```bash
npm run version:show    # 查看当前版本
npm run version:patch   # 补丁版本（bug 修复与小改动）
npm run version:minor   # 次版本（新功能，向下兼容）
npm run version:major   # 主版本（破坏性 / 架构级变更）
```

打包 Windows x64 安装包：

```bash
npm run icon            # 首次打包前生成图标集（本仓库已生成）
npm run dist:win        # NSIS 安装包 → gosslan_<版本>_x64-setup.exe
npm run dist:win:msi    # 额外产出 MSI
```

产物位于 `src-tauri/target/release/bundle/nsis/`，文件名形如 `gosslan_<版本>_x64-setup.exe`。

### 用 GitHub Actions 自动出包（无需本地装 Rust/MSVC）

仓库内置三端 workflow（Windows / macOS / Android），推送一个 `v*` 标签即自动构建三端安装包并发布 Release（apk/dmg/exe 三件套）：

```bash
npm run version:patch                 # 例：0.11.0 -> 0.11.1（同步 4 处版本 + CHANGELOG）
git add -A && git commit -m "release v0.11.1"
git tag v0.11.1
git push origin main && git push origin v0.11.1
```

> GitHub 托管 runner 已预装 Rust / Node / JDK / Android SDK，无需本地环境即可出包。
> 版本发布约定与 CHANGELOG 维护规范详见 [AI_PROJECT_HANDOFF.md §5](AI_PROJECT_HANDOFF.md)。

---

## 🧪 测试

### 前端（node 内置测试运行器，零额外依赖，需 Node ≥ 22）

```bash
npm test
```

覆盖核心纯函数（共 22 个用例）：Gossip 消息去重合并、会话未读统计与排序、主题色派生、文件大小格式化、类名合成。

### 后端（Rust）

```bash
cd src-tauri && cargo test
```

覆盖：E2EE 加解密（X25519 密钥交换 / Ed25519 签名 / ChaCha20-Poly1305）、Gossip 去重与信封签名校验、文件切片乱序重组、SQLite 存储层（好友/消息/离线队列/群组/**删除会话**）、协议 JSON 往返与 TCP 分帧。

此外还有协议级验证工具：`src-tauri/examples/e2e_peer.rs`（无 GUI 对连真实实例，覆盖发现/建链/加密/文件/离线补发）与 `scripts/e2e-dev.sh`（单机双实例全功能验证），详见 [AI_PROJECT_HANDOFF.md §6](AI_PROJECT_HANDOFF.md)。

---

## 🛰️ 架构与协议

### 设备发现（UDP :59991）

- 周期向局域网**广播**（255.255.255.255）与**组播**（239.255.42.99）一次 `announce`，
  携带设备 ID、昵称、TCP 端口、X25519/Ed25519 公钥。
- 广播周期**自适应**：<100 节点 5s、≥100 节点 10s、≥500 节点 20s，并叠加 0–2s 抖动，
  避免 500–1000 节点时的 UDP 风暴与同步惊群（详见 `docs/performance.md`）。
- **按需探测**：打开「添加好友」时，前端调用 `search_nearby_peers` → Rust 群发一次 `who_has`，
  其它节点单播回复各自 `announce`，约 1.5s 内收集在线节点；启动/日常不持续全网扫描。

### 消息传输（TCP :59992）

- 帧格式：`4 字节大端长度 + JSON 负载`。
- 建链规则：每对节点由 **device_id 字典序较小** 的一方主动拨号，避免重复建链竞态。
- 消息枚举（`protocol.rs`）：`Hello / Heartbeat / UserInfo / FriendRequest / FriendAccept /
  FriendReject / ChatMessage / GroupMessage / Ack / FileOffer / FileAccept / FileChunk / FileDone /
  ShareTreeRequest / ShareTreeResponse / ShareFileRequest / Gossip / RelayFileOffer / RelayChunk / GroupKey`。

### 端到端加密（crypto.rs，强制开启）

- 单聊：发送方用 **X25519(自己私钥, 对方公钥)** 派生共享密钥，`ChaCha20-Poly1305` 加密载荷；
  接收方用 `X25519(自己私钥, 信封中对方公钥)` 派生同一密钥解密。中继节点只能透传密文。
  直发内容带 `enc1:` 前缀标识；解密失败（缺公钥/公钥已更新）写入系统消息提示，不静默丢弃。
- 群聊：创建群时生成随机**群密钥**，用各成员公钥 ECDH 加密后分发（`GroupKey`），消息用群密钥对称加密。
- 签名：每条 Gossip 信封对 `message_id` 做 **Ed25519** 签名，接收方验签防伪造。
- 公钥缺失兜底：发送前自动查好友表 → 在线节点表 → 主动 `who_has` 探测重试；仍缺失则返回明确错误提示。

### Gossip 广播（gossip_engine.rs）

- 信封 `message_id` = SHA-256(sender_id + ts + payload)；**Bloom Filter**（概率去重）+ **LRU**（精确去重）。
- 接收新消息后向随机选取的 `fanout` 个邻居转发（TTL 衰减），实现全网覆盖。

### 大文件中继（relay_manager.rs）

- `RelayFileOffer → RelayChunk(seq, ttl) → 重组`。发送方把文件切片按轮询分配给接收方 + 空闲中继节点，
  中继节点二次转发，接收方按 `seq` 乱序重组落盘。

### 离线补发

- 消息**一律**写入 `outbox` 表（INSERT OR IGNORE 按 msg_id 幂等，防半开 TCP 静默丢包）；
  收到对方 Ack 才删行；对方上线（Hello / 心跳）触发 `flush_outbox` 自动补发，接收方以 `msg_id` 去重。
- 直发 / Gossip / 离线补发三路径共用同一确定性 message_id，跨路径幂等去重。

### 系统通知与路由跳转

- 收到消息且**应用处于后台**或**当前会话非发送者**时，前端用 `@tauri-apps/plugin-notification` 触发原生通知（标题=昵称，正文=文本截断 / `[图片]` / `[代码]` / `[文件]`）。
- 通过 `onAction` 监听通知点击：调用 Rust `focus_window`（unminimize + show + set_focus）唤起窗口，随后 `openConversation(conv_id)` 定位会话并清零未读。
- 好友申请 / 通过等生命周期事件仍由 Rust 直接发通知（无需会话上下文）。

### 桌面托盘（tray.rs）

- 点击窗口「×」→ `prevent_close` + 隐藏窗口，进程驻留托盘继续收发消息与通知；
  托盘菜单「显示主窗口 / 退出」，**只有退出才结束进程**；单击托盘图标 / macOS Dock 点击同样恢复窗口。

---

## 🧭 后续路线图（已预留扩展点）

完整对标分析与优先级见 **[docs/protocol-design.md](docs/protocol-design.md)**，要点：

- **前向保密（P0）**：静态 X25519 派生长期密钥 → 升级 Noise XX 会话（`snow`），建链握手派生会话密钥
- **好友指纹安全码 / QR 校验（P1）**：添加好友完成页当面核对指纹，防中间人
- **mDNS 第三发现通道（P1）**：覆盖跨子网 / 隔离广播域
- **蓝牙无配对通道（P2）**：`transport/bluetooth.rs` 接口契约已就位（btleplug），需通用分片协议
- **QUIC 切换**：`transport.rs` 的"分帧读写 + 建链"两原语可替换为 `quinn`，消息分发逻辑无需改动
- **服务端中转（可选）**：JSON 帧协议天然可跑在 WebSocket 上，起一个轻量中继服务即可打通跨网段
- **登录 / 账号体系（远期）**：当前用设备指纹识别用户，未来可在 `device_id` 之上叠加账号绑定，多设备同步

---

## 🔒 隐私与安全

- 所有数据仅存本机 SQLite，无任何服务器。
- 端到端加密，中继节点无法解密消息内容。
- 共享目录访问做了路径规范化校验，杜绝目录穿越。
- 如需更强的元数据隐藏，可将单聊的接收方标识从协议中移除（全节点尝试解密，仅接收方成功）。

---

## 📄 License

MIT © wd.f
