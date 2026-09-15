# 05 · 蓝牙链路策略（纯函数 + 参数）

- Status: Normative
- 上游：`01`（Link-confined）、`03`（端口）
- 参考：bitchat iOS `BLEScanDutyPolicy`/`BLEConnectionScheduler`/`BLERedundantLinkPolicy`/`BLEMaintenancePolicy`/`BLEAnnounceThrottle`；Android `BluetoothGattClientManager`/`BluetoothGattServerManager`/`PowerManager`

> 全部策略都是**无 I/O 的纯函数/小 struct**，可单测、可在仿真里驱动。这是「可回归」的前提。

---

## 1. 连接预算与调度（`BleConnectionScheduler`）

现状：`network/ble.rs:514` 每候选直接 `tokio::spawn`，无全局上限（`dial_permits` 只用于 TCP `transport.rs:2167`）。
目标：候选队列 + 全局预算 + 评分 + 超时 + 动态 RSSI 阈值。

**发现判定**（顺序命中；照抄 bitchat）：
1. 不可连接 ⇒ ignore；2. RSSI ≤ 动态阈值 ⇒ enqueue；3. 连接数 ≥ max ⇒ enqueue；
4. 距上次全局拨号 < rate limit ⇒ enqueue + 延迟重试；5. 已连接/连接中 ⇒ ignore；
6. 距上次尝试 < 2s ⇒ ignore；7. 距上次超时 < 15s ⇒ ignore；8. 距上次断开 < 3s ⇒ ignore；
9. 物理 disconnected ⇒ connectNow，否则 cancelStale。

**候选打分**（越大越先拨）：
`score = (connectable?1000:0) + (rssi+100)*2 - secondsSinceDiscovered*10 - min(20, 1<<min(4,failures)) - (recentTimeout<60s ? 10 : 0)`

| 参数 | 值 | 说明 |
|---|---|---|
| `maxCentralLinks` | 6 | 全局 central 上限 |
| `connectRateLimitInterval` | 0.5s | 全局拨号节流 |
| `connectionCandidatesMax` | 100 | 候选队列上限 |
| `dynamicRSSIThresholdDefault` | -90 | 常规阈值 |
| `rssiConnectedThreshold` | -85 | 满载时收紧 |
| `rssiIsolatedBase`/`rssiIsolatedRelaxed` | -95 / -100 | 0 连接时放宽（隔离 >30s 再放宽） |
| `weakLinkCooldown`/`weakLinkRSSICutoff` | 30s / -90 | 弱链路冷却 |
| `timeoutDiscoveryIgnore`/`disconnectDiscoveryIgnore` | 15s / 3s | 忽略窗 |

---

## 2. 扫描占空比（`ScanDutyPolicy`）

现状：已有前后台分级（active 5s / idle 30s、窗口 2s；`ble.rs:73-83`、`:544`）。
目标：在其上叠加**自适应**：
- 连接 ≤ 2 或最近有流量 ⇒ 连续扫；
- 否则 duty on/off（bitchat 默认 5s/10s，密集档按连接数调整）；
- 叠加 `PowerProfile`（§5）的电量/充电/后台/是否有直连档位。

---

## 3. 冗余链路治理（`RedundantLinkPolicy`）

现状：靠 `should_dial_ble` 镜像护栏 + `ble_no_dial` 地址表；地址轮换/状态恢复后可能重复链路或永久单侧不可拨。
目标：同角色重复链路**择新保留**（bitchat 现场实测 2–3x airtime）；保留者必须可写；
若「最新连接」尚未完成服务发现 ⇒ **整体延后**再合并（避免 retire↔reconnect 抖动）。

---

## 4. 维护节奏与 announce 节流（`MaintenancePolicy` / `AnnounceThrottle`）

- 每个维护周期决定：是否 announce / 确保广播 / 清理 / flush 定向 spool；
- announce 有**普通间隔**与**强制间隔**；身份轮换（panic/重置）时 **reset**，否则新身份短期不可见；
- 邻居列表变化触发定向 announce（见 `04` §3）。

---

## 5. 功率档（`PowerProfile`）

输入：`batteryLevel / isCharging / isBackground / hasDirectPeers`。
输出：扫描窗、announce 间隔、连接上限、RSSI 阈值、是否占空比。

| 场景 | 扫描窗（示例） | announce |
|---|---|---|
| 前台 BALANCED | on 8s / off 2s | 30s |
| 前台 POWER_SAVER | on 2s / off 28s | 60s |
| 后台 + 有直连 | on 1s / off 29s | 60s |
| 后台无直连 | on 1s / off 59s | 120–300s |
| 充电 PERFORMANCE | 连续 | 30s |

---

## 6. 出站优先级与背压（帧内让路）

现状：`ble_writer_loop`（`ble.rs:1227-1362`）取一条 `Message` 后整帧 `send_frame`；priority 只在**帧间**生效 ⇒
一个 4KiB 文件块（MTU23 下 399 片 ≈ 5.5s）会把聊天帧堵在后面。

目标（照抄 bitchat `BLEOutboundWriteBuffer` 思路）：
- 优先级层级 `high < fragment < fileTransfer < low`；
- 每链路一个**按优先级排序的待写队列**，**每一片之间**取最高优先级（不是打断中断）；
- 字节上限（建议 1MB），超限从队尾裁剪，并**回报新元素是否被裁掉**；
- 断链 ⇒ 丢弃该链路字节，避免轮换地址累积；
- 通知队列按条数上限（建议 128），断链按 target 清理。

---

## 7. 分片重组（`FragmentAssembly`）

现状：`BleReassembler` 只有条数上限（`MAX_INFLIGHT_MESSAGES=8`）+ 30s TTL。
目标：
- 在途装配上限（建议 128），超限淘汰**最老**；
- 每类型字节上限（file/noise 用大上限，其余用小上限），超限整组丢弃；
- 只有**新 index** 刷新 stall 时钟（重复片不能抑制补片）；
- 停滞的广播装配 ⇒ 定向 `REQUEST_SYNC`（有界频率）。

---

## 8. 自愈（`SelfHeal`）

照抄 Android（**已实现**，是可照抄的现成经验）：
- 扫描 watchdog 30s：该扫没扫 ⇒ 重启；
- 120s 无任何结果 ⇒ 强制重启（清卡住的 flag）；
- 重试 backoff：base 3s × retryCount，封顶 30s；扫描 rate limit 5s；
- 广播 `onStartFailure` 分类：`ALREADY_STARTED/DATA_TOO_LARGE/FEATURE_UNSUPPORTED` 不重试；其余 backoff 重启。

---

## 9. 不变量

```text
INV-NET-40  所有链路策略是纯函数（无 I/O、可单测）。
INV-NET-41  BLE 拨号必须纳入全局并发上限。
INV-NET-42  坏片/坏帧只丢该帧，绝不断链（除真写失败）。
INV-NET-43  自适应策略必须有「关闭开关」，可退回固定参数。
```
