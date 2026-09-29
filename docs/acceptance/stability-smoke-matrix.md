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
>
> ★ **但证据列里凡「N 条用例」都只是当天读数，没有任何判据管得住**（判据 C 只管 E2E 轮次的断言数）。
> 2026-09-27 逐格复算时发现两格已经变了（`mesh/`、`migration_tests.rs` ⇒ 已改成只给命令），
> 当天还对得上的那几格（`discovery.rs` / `transport.rs` / `commands/relay.rs` / `cascade_tests.rs`）**同一条规矩**：
> 下次动它们时换成命令，**不要回填一个新数**。复跑形状：`grep -rEc "#\[(tokio::)?test\]" <路径>`。
> 同一次复算还抓到一格点名的**测试标签已经不存在**（见下面第 9 行那条改口）—— 名字比数更容易死，因为数变了顶多看着不对，
> 名字死了就是一条指不到东西的证据。

## 双实例 E2E 的轮次账（断言条数**只写在这一处**）

`check-doc-numbers.mjs` 判据 C 从 `scripts/e2e-multi-instance.mjs` 现算每轮条数并与下面这几行对账。
2026-09-26 之前这些数字散在十几段叙述里，加一条判据要改 11 处 —— 那是漂移的制造机而不是防漂移，
所以按判据 C 自己的建议收成一份：**要改就只改这里，叙述性行子里不再抄数字。**

> ★ **#145（2026-09-28）：每档"该红几条"不再是文字，改由机器基线记账。**
> 反证档 / 判据自查档的**退出码 + 报红条数 + 结论总数**存在 `scripts/fixtures/selfproof-baseline.json`，
> 核对：`npm run selfproof:check`（改了判据 / 夹具 / harness 之后跑；跑满全档本机实测一次 **820.2 秒**
> （2026-09-28，20 档，二进制已就位、不含编译。我原先在此处写的"约 30–45 分钟"是**没测过的估计**，
> 实测比它短一半以上 ⇒ 时长只当量级，别当基线抄），
> **档数不在这里抄** —— 那条命令第一行就打印它现数的分母；
> 单档 `node scripts/check-selfproof-baseline.mjs --only=<档名>`），重新生成：`npm run selfproof:sync`
> （**只在全档跑齐且每档符合自己口径时才写** —— 反证档跑成全绿会拒写，而不是把洞烘进基线）。
> 下面正文里那些"实测 9/36""恰好 1 条红"因此是**当时那一次观测的记录**，不是要人回填的契约：
> 数字漂了以基线为准，改的是文字而不是记忆。
> 两点边界，别读成"反证全都有锁了"：① 本工具**不进任何门禁层**（成本），所以它红的时候没人拦停，
> 是量具而不是闸门；② 它只覆盖脚本名以 `selfproof` 结尾的那些档（分母同上，由命令自己打印），
> **env 旋钮类反证不在里面**
> （如 `E2E_NO_CAPTURE=1`、`E2E_NO_ROUTED=1` —— 那些由各档自己的当场双面实测记账）。


- 默认轮 18 条断言 —— 反向：`--negative`（收件人换幽灵 id）
- 脏前缀轮 22 条断言 —— 反向：`--fault=poison-part-lie`
- 续传轮 23 条断言 —— 反向：`--fault=resume-prefix-lie`
- 杀进程轮 25 条断言 —— 反向：`--fault=kill-mid-lie`
- 冻结轮 24 条断言 —— 反向：`--fault=peer-freeze-lie`
- 磁盘轮 24 条断言 —— 反向：`--fault=recv-readonly-lie`
- 改小轮 24 条断言 —— 反向：`--fault=src-shrunk-lie`
- 多文件轮 24 条断言 —— 反向：`--fault=multi-file-lie`
- 改口轮 24 条断言 —— 反向：`--fault=recv-dir-rotted-lie`
- 停滞轮 25 条断言 —— 反向：`--fault=stall-mid-lie`
- 发送端被杀轮 26 条断言 —— 反向：`--fault=sender-kill-mid-lie`
- 群聊轮 36 条断言 —— 反向：`--round=group-lie`（+2 是 #103 的线级那一半：同一条管道上排两封，一封明文带 `mentions:[B]`、一封**没带这个键**，读的是 **B 自己日志**里那两行 `mentions=1 / mentions=none` ⇒ 判的是"名单穿过 seal→网络→解密→解析 到达了对端进程"。+3 是 #122 的落点那一半：夹具自查 1 条（红要能归因：镜像函数数成 0 条时，下一条的红得算在夹具头上）+ 正向 1 条（B 日志 `mentions=1 targets=1`）+ 只带名单的对照 1 条（`mentions=1 targets=none` ⇒ 今天线上 4.30.x 对端的原样形状，落点必须是「不知道」而不是「空落点」）。⚠️ 反向那一趟的红数**仍是 9**（实测 `9/36`）：这几条读的是真 id、不走 `want()`，翻 id 翻不到它们 —— 别把它们算进反向红数，也别改成走 `want()`）。★ 那三条落点判据有**自己专用的反证** `--round=group-targets-lie`：
它只把线上明文里的 `mention_targets` 键摘掉（拓扑/时序/名单/正文全都与正向档一字不差），期望**恰好 1 条红**
（正向那条），夹具自查与"只带名单"对照照旧绿 —— 需要它正是因为 `group-lie` 翻 id 对这三条永远够不到。
同所有 `-lie` 档：**不进任何门禁层**。
- 链式轮 25 条断言 —— 反向：`--round=gossip3-lie`（三实例，跑在**发版前**那一层 `npm run verify:release`，日常本地层不收）
- 补递轮 23 条断言 —— 反向：`--round=gossip-late-lie`（#77 晚到成员补递；同一族三实例拓扑、启动时序相反，也只挂在**发版前**那一层。⚠️ 原本 24 条、撤掉一条：那条「C 侧从来没有与 A 的建链行」把**某一瞬间的读数**当判据，而同机局域网迟早互达 ⇒ 换个时刻跑就从 0 变 1；详见 roadmap §12.7）
- 关发现轮 26 条断言 —— 反向：`--round=lanoff-lie`（#95 之后这一轮把「关了就真的不监听」也钉住了：`tcpOpen` 三条读数=开时连得上 / 闲口连不上 / 关时连不上，第三条缺前两条就是半个守卫）（#89：判"把局域网发现关掉之后，对端再也学不到我"。
- 任务轮 49 条断言 —— 反向：`--round=task-lie`（第二阶段 §22：一条任务「A 创建 → 被指派的 B 收到 → 改成完成 → 归档 → 重开」跨两个真实进程，载荷字段对齐 `TodoPayload`、按 seq 定序；**发起方已换成两侧**（B 自己建 `todo-e2e-2` 再自己改成 done ⇒ A 这一侧的落库行只可能来自 B 真投递，`creator` 这条授权输入也被读到了），另有一步专判「重启之后这六条状态行不退、已 Ack 的队列不被点亮成二次投递」，还有一步是 §28「正在任务同步时退出」那一格（对端整个缺席时把两条任务入队 → **发送端被 SIGKILL** → 再起两端：缺席期与崩溃后队列行都必须是 2 行、崩溃不许把「已入队」抹掉，重启后必须自己送到对端并被 GroupAck 清干净 —— 文件族⑩ 已证的同一件事，群任务这一族以前没有判据）。⚠️ 两格刻意**不**在本轮判，别读成已覆盖：徽标那个数字是前端 store 算的（库里没有这一格，本轮判的是它的输入），而"发起方自己发完立刻看到"在 harness 里是循环论证（应用没有"自己执行一次动作"的入口，两侧那一行都是预置写的）—— 后者要靠内核自 emit 留的那行痕迹补。
  这一轮**故意带着 `GOSSLAN_AUTOSTART=1` 跑** —— 要证的正是那个强制联网的环境变量不许越过用户显式写的"关"；
  三条腿：开着先学到 → 关掉后 announce 计数一字不涨（且它自己日志里 `discovery_started` 为 0）→ 翻回开又涨回来。
  ⚠️ 边界：这一轮**只判"不再广播"**。"关掉之后还不该接受拨入"那半句没有被证明 —— 第一次跑时
  `tcpOpen(端口)` 在关着的实例上回了 true，而两种解释（产品的 TCP 监听不受这个键管 / 上一轮报错留下的旧进程占着口）
  当场分不开 ⇒ 归因不清的读数不当判据，另立 #95 用一次干净复跑定死）
  - **★ #95 已落地，别当它还开着**：干净复跑之后是**三条读数各设断言** —— 键为「关」时监听口连不上（这才是要的答案）、
    翻回「开」时同一口连得上（正向对照）、本轮谁也没占的那个闲口连不上（**探针自己的负对照**，
    缺它则前两条都可能是半个守卫：一个永远回 true 的探针能让"开=连得上"假绿、让"关=连不上"看着像环境问题）。
    落点：`scripts/e2e-multi-instance.mjs` 里 `openWhileOff` / `openWhenOn` / `probeControl` 三条 `check`。
    ⚠️ 它先被判错过一次，原因是**探针放错了窗口**（量在"翻回开、重起 B"之后 ⇒ 出现三个读数互不相容的假矛盾）；
    「归因不清」与「探针读错了对象」在报告里长得一模一样，区别只有读数钉没钉在事件窗口上。

- 建群崩溃轮 26 条断言 —— 反向：`--round=groupcrash-lie`（#121：§28「链路失效」那一族里今天做得成的那一格。⚠️ 它**刻意不是**「重启后不许留下半个群」那种形状 —— 真实建群路径四张写在同一个事务里，那句永远绿（半个守卫）。钉的是投递那一半：群只长在 A 的盘上 → 只起 A、对端缺席 15s → 真 SIGKILL → 死透后再等 10s → 再起两端 ⇒ B 必须**自己**学到这个群，而那份「没送到」的重试登记是**进程内**的表、已被这次崩溃抹掉 ⇒ 证明的是「群名册才是事实源、链路活着就重递」，不是「内存缓存活下来了」。反向照 group-lie 的先例：**等待用真 id、判据读翻过的 id**）

- 续发轮 39 条断言（这格不归判据 B 管：判据 C 明写"删掉标签等于绕过"⇒ 文档必须带这个现算数，加/删断言要同一次改这里；现算口径 `node scripts/check-doc-numbers.mjs` 自己打印每一轮当前条数）—— 反向：`--round=posttext-lie`（§三 点名的「大文件之后继续发送普通消息」与 §五 那张"文件层不许拖垮消息层"。默认轮的顺序是**先文本再文件**，所以"一份文件走完之后再投一条"这句话今天才有判据：同两个实例、同一条已建立的链路，在 J2 收尾之后往 A 的 outbox 塞一条普通文本，判到 B 侧恰好落一条（既不卡死也不重复）、明文解得回来、A 侧 outbox 由**对端 Ack** 删除、状态前进过 sending，再加一条 P7 形状 —— 续发这条不许把刚完成那份的终态从 `done` 改回去。**同一轮还排上了那条群消息**（§三 那句「群同步是否仍然正常」以前和这句话是两件事：群相关的轮次跑自己的档位，从不与这条旅程合流）：J2、J3 都收尾之后才往 A 的 `group_outbox` 塞一条群消息 ⇒ 判到 B 侧那个群会话恰好落一条（既不卡死也不重复投）、明文解得回来、那一行的 `conv_id` 就是群会话（同一行不可能既属群又属 1:1 ⇒ 这条同时排除了串会话）、A 侧队列行被送达回收。群走的是 `flush_group_outbox` 那条**独立队列**（靠心跳冲，不靠下一次建链）⇒「文件层没拖垮 1:1」推不出「也没拖垮群同步」，这一格必须自己有判据。反向只翻判据读的三份值（1:1 的期望明文 + 读台账用的 transfer_id + 群那条读的会话 id），预置与时序一字不动 ⇒ 恰好 6 条红；而「群队列被回收」那条读的是**真 msg_id** ⇒ 反证档里它照绿，这就是「红来自断言本身、不是链路没跑」的对照。**J5 再把 §三 点名的「快速连续发送」补上**：五条**一次性**写进 A 的 `messages` + `outbox`（逐条写会变成五次串行往返，测不到同一批内部的相互影响），判六条 —— 各恰好落一行（不丢、不重复投）／**明文逐条对上**（同批连发最贵的破坏是串味）／五条同属一条 1:1 会话／outbox 全被对端 Ack 回收／五条状态都前进过 sending／这几条不许把前面那份文件的 `done` 带回去。⚠️ 刻意**不判到达顺序**：产品口径是「Ack 可以乱序」（`src/utils/messages.test.ts:558` 那条 L2 用例写的就是这件事），把 FIFO 写成判据会把一条允许的行为判成缺陷。这一格以前的全部证据只有那条 L2 用例，它判的是乐观态与 Ack 回收 —— 「N 条真过线」从没被一次跨进程跑证过。反向再多翻一处（连发第 3 条那格的期望明文）⇒ 恰好 6 条红。复跑 `npm run test:e2e:posttext` / `npm run test:e2e:posttext-selfproof`）
- 群文件轮 28 条断言 —— 反向：`--round=gfile-lie`（§七-4「文件 + 群聊」的跨实例那一半。此前这里只有 Rust 单元用例，而单元用例判的是纯函数 ——「密封文件密钥」「解封」「落盘」各自正常，从没证明过**一次真的跨设备投递能落到对端磁盘上**。形状：A 只把货备在自己盘上（群文件行 + 该成员 pending + 源文件 + 群密钥封装过的文件密钥），B 完全不知情地上线 ⇒ 生产 `flush_pending_group_files` 走 Offer→Chunk→Done 把整条流投完，判到 **B 落盘那份文件的 sha256 逐字节等于源** + 对端自己那条 `completed` 回执 + 气泡只出现在群会话里（不串到 1:1）+ 没有残留 `.part`。★ 密封那一步**必须走生产 crypto**：入口是 `cargo run --example e2e_peer -- --gfk <群密钥 b64>`，它用 `crypto::seal_symmetric` 现封一个随机 file_key 并自检解封；JS 侧**不复刻线格式**（复刻了就只能证明"两份实现自洽"，证明不了和真实现一致）。反向只翻判据读的那份摘要（不改任何生产写盘），所以红恰好落在"读到的是不是真落盘那份"两条上，其余 26 条照绿 —— 这既是反证也是"这轮不是靠基础设施噪声变红"的对照。复跑：`npm run test:e2e:gfile` / `npm run test:e2e:gfile-selfproof`）
- 单聊表情轮 24 条断言 —— 反向：`--round=dmreaction-lie`（需求汇总三点名「群聊 + 1:1 都要」表情回应，而 1:1 这一半此前只有单元级判据：位图门控、载荷校验、前端接线各自绿，**两个真实进程之间送一条静默事件**从没被判过。形状：先把 B 侧那条会话摆成静止态（摘要一句哨兵 + 时间戳一个定值），再往 A 的 outbox 塞一条 kind=reaction，判六条 —— B 侧恰好落一条／那一行的 kind 仍是 reaction（新枚举个变体真的被对端解析并落库）／静止态确实成立（不成立则下面两条是在判一个不存在的行）／时间戳没被顶上去／摘要仍是那句哨兵（没被载荷那段 JSON 覆写）／A 侧队列行被对端 Ack 回收。★ 为什么钉 last_msg + last_ts 而**不是** unread：这两列只有 `touch_conversation` 会写，而同一句写顺带把 unread+1，静默分支走的是 `ensure_conversation`（INSERT OR IGNORE，一个字节都不碰）⇒ 钉住这两列就等于钉住「没走那条会顶会话的路」；unread 判不得 —— B 是带界面的活进程，界面对打开着的会话本来就会清未读（本轮第一次跑就是被这一点判红的：实测 unread=0、同一句 UPDATE 写的摘要哨兵却保住了）。⚠️ 判不到的两半按 §十五 记未验证、不写 PASS：发送侧门控（`dm_allowed_by_features` 在 send_message 里面，harness 没有「让应用执行一条命令」的入口）与「折叠成一枚胶囊」（前端折叠，DB 里没有那一格）。反向只翻判据读的那两份期望值（时间戳 + 摘要），预置与时序一字不动 ⇒ 恰好 2 条红，而读真 msg_id 的那三条照绿。复跑 `npm run test:e2e:dmreaction` / `npm run test:e2e:dmreaction-selfproof`）

§十六 要的 `screenshots/` 现在真的有了：每轮两张全屏 PNG（链路建立后 / 两端重启后），
`summary.json.shots` 记相对路径、`summary.html` 内嵌图集；判据**只钉「落盘且不是空图」**，
而"不是空图"的口径是 **PNG 结构成立**（签名 + IHDR 宽高 > 0），体积只留一条挡桩文件的下限。
★ 这条口径是**被推翻一次之后改的**：第一版按体积定阈值（200 KiB，本机满屏时 1.77 / 1.93 MB），
桌面接近空白时同一台机器只截出 ~104 KB ⇒ 每一轮 E2E 在起跑前被自己的"先修判据再跑轮"拦停 ——
**红的是判据不是产品**。体积随屏幕内容摆两个数量级，不能当主判据；结构可以。
「界面长什么样对不对」不在判据里 —— 那一半仍按 §12.6 记结构级 / MANUAL。反证：`E2E_NO_CAPTURE=1` ⇒ 默认轮恰好 1 条红。

⚠️ 这里只声明"反向模式按设计会红"，**不声明红几条**。红几条取决于当时翻掉了哪个期望值，
写成常数就是第二个事实源（#64 之前那些「lie 恰好 1/22 报红」的分母，加一条断言后全要重算）。


## P0 地基（1–15）

| # | 验收项 | 等级 | 证据 | 缺口 |
|---|---|---|---|---|
| 1 | 局域网发现（多网卡/虚拟网卡不误判） | SIMULATED | `network/discovery.rs` 15 条用例 | 真实网卡组合无自动化 → Smoke-1 |
| 2 | 好友请求/接受/删除/重加，两端最终一致 | SIMULATED | `friendRequests.test.ts`、`commands/` 相关用例 | **无跨进程一致性证明** → J4 |
| 3 | 在线/离线（断链 ≠ 删节点） | SIMULATED | `mesh/` 全部用例（★ 条数**不抄**：这一格原先手写 79，2026-09-27 现算已变 —— "某目录 N 条用例"这种数没有任何判据管得住，判据 C 只管 E2E 轮次断言数。复跑 `grep -rEc "#\[(tokio::)?test\]" src-tauri/src/mesh`） | 同上 |
| 4 | 双向文本 + `msg_id` 幂等 | **AUTOMATED** | `e2e-multi-instance.mjs` J1（B 侧恰好一条 + `messages.msg_id UNIQUE`） | Windows 腿未跑 |
| 5 | Outbox → Ack，Ack 只代表已持久化 | **AUTOMATED** | J1「A 侧 outbox 被 Ack 清空」+ `cascade_tests` | 反例（DB 错不得 Ack）只有进程内用例 |
| 6 | 离线持久化与自动重发（离线 ≠ 2 分钟失败） | SIMULATED | `db/offline_queue.rs`、`fail_reason_separates_retryable_from_terminal` | 真离线对端的补发未跨进程 → J1n |
| 7 | 网络恢复后不丢不重 | **部分 AUTOMATED** | J1「两端重启后仍只有一条 / outbox 不复活」+ **故障注入** `--fault=poison-part`（接收侧脏 `.part` 前缀 → 第 1 次 attempt 整体校验失败 → outbox 重试补齐，2 连绿） | 只覆盖 LAN 路径 + 优雅重启 + 接收侧脏前缀 + **接收中 SIGKILL（杀进程轮 1 连绿、lie 按设计报红）**+ **对端失联后解冻自愈（冻结轮 lie 按设计报红）**；**「断链」单机无 root 不可自动**（造不出"只断一条链路"，用中继伪造会被 route 优先级绕过 ⇒ 跑出来是假绿）；**重复帧 / 乱序帧要分层读，别当成"没测"**：L2 层 **AUTOMATED** 且点得出真测试（`file_relay.rs duplicate_chunk_is_ignored_not_double_counted`、`network/file.rs chunk_seq_rule_only_rejects_real_gaps`、`group_receive_rejects_gap_and_duplicate_seq`、`transport/ble_framing.rs out_of_order_chunks_still_complete` + `duplicate_and_partial_do_not_complete`、`network/file.rs stale_attempt_frames_are_filtered_but_legacy_frames_never_are`）；L5 帧级伪造判为**不可自动**（往对端塞一帧 = 伪造 E2EE 封装与签名，生产码路径不走 ⇒ 假绿）→ A-2 |
| 8 | 已读回执与送达状态 | SIMULATED + 人工 | `storeContract.test.ts` 已读判据、`applyConversationSnapshot` | 移动端群已读「不见了」= #30，**未定位** → Smoke-4 |
| 9 | 失败可见且可重试（重发必须重新加密） | SIMULATED | ★ **这一格点名的证据名字今天已经不在了**：原先写 `resend_reseals_before_enqueue` 等护栏 —— 那条护栏是随 `resend_message`/`cancel_send` 那批"注册了但前端零引用"的命令一起**退役**的（0-A3，`ARCHITECTURE-MAP.html` 那张卡写明"那两条不变量没有丢，改记在 `protocol-invariants.md` §6 例外"）。现在真在跑的同一条不变量是 Rust 用例 **`reseal_with_current_receiver_key_recovers_where_retry_cannot`**（钉的是：接收方换身份后 outbox 里那份旧密文**重发多少次都解不开**、必须由持有明文的发送方用**当前**公钥重封；缺明文或缺公钥时一律 `None` ⇒ **绝不伪造内容**）。复跑：`cargo test --features bluetooth --lib reseal_with_current_receiver`（★ 这条命令的过滤词打空时 `cargo test` 仍会退 0，所以要看它打印的 `1 passed`，别只看退出码） | 见 `protocol-invariants.md` §6 例外（界面上的「重发」= 重发一条新消息，新 `msg_id`）；**"界面重发"这条路径本身没有跨进程判据** |
| 10 | E2EE 身份锚定，未验签不建信任 | SIMULATED | `friend_identity_anchor_has_one_binding_rule`、INV-P21 用例 | 真实冒名建链未跨进程测 |
| 11 | SQLite 持久化 + 重启恢复（含在途队列） | **部分 AUTOMATED** | J1 重启断言 + `migration_tests.rs` 全部用例（★ 条数不抄，原先手写 19 条已变 —— #59/#60/#61 三格都是往这个文件里加断言的；复跑 `grep -cE "#\[(tokio::)?test\]" src-tauri/src/db/migration_tests.rs`）+ `fresh_schema_alone_has_exactly_the_migrated_shape` + **建群崩溃轮**（#121：群只长在 A 的盘上 → 真 `SIGKILL` 发送端（那份「没送到」的重试登记 `pending_group_keys` 是**进程内**的表，必然被抹掉）→ 再起两端 ⇒ B 必须**自己**学到这个群、成员恰好 2 位、密钥与 A 那份逐字节相同、且不凭空多出会话行） | 迁移**中途失败**不可恢复 → J7 |
| 12 | 图片/文件消息（多选并发不丢件） | **部分 AUTOMATED** | J2 已 **4 连绿**（另：脏前缀注入轮 **2 连绿** + `--fault=poison-part-lie` 反向按设计报红）（1 MB：只有 rename 后出现最终名 + 字节数 + sha256 + 发送侧 `sent → done`（必须等对端 `FileCompleteAck`）+ 接收侧 `done` + 无 `<tid>.part` 残留 + `file_outbox` 收尾删除） | 只覆盖单文件；**尺寸阶梯已按形状进本地层**（1 KB 单片 / 10 MB 多片带零头，各跑整趟默认轮、实测全绿，10 MB 整轮 11.2s；10 KB 与 1 KB 同形状由前者代表，100 MB 及以上不进本地层、由杀进程轮 100 MB 代偿）⇒ A-3 只剩**错 size** 一格**同日已补齐**（两层各一条：写盘**之前**的 chunk 级上限 `chunk_exceeds_declared` —— 恰好填满=放行 / 超一字节=拒 / 已收满再来片=拒 / size=0 不写字；加上收尾层原本就有的"字节数与声明不符 ⇒ 不算成功"。⚠️ 这条判据以前内联在吃 `AppState` 的 `write_chunk` 里所以一直没测试，见路线图 L-C 那格的 ⚠️）（⚠️ 这句以前连着写"并发未做"，与本行末尾自相矛盾 —— 并发多文件已由注入⑤覆盖，只是那条时序是"同 peer 串行 flush"）。**「目标目录变化」的另一半已做**：整个目录被删走 ⇒ 接收句柄挂在被 unlink 的 inode 上、写入与 fsync 照旧成功 ⇒ **只剩最后一次 rename 能发现成品无处安放**，由 L2 `network/file.rs::rotted_receive_directory_finishes_failed_never_done` 钉住（判据：必须 Err + 台账只能 `failed` 且不带路径 + 不许出现成品文件；已用"把 rename 失败吞掉就算成功"变异证明会报红）。它与本行末尾那条注入⑧不是重复 —— ⑧ 是目录仍在但不可写（`EACCES`、两个真实进程），这条是目录消失（`ENOENT`、生产单聊收尾函数）。**已做**：接收端写不进去（磁盘轮 1 连绿、lie 按设计报红 ⇒ 队列 5 次内 GiveUp、台账非 done、盘上零残留）；⚠️ 但**接收方自己完全无感**（offer 期只记日志、不写库不发事件）→ A-10。**已做**：收到一半才收尾失败（改口轮 lie 按设计报红 ⇒ .part 必须真被续写过、A 侧终态明确、attempts ≤ 5、接收侧台账不假 done）；★ 同一轮实测照出 **A-12** 并已修（修前实测 `A {status:gone, aT:done} / B=failed / .part=整份 / final 不存在`）：判据改成**字节数够 ≠ 收完、也 ≠ 进度**，那份没有成品的 `.part` 不再被报成 `received = size`（一个帧承载两个含义是这一格的病根）⇒ 发送侧只能落到 `failed`，交叉自洽那条已按原口径加回断言，另加一条防它变成永远为真的空转判据。**已做**：入队后源文件被改小（改小轮 3 连绿、lie 按设计报红 ⇒ offer 按截断后的真 size、落地字节+hash 与新源一致、两侧同 done、盘上只留终名那一份）；⚠️ 同一轮照出**发送侧的 size 没人更正**（实测 A 气泡/台账仍写入队时那份、B 写真实那份）→ A-11。**已做**：连续多文件 + 其中两单同名（多文件轮 lie 按设计报红 ⇒ 三单各自 done、同名落成两个不同路径、落地内容多重集合 == 源内容多重集合、无 `.part`）；⚠️ 只覆盖"同 peer 串行 flush"这条时序，**两个 offer 都在任一次 rename 之前到达**那个真会撞 final_path 的交错没覆盖（要三个实例）|
| 13 | 通知（尊重开关、失败可观察） | MANUAL-HARDWARE | — | 见 Smoke-3 |
| 14 | Win/mac/Android 三端构建与基本稳定 | **AUTOMATED**（构建层） | `verify.yml` 3 job（现算 `python3 -c` 读 `jobs:` 的顶层键 ⇒ `frontend` / `rust` / `android`）+ `build*.yml` **条数不抄**（★ 这里原先写"三个"，#36 那档内置 WebView2 落地后已多一条；复跑 `ls .github/workflows/` ⇒ 除 `verify.yml` 之外都算出包路径，**其中 `build-windows-webview2.yml` 只挂手动触发 ⇒ 它不在"自动"里，别把它算成绿**） | 构建≠运行；**三端都没跑过应用实例**；手动那档从未被点过（#36 剩的那半） |
| 15 | 两台以上真实设备联调 | MANUAL-HARDWARE | 用户真机自测 | 同机双实例已跑绿 **J1 文本 + J2 文件 + 两格故障注入**（默认轮 5 连绿；脏前缀轮 2 连绿；续传轮 1 连绿；`--negative`、`--fault=poison-part-lie`、`--fault=resume-prefix-lie` 三个反向模式都按设计报红），真机仍要人 → Smoke-5 |

## P0 mesh 本体（16–22）

| # | 验收项 | 等级 | 证据 | 缺口 |
|---|---|---|---|---|
| 16 | 多路径 + 单条有序字节流钉住单链路 | SIMULATED | `transport.rs` 95 条用例含 `both_sides_agree_on_the_same_link_budget` | 真实断链切换未跨进程 → A-2 |
| 17 | 中继转发 + 授权真的管到数据面 | SIMULATED | `commands/relay.rs` 8 条 + `relay_wiring_tests` | token **值**不得入日志/事件：有源码护栏，**无日志级断言** → A-2 |
| 18 | 群聊/群文件/群任务/@提及 | 群聊文本族 **AUTOMATED**（2026-09-26 起）+ 多跳转发与晚到补递族 **AUTOMATED**（2026-09-27 起：`--round=gossip3` / `--round=gossip-late`，都挂在发版前那一层）+ **群任务族与 @ 存储侧 AUTOMATED**（2026-09-27 起：任务轮 + 群聊轮那条 @ 正文）/ 其余 SIMULATED | `todos.test.ts` 22、`group_files` 相关；**群聊轮**（条数只写在本文件顶部「轮次账」；两个真实进程：两端预置群 → A 排「正文/撤回/第二条正文/**一句 @**/**一条只带身份号**/**一条旧形状不带该键**」→ 对端上线后由 `flush_group_outbox` 补发 → 判「各只落一行 + 明文真解得开 + 落的是群会话 + **@ 那串字节在对方库里逐字等于原文、既没被换成「@你」也没被规范化成 device id** + GroupAck 把队列清成 0 + 撤回物化 `kind=recalled` 且不删行 + @ 名单跨进程送达（带名单那条在对端解出 `mentions=1`、没带键那条解出 `none` 而不是 0）+ **落点同一条管道各钉一次（#122 第二段：名单+落点都带那条解出 `targets=1`；只带名单没有落点那条解出 `targets=none` = 今天线上老对端的原样形状，必须停在"不知道"而不是"空落点"；外加一条夹具自查让红能归因）**+ 群消息一条都不许串进 1:1」；反向 `--round=group-lie` 只翻判据读的 id ⇒ 9 条红、其余与 id 无关的保持绿） | ⚠️ 这一轮证明的是**接收侧的跨实例收敛**，不证明"发送内核自己怎么组信封"（群 payload 不做 re-seal，`transport.rs:6689-6693`，所以信封由 harness 自制）。仍**无自动化**：群文件端到端（只有 `group_files` 单元判据）、公告跨实例、群已读回执点亮，以及「@你」在**真界面上长什么样**（库里存真实昵称已钉住，替换发生在渲染层）—— 这句今天**改准成两半**：**浏览器内那一半已判**（探针第三段：同一句 @ 在「我」的视角换成 i18n 那个自我标签且 `.mention-token--self` 恰好一个、对端视角仍是名字原文）；**真 WKWebView / WebView2 那一半仍未判** ⇒ 仍归 Smoke-11 人工，不许写成"全验过"。**三个实例的群收敛（gossip 多跳）已于同日 #75 补上**（`--round=gossip3`，跑在 `npm run verify:release` 发版前那一层；判的是"A 从没直发给 C ⇒ C 靠中间人扇出收敛"，⚠️ 它**不**判"晚到的成员能不能补到"——那一格已由同日 #77 的 `--round=gossip-late` 补上）。**同日 #103 也结掉**：「有人@我」那枚红点不再按昵称判 —— 群聊明文新增三态字段 `mentions`（被 @ 者的设备 id），自动化判据在三层：单元（`messages.test.ts` 改名后仍亮 / 同名两人不再一起亮 / 缺键才兜底 / 空名单不兜底 / @所有人 与 id 互不干扰 等 8 条 + `protocol.rs` 编解码三态）、护栏（4 条注入用例 `--only=mention-identity`）、**跨进程**（群聊轮那两条：对端自己日志里 `mentions=1` 与 `mentions=none` 各一行）。⚠️ 两句边界：① 改的是**判定**，正文里那串 `@名字` 一个字节没动 ⇒ 上面"逐字等于原文、也没被规范化成 device id"那条判据依然成立（新名单是**旁边**一个独立字段）；② 跨进程那一腿判到的是**线级送达**（名单确实穿过加密到达对端进程），**不是那枚红点** —— 徽标活在 webview 的 store 里、不落库也不进日志。**这一环 09-27 深夜补上了它的最后一半**：
运行时探针第四段判的是「store 说亮了 ⇒ 界面上真看得见、整行的 accessible name 里真有那句@我」，
同一段还带三态全清的反向对照与「单聊不许出现任务数」那条 ⇒ #117 剩下的只有**真 WKWebView / WebView2**
那一层（仍归 Smoke-11 人工，别读成已收口）。，人工补一格：群里放两个同名成员，@ 其中一个，另一个不该亮 —— ⚠️ **这一格今天只对「红点」成立**：#103 改的是**判定层**（`messageMentionsMe` 走 `mention_ids`）。**呈现层是另一件事**：高亮与「@你」标签仍按我自己的昵称匹配正文里那串名字（`useMentionContext.ts:36-39` 的输入就是 `device.nickname`），而 id 是一份**名单**、定位不到「正文里哪一段 @ 是谁」⇒ 这一半已于 2026-09-28 落地为**带落点的提及**（协议新增 `mention_targets`：`{id, name, n}`，`n` = 该昵称在正文里第几次以 `@昵称` 出现；刻意不用字符偏移 —— Rust 按字节、JS 按 UTF-16，偏移跨语言传必错）⇒ **同名两人的高亮现在按身份判**，两边各自独立可测（`linkify.test.ts` 三条用例 + Rust 侧 `build_mention_targets` 三条 + 那条多段接缝判据把「发送侧算落点 → emit → 气泡收下」三段钉住）。⚠️ **边界（2026-09-28 拆成两半，别再合着写）**：名单 `mention_ids` 仍**不落库**（徽标是摄入时的一次性判定，重启不该再亮）；
而**落点 `mention_targets` 已经落库**（`messages.mention_targets`，迁移 v10→v11，可空、无回填）⇒
"重启后读历史又退回按昵称判"这半句**已经不成立**，现在成立的是"老行没有那一列的值 = 不知道 ⇒ 仍按昵称兜底"。
所以这一格要按层记成「当次投递：机器已判 / **重启后的历史：机器已判（库里读回来，单元层三条具名用例）** /
重启后**界面上真的按库里那份标**：仍无端到端判据 / 真 WebView：Smoke-11」，别一次记成「全过」。 ★ **2026-09-28 这两行各补一格（都在本行范围内）**：① 群里发完「显示两条自己发的」根因是**内存里同一个 msg_id 被插了第二份** —— 一条真实记录有两条到达路径（后端为跨窗口同步加的自 emit、以及 `invoke` 的返回值），而那次「乐观占位→真实记录」的替换原来是**按位置**做的。判据收在 `messages.test.ts` 的 `replaceOptimistic` 那几条 + `designGuards.test.ts` ㉑（形状锁，带「坏形状必须被抓到」的对照）；等级仍是 SIMULATED —— **真机上还出现两条与否只有他能判**（已发验证请求）。
② 群文件一次投递失败的**终态**不再停在「发送中」：能补发的回 pending（下次心跳/重连自己走）、永远给不了货的记 failed、用户取消记 cancelled；判据在 `commands/group_file_dispatch_tests`（含一条内存 SQLite 上「sending 对补发查询不可见、回到 pending 后可见」的行为断言）。⚠️ **「离线成员先 pending、重连后自己补上」这一格没有任何跨进程判据** ⇒ 仍归本行人工，别读成已验。
 |
| 19 | 删除一致性（不留幽灵数据） | SIMULATED | `db/cascade_tests.rs` 14 条 | 跨窗口/重启后的残留未测 → J7 |
| 20 | 蓝牙近场加入（预算同口径、大帧不静默丢） | SIMULATED + MANUAL-HARDWARE | `check-ble-constants.mjs`、`ble_framing` 10 条 | **真实 BLE 无硬件不可自动化** → Smoke-2 |
| 21 | 跨版本优雅降级（INV-P24） | SIMULATED | `unknown_wire_frame_is_tolerated_after_auth`、`messageKinds.test.ts` | 「新旧安装包互发」= MANUAL-HARDWARE → Smoke-6 |
| 22 | 大文件进行时聊天/控制帧不被饿死 | SIMULATED | P3 字节封顶用例、优先级队列用例 | 真并发下的时序无跨进程证明 → A-3 |

## 消息呈现族（23–23）

| # | 验收项 | 等级 | 证据 | 缺口 |
|---|---|---|---|---|
| 23 | 表情回应（群 + 1:1，同一表情多人聚合、能看谁点的与谁最后追加） | **部分 AUTOMATED** | 跨实例：`--round=dmreaction`（B 侧恰好落一条 + `kind` 仍是 `reaction` + 会话 `last_msg`/`last_ts` 一字未动 + A 侧队列由对端 Ack 回收）与群聊轮 `--round=group` 那条 reaction 族；折叠与聚合：`utils/reactions.test.ts`（LWW 折叠、按 `(seq,msg_id)` 排、`ROSTER_VISIBLE` 折 +N）；线上形状：`protocol_tests.rs`（载荷校验、门控位图、"群侧不许被 1:1 门控污染"）；接线：`dmReaction.test.ts` | 三格明写未自动化：① **「折叠成一枚胶囊 + 谁最后追加」是渲染层**，DB 里没有那一格 ⇒ 只有 TS 用例与运行时探针级证据；② **1:1 发送侧门控** harness 判不到（`dm_allowed_by_features` 在 `send_message` 里面，进程外没有"让应用执行一条命令"的入口）⇒ 只有源码护栏 + 单元用例；③ **真机观感**（名单浮层位置、「+N」可读性、移动端 450ms 长按）= MANUAL，本机锁屏未跑 → Smoke-11 |

## 组合面对账（总任务 §七-4 点名的那 14 条：A+B，而不是"A 绿 + B 绿"）

口径：每行只写**判据在哪个具名入口、哪一层、进不进远端 CI**，不写条数（条数由该入口自己打印）。
列里的名字**只写仓里真存在的东西**，并且分两种标法：`单元真名：`= 带 `#[test]` 的用例、`护栏「」`= `scripts/verify-guards.py` 里的用例名。
⚠️ 这一版是分两种写完之后**回查过**的：第一次落笔时我把一份"按关键词 grep 出来的函数名清单"当成了测试名单，于是 `retry_incomplete_content`、`reseal_for_send`、`mark_peer_keys_verified` 这类**生产函数**、甚至 `peer_offline_after_tail` 这种**grep 不到的名字**都混进了"判据"那一列 —— 回查（读每个名字上方的属性行）抓到 9 个，全部改掉。回查抓到 9 个，全部改掉。

| 组合 | 判据在哪（具名） | 层 | 进远端 CI？ | 状态 |
|---|---|---|---|---|
| 聊天 + 文件 / 大文件 + 普通消息 | ⚠️ **2026-09-29 就地改直**：这一格原来写"传完文件后同一对进程继续发文本并判落库"，**而那句话在代码里不存在**。默认轮真判的是这份传输本身：B 侧字节数与发送端一致、B 侧 sha256 与源文件一致（INV-P17 分片可验证）、A 侧 `file_outbox` 行被收尾删除、A 侧 send 记录终态是 done（不是「写完 socket」的 sent）、B 侧同一条传输只记一次、接收目录无 `.part` 与改名副本残留，再加 L-B 那一步的重启幂等。复跑：`grep -oE 'check\("[^"]*"' scripts/e2e-multi-instance.mjs` 逐条读名字，含「文本 / 普通消息 / 续发」的默认轮判据一条也没有。档位两步 `--size=0.001` 与 `--size=10` 是真的 | 本地 E2E 层（只判传输与重启） | ❌（远端 verify 只跑 frontend / rust / android 三组） | **2026-09-29 补齐**：续发轮 `--round=posttext` 判的就是这句话（J2 收尾之后往 A 的 outbox 塞一条普通文本 ⇒ B 侧恰好落一条、明文解得回来、A 侧 outbox 由对端 Ack 删除、状态前进过 sending、刚完成那份终态仍是 `done`）；反向 `posttext-lie` 恰好 6 条红。默认轮本身仍只判传输与重启幂等（那句话以前是许愿，已删） | 本地 E2E 层（默认轮判传输；续发轮判"传完之后"） | ❌（远端 verify 只跑 frontend / rust / android 三组） | AUTOMATED-LOCAL（含 600MB 档实测一次，见下方那一格） |
| 聊天 + 群聊 | 群聊轮 `--round=group` 里那条"群消息一条都不许串进 1:1" | 本地 E2E 层 | ❌ | AUTOMATED-LOCAL |
| 文件 + 群聊 | 单元真名：`group_file_keys_distinct_across_transfers`、`group_file_recipient_states_persist`、`list_group_files_scoped_and_newest_first`、`group_file_progress_averages_online_members`（离线成员不摊进进度）＋ **跨实例那一半 2026-09-29 起有判据**：`--round=gfile`（A 只把货备在自己盘上，B 上线后由生产 `flush_pending_group_files` 投完，判到 B 落盘那份的 sha256 等于源；条数见下方轮次账） | 单元 + 双实例轮次 | ✅ rust job（单元）/ 本地专项层（轮次，远端不跑） | AUTOMATED |
| 文件 + 消息（同一条链路混排） | `bulk_messages_are_only_large_chunks`、`frame_roundtrip_large_payload`、`roundtrip_across_sizes_and_mtus` | 单元 | ✅ | AUTOMATED |
| 文件 + reconnect（断链不误杀其它传输，P1） | 护栏「断链清理必须在确认这个 peer 真的一条链路都不剩之后」「接收器回收必须走判据与摘表同一次持锁的 take_*」；单元真名：`stalled_receiver_is_reclaimed_only_after_the_idle_window` | 护栏 + 单元 | 单元 ✅；护栏在本地/全量层 | AUTOMATED |
| 文件 + sync（乱序/重复分片、断点续传） | 单元真名：`large_gossip_payload_downgrades_to_low`、`worth_replaying_requires_ttl_to_survive_one_more_hop`；护栏「中继收文件的哈希必须对组装后的明文算」「幂等 accept 时必须重置段号」「重复 FileDone 的『本机没这份文件』出口不许退回静默」 | 护栏 + 单元 | 同上 | AUTOMATED |
| task + group | 群任务轮 `--round=task`（反证 `--round=task-lie`）；Rust：`todo_commands_log_identity_only_after_the_send_succeeded`、`tasks_window_uses_one_fixed_label_cross_checked_with_frontend`；渲染侧＝运行时探针任务段 | 本地 E2E + 单元 + 浏览器内 | 单元 ✅ | AUTOMATED-LOCAL |
| notification + unread | 徽标渲染＝运行时探针的会话行段（`npm run test:ui-runtime`）；**真系统通知与点击跳转＝Smoke-3，人工** | 浏览器内 / 人工 | 探针在本地层 | SIMULATED（最后一公里人工） |
| identity + friend（换身份不污染旧关系） | ⚠️ **2026-09-29 就地改直：上一版把这一格判成"强度只是护栏级的"，是低估**。按 §四-A 逐条对上、每条都当场读过名字上方的属性行（`#[test]` / `#[tokio::test]`）：唯一性与形状 `generated_id_keeps_the_one_prefix_and_length`、`empty_attributes_still_yield_valid_distinct_ids`；**指纹碰撞那次真机事故的正向回归** `identical_attributes_still_produce_different_ids`（属性逐字相同的两台连导两百次不许撞）；历史前缀就地迁移 `legacy_dev_prefix_is_stripped_exactly_once`；身份变化不污染旧关系 `identity_merge_never_overwrites_existing_keys`、`forget_identity_clears_keys_but_keeps_the_connection`、`removing_a_friend_also_drops_the_in_memory_identity_binding`、`friend_identity_anchor_has_one_binding_rule`；旧身份不许被复用或伪造 `hello_auth_accepts_bound_identity`、`hello_auth_rejects_forged_sig_with_victim_pubkey`、`announce_rejects_forged_signature_with_victim_pubkey`、`negotiation_fails_closed_when_identity_does_not_match`、`update_friend_pubkeys_never_overwrites_a_bound_key`、`gossip_trust_for_known_friend_never_tofus`、`new_peer_entry_respects_friend_key_anchor`；旧会话与旧记录不被身份同步删掉 `existing_conversation_survives_group_relation_sync`、`migration_preserves_identity_data`、`clear_all_data_sql_deletes_and_preserves`（清空业务数据后 device_id 与好友仍在）；昵称后缀 `default_nickname_is_deterministic_and_looks_right`、`different_devices_get_different_suffixes`。护栏两条也核到过：「身份锚点的打标点必须留在 handle_message 的 Hello 分支」「打标必须排在 upsert_peer 之后」（形状是 `name=`，不是 `Case("…`）。`acceptable_friend_keys` / `mark_peer_keys_verified` 仍是**生产函数**、没有以自己名字命名的行为用例，那句保留。⚠️ §四-A 另有**三条今天不成立**，别读成"已覆盖"：①「用户自定义 3 位后缀」全仓只命中 `device.rs` 里那句长度注释（复跑 `grep -rn 自定义.*后缀 --include='*.rs' --include='*.ts' --include='*.vue' .`，只有一行）⇒ **没有实现入口**，属"计划中的扩展点"；②「初始化时**重新生成**」与代码里的要求恰好相反（`state.rs` 那段：只认已持久化的值、没有才生成一次），而"重启不再派生"这一分支没有以自己名字存在的用例，现有覆盖只到 settings 读写与迁移不丢；③「同 ID 不同密钥 ⇒ 提示换新 ID」= ADR-0021 切片 2 **未实现**（`state.rs` 那段注释自己写着）⇒ 已经撞上号的两台今天没有自愈路径 | 单元 + 护栏 | 单元 ✅（rust job）；护栏在本地/全量层，远端不跑 | **AUTOMATED（单元级，比上一版记的强）**；跨实例那一半仍缺 —— harness 认得的轮次（`grep -oE 'ROUND === "[a-z0-9-]+"' scripts/e2e-multi-instance.mjs` 现取）里没有 identity / friend 轮 |
| Bluetooth + LAN | `ble_only_link_must_not_start_a_hopeless_large_file`、`ble_start_does_not_block_on_the_peripheral_state_wait`；**真机 BLE 收发＝Smoke-2，人工**（同机 peripheral 的广播不会回喂本机 central，实测） | 单元 + 人工 | ✅（本地层那份二进制也带 `--features bluetooth`，见 `scripts/verify.mjs` 里 build 那一步） | SIMULATED |
| LAN + 中继/routed | `unhealthy_lan_does_not_block_healthy_routed`、`route_order_plus_send_delivers_on_healthy_link_after_lan_degraded`；跨实例关局域网轮 `--round=lanoff`（反证 `lanoff-lie`） | 单元 + 本地 E2E | 单元 ✅ | AUTOMATED-LOCAL；"断一条链路看谁接管"已判为**单机不可自动**（routed 与 LAN 共用同一个 TCP 监听口） |
| 多连接 + 大文件（队列内存封顶，P3） | 护栏「链路队列必须按字节封顶：折算槽数不许被换成常量深度」「链路队列的字节预算必须真的参与折算」；`inflight_cap_blocks_state_flood`、`never_awaits_while_holding_the_links_lock` | 护栏 + 单元 | 单元 ✅ | AUTOMATED |
| 大文件 + 群同步（§三 点名的 600MB+ 那一格） | **旅程轮现跑的档位只有 `--size=0.001` 与 `--size=10`**（复跑 `grep -n -- '--size=' scripts/verify.mjs`）；**100 MB 那一档在自动化里是有的，但只出现在三个故障窗口轮**（`E2E_KILL_MB` / `E2E_SENDKILL_MB` / `E2E_STALL_MB` 默认都是 100 ⇒ 接收中被杀 / 发送中被杀 / 字节在飞时冻住对端），而**这三条都不判"传完之后聊天与群同步照常"**；形状最近的判据是 `group_rows_survive_the_link_flap_window_that_kills_single_chats` 与群密钥重递那一族 | 本地 E2E（故障轮）+ 单元 | 单元 ✅ | **2026-09-29 就地改直（群同步那一半已落地）**：续发轮的 J4 在 J2（文件）与 J3（1:1 续发文本）都收尾之后才往 A 的 `group_outbox` 排一条群消息 ⇒ 判到 B 侧那个群会话恰好落一条 + 明文解得回来 + 那一行的 `conv_id` 就是群会话 + A 侧队列行被送达回收；600 MB 档按**含 J4、但还没有 J5 的那一版**整跑复证过（复跑 `E2E_FILE_MB=600 npm run test:e2e:posttext` ⇒ **今天这一整轮已在 600 MB 档重跑过一遍并全绿**，条数与每步用时都由那一轮自己打印、这里不抄；口径变化要说清：更早那一趟只有 28 条，缺的正是后来加的 J5 那六条「快速连续发送」⇒ 那六条现在也在这个档位上过了一次）。⇒ 旧那句"没有任何自动化层"里挂在下面的**时长理由就此撤回**：600 MB 这一档跑得完，缺的从来是判据不是时间。⚠️ **仍缺的是「跨实例建群」那一半，而且原因不是时长**：`create_group` 与 `distribute_group_key` 是 Tauri IPC 命令，harness 从进程外只能写 SQLite，写不出那一条群密钥分发帧 ⇒ 这一格属真机 / 第二设备那一层，既不许记成"已覆盖"，也不许记成"自动化通过" |
| A + B + C（文件在途 + 群同步 + 进程被杀） | `--round=groupcrash`（群只长在发送端盘上 → SIGKILL → 再起两端：对端自己学到这个群、密钥逐字节相同、不多出会话行）+ 反证 `groupcrash-lie` | 本地 E2E 层 | ❌ | AUTOMATED-LOCAL |

**600MB 那一档今天真量了一次（2026-09-29，一次性证据、不接进任何门禁层）**：复跑
`E2E_FILE_MB=600 node scripts/e2e-multi-instance.mjs` ⇒ 退码 0，条数与每步用时由脚本自己打印（结论行形如「✅ 多实例 E2E 全绿（N 条断言）」）。
这一档判到的是**传输本身与重启幂等**：B 侧字节数与 sha256 都等于源、双端终态 `done`（不是"写完 socket"的 sent）、
A 侧 `file_outbox` 行被收尾删除、同一条传输只记一次、接收目录无 `.part` 与改名副本、重启后不复活已 Ack 的行。
⚠️ 它**没有**判"传完之后普通消息 / 建群 / 群同步照常"—— 上面第一行原来写着那条，实测之下那句话在代码里不存在，已就地改直。
⇒ 于是 §三 点名这一格的结论换成一个更准的形状：**缺的不是把档位从 10MB 提到 600MB（那一换挡几乎不花时间），
缺的是"传完之后同一对进程续发一条普通消息并判落库"这条判据本身**（任何尺寸都没有）。
旧的"600MB 以上因为时长所以不做"那句理由就此撤回：600MB 那一趟整轮跑完的时长远低于门禁里任何一层
（要引用就现读那一趟自己打印的行，别抄进本文档）。
⇒ **当天稍后这一格就按这个结论补上了**：`--round=posttext` 落地，并当场在 600MB 档跑过一遍
（复跑 `E2E_FILE_MB=600 node scripts/e2e-multi-instance.mjs --round=posttext` ⇒ 退码 0）
⇒ 「600MB 传完之后普通消息 / 文件消息 / **群同步**照常」这一半不再靠推断（群同步那一半由同一轮的 J4 兜住，档位复证见上一行）。⚠️ **仍缺的那一半换成更准的一句**：
群聊**创建**在 600MB 档没有判据 —— 原因不是时长也不是档位，是 `create_group` 与 `distribute_group_key` 是 Tauri IPC 命令，harness 从进程外写不出那一条群密钥分发帧（与本节第一行末那句同一个原因）。⚠️ 这一句今天收窄：它原来连「群同步」一起否掉，而群同步那一半已由同一轮的 J4 判到 ⇒ 照抄会让本节自己前后矛盾
⇒ 别把这一格读成"§三 全闭合"。

★ 这张表**不许读成「CI 覆盖组合面」**：远端只有 unit / lint / clippy / rust 测试 / android 检查与三份出包流水线，**双实例轮一条都不在远端**（复跑：`grep -n "verify.mjs --group" .github/workflows/verify.yml`）。
凡标 `AUTOMATED-LOCAL` 的行，这层保护只在有人本机跑 `npm run verify:e2e` 时存在——这正是 §七-4 要的那类组合目前唯一断在哪里的地方。

## 平台 / 硬件 Smoke 清单（这些永远不许出现在"绿"里）

| 编号 | 项目 | 可观测判据（用户本机） | 最近一次人工验收 |
|---|---|---|---|
| Smoke-1 | 多网卡/VPN/虚拟网卡下的发现 | 抓 `diag/discovery_started: … multicast_iface=`；期望出口是被评分选中的 LAN 网卡，不是 VPN/TAP | 未做（本轮） |
| Smoke-2 | BLE 真实收发（三端） | 日志 `[ble]` 前缀：peripheral 广播出去 → 对端 `+conn … path=Bluetooth`。⚠️ **这一格原先写的期望文本永远不可能出现**：它写的是 `path=Ble`，而那行日志用的是 `PathKind` 的 **Debug** 格式（`"+conn peer={peer_id} ep={endpoint} path={path:?}"`），变体名是 `Bluetooth` ⇒ 拿 `Ble` 去真机上找会误判成"BLE 链路没建起来"。复跑两条现算：`grep -n "enum PathKind" -A 12 src-tauri/src/mesh/path.rs`（看变体名）与 `grep -n '"+conn peer="' src-tauri/src/network/transport.rs`（看那行怎么拼）。★ **对照一条我差点误改的**：Smoke-1 那条判据写 `diag/discovery_started: …`，在源码里 grep 整串是 **0 命中**，但它是对的 —— 那一行是 `format!("diag/{kind}: {detail}")` **运行时拼出来的**（`state.rs`），字面量本来就不会整串出现在码里。⇒ **逐格 grep 之前先分清"整串字面量"还是"拼出来的"**，否则会把"存在"筛成"不存在"；大文件不得走 BLE（16 MiB 那道闸今天仍在：`file.rs` 里 `refuse_reason_for_best_link`，常量注释写明取值理由 —— 阈值这条复算仍成立） | 未做 |
| Smoke-3 | 系统通知 + 点击跳转 | 后台收消息应出通知；点击后主窗口跳到该会话；关开关后**不得**出通知 | 未做 |
| Smoke-4 | 移动端群已读可见性（#30） | 群消息气泡下方应出现已读人数，点开浮层不超出屏幕右缘 | 用户已确认「已读在」；根因未定位 |
| Smoke-5 | 两台以上真设备联调（J1/J2/J4 全走一遍） | 用 `test-results/run-*/summary.html` 同样的断言清单手工走 | 未做 |
| Smoke-6 | 新旧安装包互发（INV-P24） | 旧→新、新→旧各发：文本/文件/未知 kind；期望「不支持的消息类型，升级后可查看」而非裸 JSON | 未做 |
| Smoke-7 | 移动端首屏与转屏（#27） | 冷启动权限弹框盖住 WebView 后，骨架结束**不得**出现桌面导航栏；转屏布局跟随 | 修复已提交 `48345bf`，**真机未证**。★ 2026-09-29 现读补一句，避免下一个人把这格当成「完全没判据」：**形状那一半已有机器判据**（`src/utils/designGuards.test.ts` 两条——「布局级断点必须叠加 `desktop:` 变体（移动端视口会说谎）」按表逐文件查那批结构级断点，外加「`desktop:` 变体必须在 tailwind 里注册」钉住选择器必须是 `html:not(.is-mobile)`，与 `useAppStore.applyIsMobile()` 写的那个类同名才对得上）。⇒ 这两条判的是「源码形状不再可能让手机当桌面用」，**不判真机首屏** ⇒ 本行等级仍不动，人工那一半照旧。 |
| Smoke-8 | macOS 沙盒书签 / Android 厂商后台限制 | 收到文件写进自选目录不报错；重启后仍可写（书签回读成功） | 未做 |
| Smoke-9 | **两台真实设备经中继互发**（LAN/Routed 两条直连都不可达） | 两端设置页填同一台中继服务器 + 口令（保存时那次真拨应显示通过）；拔掉直连可达性后发一条文本 + 一个文件，期望：两侧日志出现 `[relay] 电路已建立 peer=… server=…`，消息落库且 `msg_id` 只有一条，文件收完 sha256 等于源，通道图标显示「中继」而不是「局域网」，且**日志与事件里不得出现口令值** | 未做（单机做不出来：`relay.rs:104` 会跳过"已有任何链路"的对端，同机 LAN 一定连得上 ⇒ 需要真被封的直连，见 roadmap §12.7） |
| Smoke-10 | **同进程两扇窗之间的回送**（#97：自己发的群任务当场出现在主聊天窗） | 在群聊里点任务图标新建一条任务 ⇒ **不退出会话、不重进**，主聊天窗时间线底部应立刻出现那张任务卡片。失败长这样：卡片只在群任务看板那扇窗出现，主窗要切走再切回来才看到 | 未做。⚠️ 这一格**没有行为级判据**。现在锁住的是它上游的几段形状（都判不了"另一扇窗当场出现那张卡片"）：Rust 侧 `lib.rs::group_send_kernel_emits_to_own_windows_after_commit_outside_the_lock`（emit 必须晚于 commit 且在锁作用域之外）；`storeContract.test.ts` 的「返回 MessageRecord 的每条命令，调用点必须 enqueueMessage」（记录进没进 store，#82 就是漏了这一行）；`messageKinds.test.ts` 的「时间线可见性逐项判决表」与「过滤只有一份判据，且 todo 卡片真的在渲染链里」（进了 store 会不会被过滤掉、有没有人渲染它）。跨窗事件不在浏览器探针的能力范围内 ⇒ 最后一公里仍只能人眼看 |
| Smoke-11 | 真 WebView 里的键盘出口（#92，原生感走查 N1/N2） | 表情面板：点表情按钮后焦点应落在第一格，←→ 逐格、↑↓ 整行（八列一步），Enter/空格塞进输入框，Esc 关闭且**焦点还给表情按钮**。搜索弹窗：⌘K 或 ⌘F 打开 → 输关键词 → ↑↓ 移动高亮 → 回车进的必须是**高亮那一条**而不是第一条 | 浏览器内（Chromium 真按键，`vite dev` + CDP，未给仓库加任何测试依赖）**三处都实测到了**：表情面板＝打开焦点落第一格（214 格）/ ←→ 逐格 / ↑↓ 整行（COLS=8）/ 首格再按左键不越界不丢焦点 / Esc 关闭且焦点**还给触发按钮** / Enter 与空格各一次 emit 且 click 与 emit 一一对应；搜索弹窗＝↑↓ 移动 `aria-current`（1→2→1→0，越界钳在 0）、走到第 3 行按回车 ⇒ **emit 恰好一次且载荷就是高亮那条（msgId=m2，不是 m0）**、弹窗随之关闭。⚠️ 两条口径边界：① 这是**浏览器内**，WKWebView / WebView2 一律未证；② 反向对照也做过——探针第一次派发 Enter 时没带 `text`，结果 0 次激活（假红），说明这一格判据不是恒过。★ **2026-09-27 浏览器那一半从"一次人肉实测"换成仓内可重跑入口**：`scripts/check-ui-runtime.mjs`（`npm run test:ui-runtime`，已接进本地层 `npm run verify:e2e`）；**判据条数不抄在这里**，跑一次由探针自己打印。
      **同日晚加了第三段＝任务卡片本体**（#97 那一格由「渲染层零断言」换成运行时断言）：标题真在 DOM 里 / 载荷内部字段名（`todo_id`、`done_at`）一个都不许外露 / 传实时表 ⇒ 状态胶囊换档 / 同一句 @ 在「我」与「对端」两种视角下换标签 / 缩略图 `role=button`+`tabindex=0` 且按 Enter 真把**那份统一预览**打开（`open=true`、`source=task-card:<id>`）/ 底部入口 emit 的是**这条**的 todo_id。两把非空转同样是「改回旧坏样子」：摘掉卡片的 `clickable` ⇒ 恰好那两条红；把「查实时表」改回「只读快照」 ⇒ 恰好那一条红。⚠️ 修夹具时自己踩的坑也记在这里：把 props 对象提到渲染函数外面求值一次 ⇒ `open` 被冻住、表情面板整块不渲染 ⇒ **改夹具要跑全量三段，别只跑新写的那一段**。两把非空转是**把修好的地方改回旧坏样子**跑出来的 —— 组件里 COLS 8→7 ⇒ 恰好"↓ 走整行"与"↓ 之后 x 不变"两条红、其余 11 条绿；把"回车开高亮那条"改回"固定开第一条" ⇒ 恰好那条红，报错直接印出 msgId=m0 而高亮在 index 2。⚠️ 这一行的等级**没往上改**：它钉的是真 WebView，上面那些仍只到浏览器内 |
      **同日深夜加了第四段＝会话行那两枚徽标（#88 与 #117 的最后一环）**：store 说「这个会话有人@我」之后，
      界面到底得不得到 —— 三件事各自的条件都要判（未读那句 / 有人@我那句 / 与我相关的任务数那句都必须出现在
      **整行的 accessible name** 里；`role=button` 一带 `aria-label` 就把后代折叠掉，这是 2026-09-26 用系统控件树
      实测到的形状），外加两枚数字徽标各自可见、三态全清时名字恰好等于群名且零枚徽标（反向对照，
      防「永远都把三句拼进去」那种写法混过去）、以及单聊行不许出现「与我相关的任务数」。
      **边界**：这一段判的是「亮了以后看不看得见、念不念得到」，**不判**「该不该亮」（那是单元与跨进程那两层）；
      真 **WKWebView（macOS）**这一层从今天起有仓内本地入口 **`npm run verify:ax`**（`scripts/check-ui-runtime-ax.mjs`：
      自己起隔离实例、读到控件树挂上为止，判「有 AXWebArea / 节点数 ≥ 15 / 带可访问名字 ≥ 5 / 可操作控件 ≥ 5」，
      反证 = 指到一个没有 WebView 的进程必须四条全红）。
      ⚠️ 它**不判键盘出口** ⇒ Smoke-11 本行仍归人工；也**有意不进任何门禁层**（要辅助访问权限 + 人类 GUI 会话 +
      只有 macOS 这一条腿）⇒ 本行状态记作 **MANUAL-LOCAL**。**WebView2（Windows）仍完全人工。**
      ★ **2026-09-28 上面那句"不判该不该亮"在 macOS 这半边第一次有了直接读数**（入口：
      `GOSSLAN_AX=1 npm run test:e2e:group`，群聊轮里那条 @ 走心跳补递，界面无障碍树读到
      `E2E-Group，7 条未读，[有人@我]`）⇒ "该亮 ⇒ 真界面上亮"在真 WKWebView 上证到一次；
      **同一轮开机那一秒投的那条不亮** ⇒ 那格是"启动窗口错过那一次 emit"（红点只有一个写入点、无重算路径），
      要不要打开软件时补点亮＝语义决定，等他拍板。**它同样有意不进任何门禁层、不算本行等级变化。**
| Smoke-12 | **已经在用的老库升到 v11**（#122 第二段那条迁移的真机验证） | 拿一个**升级前就有真实聊天/群历史**的实例装新版本，第一次启动看四件事：① 能进主界面（迁移报错会让启动直接失败，不会静默）；② 历史条数一条不少（重点看群里那些老消息）；③ 老消息里那句 `@张三` 仍照常显示（老行那一列是空的 = 不知道 ⇒ 按昵称兜底，这是**设计如此**，不是坏了）；④ 新收发一条 @ 到重名的人，重启之后**仍只对被点名那个人亮**。⚠️ 什么算真故障：启动失败 / 历史少一条 / 老行的落点被读成「这条谁都没 @」（那说明 `NULL` 被当成空数组，是 INV-P24 的反面）。**为什么只能人工**：三层门禁的双实例轮**每轮都把用户现有库整个挪走**（`e2e-multi-instance.mjs` 起跑前 `renameSync(i.db, …)`，收尾再还原）⇒ 应用永远看到空目录 ⇒ 走 `is_fresh` 分支 ⇒ **整条迁移链一行都不执行**。今天为这一格存在的机器判据只有一条单元级的（`migration_tests::v10_to_v11_adds_mention_targets_column_without_touching_existing_rows`，手抄一张 v10 的表 + 真跑 `run_migrations`），**"真用户库 + 真数据 + 真启动"没有自动化覆盖** ⇒ 不许把"三层都绿"读成"升级路径已证"。 | **未做**（等用户本机走一遍；版本号 ≥ 4.31.2） |

## 这张表怎么维护（否则三周内又会漂）

1. **新写一条测试就改一行**：等级列只能向上走（MANUAL→SIMULATED→AUTOMATED），不许静默降级。
2. 等级为 `AUTOMATED` 的必须能填出**具体测试名或用例标签**；填不出来就写 SIMULATED。
3. §8 阶段 A 之后，Smoke 清单里的判据应逐步被 harness 接管，接管的判据整行搬到矩阵里。
4. 每次宣布「稳定版完成」前，此表必须**逐格有值**，且 `AUTOMATED` 格全部在 CI 真跑（当前只有第 14 行满足）。
