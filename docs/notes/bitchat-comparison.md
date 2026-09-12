# 社区实现对照：BitChat 与 Gosslan 的多通道 mesh

- Date: 2026-09-12
- 用户裁定：**「可以参考 bitchat 协议，最终结果可以帮 bitchat 做中间节点，但不用兼容它的消息协议」**
  ⇒ 本文只回答两件事：**（1）我们和它已经一致在哪、差在哪（都要有证据）；（2）"帮它当中继"到底还缺什么、代价多大。**
- 证据来源（两类，文中逐条标注）：
  - **【APK】** 用户手机上已装的 `com.bitchat.droid` **1.7.4**（versionCode 35 / targetSdk 35）：
    `adb pull` 出 base.apk 后直接扫 `classes.dex` 字符串，以及 `AndroidManifest.xml`；
  - **【源码】** 上游 `github.com/permissionlesstech/bitchat-android`（`--depth 1`，HEAD = `versionName 2.0.2`）。
    ⚠️ 源码比手机上的 APK 新，所以常量/命名以源码为准时我会显式标注「上游 2.0.2」。
  - **【本仓库】** Gosslan `src-tauri/src/…`，行号即当前 `HEAD`。
- 另：`clash-verge-rev`（单文档多窗口 + Rust 定向发事件）与 Tauri 2.x `emit_filter` 的对照，
  结论已落在 **ADR-0018（窗口架构）**，本文不重复。

---

## 1. 传输层：它**不是**纯 BLE（这一点纠正了我最初的印象）

| 通道 | BitChat 1.7.4 | Gosslan |
|---|---|---|
| BLE mesh | ✅ central + peripheral 双角色 | ✅ 同（ADR-0015/0017） |
| Wi‑Fi Aware（Android P2P Wi‑Fi） | ✅ **【APK】** `WifiAware`×11、`WifiAwareManager`×2；**【源码】** `wifi-aware/WifiAwareMeshService.kt` 内有 `WifiAwareTransport : MeshTransport` | ❌ 未做（我们用**普通局域网** UDP 发现 + TCP，见下） |
| 互联网通道 | ✅ Nostr（**【APK】** `nostr`×209、`Nostr`×283、`wss://`×5、`relay.damus`×1）+ 可选 Tor（`ArtiTor`×1、`torproject`×1） | ❌ Phase 8 起就是"能离线跑"的定位 |
| 局域网直连 | ❌ 无（没有 multicast/DatagramSocket；**【APK】** `MulticastSocket`×0、`DatagramSocket`×0） | ✅ **LAN 优先**：UDP 发现 + TCP；同一 Wi‑Fi 不走 BLE |
| 多传输抽象 | ✅ `MeshTransport` 接口（`id`/`broadcastPacket`/`sendPacketToLink`…）+ `UnifiedMeshService` 统一调度 | ✅ 等价物：`network/` 下 LAN 与 BLE 两条 `MeshTransport` 路径 + 统一 `Message` 层 |

**结论 A**：BitChat 与我们在**架构方向上是同一类**——多通道、就近优先、尽力而为的 mesh。
差别在**具体通道组合**：它用 Wi‑Fi Aware/Nostr 补覆盖，我们用局域网 TCP 补带宽与延迟。
所以"参考 bitchat"这件事，真正可比的不是"要不要防 BLE 单点"，而是**通道编排策略**。

## 2. BLE 层细节对照

| 维度 | BitChat | Gosslan | 结论 |
|---|---|---|---|
| 服务 UUID | `F47B5E2D-4A9E-4C5A-9B3F-8E1D2C3A4B5C`（**【APK】** ×1；**【源码】** `AppConstants.kt:27`） | `6b1a7e60-3f4c-4d8a-9c2b-1e5f7a9d0c31`（`transport/bluetooth.rs:23`） | **不同** ⇒ 传输层天然不互通 |
| 特征 UUID | `A1B2C3D4-E5F6-4A5B-8C9D-0E1F2A3B4C5D`（+ 标准 CCCD `0x2902`，**【APK】** `00002902`×1） | RX `…0c32`（写）/ TX `…0c33`（通知）+ CCCD | **不同** |
| 广播内容 | 服务 UUID **+ 服务数据里塞对端 peerID**（**【APK】** `addServiceData`×1、`addServiceUuid`×1；**【源码】** `BluetoothGattServerManager.kt:390,404` 用 `peerIDBytes`） | **只有服务 UUID**，身份靠连上后的 Hello 交换（`transport/bluetooth_peripheral.rs:17,588` 明确"本地名不放"） | ⚠️ **我们可抄**，见 §4.2-1 |
| 扫描侧过滤 | 平台层 `setServiceUuid`（`BluetoothGattClientManager.kt:212`）**并且**再手工检查 `scanRecord.serviceUuids`（同文件 :378） | 平台层不过滤、Rust 侧查 `properties().services`（`transport/bluetooth.rs` scan_peers） | 双方都踩过"平台过滤不可靠" ⇒ 结论一致 |
| 包格式 | 定长头 14/16B：version/type/**ttl**/timestamp(8)/flags/payloadLen + senderID(8) + [recipientID(8)] + payload + **Ed25519 签名(64)**，另含可选**源路由** `route`（`protocol/BinaryProtocol.kt:64-90`） | 我们自己的 `Message`：线上是 **JSON**（`network/transport.rs:53` 的 `serde_json::to_vec`）+ 可选 E2EE；TTL 在 Gossip 头 | 结构同思路，**编码/签名不兼容**（符合用户裁定） |
| 分片 | 阈值 512B、单片 469B、超时 30s；上限：256 片/消息、1MiB/消息、64 并发组、**4MiB 全局**（**【源码/上游】** `AppConstants.kt:38-45`） | 单片按协商 MTU；上限：`MAX_BLE_MESSAGE_BYTES = 512KiB`、`MAX_BLE_CHUNKS_PER_MESSAGE = 8192`、`MAX_INFLIGHT_MESSAGES = 8`、`PARTIAL_TTL_MS = 30s`（`transport/ble_framing.rs:45-69`） | 都有超时与上限；**它有"全局字节上限"**（跨消息的防内存放大），我们只有"条数上限" ⇒ 见 §4.2-2 |
| 跳数/TTL | `MESSAGE_TTL_HOPS = 7`、`SYNC_TTL_HOPS = 0`（= 仅邻居，不转发）（**【源码/上游】** `AppConstants.kt:10-11`）；**TTL 不参与签名**，因为中继会改它（`BinaryProtocol.kt:113-118` 有明确注释） | 生产参数 `MeshRouter::new(100_000, 10_000, max_ttl=6, fanout=4, 256)`（`state.rs:905`）；外部帧声明 TTL 限制 1..=16（`MAX_OPAQUE_TTL`，`protocol.rs:515`）再被 `max_ttl` 裁到 6（`mesh/router.rs:192`） | 思路一致（都有上限+裁剪）；**"TTL 不进签名/只对固定 TTL 签名"这一条我们要确认**，见 §4.2-3 |
| 去重 | `SecurityManager.processedMessages`（synchronized set，上限 `MAX_PROCESSED_MESSAGES = 10_000`，按时间过期 + 超限淘汰；`SecurityManager.kt:33,80,100,403`）；UI 层还有"多传输重复投递"去重（`ui/MessageManager.kt:14`） | `MeshRouter` 全局去重按 `msg_id`；外部帧按自己的 `id`（`mesh/router.rs`） | **我们一致**（都是"大小有界的近期集合"） |
| 离线暂存 | ✅ `StoreForwardManager`（**【APK】** 类名×1）：12h、普通对端 100 条、收藏对端 1000 条（**【源码/上游】** `AppConstants.kt:74-76`） | ❌ 未做（当前语义：对端不在线就发不出，等它上线） | ⚠️ **差距**，但属产品范围问题，见 §5 |
| 加密 | Noise（**【APK】** `Noise`×77）+ noise/southernstorm 实现 | 我们自己的加密层 | 不互通（符合裁定） |

## 3. 结论一：现在**不能**直接给 BitChat 当 BLE 中继（差的是门牌号，不是协议）

我们 Phase 8（ADR-0017）已经实现且**与协议无关**的中继语义：
收到 `Message::OpaqueExternal { id, ttl, payload }`（`protocol.rs:501`、`network/transport.rs:2059`）
→ 去重 → TTL 递减 → fan-out 转发，**不解析载荷、不建 BitChat 用户/channel**。
这正是用户要的"当中继但不兼容协议"。

但 BitChat 在 BLE 上只认**它自己的 UUID**：
它扫描时按 `F47B5E2D-…` 过滤/校验（`BluetoothGattClientManager.kt:212,378`），
写数据时用 `A1B2C3D4-…`（同文件 :527-529）；我们广播/提供的是 `6b1a7e60-…`。
⇒ 两边在 BLE 层**互相看不见**，目前没有任何 BitChat 帧会流到我们这里；
`OpaqueExternal` 这条流水线只在"帧从别的途径送进来"时才有用。

## 4. 结论二：要真当中继，需要的是**BLE 双栈外设**，不是改协议

### 4.1 双栈的具体做法（我们已掌握全部所需事实）

1. **GATT server 同时注册两套服务**：我们原有 RX/TX + BitChat 的 `F47B5E2D-…` /
   `A1B2C3D4-…`（我们只当**不透明管道**，不解析其载荷）；
2. **central 侧同时匹配两个服务 UUID**，连上 BitChat 设备后按它的特征读写；
3. 收到的 BitChat 帧 → 包成 `OpaqueExternal` 进现有中继流水线；反向把我们队列里的帧写回；
4. **广播**要同时被两边看见：legacy 广播载荷 31B 装不下两个 128 位 UUID（2×18=36B，已有测试
   `advertisement_fits_legacy_budget_and_deliberately_omits_local_name` 证明 2 个 UUID 就超了
   `LEGACY_ADV_PAYLOAD_LIMIT`）⇒ 要么 **extended advertising**，要么 Android 上开**第二个
   advertising set**，macOS 上按需切换/放扫描响应；
5. **产品决策**：这等于"附近有 BitChat 设备时我们也广播它的 UUID"，
   建议做成**默认关闭 + 设置里显式打开**。

**代价**：双套 GATT + 多广播实例 ⇒ 复杂度、功耗、以及"被当成 BitChat 节点"的合规/身份问题。
**收益**：把 BitChat 的 mesh 与我们缝在一起（在"两台手机不在同一 Wi‑Fi"这种场景下才有意义）。

**当前决定**：**暂不实现**（等用户明确要）。两条流水线已解耦 ⇒ 将来加双栈**不需要改**中继语义。

### 4.2 值得抄（按价值排序，都能落到我们的代码）

1. **广播里带身份**（BitChat：service data = peerID，8B）——我们广播里**只有 UUID**
   （`bluetooth_peripheral.rs:17,588`）。加上它，扫描方**不连接**就知道"对面是谁"，
   可以直接用它做**去重键**（Android/iOS 的 BLE 地址会轮换，地址不可做身份）
   并**在广播层就决定要不要拨**（`should_dial_ble(my_id, peer_id) = my_id > peer_id`）。
   👉 这正好打击我们 4.2.x 反复踩的痛点：互相拨号把好链路拆掉、`ble_dial_backoff` 反复重试。
2. **跨消息的全局内存上限**（BitChat：256 片/消息、1MiB/消息、64 组、4MiB 全局）——
   我们只有 `MAX_INFLIGHT_MESSAGES = 8`（条数）+ `MAX_BLE_MESSAGE_BYTES = 512KiB`（单条），
   把 8 条都填到 512KiB 仍有 4MiB 级峰值；**对齐一个全局字节上限**成本极低。
3. **TTL 与签名解耦**（BitChat：签名前把 TTL 固定 —— `BinaryProtocol.kt:113-128`）——
   我们中继也会改 TTL；如果签名覆盖 TTL，则"转发即失效"或需要重签。
   我们有 `OpaqueExternal`（不动别人的字节）绕开了这个问题，但**自己协议内的转发**要确认这点。
4. **TTL 分档**（普通 7 跳 vs 同步包 TTL=0 仅邻居）——我们只有一个 TTL 语义；
   "邻居发现类"消息不该全网泛洪，分档能显著削负载。

### 4.3 不值得抄

- **把身份寄托在单一广播 UUID 上**：我们要兼容时用显式双栈开关，而不是默认混进公共 mesh；
- **纯 P2P、没有局域网路径**：同一 Wi‑Fi 下 TCP 的带宽/延迟远好于 BLE/Wi‑Fi Aware，
  用户已明确"局域网为第一通道"⇒ 我们的 LAN-first 保留；
- **Nostr/geohash 那一层社交产品**（位置频道、gift-wrap 私信、Arti/Tor）：
  不是本项目范围（用户只要求"参考协议 + 能当中继"）；
- **iOS 兼容包袱**：它的包格式注释里到处是"100% backward compatible with iOS version"，
  我们已停做 iOS（ADR-0015），不需要为兼容牺牲设计。

## 5. 与用户既有裁定的一致性检查

| 用户裁定 | 本对照的结论 |
|---|---|
| 「不必兼容它的消息协议」 | ✅ 只借用传输层思路；`OpaqueExternal` 不解析载荷 |
| 「如果两个通道都关了，就是我离线」 | ⚠️ BitChat 用 store-and-forward（12h 缓存）**淡化了"离线"**；我们不做，语义更简单、也更符合用户定义 |
| 「局域网优先，BLE 兜底，双通道设备当中继」 | ✅ 与 BitChat 的多传输编排同向；我们是 LAN+BLE，它是 BLE+Wi‑Fi Aware+Nostr |
| 「手机运行即中继」 | ✅ 双方一致（都在跑的时候转发别人的包） |

## 6. 遗留（未做，需用户决定才动）

1. **双栈 BLE 外设**（§4.1）——需先决定是否"默认广播 BitChat 的 UUID"（我建议默认关）；
2. **广播带 peerID**（§4.2-1）——改动小、收益直接命中当前 BLE 稳定性痛点，建议优先；
3. **全局分片字节上限**（§4.2-2）——纯加固；
4. 与 BitChat 的**加密/寻址**互通**明确不做**（用户已裁定）；
5. 本文事实取自 **Android 1.7.4 APK + 上游 2.0.2 源码**；iOS 端不涉及（本项目停做 iOS）。
