# Gosslan V5 架构规范：LAN + BLE 无感融合的去中心化网络

- Date: 2026-09-14
- Status: **Normative（规范）** —— 本目录是 Gosslan 目标架构的唯一权威来源
- 版本目标：**V5.0**（下一个大版本）
- 前置阅读：`../AI_ENGINEERING_INDEX.md`、`../protocol-invariants.md`、`../adr/`

> **为什么需要本目录**：项目原来的设计总纲在 `.workbuddy/mesh-task/`，而 `.workbuddy/` 被 `.gitignore`
> 排除、**不入库** ⇒ 代码库里没有一份可提交、可版本化、可与代码对齐的目标架构。
> 本目录把它落成仓库内的规范，并已按真实代码逐条核对（2026-09-14：`state.rs` / `network/ble.rs` /
> `network/transport.rs` / `mesh/`）。

---

## 0. 一句话

> **一台设备 = 一个节点；LAN 与蓝牙只是它的两条链路；两者的融合发生在「一个串行 mesh 引擎」里，
> 而不是在 UI、发现层或某个传输实现里。**

---

## 1. 目标

1. **蓝牙设备无感加入去中心化局域网**：用户不选通道、不改设置；BLE-only 设备也能与 LAN 用户正常聊天、
   收发文件/群消息/好友请求/已读。
2. **LAN 现有全部聊天功能零回归**：消息、文件、群、好友、已读、通知，行为逐字节不变（见 §6 契约）。
3. **蓝牙设备可给 BitChat 做透明中继**：不解密、不落库、不建其身份/频道，只搬运不透明字节。
4. **覆盖 Android / iOS / macOS / Windows** 四个平台。
5. 最终达到「像 BitChat 一样流畅」的链路质量：连接成功率高、重连快、假在线少、消息不卡「发送中」。
6. **允许为流畅度小幅调整上层接口**（交互骨架已定；仅在架构需要时做最小改动，见 `07-roadmap-v5.md`）。

## 2. 非目标（明确不做）

- 不解密 BitChat 私聊、不落库、不进 gossip、不复制其消息/身份/频道/UI/DB 设计。
- 不做 Nostr / Tor / geohash 那层社交产品。
- 不引入 QUIC / mDNS / 服务端中继 / 账号系统 / 复杂 DHT。
- 不为「未来可能支持」提前加抽象（YAGNI）。

---

## 3. 三条硬约束（任何设计不得违反）

1. **LAN 零回归**：蓝牙是**加法**；关掉蓝牙 = 与今天一致（§6）。
2. **无感**：用户永远不需要理解「这条消息走了 LAN 还是蓝牙还是中继」。
3. **可落地**：每个阶段可编译、可测试、可回退；不允许一次性重写。

---

## 4. 核心原则（P1–P7）

| 编号 | 原则 |
|---|---|
| P1 | **Node 是节点，Transport 是连接方式，Discovery 是找节点，Router 决定往哪走。** |
| P2 | 一个 `Peer` 可以有多条 `Connection`；身份 = `device_id`（不是 IP、不是蓝牙地址）。 |
| P3 | Transport 只搬字节，**不允许**理解 `ChatMessage`/`FriendRequest`/文件分片等业务类型。 |
| P4 | 去重与 TTL **只有一份**，在 mesh 引擎里，跨 transport 生效。 |
| P5 | mesh 引擎是**单串行状态域**；射频/链路状态只属于链路层；UI 读 lock-backed 快照。 |
| P6 | 外部帧（BitChat）是**不透明载荷**，走同一条转发流水线，绝不进入业务处理。 |
| P7 | 每个阶段可单独回退；LAN 路径永远可退回当前实现。 |

---

## 5. 目标架构总图

```text
┌──────────────────────── App / UI（Vue + Tauri commands） ────────────────────────┐
│  只读 lock-backed 快照；UI 不感知传输（最多显示「直连 / 中继」）                  │
└───────────────────────────────┬──────────────────────────────────────────────────┘
                                │ MeshEvent / MeshCommand
┌───────────────────────────────▼──────────────────────────────────────────────────┐
│                      MeshEngine（单串行 actor / mailbox）                         │
│  线编解码 · 全局去重 · TTL · Peer/Connection 图 · relay 授权 · 路由/网关 ·         │
│  可靠层（outbox/ACK/read/seq）· 特性分发                                           │
│  ── 只产出 Effect（PersistMessage / WriteAck / SendFrame…），不直接碰 DB/射频 ──  │
└───────────────────────────────┬──────────────────────────────────────────────────┘
                                │ LinkEvent / LinkCommand
        ┌───────────────────────┼────────────────────────┐
        ▼                       ▼                        ▼
    LanLink                  BleLink                OpaqueExternalLink
  (UDP 发现 + TCP)          (平台 GATT)              (BitChat 桥)
  【语义冻结】                 │                        │
        └─────────────── Platform Link Layer ──────────┘
        ┌───────────────────────┼────────────────────────┐
        ▼                       ▼                        ▼
  macOS/iOS: CoreBluetooth  Android: btleplug +      Windows: btleplug +
  central + peripheral      Kotlin GATT server        WinRT GattServiceProvider
  (objc2, 共享一份代码)      (JNI)
                                 │
                    Persistence Worker（DB worker）
```

---

## 6. LAN 零回归契约（硬约束）

| 编号 | 契约 | 如何保证 |
|---|---|---|
| C1 | **线格式不变**：`Message`/`GossipEnvelope` 序列化、`Hello` 签名串、E2EE、协议版本 | 不改 `protocol.rs`；结构护栏 |
| C2 | **语义不变**：outbox / ACK / 已读 / seq / 文件 / 好友 / 群 | 只搬状态位置，不改状态机；golden 测试 |
| C3 | **发现不变**：UDP 广播/组播/`who_has` 与端口绑定 | `network/discovery.rs` 不改语义；不并行双绑端口 |
| C4 | **关蓝牙 = 今天**：`bt_enabled=false` 时与当前实现逐字节一致 | 特性开关 + 对照测试 |
| C5 | **影子双跑**：新引擎先镜像状态并比对旧路径结果，零差异后才切读写 | 灰度开关 + diff 日志 |
| C6 | 新路径稳定前**不删** `state.links` / `state.peers` / `network::discovery` | 分阶段删除 |

对应的公开不变量见 `../protocol-invariants.md` 的 `INV-NET-*` 段（本目录负责定义，那里负责登记）。

---

## 7. BLE 无感融合：机制概览

1. **BLE 只是第三种 Link**：与 LAN 走同一个引擎、同一份去重/TTL、同一套 `Message`。
2. **双链路节点自动成为网关**：A(BLE)—B(LAN+BLE)—C(LAN) 时，B 把 A 的帧按引擎转发规则送进 LAN，
   反之亦然；节点不需要声明「我是网关」。
3. **邻居列表跨 transport 取并集**，随后随 announce 广播 ⇒ C 能「看到」A 存在（即使 C 根本没有蓝牙）。
4. **直连优先，无直连才穿网关**：`pick_link` 仍是 LAN > Routed > BLE；有中继路径时按源路由/洪泛。
5. **UI 无感**：用户只看到「在线/直连/中继」，不出现「请切换到蓝牙」这类提示。

细节见 `04-mesh-engine-and-routing.md` 与 `05-radio-policy.md`，决策见 `../adr/0022-ble-seamless-lan-join.md`。

---

## 8. 文档索引

> **先看这一页**：`00-v5-one-pager.md`（给用户拍板用的批准版）。

| 文件 | 内容 |
|---|---|
| `00-v5-one-pager.md` | **一页纸**：为什么改、改成什么样、你的决策、怎么开工 |
| `README.md`（本文） | 规范总览、原则、LAN 契约、索引 |
| `01-layers-and-concurrency.md` | 分层边界 + 并发所有权 + 同步边顺序 + 从现状迁移 |
| `02-data-model.md` | 唯一 Peer/Connection/Endpoint/Health 模型；两套模型收敛决策 |
| `03-link-layer-and-platforms.md` | LinkEvent/LinkCommand 端口；四平台适配器；iOS 方案 |
| `04-mesh-engine-and-routing.md` | 引擎职责、事件/效果、路由/网关、BitChat 中继、DB 边界 |
| `05-radio-policy.md` | 连接预算/扫描占空比/冗余链路/自愈/功率 等纯策略与参数 |
| `06-testing-slo-and-guardrails.md` | 确定性仿真、SLO 度量、护栏清单、真机矩阵 |
| `07-roadmap-v5.md` | V5.0 分阶段实施计划（每阶段交付/验收/回退） |
| `08-implementation-map.md` | **实现映射**：Phase → 文件/符号/测试/回退 |
| `09-decisions-and-readiness.md` | **开工门禁**：需你拍板的决策 + 就绪清单 + 授权流程 |

## 9. ADR 索引（新增）

| ADR | 决策 |
|---|---|
| `../adr/0020-layered-transport-and-single-mesh-engine.md` | 分层 + 单串行 mesh 引擎 |
| `../adr/0021-single-peer-connection-model.md` | 收敛为一套 Peer/Connection 模型 |
| `../adr/0022-ble-seamless-lan-join.md` | BLE 无感加入 LAN + 网关语义 + LAN 契约 |
| `../adr/0023-bitchat-dual-stack-relay.md` | BitChat 双栈透明中继 |
| `../adr/0024-cross-platform-link-layer-and-ios.md` | 跨平台链路层 + iOS 支持 |
| `../adr/0025-deterministic-mesh-simulation.md` | 确定性仿真与链路策略 |

## 10. 术语

| 术语 | 含义 |
|---|---|
| Node | 一台设备，身份 = `device_id`（`gosslan-…`） |
| Peer | 引擎里对某个 Node 的聚合视图（身份 + 能力 + 多条 Connection） |
| Connection | Peer 的一条可达路径（endpoint + path_kind + health + 背压队列） |
| Link | 链路层对一条物理连接（TCP/GATT）的抽象，用 `LinkId` 标识 |
| LinkEvent / LinkCommand | 链路层 → 引擎 / 引擎 → 链路层 的唯一消息面 |
| MeshEngine | 单串行状态域，拥有全部协议状态 |
| Effect | 引擎产出的副作用意图（落库/写帧/通知），由外部执行 |
| Gateway | 同时有多条不同 transport 链路、并替别人跨 transport 转发的节点（自动角色） |
| OpaqueExternal | BitChat 的不透明帧变体 |

## 11. 阅读顺序 & 验证

1. 先读本文 → `01` → `02`，理解边界与模型。
2. 再看 `03` → `04`，理解链路端口与引擎。
3. 实现前读 `05`（策略参数）与 `06`（怎么测）。
4. 动手按 `07-roadmap-v5.md` 逐阶段做，每阶段跑 `../AI_ENGINEERING_INDEX.md` 的必读与测试。

```bash
# Rust 单测（含 BLE feature）
cargo test --manifest-path src-tauri/Cargo.toml --lib --features bluetooth
# 前端纯逻辑测试
npm test
# 护栏非空转验证（必须后台跑，见 AI_RULES）
python3 scripts/verify-guards.py
```

> ⚠️ 若文档与可执行代码/测试冲突，**不要静默选一边**：先报告冲突，再判定是文档过期还是实现有误
> （与 `../AI_ENGINEERING_INDEX.md` 的 Rule 一致）。
