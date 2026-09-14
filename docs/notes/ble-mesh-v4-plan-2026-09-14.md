# Gosslan 蓝牙稳定性架构重构计划（对标 bitchat）

- Date: 2026-09-14
- 性质：**计划文档，本轮不改代码**（用户 2026-09-14：只梳理可执行计划）
- ⚠️ **2026-09-14 已按真实代码复核**：部分断言已修正（M3-d/端口/前后台扫描等），
  逐条证据与风险修订见配套评审 `docs/notes/ble-mesh-v4-plan-review-2026-09-14.md`。
- 目标（用户原话）：
  1. 蓝牙总是很不稳定 ⇒ 从**架构设计**层面找根因并借鉴 bitchat；
  2. **iOS + 安卓**都要适配，最好也覆盖 **mac / win**；
  3. 最终形态是 **LAN + 蓝牙融合的去中心化网络**；
  4. 蓝牙设备可以给 **BitChat 做转发节点**（**不需要兼容它的数据/协议**）；
  5. **LAN 网络不能出问题**（可以融合，但不能回归）；
  6. 现有设计不好的地方**可以大改，不考虑兼容性**。
- 取证来源：
  - **【IOS】** `github.com/permissionlesstech/bitchat` @ `9b84b36`（clone 到 `/tmp/btstudy/bitchat`）
  - **【AND】** `github.com/permissionlesstech/bitchat-android` @ `c127eb8`
  - **【本仓库】** Gosslan `src-tauri/src/**`、`docs/**`、`.workbuddy/mesh-task/**`
- 前置阅读（不重复其内容）：
  `docs/notes/bitchat-comparison.md`（协议/线格式层对照）、
  `docs/notes/ble-audit-2026-09-13.md`（4.3.x 已修清单）、
  `docs/notes/audit-2026-09-13-mesh-ble-efficiency.md`（剩余缺口 §5/§6）、
  `docs/adr/0015-ble-transport.md`（BLE 传输 ADR，含三平台外设实现）、
  `docs/adr/0017-opaque-external-wire-frame.md`（BitChat 透明中继）。

---

## 0. TL;DR（先看这一段）

**蓝牙不稳定的根因是架构，不是"还有几个 bug"。** 我们的做法是
`Arc<AppState>` + 十几把 `Mutex` + 到处 `tokio::spawn`，把
**平台射频生命周期、链路策略、mesh 协议状态、可靠消息状态**全塞进
`network/ble.rs`（2173 行）和 `network/transport.rs`（8332 行）里；
没有**单一串行状态域**，没有**链路生命周期端口**（只有帧级 `FrameSink`/`FrameSource`），没有**确定性多节点仿真**，
所以每次回归都只能"两台/三台真机试"，而且平台差异用 `#[cfg]` 散落在业务逻辑里
（Windows 曾经整段被 `cfg` 排掉，见 ADR-0015 §7.9）。

bitchat 走过同一条路并且**自己写了复盘**（`docs/BLE-ARCHITECTURE-V3.md`）：
它把 8.3k 行的 `BLEService` 拆成
**① 平台链路层（唯一 import CoreBluetooth）→ ② 单串行队列的 mesh 引擎 → ③ 特性模块 → ④ 能力协议边界**，
并把 ~60 个链路策略抽成**纯函数`struct`**，再用 `SimulatedLinkLayer` 把"两台手机才能测"
变成 ~40ms 的确定性单测。**这是我们没有的那一层。**

最该抄的三件事（按价值排序）：

| # | 借鉴 | 直击我们的哪个不稳定 |
|---|---|---|
| 1 | **单一串行 mesh 引擎 + 明确的并发所有权契约**（engine-confined / bleQueue-confined / lock-backed 三选一，debug 强制同步边序） | 散 Mutex 的 TOCTOU、死锁、状态漂移；`ble_no_dial`/`ble_dial_failures`/`handshaking` 三表手动同步 |
| 2 | **纯链路策略对象 + 确定性仿真**（扫描占空比、连接预算调度、冗余链路、扫描/广播自愈、功率档） | "总是时好时坏"无法定位、无法回归；只能真机复现 |
| 3 | **分层端口**（LinkLayer 只见射频、MeshEngine 只见帧、App 只见能力协议） | 平台 `#[cfg]` 渗透业务逻辑；两套传输抽象（`TransportManager` vs `network::ble`）并行漂移 |

**iOS 是新增平台**：当前仓库**没有任何 iOS 工程**（`src-tauri/gen/` 只有 `android`），
且需确认 btleplug 的 Apple 后端能在 iOS 上跑。好消息是 btleplug 用
`target_vendor = "apple"` 选 `corebluetooth`（`vendor/btleplug/src/platform.rs`），
**理论上能编 iOS**；而 macOS 的外设实现本就基于 `objc2-core-bluetooth` 的
`CBPeripheralManager`，**iOS 可直接复用**。详见 §5.7。

---

## 1. 现状事实基线（Gosslan）

### 1.1 代码规模与并发模型（【本仓库】）

| 文件 | 行数 | 职责 | 平台耦合 |
|---|---|---|---|
| `network/ble.rs` | 2173 | 扫描 / 拨号 / 握手 / 外设事件循环 / 读写循环 / 退避 | 10+ 处 `#[cfg(target_os=…)]` |
| `network/transport.rs` | 8332 | 消息处理、outbox、ACK、好友、群、文件、中继、OpaqueExternal | 与 BLE 强耦合 |
| `network/discovery.rs` | 967 | LAN UDP 发现（**红线，不能碰**） | 低 |
| `network/file.rs` | 2002 | 文件传输 | 中 |
| `transport/bluetooth.rs` | 640 | btleplug central 驱动 | mac/win/android |
| `transport/bluetooth_peripheral.rs` | 793 | CoreBluetooth 外设 | macOS |
| `transport/bluetooth_peripheral_windows.rs` | 689 | WinRT `GattServiceProvider` 外设 | Windows |
| `transport/ble_android.rs` | 492 | Android 外设 JNI 桥 | Android |
| `mesh/*` | ~2000 | 路由/选路/中继授权/peer/connection | 低（抽象已备好） |

**并发模型**：`Arc<AppState>` 持有 `Mutex<MeshRouter>`、`Mutex<peers>`、`Mutex<gossip>`、
`Mutex<links>`，另有 `ble_no_dial`、`ble_dial_failures`、`handshaking`、`dial_permits` 等
若干表；`network/ble.rs` 里每个候选对端 `tokio::spawn` 一个 `dial_and_register`。
**没有单一 owner，没有 mailbox/actor，没有统一的状态转移日志。**

### 1.2 平台覆盖现状

| 平台 | LAN | BLE central | BLE peripheral (GATT server) | 备注 |
|---|---|---|---|---|
| macOS | ✅ | ✅ btleplug | ✅ objc2 CoreBluetooth | 最完整 |
| Windows | ✅ | ✅ btleplug（含本地补丁） | ✅ WinRT | ADR-0015 §7.9/§7.10 |
| Android | ✅ | ✅ btleplug droidplug | ✅ Kotlin `BlePeripheral` + JNI | ADR-0015 §7.7/§7.8 |
| **iOS** | ❌ | ❌ | ❌ | **完全未支持（新增需求）** |

### 1.3 已知缺陷（来自既有审计，尚未修）

来自 `audit-2026-09-13-mesh-ble-efficiency.md` §5/§6，与本次架构直接相关：

1. **帧内不可抢占**：写循环一次发完整帧，4KiB 块 = 399 片 ≈ 5.5s，聊天帧被堵在后面（§5-1）。
2. **跨跳无补发 / 无 store-and-forward**：A→C 无直连时首帧丢 = 永久"发送中"（§5-2/3）。
3. **fanout 候选取 `peers`（含无链路节点），失败静默**（§5-5）。
4. **Gossip TTL 上限失效**：自报 ttl=255 可扩散 255 跳（§5-6，安全/放大面）。
5. ~~**M3-d 未接线**~~ **已修复（2026-09-14）**：源发 Gossip 已按路径优先级选链路
   （`network/transport.rs:309-323` 的 `best_link_kind`）；仅剩「同路径类型内按活性选路」，`try_send` 已用 `pick_link` 覆盖。
6. **BLE 拨号不占并发许可**：外设三表无上限 ⇒ 拥挤环境无界 spawn（§6-4）。
7. **`ble_no_dial` 按 BLE 地址记、只在 stop 清** ⇒ 对端换角色/地址后可能永久单侧不可拨（§6-5）。
8. **`start()` 在 `adapter()` 失败时 `?` 早退** ⇒ 外设角色根本不启动（§6-6）。
9. **stop 不清 `ble_dial_failures`** ⇒ 重开通道仍被退避挡住（§6-7）。
10. **两套传输抽象并行**：`transport::Transport` trait + `TransportManager::route`（`#[allow(dead_code)]`）
    与真正在跑的 `network::ble` 各说各话（§5-11 提到 relay 死代码同类问题）。

### 1.4 必须保护的 LAN 资产（红线）

- `network/discovery.rs`（UDP 广播 + 组播 + `who_has`）；
- `transport/tcp.rs` + `transport.rs` 的 TCP 读写循环；
- **统一链路表 `links: peer_id -> Vec<Link>`**（LAN/Routed/BLE 同表，ADR-0014）；
- **消息语义**：`Message` 线格式、E2EE（X25519 + ChaCha20-Poly1305）、Ed25519、
  outbox + ACK + read receipt、SQLite schema、文件传输；
- **选路**：`mesh/selection.rs` 的 `LAN > Routed > BLE`；
- **TTL/去重只有一份**（`MeshRouter`），不允许每 transport 各做一份（这正是目标架构的既有优势）。

> 本次重构是**传输/路由边界**的重构，**不碰**上面对应的消息层与数据库层。

---

## 2. bitchat 架构解剖

### 2.1 iOS：BLE-ARCHITECTURE-V3 的目标形态（【IOS】`docs/BLE-ARCHITECTURE-V3.md`）

它把 8.3k 行的 god object 定义为四层：

1. **`BLELinkLayer`** —— **唯一 import CoreBluetooth 的地方**。拥有两个 manager、
   扫描/广播、占空比、连接调度、MTU、写/通知背压缓冲、state restoration。
   向上只讲 `LinkEvent`（link up/down / bytes in / writable），
   向下只收 `LinkCommand`（send bytes on link / scan-advertise policy）。
   **完全不知道 packet、peer、Noise。** bleQueue 独占。
2. **Mesh engine** —— **一个串行队列拥有全部协议状态**：线编解码、分片、去重、
   中继策略、peer registry、拓扑、gossip sync、Noise 编排。同步单写者逻辑。
3. **Feature modules** —— courier/board/prekeys/private media/file/voice/groups…
   各自拥有状态、各自注册消息类型。新增功能 = 新增模块，不改引擎。
4. **App boundary** —— 小的 `Transport` 核心 + 用 `as?` 发现的 **capability 协议**
   （`MeshBridgingTransport` 等 8 个），取代 ~90 个要求的 god protocol。

**并发契约（三条所有权，二选一，不允许模糊）**：

- **Engine-confined**：只在串行引擎队列上改；跨线程调用走 `onEngine`。
- **bleQueue-confined**：紧挨 CoreBluetooth 对象的链路状态（link store、写/通知缓冲、link-auth）。
- **Lock-backed store**：有正当跨域读者的状态（peer registry、本地身份/能力、流量监控）；
  写仍来自一个域，锁只为读者不阻塞队列；每次改是**整段转移**方法，读者看不到撕裂状态。

**同步边顺序（构造上无死锁，`onEngine` 在 debug 下强制）**：

```
main / test threads ──sync──▶ engine ──sync──▶ bleQueue
                                  └──sync──▶ noise / identity queues (叶子)
```

反向**禁止** sync-wait：bleQueue / crypto 队列只能 `async` 到 engine；
任何东西都不 sync-dispatch 到 main。

**已落地**：`BLEPeerRegistryStore`（lock-backed）、`BLERadioController`（链路层 (a) 段）、
`BLELinkAuthState`/`BLELinkBindings` 迁到 engine（(b) 段）、capability ports、
`BLEMeshPingTracker`/`BLEPrivateMediaSessionStore` 特性自持状态、
**`SimulatedMesh` 确定性多节点仿真**（5 个场景 ~40ms）、`BLEQueueContractTests` grep 护栏。

### 2.2 iOS：纯策略"卫星"清单（【IOS】`bitchat/Services/BLE/`，~60 个文件）

这是**最可复制的部分**。每个都是无 I/O、可单测的纯函数/小 struct：

| 策略 | 文件 | 作用 |
|---|---|---|
| 扫描占空比 | `BLEScanDutyPolicy.swift` | 连接≤2 或有近期流量 ⇒ 连续扫；否则 duty on/off（密集更激进） |
| 连接调度 | `BLEConnectionScheduler.swift` | 候选队列 + 预算 + 评分 + rate limit + 超时 + RSSI 阈值 |
| 冗余链路 | `BLERedundantLinkPolicy.swift` | 同角色重复链路择**最新连接**保留（地址轮换场景，实测 2–3x airtime） |
| 维护节奏 | `BLEMaintenancePolicy.swift` | 每周期决定 announce / 确保广播 / 清理 / flush spool |
| announce 节流 | `BLEAnnounceThrottle.swift` | 普通/强制间隔；**身份轮换时 reset** |
| 包新鲜度 | `BLEPacketFreshnessPolicy.swift` | 900s 过期丢弃 |
| 入站写缓冲 | `BLEInboundWriteBuffer.swift` | 按 central 的 offset 组装 + 字节上限 |
| 出站链路规划 | `BLEOutboundLinkPlanner.swift` | 选哪些链路、最小链路数、定向 spool |
| 源路由发起 | `BLESourceRouteOriginationPolicy.swift` | 何时给包加 v2 源路由（6 条 gate） |
| 路由转发 | `BLERouteForwardingPolicy.swift` | 沿源路由转发 / 回退洪泛 |
| fanout 选择 | `BLEFanoutSelector.swift` | 有界 fanout + 每 peer 去重 + 确定性子集 |
| 接收流水线 | `BLEReceivePipeline.swift` | 接收上下文 + 重复时取消已排 relay 的判据 |
| peer 事件去抖 | `BLEPeerEventDebouncer.swift` | UI 抖动 |
| 邻居重建策略 | `BLENoiseReconnectPolicy.swift` | 每物理链路 epoch 限一次重试 |
| 最近外设缓存 | `BLERecentPeripheralCache.swift` | 后台 wake-on-proximity 目标 |
| 引擎调度器 | `BLEEngineScheduler.swift` | **唯一的引擎侧延迟来源**，可注入手动时钟 |

**关键数字**（`TransportConfig.swift`）：
`bleMaxCentralLinks=6`、`bleConnectRateLimitInterval=0.5s`、
`bleDutyOnDuration=5s` / `bleDutyOffDuration=10s`、
`bleHighDegreeThreshold=6`、`bleDefaultFragmentSize=469`、
`blePendingWriteBufferCapBytes=1MB`、`bleNotificationAssemblerHardCapBytes=8MB`、
`bleRecentTrafficForceScanSeconds`、`messageTTLDefault=7`。

### 2.3 Android：工程化分工与自愈（【AND】`app/.../mesh/`）

比 iOS 更早工程化，文件职责清晰：

- `BluetoothConnectionManager`（协调者）+ `BluetoothConnectionTracker`（连接状态、RSSI、
  first-ANNOUNCE 标记、连接驱逐 `getConnectionsToEvict`）；
- `BluetoothGattClientManager`（**自愈扫描**：watchdog + `scheduleScanRestart` + `forceRestartScan`）；
- `BluetoothGattServerManager`（**自愈广播**：`scheduleAdvertiseRestart` + 状态回调）；
- ⚠️ `DeviceMonitoringManager`（坏设备 blocklist）：**只是设计文档**（`docs/device_manager.md`），main HEAD **没有这个实现文件**（GitHub contents API 404、全仓库 grep 无引用）。设计内容（15s 无 ANNOUNCE / 60s 静默 / 5 次错误断开 ⇒ 封 15min）**可作自研参考，但不是现成代码**；
- `PowerManager`（`docs/device_manager.md` 同批）：按电量档位（NORMAL/LOW/CRITICAL）
  给 `ScanSettings`/`AdvertiseSettings`/`maxConnections`/`RSSI 阈值`/`是否占空比`；
- `FragmentManager`（分片，注释明确对齐 iOS 的 512/469 值）；
- `MeshCore`（**传输无关核心**，`UnifiedMeshService` 多传输复用）、
  `MeshTransport` 接口（`broadcastPacket/sendPacketToPeer/sendPacketToLink/getDeviceAddressForPeer`）；
- `PacketRelayManager`（TTL 递减 + 源路由 next-hop + 回退洪泛）。

### 2.4 两平台都有的"源路由 v2 + 邻居 gossip"（【IOS】`docs/SOURCE_ROUTING.md`，【AND】同）

- ANNOUNCE 载荷加 TLV `0x04 DIRECT_NEIGHBORS`：最多 10 个 8 字节 peerID；
- 收方维护拓扑图；**边必须双向确认**（A 报 B 且 B 报 A）才可用于路由，否则只画虚线；
- 定向包可带 `[中间跳…]` 源路由（不含 sender/recipient）；TTL 不进签名 ⇒ 中继可改；
- 路由失败回退洪泛：10s 内没收到该接收者 authored 的包 ⇒ 记失败，60s 内该 peer 改洪泛；
- **iOS origination 有 6 条 gate**（本地作者 / 定向 / TTL>1 / 未直连 / 存在完整 v2 路径 ≤4 跳 / 无近期失败）。

### 2.5 最重要的"教训"（【IOS】§"Why the satellite strategy stalled"）

> V2 的路线是把纯策略和闭包 handler 从 `BLEService` 里剥出来。
> **~30 个纯策略 struct 是明确的胜利；5 个大 handler 抽取不是。**
> 每个 handler 需要 20–30 个弱捕获 service 的闭包组成的 "environment"，
> 逻辑走了、**状态所有权和同步却没走**，5 个 `make*HandlerEnvironment()` 工厂就 ~1.5k 行。
> 结果是队列序死锁（7 月 9 日 main↔bleQueue ABBA 冻结）和时序依赖测试。

**对我们的直接指导**：抽**纯策略**可以放心做、天天做；
抽 **handler** 必须**连同状态所有权与并发域一起搬**，否则只是把 8332 行切成更难懂的碎片。

---

## 3. 差距对照表（为什么我们不稳）

| # | 维度 | bitchat | Gosslan 现状 | 不稳定的因果 |
|---|---|---|---|---|
| A1 | **状态所有权** | 三选一所有权契约 + debug 强制同步边序 | `Arc<AppState>` + 十几把 Mutex + 散 `tokio::spawn` | TOCTOU、锁序反转、`ble_no_dial`/`ble_dial_failures`/`handshaking` 三表漂移 |
| A2 | **链路层端口** | `BLELinkLayer` 是唯一 CoreBluetooth import，只说 `LinkEvent` | 已有**帧级** `FrameSink`/`FrameSource`（`ble.rs:1131-1225`）与三端同形外设；缺**链路生命周期/命令**端口 | Windows 曾整段被 `cfg` 排掉；scan/connect/advertise 策略散在 `ble.rs` 与各驱动 |
| A3 | **引擎单串行** | 一个 engine queue 拥有全部协议状态 | 协议状态散在 `transport.rs` + `AppState` 多锁 | 无法单写者推理；回归靠真机 |
| B1 | **连接预算/调度** | `BLEConnectionScheduler`：候选队列+评分+预算(`maxCentralLinks=6`)+0.5s rate limit+超时+RSSI 自适应 | 每候选直接 spawn，无全局上限 | 拥挤环境无界拨号、连接风暴、弱链路反复重试 |
| B2 | **扫描占空比** | `BLEScanDutyPolicy` 自适应（连续/duty，随连接数与流量，密集更激进） | **已有前后台分级**（active 5s / idle 30s、窗口 2s，`ble.rs:73-83`、`:544`）；缺电量/充电档与自适应 duty | 后台功耗不可控；发现延迟无自适应 |
| B3 | **冗余链路** | `BLERedundantLinkPolicy` 择新保留 | 靠 `should_dial_ble` 镜像 + 地址表 `ble_no_dial` | 地址轮换/状态恢复后重复链路，或永久单侧不可拨 |
| B4 | **坏设备治理** | ⚠️ 仅**设计文档**（`docs/device_manager.md`），main HEAD 无实现；已实现的是扫描/广播自愈 | 无 blocklist | 一个坏对端仍可能反复拖累扫描/连接/电量（但这条属「可自研」，不是「可照抄现成代码」） |
| B5 | **功率/生命周期** | Android `PowerManager` 电量档 + iOS 后台 wake-on-proximity/state restoration | 固定参数，后台行为不可控 | 掉电、息屏、后台扫描/重连表现差 |
| B6 | **维护节奏** | `BLEMaintenancePolicy` + `BLEAnnounceThrottle`（含轮换 reset） | announce/Hello/心跳散落在 `ble.rs`/`transport.rs` | announce 抖动、轮换后短期不可见 |
| C1 | **确定性仿真** | `SimulatedMesh`：真实 engine 边到边，多节点测试 ~40ms | 无；只能 2–3 台真机 | **"总是很不稳定"无法收敛的根本** |
| C2 | **策略可测** | ~60 个纯策略单测 | 策略埋在异步函数里，只能集成测 | 改一处不知影响哪里 |
| D1 | **源路由/拓扑** | v2 源路由 + 双边确认拓扑 + ≤4 跳 BFS + 失败回退 | 只有洪泛 + fanout，无路径概念 | ≥3 节点单聊不可靠；多跳无补发 |
| D2 | **邻居 gossip** | ANNOUNCE TLV `0x04`，跨 transport 取并集 | 无 | 无法构建 mesh 图，无法做路由 |
| E1 | **外部中继** | relay 内生 | `OpaqueExternal` 流水线已就绪但**无生产者**（BLE UUID 不互通） | "给 BitChat 做转发节点"目前无帧可转 |
| E2 | **传输抽象** | `Transport` 核心 + 8 个 capability 协议 | 两套抽象并行（`Transport` trait vs `network::ble`），`TransportManager::route` 死代码 | 认知负担 + 真实路径与文档不符 |
| F1 | **平台覆盖** | iOS + Android（+ macOS 版） | mac/win/android，**iOS 缺失** | 新需求 |
| F2 | **iOS 复用** | 两平台各自原生实现 | macOS 外设已是 objc2 CoreBluetooth（**iOS 可复用**） | 复用度高，但需中央角色方案 + 工程初始化 |

---

## 4. 目标架构 V4（Gosslan）

重构成**一个 Node、多种连接、一个引擎**，平台差异只出现在最底层：

```
┌─────────────────────────── App boundary ────────────────────────────┐
│  Transport(core) + capability 协议 (FileTransfer / Group / Voice …)   │
│  UI 只读 lock-backed 快照                                             │
└───────────────────────────────┬─────────────────────────────────────┘
                                │ MeshEvent / MeshCommand
┌───────────────────────────────▼─────────────────────────────────────┐
│                    MeshEngine（单串行 actor / mailbox）              │
│  ── 线编解码（Gosslan Message + OpaqueExternal）                     │
│  ── 全局去重 + TTL（唯一一份，跨 transport）                          │
│  ── peer / connection registry（写在此域；读用 lock-backed 快照）     │
│  ── relay policy（all/friends/allowlist/off）                        │
│  ── source-route planner + topology graph（新增，§5.5）              │
│  ── reliable layer：outbox / Ack / read / retry（保留现有语义）       │
│  ── feature modules（chat / file / group / friend / …）              │
└───────────────────────────────┬─────────────────────────────────────┘
                                │ LinkEvent / LinkCommand
        ┌───────────────────────┼────────────────────────┐
        ▼                       ▼                        ▼
  LanLink                  BleLink                 OpaqueExternalLink
  (UDP 发现 + TCP)         (平台 GATT)             (BitChat 桥, §5.6)
  【红线，语义不变】          │                        │
        └───────────── Platform Link Layer ───────────┘
        ┌───────────────────────┼────────────────────────┐
        ▼                       ▼                        ▼
  mac/iOS: CoreBluetooth   Android: btleplug +     Windows: btleplug +
  central+peripheral       Kotlin GATT Server       WinRT GattServiceProvider
  (objc2, 共享一份代码)     (JNI)
```

**并发所有权（照抄三选一，写进代码注释 + 静态护栏）**：

- **Engine-confined**：dedup/TTL/peer registry/relay/route/topology/reliable 状态。
  只有一个 `tokio::task` 拥有；外部经 `mpsc` mailbox 发消息，**不直接锁**。
- **Link-confined**：每个平台的射频对象、写/通知缓冲、订阅表、重组器；
  只在自己的任务里碰，向上只发 `LinkEvent`。
- **Lock-backed**：UI 需要同步读的快照（peer 列表、链路状态、流量监控）；
  写只发生在 engine 域，且每次是**整段转移**方法。

**不用"20–30 个闭包 environment"来抽 handler**（§2.5 的教训）：
抽 handler 时**一次性把状态所有权搬进 engine**，而不是把 `transport.rs` 切碎。

**统一模型复用现有 `mesh/` 抽象**：`mesh::peer`（Peer 多 Connection）、
`mesh::connection`、`mesh::endpoint`、`mesh::path`、`mesh::selection`、
`mesh::relay_policy`、`mesh::router` —— 这些方向是对的，只是**还没接管运行时**。

---

## 5. 可实践计划（分阶段；每阶段可编译、可测、可回退）

> 规则沿用仓库现有纪律：每步 1 个可验证的最小单元；每步 `cargo test --lib --features bluetooth` +
> `npm test` + `npm run build` 全绿才进下一步；每个提交带 `Version-Bump:`。
> ⚠️ `semver.mjs` 会把 `feat/refactor/perf` + 标题含"蓝牙/BLE/mesh/传输/…" + churn≥150 判成 **major**，
> 想升 minor 时标题要避开线索词（见 `docs/notes/commit-plan-2026-09-13.md` §3）。

### Phase 0 — 冻结基线 + 纯策略卫星（不改行为，1–2 天）

**目标**：先把"能被单测的策略"从 `ble.rs` 里抽出来，**行为逐字节不变**。
这是 bitchat 验证过的低风险高收益动作。

**动作**：
1. 新建 `src-tauri/src/transport/ble/policy/`，先抽**已存在的判断**为纯函数并加单测：
   - `should_dial_ble`（已有）、`ble_dial_backoff_ms`（已有）、`peripheral_route_action`（已有）
     → 迁入；
   - 新增纯函数（**只描述现状**）：`scan_plan(connected, recent_traffic, app_active) -> ScanPlan`、
     `write_backpressure_decision(queue_len, cap)`、`connect_timeout_decision(state, now)`。
2. **不新增行为**；护栏 = 现有 441 测试 + 新纯函数测试 + 一条"源码不出现裸 sleep/magic number"的结构测试。
3. 顺手修**零风险**的既有缺口：§1.3 的 #4（TTL 钳制）、#7（stop 清退避）、#9/#10 中纯逻辑部分。

**验收**：三平台 `cargo check`；行为回归全绿；纯策略测试通过。
**风险**：极低。**LAN**：不涉及。

### Phase 1 — 连接预算与链路治理（止血，3–5 天）

**目标**：不重构分层，先把最影响"不稳定"的四件事做对。

**动作**：
1. **`BleConnectionScheduler` 等价位**：候选队列（RSSI + 未尝试优先 + 最近成功优先 + 退避）、
   全局 `max_central_links`（建议 4–6，可配置）、rate limit（建议 0.5s）、
   连接超时与失败分类、**拨号纳入 `dial_permits`**（修 §1.3 #6）。
2. **冗余链路策略**：同角色重复链路择新保留（借鉴 `BLERedundantLinkPolicy`），
   替换/补强现有 `ble_no_dial` 地址表（修 §1.3 #7）。
3. **坏设备 blocklist**（借鉴 Android `docs/device_manager.md` 的**设计**——main HEAD 无对应实现文件）：
   15s 无 Hello / 60s 静默 / N 次异常断开 ⇒ 冷却；只影响该地址，不进 DB。
4. **自愈扫描/广播**（借鉴 Android `GattClientManager`/`GattServerManager`）：
   watchdog 检测"应该扫/广播却没在跑"并带退避重启；广播失败必须留痕。
5. **连接后重试**（Windows 已知：`connect()` 后立刻 `discover_services()` 会 `Not connected`，
   见 `windows-ble-diagnosis` §2）—— 把有界重试补进统一 port。

**验收**：`ble-audit-2026-09-13.md` §7 的 Test A~F 真机 10/10；
新增纯函数/仿真级测试（候选排序、退避、blocklist TTL、冗余择新）。
**风险**：中（真机行为）。**LAN**：不涉及（BLE 内部）。

### Phase 2 — 补齐 LinkLayer 生命周期端口，统一三平台适配器（5–8 天）

**目标**：平台代码与 mesh 逻辑彻底分开，为 iOS 和仿真铺路。

**动作**：
1. 定义 Rust 端口：
   ```rust
   enum LinkEvent { LinkUp{link_id,…}, LinkDown{link_id, reason}, BytesIn{link_id, bytes}, Writable{link_id} }
   enum LinkCommand { Send{link_id, bytes}, Scan(ScanPolicy), Advertise(AdvPolicy), Disconnect(link_id) }
   ```
2. 把 `transport/bluetooth.rs`、`ble_android.rs`、`bluetooth_peripheral*.rs`
   收进 `transport/ble/link/{macos,android,windows}/`，**对外只暴露 port**。
3. `network/ble.rs` 退化成"端口事件循环 + 握手编排"，不再直接碰平台类型。
4. 平台差异从业务逻辑里的 `#[cfg]` 收敛到 `link/mod.rs` 的工厂。
5. 删掉并行的死抽象（`TransportManager::route`、未用的 `Transport` trait 路径），
   只保留一条真实路径（呼应 §1.3 #10）。

**验收**：三平台 `cargo check --features bluetooth` 0 warning；真机 Test A~F 通过；
新增"link 层不得依赖 message/DB 类型"的结构护栏。
**风险**：中高（大范围移动）。**LAN**：不受影响（LAN 不在 port 内，或也可套同一 port 但语义不变）。

### Phase 3 — MeshEngine 单串行 actor（大重构，2–3 周）

**目标**：把散 Mutex 收敛成一个 mailbox 拥有的引擎。**这是"稳定"的地基。**

**动作**：
1. 新建 `MeshEngine`：持有 `MeshRouter`、peer/connection registry、gossip 状态、
   topology、outbox/ack 状态；只通过 `mpsc` 收 `MeshEvent`、发 `MeshCommand`/`MeshEffect`。
2. `AppState` 里对应的 `Mutex<…>` **整体删除**；UI 改读 lock-backed 快照
   （照 `BLEPeerRegistryStore` 思路）。
3. `network/transport.rs` 的 `handle_message`/`handle_gossip` 主体迁为引擎方法；
   **保留** `Message` 线格式、E2EE、DB、outbox/ACK 语义（只换"在哪个域执行"）。
4. 加**队列契约测试**（grep 护栏）：只允许规定入口跨域；禁止业务代码直接锁引擎状态。
5. ⚠️ **禁止**用闭包 environment 抽 handler（§2.5）；要搬就连状态一起搬。

**验收**：LAN 全量回归 + 现有 `npm test`/`cargo test` 全绿；真机 LAN 聊天/文件/群/好友均正常；
新增"引擎外部无 `Mutex<MeshRouter>` 引用"的结构测试。
**风险**：高（但可 strangler：先双跑，engine 仅镜像状态，比对无差异后再切读写）。
**LAN**：本阶段风险最高，必须**影子模式**（见 §6）。

### Phase 4 — 确定性仿真 + 多节点测试（与 Phase 3 并行收尾，1 周）

**目标**：把"两台手机才能测"变成毫秒级单测。**这是长期稳定的最大杠杆。**

**动作**：
1. `SimulatedLink`：实现 LinkLayer port，无射频、虚拟时钟、可注入丢包/延迟/断链。
2. 用**真实 engine** 边到边组网（借鉴 `SimulatedMesh` 的 outbound tap + ingest 路径）。
3. 场景清单：
   - 三角拓扑：A→C 经 B 中继，TTL 正确递减；
   - 重复洪泛去重（同帧多路径到达只转发一次）；
   - 断链重连 → outbox 重发 → ACK；
   - 地址轮换 → 冗余链路择新；
   - TTL 边界（自报 255 被钳制）；
   - partition heal（分区恢复后收敛）；
   - BLE ↔ LAN 跨 transport 中继（TTL 跨 transport 连续）。
4. 把现有"只能真机"的回归逐步迁进仿真。

**验收**：≥8 个确定性多节点测试；单测总时长仍 < 数秒。
**风险**：中（要先有 port）。**LAN**：仿真覆盖 LAN 路径，是护栏不是风险。

### Phase 5 — 源路由 + 邻居 gossip（1–2 周）

**目标**：≥3 节点单聊可靠、可控；多跳不再是纯洪泛。

**动作**：
1. ANNOUNCE 增加 direct-neighbors（**跨 transport 取并集**，含 LAN 邻居）。
2. `MeshTopologyTracker`：边**双向确认**、60s 过期、BFS ≤4 跳、v2 能力 gate。
3. 定向包加源路由；失败回退洪泛 60s（`SourceRouteFailureCache` 等价物）。
4. 跨跳补发 + store-and-forward（修 §1.3 #2）；先补 **TTL clamp**（`transport.rs:4098` 现直接 `ttl-1` 无裁剪）。
5. fanout 候选改为"有链路的 peer"（修 §1.3 #3）。

**验收**：仿真三角/链式拓扑 + 真机三设备；群消息借非成员中继可达。
**风险**：中。**LAN**：邻居列表包含 LAN，需确保 LAN 单跳仍走直连（`selection.rs` 不变）。

### Phase 6 — BitChat 透明中继（双栈 BLE，1–2 周）

**目标**：真正"给 BitChat 做转发节点"，**不解析它的载荷**。

**动作**：
1. **外设双栈**：GATT server 同时注册 Gosslan 与 BitChat 的 service/char
   （BitChat: service `F47B5E2D-4A9E-4C5A-9B3F-8E1D2C3A4B5C`、
   char `A1B2C3D4-E5F6-4A5B-8C9D-0E1F2A3B4C5D`，见 `docs/notes/bitchat-comparison.md` §2）。
2. **central 双匹配**：扫描同时匹配两个 service UUID；连上 BitChat 后按其特征读写。
3. **广告**：legacy 31B 装不下 2 个 128 位 UUID（已有测试证明）⇒ 用
   **extended advertising / 第二 advertising set / scan response**（三平台能力不同，逐平台确认）。
4. **帧入口**：收到的 BitChat 字节 → 包成 `Message::OpaqueExternal` → **现有流水线**
   （去重 + TTL 递减 + fan-out），**不建用户/channel、不落库**。
5. **默认关闭 + 设置显式开关**（被当成 BitChat 节点的合规/身份/功耗问题）。
6. 反向：把队列里的 frame 写回 BitChat 链路（只按字节管道，不解释）。

**验收**：真 BitChat 设备能发现我们、我们能收它的包并转发到 LAN/其它 BLE 节点；
关掉开关后零影响、零广播。
**风险**：中高（射频真机；合规）。**LAN**：OpaqueExternal 已在现有流水线，LAN 是中继的一跳。

### Phase 7 — iOS 平台适配（2–3 周，需先 spike）

**目标**：iOS 也能 LAN + BLE，并与 mac/Android/Windows 互通。

**动作（先 spike，再定方案）**：
1. **Spike A（1–2 天）**：`cargo build --target aarch64-apple-ios --features bluetooth`。
   btleplug 用 `target_vendor="apple"` 选 `corebluetooth`（已核实 `platform.rs`），
   **理论可编**，但需验证其 CoreBluetooth 代码是否用了 iOS 不存在的 API。
2. **Spike B**：`cargo tauri ios init` 生成 `src-tauri/gen/apple`；
   确认 Vue 前端 / 各 Rust 依赖的 iOS 兼容（`machine-uid` 已 `cfg` 排除 iOS）。
3. **Central 方案**：
   - 若 Spike A 通过 ⇒ 直接用 btleplug；
   - 否则**自研 CoreBluetooth central**，与现有 macOS 外设
     （`bluetooth_peripheral.rs`，objc2 `CBPeripheralManager`）**同一模块风格**，
     central 用 `CBCentralManager`（objc2-core-bluetooth 同时提供）。
4. **Peripheral**：macOS 的 `bluetooth_peripheral.rs` **改一处 `cfg` 即可复用**
   （iOS 与 macOS 同套 `CBPeripheralManager` API）。
5. **权限与后台**：Info.plist `NSBluetoothAlwaysUsageDescription`；
   后台模式 `bluetooth-central`/`bluetooth-peripheral`；
   state restoration + wake-on-proximity（借鉴 `BLERadioController` 的 `armPendingBackgroundConnects`/
   `cancelStalePendingConnects`）。
6. **打包**：仓库约定 macOS 上并行打 Android + macOS；iOS 产物单独脚本/CI（需 Xcode）。

**验收**：iOS ↔ Android、iOS ↔ macOS 双向发现/连接/发消息/文件；
iOS 后台切回能自动恢复。
**风险**：高（新平台 + 苹果审核 + 后台限制）。**LAN**：iOS LAN 走同一 UDP/TCP 代码，语义不变。

### Phase 8 — 功率与生命周期策略（1 周，可与其他并行）

**目标**：省电且不掉线，移动端体验对齐 bitchat。

**动作**：
1. 电量档位策略（`PowerManager` 等价）：scan/advertise/连接数/RSSI/占空比。
2. 前后台切换：后台降低扫描占空比、保留 pending connect（iOS）、前台取消陈旧 pending。
3. 在现有前后台分级（5s/30s）之上加自适应占空比（`BLEScanDutyPolicy` 等价）。
4. 维护节奏 + announce 节流（`BLEMaintenancePolicy`/`BLEAnnounceThrottle` 等价），
   身份轮换时 reset。

**验收**：真机功耗对比 + 后台恢复延迟；仿真测试覆盖策略分支。
**风险**：低中。**LAN**：不涉及。

---

## 6. LAN 零回归策略（硬约束）

用户明确"不能让 LAN 网络出现问题"。执行纪律：

1. **边界不变**：重构只发生在 **transport / router** 层；`Message` 线格式、E2EE、
   Ed25519、SQLite schema、outbox/ACK/read、文件语义 **一律不动**。
2. **每阶段默认 feature/影子模式**：
   - Phase 2/3 的引擎切换先用**双跑**（新引擎只镜像状态，逐项比对旧路径结果），
     比对零差异后再切读写。
   - 新增策略（调度/blocklist/占空比）只作用于 **BLE**，开关可单关。
3. **黄金回归集**（每次提交必跑，且必须真机一轮）：
   - `npm test`（369+）、`cargo test --lib --features bluetooth`（441+）、`npm run build`；
   - 真机：**同 Wi-Fi 两台**（LAN 直连）、**不同网络**（BLE 或 none）、
     手机 ↔ Mac、Windows ↔ Android；聊天/文件/群/好友/已读全链路。
4. **结构性护栏**：
   - "LAN/TCP 读写循环不得引用 BLE 平台类型"；
   - "引擎外部不得直接锁 MeshRouter"；
   - "去重/TTL 只有一份"（禁止新增 per-transport dedup）；
   - "关掉蓝牙开关后 LAN 完全不受影响"（真机 Test D，`ble-audit` §7）。
5. **回退粒度**：每阶段一个可 revert 的提交序列；Phase 3 保留旧路径直到影子比对通过。
6. **文档同步**：`AI_RULES.md` §3 冻结清单与现状不符（审计 §7），本计划执行时一并治理。

---

## 7. 风险与取舍

| 风险 | 等级 | 缓解 |
|---|---|---|
| Phase 3 大重构回归 LAN | 高 | strangler + 影子双跑 + 真机黄金回归；不碰消息层 |
| btleplug iOS 不可用 | 中 | Spike 优先；退路=自研 CoreBluetooth central（与 mac 外设同模块） |
| BitChat 双栈合规/功耗/身份 | 中高 | 默认关闭 + 显式开关 + 只做字节管道 |
| 射频不可自动化 | 高 | Phase 4 仿真先覆盖协议逻辑；真机清单固化为 Test A~F |
| 平台 `#[cfg]` 收敛遗漏 | 中 | 0-warning 门禁 + 结构护栏测试 |
| 版本档位误判（major 被抬档） | 低 | 提交标题避开线索词；`npm run version:check` 门禁 |
| 工作时间/范围过大 | 中 | 每个 Phase 独立可交付；Phase 0/1 不依赖大重构，可立即开工 |

**明确取舍**：
- **不做**与 BitChat 的加密/寻址互通（用户已裁定：不兼容其数据）。
- **不做** Nostr/Tor/geohash 那层社交产品。
- **保留** LAN-first；不照抄 BitChat 的"纯 P2P、无局域网路径"。
- **保留**"两个通道都关 = 我离线"的语义（不引入 BitChat 式 store-and-forward 淡化离线）；
  但**跨跳补发**属于 mesh 传输语义，要做。

---

## 8. 立即可做的下一步（建议第一个 commit）

**Phase 0 第一步（patch，零行为变化）**：

> `refactor(trans): 抽出链路策略纯函数（行为不变）`

- 新建 `transport/ble/policy/`，迁移 `should_dial_ble`/`ble_dial_backoff_ms`/`peripheral_route_action`；
- 新增 `scan_plan` / `connect_timeout_decision` / `backpressure_decision` 纯函数（仅描述现状）；
- 补单测 + 一条"策略层不 import 平台/异步"的结构护栏；
- 顺手修 §1.3 的 #4（TTL 钳制）与 #7（stop 清退避）。

**为什么先做这个**：零风险、马上可验证、直接为 Phase 1/4 提供可测单元，
且完全符合 bitchat 复盘里"纯策略卫星是明确胜利"的结论。

**随后**：Phase 1（连接预算 + blocklist + 自愈）→ Phase 2（LinkLayer port）→
Phase 3/4（engine actor + 仿真）→ Phase 5（源路由）→ Phase 6（BitChat 双栈）→
Phase 7（iOS）→ Phase 8（功率）。

---

## 附录 A：bitchat 关键文件索引（供实现时查）

**iOS**
- `docs/BLE-ARCHITECTURE-V3.md`（目标形态 + 并发契约 + 迁移顺序）
- `docs/SOURCE_ROUTING.md`（v2 源路由 + 失败回退）
- `bitchat/Services/BLE/*.swift`（~60 个策略）
- `bitchat/Services/Transport.swift`、`MeshTransportCapabilities.swift`（能力协议）
- `bitchat/Services/TransportConfig.swift`（全部常量）
- `bitchatTests/Simulation/SimulatedMesh.swift`、`Mocks/BLEEngineManualScheduler.swift`

**Android**
- `app/.../mesh/BluetoothConnectionManager.kt` / `BluetoothConnectionTracker.kt`
- `app/.../mesh/BluetoothGattClientManager.kt` / `BluetoothGattServerManager.kt`（自愈）
- `docs/device_manager.md`（坏设备 blocklist 设计；⚠️ main HEAD **无实现文件**，仅作设计参考）
- `app/.../mesh/PowerManager.kt`（功率档）
- `app/.../mesh/MeshCore.kt` / `UnifiedMeshService.kt` / `MeshTransport.kt`（传输抽象）
- `app/.../services/meshgraph/{MeshGraphService,RoutePlanner,GossipTLV}.kt`（拓扑路由）
- `docs/ANNOUNCEMENT_GOSSIP.md`、`docs/SOURCE_ROUTING.md`、`docs/device-transport-test-matrix.md`

## 附录 B：本次分析的事实核对

- btleplug Apple 后端：`src-tauri/vendor/btleplug/src/platform.rs` 用 `target_vendor = "apple"`
  ⇒ **iOS 会选 corebluetooth**（能否运行待 spike）。
- iOS 工程：`src-tauri/gen/` 仅 `android`、`schemas` ⇒ **未初始化 iOS**。
- BitChat UUID 常量来自 `docs/notes/bitchat-comparison.md` §2（APK + 上游源码双取证）。
- 本仓库 BLE 结构、`OpaqueExternal` 流水线、`MeshRouter` 参数（`100_000, 10_000, max_ttl=6, fanout=4, 256`）
  来自 `src-tauri/src/network/ble.rs`、`network/transport.rs:2555`、`state.rs:1031`。

---

## 附录 C：最新实现核对 + 实现级细节（2026-09-14 二轮，ego-browser 复核）

### C.0 核对结论

- 用 ego-browser 查了两仓库 live HEAD（默认分支 main）：
  - iOS `bitchat` = `9b84b361225facd8e623f25d76f889d3dc54a879`（2026-08-10）
  - Android `bitchat-android` = `c127eb83ab94c069c32d37530d2faecd381cd2a8`（2026-09-13）
  - 与本地 `git clone --depth 1` **逐字节一致** ⇒ 本文结论就是最新实现。
- ⚠️ **更正一处（已回填正文）**：`bitchat-android/docs/device_manager.md` 描述的 `DeviceMonitoringManager`
  在 main HEAD **不存在**（GitHub contents API 404、全仓库 grep 无引用）——它是**设计文档**，不是现成实现。
  真正**已实现**的鲁棒性机制是 `BluetoothGattClientManager` 的扫描自愈 + `BluetoothGattServerManager` 的广播自愈（见 C.4）。

### C.1 连接调度器：判定树与打分（可直接照抄）【IOS BLEConnectionScheduler.swift】

常量（`TransportConfig.swift`）：

| 常量 | 值 |
|---|---|
| `bleMaxCentralLinks` | 6 |
| `bleConnectRateLimitInterval` | 0.5s |
| `bleConnectionCandidatesMax` | 100 |
| `bleDynamicRSSIThresholdDefault` | -90 |
| `bleRSSIConnectedThreshold` | -85 |
| `bleRSSIIsolatedBase` / `bleRSSIIsolatedRelaxed` | -95 / -100 |
| `bleIsolationRelaxThresholdSeconds` | 30 |
| `bleWeakLinkCooldownSeconds` / `bleWeakLinkRSSICutoff` | 30 / -90 |
| `bleTimeoutDiscoveryIgnoreSeconds` | 15 |
| `bleDisconnectDiscoveryIgnoreSeconds` | 3 |

发现判定 `handleDiscovery`（顺序命中，先命中先返回）：
1. 不可连接 ⇒ `ignore`
2. RSSI ≤ 动态阈值 ⇒ `enqueue`
3. 已连接/连接中数 ≥ 6 ⇒ `enqueue`
4. 距上次全局拨号 < 0.5s ⇒ `enqueue` + `scheduleRetry(剩余+0.05)`
5. 已有连接/连接中 ⇒ `ignore`
6. 距上次尝试 < 2s ⇒ `ignore`
7. 距上次超时 < 15s ⇒ `ignore`
8. 距上次断开 < 3s ⇒ `ignore`
9. 物理状态 disconnected ⇒ `connectNow`；否则 `cancelStaleConnection`

候选打分（越大越先拨）：
`score = (connectable?1000:0) + (rssi+100)*2 - secondsSinceDiscovered*10 - min(20, 1<<min(4,failures)) - (recentTimeoutWithin60s ? 10 : 0)`

动态 RSSI 阈值：
- 0 连接：隔离 <30s ⇒ -95，≥30s ⇒ -100（放宽容错）
- 有连接：默认 -90；满载或候选爆满 ⇒ -85（收紧，避免弱链路挤占预算）

> 直击我方痛点：我们现在每候选直接 `tokio::spawn`，**没有候选队列、没有全局预算、没有打分、没有 3s 断开沉降窗**；
> 这正是「拥挤环境无界拨号 / 弱链路反复重试 / 断开即重连来回抖」的来源。

### C.2 出站优先级与背压（「帧内让路」的正确实现）【IOS BLEOutboundWriteBuffer.swift】

- 优先级：`high(0) < fragment(1, suborder=总片数) < fileTransfer(2, max-1) < low(2, max)`。
  ⇒ **高优先级帧在「下一片写」之前插入**：不是打断正在写的中断，而是**每片之间取队列里最高优先级**。
- 每 peripheral 一个队列 + 字节上限（`blePendingWriteBufferCapBytes = 1MB`）；超限从队尾（最低优先级）裁剪，
  并**回报新元素是否被裁掉**（`accepted`）⇒ 调用方区分「入队了」和「入队即被丢」。
- 通知队列按**条数**上限（`blePendingNotificationsCapCount = 128`）；断链按 target 清理，broadcast 条目留给存活订阅者。
- 断链 ⇒ `discardAll(for:)` 清该链路字节，避免轮换 UUID 累积（对应我方 `ble_no_dial` 地址漂移同类问题）。

> 对我方：照抄「每链路优先级队列 + 字节上限 + accepted 回报 + 断链丢弃」，**一个机制同时解决**
> 审计 §5-1（帧内不可抢占）与内存放大；且属纯 BLE 内部，不碰 LAN。建议从 Phase 3 提前到 Phase 1。

### C.3 分片重组与传输调度【IOS BLEFragmentAssemblyBuffer / BLEOutboundFragmentTransferScheduler】

**入站重组**：
- key =（senderID 8B, fragmentID 8B）；header 校验 `total ∈ (0,10000]`、`index < total`。
- 在途装配上限 `bleMaxInFlightAssemblies = 128`，超限**淘汰最老**（按 start timestamp）。
- 每类型字节上限（file/noise 用 `maxFramedFileBytes`，其余 `maxPayloadBytes`）；超限**整组丢弃**。
- 只有**新 index** 才刷新 stall 时钟（转发来的重复片不能抑制 `REQUEST_SYNC`）。
- 停滞的**广播**装配 ⇒ 生成 8B 流 ID 列表，按「最老停滞优先」取前 `RequestSyncPacket.maxFragmentIdFilterCount` 个，
  每个限 `retryAfter` 内只请求一次；定向装配不请求。

> 对我方：`BleReassembler` 只有「条数上限」（`MAX_INFLIGHT_MESSAGES=8`）+ 30s TTL，缺「字节/类型上限」「老装配淘汰」「停滞后定向补片」。

**出站大传输**：
- 并发传输上限 `bleMaxConcurrentTransfers = 2`；其余进 pending 队列。
- **同内容去重**：无显式 transferId 的重发路径，若同 contentKey 的传输已在飞（广播覆盖任意，定向只覆盖同收件人）⇒ `droppedDuplicate`
  （现场实测：41KB 语音文件被完整发了两遍）。
- 显式 transferId 的 App 发起传输**永不去重**（UI 在追进度）。
- 严格直连请求**事务化**：start-or-reject，绝不进 pending（返回 false ⇒ 进程内无残留，原始可安全重试）。

### C.4 Android 自愈（**已实现**，这才是可照抄的）

`BluetoothGattClientManager`（扫描）：
- `SCAN_WATCHDOG_INTERVAL_MS = 30s`：周期检查「该扫却没扫」⇒ 重启。
- `SCAN_STALE_RESULT_MS = 120s`：自以为在扫但 120s 无任何结果 ⇒ `forceRestartScan`（清可能卡住的 flag）。
- `scheduleScanRestart`：`base 3s × retryCount`，封顶 30s。
- `scanRateLimit = 5s`：避免 `scanning too frequently`。
- `scanningDesired` 与「是否真在扫」分离（区分「故意停」与「故障」）。

`BluetoothGattServerManager`（广播）：
- `onStartFailure` 分类：`ALREADY_STARTED/DATA_TOO_LARGE/FEATURE_UNSUPPORTED` 不重试；
  `TOO_MANY_ADVERTISERS/INTERNAL_ERROR/其他` ⇒ `scheduleAdvertiseRestart`（backoff）。
- **scan response 里带 peerID 前 8 字节**作为 service data ⇒ 扫描方在 MAC 轮换时仍能去重身份
  （与 `bitchat-comparison.md` §4.2-1 的「广播带身份」结论一致，这里是 Android **已实现**的确证）。
- 广播前检查 `isMultipleAdvertisementSupported`。

### C.5 功率档【AND PowerManager.resolve，**已实现**】

输入：`batteryLevel / isCharging / isBackground / hasDirectPeers`；
模式：`PERFORMANCE(充电) / BALANCED / POWER_SAVER / ULTRA_LOW_POWER`（后台+CRITICAL ⇒ ULTRA）。
BLE 扫描窗（示例）：
- 前台 BALANCED：`on 8s / off 2s`
- 前台 POWER_SAVER：`on 2s / off 28s`
- 后台+有直连：`on 1s / off 29s`
- 后台无直连：`on 1s / off 59s`
- 充电 PERFORMANCE：连续
announce 间隔：前台 BALANCED 30s、POWER_SAVER 60s；后台 NORMAL 60s / LOW 120s / CRITICAL 300s。

> 对我方：已有前后台分级（active 5s / idle 30s）；应在其上加「电量 × 充电态 × 是否有直连」的档位表（Phase 8）。

### C.6 跨 transport 邻居并集（LAN+BLE 融合的直接证据）【AND MeshCore.kt】

`getDirectPeerIDsForGossip()`：
1. 取本 transport 的 verified direct peers；
2. `AppStateStore.setTransportDirectPeers(transport.id, localDirect)` 登记；
3. 取 `AppStateStore.getDirectPeers()`（**所有 transport 的并集**）；
4. `distinct().take(10)` 编码进 ANNOUNCE 的 gossip TLV。

⇒ 这正是「一个 Node、多连接、邻居表跨通道并集」的实现。我方 Phase 5 应照此做（含 LAN 邻居）。

### C.7 中继与源路由【AND PacketRelayManager.kt】

- TTL==0 ⇒ 不转发；否则 `ttl-1`。
- 语音帧：网络 >6 时额外钳到 5 跳，并 `delay(8..25ms)` 抖动。
- **源路由**：`route` 内有重复跳 ⇒ 整包丢弃（防环）；找到自己在 route 的 index，
  下一跳 = `route[index+1]`，若是最后一跳则下一跳 = recipient；`sendToPeer` 失败 ⇒ **回退广播**。
- **概率转发**（无源路由时）：TTL≥4 必转发；网络 ≤3 必转发；否则按规模：≤10 ⇒ 1.0、≤30 ⇒ 0.85、≤50 ⇒ 0.7、≤100 ⇒ 0.55、else 0.4。

> 对我方：只有 fanout 截断，没有「源路由下一跳 + 失败回退广播」，也没有「概率转发」分级；
> 可借鉴，但要与现有 fanout 语义合并，别变成两套规则打架。

### C.8 确定性仿真骨架（**照抄**）【IOS bitchatTests/Simulation/SimulatedMesh.swift】

- 用**真实引擎**、`initializeBluetoothManagers: false`；节点间通过 `_test_onOutboundPacket`（发）
  + `_test_ingestFrame`（收，走生产 ingress 归属路径）连线。
- 测试线程 `pump()`：取 pending → 投递到邻居 → `_test_fenceEngine()`，直到静止；
  `maxRounds` 用尽即 `fatalError`（**这就是「中继风暴 / 不收敛」的边界断言**）。
- 时间：每节点一个 `BLEEngineManualScheduler`，`advanceTime(by:)` 显式释放定时器工作（中继抖动、延迟 flush、重试）——**绝不 sleep**。
- 能力：`silence(a,b)`（射频静默，绑定保留）、`connectDuplicateLinks`（同 peer 双链路）、
  `emittedPackets(from:)`（攻击者抓包，用于重放测试）、`settleUntil`（按调度器时间收敛）。
- 明确记录保真边界：无物理链路 ⇒ fanout 规划/背压不被覆盖；协议行为（归属/绑定/去重/TTL/中继/会话）忠实。

### C.9 对计划的净影响

1. §2.3 与 §3-B4 已更正：坏设备 blocklist 是**设计**而非现成实现（自研成本要计入）。
2. Phase 1 增补：扫描/广播自愈参数（C.4）；并把「出站优先级队列 + 字节上限」（C.2）从 Phase 3 提前到 Phase 1
   ——它直接缓解「帧内不可抢占」与内存放大，属纯 BLE 内部、不碰 LAN。
3. Phase 4 仿真骨架按 C.8 落地（真实引擎 + 手动调度器 + 静止断言）。
4. Phase 5 邻居 gossip 按 C.6 的「跨 transport 并集」实现。
5. 我方「帧内让路」机制确定为 **C.2**：每链路优先级写队列 + 逐片取最高优先级（不是打断中断）。
