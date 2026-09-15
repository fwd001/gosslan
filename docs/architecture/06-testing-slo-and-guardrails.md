# 06 · 测试、SLO 与护栏

- Status: Normative
- 上游：`01`（引擎纯状态机）、`03`（SimulatedLink）
- 决策 ADR：`../adr/0025-deterministic-mesh-simulation.md`

---

## 0. 为什么这一章是「稳定」的关键

现状「蓝牙总是很不稳定」无法收敛的根本原因：**只能靠 2–3 台真机复现**，没有确定性回归。
V5 的解法：**真实引擎 + 模拟链路 + 虚拟时钟**，把多节点场景变成毫秒级单测，再叠加真机矩阵。

---

## 1. SLO（先量化「稳」，再谈「改」）

| 指标 | 目标（3 设备静态场景，示例，需用户确认） | 采集点 |
|---|---|---|
| 发现→建链成功率 | ≥ 95% | `[DISCOVERY]`→`[SESSION]` 日志 |
| 发现→建链 P95 | < 10s | 同上时间差 |
| 重连 P95（断链后） | < 15s | `[DISCONNECT]`→`[SESSION]` |
| 健康链路误拆率 | 0（15s 判不健康、45s 拆除） | watchdog 日志 |
| 单聊消息投递率（含 ACK） | ≥ 99% | outbox/ACK |
| 假在线率（有链路但发不出） | 0 | `[SEND]` 失败率 |
| 大文件（1MB）完成率 | ≥ 99% | file 进展日志 |

所有指标必须能在应用内诊断面板（已有 `BleDiag`/`ble_scan` 框架）看到。

---

## 2. 确定性仿真（`SimulatedMesh`）

照抄 bitchat `SimulatedMesh` 的骨架，但要适配 Rust/tokio：

- **真实引擎**：不复制逻辑，直接跑生产引擎；
- **模拟链路**：`SimulatedLink` 实现 `03` 的端口，可注入 `LinkUp`/`BytesIn`/丢包/延迟/断链；
- **虚拟时钟**：定时器经可注入调度器，支持 `tokio::time::pause()`（绝不 sleep）；
- **收敛断言**：`pump()` 反复投递直到静止；超出轮数即 **fail**（这就是「中继风暴」的边界断言）；
- **能力**：`silence(a,b)`（射频静默但保留绑定）、`duplicateLinks(a,b)`（双链路）、
  `emittedPackets(node)`（攻击者抓包/重放）。

### 必测场景（≥8）
1. 三角拓扑 A→C 经 B 中继，TTL 正确递减；
2. 重复洪泛去重（多路径到达只转发一次）；
3. 断链重连 → outbox 重发 → ACK；
4. 地址轮换 → 冗余链路择新；
5. TTL 边界（自报 255 被 clamp）；
6. partition heal（分区恢复收敛）；
7. **BLE ↔ LAN 跨 transport 中继**（TTL 跨 transport 连续）；
8. BitChat OpaqueExternal 经网关转发且不落库。

### 保真边界（必须写明，避免误信）
模拟链路**不覆盖**：射频时序、真实 MTU、GATT 背压细节。这些由真机矩阵覆盖。

---

## 3. 护栏清单（每阶段必须绑定）

| 类型 | 形式 | 例子 |
|---|---|---|
| 纯函数单测 | 直接断言策略输出 | 调度打分/退避/占空比/TTL clamp |
| 结构护栏 | 源码 grep 断言 | 「链路层不 import protocol」「引擎外部不锁 MeshState」 |
| 非空转护栏 | 故意改坏源码，断言测试 FAIL，再恢复 | `scripts/verify-guards.py`（**必须后台跑**） |
| 影子双跑 | 新引擎镜像状态，与旧路径 diff | Phase 3b |
| 真机矩阵 | 见 §4 | 每阶段发一个可安装包 |

命令：
```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib --features bluetooth
npm test
npm run build
npm run version:check
# 必须后台跑，避免中途被 kill 留下改坏的源码：
python3 scripts/verify-guards.py
```

---

## 4. 真机矩阵（射频不可自动化）

| 场景 | 设备 | 判据 |
|---|---|---|
| LAN 双机同 Wi-Fi | Mac + Windows | 聊天/文件/群/好友全通（**黄金回归**） |
| 蓝牙双机 | Android + macOS | Test A–F |
| 手机 ↔ Windows | Android + Windows | 双向发现/连接/消息 |
| 三机 mesh | Android + macOS + Windows | A→C 经 B 中继；群消息可达 |
| 关蓝牙 | 任一 | LAN 完全不受影响（C4） |
| iOS（Phase 7） | iPhone + Android/mac | 双向发现/后台恢复 |
| BitChat 互通（Phase 6） | 真 BitChat 设备 | 能收其包并转发；关开关零影响 |

---

## 5. 不变量

```text
INV-NET-50  每个阶段必须有可自动化的验收（纯函数或仿真）。
INV-NET-51  仿真只跑生产引擎，不复制协议逻辑。
INV-NET-52  仿真收敛失败必须 fail，而不是超时跳过。
INV-NET-53  真机矩阵未过，不得宣布阶段完成。
```
