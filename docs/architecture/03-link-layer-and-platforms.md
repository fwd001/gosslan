# 03 · 链路层端口与平台适配

- Status: Normative
- 上游：`01-layers-and-concurrency.md`
- 决策 ADR：`../adr/0024-cross-platform-link-layer-and-ios.md`

---

## 0. 目的

定义**引擎与射频之间唯一的消息面**（LinkEvent/LinkCommand），并把四平台差异全部收进适配器。
现状问题：平台 `#[cfg]` 散落在 `network/ble.rs`（10+ 处），链路策略与协议逻辑同处一文件。

---

## 1. 端口定义（Rust 形状）

```rust
pub struct LinkId(u64);            // 不透明；不是 peer、不是 MAC
pub enum LinkKind { Lan, BleCentral, BlePeripheral, OpaqueExternal }

pub enum LinkEvent {
    Up       { link: LinkId, kind: LinkKind, endpoint: Endpoint, peer_hint: Option<NodeId> },
    Down     { link: LinkId, reason: String },
    BytesIn  { link: LinkId, bytes: Vec<u8> },
    Writable { link: LinkId },
}

pub enum LinkCommand {
    Send       { link: LinkId, bytes: Vec<u8> },
    Scan       ( ScanPolicy ),
    Advertise  ( AdvertisePolicy ),
    Disconnect { link: LinkId },
}
```

引擎**只**通过这些类型与链路交互；链路层**只**通过这些类型与引擎交互（INV-NET-02）。

---

## 2. 现状与差距（2026-09-14 核对）

| 已有 | 位置 | 说明 |
|---|---|---|
| 帧级端口 `FrameSink`/`FrameSource` | `network/ble.rs:1131-1225` | **可复用**为 LinkCommand::Send / LinkEvent::BytesIn 的基础 |
| 三端同形外设接口 | `bluetooth_peripheral.rs` / `ble_android.rs` / `bluetooth_peripheral_windows.rs` | 已经「接口逐字同形」，只差统一 port 包装 |

| 欠缺 | 影响 |
|---|---|
| 链路生命周期/命令端口 | `scan_loop`/`dial_and_register`/`peripheral_accept_loop` 直接调 `driver::*` |
| 平台门收敛 | `#[cfg]` 散在业务逻辑；Windows 曾被整段排掉 |
| 模拟链路 | 无法确定性测试（Phase 4 的前置） |

---

## 3. 适配器清单

| 适配器 | 平台 | 现状 |
|---|---|---|
| `LanLink` | 全平台 | UDP 发现 + TCP；**语义冻结**（C1–C3） |
| `BleCentral` | mac/win/android/iOS | btleplug（mac 走 corebluetooth） |
| `BlePeripheral` | mac/android/win/**iOS** | CoreBluetooth / Kotlin / WinRT（iOS 与 macOS 共用） |
| `OpaqueExternalLink` | 任意 BLE | BitChat 字节管道（见 `04`） |
| `SimulatedLink` | 测试 | 实现同一 port，注入 Up/BytesIn/丢包/延迟 |

---

## 4. 四平台矩阵

| 角色 | macOS | iOS | Windows | Android |
|---|---|---|---|---|
| BLE central | btleplug(corebluetooth) | btleplug(apple) **待 spike** 或自研 `CBCentralManager` | btleplug(winrt) | btleplug(droidplug) |
| BLE peripheral | objc2 `CBPeripheralManager` | **同 macOS（改 cfg）** | WinRT `GattServiceProvider` | Kotlin `BluetoothGattServer` + JNI |
| 权限/清单 | entitlements `device.bluetooth` + `NSBluetoothAlwaysUsageDescription` | Info.plist + 后台模式 | 应用能力 | `BLUETOOTH_SCAN/CONNECT/ADVERTISE` |
| 后台 | 常规 | `bluetooth-central`/`bluetooth-peripheral` + state restoration | — | 前台服务 |

---

## 5. iOS 方案（新增平台）

1. btleplug 用 `target_vendor = "apple"` 选 `corebluetooth`（已核实 `vendor/btleplug/src/platform.rs`），
   理论可编 iOS ⇒ **先做 1–2 天 spike**（`cargo build --target aarch64-apple-ios --features bluetooth`）。
2. **退路**：自研 `CBCentralManager`（`objc2-core-bluetooth` 同时提供），与 macOS 外设同模块风格。
3. Tauri iOS 工程尚未初始化（`src-tauri/gen/` 只有 `android`）⇒ 需要 `tauri ios init`。
4. 后台：`bluetooth-central`/`bluetooth-peripheral` + CoreBluetooth state restoration +
   前台/后台扫描节奏（见 `05`）。
5. 验收：iOS↔Android、iOS↔macOS 双向发现/连接/消息/文件；切后台再回前台自动恢复。

---

## 6. 测试钩子（为 `06` 准备）

- `SimulatedLink`：无射频、虚拟时钟，可注入 `LinkUp`/`BytesIn`/丢包/延迟/断链；
- 引擎提供测试用 ingest 入口（走**生产**归属路径，不改线上逻辑）；
- 定时器封装成可注入调度器，支持 `tokio::time::pause()`。

---

## 7. 不变量

```text
INV-NET-20  链路层不 import protocol / db / UI 类型。
INV-NET-21  引擎只经 LinkCommand 驱动链路，不直接调 driver。
INV-NET-22  三端（mac/iOS/Android/Windows）外设接口保持同形。
INV-NET-23  LinkId 不参与身份判定（身份只由 Hello 验签建立）。
```
