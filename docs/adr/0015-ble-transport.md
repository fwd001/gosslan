# ADR-0015: BLE Transport（Phase 7 / P3）

- Status: Proposed（**待用户审核**；实现按 feature 门推进，默认关闭）
- 进度：7-a/7-b/7-c 完成；7-e 完成"驱动 + 编译/单测验证"，接线与真机待做；7-d 待设计
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

### 3.1 ⚠️ 更正：btleplug **只做 central（host）角色**

2026-09-12 深夜在本机拉下 btleplug 0.13 源码后核实（`README.md` 第 19 行原文）：
> btleplug is meant to be *host/central mode only*. If you are interested in peripheral BTLE
> (i.e. advertising), use `bluster` or `ble-peripheral-rust`.

也就是说：**两个 Gosslan 节点不能只靠 btleplug 互相发现** —— GATT 连接必须有一方
**广播（advertise）并提供服务端**，而 btleplug 不提供这个角色。这推翻了我在本 ADR §3
里"一套 API 覆盖三平台"的表述（那句话在"central 角色"这个维度上是对的，但**角色不齐**）。

可选的补法（已核实的现实）：

| 方案 | 覆盖 | 代价 |
|---|---|---|
| `bluster` 0.2 / `ble-peripheral-rust` 0.2（peripheral 角色） | macOS/iOS（CoreBluetooth）、Linux（BlueZ）；关键字里**没有 Windows** | 新增第二个蓝牙依赖；Windows 仍缺 |
| 平台原生实现的 peripheral 角色 | macOS/iOS `CBPeripheralManager`（项目已有 `objc2`）；Android `BluetoothLeAdvertiser` + `BluetoothGattServer`（需 JNI）；Windows `GattServiceProvider`（WinRT，需 `windows` crate） | 三套平台代码，且只能真机验证 |

**建议（待用户裁决）**：**角色分工** —— PC（macOS/Windows）做 **central**（btleplug，已实现），
**手机做 peripheral**（Android/iOS 平台 API）。理由：
① 用户的真实场景就是"手机 ↔ 电脑"，手机做 peripheral 刚好覆盖；
② 只需在移动端实现一个角色，桌面端零额外平台代码；
③ Windows 的 peripheral 角色在 Rust 生态里目前没有现成 crate，绕开它是关键收益。
代价：手机与手机之间在 BLE 上要连，得有一方当 central（btleplug 在 Android 上可用），
但那一侧的服务端仍要等移动端 peripheral 实现后才能被连 —— 属后续增强。

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
| 7-a | 本 ADR + `bluetooth` feature 门骨架（`btleplug` 可选依赖） | ✅ `fe0d4a2`，默认构建不含 btleplug（`cargo tree` 已验证） |
| 7-b | 收发适配器（分片/重组/流控/MTU） | ✅ 编解码 `4984156`（9 条单测：往返/乱序/重复/残缺/恶意头/在途上限/TTL）；驱动用法见 7-e |
| 7-c | `Link.path_kind` 显式携带 + `Endpoint` 抽象 | ✅ `353a964` + `8ce2b18`：单测覆盖 LAN/Routed/BLE 三态 + "私有段 Routed 不算 LAN" + "BLE 不优先于 TCP" |
| 7-d | `BleDiscovery` 产出 `PeerCandidate`（发现 ≠ 建连） | ⬜ 待做。**设计要点**：BLE 地址不是身份（身份只能由双向 Hello 验签建立），
所以候选要么携带占位身份、要么扩展 `PeerCandidate` 允许"身份未知" —— 需要与 P-A01/P-A04 一起定，不能顺手塞。 |
| 7-e | 驱动 + 接线 + 双向 Hello 验签 + 一条单聊消息 | ⚠️ **central 侧完成**（`cc273b5` + `c2124ef`；见 §3.1 的更正：peripheral 角色仍需移动端平台实现）：`transport/bluetooth.rs::driver` 按 btleplug 0.13 真实源码实现
（adapter / scan_peers / connect+特征校验 / send_frame 分片写 / next_frame 通知重组 / payload_mtu），
**`cargo build --features bluetooth` 与 `cargo test --features bluetooth`（367 passed / 0 warning）
已在本机 macOS 通过**（把 `CARGO_HOME` 指到仓库内 `src-tauri/target/cargo-home` 绕开"不能写 ~/.cargo"）。
剩：接到 `BluetoothTransport`/`state.links`（`start` 探测 → 扫描任务 → 连接 → 登记
`Endpoint::Ble` + `PathKind::Bluetooth` → 收发喂 `handle_message`）+ **三平台真机**。 |
