# ADR-0015: BLE Transport（Phase 7 / P3）

- Status: Proposed（**待用户审核**；实现按 feature 门推进，默认关闭）
- Date: 2026-09-12
- Owners: Gosslan
- Related:
  - 计划：`.workbuddy/mesh-task/P2-P3-开发计划.md` §3（Phase 7）
  - 设计文档：§8 身份与握手 / §19 Routing 与 Transport Selection / §34 Online 状态
  - 依赖决议：**引入 `btleplug`（唯一被允许的"先写 ADR 再加依赖"例外）**
  - CHANGELOG: `[Unreleased]`
  - Protocol: **需要新字段**（Hello 的可达标识）→ 必须走 ADR-0007 的版本化双读，不能塞进 `tcp_port`

---

## 1. Context（为什么需要第三种传输）

今天只有两种互联方式，覆盖不住用户的真实场景：

| 场景 | 今天能不能连 |
|---|---|
| 两台电脑在同一局域网 | ✅ UDP 发现 + TCP |
| 两台电脑跨网段/VPN | ✅ 用户手填「路由端点」 |
| **手机与电脑不在同一 WiFi**（家里路由器隔离、手机热点、出差） | ❌ 完全连不上 |

用户设备实况：Android 手机 + Mac + Windows，**两台电脑能组局域网，手机要靠蓝牙与两台电脑互联**。
BLE 的价值正在这里：它**不依赖 IP 网段**，天然满足"零配置、无感知"。

## 2. Decision（决定）

1. 引入 `btleplug`（Rust，三平台 GATT 客户端/服务端抽象），以 **Cargo feature `bluetooth`**
   隔离，**默认关闭**：不开 feature 时 `cargo build/test`、E2E、产物与今天**逐字节一致**。
2. **只做 BLE packet transport**：分帧、字节流、链路登记。不做聊天、不解密、不写 SQLite、
   不建 BitChat 用户/channel/UI。Gosslan 的业务层（Gossip/E2EE/Outbox）完全复用。
3. **身份只能由握手建立**（BLE 没有 IP/端口可当身份）：沿用 §8 的 `Hello → 验签 →
   device_id` 单向流程与 P1-2 已完成的**双向 Hello 验签**。BLE 端点（MAC/句柄）是
   **临时信息**（架构原则 P-A01），绝不作为身份。
4. **路径类型显式携带**：`Link` 增加 `path_kind: PathKind`，禁止从端点反推
   （今天 `path_kind_for(ip)` 已把"用户配置的私有网段 Routed 端点"误判成 LAN，
   顺带废掉了 D5 的修复）。`Endpoint` 用现有枚举 `mesh::Endpoint::{Tcp,Ble}`。
5. **落地顺序：macOS 打通 → Windows → Android**，每平台单独 commit；每个平台都能
   "只走 BLE"完成一次单聊收发才算通过。

## 3. 为什么是 btleplug（而不是别的）

| 方案 | 结论 |
|---|---|
| `btleplug` | ✅ 同一套 API 覆盖 macOS(CoreBluetooth)/Windows(WinRT)/Linux(BlueZ)/Android；纯 Rust；活跃维护 |
| 各平台原生绑定（CoreBluetooth/WinRT/Android Java） | ❌ 三套代码 + JNI/objc，项目体量撑不住（`objc2` 已够重） |
| `bluer`（BlueZ）/`ble-peripheral` | ❌ 只覆盖 Linux 或只有外设角色 |
| 走 IP 隧道（把 BLE 当串口） | ❌ 等于自造 L2 协议，工作量与风险都更高 |

**已知代价（必须写下来）**：btleplug 的 GATT 是"写特征 + notify 分片"，**不是字节流**，
所以 `writer_loop`/`reader_loop` 不能原样复用，需要一个收发适配器（分片/重组 + 流控），
以及 MTU 协商（默认 23 字节 → 协商后常见 185/512）。这是 Phase 7 的主要工作量。

## 4. 权限（已在本轮落地，先声明不请求）

| 平台 | 声明 | 备注 |
|---|---|---|
| iOS | `NSBluetoothAlwaysUsageDescription`（+ 旧键 `NSBluetoothPeripheralUsageDescription`），中英本地化 | **缺失会直接崩溃**，不是"权限被拒" |
| Android 12+ | `BLUETOOTH_SCAN`（`neverForLocation`）+ `BLUETOOTH_CONNECT` | 不加 `neverForLocation` 会额外要定位权限（聊天应用弹定位非常可疑） |
| Android ≤11 | `BLUETOOTH`/`BLUETOOTH_ADMIN`（`maxSdkVersion=30`）+ `ACCESS_FINE_LOCATION`（同上限） | 旧系统扫 BLE 必须有定位权限 |
| Android | `uses-feature android.hardware.bluetooth_le` `required=false` | 无蓝牙设备也能安装，功能降级为纯 LAN |
| macOS | 首次使用弹系统授权；沙盒需要 `com.apple.security.device.bluetooth` | **待补**（与 feature 同批加进 entitlements） |

## 5. Consequence

- ✅ 手机与电脑不在同一 WiFi 也能连；零配置（不需要填 IP/端口）。
- ✅ 默认关闭 ⇒ 现有局域网链路零风险；出问题可以只关 feature。
- ⚠️ 三平台成熟度差异大，Android 后台/权限最麻烦；**必须在每平台真机验证**，
  本仓库的自动化测试覆盖不到射频层。
- ⚠️ 新增第三种传输后，TTL/去重/选路记账必须只有一份（`handle_gossip` 第 4 步），
  死的 `MeshRouter::relay_enabled` 先收敛掉，否则三份记账必然漂移。
- ⚠️ 协议：BLE 侧上报可达标识需要 Hello 新字段 + 签名前缀升 v2 双读（ADR-0007）。

## 6. 分步与验收（每步 1 commit，全绿才进下一步）

| 步 | 内容 | 验收 |
|---|---|---|
| 7-a | 本 ADR + `bluetooth` feature 门骨架（`btleplug` 可选依赖） | 不开 feature 时 `cargo test --lib`/E2E 与今天一致 |
| 7-b | 收发适配器（分片/重组/流控/MTU），与 `transport/tcp.rs` 同构 | 单测：分片往返、乱序/丢片、超长拒绝 |
| 7-c | `Link.path_kind` 显式携带 + `Endpoint` 抽象（含 `has_lan_path`/选路/拨号判据迁移） | 单测：LAN/Routed/BLE 三态；私有段 Routed 不再被判成 LAN |
| 7-d | `BleDiscovery` 产出 `PeerCandidate`（发现 ≠ 建连） | 单测：候选不建连接 |
| 7-e | BLE 通道完成双向 Hello 验签 + 一条单聊消息 | **真机**：macOS ↔ macOS，然后 macOS ↔ Windows，最后 Android 接入 |
