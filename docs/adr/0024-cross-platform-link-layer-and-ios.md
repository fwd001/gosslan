# ADR-0024: 跨平台链路层端口 + iOS 支持

- Status: Proposed（V5.0 方向）
- Date: 2026-09-14
- Owners: Gosslan
- Related:
  - 规范：`../architecture/03-link-layer-and-platforms.md`
  - 依赖：ADR-0020
  - 既有：ADR-0015（BLE transport）
  - CHANGELOG: `[Unreleased]`

---

## 1. Context

平台 `#[cfg]` 散落在 `network/ble.rs`（10+ 处），Windows 曾被整段排掉（ADR-0015 §7.9）；
用户新增需求：**iOS 也要支持**。当前仓库**没有 iOS 工程**（`src-tauri/gen/` 只有 `android`）。

已有可复用资产：
- 帧级端口 `FrameSink`/`FrameSource`（`network/ble.rs:1131-1225`）；
- 三端同形外设接口（CoreBluetooth / Kotlin JNI / WinRT）；
- btleplug 用 `target_vendor="apple"` 选 corebluetooth（`vendor/btleplug/src/platform.rs`）⇒ **理论可编 iOS**。

---

## 2. Decision

1. 定义 `LinkId`/`LinkEvent`/`LinkCommand` 作为引擎↔链路唯一消息面（`03` §1），
   在既有 `FrameSink`/`FrameSource` 之上包装，**不重写**分片。
2. 平台差异收敛到 `link/mod.rs` 工厂；链路层不得 import protocol/db/UI（INV-NET-02）。
3. 四平台适配器：`LanLink`、`BleCentral`、`BlePeripheral`、`OpaqueExternalLink`、`SimulatedLink`（测试）。
4. **iOS**：
   - 先做 btleplug apple spike（`cargo build --target aarch64-apple-ios --features bluetooth`）；
   - 若不可行 ⇒ 自研 `CBCentralManager`（`objc2-core-bluetooth`），与外设同模块；
   - peripheral **复用 macOS 的 `bluetooth_peripheral.rs`（改 cfg）**；
   - `tauri ios init` 生成工程；Info.plist 权限 + 后台模式 + state restoration。

---

## 3. Detailed Design

见 `../architecture/03-link-layer-and-platforms.md`（端口、矩阵、iOS 方案、测试钩子）。

---

## 4. Invariants

```text
INV-NET-20  链路层不 import protocol / db / UI 类型。
INV-NET-21  引擎只经 LinkCommand 驱动链路。
INV-NET-22  四平台外设接口保持同形。
INV-NET-23  LinkId 不参与身份判定。
```

---

## 5. Alternatives

**A. 继续用 `#[cfg]` 分支**：无法支持 iOS，且平台差异渗入协议层；拒绝。
**B. 每平台独立完整实现（含协议）**：重复且不一致；拒绝。
**C. iOS 只做 central 或只做 peripheral**：与 Windows 教训相同（不做外设 ⇒ 永不被发现）；拒绝。

---

## 6. Compatibility

- 不改线格式；
- macOS/Windows/Android 行为保持；
- iOS 为新增平台，无历史包袱。

---

## 7. Failure Modes

- btleplug iOS 运行时不工作 ⇒ 走自研 CoreBluetooth 退路（已规划）；
- iOS 后台限制 ⇒ state restoration + 前后台扫描节奏（`05` §5）；
- 权限缺失 ⇒ 明确报错且不影响 LAN。

---

## 8. Testing

- 三/四平台 `cargo check --features bluetooth` 0 warning；
- macOS↔iOS、Android↔iOS 双向真机；
- 结构护栏：链路层 import 白名单。

---

## 9. Consequences

**Positive**：四平台一致；平台代码可替换、可测试；iOS 可复用 macOS 外设。
**Negative**：新增 iOS 工程与审核成本。

## 10. Revisit Conditions

若 btleplug 上游正式支持 iOS，可切换回统一后端，删除自研 central。

## 11. References

- `../architecture/03-link-layer-and-platforms.md`、`docs/adr/0015-ble-transport.md`
