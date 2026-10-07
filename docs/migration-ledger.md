# 迁移台账（Migration Ledger）

> **这份台账只回答一个问题：每个关注点有几个"家"，哪个在跑数据？**
>
> 配套：`docs/domains.data.mjs`（机器可读的领域图，`.mjs` 是因为工具链里没有 YAML
> 解析器 —— 见该文件头的说明）。两份文件必须一致 —— 由
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

**所以 `docs/domains.data.mjs` 里每个领域强制带一个 `activeHome` 字段** ——
不回答这个问题，领域图就是一份"AI 会照着执行、但与真实活路径不符"的地图，
**比没有地图更危险**。

本台账的每一条都给出 `file:line` 证据，都是实测（`grep` 调用点）而非推测。

---

## 1. 台账正文

| # | 关注点 | 家 A（旧） | 家 B（新） | **谁在跑数据** | 证据 | 收口动作 |
|---|---|---|---|---|---|---|
| 1 | **局域网发现（UDP 广播/组播）** | `network/discovery.rs`（含 `UdpSocket::bind` / `recv_from` / `send_to` 循环） | ~~`discovery/`（`trait` / `lan` / `manager`）~~ **已于 0-A2 删除** | **A（旧家，唯一家）** | 删除前的证据：`network/mod.rs:54` 起 `discovery::spawn(...)` 在跑；新家 `DiscoveryManager` 除 `discovery/mod.rs` 的 re-export 外**无任何调用点**，`LanDiscovery` 只出现在注释里 | ✅ **已收口（2026-09-24 架构复审 0-A2）**：未接线的抽象层整体删除，`discovery/` 现在只剩 `routed.rs` 的**配置解析**（`parse_endpoints` 等，活的）。原"收口动作"（把 socket 循环搬进 `discovery/lan.rs`）**作废** —— 那只是设想，从未有证据说它比旧家好 |
| 2 | **TCP 帧原语（4 字节大端长度 + payload）** | 曾内联在 `network/transport.rs` | `transport/tcp.rs` | **B（新家）** | 真实调用点在**第三处**：`network/transport/outbound.rs:45` `transport::tcp::write_bytes`、`:105` `transport::tcp::read_bytes_capped`；`outbound.rs:46-47` 注释自述"单一真相源见 `transport::tcp`（P-A03）"。⚠️ 原先此处写的 `network/transport.rs:40,57,62` 是**失效行号**（该文件对 `crate::transport::tcp` 零引用），已修正 | ✅ 已收口（只是 `TcpTransport` **结构体**仍未接线，见第 4 行） |
| 3 | **TCP 数据面 / 建链 / 心跳** | `network/transport.rs`（**全仓最大的单文件**：建链竞态、Gossip 广播、中继切片、群密钥、E2EE 解密分发。行数现算 `wc -l src-tauri/src/network/transport.rs`，别抄进本文 —— 它逐月都在长） | — | **A（旧家，单家）** | `state.rs:1042` 注释"心跳在 `network::transport` 每 5s 一次"；`ensure_link` / `should_dial` 都在此 | Phase 7：先回答"这个文件里有几个独立的变化原因"（行数**不是**判据），再按变化原因拆 |
| 4 | **传输抽象 / 通道分流** | — | `transport/mod.rs`（~~`Transport` trait + `TransportManager::route` + `Channel` + `LARGE_PAYLOAD_THRESHOLD`~~ **0-A2 已删**） | **只剩状态聚合，分流从来不在这里** | 删除前：`route()` 带 `#[allow(dead_code)]` + 注释"待蓝牙后端接入后再调用"，零调用点；`transport/lan.rs` 的 `send`/`broadcast` 是 `outbound.rs` 同名逻辑的**第二份实现**且零调用点。真正的分流：`network/dispatch.rs::message_priority`（语义分类）+ `mesh/selection.rs::pick_link`（选路）+ `file.rs::chunk_size_for_path`（按链路能力挑分片尺寸） | ✅ **已收口（0-A2）**：`transport/mod.rs` 现在只做 `ChannelStatus[]` 汇总 + "未编译 BLE 后端"时的开关兜底。**别再往这里加分流** |
| 5 | **BLE 中央角色（扫描/连接/握手）** | `network/ble.rs`（扫描循环、握手验签、链路登记、去重、退避） | `transport/bluetooth.rs::driver`（btleplug 封装） | **两家都在跑**（分层，不是重复） | `network/ble.rs:42` `use ...::driver::{self, BleReader, BleWriter}`；实际调用 7 处：`:179 driver::adapter()`、`:386 driver::scan_peers()`、`:785 driver::connect()`、`:835 driver::BleConnection`、`:863 writer.send_frame()` | ✅ 这是**正常分层**（driver = 字节级、`network/ble.rs` = 策略级）。不要合并 |
| 6 | **BLE 外设角色（GATT server）** | — | `transport/bluetooth_peripheral.rs`(macOS) / `_windows.rs` / `ble_android.rs` | **B（新家，三家平台实现）** | `network/ble.rs:54-64` 按 `target_os` 分别 `use`；`transport/mod.rs:15,19,24` 的 `cfg(target_os)` 门控 | ✅ 已收口；Phase 4 给 Android 加了编译门禁 |
| 7 | **BLE 载荷预算 / 分片** | 曾有三份：`ble_framing.rs` 函数内匿名常量、`bluetooth.rs:110,112` 重复常量、macOS `central_payload_mtu` 自算一份、Android `payload_mtu` 自算一份 | `transport/ble_framing.rs`（唯一） | **B（新家，已收敛）** | Phase 3：`4.18.7→4.18.10` 连着四版修同一问题；Phase 4：Android 那份还有真 bug（`1..=512` 放行装不下分片头的值 ⇒ 整条链路发不出消息） | ✅ 已收口（`INV-P23` + `scripts/check-ble-constants.mjs` 三条判据） |
| 8 | **Mesh 路由 / 选路 / 中继策略** | — | `mesh/`（目录，现算 `ls src-tauri/src/mesh/*.rs \| wc -l`） | **单家（活）** | 被 9 个外部文件引用：`network/{ble,transport,file}.rs`、`discovery/routed.rs`（0-A2 后 `discovery/` 只剩它，且它现在只保留配置解析、不再引用 mesh）、`commands.rs`、`state.rs`。⚠️ `MeshRouter::select_outgoing` **生产不走**（`outbound.rs:428` 说明：群洪泛按 `fanout` 截断会把成员静默切掉，所以只用 `exclude_source`） | ✅ **本仓库最成型的领域模块**（有 ADR-0013/0014 背书）。是"领域该长什么样"的参照 |
| 9 | **文件切片中继（BitTorrent 式分发）** | — | `file_relay.rs`（原 `relay_manager.rs`，2026-09-17 重命名以消"relay"命名撞车） | **只剩接收侧** | 活：`network/transport.rs` 的 `RelayFileOffer`/`RelayChunk` 分支用 `begin_reassemble` + `add_chunk`，`sweep_stale_relay` 用 `sweep_stale_reassemblies`，`network/file/relay_push.rs:94` 用 `MIN_CHUNK_SIZE`（2026-10-07 该函数搬进 `file/relay_push.rs`，行号现读）。删除前证据：发送侧整组（`split_bytes`/`slice_file*`/`register_send`/`next_chunk`/`is_send_done`/`plan_distribution`/`ack_chunk`/`finish_send`/`progress`/`active_sends` + `ChunkData`/`RelayPlan`/`DEFAULT`/`MAX_CHUNK_SIZE`）**零生产调用点**，整个 `impl` 块头上挂着 `#[allow(dead_code)]` | ✅ **发送侧已删（0-A2）**。⚠️ 两个遗留：① 顶栏"N 中继"原先取 `active_sends()`，而 `senders` 只有 `register_send` 会写 ⇒ **那个数永远是 0**；已改为数 `path_kind==Relay` 的活跃链路（`state::relay_circuit_count`，与 `relay.connected` 同判据）。② 接收重组**全量驻内存**（复审 P4），留给第 2 步 |
| 10 | **内容生命周期（文本/文件/图片统一）** | — | `content/`（目录） | **单家（活）** | `network/{transport,file}.rs`、`db.rs`、`commands.rs` 引用 | ✅ 已收口 |
| 11 | **二进制落盘 + 缓存清理** | — | `storage/`（目录） | **单家（活）** | `commands.rs:63` 用 `cache_cleaner::{CachePolicy, CleanupReport}` | ✅ 已收口 |

**统计（2026-09-24 架构复审 0-A2 之后）**：11 个关注点里 **0 个双家未收口**（原"局域网发现"靠
**删掉未接线的那一家**收口，而不是搬过去）、**1 个三家但属正常分层**（BLE：driver / 策略 / 平台外设）、
**0 个部分未接线**（原"传输抽象"与"文件切片中继"的未接线部分已删）、其余单家/已收口。
⚠️ 但**"0 双家"不等于"边界都清楚了"**：`network/transport.rs` 仍是单家里的巨型文件（复审 P8），
`db::` 被大量文件直接引用（复审 P5；现算口径见 `scripts/check-domain-deps.mjs` 的扫描范围）——
那是"一个家里堆了太多变化原因"，属另一种结构问题。

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
| 1 | ~~`transport/bluetooth.rs:37`：「## 状态：**已实现、尚未接线**（7-e 的一半）」~~ | ~~`network/ble.rs` 有 **7 处**实际调用~~ | ✅ **判定作废（2026-10-03 复核）**：源码已改为 `transport/bluetooth.rs:44`「## 状态：**已接线**（2026-09-16 核对调用点后修正此前的"尚未接线"）」。下方"未接线的只是 `BluetoothTransport`"的补充说明仍然成立 |
| 2 | ~~`transport/tcp.rs:14`：`#![allow(dead_code)] // 旁路阶段：待接线后移除`~~ | ~~`network/transport.rs:40,57,62` 用了它的 `TcpReceiver` / `TcpSender` / `write_bytes` / `read_bytes`~~ | ✅ **判定作废（2026-10-03 复核）**：源码已改为 `transport/tcp.rs:12-13`「## 接线状态：**大部分已接线**（2026-09-16 逐项核对调用点，修正了此前的"旁路阶段"）」。⚠️ 当时这行的证据行号本身就是错的（真实调用点在 `network/transport/outbound.rs:45,105`），所以"文档过期"这个结论虽成立，理由却指向了无关代码 |
| 3 | `transport/mod.rs` 的 `TransportManager::route()`：「待蓝牙后端接入后，在消息/文件发送路径中调用以真正分流」 | `route()` **已于 0-A2 整体删除**（该文件现在只剩 `ChannelStatus[]` 汇总） | ⚠️ **本条已随实现删除而失效**（2026-10-03 复核）：原文引 `transport/mod.rs:112`，那一行今天是 `running: self.bluetooth.running()`；`route`/`allow(dead_code)` 在该文件里**零命中**。判定本身曾成立（`route()` 确实无生产调用点），现在该问的是"要不要把分流加回来"，不是"它接到哪了" |
| 4 | `transport/bluetooth.rs:8`：「`BluetoothTransport` … ⚠️ **占位**：只服务 `TransportManager::status()` 的展示（`running` 恒 `false`、`peer_count` 恒 0）」 | 与 `network/ble.rs` 的分工一致 | ✅ **准确**（这是"条件化占位"的正面样本） |
| 5 | `transport/bluetooth.rs:643` + 模块头 `:8`：`BluetoothTransport` 是"尚未接线"的占位实现 | 确实只用于 `status()` | ✅ **准确**。⚠️ 原文引 `lib.rs` 第 1920 行与 `network/ble.rs` 第 153 行 —— **两处行号都已失效**：`lib.rs` 全文只有 560 行，而 `BluetoothTransport` 在 `lib.rs` 里**零引用**（真正定义在 `transport/bluetooth.rs:643`）；`network/ble.rs` 里 `BluetoothTransport` 与 `TransportManager` 也都零引用。原文那两个行号按不复写（它们是**历史读数**，门禁只管"当前事实"的引用） |

> 第 1、2 条与 Phase 3 修掉的 `ble_framing.rs` 那句"接入前没有任何生产调用点"是**同一个病**：
> **"待接线"的注释在接线之后没人回头改**。而且它们都挂着 `#[allow(dead_code)]`，
> 把编译器本来会给出的提示一起静音了 —— 于是过期声明可以存活很久。
>
> **2026-10-03 更新**：第 1、2 条**已经修好了**（Phase 6/7 干的，2026-09-16），本表曾长期挂着
> 已作废的红牌，让下一个人重新调查一遍当天就解决的事。留着它们不是为了记录，是为了提醒：
> **这类"过期声明"不会自己消失，也不会被任何现有门禁抓到** —— `check-doc-numbers.mjs`
> 扫的是文档里的**硬数字**，不扫"源码注释里关于接线的声明"，也不校验 `file:line` 是否
> 落在真符号上。本次审计已实测到第三个实例（`transport/tcp.rs` 模块头，注释在 2026-09-16
> 核过、2026-10-03 复核时行号已全部腐烂）。
> **门禁已落地**：`scripts/check-doc-citations.mjs`（已接进 `verify.mjs` 第 7 步）——
> 扫活文档里的 `file:line`，判「文件存在」+「行号在范围内」。它上线当场抓到本台账两处
> 断裂：一处引的 `transport/mod.rs` 行号**超出该文件行数**、一处引的 `lib.rs` 行号超出全文五倍
> （按原样复述这两条行号就是新断裂 ⇒ 说明「提到某个行号」和「断言某个行号」必须分开写），
> 已在下面 §3 第 3–5 条重钉。
> ⚠️ **它的已知边界**（不是漏了）：只判"范围"，**不判那一行是不是文档说的那个符号** ——
> 符号级校验要解析 Rust/TS 语法，解析错了门禁会静默失效，比没有门禁更危险。
> 后果举例：§3 第 3 条引的 `route()` 已被 0-A2 整体删除，行号仍在范围内、门禁判绿，
> 但**那一行今天根本不是 `route()`**。所以本台账的约定仍是：
> **行号只当"大致位置"，判"是不是那个符号"由人现算 `grep -n`。**

---

## 4. 收口顺序（Phase 6/7 的输入）

按「风险 ÷ 护栏成熟度」排：

| 序 | 关注点 | 为什么排这个位置 | 前置条件 |
|---|---|---|---|
| 1 | ~~修正 §3 的第 1、2 条过期声明~~ | ✅ **已完成**（当时列为第 1 步，Phase 6/7 已改，见 §3 末尾） | — |
| 2 | ~~`relay_manager.rs` 改名 ~~ | ✅ **已完成**（2026-09-17） | — |
| 3 | **发现**（唯一真正的双家） | 有非空转护栏（Presence 那条）、新旧边界清晰（`discovery/trait.rs` 已就位） | 需要真机验证广播行为（类似 `loopback_broadcast_works` 的做法） |
| 4 | 打开 `domains.data.mjs` 里第一个 `enforce` | 上面某条收口完成后，才能"开一个" | 该领域边界成为事实 |
| 5 | TCP 数据面拆分（`network/transport.rs`，全仓最大单文件） | **最后**：最大、最活、改动风险最高 | 先回答"有几个独立变化原因"（行数**不是**判据） |
| 6 | `db::` 穿透传输层 | 十几个文件引用，横跨 network/transport/mesh —— 架构上最值得收口 | 需要先有 Application 层（或至少约定"传输层不得直接写库"） |

---

## 5. 维护规则

1. **改了某个关注点的"活路径"，必须同时更新本台账与 `domains.data.mjs` 的 `activeHome`** ——
   否则地图开始说谎。`scripts/check-domain-map.mjs` 能守住"路径存在/不重复/不遗漏"，
   但**守不住"哪个是活的"** —— 那一栏只能靠人诚实。
2. 新增关注点（或新增第二个家）时，先加一行台账再动手。
3. 迁移收口（删掉老家）时，把该行从"双家"改成"单家"，并删掉 `domains.data.mjs` 里的
   `secondHome`（以及它的状态说明字段）。
