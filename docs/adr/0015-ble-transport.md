# ADR-0015: BLE Transport（Phase 7 / P3）

- Status: Proposed（**待用户审核**；实现按 feature 门推进，默认关闭）
- 进度：7-a/7-b/7-c 完成；7-e **central + macOS peripheral 两侧接线均已完成**（编译/单测验证），真机待做；7-d 待设计；7-f（移动端做 peripheral）待做
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
| 7-d | `BleDiscovery` 产出 `PeerCandidate`（发现 ≠ 建连） | ✅ **已关闭（2026-09-12 用户裁定）**：BLE 语义就是"发现即连接"，身份只由双向 Hello 验签确定，当前实现不需要候选抽象。原设计要点（仅存档）：**设计要点**：BLE 地址不是身份（身份只能由双向 Hello 验签建立），
所以候选要么携带占位身份、要么扩展 `PeerCandidate` 允许"身份未知" —— 需要与 P-A01/P-A04 一起定，不能顺手塞。 |
| 7-e | 驱动 + 接线 + 双向 Hello 验签 + 一条单聊消息 | ⚠️ **central + macOS peripheral 两侧完成，真机待验**（central：`cc273b5` + `c2124ef`；peripheral：`b317c27`，沙盒权限 `42c1108`）：`transport/bluetooth.rs::driver` 按 btleplug 0.13 真实源码实现
（adapter / scan_peers / connect+特征校验 / send_frame 分片写 / next_frame 通知重组 / payload_mtu），
**`cargo build --features bluetooth` 与 `cargo test --features bluetooth`（367 passed / 0 warning）
已在本机 macOS 通过**（把 `CARGO_HOME` 指到仓库内 `src-tauri/target/cargo-home` 绕开"不能写 ~/.cargo"）。
接线已完成（`start` 探测 → 扫描任务 → 连接 → 登记 `Endpoint::Ble` + `PathKind::Bluetooth`
→ 收发喂 `handle_message`），macOS 侧另补了 **peripheral（GATT server）角色**（见 §7）。
`cargo test --lib --features bluetooth` = **371 passed / 0 warning**，Android target 同样 0 warning。
剩：**三平台真机**。 |
| 7-f | 移动端/Windows 的 peripheral 角色 | 🚧 **Android 整体完成**（Kotlin `e66fd8b` + Rust JNI 桥；真实 APK 构建验证；仅剩真机）。手机做 peripheral（Android `BluetoothLeAdvertiser` + `BluetoothGattServer` 经 JNI；iOS 与 macOS **同款 `CBPeripheralManager` 代码**）后，Windows/手机之间才能不经 Mac 直连；Windows 仍需 `GattServiceProvider`（WinRT）。 |

---

## 7. 实施清单：peripheral 角色（macOS 先做，供 7-e 收尾）

> 本节是**照着实测过的 API 写的**（2026-09-12 把 `objc2-core-bluetooth` 0.3.2 的 `.crate`
> 解包后逐个核对方法签名）。**已于 `b317c27` 落地，实施结果与偏差见 §7.5。**

### 7.1 依赖（macOS 专属 + 纳入 `bluetooth` feature）
```toml
[target.'cfg(target_os = "macos")'.dependencies]
# peripheral 角色：btleplug 只做 central（见 §3.1），macOS 用 CoreBluetooth 的
# CBPeripheralManager（objc2 家族，与现有 objc2 0.6 / objc2-app-kit 0.3 同版本线）
objc2-core-bluetooth = { version = "0.3", optional = true, default-features = false, features = [
  "std", "CBPeripheralManager", "CBPeripheralManagerConstants", "CBATTRequest",
  "CBAdvertisementData", "CBService", "CBCharacteristic", "CBUUID", "CBManager", "CBCentral",
] }
objc2-foundation = { version = "0.3", optional = true, features = ["NSData","NSString","NSError","NSUUID","NSArray","NSDictionary","NSObject","NSValue"] }
```
- ⚠️ 实测：`objc2-core-bluetooth` 的 feature 名只有类名（CBService / CBCharacteristic …），
  **没有** `CBMutableService`/`CBMutableCharacteristic` 这两个 feature —— 它们是
  `CBService`/`CBCharacteristic` 模块里的类型（`CBService::CBMutableService`）。
- `objc2-foundation` 目前只是 dev-dependency，要实现 peripheral 必须提升为 macOS 可选依赖。
- 两者都纳入 `bluetooth` feature；**默认构建与其它平台完全不受影响**。

### 7.2 文件与角色选择
- 新文件 `src-tauri/src/transport/bluetooth_peripheral.rs`（macOS 实现）；
- `network/ble.rs` 增加"本机是否做 peripheral"的分支。建议：**桌面做 central、手机做 peripheral**
  （见 §3.1），用一个设置项表达（默认值按平台给，用户可改）。

### 7.3 关键调用顺序（实测签名）
1. `CBPeripheralManager::initWithDelegate_queue(delegate, queue)` —— 传**私有串行队列**
   （不能用主队列：delegate 回调里要做分片重组与 `handle_message`）。
2. delegate `peripheralManagerDidUpdateState:` → `state == PoweredOn` 才继续；
   否则把通道标为不可用（未授权、无适配器都在这里）。
3. `CBMutableCharacteristic::initWithType_properties_value_permissions(RX, Write|WriteWithoutResponse, nil, 0)`
   与 `(TX, Notify, nil, 0)`；`CBMutableService::initWithType_primary(SERVICE_UUID, true)`，
   用 `setCharacteristics:` 挂上两个特征 → `addService:`。
4. `startAdvertising:` 传 `{ CBAdvertisementDataServiceUUIDsKey: [SERVICE_UUID] }`。
5. 收数据：delegate `peripheralManager:didReceiveWriteRequests:` → 逐个 `CBATTRequest`
   取 `value` 喂 `BleReassembler`；**必须** `respondToRequest_withResult(req, CBATTErrorSuccess)`，
   否则中心端每次写都会等到超时（GATT 语义）。
6. 发数据：`updateValue_forCharacteristic_onSubscribedCentrals(TX, data, centrals)`；
   它在发送队列满时返回 `false` → 必须等 `peripheralManagerIsReadyToUpdateSubscribers:`
   再重试，**否则静默丢帧**（这是 peripheral 侧最容易漏的一处）。
7. 订阅跟踪：`peripheralManager:central:didSubscribeToCharacteristic:` 记下 central 列表。

### 7.4 验证（必须真机，逐条；状态 ⬜ = 待用户实测）
1. ⬜ 另一台设备（Android 手机的 central，已实现）能**扫到** Mac 并连上；
2. ⬜ 双向 Hello **验签通过**（日志里 `+ble-link(外设) peer=…`）；
3. ⬜ 单聊一条消息**双向**送达（含图片/文件分片）；
4. ⬜ 关掉「蓝牙」开关后：BLE 链路全拆、**局域网聊天不受影响**；
5. ⬜ 手机做 peripheral 时同样跑 1–4（Android `BluetoothLeAdvertiser` + `BluetoothGattServer`
   经 JNI；iOS `CBPeripheralManager` 经 objc2，与 macOS 同款代码）。

### 7.5 实施结果与偏差（`b317c27` + `42c1108`）

实际写下来，与 §7.1–7.3 清单有六处**有意**的偏差，都记在这里以免下次重蹈：

1. **用主队列，不是私有串行队列**（清单第 1 条）。回调里只做「拷字节 + 查表 + 发通道」，
   重活（验签、`handle_message`、加解密）全在 tokio 侧，因此不存在"卡住回调"的风险；
   而私有队列要求我们自己驱动它，反而多一层复杂度。**这条是"不阻断渲染"红线的一部分：
   回调在主队列上必须是微秒级的。**
2. **`default-features = false` 不需要**：直接用默认 feature 即可 ——
   `objc2-core-bluetooth` 自己会打开 `objc2-foundation` 的 NSData/NSArray/NSDictionary/NSString，
   feature 统一后**没有**修改 `objc2-foundation` 的依赖声明（清单第 139 行的假设不成立）。
3. **`start()` 必须是同步函数**：`Retained<CBUUID>`/`Retained<NSData>` **不是 `Send`**，
   只要它们跨过任何 `await`，整个 future 就非 `Send`，会一路传染到 `#[tauri::command]`
   的返回值（`set_channel_enabled` 编译失败）。于是 CoreBluetooth 对象的构造全程无 await，
   "等状态回调"这一步交给调用方（它手里只有 `PeripheralServer` 和 oneshot，都是 `Send`）。
   同理，`NSData` 的构造/析构被限在一个块里，**在 await 之前**完成。
4. **`SendObj<T>` 是必要的显式断言**：objc2 保守地不为框架类实现 `Send`/`Sync`。
   断言依据是 Apple 文档（manager 方法可从任意线程调用、回调串行派发到构造时给的队列），
   且本模块从不在多个任务里并发调用同一对象的同一方法。
5. **外设角色没有"central 断开"回调**（只有 `didUnsubscribeFromCharacteristic:`）：
   所以「同一 BLE 端点的旧链路让位」是**必须**的 —— 否则对端重连时会永远撞在
   `should_accept_inbound_public` 上，形成黑洞。写入失败/通道关闭也会促成收尾。
6. **发送队列满 = 等待而不是报错**：`updateValue` 返回 `false` 时等
   `peripheralManagerIsReadyToUpdateSubscribers:`（8s 上限）再重试；未订阅时等订阅信号
   （Hello 回程常常赶在订阅完成之前）。等待用 `timeout + 50ms 兜底`，
   **绝不能忙等**（watch 通道关闭时 `changed()` 会立刻返回 `Err`，不睡就是死循环）。

另外两条**打包层**的坑（`42c1108`），不解决的话射频代码再对也没用：
- macOS 沙盒缺 `com.apple.security.device.bluetooth` ⇒ CoreBluetooth 的 state 恒为
  Unauthorized，**central 也一起失效**（现象是"开着蓝牙却一个设备都扫不到"）；
- macOS 11+ 同样要求 Info.plist 里有 `NSBluetoothAlwaysUsageDescription` ⇒
  已显式声明 `bundle.macOS.infoPlist`，不再依赖"自动探测同名文件"。

### 7.6 上线前自查补掉的两处**静默**故障（`81606cd`）

外设角色落地后回头自查，抓到两处"读代码看不出来、真机上只觉得'不好用'"的问题，
都属于本项目最该由机器/日志盯住的那类（**写对了但不起作用，且不留痕迹**）：

1. **蓝牙被关掉 / 权限被撤 ⇒ 订阅状态陈旧**。CoreBluetooth 此时会清空本地 GATT 数据库、
   断开所有 central，但**不会**回调 `didUnsubscribeFromCharacteristic:`。我们那份"谁订阅了我"
   因此一直是旧数据：`is_subscribed` 仍为 true，写任务要等 `updateValue` 失败
   （**最长 8s**）才收尾，日志里也没有任何记录。现在离开 `PoweredOn` 就：
   作废全部订阅与半截消息 → 唤醒等待中的写任务 → 为每个 central 各发一条 `Unlinked`
   （网络层**立刻**拆链路）→ 记一条 warn。纯函数 `detach_targets` 守住"离开 PoweredOn
   必须摘掉全部订阅"这条判据（+1 单测，已做非空转验证）。
2. **广播失败被静默丢弃**。`peripheralManagerDidStartAdvertising:error:` 的结果原先只在
   "首次状态回调的 oneshot 还开着"时才上报，而那个 oneshot 在第一次
   `peripheralManagerDidUpdateState:` 里就被 `take()` 了 ⇒ 之后任何广播失败（例如用户
   关掉蓝牙再打开、我们重新广播时失败）都不会留下任何日志。现象是"蓝牙开着却没人能发现我们"，
   而手册排障表恰好写着"看日志"。现在改走常驻的 `events` 通道（新增
   `PeripheralEvent::{Notice, Warning}` → `logger.info` / `logger.warn`），**每次**都报，
   并带上可操作建议。

顺带修正的语义：**回到 `PoweredOn` 必须重新 `addService` + `startAdvertising`**
（CoreBluetooth 在离开 `PoweredOn` 时清空过本地数据库，不重新发布就永远不会再有人连得上我们）；
以及 `did_unsubscribe` 里把两层锁拆开，不再在持有 `state` 时去拿 `reassemblers`。

⚠️ 仍未验证：真机上"关蓝牙 → 链路立刻消失且日志给出原因"要用户实测（本机无法触发状态切换）；
"对端走远/掉电"依然**没有**任何回调可用，只能靠写失败或心跳超时收尾 —— 这是外设角色的平台限制。

### 7.7 Android 外设角色的实现要点（7-f 第一步，`e66fd8b`）

平台 API 与 macOS 完全不同，但**行为契约必须一致**（同一套 UUID、同样的广播内容、
同样的写/通知语义、同样的四类 native 回调）。要点与坑：

1. **广播里只放服务 UUID**：理由与 macOS 侧逐字相同（legacy 广播 31 字节，128 位 UUID 占 18），
   `AdvertiseData.Builder().setIncludeDeviceName(false)`；`ADVERTISE_FAILED_DATA_TOO_LARGE`
   要给出可读提示。
2. 🔴 **`BLUETOOTH_ADVERTISE` 是独立运行时权限**（Android 12+）。缺它时 `startAdvertising`
   直接抛 `SecurityException`，现象是"手机能扫别人、别人永远发现不了手机"。
   三个权限（SCAN / CONNECT / ADVERTISE）缺一不可。
3. **CCCD 必须显式挂**：客户端要开通知就要写 `00002902-…` 描述符，Android 不会替我们加
   （CoreBluetooth 会隐式处理）—— 这是两侧结构上唯一的差异。`onDescriptorWriteRequest`
   里也必须 `sendResponse`。
4. **`onCharacteristicWriteRequest` 必须先 `sendResponse` 再处理数据**，否则对端每次写都等到超时；
   写请求的 response value 会被忽略，传 `null` 即可。
5. **API 33 的 `notifyCharacteristicChanged(device, char, false, value)` 返回状态码 `Int`**，
   而旧重载返回 `Boolean` —— 两个分支写在一起会**编译不过**（本项目已真实踩到）。
   ≤32 仍需"先 `setValue` 再 notify"（`setValue` 在 33+ 已废弃）。
6. **回调线程**：系统在主线程投递回调，因此回调里只做"拷字节 + 查表 + 调 native"，
   重活（分片重组、验签、落库）全在 Rust/tokio —— 与 macOS 侧同一条"不阻断 UI"的红线。
7. **Android 会回调断开**（`onConnectionStateChange`），比 CoreBluetooth 强：
   可以立刻发 `unlinked`，不用等写失败。

**本机构建/验证方法**（沙盒或 CI 里 `~/.android` 不可写时）：
```bash
ANDROID_USER_HOME=$PWD/target/android-home npm run android:build:debug
# 注意：**不要**设 ANDROID_SDK_HOME —— AGP 8 会把它当成 SDK 路径，直接加载不了插件
aapt2 dump permissions app-universal-debug.apk | grep -i bluetooth   # 复核权限真的进了包
```

### 7.8 Android 外设的 Rust↔Kotlin 桥（7-f 第二步）

Kotlin 侧拿不到 Rust 的网络层，Rust 侧也写不出 `BluetoothGattServerCallback`（回调必须是
Java 对象），所以两侧必须通过 JNI 对接。要点：

1. **依赖**：`jni = "0.22"`（Android 目标、optional，随 `bluetooth` feature 打开）。
   btleplug 的 droidplug 后端**本来就依赖同一个 `jni 0.22`**（Android 非 optional），
   所以这里没有引入任何新的第三方 crate。
2. **`bootstrap` 的鸡生蛋问题**：JNI 的 `FindClass` 用的是"调用它的 native 方法的类的类加载器"，
   从普通 tokio 线程里 `FindClass("com/gosslan/app/BlePeripheral")` 会走**系统类加载器**、
   找不到 App 的类。因此 `MainActivity.onCreate` 里调一次 `BlePeripheral.bootstrap(context)`，
   它在 App 代码还在栈上时调 `nativeBootstrap()`；Rust 在那次调用里缓存 `JavaVM` 与
   **类的全局引用**（`env.get_object_class(&this)` → `new_global_ref`），之后任何线程都能用。
3. **符号 vs 注册**：`native_method!` 的 `extern` 会**直接导出 JNI 符号名**
   （`Java_com_gosslan_app_BlePeripheral_nativeBootstrap`…）⇒ `bootstrap` 靠名字就能解析；
   其余四个在 `nativeBootstrap` 里用 `register_native_methods` **再显式注册一次** ——
   签名写错会当场以 `NoSuchMethodError` 报出来，而不是等到真机收发时静默失效。
4. **没开 `bluetooth` feature 时必须安全**：Rust 里没有 `nativeBootstrap` 实现，
   Kotlin 侧用 `try { nativeBootstrap() } catch (e: UnsatisfiedLinkError)` 吞掉 ——
   否则**默认构建会在启动路径上崩溃**。
5. **职责边界**：Kotlin 只做"平台 API + 原样转发分片"，**分片重组仍在 Rust**
   （复用 `ble_framing::BleReassembler`，与 macOS 完全同一份实现），
   于是"帧"这个概念在三端只有一个定义。
6. **接口同形**：`ble_android.rs` 暴露的 `start/stop/PeripheralServer/PeripheralWriter/
   PeripheralEvent` 与 `bluetooth_peripheral.rs`（macOS）**逐一对应**，
   所以 `network/ble.rs` 里的事件循环、握手、路由、读写循环**两个平台共用一份**
   （只有 import 按平台切换，其余零改动）。
7. **方法名映射**：`native_method!` 会把 Rust 的 snake_case 方法名转成 lowerCamelCase
   （`native_on_frame` → `nativeOnFrame`），数组类型写作 `jbyte[]`；
   而 `call_static_method` 的签名要用**原始 JNI 描述符**
   （`jni_sig!("(Ljava/lang/String;[B)Z")` —— 写成 `(java.lang.String) -> boolean` 会被当成字段签名）。
