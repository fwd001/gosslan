# 改造方案评审：对照真实代码（ble-mesh-v4-plan）

> ⚠️ **历史文档（V4）**：评审结论已并入 V5（`docs/architecture/`）。本文保留为「为什么这么设计」的证据链。

- Date: 2026-09-14
- 评审对象：`docs/notes/ble-mesh-v4-plan-2026-09-14.md`（含附录 C）
- 评审方法：**逐条读实现代码核对**（不采信文档转述）
  - `src-tauri/src/state.rs`（1375 行）· `network/ble.rs`（2173）· `network/transport.rs`（8399）· `mesh/*`（约 2000）
- 结论一句话：**方向对（分层 + 单串行 + 纯策略 + 仿真），但方案有 6 处与当前代码不符/过时、5 处低估风险、5 处遗漏；
  按现状直接执行会踩「改了但问题不在那里」和「低估 LAN 回归面」两类坑。**

---

## 0. 总评

| 维度 | 评价 |
|---|---|
| 问题定位（蓝牙不稳=架构问题） | ✅ 成立，且比方案写得更严重（见 §3.1 两套模型） |
| 目标架构（LinkLayer / MeshEngine / 纯策略 / 仿真） | ✅ 方向正确，与 bitchat 复盘一致 |
| 现状描述准确性 | ⚠️ 6 处与代码不符/过时（§2）——其中 1 处是**已修复**的缺陷被列成待办 |
| 风险估计 | ⚠️ 偏乐观（§3）：LAN 回归面、DB 耦合、爆炸半径、仿真前置条件都被低估 |
| 可执行性 | ⚠️ Phase 3「大重构」缺中间态，且与仓库「每阶段可编译可回退」纪律冲突 |
| 可验收性 | ⚠️ 缺稳定性 SLO/度量，验收多为「真机跑通」定性描述 |

---

## 1. 逐条核对表（方案断言 → 代码证据 → 判定）

| # | 方案断言 | 代码证据 | 判定 |
|---|---|---|---|
| 1 | `Arc<AppState>` + 十几把 Mutex，无单一状态域 | `state.rs:607-839` 列出约 40 个字段，其中 `mesh_router`/`peers`/`links`/`gossip`/`peer_manager`/`relay_policy` 等均为独立 Mutex | ✅ 成立 |
| 2 | 平台 API 细节与 mesh 逻辑混在 `ble.rs` | 成立但**已有部分端口**：`FrameSink`/`FrameSource`（`ble.rs:1131-1225`）+ 三端同形外设接口；缺的是**链路生命周期/命令**端口 | ⚠️ 部分成立（§2.2） |
| 3 | 每候选直接 spawn，无全局上限 | `ble.rs:514` 每候选 `tokio::spawn`；`dial_permits` **只在** `transport.rs:2167`（TCP 拨号）使用，BLE 路径不取许可 | ✅ 成立 |
| 4 | 扫描策略粗糙、固定参数 | 固定窗口 2s（`ble.rs:73`），但**已有前台/后台分级**（`SCAN_INTERVAL_ACTIVE=5s` / `IDLE=30s`，`ble.rs:77-83`，按 `app_active` 在 `ble.rs:544` 选）；缺的是**电量/充电档 + 自适应占空比** | ⚠️ 部分成立（§2.4） |
| 5 | 帧内不可抢占（大帧堵聊天） | `ble_writer_loop` 取到一条 `Message` 后整帧 `send_frame`（`ble.rs:1251-1271`），priority 只在**帧间**生效 | ✅ 成立 |
| 6 | 两套传输抽象并行 | `transport/mod.rs:51/112/122` 三处 `#[allow(dead_code)]`；真实路径是 `network::ble` | ✅ 成立 |
| 7 | 跨跳无补发 / 无 store-and-forward | `flush_outbox` 只按直连 peer `try_send`；转发仅在两端在线时直发 | ✅ 成立 |
| 8 | fanout 候选取 `peers`（含无链路节点） | `transport.rs:4074-4094`：候选来自 `state.peers.keys()`，不是 `links` | ✅ 成立 |
| 9 | Gossip TTL 上限失效 | `transport.rs:4098` `fwd.ttl -= 1`，随后直接发；`MeshRouter` 的 `max_ttl` 裁剪（`mesh/router.rs:192`）**不在这条转发路径上** | ✅ 成立 |
| 10 | M3-d 未接线：源发 Gossip 取 `links.get(peer).first()` | `transport.rs:309-323`：**已改成 `best_link_kind`（LAN>Routed>Bluetooth）选链路**，注释标明「M3-d（2026-09-14 全 Windows 局域网真机）」 | ❌ **已修复，方案过时**（§2.1） |
| 11 | 坏设备治理可借鉴 Android | 已更正为设计文档（留待自研） | ✅ 正确 |
| 12 | `start()` 在 adapter 失败时早退，外设不启动 | `ble.rs:179` `driver::adapter().await?`，`:224` 外设 spawn 在其后 ⇒ 无适配器时确实跳过外设 | ✅ 成立 |

---

## 2. 方案与代码不符 / 过时（必须修正，否则会做无用功）

### 2.1 「M3-d 未接线」是**过期结论**（最高优先修正）
`transport.rs:309-323` 已经按路径优先级选发送链路，并带注释「M3-d（2026-09-14）」。
方案 §1.3 #5 与 Phase 5 第 4 条「源发按健康度选路（修 §1.3 #5）」应改为：
**源发已按路径优先级（`best_link_kind`）；仍缺的是「同路径类型内按 `pick_link` 的活性/失败数选路」**——
而 `try_send`（`transport.rs:244-269`）已经用 `route_order` + `pick_link` 做了这件事。
⇒ 实际剩余缺口很小（源发 Gossip 的路径优先级已解决），Phase 5 不应再列为一项。

### 2.2 「没有独立的链路层端口」不准确
`FrameSink`（`ble.rs:1131`）/`FrameSource`（`ble.rs:1160`）**已经是帧级端口**，且 macOS/Android/Windows 三套外设
（`bluetooth_peripheral.rs`/`ble_android.rs`/`bluetooth_peripheral_windows.rs`）已同形。
真正缺的是：**链路生命周期与命令端口**（scan/advertise/connect 策略、LinkUp/LinkDown/Writable 事件）——
现在这些散落在 `scan_loop`、`dial_and_register`、`peripheral_accept_loop` 与各驱动里。
⇒ Phase 2 应写「在既有帧级端口之上补生命周期端口」，而不是「从零抽端口」。

### 2.3 「先抽纯策略」低估了已完成度
`ble.rs` 里**已经有 9 个纯函数**：`ble_throughput_estimate`(120)、`scan_interval`(137)、
`ble_dial_backoff_ms`(592)、`should_dial_ble`(653)、`peripheral_route_action`(685)、
`frame_is_hello`(706)、`is_logworthy_frame`(1012)、`frame_trace`(1023)、`preamble_action`(1117)；
`mesh/selection.rs:44 pick_link`、`transport.rs:125 route_order`、`mesh/relay_policy.rs:129 should_forward` 等也是纯函数。
⇒ Phase 0 的价值不在「抽出这些」，而在**补缺失的新策略**（连接调度/占空比/冗余链路/自愈/功率）并加测试。

### 2.4 「固定扫描参数、无前后台策略」不准确
`app_active`（`state.rs:689`）已驱动前台 5s / 后台 30s（`ble.rs:544-545`），也有用户触发/内部唤醒两条立即扫描路径
（`ble.rs:248-269`）。缺的是**电量档、充电态、是否有直连**的细粒度档位与自适应占空比（附录 C.5 有价值）。

### 2.5 退避上限「分钟级」已过期
`ble_dial_backoff_ms`（`ble.rs:592-604`）现为：前 3 次不退，第 4/5/6 次 5s/10s/20s，封顶 **20s**。
方案若仍引用「曾封顶 10 分钟」只能作为历史背景，不能作为待修项。

### 2.6 PowerManager 的属性归属需注意
`PowerManager` 是 Android 仓库的实现（已实现，附录 C.5 正确）；但正文 §2.3 把它与 `docs/device_manager.md` 并列时
措辞像是同一份未落地设计，容易被误读。建议明确：**功率档=已实现；blocklist=仅设计**。

---

## 3. 方案低估的风险（必须补）

### 3.1 真正的重复不是「几把锁」，而是**两套 Peer/Link 模型**（最重要）
- `state.rs`：`Peer`(154) + `Link`(187) —— 传输层模型，运行时就靠它。
- `mesh/`：`Peer`/`Connection` —— mesh 层模型，`mesh/manager.rs:10` 自述「**Phase 2 仍是旁路：不接管 state::peers / state::links**」。
- `transport.rs:113-116` 自述：「mesh 层（Connection）与传输层（Link）是**两套链路表**，且不保证 1:1 同序」，
  于是 `route_order`（`transport.rs:125-162`）用**端点对齐**把两套表缝起来，还要为「登记窗口」合成假健康候选（`:142-145`）。

⇒ 这才是「状态漂移」的结构性来源，比「有 40 把锁」具体得多。
**V4 必须明确：保留哪一套、删除哪一套，或定义唯一的 bridge 契约。** 方案目前只说「复用现有 mesh/ 抽象」，太模糊。

### 3.2 引擎里 105 处 `state.db.lock()`：actor 化会撞上**同步 SQLite**
`state.rs:609` 是 `Mutex<Connection>`（rusqlite 同步句柄），`network/transport.rs` 里出现 **105 次** `state.db.lock()`，
且在 `handle_gossip`（如 `transport.rs:4054`）这种热路径里。
把状态收进单串行 actor 后，若 DB 调用也在 actor 内，**同步阻塞会顶住 mailbox**；若不在，则要重新划分「谁写库」。
方案完全没谈这一点。建议：**engine 只产出 effect（PersistMessage/Ack/SaveOutbox…），DB 由独立 worker 消费**。

### 3.3 LAN 回归面被低估：mesh 引擎**就是** LAN 主路径
方案说「只动传输/路由边界」，但 `handle_message`、`handle_gossip`、`try_send`、`broadcast_gossip`、
`register_connection`/`unregister_connection` 都是 **LAN 与 BLE 共用**的（`ble.rs:34-39` 直接 import 这些）。
⇒ Phase 3 不是「不碰 LAN」，而是「**直接改 LAN 的核心收发路径**」。风险等级应标为**高**，并强制影子双跑。

### 3.4 「AppState 的 Mutex 整体删除」爆炸半径巨大
这些字段不只被 `transport.rs` 用：`commands.rs`（Tauri 命令层）、`network/file.rs`、`network/discovery.rs`、
`network/mod.rs`、`ble.rs` 都在锁它们。整体删除 = 全仓库改调用面。
⇒ 方案应**限定范围**（先收敛 `mesh_router`/`gossip`/`peer_manager`/`peers`/`links`/`relay_policy` 这 6 个 mesh 自有字段），
`db`/`downloads_dir`/`nickname` 等非 mesh 字段保持不动。

### 3.5 Rust 侧仿真缺注入点，Phase 4 不是「收尾」而是**前置依赖**
bitchat 能建 `SimulatedMesh` 的前提是 `BLEService` 接受可注入的 engine scheduler + `initializeBluetoothManagers: false` + `_test_ingestFrame` 钩子。
我们这边 `network/ble.rs` **直接调用 `driver::*`** 并 `tokio::spawn`（ble.rs 9 处 spawn、transport.rs 14 处），没有可注入时钟/链路。
好消息是**已有 `FrameSink`/`FrameSource` 可复用为模拟链路**（`ble.rs:1131/1160`），但需要：
① Phase 2 的生命周期端口；② `tokio::time::pause()` 兼容的定时器封装；③ 测试用 ingest 入口。
⇒ Phase 4 应明确「依赖 Phase 2 的端口」，不能与 Phase 3「并行收尾」当作无依赖。

---

## 4. 方案遗漏

1. **没有稳定性 SLO/度量**：应把「蓝牙不稳」量化——例如
   连接成功率、发现→建链 P50/P95、重连时延、消息投递率、假在线率；Phase 0 先埋点（`ble_scan`/`BleDiag` 已有统计框架可扩展）。
2. **没把现有护栏与阶段绑定**：仓库有 441 个 Rust 测试 + `scripts/verify-guards.py`（改源码跑测试再还原）+
   `npm test`；方案只笼统写「全绿」。每阶段应列出**新增护栏**（纯函数单测 / 源 grep 结构测试 / 真机清单）。
3. **没有「改哪个文件、加哪个模块」的映射**：附录 C 有策略细节，但 Phase 没落到文件；执行时仍要重新设计。
4. **wire 变更流程没提**：Phase 5 的邻居 gossip TLV、Phase 6 的 OpaqueExternal 都动线格式，
   而仓库有 INV-P13（协议变更流程）与 ADR-0007（版本化）；方案写「不考虑兼容性」与仓库纪律需对齐。
5. **死代码清理没进阶段**：`transport/mod.rs` 的 `Transport` trait / `TransportManager::route`、`RelayManager`
   （`state.rs:728`，审计称死代码）应作为 Phase 0 的「减少认知负担」项，而不是留到最后。
6. **没有回滚/灰度方案**（除 Phase 3 一句「影子双跑」）：BLE 新策略应可**按特性开关单关**，
   便于真机出问题时一键回退到当前行为。

---

## 5. 修订建议（可执行）

### 5.1 Phase 3 拆成 3a/3b，引入中间态（关键）
- **Phase 3a（机械、低风险）**：把 6 个 mesh 自有 Mutex 合并为一个 `Mutex<MeshState>`（或 `tokio::sync::Mutex`），
  并写死**锁顺序契约**（一个函数内只拿一次）。行为零变化、可单独回退。
- **Phase 3b（actor 化）**：把 `Mutex<MeshState>` 换成 mailbox + 单任务；DB 改 effect/worker。
⇒ 把「一次性大重构」变成「两步可验收」，符合仓库纪律。

### 5.2 先定「单一模型」，再动结构
在 V4 开头增加一节「模型收敛决策」：
- 建议**让 `mesh::Peer/Connection` 成为唯一模型**，`state::Link` 退化为「传输句柄（bulk/priority/cancel）+ path_kind」；
- 或相反（保留 `state::Link`，删 `mesh::Connection`）。**二选一，不允许两条并存**。
- `route_order` 的「按端点对齐 + 合成候选」逻辑应作为**过渡期契约**写进 ADR，并在 3b 后删除。

### 5.3 DB 不进 engine
engine 只发 `Effect`：`PersistMessage`/`UpsertOutbox`/`WriteAck`/`TouchConversation`；
由独立 DB worker（可 `spawn_blocking`）消费。否则 105 处同步锁会变成 actor 的吞吐瓶颈甚至死锁源。

### 5.4 Phase 1 增加 SLO + 度量
明确目标值（示例，需用户确认）：扫到→建链 P95 < 10s；重连 P95 < 15s；健康链路误拆率 = 0；
单聊投递率 ≥ 99%（3 设备静态场景）。没有这组数字，「稳定了没有」无法判定。

### 5.5 Phase 4 前置条件写清
依赖 Phase 2 的端口 + 手动时钟 + 测试 ingest；场景清单保留（三角中继/去重/重连/轮换/TTL/partition heal/跨 transport）。

### 5.6 每阶段绑定护栏与开关
表格列：阶段 / 新增纯函数单测 / 结构护栏 / 真机判据 / 回退开关。

### 5.7 顺序微调（建议）
Phase 0 改为「删死代码 + 补缺失策略 + 埋 SLO」；
Phase 1「连接调度 + 自愈 + 出站优先级队列（纯 BLE 内部）」；
Phase 2「生命周期端口 + 仿真底座」；
Phase 3a「合并 mesh 状态」→ 3b「actor + DB effect」；
Phase 4「确定性仿真场景」；
Phase 5「源路由 + 邻居并集（先确认 TTL clamp）」；
Phase 6「BitChat 双栈」；Phase 7「iOS」；Phase 8「功率档」。

---

## 6. 修订后里程碑（建议表）

| 阶段 | 目标 | 关键产出 | 风险 | 回退 |
|---|---|---|---|---|
| 0 | 减负 + 度量 | 删 `Transport`/`RelayManager` 死代码；补调度/占空比纯函数；SLO 埋点 | 极低 | 单提交 revert |
| 1 | BLE 内部止血 | 连接调度器/blocklist/自愈；每链路优先级写队列 + 字节上限 | 中 | 特性开关 |
| 2 | 端口 + 仿真底座 | `LinkEvent`/`LinkCommand`；三端适配器收口；模拟链路 | 中高 | 双路径 |
| 3a | 状态合并 | 6 个 mesh Mutex → `Mutex<MeshState>` + 锁序契约 | 中 | 单提交 revert |
| 3b | actor + DB effect | 单任务 mailbox；DB worker | 高 | 影子双跑 |
| 4 | 确定性仿真 | ≥8 个多节点场景 | 中 | — |
| 5 | 源路由 + 邻居并集 | TLV + 拓扑 + 回退洪泛；**先补 TTL clamp** | 中 | 特性开关 |
| 6 | BitChat 双栈 | 双 UUID GATT + OpaqueExternal 生产端 | 中高 | 默认关 |
| 7 | iOS | CoreBluetooth central/peripheral + 权限/后台 | 高 | 平台门 |
| 8 | 功率档 | 电量×前后台×直连 档位表 | 低中 | 特性开关 |

---

## 7. 结论

方案**可以作为路线图**，但不能照现状执行。落地前至少要改三件事：
1. **删掉已过时/失实的断言**（M3-d 已修、「无端口」、「无前后台策略」、退避已缩短）；
2. **把「两套 Peer/Link 模型 + 105 处同步 DB 锁」写成一级问题**（它们才是漂移与回归风险的来源）；
3. **Phase 3 拆 3a/3b 并补 SLO/护栏/回退开关**，否则大重构既难验收也难回退。

改完这三点后，我仍然支持原方案的核心判断：**蓝牙不稳的根因在架构（无单串行域、无生命周期端口、无确定性仿真），
而 bitchat 的三层 + 纯策略 + 模拟器是可以直接借鉴的正确方向。**