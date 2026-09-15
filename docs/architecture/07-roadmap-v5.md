# 07 · V5 路线图（分阶段实施）

- Status: Normative（实施总纲）
- 上游：本目录 `README.md` 与 `01`–`06`
- 目标版本：**V5.0**（「LAN + BLE 无感融合 + 四平台 + BitChat 透明中继」）

---

## 0. 纪律（每阶段都必须满足）

1. **一个阶段 = 一个可独立验证、可回退的最小单元**；不达标不进下一步。
2. 每阶段结束：`cargo test --lib --features bluetooth` + `npm test` + `npm run build` 全绿。
3. 每阶段必须有**新增自动化验收**（纯函数单测或仿真场景），并绑定护栏（`06` §3）。
4. **LAN 红线**：C1–C6（`README.md` §6）。任何阶段若威胁 LAN，必须先影子双跑。
5. 每阶段发一个可安装包跑真机矩阵（`06` §4）。
6. **版本规则（本迭代豁免）**：V5 是一次**全新迭代 / 大改造**——本次**不要求**逐提交带 `Version-Bump:`，
   `npm run version:check` 在本次迭代内**不作为门禁**（预期报红）；版本号在本次收口时**一次性定为 5.0.0**。
   本迭代完成后，后续小修小补**恢复** `../VERSIONING.md` 的逐提交规则。
   （现行 `VERSIONING.md`：MAJOR 只留给向后不兼容，线索词 / churn 不再影响定档。）

### 版本节奏（建议）

| 里程碑 | 内容 | 版本 |
|---|---|---|
| M1 | Phase 0–1（BLE 内部止血 + 度量） | 4.x patch/minor |
| M2 | Phase 2（端口 + 仿真底座） | 4.x minor |
| M3 | Phase 3a/3b/3c（引擎单串行 + DB effect） | **5.0.0-alpha** |
| M4 | Phase 4–5（仿真 + 源路由/邻居并集） | 5.0.0-beta |
| M5 | Phase 6–8（BitChat + iOS + 功率） | **5.0.0** |

---

## Phase 0 — 减负 + 度量（不改行为）

**目标**：降低认知负担，先让「稳不稳」可量化。

**交付**
- 删除死代码：`transport::Transport` trait 与 `TransportManager::route`（`transport/mod.rs` 三处 `#[allow(dead_code)]`）、
  `RelayManager` 发送侧（`state.rs:728`，审计称死代码）；
- 补齐缺失的**纯策略骨架**：`BleConnectionScheduler` / `ScanDutyPolicy` / `PowerProfile`（先只描述现状，不接线）；
- **SLO 埋点**：在现有 `BleDiag`/`ble_scan` 上扩展，输出 `06` §1 的指标快照。

**验收**：行为零变化（现有测试全绿 + 新增纯函数单测）；诊断面板能读出 SLO 指标。
**回退**：单提交 revert。**风险**：极低。**主要文件**：`transport/mod.rs`、`network/ble.rs`、`state.rs`。

---

## Phase 1 — BLE 内部止血（纯 BLE，不碰 LAN）

**目标**：把最影响「不稳定」的四件事做对，且**完全在 BLE 内部**。

**交付**
1. `BleConnectionScheduler` 接线：候选队列 + 全局上限（建议 6）+ 0.5s 节流 + 超时 + 打分；
2. 坏设备 blocklist（借鉴 `docs/device_manager.md` 的**设计**，**注意 main 无实现**）：15s 无 Hello / 60s 静默 / N 次错误断开 ⇒ 冷却；
3. 自愈：扫描 watchdog（30s/120s）与广播失败 backoff（`05` §8）；
4. **出站优先级写队列 + 字节上限**（`05` §6）：把「帧内不可抢占」修掉；
5. 修既有缺口：**TTL clamp**（`transport.rs:4098`）、转发候选过滤为「有链路的 peer」、`start()` adapter 失败时仍尝试外设。

**验收**：真机 Test A–F（`docs/notes/ble-audit-2026-09-13.md` §7）；新增纯函数单测。
**回退**：每项一个特性开关（`INV-NET-43`）。**风险**：中。**LAN**：不涉及。

---

## Phase 2 — 链路端口 + 仿真底座

**目标**：平台与协议分家，并让「多节点测试」成为可能。

**交付**
1. 定义 `LinkId`/`LinkEvent`/`LinkCommand`（`03` §1）；
2. 在既有 `FrameSink`/`FrameSource` 上包装，**不重写**分片逻辑；
3. 三端外设适配器收口为 `BlePeripheral` port；`LanLink` 包一层（**语义冻结**）；
4. `SimulatedLink` + 手动时钟（`06` §2）跑通「两个节点握手 + 一条消息」；
5. 平台 `#[cfg]` 从业务逻辑收敛到 `link/mod.rs` 工厂。

**验收**：三平台 `cargo check --features bluetooth` 0 warning；真机 Test A–F 通过；
新增结构护栏「链路层不 import protocol/db/UI」（INV-NET-02/20）。
**回退**：旧路径保留，双路径可切。**风险**：中高。

---

## Phase 3a — mesh 状态合并（行为零变化）

**目标**：把 6 个散 Mutex 收成一个，写死锁序，**不改语义**。

**交付**
- `mesh_router`/`gossip`/`peer_manager`/`peers`/`links`/`relay_policy` → 一个 `Mutex<MeshState>`（或 `tokio::sync::Mutex`）；
- 文档化锁序：一个函数内只拿一次 `MeshState`；
- 保留 `db`/`downloads_dir`/`nickname` 等非 mesh 字段不动。

**验收**：LAN 全量回归（`npm test` + `cargo test` + 真机 LAN 黄金回归）全绿。
**回退**：单提交 revert。**风险**：中。**LAN**：高敏感，必须影子双跑。

---

## Phase 3b — MeshEngine actor + DB effect

**目标**：单串行状态域；DB 离引擎。

**交付**
- `Mutex<MeshState>` → 单任务 mailbox（`MeshEvent` 进、`MeshEffect` 出）；
- 105 处 `state.db.lock()` 改为 `Effect::PersistMessage/WriteAck/...` + persistence worker（`spawn_blocking`）；
- UI 改读 lock-backed 快照（`PeerView`）；
- 队列契约 grep 护栏：引擎外部不得直接锁引擎状态（INV-NET-01）。

**验收**：影子双跑 diff = 0 后切读写；LAN 黄金回归；新增 ≥3 个仿真场景。
**回退**：影子模式可随时关回旧路径。**风险**：**高**。**LAN**：这是最大回归面，强制影子。

---

## Phase 3c — 删除过渡桥

**交付**：删除 `route_order` 的端点对齐 + 合成候选（`transport.rs:125-162`），只留唯一模型（`02`）。
**验收**：全绿 + 仿真场景通过。**风险**：低。

---

## Phase 4 — 确定性仿真场景（≥8）

**交付**：`06` §2 的 8 个场景全部落地；把现有「只能真机」的回归逐步迁进仿真。
**验收**：仿真单测 < 数秒；收敛失败即 fail。**风险**：中。

---

## Phase 5 — 源路由 + 邻居并集（多跳可靠）

**交付**
- ANNOUNCE 携带直接邻居（跨 transport **并集**，cap 10）；
- 拓扑：双向确认边、60s 过期、≤4 跳路径、v2 能力 gate；
- 定向帧源路由 + 失败回退洪泛（60s 失败缓存）；
- 跨跳补发 + store-and-forward；fanout 候选改「有链路的 peer」。

**验收**：仿真三角/链式拓扑 + 真机三设备；群消息借非成员中继可达。
**风险**：中。**LAN**：邻居表含 LAN，需确保 LAN 单跳仍走直连（`selection` 不变）。

---

## Phase 6 — BitChat 双栈透明中继

**交付**：外设双 UUID、central 双匹配、广告方案（extended/second set/scan response）、
`OpaqueExternal` 生产端；**默认关 + 设置开关**。
**验收**：真 BitChat 设备能发现我们、我们能收其包并转发到 LAN/其它 BLE；关开关零影响。
**风险**：中高（射频 + 合规）。

---

## Phase 7 — iOS 平台适配

**交付**：btleplug apple spike（或自研 `CBCentralManager`）、Tauri iOS 工程、
外设复用 macOS 代码、权限/后台模式、打包脚本。
**验收**：iOS↔Android、iOS↔macOS 双向；后台切回自动恢复。**风险**：高。

---

## Phase 8 — 功率与生命周期策略

**交付**：`PowerProfile`（`05` §5）替换/叠加固定节奏；前后台 pending connect（iOS）。
**验收**：真机功耗对比 + 后台恢复延迟；仿真覆盖策略分支。**风险**：低中。

---

## 9. 上层交互的允许改动（小改，非重构）

交互骨架已定，允许的**最小**改动（仅为流畅度）：
- 对端状态细分：「已发现（未建联）」vs「已连接」而不是笼统「已发送」；
- 路径展示：显示「直连 / 中继」与路径类型，不显示技术细节；
- 诊断页：把 `06` §1 的 SLO 指标可视化。

**不允许**：改动消息/好友/群/文件的语义与持久化（那是 C1/C2）。

---

## 10. 完成定义（Definition of Done, V5.0）

1. 四平台均能 LAN + BLE，且 BLE-only 设备经网关可被 LAN 用户正常聊天（无感）。
2. `06` §1 的 SLO 达标。
3. ≥8 个确定性仿真场景 + 真机矩阵全过。
4. LAN 黄金回归零差异；关蓝牙与今天一致（C4）。
5. BitChat 透明中继可选开启，默认零影响。
6. 删除两套模型中的一套；引擎为单串行；DB 离引擎。
