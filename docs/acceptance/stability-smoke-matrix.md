# 稳定版验证覆盖矩阵（AUTOMATED / SIMULATED / MANUAL-HARDWARE）

> 建立于 2026-09-25，产物归属 `docs/stability-roadmap.md` §5/§6（M-5）。
> 存在理由只有一条：**总指令§十要求「绝不能把没有测试伪装成 PASS」**。
> `docs/acceptance/1.0-release.md` 列了 22 条 P0 但**一个状态都没有** —— 那不算清单，算愿望列表。
>
> 三档定义（不许混用）：
> - **AUTOMATED** —— CI 或本地门禁里真的跑，红了会拦。
> - **SIMULATED** —— 只在同一进程内的夹具/假对端里跑过。**它证明机制，不证明进程。**
> - **MANUAL-HARDWARE** —— 只能真机或人眼看。必须写出**可观测判据**（看哪个界面、抓哪个日志前缀、期望什么），
>   否则这一格等于没写。
>
> 证据列写的是**具体名字**（测试名 / 判据脚本 / 用例标签），不是"有测试"。

## P0 地基（1–15）

| # | 验收项 | 等级 | 证据 | 缺口 |
|---|---|---|---|---|
| 1 | 局域网发现（多网卡/虚拟网卡不误判） | SIMULATED | `network/discovery.rs` 15 条用例 | 真实网卡组合无自动化 → Smoke-1 |
| 2 | 好友请求/接受/删除/重加，两端最终一致 | SIMULATED | `friendRequests.test.ts`、`commands/` 相关用例 | **无跨进程一致性证明** → J4 |
| 3 | 在线/离线（断链 ≠ 删节点） | SIMULATED | `mesh/` 79 条用例 | 同上 |
| 4 | 双向文本 + `msg_id` 幂等 | **AUTOMATED** | `e2e-multi-instance.mjs` J1（B 侧恰好一条 + `messages.msg_id UNIQUE`） | Windows 腿未跑 |
| 5 | Outbox → Ack，Ack 只代表已持久化 | **AUTOMATED** | J1「A 侧 outbox 被 Ack 清空」+ `cascade_tests` | 反例（DB 错不得 Ack）只有进程内用例 |
| 6 | 离线持久化与自动重发（离线 ≠ 2 分钟失败） | SIMULATED | `db/offline_queue.rs`、`fail_reason_separates_retryable_from_terminal` | 真离线对端的补发未跨进程 → J1n |
| 7 | 网络恢复后不丢不重 | **部分 AUTOMATED** | J1「两端重启后仍只有一条 / outbox 不复活」+ **故障注入** `--fault=poison-part`（接收侧脏 `.part` 前缀 → 第 1 次 attempt 整体校验失败 → outbox 重试补齐，20 断言 2 连绿） | 只覆盖 LAN 路径 + 优雅重启 + 接收侧脏前缀 + **接收中 SIGKILL（杀进程轮 23 断言 1 连绿、lie 2/23 报红）**+ **对端失联后解冻自愈（冻结轮 22 断言、lie 恰好 1/22 报红）**；**断链 / 重复帧 / 乱序未测**（断链在单机无 root 造不出"只断一条链路"，用中继伪造会被 route 优先级绕过 ⇒ 跑出来是假绿）→ A-2 |
| 8 | 已读回执与送达状态 | SIMULATED + 人工 | `storeContract.test.ts` 已读判据、`applyConversationSnapshot` | 移动端群已读「不见了」= #30，**未定位** → Smoke-4 |
| 9 | 失败可见且可重试（重发必须重新加密） | SIMULATED | `resend_reseals_before_enqueue` 等护栏 + 不变量登记 | 见 `protocol-invariants.md` §6 例外 |
| 10 | E2EE 身份锚定，未验签不建信任 | SIMULATED | `friend_identity_anchor_has_one_binding_rule`、INV-P21 用例 | 真实冒名建链未跨进程测 |
| 11 | SQLite 持久化 + 重启恢复（含在途队列） | **部分 AUTOMATED** | J1 重启断言 + `migration_tests.rs` 19 条 + `fresh_schema_alone_has_exactly_the_migrated_shape` | 迁移**中途失败**不可恢复 → J7 |
| 12 | 图片/文件消息（多选并发不丢件） | **部分 AUTOMATED** | J2 已 **4 连绿**（另：故障注入轮 20 断言 **2 连绿** + `--fault=poison-part-lie` 反向按设计报红）（1 MB：只有 rename 后出现最终名 + 字节数 + sha256 + 发送侧 `sent → done`（必须等对端 `FileCompleteAck`）+ 接收侧 `done` + 无 `<tid>.part` 残留 + `file_outbox` 收尾删除） | 只覆盖单文件 1 MB；尺寸阶梯/并发未做 → A-3。**已做**：接收端写不进去（磁盘轮 22 断言 1 连绿、lie 恰好 1/22 报红 ⇒ 队列 5 次内 GiveUp、台账非 done、盘上零残留）；⚠️ 但**接收方自己完全无感**（offer 期只记日志、不写库不发事件）→ A-10。**已做**：收到一半才收尾失败（改口轮 22 断言、lie 恰好 1/22 报红 ⇒ .part 必须真被续写过、A 侧终态明确、attempts ≤ 5、接收侧台账不假 done）；★ 同一轮实测照出 **A-12** 并已修（修前实测 `A {status:gone, aT:done} / B=failed / .part=整份 / final 不存在`）：判据改成**字节数够 ≠ 收完、也 ≠ 进度**，那份没有成品的 `.part` 不再被报成 `received = size`（一个帧承载两个含义是这一格的病根）⇒ 发送侧只能落到 `failed`，交叉自洽那条已按原口径加回断言，另加一条防它变成永远为真的空转判据。**已做**：入队后源文件被改小（改小轮 22 断言 3 连绿、lie 恰好 1/22 报红 ⇒ offer 按截断后的真 size、落地字节+hash 与新源一致、两侧同 done、盘上只留终名那一份）；⚠️ 同一轮照出**发送侧的 size 没人更正**（实测 A 气泡/台账仍写入队时那份、B 写真实那份）→ A-11。**已做**：连续多文件 + 其中两单同名（多文件轮 22 断言、lie 恰好 1/22 报红 ⇒ 三单各自 done、同名落成两个不同路径、落地内容多重集合 == 源内容多重集合、无 `.part`）；⚠️ 只覆盖"同 peer 串行 flush"这条时序，**两个 offer 都在任一次 rename 之前到达**那个真会撞 final_path 的交错没覆盖（要三个实例）|
| 13 | 通知（尊重开关、失败可观察） | MANUAL-HARDWARE | — | 见 Smoke-3 |
| 14 | Win/mac/Android 三端构建与基本稳定 | **AUTOMATED**（构建层） | `verify.yml` 3 job + 三个 `build*.yml` | 构建≠运行；**三端都没跑过应用实例** |
| 15 | 两台以上真实设备联调 | MANUAL-HARDWARE | 用户真机自测 | 同机双实例已跑绿 **J1 文本 + J2 文件 + 两格故障注入**（默认轮 16 断言 / 5 连绿；脏前缀轮 20 断言 / 2 连绿；续传轮 21 断言 / 1 连绿；`--negative`、`--fault=poison-part-lie`、`--fault=resume-prefix-lie` 三个反向模式都按设计报红），真机仍要人 → Smoke-5 |

## P0 mesh 本体（16–22）

| # | 验收项 | 等级 | 证据 | 缺口 |
|---|---|---|---|---|
| 16 | 多路径 + 单条有序字节流钉住单链路 | SIMULATED | `transport.rs` 95 条用例含 `both_sides_agree_on_the_same_link_budget` | 真实断链切换未跨进程 → A-2 |
| 17 | 中继转发 + 授权真的管到数据面 | SIMULATED | `commands/relay.rs` 8 条 + `relay_wiring_tests` | token **值**不得入日志/事件：有源码护栏，**无日志级断言** → A-2 |
| 18 | 群聊/群文件/群任务/@提及 | SIMULATED | `todos.test.ts` 22、`group_files` 相关 | 群收敛 + 离线成员补齐完全无自动化 → J4 |
| 19 | 删除一致性（不留幽灵数据） | SIMULATED | `db/cascade_tests.rs` 14 条 | 跨窗口/重启后的残留未测 → J7 |
| 20 | 蓝牙近场加入（预算同口径、大帧不静默丢） | SIMULATED + MANUAL-HARDWARE | `check-ble-constants.mjs`、`ble_framing` 10 条 | **真实 BLE 无硬件不可自动化** → Smoke-2 |
| 21 | 跨版本优雅降级（INV-P24） | SIMULATED | `unknown_wire_frame_is_tolerated_after_auth`、`messageKinds.test.ts` | 「新旧安装包互发」= MANUAL-HARDWARE → Smoke-6 |
| 22 | 大文件进行时聊天/控制帧不被饿死 | SIMULATED | P3 字节封顶用例、优先级队列用例 | 真并发下的时序无跨进程证明 → A-3 |

## 平台 / 硬件 Smoke 清单（这些永远不许出现在"绿"里）

| 编号 | 项目 | 可观测判据（用户本机） | 最近一次人工验收 |
|---|---|---|---|
| Smoke-1 | 多网卡/VPN/虚拟网卡下的发现 | 抓 `diag/discovery_started: … multicast_iface=`；期望出口是被评分选中的 LAN 网卡，不是 VPN/TAP | 未做（本轮） |
| Smoke-2 | BLE 真实收发（三端） | 日志 `[ble]` 前缀：peripheral 广播出去 → 对端 `+conn … path=Ble`；大文件不得走 BLE（16 MiB 阈值） | 未做 |
| Smoke-3 | 系统通知 + 点击跳转 | 后台收消息应出通知；点击后主窗口跳到该会话；关开关后**不得**出通知 | 未做 |
| Smoke-4 | 移动端群已读可见性（#30） | 群消息气泡下方应出现已读人数，点开浮层不超出屏幕右缘 | 用户已确认「已读在」；根因未定位 |
| Smoke-5 | 两台以上真设备联调（J1/J2/J4 全走一遍） | 用 `test-results/run-*/summary.html` 同样的断言清单手工走 | 未做 |
| Smoke-6 | 新旧安装包互发（INV-P24） | 旧→新、新→旧各发：文本/文件/未知 kind；期望「不支持的消息类型，升级后可查看」而非裸 JSON | 未做 |
| Smoke-7 | 移动端首屏与转屏（#27） | 冷启动权限弹框盖住 WebView 后，骨架结束**不得**出现桌面导航栏；转屏布局跟随 | 修复已提交 `48345bf`，**真机未证** |
| Smoke-8 | macOS 沙盒书签 / Android 厂商后台限制 | 收到文件写进自选目录不报错；重启后仍可写（书签回读成功） | 未做 |

## 这张表怎么维护（否则三周内又会漂）

1. **新写一条测试就改一行**：等级列只能向上走（MANUAL→SIMULATED→AUTOMATED），不许静默降级。
2. 等级为 `AUTOMATED` 的必须能填出**具体测试名或用例标签**；填不出来就写 SIMULATED。
3. §8 阶段 A 之后，Smoke 清单里的判据应逐步被 harness 接管，接管的判据整行搬到矩阵里。
4. 每次宣布「稳定版完成」前，此表必须**逐格有值**，且 `AUTOMATED` 格全部在 CI 真跑（当前只有第 14 行满足）。
