# 迁移台账（Migration Ledger）

> **这份台账只回答一个问题：每个关注点有几个"家"，哪个在跑数据？**
>
> 配套：`docs/domains.yml`（机器可读的领域图）。两份文件必须一致 —— 由
> `scripts/check-domain-map.mjs` 守门（路径存在、一文件不属两域、无文件漏归属）。

---

## 0. 为什么需要这份台账

本仓库处在一次**半完成的 ADR 迁移**中：从 `network/`（老栈）走向
`transport/` + `discovery/` + `mesh/`（新栈，分 `P-A03` / `P-A04` / `7-e` 等阶段）。

这本身是**正确**的安排（新栈的文档头把"待接线"写得很清楚）。风险在于：

```text
同一个关注点有两个家，且都"看起来对"
      ↓
AI（或人）改「传输 / 在线状态 / 发现」时 grep 命中 2 处
      ↓
改了搜索先命中的那个（通常是更小、更新、文档更好的新家）
      ↓
真跑数据的是老家 ⇒ 症状不变或换了个形态
      ↓
下一轮再修，命中另一处 ⇒ 「改完这个 bug 又冒那个」
```

**所以 `docs/domains.yml` 里每个领域强制带一个 `active_home` 字段** ——
不回答这个问题，领域图就是一份"AI 会照着执行、但与真实活路径不符"的地图，
**比没有地图更危险**。

本台账的每一条都给出 `file:line` 证据，都是实测（`grep` 调用点）而非推测。

---

## 1. 台账正文

| # | 关注点 | 家 A（旧） | 家 B（新） | **谁在跑数据** | 证据 | 收口动作 |
|---|---|---|---|---|---|---|
| 1 | **局域网发现（UDP 广播/组播）** | `network/discovery.rs`（1089 行，含 `UdpSocket::bind` / `recv_from` / `send_to` 循环） | ~~`discovery/`（`trait` / `lan` / `manager`）~~ **已于 0-A2 删除** | **A（旧家，唯一家）** | 删除前的证据：`network/mod.rs:54` 起 `discovery::spawn(...)` 在跑；新家 `DiscoveryManager` 除 `discovery/mod.rs` 的 re-export 外**无任何调用点**，`LanDiscovery` 只出现在注释里 | ✅ **已收口（2026-09-24 架构复审 0-A2）**：未接线的抽象层整体删除，`discovery/` 现在只剩 `routed.rs` 的**配置解析**（`parse_endpoints` 等，活的）。原"收口动作"（把 socket 循环搬进 `discovery/lan.rs`）**作废** —— 那只是设想，从未有证据说它比旧家好 |
| 2 | **TCP 帧原语（4 字节大端长度 + payload）** | 曾内联在 `network/transport.rs` | `transport/tcp.rs`（356 行） | **B（新家）** | `network/transport.rs:40` `use crate::transport::tcp::{TcpReceiver, TcpSender}`；`:57` `tcp::write_bytes`；`:62` `tcp::read_bytes`；注释自述"单一真相源见 `transport::tcp`（P-A03）" | ✅ 已收口（只是 `TcpTransport` **结构体**仍未接线，见第 4 行） |
| 3 | **TCP 数据面 / 建链 / 心跳** | `network/transport.rs`（**8836 行**：建链竞态、Gossip 广播、中继切片、群密钥、E2EE 解密分发） | — | **A（旧家，单家）** | `state.rs:1042` 注释"心跳在 `network::transport` 每 5s 一次"；`ensure_link` / `should_dial` 都在此 | Phase 7：先回答"这个文件里有几个独立的变化原因"（行数**不是**判据），再按变化原因拆 |
| 4 | **传输抽象 / 通道分流** | — | `transport/mod.rs`（~~`Transport` trait + `TransportManager::route` + `Channel` + `LARGE_PAYLOAD_THRESHOLD`~~ **0-A2 已删**） | **只剩状态聚合，分流从来不在这里** | 删除前：`route()` 带 `#[allow(dead_code)]` + 注释"待蓝牙后端接入后再调用"，零调用点；`transport/lan.rs` 的 `send`/`broadcast` 是 `outbound.rs` 同名逻辑的**第二份实现**且零调用点。真正的分流：`network/dispatch.rs::message_priority`（语义分类）+ `mesh/selection.rs::pick_link`（选路）+ `file.rs::chunk_size_for_path`（按链路能力挑分片尺寸） | ✅ **已收口（0-A2）**：`transport/mod.rs` 现在只做 `ChannelStatus[]` 汇总 + "未编译 BLE 后端"时的开关兜底。**别再往这里加分流** |
| 5 | **BLE 中央角色（扫描/连接/握手）** | `network/ble.rs`（2236 行：扫描循环、握手验签、链路登记、去重、退避） | `transport/bluetooth.rs::driver`（btleplug 封装） | **两家都在跑**（分层，不是重复） | `network/ble.rs:42` `use ...::driver::{self, BleReader, BleWriter}`；实际调用 7 处：`:179 driver::adapter()`、`:386 driver::scan_peers()`、`:785 driver::connect()`、`:835 driver::BleConnection`、`:863 writer.send_frame()` | ✅ 这是**正常分层**（driver = 字节级、`network/ble.rs` = 策略级）。不要合并 |
| 6 | **BLE 外设角色（GATT server）** | — | `transport/bluetooth_peripheral.rs`(macOS) / `_windows.rs` / `ble_android.rs` | **B（新家，三家平台实现）** | `network/ble.rs:54-64` 按 `target_os` 分别 `use`；`transport/mod.rs:15,19,24` 的 `cfg(target_os)` 门控 | ✅ 已收口；Phase 4 给 Android 加了编译门禁 |
| 7 | **BLE 载荷预算 / 分片** | 曾有三份：`ble_framing.rs` 函数内匿名常量、`bluetooth.rs:110,112` 重复常量、macOS `central_payload_mtu` 自算一份、Android `payload_mtu` 自算一份 | `transport/ble_framing.rs`（唯一） | **B（新家，已收敛）** | Phase 3：`4.18.7→4.18.10` 连着四版修同一问题；Phase 4：Android 那份还有真 bug（`1..=512` 放行装不下分片头的值 ⇒ 整条链路发不出消息） | ✅ 已收口（`INV-P23` + `scripts/check-ble-constants.mjs` 三条判据） |
| 8 | **Mesh 路由 / 选路 / 中继策略** | — | `mesh/`（10 文件 2432 行） | **单家（活）** | 被 9 个外部文件引用：`network/{ble,transport,file}.rs`、`discovery/routed.rs`（0-A2 后 `discovery/` 只剩它，且它现在只保留配置解析、不再引用 mesh）、`commands.rs`、`state.rs`。⚠️ `MeshRouter::select_outgoing` **生产不走**（`outbound.rs:428` 说明：群洪泛按 `fanout` 截断会把成员静默切掉，所以只用 `exclude_source`） | ✅ **本仓库最成型的领域模块**（有 ADR-0013/0014 背书）。是"领域该长什么样"的参照 |
| 9 | **文件切片中继（BitTorrent 式分发）** | — | `file_relay.rs`（299 → **86 行**；原 `relay_manager.rs`，2026-09-17 重命名以消"relay"命名撞车） | **只剩接收侧** | 活：`network/transport.rs` 的 `RelayFileOffer`/`RelayChunk` 分支用 `begin_reassemble` + `add_chunk`，`sweep_stale_relay` 用 `sweep_stale_reassemblies`，`network/file.rs:671` 用 `MIN_CHUNK_SIZE`。删除前证据：发送侧整组（`split_bytes`/`slice_file*`/`register_send`/`next_chunk`/`is_send_done`/`plan_distribution`/`ack_chunk`/`finish_send`/`progress`/`active_sends` + `ChunkData`/`RelayPlan`/`DEFAULT`/`MAX_CHUNK_SIZE`）**零生产调用点**，整个 `impl` 块头上挂着 `#[allow(dead_code)]` | ✅ **发送侧已删（0-A2）**。⚠️ 两个遗留：① 顶栏"N 中继"原先取 `active_sends()`，而 `senders` 只有 `register_send` 会写 ⇒ **那个数永远是 0**；已改为数 `path_kind==Relay` 的活跃链路（`state::relay_circuit_count`，与 `relay.connected` 同判据）。② 接收重组**全量驻内存**（复审 P4），留给第 2 步 |
| 10 | **内容生命周期（文本/文件/图片统一）** | — | `content/`（4 文件 725 行） | **单家（活）** | `network/{transport,file}.rs`、`db.rs`、`commands.rs` 引用 | ✅ 已收口 |
| 11 | **二进制落盘 + 缓存清理** | — | `storage/`（2 文件 212 行） | **单家（活）** | `commands.rs:63` 用 `cache_cleaner::{CachePolicy, CleanupReport}` | ✅ 已收口 |

**统计（2026-09-24 架构复审 0-A2 之后）**：11 个关注点里 **0 个双家未收口**（原"局域网发现"靠
**删掉未接线的那一家**收口，而不是搬过去）、**1 个三家但属正常分层**（BLE：driver / 策略 / 平台外设）、
**0 个部分未接线**（原"传输抽象"与"文件切片中继"的未接线部分已删）、其余单家/已收口。
⚠️ 但**"0 双家"不等于"边界都清楚了"**：`network/transport.rs` 仍是单家里的巨型文件（复审 P8），
`db::` 被 43 个文件直接引用（复审 P5）—— 那是"一个家里堆了太多变化原因"，属另一种结构问题。

---

## 2. 命名撞车（作者已经要用注释来区分了 —— 这就是结构问题的化石）

| 撞车 | 两处 | 现状 |
|---|---|---|
| 两个 `transport.rs` | `network/transport.rs`（数据面 + 建链 + 心跳，活）vs `transport/{mod,tcp}.rs` | ⚠️ **改名仍未消解，但语义已经不同**：0-A2 之后 `transport/mod.rs` 只剩**状态聚合**（不再有"新栈"的通道抽象与分流），`transport/tcp.rs` 是**唯一**的帧原语（4B 大端长度 + payload，被 `network/transport.rs` 复用）。搜索 `transport` 还是会同时命中两处，但今天读错了不会误以为找到了分流点 |
| 两个 "relay" | `file_relay.rs`（**文件切片中继**；原 `relay_manager.rs`，2026-09-17 改名）vs `mesh::router`（**路由转发**） | ✅ **已消解**。模块名自带语义，不再需要 `lib.rs:19` 那句"无关"注释 |
| 两个 "discovery" | `network/discovery.rs`（活）vs `discovery/`（未接线） | ✅ **已消解（0-A2）**：`discovery/` 现在只剩 `routed.rs` 的**配置解析**，`mod.rs` 顶部直接写明"这里没有发现机制，真正在跑的是 `network/discovery.rs`" |
| 两个 payload budget | 已收敛（Phase 3/4） | ✅ 已消解 |

**剩余建议**（Phase 6/7 的最小动作，不需要重构）：
- 两个 `transport.rs` 的消解必须等迁移收口，**现在不要动**（改名会让"哪个是活的"更难判断）。

---

## 3. 过期的"未接线"声明（文档与代码冲突 —— 按项目规矩必须上报，不得静默择一）

`docs/AI_ENGINEERING_INDEX.md` 的规矩：「若文档与可执行代码/测试冲突，**不得静默择一**，
必须上报并判定是文档过期还是实现过期」。以下是本台账实测发现的冲突：

| # | 声明 | 实测 | 判定 |
|---|---|---|---|
| 1 | `transport/bluetooth.rs:37`：「## 状态：**已实现、尚未接线**（7-e 的一半）」 | `network/ble.rs` 有 **7 处**实际调用（`driver::adapter` / `scan_peers` / `connect` / `BleConnection` / `send_frame`） | ❌ **文档过期**（至少对 `driver` 部分）。准确表述应是：**`driver` 已接线；未接线的是同文件的 `BluetoothTransport`（`Transport` trait 实现，仅服务 `status()` 展示）与 `TransportManager::route()`** |
| 2 | `transport/tcp.rs:14`：`#![allow(dead_code)] // 旁路阶段：待接线后移除` | `network/transport.rs:40,57,62` 用了它的 `TcpReceiver` / `TcpSender` / `write_bytes` / `read_bytes` | ❌ **文档过期（部分）**：帧原语**已接线**并被自述为"单一真相源（P-A03）"；未接线的只是 `TcpTransport` 结构体 |
| 3 | `transport/mod.rs:112`：「待蓝牙后端接入后，在消息/文件发送路径中调用以真正分流」 | `route()` 确实无生产调用点 | ✅ **准确** |
| 4 | `transport/mod.rs:122` + 注释：「开了 `bluetooth` feature 时**不用它**……只服务未编译 BLE 后端的默认构建」 | 与 `network/ble.rs` 的分工一致 | ✅ **准确**（这是"条件化 allow"的正面样本） |
| 5 | `lib.rs:1920` / `network/ble.rs:153`：`TransportManager` 里的 `BluetoothTransport` 是"尚未接线"的占位实现 | 确实只用于 `status()` | ✅ **准确** |

> 第 1、2 条与 Phase 3 修掉的 `ble_framing.rs` 那句"接入前没有任何生产调用点"是**同一个病**：
> **"待接线"的注释在接线之后没人回头改**。而且它们都挂着 `#[allow(dead_code)]`，
> 把编译器本来会给出的提示一起静音了 —— 于是过期声明可以存活很久。
>
> 本轮**只上报、不改**（Phase 5 是只读审计）。修正它们属于 Phase 6/7 的最小动作。

---

## 4. 收口顺序（Phase 6/7 的输入）

按「风险 ÷ 护栏成熟度」排：

| 序 | 关注点 | 为什么排这个位置 | 前置条件 |
|---|---|---|---|
| 1 | ~~修正 §3 的第 1、2 条过期声明~~ | ~~零风险（只改注释），且**不修就会继续误导**下一次判断~~ | 无 |
| 2 | ~~`relay_manager.rs` 改名 ~~ | ~~一行 `git mv` + 3 处引用，消掉一处命名撞车~~ | 无 |
| 3 | **发现**（唯一真正的双家） | 有非空转护栏（Presence 那条）、新旧边界清晰（`discovery/trait.rs` 已就位） | 需要真机验证广播行为（类似 `loopback_broadcast_works` 的做法） |
| 4 | 打开 `domains.yml` 里第一个 `enforce` | 上面某条收口完成后，才能"开一个" | 该领域边界成为事实 |
| 5 | TCP 数据面拆分（`network/transport.rs` 8836 行） | **最后**：最大、最活、改动风险最高 | 先回答"有几个独立变化原因"（行数不是判据） |
| 6 | `db::` 穿透传输层 | 13 个文件引用，横跨 network/transport/mesh —— 架构上最值得收口 | 需要先有 Application 层（或至少约定"传输层不得直接写库"） |

---

## 5. 维护规则

1. **改了某个关注点的"活路径"，必须同时更新本台账与 `domains.yml` 的 `active_home`** ——
   否则地图开始说谎。`scripts/check-domain-map.mjs` 能守住"路径存在/不重复/不遗漏"，
   但**守不住"哪个是活的"** —— 那一栏只能靠人诚实。
2. 新增关注点（或新增第二个家）时，先加一行台账再动手。
3. 迁移收口（删掉老家）时，把该行从"双家"改成"单家"，并删掉 `domains.yml` 里的
   `second_home` / `second_home_status`。
