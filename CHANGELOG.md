# Changelog

本项目遵循[语义化版本 SemVer](https://semver.org/lang/zh-CN/)：

- **major**：破坏性变更 / 架构级重构（不向后兼容）
- **minor**：新增功能（向下兼容）
- **patch**：Bug 修复与细节优化

版本号统一由 `npm run version:patch|minor|major` 维护，一次改动同步 `package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/tauri.conf.json` 五处，并把本文件 `[Unreleased]` 小节落为带日期的版本小节。

## [Unreleased]

### Test（2026-09-26 · 故障注入第七格：连续多文件 + 两单同名 —— 「少一个文件」就是丢数据，所以这条必须在）
- 新增 `--fault=multi-file`（+ `npm run test:fault-injection:multi` / `:multi-selfproof`，已接进 `verify --group local`）：
  一次入队 3 个 transfer，其中**两单文件名完全相同、内容不同**（两份源放不同目录 —— offer 的 name 取自路径的 file_name）。
  钉的是 §七「连续多文件」×「磁盘已有同名文件」的交叉：串行投递里后一单被挤死、或两单落成同一个路径互相覆盖，
  用户看到的是**没有任何提示地少一张图**。
- 两条证据：正向 22 断言全绿（实测 3 单入队 → **3.5 s** 全部终态，落地 `photo-x.bin` + `photo-x (1).bin` + `note-x.bin`、
  三单各自 done、队列清空、两侧台账各一行、每单记的字节数各自等于自己那张源、无 `.part`）；
  反向只把其中一份的期望摘要换掉 ⇒ **恰好 1/22 报红**、报在「落地内容多重集合 == 源内容多重集合」那条上。
- ★ 判据为什么用**多重集合**而不是逐单比名字：落地名会被 `unique_path` 改，逐单比名字就退化成"对上三份里任意一份" ——
  那正是内容串味能躲过去的缝隙。
- ★ **边界写清楚，不假装 PASS**：这一格证明的是「同 peer 的 flush 串行 ⇒ 后一单 offer 时前一单已 rename，所以能让位」。
  **没覆盖**"两个 offer 都在任一次 rename 之前到达" —— 那种交错下两单会拿到**同一个 final_path**、后一次 rename 直接覆盖前一次
  （`file.rs:1599-1602` 的注释承认这个形状，但它只把 `.part` 改成按 transfer_id 命名，没解决 final 撞名）。
  要造它需要**三个实例**（两个发送者 → 一个接收者），当前 harness 的实例表写死两个 ⇒ 记为边界，不是记为已测。

### Test（2026-09-26 · 故障注入第六格：§七「错误 size」里唯一真实会发生的那一半，顺带照出 A-11）
- 新增 `--fault=src-shrunk`（+ `npm run test:fault-injection:shrink` / `:shrink-selfproof`，已接进 `verify --group local`）：
  先 `SIGSTOP` 冻住接收端 → **入队时把 `send_file` 那一刻该写的三行全写**（气泡 / 传输台账 / 队列）→
  把磁盘上的原件 `truncate` 到 4096 B → 解冻。§七 那张单子上的"错误 size"分两半：
  **线上谎报 size** 早就被 length+hash 判死了（再造一格没意义），**真正会发生的是排队期间原件被改**（用户在等对端上线时编辑了同一个路径）⇒ 这一格测的是后者。
- 两条证据：正向 22 断言 **2 连绿**（实测解冻 → 两侧 done 3.5 s、A 日志 `offer-sending ... size=4096`、
  落地字节与 sha 都等于**截断后**的源、队列行已关、接收目录只留终名那一份）；
  反向只换期望摘要 ⇒ **恰好 1/22 报红**、且正报在那条摘要断言上。
- ★ 这一格的窗口**不是撞运气撞来的**：A 只在收到入站帧时才读盘（= A-9 的实测结论），所以"先冻 → 入队 → 截断 → 解冻"必然让 A 读到新 size。
  **把 A-9 的缺口当机制用，不等于认可这个缺口** —— A-9 该修还是要修。
- ★ **照出新缺口 A-11（待拍板）**：真 size 由投递任务按磁盘重算，但 `upsert_transfer` 的 ON CONFLICT 只改 status/path/progress、
  `fill_message_sha256` 只回填 sha ⇒ 实测入队 1 MB / 实发 4096 B 时**发送侧气泡与台账仍写 1048576**，接收侧写 4096。
  用户可见后果：同一条 transfer 在两台设备上"多大"不等，且发送侧那一行的 sha256 已经是新那份 ⇒ 同一行自己都不自洽。
  本轮只用一行打印钉住分歧（不许写成断言，那要先改产品）；修的形状与"先红后绿的那条判据"都写在 roadmap 的 A-11 行里。
- ★ 顺手被自己的守卫抓一次：为了"没等到终态时不崩"我加了一条兜底 `check` ⇒ `check-doc-numbers` 现算 23 而运行时只跑 6 条，
  文档全被判红。**根因是那条兜底本来就是死代码**（`waitFor` 超时会 throw，走不到那里）⇒ 删掉它，数字自己回到 22。

### Test（2026-09-26 · 故障注入第五格：接收目录写不进去 —— 顺带把我自己写错的判据抓出来一次）
- 新增 `--fault=recv-readonly`（+ `npm run test:fault-injection:disk` / `:disk-selfproof`，已接进 `verify --group local`）：
  把接收端的**下载目录整体改成只读**再入队 1 MB。这一轮走的是"对端活着、照常发心跳"那条路，
  与冻结轮互为对照 ⇒ 它是 `file_outbox` 的 GiveUp **第一次被真进程跑到**。
- 两条证据：正向 22 断言全绿（实测入队 → **43.2 s** 后队列行落到 `failed` / `attempts=5`，
  A 日志 `reason=连续重试超限`，B 日志 3 次 `Permission denied (os error 13)`，接收目录零残留）；
  反向只把"该落到哪个终态"换成 `done` ⇒ **恰好 1/22 报红**、退出码 1。
- ★ 第一版判据把 `sending` 当成了终态 ⇒ 在第 4 次尝试的 33.2 s 处抓到 `{status:'sending',attempts:4}` 判红。
  `sending` 是"正在投递"的中间态 —— **红的是我的判据，不是产品**；已改，并把这个坑写进 harness 注释。
- ★ 一度怀疑"崩溃后卡在 `sending` 就永远没人捞"（队列查询只认 `pending`）⇒ 查码确认
  `reset_sending_to_pending` 在 AppState 初始化时正是为这件事存在的 ⇒ **不是缺口，没往路线图加账**。
  留这条是为了记一个反面习惯：**怀疑成立之前先把码查完**。
- 新发现登记 **A-10（待用户拍板，未动生产码）**：这一格命中的分支**接收侧不写库、不发事件**，
  只有 `FileReject` + 一条 error 日志 ⇒ 磁盘满 / 接收目录设在只读盘时，**接收方完全不知道发生过这件事**
  （发送方也要 43 s 后才看到失败）。补提示属于新增用户可见行为 ⇒ 当前阶段只记账不做。
- 验证：`npm run verify` 快速层全绿（步数现算，未增不减）；带轮次名的声明（「磁盘轮 N 断言」）
  已按现算值登记在 smoke 矩阵里，架构图卡片因为不再写任何总数所以无需跟改。

### Test（2026-09-26 · 故障注入第四格：对端失联用 SIGSTOP 冻住 —— 而它证伪了我自己的设计）
- 新增 `--fault=peer-freeze`（+ `npm run test:fault-injection:freeze` / `:freeze-selfproof`，并已接进 `verify --group local`）：
  `SIGSTOP` 冻住接收端 ⇒ 进程活着、内核照样收 SYN、发送侧链路仍是 "open"，唯一缺的是对端回执；
  冻结期内入队 1 MB，读**两侧** DB + 盘上文件，再 `SIGCONT`。
- 判据（正向绿 / 反向红，都是跑出来的）：失联期间「接收目录不许出现终名」「发送侧不许 done」「接收侧也不许 done」三条全绿；
  解冻后终名落地（实测 0.5 s 内）、内容 == 源、两侧 `done`、`outbox=0`、只留一份终名文件。
  `--fault=peer-freeze-lie`（只把判据的期望摘要换成全 0）⇒ **恰好 1 条报红**、退出码 1。
- ★ **这一格最有价值的产出是它打了自己的脸**：原设计要钉「write 成功 ≠ 已送达」，前提是"失联期间这一单正在被尝试"。
  实测（冻结 30 s 与 60 s 两种时长）发送侧 `attempts` **一次都没涨**、`file_transfers` 连行都没有 ⇒ 根本没有在飞的写，
  那条断言会是一句空转的同义反复。**已把判据改成它真能证明的东西**，并在 harness 注释里写死"没覆盖什么"。
- ★ 由此发现一条 A 类风险，登记为 roadmap **A-9**（**待用户拍板，未动生产码**）：
  文件 outbox 只有链路事件才会被带动重投，`next_attempt_at` 到点没有定时器去看 ⇒
  给"半死不活"（链路没断、也不报错）的对端发文件会静静挂着。用户可见，但修它要动网络核心重试时序，§十八要求先由人确认。
- 顺带修掉两处**文档漂移**（都是被自家守卫逮的）：架构图 E2E 卡片把"轮数 / 最大断言数"这类**总数型声明**删掉了
  （总数没有任何判据管，删标签等于绕过判据 C —— 这次就是删标签被判红），轮次声明统一落到会被现算对账的位置；
  smoke 矩阵里"杀进程轮 23 断言"原本没带轮次名 ⇒ 等于没被对账，已补名。
- 验证：`npm run verify` 快速层全绿（步数由 `--list` 现算，未增不减）；`check-doc-numbers` 现算
  「默认/脏前缀/续传/杀进程/冻结」五轮断言数并对齐门禁 local 层。

### Test（2026-09-26 · §15 统一入口：多实例 E2E 从「记得跑」变成门禁的一层）

- 事实：`grep e2e-multi-instance scripts/verify.mjs` 此前**零命中** —— package.json 里那一整排
  `npm run test:*` 脚本一直只活在脚本表里，没有任何门禁会跑它们（只有「某人记得跑」才算跑过）。
- 现在 `scripts/verify.mjs` 多了一个显式的 **`local` 归属组**（`npm run verify:e2e` = `--group local`），
  收四条正向轮次：默认轮、脏前缀注入、真前缀续传、接收中 SIGKILL；`verify:all` = 全量层 + 本地层。
  反向 / lie 模式**故意不进**这一层 —— 它们预期红，进门禁会把「能红」变成「常红」。
- **不假装 CI 覆盖了它**：归属列写 frontend/rust 会造出「CI 有这条 job」的假象，而 CI 现在真跑不动
  （要 release 产物 + 桌面 GUI 会话 + 百 MB 磁盘，Windows 腿还卡在 #47）⇒ 用 `local` 显式声明「CI 不跑」，
  且默认层与全量层都不收它 ⇒ **快速层 11 / 全量层 17 的计数一位没动**（这两个数有文档硬数字守卫
  对着 `--list` 现算对账）。不带 `--group` 的每次运行都会先打印一行「本地专项层本轮没跑 + 原因 + 怎么跑」，
  因为§十禁止把「没跑」暗示成「全绿」。
- 已实测：`--list` 11 步 / `--list --full-gate` 17 步不变、`--list --group local` 恰好四条；
  `npm run verify:e2e` 真跑 **4/4 全绿共 94.4 s**（含杀进程轮 `✅ 多实例 E2E 全绿（23 条断言）`）。
- **接进门禁之后新出现的静默面 = 「接进来又被删掉一步」**，所以给 `check-doc-numbers.mjs` 加了一条对账：
  harness 的每条正向注入轮必须被 `--group local` 逐条点名。正向轮清单**直接读 harness 的
  `const X = FAULT === "…"`**（不手抄第二份名单），三种改坏都要红 —— 少点一条 / 名字打错 /
  把预期红的 `-lie` 模式塞进门禁。已在 `verify-guards.py` 登记为非空转用例（✅ 改坏即 FAIL、恢复即 PASS）。
- 顺手修掉 roadmap 里那句「两条规则」—— 加到第四条它就错了，而且**没有任何守卫管得住这种总数**：
  改成"条数不在这里写"。同一条教训的第二次应用。


### Test（2026-09-26 · 故障注入第三格：接收中真 SIGKILL，100 MB 在飞窗口）

- `scripts/e2e-multi-instance.mjs` 新增 `--fault=kill-mid` / `--fault=kill-mid-lie`
  （`npm run test:fault-injection:kill` / `:kill-selfproof`）：**零生产码改动**，
  在 100 MB 单文件传到一半时把接收端 `SIGKILL`，重启后验它必须按盘上真实字节续完。
  **23 条断言 1 连绿**；lie 模式恰好 **2/23** 报红（续发点、摘要各一条）⇒ 判据非空转。
- 实测链条：`.part` 涨到 **786432 字节**（768 KiB）时杀 ⇒ 终名文件不存在、发送侧不是 `done`
  ⇒ 重启后发送侧日志「接收端已有 786432 字节」⇒ 拼完 sha256 与源一致、两侧 `done`、`outbox=0`、无 `.part` 残留。
  §七「接收过程中杀进程」/ §八「对端进程退出」由此从 SIMULATED 转 **AUTOMATED**。
- 顺带量到 §七尺寸阶梯的第一格：**100 MB 单文件在回环上 ~0.78 s 传完（≈130 MB/s）**；
  默认轮在 `E2E_FILE_MB=100` 下 16 条断言同样全绿。harness 新增 `E2E_FILE_MB` / `E2E_KILL_MB` 两个尺寸旋钮（默认不变）。
- harness 自己的两个坑（都长得像"产品坏了"，实为判据问题）：① **注入时机必须属于判据自己** ——
  前两版在停机时预置，传输早在"等链路"那步结束，连红两轮；`waitFor` 的 500 ms 粒度也抓不住 0.78 s 窗口，改成 50 ms 自旋。
  ② 被信号杀死的子进程 `exitCode === null`，判"死透"要看 `signalCode`。
- `check-doc-numbers.mjs` 判据 C 改成**按 harness 里的模式块自动归堆**：加一条注入必须同时在 `MODE_LABEL`
  登记轮次名，否则守卫当场红（"新模式没登记" = 文档永远不会要求它 = 这一轮断言数没人对账）；
  现算四档 16 / 20 / 21 / **23**。
- 文档同步：roadmap L5 / §7 J3 / §9 A-2 / §11.7 注入③行 / harness bug 列表（五 → 七）、
  Smoke 第 7 行、架构图覆盖卡（3/21 → **4/23**，内嵌 script `node --check` 过）。
- 仍未做：断链、重复/乱序帧、旧 attempt 帧、错 size、DB 锁竞争；harness 仍**只在本地**，未进 CI（卡在 #47 Windows 基线）。


### Test (2026-09-26 · 第二条故障注入（真前缀必须续传）+ E2E 断言数改成机器现算)

§八的注入层从 1 格变 2 格，而且这一轮把"文档里的断言数"从手抄变成了现算对账。

- `--fault=resume-prefix`（`npm run test:fault-injection:resume`）：B 侧预置**源文件自己的前 64 KiB**
  当 `<tid>.part` + A 侧一条待发 1 MB。判据 5 条 / **21 断言 1 连绿**，A 日志实证
  「接收端已有 65536 字节，从断点续发」⇒ **对有效前缀走的是续传，不是从 0 重灌**，
  拼出来的文件 sha256 与源一致、两侧 `done`、`outbox=0`、接收目录只剩 1 个终名文件。
  这一格钉的正是 `decide_offer` 注释里那次 160 MB 真机事故（每轮重灌 ⇒ 界面恒 0% ⇒ 判"分片失败"）。
- `--fault=resume-prefix-lie`（`npm run test:fault-injection:resume-selfproof`）：注入不变，
  只把两个输入换成错值（期望已收字节数减半 + 期望摘要换成 64 个 0）⇒ **恰好 2/21 报红**，
  红字带着 `预期 32768 / 实际 65536` 与 `实际 877f591f4162…` ⇒ 这两条也不是同义反复。
- 顺手把「旅程数 / 断言数」这类数字接进 `check-doc-numbers.mjs` 判据 C：
  现在默认轮 **16** / 脏前缀轮 **20** / 续传轮 **21** 三个数由脚本从 harness 现算归堆得出
  （只数 `check("字符串名"…` 这种真断言调用点，`function check(` 与失败记录器不算），
  活文档里凡声明「默认轮/脏前缀轮/续传轮 N 断言」必须等于现算值，且**三种标注一个都不许消失**
  （否则"删掉标签"就是绕过这条守卫的最短路径）。
  两个方向都做了变异自测：给 harness 加一条真断言 ⇒ 6 处报红；把文档里的 21 改成 24 ⇒ 1 处报红；还原后绿。


### Test (2026-09-26 · 故障注入第一格，而且判据自己被反证过一次)

§八要的「故障注入」层从 A-2 的 16 项里落地第一格，仍然**零生产码改动**。

- `--fault=poison-part`（`npm run test:fault-injection`）：B 侧先落一个 4096 字节随机内容的
  `<transfer_id>.part`，同时给 A 预置一条 pending 的 1 MB 文件 outbox ⇒ 起实例后走的正是
  「接收端已有前缀 ⇒ 要求从此续发」这条真实恢复路径。**20 条断言 / 2 连绿。**
- A 侧日志给出的完整因果链（本轮最有价值的产出：它把"契约"变成了"观察"）：第 1 次 attempt
  `COMPLETED ok=false`（脏前缀被播种进 hasher ⇒ 整体 SHA-256 必不符）→ 10 s 后 outbox 重试 →
  前缀被丢弃、整份重收 → `ok=true`；终名文件字节数与 sha256 都等于源、两侧 `done`、`outbox=0`、
  无 `.part` 残留 ⇒ **对"被污染的前缀"，产品的处置是"拒收坏内容 + 重试补齐"，不是停在中间态。**
- 我第一版判据写的是「这种注入之后两侧都不许 `done`」⇒ 跑出来 `✗ 3/20`。**红的是判据，不是产品**：
  写的是"我以为失败长什么样"。已改写成"只允许两种结局、不许交叉"的互斥蕴含
  （补齐 ⇒ 双侧 done + outbox 已清；没补齐 ⇒ 双侧非 done 且没有假终名 + 不留脏前缀）。
- `--fault=poison-part-lie`（`npm run test:fault-injection:selfproof`）：注入完全不变，只把判据比对用的
  期望摘要换成 64 个 `0` ⇒ **恰好 2/20 按设计报红**（红字里带着真实的 `实际 81972590575b…` 与
  `B=done A=done outbox=0`），另外两条按蕴含正确保持绿 ⇒ 这四条读的是**真文件字节与真 DB 行**，
  不是同义反复；而且证伪它**不需要改一行生产码、不需要重编译**。
- 顺手修掉一个**假红**：「二进制不得比源码新」的守卫原先跟 HEAD 比，于是**纯文档提交**也会把 E2E
  挡在门外 ⇒ 改成跟「最后一次动过 `src-tauri/src` 的提交」比，内容判据不变、噪声消失。
- ⚠️ 边界：断链、杀进程、重复帧、乱序、旧 attempt 到达、DB 锁竞争**仍未做**；并且实测排除了一条
  看起来可行的注入 —— 改 A 侧库里存的 sha256 **上不了线**（发送侧的摘要在发送时从磁盘重算）。


### Test (2026-09-26 · 文档硬数字漂移改成机器门禁 B-3)

§十三要求「代码改变 → 检测契约漂移 → 要求更新文档 → verify 重新通过」，而这条一夜之间漂了**四处**。
最刺眼的是同一个「取锁点条数」在仓库里同时写作 **292 / 295 / 296**：`verify.mjs` 的 `why:` 写 292、
`stability-roadmap.md` 写 296，而同一步在同一次运行里实际打印 **284 guard 绑定 + 11 语句临时量 = 295**。
⇒ 病根不是"哪个数字错了"，是**有唯一事实源的数字被人手抄了一份**。

- 新增 `scripts/check-doc-numbers.mjs`，接进 `npm run verify` 快速层（**这是加进步骤表的第一次真做**）。两条规则：
  ① 文档里凡声明「全量/快速 N 步」，N 必须等于 `verify.mjs --list` **现算**的条数；
  ② 取锁点条数在契约面文件（README / 两份验收文档 / `protocol-invariants.md` / 契约图 / `verify.mjs` /
     `check-lock-scope.mjs`）**一律禁止手写**。
- **先跑红再修**：脚本写完第一次运行就报 **8 处**（含上面那两处真漂移），逐处删掉手写数字后转绿。
  反向自证：往 `check-lock-scope.mjs` 注一行 `500 处取锁点` ⇒ 退出 1 并点名该文件；删掉后 `cksum` 与注入前一致。
- 顺带清掉的手写数字：`verify.mjs` 的 `why:`、`check-lock-scope.mjs` 头注释两处、
  `protocol-invariants.md` 一处、`ARCHITECTURE-MAP.html` 契约表一处（改完 `node --check` 内嵌 script 通过、
  `mapContract.test.ts` 3/3 通过）。**带日期的历史陈述（CHANGELOG 旧版本小节、审计复盘文档）故意不改**，
  那是事实记录。
- **这条改动自己就是最好的证明**：给步骤表加了一步之后，快速层从 10 步变 11 步、全量从 16 步变 17 步，
  **没有任何一处文档需要跟着改** —— 守卫现算出新期望值。这就是"能算的就不许写"要的形状。
- ⚠️ 已知边界：`CHANGELOG.md` 与 `docs/stability-roadmap.md` 是**漂移事故台账**，故意不扫（它们必须能原样写
  "当初漂成什么样"）。所以本守卫只保证**契约面没有第二个事实源**，不等于全仓库再无手写数字。

### Test (2026-09-25 · 第一次有机器证明的「两个真实 Gosslan 互发消息」)

稳定版总指令§四把「真实用户操作级 E2E」定成最重要一项，审计实测结论是**这一层先前为 0**：
606 前端 / 690 Rust / 185 护栏全部在**同一进程内**，CI 从不启动应用。本轮补上第一格。

- `scripts/e2e-multi-instance.mjs` + `npm run test:e2e:multi-instance`：**零生产码改动**
  （不加控制套接字、不加命令、不开新 feature）。用现有多开能力（独立 DB/日志/设备身份/TCP 端口）
  起两个真实实例，「用户点了发送」用**停机预置 SQLite** 表达，投递与回收用双方 DB + 日志 + 磁盘核对。
  之所以不需要在测试里实现密码学：`reseal_for_send` 见 `enc1:` 前缀就会拿 `messages` 行的明文重新封袋
  —— 这正是 enqueue-before-deliver 不变量的红利：**入队即事实源**。
- J1 的 8 条断言：B 侧恰好一条 / 内容正确 / 方向正确 / A 侧 outbox 被 Ack 清空 / 状态前进过 sending /
  `msg_id` 唯一（INV-P01）/ **两端重启后仍只有一条** / 重启不复活已 Ack 的 outbox 行（不二次投递）。
- **自带反向自证** `--negative`：两端照常起、链路照建，只把收件人换成幽灵 id ⇒ 投递断言必须报红。
  红只能来自「没送到」，不来自「B 没启动」这种基础设施噪声。
- **J2 文件 A→B 再接 8 条断言**（同一对实例、同一轮里跑）：只有 rename 后的 `<transfer_id>.bin` 才算落地 /
  字节数 1 MB / **sha256 与源文件一致**（INV-P17 分片可验证）/ A 侧 `file_outbox` 收尾删除 /
  A 侧终态 `done` 且 `progress 1.0` / B 侧终态 `done` / 同一条传输只记一次 / 接收目录无 `.part`、无改名副本残留。
  **钉 `done` 而不是 `sent` 是这一格最有价值的部分**：`sent` 只表示「我把字节写进了 socket」，
  `done` 必须等对端 `FileCompleteAck` —— 这正是总指令「不要因 TCP write 成功就认为已送达」的机器形状，
  任何实现只要"自己写完就罢"就必须红。
- 实跑：J1 阶段正向**连 5 轮全绿**（8 断言）；接上 J2 + 重启注入后 **16 断言 / 4 连绿**，
  最新一轮 `/usr/bin/time` 墙钟 **12.8 s**（报告里的 `duration_s` 是步骤计时合计 ≈8 s，两者量纲不同，别混着抄）。
  反向 1 次报红。报告落
  `test-results/run-<ts>/{summary.json,summary.html,instance-*.app.log,sqlite-*/,recv/,backup-*/}`；
  用户原有 `gosslan-1/2.db` 每轮自动备份并还原。⚠️ 只覆盖 macOS + LAN 路径的 J1/J2 两格，**尚未进 CI**；
  文件侧只测了「1 MB / 单文件 / 链路不断」这一格，尺寸阶梯/并发/断链/杀进程全部未做。
- 过程中抓到 **4 个 harness 自己的 bug**（现场都长得像「产品坏了」，其中三个是假绿/假红级别）：
  ① 断言读错文件、拿错 id（表现为「链路超时 90s」和 `127.0.0.1:undefined`）⇒ 报红文案从此必须说清
     「读的是哪个文件、找的是哪个 id」；
  ② 我给自己写的「二进制不得比源码新文件更旧」守卫第一次运行就拦下一个 9-20 的产物，但它用 **mtime** 当判据，
     被护栏注入器"写回内容却不变"打穿、把 E2E 挡在门外 ⇒ 改成**内容判据**（`git status` 干净且二进制晚于 HEAD 才放行）；
  ③ **假绿**：终局判定只看「有没有步骤抛异常」，而 `check(false)` 不抛异常 ⇒ 一条报红的断言会被写成 `PASS`。
     改成**终局由断言台账推导**，且这条修法当场被验证有效：上面那条我写错的 `sent`/`done` 契约
     跑出的是 `✗ 1/16 条断言报红`，不是假绿；
  ④ **存在性断言 = 半个断言**：J2 第一版钉的是「A 侧存在一行 `file_transfers`」，而那行是网络层自己建的、
     传输根本没发生它也在 ⇒ 改钉 `status` / `progress` 的具体值。

### Docs (2026-09-25 · 把审计发现的漂移数字与契约图对齐)

- 契约图实测更正：IPC **133 → 129**（5 处）、`DB_VERSION` **8 → 9**、护栏 **181 → 185**，
  并新增一格「跨进程双实例 E2E = 1 旅程 / 8 断言（仅 J1，本地）」——同日接上 J2 后已改为
  **2 旅程 / 16 断言**（覆盖列随本轮 Test 小节同步）。
  「138 → 133」那行历史陈述保留但补了「此后到 129，等值由 `mapContract.test.ts` 每次实算核对」。
- ⚠️ 顺手更正我**自己上一轮写进 roadmap 的一条结论**：原写「同机 LAN 发现天然失效」，
  实跑证明两台同机实例**能**靠 announce 建链（`+conn … path=Lan`）；`SO_REUSEPORT` 的分担
  只在 ≥3 实例 / presence 学习场景才致命。`routed_endpoints` 因此从「唯一通路」降级为「确定性兜底」。
  `29 个事件` 那条我先核了才没改 —— 那张卡片自己写明「表内合并成 28 行」，不是漂移。
- **每次开工都被 AI 读一遍的那份验收文档也漂了**：`docs/acceptance/1.0-release.md` 手写「全量门禁 15 步」
  并抄了一遍步骤清单，实测**是 16 步、清单漏了「db 锁作用域守卫」**。（命令本身没写错：`--full` 是
  `--full-gate` 的合法别名，`verify.mjs:125/144` —— 先把这点证实了才动手，否则会是又一次未证断言。）
  修法是**把手写清单删掉**、改成「步骤名与条数看脚本自己的收尾输出」：§十三要的是不再维护第二套事实，
  而不是把第二套抄对一遍。文档数字的机器门禁仍记在 B-3（未做）。
- 新增 `docs/acceptance/stability-smoke-matrix.md`（关掉审计缺项 M-5）：22 条 P0 逐条标
  **AUTOMATED / SIMULATED / MANUAL-HARDWARE** + 8 条只能真机的 Smoke，每条都要写出**证据名字**。
  规矩：标 AUTOMATED 必须点名一条真存在的测试，等级只准往上走；没跑绿的旅程不算覆盖
  —— 所以 J2 现在写的是「未跑绿 = 不算」，跑绿了才允许升上去。


### Docs (2026-09-25 · 稳定版第一阶段：只做审计，产出工作地图)

总指令把当前阶段定为「先完整审计，不要马上大改代码」。本轮**没有改任何生产代码**，
产出 `docs/stability-roadmap.md` —— 它是后续所有稳定版工作的地图，也是判「现在处在哪一格」的唯一入口。

审计的三条主结论（都在文件里带实测出处）：

1. **架构这一层不是风险来源，证明的形状才是。** 现有 606 条前端用例 / 690 条 Rust 用例 /
   185 条护栏非空转用例密度不低，但**跨进程 E2E 是 0**：CI 从不启动应用，三个 example 手工且不在门禁里。
   「用户点一下，对面真的出现且只有一条」这件事今天没有任何机器证明。
2. **26 条不变量里只有 6 条有具名验证钩子**（P11/P22/P23/P24/P25/P26）。总指令§三那份
   「物理定律」清单里，大多数定律目前没有锁 —— 这是「未来 AI 改坏而 CI 不响」的直接入口。
3. **文档硬数字大面积漂**：契约图写 `DB_VERSION = 8`（真 9）、README 写 138 条 IPC（真 129）、
   `schema.sql` 只有 14 张表（真源 19）而**没有任何判据读它**。⇒ 根因是「只有 CMDS 第一列被机器核对」，
   修法是把数字改成实算，而不是再抄一遍。

本轮顺手收掉的最小一批（纯文档，无代码）：README 的 138→129 与 INV 区间、索引的 138→129 与 INV 区间、
护栏条数不再写死；新文档在 README 表格/目录树与 `AI_ENGINEERING_INDEX` 阅读顺序三处登记，索引编号重排。

`scripts/verify-guards.py` 里 #27 那一族的 4 条护栏（`--only mobile-layout`）**已跑完非空转验证**：
四条全部「改坏即 FAIL、恢复即 PASS」，注入残留为 0（逐个 `grep` 过），护栏总数 181 → **185**。
这一步不能省：未验证的守卫比没有守卫更危险 —— 它让人以为那条判据真的在被守着。


### Fixed (2026-09-25 · 我自己把 Windows 基线写错了，并且给守卫补上这一格)

上一批我给 Windows 基线**手工**补两条用例名时，按记忆写成 `db::tests::a_done_transfer…`，
真路径是 `db::cascade_tests::…`。CI 立刻按预期报「基线里的 2 条用例没有跑（静默跳过）」——
那个形状和我今天刚修的"陈旧条目"长得一模一样，所以如果没有今天新加的跨平台核对，
我会再花一轮去查代码。

- 修正两条模块前缀（函数名部分是对的，所以昨天那条 `checkNamesStillExist` **查不出**它）。
- 给同一道守卫补上缺的那一格：**同一个函数名在不同平台基线里必须带同一个模块路径**。
  这条在任何平台都算得出来（不需要 Windows），报错文案直接说"手工补基线请整条复制
  `--update` 打出的名字，模块前缀不能凭记忆写"。
- 判据非空转：把 `db::cascade_tests::a_done_transfer…` 改回 `db::tests::…` ⇒ 守卫立刻红并点名；
  还原后 `cmp` 级核对内容一致。
- ⚠️ 顺带记一笔**流程错误**（比 bug 更值得记）：`48345bf` 在 CI 上被 Change Budget 判 L3
  而我只写了 `[plan]` —— L3 要 `[plan] [impact]` 双标记。本地当时看到的是
  "零覆盖"，因为**推完之后范围必为空**。⇒ 以后 push 之前必须跑
  `node scripts/check-change-budget.mjs --range origin/main..HEAD` 拿真判定，
  不能拿"零覆盖"当通过（这条早已写在记忆里，今天是我没做）。
- 另一次 CI 红是 `48345bf` 自己：它同时是"移动端首屏"那一刀，14 文件 / 192 行 ⇒ L3。
  已推的提交不改历史，下一次 push 的范围不再包含它 ⇒ 那一轮 Run 会一直显示红，
  我在这里如实标出来，不把它算成"CI 绿"。



## [4.29.45] - 2026-09-25

### Fixed (2026-09-25 · #27 移动端首屏被判成桌面布局：两处各一半)

工单原话是「权限弹框后骨架屏结束，样式崩掉（注明不是移动布局本身的问题）」。
先把机制钉死，再改 —— 两条独立成因，缺一条都还会看见：

**成因 A：`isMobile` 要等 IPC。** `useAppStore` 里它是 `ref(false)`，唯一赋值点
`applyIsMobile()` 排在 `await api.getSettings()` **之后**。而撤骨架的两个条件
（`App.vue` 在 `init()` 之后的 `finally` 派发 `gosslan:app-ready` + `boot.ts` 那条 5s 硬定时器）
**与 init 有没有跑完无关** ⇒ `getSettings()`/两次 `listen` 任一 reject，`isMobile` 就永久停在
`false`，手机上骨架一撤露出的是**桌面三栏**。Android 的运行时权限弹框
（`MainActivity.kt` 在 `decorView.post` 里申请）恰好就是最容易让首屏 IPC 失败/延后的那一刻。
- `isMobile` 改成**建 store 时就判一次**（只依赖 UA 与 `matchMedia`，不需要等任何 IPC）；
  `init()` 里那段整体挪到**第一个 await 之前**，负责再确认 + 挂 change/resize + 写 `<html>` 类。
- 判据：`storeContract` 钉住「`applyIsMobile()` 必须排在 `init()` 任何 `await` 之前」
  + 「必须写 `html.is-mobile`」。判据形状沿用今天那条教训 —— 钉**第一个 await 的位置**，
  不钉某个具体调用名，换调用名绕不过去。先写后红（红在"必须排在 await 之前"）。

**成因 B：CSS 侧仍然只看视口宽度。** 2026-09-21 那次只把 **JS** 判据改成平台优先
（`resolveMobileLayout`），可 Tailwind 的 `md:`/`sm:` 还是纯宽度 —— 手机上报到兜底 980px 时
**JS 说移动、CSS 说桌面**：导航栏 `md:flex` 回来了、`md:pb-0` 把底部安全区内边距清零
（输入框被 TabBar 压住）、抽屉 `w-full` 变成 980px 宽。
- `tailwind.config.js` 新增一个 `desktop:` 变体（`html:not(.is-mobile) &`），
  结构级断点一律写成 **叠加**形式 `desktop:md:flex` / `desktop:sm:flex-none`：
  既要"不是移动布局"也要"够宽" ⇒ **桌面窄窗口的行为与改造前一字不差**，只砍掉
  "手机上被兜底视口点亮"那一档。改了 10 处（导航栏、三栏外壳、底部留白、收藏两栏、
  搜索对话框、诊断面板），纯文字密度类（`sm:inline` 的秒级时间戳）**刻意不改** ——
  宽视口下多显示一点无害，全钉进去会让人下次不敢用断点。
- `is-mobile` 这个类**只有 `applyIsMobile()` 一个写者**（CSS 只读），不留第二份判据。
- 判据两条（`designGuards.test.ts`）：结构级断点清单里每一项必须带 `desktop:` 前缀；
  变体必须在 tailwind 里注册（拼错的后缀 Tailwind 静默不生成规则，等于没改）。
  非空转证据：把 `NavRail.vue` 的 `desktop:md:flex` 改回 `md:flex` ⇒ 守卫立刻点名该文件红。
- 编译产物核对（不是"看着对"）：`npm run build` 后的 CSS 里
  `html:not(.is-mobile) .desktop\:sm\:flex{display:flex}` 共 16 处。
- ⚠️ 真机待验：手机首启（权限弹框前/后）与转屏，看导航栏是否出现、输入框是否被 TabBar 压住。
  还有一处**本次没动**：`skeleton.css` 的 `@media (max-width:767px)` 只影响骨架自身观感，
  且骨架在 `is-mobile` 写入之前就把真实布局盖住了。

### Changelog
- `src/stores/useAppStore.ts`：`isMobile` 建立时即判定 + `applyIsMobile` 移到 await 之前 + 写 `html.is-mobile`。
- `tailwind.config.js`：新增 `desktop` 变体。
- 10 处结构级断点叠加 `desktop:`；`designGuards.test.ts` +2 条判据、`storeContract.test.ts` +1 条。



## [4.29.44] - 2026-09-25

### Test (2026-09-25 · #25 那笔欠账：emit 抑制第一次有了行为测试)

`fail_file_job` 的"已收完的文件不许被报成失败"这条闸门，此前只有**源码结构守卫**
（`lib.rs` 钉 `finalize_file_failure` 体内的写序锚点）—— 结构守卫只能保证"这些步骤还在、
还在原来的顺序"，保证不了"返回值仍然是那个意思"。而这个返回值是
**要不要 emit `file-failed` 的唯一依据**（A3 纪律），回归时编译器不响、别的测试也不响，
只有界面上会出现「一个打开就在那儿的文件显示失败」。

- P7 把三份收尾合成一份之后，这块逻辑已经天然不吃 `AppState` ⇒ 不需要再拆签名，
  直接打在 `db::finalize_file_failure` 上（两条，一正一反）：
  · `a_done_transfer_is_not_announced_failed_yet_its_queue_row_closes` ——
    `done` 行必须回报 `false`、气泡与台账一个字都不许动，**但队列行必须关掉**
    （不关就是活锁：它每 tick 被 `list_expired_file_outbox` 重扫一遍又什么都不做）；
  · `an_unfinished_transfer_is_announced_failed_and_all_three_rows_move` ——
    没收成的必须回报 `true` 且三处一起推进。只钉正向会变成"永远返回 false 也能通过"，
    而那正好是"关掉 emit、前端永久卡在 X%"的形状。
- 非空转证据（两条各自改坏必须红，跑完已还原并核对文件一致）：
  把闸门 `if !already_done || cancelled` 改成 `if true` ⇒ 第 684 行断言炸；
  把关行条件 `if changed || already_done` 改成 `if changed` ⇒ 第 698 行断言炸。
- 两份平台基线都补上这两条（mac 688 → 690；win 同步 +2），
  清单守卫跨平台名字核对一并通过。

### Changelog
- `src-tauri/src/db/cascade_tests.rs`：+2 行为测试 + 4 个夹具/读回助手。


## [4.29.43] - 2026-09-25

### Fixed (2026-09-25 · 发版脚本自己把 Cargo.lock 跟上，并且断言跟上了)

顺手查出来的第二个"两份事实源"：`scripts/version.mjs` 同步 5 个版本文件，
但 **Cargo.lock 那一步只写了一句注释**——"由 cargo 构建时自动同步，发版前记得手工跑一次
`cargo check`"。于是 4.29.41 与 4.29.42 两次发版提交里，lock 都停在旧版本号，
而 lock 与 Cargo.toml 不一致正是 `--locked` 构建会直接拒绝的那类不一致
（CI 现在没用 `--locked` 才侥幸没红）。

- 现在由脚本自己跑 `cargo update --offline -p <包名>`，然后**读回 lock 里那个包的
  version 断言等于新版本号**：对不上就 `exit 1` 并说明手工修法。
  能自动化的事不留注释给人记 —— 留了注释也一样会漏，这两次就是证据。
- 包名从 `Cargo.toml` 里读，不写死 `gosslan`（写死就是第三份事实源）。

⚠️ 写这条判据时自己踩到的一课先记下：第一版把 `spawnSync` 的 import 漏了，
`node --check` **不报**（它是未定义标识符，不是语法错误）—— 只有真的跑一次才发现。
所以下面"验证"那行必须是一次真实执行，不是"看代码觉得对"。

### Changelog
- `scripts/version.mjs`：新增 Cargo.lock 同步 + 版本一致性断言。


## [4.29.42] - 2026-09-25

### Fixed (2026-09-25 · v4.29.41 的两处收尾：移动端漏删的桩 + Windows 基线为什么会烂)

4.29.41 推上去之后 CI 的 Verify 仍然红，逐项查下来是两个各自独立的原因，
都不是功能代码坏了：

1. **`close_log_window` 的 `#[cfg(mobile)]` 桩没删** —— macOS 上 `cargo clippy -D warnings`
   与全部单测都干净（那个分支在本平台根本不编译），只有
   `check-mobile.sh --bluetooth`（`cargo check --target aarch64-linux-android`，0 warning 铁律）
   报 `function is never used`。⇒ 教训：**删带 cfg 分支的东西必须一次删完所有分支**，
   而这一层只有移动端编译门禁看得见 —— 这条腿不是冗余，是唯一能看见它的那双眼睛。
   顺带把三处因删除而开始说谎的注释改对（`chat_search.rs` 的职责边界与"与
   `search_messages` 的区别"那段、`transport.rs` 引用 `commands::send_file` 的那句 ——
   它现在已是 `send_file_auto` 的私有实现）。
2. **Windows 那条腿的红是"基线烂了"，不是"测试静默跳过"**。清单守卫的致命条件是
   「基线里有名字、本平台没跑」，而它按平台分文件 ⇒ mac 侧我跑 `--update` 会自动跟上，
   Windows 侧留着一堆**源码里已经不存在**的名字。这两种红在界面上长得一模一样，
   上一次为此来回跑了两轮 CI。

- 根治办法不是"我去手工同步一次"，而是**把这件事变成任何平台都能查出来的判据**：
  `check-test-manifest.mjs` 新增 `checkNamesStillExist()` —— 扫全仓 Rust 源码收集
  所有 `fn <名字>`，然后**把所有平台的基线全查一遍**：名字对应的函数已经不存在
  ⇒ 一定是陈旧条目（删它没有争议），报出"哪个文件的哪一行"。
  平台门控的用例函数还在，所以不会被它误伤 —— 这正是"漏跑"与"换平台"的分界线。
- 一个真实的假警报当场被抓住并修掉：`network::transport::tests::gossip_group不受好友检查影响`
  是**中文函数名**，`[A-Za-z_]\w*` 会把它漏成陈旧条目 ⇒ 标识符类放宽到"非空白非括号"。
  （这是这条判据自己的形状判据没写够，跟本仓反复踩的那一族同源。）
- Windows 基线清掉 9 条已经不存在的名字（3 条 `file_relay` 旧名 + 6 条
  `search_messages_*`），483 → 485 条（含本批补的 11 条跨平台新用例）。
  ⚠️ **该文件仍落后约 200 条**（历史上只在 mac 上跑 `--update` 攒下的债，就是 #47）：
  那 200 条是"会跑但没纳入保护" = 警告级，不拦门禁。要真正清掉必须在 Windows 上
  跑一次 `--update` —— 本机做不到，等下一次有 Windows 环境时收；
  但**"致命的那一类"从今天起在任何平台都能被拦住**。
- `npm run verify` 快速层 10 步绿；清单守卫新增一行
  `✓ 基线名字全部仍存在于源码（跨平台核对 2 份基线）`。

## [4.29.41] - 2026-09-25

### Removed (2026-09-25 · 把 IPC 对账做成三段，顺手清掉门面之下那 5 条)

0-A3 那批收了「注册表 ↔ 门面」这一层，但**门面之下**还留着 5 条谁都不调的包装
（当时记在契约图的漂移行里、标着"等拍板"）。这次先加判据再删，删完三段的第二段自己就红了：

- 新增第三段对账（`src/utils/mapContract.test.ts`）：门面包装必须在 `src/api/` 之外
  有引用点。**先按行匹配产出了 3 个假阳性**（`api` 换行 `.openImagePreview(...)`
  这类跨行调用点）⇒ 匹配前把空白压平，之后红名单精确等于那 5 条。
  ⚠️ 已知边界（写在判据注释里）：这一层判"有没有被引用"，不判"UI 是否可达"——
  只被另一条死方法引用的仍算活。
- 删的是**入口**，不是实现：
  · `send_file` 命令注册删掉后，编译器立刻指出它的唯一调用者是 `send_file_auto`
    ⇒ 降级成模块内私有函数。**这条是本批最有价值的一刻**：它证明了"第二条入口"
    从来不是白占几十行，而是一份会跟着主路径漂移的平行实现。
  · `send_file_relay` 同删 —— 中继发文件的能力由自动路径提供（无直连时借一跳，
    `transport.rs` 那条分支直接调 `file::send_file_via_relay`），界面里从来没有第二个按钮。
  · `search_messages`（界面用的是 `searchChatHistory`）连它的**私有查询**
    `db::search_messages_in_conv` 与两份 `SearchResult`（Rust + TS）一起删 ——
    `search_messages` 那 6 条测试里唯一有价值的覆盖是 LIKE 通配符转义，
    而活的 `search_history` 用的是同一份 `escape_like` + `ESCAPE '\'` **却没有一条转义测试**
    ⇒ 断言搬过去打在活查询上（`search_history_treats_like_wildcards_as_literals`），
    而不是连覆盖一起删。
  · `get_interface_candidates`（`DiscoveryDiag` 里已经带 candidates）、
    `close_log_window`（窗口用 `window_close`）同删。
- 顺带发现并留档：`store` 里的 `sendFileRelayTo` 也是零调用（第三段判据按"有没有被引用"
  会把它算活，所以一并删掉了方法与其导出）。
- 契约图同步：IPC 表少 5 行、SEM 描述少 5 条、那条漂移行改写成"已收口 + 三段的边界"。
- Rust 测试 693 → 688（−6 条死查询测试 +1 条搬到活查询上 −... 净 -5）；基线已同步。
- clippy `-D warnings` 通过（删完命令后 `SearchResult` / `search_messages_in_conv`
  会变成 dead_code，正是靠它俩把"牵连到的私有件"找全的）。
- ⚠️ **本机 clippy 全绿，是 Android 那条腿把它抓住的**：`close_log_window` 有两个
  cfg 分支（desktop 真实现 + `#[cfg(mobile)]` 桩），我只删了第一个 ⇒ macOS 上
  `cargo clippy` 与全部单测都干净，而 `check-mobile.sh --bluetooth` 报
  `function close_log_window is never used`（0 warning 是铁律）。
  ⇒ 删带 cfg 分支的东西必须**把所有分支一次删完**，并且这一层只有移动端口径看得见。
  顺带把三处已经说谎的注释改掉（`chat_search.rs` 的职责边界与对比说明、
  `transport.rs` 引用 `commands::send_file` 的那句 —— 它现在是私有实现）。
- **修 Windows CI 那条腿上的清单守卫（#47 的一半）**：`fc8fb19` 的 Verify 在
  `Rust 单测 / 清单（windows-latest）` 上红，逐项排下来与代码无关 ——
  `cargo fmt` ✅、`cargo clippy -D warnings` ✅、`cargo test` **686 passed / 0 failed** ✅，
  只有第 4 步清单守卫报「基线里的 3 条用例没有跑」。那 3 条正是本批 P4 改写
  `file_relay.rs` 测试时**删掉的旧名字**（`reassemble_out_of_order` /
  `rejects_out_of_range_chunks` / `sweep_stale_reassemblies_keeps_active_and_drops_expired`）。
  macOS 侧我跑过 `--update` 所以看不出来，而 `test-baseline.windows.txt` 是**另一份文件**：
  守卫按平台分文件，改测试名必须两边都同步，否则"红的是 Windows 那条腿、原因在基线"。
  ⇒ 从 Windows 基线里摘掉那 3 条不存在于任何平台的名字，并补上本批新增的 11 条
  （7 条 `file_relay` + 4 条 `latest_page`，都不带 cfg 门控、四个平台都会跑）。
  ⚠️ 该文件仍落后约 200 条（历史上只在 mac 上跑 `--update` 攒下的债，就是 #47），
  那些是"新增未纳入保护"= 警告级，不拦门禁 —— 真正需要的是在 Windows 上跑一次
  `--update`，本机做不到，等下一次有 Windows 环境时收。

### Perf (2026-09-25 · 第 7 步 2：附近设备列表不再每 333ms 把整屏作废)

复审第 7 步第 2 条写的是"三处 O(n) 微改"。**先量再改，结果两条被推翻、一条换了病因**：

- `nicknameOf` 的两次线性扫**不是问题**：一屏 24 行 × 300 台设备逐帧全查 =
  **0.066 ms/帧**（16.7ms 预算的 0.4%）。为它建一张索引表属于"为不可能的场景加防御"。
- `groupReaderIds` 每行返回新数组确实是引用问题，但输入只有群成员数量级（通常 ≤ 几十）
  且只在群聊气泡上调用，拿不到可复现的代价证据 ⇒ 不动，等有掉帧事实再说。
- **真凶是 `peers.value = list`**：`peers-updated` 最多每秒 3 次，三条写入点
  （`refreshPeers` / `searchNearbyPeers` / `onPeers`）每次都是**整表直写** ——
  换掉数组也换掉里面**每一个对象**，于是任何读过 `peers` 的渲染全部失效
  （消息行模板里的 `nicknameOf` 就读它）。效果是"每 333ms 把整屏重画一遍"，
  与这一拍到底有没有东西变了**完全无关**。

改法是把"要不要写"变成一个可单测的纯函数：

- 新增 `utils/peerMerge.ts::mergePeerList(现值, 新一拍)`：**内容一字未改 ⇒ 返回 `null`**
  （调用方一次赋值都不做）；有变化 ⇒ 逐台按 `device_id` 比对，
  **没变的那台沿用旧对象**，只有真变的那台是新引用。字段比对**不手写清单**，
  取两边 own keys 的并集 ⇒ `Peer` 以后加字段自动进比对（手写清单漏一个字段的后果是
  "值变了但界面不更新"，比多比对难得多）。
- 顺序变化刻意**算**变化：它就是"附近设备"列表的呈现顺序，压掉它是真画错。
- 三条写入点全部改走它，并加一条形状守卫：`peers.value = ` 的写入点必须恰好 3 处、
  且每处右值必须是 `merged` ⇒ 有人退回整表直写就红。
- 判据：`peerMerge.test.ts` 5 条行为测试（先写后红，红的是"模块不存在"）+ 上述形状守卫 1 条。
  非空转证据：把 `searchNearbyPeers` 改回 `peers.value = list` ⇒ 守卫立刻变红。
- 前端测试 596 → 602。契约图统计同步；复审文档第 7 步补执行进度表与两条实测修正。

### Fixed (2026-09-25 · 第 2 步 P4 收口：经中继收文件不再整份驻内存)

八个步骤里最后一条"会杀掉程序"的账。与对端没有直连时，文件走"借一跳邻居"
（`RelayFileOffer` + `RelayChunk`）。**接收侧原先把整个文件留在内存里**：
`Reassembly { chunks: HashMap<u32, Vec<u8>> }` 收一片存一片，收齐后再
`extend_from_slice` 组装出**第二份**完整字节 ⇒ 峰值 ≈ **2× 文件大小**，
而当时的尺寸闸门只有一句 `size > i64::MAX`，等于没有。600MB 的文件就是 1.2GB 内存
—— 移动端必被系统杀掉（用户看到的是"传到一半 App 没了"），桌面端整台机器发僵。

直连路径早有流式纪律（边收边写 `.part`），**同一个关注点的第二条实现忘了带上第一条的纪律**。

**为什么不能就地照抄直连**：`.part` 的偏移靠 `seq × chunk_size`，而 `RelayFileOffer`
里没有 `chunk_size` 这个字段。也不能从 `(size, total_chunks)` 反推 ——
`total_chunks = ceil(size / chunk_size)` 是**不可逆**的：size=65537、chunk_size=65536
⇒ total=2 ⇒ 反推出 32769，偏移全部错位，写完的文件哈希必然对不上。
所以这次给 offer 补上那个数（`#[serde(default)] chunk_size: u32`）。

- **接收侧改成边收边写盘**：开档时把 `{id}.relay.part` 预分配到声明尺寸
  （`set_len`，稀疏文件不真占空间），每片解密后按 `seq × chunk_size` `seek + write_all`
  落进去，内存里只留"收到过哪些 seq"。收齐 → `sync_all` → **流式**算 SHA-256
  （`sha256_file_hex`，边读边算）→ 改名落盘。峰值 = 一个分片 + 一个读缓冲。
  `save_received_bytes(&[u8])` 换成 `move_received_file(&Path)`：旧的签名本身就要求
  "把整份文件读回内存再写一遍"，那是同一个 OOM 的另一半。
- **形状自相矛盾 ⇒ 当场判死并删 `.part`**，四条：seq 越界、非末片长度不等于声明的
  分片尺寸、这一片会写到声明尺寸之外（磁盘写爆的入口）、写盘本身失败（多半是满）。
  每条都带一句真实原因发给界面，不静默消失。
- **老发送方（不发 `chunk_size`）明确拒收**并说明"对端版本过旧"，**不**退回内存重组 ——
  留着那条路就是留着那个 OOM，而且老对端照样能把我方撑爆。方向是单向的：
  serde 会忽略不认识的字段 ⇒ 老接收方收新 offer 不受影响，只有"新接收 ← 老发送"要升级。
  （用户 2026-09-25 明确批准"4 个版本以前不必兼容"。）
- 顺手补掉三个同族的洞：
  ① `transfer_id` 在中继路径上**从来没过文件名消毒**（直连那两条都过了）。它现在会
     变成文件名 ⇒ 复用同一份 `safe_transfer_id`，否则 `../escape` 就是对端一句话把文件
     写到下载目录之外。
  ② 中继会话状态被 TTL 回收后，旧代码**跳过哈希校验照样落盘**（`if let Some(rs) = rs`
     没有 else）⇒ 现在判失败：没有可信依据就不许宣称这份文件可用。
  ③ 中继接收期间界面一直停在 0%（旧注释写着"此处省略"）⇒ 现在有进度，**每秒最多一条**
     （BLE 上 4 KiB 一片，几百 KB 就是上万片，逐片 emit 会把"正在收文件"变成"界面卡顿"）。
- 判据：`file_relay.rs` 新增 7 条行为测试（流式落盘并给回路径 / 重复分片不双计 /
  四条形状判据各自判死且删掉 `.part` / 没声明 chunk_size 就拒收 / 非法 transfer_id
  连文件都不许创建 / 回收过期重组**连文件一起删** / 0 字节文件正常完成），
  先写后红（4 条编译期缺失错误，逐条对着新 API）。既有的 1.8 回归测试
  `relay_receiver_hash_lifecycle_success`（乱序 + 重复投递）保留同一语义，
  只把"组装出的字节"换成"落盘的文件"。
- lib.rs 那条源码守卫 `relay_receive_hashes_assembled_plaintext_once` 随实现一起变了 ——
  它原先钉的是字面量 `Sha256::digest(&full)`，这次**红得正确**，判据升级为钉语义：
  校验点必须是对文件整体流式算的 `sha256_file_hex(`、`Reassembly` 结构体里不得再出现
  `Vec<u8>`、`add_chunk` 必须真的 `seek + write_all`、开档必须 `set_len`。
- ⚙️ **`check-lock-scope` 当场抓到我自己新写的一处锁内 emit**（`dbc` 存活期内
  `emit("file-failed")`）⇒ 已用 `{ }` 圈住。留在这里是为了说明这套判据真的在拦事。
- Rust 测试 689 → 693（+7 新 −3 合并改写）。真机待验：两台只有公网中继的设备互传
  >100MB 文件（含蓝牙邻居在场的情况）、以及老版本发给新版本应当看到明确拒收提示。

### Test (2026-09-25 · 第 7 步 1 收尾：两条腐烂的护栏锚点 + 契约图第一次可对账)

改完上一刀做了次**全量静态锚点核对**（把 `verify-guards.py` 当模块 import，遍历 182 条用例
逐条数注入锚点命中次数，用的是脚本自带的 `include!` 递归解析器）。结果 2 条已经腐烂：

1. **「降级拒绝必须早于任何写操作」的锚点命中 0 次**。成因就是 #46 那一刀：它把"数表"
   那一段插进了 `}` 与 `conn.execute_batch(SCHEMA)?;` 之间，而锚点钉的正是一整块三行。
   后果不是报错而是**这条红线静默消失** —— 而它守的是"弹窗里那句『你的数据没有被修改』
   是不是谎话"。判据改成只钉"降级判定那两行本身"，与后面排着什么无关；
   重钉后已证明非空转（改坏 → `downgrade_refusal_writes_nothing` 变红 → 恢复变绿）。
2. **「resend_message 的群聊判废必须在置 sending 之前」整条用例已无的放矢**：它盯的函数
   连同它的守卫测试都在 0-A3 那批"零引用命令"里删掉了。留在清单里只会每次跑出一个
   假失败把真失败淹掉 ⇒ 删除。**用例 182 → 181，锚点 191 → 190。**

这两条都是 `rust` 标签，而全量门禁那一步只扫打了 `frontend` 标签的子集 ⇒
**日常与 CI 都不会碰到它们**，腐烂可以无限期存留。这正是"锚点腐烂不会让任何测试变红，
只会让人以为某条红线在被守着"的现场。

- 新增 `src/utils/mapContract.test.ts`：契约图里那两份手抄清单第一次**双向对账** ——
  ① 前端 `invoke("x")` 的每个名字必须在 `generate_handler!` 注册过（拼错的名字
  编译/类型/构建全过，只在运行时变成"点了没反应"）；② 注册了必须上图、上图了必须注册，
  两侧都不许重复。今天实测两侧 134 条逐字一致。
- 非空转证据（三条变异各自必须红）：api 层多调一条不存在的命令 → 红；
  从图上删一行已注册的命令 → 红；图上重复登记一行 → 红。
- 顺手修一个统计漂移：图里"前端测试条数"写 593，实跑 **596**（上一批加的用例没同步）。
- `npm test` 596 全绿。护栏脚本改动只影响 `--full-gate` 的逐条改坏层，未跑全量 181 条
  （那两条改动过的已单独逐条证明）。

### Perf (2026-09-25 · 第 7 步 1：打开一个冷会话只打一次 IPC)

复审第 7 步（边界与前端）里唯一有真实用户感知的一条。切到一个**没缓存**的会话时，
消息区先空白、过半秒才涌出来 —— 那一半不是查库慢，是**问了两回**：

```
总数 = await get_message_count(conv)      ← 只为算 offset
list = await get_messages(conv, 100, 总数 - 100)
```

两轮都是 IPC，而且每轮都要排队过全局那把 `Mutex<Connection>`（好友列表、心跳、文件进度、
所有会话的读写共用一把锁）。第二条本来就是第一条查完之后**可能已经不是同一份库状态**的
那个快照。翻页路径不受影响（那里"总数"是判"还有没有更早历史"的依据，不是顺手拿来算 offset）。

- 新增后端一条命令 `get_latest_messages(conv_id, limit)`：一条 SQL 按
  `seq DESC, id DESC LIMIT n` 取尾部再倒回正序。冷加载从两轮压成一轮。
  `get_message_count` 与 `get_messages` 都还有活消费者（翻页），一条都没删。
- **等价性是被逐行钉住的，不是"看起来一样"**：`latest_page_is_identical_to_the_count_plus_offset_two_step`
  把 250 条的会话按旧写法算一遍 offset、与新写法各取一次，逐行比对 msg_id 序列；
  另一条 `..._breaks_seq_ties_by_id_...` 专门钉"同一会话内 `seq` 会撞号 ⇒ 必须用 `id` 打破平局"
  （`seq` 是 Lamport 时钟，双方同时发送时同一会话里可以出现两个相同序号，
  只用 `seq` 排序会让平局那几条在两轮之间换位 = 消息顺序画反）。
- 顺带修掉一处**已经存在的假绿守卫**：`storeContract` 那条"作废点必须在 await 之前"
  原先钉的是字面量 `await api.getMessageCount`。这条命令一换，那个字面量就从源码里消失了，
  守卫会**永远绿灯**，而它守的是"早退路径漏清『已翻到顶』⇒ 该会话再也翻不动历史"。
  判据改成钉**第一个 `await`** 的位置 —— 与调用名无关，改名换命令都不再能绕过它。
- 新增一条前端守卫：冷加载函数体内消息页 IPC 调用点**恰好一处**，且必须是
  `getLatestMessages`（退回 `getMessages` 就意味着要把 count 那一轮带回来）。
- 契约图同步：`docs/ARCHITECTURE-MAP.html` 接口表 +1 条、`get_message_count` 那行改口径
  （冷加载不再问它）；`useChatStore` 里"命中缓存 = 零 IPC"那句注释是**错的**
  （切会话无论冷热都会重查一页，命中缓存只是"立刻有东西可画"），一并改成实话。
- 测试基线：Rust 685 → 689（+4 条 db 行为测试），前端用例数不变（新增的是既有文件里的断言）。
- 真实用户可见的变化：切到冷会话少一次往返排队。**真机验证待用户**：切进一个几百条历史的
  会话，看底部是不是最新消息、往上翻还能不能翻到最早一条、以及带未读的会话分割线位置没变。

### Fix (2026-09-25 · 第 5 步 3：未读的过渡竞态 —— 旧快照不得点亮已清零的红点)

复审这条写的是"未读收成单一来源（后端给值）"。**核过之后前提不成立**：后端 `mark_read` 就是
`UPDATE conversations SET unread = 0`，`totalUnread` 也只由 `conversations[].unread` 求和 ⇒
真相源本来就只有一个。真正在漏的是**过渡**，而且是我上一轮改动放大出来的：

`t=400` 发起 `getConversations()` → `t=500` 用户打开会话、本地乐观清零（用户 2026-09-12
定的"所有异步操作尽量乐观更新"）→ `t=600` 那份带着 `unread = 3` 的旧快照落地 ⇒
**红点自己亮回来、列表顺序跟着回退**（就是代码里早就写下的那句「我没点它怎么又红了」）。
`StaleGuard` 挡不住它 —— 它只回答"我是不是最新的一次"，而这里请求就是最新那次，**数据是旧的**。
上一轮加的"回到前台重拉会话"把这条从偶发变成了每次切回前台都可能撞。

- 新增纯函数 `applyConversationSnapshot(现值, 快照, 已清零水位, 快照发起时刻)`：
  **只豁免 `unread` 一个字段**，`last_msg` / `last_ts` / `pinned` / 名称头像一律按快照走。
  方向也刻意只做一侧 —— 快照**晚于**清零时必须照抄后端，否则就是把真未读永久压掉（严重得多）。
- 本地清零收敛成唯一入口 `clearUnreadLocally(convId)`：改内存与打水位必须同时发生，
  原先散在 4 处（打开会话 + 三处 `markRead().then()`）的各写一遍全部改掉。
  顺带修掉一处闭包错误：回到前台那处的回调在 `await` 之后才读 `activeConv.value`，
  期间用户若已切走，清的是**新**会话的红点而根本没给它发已读 ⇒ 现在先把 id 取进局部变量。
- 水位由 `pruneUnreadClears` 按 30s 窗口回收，避免这个 Map 跟着会话数无界增长。
- 判据：`channelState.test.ts` 钉住「`.unread = 0` 非注释行恰好一处 + 必须住在
  `clearUnreadLocally` 里 + 快照必须经新函数落地 + 发起时刻必须取自**发起前**」；
  行为测试 3 条在 `messages.test.ts`（豁免方向两边都钉）。前端测试 589 → 593。
- ⚠️ **不做**的两件：`utils/messages.ts:328` 的 `conv.unread += msgs.length` 仍是前端本地计数
  （改成后端给值要么每条新消息多一次 IPC、要么改事件载荷 —— 与刚判定不做的第 2 条同源）；
  以及第 5 步第 2 条本身（那些 handler 已经是"事件当触发器、回头读库"的正确形状）。

### Fix (2026-09-25 · 第 5 步 1/4：常驻窗口的自愈兜底 —— 判据从点名改成"现场读事实")

**症状**：手机/桌面切回来界面还是旧的，要点一下才更新。根因是这两扇**常驻窗口**
（关窗 = 隐藏 ⇒ 文档永不重新加载）里，取数只发生在挂载那一次：

- 主窗口：`visibilitychange` 只调 `reportActivity`（蓝牙节奏）与补发已读，**不重拉任何表**；
- 群任务窗口：重拉只挂在 Rust 复用它时发的 `group-todos-target` 上 ⇒ 从任务栏 / ⌘Tab /
  托盘唤回来没人发这个事件，列表停在离开那一刻（别人完成了任务、新加了任务都看不见）。

**兜底**：主窗口重新可见时重拉「会话 + 传输」两张表（它们正是 `message-acked` /
`message-failed` / `peer-read` / `file-*` 这些**只带 id、就地改内存**事件的真相源，错过一条
就永久错）；群任务窗口获得焦点时复用 `applyTarget`（**不另写一份取数**，免得又长出第二个家）。

**判据这条才是本轮的重点**：原先"常驻窗口必须重拉"只被一条**点名 settings** 的守卫钉着，
而它的前提早就塌了 —— `AUX_WINDOWS_RESIDENT = false`（设置/日志关闭即销毁），
被钉那扇窗**根本不再常驻**，真正的两扇常驻窗口一个都没被覆盖。现在换成：
从 `commands/logs.rs` 现场读 `AUX_*_RESIDENT`，强制每个标记在判据表里登记，
自取数的那几扇必须有"重新可见/焦点 ⇒ 真的重拉"，被推送的那几扇必须有推送监听。
⇒ 翻常驻标记、新增一扇窗口不表态，都会直接红。这正是 P7 那一课（点名式=半个守卫）的复用。

**顺带清掉三处说谎的说明**（文档与代码相反是本仓点过名的漂移族）：

- `entries/settings.ts` 与那条旧守卫的标题都写着"设置窗口是常驻的" ⇒ 已改成实话
  （焦点重拉留着，但它不再声称自己承担常驻兜底的责任）；
- `commands/logs.rs` 的预览窗口注释写着"见 `install_hide_on_close` 里的 `aux-hidden` 事件"，
  而**全仓不存在 `aux-hidden`**（那个回调只做 `prevent_close + hide`）⇒ 改成写实：
  释放动作在前端监听关闭请求里做。

⚠️ 边界：这轮只动了 review 的第 1、4 两条。事件补可应用载荷（第 2 条）与未读单一来源
（第 3 条）还没做；#30「移动端群已读不见了」需要真机取证，本轮做的是**让它在重新可见时
自愈**，不等于根因已定位。前端测试 588 → 589。

### Fix (2026-09-25 · 第 4 步 P7 收口：文件终态三份收尾合并成一个出口 + 删一个零调用函数)

**P7 第 1/2/3 刀**（done 不可降级 / 取消不记失败 / 写序）落完之后，剩下的正是那句
"三份实现未合并"。这次合并了，另外删掉一个没人调的函数。

`file_outbox` 的终态落库此前有**三份各写一遍**的实现：清扫器 `finalize_expired_file`、
发送放弃 `fail_file_job`、用户取消 `cancel_file_transfer`。三处漂移全部实测到：

- 写序（关行必须最后）只在两处成立 —— 第 3 刀已修；
- `done` 闸门只装在两扇门上，**清扫器那扇没有**：本次给它补上（判据
  `expired_file_never_rewrites_a_completed_transfer` 第一次跑就是红的）；
- 清扫器还多写一句 `gfile-{transfer_id}`。⚠️ **诚实交代：我上一轮把它说成
  "一个收件人超时 ⇒ 整条群消息显示失败、用户于是重发出重复文件"，那个后果不成立** ——
  生产代码里唯一写 `file_outbox` 的地方 `group_id` 恒为 `None`（群文件的逐人台账在
  `group_file_recipients`），那句永远命中 0 行；而且 `set_message_status` 本身拒绝把
  `delivered`/`read` 改回失败，只有还在途中的气泡会被改写。所以它是**潜伏**缺陷而不是
  活跃故障。仍按第 4 刀删掉，理由是「靠暂时没人这么写才不出事」的代码正是本仓一直在
  出事的那类（`file_outbox` 的 schema 里就有 `group_id` 这一列）。

合并成一个出口 `db::finalize_file_failure(conn, transfer_id, end)`，`end` 三档
（`Expired` / `GiveUp` / `Cancelled`）。口径差别收敛成函数里一处 `if cancelled`：
**取消**是用户动作且 1:1 与群共用这个入口 ⇒ 写 `file-` 也写 `gfile-`；
**超时/放弃**只代表某一个收件人没收到 ⇒ 不碰群气泡。两个方向各有判据
（`expired_file_does_not_touch_the_group_bubble` / `a_cancelled_group_file_marks_its_own_bubble`），
另配一条变异用例（把 `if cancelled` 写成 `if true` ⇒ 反向判据必须红）。

⚠️ 一个刻意的取舍：台账那笔写走 `upsert_transfer`，**不**走"回报改了几行"的助手 ——
后者在台账行不存在时报 false，会让"要不要关行"的条件永不成立 ⇒ 清扫器每 tick 空转。
"少一次 emit"不值得换来一个活锁；而"不许把 done 降级"由 `upsert_transfer` 自己守。
顺带删掉因此变成零调用点的 `mark_queued_transfer_failed`（连同它的测试）——
留着就是第二个家，正是本步一直在消灭的东西。

① 另一个删掉的死实现：`content::store::mark_complete`（**全仓零调用点**，实测只有它
自己的定义那一行）。它守着的判断也顺手被证伪：复审写的是"发送侧没接上标记完成"，
其实发送侧一直在记完成 —— 走的是 `record_local`（实测 `Direction::Send` 3 处、
`Direction::Receive` 3 处）。所以这不是"漏接线"，是一个长得像正主的岔路。

判据：3 条新测试（1 条红→绿、1 条反向、1 条边界）、2 条旧变异用例随合并**搬过家**
（锚点腐烂是这一步的固定副产物，跑 `--only` 逐条重证才算数）、新增 1 条 gfile 口径用例。
护栏 180 → **182**，Rust 基线 683 → **685**。第 4 步 P7 至此收口。

## [4.29.40] - 2026-09-25

### Fix (2026-09-25 · 第 4 步 P7 第三刀：写序 —— 把行踢出重试集合的那一步必须排最后)

审计 A3 当年给 `finalize_expired_file` 立过这条规矩并配了逐函数守卫：
**先做完面向用户的写，最后才让那一行从重试集合里消失**。但"落终态"这件事当时**有三份实现** ——
`fail_file_job`（发送放弃）与 `cancel_file_transfer`（用户取消）各自又抄了一遍同形状的收尾，
而点名式守卫看不见没被点名的那两个 ⇒ 被明令禁止的顺序在这两处一直成立。

为什么这不是洁癖：`list_expired_file_outbox` / flush 只选 `pending` / `sending`，
所以 `mark_file_outbox_failed`（或 `..._cancelled`）一跑，**这一行就再也扫不到**。
它排在最前面时，后面任何一步失败（写锁、磁盘满、消息行已被删）都没有人来补 ——
症状不是"报错了"，而是**那条气泡永久停在「发送中」，且系统已经忘记它存在**。

- `fail_file_job`：改成"分支只决定要不要做面向用户的写，**破坏性写在分支之后统一做一次**"
  （顺带把重复的两处 `mark_file_outbox_failed` 收成一处；已 `done` 那一支仍然关掉队列行，
  否则每 tick 被重扫一遍又什么都不做）。emit 依旧由"台账真的改了"门控。
- `cancel_file_transfer`：两笔气泡写 + 台账写在前，`mark_file_outbox_cancelled` 在后。
- **判据从点名升级成自动扫**：新增 `lib.rs::every_finalize_path_defers_the_destructive_write`
  —— 把 transport / commands / db 三份聚合源切成顶层条目，凡调用了
  "踢出重试集合"那一类写（`mark_file_outbox_{failed,cancelled}` / `delete_file_outbox` /
  `delete_{group_,}outbox_by_msg_id`）的函数，都要求它排在所有"面向用户的写"
  （`set_message_status` / `upsert_transfer` / 两个 transfer 助手）之后。
  **新增第三个收尾函数会被自动扫到，不必记得登记** —— 这正是点名式做不到的那一点。
  首次跑就报出那两处（不是我猜的，是它自己找出来的）。
- 两个刻意的保守方向写进了判据注释：注释里出现带左括号的函数名也算命中（宁可多报，
  漏报没人看得见）；分两支各调一次破坏性写会被判红。
- 不变量并入 **INV-P26**（§26 未新增节号，避免"每加一条就挪一次必测矩阵"的无意义churn）。

⚠️ **没做**的：这三份收尾**仍是三份**（本次只对齐了顺序，没合并实现）。合并的正确形状是
`db::finalize_file_failure(dbc, transfer_id, kind) -> bool` 一个函数 owning 全部四笔写，
但它要同时决定"取消算不算 emitted"「群气泡 `gfile-` 该不该被单个收件人的失败改写」——
后者**看起来像缺口、其实是有意不写**（群文件 N 个收件人共用一条气泡，一个失败不该改全局态），
需要先确认语义再动。记在待办，不在本轮顺手改。护栏 180 → 181，Rust 基线 683 → 684。

### Fix (2026-09-25 · 第 4 步 P7 第二刀：用户取消不再记成失败)

`cancel_file_transfer` 自己的注释写着「用户主动停止用 cancelled，自动失败用 failed」，
而它紧接着调的是 `db::mark_file_outbox_failed` ⇒ `file_outbox.status` 落的是 `'failed'`。
**注释与代码相反**是本仓点过名的漂移族（它会引导下一个人「照代码改注释」而不是改代码）。

要紧的程度说清楚：三条队列查询（`list_pending_file_outbox` / `list_expired_file_outbox` /
`reset_sending_to_pending`）只认 `pending` / `sending` ⇒ 写 `failed` 与写 `cancelled`
在**功能上完全等价**，坏的是台账 —— 下一个排查"这一单为什么失败"的人（或 AI）读到的
是用户自己按下的取消按钮。

- 新增 `db::mark_file_outbox_cancelled`，取消路径改用它；`mark_queued_transfer_failed`
  （自动判死）那边**仍然写 failed** —— 两个口径不许合并。
- `file_outbox.status` 的注释补上 `cancelled`。
- 先钉**前置证据**再引入状态：`cancelled_file_outbox_rows_are_never_requeued` 直接插四种状态，
  断言三条队列查询各自只认什么 —— 它今天就能跑（`cancelled` 还没人写），
  用途是证明"写进去 = 永久出局"，不是凭直觉新加状态。
- 判据 `a_user_cancel_is_not_recorded_as_a_failure`（源码级，双向断言：不许出现
  `mark_file_outbox_failed(`、必须出现 `mark_file_outbox_cancelled(`）+ 一条变异用例
  （把那句调用换回去 ⇒ 编译照过、队列行为一模一样、只有守卫红）。护栏 179 → 180。
- 顺带吃到上一刀的收益：`upsert_transfer(..., "cancelled", ...)` 现在**不会**再把一个
  已经 `done` 的传输行改成"已取消"（INV-P26 的闸门覆盖这条路径）。

### Fix (2026-09-25 · 第 4 步 P7 第一刀：`file_transfers` 的终态契约 —— `done` 不可降级)

`file_transfers.status` 今天有 **39 个写入点**（实测：`active` 12 处、`failed` 12 处、`done` 6 处、
`pending` 3 处，另有 `sent` / `cancelled`），而没有任何一张"谁能写什么"的表。于是任何一处
**晚到一步** —— 清扫器、重复帧、上一轮 attempt 还堵在链路队列里的残留 —— 都会把"已收到"
改成"失败"、把进度条从 100% 打回 0%，而那个文件此刻正躺在下载目录里能打开。
这正是复审 §P7 说的"两端状态互相矛盾"里最刺眼的一种。

- `upsert_transfer` 的 `DO UPDATE` 加一句 `WHERE file_transfers.status <> 'done'`
  —— 状态、进度、路径三个字段是同一条 DO UPDATE，闸门加在语句上就一起生效。
- `commands/files.rs::fail_file_job` 里那句**绕过助手**的裸
  `UPDATE file_transfers SET status='failed' WHERE id=?1` 收进 `db::mark_queued_transfer_failed`
  （同一个契约，合法集合是"非 done"，与只认 active 的 `mark_transfer_failed_if_active` **不是一回事**：
  队列判死的起点是 `pending`）。并且已经 done 时**连 `file-failed` 都不 emit** ——
  为一个收好的文件弹"传输失败"是纯粹的谎话。队列行 `file_outbox` 照常判死，不留活口。
- 新增源码守卫 `terminal_status_writes_have_one_home`：除 `db/file_transfer.rs` 之外
  再出现直接 `UPDATE file_transfers SET status` 就红。理由与"闸门本身"同等重要 ——
  **同一件事有两个家时，改一个忘一个是常态**（本仓 §9 那族平行实现反复就是这个形状）。

⚠️ **复审原本建议的写法不能照抄**：它写的是
`WHERE status NOT IN ('done','failed','cancelled')`。但 `retry_incomplete_content` 复用
**同一个 `transfer_id`** 发 `ContentRequest`（`transport.rs:2644`）⇒ 把 `failed` 一起钉死就是
"一判死永远停在失败，而字节其实还在流" —— 症状恰好是本次要修的那个的**反面**。
所以不可降级的集合只有 `done`（唯一有磁盘证据的状态），并且专门留了一条**反向判据**
`a_failed_row_can_be_reactivated_by_the_next_attempt`：`failed` / `cancelled` / `pending`
必须还能改回 `active`。它今天就是绿的，它存在的意义是让下一次"顺手扩大集合"变红。

不变量本文 = **INV-P26**（新 §26，原「必测矩阵」顺延 §27）。判据四条 + 两条变异用例
（摘掉闸门 → 正向红；照复审那样扩大集合 → 反向红），护栏用例 177 → 179，Rust 基线 677 → 681。
⚠️ `fail_file_job` 的 emit 抑制没有行为级测试（那个函数吃 `&AppState`，本仓造不出来）——
它只有 SQL 层的判据与读码保证，真机验证归用户。

### Fix (2026-09-25 · #46：`is_fresh` 恒为假 —— 全新库不再重放整条迁移链)

`init()` 里 `is_fresh = current == 0 && pre_table_count == 0`，可那句**数表**发生在
`execute_batch(SCHEMA)` **之后** —— 而 SCHEMA 会建出全部 19 张表 ⇒ `pre_table_count` 永远是 19
⇒ `is_fresh` 恒为假 ⇒ 全新库走 else 分支，把 v1→v9 **整条迁移链重放一遍**。后果分两个量级：

- 看得见的：首次启动多打 9 行假的「`[gosslan-db] running v1→v2: friends 加公钥列…`」日志、
  v7 两句「孤儿清理跳过一条语句：no such column: id」告警（老库才有的形状，新库当然没有）。
  用户第一次打开、翻日志排查别的问题时，这 11 行是**纯粹的误导**。
- 看不见的：v5→v6 先建 `idx_outbox_msg_id`、v8→v9 再删掉它 —— 白做一轮写放大。

**为什么以前"没坏"**：迁移全部幂等（`column_exists` / `IF NOT EXISTS` 守着），所以形状是对的。
这也正是这个退化能活这么久的原因：**它在形状上完全看不出来**，`user_version` 两种走法都停在
`DB_VERSION`，没有任何一条断言会因为重放而红。

⚠️ **不能只把那句数表往上挪一行** —— 挪上去之前先量化了"新库今天靠重放拿到什么"：
`migration_tests::fresh_schema_alone_has_exactly_the_migrated_shape` 逐条比对
「SCHEMA + 一个共用出口」与「SCHEMA + 整条迁移链」的 `sqlite_master` 全集。
它第一次跑就是红的，差集恰好一项：**`index idx_messages_conv_seq`** —— 这条索引只写在迁移 v2→v3 里
（`seq` 是迁移才加到 `messages` 上的列，放进 SCHEMA 会让老库 `no such column: seq` 直接开不起来，
0-B 那天真撞过）。也就是说：**新库那个会话排序的热查询索引，今天是靠"意外重放迁移"拿到的**。
直接跳过迁移就会让它消失 —— 那比假日志严重一个量级。

所以这次是三件事一起：

- `pre_table_count` 移到 `execute_batch(SCHEMA)` **之前**（判据的位置就是本次的修复本体）。
- 新增 `ensure_post_schema_shape()`：**两条分支都经过**，专门放"SCHEMA 表达不了、
  又必须每条启动路径都拿到"的形状。今天里面只有一条索引；它存在的意义就是把那笔差额
  收在一个看得见的地方，而不是散落在"新库恰好会重放迁移"这个意外里。
- `is_fresh` 顶上那段"今天它区分不了任何东西"的自陈注释删掉了 —— 文档说谎比没文档更坏。

判据三条：`fresh_schema_alone_has_exactly_the_migrated_shape`（形状恒等，防"跳过迁移少东西"）、
`tables_are_counted_before_the_schema_is_applied`（源码顺序，防本退化复发）、
以及既有那族（`fresh_db_has_every_hot_query_index` / `index_on_a_migration_added_column_must_not_live_in_schema`
/ `hot_tables_carry_exactly_the_intended_indexes`）继续守住索引形状。
第二条另配一条 `verify-guards` 变异用例：**把两句调换顺序**（编译照过、测试套照绿，
只有这条守卫会红）—— 这正是"形状上看不出来"那个退化的反面证明。护栏用例 176 → 177，
Rust 基线 675 → 677。

### Fix (2026-09-25 · 第 3 步 P5 的一小半：锁内 emit 清零，并把纪律做成全扫判据)

生产环境只有一条 SQLite 连接（`AppState.db: Mutex<Connection>`，实测 292 处取锁点），
前端每个 IPC 命令都要抢同一把锁。所以「锁内 emit」不是风格问题，是一条真实的卡顿链：
后端在锁内发 `file-failed` → WebView 监听器立刻回一次 IPC → 那次 IPC 阻塞在还没释放的锁上
⇒ **用户看到"传输出错那一刻界面整个冻一下"**。

复审（P5）记的是 4 处，实测是 **5 处**，且行号已被这两天 P1/P2 的移动改废 —— 全部重新定位：

- `network/file.rs::fail_taken_receive` —— **2026-09-25 补接收侧静默回收时新引入的那一处**。
  同一个函数体里「写库 + emit」连着两行，逐行 review 谁都看不出毛病。
- `network/transport.rs` 群 Ack 清 outbox 一处。
- 中继收文件失败分支三处（尺寸不符 / 哈希不符 / 落盘失败）。

五处统一改成「`{ … }` 圈住写库，emit 出锁再做」，语义零变化（写序、终态、事件内容都不动）。

判据没有停在「把这 5 处改完」：新增 `scripts/check-lock-scope.mjs`（挂进 `npm run verify`
快速层，CI frontend 组），把这条从一句注释升级成一条有 id 的不变量 **INV-P25**：

- **281 个 guard 绑定**按「块结束 / `drop(NAME)`」判存活期；块结束用缩进判（rustfmt 保证语句
  缩进 I 的块闭合在 I-4），因此**不需要数大括号**，不受字符串 / `format!` / raw string 干扰。
- **11 个语句级临时量**（实参、`if let` 条件、let-else）按本语句结束判 —— 依赖 edition 2021
  的析构时机，脚本头部与 §25 都写明了「升 2024 时这一半必须改成块级」。
- **判不出作用域的直接算红**，不许静默跳过：一个会因为看不懂而放过的守卫比没有守卫更坏。
- 判据自带 7 段夹具自证（`--self-test`，抓 3 / 放 4），每次跑全仓前先跑它；
  另有一条 `verify-guards.py` 变异用例把「emit 挪回锁内」注进 `fail_taken_receive`，
  证明这条判据对**真源码**有效而不是只对夹具有效（护栏用例 175 → 176）。

它守不住的两件事一并写进 §25：跨函数的 emit（缓解靠命名纪律「会 emit 的一律叫 `emit*`」，
现状 `emit_failed` 是唯一 helper）、以及 db 之外的其它 Mutex（没有被前端 IPC 抢，要管得逐个论证）。

## [4.29.39] - 2026-09-25

### Perf (2026-09-25 · 第 2 步 · P3：链路队列按**字节**封顶，不再按帧数)

四个建链点（入站 / 出站拨号 / BLE 两处）各自写死三条深 1024 的 `mpsc` 队列，
而一片 LAN 分块上线是 `base64(256 KiB + 12B nonce + 16B tag) ≈ 341 KB`
⇒ **单链路最坏 ~350 MB**，多连接、群文件多收件人按连接翻倍。
背压本来就有（`send_on_link` 满了原地等 + `stall_tick` 每 5s 检查），
错的只是**位置**：缓冲先分配完，才轮到排队 —— 手机上就是 OOM / 整机变慢 / 进度假快。

- 新增 `LINK_QUEUE_BYTE_BUDGET = 8 MB`，`low_queue_slots(chunk)` 把预算折算成槽数
  （LAN 分片 ⇒ 24 槽；BLE 4 KB 分片 ⇒ 仍取上限 1024，只收紧不放宽；
  下限 8 是为了 `mpsc::channel(0)` 不会 panic）。
- 四个建链点统一走 `link_channels(chunk)` —— 顺手消掉那份**霰弹式重复**
  （改一处忘三处，正是本仓反复出问题的形状）。
- **优先级模型一字未动**：high/normal 仍按帧数。那两条上的帧被 `MAX_MESSAGE_LEN`
  与各类载荷上限卡着，1024 帧不是内存问题；这次只改容量语义。

**判据是一对，缺一不可**（两条都由变异测试证明会红）：
`link_low_queue_is_bounded_by_bytes_not_frame_count` 测「算得对不对」
（单调、绝不为 0、**槽数 × 单帧线上字节 ≤ 预算**），
`link_queues_are_created_from_one_place` 测「接没接上」（不许再有 1024 字面量、
四个建链点都走 `link_channels`、`low` 必须由折算函数开出）。
把 `low_queue_slots` 换成常量深度只有第二条会红；把 `link_channels` 内部绕开折算
只有第一条会红 —— 单留任何一条都会漏。
⚠️ 两个坑当场踩过并记下：护栏的**探针字面量**必须拼起来写（本文件就是被扫的源码，
直写会把护栏自己数进去）；连**散文注释**里写 `mpsc::channel(1024)` 都会被算成一次命中。

**没测的一件事**（不猜）：队列变浅对**吞吐**的影响没有实测。理由只是 writer 连续排空、
24 片 ≈ 8 MB 足够掩盖一次调度抖动，且 60s 停滞判据本来就以"写不出去"为准 ——
真机大文件速度是否变化由用户实测判定（P3 的初衷正是那条实测）。

**P4（中继借用接收把整个文件读进内存）没动**：它是待拍板项，两个方案语义相反
（改流式 `.part` 复用直连那套接收器 / 先加 `RELAY_RECEIVE_MAX_BYTES` 尺寸闸 + 界面诚实提示），
我倾向前者，但那是"第二条实现向第一条看齐"的改动，比这一刀大。

新增用例 2 条（Rust 基线 673 → **675**）、变异用例 2 条（护栏非空转 173 → **175**）。
契约图同步：管线里"不能越过自己 ≤1024 深的队列"改成字节预算口径、漂移表 +1 行、
护栏与测试计数刷新（顺手发现自己刚写进去的一行被截断，已补全）。

## [4.29.38] - 2026-09-25

### Fixed (2026-09-25 · 第 1 步 · 故障隔离 P1：断一条链路不再杀掉该 peer 的全部文件接收)

`reader_loop` 的收尾一开头就按 **peer** 清 `file_receivers` 与 `group_file_receivers`，
而同一个函数下面那段「只删这一条连接」的注释早就写明了「**断一条 ≠ peer 下线**」——
两处一直自相矛盾。后果：LAN + Tailscale 双链路时断其中一条，会把另一条上**正在收**的
文件一起判死（用户看到的正是"传大文件传到一半失败，但网络明明是通的"）。

- 两处清理都挪进 `if peer_now_offline { … }` 门内，即"这个 peer 一条链路都不剩"才动手。
- 延后之后必须有兜底，否则只是把「杀错人」换成「泄漏」，所以补了 **`file::receive_is_stale`**
  （5 分钟没被喂片）+ `transport::sweep_stalled_receives`（挂在每小时那一趟）。
  为什么非补不可（实测，不是设想）：**`protocol.rs` 里没有任何 cancel 帧**，
  发送侧 60s 停滞只是自己退回 `file_outbox`，**从不通知接收端**；
  而 `sweep_stale_parts` 明确跳过"还在表里"的 `.part`（把静默接收器当活跃证据），
  `resume_receive` 的 24h TTL 又只在**有人再来敲这一单**时才生效 ⇒ 被放弃的单永久留着
  表项 + 文件句柄 + `.part`。
- `FileReceiver` 加 `fed_at_ms`（单聊与群文件共用这一个结构 ⇒ 一处加字段两张表都受益）。
  **不用 `last_report_ms` 顶替**：那是 IPC 节流用的、还有一处初始化成 0，不是"还在不在收"的证据。
- 摘表与判据**必须在同一次持锁里**（`take_stalled_receive` / `take_stalled_group_receive`）：
  先快照一批 id、释放锁、再逐个收尾 —— 这两步之间完全可以挤进一个新 `FileOffer`
  （同一 transfer_id 重建接收器、`fed_at_ms` 就是现在），按 id 收尾就把**正在收**的那单判死了。
- 收尾只有一份实现：`fail_receive` 拆成「摘表」+ `fail_taken_receive`（落终态 / Incomplete / emit），
  `fail_group_receive` 同理拆出 `fail_taken_group_receive`；清扫器复用同一份，不另写一遍。
- 窗口 5 分钟必须**宽于**发送侧 60s abort（否则会在对端重排队的间隙里清掉自己那半截）——
  这条关系是断言，不是注释。
- **有意接受的代价**：这条回收挂在每小时那一趟，所以"对端在线但这单被放弃"时 spinner
  最长多留 1 小时。治的是永久泄漏，不是界面延迟；要更快就把它挂到 30s 那趟清扫器上（已记进契约图）。

**判据**：行为侧 `stalled_receiver_is_reclaimed_only_after_the_idle_window`（边界 + 与发送侧
窗口的关系）；接线侧 `peer_wide_receiver_cleanup_is_gated_on_total_link_loss` 与
`hourly_sweep_covers_parts_relay_and_stalled_receivers`（`reader_loop` / 周期任务都吃
`Arc<AppState>`，单测造不出来 ⇒ 只能源码钉，判据与副作用的分工写在各自身上）。
`verify-guards.py` 新增 **3 条**变异用例：清理挪回门之前 / 每小时那趟不再回收 /
退回"快照 id 再逐个杀" —— 三条都必须改坏即红。
⚠️ 这一轮被非空转验证抓出来的两个"假红/假绿"都记下来了：护栏切片一路取到文件末尾，
会把**测试模块里自己的字面量**数进去（永远命中 ⇒ 假过；删掉字面量后又红在编译错误上）；
注入必须"照样编译、但语义变坏"，否则红的是 `cannot find value` 而不是判据。

契约图同步（这次改的是跨层不变量，图比代码更容易腐烂）：文件管线加了一步「接收侧两种收尾、
判据不是同一把」；每小时那一步补上接收器回收；`file_transfers` 那行「无索引」是 0-B 的遗留
错误也已改对（顺手发现 0-B 漏改了这一行）；漂移表加 2 行（P1 已修 + 1 小时延迟是有意）。
Rust 基线 670 → **673**。`Cargo.lock` 的 `gosslan` 版本这次才跟上 4.29.37（一并提交）。

### Fixed (2026-09-25 · CI：Windows 那条腿从 0-A2 起一直红着 —— 陈旧基线，不是代码坏了)

`check-test-manifest.mjs` 的判据是单向致命的：**基线里有、实际没跑 ⇒ FAIL**（"静默跳过"必须拦），
而"跑了但没登记"只是警告。`src-tauri/test-baseline.windows.txt` 上一次生成是 **09-22**，
0-A2 删掉的三处死实现里有 **17 条测试跟着没了**，Windows job 因此从那天起一直红：
`discovery::lan::tests::*`(7) + `discovery::manager::tests::*`(4) + `discovery::routed::tests::*`(4)
+ `transport::tests::route_*`(2)。

- 只删这 17 条，基线 500 → **483**。逐条自己复跑过证据，不信 diff：
  `discovery/{trait,manager,lan}.rs` 文件已不存在（`ls` 直接确认）、
  四个函数名全仓 `grep` 零命中。
- **三条 Windows 专属项原样保留** —— 它们在"只在 windows 基线里"那份差集里，
  看着像陈旧项，其实是被 macOS 侧互斥 `#[cfg]` 挡掉的活代码：
  `network::transport.rs:9530 windows_abortive_close_allows_immediate_rebind`、
  `transport/bluetooth_peripheral_windows.rs:673 / :709`。
  （按差集无脑删就会顺手删掉这三条保护，而它们在 macOS 上永远不会有人发现。）
- 顺带一个必须记账的真相：**Windows 基线落后 190 条**。"跑了没登记"不报红，
  所以 09-22 之后新增的用例（0-A1 的登记守卫、0-B 的 6 条、P2 的 3 条、中继一族…）
  在 Windows 上**完全不受清单守卫保护**。修它只能在 Windows 上跑一次
  `node scripts/check-test-manifest.mjs --update`（本机交叉 `--target` 检查与 GitHub API
  都被开发机的权限层拦了，所以这事只能落在 CI/Windows 机器上）——已另立待办。

## [4.29.37] - 2026-09-25

### Fixed (2026-09-25 · 第 1 步 · 故障隔离 P2：写失败按**错误来源**分流，不再一把抓)

旧形状是 `writer_loop` 里一句 `if res.is_ok() { Ok } else { Failed }` —— 任何写失败都
"记一次连接失败 + 拆这条写半"。于是一条**本机就写不出去**的帧（序列化不出来，或长度超过
`MAX_FRAME`）会把一条好端端的连接判死；而连接一死，`reader_loop` 收尾又按 peer -wide
清接收器 ⇒ **一个超长帧拖死同一好友其它链路上正在跑的文件传输**（这就是 P1 与 P2 的连结点）。

- `write_frame` 现在返回**带来源**的错误 `WriteError::{Local, Socket}`，而不是让调用方去嗅
  `io::ErrorKind`：**只有产出错误的那一层知道自己有没有碰过 socket**。
  `Local` = 长度校验/序列化失败，**一个字节都没写进 socket**；`Socket` = `write_all` 真失败。
- `Local` ⇒ 丢掉这一帧 + `eprintln!` **error 级**留痕（带 `msg.wire_kind()` 类型名），**链路保留**；
  未送达的帧仍留在 outbox / file_outbox 里，界面上是"排队中/未送达"而不是"已送达"
  —— 不许静默丢（INV-005），也不许把它伪装成"文件失败"就完事（那是我们自己的 bug）。
- `Socket` ⇒ **行为一字未变**：仍然 `mark_conn_failure` + 拆这条写半。
  这是刻意加的反向约束：**不允许"为了保护文件传输"继续复用一条已经写不出去的连接**
  （用户 2026-09-24 划的界；目标是"连接失败 ≠ peer 失败 ≠ 该 peer 所有文件失败"，
  而不是"文件永不拆链"）。
- 拨号握手那处（`write_frame` 的另一个调用点）**不分流**：那一刻链路还没建立，
  没有"保留"可言，两种失败都归 `DialOutcome::Failed`，只把原因换成 `WriteError::reason()`。

**判据（3 条用例 + 1 条变异）**
- `oversize_frame_is_a_local_failure_and_writes_nothing`：真 `duplex` 链路 + 超过 `MAX_FRAME`
  的帧 ⇒ 归 `Local`，**并且断言对端一个字节都没读到**（"半截帧污染流"比"没写"更糟，所以这条
  断言不是锦上添花）。
- `socket_write_failure_is_classified_as_socket`：对端整半被 drop ⇒ 归 `Socket`（反向护栏）。
- `writer_loop_splits_local_from_socket_failure_exactly_once`：`writer_loop` 吃 `Arc<AppState>`
  造不出来 ⇒ 分流点的**接线**只能用源码护栏钉：两半各一条断言 + "判死点全函数只有一处、
  且不在 Local 分支里"。
- `verify-guards.py` 新增一条双变异用例：把 `Local` 折回 `Failed`（旧形状）必须红，
  把 `Socket` 折成 `Local`（"保护文件"式修法）也必须红 —— 两个退化方向各对应一次真事故。
  ⚠️ 锚点带行尾逗号才唯一：护栏自己的 `contains(...)` 里写的是不带逗号的那半截。

Rust 基线 667 → **670**。**P1 那半（接收侧按端点/仅在全链路断开时才清）没做**，
卡在一条未定的前提上：延后清理之后，"喂不到分片的接收器"由谁回收
（现在既没有接收侧的 idle TTL，`sweep_stale_parts` 反而把内存里的接收器当"活跃"永久保护）——
先答这个再动，否则会把一个"杀错人"换成一个"泄漏"。

## [4.29.36] - 2026-09-25

### Performance (2026-09-25 · 架构改造 0-B：4 条热查询补索引，判据用查询计划而不是"索引存在")

复审 P5 的"缺索引"一族。**每条都对着生产 SQL 本体跑 `EXPLAIN QUERY PLAN` 断言**，
因为"索引建了但查询用不上"照样是全表扫 —— 列序错了测试不会红，人眼也不会发现。

- **`idx_group_recalled_msg(msg_id)`**：撤回判定 `is_recalled` 只给 `msg_id`，而这张表
  的 PK 是 `(conv_id, msg_id)`（前缀用不上）⇒ **每条群收件**都在扫这张只增不减的 G-Set。
- **`idx_file_transfers_created(created_at DESC)`**：`list_transfers` 是
  `ORDER BY created_at DESC` 且没有任何 WHERE，而这张表**原先零索引** ⇒ 每次开传输面板
  都是全表扫 + 临时 B-tree 排序，而历史传输不删。
- **`idx_content_transfers_peer(peer_id, status, updated_at)`**：建链时捞"该 peer 还有哪些
  可恢复内容"，而 `peer_id` 原先不是任何索引的前缀。
- **`idx_file_outbox_peer_due(peer_id, status, next_attempt_at)`**：旧的两列版只能定位到
  "这个 peer 的全部待发行"，时间窗要逐行回表比。
- **迁移 v8→v9 删两条被取代的索引**（"删"是 SCHEMA 表达不了的，所以它才是迁移的正当职责）：
  `idx_outbox_msg_id`（与 `outbox.msg_id` 的**内联 UNIQUE** 是同一件事的两份 ⇒ 热表上白付的写放大）、
  旧 `idx_file_outbox_peer`（是新索引的严格前缀）。测试同时证明：删掉索引后重复 `msg_id`
  仍然插不进去（约束的权威从来是内联那个），且第二次启动 SCHEMA 不会把它们造回来。

**两处原判断被实测推翻**（都记进了复审报告的 0-B 表，别再照旧说法动手）：
① 「`idx_messages_conv_seq` 在新库上不存在」**不成立** —— `init()` 里 `is_fresh` 恒为假：
`pre_table_count` 是在 `execute_batch(SCHEMA)` **之后**才数的，新库此时已有 19 张表 ⇒
走 else 分支，**全新库会把整条迁移链重放一遍**。真实副作用是首次启动在日志里打出一串
**假的**「正在迁移 v1→v2…」；修它要挪取数位置 = 改启动行为，**单独一轮**，本次只把注释改对。
② 「索引一律并进 SCHEMA」这条规矩**有边界**：把 `idx_messages_conv_seq` 挪进 SCHEMA 之后
**三个"老库升级"用例全红** —— SCHEMA 跑在迁移之前，而 `messages.seq` 是 v2→v3 才 ADD 的列 ⇒
老库上那句 `CREATE INDEX` 报 `no such column: seq`，`init()` 直接失败 =
**用户打不开自己的数据库**。这条索引因此留在 v2→v3（建列之后），并新增判据
`index_on_a_migration_added_column_must_not_live_in_schema` 钉住这个坑。

**写放大实测**（0-B 验收第 ③ 条；A/B **换序各跑一轮**，否则分不清"索引变贵"和"缓存热了"）：
2000 行 × 21 次真实 upsert —— `file_transfers` 883/890 → 903/911ms（噪声内）；
`content_transfers` 1939/1963 → **2282/2290ms（+17%，与顺序无关）**；
库文件 618,496 → 724,992 字节（≈ +53 B/行）。折算**每个进度 tick 多约 8µs**，
而 tick 间隔是 250ms ⇒ 接受。⚠️ 没把耗时写成断言（CI 上必飘），钉的是
**每表索引数量封顶**（写放大的确定性代理）；`dbstat` 在这个 bundled 构建里读不到（实测报错），
所以空间那条线改用文件大小量。

**用户看得到什么**：打开消息多的会话、传输面板、以及每次建链/心跳的补发捞取不再全表扫；
写路径的成本是每 250ms 一次进度更新多几微秒。行为语义零改动。

新增用例 6 条，Rust 基线 **661 → 667**；`verify:full` 全绿。
**没做**：P5 表里第 5 行「跨会话搜索缺 `messages(ts)`」—— 它不在 0-B1 的四条里，
实际走的是 `search_chat_history`，量级未测就不加索引。

## [4.29.35] - 2026-09-25

### Removed (2026-09-25 · 0-A3 收尾：5 条零引用命令按拍板删掉)

上一条 0-A3 只做了挂账，用户 2026-09-25 拍板「都删」⇒ 隐形接口清零。
**注册表 138 → 133，与前端调用面正好相等**，`DEAD_COMMANDS` 白名单现在是空的
（此后新增一条不接界面的命令直接红，零容忍）。

- **`commands/chat.rs` 尾部 271 行删除**：`cancel_send` / `resend_message` / `recall_message`
  （连同文件头的职责边界注释一起改：这里从此不声称管「撤回」）。
  `commands/group_files.rs` 删 `send_group_poll` / `cast_group_poll_vote` + 随之孤立的
  `MAX_POLL_OPTIONS`（留着就是 clippy `-D warnings` 的 dead_code 红）。
- **两条源码护栏跟着删**（它们钉的是被删函数的体）：
  `resend_message_sets_sending_only_after_all_failure_paths`（审计 1.2「所有可失败步骤通过
  之后才置 sending」）、`resend_reseals_before_enqueue`（2026-09-19 P0「先重新密封再入队、
  seq 必须沿用原记录」）。**这两条知识没有丢**：改记在
  `docs/protocol-invariants.md` §6，并明确写出"界面上的重发是已知例外" —— 它按 INV-P06
  生成的是**新** `msg_id`，不是重投。将来谁真要做「按原 id 重投」，必须把这两条连实现一起带回。
- **两个死监听删除**：`message-cancelled` / `message-resending` 的唯一发射点就在被删命令里。
  顺序是刻意反着走的：先清空白名单跑红（守卫点名这 5 条），再删实现 ——
  「前端监听的事件必须真的有人发」这条守卫只会在发射点先消失时红。
- **保留的三样**（判过才留，不是没看见）：
  ① `poll` / `poll_vote` 的**线上词表与渲染**（老消息与对端新版本仍要能显示），只是本端不再产生新投票；
  ② `messages.status = 'cancelled'`（`cancel_file_transfer` 至今在写它，且老库里已有这类行）——
  `db::set_message_status` 那句"为什么 failed/cancelled 是软终态"的注释已改成指向活理由；
  ③ 群聊版撤回 `recall_group_message`（界面在用）。
- **非空转证据**：清空白名单后守卫报
  `新增了注册但前端从不调用的命令：cancel_send, cast_group_poll_vote, recall_message, resend_message, send_group_poll`，
  与 `generate_handler!` 差集逐字一致。Rust 测试基线 663 → **661**（少那两条护栏），
  前端 **588** 条不变。
- **顺带查出的下一层（未动，等拍板）**：门面收成一道缝之后才看得见 ——
  有 **5 条包装没有任何 UI 调用点**：`sendFile(send_file)` · `sendFileRelay(send_file_relay)` ·
  `searchMessages(search_messages)` · `getInterfaceCandidates(get_interface_candidates)` ·
  `closeLogWindow(close_log_window)`。对账守卫看不见这一层（它比的是"注册表 vs 门面"，
  不是"门面 vs 界面"）。逐条手核过：活的是 `sendFileAuto` 与 `searchChatHistory`；
  `openImagePreview` / `getGroupTodosContext` 是**假阳性**（调用点跨行写成 `api` 换行 `.name()`，
  单行正则会被骗 —— 所以这 5 条是按名字逐个 grep 确认的，不是脚本数出来的）。

## [4.29.34] - 2026-09-25

### Refactor (2026-09-25 · 架构改造 0-A3：把 IPC 收成一道缝，并让它可对账)

复审 P10 的第四组：`src/api/index.ts` 自称"前端唯一碰 `@tauri-apps/api/core` 的地方"，
实测**有 10 处旁路**（`boot.ts` · `localFile.ts` · `MessageItem.vue` · `FavoritePanel.vue` ·
`MessageComposer.vue` · `GroupTasksBoard.vue` · `ImageLightbox.vue` · `GroupFilesPanel.vue` ·
`SettingsWindow.vue`）。危害不是"不工作"，而是**契约面画不全**：其中 6 条命令
（`copy_file` · `copy_file_to_clipboard` · `save_data_file` · `read_clipboard_file_paths` ·
`get_group_file_delivery_summary` · `log_frontend_error`）在门面上根本没有包装 ⇒
任何人（或 AI）只读 `src/api/index.ts` 来画接口表，会**整截漏掉**真实调用面。

- **6 条包装补齐**，10 处旁路全部改走门面（含 `ImageLightbox.vue` 里那处
  `await import("@tauri-apps/api/core")` 动态 import —— 它同样绕过门面，只是静态 grep 看不见）。
  现在 `grep "@tauri-apps/api/core" src/` 只剩门面自己一行。
- **两条新守卫**（`src/api/events.test.ts`）：① 「IPC 只能从 `src/api` 门面走」—— 扫全部前端源码，
  静态与动态 import 都算违规；② 「注册表与前端调用面必须逐条对账」—— 从 `generate_handler!`
  抽注册集（138）、从门面抽调用集（133），差集必须**逐条命中显式挂账表**，多一条少一条都红。
  这条是"注册了但没人调"唯一的机器防线：以前只有人肉数，数完就烂。
- **5 条零引用命令：挂账，没有删**（`cancel_send` · `resend_message` · `recall_message` ·
  `send_group_poll` · `cast_group_poll_vote`）。复审原文是"要么补界面要么删接口"，这里**故意偏离**：
  `resend_message` 头上压着一条活护栏（`lib.rs` 的
  `resend_message_sets_sending_only_after_all_failure_paths`），删接口等于连护栏一起删；
  三条 chat 命令是完整实现而非半成品；投票两条的 kind 已在线上词表里、只差 UI。
  ⇒ 已挂账可查，**删不删仍待拍板**。
- **一处旧护栏跟着搬家**（`channelState.test.ts`「点击重取必须接通后端 `request_content`」）：
  原判据是 `MessageItem.vue` 里的 `invoke<boolean>("request_content"` 字面量，收拢后字面量进了门面。
  断言改成两环同钉 —— 组件必须调 `api.requestContent(`，且门面的 `requestContent` 必须真的
  `invoke<boolean>("request_content"`。钉的语义没变（重取必须真打后端），只是**换了钉的位置**：
  这类"钉字面量"的护栏在代码搬家时必须同步改，否则它会红在错的地方、并被当成"测试过期"直接删。

前端测试 586 → 588（两条新守卫）。`vue-tsc --noEmit` 干净。

### Removed (2026-09-24 · 架构改造 0-A2：删掉三处"看起来像入口"的死实现)

复审 P10 的三组未接线实现，逐条自己复跑过调用点才动手（`transport/bluetooth.rs` 头部那句
"这些注释大多还挂着 `#[allow(dead_code)]`，把编译器本会给出的提示一起静音了 —— **动这一带代码前
请先核对调用点，别信注释**"就是为这类事故写的）。

- **`discovery/{trait,manager,lan}.rs` + `RoutedDiscovery` 删除**（`discovery/` 现在只剩 `routed.rs`
  的配置解析）。判据：`DiscoveryManager` / `announce_to_candidate` / `RoutedDiscovery` 在 `discovery/`
  之外**零引用**；真跑数据的发现在 `network/discovery.rs`（由 `network/mod.rs:55` spawn）。
  ⚠️ 原"收口动作"写的是"把 socket 循环搬进 `discovery/lan.rs`、删掉旧家"—— 那是设想，不是证据；
  现在按相反方向收口（删掉没在跑的那一家），并在 `mod.rs` 顶部写明家在哪儿。
- **`transport/mod.rs` 的通道抽象删除**：`Transport` trait、`TransportManager::route`、
  `route_payload`、`Channel`、`LARGE_PAYLOAD_THRESHOLD`（64 KiB"大负载走 LAN"）+ 两条测试。
  `lan.rs` / `bluetooth.rs` 的 `impl Transport for …` 收成固有 impl，并删掉 `LanTransport::send`
  /`broadcast`（`outbound.rs` 同名逻辑的**第二份实现**，零调用）与无人用的 `name`/`start`/`stop`。
  该模块现在只做一件事：**状态聚合**。真分流只有一处 —— `dispatch.rs::message_priority`
  + `mesh/selection.rs::pick_link` + `file.rs::chunk_size_for_path`。
- **`file_relay.rs` 发送侧整组删除**（299 → 86 行）：`split_bytes` / `slice_file*` /
  `register_send` / `next_chunk` / `is_send_done` / `plan_distribution` / `ack_chunk` /
  `finish_send` / `progress` / `active_sends` + `ChunkData` / `RelayPlan` /
  `DEFAULT_CHUNK_SIZE` / `MAX_CHUNK_SIZE`。整个 `impl` 块头上原本挂着 `#[allow(dead_code)]` ——
  那正是它能存活至今的原因。保留接收侧（`begin_reassemble` / `add_chunk` /
  `sweep_stale_reassemblies`，都在跑）与 `MIN_CHUNK_SIZE`（`file.rs` 挑分片尺寸时用它兜底）。
- **顺带修掉一个"永远为 0"的界面数字**：顶栏「N 中继」原先取 `active_sends()`，而 `senders`
  只有 `register_send` 会写 ⇒ 恒为 0。改为数 `path_kind == Relay` 的活跃链路，判据与
  `relay.connected` 同源（新增 `state::link_is_relay_circuit` + `relay_circuit_count`，
  两处共用，不留第三份口径）。`get_topology` 是同步命令而 `links` 是 `tokio::Mutex`
  ⇒ 用 `try_lock` + 抢不到报 0（顶栏数字不值得阻塞工作线程）。
- **`MeshRouter::select_outgoing` 不删、不标 deprecated**：它逻辑完整且有测试，只是生产不走
  （群洪泛按 `fanout` 截断会把成员**静默切掉**，所以 `outbound.rs:428` 刻意只用 `exclude_source`）。
  改成把这句判断写进文档注释。**也刻意不加 `#[allow(dead_code)]`** —— 那个 allow 会静音编译器
  本会给的提示，正是上面那条注释警告过的机制。
- 文档同步：`docs/domains.data.mjs` 的 transport notes、`docs/migration-ledger.md` 第 1/4/8/9 行
  + 统计 + §2 命名撞车（"两个 discovery"从 ⚠️ 变 ✅）、`docs/ARCHITECTURE-MAP.html` 对应条目与
  领域图快照。
- **口径纠正**：`transport/` 不是"没在跑的新栈"，是**字节层 + 状态聚合**（`tcp.rs` 的帧原语被
  `network/transport.rs` 复用）。复审报告里那句"新栈"已改准。
- 测试基线 680 → **663**（删掉的 17 条全属于被删实现自身：`discovery` 15 + `transport/mod.rs` 2）。
  `cargo clippy --features bluetooth -- -D warnings` 干净、`check-domain-map` / `check-domain-deps` 绿。
  ⚠️ 顺带发现：默认构建（**不带** `bluetooth` feature）下 `commands/network.rs:132` 有一条既有
  `needless_return` clippy 提示 —— 门禁只 lint `--features bluetooth` 那一档，所以今天不红。
  本次不动（不夹带），登记在 HANDOFF。

### Test (2026-09-24 · 架构改造第 0-A1 步：把"记得登记分册"变成"不登记就红")

架构复审（`docs/ARCHITECTURE-REVIEW-2026-09-24.md`）的 P10 说"手工清单与编译器集合不一致 ⇒ 假绿"。
**先把话说准**：实测那三份视图今天是**全的**（commands 24 分册 / db 16 / transport 3，各自减去
`*_tests.rs`），真正的缺陷不是"已经漏了"，而是**它靠人记得登记** —— 而这件事已经漏过两次：
4.25.0 接线中继时 `commands/relay.rs` 与 `transport/relay.rs` 都只登记了 `include!` 和领域图、
漏了守卫用的 `include_str!` 清单（现场注释还在 `lib.rs:611-613` 与 `network/mod.rs:229-231`）。
漏登记的后果不是报错而是**静默假绿**：以"全部命令面"为判据的守卫扫不到那个分册，于是永远通过。

- 新增守卫 `guard_source_views_register_every_include_subfile`（`lib.rs`）：从入口文件
  （`commands.rs` / `db.rs` / `network/transport.rs`）**递归展开 `include!`** 得到编译器实际看到的
  分册集合，与守卫函数里登记的 `include_str!` 集合**双向比**：
  少登记 ⇒ 假绿 ⇒ 红；多登记 ⇒ 守卫会扫根本不在这个模块里的代码（假红的来源）⇒ 也红。
  `*_tests.rs` 分册是**故意**不进视图的（测试文本会把生产模式扫描带偏），所以判据写成
  "闭包减去测试分册"并把理由写在断言消息里，而不是靠沉默。
- 两个自己踩到的坑，都留在了代码注释里：① 现成的 `rust_fn_body` 靠找 `\n}\n` 定尾，
  而 `mod tests` 里的函数缩进四格 ⇒ 它找不到锚点会**返回剩余整个文件**（它刻意的兜底），
  于是登记清单会被后面所有 `include_str!` 污染 —— 这条守卫要的是精确集合，另写了花括号配平的切法。
  ② 第一版用一条"闭包 ≥ 10"当防空转自检，直接把 transport（只有 3 个分册）判成"解析器失效"
  —— 阈值不能跨用例共用，改成**每个用例点一枚自己的 canary**（挑的都是 relay 那批历史肇事者）。
- 变异证明三条，全部「改坏即 FAIL、恢复即 PASS」，还原后 `cmp` 字节一致：
  ① 新加一个分册并 `include!` 进来但不登记 ⇒ 红，且消息点名那个文件；
  ② 往 `all_commands_src()` 里塞一个不在闭包内的 `include_str!` ⇒ 红（多登记方向）；
  ③ 把 canary 指向不存在的分册 ⇒ 红（证明自检不空转）。
  ①已登记成 `verify-guards.py` 的常设用例（注入就是"删掉 relay 那一行"，即当年真实形状）。
- Rust 测试基线 679 → **680**；`cargo clippy -D warnings` 干净。
  ⚠️ 顺带记一笔：`cargo clippy --all-targets` 会撞上 `examples/e2e_peer.rs` 里 3 条既有错误 ——
  门禁跑的是不带 `--all-targets` 的那条，所以它今天不在红灯范围内（不是本次引入）。

## [4.29.33] - 2026-09-24

### Changed (2026-09-24 · 群任务窗口改成固定 label，两扇高频窗支持"不传参数预热")

用户口径："群任务窗口和图片窗口要支持先创建 WebView，可不传参数、后台默默创建、界面也不显示，
等调用弹出展示时能瞬间激活。" —— **"不传参数"这一条决定了窗口身份必须改**：
群任务窗口原来是每群一扇（label = `todo-<groupId>`），预热那一刻根本不知道该建哪一扇。

- **群任务窗口变成一扇固定 label `tasks` 的窗口**，"现在该显示哪个群"改由后端那份
  **当前上下文**给（`AppState::task_window_group`，与预览窗口的 `preview_gallery` 同一套
  "当前值"语义）：窗口挂载时自己 `get_group_todos_context` 取一次，已经存在的窗口靠定向事件
  `group-todos-target` 被叫醒。**代价说清楚**：任务栏里不再能按群区分这扇窗。
- 固定 label 之后不再需要淘汰逻辑 —— 上一版为"每群一扇"加的隐藏 LRU（`AUX_HIDDEN_GROUP_TODOS*`、
  `prune_hidden_group_todos`）整体删除，全局就一扇，不会累积。
- **换群必须做两件事**（这是"常驻 + 可切换"换来的责任，以前由销毁重建顺带保证）：
  ① **重读该群数据** —— 那条定向事件现在承担"换群 + 重读"，不带任务目标也要发；
  ② **重置看板内部状态** —— 筛选、正在编辑的草稿、展开态都属于上一个群，留着就是串群。
  做法是给看板加 `:key="groupId"` 整块重挂，而不是在组件里手写"切群清草稿"（会漏）。
- **预热**：`prewarm_image_preview_window` 改成 `prewarm_aux_windows`，一条命令把
  预览 + 群任务两扇都"建好、文档加载好"，**不显示也不抢焦点**（`reveal = false`）。
  两扇的窗口构造各自与"真正打开"共用同一个 helper —— 两条路各写一份 builder 的话，
  "预热出来的窗口和真开的不一样"这种缺陷查不出来。主窗口 `app-ready` 之后延后 1.2s 触发；
  失败不提示（最坏就是第一次仍然慢一点）。
- 取数与订阅从 `entries/todos.ts` 搬到根组件：固定 label 之后"该显示哪个群"会在运行中变化，
  写在入口那道门里只对第一次打开有效，换群时反而没人负责。
- 守卫：`group_todos_window_label_derives_from_group_id` 换成
  `tasks_window_uses_one_fixed_label_cross_checked_with_frontend`（Rust 常量与前端启动器
  字面量交叉核对，两份各写一遍必然漂移这一族本仓库已踩过多次）；capability 的 `todo-*`
  通配换成固定 `tasks` 并进 `WINDOW_LABELS`；预热判据要求**两扇**都 `reveal=false`；
  入口那道门改成"只准 `app.init()`，取数必须在根组件里"。
- **这条判据自己被抓出来一次半空转**：预热的断言第二个 needle 写成在 `,""` 处截断，
  把 `false` 翻成 `true`（= 启动时就弹出一扇空窗）它照样绿。补全实参后同一个注入变红。
  变异共四处：拿掉看板 `:key` ⇒ 红；把取数塞回入口门里 ⇒ 红；预热改 reveal=true ⇒ 红；
  （上一版已验的"淘汰写进关闭回调"这条随着 LRU 一起删掉了）。
- 删除 `src/utils/auxWindowLabels.ts`（固定 label 之后没有 label↔群 的换算要做）与它的测试，
  `package.json` 的测试登记同步。

**仍然存在的边界**：预热只覆盖桌面端（移动端 `prewarm_aux_windows` 是桩，返回错误，前端不会调）；
预热失败或被系统回收后，第一次打开仍然要付一次 WebView 创建 —— 那是"秒开"的成本前置，
不是消失。

### Test (2026-09-24 · 护栏注入锚点全量核对：5 条已经腐烂)

上一批的 `verify:full` 红在第 14 步："注入锚点在 `todos.ts` 里出现 0 次"。**根因不是产品码，
是护栏脚本跟着源码腐烂** —— 变异用例靠"锚点在目标文件里恰好出现 1 次"定位，源码一挪就变成
"验证过程出错"，而 `verify:full` 只跑 `--only frontend` 那 50 条 ⇒ **打了 rust 标签的过期锚点
可以静默存留很久**。这次不再等门禁一条条撞红，而是把 **168 条锚点全量静态核对**（必须用脚本自带的
`_resolve_anchor_file`：它顺着 `include!` 找子模块，自己 naive 数会假报 20 条），查出 5 条真过期：

- **群任务入口不得初始化聊天事件**：锚点 `const chat = useChatStore();` 随"取数搬进根组件"消失了。
  改锚点时发现**判据本身有个洞**：入口不再声明 `const chat`，绕过 `chat.init(` 这条字面量的最短路径
  就是 `useChatStore().init()` ⇒ 禁令收紧成"入口连 `useChatStore` 都不许出现"（取数搬走之后，
  它在入口只剩 init / 取数两条红线用法）。写禁令时要问一句：**绕过这条字面量的最短路径是什么**。
- **门控表能力位自声明**：`content_features()` 在 F1′ 那批加了第三个位 `CONTENT_FEATURE_FILE_EPOCH`。
- **capability 覆盖每个窗口** / **外链窗口不得被 capability 覆盖**：`windows` 数组这两批改过
  （加 `preview`、`todo-*` 换成固定 `tasks`）。
- **群任务 label**：动态 `todo-*` 已删 ⇒ 用例改钉"builder 与 `ensure_aux_window` 两处都必须取自常量"，
  并写成**两处同时改坏**（护栏看的是整个函数体，只改一处仍然绿）。

5 条逐条跑过变异证明：改坏即 FAIL、恢复即 PASS。**产品行为零改动。**

## [4.29.32] - 2026-09-24

### Changed (2026-09-24 · 辅助窗口默认不再销毁 + 预览窗口预热)

用户口径两条："任务列表、图片这些新窗口创建完关闭就不销毁了，除了设置日志，其他新窗口默认
不销毁，第二次打开速度优化快点"；以及"启动的时候就后台异步把这些高频窗口的 WebView 开销开好，
到用的时候秒开"。

- **常驻分布改成**：群任务 / 图片预览 / 外链 = 关闭即隐藏；**设置 / 日志仍然销毁**
  （这两个每次都要读最新的环境数据与日志尾部，预热它们只是白占一份 WebView）。
  每条策略仍然只写在各自的 `AUX_*_RESIDENT` 常量上，守卫按表逐条核对布尔值。
- **"每群一扇"必须有上限**：群任务窗口的 label 是 `todo-<groupId>`，常驻之后
  "访问过 N 个群"就是 N 个隐藏 WebView —— 那正是它们当初被设成销毁的唯一理由。
  现在按 LRU 只留 **3** 扇（`AUX_HIDDEN_GROUP_TODOS_MAX`），第四扇开始淘汰最老的。
  ⚠️ **淘汰不能写在关闭回调里**：那个回调跑在 wry 的主线程事件循环上，本文件已经为
  "在主线程里做需要等主线程的事"死锁过一次（2026-09-20 真机）。所以隐藏时只记账
  （一次 `Mutex<Vec<String>>` push），真正的 `destroy()` 放在开窗命令返回之后做。
  复用时要把那扇从隐藏名单里摘掉，否则刚给用户看的窗口会被当成"最老的隐藏窗口"当面销毁。
- **复用时必须自己重读数据**：以前"每次打开都是新数据"是销毁重建顺带保证的，现在没人保证了。
  所以那条 `group-todo-focus` 定向事件从"只带任务目标时才发"改成**复用就无条件发**，
  窗口侧收到即 `loadGroupTodos` 重读；挂载那条路反过来**不重读**（首屏正在由入口后台加载）。
  #39 的"点卡片直达详情"仍然成立：目标先存着，等首屏落地再投。
- **预览窗口预热**（`prewarm_image_preview_window`）：主窗口 `app-ready` 之后延后 1.2s
  在后台把那扇预览窗建好、文档加载好，**不显示也不抢焦点**；之后第一次点图走的就是
  "已存在 ⇒ show + 聚焦"那条快路径。窗口构造与真正打开**共用同一份**
  `ensure_preview_window`（两条路各写一份 builder 的话，"预热出来的窗口和真开的不一样"
  这种缺陷查不出来）。为什么只预热它：设置/日志按口径仍然销毁，群任务窗口的 label
  启动时还不知道是哪个群。
- **常驻带来的资源义务**：预览窗口的相册现在必须在关闭时由那个文档自己放掉 ——
  挂在 `close-requested` 上（标题栏 ✕、看图器 ✕/Esc、⌘W/Ctrl+W 三条路都经过它），
  否则几十 MB 的图解码位图会永久留在看不见的窗口里，而且下次打开会先闪一下上一张图。
- 判据/变异：`AUX_*_RESIDENT` 四条布尔逐个钉；淘汰必须在开窗侧、不得出现在关闭回调里
  （注入 `w.destroy()` 到回调 ⇒ 红）；复用事件必须无条件（把条件收回成 `&& focus_todo_id.is_some()`
  ⇒ 红在"复用时必须无条件定向发事件"）；挂载/事件两条路的 `false`/`true` 各钉一次；
  预热必须传 `reveal = false` 且其函数体里不许出现 `show()`/`set_focus()`。
- **仍然没解决**：某个群**第一次**打开任务窗口仍要付一次 WebView 创建（启动时无法预知是哪个群）。
  要连这一次也省掉，只有把任务窗口收成"一扇可切换群的固定 label 窗口"——那会改变窗口身份与
  任务栏观感（现在每群一扇是为了在任务栏里分清是哪个群），属产品决定，未擅自改。
  预热失败不提示（最坏就是第一次点图仍然慢一点）。

## [4.29.31] - 2026-09-24

### Fixed (2026-09-24 · 用户实测：预览窗口只有骨架 / 点「查看任务」像没点上)

#### 1. 独立「图片预览」窗口永远停在骨架屏 —— 上一批那个功能实际上没生效

用户装远端打的 Windows 包实测："点击图片预览只有个骨架图没有内容，功能没有实现"。**根因不在
取数**（那条链是纯 IPC，`msgId`/`cid` 在预览窗口自己的文档里能取回字节），而在撤骨架那一行：
`dismissBoot()` 原先遍历一份**硬编码 id 清单**（`boot` / `boot-logs` / `boot-settings` /
`boot-todos`），加第 5 个窗口时没人回去改它 ⇒ `boot-preview` 永远不淡出也不移除。而那块骨架是
`position:fixed; inset:0; z-index:9999` + 不透明底色 ⇒ Vue 挂载成功、图片也取到了，
**整页被一张看不见的骨架盖着**。与平台无关（macOS 同样），只是用户先在 Windows 上试。

- 更糟的是**守卫当时是绿的**：`windowEntries` 里那条查的是 HTML 侧（骨架 id + `<html>` 类），
  完全看不见"谁来撤、按什么撤"。这一族缺陷的形状就是"定义在一处、消费清单写在另一处"。
- 修法**不是**往清单里再补一个 id（那只是把同一个坑留给第 6 个窗口）：改成骨架元素
  **自带 `class="boot-skeleton"`**（五个 HTML 各自那一行），撤除方按类查 —— 新增窗口时
  "写骨架"和"打标"是同一次编辑，漏不掉。
- 两头各钉一次（`windowEntries` 新守卫）：表里每个窗口的骨架根必须带那个类，且 `dismissBoot`
  必须按 `.boot-skeleton` 查。两次变异各自只咬住对应那半（拿掉 `preview.html` 的类 ⇒ 红；
  把撤除用的类名改错 ⇒ 红）。
- 顺带补一条**只读代码就能断定**的缺陷：预览窗口是 `decorations:false` + 自绘标题栏，而那条
  caption 是整扇窗**唯一的拖拽区**（`startDragging` 挂在它的 mousedown 上）与 ✕ 所在；
  `ImageLightbox` 是 `fixed inset-0 z-[80]`，铺满就把标题栏连按钮一起盖住 ⇒ 窗口移不动也关不掉。
  给组件加一个可选 `top-inset`，预览窗口传 `var(--gosslan-title-h)` 让出那一条；
  主窗口与移动端不传 ⇒ 样式对象里连 `top` 这个键都不出现，渲染结果逐字不变。

#### 2. 群「查看任务」：先挂载、数据后台补（同一条路径上三处缺陷一起修）

用户描述："弹窗弹出很慢，我还以为没点了，点了好几下一会才弹出来"。三条独立成因：

- **窗口内容被"挂载前那道门"堵住**（主因）：`src/entries/todos.ts` 原先在 `beforeMount` 里串了
  `loadGroupTodos`（四次刷新 + 一次消息拉取）与 `watchGroupTodos`，而骨架屏是**挂载之后**才撤的
  ⇒ 那段时间窗口里什么都没有，只剩一块灰底。现在门里只留 `app.init()`（外观必须先于渲染，
  否则闪错主题），取数与订阅挪到挂载之后；看板新增一档「正在加载任务…」，由 store 里新加的
  `todosLoadedOnce(groupId)` 驱动 —— 语义是"这一次取数**结束了**"（成功失败都置位），
  少了这一档，先挂载就会让首帧理直气壮地显示「暂无任务」，那是**假空态**，比慢更糟；
  而失败若不置位，一次 DB 出错就把窗口永久留在转圈上（连点都救不回来的状态）。
  取数失败另外 toast 一句：静默空列表会被读成"任务丢了"。
- **装饰性数据能否决用户动作**：`open_group_todos_window` 在"取群名填系统标题"那一步，
  拿不到 DB 锁时直接把整个开窗判成失败 ⇒ DB 一忙（文件传输的进度写库就算）这扇窗根本打不开，
  而前端只把它变成一句 toast。现在降级成"没有群名"继续开窗（文档加载后前端本来会再取一次并
  接管标题）。新守卫 `cosmetic_title_read_cannot_abort_the_window` 用**正向形状**钉 ——
  刻意不写负向断言：本文件读的是原始源码（不剥注释），一句解释"以前错在哪"的注释会把自己踩红，
  那是同一天上午刚踩过的课。
- **反馈轻到看不见**：`ChatHeader` 那颗按钮在"正在开窗口"时只加了 `opacity-60` —— 在一个 18px
  图标上几乎不可辨，而连点又被启动器的单飞/防抖吃掉（不报错、不重复开窗，但也**没有任何回应**）
  ⇒ 主观就是"点了没反应"。改成图标位直接换成转圈；成员面板里那颗「查看全部」同样补上
  （同一个 pending 状态透传）。
- #39「点卡片直达详情」必须跟着改：窗口现在挂载时数据还没到，而 `focusTodo(id)` 在空列表里
  找不到那条时**什么都不做**（那是它的设计）。所以取回的目标先存 `pendingFocusId`，等
  `todosLoadedOnce` 置位后再投 —— 少这一步的表现就是"窗口开了却没展开那条"。
- 判据：`windowEntries` 新增"群任务窗口的挂载前门里只准有 `app.init()`"，用**位置**判
  （取数调用必须出现在挂载完成之后；只按 `mountAuxWindow(` 的位置判是没用的 ——
  门里的代码在文字上也在它之后）。变异：把两条取数塞回门里 ⇒ 红在"必须排在挂载之后"。

**没解决、也不假装解决的**：第一次开这扇窗仍要付一次 WebView 创建的成本（它关窗即销毁 ⇒ 下次还要付）。
要让"第二次起接近瞬时"只有两条路 —— 群任务窗口常驻（每群一个 label ⇒ 访问过的群各留一个 WebView，
数量无上限），或收成**一扇可切换群的窗口**（像这次的预览窗口那样：固定 label + 暂存 + 定向事件）。
后者改的是窗口身份与任务栏观感，要用户点头再动。另外消息气泡里那张任务卡片的入口没有转圈反馈
（要把状态穿过两层组件），本次没做。Rust 用例基线 678 → **679**。

### Test (2026-09-24 · 文件传输链回归 · 第 2 段 —— 投递失败之后"再试还是判死")

接第 1 段（`fe01723`）同一把刀法：把决定用户看到什么的判断从吃 `AppState` 的循环里搬出来，
搬成能测的纯函数，再让调用点只剩"照裁决写库"。

- **重试裁决 `send_retry_verdict(err, attempts)`**（纯函数 + 4 条单测）。原先这三行判断
  长在 `flush_pending_files` 的循环里，而那个循环要 `AppState`，一次都触发不了；
  它偏偏就是"用户看到失败+原因"还是"永久转圈"的唯一分岔口 —— 真机 160MB 那次的形状正是
  从这里漏出去的：每轮都算"可重试"，于是连续 5 次从头重灌，谁也不报错。四条判据：
  永久错误当场放弃且**理由就是原始文案**（不许换成笼统一句）；预算内回到 pending；
  **`retryable` 也要过预算**（少了这一关，"可重试"就等于"永远转圈"）；
  读不到次数（prepare 失败、行已被别的出口删掉）时按 0 次处理，**不得判死** ——
  那是本机自己一时出故障，把它当成"已经试满 5 次"会毁掉一次本可恢复的投递。
- 调用点同步收成"只有写库"：退避毫秒数改由裁决给出（原来调用点写死一个 `5_000`，
  与 `MAX_FILE_OUTBOX_RETRIES` 的注释各说一套），日志由 `attempts_over=布尔` 换成
  `attempts=实数`（判据搬走之后，布尔那个数已经是第二份事实来源了）。
- **接线也钉住**（新守卫 `the_outbox_retry_decision_has_exactly_one_judge_and_one_caller`）：
  四条单测证的是纯函数本身，这条证"循环里真的在用它" —— 抽取判据最容易漏的就是这半边：
  判据搬走、测试全绿，循环里还留着旧的 `if !retryable || over_limit`，两份判据不一致时
  没人发现。同时钉住 `GiveUp` 分支必须真的 `fail_file_job` + 留日志、`Retry` 分支必须用裁决
  给的退避、以及 `commands/` 里不许再出现第二次 `>= MAX_FILE_OUTBOX_RETRIES`。
- 变异自证（每条都先 `grep -c` 确认注入真的在文件里，再跑测试 —— 这个顺序本身是一次教训：
  有一次注入被前一条命令的还原带走了，"通过"什么也没证明）：去掉永久错误分岔 ⇒ 红 2 条；
  把"读不到次数"当成已用满 ⇒ 红在"不得判死"；`>=` 改 `>` ⇒ 红在预算上限；
  删掉 `fail_file_job` ⇒ 红在"放弃只存在于日志里"；把调用点参数换成写死值 ⇒ 红在"0 处调用"。
  五处还原后全部 `cmp` 字节一致。Rust 用例基线 673 → **678**。
- 行为**没有**变化：四格真值表与改前逐项等价（含 `unwrap_or(false)` 的降级方向）。

## [4.29.30] - 2026-09-24

### Test (2026-09-24 · 文件传输链回归 · 第 1 段 —— 隔离与"真成功")

用户清单 #25：把这条链上"只有真机并发才炸、编译和单测全绿"的部分**改成测得出来的**。
这一段先做两件能真的事，并为此抽了一处缝。

- **业务隔离（大文件不得拖死别的业务）**：`saturated_chunk_channel_does_not_stall_text_or_control`
  把一条链路的 Low 通道灌满且让对端不取，断言同一条链路上的文本（Normal）与心跳（High）
  仍在同样的时间预算内送出、且各自落在自己的通道里。已有的
  `send_on_link_respects_priority_channels` 只证明"分片落在哪个通道"，**不证明"分片堵的时候
  别人还在动"** —— 三级通道被合成两级时它照样全绿，而用户看到的是"传大文件期间聊天一起卡住"
  （和 600MB 复核里"持锁 fsync 堵住并发 write_chunk"是同一类耦合，只是发生在发送侧）。
- **显示成功必须真成功**：把 `finish_receive` 摘出接收器之后的那半段独立成
  `finish_receiver_into(&Mutex<Connection>, ...)`（它不要 `AppState` —— 那个要 tauri
  `AppHandle`，单测造不出来）。于是 `receive_is_done_only_when_size_and_sha_both_prove_it`
  第一次是**拿生产码**验这四条：少收一片 ⇒ 失败且不出现成品文件、半截临时文件被删；
  长度对但内容错（分片损坏的应用层形状）⇒ 只有 SHA 抓得住；**发送方没声明哈希（空期望）
  必须 fail-closed**，否则一条不带哈希的 `FileDone` 就能宣布成功；hex 大小写不同仍是同一个哈希。
  每条都同时断言 `file_transfers` 落的终态（`failed` 且 `path` 为空 / `done` 且带真实路径）。
  ⚠️ 此前接收端测试是**影子副本**（`receive_one_chunk` / `receive_group_chunk` 各自重写一遍
  操作序列 ⇒ 生产码改坏它们不会红），那正是"看起来有测试"的形态，本次没有删它们（它们还管着
  seq/解密/超量这几步），只是不再靠它们证明"成功"。
- 两处形状守卫/变异自证：改完 `receive_finalize_slow_work_happens_outside_the_receiver_lock`
  （它原来在包装函数体里找 `.sync_all()`，拆分后会 panic 在"找不到慢活锚点"），新判据多一条
  结构性的——核心函数**不许再碰 `file_receivers` 表**，一碰就能把锁写回慢活的作用域；
  两条新测试各自打过变异（把 Low 并进 normal ⇒ 红在"分片堵塞时文本必须照常送出"；
  把 `received != size` 放宽成 `>` ⇒ 红在第 ② 条；空期望改成放行 ⇒ 红在第 ④ 条），
  还原后用 `cmp` 确认字节一致。Rust 用例基线 671 → **673**。
- **这一段没有覆盖的（如实报）**：多文件同发 / 中途断链 / 收发两端重启 / `FileCompleteAck`
  丢失这几条仍只有源码守卫与真机证据，没有行为级测试 —— 它们要么需要 `AppState`（发送侧全流程）、
  要么需要两条在途传输 + 真实计时。下一段的做法是把发送侧 `stream_file` 的"裁决"部分同样拆成
  不吃 `AppState` 的纯核，而不是在测试里再抄一遍序列。

## [4.29.29] - 2026-09-24

### Added (2026-09-24 · 批次 x 第二步 —— 桌面端全局唯一的「图片预览」窗口)

接上一步收口的状态，实现用户 #40 剩下的那半句："开一个新窗口、全局只有一个、
不同界面查看图片替换里面的内容、窗口比聊天界面大一圈"。

- 新增第四扇独立窗口 `preview`（固定 label ⇒ **全局只有一个**，与设置/日志同一套
  `ensure_aux_window` 单例+串行创建；不是像 `todo-*` 那样每群一个）。从会话、任务看板、
  任务详情、合并转发卡片点开的图**都是同一扇窗**，第二次点只是换内容。
- 内容跨窗口**不传数组、传引用**：主窗口把 `{items, index}` 写进后端那份"当前相册"，
  窗口自己按 `msgId` / `cid` 去取字节。载荷有上限（500 张 / 名字 200 字符 / 内联 data URL
  合计 32MB），超限或有条目不可寻址就**整份拒绝**，前端退回应用内覆盖层 ——
  用户点了必须看到图，不能"什么都没发生"，也不能少显示几张当半成品。
  ⚠️ `blob:` objectURL 是**发起那个文档的句柄**，跨文档拿到只会渲染成一片破图，
  所以前端 `deliverableToWindow` 与 Rust 侧 `validate_preview` 两道都挡（合并卡片那处
  同时改成优先给 `cid`，于是它也走得了新窗口）。
- 投递时序沿用批次 w 验证过的那条：已开着才定向 `emit`，新建那条路靠窗口挂载时自己取
  （新 webview 还没有监听器，事件一定发丢）。取用**不清**后端那份当前值 ——
  每次投递整体覆盖，留着不会串给下一次；取走即清反而会在"事件先到、监听器还没注册"的
  窗口期把内容丢掉。
- 尺寸 1120×820 的设计值受既有不变式「辅助窗口永不比主窗口大」约束，由 `fit_aux_window`
  夹到"主窗口两侧各减 24px" ⇒ 就是"比聊天界面大一圈"，主窗口小则跟着缩。
- 关闭语义不需要回传：`openGallery` 每次无条件重投，所以用户 ✕ 掉窗口后再点同一条也重开。
- 移动端保持应用内全屏覆盖层（那条命令的 `#[cfg(mobile)]` 桩直接返回 Err，
  而前端在 `isMobile` 这一步就不会去调它）。
- 守卫：`validate_preview` 补了纯函数单测（三种合法来源 + 空/越界/超限/`blob:`/空串各判一次）；
  `aux_windows_open_their_own_document` 与 `aux_window_open_is_singleton_serialized_and_resident`
  纳入新窗口，并顺手把前者原来那个"新增 URL 会读到别人的 HTML"的 `_ =>` 兜底改成显式四分支
  （那正是假绿的形状）。`windowEntries` 的禁止性判据改成先剥注释再判 ——
  实测第一版是被"注释里写着这里绝不调 `chat.init()`"自己踩红线踩红的；
  三条新守卫都做了变异验证（换成 index.html / 删 `.visible(false)` / 常驻开关改 true 各自变红）。

## [4.29.28] - 2026-09-24

### Changed (2026-09-24 · 批次 x 第一步 —— 图片预览收成"一个公共能力"）

用户 2026-09-24 #40 要求看图是一个**公共组件**：吃一个数组、能左右循环、全局只有一个窗口/实例，
后续任何功能都能调。这一步先做**收口**（下一步再把这份状态镜像到桌面独立预览窗口）。

- 原先 `ImageLightbox` 有**四份实例**（会话 / 任务看板 / 任务详情 / 合并转发卡片），
  各自持有 `images/index/open`。问题不是重复代码，而是两件：
  同一件"看图"在不同入口的能力**取决于那个面板有没有把数组传全**（会话里是整屏上下文循环、
  任务里只有这一条的几张图 —— 本该是同一个契约）；且"合并卡片的图是 objectURL、组件一关就被回收"
  这份知识锁在那个组件里，别处复用不到。
- 现在契约是一次调用 `openGallery(items, startIndex, source)`，状态在 `useImagePreviewStore`，
  渲染点只有 `ResponsiveLayout` 里那**一个** `<ImageLightbox>`。
  `source` 是这次唯一新增的概念，它解决的正是"实例搬出来源面板"带来的两个反向问题：
  来源面板关闭时**不能**无条件关掉预览（否则"从任务详情点开图、再把详情关掉"会把图一起弄没），
  但也**必须**在 URL 失效前收掉（合并卡片那些 blob: 地址一回收就是破图）——
  所以按来源收：只有"这份相册就是它给的"才关。合并卡片那处的顺序是"先收预览、再 revokeObjectURL"。
- 空数组一律**什么都不做**（调用方不必各自判空）；起始下标越界夹进范围内
  （越界说明调用点算错了顺序，静默夹住比跳到一张不存在的图好）。
- 守卫：新增一条"预览只有一处渲染点、别处只能走 store"（`designGuards`，与"未读徽标只有一处实现"
  同族），并更新 `storeContract` 里那条"切会话必须收尾会话级浮层" —— 它原来钉的是本地
  `lightboxOpen.value = false`，现在同一要求换成按来源收的 `preview.closeIfFrom(\`conv:${prev}\`)`，
  判据仍是代码形状。写守卫过程中自己踩到一处：数渲染点时"看行首是不是 `<!--`"不够，
  注释是多行的、提到组件名的那行不带 `<!--`（第一版把三处注释全判成渲染点）；
  改成只在模板段内、先剥掉 `<!-- ... -->` 再数，并且**只报文件不报行号**
  （剥注释后偏移变了，报出来的行号会是假的 ⇒ 宁可少给信息也不给错信息）。

## [4.29.27] - 2026-09-24

### Added (2026-09-24 · 批次 w —— 桌面端点任务卡片，独立窗口里也展开那条)

批次 t 只做到"应用内弹窗/移动端直达详情"，桌面端仍只开到看板（当时的理由是"不想为这点便利
新造一条定向跨窗口通道"）。用户 2026-09-24 明确要求补上 ⇒ 这条通道做了，而且必须**两条路都覆盖**：

- **新建窗口**：只写一次性暂存（`AppState::todo_focus_request`，`groupId → todoId`），
  窗口挂载时自己 `take_group_todo_focus` 取走。只发事件在这里必然丢 —— 建窗是异步的，
  事件到达时那个文档还没有任何监听者，表现就是"第一次点没反应、第二次才有"。
- **窗口本来就开着**：它不会经历挂载 ⇒ 写进暂存之后**定向 emit** 一条 `group-todo-focus`
  叫醒它，它再取同一个暂存（取走即清，所以不会重复展开，也不会把上次那条带进下次打开）。
- **投递还必须是独立的一条命令**（`request_group_todo_focus`）。原因不在后端而在前端：
  `launchAuxWindow` 带单飞 + 连点防抖，判定"这次不算新打开"时**根本不会调用**
  `open_group_todos_window` ⇒ 目标如果只随那次调用走，第二张卡片带的 id 就地消失，
  又是"点了没反应"。所以 ChatWindow 先投递目标、再走启动器（顺序有源码守卫钉，
  且两个下标各自独立取再比大小 —— 从前者往后找后者的写法永远测不出反向改动）。
- **不带目标地打开会把暂存清掉**（标题栏那个按钮）：不清的话下次从按钮打开窗口，
  会被上次那条任务莫名展开。
- `todoId` **不进窗口 label**：`todo-<groupId>` 是"每群一窗"的**身份**，塞进任务 ID 就变成
  "每条任务一个窗口"。两个 ID 都按不透明字符串夹紧（长度 + `[A-Za-z0-9_-]`），
  载荷只带 `groupId`，目标由窗口自己按本群去取 ⇒ 别群的任务串不过来。
- 事件名两端是否对上由既有的 `api/events.test.ts` 负责（它扫 Rust 的 emit 与前端 listen 的名单），
  这里不重复钉；新加的守卫钉的是**接线**：暂存写/清、只在复用时才 emit、挂载时取、投递排在启动器之前。
- 又抓到一处自己写的弱判据：新写的顺序断言最初是"从 `reqAt` 往后找 `launchAuxWindow`"，
  这种写法对反向改动完全无感（永远只会得到"在前面"）；改成两个下标独立取再比，
  变异回测（把投递挪到启动器之后）才真的报出来。
  另记一条踩坑：测试里的局部变量**别叫 `chat`** —— `storeContract` 会把 `chat.indexOf(...)`
  读成"界面用了 store 上不存在的一个成员 `indexOf`"而判红。

## [4.29.26] - 2026-09-24

### Fixed (2026-09-24 · 批次 v —— 任务列表：下拉不再被遮挡 + 归档/还原进列表)

用户 2026-09-24 的三条。

- **「换状态」下拉被遮挡的根因**：菜单原先挂在任务行里（`absolute right-0 top-full`），
  而它被**两层**东西裁掉 —— 列表卡片自己的 `overflow-hidden`（画圆角用的）和外面那层
  `overflow-y-auto` 滚动容器。这是同一个坑的**第三处**：表情面板与「已读成员」弹层
  2026-09-24 已经为同样原因改成 Teleport + fixed，判据也早抽成了 `utils/popupPosition`，
  这次只是把看板的行内菜单也接上去（不再抄第三份算法）。
  现在菜单 Teleport 到 body，横向按入口右缘对齐向左展开（结构上顶不出右缘），
  纵向过视口中线就往上弹（用 `top`/`bottom` 贴边，不估菜单高度），滚动或改窗口即收起，
  并接入全局浮层互斥（与右键菜单 / 表情面板 / 已读弹层同刻只开一个）。
- **归档与还原都进列表**：完成行上一枚「归档」，已归档行上一枚「还原」，
  不必再点开详情才能收摊或拎回来。两枚按钮与「完成」按状态互斥（只有完成态能归档），
  所以不会出现同屏抢位。
- **还原对全体群成员开放**（同日追加：「归档和被归档的数据还原，任何人都可以操作，
  其他权限不变」）。后端原来只有"只动归档位"一条窄档，还原被算进"改状态"那一档
  （只有创建者/群主/被指派人能做）—— 而列表上给每个成员摆一个点了会被后端拒的按钮是不能接受的，
  所以放的是判据本身：新增第二条窄档 `reopen_only_change`（库里是「完成」、请求退回「待办」、
  其余字段逐字相同）。
  ⚠️ 方向**单向**：只认 `done → todo`，反过来"把没干完的任务标成完成"仍然不放宽 ——
  成员能收摊、也能把活拎回来，但不能替别人宣布干完了。
  两条窄档在后端合成一个 `MemberLane`（三条判据都要求"除那一位之外逐字相同"，
  按位置参数摊开就是九个入参），判据各有单测。
- **顺手抓到一条守卫空转**：`浮层协议` 那条源码守卫只匹配 `const X = useExclusivePopup(`，
  而本仓**一半的浮层是解构写法**（`const { isActive: readersOpen, ... } = ...`）——
  已读弹层和这次的行内菜单对它是完全隐形的，等于"协议只守一半使用点"。
  补上解构形状后 `seen` 从 7 涨到 9，并加了一条 `seen >= 8` 的下界断言
  （扫不到使用点就是判据形状失配，宁可显式报错也不要静默全绿）；
  变异回测：删掉新菜单那行 `watch` → 守卫精确点名 `GroupTasksBoard.vue → rowMenuOpen`。
- **另一处自纠**：Rust 侧那条接线守卫按 `lane_at + 400` 数窗口字节，而 flatten 之后的源码里
  全是中文注释 ⇒ 切进多字节字符中间直接 panic。改成按闭合标记 `"};"` 切。
  （同一个教训在 `group_keys_always_precede_group_messages` 里已经写过一次：**别按字节数窗口**。）

## [4.29.25] - 2026-09-24

### Fixed (2026-09-24 · 批次 u —— 头像上传不再被静默丢掉)

用户 2026-09-24 #24「头像上传后当前页要立即更新」。**查下来根因不在刷新**：
同页那条 watch（`[app.device?.nickname, app.device?.avatar]` → 重同步 ref）和跨窗口那条
（后端 `notify_settings_changed(["nickname","avatar"])` → 别的窗口定向重拉 `device_info`）
本来都在。真正的缺陷是**保存被静默丢掉**：

- `onAvatarChange` 直接复用昵称那条 `saveProfileNow()`，而它对"昵称为空"是**提前 return** ——
  于是"输入框恰好被清空 + 点头像换图"这一次上传什么都没做，页面却已经显示新头像：
  切个页/重开就弹回旧图，界面上没有任何报错。现在头像路径自己走一个落库出口
  （`persistProfile`），昵称为空时以**库里现有的昵称**一起提交；连库里都没有昵称
  （设备信息还没到位）就明确拒绝并把本地改动回滚，绝不发一个空昵称出去。
- 保存失败（后端嫌头像大、IPC 出错）以前**不回滚**：页面上挂着的是一个从没落库的头像。
  现在 `catch` 里把 `avatar` 改回旧值再报一次错（错误文案带后端原因）。
- 落库收成一个出口：`app.updateProfile` 全文件只出现一次（原来昵称路径与头像路径各调一份，
  两份错误处理迟早漂移）。store 仍是唯一真相 —— `updateProfile` 的返回值就是新 `device`，
  上面那条 watch 会把界面重同步成**后端确认过的值**（含截断）。
- 新增一条源码守卫，并在写的过程中抓到它自己**两个**空转：① 按裸名字搜 `saveProfileNow(`
  会因为注释里写着这个名字而误报（注释提到 ≠ 代码调用），改成匹配 `await saveProfileNow(` 的
  调用形状；② "文件里有没有 `avatar.value = prev`"是弱判据 —— "昵称为空"那条拒绝分支里也有
  同样的赋值，把 `catch` 里那句删掉守卫照样绿（实测过），所以改成按 `catch` 块切窗口再判。
  两条都做了变异回测。

## [4.29.24] - 2026-09-24

### Fixed (2026-09-24 · 批次 t —— 群任务三处外显同源于同一份折叠结果)

用户 2026-09-24 #23。三条都"看起来功能还在"，所以只能钉调用路径。

- **时间线里的任务卡片不再永远显示「待办」**。卡片气泡读的是**创建那条 `todo` 消息的载荷**，
  而之后每一次改动都走 `todo_update`（静默事件、不进时间线）⇒ 卡片自己的载荷永远停在创建那一刻，
  而新建任务恒为「待办」。表现就是"群里任务早干完了，聊天里那条还挂着待办" ——
  而这恰恰是用户去点它的原因。现在状态改查会话层折一次的 `todo_id → 当前状态`
  表（`ChatWindow.todoLiveStatus`，与 `reactionMap` 同构：**会话层算一次**再按 id 分发；
  每条气泡各自折叠就是 O(n²)，那条教训在表情回应那里已经踩过一次）。
  表里查不到（任务被删 / 那条没被折进来）才退回快照 —— 宁可显示旧的，也不空着或骗人说已同步。
  标题/描述/指派人仍只有快照：那些字段没有实时源，看板才是权威列表。
- **点卡片直达这一条任务的详情**（移动端与应用内弹窗；桌面端独立窗口仍只开到看板 ——
  它的参数只能走窗口 label，而 label 是"每群一个窗口"的**身份**，把 `todoId` 塞进去会变成
  "每条任务一个窗口"；本仓零 `emit_to`，为这个便利新造一条定向跨窗口通道 + 处理
  "窗口还没起来就先到的事件"这个竞态不值当，**这是刻意保留的边界**）。
  归档态会先切到「已归档」那一档再开详情，否则详情背后根本不是它所在那一屏；
  watch 判据是 `[open, focusTodoId]` **一起看** —— 只看 id 的话，关掉面板再点**同一条**卡片
  第二次不触发（值没变），用户看到的就是"这按钮时灵时不灵"。
- **两处任务计数与看板对齐**：弹窗标题的 `(N)` 与群成员面板的摘要原先算的是"**含归档**的全部"，
  而看板默认那一档是活动任务 ⇒ 标题写 (9)、进去只看到 6 条。两处都改成
  `foldTodos(...).filter(x => !isEffectivelyArchived(x))`，与看板逐字同口径
  （成员面板里 `total` 也因此恒等于各状态之和，不会一个含归档一个不含）。
- 新增一条源码守卫钉这三件事（折叠发生在会话层 / 表要一路传到卡片 / 两处计数都要滤归档 /
  watch 必须带 `open`）。四条断言各做变异回测：卡片退回只读快照、面板去掉归档过滤、
  watch 只看 id，守卫都精确点名对应的那一句。

## [4.29.23] - 2026-09-24

### Changed (2026-09-24 · 批次 s —— 群任务：归档放宽给全体群成员 + 列表按状态着色)

用户 2026-09-24 的三条：「群里所有人都可以归档」「完成的需求不要加横线，像删除一样」
「不同状态的任务列表显示不同颜色」。

- **归档对全体群成员开放**。原先"归档"和"改状态/改描述"同处一档（创建者 / 群主 / 当前被指派人），
  别人干完活想把它收起来都得等创建者动手。现在后端判权多一条窄档
  （`commands::may_change_todo`）：**本次请求只动归档位**（标题 / 状态 / 指派人 / 描述 / 图片
  与库里最新定义逐字相同，且归档值确实翻转）且**本人是本群成员**才走这一档。
  三条边界缺一不可 —— 少第一条，"改标题顺带归档"就能顺着口子把前两档作废；少第二条，外人能归档；
  少第三条（值没翻转 = 什么都没改），一次空请求也能绕进来。
  **⚠️ "重新打开"不在这一档**：它是把状态从「完成」改回「待办」，仍归创建者 / 群主 / 被指派人，
  所以"谁都能归档"不等于"谁都能取消归档"（前端的「恢复」按钮口径与此一致）。
  前端镜像判据 `canArchiveTodo` 与后端逐项对齐，两边各有一份用例表
  （Rust `todo_archive_only_lane_is_narrow_and_member_only` / 前端「归档档」，
  后者同时断言"放宽没有外溢到改状态与改结构"）。
  同一份测试里还钉了**接线**：命令层必须调总判权 `may_change_todo` 并把 `members` 真的算进来 ——
  误接回两档的 `may_update_todo` 不会有编译错误（函数还在、签名也对得上），
  表现只是"所有人都归档不了"。变异回测：把调用点改回两档判据，守卫精确报出。
- **顺带修掉一处"显示成功但没真成功"**：原先对未完成的任务请求归档，后端会
  **静默**把 `archived` 写回 false 并返回一条成功记录（调用方以为归档了，任务却没动）。
  现在明确拒绝并给话「只有完成的任务可以归档」。放宽到全体成员后更容易撞上这条路，
  所以一并补上。
- **完成态不再划删除线**（两处：活动列表的「完成」分组行、已归档列表行）。
  划掉会让"干完了"读成"作废了"；状态本来就由**左缘色条 + 右侧胶囊 + 分组头**三处一起说清楚。
  已归档行仍用 `text-2` 降一档（归档态靠"淡"表达，不靠"划"）。
- **列表按状态着色**：每一行（以及它所属分组的那一节标题条）左缘一道 2px 状态色条，
  判据收在 `TODO_STATUS_BAR` 一处 —— 与状态胶囊、状态文字同一族色
  （进行中原色、延期 warning、完成 success，「待办」是"还没开始"⇒ 与离线点同一个中性灰 token）。
  为什么不做"整行换底色"：底色要么淡到等于没有，要么把 hover / 选中态盖掉，四种底色同屏也花。
  新增两条断言：色条表必须覆盖全部四态（漏一个界面就"没样式"）、且只准引用 token
  （写死色值的话浅色下好看的那支在深色下会糊进面板底）。

## [4.29.22] - 2026-09-24

### Added (2026-09-24 · 批次 r —— 通讯录里认得出哪一行是自己)

- **自己那一行名字后加「（我）」**（用户 2026-09-24 #31）。「自己」在通讯录里是一条**伪好友行**
  （`ConversationList.selfFriend`，与好友共用排序/首字母分组/搜索/点进资料页），以前只能靠昵称认出
  自己，改名之后更容易认错。
  标记挂在**渲染层**而不是改数据：往 `nickname` 上拼后缀会连带污染首字母分组、排序与搜索命中。
  后缀走 i18n（`friend.selfSuffix`：中文「（我）」/ 英文 " (me)"），并同一份名字既进悬停 `title`
  也进整行 `aria-label` —— 只加视觉标记的话，读屏用户听不到"这是我"。
  行身份判据复用 `utils/selfChat::isSelfConversation`（那个文件的注释明写"四处各写一遍
  `=== device_id` 迟早漏一处"），没有新增第四份判据。
  新增源码守卫三条：通讯录的**两个渲染分支**（搜索平铺 / 浏览分组）都必须挂上标记 ——
  只挂一个是"搜到了带、平时不带"这种半个界面上的修复；判据必须走 `isSelfConversation`；
  模板里不得硬编码中文后缀。变异回测：删掉分组分支的 `:is-self`、把判据改写回 `=== device_id`
  两条断言都精确点名（顺带发现并修掉守卫自己的一处空转：`indexOf("<template>")` 找不到返回 -1
  会让"切片里不含中文后缀"永远成立）。

## [4.29.21] - 2026-09-24

### Fixed (2026-09-24 · 批次 q —— 切换会话的响应优先)

用户 2026-09-24 #29：「切换会话要瞬间响应，内容后台异步加载」。骨架与"先切 `activeConv` 再拉数据"
在 `openConversation` / `ChatWindow` 里本来就有（`ConversationList.openConv` 也一直是先翻页再调用），
缺的是**另外三条入口**：它们把翻页写在了 `await` 后面，于是那条"异步"路径变成了整页的前置条件。

- **「发消息」（资料页按钮）不再等读库才翻页**。`openConversation` 第一行**同步**写下 `activeConv`，
  其后才是骨架 + `getMessageCount` → `getMessages` 两轮**串行** IPC（还要排队过后端那把全局
  `Mutex<Connection>`）；原先 `app.mobileView = "chat"` 排在这两轮之后 ⇒ 移动端点下去那一下毫无反应。
- **点系统通知进会话**同一条修法。`focusWindow()` 仍排在前面 —— 窗口还在托盘里时就翻页，
  用户会在窗口浮出的瞬间看到一次跳变，那条是刻意保留的。
- **收藏跳转**改为先收起收藏页、再发起定位。这是三处里最贵的一处：`locateMessageInConv`
  命中可能早于已加载窗口，它会从最新一页往前**逐页翻到 MAX_PAGES(10)** 才报"没找到"；
  原先收藏页要原地挂着等这几轮读库。找不到时仍如实 toast（人已经在会话里），不静默。
- **消息缓存的 LRU 上界 4 → 8**：这条上界直接决定"切过去是当场有内容还是先看到骨架"
  （骨架判据就是 `messages[convId] === undefined`）。淘汰是纯计数、与多久没打开无关 ⇒
  常聊 5 个人按 A→B→C→D→E→A 转一圈，回到 A 那一下**必然**是冷加载，这是确定会发生而不是偶发。
  代价：每会话上界 1000 条 × 0.5–1 KB ⇒ 最坏多留 4 个会话约 2–4 MB 的二级内存副本；
  渲染侧仍只画活跃会话视口内的行，放大的是内存不是帧开销。**未测真机内存曲线**（见已知边界）。
- 新增源码守卫：三处入口的"翻页/收页"必须排在读数据之前，且**两侧都要求找得到**
  （`indexOf` 找不到返回 -1，只比大小的话"把那行删掉"反而会让守卫变绿）。
  三条断言各做一次变异回测：把翻页挪回 `await` 之后，守卫逐条精确点名。

## [4.29.20] - 2026-09-24

### Added (2026-09-24 · 批次 p —— 移动端聊天页返回箭头上的未读总和)

- **进聊天页后「外面还有多少条没看」有地方看了**。移动端打开一个会话时底部 TabBar 是隐藏的，
  未读总数在那一屏上原本**没有任何外显**（用户 2026-09-24：返回箭头上应该跟那个总和同步）。
  现在返回箭头右上角挂一枚未读徽标，数字与 TabBar、桌面导航栏、托盘/Dock 角标**同源**：
  调用方传 `chat.totalUnread`，`ChatHeader` 只接 prop、不自己聚合（当前会话进入即清未读，
  所以总数天然就是"外面"的量，不需要再减一次）。
  徽标用全应用唯一实现 `UnreadBadge`（`designGuards` 禁止再手写一份），箭头的 `title`/`aria-label`
  一并带上「返回，N 条未读」，读屏用户拿到的信息与视觉一致。
  新增源码守卫：头部出现 `.reduce(` 聚合、或调用方不再原样传 `chat.totalUnread`、
  或 TabBar 与箭头读的判据分家 ⇒ 报错。两条变异回测各自精确点名对应的断言。

## [4.29.19] - 2026-09-24

### Fixed (2026-09-24 真机复核 · 批次 o —— 群聊两个浮层缺陷)

先按"根因优先"走完调查，再动手；两处里**只有一处**有可确认的机制，另一处按红线记成待证据，
不照猜测改布局。

- **① 表情面板能同时开两个 —— 根因确定并已修**。全局浮层互斥协议有两半：
  `claim()` 是"我要展开"，`watch(popup.isActive)` 是"我被抢了 ⇒ 收起自己"。
  `MessageItem` 里右键菜单的 `ctxMenuPopup` 一直带着那条 watch（`popupRegistry.ts` 顶部写的
  就是这个 bug 的原型），而表情面板的 `reactionPopup` **只 claim 不看被抢** ⇒ B 拿到展开权后
  A 的 `isActive` 变 false 却没人读，A 的面板原地挂着。全仓 8 个浮层里只有这一个漏了 watch，
  用户复现路径（点 A 的表情、再点 B 的）正好命中它。补上那条 watch 即修，不动任何语义。
  另加一条源码守卫 `浮层协议：每个 const X = useExclusivePopup(...) 都必须 watch(X.isActive)`
  （放在 `popupRegistry.test.ts`，它是这套协议的家）—— 变异回测里删掉那条 watch，守卫精确点名
  `components/MessageItem.vue → reactionPopup`。
- **"点面板外面不关"这条没有改成**（如实记录）：`onDocClickForReactionPicker` 挂在 document 上，
  入口与面板各自 `@click.stop`，点别处的 click 确实会冒泡到 document ⇒ 静态读码找不到失效机制。
  真正的现象由 ① 解释得通——**两块面板**时关掉其中一块看起来像"关不掉"。若补上 watch 后真机
  仍然收不掉，需要一次带录屏/日志的复现再查，不预先改事件模型。
- **② 群"已读成员"弹层被屏幕边缘裁掉 —— 机制确认并已修**。它原先挂在消息行里
  （`absolute right-0`），而消息列表是 `overflow-y: auto` 的滚动容器 ⇒ 横向一并被裁。
  表情面板为同一个原因早就改成 Teleport + fixed（`MessageItem:1112` 的注释写着这条），
  已读弹层当时漏了 —— 同一个坑两处各修一次。
  → 现在也 Teleport 到 body，坐标按**入口右缘对齐、面板向左展开**算 ⇒ 结构上不可能顶出右缘；
  纵向用视口边距（`top` 或 `bottom`）定位，不再需要估面板高度（列表可滚动，真实高度拿不到，
  拿估算值判方向会在临界值来回翻）。滚动/改窗口即收起，且忽略面板自己的内部滚动
  （表情面板那条"一拉滚动条弹框就消失"的教训直接复用）。
- 摆位判据收成新原语 `src/utils/popupPosition.ts`（`popupWidth` / `popupLeft` / `popupPlacement`），
  表情面板与已读弹层共用一份；3 条真值表单测钉住"入口贴右缘不许顶出屏幕""极窄视口不为负"
  "中线以下才往上弹"。前端 574 → **579**、vue-tsc 0、build 0。
- **仍待真机证据的一条**：移动端"群消息的已读**不见了**"（不是被裁，是整排头像不显示）。
  静态读码没找到平台闸门或数据断点 —— 渲染条件是 `mine && !isSelfMsg` + `readerIds.length > 0`，
  而 `readerIds` 来自 `chat.groupReaderIds(groupId, ts)`。要定它需要一次设备侧取证
  （同一条消息在桌面端有没有已读排、以及 `group-read` 事件到没到），照猜测改渲染条件只会
  把另一种情况弄坏，故本轮不动，记为已知未解项。

## [4.29.18] - 2026-09-24

### Fixed (2026-09-24 真机复核 · 批次 n —— 群同步：密钥必须先于消息，且每次建链都重发)

RC2 的因果链（真机现象："大文件失败之后群聊一直不同步，而单聊正常"）：
① `distribute_group_key` 的 `try_send` 返回 Ok 只代表帧**进了那条链路的 mpsc 队列**，
而 `flush_pending_group_keys` 据此清除登记 ⇒ 链路随即死掉时 GroupKey 跟着队列一起没了；
GroupKey **没有回执帧**，旧代码只有"公钥变化 / 新节点"才再发 ⇒ 密钥永久缺席。
② 三处补发点把 `flush_group_outbox`（群消息）排在 `flush_pending_group_keys`（群密钥）**之前**。
③ `handle_gossip` 在**解密之前**就把 msg_id 登进两层去重表 ⇒ 密钥没到的那条被静默跳过、
不回 GroupAck，而 msg_id 已烧掉 ⇒ 之后每次重发都被"见过"挡掉。
④ 发送端等不到 Ack，对端"可达"满 **120 秒** sweeper 就把群 outbox 行删掉置 failed ⇒ 唯一的补发载体消失。
四段合起来 = **永久、静默**的群消息丢失。单聊不受影响（直发 `ChatMessage` 不过传播层去重、
用 ECDH 不需要预分发密钥）—— 这个不对称正是"群坏了单聊好"的成因。

- 修 ①：新增 `requeue_group_keys_for_peer`，**每次**建链 / Hello / 心跳都把该 peer 所属的群
  全部重新登记一遍再发。接收侧 `handle_group_key` 本来就是幂等 upsert ⇒ 代价是每次链路建立
  多几个小帧，换来的是"密钥抖丢一次就永久丢"这条整段消失。判据拆成纯函数
  `group_ids_containing(groups, peer)`（只看成员里有他，不看"是否发过"）。
- 修 ②：三处触发点统一改成**先密钥后消息**，并用源码守卫钉住顺序
  （`group_keys_always_precede_group_messages`：对每一处 `flush_group_outbox` 回看同一串里
  必须先有 `flush_pending_group_keys`，再数三处重登记调用）。这条只能钉接线 —— 先后顺序没有纯函数。
- 修 ④：群 outbox 的判死窗口单独拆成 `GROUP_OUTBOX_FAIL_DEADLINE_MS = 30min`（与文件 outbox
  同量级），不再复用单聊的 120s。理由写进常量注释：群行是群消息**唯一**的补发载体，
  而 120s 比"链路抖动 + 密钥重发"的收敛时间还短；离线对端仍按 7 天保留（没削弱离线补发）。
- **③ 本轮刻意不改**（如实记录，不是漏）：把去重登记挪到"成功消费之后"会连带改掉**转发**语义 ——
  `handle_gossip` 第 4 步的 fan-out 依赖同一次登记来避免重复转发，延迟登记等于让每一份不可消费的
  副本（非成员本来就解不开）被反复重新转发，是放大广播而不是修 bug。①②④ 落地后"密钥后到"
  这条路已经被堵住：同一链路上密钥帧一定先于消息帧被处理。要真动 ③，得把"传播层去重"与
  "消费层去重"拆成两套键并各自定义淘汰，那是独立一轮设计，不塞在这次修复里。
- 测试：+3 条（`group_ids_containing` 成员筛选与稳定顺序 / 群窗口活过单聊窗口且到点仍放弃、
  离线保留不受影响 / 顺序守卫），`verify-guards.py` +2 条变异用例（tag `rc2-group-sync`，
  两条都实测"改坏即 FAIL、恢复即 PASS"）。守卫自身先修掉两个真缺陷：按字节回退窗口会切进
  多字节字符直接 panic、`count` 把函数定义那行也算进调用次数。Rust lib 666 → **669**。

## [4.29.17] - 2026-09-24

### Fixed (2026-09-24 真机 600MB 复核 · 批次 m —— attempt epoch：接收端不再被上一轮的分片打死)

这是 RC1 的根治（用户拍板选 B，直接上 attempt epoch，不做"容忍重试"的折中版）。
机制一句话：**一轮超时后 outbox 重投，但上一轮已经塞进链路队列的分片不会被撤回**
（Low 队列每链路 1024 槽 ≈ 262MB 明文）。每轮的 `seq` 都从 0 重编，于是旧轮的高 `seq`
落到新轮上就被 `chunk_seq_decision` 判成"跳号"⇒ `fail_receive` 摘掉整个接收器、整单死。
旧代码注释里"重传 seq 从 0 ⇒ 残片算 Duplicate 会被忽略"这条推理，只在残片**先到**、
新 Offer 把 `next_seq` 归零**之后"才成立 —— 队列里还压着几百 MB 时顺序恰恰是反的。

- 协议：`FileOffer` / `FileChunk` / `FileDone` 各加 `attempt: Option<u32>`
  （`serde(default, skip_serializing_if = "Option::is_none")`），新能力位
  `CONTENT_FEATURE_FILE_EPOCH = 1 << 2`。**三帧必须一起带**：少带一类，那一类就逃过过滤。
- 接收侧：`FileReceiver` 记 `attempt` 与 `stale_dropped`；分片与完成帧先过
  `file::frame_is_current`，非当前轮次的**安静丢掉**（不再判死），并在第一次丢弃时留一条
  日志 + 计数 —— 没有计数，"进度怎么不动了"就又成了一桩无解释的事故。
  `FileDone` 的判定必须在 `finish_receive` **之前**：那份函数一进来就把接收器摘走，
  摘完再判就分不清"陈旧"与"重复"（重复那条会补一个莫须有的成功 Ack）。
- **Offer 不参与过滤，反而是设定轮次的那一步**（两条接受路径各设一次）。
  这是刻意的设计而不是疏漏：如果 Offer 也被"比本机旧就丢掉"，本机重启后计数器回到小值时，
  新的一轮会被接收端永久判成陈旧 ⇒ 那份文件饿死。`send_attempt` 因此**必须**取持久化的
  `file_outbox.attempts`，绝不能用进程内计数器（守卫里把这条钉死了）。
- 兼容：老端不声明该位 ⇒ 本机一个字段都不带（`skip_serializing_if` 保证字节层面与旧版本
  完全一致，不去赌对方的反序列化器容不容得下未知字段）；老端发来的帧读成 `None` ⇒
  `frame_is_current(None, _) = true` ⇒ **完全旧语义**。升级不改变任何现存行为。
- 范围：只含 1:1 那三帧。**群文件（`GroupFileChunk`）与中继（`RelayChunk`）本轮刻意不动** ——
  群侧比单聊更脆（`seq != next_seq` 一律判死、且没有续传语义），要统一得连带它的
  段/attempt 语义一起设计，混进来会让这一版的取证面翻倍。已单独记为后续项。
- 测试与守卫：`stale_attempt_frames_are_filtered_but_legacy_frames_never_are`（三条设计各自钉住，
  含"未来轮次也不算当前"）、`file_frame_attempt_field_is_wire_compatible_both_ways`
  （不带字段的老 JSON 能读 + `None` 时字段整个消失）、`file_epoch_feature_is_advertised`
  （门控用了哪一位就必须声明哪一位 —— 这条不在 `kind_required_feature` 的覆盖范围里）、
  以及源码守卫 `file_attempt_epoch_is_wired_on_both_sides`（三帧带轮次 / 门控 / 取持久化
  attempts / 接收侧两处过滤 / Offer 两条路径各设一次）。
  `verify-guards.py` +3 条变异用例（tag `file-epoch`），**全部用"写死 None"这种编译得过、
  判据才红的注入**（删字段红的是编译器，那种确认证明不了接线）。Rust lib 662 → **666**。

## [4.29.16] - 2026-09-23

### Fixed (2026-09-23 真机 600MB 复核 · 批次 l —— 不含协议变更的那三条)

用户真机复报：macOS↔Android 同千兆 LAN、LAN+蓝牙都开，一次发多个文件（含一个 600MB+）⇒
Android 报"分片接收失败"、macOS 显示成功且无错误提示、之后**群聊新建/新增内容也同步不了**、单聊文字正常。
完整 RCA（四路只读调查 + 逐条 sed/grep 复核）写在 HANDOFF §9；本批先落**不需要改协议**的三条，
"接收端 Gap 立即判死"那条根治要用 attempt epoch（协议变更），单独一版。

- **deadline 改按线上字节估算**（`send_deadline_for`，`network/file.rs:182`）：每片 256KiB 明文
  上线要过 ChaCha20-Poly1305（+28B）再 Base64（×4/3）⇒ **×1.334**。原先按明文算，等于给每条链路
  少发 25% 窗口 —— 600MiB 只给 21min，而 512KiB/s 的链路实需 26.7min ⇒ **单轮注定超窗**，只能靠
  5 次重投 + `.part` 续传接力，而"接力"正是跨链路重选 ⇒ 陈旧分片 ⇒ 接收端 Gap 判死的入口。
  现在 600MiB 给 1660s（≈27.7min），精确值钉进测试（退回明文口径当场红）。
- **只剩蓝牙链路时不启动大文件**（新常量 `BLE_FILE_SIZE_LIMIT` = 16MiB + 纯函数
  `refuse_reason_for_best_link`，判据在 `commands/files.rs` 的投递循环里接）：BLE 上分片被压到
  4KiB，600MB = 153,600 片、按实测 ≈14KB/s 要十几小时，而单轮封顶 1h ⇒ 必然反复超窗重投。
  更关键的是 `file_sending` **按 peer 去重**、一次只跑一个发送任务 ⇒ 一个大文件在蓝牙上爬，
  会把同 peer 的其它文件全部堵在队列里（用户报的"文件消息继续异常"就是这一层）。
  处理方式：保持 pending、**不消耗 attempts、不落 failed**（判据必须排在
  `mark_file_outbox_sending` 之前，那一步会 attempts+1，5 次一到就永久 failed，
  "等 LAN 回来自动重试"就成了空话），LAN 一回来下一次 flush 自然接上。
- **接收端 `finish_receive` 的慢活移出 `file_receivers` 锁**：SHA finalize + `sync_all()` + rename
  原先全跑在函数作用域的锁守卫里，而它执行在 reader_loop 中 ⇒ 600MB 的 fsync 期间
  **其它并发文件的 `write_chunk` 全部堵在同一把锁上**，那些传输不再写出 ⇒ 发送端 60s 停滞判据
  把它们判死。改成"块内摘出接收器、块尾即放锁"，来源不符的塞回路径留在锁内。
- **停滞事件现在能带上原因**（`FileStalledInfo.reason`，可选、serde 跳过 None ⇒ 老前端忽略）：
  只有蓝牙这条是"在等更好的链路"，不是"网络卡住" —— 两者共用一句「网络停滞」会把用户引去
  查一个没问题的网络。前端把标记与原因存进**同一个 Map**（`stalledTransfers: Map<id, 原因>`），
  分两处存就一定有一边忘清；气泡优先显示原因、没有原因才回退通用文案。
- 测试与守卫：`send_deadline_scales_with_size` 改钉精确值；新增
  `ble_only_link_must_not_start_a_hopeless_large_file`（阈值边界取"不超过就发"、判据只看**最佳**
  链路、无链路时不表态）；新增源码守卫 `receive_finalize_slow_work_happens_outside_the_receiver_lock`
  （**位置比较**而不是调用次数 —— 退化前后 `sync_all()`/`finalize()` 次数一模一样）；
  前端 `storeContract` +1 条钉住 reason 的整条消费链。Rust lib **660 → 662**（基线已同步），
  前端 **574** 条。守卫非空转：把 `finish_receive` 改回函数作用域锁 ⇒ 只红这一条、提示正确，
  注入后源文件 `cmp` 字节一致复原。

## [4.29.15] - 2026-09-23

### Fixed (2026-09-23 稳定性审计 阶段 4 · 批次 k —— 「不打扰」的判据收敛成一份)

阶段 4 复核时记下一条"已知漂移未收敛"：后端 `protocol.rs::is_non_notifying_kind` 明写
`system` 归"不计未读、不改会话预览、不弹通知"（并说明理由 —— 加人通知现在**经消息管道广播**给
全体成员，否则「X 加入了群聊」会给每个人推一条通知），而前端**两处**各自只滤 `isSilentKind`。
当时判断"要先证明哪条路径真会走到这里"故未动。本轮把那条前提证掉了：`ShareFileRequest` 的系统
消息、群成员变更广播都会以 `kind: "system"` 走到摄入路径 ⇒ 这不是理论问题。

- `utils/messages.ts::applyIncomingToConversations`：`!isSilentKind(m.kind)` → `countsTowardUnread(m.kind)`。
  表现很具体：收到一条系统消息，前端 `unread +1`、预览被顶成那句话、会话还被排到最前，
  而后端 DB 里未读是 0 ⇒ 下一次 `refreshConversations` 数字又掉回去（用户看到"红点自己跳"）。
- `useChatStore` 的通知闸门同一处收敛：此前**手机/桌面会为一条系统消息弹系统通知**，
  而后端明写它不打扰。
- 收敛方向是"两处消费点共用一份判据"（`utils/messageKinds::countsTowardUnread`），
  不是各自再加一个 `|| kind !== "system"` —— 后者就是本仓一直禁的第二套判据。
  `messageKinds.ts` 里那段"已知漂移未收敛"的注释同步改成"已收敛 + 两处消费点名单"。
- 测试：`messages.test.ts` +2（系统消息混在真消息里只记真消息那条 / 整批都是系统消息时会话
  完全不动）、`storeContract.test.ts` +1 结构守卫（两处都必须用那份判据，且因为守卫先
  `stripComments`，注释里提函数名不会让它自证通过）。前端 571 → **574**。

## [4.29.14] - 2026-09-23

### Fixed (2026-09-23 稳定性审计 · A1 的 L2 第一段 —— 中继推送不再宣称未经证明的成功)

`relay_push_file`（`network/file.rs`）写完最后一片就无条件 `status=done, progress=1.0` 并广播
`file-done`。L1 只补上了"0 个邻居接住"那条 Err 出口，**成功出口仍然是无证据的断言**：循环跑完
只代表"每一片都被至少一个邻居接住"，邻居不保证与对端有直连，更没有"对端收全 + SHA 校验通过 +
落盘"的任何证据。命中 `docs/acceptance/1.0-release.md` 禁止事项两条 —— "不要因为 TCP write
成功就认为消息已送达"、"不要让界面显示成功和对端实际收到不一致"。

- 终态改 **`sent`**（新增值，含义"已写出、未获回执"），并**去掉 `file-done` 广播**、保留
  `file-progress`（后者说的是本机写出进度，属实）。留 `file-done` 不行的理由很具体：前端
  `onFileDone` 会把内存里那行写成 done ⇒ 直接造成"库里 sent、界面上 ✓"这条新分裂。
- 词表两处同步（`src-tauri/src/schema.sql:107` 与 `db.rs:435`，两份 DDL 必须逐字一致）。
  `file_transfers.status` 是裸 TEXT **无 CHECK** ⇒ 加值不需要迁移。
- **`sent` 必须是终态** —— 这是选它而不是"停在 active 等回执"的硬理由：A2 的回收
  `mark_transfer_failed_if_active` 只改 `status='active'`，所以 `sent` 不会被一小时回收扫成
  `failed`（那等于把"对方可能已收到"判成失败，用户于是去重推一份本已成功的文件）。
  DB 测试把三条边界一起钉住：`sent` 不被回收改写、`is_transfer_done` 不认 `sent`、
  第二次回收回报 false（不重复 emit）。
- 守卫：`relay_send_does_not_claim_unproven_success` 加两条断言（终态只能是 sent、体内不得出现
  `file-done` 事件名），`verify-guards.py` 加两条变异用例（tag `a1-l2`），两条都实测"改坏即
  FAIL、恢复即 PASS"；第二条注入刻意**只换事件名、载荷结构不动**，保证红的是判据而不是编译器。
- **未做的那一半，连理由一起记**：接收端回执（给 `FileCompleteAck` 加 `from`/`to` 走定向一跳
  中继 + `content_features()` 能力位门控 + 发送端有界等待，把 `sent` 升成 `done`）没做。
  实测依据：这条链在**发送方没有任何 UI 消费者** —— `chat.transfers` 全仓只有
  `useMessageFile.ts:36` 按消息 id 查用一处，而中继共享下载不产生发出方气泡
  （`send_file_via_relay` 只服务共享目录下载）。为一条没有消费者的状态新增协议帧，等于同时踩
  "不要新增未来特性"与"新帧必须能力位门控"两条红线，收益为零而协议面是实的。
  假成功已经去掉了；要真做出"对方已收到"，得连带发送方的可见界面一起做，那属新功能范围。

## [4.29.13] - 2026-09-23

### Fixed (2026-09-23 稳定性审计 阶段 4 · 批次 i —— `app.init()` 的注册配对)

- **`app.init()` 复用批次 h 的 `utils/initScope`**（清单 4.3-8 的现身）：一次 init 注册六类资源，
  此前**只有一条**有解绑 —— `settingsUnlisten?.()`，而且那两个变量是 setup 级的，
  `acceptHMRUpdate` 换掉整个 store 实例后连句柄都丢了，等于没有守卫。
  → 现在六类全部配对：`onSettingsChanged` / `onRuntimeChanged` 的 unlisten、系统外观 mq 监听、
  键盘高度的 `visualViewport` resize+scroll、移动布局的 mq+window resize、
  以及两个兜底定时器（2s `ensureBluetoothOn`、500ms `refreshRuntime`）。
  `watchSystemAppearance()` / `watchKeyboard()` 改为**返回卸载函数**，`settingsUnlisten` /
  `runtimeUnlisten` 两个变量删除（守卫里钉住"那半套不许复活"）。
  症状不炸但真实：跟随系统时切一次系统外观，`persistSettings` 会被连写 N 次 IPC。
- 测试：`storeContract.test.ts` +1 条结构守卫（模块作用域声明 / dispose 早于注册 / 六类注册各自配对 /
  两个 `watch*` 必须真返回卸载函数 / add-remove 数量相等）。6 个变异逐条验过只红这一条。前端 570 → **571**。

### 复核结论（清单 §4.3 P3 八条 → 只有一条是真的）

清单行号基于 2026-09-22 前的代码，逐条重跑原文后：**六条已不成立或已被既有设计覆盖**，
记在这里免得下一个人去修不存在的 bug。

| 条目 | 复核结果 |
|---|---|
| 4.3-1 `locateMessage` 的 from/to 用未过滤索引 | **形状已消失**：`locateMessageInConv` 不再算下标，只写 `locateRequest`，落点由 `VirtualList` 按 `itemKey` 从**真实 DOM 位置**校正 |
| 4.3-2 回前台不重建会话数据 | 监听配对已在批次 h 收口；"回前台补拉全量"属**新增行为**且与"不新增功能"冲突，判为不做（现有兜底：5s 拓扑轮询 + `pending` 批次强制冲刷 + 补发已读） |
| 4.3-3 `pendingScrollTarget` 在 onActivated 不被消费 | **符号全仓不存在**（`grep` 零命中）；跳转已由 `pendingJump` 的"重试直到落定 + 1500ms 上界"接管 |
| 4.3-4 连续 `loadMore` 的 200ms 节流对超长消息无效 | **没有 200ms 节流**；真正的闸门是 store 里 `loadingMore` 的单飞（批次 e 修成"合并进在飞的那次"）+ `MAX_PAGES` 上界 |
| 4.3-5 `resetInflight` 永不复位 | **符号不存在**：整套 `settingsDirty`/`lastLocalWriteAt`/grace 状态机已删 —— 后端 `emit_filter` 不把 `settings-changed` 回发给发起窗口，本窗口根本收不到自己写的变更（见 `useAppStore.ts` 该处注释） |
| 4.3-6 用户上滚中断自动跟随后不复位 | **已被设计覆盖**：`scrollToBottom()` 显式 `pinned = true` 并 emit `nearBottom`，注释写明"显式贴底 = 贴底意图" |
| 4.3-7 `saveJson` 的在途去重丢弃用户新改动 | **符号不存在**：设置落库改走 `persistSoon = debounce(persistSettings, 300)`，而 `persistSettings` 在**执行时**读 store ⇒ 后写那次必是最新值，另有两个 flush 出口兜住退出 |
| 4.3-8 主题 watcher 只增不减 | **成立**（换了位置：`watchSystemAppearance()` 在 init 里，每次 init 加一份），本批已修 |

## [4.29.12] - 2026-09-23

### Fixed (2026-09-23 稳定性审计 阶段 4 · 批次 h —— `chat.init()` 的重复初始化)

清单 4.2-2：`chat.init()` 没有重复初始化守卫。它的注册一次覆盖**四类资源**
（`bindEvents` 的 30 条 `listen`、通知插件的 `onAction` 回调、5s 拓扑 `setInterval`、
`visibilitychange` 的匿名 handler），此前**一条都没有卸载路径**。

单次冷启动看不出来，第二次 init 才炸：`App.vue` 的 `onMounted` 会再跑一次（Pinia
`acceptHMRUpdate` 换掉整个 store 实例，新实例的 setup 里没有任何"上一轮注册了什么"的痕迹），
于是每个事件回调跑**两遍**，而第一遍的闭包绑的是一份已经被丢弃的 state —— 表现是消息重复入账、
系统通知翻倍、给早已切走的会话补发已读回执（等于替用户判了已读）。开发期每次热替换都会摊上，
所以这类"怎么又多弹一条通知"的问题此前只能靠重启压掉。

- 新增 `src/utils/initScope.ts`：`onDispose(fn)` / `dispose()` 两个方法，逆序拆（后注册的先拆）、
  一个卸载器抛错不牵连剩下的、重复 `dispose()` 只有效果一次。
  **关键语义是"迟到的注册就地执行"**：`listen()` 的卸载函数要等一次 IPC 往返才拿得到，
  那时本轮可能已经被下一轮拆掉了 —— 攒进一个已废弃的列表等于永远没人调用它，监听就真的留下了。
- 守卫句柄 `chatInitScope` 放在**模块作用域**（不是 store 状态），理由见上：换实例后只有模块级
  变量还能看见上一轮。`init()` 第一件事就是 `chatInitScope?.dispose()`，排在任何注册之前
  （否则本轮刚注册的东西会被自己拆掉）。
- 顺带纠正一条真实缺陷：`void onAction(...)` 把返回的 `PluginListener` 直接丢掉了 ——
  它不是 `UnlistenFn`，必须显式 `unregister()`。移动端的通知点击回调此前**摘不下来**。
  注册失败也补了留痕（原来是裸的 fire-and-forget，静默吞掉 = 点通知不跳转但毫无线索）。
- `markRead` 去抖里排着的那次定时器一并纳入卸载：不然废弃实例会在 300ms 后把已读回执发出去。
- 测试：`initScope.test.ts` 5 条 + `storeContract.test.ts` 结构守卫 1 条（钉住"模块作用域声明"
  "dispose 早于注册""四类注册各自配对""不许回到 `void onAction`""add/remove 数量相等"）。
  6 个变异逐条验过只红这一条守卫，非空转。前端测试 564 → **570**。

**已知限制（不强行修）**：卸载是异步 IPC，`dispose()` 返回到监听真正摘掉之间有一个往返窗口，
期间到达的事件仍会喂给旧闭包。这一窗口只会多算一次入账，且 `markRead` 后端幂等，
不值得为它引入"事件序号 + 丢弃旧轮事件"的第二套机制。

## [4.29.11] - 2026-09-23

### Fixed (2026-09-23 稳定性审计 阶段 4 · 批次 g —— 通知链路、发起窗口快照、图片重试、定时器)

- **通知整批凭空消失**（清单 4.2-3）：`flushNotifications` 是**先 `notifyQueue.clear()` 再
  `await` 权限**，而那条链没有 `catch` ⇒ `isPermissionGranted()` / `requestPermission()` 任一
  IPC reject，这批通知就没了（用户少收一条、毫无线索，还附赠一条 unhandled rejection）。
  → 补 `.catch`：**退回队列**等下一批重试。放回是安全的——失败点在任何一条通知**发出之前**，
  不会重复提醒。合并放回用新的纯函数 `mergeNoticesInto`（同会话累加条数、`last` 取更新的那条），
  否则放回的一批会把窗口期内新到的消息覆盖成旧的，正文与点击跳转都指错。
- **权限被拒后每个批次重问两次**（清单 4.2-3 后半）：缓存原先是 `boolean`，"没问过"和"被拒了"
  同为 `false` ⇒ 每 1.5s 的通知批次都跑 `isPermissionGranted` + `requestPermission` 两次 IPC。
  → 改三态 `boolean | null`；`null`=没问过、`false`=问过且被拒（短路）、`true`=已授权。
  **`setNotifyEnabled` 打开时强制重问**（`force`），否则"上次被拒、后来在系统设置里放开"会被
  缓存永远挡死。清单说的"每批再弹一次授权框"按平台降级：macOS/Windows 对已拒过的应用通常直接
  返回 denied 不再弹框，所以反复付出的是 IPC 与噪声，不一定是对话框。
- **发起窗口自己的运行状态停更**（清单 4.2-4）：`startNetwork` / `stopNetwork` 都返回新的
  `RuntimeSnapshot`，而后端 `notify_runtime_changed` **刻意不回发给发起窗口**（它在返回值里
  已经拿到了）。前端把返回值丢掉 ⇒ 本窗口 `present`/`runtime` 不再更新，表现是
  "局域网开关都关了，头像还显示在线"。→ 两处都 `applyRuntimeSnapshot(await ...)`，
  并把 `runtime`/`present` 加进失败回滚的快照（原先回滚漏了它们）。
- **图片重试结构性必然失败**（清单 4.2-11，比描述更糟）：`effectiveSrc` 给 `props.src`
  拼 `?r=N` 来"强制重取"，而这里的 src 只有 `blob:`（objectURL）和 `data:` 两种形态，
  两类都不接受 query ⇒ 退避的 5 次重试**每一次都打在无效地址上**，即使文件早就在本机也
  必然停在「图片加载失败」，手动点击重试同样无效。再加评审补的一刀：`watch(props.src)`
  只重置 `attempt/state`，`loadKey` 从不归零 ⇒ 换了一张新图还带着上一代的 `?r=6`。
  → 彻底去掉查询串，改用 `:key="loadKey"` 换 `<img>` 元素（与 URL 形态无关的强制重取），
  并在换图时把计数归零。
- **辅助窗口初始化失败会吞掉补救注册**（清单 4.2-8）：`mountAuxWindow` 是
  `try { await beforeMount() } finally { mount }`，异常继续外抛 ⇒ 调用方
  `.then(() => 注册焦点刷新)` 整段被跳过。偏偏"焦点刷新"就是那个窗口的自我修复通道，
  于是设置窗口拿不到网卡/共享目录后再也不重拉、任务窗口空列表且无实时监听。
  → 失败时**仍然挂载**（不给用户白窗）、**仍然上报**（走既有 `log_frontend_error`，
  不藏错误），但把 `{ok:false,error}` 交回调用方，让补救注册照样执行。
- **两个组件的定时器只登记不注销**（清单 4.2-15 的一部分）：`AddFriendModal` 的冷却
  `setTimeout`（无 `onUnmounted`）、`LogViewer` 的「已复制」与「再点一次确认清空」两个。
  如实说严重度：**不是可见 bug**（回调写的是已销毁实例的 ref），属于"只登记不注销"这一类
  清理不彻底，顺手收口。

### 复核纠正（清单/评审里有两条不成立）

- **"移动端 `void sendNotification({...})` 没有 catch" 不成立**：插件这个 API 是
  fire-and-forget（返回 `void`，不是 Promise），没有可 catch 的失败信号 —— 我按它写了
  `.catch()` 被 `vue-tsc` 当场拒绝（TS2339），已撤销并把这条事实写进代码注释。真正会
  reject 的是上面那次权限查询。
- **"清单 4.2-12 `VirtualList` 的 key 回退成 index" 前提不可达**：唯一消费者的列表元素类型
  是 `MessageRecord`，`msg_id` 在前端类型与 `schema.sql:36`（`UNIQUE NOT NULL`）里都是必填，
  自造的乐观/占位记录也恒带 `tmp-*` id ⇒ 两条兜底分支都取不到。判为不修（记录成因），
  避免下一个人照着清单去"修一个不存在的 bug"。

### Tests (阶段 4 · 批次 g)

- `notifications.test.ts` +5（回队合并的四条语义 + 空批次不动队列）。
- `storeContract.test.ts` +4 条形状守卫：出队与发出之间必须有退回队列的 catch；权限缓存必须
  三态且开关走 `force`；两个 network 命令的返回值必须被 `applyRuntimeSnapshot` 消费；
  `effectiveSrc`/查询串重试禁止复现 + `:key="loadKey"` 与归零必须在。
- 前端测试 555 → 564；`vue-tsc` 0；`npm run build` 通过；快速层 9 步全绿。

### 已知限制（批次 g）

- todos 独立窗口在 `loadGroupTodos` 失败后仍是"空列表 + 无实时监听"，且**没有重试入口**：
  本批只保证了"该注册的一定会注册"，给它加焦点重试属新交互设计，没在真机上验过不做。
- `clearAllData` 之外的那条 `setNotifyEnabled` 路径不改权限缓存的三态初值；用户在系统里撤销
  授权后本机要等下一次 `force`（点开关）才会重新感知 —— 与改前一致，不是本批引入。

## [4.29.10] - 2026-09-23

### Fixed (2026-09-23 稳定性审计 阶段 4 · 批次 f —— 「后发先至」一族)

同一条不变量的四处分身：`x.value = await api.foo()`。IPC 没有顺序保证 ⇒ 先发起的请求可以
后回来，用旧快照覆盖新状态。本批收成一份实现（`src/utils/staleGuard.ts` 的
`StaleGuard.begin/isCurrent`，令牌按**被写的状态**分 key），逐个接入并加结构守卫。

- **store 的 8 个 refresher**（清单 4.2）：`refreshPeers` / `searchNearbyPeers`（与前者**共用
  `peers` 这个 key**，因为写的是同一个状态，必须互相作废）/ `refreshFriends` / `refreshPending` /
  `refreshConversations` / `refreshGroups` / `refreshTopology` / `refreshFavorites`。
  调用点大量是 `void refreshX()`（每个群操作成对调两个、好友申请通过、gossip 更新…），
  所以并发是常态。可感知的后果都是真的：`refreshConversations` 的旧快照带着**乐观清零之前**的
  `unread` ⇒ 红点自己亮回来、列表顺序回退；`refreshFavorites` 两条来源（消息菜单 / 收藏面板）
  互相覆盖 ⇒ "点了星号又没了"；`refreshGroups` 的群读名单是第二次 await 之后才写 ⇒ 已退群成员
  的绿勾被写回来（所以那个函数里过了**两次**闸）。
- **`refreshTransfers` 从自带的 `transfersReqSeq` 迁入同一份实现**：它早就为这个坑单独修过
  一次（旧快照里没有刚建的 transfer ⇒ 进度条永久钉在 0%），但那份计数是**第二套真相源**；
  迁移后同族只剩一处实现。
- **`ImageLightbox.resolveCurrent`**（清单 4.2-9）：连按方向键翻图时，缓存命中的那次同步返回、
  未命中的那次跨 IPC ⇒ 先发起的后回来会把当前这张覆盖成上一张（错图）或裂图。
  现在所有落地（含同步分支与"没有图就清空"那条）都过同一道闸 —— 旧形状 `apply(await ...)`
  被结构守卫禁掉。
- **`ShareDirectory`**（清单 4.2-10）：两半都修。① `load()` 无过期守卫 ⇒ 旧会话的目录树回填进
  新会话的面板；② **更要紧的是** `download()` 在点击那一刻才读 `friendId()` ⇒ 面板开着时
  `activeConv` 会被程序化路径换掉（点系统通知 → `openConversation`），于是"A 的树里的一行"
  发给 B 去校验路径：轻则莫名"下载失败"，**重则拿到 B 上同名的另一个文件**。
  现在树只认自己那份（`loadedFor`），下载必须用它；面板开着时换会话会重拉。
- **`MessageItem.deliverySummary`**（清单 4.2-13）：watch 源有两个（msg_id 与 status），
  列表回收复用实例或快速翻状态时旧请求后到 ⇒ 群文件投递读数停在旧值；而它的 `catch` 会把
  **更新那次刚写好的值抹成 null**（绿勾群读数一闪之后整块消失）。catch 也过闸。

### Tests (阶段 4 · 批次 f)

- `staleGuard.test.ts` 5 条：含两条**真实交错**用例（手工可控 promise 制造"先发起的后回来"，
  断言新值不被覆盖；另一条断言被丢弃的那次连 `error`/`loading` 这些副作用都不执行）。
- 结构守卫 2 条：① `useChatStore.ts` 里**不允许再出现** `x.value = await api.…` 这一形状
  （判据先剥注释，避免"注释里写了这个形状"把守卫自己变红）；② 三个组件各自的
  "必须有过闸调用 + 旧的直写形状必须不存在"。
- 前端测试 548 → 555；`vue-tsc` 0；`npm run build` 通过；快速层 9 步全绿。

### 已知限制（批次 f，明确不修 / 待扩范围）

- **`useAppStore` 还有 4 处同形状**（`device` / `shareDir` / `interfaces` / `updateProfile` 的
  返回值直写）。多数是一次性初始化写、并发面与本批不同，本批**没有**顺手改；守卫的报错文案里
  写明了这一点，收敛它们时把判据范围一起扩过去，别只改代码不收守卫。
- `StaleGuard` **刻意不提供 `forget(key)`**：删掉计数会让下一次 `begin` 从 1 重来，而一个仍在飞
  的旧请求手里的令牌可能恰好就是 1 ⇒ 它反而被判成"最新的一次"，把旧快照写回来。本工具的所有
  key 都是固定小集合，不清理也不会增长。
- 令牌只回答"我是不是最新一次"，不回答"数据是否变化"：所以调用点的**所有**副作用
  （写结果、写 error、`finally` 里清 loading）都必须在闸之后 —— 漏一个就等于那个副作用仍被
  旧请求执行。这条写进了 util 的文档边界。

## [4.29.9] - 2026-09-23

### Fixed (2026-09-23 稳定性审计 阶段 4 · 批次 e —— 独立评审回修批次 a/d 自己引入的问题)

对 `3ab31be..HEAD` 做只读评审后确认：**四处"修 A 引入 B"，其中两处是用户可感知的回归**，全部回修。

- **翻页单飞闸把定位链路弹回成"没有更早历史"（批次 a 引入，严重）**：`locateMessage`（
  `ChatWindow.vue`）与 `locateMessageInConv`（store）的循环写的是
  `await loadMoreMessages()` → "长度没变 ⇒ 翻到头了"。而新加的 `loadingMore` 闸门在有人in飞时
  **直接 return**，一次弹回就被判定"原消息在更早的历史里"并拒绝定位 —— 点引用/搜索命中会误报。
  旧代码两次并发各自拉页，定位那一次总能前进。
  → 单飞改成**并入在飞的那一次**（`if (running) return running`），后来者与首发者拿同一个
  Promise；摘除挂在 `.finally` 链上，保证 IPC 抛错时闸门一定打开。
- **`historyTops` 的失效点位置错（批次 a 引入，严重）**：原来只在 `loadMessages` **成功路径末尾**
  清，而 IPC 失败走 catch 提前 return、seq 被后来者抢走也提前 return —— 那些路径下内存列表
  同样只剩最新一页，`historyTops` 若还留着 true，该会话的翻页就被**永久挡死**，
  而 `loadMoreMessages` 恰是那种"空列表"下唯一的自愈通道。
  → 作废点前移到 `loadMessages` 函数开头（与 `loadSeqs` 同帧、在任何 await 之前）；
  缓存淘汰（`enforceMessageCacheBound`）也一并清这本账。**刻意不**跟着删 `loadingMore`：
  那条 Promise 自己会落地摘除，在淘汰点删反而会让新发起的一次与仍在飞的一次并行拉页。
- **prepend 未读位移没扣 `mergeMessages` 的去重（批次 a 残留，中等）**：位移量原本是
  "这一页里会渲染的条数"，但 `mergeMessages` 按 msg_id 去重，而**重叠是常态**——
  会话总数落在 101~199 时第二页请求的 `offset` 仍是 0，整页与内存里的最新一页大面积重叠；
  翻页期间来了新消息同样重叠。后果是分割线落到真锚点**下方**（部分真未读看起来像已读）。
  → 只数"真正新插进列表、且会渲染"的行。有界性也说明了：每页至多多算一次，不逐页累积。
- 评审顺带确认**不影响既有功能**的几处（记录判据来源，避免以后又被怀疑）：`unreadJump.index`
  五个消费点现在全在同一个渲染坐标系；`-1` 占位语义未被破坏；三条返回路径（头部箭头 /
  Android 系统返回 / 通知反向）都会走到新加的 `mobileView` watcher；`send-image` 全仓单一
  emit + 单一监听，`File` 的读取与旧代码在同一个事件任务内启动（`clipboardData` 的
  "必须同步取"约束仍然满足）；8 MiB 与后端 `MAX_OUTGOING_IMAGE_BYTES` 同值且同为解码后口径；
  `bytesToBase64` 与旧实现逐字符等价（含 8191/8192/8193 分块边界与 0 长度）；本仓图片源只有
  `blob:` 与 `data:`，新加的 `!res.ok` 分支实际不可达、不构成误伤；`pendingAcks`(512) 与
  `notifMap`(128) 淘汰后无用户可见后果（前者消费在同一调用内，后者有 `extra.conv_id` 兜底）。

### 已知限制（阶段 4 · 批次 e，明确不修）

- **"未读全是 `announcement`/`poll` 时分割线整体消失"**：这两类计未读但不进时间线，新锚点
  数不出任何行 ⇒ 返回 -1 ⇒ 不画、交给贴底兜底。方向是"宁可不画也不画错"，但这是**行为漂移**
  而不是纯坐标修正；要两全得让后端按可见性给数（协议改动）。
- `openMerge`（合并卡片详情）与 `announceDeleteArmed`（公告两段式删除确认）切会话时仍不收尾：
  前者载荷是内容快照不会串会话数据，后者影响面是"下次打开公告全文直接落在第二段确认"。窄，记 TODO。
- 桌面端多选进行中切走左侧 rail ⇒ ChatWindow 卸载但 `app.multiSelectActive` 残留
  （卸载路径不调 `exitMultiSelect`）：预存在，非本批引入，只影响移动端判据。
- 旧格式 `data:` 内容若含空白，新短路会把空白直传给后端的严格 base64 引擎（旧路径经浏览器
  解析会剥掉）；未找到 `FileReader` 会产出空白的证据，判为理论风险。
- 后端"图片过大，请压缩后重试"的原文在 UI 上只在 `toastError` 展开 `e.message` 时可见，
  统一文案是 `msg.sendFailed`；可接受。

### Tests (阶段 4 · 批次 e)

- `storeContract.test.ts` 加 3 条结构守卫，把上面三条回归钉死：单飞必须 `return running` 且
  必须挂 `.finally`；`historyTops.delete` 必须排在 `loadMessages` 第一个 await 之前；
  prepend 位移必须用"未去重且会渲染"的判据。三条各自对应一段"改回旧写法必红"的形状。
- 前端测试 545 → 548；`vue-tsc --noEmit` 0；`npm run build` 通过。

## [4.29.8] - 2026-09-23

### Fixed (2026-09-23 稳定性审计 阶段 4 · UI/交互 批次 a —— 未读定位与历史翻页)

清单 §4.1 的 7 条 P1 逐条重跑原文复核后：4 条**仍成立**、3 条与描述不符（按复核结论改）。
本批是其中两条，同属"打开会话 → 定位/翻页"这一条链。

- **4.1-1 未读分割线的坐标系**（`useChatStore.ts` + `ChatWindow.vue` + `utils/`）：锚点下标在
  `messages[convId]` **原始列表**里算，却在 `ChatWindow` **过滤后列表**里当偏移用。两个集合的
  差异是**双向**的 —— 静默行与 `system` 占下标却不占未读，`announcement`/`poll` 占未读却根本不
  显示 ⇒ "以下是未读消息"画在错的那条消息上、`scrollToIndex` 跳错位置，且群里有表情/置顶时
  偏移常非零。**第二处实例**：prepend 历史后按 `older.length` 平移锚点，而老页里的静默行不进
  渲染列表 ⇒ 每翻一页误差再累积一次。
  修法：判据收敛成 `isRenderedInTimeline()`（`ChatWindow` 的过滤与 store 的换算共用一份），
  锚点计算抽成纯函数 `unreadAnchorIndex(renderedList, unread)`：从末尾数第 N 条
  **既渲染又计未读**的行，数不完则 clamp 到最早的计未读行（不是粗暴的 0 —— 首行可能是 system），
  一行都数不出则返回 -1（宁可不画）。`countsTowardUnread()` 显式对齐 Rust
  `is_non_notifying_kind`（`protocol.rs:377`）。
  残余误差已写进注释、刻意不在本次解决：未读含"不渲染的 card"时分割线会偏上几行，
  方向单一且不超过 `unread`，比原来"两个方向随机偏"安全；要精确需后端按可见性给数。
- **4.1-2 历史翻页的每帧 IPC 与并发双拉**：`VirtualList` 的 `scrollTop < 60` 是**纯状态判断不是
  边沿触发**，而 `computeScrollState()` 有 6 个入口（滚动 / resize / applyJump 轮询 /
  `scrollToIndex` / items 变化 / 总高变化）⇒ `loadMore` 被每帧调用。store 侧只有同步的页数判据，
  "已翻满"那条在 IPC **之后**才判 ⇒ 已翻到顶但不足 10 页的会话（绝大多数）每帧白付一次
  `getMessageCount`。更要紧的是并发的两次 `loadMore` 互相判不出过期：都过得了判据、各自
  `getMessages` 拉一页，却把 `pagesLoaded` 写成同一个值 ⇒ **前进一页、拉了两页数据**
  （不只是多付 IPC，是正确性问题）。
  修法：`loadMoreMessages` 加两道闸 —— `loadingMore`（同会话同时只允许一个在飞，`finally`
  必放行，否则就是"翻一次之后再也翻不动"）与 `historyTops`（已知到顶的会话不再付 IPC；
  ⚠️ 必须在每次 `loadMessages` 重新加载时作废，否则"看过的会话重开后翻不动历史"）。
  **刻意没有**改成边沿触发：那会让用户停在顶部时只翻得出一页、必须"往下滚一点再滚回来"
  才能继续 —— 拿一个真实可用的行为去换噪声，不划算（成因与取舍写进代码注释）。
- 复核纠正（记录，不改）：`applyIncomingToConversations` 的前端"不计未读"判据只滤
  `isSilentKind`、**漏了 `system`**，与后端 `is_non_notifying_kind` 不一致 ⇒ 本地系统消息会让
  前端未读 +1 而后端不记。不在这批顺手改：要先证明哪条路径真会走到这里、以及收敛后的期望
  行为，已列为 §4.2 的独立条目。

### Tests (阶段 4 · 批次 a)

- `unreadAnchorIndex` 6 条纯函数用例（`system` 不消耗额度 / clamp 到最早的计未读行而非 0 /
  全 system 返回 -1 / 空列表与 `unread<=0` / 静默行被误传进来也不参与换算）。
  第 2、3 条正是"旧公式 `len - min(unread,len)`"会红的那两种形状。
- 前端测试 525 → 531；`vue-tsc --noEmit` 0；快速层 9 步全绿。
- 已知取证缺口：`loadMoreMessages` 的并发与 `historyTops` 时序**没有**运行时守卫 ——
  本仓没有能驱动 store 异步竞态的夹具（与阶段 1.4 的 `loadSeqs` 同一处境），判据与后果写在
  代码注释里；能在浏览器外证的部分（不重复 IPC、不双拉页）依赖这两道闸本身。

### Fixed (2026-09-23 稳定性审计 阶段 4 · UI/交互 批次 b —— 会话级状态泄漏)

- **4.1-3 `refreshLinkState` 缺过期守卫**：`getConvLink` 是 IPC，回来后无条件写进共享的
  `linkState` ⇒ 快速切会话时**上一个对端的链路状态被显示在当前聊天头**上（"连着 Wi-Fi 却
  显示蓝牙/中继"），与同文件注释立的契约"提示必须和现在这条链路一致"直接冲突。
  → 回填前核对 `id !== chat.activeConv` 则丢弃。
- **4.1-4 移动端多选中返回会话列表 → 底部 TabBar 永久消失**：`app.multiSelectActive` 只有
  ChatWindow 的 `enter/exitMultiSelect` 两个写点，而"返回"是**布局层改 `app.mobileView`** 的导航
  动作 —— 它既不卸载 ChatWindow（挂载条件只看 `navState === 'chats' && activeConv`），也不触发
  `activeConv` 的 watcher，于是标志一直挂着、TabBar 一直隐藏，而 TabBar 恰是唯一的导航出口
  （多选操作条在被平移出屏的面板内，列表页看不到也点不到）⇒ 事实上不可自愈。
  → 在 ChatWindow 里 watch `mobileView`：离开聊天视图就退出多选（与"切会话退多选"同一口径）。
  **刻意没有**改 TabBar 的判据（把 `multiSelectActive` 限定在聊天视图）—— 它当初为什么被算进
  显示条件我没有取证，动它等于顺手改别处的既有意图。
- **4.1-7 切会话不清会话级浮层**：`activeConv` 的 watcher 只清了 quote / forward / 多选
  （那三样是"会把内容**发错**会话"才修的），而 `membersOpen`/`filesOpen`/`tasksOpen`/
  `lightboxOpen`/`announceViewOpen` 全都不收尾。可达路径不是"点列表"（BaseModal 会模态阻断），
  而是**点系统通知**：`useChatStore.handleNotificationClick` 会程序化 `openConversation(B)` 而
  不通知任何浮层 ⇒ "挂着 A 群的面板、标题是 B"。`GroupFilesPanel` 还把自己的清单缓存进
  `files` ref 且**只 watch `open` 不 watch `groupId`**，所以换群后整张清单仍是上一个群的。
  → 五个标志进同一个 watcher；`GroupFilesPanel` 补 `watch(groupId)`（清空 + 重拉），
  并给 `load()` 加慢响应过期判据 —— 只补 watch 不补这里，切群瞬间的旧响应仍会回填。
  **复核纠正**：清单说"refetch 会打到 B 群"**不成立** —— `refetch`/`openFile` 全部从行数据自取
  `f.sender_id` / `f.transfer_id`，不读 `props.groupId`；错的是"展示"，不是"发请求"。
- 复核新发现（**记为 §4.2 独立条目，本批不改**）：前端 `applyIncomingToConversations` 的
  "不计未读"判据只滤 `isSilentKind`、**漏了 `system`**，与后端 `is_non_notifying_kind`
  （`protocol.rs:377`，= 静类 `|| system`）不一致 ⇒ 本地系统消息会让前端未读 +1 而后端不记，
  两边从此对不上。不改的理由：要先证明哪条路径真会走到这里、以及收敛后用户期望的行为。

### Tests (阶段 4 · 批次 b)

- `storeContract.test.ts` 加 2 条结构性守卫（本文件就是"读源码断结构契约"的既有先例）：
  ① 切会话必须清那五个浮层标志、离开聊天视图必须退多选；② 跨 IPC 的回填必须核对
  "数据还是不是当前会话/群"（`refreshLinkState` 与 `GroupFilesPanel.load` 各一处）。
  判据一律取**代码形状**（`x.value = false` / `if (id !== chat.activeConv) return`），
  且逐条 grep 确认没落在注释行里 —— 本文件读的是含注释的原始源码，拿文案当判据会变成
  "注释替代码通过"（阶段 3 的 A5 变异用例就是这么骗过一次自己的）。
  守卫自身也红过一次：区间上限拿成"到文件尾"导致恒判失败，已改为按函数结束边界切。
- 前端测试 531 → 533；`vue-tsc` 0；`npm run build` 通过；快速层 9 步全绿。

### Fixed (2026-09-23 稳定性审计 阶段 4 · UI/交互 批次 c —— 图片两条路径)

- **4.1-5 粘贴图片**（`MessageComposer.vue` + `ChatWindow.vue` + 新 `utils/imageBytes.ts`）：
  - **发错会话（成因与清单不同）**：Composer 里 `emit("send-image", await fileToDataUrl(f))`
    —— `await` 在 emit **之前**，所以 ChatWindow 只能在"文件读完之后"才决定发给谁；
    粘贴一张大图的那几百毫秒里用户切了会话，图片就发进了**切到之后**的那个会话。
    → 改成 Composer **同步 emit `File` 本身**、读取挪到 ChatWindow，并在函数第一行就
    `const convId = chat.activeConv` 捕获。这同时保住另一条约束：`imageFile` 必须在任何
    await 之前从 `clipboardData` 取出（Chromium/WebKit 在 paste 事件返回后清空它），
    所以捕获点只能在 Composer 内、异步工作只能在被调方做。
  - **超限图片先把内存打满**：后端其实**有** 8 MiB 硬上限（`commands.rs:41`），但它在
    `base64::decode` **之后**才比长度 ⇒ 一张超限图会先在 JS 堆整读成 data URL、再作为 JSON
    字符串跨 IPC、再在 Rust 侧解回 `Vec<u8>`，**三端各分配一份**之后才被拒绝。
    Android 的 ART 堆通常只有 256MB（同仓注释记载：5 张图一起发直接 FATAL OOM）。
    → 前端加一道同数值的前置闸；`MAX_PASTED_IMAGE_BYTES` 与 Rust 常量由**契约测试比对**
    （不比对就是"改了那边忘了这边"的标准剧本）。
  - **失败静默**：`onSendImage` 全程无 catch，而 `sendImage` 把 `save_outgoingImage` 留在
    自己的 try **之外** ⇒ 超上限 / 非法 MIME / 解码失败三类 reject 一路无人接手，
    用户看到的是"按了 Ctrl+V 什么也没发生"。→ ChatWindow 侧 try/catch + `toastError`。
- **4.1-6 另存图片**（`MessageItem.vue` + `ImageLightbox.vue` → `utils/imageBytes.ts`）：
  两处**逐字重复**的 7 行（`fetch` → `arrayBuffer` → `binary += String.fromCharCode(...)` →
  `btoa`）同时持有约 4 份文件内容。改成共用一份工具，并拿掉其中两份浪费：
  ① 源本身是 data URL 时**直接摘出 base64 段** —— 旧写法等于把 base64 解码成字节再编码回
  **同一个字符串**，纯属白做；② 分块编码后一次 `join`，而不是 `+=` 每 32 KiB 重建整个字符串。
  ⚠️ `dataUrlBase64()` 只认 `;base64,` 那一种：不带该标记的 data URL 是**百分号编码原文**，
  直接切尾巴会产出"看着像 base64、解出来是乱码"的坏文件，而后端只会照单解码写盘 —— 错得很安静。
  **已知取舍（写进模块头注释）**：正解是 `tauri-plugin-fs` 的 `writeFile(path, Uint8Array)`
  走二进制、彻底不产 base64。Rust 侧其实已注册（`Cargo.toml:33` + `lib.rs:153`），但前端包
  `@tauri-apps/plugin-fs` 不在依赖里、装上还要改 `capabilities` 的 fs scope —— 发版收尾阶段
  引新依赖 + 扩权限面的风险大于收益，所以本批只收敛浪费、不改传输形状。

### Tests (阶段 4 · 批次 c)

- 新增 `src/utils/imageBytes.test.ts`（6 条）：分块边界的 8191/8192/8193 三个长度是刻意挑的
  （分块写错只会在那一个长度上错，随机大样本反而容易盖过去）；填充位单独一条；
  百分号编码 data URL 必须返回 null；以及与 Rust `MAX_OUTGOING_IMAGE_BYTES` 的跨语言比对。
- 踩到并修掉一处"运气测试"：原本有一条用 `urlToBase64("data:text/plain,abc")` 断言 reject，
  其结果取决于 Node 的 `fetch` 是否支持 `data:` URL ⇒ 换成只断言"决策"而不真去 fetch。
- **清单守卫立了一功**：新增 `.test.ts` 后 `check-test-manifest` 直接报"它不会被执行，而
  `npm test` 依然全绿"，并要求手工登记进 `package.json` 的 `scripts.test`。
  这正是该守卫存在的理由（与"漏 `--features`"同类：退出码 0 的空转）。
- 前端测试 533 → 539（55 个测试文件全部已登记）；`vue-tsc` 0；`npm run build` 通过；快速层 9 步绿。
- 顺带记录（**未改**）：同文件里 `paste-files` 分支的 emit 也发生在 `await invoke(...)` 之后，
  属同一形状的窄窗口（一次 IPC 而非整文件读取）；已列入 §4.2 待办，不在本批夹带。

### Fixed (2026-09-23 稳定性审计 阶段 4 · 批次 d —— 只增不减的模块级集合)

复核 §4.2 十四条的过程中发现：泄漏不是个别现象，而是同一形状的四处。
新增 `src/utils/bounded.ts` 的 `trimOldest(coll, max)`（Map/Set 的迭代序即插入序 ⇒ FIFO），
四处共用一份；淘汰的都是最早写入、早就不在屏上的条目，在屏的最近测量必然活着。

- **`pendingAcks`**（上限 512）：消费点只有 `send()` 一处，而它的"是否已存在"判据只扫
  `messages.value`（LRU 保留 4 个会话）⇒「会话被淘汰 / 文件回执 / outbox 补投」这三类 Ack
  永远命中不了消费点，条目只进不出。上闸只清本来就没人要的，真删除仍由 `send()` 做。
- **`notifMap`**（上限 128）：删除点只有"通知被点击"与"动作按钮命中"两处 ⇒ 用户直接把通知
  划掉时条目永久留着，而 `notifSeq` 单调递增，**每条通知必进一份**（比 `pendingAcks` 更确定）。
- **`heightOverride`**（上限 2000，`VirtualList.vue`）：每实测过一条消息高度留一项，
  store 收缩会话缓存时清掉了 `pagesLoaded/loadSeqs` 却不清这里 ⇒ 长时间滚动的会话单调增长。
- **两条清除路径同口径**：`clearAllData` 只清了 `pendingAcks`，而 `resetAfterDataCleared`
  谁都没清 ⇒ 两条路径走出两种残留。现在两边一起清 `pendingAcks` / `pendingReplace` / `notifMap`。

### 已知限制（阶段 4 · 批次 d，明确记录不修的理由）

- **`filePreview` / `favoritePreview` 的模块级缓存不加容量闸**：它握的是 **objectURL**，
  而 `filePreview.ts:7-12` 钉着「objectURL 生命周期归缓存所有，消费者不得 revoke」
  （2026-09-21 真根因：同一个 cid/msg_id 的 URL 被任务卡、看板表单、任务详情三处共用，
  任何一处 revoke 就把其它视图一起打回裂图，且缓存里那个死 URL 仍会被命中）。
  所以给它加"超容量淘汰"要么不 revoke（blob 内存根本没释放，闸白装），要么 revoke
  （重演那次事故）。**正确修法是引用计数，属设计变更**，不在稳定性清单里顺手做。

## [4.29.7] - 2026-09-23

### Fixed (审计 A1 · 中继发送端假成功 —— L1 那一半)

A1 用户已选**完整修复**，分两层落地。本次是 **L1：零协议变更**，只做"没有证据就不能判成功"里
不需要新帧的那一半；**L2（接收端回执 + 能力位 + 新终态 `sent`）尚未实施**，见下。

- **假成功的下限判据**：`relay_send_to_neighbors` 旧实现对每个邻居 `let _ = try_send(...)`，
  于是"一个邻居都没接住"与"全都接住"在调用方看来完全一样。现在返回**接住该帧的邻居数**，
  并把解释边界写进注释：`0 ⇒ 必然没送出` 才可用，`≥1` **不代表**送达（邻居未必与目标有直连）。
  `send_file_via_relay` 两处按此判定：Offer 无人接住 ⇒ **在读盘之前**就失败；任一分片无人接住
  ⇒ 当场中止（继续推只是把日志刷满、界面停在最后一个报过的百分比）。
- **发送方向的终态黑洞**：中继发送建的行写 `status='active'`，而**发送方向没有任何清扫器**
  （`sweep_stale_relay` 清的是接收侧那两张内存表，outbox 清扫器管的是 `file_outbox`）⇒
  取消 / 读盘失败 / 整体超时 / 分片失败**每一条 Err 路径**都留一行 active 在库里：
  界面当场报失败，重启后那条传输又回到"进行中 X%"并永久挂着。现在 `send_file_via_relay`
  做成"外层落终态 + 内层推流"的唯一出口，失败统一 `mark_transfer_failed_if_active`
  （已 done 的行不动，绝不改写既有终态），落库失败则 warn 留痕。
- **请求方的同名洞**：`download_shared_file` 走中继时发完就返回，此前**无条件**返回 Ok
  并插一条"你正在下载…"，邻居数为 0 时请求根本没离开本机。现在 0 个邻居接住直接报错误。
- **系统消息不再用完成时态**：`「X」下载了你的文件「y」` 插在推第一个分片**之前**，
  那一刻这条链上没有任何成功证据 ⇒ 改为「请求下载」。这是 A1 在用户侧最难看的一环
  （不是进度条骗人，而是一句已经下过的断言）。
- 刻意**不用**邻居计数的两处：`browse_share_dir` 与对端回目录树 —— 它们自己等 10s 响应，
  "对端真的回了"比"邻居接住了"是更强的证据，用弱判据反而会把正常慢链路误判成失败。

### Tests (A1-L1)

- 新增守卫 `relay_send_does_not_claim_unproven_success`（一个不变量一个守卫，5 条断言）+
  3 条 `verify-guards.py` 变异用例（`--only a1-l1` 实跑，三条全部"改坏即 FAIL、恢复即 PASS"）。
- 两条踩坑记录写进了代码注释：① 探针只数 `==0{` 会撞上同函数里无害的 `if size == 0`
  进度计算（实测第一次就跑出 3），已改成带 `.await`/`return Err` 的形状；② 曾经单列的
  "调用处数"断言被"判据处数"完全覆盖，留着只会让变异用例打在前一条上、拿到弱确认，已删。
- Rust lib 测试基线 659 → 660；`cargo clippy --features bluetooth -- -D warnings` 0；
  Android 门禁（默认 + `--features bluetooth`）0 warning。

### 已知限制（A1 剩余部分，L2 待实施）

- `≥1 个邻居接住` 仍然不是送达证明：**真送达要等接收端回执**。L2 的形状已定：
  新帧 `ShareFileAck`（复用 `ShareTreeResponse` 那套"有直连 try_send / 无直连借一跳中继回送"
  的对称结构）+ 能力位（走 `content_features()` 位图，不复用 `protocol_version` ——
  `protocol.rs:92` 的理由仍然成立：V1 内部不单调）+ 发送端有界等待 +
  **新终态 `sent`（"已发出，未确认"）**。
  ⚠️ L2 的"待确认"必须是**终态**而不是停在 `active`：对端不声明能力时永远等不到回执，
  非终态就是本阶段一路在修的"永久转圈"，而且会被 A2 的一小时回收扫成 `failed`。
  `file_transfers.status` 是裸 TEXT 无 CHECK 约束 ⇒ 加值不需要迁移，但词表注释写在
  `schema.sql` 与 `db.rs` **两处**，加值时必须同步。
- L2 依赖 A4（终态帧 `try_send` 无痕）：回执帧自己也会丢。丢回执 ⇒ 发送端判失败 ⇒ 重试 ⇒
  重推，而这一条已被本轮的 A6 `AlreadyHave` 挡住（不会落"名字(1)"副本），所以 L2 现在
  才具备安全落地的前提。

### Fixed (工具链 / 门禁)

- `scripts/check-mobile.sh` 在 `set -u` 下把 `$ANDROID_HOME` 裸写进 `for` 的**列表**里：
  未导出该变量的机器上 bash 在列表展开阶段就报 `ANDROID_HOME: unbound variable` 并终止，
  连脚本自己那句"找不到 Android NDK，请设置 ANDROID_NDK_ROOT"的报错都到不了 ⇒
  `npm run verify:full` 第 15 步（Android 编译门禁）在此环境**永久不可用**，
  而 NDK 其实就装在默认路径 `~/Library/Android/sdk/ndk/`。改成 `${ANDROID_HOME:-}` 后
  实跑通过：`aarch64-linux-android` 默认 feature 与 `--features bluetooth` 两轮均 0 warning。
  这条顺带补上了"阶段 3 的 Rust 改动在 Android target 上编得过"的取证（桌面口径的
  `cargo test --lib` 看不见那段代码，正是该门禁当初存在的理由）。

## [4.29.6] - 2026-09-23

### Fixed (2026-09-23 稳定性审计 阶段 3 · 协议与文件可靠性)

清单 11 条（A1–A7 / B1–B3 / G1）逐条重跑原文坐实后**修 7 条**，另 4 条记为已知限制（理由见末尾）。
其中 2 条是修完之后再自查查出来的**修法自身缺陷**（A3 写序、A6 收尾），不是清单条目。

**协议与选路**
- **G1** `mesh/router.rs::on_receive` 改为**先判 TTL、再登记去重**。旧顺序是先去重：TTL 不在帧
  签名内，恶意/异常中继把某帧的 ttl 改成 0 再转发，本机登记 frame_id 后 Drop ⇒ 之后所有诚实
  路径送来的合法副本（ttl 正常）全被判 `Duplicate` = **单跳点封杀这条消息**。现在 TTL=0 的帧
  根本不进登记。回归用例 `zero_ttl_frame_does_not_burn_the_dedup_slot`（双侧断言）。
- **B1** `send_file_via_relay` 的分片大小迁就链路中最受限的邻居：中继帧泛洪给**所有**有直连的
  邻居，任一邻居是 BLE 时 64KiB 分片（base64 后 ~87KB）会反复撑爆 BLE 写超时并**拆掉整条链路**
  （直传早有 `chunk_size_for_path` 门控，中继路径此前漏掉）。判据刻意用「邻居名下**存在** BLE
  链路」而不是「邻居的最佳链路是 BLE」：`send_over_order` 在首选链路队列满（Full）时会顺延到
  order 里的下一条，同一邻居有 LAN+BLE 时那一帧照样落到 BLE —— 按最佳链路判会把罕见但致命的
  拆链换成"LAN 邻居多收些小帧"，不划算（成因写进注释）。
- **B3** `network/ble.rs` 丢弃"握手中 central 的重复 Hello"时补一条日志。行为不变，但此前这条
  输入路径在日志里完全不可见，重连方报"握手超时"时无从排查。

**文件传输终态与续传**
- **A2** `sweep_stale_relay` 回收过期中继态时，给 `file_transfers` 里仍 active 的行标 failed 并
  emit `file-failed`（旧行为只 `retain` 内存 ⇒ DB 行永远停在某个百分比，**接收端永久卡 X%**）。
  新增 `db::mark_transfer_failed_if_active`：只改 active 行，done/failed 等既有终态不许被回收
  动作改写。emit 一律挪到 db 锁**之外**（锁内只收集要通知的 id）—— 在锁内 emit 会把阶段 2 刚
  消灭的「锁内慢活」请回来；写库失败不再 `unwrap_or(false)` 静默，改为 warn 留痕。
  `sweep_stale_reassemblies` 改为返回被回收的 transfer_id 列表。
- **A5** `Message::FileDone` 的 `Ok(None)` 分支补 else：接收器已清且库非 done 时也回一帧失败
  Ack。旧实现只有 `if already_done { 回成功 }` 而**没有 else** ⇒ 本机从没收下这份文件时一帧都不
  回，发送端只能干等整个静默窗口（`FILE_ACK_IDLE`）才判失败，然后整套重发。
- **A6** 重复 Offer 判据补第四输入「本机已收完」：`decide_offer` 新增 `AlreadyHave`，命中时回
  `FileReject{ received = size }`。此前判据只有「活跃接收器 / .part 前缀 / from_bytes」三项，
  而收完之后 .part 已改名、接收器已清空 ⇒ 三输入全归零，与"全新传输"同形 ⇒ 重复 Offer 判成
  Accept ⇒ 整份重推落"名字(1)"副本。发送端 `received ≥ size` 时直接进完成路径，不再重发 Offer。
- **A6 的自审发现（严重）**：该捷径最初的 `return Ok(())` 绕过了 `stream_file` 里整段收尾
  （落 done / 推进 `file-*` 到 delivered / `message-acked`+`file-done`+`file-progress` 三个事件）
  ⇒ **对方已经收到文件，我方气泡永久转圈、transfer 行停在 pending**。收尾抽成
  `finalize_send_accepted()` 供两个出口共用（不在捷径里重抄一遍，避免第二份真相源）。
- **A3 的自审发现（严重，非清单条目）**：`finalize_expired_file` 四步里唯一会把行踢出重试集合的
  `mark_file_outbox_failed` 原先排**第一**（`list_expired_file_outbox` 只选 pending/sending）⇒
  后面任一步失败时"返回 false、下一 tick 重试"是假话：行已经是 failed，再也扫不到，
  **界面永久停在"发送中"**。破坏性写改到最后一位，`finalize_expired_message` 的同型判据一并钉住。
- A6/A5 两处「本机是否已收完」的查询由 `list_transfers()` 全表扫改成新的单行查询
  `db::is_transfer_done()`（Offer/FileDone 热路径每帧付一次，而 `file_transfers` 恰是随使用单调
  增长的表）；查询失败按"未收完"处理（保守方向：最坏重传一次，不丢数据）但必须 warn 留痕。

**判定不修 / 待单独实施（本轮明确记录）**
- **A1 中继发送端假成功**：`relay_send_to_neighbors` 对每个邻居 `let _ = try_send`，发送端把
  "字节写出"当"送达"、跑完就落 done —— 命中 `docs/acceptance/1.0-release.md` 禁止项。
  用户已选择**完整修复**，但完整语义需要"接收端回执帧 + 能力位门控"（老版本不回执，不能干等），
  属协议级改动，方案单独评审后实施，不在本阶段夹带。
- **A7 群文件协议层无续传**：群文件链路根本没有 offer/位置回执，续传需要协议级改动 + 跨版本
  兼容设计。发版尾声硬塞风险大于收益。
- **B2 Windows BLE 重组表回收**：需要 Windows 真机验证，无法在本环境取证。
- **A4/A3 终态帧 `try_send` 无痕**：终态帧（FileDone / Ack）投递失败时不留痕迹，修法涉及
  "终态要不要重试"的设计决策，与 A1 同一批处理。
- **A6 判据的依赖边界**（本轮复核确认，不额外加固）：`AlreadyHave` 的事实来源是
  `file_transfers.status = 'done'`，直传与中继两条接收路径都在成功落盘后写这个终态（已核对
  `transport.rs` 中继完成分支与 `stream_file` 成功回执）。若那次 `upsert_transfer` 自身失败
  （`.ok()` 丢弃），重复 Offer 仍会退化成重新接收落"名字(1)"副本 —— 这是"落终态失败"这一类
  问题的共同根，修法属 A4 的"终态写入需要可靠投递"范畴，不在本阶段夹带。

### Tests (阶段 3)

- 新增 4 条 `lib.rs` 源码守卫：`relay_reclaim_finalizes_in_db_and_emits_outside_the_lock`、
  `terminal_finalize_defers_the_destructive_write`、`duplicate_file_done_still_answers_with_an_ack`、
  `already_have_shortcut_shares_the_send_finalization`。
- 新增 4 条 `verify-guards.py` 变异用例（`--only st3-terminal` 实跑，四条全部"改坏即 FAIL、
  恢复即 PASS"）。其中 A5 那条第一版把 `if` 的收尾花括号一起删了 ⇒ 红的是编译器而不是判据
  （弱确认），已修正并在用例注释里记下这个坑。
- 新增功能用例：`finalize_expired_file_failure_leaves_the_row_retryable`（失败时行必须仍可被
  扫到）、`transfer_terminal_helpers_touch_only_their_own_rows`（两个单行助手的边界）、
  `completed_transfer_rejects_duplicate_offer_without_resend`、`zero_ttl_frame_does_not_burn_the_dedup_slot`。
- Rust lib 测试基线 653 → 659。

## [4.29.5] - 2026-09-23

### Fixed (2026-09-23 稳定性审计 阶段 2 · 结构性根因：全局 db 锁 / 主线程 / 静默吞错)

16 条复核（1 条已被 4.29.2 修掉、2 条判定为既有设计决策/自愈设计不改），实际修复 13 条。
每条动手前均已按 AI_RULES §21 重新复核原文坐实（并行会话当天有提交，清单行号有漂移）。

**批次 a · 静默吞错 / 卡死 / 主线程**
- **m** `commands/chat.rs` 全仓最后一处生产代码 `if let Ok(dbc) = s.db.lock()`（锁中毒静默
  跳过置 sent，消息停在 sending，与 1.2 同形）→ 改 poison-tolerant，与全仓纪律统一。
- **k** `update_profile` 对每条链路 `tx.send().await` 无超时且 `let _ =` 吞错：对端僵死时
  "保存资料"按钮永远转；昵称改了某些对端收不到且无任何线索。→ 500ms 有界发送
  （同 `outbound::SEND_QUEUE_FULL_TIMEOUT` 纪律）+ 失败/超时留痕（warn 日志）。
- **o** 每小时 `.part` 清扫在 async 任务上同步 read_dir/删除 + 抢 3 把锁（启动时立刻一轮），
  慢盘上占满 tokio worker → `sweep_stale_parts` 移入 `spawn_blocking`（对照缓存自动清理
  的既有正确用法），JoinError 留痕。
- **i** `is_zh()` / `notifications::enabled()` 每段后端文案/每条通知抢一次全局 db 锁，
  消息洪峰 + 通知 + 托盘重建时加剧争用 → AppState 加 `lang_pref`/`notify_pref` 原子缓存
  （与既有 `ui_lang` 同型：未加载读库一次回填，此后零锁）；save_settings 写入点同步缓存、
  reset_settings 失效缓存（下次读库回填默认）。
- **j** `set_unread_badge` 是同步命令：macOS 主线程 IPC 回调里内联跑整张托盘图标逐像素
  混合 + set_icon（配合锁内 VACUUM 就是全 UI 冻结）→ 改 async + `spawn_blocking`
  （tauri 的 tray set_icon/set_tooltip 经 `run_item_main_thread!` 内部派发回主线程，已实证）；
  主线程守卫 `blocking_commands_run_off_the_main_thread` 补 `tray::` 标记（字面量 marker
  原本抓不到跨模块调用）。
- **n 判定不修（误报）**：`get_lan_enabled`/`get_bt_enabled` 读时持久化默认值是被
  lib.rs 源码守卫 + 测试钉住的既有设计（"缺省值必须立刻持久化"），非缺陷。

**批次 b · 锁内慢活**
- **a** `clean_cache_now` 持全局 db 锁期间递归删文件 + VACUUM（可秒~分钟级，用户点
  "立即清理"全 App 冻住）→ 文件清理移 `spawn_blocking`（不持锁），VACUUM 仅在
  `removed > 0` 时短暂持锁执行（与 6h 自动清理同纪律）；删除已无调用者的
  `cache_cleaner::clean`（锁 + VACUUM 耦合的旧接口），VACUUM 纪律写进 `clean_files` 文档。
- **c** 聊天搜索结果页每个会话 `conversation_meta()`、每条命中 `sender_display_name()`
  各抢一次 db 锁（50 会话 × 60 条 ≈ 3000 次锁往返）→ `SearchMeta::prefetch`：
  db 一把锁查齐全部点查 + peers 一把锁补非好友昵称（两锁不叠加，语义与逐条版一致）。
- **d** `read_content_preview` 持 db 锁做 `fs::canonicalize`（网络盘可挂任意久）且构成
  db → downloads_dir 锁序耦合 → 锁内只留两个点查，canonicalize/越权校验全部移出锁外。
- **e** `export_chat_text` 在 tokio worker 上同步读全库 + 渲染 + 写盘 → 整体移
  `spawn_blocking`。已知限制：读全库仍在 db 锁内（单连接架构，分批读需读连接池，
  导出是低频自救操作，收益不抵风险——如实记录）。
- **h** 群 `mark_read` 每成员 2 次抢 db 锁（200 人群 = 400 次/滚动触发）→ 单次锁内
  批查全部成员最近消息，发送（await）在锁外，回执持久化合并为一次锁。
- **f/g 判定不修（已知限制）**：群公告面板全表扫描（`conv_id LIKE 'group:%'`）、
  `latest_todo_def` 无 todo_id 谓词倒序扫描——修法均需 schema 索引/内容索引表
  （架构级），且均为低频路径，记录为已知限制。

**批次 c · 迁移健壮性**
- **q** `content_transfers` 补列探测失败默认"列存在"→ 真缺列的老库跳过 ALTER，
  `row_to_record` 的 `r.get("transfer_id")` 让**所有行**不可读；ALTER 失败也被纯吞。
  → 探测失败改按"没有该列"处理（最坏是对已有列的库多跑一次幂等 ALTER，无害）；
  ALTER 失败留痕（duplicate column 除外）。
- **p 判定不修（自愈设计）**：迁移 `user_version` bump 在步骤事务之外，但每步
  `column_exists` 预检 + 重跑跳过使"commit 与 bump 之间崩溃"在下次启动自愈
  （幂等设计正是为此）；"步骤内吞 ALTER 错"实为带日志的预检跳过，非 `let _ =`。

### 护栏

- 主线程守卫新增 `tray::` 重资源标记：任何**同步**命令内联调用托盘重绘即红（j 的
  结构性防回归）。

## [4.29.4] - 2026-09-23

### Fixed (2026-09-23 稳定性审计 阶段 1 · 止血：8 条 P0)

按 2026-09-23 全量问题清单（四分区并行审查）阶段 1 批量修复。
原则：不加任何功能，只消除数据丢失 / 永久卡死 / 消息丢失路径。每条动手前均按 AI_RULES §21
重新复核过 `文件:行号` 仍成立。

**1.1 缓存清理器跟随符号链接 → 磁盘任意位置文件被永久删除（P0 数据丢失）**
`storage/cache_cleaner.rs` 的 `walk_files` 用 `e.path().metadata()` 判类型（**跟随软链**，与注释声称相反）。
缓存目录是远端输入可达面：收到的文件名里放一个软链，其目标会被收集进清理列表并 `remove_file` 永久删除。
修复：改用 `DirEntry::file_type()`（不跟随链接），软链/套接字一律跳过；`plan_removal` 的
`to_remove.contains` O(n²) 顺手改标记数组（大缓存下配额循环卡全局 db 锁）。
回归：`walk_files_never_follows_symlinks`（真实文件系统：文件软链 + 目录软链 + 极端配额，外部文件必须幸存）。

**1.2 `resend_message` 把消息永久卡在 "sending"（P0）**
旧顺序先 `set_message_status("sending")` 再判群聊/取公钥/密钥交换/加密；群分支无条件 `Err` 不回滚、
`?` 路径同样不回滚 ⇒ 状态永久卡 sending，且重发入口的 `"sending" => Err` 守卫把重试挡死 ⇒
消息永远发不出去（对失败群消息点重发必现）。修复：全部可失败步骤移到置 sending 之前，
错误路径无需回滚；置位后只剩幂等 outbox 写入与 try_send。

**1.3 `loadMessages` 快照整表覆盖吞掉在途乐观气泡（P0 诱导重复消息）**
发送在途期间对该会话发生一次 `loadMessages`（点会话 / 状态事件 / 搜索定位），DB 快照里还没有
tmp-* 气泡 ⇒ 整表覆盖后"刚发的消息凭空消失"，invoke 返回时 `replaceMessage` 既找不到列表项
也不在 pending 批次 ⇒ 真实记录静默丢弃（后端不回声，无第二路径补回）⇒ 用户以为失败而重发。
修复：`utils/messages.ts` 新增 `appendLocalOnly` —— 快照落地时把内存独有的 `tmp-*` / `file-failed-*`
记录按原顺序追回尾部（seq=MAX_SAFE_INTEGER）；已在快照里的不重复追加，非本地独有记录不复活。
回归：messages.test.ts 5 个新用例。

**1.4 非活跃会话的状态事件取消活跃会话加载 → 骨架永久转圈（P0）**
`loadSeq` 是全局单计数器，`onMessageStatusChanged` 对**任意**含该 msg_id 的会话触发
`loadMessages`（含非活跃）⇒ 一次 `++loadSeq` 把飞行中的活跃会话加载/翻页判为过期 ⇒
`messages[convId]` 永远 undefined，聊天区骨架永久转圈。修复：`loadSeq` 按会话分桶
（淘汰时一并清理）；`onMessageStatusChanged` 只处理活跃会话（非活跃会话的结果本就会被
activeConv 守卫丢弃，切换回来时 openConversation 会重查）。

**1.5+1.6 启动竞态：`bindEvents` 排在全部初始刷新之后 / 任一刷新失败则永不绑定（P0）**
Tauri `listen` 不回放历史事件 —— 旧顺序（8 个 refresh 全部完成后才 bindEvents）把启动窗口期的
系统通知/新消息/在线状态**永久丢失**（局域网"打开就收消息"是高频场景）；且 refresh 无 catch，
任一 reject ⇒ `bindEvents` 永不执行，界面"活着但功能全死"，需重启恢复。
修复：`init()` 里 bindEvents **前置**（listen IPC 即发即生效）并自带 catch（失败留痕 + toast
`chat.eventBindFail` 提示重启）；初始刷新改 `Promise.allSettled`，逐条记录失败；
`App.vue` 对 `app.init` / `chat.init` 分别兜错，app.init 失败不再跳过 chat.init。

**1.7 批量冲刷的 visibilitychange 兜底是死代码（P0）**
`scheduleFlush` 的 `if (flushScheduled) return` 守卫在 rAF 回调丢失时永久卡 true ⇒
此后所有 `enqueueMessage` 堆积、消息永不渲染且无恢复手段，兜底 `scheduleFlush()` 又必被
第一行挡回。修复：抽出幂等 `flushNow`；rAF 路径加 1s setTimeout 安全网（正常时 no-op）；
visibilitychange 兜底改直接 `flushNow()`；`pending` 堆积到 5000 条时 console.error 留痕。

**1.8 中继收文件 SHA-256 按到达顺序、去重之前喂哈希 → 误报校验失败，两端状态相反（P0/P1）**
`handle_relay_chunk` 逐片"到达即喂"增量哈希，而中继分片天然**重复**（多邻居泛洪）且**乱序**
（多路径时延不同），喂点在 `add_chunk` 去重/排序之前 ⇒ 分片收齐却必然"文件完整性校验失败"：
接收端失败、发送端却显示成功（无回执，relay offer/chunk 不在 outbox，无重试路径）。
修复：删除增量哈希（`RelayFileReceive` 去掉 `hasher` 字段），重组完成后对按 seq 组装出的
明文一次性 `Sha256::digest(&full)`。回归：`relay_receiver_hash_lifecycle_success`
重写为乱序 + 重复分片场景仍校验通过。

### 护栏（防回归，verify-guards.py 已登记变异用例）

- `cache_cleaner_walk_never_follows_symlinks`：walk_files 必须用 `DirEntry::file_type()`，
  不得出现 `e.path().metadata()`。变异：换回跟随链接的写法必须红。
- `resend_message_sets_sending_only_after_all_failure_paths`：全部报错锚点必须排在
  `set_message_status("sending")` 之前。变异：删掉群聊判废必须红。
- `relay_receive_hashes_assembled_plaintext_once`：`handle_relay_chunk` 不得出现增量哈希，
  校验必须是 `Sha256::digest(&full)`。变异：哈希对象改成 `&name` 必须红。

## [4.29.3] - 2026-09-23

### Fixed (审计 A8：transport 生产锁中毒不再 panic 带走 reader_loop)

`network/transport.rs` 里 11 处 `state.<field>.lock().unwrap()`（peers ×4、pending_requests、
pending_file_accept ×2、pending_file_complete、relay、group_file_keys、group_keys）与全仓主导写法
`lock().unwrap_or_else(|e| e.into_inner())` 不一致。全仓**没有 `catch_unwind`** ⇒ 任何一次锁中毒 panic
都会带走所在任务；落在 `reader_loop` 上时会**跳过它紧接着的收尾**（`links.remove` / `mark_peer_offline`
/ 接收器清理），于是那条连接的对端在界面上**永久显示"在线"**、再也判不出离线。

- 11 处一律改成 poison-tolerant（`unwrap_or_else(|e| e.into_inner())`），与主导写法统一（行为仅在中毒路径变化）。
- 源码守卫 `transport_locks_tolerate_poison`：**去掉全部空白后**扫 transport 全集视图，不许再出现
  `.lock().unwrap()`（单行与多行链式一并覆盖）。transport.rs 与各 `transport/*.rs` 分册的测试模块本来
  就不用这个写法，故整视图扫不误伤；已登记 verify-guards 变异用例（把 `pending_file_complete` 那处改回
  `.unwrap()` 必须红）。

> `file.rs` / `commands/logs_tests.rs` 里的 `.lock().unwrap()` 都在 `#[cfg(test)]` 代码里 —— 测试期中毒
> 就是 bug、panic 反而是想要的信号，不在本守卫范围，也**不在这份 transport 视图里**（不同模块）。

## [4.29.2] - 2026-09-23

### Fixed (审计 A3：出站清扫器先 emit「失败」后写库 ⇒ 重复投递)

`spawn_outbox_sweeper` 三处终态（单聊 / 群 / 文件）都把 `emit("message-failed" / "file-failed")`
写在 `if let Ok(dbc){ 写库 }` **之外**，且写库返回值被 `let _ =` 丢掉。后果：db 锁中毒（或写库失败）
时**界面报失败、而 outbox 行还在** ⇒ 下一次 `flush_outbox` 把它再发一遍 = 用户看到「我以为失败了
的消息又发出去了」。另外三处 `let Ok(dbc)=lock() else { continue }` 在锁中毒时**静默跳过整个 tick**，
而这个清扫器是「无链路消息」唯一的终态出口 ⇒ 消息永久停在「发送中」。

- 抽出两个纯 DB 助手 `finalize_expired_message` / `finalize_expired_file`：返回**两步写库的
  conjunction**；三处 emit 改为 `if finalized { emit } else { warn 并保留行、下轮重试 }`。
- db 锁读取一律 poison-tolerant（`unwrap_or_else(|e| e.into_inner())`），不再静默 `continue`；
  锁只在写库作用域内持有、**不跨 emit**（审计 B1：不持锁做慢活）。
- 回归用例 `finalize_expired_message_writes_both_and_gates_on_failure`（真 DB：绿路两步都写、失败路
  返回 false）+ 源码守卫 `outbox_sweeper_emits_only_after_db_write_succeeds`（数 `if finalized {`
  处数 == emit 处数，且不许再有 `else { continue }`），已登记变异用例证明非空转。

## [4.29.1] - 2026-09-23

### Added (守卫非空转记账)

- 给 4.28.0 那条源码守卫 `relay_circuit_is_tagged_relay_not_routed` 在 `verify-guards.py` 登记变异
  用例：把 `relay.rs` 拨号处的 `PathKind::Relay` 注入回 `Routed`（仍可编译，否则红的是编译器不是判据），
  守卫必须红。已 `--only` 实跑证明「改坏即 FAIL、恢复即 PASS」。test-only，无运行时行为变化。

## [4.29.0] - 2026-09-22

### Changed (四种通道全局统一②：连接图标收到唯一一处，好友列表也显示链路类型)

承接 4.28.0 的链路类型统一。此前**连接图标的选择逻辑只内联在聊天头一处**，好友列表压根没有连接
图标（只有在线圆点）—— 用户 2026-09-22 要求"四种通道的图标各界面全局统一、好友列表也同步"。

- 新增 `peerConnectionInfo::linkIconName`（图标名的**唯一判据**，与文案判据 `linkLabelKey` 同源同序）
  + `components/ui/LinkIcon.vue`（图标渲染的**唯一处**：Router=局域网 / Network=跨网段·VPN /
  Globe=公网中转 / Share2=经 N 跳 mesh 桥接 / Bluetooth=蓝牙）。
- **聊天头改为引用这两者**（删掉自己那份 lucide v-if 链）—— 不再各抄一份、迟早漂移。
- **好友列表新增连接图标**：`Friend` 不带链路，由 `ConversationList` 按 device_id 关联节点表
  （`chat.peers` 的 `link`）传入；图标进 `v-memo` 依赖，链路变化时才重渲染。无链路（离线 / 只发现
  未建链）时**不画图标** —— 画一个就是骗。
- 守卫：`channelState.test.ts` 钉"图标只有一处判据 + 一处渲染，聊天头与好友列表都引用它"。

## [4.28.1] - 2026-09-22

### Fixed (加好友界面：局域网/蓝牙关闭时如实显示跨网通道，不再误导"中转坏了")

用户 2026-09-22 实测：不在局域网、只开了公网中转，把「添加好友」页的局域网/蓝牙两个开关关掉后，
界面只说"两条通道都关了"，于是以为"中转搜不到人 = 中转坏了"。**真相是中转本就不为陌生人配对**
（哑管道，ADR-0020 D4）—— 这是设计而非故障，但界面从没把这件事说清。

- 运行快照 `RuntimeSnapshot` 新增两个**可公开**字段：`routedEndpoints`（VPN/跨网段端点数）与
  `relay { enabled, connected }`（中转开没开 / 通没通）。**绝不含口令与服务器地址** —— 快照会进
  `runtime-changed` 事件载荷、被各窗口读到，敏感值仍只属于设置页（与 `relay_token_never_reaches_logs`
  同一条边界）。`relay.connected` 用新的 `path_kind==Relay` 判，不按服务器地址。
- 「添加好友」空状态据此分两种文案：发现通道全关、但跨网通道在线时，说清"它们只连通**已是好友**的人，
  公网首次加好友需经共同好友或打开局域网/蓝牙"，而不是笼统的"都关了"；并在开关下方列出 VPN/中转的
  真实状态（只读，配置仍去设置页）。
- 守卫：`channelState.test.ts` 新增"空状态必须把跨网通道算进去 + 加好友页不得出现口令 / `getRelayConfig`"。

## [4.28.0] - 2026-09-22

### Changed (四种通道全局统一①：公网中转成为独立链路类型 `PathKind::Relay`)

用户 2026-09-22 要求「局域网 / VPN 跨网段 / 蓝牙 / 公网中转」四种通道在各界面各处的枚举全局统一。
此前**公网中转电路复用 `PathKind::Routed`**（`relay_rendezvous_task` 的拨号处），后果是用户能看见的：
中转链路被界面标成「跨网段 / VPN」，并和真正的 VPN 直达路径挤进同一个选路优先级。

- **新增 `PathKind::Relay`**（`mesh/path.rs`，`as_str()` = `"relay"`）；中转会合拨号改用它登记电路。
- **选路优先级显式排成 LAN(0) > Routed(1) > Relay(2) > Bluetooth(3)**（`mesh/selection.rs::path_rank`
  + `state.rs::best_link_kind`）：VPN 对端 IP 直达优于经服务器转发的中转，中转带宽又远高于近场 BLE。
  `best_link_kind` 是**手写优先级数组、不是穷尽 match** —— 加变体时编译器不提醒，已补注释钉住这个静默缺口。
- **前端区分「公网中转直连电路」与「经 N 跳 mesh 转发」**：`peerConnectionInfo.ts` 新增
  `link === "relay"` → `peer.link.relayServer`（「公网中转」），与 `hop > 0` 的 `peer.link.relay`
  （「经 N 跳中继」）分开；聊天头新增 Globe 图标 + `chat.header.linkRelayServer`。中转电路的 endpoint
  是**服务器地址**，`shouldShowAddress` 对它返回 false（绝不把服务器 IP 冒充成对端 IP）。
- 接线守卫 `relay_circuit_is_tagged_relay_not_routed`：钉住「唯一那处拨号构造点必须用 `PathKind::Relay`」。
  只钉枚举的单测在拨号改回 `Routed` 时照样全绿，故接线单独断言。新用例
  `path_priority_relay_beats_bluetooth_but_loses_to_routed`；`path_kind_names_are_stable` /
  `path_rank_order_is_explicit` 扩到含 Relay。

> 边界：**不改**中继电路的识别口径（`relay_link_snapshot` / `drop_relay_circuits` 仍按「endpoint == 服务器
> 地址」判 —— relay 链路同时满足 `path_kind==Relay` 与 `endpoint==server`，行为不变）。发版前不动这条敏感路径。
> `chunk_size_for_path` 只特判 `"bluetooth"`，relay 由 `"routed"` 改 `"relay"` 后仍取 `FILE_CHUNK`，分片大小不变。

## [4.27.4] - 2026-09-22

### Added (中继首行抽成唯一构造点，并钉住"`rejected` 文案不是假指控"的那个前提)

`INTEGRATION.md` §1.1 昨晚补了**第三种命运**：首行放过、但配对前写的载荷攒到 `PENDING_MAX`(1 MiB)
时同样 `destroy()` —— 它对客户端的表现和"口令错"**一模一样**（都是首行之后被关）。
我们 `rejected` 那一档的文案说"口令或版本不匹配"，这句话只在"我们配对前根本没写多少东西"时
才成立 —— 而这件事**目前靠巧合**：首行（≤202 B）+ 一条协商线，约 0.5 KB。

- **`relay_preamble(token, channel_hex)` 成为首行的唯一构造点**，真拨与设置页的"保存前真拨"
  都走它。这不只是防重复：探测必须和真拨发**同一串字节**，否则测出来的不是同一条判据
  （服务器对首行是"口令 / 版本 / 格式"三合一拒绝，差一个字符就会产出"探测说通、真拨说拒"
  这种无法解释的现象）。
- 新用例 `pre_pairing_writes_stay_far_below_the_pending_cap`：量的就是上面那个唯一构造点
  + 真实的 `build_wrap_offer().to_line()`，要求**离 1 MiB 留 100 倍余量**。
  它守的不是今天的大小，是未来那一下 —— 谁往配对前塞心跳、塞首片、塞头像，它先红，
  并直接说清届时该怎么改（把 `rejected` 按**累计字节数**分叉成两档）。
  ⚠️ 已知边界：如果有人绕过 `relay_preamble` 自己再写一次首行，这条测不到 —— 那属于
  "两处规矩"的另一类，靠构造点本身的存在来防，不另加文本守卫。
- **对契约的一处判据异议（回给服务器仓）**：它给的分叉判据是"被关之前我除了首行还写过别的字节吗"。
  这条对我们**会误判**：口令错时服务器在首行处就关掉，而我们那次 `write_all(协商线)` 通常仍然
  成功（写进本地缓冲才发现对端已关）⇒ "写过别的字节"为真 ⇒ 按它的规则会把真的口令错
  说成"撞了暂存上限"。判据应当是**累计字节数 ≥ `PENDING_MAX`**，不是"写过没有"。

### 一次没能复现的红（如实记着，不当没发生）

实现过程中有一次跑 `--lib -- relay` 报 `probe_report_never_carries_the_token` 失败，
输出里的口令值是一个**仓库里根本不存在的串**（`grep -rn "bad-token"` 全仓 0 命中）。
之后连续三次（全量 ×2 + 该模块单线程 ×1）都是绿的：`cargo test --lib` 642 passed / 0 failed、
`commands::relay_config_tests --test-threads=1` 8 passed / 0 failed。
最合理的解释是那条输出来自上一次变异注入尚未还原时的**旧二进制**（我当时把"注入 → 跑测试 →
还原"写在同一条命令里）。**没有证据说它一定不是真 flake** —— 这两条探测用例都绑真 loopback
端口并发跑，端口复用理论上是可能的。若再出现一次，处理方向是：给每条用例固定各自独立的
listener 并在断言前 drain，而不是把断言改松。

### Tests
- `cargo test --features bluetooth --lib` ⇒ **642 passed / 0 failed**；`cargo fmt -- --check` ✓；
  `cargo clippy --features bluetooth -- -D warnings` 0 条；基线 641 → **642**。
- ⚠️ 零运行时行为变化（首行字符串逐字节相同，只是改成一个函数）⇒ 不需要真机；
  中继相关的真机账仍挂在 #8。

Version-Bump: patch

## [4.27.3] - 2026-09-22

### Fixed (1:1 大文件永远传不完 —— 接收端有活跃接收器时不回真实位置，续传形同失效)

用户第一次跨网真机测试报：4–5MB 文件正常，**160MB 永远停在"发送中、进度 0%"，最后报分片失败**；
文字能到但转圈很久。查下来不是中继不稳，是我们自己的续传协议只做了一半。

**根因（不需要任何丢包假设）**：这里曾经有**两套**位置规矩 ——

```text
没有活跃接收器  →  比对 .part 磁盘前缀，不符就回 FileReject{received}   ✅ 一直是对的
有活跃接收器    →  一律裸回 FileAccept，完全不看 from_bytes             ❌ 事故就在这半
```

发送端每一轮重试都从 `from_bytes = 0` 起步（`flush_pending_files` 走的是不带位置的
`send_file_from_path`），它只能靠 `FileReject.received` 知道该从哪儿续。于是第二轮 offer 到达时，
接收器手里已经有 40MB、`next_seq` 停在上一段末尾，却回了个裸 Accept：发送端从头重灌，那 40MB
被 `chunk_seq_decision` 的 `Duplicate` 分支**静默丢掉**（不报错）⇒ **每一轮都要重传一遍已收前缀**
⇒ 慢链路上净推进永远不够 ⇒ 恒 0%、最后超时判死。4–5MB 之所以"没问题"，是因为它一轮就传完了，
根本走不到重试。

- 判据收敛成**一份纯函数** `file::decide_offer(has_active, active_received, disk_retained, from_bytes)`，
  三档答复：`ResumeFrom(held)` / `AcceptResumeSegment` / `Accept`。
  `held` 的权威取法也一起定死：**有活跃接收器时用内存里的 `received`**（每片 `write_all` 后即更新），
  没有时才退回 `.part` 长度 —— 报小了会让发送端重灌已落盘字节（文件超长、SHA-256 必不匹配），
  报大了会让它以为对端有它没有的东西（永远等不齐）。
- **幂等 Accept 修的那条真机缺陷保留**（"两边都显示成功、接收侧列表里没有"）：位置一致时仍然回
  Accept，绝不退回 reject。
- ⚠️ **段号归零必须做，但只在续传段做**，这一条是我自己第一版改错后收紧的：
  少归零 ⇒ 新数据被当成迟到重复片丢掉（差一截、不报错）；
  **无条件**归零 ⇒ 上一轮 attempt 被 timeout 丢掉时它**已入队的分片还在 writer_loop 里往外排**
  （队列 1024 槽 ≈ 262MB，丢 future 不排空队列），那些片的 seq 已到 160+，被拍回 0 就成「跳号」
  ⇒ `Err(文件分片顺序错误)` ⇒ 整单死。两个方向现在都有用例钉着。
- 顺带删掉 `has_receiver()` —— 判据收拢后它没有调用者了（留着会被只编 lib 的 clippy 判死码，
  4.25.1 那次 CI 红就是这个形状）。
- **跨版本兼容**：没有任何新帧、没有字段变更。新接收器 + 老发送器 ⇒ 老发送器早就听得懂
  `FileReject.received`（那条分支就是为它写的）；老接收器 + 新发送器 ⇒ 行为与今天一致（浪费但能完成）。
  不构成交织期风险（INV-P24 意义上"新功能静默失效"的那类不出现，这里只是接收端答复更准）。

### Docs
`docs/protocol-invariants.md` INV-P17（"分片必须可 identify/order/dedupe/validate/reassemble，
不能仅依赖 arrival order"）补了一段续推断言：`order` 对续传同样成立，且**两个方向都要做**，
只做一个就是这次的事故。

### Tests
- 三条新用例（TDD：先看着它们在"照搬今天行为"的桩上按预期红，再实现到绿）：
  `live_receiver_must_tell_the_sender_where_it_actually_is`（红→绿，本次的主判据）、
  `the_active_receiver_is_the_authority_not_the_disk_prefix`（红→绿，钉住 `held` 的权威来源）、
  `matching_position_still_accepts_idempotently`（一开始就绿——它的职责是**防止过度纠正**，
  把"同起点重复 offer 不许归零"也钉住）。
- `cargo test --features bluetooth --lib` ⇒ **641 passed / 0 failed**；fmt ✓；
  `clippy -- -D warnings` 0 条；基线 638 → **641** 全部在跑。
- 两条变异用例登记进 `verify-guards.py`，实测"改坏即 FAIL、恢复即 PASS"：
  ①删掉 `restart_segment` 那一句 ⇒ 接线守卫红；②把 `held` 退回"永远等于 from_bytes"
  （等价于旧的"一律 Accept"）⇒ 三条纯函数用例红。
- ⚠️ 覆盖边界：**这三条是纯函数与接线判据，没有端到端**——本仓库没有能驱动
  `handle_file_offer` + `stream_file` 的异步夹具（造它需要 AppState + 真链路 + 双端任务），
  所以"160MB 在 1Mbps 链路上真的能续完"仍只在真机上能验。已挂进 #8：
  **两台设备、跨网、经中继，发一个 160MB 文件，看进度是否连续上升、断点是否能续、
  以及日志里是否出现"接收端已有 N 字节，要求发送端从此续发"**。
- ⚠️ 未修的相关项（另开，不混进本提交）：**A** `FileCompleteAck{false}` / `FileReject` 这类
  否定确认在队满时被 `let _ = try_send(...)` 静默丢掉；**B** `send_deadline_for` 按明文估算，
  漏了 Base64 的 ×1.334；**C** `ble_file_transfer_respects_link_limits` 这个名字已经盖不住它
  现在守的东西（本次它红过一次，就是因为里面藏着中继接线断言）。

Version-Bump: patch

## [4.27.2] - 2026-09-22

### Security (要求 7 落地：口令默认掩码、加一条"口令值不许进日志/诊断"的守卫，并把做不到的那条写明)

`INTEGRATION.md` 要求 7 有四小条。三条是直接满足的（不写日志 / 不进崩溃上报 / 不进诊断面板），
第四条"存进系统的安全存储（Keychain / 凭据管理器 / Keystore）"**没做** —— 这一版把能做的做成守卫、
把做不到的写成偏离，不再让它停在"看起来做了"的状态。

- **新增源码守卫 `relay_token_never_reaches_logs_or_diagnostics`**：按**语句**（`;` 切，因为日志
  调用普遍跨行、逐行扫会漏掉真正插值那一行）扫 transport 与 commands 全集里的 `logger.` /
  `push_diag_event`，命中口令值表达式（`.token`、`token_norm`）且不是"只报长度"
  （`token_len` / `chars().count()`）即红。这条守的是最容易手滑写出的那一行
  `format!("token={}", cfg.token)` —— 日志会被导出、会被贴进求助帖，而口令是这台服务器
  唯一的准入手段。非空转实测：把探测日志的 `token_len={}` 改成 `token={}` ⇒ 红在"把口令**值**
  写进了日志或诊断事件"；还原 ⇒ 通过。已登记进 `verify-guards.py`（`--only relay`）。
- **设置页口令格默认掩码** + 显式「显示/隐藏」。理由不是抽象的合规：用户为求助而**截图设置页**
  是最常见的一张截图，而服务器地址就在同一格里 ⇒ 一张图交出一份可用凭据。
  刻意**没有**做成"只写不可读"——那会把"我到底填的哪个口令"变成只能重填才知道，
  而口令本来就躺在服务器的部署文件里，藏它对攻击者没有意义，只对用户自己有意义。
- **换口令的成本写进界面提示**（`tokenHint`）：服务器改了口令，这批人每台设备都要重填一次，
  没有同步也没有找回。指南最后那条警告此前只存在于文档里。
- ADR-0020 新增一节「与要求 7 的一处偏离」：为什么**不**单独给口令接 secret store ——
  同一份 SQLite 里躺着的 Ed25519/X25519 **身份私钥**比口令严重得多（私钥=冒充你，
  口令=借这台服务器拼字节），只做口令那一条得到的是"看起来更专业、实际防护没变"的不对称实现。
  要做就三平台一次做完（迁移 + 首启读出旧值 + 迁移失败的回退语义），那是另一篇 ADR。

### Fixed (顺带修掉两处**假绿**：守卫视图漏了中继分册)

`transport/relay.rs` 与 `commands/relay.rs` 都是 `include!` 进同一模块的分册，而源码守卫读的是
**文件文本**。4.25.0 接线时只把 `transport/relay.rs` 登记进了 `docs/domains.data.mjs`（那次是为了
领域图报红），**没登记进 `transport_src_for_guards()`，`commands/relay.rs` 连 `all_commands_src()`
也没进** ⇒ 所有以"transport 全集 / 全部命令面"为判据的守卫**看不见拨号器与中继命令本身**。
这类漏法不报错，只是永远绿 —— 比假红危险，因为它会让人以为那条判据在被守着。

- 两份视图各补一行登记，并在上面那条新守卫开头加**自证**：视图里必须真的含有
  `fn relay_negotiate(` 与 `fn check_relay_server(`，否则直接红（"这条守卫会假绿"）。
  配套变异用例：把登记那行去掉 ⇒ 自证断言先响。
- 复核过的连带影响：`friend_identity_anchor_has_one_binding_rule`、`relay_data_plane_respects_policy`
  等计数型守卫在视图扩大后**依然全绿**（中继分册里没有它们数的符号），所以这次是补覆盖面，
  不是把哪条改松。

### Tests
- `cargo test --features bluetooth --lib` ⇒ **638 passed / 0 failed**；`cargo fmt -- --check` ✓；
  `cargo clippy --features bluetooth -- -D warnings` 0 条。
- `npm test` ⇒ **516 passed / 0 failed**（含中英 key 集合一致）；`npm run build`（vite + vue-tsc）✓。
- 基线 637 → **638 行**（新增一条守卫用例；按集合 diff 核过只增不删）。
- ⚠️ 覆盖边界：守卫是**文本判据**，它能挡住"日志里插值口令"这一类写法，挡不住"把口令写进
  某个将来新增的结构体再整体 `{:?}` 出来"那种间接路径 —— 那要等诊断面板真的加中继那一格时
  用类型来守（届时该把 `RelayRuntime` 的 `token` 换成 newtype 并 `Debug` 手写掩码）。
- ⚠️ 掩码只覆盖设置页这一格；日志与诊断此前就已只记长度，本轮补的是"以后也别改回去"的护栏。

Version-Bump: patch

## [4.27.1] - 2026-09-22

### Fixed (中继会合占空比只有 50% —— 两端反相时每一轮都正好错开，永久配不上)

`INTEGRATION.md` 要求 3 那一格写着"⚠️ 能跑，但占空比约 50%"。自己复算过，成立，而且比"会空转
几轮"更糟：

- 一轮的耗时 = **排空拨号集合**（`while dials.join_next().await`），而单次拨号被协商窗口封顶
  10s ⇒ 周期 = `tick(10s) + 窗口(10s) ≈ 20s`，窗口只有 10s ⇒ **占空比 50%**。
- 两端周期相同（同一份代码、同一个 tick）时**相位是恒定的**，不是随机漂移。差 ≈10s 的那一组
  相位里，A 的登记区间正好落在 B 的空隙里 ⇒ 每一轮都擦身而过，**永远配不上**。
  这与已被回退的 `7e8be76`「按轮交替」是同一类缺陷，只是反相的东西从计数器换成了节奏。

- 改法一（结构）：拨号集合**提到循环外跨轮持有**，轮次**不等**它；每轮开头用
  `try_join_next` 非阻塞回收已完成的。旧那句"必须排空：JoinSet 被 drop 会 abort 未完成任务"
  的教训没有被丢掉 —— 排空之所以必要是因为当时集合是轮内的局部变量；现在它活着，
  只有 `shutdown` 分支 break 之后才随作用域 drop（那时 abort 正是想要的行为）。
- 改法二（参数）：`RELAY_RENDEZVOUS_SECS` 10s → **2s**。周期现在就是 tick 本身（不等拨号），
  2s 的代价是每轮多两次本机 SQLite 读 + 一次链路锁快照。合成后窗口 10s / 周期 ≈12s ⇒
  **占空比 ≈83%**。
- 新增的在途账：`inflight` 集合与"已有电路"**同等参与上限与去重**（`relay_link_snapshot`
  只看得见**已建成**的链路，轮次不再阻塞后，同一对端会在上一条还没成之前被再拨一次 ——
  `DialGuard` 会挡住第二条 socket，但 `MAX_RELAY_CIRCUITS` 的判据会失守，上限被突破成 2×N）。
  任务 panic 时拿不到 peer ⇒ 兜一条不变式："没有还在跑的任务 ⇒ 不可能有在途拨号"，每轮开头
  据此清空（有任务在跑时一个都不清，所以不会误摘）。
- ⚠️ **没有**用"把窗口拉长到 30s"这条路来提占空比，那条看着更省事但是错的：窗口 ≥ 服务器
  `WAIT_TIMEOUT_MS`(30s) 之后，是**服务器**主动关闭我们这条 socket，我们读到 EOF ⇒ 按 §1.1 的
  判据会归类成 `Rejected`（"口令或版本不匹配"）—— 一张彻头彻尾的假指控单。这条耦合现在写在
  常量注释里，并由用例钉住。
- 新用例 `relay_window_covers_round_period` 钉三条不等式：`窗口 ≥ 2× 周期`（占空比 ≥ 2/3 ⇒
  两个各占不到 1/3 周期的空隙盖不满一整圆 ⇒ 任意相位都存在重叠区间）、
  `窗口 + 5 < 30s`（永远由我们先丢 socket）、`周期 ≤ 5s`（改完配置等一轮就生效）。
  非空转实测：把 tick 改成 30s ⇒ 红在"登记窗口 10s 必须 ≥ 2× 轮周期 30s"；还原 ⇒ 通过。

### Tests
- `cargo test --features bluetooth --lib` ⇒ **637 passed / 0 failed**；`cargo fmt -- --check` ✓；
  `cargo clippy --features bluetooth -- -D warnings` 0 条。
- 基线 `test-baseline.macos.txt` 636 → **637**，按集合 diff 核过**只增这一条、无删除**。
- ⚠️ 覆盖边界：**这条恰恰需要跨网真机才算验证到位**（本机 duplex 测不到"两端各自的墙钟相位"）。
  能静态保证的只有那三条不等式与"轮次不再阻塞"的代码形状；真机上要验的是
  两台不同网络、都开着中转的设备在**任意启动时刻**下 30 秒内能配上对。

Version-Bump: patch

## [4.27.0] - 2026-09-22

### Changed (设备 ID 形状 24 → 21 字符：`gosslan-` + 13 位小写 hex)

用户 2026-09-22 定的规格是"默认 15–20 字符、撞号时自定义最多 +3"，而现网形状是 24。
取 **21** 而不是硬压到 20 的理由不是省那四个字符：**21 + 3 = 24 正好回到旧形状** ——
"撞了号又不想等"的人手工加三位后缀后，长度与今天所有设备一致，UI/日志宽度不用特判。

- `DEVICE_ID_HEX_LEN` 16 → 13，`generate_device_id` 的取值随之变成 `hex(...)[..13]`。
- **熵 64 bit → 52 bit 是明知故犯**，理由写进 `device.rs` 与 ADR-0021 §4：唯一性由首启那
  16 字节随机数提供（不是由截断后的位数提供），52 bit 在百万台设备量级下的生日碰撞概率
  仍低于 1e-4，而这个产品的对手不是注册机。**如果哪天要做"服务端注册/设备认证"，这条要重估。**
- **已装设备一个字都不变**：启动时"只认持久化值"，所以库里那 24 字符会一直留着 ⇒
  系统里长短两种 id **并存是预期状态**。核对过没有任何一处按"等长"写过
  （`nickname.rs` 取的是哈希**字节** `h[0] / h[1]`，不是字符位置；镜像规则是 ASCII 字典序，
  不同长度照样可比）。
- 护栏：`device.rs` 里那条形状断言现在**同时**断言"长度 = 常量"和"常量 = 21"（字面量）。
  后者是刻意的 —— 只写前者的话，把 hex 位数从 13 改成 8 会跟着一起变绿。

### Fixed (顺手挡掉一个自己刚踩过的坑)

新增的 `DEVICE_ID_LEN` 一开始写成 `pub const`，而它只被测试用 ⇒ `cargo clippy -- -D warnings`
**只编 lib、不带 `--tests`**（CI 的 rust 组就是这个形状），于是报 `constant is never used` 直接红。
改成 `#[cfg(test)] pub(crate) const` 并把原因写在注释里 —— 与 4.25.1 那次
`parse_channel_hex` 是同一类坑（"只给测试用的东西"在生产构建里就是死码）。

### Tests
- `cargo test --features bluetooth --lib` ⇒ **636 passed / 0 failed**（含 `generated_id_keeps_the_one_prefix_and_length`
  钉住"前缀只有一个、长度 21、只含小写 hex"，以及 `identical_attributes_still_produce_different_ids`
  那 200 次连发生成互不相同 —— 截断到 13 位之后这条依然是判据，不是运气）。
- `cargo fmt -- --check` ✓；`cargo clippy --features bluetooth -- -D warnings` 0 条；
  `check-test-manifest --only rust` ⇒ 基线 636 条全部在跑（用例数没变，只改了断言）。
- ⚠️ 覆盖边界：**没跑真机升级路径**。已装设备"保持 24"这条是靠"启动只读持久化值"的代码路径
  成立的（`state.rs` 的 `match db::get_setting(...)`），而不是靠迁移 —— 真机上要验的是
  老设备升级后 id 一字未变、好友与链路都不受影响（新装才会看到 21 字符）。

Version-Bump: minor

## [4.26.0] - 2026-09-22

### Added (公网中转：保存时真拨一次 + 三类失败分开说 + 只填 IP 时自动试端口)

`INTEGRATION.md` §1.1 把服务器行为写成了成文契约：**它的两种命运差两个数量级**（首行非法 ⇒
立刻 `destroy()`；首行被接受 ⇒ 挂进等待表 30s），加上"TCP 连不上"是纯本端可判的 ⇒
用户其实能分开三件事。而客户端此前把它们全折叠成一句"建链未成功"，于是"我口令打错了"和
"对方那边还没开开关"长得一模一样 —— 用户照着去改口令，或去查那台其实好好的服务器。

- **`check_relay_server`（新命令）**：保存后自动拨一次。它**不经过** `connect_to_peer`
  （那条路会登记链路、占用在途守卫、真发 Hello），而是自己开一条裸 socket，发一行
  `GSRL1 <token> <随机 64hex>\n`，然后看服务器是"立刻关"还是"挂着"。假哈希是刻意的：
  形状合法但服务器上不会有人登记同一个 ch ⇒ 判据不受第三方干扰，也不会误连到别人的会话。
- 三档结论是**机器可读串**（`unreachable` / `rejected` / `held`），界面按串取文案，
  不拿中文去匹配。文案里那条最值钱的边界写死了：**"对方没开中转"和"对方不在线"对哑管道
  是同一个观测，分不出来** ⇒ 不做第四档，也不在 UI 上承诺它。
- `rejected` 那档刻意不写"口令错误"：这台服务器把「口令错 / 协议版本不符 / 首行格式非法」
  走同一个 `reject()`，精确原因只有服务器日志有。写死"口令错误"会把版本不匹配的人指引去改口令。
- **只填 IP 时依次试候选端口**（`RELAY_PROBE_PORTS` = 59993 默认口 → 443 → 59994），
  试通的那个**回填并再存一次** ⇒ "记住可用端口"就是 `relay_server` 这一个字段，不新增状态。
  用户**显式写了端口就只试那一个**：他已经给了答案，再猜会把"端口不通"稀释成三个端口的失败。
  第一个不是"连不上"的结论就停止换端口 —— 被拒说明那地址上确实有一台 Gosslan 中转。
- 设置页多一个「测一下」：服务器可能是**后**部署的，重测不该要求用户先改一遍字段。
- 探测窗口 2.5s，**刻意远小于**服务器的 30s 与客户端自己的 10s 看门狗：判"活着"只需盖过公网
  RTT；拖过 10s 会撞自己的看门狗，拖过 30s 是白占服务器一个 pending 槽（每槽 1 MiB）。
  这条耦合（客户端超时 < 服务器 `WAIT_TIMEOUT_MS`）一旦反过来，判据就得跟着改 —— 写在常量注释里。
- 顺带把**运行时**那条路也分了档：`relay_negotiate` 的错误从 `String` 改成
  `(RelayProbeKind, String)`，"首行写出后立刻断开"判 `Rejected`、"10s 没人说话"判 `Held`、
  "连上了却写不出去"判 `Unreachable`，而"配对成功但身份不符"单列 `PeerIdentity`
  （它的处置动作完全不同：重新加好友，而不是改口令）。诊断事件与日志都带上档位前缀。

### Tests

- 新增 4 条 Rust 用例（`--lib` 全量 **636 passed / 0 failed**）：两种命运的实测分离
  （真 loopback 监听，一个是读完就 `drop`、一个 `sleep(30s)` 挂着）、没人监听 ⇒ `unreachable`、
  **报告任何字段都不许含口令**、最长合法口令的首行仍 ≤ 服务器 256 字节闸门、
  以及候选端口表（纯函数，避免把"443 在开发机上可能被本地代理占着"测成网络事实）。
- 新增 1 条前端契约用例（`npm test` **516 passed / 0 failed**）：档位那三个名字必须在
  Rust 的 `as_str`、TS 的 `RelayProbe.kind` 联合类型、**以及中英两种字典**里是同一套。
  非空转实测：把 Rust 里 `"held"` 改成 `"hold_on"` ⇒ 红在"前端声明了 Rust 不产生的档位：held"；
  还原 ⇒ 16/16 绿。（这一档正是唯一告诉用户"你配置没错、问题在对方"的那档，少一行翻译就是空白。）
- 基线：`test-baseline.macos.txt` **632 → 636**（`--update` 生成，核对过 diff **只增不删**）。
- ⚠️ 覆盖边界：**跨网真机仍未跑**（要两台不同网络的设备 + 一台中转服务器）。本轮在本机测到的是
  loopback 上的"两种关闭时机"分离，测不到的是公网 RTT/抖动下 2.5s 窗口够不够、以及云安全组
  没放行时报的是不是 `unreachable`（按 TCP 语义应该是，但没验过）。
- ⚠️ `rejected` 这一档在真机上还有一个已知的假阳性来源：**协议版本比服务器新或旧**都会走同一条
  `reject()`。文案已经按这个写了（不写死"口令错误"），但真遇到时仍然只能靠服务器日志定案。

Version-Bump: minor

## [4.25.9] - 2026-09-22

### Docs (把"撞号后提示换新 ID"从既成事实改回未实现 —— 代码注释与 ADR 一起改准)

`a6fbbf1`（v4.25.5，设备身份改生成式）之后，`state.rs:1089-1092` 那段注释写的是：

> 今天已经撞上号的设备走"检测到同 ID 不同密钥 ⇒ 提示换新 ID"那条路

**那条路还没实现。** 全仓只有 `warn_key_conflict_once`（冲突时提示一次），没有任何
"换 ID"的命令、按钮或重启后生效的路径 —— grep `换新 ID` 只能命中这条注释本身和 ADR 里那句
**政策**（"其中一台换新 ID；历史不迁移；好友重加"，那是用户 2026-09-22 拍板的口径，不是实现状态）。

这不是措辞问题：注释说"有这条路"，下一个读到的人就不会去建它，而**用户报的那次真机事故恰恰
需要它** —— 库里已存的两个相同 id，光改生成代码一个都救不了（新逻辑只在"库里没有值"时生成）。
所以本轮把两处一起改准：

- `state.rs`：明说"这段代码救不了已经撞上号的两台"，并把切片 2 的名字与状态写清楚。
- ADR-0021 §6：新增一条 ⚠️，列出**本次只落地了"生成 + 只认持久化值"两件事**，
  而 §5 那三条（提示换新 ID / 自定义后缀 +3 与字符集白名单 / 旧会话转只读）**都还没实现**；
  在那之前撞上号的两台只能清应用数据自救（代价是丢聊天与好友，这也是用户已接受的口径）。
- 待办已开成任务：**切片 2**（撞号提示 + 自定义后缀）。里面还压着一个需要用户拍板的口径冲突：
  现网 id 是 **24 字符**（`gosslan-` + 16 位 hex = 64 bit），而口头规格是"默认 15–20、自定义 +3" ——
  缩短 hex 会直接掉熵，而形状不变量有护栏钉着（昵称由 id 派生、ASCII 排序、日志/UI 宽度）。
  **不擅自改形状。**
- 勘误（上一条 4.25.7 `52dced4` 的提交信息）：里面写"5 处 `mark_peer_keys_verified(` = 定义 690 /
  打标 1099、1990、2397、2772"。**定义的实际行号是 698**（我引用的是插文档之前的数），
  四处打标的行号是对的。那条已经 commit 了，不改写历史，在这里更正。

Version-Bump: patch

## [4.25.8] - 2026-09-22

### Tests (事件扫描器那处修复补上非空转登记，并把"只写在注释里的边界"换成会红的绊线)

自查 `e8b335a`（v4.25.2，"IPC 契约扫描器把 Rust 注释扫成后端在发的事件"）：修复本身是对的，
但它违反了这个仓库的铁律——**新护栏必须被证明会失败**。当时只手工注入过一次，没登记进
`scripts/verify-guards.py`。这一条把账补上，过程中又挖出两处：

- 登记 2 条变异用例（`python3 scripts/verify-guards.py --only ipc-events` 实测**两条都
  "改坏即 FAIL、恢复即 PASS"**）：
  ① 删掉行注释那一段剥离 ⇒ 夹具 `事件扫描器不被注释骗` 红在 `phantom-line`；
  ② 把 `skipString` 里 `const raw = /^r#*"/` 简化成 `/^r"/` ⇒ 红在 `real-after-raw-odd-quote`。
- **第一版 ② 选错了变异点，跑出来是"护栏空转"**，而这不是我随手写错那么简单：raw 字符串的识别
  在**两处冗余存在**（外层 `stripRustComments` 的 arm 与 `skipString` 内部的回落），改坏外层会被
  内层救回来 ⇒ 整组 11/11 依然绿。**改了外层会不会红**这种直觉在这里是错的，
  这个事实现在写在夹具的注释里（连同"别拿它当护栏存在与否的证据"）。
- 夹具因此补一条**引号数为奇数**的 raw 字符串（`r#"引号 " 和斜杠 // 都在原始串里"#`）：
  只有这种形状才会让"不认识 `r#"`"的扫描器把引号两两配对配错相位，于是串里的 `//` 落到代码里、
  整行被当行注释吃掉 ⇒ 真事件**少报**。原来那条引号成对的样例治不了这个形状。
- `e8b335a` 的提交信息里有一句**不成立的保证**："哪天有人开始用 `emit_to`，夹具里那条空断言会先响"。
  那条断言的内容就是"`emit_to` 扫不到东西"，所以它永远绿 —— 它把盲区**写成规矩**而不是发现它。
  现在换成真的绊线：绊线扫全部 Rust 源码（剥注释后）找 `emit_to(`，出现即红，
  并直接说清该怎么办（扩正则 + 同步改那条 `[]` 夹具，或改回 `emit`）。
  实测：往 `device.rs` 塞一行 `emit_to(...)` ⇒ `fail 1`（断言文案命中）；还原后 `pass 11 / fail 0`。
- ⚠️ 覆盖边界：绊线只拦"用 `emit_to` 且事件名不是第一个实参"这一类；其它调用形态
  （先把名字存进变量再 `emit(&name, …)`）仍然扫不到 —— 那属于 #33③ 的"事件名常量化"那片，
  本轮不扩。

Version-Bump: patch

## [4.25.7] - 2026-09-22

### Fixed (4.25.3 的打标漏在了一条 transport 通用路径上 —— 第一次连上的好友绑不上身份锚点)

自查 4.25.3（`17b6329`，"锚点只认被证明过的来源"）时发现：那一次把三处握手后的打标补齐了，
但**打标点放错了位置**，出站拨号与蓝牙这两条路径上它等于没做。

`mark_peer_keys_verified` 在 `peers` 条目不存在时是**空操作**（这是刻意设计：不凭空造条目，
否则"搜得到节点却加不上好友"）。而 `upsert_peer` 新建条目时**恒标 `keys_verified: false`**。
于是：

- 入站连接：对方先 announce 过 ⇒ 条目已存在 ⇒ 握手处那次打标生效（这条一直是好的）。
- **出站拨号**：握手处先打标，`peers` 条目要等 Hello 落进 `handle_message` 才由 `upsert_peer`
  建出来 ⇒ 打标跑在条目存在之前 ⇒ 空操作。
- **BLE**：同一个次序问题（`verify_hello_for_ble` 在 `handle_message` 之前）。

后果是静默的：那一次会话里 `friends.ed25519_pubkey` 一直是 NULL ⇒ **安全码算不出、公网中继
永不准入**（`list_bound_friend_identities` 只看它非空），而 4.25.3 的提交信息、代码注释和
INV-P11 那条"留 NULL 不是死路"都把它写成"任何一次验签通过的 Hello 都会经 `upsert_peer`
把它绑上" —— **`upsert_peer` 并不会打标**，那句话把机制说错了。只有下一次重连（条目已存在）
才自愈，所以现象是"有时候能连有时候不能"，正是本仓库最不该留的那类形状。

- 改法（一行 + 一份留存的钥匙）：在 `handle_message` 的 Hello 分支里、`upsert_peer` **之后**
  补一次 `mark_peer_keys_verified`。这是 TCP 入站 / 出站 / BLE 三条 transport **共用**的那个
  写入点（能这么写是因为 INV-P21：身份先于链路，落到这里的 Hello 必然已验签）。
  握手处那三次保留 —— 它们覆盖"announce 已建条目"的情形，是最早的升级点。
- 次序是判据的一部分：**打标写在 `upsert_peer` 之前不会报错，只会静默绑不上**。所以守卫不只数
  数量，还钉次序：`friend_identity_anchor_has_one_binding_rule` 现在断言
  `mark_peer_keys_verified(` 共 **5 处**（1 定义 + 4 打标），并断言 `handle_message` 函数体里
  `upsert_peer(` 的下标 **小于** 打标的下标。
- 两条非空转用例已登记进 `scripts/verify-guards.py`（名字都含"打标"）：
  ①删掉那一行 ⇒ 计数断言红；②把它挪到 `upsert_peer` 之前 ⇒ 计数不变、只有次序断言红。
  两条都实测过"改坏即 FAIL、恢复即 PASS"。
- 顺带把说错的机制改准：`mark_peer_keys_verified` 的函数文档（原来整段被挤到
  `peer_keys_trusted` 的文档里，函数自己在裸奔）、`peer_keys_trusted`/`bind_friend_keys_on_accept`
  里"自愈经 upsert_peer"那句、以及 `docs/protocol-invariants.md` INV-P11 的那一条。
- ⚠️ 覆盖边界：**没有端到端行为用例**（那要造一个带签名 Hello 的 `handle_message` 异步夹具，
  本仓库现在没有这类夹具）。这次补的是"文本次序 + 数量"两条断言加变异证明；真机上要验的是
  ①只靠蓝牙连上的好友能算出安全码、②只靠出站拨号连上的好友在 4.25.x 中继设置页里被准入。
- ⚠️ 自查另外两处**没有**跟着改，已单独记账：`get_safety_number`（`commands/friends.rs:32-49`）
  读 `peers` 里的公钥时不看 `keys_verified`（与"锚点只认被证明过的来源"不对称，是显示侧缺口，
  不是本次改出来的）；`a6fbbf1`（v4.25.5）承诺的"撞号后提示换新 ID"仍未实现（那是设备身份的
  切片 2，本轮另开一条记账）。

Version-Bump: patch

## [4.25.6] - 2026-09-22

### Reverted (回退 4.25.4 的"换天窗口按轮交替"：它没有修好问题，而且提交信息里的保证不成立)

4.25.4 声称"两端各自交替 ⇒ 最多两轮必然落在同一天"。**这句是错的**：轮次计数器是进程内的、
两端都从 0 起算、相位互相独立。反相时（一端在第 0 轮、另一端在第 1 轮）整个窗口内每一轮都是
一端拨今日、另一端拨昨日 —— 一次也配不上，最坏要卡满整个 ±1 小时窗口。它把"偶发失败"改成了
"另一半概率永久失败"，而且改前的版本本来就是被动自愈。

重新算过之后，连"需要主动方案"这个前提也不成立：

- 两端只在**时钟于零点两侧对不齐**时才会算出不同的 day，而对不齐的时长就等于**两端时钟偏差**
  （NTP 下秒级），不是指南说的 1 小时；两侧各自跨过零点后自然重新一致，最坏卡一轮重拨。
- 指南给的"窗口内同时拨今日与昨日两个哈希"这里做不到：在途拨号守卫的键是 `peer:{对端 id}`
  （`transport.rs:2241`），同一对端的第二条拨号会被挡掉。要做到真同时，得把并发守卫拆成按通道
  键 —— 为一个秒级窗口改并发守卫，代价远大于收益。

处置：删掉 `relay_day_to_try` / `relay_in_day_rollover_window` 与那个进程内 `round` 计数器，
拨号回到 `relay_epoch_day(db::now_ms())`；删掉只钉住"交替序"的用例
`rollover_window_alternates_the_day_per_round`（它验的是一个不成立的性质，保留会让下一个人以为
交替是设计）。**保留**的是把理由写进代码：`relay_epoch_day` 之后留了一段注释说明为什么这里
是被动自愈，并把 ADR-0020 D2 那条"几分钟内自愈"补成准确的偏差口径。

- ⚠️ 覆盖边界：跨网真机仍未跑过换天现场（要两台不同网络设备在 UTC 00:00 前后各开一次中继）。
  本机验证只有：`--lib -- relay_wiring` 6 passed / 0 failed、clippy `-D warnings` 干净、
  用例基线同步（macos 移除那一条）。
- 自我记账：这是一个**已推送的提交**里写错的保证，不改写历史，只在 CHANGELOG 与代码注释里
  留下更正后的结论。

Version-Bump: patch

## [4.25.5] - 2026-09-22

### Fixed (两台设备的"设备指纹"能完全相同 —— 身份改成首启生成，不再从机器属性派生)

真机事故（2026-09-22 早上）：同一局域网两台新装设备的 `device_id` 一模一样，随后互相顶号、
连不上、日志看着像网络故障。

根因不是熵不够，是**身份被派生出来、而且派生优先级压过持久化值**（旧 `state.rs:1087-1127`）：

```text
1. gosslan- + SHA256("gosslan-machine:" ‖ 机器码)[..16]   ← 有机器码就不看库里存了什么
2. 库里持久化的 device_id
3. gosslan- + SHA256("gosslan-host:" ‖ 主机名)[..16]      ← 安卓恒走这条
```

机器码路径在**克隆 / 未 `sysprep /generalize` 的 Windows 镜像、VM 模板、PVE 完整克隆、容器、
迁移助理保下来的 `IOPlatformUUID`** 下相同；主机名路径在**两台新机的默认主机名相同**
（`MacBook-Pro.local`、`DESKTOP-ABC123`）下相同。"机器码优先"还多一层坏：手动改库也改不掉。

撞了以后是"打架"而不是"难看"，因为这些全都按 `device_id` 索引：`peers` / `links` / `friends`
/ `conv_id` 互相覆盖；Hello 的锚点是"`device_id` → 绑定的 Ed25519"（INV-P21/P11），第二台来连
就是一次**密钥冲突 ⇒ 硬拒 + 只提示一次**；「大 id 主动拨、小 id 只接受」的镜像规则在 id
**相等**时直接退化（拨号处有 `peer_id == self.device_id` 过滤，它以为"那是我自己"）；
ADR-0020 的中继准入判据也按 id 索引 ⇒ id 可撞则"锚点绑对了"无从谈起。

- 改成：`gosslan-` + hex(SHA256(**首启 16 字节随机** ‖ 机器码 ‖ 主机名 ‖ 排序后的网卡名))。
  **唯一性由随机数保证**（属性全等也不撞），**稳定性由"只认持久化值、绝不重新派生"保证**
  （换网卡、开随机 MAC、升系统都不动身份）。设备属性按用户要求继续参与计算，但降级为熵与
  "同镜像可识别"的线索。
- 形状一字未动（24 字符、`gosslan-` 前缀、小写 hex）：昵称由 id 派生、镜像规则要 ASCII 可排序、
  UI 与日志宽度都按这个形状写着；`device.rs` 里那三条护栏（前缀唯一 + 长度固定 + 只含小写 hex）
  把 2026-09-13"多套一层 `dev-` 导致安卓恒为最小 id、永不主动拨号"的教训继续钉住。
- ⚠️ **MAC 与蓝牙地址刻意不进哈希**（已与用户对齐）：`if-addrs` 不跨平台给 MAC（要写
  Win/BSD/Linux 三套系统调用 + Android JNI），而 Android/iOS/Win11 默认开 MAC 随机化 ⇒
  它加不了唯一性，只会加"换网络就换身份"的不稳定。要加也就是往属性快照里添一项。
- 迁移策略是**刻意保守**的：旧代码每次启动都把派生值写回 `settings` ⇒ 已存的值就等于派生值 ⇒
  换优先级**不改变任何现存设备的 ID、不打断任何已有好友绑定**。已经撞上号的那两台光改生成代码
  治不了，按用户决定处理：**其中一台换新 ID，聊天历史不迁移，好友重新添加，旧会话留在本机但
  只读**。判据不靠猜 —— 只有当本机观察到"同一个 `device_id` 带着不同的已验签 Ed25519"才提示
  （走 INV-P11 已有的 `key_conflict` 路径），谁先动手谁换，另一台之后不再看到冲突。
- 已装设备侧的换新 ID / 自定义后缀 / 冲突提示**是下一片**（要动 commands + 设置页 + 重启语义，
  所以没混进这一版）。自定义后缀的约束已经写进 ADR：字符集限小写字母数字、上限 = 默认长度 + 3、
  **不接受空格 / `:` / `/` / `\` / 引号**（id 会进日志、进 `peer:{id}` 这类拼接键、进诊断面板），
  自由文本只给昵称不给 id。
- 新 ADR：`docs/adr/0021-device-id-is-generated-not-derived.md`（含事故、两条派生路径的撞法、
  为什么不引 attestation、迁移决定）。同步 `AI_PROJECT_HANDOFF.md` 的设备指纹行、
  `README.md` 与 handoff 的 `device.rs` 注释、`docs/AI_ENGINEERING_INDEX.md` 的 ADR 清单 ——
  这几处以前描述的是"桌面 machine-uid、移动端主机名兜底"，留着就是一份和实现各说一份的说明。
- 测试：`device.rs` 里旧的 `fingerprint_has_prefix` / `every_fingerprint_path_shares_one_prefix`
  随函数一起删除，换成三条 —— `generated_id_keeps_the_one_prefix_and_length`（形状/前缀不变量）、
  **`identical_attributes_still_produce_different_ids`**（本次事故的正向回归：属性完全相同生成
  200 次必须两两不同；旧实现在这里 100% 撞）、`empty_attributes_still_yield_valid_distinct_ids`
  （容器/移动端拿不到属性也不失败）。两份用例基线同步增删（旧名留着会让清单守卫判"静默消失"而红）。

Version-Bump: patch

## [4.25.4] - 2026-09-22

### Fixed (中继在 UTC 换天前后配不上对端 —— 接入指南要求 5 的最后一格)

通道哈希里带 epoch day（UTC 天序号），换天那一刻两端可能各算"今天"和"昨天" ⇒ 两个不同的
通道，服务器各自等 30 秒把连接踢掉、再重拨、还是各算各的。现场表现是最难查的那种：
**两端都显示"已连接"、服务器日志一切正常、就是配不成**。

- 指南给的主动方案是"窗口内同时拨今日与昨日两个哈希"。**这里做不到"同时"**：在途拨号守卫的
  键是 `peer:{对端 id}`（`transport.rs:2241`），同一个对端的第二条拨号会被它挡掉。
  所以改成**按轮交替**（每轮 10 秒）：偶数轮今日、奇数轮昨日 —— 两端各自交替时最多两轮必然
  落在同一天。这不违反要求 1（那条禁的是"同一个 ch 同时存在两条自己的连接"，任一时刻只有一
  个 ch 在拨），也不引入第二条并发电路。
- 判据抽成纯函数 `relay_day_to_try(now_ms, round)` + `relay_in_day_rollover_window(now_ms)`，
  新用例 `rollover_window_alternates_the_day_per_round` 钉四件事：窗口外永远今日、窗口内
  相邻两轮覆盖两天且第二轮回到今天（不能一路往前退）、**边界两侧判据一致**（一端算窗口内、
  另一端算外等于没修）、时钟未同步时两侧都停在第 0 天且不参与交替。
- 边界写法上让了一步 clippy：`sec < a || sec >= b` 被判成手写 range contains，改成
  `!(3_600..82_800).contains(&sec_of_day)` —— 顺手也少了一处能写错的边界。
- ⚠️ 覆盖边界：跨网真机没跑（要两台不同网络的设备在 UTC 00:00 前后各开一次中继才能验到
  现场），本机只有这个纯函数判据 + 交替序的测试；窗口长度取指南建议的 ±1 小时，写死在
  函数里，不做配置项（多一个旋钮就多一处"两端可能不一致"）。
- 已登记进 macos 用例基线。windows 那份连 `relay_wiring_tests::*` 整组都还没有（属 #23：
  要在 Windows 上 dump 一次真实 `--list`），这次不猜着往里加。

Version-Bump: patch

## [4.25.3] - 2026-09-22

### Security (#32 第一片：好友身份锚点的**绑定来源**从此只认被证明过的握手，公网中继的准入因此变严)

不对称在哪：`upsert_peer` 把钥匙写进 `friends` 时要求 `peers.keys_verified`（只有验签通过的
Hello 能打这个标，注释里写清了它防的是"未签名 UDP announce 伪造 device_id + 公钥 ⇒ E2EE 被
击穿且重启不恢复"）；而**三条"成为好友"的路径读的是同一张 `peers` 表，却不过这道闸** ——
直连 `FriendAccept`、跨跳 Gossip `FriendAccept`、本机点"同意"，各写一遍、各自 `update_friend_pubkeys`。
因为那个写入是 **fill-only（首写者永久胜出）**，"谁先来"就永久决定了锚点。

为什么现在必须收：`friends.ed25519_pubkey` 的消费者从两个变成三个 —— Hello 验签锚点（INV-P21）、
安全码输入，加上刚接线的**公网中继准入判据**（`list_bound_friend_identities` 只看它非空，
ADR-0020 自己称这把钥匙为"整个设计的支点"）。绑错的后果于是从"消息被加密给攻击者"扩到
"**我们主动跨公网给攻击者建电路、并把协商验签锚在它的钥匙上**"。它没有制造新洞，但把
fill-race 的代价抬高了一档。

- 规则改成两列区别对待，且**判据只有一份**：新增 `peer_keys_trusted`（把 `upsert_peer` 里
  内联的闸提成函数）+ `acceptable_friend_keys(verified, x, e)`；三条 accept 路径合并成同一个
  `bind_friend_keys_on_accept(state, conn, id)`。`x25519` 照旧早绑（不绑就是"首次加密发送失败"，
  那三处原注释说的都是这件事；Gossip 的补齐更以"这一封能解密"作持有证明），
  **`ed25519` 只认 verified 来源**，否则留 NULL。
- ⚠️ 同一片必须一起做的第二半：`mark_peer_keys_verified` 此前全仓**只有一处调用**
  （入站首帧）。出站拨号与 BLE 两条同样验过签的路径不打标 ⇒ 收紧后"只靠蓝牙/拨号连上的好友"
  锚点永远补不上（安全码算不出、中继永不准入）—— 那会把安全改动做成可用性回退。现在三条握手
  都打标，NULL 于是是**暂时的**：任何一次验签通过的 Hello 都会经 `upsert_peer` 自愈。
- 守卫：`friend_identity_anchor_has_one_binding_rule`（三处 accept 必须都走同一个 helper、
  `upsert_peer` 那道闸不许拆、Gossip 那处**只准**绑 x25519 —— 全部用函数体 + `code_flat` 判，
  不用全文计数）；单测 `accept_binds_encryption_key_but_defers_unverified_anchor`（两列的差别、
  NULL 的自愈路径、以及"先绑上的加密钥匙不许被后来者改掉"）。两条已登记进 **macos + windows**
  两份用例基线。
- 文档同步：`docs/protocol-invariants.md` INV-P11 新增「锚点是被谁写进去的」一节（上表管绑定
  之后，这节管绑定那一刻）；`ADR-0020` 那条"支点"补上成立前提与收紧后的行为。
- 计数踩坑记两条（都是"守卫自己骗自己"那一类）：① `mark_peer_keys_verified(` 含
  `peer_keys_verified(` 这个子串，全文计数把它算成 7 次 ⇒ 把闸改名 `peer_keys_trusted`，
  而不是把断言写绕；② 单测也会调 `acceptable_friend_keys`，全文计数会误判"逻辑重复" ⇒
  改成"函数体 + 定义处"计数。
- ⚠️ 覆盖边界与真机：这是**行为变化**，不是纯重构。本机侧 630 条用例 + 两条新用例全过
  （唯一红的 `ble_file_transfer_respects_link_limits` 是隔壁工作区那行未提交的群 Offer 造成的，
  与本片无关）。需要真机确认的是：**新加好友后安全码是否照常出得来**、群密钥分发/文件首发的
  "取不到公钥"有没有变多（x25519 路径没动，理论上不该变），以及跨网新配对时中继是否会在
  锚点补齐后才放行（预期如此）。

Version-Bump: patch

## [4.25.2] - 2026-09-22

### Fixed (IPC 契约扫描器会把自己源码里的注释当成"后端在发事件" —— #33③ 第一片)

`src/api/events.test.ts` 用正则在 Rust 全文里扫 `emit(`。形态出现在**注释**里时，它会被扫成
"一个没人监听的孤儿事件"，报出一个不存在的事件名，看不出所以然。v4.24.0 现场踩了两次：
第一次是断言文本里把那几个字符连左括号写全，第二次是**写注释解释这件事，又被扫到一次**。
当时的处理都是改写文案绕开 —— 那是躲症状，护栏的缺陷还在原地（HANDOFF §5B 记的就是它）。

- 补上按语法边界的剥离：行注释、可嵌套块注释、`///` 文档注释统统抹成等长空白（**保留换行**，
  行号不偏移，别的断言不受影响）；字符串 / raw 字符串 / byte 字符串整段跳过，否则串里的 `//`
  会把后半文件当注释吃掉；字符字面量 `'a'` / `'\n'` 跳过，而生命周期 `'a` 后面没有闭合引号 ⇒
  不会被误判成字面量（这是这类小状态机最容易翻车的地方）。
- 顺带同一把尺子用于 `const NAME: &str = "…"` 的收集 —— 文档注释里举例写的常量以前会被当成
  真常量表项。
- 新夹具 `事件扫描器不被注释骗，也不误伤代码`：**正反两面都钉** —— 只断言"注释里的不算"会被
  "整段都被抹掉"这种过度剥离糊过去，所以同一份喂料里还放着真发送、串内含 `//`、raw 串、
  带生命周期参数的函数。夹具末尾另有一条**自我保鲜**断言：要求样例里仍然含有 `phantom-*`
  假形态，否则"不剥注释"的对照就不成立了。
- ⚠️ 扫描器的两条**已知边界**，都写进断言而不是靠记忆：
  ① 只认"事件名是第一个实参"的形态（`emit("x")` / `emit_filter("x")`）——本仓 `emit_to` 用了
  0 次，不为此扩正则；一旦开始用 `emit_to(target, "x")` 会漏扫，夹具里那条空断言会先提醒。
  ② **字符串字面量里**的完整 `emit("x"` 形态仍会被扫到 —— 这次刻意不修，因为要找的就是
  `emit("x")` 里那个字符串本身，"看到引号就砍"会把真发送一起抹掉。要做对得先把事件名收进
  常量表再按名比对，那是 #33③ 的另一片。

Version-Bump: patch

## [4.25.1] - 2026-09-22

### Fixed (公网中继接线留下的最后一条 dead code 把 CI 的两条 Rust job 打红了)

`6798808`（及它前面几个 relay 提交）之后，CI 上 `Rust 单测 / 清单（macos-latest）` 与
`（windows-latest）` **两条都红**，原因只有一行：

```text
error: function `parse_channel_hex` is never used   --> src/transport/relay_seal.rs:123
```

`cargo clippy --features bluetooth -- -D warnings` 只编 lib（不带 `--tests`），而这个函数
**只有同文件的 3 条断言在用** ⇒ 生产构建里它确实是死代码。relay 那一批刚接线的 7 条
dead-code 警告里，其余 6 条都被接线消掉了，只剩这一条。

- 处理：给它 `#[allow(dead_code)]` + 写清"为什么留、谁在用、和 `tcp.rs` 的
  `send_bytes`/`receive_bytes` 同一个口径"。**没有删函数**的理由是它不是废码而是
  `channel_hex` 的格式契约反边 —— 那 3 条断言守的是"64 位小写十六进制、长度与字符集都严格"，
  而服务器侧按同一个格式校验（`server.mjs` 的 `/^[0-9a-f]{64}$/`）；删函数就要连带删断言，
  格式约束只剩服务器一侧在守，客户端哪天改成大写或 base64 不会有测试报错。
- ⚠️ 顺带记一条比这条警告更值钱的机制：**CI 的 verify 是 fail-fast**，clippy 是 rust 组第 2 步，
  它一红，后面的 **`cargo test` 与 `check-test-manifest`（Rust）两步根本没跑** —— 也就是说
  "relay 那批接线的单测到底过不过"在 CI 上是未知的（本地实测：过，见下）。同一个形状
  v4.23.4/35 已经付过一次钱（那次是"verify 全绿"却漏看了 build-android 的红）。
- 另记：本仓库对"这类小修要不要 bump 版本"没有一致口径（`62fd5ca fix(gates)` 就没 bump），
  而用户协议是"每个改动一次版本提升" ⇒ 这次按协议 bump patch，口径冲突留给用户裁定。

Version-Bump: patch

## [4.25.0] - 2026-09-22

### Added (公网盲管道中继：局域网连不上时多一条链路 —— ADR-0020)

家庭宽带与手机网络普遍在 CGNAT 后面，**两侧都没有可被对方直连的公网地址**，
于是既有的"跨网段"这条路（`routed_endpoints` 填对端 `ip:port`）在这种场景下无路可走。
本轮补的是"两边各自拨号到同一台服务器上会合"这条路：服务器**只管组网**，
把两条登记了同一串通道哈希的 TCP 拼成双向字节管道，不解析帧、不落盘。
定位仍是没有强制的官方后台 —— 无账号、无注册、官方不部署，用户自己填地址才有这条链路。

三笔客户端提交各自独立：`9577a99` 配置面 → `a9ac515` 接线与记录层 → `911a6df` 设置页。
服务器代码在独立仓库 [fwd001/gosslan-relay-server](https://github.com/fwd001/gosslan-relay-server)。

**为什么改动面能这么小**（这是设计的全部价值，不是运气）：`connect_to_peer` 的链路 key
来自**握手验签学到的 device_id**，不来自 socket 地址（`discovery/routed.rs` 的 P-A01），
所以"穿过一台哑管道的字节流"对上层与一条直连 TCP 没有区别。于是
`connect_to_peer` 只多一个 `Option<RelayCtx>` 参数，密封态挂在
`TcpSender`/`TcpReceiver` 内部 —— `writer_loop` / `reader_loop` / `try_send` /
`route_order` / 分片流钉链路的签名与行为一字未改，未启用时逐字节委托底层。
**不新增任何 `Message` 变体或 `kind`** ⇒ 不触发 INV-P24 与 ADR-0007
"先铺开再上新帧"的排队，老版本设备只是用不了这条路，其余一切照旧。
`PathKind` 复用 `Routed`：选路优先级（`mesh/selection.rs` 的 LAN 恒优先、严格更优才替换）
已经在保证"局域网能连上时这条链路自然闲置"，新增枚举换不来任何行为差异。

**为什么必须加一层记录封装**（本轮的功能性目的，不是顺手加固）：正文本来就 E2EE，
但帧的其余部分是明文 JSON —— `device_id`、昵称、群名册、文件名、帧类型、已读回执全可读；
更关键的是 `Message::ChatMessage` 直连帧**没有信封签名**，只靠"链路对端 == `from`"兜
（INV-P21），而一台能控制这条 TCP 的公网中继可以**注入 `Ack`**，`Ack` 会让发送方删掉
outbox 行。所以中继电路在 `Hello` 之前先做一次带认证的协商（签名材料含双方 device_id +
通道哈希 + 角色 + 临时公钥，验签只认 friends 表绑定值），此后每条帧
`ChaCha20Poly1305(严格递增计数器 ‖ 内层帧)`，注入/重放/截断一律断链，
**失败不降级为明文**（§19 Crypto Rules）。

| 中继能看到 / 做不到 | |
| --- | --- |
| 做不到 | 读正文、读元数据（id/昵称/群名册/文件名/已读）、注入、重放、冒充好友、落盘 |
| 看得到 | 连接时刻与时长、双向字节数、帧的精确长度、同一天内"这串通道哈希又出现了" |

### Fixed (本轮测试抓掉的三个真 bug —— 都是"上线即炸"的级别)

1. `poll_read` 把底层 `Pending` 误判成流被截断 ⇒ 一条记录合法地横跨多次 socket 读
   （16KB 分片必然横跨），**每条大帧链路刚起步就自杀**。由 `duplex(1)` 的背压用例抓到。
2. 把明文拷进 `ReadBuf` 后漏 `buf.advance(n)` ⇒ 消费方永远看到 0 字节并判 EOF，
   **密封链路一帧都送不出去**。
3. 协商线原本携带长期 Ed25519 公钥：它是冗余字段（校验方本地就有绑定值），
   但一旦上线，中继就能反算出以后**每一天**的通道哈希 ⇒ 按天轮换白做。
   现在线上只有 4 个字段，并有断言钉住"协商线不得出现 device_id / 长期公钥"。

另：删掉我自己加的 5 个"以防万一"接口（`counter`/`expect`/`is_staged`/`mid_frame`/
`has_plain` 与 `WrappedSession.peer_device_id`）。它们会让 `clippy -D warnings`
报 5 条 dead_code 而把 main 弄红 —— 未接线的扩展点不该先落实现。

### Tests

- `cargo test --lib -- network::transport::relay transport::relay_seal transport::tcp`
  → **33 passed / 0 failed**。含一条端到端：两个客户端 + 一个**照文档实现的最小假中继**
  （读首行、按通道配对、之后只搬字节），协商成功后用既有 `write_bytes`/`read_bytes`
  双向换帧，其中一趟 40KB 专走"一条记录横跨多次 socket 读"的路径。
- 全量 `cargo test --features bluetooth --lib` → 628 passed / **1 failed**，
  唯一失败 `tests::ble_file_transfer_respects_link_limits` 扫描的是并行会话
  **未提交**的 `commands/group_file_dispatch.rs`（本轮未碰该文件），本轮前后同为红，
  净效果 +10 条用例、0 条新增失败。
- 服务器侧 `node selftest.mjs` 8/8、`node bench.mjs 15`（p50 0.16ms、空载 RSS 46MiB）。
- 前端 `npm test` 514 passed（含 zh/en key 集合完全一致）；`vue-tsc --noEmit` 无错。

### 已知边界与遗留（不是待办清单，是取舍）

- 中继"看起来可达"，它静默丢包时 outbox 仍按有链路的 120s 口径判失败
  （`should_fail_expired_outbox` 既有语义，本轮刻意不改）。
- 多条中继电路的 `Link.endpoint` 都是同一个服务器地址；今天 `route_order` 只在单个
  peer 的链路列表内按端点对齐，因此不受影响，但任何"按地址区分连接"的新逻辑要注意。
- 只给已绑定公钥的好友建电路，且并发上限 8 条（已有电路计入）；超出者走 Gossip 多跳。
- **安卓后台**：前台服务未实装 ⇒ 手机切后台这条链路就没了。
- 只能真机验证、本轮**未打勾**：真实 CGNAT 下的连通与延迟、三端防火墙、
  换口令后的提示是否读得懂。
- Windows 用例基线（`test-baseline.windows.txt`）需要在一台 Windows 上补
  `node scripts/check-test-manifest.mjs --update`；在 macOS 上跑会污染另一条腿。

Version-Bump: minor

## [4.24.5] - 2026-09-21

### Fixed (写出记账按 (传输 × 收件人) 成键，群投递侧补上回收 —— #35)

`file_wire_progress` 过去**只按 `transfer_id` 成键**，而一次群文件投递是**每个成员各 spawn
一个任务、共用同一个 `transfer_id`**（`group_file_dispatch.rs` 的注释里写着这件事，
当年 cancel 表正是因此改成按人成键 —— 只修了那一半）。两种坏行为都是静默的：

| 消费者 | 读的是 | 一把键时的后果 |
| --- | --- | --- |
| `stall_tick`（群侧 `dispatch:148` 也在用） | `at_ms` | 甲还在链路上走的字节会把乙的"最近有写出"一直刷新 ⇒ **真卡死的乙永远判不出停滞** |
| `wait_complete_ack` | `at_ms` | 等确认的 30s 安静窗口被别人的写出续命 |
| `wire_progress_bytes` | `chunks` | 别人走过的片数算进这一条链路的进度 |

回收侧更直接：单聊 v4.22.38 装了 `WireLedger`（Drop 覆盖十余处提前 return），**群侧从来没装**，
而守卫规定清理只许有 `WireLedger::drop` 一处 ⇒ 每发一次群文件，每个成员各留下一条永不回收的
记录（表无界增长），同一 `transfer_id` 的补发/重试进门也不是 0（第一次停滞判定被上轮残留推迟）。

- 键：新增 `file_peer_key(transfer_id, recipient)` 作为**唯一**格式，`file_cancel_key` /
  `file_cancel_prefix` 改为委托它 —— 不出现第二种分隔符拼法（"同一件事两处各算一遍"是本仓库
  反复付过钱的那类缺陷）。
- 证据点：`mark_file_wire_progress(state, msg, recipient)`，`recipient` 取**这条链路在 `links`
  表里的归属 id**（writer_loop 自己的 `peer_id`），TCP 与 BLE 两条写循环各传一次。拆出
  `bump_file_wire_progress_in(table, key, now)` 只为让"按人分开"能被单测直接喂表验。
- 读侧全部改读同一把键：`file_wire_progress_at` / `file_wire_chunks_at` / `stall_tick` /
  `wait_complete_ack` 的参数语义变成"键"，`stall_tick` 发给前端的 `file-stalled` 事件仍带
  **裸 `transfer_id`**（界面按它认传输，键里带收件人只会让它认不出来）。
- 群侧：`dispatch_group_file_to_peer` 开头装 `file::WireLedger::install(state, transfer_id,
  recipient)`，`stall_tick` 带上 `recipient`。
- **1:1 行为等价**（这就是"不能出问题"的根据）：单聊一个 transfer 只有一个收件人，
  键从 `t` 变成 `t\0peer` 是同一个桶换了个名字，`at_ms`/`chunks` 的数值路径一字未动；
  而 `resolve_stream_link(state, peer_id)` 取的链路其 `peer_id` 就是当初传进来的收件人
  ⇒ 写侧与读侧必然同名。
- ⚠️ **中继帧 `RelayChunk` 故意不记账**，不是漏：那种帧"送给谁"由帧自己的 `to` 决定、
  与写它的链路无关。按 `to` 记 ⇒ 每个**转发节点**都替别人的传输留一条记录，而转发侧没有
  `WireLedger`，谁都不会去删；按下一跳记 ⇒ 共用同一中继的两个成员被并成一个数。
  这条决定用断言钉住（`!mark.contains("RelayChunk")`），要改必须先解决"转发侧谁回收"。
  中继侧的进度记账是另一件事，不并进本条口径。
- 守卫：`file_send_progress_counts_wire_not_queue` 从三条扩到六条（⑤ 读写同键、⑥ 群侧装守卫 +
  中继不记账被钉住）；`verify-guards.py` 相应**更新一条注入文本**（旧写法已不匹配）并**新增两条**
  注入用例：键退回裸 `transfer_id`、删掉群侧那行 `install` —— 两者都必须让接线断言红。
- 新单测 `wire_progress_is_counted_per_recipient_within_one_group_transfer`（登记进 macos 基线）：
  同 transfer 的两个收件人各自记 `chunks`/`at_ms`，甲收尾只删甲那条。
  ⚠️ **没往 windows 基线加**：那边仍缺约百条（#23 要的是在 Windows 上 dump 一份真 `--list`），
  多写没命中 = windows job 直接红，而"未纳入保护"只是 warn —— 两害相权留给 #23 一次做对。
- 覆盖边界：**多成员群文件发送只能真机验**（要看的是"卡死的那个人终于被判停滞、进度不再抢先"），
  本机这套是单测 + 接线断言 + 非空转注入，不等于真机行为已确认；中继侧本条未动。

Version-Bump: patch

## [4.24.4] - 2026-09-21

### Tests (把 PR #24 的 ⑭ 静态守卫登记进非空转用例集；顺带撤回一条我自己报错的审查结论)

**登记**：`designGuards.test.ts` ⑭「预览 objectURL 的消费者不得 revoke」（PR #24 加的）
本身能红，但没进 `verify-guards.py` 的用例集 —— 而本仓库的规矩是**新源码守卫必须证明
「改坏一定 FAIL」**（v4.22.31 那条"用注入 `cmd:cargo` 自证"的假证明之后立下的）。
现在补上：注入 = 把历史上那行原样放回 `TodoImageThumb.vue` 的卸载钩子
（`if (url.value) URL.revokeObjectURL(url.value);`），必须被 ⑭ 抓住。

- 实测 `python3 scripts/verify-guards.py --only lifecycle` ⇒ **3/3 改坏 FAIL、恢复 PASS**
  （⑭ 这条 + 上一版的 VirtualList 卸载出口 + 既有的写出记账回收守卫）。
  hint 用的是守卫自己那句原文（`里出现了 revokeObjectURL`），不是"任何一条红"。
- 顺手记一条**关于审查自身的教训**：我第一次报"PR #24 用 `let _ =` 静默吞掉 `record_local`
  失败"是**判错了**。核对后：`let _ = record_local(...)` 是本仓库既有约定，共 4 处同形，
  其中 3 处早于该 PR（`network/transport.rs:4899`、`commands/group_announcements.rs:235/278`）。
  他那一行与约定一致 ⇒ 不改。**只改一处会让同一件事出现两种口径**，正是本仓库最贵的那类缺陷；
  要动就得 4 处一起动，而那要碰文件传输主链路 ⇒ 按规矩先 RCA、单独立项（已进后续计划）。

Version-Bump: patch

## [4.24.3] - 2026-09-21

### Fixed (PR #24 审查发现：滚动落定的轮询定时器没有卸载出口，组件死了它还在跑)

`scrollToIndex` 的"1.5s 落定窗口"（PR #24 为修「第一次定位总是不准」加的，每 100ms 校正一次）
**自动收口只写在 `applyJump` 内部**，而它开头是：

```ts
const el = container.value;
if (!j || !el) return;          // ← 卸载后必然从这里返回
if (Date.now() > j.until) { pendingJump = null; return; }   // ← 只有走到这里才会清 pendingJump
```

组件卸载后 Vue 把模板 ref 置 null ⇒ 每次轮询都从 `!el` 那一支早退，**永远走不到**过窗收口，
而清表的唯一出口在那之后 ⇒ 定时器以 10Hz 常驻在一个已死的组件上，不会自愈。
触发条件不是边缘场景：关掉带列表的辅助窗口、或跳转后立刻切会话，只要落在 1.5s 窗口里就漏一条。

- 修法是一行归属：`onBeforeUnmount` 里显式 `clearInterval(jumpTimer)`（与它已经在清的
  `raf` / `remeasureRaf` / `settleTimer` 同一处、同一口径）。轮询逻辑本身没动。
- 静态守卫：`designGuards.test.ts` 新增 ⑮，钉「卸载钩子里必须有 `clearInterval(jumpTimer)`」。
- 非空转：`verify-guards.py` 新增用例，注入方式是把那三行清理缩成一句自赋值 —— 行为照常、
  类型照过，只有守卫会红。实测**改坏 FAIL、恢复 PASS**，且失败输出里确实是守卫那句
  （`100ms 轮询不会停`）—— 上一版现场学到的：hint 必须对得上实际报错，否则"绿了但不知道
  是谁报的"等于没证明。
- 覆盖边界：**这是逻辑推导 + 静态守卫，不是运行时实测**。本仓库的前端测试是 `node --test`
  扫 utils 层，没有组件挂载设施，所以"卸载后定时器仍在跑"没有在真机/浏览器里量出来过；
  要真验证得给 VirtualList 加一层挂载测试（已记进后续计划）。修复本身是纯清理，最坏情况
  是"没修好但也没弄坏别的"，不会引入新行为。

Version-Bump: patch

## [4.24.2] - 2026-09-21

### Changed (补 PR #24 的记账：版本、CHANGELOG、Rust 用例清单)

`337cfc6`（yann9，经 PR #24 合入 `fd5a477`）带了一整批真机缺陷修复，但**版本记账没落地**：
message 里写了 `Version-Bump: patch`，实际只动了 `package.json`，而且填的是它分支基线上的
旧号 `4.22.41`（main 当时已经是 4.24.0）。合并时版本文件按 main 保留 ⇒ 这批功能在 main 上
**既没有版本号也没有 CHANGELOG 条目**。判据 4（v4.23.3 加的那条"声明必须落地"）在 CI 上
把这次点名了 —— 守卫按设计红了，不是误报：
`frontend` 组 fail-fast 之后，**CHANGELOG 结构 / `npm test` / `vue-tsc` 三步在他的代码上
从没跑过**。所以本次记账同时把这三步在合并后的树上补齐：全绿（`npm test`、
`npm run build` 的 vue-tsc 类型检查、以及合并后整棵树的 `verify:full` 15 步）。
条数按仓库惯例由各工具自己打印，不写进文档。

这批修了什么（细节与根因见 `337cfc6` 的 commit message，此处只记账不重述）：

- **群任务图片**三个互不相干的根因：表单选图后无法预览（`todo_image_meta` 顺手登记本机内容
  副本）、详情图大概率裂（`TodoImageThumb` 卸载时 revoke 了**缓存共用的那个 objectURL**，
  改为不 revoke + 静态守卫 ⑭ 钉住）、图片点不开大图（`ImageLightbox` 按 cid 解析相册条目）。
- **群任务独立窗口**：设计尺寸 560×620 → 780×620；只记大小不记位置，几何落地后按 label
  还原尺寸并重新居中，夹在 `[min, design]` 之间（新用例
  `restored_aux_window_size_is_clamped_between_min_and_design`，本次登记进 macos + windows 两份基线）。
- **选中框与相邻条目白线**：多选/引用定位统一为整行满宽浅底同色；白线真根因是虚拟列表按
  **取整后**高度定位而条目盒子是内容自然高（代码气泡 256.5px）⇒ 相邻差 0.5px，改为保留小数。
- **表情回应桌面入口**：飞书式悬停笑脸 → 气泡外侧选择器，`absolute`/`Teleport` 不改消息高度；
  选择器自算 fixed 坐标，且**自己内部的滚动不再把自己收起**。
- **引用消息**：顺序与样式按微信；点击=直接查看被引用内容，原消息不在本机时明确提示而不
  无脑跳转；「定位到引用的消息」进右键菜单与长按面板。
- **列表定位**："第一次总是不准"落定改为以**真实 DOM 位置**为准 + 按 **msg_id（不是下标）**
  在 1.5s 窗口内轮询校正（历史 prepend 会让同一整体下标指向另一条消息）。
- **卡片消息可复制/可收藏**：判据收成单源（`utils/messageKinds` 的 `isForwardableKind` /
  `isFavoritableKind`、`utils/cardText`、`utils/fileMeta`），右键菜单、长按面板、收藏页
  三处从此共用 —— 消除的正是"同一件事多份实现互相漂移"这一类根因。

⚠️ 本次记账同时记下审查里发现的三件事，其中两件随后各起一版修掉（VirtualList 落定轮询的
定时器未在卸载时清理、`todo_image_meta` 用 `let _ =` 吞掉登记失败），一件留作后续：
预览 objectURL 收归缓存所有之后，`invalidateFilePreview` 只 `cache.delete` 不 revoke 且缓存
无上限 ⇒ 长会话里每条图片（上限 15MB）的 Blob 永不释放 —— 需要的是缓存自己的 LRU 出口，
不是把 revoke 加回消费者。

Version-Bump: patch

## [4.24.1] - 2026-09-21

### Fixed (好友只是不在线，却被说成"版本较旧" —— 1:1 门控的两种挡下原因分成两句话)

#37② 的"小半"。根因不在门控方向，在**文案把一个未知当成了事实**：
`commands/chat.rs::send_message` 读能力位图时写的是 `.get(&friend_id).copied().unwrap_or(0)`，
于是"表里没这条"和"表里有这条、值是 0"塌成同一个 0，共用 `kind_unsupported_hint` 那句
"**对方的 Gosslan 版本较旧**，不支持「merge」"。

可这两件事的真相相反（`peer_content_features` 的写入点只有一个 = 验签通过的 Hello，
回收点 = `sweep_peers`，而且是内存态）：

| 表里的状态 | 真相 | 该说什么 |
| --- | --- | --- |
| `Some(0)` 之类、缺该能力位 | 它**自己发过 Hello** 且没声明这个能力（老端该字段 `#[serde(default)]`）⇒ 确知不支持 | 沿用原句：让对方升级 |
| `None` | 从没交换过 Hello：对方此刻不在线、本机刚重启、或链路还没建 ⇒ **能力未知** | 说"还不知道"，等对方上线后重发 |

第二种恰恰是常态（这两张表重启即空、离线即回收），而它对用户的指令完全相反：看到
"版本较旧"的人会去催对方升级，而真正要做的只是等对方上线。

- `protocol::kind_blocked_hint(kind, Option<u32>)`：新入口，按 `Some`/`None` 选句子，
  `Some` 分支**直接调用** `kind_unsupported_hint` —— 句子还是只有一句，不出现第二份文案。
  两个函数并排住在 `protocol.rs`，判据与它的解释继续同处。
- 发送点只改读法：`Option<u32>` 交给判据 `.unwrap_or(0)`，**判据仍是 `kind_allowed_by_features`
  唯一一处，挡不挡、何时挡一字未动**（未知照旧不发）。
- 接线守卫扩到"文案入口"：`new_message_kinds_are_gated_at_the_send_path` 现在同时钉
  判据唯一（含新增 `pub fn kind_blocked_hint(` 只许一处）与"发送口不许绕过三态入口自己挑文案"。
- 新单测 `blocked_hint_distinguishes_unknown_from_declared_unsupported`（已登记进 macos 与
  windows 两份基线）：未知那句**不许含**"版本较旧"、必须给下一步（含"不在线"与失败气泡上那个
  「重新发送」按钮的原词）；确知那句必须
  `== kind_unsupported_hint(...)`；并再钉一次"两种情况都得先挡下来"。
  措辞上删掉了"去看联系人详情页的「对方版本」"这一指引 —— 那一行**刻意只在知道点什么时出现**
  （`FriendProfile.vue` 的 `v-if`），未知时它根本不在，指过去是第二次误导。
- `verify-guards.py`：① 既有用例的注入文本跟着代码改（旧文本已经不匹配了）；② 新增一条
  "把三态入口退回旧那句"的注入 ⇒ 实测改坏 FAIL、恢复 PASS。
  过程中现场撞到 `expect_fail_hint` 对不上：注入先被**前一条**断言抓到，hint 必须写实际报出的
  那句 ⇒ 已改成断言原文里的子串，避免"绿了但不知道是谁报的"。
- 文档：`docs/protocol-invariants.md` INV-P24 第 4 条的"未知即不支持"补上"决策方向 vs 解释
  分两句"；同一格的"残留"一行原本写着群侧"老成员只会静默看不见"，v4.24.0 已经不静默了，
  顺手按事实改成"渲染退化 + 发送方收受众预告"。
- 覆盖边界：只改了"为什么没发出去"这句话，**没改**发得出去与否；离线对端仍然是"发不出去
  而不是延后判断"（那要 outbox 载荷可改写，本轮按约定不碰）。真机上"对方不在线时发合并转发"
  的新文案效果未验。

## [4.24.0] - 2026-09-21

### Added (群聊发 merge / 未来受门控的 kind：先说清"谁会把这条看成原始文本")

INV-P24 第 4 条在 1:1 是"不门控不许发"，群侧一直没接。**接之前先考古**（git 历史实测，结论
推翻了这条待办原本的假设）：两条接收路径根本不是一回事 ——

| | 单聊 | 群聊 |
| --- | --- | --- |
| kind 住在哪 | `ChatMessage.kind`（老版本是嵌套枚举） | 群密钥加密的 Gossip 载荷里，**从来不在** `MsgKind` |
| 老对端收到不认识的 kind | 整帧解析失败，且读循环 `Err(_) => break` ⇒ **丢帧 + 断链** | `parse_gossip_payload` 按**自由字符串**解析（同一份实现逐字见于 v2.1.2 / v4.3.9 / v4.8.2 / v4.20.0），DB `kind TEXT` 无 CHECK ⇒ **不丢帧、不断链** |
| 实际退化 | 发送方反复重投、最后假报"发送失败" | 只有渲染：老成员把这条看成一段原始文本（V3b 只修了我们这侧） |

⇒ 群侧**不该拦发送**（为了少数人的难看排版挡住整群消息，代价大于收益），该做的是**把已知的
事实说出来**。所以：

- `protocol::kind_audience` + `kind_audience_hint`：三态分开数 —— 确知缺位 / 版本未知 /
  支持。刻意**不复用** `kind_allowed_by_features` 那个"不知道就当不支持"的默认值：群成员
  离线是常态，而能力位图只在内存里、重启即空 ⇒ 把"未知"并进去会让提示每次都在响，而一条
  永远在响的提示等于没有提示。"版本未知"另起一句说。
- 发送内核（`send_group_payload`）在消息发出后按需发一条 `content-audience` 事件；接在
  内核而不是命令入口，是因为文本/代码/合并/任务/投票/公告全走这里，将来新增受门控的 kind
  自动被覆盖。`let _ =` 发送 —— 提示送不出去不该影响已经发出去的消息。
- 前端 `bindEvents` 接上并 toast 那句（文案整条来自 Rust，判据与句子在同一处）。
- 守卫：`group_audience_is_wired_and_keeps_three_states` 钉"判据唯一 + 内核真调用 + 受众段
  不拒发"；`api/events.test.ts` 双向核对事件名两端（它本来就在，这条新事件是它自动接住的）。
- ⚠️ 记两条今天现场踩到的坑（都是本仓库已有先例的那一类）：
  ① 守卫断言里把那三个字符加左括号写全，会被 `events.test.ts` 的全文扫描当成一个"没人听的
     事件"；改成注释里解释这件事、**再写一遍又被扫到一次** —— 最后是改写文本才过。
  ② 我顺手"加固"扫描器（从第一个 `#[cfg(test)]` 截断），结果把 `transport.rs` 6185/6220 两处
     真实发送点连着 4600 行生产代码一起砍掉 ⇒ 5 个既有事件误判成"没人发"。**已撤回**，
     扫描器的缺陷（全文文本扫描、分不清代码与字符串/注释）挂进 #33③，等一次做对。
- 覆盖边界：这条提示是 fire-and-forget 的 toast，没做"发之前先确认"和"改成发纯文字摘要"
  —— 后者要的是 ② 那条 outbox 能力（离线成员在 Hello 到货时再判），单独立项。真机上
  群里放一个老版本成员看这句话的效果，还没验。

## [4.23.5] - 2026-09-21

### Fixed (上一条把安卓出包 CI 打断了 —— 门禁的"空转硬失败"只属于门禁自己的 workflow)

v4.23.4 加的那条"CI push→main 拿不到 before..sha ⇒ 退出码 1"是**越界的**：main 上
`0d151f2` 之后 `build-android` 的第 12 步「Build release APK」红了。

机制（本地按 CI 环境复现过，不是猜）：`scripts/build-android-releases.sh` 的守卫清单里也
跑 `check-change-budget.mjs`，而那个 workflow 从没映射 `GITHUB_EVENT_BEFORE` ⇒ 新加的硬失败
在**出包**流程里触发 ⇒ 循环 `exit 1` ⇒ 整条构建中止。复现命令与结果：同一串守卫在
`GITHUB_EVENT_NAME=push GITHUB_REF_NAME=main`（无 before）下逐条跑到
`check-change-budget.mjs` 退出 1，前四条全过。

判错的地方不是"要不要暴露空转"，而是**由谁承担**：判不到范围，对门禁 workflow 是"你在骗我
说绿了"，对出包脚本只是"这道门禁这次没看过东西"。把前者套到后者身上，等于让一个静态检查有
能力打断发布。

- 硬失败改成**显式 opt-in**：只有 `GOSSLAN_BUDGET_STRICT=1`（verify.yml 声明）时
  push→main 拿不到 before 才退 1。其它消费方最多拿到 2（零覆盖）。
- `build-android-releases.sh`：守卫循环把退出码 2 当**警告并继续**（打印"零覆盖 ≠ 放行，是
  它无对象可判"），其余非 0 照旧中止。
- `build-android.yml` 也补上 `GITHUB_EVENT_BEFORE`：它的守卫清单既然包含这道门禁，就该喂
  给它真正的范围 —— 但**不开** strict，出包与门禁的职责分开。
- 三场景退出码实测：`before 有 + 非 strict` ⇒ 0；`before 无 + 非 strict` ⇒ 0（旧形状不再打断
  出包）；`before 有 + strict` ⇒ 0；（`before 无 + strict` ⇒ 1，v4.23.4 已测）。守卫循环单独
  跑真实零覆盖：打印 ⚠️ 后继续，整段 exit 0。
- ⚠️ 证据边界：v4.23.4 那次"CI 全绿 ⇒ env 映射生效"的推论只对 **verify.yml 的 frontend job**
  成立（那里确实绿了，Change Budget 第一次判到 `before..sha`）；同一次 push 的
  `build-android` 红被我漏看了 —— 汇总时只核了 verify 的三条 job。这条修复的最终证据是
  下一次 push 的 `build-android` 变绿。

## [4.23.4] - 2026-09-21

### Fixed (Change Budget 在 CI 上其实是零覆盖 —— 「空范围即绿」现在单独成一类)

#33② 的怀疑成立，而且比"报告得不够细"更糟：**这道门禁从来没在 CI 上判过任何东西。**

机制：`check-change-budget.mjs` 在 push 事件上要的是 `github.event.before..github.sha`，
而 **`GITHUB_EVENT_BEFORE` 不是 Actions 的默认环境变量**（默认只有 `GITHUB_SHA` /
`GITHUB_REF_NAME` / `GITHUB_EVENT_NAME` / `GITHUB_EVENT_PATH`），`verify.yml` 从没映射过它
⇒ 脚本静默退到 `origin/<分支>..HEAD`，而 push 之后远端 ref 已经指向 HEAD ⇒ **空范围**
⇒ 判 0 个 commit 然后打一行"自然通过"退出 0。`fetch-depth: 0` 那段注释一直在说在做
before..sha 探测：探测代码在，输入没喂进来，于是那句说明长期是假的。

- `verify.yml`：补上 `env: GITHUB_EVENT_BEFORE: ${{ github.event.before }}`（连"为什么必须有
  这一行"一起写在该文件顶部，别再靠注释口头承诺）。
- **push→main 上零覆盖 = 硬失败**（退出码 1）。这是"CI 绿"第一次真的等价于"Change Budget
  判过这次推送的 commit"：范围来源不是事件里的 before..sha 就红，且红意走 `ci-run.sh` 的
  注解通道 ⇒ 匿名可读。**下次 main push 全绿本身就是这条修复的证据**。
- **零覆盖不再冒充通过**：脚本新增退出码 2，`verify.mjs` 把它单列成"⚠️ 零覆盖（没判到任何
  对象）"，与 ✅/❌/⏭ 并列（本地 push 之后重跑就是这个状态 —— 正确，但不许读成"守住了"）。
- 范围候选改为"取第一个非空"：`origin/<分支>..HEAD` → 非 main 分支再试
  `origin/main..HEAD` → 只有 CI 才兜底 `HEAD~1..HEAD`。本地故意不给这条兜底：那会把已推送的
  commit 反复重判，又误红又假装判过了。
- 顺手清掉两处会腐烂的数字：`verify.mjs` 头部的"457/505"、`check-test-manifest.mjs` 头部的
  "455/503/87"（实际今天 501 条前端断言）—— 与 v4.23.0 在 verify.yml 里做的是同一件事。
- ⚠️ 一处**自我不一致**记在案：上一版 a7edb69 前缀写 `feat(gates)` 而 trailer 声明 `patch`，
  按 `semver.mjs` 的类型表 `feat ⇒ minor` ⇒ 这一对不自洽（`version:check` 不在 CI 上，所以
  没人报）。gates 类改动产品无变化就是 patch，以后这类前缀一律用 `fix` / `chore`，别让前缀
  与 trailer 各说一份 —— 尤其现在 trailer 是判据 3 的输入。

**验证**：三场景退出码实测 —— 本地已 push ⇒ `2`；模拟 CI 且 before 已映射 ⇒ 判到
`c9e5193..a7edb69` 这 1 个 commit、`0`；模拟 CI 但 env 缺失 ⇒ `1` 并指名"先查那段 env"。
`npm run verify` 快速层 9 步 ✅（Change Budget 显示为"⚠️ 零覆盖 0.1s"而不是 ✅）、
`verify-guards.py --only frontend` 46/46 ✅、verify.yml 经 Psych 解析通过（env 键存在，
steps 4/6/10 未变）。边界：零覆盖/空转这两条新退出码**没有**非空转用例 —— 注入模型是"改坏
必须 FAIL"，而它要求的是"没东西可判时必须非 0"，方向相反，需要的是另一条接缝；本轮用上面
三次实测代替，缺口留在这里说明白。

## [4.23.3] - 2026-09-21

### Changed (Change Budget 加了第 4 道判据：`Version-Bump` 不落地就是没写)

PR #22/#23 留下的真实缺口：`e5770ce`、`f6bb81c` 两条提交都写了 `Version-Bump: patch|minor`
trailer，但四个版本清单文件**一个都没动** —— 版本最后是 v4.23.0 手工补的账。当时判断这是
"作者不守规矩"，实际更像新旧口径并存：`docs/VERSIONING.md` 写的仍是"提交只声明、攒一批再
`version:release`"那套流程，而 4.22.x 起的实际做法是**每个提交自带 bump**。

- **判据 4（新）**：写了 `Version-Bump: <档>` ⇒ 同一个提交必须动满 `package.json` /
  `package-lock.json` / `Cargo.toml` / `tauri.conf.json` 四个文件；反方向同样成立（四个都动
  了却没声明 ⇒ 红）。`Cargo.lock` 不要求 —— 它由 cargo 在构建时同步，可以合法晚一版。
  `chore(release)` 前缀豁免"声明"那一半（那条流的档位写在 subject 里）。
  近 200 个提交实测：只有上述两条命中，其余形状全一致 ⇒ 不会误伤历史。
- **判据 3 的窗口改吃声明，不吃前缀**：原先 `git log --grep=^fix` ⇒ 把一条修复写成
  `feat(ui): …… + 权限修复` 就永久看不见它。现在"修补形状" = 声明 `patch`（断言没有新
  能力），前缀是 fix 还是 feat 都进窗口；没有声明的旧提交仍按 `^fix` 判，不缩小既有覆盖。
  这一条之所以能成立，是因为判据 4 先把 trailer 变成了**会被验证的输入** —— 否则
  "窗口看声明"等于"窗口看一个可以随手乱写、也可以随手不写的字符串"。
- 顺带修掉一处会静默错数据的形状：`git log --format` 每条记录后补的换行留在**下一块**开头，
  按 \x01 切块取 sha 时只有第一条能对上 key，其余全被当成"没有 trailer"。第一版就是这么红
  着被我 dry-run 抓出来的（12 条真实提交里 5 条误报）；已加 trim 并写明不 trim 的后果。
- 文档对齐：`docs/VERSIONING.md` 的 §3 改成实际口径，并写清 `version:check` **没有**进 CI
  （它的记账半边在历史上是红的），CI 上真正生效的是判据 4；`AI_PROJECT_HANDOFF.md` 那一行
  原本只指向 `version:check`，等于把 AI 引到一条不在门禁里的闸。
- 窗口取数从"另跑一次 git log"改成复用已解析的受检提交：两次各取一遍是上一条误伤的成因，
  也省掉一处"取窗口失败 ⇒ 静默跳过判据 3"的降级路径（拿不到 message 现在直接报错退出，
  空转的门禁比没有更糟）。
- 非空转：3 条新用例（声明没落地 / 真 bump 缺声明 / `feat` 前缀 + patch 声明进窗口），
  全部实测"改坏即 FAIL、恢复即 PASS"；原 4 条照旧。第 3 条是**唯一**能守住"别退回只看
  前缀"的用例 —— 用 `fix(ble)` 注入的那条改回去照样会红，看不出差别。

## [4.23.2] - 2026-09-21

### Fixed (@提及 高亮的底色兜底：两处组件各抄了一份主题 token 的值)

`mentionHighlightColor` 要拿"实际渲染的底色"算对比度，读不到底色时由调用方兜底 —— 而两
处调用方各自把颜色**写死**了：

| 调用点 | 读不到的底色 | 兜底（暗 / 亮） |
| --- | --- | --- |
| `MessageTextBubble.vue` | inline `--bubble-bg-raw` + DOM 都拿不到 | `#1c2434` / `#eeeef0` |
| `MessageContentModal.vue` | CSS 变量 `--gosslan-panel` 读空 | `#1e293b` / `#ffffff` |

两对兜底都不是凭空的数：`#1e293b / #ffffff` 正是 `src/style.css` 里 `--gosslan-panel`
的暗/亮两个值（`:217` / `:86`），`#1c2434 / #eeeef0` 正是 `--gosslan-card` 的暗/亮两值
（`:222` / `:95`，而 `#eeeef0` 同时还是 `chatStyle.ts:116` 的 `LIGHT_OTHER_BUBBLE`）。也
就是说主题色值被抄进了组件，而且**两个调用点抄的还是不同的 token** —— 同一句话在气泡与
全文弹窗里算出的高亮色本就可能不同；token 一改，抄本静默失准。

改为**消掉字面量**而不是搬运它：`bubbleBg` 变可选，底色未知时不做对比度试算，直接取候
选梯度的末端（暗色最浅档 / 亮色最深档）。梯度本身就是"离主题中性底越来越远"的排序，末
端对主题内任一底色对比度最高，因此不需要一个新的颜色事实来兜底。

- 新增断言：未知底色下全主题 × 明暗都得拿到色、对主题内两种气泡底仍 ≥ 4.5、且不得比试
  算结果更靠梯度内侧（末端选反了会红）。
- 新增源码守卫：组件里再出现 `mentionHighlightColor(..., "#xxx")` 即红，并在
  `verify-guards.py` 登记为用例，**注入兜底色实测改坏即 FAIL**（不是空转）。
- ⚠️ 覆盖边界：这条只改"底色读不到"那条分支；底色读得到时仍走原来的逐档试算，未验证
  的兜底色理论上不再保证 ≥ 4.5（原来那份保证是针对**猜的底色**给的，本身就是假的）。
  真机上 @提及 的可见颜色未复验。

## [4.23.1] - 2026-09-21

### Changed (任务鉴权：删掉不参与判权的入参，权限表按代码实际那样写)

#22 把群主权限放宽成"两档"之后，`may_update_todo` 的 `edits_assignees` 一次都没被读
（签名里留成 `_edits_assignees`），但函数上方那张表仍在单独讲"改指派人"一档 ——
调用点的注释更是写成"三档"。这类**说明与实现各说一份**的形状，本项目已经反复为它付过钱。

改的不只是那个参数，还有三处口径：

- 判据其实只有一个输入：`edits_structure = deleted || title != def.title`。
  所以**描述 / 图片 / 指派人 / 状态 / 归档同属第二档**（被指派人可改），表按这个写。
- 删参数；调用点那个 `edits_assignees` 局部变量**保留**，但它只用于挑错误文案
  （同一档里三种角色各自的提示不同），不再参与判权 —— 这点在调用点写明了，
  否则下一个人会以为它还有判权语义。
- 判定顺序的理由照旧成立且更重要了：**必须先问是不是结构改动**，否则
  "同时改指派人 + 标题"会被被指派人那一档一并放行 —— 这正是旧实现踩过的形状。
- 测试矩阵去掉只换那个无效维度的重复行：16 条 → 10 条，**每条都是不同行为**
  （创建者两档可 / 被指派人第二档可、第一档拒 / 群主两档都可 / 无关成员两档都拒 /
  外加"actor 恰好等于群主才算群主"的自洽检查）。断言数降下来不是放宽覆盖 ——
  原来那 6 条重复行判的是同一件事。测试头注释里过期的"三档"描述同步改对。

验证：`cargo test --lib` **580 passed / 0 failed**（与改前同数，矩阵仍是一个用例名）；
`cargo fmt --all` 已跑。这条不改行为，所以没有真机项。

## [4.23.0] - 2026-09-21

### Changed (CI 与本地的"跑哪些检查"合成一份：CI 只说跑哪一组)

`.github/workflows/verify.yml` 以前把 15 项检查**又抄了一遍**（10 + 4 + 1 个 `run:` 步骤），
于是"清单"有两份、必然漂移 —— 证据就在同一个文件头部：那里挂着「457 条前端断言 +
503 条 Rust 用例 + 93 条非空转护栏」，而这三个数字早就对不上实际。

v4.22.41 已经把归属做成了步骤表上的一列（`group`）并强制声明，这一版把 CI 切过去：
三个 job 各自只跑 `node scripts/verify.mjs --group frontend|rust|android`，
环境准备（`fetch-depth: 0`、`npm ci`、rust-toolchain/rust-cache、JDK+NDK、
`ci-run.sh` 的 check 注解通道）全部留在 YAML —— 那些是"怎么准备机器"，不是"跑哪些检查"。
步骤级理由（`--features bluetooth` 不能省、清单守卫是它的兜底、Change Budget 需要历史）
改由 verify.mjs 里各步的 `why` 承担，不再在两处各写一份。

顺带删掉头部那三个腐烂数字（并且注明它们曾经烂在那里）。

### Added / Fixed (PR #22 主题色体系 + 移动端下钻页统一 + 群任务编辑权限，记账补录)

**这批改动在合并时声明了 `Version-Bump: minor` 却没落地版本号、也没写本文件小节**；
它们早已在 main 上，这一节是补记账（作者：yann9，PR #22，+1845/−620 / 57 文件）。

- **主题色体系**：呈现层颜色 token 化（`src/style.css` 的 `--gosslan-*` 一族 +
  `src/utils/chatStyle.ts` 用户可自定义聊天底色 + `color.ts` / `platform.ts` 配套），
  以及 `src/utils/designGuards.test.ts` 里的设计守卫。
- **移动端下钻页统一**：转场、返回箭头、列表风格收口（`MobilePageFrame`、`AuxWindowShell`、
  `useBackLayer` 等），`FavoritePanel` / 「我的」/「链接」几个下钻页对齐。
- **群任务编辑权限**：`may_update_todo` 改为「创建者或群主 ⇒ 一切字段；非群主的结构改动
  （标题/描述/图片/删除）一律拒；其余看被指派人」。起因是用户报"群主不能编辑群任务"
  —— 此前群主改描述/状态会落到"被指派人"那一档被拒。鉴权仍在命令层，
  配套测试是四角色矩阵（`alice`/`bob`/`carol`/`owner` × 组合，含 5 条负例，
  `commands/logs_tests.rs`）。
- 审查记录（不改代码，见 commit 报告）：这一批留下两处"事实抄两份"待清理
  —— @提及高亮的面板底色在 `MessageTextBubble.vue` 与 `MessageContentModal.vue`
  各写一份兜底且**两处值不同**；`may_update_todo` 的 `_edits_assignees` 已成死参数
  而函数上方权限表仍在讲那一档。均登记为待办 #40。

### Fixed (PR #23 一批移动端 / 设置缺陷修复，记账补录)

同样是**合并时声明了 `Version-Bump: patch` 却没落地版本号、没写小节**的补录
（作者：yann9，PR #23，25 文件 / +496−76）。

- **弹窗 z 序**：`BaseModal` 原先 `z-50`，而 HeadlessUI 的 `Dialog` 挂到 body 下的 portal 根，
  与移动端整页下钻（`z-[60]`）在文档根层级比 ⇒ 弹窗"DOM 里有、屏幕上看不见"，
  用户体感是"点了没反应"。修法之外还加了静态守卫：整页框架 < 弹窗 < 右键菜单 < 图片预览/Toast
  （`designGuards.test.ts` 第 ⑫ 条，带反向清单校验）。
- **安卓首启动渲染成桌面三栏**：真因是启动时系统权限弹框盖在 WebView 的**首次布局**上，
  `matchMedia("(max-width: 767px)")` 读到兜底视口宽度（980 那档）⇒ `isMobile=false`。
  两侧互补修法：注入侧把 `requestRuntimePermissions()` 推到 `decorView.post { post { … } }`
  （首帧 traversal 之后），前端侧新增 `platform.ts::resolveMobileLayout`（平台优先、宽度兜底）。
- **返回栈在 pushState 不可用时的自触发 pop**：退化用 `location.hash` 压条目会**自己**引发一次
  `popstate` ⇒ 刚压入的层被立刻关掉（"弹窗一出现就消失"）。现在 `HistoryPort.push` 返回落点、
  纯逻辑吃掉那一次。
- **导航选中态**：「链接」的指南针图标缺 `:fill` 绑定 ⇒ 选中只变色不实心；补上并加守卫 ⑬
  （清单外的 `navState` key 会红，防清单过期）。
- 设置页结构/骨架主色等零项，详见该 commit。

## [4.22.41] - 2026-09-21

### Changed (门禁"跑哪些检查"收成一份：步骤表加 CI 归属，CI 只需说跑哪一组)

`verify.mjs` 头部第 28 行一直写着"这个脚本让 **CI、发布脚本、本地开发跑同一套**"——
这句是**愿望被当成事实**：CI 用的是 `.github/workflows/verify.yml` 里另一份手写的 `run:`
列表，两边各自维护。腐烂已经发生并且看得见：verify.yml 头部那句
「457 条前端断言 + 503 条 Rust 用例 + 93 条非空转护栏」早就不对了（Rust 用例现在 596 条），
而 `verify.mjs` 自己也有一份同类数字（"505 条用例"、"457 条前端断言"）。
两个清单 + 两份腐烂的计数，就是 #33① 说的"漂过两次"。

**这一步（A）只做本地侧，CI 一行不动**（CI 是其余所有工作的安全网，不能顺手改）：

- 每个步骤多一列 `group: "frontend" | "rust" | "android"` —— 就是 CI 那个 job 的名字。
  它是**步骤对象上的一列**，不是第二份清单，所以没有"两处不同步"这件事可言。
- `group` 刻意**不从名字派生**：归属表达"哪台机器、带什么工具链跑"，与"快不快/要不要编译"
  是两条正交的轴 —— 现成反例就在步骤表里：「护栏非空转（前端子集）」按分层是**重门禁层**
  （全量子集要重编译 Rust 用例），但 CI 把它放在 **frontend job**。合成一个字段两头都判错。
- 强制声明：漏写 `group` 时 `--list` 当场退出码 1。理由不是格式洁癖，而是漏声明的后果是
  **这条门禁本地照跑、CI 永不跑，两边看都是绿的** —— 比写错更难发现。
- `--group <名字>` 跑该组全部步骤，并且结束时**点名"不属于本组"的每一项**（组模式同样不许
  用一片绿暗示"全套过了"）；组模式下跳过分层自检（那把尺子量的是快速层，量组模式会假红）。
- 顺手把最终成功文案改成 mode-aware：`--group frontend` 跑完不再谎称"快速层门禁通过"。
- 干掉 `verify.mjs` 里 4 处写死的条数（护栏/前端断言/Rust 用例/--full 说明），改成
  "条数交给该步自己打印"。仓库里早有这句注释，只是没照做。

**下一步（B，等你点头）**：把 verify.yml 的三个 job 换成
`node scripts/verify.mjs --group frontend|rust|android`，环境准备（checkout `fetch-depth: 0`、
`npm ci`、rust-toolchain/rust-cache、JDK+NDK、ci-run.sh 注解）全部留在 YAML —— 那些是
"怎么准备机器"，不是"跑哪些检查"。这一步会动 CI 执行形态，而本机没法验证 Windows/Android
两条腿，所以单独一个提交、单独一次确认。

### 验证

- 三组里两组**真跑过**（不只是 `--list`）：`--group frontend` 10 步 ✅ / 99.9s（正是 CI
  frontend job 将来执行的命令）；`--group rust` 4 步 ✅ / 98.5s（fmt → clippy → 单测 → 清单守卫）。
  android 组只核过选择正确（本机跑一次交叉编译要一分多钟，收益不抵时间）。
- 默认路径没被改动碰坏：`npm run verify` 快速层 9 步 ✅ / 6.8s / EXIT=0。
- 新护栏非空转实测：「每个步骤都必须声明 CI 归属」——摘掉某条的 `group` ⇒ `--list` 退出码 1，
  恢复 ⇒ 0。
- 组模式输出诚实：跑完点名"不属于本组"的每一项（frontend 组列 5 项、rust 组列 11 项），
  并显示 `--group rust 那一组通过` 而不是"快速层门禁通过"。
- **本轮 CI 一行未动**，所以不存在"改坏安全网"的风险；但也要说清：收益要到 B 落地才成立，
  现在 `verify.yml` 那份 `run:` 列表已经是**多余的第二份**，只是这次多了强制声明做兜底。

## [4.22.40] - 2026-09-21

### Fixed (门禁分层的自证补上非空转验证 —— 顺便修正一次"证明过了"的假证据)

v4.22.31 给 `verify` 分了两层，并在旁边加了一条自证：*快速层里不许出现"会碰 Rust 工具链、
却没被归进重门禁层"的步骤*。当时我报告"这条自证验过非空转"，**那个证明是无效的**：
注入方式是往步骤表里加一条 `cmd:"cargo"` 的步骤，而 `cargo` 步骤本来就会被 `isHeavyStep`
归进重门禁层 ⇒ 它永远不可能"漏"，所以那次跑出来的"通过"什么也没证明。
（当时的备用方案 —— 注入一条 bash 步骤 —— 被权限策略拦下，于是这件事就一直欠着。）

真正能触发它的组合必须是"**会碰工具链** 且 **没被归层**"。所以注入改成：把
`移动端编译门禁（Android）` 这条 `cmd:"bash"` 步骤**改名去掉关键词** —— 名字不再命中
`isHeavyStep` 的关键词，而 `mayTouchToolchain` 仍认它是 bash ⇒ 它落进快速层，
`npm run verify` 必须当场以退出码 1 红掉。已登记为 `verify-guards.py` 用例
「门禁分层的自证不是装饰」，实测「改坏即 FAIL、恢复即 PASS」。

顺手做的两个小选择，都是为了让它留在**日常**子集而不是发版前才跑：
命令用 `node scripts/verify.mjs --list`（自证在 `listOnly` 分支**之前**执行，
所以既不编译也不跑测试，秒级出结论），标签打 `["gates","frontend","new-guards"]`
（`frontend` 让全量层的日常子集就覆盖到它）。

同时改掉 `verify.mjs` 里那段说"本条自证尚未做过非空转验证"的注释 —— 它现在做过了，
而一条写着"没验证"的注释会让人重新怀疑一个已经守住了的门禁；注释里也把那次无效注入
记了下来，免得下次有人再用同一种假证自骗。

**为什么这条值得单独一个版本**：它不影响功能，但它管的是"会不会有一天 `npm run verify`
悄悄变成三分钟、于是大家开始绕过它"。本项目反复得到的教训是：**会让人想绕过的门禁
等于没有门禁**，而"我以为它守住了"比"它没守住"更危险。

### 验证

- 新用例单独跑：`verify-guards.py --only "分层"` → 「改坏即 FAIL、恢复即 PASS」。
- 日常子集整体跑：`verify-guards.py --only frontend` → **44 条全过**，新用例排在 `[1/44]`，
  GUARDS_EXIT=0；跑完核对 `verify.mjs:263` 的步骤名已还原（护栏会改写源文件，
  不核对就是把实验现场提交进仓库）。
- `npm run verify` 快速层 9 步 ✅ / 6.8s / EXIT=0；`scripts/verify.mjs` 改的是注释，
  分层判定逻辑未动 —— 而它正是被上面那条注入用例覆盖的对象。
- **本轮没跑 `verify:full`**，理由是 §37.1 的口径：一行 Rust 都没改，重门禁层的
  兜底是 CI（每次 push 全跑）。这是"按规则跳过"，不是"忘了跑"。
- 基线仍 596（无新增 Rust 用例）。`Cargo.lock` 已同步到 4.22.40 ——
  `version:patch` 不刷 lock 这个老坑今天照样要手动 `cargo check` 补一次。

## [4.22.39] - 2026-09-21

### Added (INV-P24 第 4 条「不门控不许发」落地 —— 而且它第一件事是修一个现存 bug)

协议不变量里最后开着的第 4 条要求"第一个新帧上线前必须先补门控"。听起来是给假想需求
修基础设施，但 RCA 一查发现**它已经欠了一个真实现场**：

`MsgKind::Merge`（合并转发卡片）是 V1 期间才加的（`9b26006`，最早随 v4.22.30 这条线发布），
而 **v4.8.2 / v4.18.10 / v4.20.0 三个已发布版本里没有这个变体**。那些实例的
`ChatMessage.kind` 仍是嵌套枚举 ⇒ 收到 `kind:"merge"` 时整帧解析失败被丢掉 ——
v4.22.34 修的是**我们这一侧**的容忍，老版本里那个 bug 还在。用户侧的形状：
发一份合并转发给老设备，对方毫无反应，我方 outbox 反复重投，最后显示"发送失败"，
两边都不知道为什么。ADR-0007 说"网里同时存在 4.18 的手机和 4.22 的 Mac 是当前现实"，
说的就是这个。

顺带得到一个必须记住的结论：**V1 内部并不单调**。`protocol_version` 是"破坏兼容才 +1"的
兼容性判据，而 kind 是在 V1 里悄悄加的 ⇒ 版本号没变、能力却变了。所以门控**不能**用
protocol_version 判，只能用能力位 —— 这也是"绝不用 app version 做兼容判断"那条规则
唯一的例外补偿：能力由对端自己声明，不从版本号倒推。

**改法（沿用仓库里已经跑通的那套 capability 协商，不造第二套）**：

- `CONTENT_FEATURE_MERGE = 1 << 1`，本机 Hello 开始声明它。老端不看这个字段、
  新端据此决定能不能对它发这类帧 —— 与 `CONTENT_FEATURE_PULL` 同一条通路，不进签名。
- `kind_required_feature(kind)`：「哪个 kind 需要对端哪一点能力」**唯一映射表**。
- `kind_allowed_by_features(kind, peer_features)`：**唯一判据**。对端从没交换过 Hello
  或已离线 ⇒ 位图按 `0` 处理，即"不知道就当不支持"（宁可少发一条，也不要静默丢帧）。
- 判据落在 `commands/chat.rs::send_message` 一处，位置在**公钥查找之前**：这条根本不会
  发出去，不该再触发一次 who_has 探测白等 1.2 秒；自发消息（`insert_self_message`）分支
  在门控**之前**返回，给自己发不会被判"对方版本不支持"。
- 被挡下的文案由 `kind_unsupported_hint` 出（跟规则放一起），说清"对方版本较旧、
  已停止发送、让对方升级后再发"。刻意**不写具体版本号**：能力位才是事实来源，
  "多少版以上"写死就会腐烂；要看对方版本，联系人详情的「对方版本」那行有。

**测试与护栏**：
- `gated_kind_needs_the_peers_own_declaration`：`text` 对 0 位图放行 / `merge` 对 0 位图
  拒绝 / 有对应位放行 / **只有别的位不算**（防"位图非零就放行"这种糊法）。
- `every_gated_kind_is_advertised_by_us`：扫 `WIRE_KINDS`，凡登记了门控要求的 kind，
  本机广播的能力位必须包含它 —— 防"加了门控忘了声明能力"把自己永久锁死（静默）。
- 源守卫 `new_message_kinds_are_gated_at_the_send_path`：判据与映射表各只许一处、
  `merge` 必须在表里、发送路径必须真的调用判据，且顺序满足"自发分支在前、公钥探测在后"。
  接线断言是必需的：判据函数写得再对，发送点不调用 = 没有门控。
- 两条 `verify-guards` 用例分别注入「删掉发送点那次调用」（**不是**改成 `if false`，
  那仍然算调用，抓不出来）和「从本机能力位图里摘掉 MERGE」。

**残留（已开 #37，不偷偷留口径）**：群聊发送口没门控（gossip fire-and-forget，老成员只是
自己看不见，且离线成员能力未知，"全成员都支持才发"现在无法判定）；离线/未知对端按
"不支持"处理的代价是发不出去，更好的形状是入队、等对方 Hello 到货在 flush outbox 时再判
（那时才第一次真知道），但那要求 outbox 载荷可重建 —— 那条链路刚修过 P0，得独立取证再动。

### 验证

- `npm run verify:full` **15 步全绿 / EXIT=0 / 485.6s**（clippy `-D warnings` 0 告警、
  Rust 单测含 examples、Android aarch64 0 warning、护栏前端子集全过）。
- 两条新护栏非空转（`--only gating`，GUARDS_EXIT=0）：删掉发送点那次门控调用 ⇒ 守卫 FAIL、
  恢复 ⇒ PASS；把 MERGE 位从本机广播里摘掉 ⇒ `every_gated_kind_is_advertised_by_us` FAIL、
  恢复 ⇒ PASS。注入语料特意做成"**不是** `if false`"—— 后者仍然算一次调用，抓不出来。
- 跑完逐字核对 `chat.rs` / `protocol.rs` 无注入残留（护栏工具会改写源文件再还原，
  不核对就等于把别人的实验现场提交进仓库）。
- macos 基线 593 → **596**（+3：判据两行为一条、"门控表与广播能力一致"一条、发送侧接线守卫一条）。
- **真机才能验的一项**：手里还留着 v4.20.0 及更早安装包/ APK 的话，装一台当对端，
  从新版本给它发合并转发 —— 期望从"对方静默收不到 + 我方发送失败"变成
  我方立刻弹一句"对方的 Gosslan 版本较旧，不支持「merge」…"且不发出去。
  这一条**没有做**：本机 `examples/e2e_peer.rs` 那个假对端广播的是本机自己的
  `content_features()`，拿它当"老实例"是自证循环，不作证据。
- 能力位从 Hello 落进 `peer_content_features` 这一段是**复用**既有通路（拉取能力今天
  就走它），不是新代码；所以本单位没有为它另写测试，但也别当成"已验证过的老代码"——
  它验证过的只是"能读到并缓存一个 u32"。

## [4.22.38] - 2026-09-21

### Fixed (写出记账随发送尝试一起回收 —— 上一轮进度修复的漏掉的那半)

v4.22.37 把发送进度改成按 writer 记的 `chunks` 换算，但那张表的清理只写在
`stream_file` 的**成功路径**末尾一处。而 `stream_file` 有十来处提前 `return Err(...)`
（用户取消 / 链路已关闭 / 加密失败 / `?`），一条都没覆盖。

后果不是内存，是**口径**：一次失败的发送会在 `file_wire_progress` 里留下上一次的 `chunks`，
下一次重试同一文件（新 transfer_id 也一样会撞上"这张表从不回收"）时读到的计数比真实
写出量大，于是进度又退回"按入队算"—— 正好把上一轮那条修复抹掉，而且现场只在
"传失败 → 再重发"时才出现，最容易漏测。

**改法**：`WireLedger` 一个持有表引用 + transfer_id 的 RAII 守卫，`Drop` 里删。
`stream_file` 开头装一次，函数末尾那处手写清理删掉 —— 成功、取消、`?`、panic 展开
走的是同一条回收路径，"什么时候删"这件事全仓只剩一处。

顺手把 `clear_file_wire_progress(state, id)` 换成 `clear_file_wire_progress_in(table, id)`：
守卫持有的是表而不是 `AppState`，这样回收语义可以脱离 `AppState` 单测（见下）。

### Tests

- `wire_ledger_is_reclaimed_on_every_exit_path`：本地造一张表，跑两次"发送尝试"，
  一次正常返回、一次**提前 return**，两次都断言表已空。
- 源守卫 `file_send_progress_counts_wire_not_queue` 加了两条断言：`stream_file` 里必须
  装 `WireLedger {`，且**不许再出现第二处** `clear_file_wire_progress`。
  这两条是必须的 —— 只测 Drop 本身的话，把 `stream_file` 里装守卫那行删掉，Drop 测试
  照样全绿（它测的是辅助类型，不是接线）。为此另登记一条 `verify-guards` 用例，
  注入方式就是"删掉装守卫那行"。

### 边界（说清没做什么，已并入 #35）

群发路径**没有**一起改。原因不是省事：`commands/group_file_dispatch.rs:66-71` 写明群发是
"每个成员各 spawn 一个任务、**共用同一个 transfer_id**"（cancel 键为此专门带了 recipient），
所以这张只按 transfer_id 记的表里的 `chunks` 是 N 个成员的**总和** —— 既不能拿来算单个成员
的进度，也不能由任何一个成员先结束时清理。而且不能简单换成"按 writer 所在连接的 peer_id 记"：
Routed 链路上 writer 的 peer 是中继而非目的地，两个走同一中继的成员还是会撞。
要修得先定一个能从发送任务传到 writer 再传回的稳定身份（帧里带 per-recipient attempt id），
那属于协议面，按 INV-P24 的加字段路子走。中继发文件（`RelayChunk`）同理且更远 ——
它是多邻居 fire-and-forget，"写给了谁"不唯一，进度口径要由邻居确认来定义。

### 验证

- `npm run verify:full` **15 步全绿 / EXIT=0 / 207.7s**（clippy `-D warnings` 0 告警、
  Rust 单测含 examples、Android aarch64 0 warning）。
- 非空转：`verify-guards.py --only "发送"` → 3 条（本轮新增的"装守卫那行被删掉"注入、
  上一轮的"进度改回入队"注入，外加一条同名匹配的旧用例）全部
  「改坏即 FAIL、恢复即 PASS」，GUARDS_EXIT=0；跑完确认 `file.rs` 已还原、无注入残留。
- macos 基线 592 → **593**（新增 `wire_ledger_is_reclaimed_on_every_exit_path`）。
- 真机证据：本轮**没有新增需要界面的验证** —— 修的是失败重试路径的内存表回收，
  外部不可见。要现场看的话需要"发一个大文件 → 中途取消 → 重发同一个文件"，
  观察点应是第二次的进度从一开始就按链路爬，而不是直接跳到接近完成。

## [4.22.37] - 2026-09-21

### Fixed (发送进度不再按"入队"计数 —— 262MB 还在队列里就显示 100% 的假象)

用户报的形态是"文件跑到 100% 卡住/报失败"。根因不是进度条画错，是**进度这个数字量的
不是同一件事**：`stream_file` 每灌完一片就 `sent += n`，而 `send_on_link` 返回成功只代表
这一帧**进了那条链路的 mpsc 队列**（容量 1024，`transport.rs:1072`），不代表它写到了 socket 上。
LAN 一片 256KB ⇒ 队列最多能囤 **1024 × 256KB ≈ 262MB**：文件比这大时，界面早就 100%，
链路上其实还在跑几分钟。

讽刺的是这件事我们早就知道 —— R7 为了把 `FileCompleteAck` 从"固定 30s 墙钟"改成"安静 30s
才算失败"，专门加了 `file_wire_progress` 这张"真的写出去了"的表，注释里逐字写着
「判据必须落在**写出**而不是"入队"上 —— 队列能装 1024 帧」。只是那份证据**只给了等确认用，
没给进度用**。同一份事实两处口径不同，就是这一条。

**改法（不加新机制，接在已有证据点上）**：

- `file_wire_progress` 的值从裸 `i64` 时间戳变成 `FileWireProgress { at_ms, chunks }`；
  唯一的写入点 `mark_file_wire_progress`（TCP 的 `writer_loop` 与 BLE 的写循环都在
  `write_frame` 成功之后调它）在刷新时刻的同时**累加片数**。`file_wire_progress_at()`
  语义不变，新增 `file_wire_chunks_at()`。
- `stream_file` 的进度与 `file-progress` 事件改成读 `wire_progress_bytes(...)`：
  `已写出片数 × 该片大小 + 续传前缀`，并**两处钳制** —— 不超过本机已入队量（计数器按
  transfer_id 记，残留值不该把进度推超过实际读出来的字节），不超过 `size`（最后一片是
  短片，按整片折算会算出 >100%）。
- 终点没动：`progress = 1.0` 仍然只在收到对端 `FileCompleteAck` 之后写。所以现在的形状是
  "进度条按链路爬 → 对端确认完成 → 才 100%"，而不是"早就 100% → 干等 → 可能报失败"。

**没顺手改的（说清边界）**：中继发文件（`RelayChunk` 那条路径，`file.rs` 里另一处
`received: sent`）仍是入队口径。它的"完成"由邻居的 ack 决定、且是 fire-and-forget 多邻居泛洪，
把它一起改需要另设一次证据点 —— 不在这条修复的范围里，另开待办而不是顺手糊上去。
群文件（`GroupFileChunk`）现在也共用同一个计数写入点，但其发送路径的进度还没换算，同样待后续。

**测试**：`progress_counts_written_chunks_not_enqueued_bytes` 用纯函数覆盖 5 个形状
（全队列入队但只写出 1 片、一片没写出、断点续传前缀、短片越界、计数器残留）。
源守卫 `file_send_progress_counts_wire_not_queue` 钉三处必须同时成立（writer 累加、
状态结构带 `chunks`、`stream_file` 用换算值且不得再出现 `received: sent`）；
判据取**函数体**而非全文，因为 `received: sent` 在中继那条路径里是合法的，全文扫会误伤。

**为什么不做成"读队列占用量倒推"**：`Sender::capacity()` 能给出队列里剩几帧，看起来更省事，
但 Low 队列是 `FileChunk` / `GroupFileChunk` / `FileDone` / 大头像**共用**的 —— 用帧数推字节
在多文件并发时必然算错（而这正是本条要修的场景）。计数必须由 writer 按 transfer_id 记。

## 验证

- `npm run verify:full` **15 步全绿 / EXIT=0 / 168.6s**（clippy `-D warnings` 0 告警、
  Rust 单测含 examples、Android aarch64 54.9s 0 warning）。
- 新护栏非空转：`verify-guards.py --only "发送进度"` → 「改坏即 FAIL、恢复即 PASS」通过。
- macos 基线 590 → **592**（纯函数换算 1 条 + 源守卫 1 条）。
- 换算本身是纯函数测试覆盖的（5 个形状含主症状"1024 片入队 / 1 片写出 ⇒ 262144 而不是
  268MB"），所以"数字对不对"有机器证据。
- **没有真机证据**：进度条按链路爬这个观感需要"大文件 + 慢链路"（BLE 或拥塞的 600MB）
  才看得出来，本机自环复现不出队列积压。留给下次真机互传时顺带确认：现象应当是
  "进度到不了 100% 直到对端确认完成"，而不是过去的"早就 100% 然后卡住"。

## 顺带发现（不在本次修复里，已并入待办 #35）

`file_wire_progress` 全仓**只有一个清理点**（`file.rs:937`，stream_file 成功路径）。于是
群文件发送每片都写表却从不回收 ⇒ 每个群传输永久留一条记录（记时间戳时就有这个泄漏，
现在多带一个 `chunks`）；`stream_file` 的 cancel / 链路失败等提前 return 同样不清。
后者恰好是本次 `min(enqueued)` 钳制兜住的那种脏数据，所以**不影响正确性**，但表会一直长。

## [4.22.36] - 2026-09-20

### Fixed (数据比本机新时，"拒绝启动"终于说得清发生了什么 — AI_RULES §13 的落地补齐)

用户诉求里那句「版本不支持应该提醒，而不是报错」在**本地数据**这一侧一直没做到：
`db::init` 遇到 `user_version > DB_VERSION` 的行为是对的（拒绝降级、不删数据），
但用户看到的效果是"窗口闪一下就没了"。三层叠加：

1. 文案是英文技术串（`refusing to open (downgrade detected, ...)`），而且为了传出去
   被硬塞进 `rusqlite::Error::InvalidParameterName` —— 一个语义完全不相干的变体；
2. 这个错误从 `AppState::init` 冒到 Tauri 的 `setup`，`setup` 返回 Err 会被框架变成
   `panic!("Failed to setup app: ...")`（tauri-2.11.5/src/app.rs:1424）；
3. Windows 发布版是 `windows_subsystem = "windows"`（无控制台）⇒ panic 的 stderr 无处可去。

于是"正确地拒绝"在用户端等于"无声崩溃"。AI_RULES §13 早就写了"拒绝必须给用户可读解释，
不能只是一个错误"——规则在场，实现没跟上。

**顺带修掉一个更隐蔽的问题**：降级判定原本在 `run_migrations` 里，而 `run_migrations`
是在 `conn.execute_batch(SCHEMA)` **之后**才跑的。那句是 `CREATE TABLE IF NOT EXISTS`，
对已有表无害，但它确实写文件 —— 也就是说旧代码一边宣称"你的数据没有被改动"，一边
已经把本机这套 schema 的缺失表建进了一个**比本机更新**的库里。现在判定挪到
`Connection::open` 之后的第一件事，并在 `migration_tests.rs` 里用
「拒绝之后库里仍然一张表都没有」钉住位置（`downgrade_refusal_writes_nothing`）。

**改法**：

- `db::init` 的失败改成带类型的 `enum InitError { Downgrade { current }, Sqlite(..) }`
  （`Display` 给单行技术描述写日志，`downgrade_message(current)` 给人看的那句话：两个版本号
  都在场、承诺数据未被修改、说清该升级而不是删库）。**按类型分支而不是错误字符串**是刻意的 ——
  协议层已经因为"按 `unknown variant` 前缀分类"吞过一整条消息（INV-P24 第 2 条，v4.22.34
  才修掉），同一类判断不许再犯第二次。
- `lib.rs` 的 `setup` 在启动失败处分类型识别降级，弹**原生非阻塞**对话框，用户点掉后
  `exit(1)`；其余错误仍按原路返回。
- 弹窗**只能是 `show`，不能是 `blocking_show`**：插件的桌面实现是
  `run_on_main_thread(...)`（tauri-plugin-dialog-2.7.3/src/desktop.rs:222），而 `setup`
  正跑在主线程上 ⇒ 阻塞版的 `rx.recv()` 会把主线程钉住，排在队列后面的弹窗任务永远
  执行不到 —— 与 v4.22.30 刚修掉的 Windows 开窗卡死是**同一个形状**。所以提前
  `return Ok(())` 让主线程回到事件循环，代价是不再 `manage(state)`（前端命令全部拿到
  "state not found" 的 IPC 错误、界面停在空白）：比"半死的界面"诚实，且弹窗盖在上面。
- 弹窗文案中英各一句不是敷衍：此刻数据库没打开，读不到用户的 `language` 偏好，
  而这个弹窗必须在 webview 之外显示 —— 两种语言都给，而不是猜一种。

**测试**（`db/migration_tests.rs`）：`migration_refuses_downgrade` 从 "is_err" 收紧成必须是
`InitError::Downgrade { current: 99 }`；新增 `downgrade_refusal_writes_nothing`（上面那条
位置红线）、`downgrade_message_says_who_is_old_and_what_to_do`（两个版本号 + "没有被修改" +
"升级"必须在场，老英文串与 `InvalidParameterName` 必须不在场）。
护栏源守卫 `boot_downgrade_refusal_is_typed_precedes_writes_and_non_blocking`
判 ①唯一一处 ②按类型分支 ③非阻塞，两条 `verify-guards.py` 用例分别注入
"判定挪到写入之后"和"改用 blocking_show"验证非空转。

**自查后删掉的两处**（都是我自己多写的东西，记下来是因为"删测试"必须留痕）：

1. `#[cfg(not(desktop))] { let _ = msg; return Err(e); }` 是**死分支** —— 桌面块不参与编译时
   本来就落到外层 `return Err(e)`。移动端行为不变，少 5 行。
2. 原本给 `InitError` 写了 `user_message(&self)`，带一条 `Sqlite` 分支返回"无法打开本地数据库"，
   并配了一条测试断言"非降级错误不许套降级文案"。但那条分支在生产里**永远不会被显示**
   （只有降级走弹窗）—— 为一段不会执行的路径写文案再写测试禁止误用，是绕了一圈的过度设计。
   改成 `downgrade_message(current)` 只服务降级那一种，于是"把降级文案套到文件损坏上"
   在类型上直接不成立。**随之删掉 `sqlite_errors_are_not_reported_as_downgrade` 这一条测试**
   （它保护的对象已经不存在，不是改断言迁就实现），用例总数 591 → 590。

**自己写出来才发现的两个坑**（都记在这里，因为它们是同两类反复出事的形状）：

1. 守卫第一次跑就把自己判红了 —— 它在 `lib.rs` 里，而注释里写了 `blocking_show()`
   这个 API 名。修法不是改注释措辞（那样散文一动守卫就瞎），而是判据只看代码行
   （剥掉 `//` 开头行）**并且**只取 `#[cfg(test)]` 之前那段：否则正判据
   （`.show(move |_|`）会先命中守卫自己的源码，变成"永远为真"的空转守卫 ——
   这比判据太松更危险，因为它看起来是绿的。
2. 移动端刻意**不**走弹窗（`#[cfg(desktop)]` 才弹）：那个时点原生对话框在 Android 上
   显不显示，我在这里没有实测手段；而"提前 `return Ok(())` 但弹窗没出现"的后果是
   永久白屏，比现在的"崩溃 + logcat 一行"更糟。所以移动端保持原行为，
   由 `check-mobile.sh` 证明 `#[cfg(not(desktop))]` 那条分支编得过。

## 验证

- `npm run verify:full` **15 步全绿 / EXIT=0 / 376.7s**（这一轮跑了两遍：审查改动后的
  最终代码重跑一遍才算数）：clippy `-D warnings` 0 告警、
  `cargo test --features bluetooth`（examples 一并编）、Android aarch64 编译门禁 76.6s
  通过（就是上面那条移动端分支的编译证据）。
- 两条新护栏的**非空转**证据：`python3 -u scripts/verify-guards.py --only boot`
  → 两条都是「改坏即 FAIL、恢复即 PASS」，EXIT=0。
- macos 基线 587 → **590**：新增 3 条（`downgrade_refusal_writes_nothing` +
  `downgrade_message_says_who_is_old_and_what_to_do` + 源守卫那 1 条），
  另有 1 条老测试 `migration_refuses_downgrade` 从 "is_err" 收紧成判类型；
  以及**删掉 1 条**（`sqlite_errors_are_not_reported_as_downgrade`，见上"自查后删掉的两处"）——
  587 + 3 = 590 对得上，删的那条不是为了让谁变绿。
- **真机确认（用户 2026-09-20 亲眼）**：起隔离实例 `gosslan-1.db`（`PRAGMA user_version = 99`，
  不碰真实 `gosslan.db`）跑 release 二进制 ⇒ 屏幕上出现原生弹窗「本机 Gosslan 比这份数据旧，
  无法打开…（中英各一段）」，点 OK 后进程干净退出；日志侧同时留下
  `[gosslan-db] FATAL: DB user_version=99 > app DB_VERSION=8: …（未执行任何迁移）`。
  这条路径**不再是"只能靠单元测试相信"的**。
  边界要说清：**以上是 macOS**。Windows 走的是同一条 `#[cfg(desktop)]` 分支，但
  Windows 的 WebView2/TaskDialog 行为差异已经坑过我们一次（v4.22.30 的开窗卡死），
  所以"Windows 上这个弹窗也正常"**还没有证据** —— 下次在 Win 机上出包时顺手验一次
  （同样用 `GOSSLAN_INSTANCE=1` + 一个 `user_version=99` 的 `gosslan-1.db`，不碰真库）。
  移动端则是**刻意没做**（见上），保持今天的行为：崩溃 + logcat 一行。
- ⚠️ 顺带记一个"看着像证据其实不是"的坑：插件桌面实现里 `ok=true` **同时**是
  "用户点了 OK" 和 "弹窗根本没显示、结果被 default 掉了"两种情况（`desktop.rs:219`
  把 `MessageDialogResult` 直接喂给回调，失败路径也走同一个值）。我一度根据"回调 2~6 秒就返回了"
  判成弹窗没显示 —— 实际是人在键盘上点的。**回调返回不是显示证据，人眼才是**。
  以后要机器判"弹窗到底有没有出现"，得换个可观测的面（窗口存在性/截图），别拿回调当证据。
- 我这台 shell 的两条观测通道都被权限堵死，记下来免得下次再试：`screencapture` 报
  `could not create image from display`（无屏幕录制权限），`System Events` 报
  `-25211`（无辅助访问权限）—— 所以"截个图看弹窗"这种验证在这里做不了，只能请人看。
- 顺带一个门禁自身的观测：上一版全量层 1067.3s 里"护栏非空转（前端子集）"占 594.2s，
  本轮同一步只有 68.7s —— 差在**并行的 `tauri build` 抢 cargo 构建锁**。
  期间一条用例还因为 `Command ... timed out after 900 seconds` 被判"不符合预期"，
  而它其实什么都没做错。结论：护栏扫描的 900s 超时在"有人正在打包"时会误报，
  这是门禁鲁棒性问题（已并入待办 #25/#33 一起处理，不在本轮偷偷改）。

## [4.22.35] - 2026-09-20

### Added (「对方版本较新」从诊断面板里的一条数据，变成用户看得见的状态 — INV-P24 第 2 条收尾)

前两轮把机器侧的降级做完了（未知帧不拆链 → 版本互相上报 → 未知 kind 不显示裸 JSON），
但**人看不到任何解释**：如果对方是 Gosslan 5.0，本机只会对每条新类型消息默默显示
`[不支持的消息]` 占位 —— 用户既不知道该干什么，也不知道该催谁升级。
V2 把版本数据收进了内存，这一轮才把它接到界面上。

**改法**：判定收敛成**一个函数**，结论随好友记录下发。

- `protocol::peer_protocol_is_newer(declared: Option<u32>) -> bool` 是全仓唯一的兼容判定处。
  前端**不许自己比数字** —— 那样 `PROTOCOL_VERSION` 就有第二份真相源（TS 里那份常量迟早和
  Rust 漂移，而本项目已经在这类"两处各写一份"的形状上反复出过事）。
- `Friend` 多两个读时富化字段：`peer_app_version`（给人看的字符串）、
  `peer_version_newer`（算好的结论）。`db/friends.rs` 给空值，`get_friends` 在锁外先快照
  `peer_versions` 再填 —— 与 `device_type` 同一套「DB 存身份、内存存现场」的口径。
- UI 两处：单聊头部一个不抢戏的向上箭头（整句说明在联系人详情），联系人详情一行
  「对方版本 4.22.99 / 对方的 Gosslan 比本机新，部分新类型消息需升级本机后才能查看」。
  三处口径一致：头部说"对方较新"、详情说"升级本机才能查看"、真正看不懂那条消息由
  `UnsupportedKindBubble` 就地解释同一件事。

**`None` 不猜**：老版本不声明 `protocol_version`，判 false。把"未知"当"更高"会在每个老实例
好友上刷一排"对方版本较新"的假告警（而老实例才是现网常态）；当"更低"又会藏掉真实差异。
所以老实例就是什么都不显示 —— 与诊断面板把老端如实显示成"未声明"、不替它猜版本号是同一个原则。
同理，联系人详情的版本行只在"至少知道一件事"时才出现，两个字段都空就整行不显示，不糊一面"未知"。

**测试**：`peer_protocol_newer_only_when_declared_higher` 覆盖 `None` / `Some(0)` /
`Some(PROTOCOL_VERSION)` / `Some(+1)` 四种输入。其中 `Some(PROTOCOL_VERSION)` → false 是
**防空转的关键一条**：判定若误写成 `>=`，其余三条照样全绿，而现网会变成满屏误报。
本轮**没有加前端行为测试** —— 该判定的逻辑全在那个已被直接测过的 Rust 函数里，
组件只是把布尔值渲染成一个标记；给组件加测试只能测到"mock 里我写的值渲染出来了"。

验证：`npm run verify:full` **15 步全绿 / EXIT=0 / 1067.3s**（clippy 0 告警、Rust 单测
含 examples、护栏非空转子集、Android 0 warning）；新用例已登记进 macos 基线
（586→587，删除 0 条）。UI 侧**只做了类型与静态检查**（`npm run build` 通过），
没有真机截图确认 —— 现网还没有比本机新的 Gosslan，那个箭头标记当前**无法在真机上被触发**，
要等第一个 `PROTOCOL_VERSION` 提升的版本铺开才有端到端证据。

## [4.22.34] - 2026-09-20

### Fixed (对端发来的未知消息类型不再被整帧丢弃 — INV-P24 第 2 条的另一半)

上一轮建好了"未知内容显示成占位"的兜底，但**单聊根本走不到那个兜底**：
`ChatMessage.kind` 是 `MsgKind` 枚举，对端发 `kind:"sticker"` 时 serde 报
`unknown variant` —— 而 `decode_frame` 只能看错误字符串的前缀，**分不清"未知帧类型"和
"未知嵌套枚举值"**，于是整条 `chat_message` 被降级成 `Message::Unknown` 丢弃。
后果不是"少一个占位"，而是**消息根本进不了库**：接收方什么都不知道（连"看不懂"都看不见），
发送方拿不到 Ack ⇒ outbox 一直重投 ⇒ 最后显示「发送失败」。
也就是说 V1 那条"未知帧不拆链"的降级，顺手把"未知 kind"也一起吃掉了。

**改法**：`ChatMessage.kind` 在**线格式上**改成 `String`（JSON 完全不变，对端无感），
接收侧原样入库，交给上一轮的 `isKnownKind` 兜底显示。

- 发送侧词表不变：`commands::send_message` 仍先归一化成 `MsgKind` 再 `as_str()` 发出，
  所以"本机不会发出乱码 kind"这条性质保留。
- `open_direct_content` 不再 `kind.as_str()`，改为**原样透传**（此前它就是把枚举转回字符串，
  现在少一次有损往返）。
- 重发路径同样受益：`MsgKind::from_wire_str` 那种"不认识就当 text"的回落不再是正确性的前提
  ——以前若真有一条未知 kind 的消息被重发，它会以 `text` 身份把 JSON 正文发给对方
  （正是用户禁止的那个形态），现在这条路径不存在了。
- 消息身份不受影响：`msg_id` 由发送方给出、kind **不参与**任何哈希（已逐处核实），
  去重/Ack/排序口径不变。

**判据测试**（`unknown_message_kind_still_decodes_as_a_chat_message`）：同一个未知值
放在 `kind` 上必须还能解析（且原样保留、不回落 text），放在 `type` 上仍然必须是硬解析错误
（那才是该降级成 Unknown 的场景）。第二条是对照，防整条测试空转。

顺带修掉两处**会说谎的注释**：`MsgKind::Merge` 上方写着"帧的 kind 是编成这个枚举传的，
漏了就回退成 text ⇒ 对方看到裸 JSON"——事实已变（漏登记的代价从"对方看到 JSON"
降级为"本机发不出这种消息"）；`previewText`/`preview_text` 的跨语言契约说明同步。

**INV-P24 第 2 条到此闭合**：单聊与群聊两条路径现在都会把未知 kind 送到
`UnsupportedKindBubble`（上一轮只有群聊能走到）。

验证：`npm run verify:full` **15 步全绿 / 367.8s / EXIT=0**（clippy 0 告警、
`cargo test --features bluetooth` 含 examples、护栏非空转子集、Android 0 warning）；
新用例已登记进 macos 基线（585→586，删除 0 条）。
仍**没有真机证据**：现网还没有比本机新的 Gosslan，验证方式是单元测试 + 契约测试；
端到端确认要等第一个新 kind 上线（可用 `examples/e2e_peer.rs` 发一条未知 kind）。

## [4.22.33] - 2026-09-20

### Fixed (本机不认识的消息类型不再显示成裸 JSON — INV-P24 第 2 条落地)

用户诉求原话：「老设备和新设备聊天…不应该出现 json 字符串，要优雅降级」。V1/V2 解决了
"连不上"和"谁知道对方是什么版本"，这一轮解决**看得懂的东西不许以 JSON 形式露出来**。

**判据只有一个来源**：`protocol::is_known_kind()` / `utils/messageKinds.isKnownKind()`，
即"查 `WIRE_KINDS` 表"。不能拿 `kind_class` / `kindClass` 代替 —— 它们对未知 kind **回落到
Bubble**（那是故意的：宁可多显示一条，也不静默吞掉对端的新内容），用它判"认识不认识"会永远
判成认识、兜底分支永不触发，而症状只是"少了一句占位"，没人会当 bug 报。这条区别由
`messageKinds.test.ts` 反向对照钉住。

覆盖到的每一层（缺一个就会在某个角落重新冒出 JSON）：

| 站点 | 之前 | 现在 |
|---|---|---|
| 会话列表 / 系统通知（Rust `preview_text`） | `_ =>` 无条件截断正文 ⇒ JSON | 未知 kind ⇒「[不支持的消息]」；`text`/`system` 照旧透传 |
| 同一件事的前端侧（`previewText`） | `default:` 截 30 字符 ⇒ JSON | 同一判据同一文案 |
| 时间线气泡（`MessageItem` 的 `v-else`） | `{{ message.content }}` 原样上屏 | 新 `UnsupportedKindBubble`：可解释说明 + **主动展开**才给原文（附 `type:` 供诊断） |
| 引用片段（`quoteSnippet`） | 落到"取前 40 字" | 占位文案 |
| 搜索命中行（`ChatSearchDialog`） | `m.content` 直接 `v-html` | `cellText()` 判未知 |
| 收藏列表（`rowTitle`/`rowSubtitle`） | 兜底成「文件」——给不认识的东西编身份 | 占位文案 + 空摘要 |
| 虚拟列表高度（`messageHeight`） | 按载荷长度估 ⇒ 占位气泡下面一大片空白 | 未知 kind 走固定 `UNSUPPORTED_BUBBLE` |

顺带修掉一条**现存的**错误：`preview_text` 以前不看 `display_kind`，于是 4.22.1 之前写坏的
历史行（`kind="video"`）在会话列表里显示成载荷 JSON 前 30 字符。现在 Rust 侧先归一化再判，
`video` → `[文件]`（既不是 JSON，也不是"不支持"）。

补了一条**欠账很久的契约**：`protocol.rs` 里原本写着"前端 `previewText` 与本函数没有跨语言
契约测试，改动时两处一起改" —— 靠自觉的两处一致迟早漂。现在 `messageKinds.test.ts` 直接读
`protocol.rs` 比对 `WIRE_KINDS` 的 Bubble 清单与 `UNSUPPORTED_PREVIEW_LABEL` 字面量。

**诚实的覆盖边界**：今天真正会走进这个兜底的只有**群聊**（gossip 载荷的 kind 本来就是 String，
未知值能进库）与历史坏行；**单聊**的未知 kind 会在帧层就被 V1 的 Unknown 降级**整帧丢掉**
（`ChatMessage.kind` 是嵌套枚举 ⇒ serde 报 unknown variant ⇒ 与"未知帧类型"无法区分），
所以"单聊收到新类型消息显示占位"要等下一轮（任务 #27：kind 改回 String）。本轮先落兜底再放开
入口，顺序不能反 —— 反过来做会让那条 JSON 立刻出现在会话列表里。

测试：Rust `preview_text_hides_payload_for_unknown_kind`（含 text/system 透传与 video→[文件]
两个**防空转对照**）、`only_is_known_kind_can_tell_unrecognized_apart`；前端
`previewText：本机不认识的 kind 给占位而不是载荷`、契约测试两条新断言（Bubble 清单 + 占位文案
字面量 + 表外必判未知）。`npm test` 491 全绿。

## [4.22.32] - 2026-09-20

### Fixed (上一轮门禁分层埋的一条静默空转 + 两处假数字 — code review 抓出)

双轴自查（标准轴 / 规格轴）最近 5 个提交，查出 1 条真缺陷、3 处过时文案，全在门禁层：

- **[真缺陷] `npm run verify -- --full` 会一条护栏都不跑还全绿。** 上一轮把护栏扫描归进重门禁层，
  但 `--full` 只控制护栏**范围**、不拉起该层 ⇒ 沿用旧写法（`.github/workflows/verify.yml` 注释里
  就还留着）的人实际只跑快速层 9 步，输出仍是一片绿。这正是本项目最忌讳的"没守却当作守到了"，
  比慢严重得多。修法：`--full` **自动蕴含** `--full-gate`；CI 注释同步改成 `npm run verify:full -- --full`。
  实测：`--full --list` = 15 步、默认 `--list` = 9 步。
- **[假数字] 护栏步骤的 `why` 同时写着"97 条"和"前端子集十几秒"**：实际 123 条、135.7～168.4s。
  改成不腐烂的措辞，条数与耗时交给 `verify-guards.py` 自己打印的 `[n/m]` 进度。
- **[自证] 加了一条分层自检**：快速层里出现"会被识别为碰工具链（cargo / rustup / bash / python* /
  args 含 `cargo*`）却没归进重门禁层"的步骤 ⇒ 当场退出码 1 并给出修法。
  ⚠️ **诚实标注两处**：① 盲区是经 npm/npx 间接拉起工具链的步骤（文本判据看不见），那类漏判的后果
  是"快速层变慢"（可见）而非"门禁被跳过"（安全方向）；② **本条自证还没做过非空转验证**——
  想注入一条 bash 步骤证明它会红时，临时探针文件被权限策略拦下。第一次尝试也设计错了
  （注入 `cmd:"cargo"` 的步骤会被 `isHeavyStep` 自己认出并过滤，测不出泄漏）。
  后续按 `verify-guards.py` 的规矩登记成 CASES 用例补上（已进任务清单）。
- **[规格轴记录，不改] 用户提的"并行"没有实现**，理由是 `clippy` 与 `cargo test` 抢同一个 target
  锁、`cargo` 自身已吃满多核（实测 401% CPU），并行收益远小于"根本不编译"；分层已把 412s → 7.4s。
- **[规格轴遗留，已排期] CI 的步骤清单仍是手抄一份**，且已与本文件漂移（它写护栏子集"约 1.5 分钟"，
  实测 2.3 分钟）。收敛成"CI 调 `npm run verify`"是任务 E 既有条目，不该塞进这一轮。

验证：`node --check` + 两种 `--list`（15 / 9 步符合预期）+ 快速层实跑 **9 步 7.2s ✅ EXIT=0**
（自检在现存 15 步上不误伤）。本提交纯门禁/文案，不碰被测代码。

## [4.22.31] - 2026-09-20

### Chores (验证入口拆成两层：日常 7.4s，编译攒着跑或交给 CI)

用户提的："一直在跑 verify 卡很久，能不能并行快点，又不影响稳定性"。**先量再改**，拆分后
的账很清楚——慢的从来不是"检查"，是"编译"：

| 一次全量 verify = 412s | 实测 |
|---|---|
| 真的在执行测试 | **5.1s**（583 条 Rust + 489 条前端） |
| 冷编译测试产物（lib test + examples + bin） | 177s |
| clippy 按 check profile 把同一个 crate **再编一遍** | 43s |
| 护栏非空转扫描（每条改坏→跑→还原） | 168s |
| 其余结构门禁 + 前端构建 | ~19s |

**新契约**：

- `npm run verify` = **快速层**（9 步，实测 **7.4s**）：6 个结构门禁（清单/不变量例外/BLE 常量/
  领域图/领域依赖/Change Budget）+ CHANGELOG 结构 + `npm test` + `npm run build`
  （vue-tsc 是前端**唯一**的类型门禁，且它不碰 cargo，所以留在快速层）。
- `npm run verify:full` = 重门禁层（15 步，热缓存实测 **82s**）：再加 `cargo fmt`/clippy/
  `cargo test --features bluetooth`/Rust 清单/护栏扫描/Android 交叉编译；`-- --full` 再把护栏
  从前端子集扩到 123 条全量。
- **默认语义变了，所以安全网写死在输出里**：快速层结束时逐条列出"本次未跑"的重门禁，并指向
  `verify:full` 与 CI；结束行从"全部通过"改成"**快速层门禁**通过"。绝不让绿色被读成"编得过"。
- 分层成立的前提（实测过，不是假设）：`verify.yml` 在**任意分支每次 push** 跑全套
  （前端 job + Rust job × mac/win 矩阵 + Android job）。判据写进 `AI_RULES.md` §37.1：
  文档/前端改动的日常 = 快速层；**改过 Rust/`Cargo.*`/构建配置 = 提交前必须 `verify:full`**；
  发版/出包 = 再加 `-- --full`。

**顺带修掉两个会在分层时咬人的细节**：
- 快速层不再 `findPython()`，且"找不到 python 就硬失败"的判据收紧成**只在护栏真要跑时**才成立
  —— 否则没装 python 的机器会因为一个跟护栏无关的快速检查直接红。
- `dist/` 缺失的提示语原来写死"第 4 步会先构建前端"，分层后步序变了就是假信息；改成指步骤名。

**刻意没做的两件事**（避免为了快换一个会漏判的门禁）：
- 没按 diff 缩范围（"改了什么就只跑对应门禁"）：那需要一张"哪些改动必须跑哪些门禁"的判定表，
  漏一格就是**静默少检查** —— 正是本项目反复出事故的那类形状。分层已经把 412s 降到 7.4s，
  不值得再换一个会漏判的门禁。
- 没改 `verify-guards.py` 的 mtime 还原（改坏→还原只回内容不回 mtime ⇒ 下一轮 cargo 白编译）：
  收益真实但小，而它要动的是**信号安全的现场恢复路径**（`try/finally` + `atexit` + SIGINT），
  写坏了会留下一个"被改坏"的工作区 —— 属于拿稳定性换秒数。留作后续单独一轮，动它必须连
  "被打断后工作区仍然干净"一起验。

验证：`node --check` + `npm run verify -- --list`（两层归属与"未跑清单"符合预期）+
快速层实跑 **9 步 7.4s 全绿** + 重门禁层实跑 **14 步 82s 全绿**（含 Android 48.4s、
clippy `-D warnings` 0 告警、`cargo test --features bluetooth` 含 examples）+
最后一次带护栏全链路 `npm run verify:full` 复跑。文档同步 `AI_RULES.md` §37.1 与 README 测试章
（README 里"共 22 个用例"这类会腐烂的数字换成"以命令输出为准"，并补上 `--features bluetooth`
不可省的提醒）。

## [4.22.30] - 2026-09-20

### Fixed (Win 端点「设置」整个界面卡死、只能杀进程 — 开窗命令回到工作线程)

**现象**（用户 2026-09-20 真机）：Windows 上点「设置」→ 整个窗口无响应，一直不恢复，只能杀进程。

**根因**：`v4.22.2` 为修 macOS 的 `EXC_BAD_ACCESS`（AppKit 必须在主线程）把 4 个开窗命令从
`async` 改成了**同步**。而同步命令在 Tauri 里是**在 wry 的 IPC 回调里内联跑主线程**的；
Windows 上建 WebView2 会在**调用线程里泵消息**（`tauri-runtime-wry` 源码原话：
"must be called from a separate thread, otherwise the channel will introduce a deadlock"）
⇒ 主线程一边建窗一边泵消息 ⇒ 重入正在处理的 IPC ⇒ 第二次进 `ensure_aux_window` 时
`AUX_WINDOW_CREATE_LOCK`（`std::sync::Mutex`，**不可重入**）由同一线程二次 `lock()` ⇒
**永久自锁**，界面整体无响应。macOS 建窗不泵消息，所以只有 Windows 中招 —— 这也解释了
为什么当初那个改法在 macOS 上"验证通过"却埋了雷。

**改动**（回到工作线程 + 只把 AppKit 那几行投回主线程，两个平台的约束同时满足）：
- 4 个 `open_*_window` 命令恢复 `#[tauri::command(async)]`：`build()` 只把创建**入队**到事件循环
  并立刻返回（`proxy.send_event`），主线程不在任何 IPC 回调里 ⇒ 没有可重入的现场。
- 新增 `decorate_aux_window(win, app)`：macOS 的 `set_closable` / `disable_shadow` 改由
  `app.run_on_main_thread(...)` **只入队不等待**投回主线程；窗口创建（`CreateWindow`）与它同队列
  且先入队 ⇒ AppKit 调用一定发生在窗口建出来之后。
- 修掉那条**把 bug 引进来的注释**：原文写"`WebviewWindowBuilder::build()` 内部会把创建分派到
  主线程、同步等返回，所以 async/同步对耗时没影响"—— 源码里 `send_user_message` 在工作线程上
  是 `proxy.send_event`（**投完就返回，不等待**），"同步等返回"只发生在 **getter**
  （`rx.recv()`）。注释已改成实测事实 + 后果，免得下一个人再照它改回去。
- 护栏收口：`blocking_commands_run_off_the_main_thread` 的 4 条 `ALLOWED` 例外**全部撤掉**，
  并新增一条独立判据「函数体里出现 `ensure_aux_window(` / `WebviewWindowBuilder::new(` 的命令
  必须是 async」—— 光靠原有重资源标记抓不到它（开窗命令体里没有 `db::` 这类字样，db 访问都在
  已 `try_lock` 的辅助函数里），而它恰恰最不能同步。`verify-guards.py` 新用例
  「开窗命令必须留在工作线程」实跑：改回同步即 FAIL（提示"创建窗口"）、恢复即 PASS。

**验证**：`blocking_commands_run_off_the_main_thread` 通过；`npm run verify` 全 15 步见下；
macOS 侧需要用户本机冒烟一次（`open_log_window` / `open_settings_window` 各点一次，
确认窗口正常出现、无阴影、⌘W 能关）—— 本环境跑不了 GUI（见 `AI_RULES` 的真机验证约定）。

## [4.22.29] - 2026-09-20

### Chores (Rust 用例基线补齐 78 条 — 门禁从"只盯老名字"变成真兜底)

`check-test-manifest.mjs` 第 13 步拿 `src-tauri/test-baseline.macos.txt` 与实际
`cargo test --features bluetooth --lib -- --list` 比对：**基线里的名字没跑就红**（防"模块没挂进
mod.rs / 漏 `--features bluetooth` / `#[cfg]` 不满足"这类静默跳过）。它只盯"少了"，所以新增用例
只会打印一条 `⚠ 待纳入` 就放过 —— 最近这一串修复攒下 **78 条**新用例，全都没进基线。

- `--update` 后 macos 基线 505 → **583 条**，逐条核对：**删除 0 条**（`git diff` 里那 4 行 `-`
  只是重新排序后的位移，`comm -23` 证实集合无缩减）。
- 为什么值得单独一个提交：这 78 条现在才真正进入保护。它们没进基线期间，任何一条被悄悄改名、
  被 `#[cfg]` 挡掉不再执行，第 13 步都不会报 —— 而它恰恰是本项目"门禁空转过"那类事故的解药。
- windows 基线**没动**：基线按平台分开（`test-baseline.<os>.txt`），平台门控的用例集本来就不一样，
  在 macos 上跑 `--update` 只会污染 windows 腿。windows 那 78 条要在 windows 上补齐
  （或按 CI 的提示逐条核对），这不在本轮范围内，已记为待办。

## [4.22.28] - 2026-09-20

### Added (节点之间终于知道对方是什么版本 — ADR-0007 落地①后半)

上一轮让老设备"看得懂新帧看不懂就跳过"，这一轮给它一个能解释"谁老"的依据：`Hello` 增加两个
**可选**字段 `protocol_version` / `app_version`（`PROTOCOL_VERSION = 1`，本机应用版本取
`CARGO_PKG_VERSION`）。老设备不发这两个字段 ⇒ 解成 `None`；新设备收到老设备**没听说过**的字段
照样忽略 ⇒ **这一步不断现有互通**（这正是 ADR-0007 决策 1 与决策 3 的区别：决策 3 会断，所以
必须等 ① 铺开）。

- **不进签名材料**：`hello_signing_bytes` 一个字节都不变。进了签名材料就是"报版本"这件事本身
  变成破坏性变更 —— 老端验签失败 ⇒ 连不上，比不报版本严重得多。
- 两个字段分开用：`protocol_version` 是**兼容性判据**，`app_version` **只给人看**。绝不用应用
  版本做兼容判断（`4.22.10` 与 `4.22.9` 线格式相同，字符串比较会凭空排出高低）。
- 记录点只有一个：验签通过后 `handle_message` 的 `Hello` 分支写 `AppState::peer_versions`
  —— TCP 与 BLE 建链后都把首帧交回这里，所以两条 transport 天然同一份事实，没有第二处写入。
  不挂到 `Peer` 上：`Peer` 会被不带版本的 UDP announce 反复重建，挂上去每次广播后丢真值。
  回收点与 `peer_content_features` 同一个（`sweep_peers`），否则长跑无界增长。
- 立刻有消费者，不是"先存着"：**①** 未知帧降级日志从"倒推对方更新"变成写清
  「对端声明（协议=? 应用=?）／本机协议=?」；**②** 隐藏诊断面板新增「版本互通」格
  （本机版本 + 每个已知对端的声明，昵称现查 `peers`；老端如实显示"未声明"，不替它猜版本号；
  对端协议比本机高时标红）。数据仍走 `get_discovery_diag` 一个命令 —— 面板"一次拿全、
  不出现两份数据对不上"的既有纪律。
- 刻意**没做**的：`MIN_PROTOCOL_VERSION`、版本区间协商、capability 位图。今天没有任何一条按版本
  门控的消息，那些机制零调用点；它们该跟第一个需要门控的新帧一起出现。
- 测试/守卫：`hello_version_fields_roundtrip_and_old_peer_declares_nothing`（新端往返 +
  老格式 Hello 必须解析成 `None` + **未知字段必须被忽略**这条"不许 deny_unknown_fields"的哨兵）；
  守卫 `peer_version_is_declared_not_signed_and_reclaimed`（签名材料不含版本字段 /
  本机必须声明 / Hello 必须记录 / 面板必须读取 / 离线必须回收，5 条）；
  `verify-guards.py` 新用例「Hello 必须声明本机版本」实跑改坏即 FAIL、恢复即 PASS。
- 三个外部对端模拟器（`examples/{dual_link,mirror_dial,e2e_peer}.rs`）一并补上这两个字段，
  并且**刻意留成 `None`**：它们模拟的就是网里现存的老实例，于是每次跑模拟器都在验证
  "对端不声明版本"的降级路径。改这里的过程也暴露一个自查口径错误 —— 我只跑了
  `cargo test --lib`，而 verify 第 12 步是不带 `--lib` 的 `cargo test`，**examples 只有后者看得见**，
  所以第一次本地"全绿"是假的（三个 example 编译失败）。以后加协议字段一律用
  `cargo check --all-targets` 或 `npm run verify` 兜底。
- 文档：INV-P24 补「落地进度」块（哪几条已做、哪几条没做，别让下一个 AI 拿旧事实当现状），
  测试矩阵补"老端 Hello 不带版本字段"一行；ADR-0007 记 ① 已完成 + 刻意不做的清单。

## [4.22.27] - 2026-09-20

### Fixed (未知帧类型降级为"忽略"，不再是"断链" — INV-P24 落地①)

上一轮把跨版本降级写成约束（INV-P24 / ADR-0007 Accepted），这一轮落实它的第 ① 步，
也是唯一一条"不做就会伤现有互通"的：

`Message` 是 `#[serde(tag = "type")]` 的内部标签枚举，遇到不认识的 `type` 直接反序列化失败
⇒ `read_frame` 返回 `io::Error(InvalidData)` ⇒ `reader_loop` 当连接错误处理 ⇒ **拆链**
⇒ 重连后同一帧再拆。也就是说在新版本上线任何一种新帧之前，老设备的表现不是"少收一条消息"，
而是**跟新设备连不上**。本项目无服务器、无强制升级通道，这个后果不能靠"整批升级"解决。

- `transport/outbound.rs` 新增 `decode_frame(buf)`：未知 `type` ⇒ `Message::Unknown{wire_type}`
  （只由解码产生，发送侧永不构造）；**已知 `type` 但字段畸形仍然报错** —— 那是我们自己的
  bug，静默吞掉等于藏起协议错误（INV-005）。判定用 serde 自己的措辞（`unknown variant ...`），
  **不维护第二份类型清单**（清单会漂移，本项目已有多次影子常量/影子表的教训）。
- `handle_message` 加 `Message::Unknown` 分支：忽略 + `log_throttled("unknown_frame", 10s)` 的
  警告日志（写明"本机版本低于对端，升级后可识别；链路保持"）。
- **握手首帧刻意不降级**：`read_frame_preauth` 保持严格 —— 未认证的连接没有"看不懂就放过"的
  理由；降级只给已建链的数据面用。
- 测试 4 条：未知 type 降级 / 已知类型畸形仍报错（并断言报错原因不是 unknown variant，防止
  降级判定吞太宽）/ 非帧字节仍报错 / 走真实 `read_frame` 的端到端降级（tokio duplex）。
  另把 `protocol.rs` 里那条"未知 type 是硬解析错误"的旧测试**注释改正**：它过去把
  "混版本会断链、必须整批升级"当契约钉，如今 serde 层事实不变（降级正是靠它判别），
  但立场已指向 INV-P24；对照用例（已知变体必须能解析）保留，防空转。
- 守卫 `unknown_wire_frame_is_tolerated_after_auth`（5 条断言，含"preauth 不得走 decode_frame"
  这条反向断言）+ verify-guards 的"改坏必须 FAIL、恢复必须 PASS"用例（实跑通过）。
- `ADR-0017` 里"不必再保证未知 type 不被断链"那句**标注为已被取代**并指向 INV-P24/ADR-0007：
  不改掉它，下一个 AI 会拿它当依据把降级又删回去。

验证：`cargo test --features bluetooth --lib` 581 全绿、clippy `-D warnings` 0 warning、
Android aarch64 0 warning、新守卫用例非空转通过。
覆盖边界：跨版本真机（老包收新帧）需要两个不同安装版本互发，属用户真机验收项。

## [4.22.26] - 2026-09-20

### Docs (跨版本兼容成为约束：INV-P24 + ADR-0007 落定 + 三节按现实重写)

用户 2026-09-20 提出"版本不支持要提醒、不许报错、不许显示 JSON 字符串，全部功能都要优雅
降级"，并把三项协议决策与产品边界文档的写作一并授权。落笔前先核实了四个事实：

- 全仓 **没有任何** `protocol_version` / `app_version` 字段（只有本地 SQLite `user_version`）
  ⇒ 节点之间无法知道对方是什么版本；
- `Message` 是 `#[serde(tag = "type")]` 且**没有** `#[serde(other)]`；`read_frame` 把反序列化
  失败转成 `io::Error(InvalidData)` ⇒ reader 当连接错误处理 ⇒ **拆链 + 重连再拆的死循环**。
  也就是说今天上线一种新帧，老设备不是"看不懂这条"，而是**跟新设备连不上**；
- `AI_RULES.md` §12 的前提"尚未发布、没有历史客户端需兼容"被仓库自身证伪：Releases 上有
  `v4.8.2`(09-14) → `v4.20.0`(09-18) 带安装包与 sha256 的正式发布，且无强制升级通道；
- `docs/adr/0007-protocol-versioning.md` 早已写好七条规则，但因上面那个假前提一直停在
  Proposed —— 这是它没被实现的唯一原因。

改动：

- `docs/protocol-invariants.md` 新增 **INV-P24「版本不同必须降级，不许炸也不许装」**：
  未知帧不得成为连接错误 / 未知内容不得成为裸 JSON / 新字段必须可缺省 / 新能力必须先协商
  再用，外加"灰度顺序不可颠倒"与"对端更高 ⇒ 可解释状态、过低 ⇒ 明确拒绝并指出谁要升级"；
  测试矩阵补三行。原 §24 测试矩阵顺延为 §25。
- `ADR-0007` → **Accepted**，写进三项决策：① 上 `protocol_version`（兼容判据）+
  `app_version`（只给人看，绝不用于兼容判断）——**加可选字段这一步不断老版本互通**；
  ② 群文件半途重投**不加 epoch**，改为"部分完成的 transfer 不被重投复用"（重发产生新 id），
  将来做群文件续传时再作为 capability 门控上线；③ HKDF 域分离/nonce **做，但按对端版本
  分派、不硬切**，并强制每个 legacy 分支在 ADR 里写明退出条件。附落地顺序。
- `AI_RULES.md`：§12 前提更正 + 规则从"不要写兼容层"改成"兼容性靠加法与门控获得"；
  §12.1 变成三条硬门槛；§11 检查清单补两条（老版本看到什么 / 要不要 bump）；
  §13 从"未发布所以随便改"改成"已发布包装着真实数据 ⇒ 能升不能毁、读侧对历史形态容错"；
  §1/§2.2/§3 按 mesh 定位重写（旧 §3 把蓝牙/跨网段/中继/mesh 列为冻结，而代码早已发布它们
  —— 约束与代码相反时每个新会话第一课就是"别做你正在做的事"，那是无效约束）。
- 索引三处同步（README 文档表 + 路线图、`docs/AI_ENGINEERING_INDEX.md` 的 ADR 清单与
  INV 范围）：顺手修掉路线图里两处会误导 AI 的过期条目（"蓝牙无配对通道待实现"其实已完成、
  "服务端中转"与无服务器定位冲突）。

不改代码 —— 本轮只把约束与决策定下来；实现（INV-P24 的 Unknown 兜底 + Hello 版本字段 +
前端渲染兜底）按 ADR-0007 的顺序另起 commit。

## [4.22.25] - 2026-09-20

### Changed (transport.rs 分册第 2 步：Gossip 处理搬出主文件)

`gossip.rs` 分册 903 行：`group_envelope_consumable`（只管消费不管转发的判据）、
`gossip_trust_for_unpeer_sender`（信任判定，friends 锚）、`handle_gossip`（去重 / 消费 / 扇出）、
`parse_gossip_payload`。同 `include!` 机制，运行时零差异。

搬的时候暴露两个真问题，都当场修掉：

1. **一段被错接的文档**：`transport.rs` 里 `group_envelope_consumable` 头上挂着 11 行
   描述"多跳转发：把 MeshFrame 载荷还原成 GossipEnvelope"的文档 —— 那个函数早已不在这里，
   文档留下来会让下一个人以为这个判据负责转发（恰恰是它**不管**的那件事）。随分册删除。
2. **守卫的自匹配陷阱**（这是分册真正教回来的东西）：`handle_gossip_does_not_bail_out_for_non_members`
   用 `src.find("async fn handle_gossip")` 取函数体，而**聚合文本里本测试自己那句字面量也算一次
   命中**。以前主文件里"定义在前、测试在后"所以侥幸正确；分册后主文件排在分册前面，
   `find` 先命中测试里那一串，body 于是包含下面那条 forbidden 字面量 ⇒ 守卫自证其罪地理应失败。
   锚点改成"行首 + 带左括号"（`\nasync fn handle_gossip(`）才真正稳。
   同形陷阱在另外两个守卫（`build_signed_hello` / `broadcast_presence`）上目前还不会触发
   （定义在测试之前），等第 3 步搬握手时会撞上，届时同法处理。
3. verify-guards 的那条锚点随函数搬到 `transport/gossip.rs`（改坏必须 FAIL 已重跑通过）；
   领域图认领新文件（判据 D 又一次当场拦住）；`transport_src_for_guards()` 登记第三册。

验证：`cargo test --features bluetooth --lib` 576 全绿、clippy `-D warnings` 无输出、
领域图一致、被搬走的那条守卫非空转重跑通过。

## [4.22.24] - 2026-09-20

### Fixed (链路停滞成为可见状态 + 发送期限上限 2h→1h)

钉链路 + 满则背压之后，剩下的诚实问题是：**对端活着但不收了**（安卓被挂后台、接收端
磁盘/主线程卡住），`send_on_link` 会一直挂在 `tx.send()` 上 —— 既不再发进度也不报错，
界面冻在同一个百分比，最长到 deadline（原来 2h）。用户只能猜"是不是软件死了"。
这条是上一轮 review 指出、用户拍板的口径：保留不换路（换路必然乱序），但要**看得见**，
并且别等到 2 小时。

- 新增纯函数 `file::stall_verdict(idle_ms)` + 两个阈值：
  `FILE_STALL_WARN_MS = 15s`（提醒）／`FILE_STALL_ABORT_MS = 60s`（放弃本次尝试）。
  `idle_ms` 以 **writer 实发**为准（`mark_file_wire_progress` 那条既有链路），不是投进队列。
  调用方负责用"本次尝试起点"做下界 —— 否则上一轮 attempt 留下的旧时间戳会让第一个 tick
  就误判停滞（这个坑写在函数注释里，并用 `max(started_ms)` 兜住）。
- `transport::send_on_link_with_tick`：等待期间每 5s 醒一次跑检查。摘掉它=无声退化，
  所以守卫与 verify-guards 都钉住；单聊与群发共用同一个 `file::stall_tick`。
  ⚠️ 这个写法的安全性是**实测**出来的，不是猜的：`timed_out_send_leaves_nothing_behind`
  证明 `tx.send()` 的 future 被丢弃不会把消息留在队列里 ⇒ 超时后重发同一条不会造出
  重复分片（重复分片在群接收端是致命的：严格 `seq != next_seq` 判死）。
- 新事件 `file-stalled {transfer_id, stalled, idle_ms}`，**只在状态翻转时发**一次；
  前端 `stalledTransfers` 单独存（不放 `transfers[]` 行上 —— `refreshTransfers()` 会整体
  替换那个数组，行上的临时标记会被无声冲掉），气泡文案切到「网络停滞，等待恢复…」，
  进度条保留不动。done/failed 一律收回提示。
- 放弃时错误原因是 retryable 的「链路停滞」⇒ outbox 5s 后按对端真实已收字节续传，
  最多 `MAX_FILE_OUTBOX_RETRIES` 次才置失败。进度条不再"自己走完"，停在真实位置。
- `send_deadline_for` 上限 2h → **1h**（停滞判定接管了"对端不收"这件事，
  deadline 只需要负责"整件事最多占多久资源"）。
- 守卫更新：`ble_file_transfer_respects_link_limits` 的断言改用 `code_flat`
  （裸子串会被 rustfmt 拆行后静默空转 —— review 指出、上一轮我自己也撞过），
  并新增两条 verify-guards 用例（分片+Done 同链路、等待期间必须有停滞检查）。

测试：`stall_verdict_boundaries`（三档边界 + "提醒必须早于放弃"+"放弃必须早于最短
deadline"）、`timed_out_send_leaves_nothing_behind`（send future 的取消安全实测）。

## [4.22.23] - 2026-09-20

### Fixed (进度 upsert 不再把 file_transfers.path 擦成 NULL)

`upsert_transfer` 的冲突分支写的是 `path = excluded.path`，**没有 COALESCE**；
而发送路径每 250ms 的进度落库一律传 `path = None`（只有建行与完成时传 Some）。
后果：建行时写进去的本地路径，被第一次进度 tick 抹成 NULL。

这本身是潜伏缺陷，但它正好落在群图片预览的救援路径上 —— 前端
`useMessageFile` 取 `path` 是 `meta.path ?? transfers[].path`，DB content 缺 path 的
一段时间里唯一能用的就是这个兜底源。上一轮修 stale 快照竞态（4.22.17）时，review 顺手
指出这个兜底随时会被擦掉，本条把源头堵住，与 `content_transfers` 的
`path = COALESCE(excluded.path, ...)` 同口径。

- 测试 `transfer_progress_upsert_never_erases_the_known_path`：传 None 保旧值、
  progress 等其他字段照常更新（不能为了保 path 把进度冻住）、真知道新路径时正常覆盖。

## [4.22.22] - 2026-09-20

### Fixed (cid 回填后必须通知前端 — 修 v4.22.16 引入的合并卡片丢钥匙)

v4.22.16 把整文件哈希从"建发送记录之前"挪到投递任务里算完再回填，气泡不再等 O(体积)
扫描 —— 但回填**只写库、不 emit**，于是内存里那条记录一直是 `sha256:""` 的版本，直到下次
重查会话才从 DB 拿回来。后果是具体的一条功能回归：

用户在同一会话里把刚发出去的文件勾选转成合并卡片时，卡片载荷由**内存里的记录**经
`mediaSafeContent` 生成（`ChatWindow.vue` 的 forwardSelection → `buildMergePayload`），
`mediaCid` 取到空串 ⇒ 卡片只渲染成一行占位文字，且 `requestContentByCid` 永远不会发出
⇒ **对端再也拉不回这份内容**。更早的窗口：文件排着等一个离线好友时，投递任务根本没跑，
cid 会**无限期**为空（不只是"算哈希那几秒"）。

- `db::fill_message_sha256` 改为返回 `Result<bool>`（这次到底改没改行），四种情形分别可测：
  真改了 / 同值幂等 / 空值跳过 / 缺行不动。
- 投递任务在真的回填后取回该行并 `emit("message-received", &rec)`，让 store 走既有的
  upsert 路径把 cid 补进内存。自记录不会打扰用户：`maybeNotify` 对 `sender_id == 自己`
  直接 return；`applyIncoming` 的状态仍是"只前进不回退"。
- 守卫 `no_whole_file_scan_before_the_file_bubble` 加一条断言：回填改了库就必须通知前端。

## [4.22.21] - 2026-09-20

### Fixed (接收端分片判死立刻回否定确认 — 不再把整份文件灌进已死的传输)

真机形状：600MB 跑到 100%、两边都显示失败，中间几十分钟界面上一直"发送中"。

接收端对某一片判死时（`write_chunk` 返回 Err）只 `fail_receive` 把本地接收器摘掉，
**谁也不告诉**。发送端于是把剩下的整份文件继续灌进一条已经死掉的传输；等 `FileDone`
到达时接收端已无该 transfer → 走"重复 FileDone"分支、`already_done` 为假 → **一个 ack
都不发** → 发送端在 `wait_complete_ack` 里干等满一个 `FILE_ACK_IDLE`，再以
"接收方未确认文件完成"判**可重试** → outbox 连着再整发 5 次。

三段一起修（缺任何一段都不成立）：

- 接收端：`fail_receive` 返回 `true`（真的中止了一个在途接收器）时回
  `FileCompleteAck{success:false}`。返回 `false` 表示早已中止/未知传输/来源不符，
  不重复回。这是**已有的帧、已有的处理逻辑**，不需要新协议。
- 发送端注册点前移：`pending_file_complete` 原本在分片循环**之后**才登记 —— 那时到达的
  否定确认因为找不到 rx 被直接丢掉，回帧等于白回。现在提前到循环之前登记。
- 发送端在分片循环的 `select!` 里盯它：一收到否定确认当场以可重试失败退出，
  并按对端真实已收字节续传（`FileOffer.from_bytes` + `FileReject.received` 那条既有路径），
  不再把剩余字节灌完。

守卫：`receiver_abort_notifies_the_sender_inside_the_loop` 三条断言（回帧、不得退回旧的
"静默 abort"写法、循环里必须有 ack 分支且 `pending_file_complete` 不得被重复注册）；
verify-guards 登记"改坏必须 FAIL、恢复必须 PASS"用例（实跑通过）。
覆盖边界：回帧→停手是跨进程双端行为，仓库没有双实例 harness，按"源码守卫 + 真机待验"交付。

**未修（记录在案）**：两条 cancel 分支 `_ = &mut *cancel_rx => return ...` 提前 return 时
不摘 `pending_file_complete`，用户取消后那条表项会留在表里（体量极小，且下次同 id 注册会覆盖）。

## [4.22.20] - 2026-09-20

### Fixed (群文件的 Offer 也钉在分片那条链路上 — 补 v4.22.14 留下的分裂)

上一轮把分片流与 `FileDone` 钉到了单条链路，但**群路径的 `GroupFileOffer` 还留在
`try_send` 上**，这比不改更危险：

`Offer` 是 Normal 优先级、`Chunk` 是 Low，两者走各自的选路。burst 里两个成员各一条流时
Normal 通道也可能满，`send_over_order` 就会把这一条 Offer failover 到**另一条连接** ——
于是分片落在链路 A、Offer 落在链路 B。而接收端在「没有该 transfer 的会话密钥」时对分片是
**静默 return**（`transport.rs` 的 group chunk 分支既不报错也不回执），这批先到的分片
就被永久丢弃，等 Offer 到了、密钥有了，首个 seq 也对不上 ⇒ 整条传输判死。
真机形状仍然是那条：群里连发 9-10 张总有 1-2 张收不全，单独发同一张必成功。

修法：在发 Offer 之前就 `resolve_stream_link` 一次，Offer / 全部 Chunk / Done 三类帧
一律走 `send_on_link(&link, ..)`；分块大小也从这条已钉住的链路读 `path_kind`。
单聊的 Offer 刻意不动 —— 它后面紧跟一个 `FileAccept` 等待，分片不可能在密钥之前出发，
没有这条分裂（原因写进守卫注释，避免下一个人"顺手统一"）。

守卫：`ble_file_transfer_respects_link_limits` 增加群路径三条断言（Offer 与 Chunk 必须
`send_on_link(&link,..)`、且整个 dispatch 文件里不得再出现 `try_send(state, recipient,`），
配套 verify-guards 的"改坏必须 FAIL"用例。

## [4.22.19] - 2026-09-20

### Fixed (群文件取消登记按收件人分键 — 三成员以上只有一个人收得到)

上一轮提交前的独立 review 抓出的 BLOCKER，代码逐行核实成立（不是 review 误判）：

`group_announcements.rs` 对每个可达成员各 `tokio::spawn` 一个投递任务，**共用同一个
`transfer_id`**，而 `group_file_dispatch.rs` 用 `file_send_cancels.insert(transfer_id, tx)`
登记取消句柄 —— `HashMap::insert` 会把前一个任务的 `Sender` 挤掉，oneshot 的 `Receiver`
在 `Sender` 被 drop 时立刻以 `Err(RecvError)` 完成，而投递循环里的
`_ = &mut cancel_rx => return Err("用户取消发送")` **分不清「用户真点了取消」和
「登记被顶替」** ⇒ N 个成员里只有最后注册那个能发完，其余当场以「用户取消发送」这个
**假原因**中断。

这条是既有缺陷，但被 v4.22.14 显著放大：我把 cancel 分支挪到了分片发送周围（任务在那里
停留整段传输时间），原本要卡进"文件读取那一微秒"才可能触发的顶替，现在几乎必现。

- 新增 `file::file_cancel_key(transfer_id, recipient)` 与 `file_cancel_prefix`，三处登记点
  （单聊直传、中继、群投递）全部带上收件人；用 NUL 作分隔符，因为 `transfer_id`(uuid) 与
  `device_id` 都不会含它 —— 前缀匹配不会误伤「id 恰好同前缀」的兄弟传输。
- `cancel_file_transfer` 改为按前缀**批量**发信号：一条群文件本来就有 N 条在途流，
  只拿一条等于"取消只停住最后一个成员"。
- 群投递错误出口补终态：`dispatch_group_file_to_peer` 一进来就把该成员置 `sending`，
  而离线补发只捞 `status='pending'`（`db/group_files.rs:107`）—— 出错后留在 sending
  就是「永远在发、重启也不会重试」，群文件面板上那条永远转圈。现在按原因落
  `cancelled`（用户主动）或 `failed`（其余）。

测试：`file_cancel_keys_are_scoped_per_recipient`（含"前缀不得误伤 t11"这条真陷阱）；
源码守卫 `group_file_cancel_registry_is_scoped_per_recipient`（用 `code_flat` 做空白归一，
避免被 rustfmt 拆行后空转）+ verify-guards 登记该守卫的"改坏必须 FAIL"用例。
覆盖边界：登记顶替→误取消的端到端行为需要真实 `AppState` 与两个并发投递任务才能驱动，
本仓库没有那套 harness，故按「纯函数键 + 源码守卫 + 真机待验」交付。

## [4.22.18] - 2026-09-20

### Changed (transport.rs 分册第 1 步：出站投递与链路选路搬出主文件)

`network/transport.rs` 已经 1 万行，任何一次评审都要在同一屏里同时看选路、握手、
gossip、文件、群文件、中继六件事 —— 这是"改一处顺手碰坏另一处"的结构性温床。
按 `commands.rs` 的既有先例用 `include!` 分册：**同一模块、零 `use` 改动、运行时零差异**，
只是把单文件切到可评审的粒度（分 5-6 步做，每步只搬一段连续代码）。

本步搬「出站投递 + 链路选路」418 行 → `network/transport/outbound.rs`
（帧编码 `write_frame`/`read_frame`、`route_order`、`send_over_order`、`try_send`、
上一轮新增的 `resolve_stream_link`/`send_on_link`、`relay_send_to_neighbors`、
`broadcast_gossip`、`reachable_neighbors`）。

配套改动（这一步真正的风险所在）：源码守卫是用 `include_str!` 读**文件文本**的，
只读主文件会让搬进分册的代码 0 命中 ⇒ 守卫**假红**，而下一个人会以为红的是代码不是锚点。
新增 `network::transport_src_for_guards()`（cfg(test)）把主文件与全部分册拼成全集，
11 处 transport 守卫一律改读它；今后新增分册必须在那个函数里同步登记一行。

验证：`cargo test --features bluetooth --lib` 570 全绿（含全部源码守卫）；
`check-domain-deps` 按模块路径判定，`network::transport` 域归属不变。

## [4.22.17] - 2026-09-20

### Fixed (群图片预览的 stale 快照竞态 + 「发送中 0% 而对端已读」)

真机两条残留：① 群里连发 9-10 张图总有 1-2 张预览不出（单独发同一张必成功）；
② 文件对方已收已读（双对勾），本机仍钉在「发送中 0%」。

**① 是快照竞态，不是 4.22.13 修的 dedupe**：群文件消息在 Offer 阶段落库的内容**不带
本地 path**，收完才回填。而 `loadMessages` 的 DB 快照经 `preserveDeliveryStatus` 合并时
**只保 status、content 一律取 DB** ⇒ 一次在回填前取数的重查后到，就把内存里已回填的 path
擦回无 path 形态；`useMessageFile` 在 path 为空时根本不发预览请求 → 空白气泡。
单聊不受影响，因为单聊 Done 是**直接 insert 一条带 path 的记录**（这就是"群聊坏、单聊好"
一直找不到根因的形状差）。

- 新增纯函数 `pickMediaContent(prev, incoming)`：媒体行只在「先前有 path、新的没有」时
  保住旧 content，其余一律以新载荷为准（DB 是真相这条原则没被放宽）；
  `preserveDeliveryStatus` 与 `applyIncoming` 两条覆盖路径同时接上。测试 ×2。
- `handle_group_file_done` 补 emit `file-done`：单聊、中继、群**发送方**都发，唯独群
  **接收端**漏了。漏掉的后果不是少个事件 —— 接收端在字节落盘前读预览会得到「已被清理」，
  而那种确定性失败判定是被**永久缓存**的（`utils/filePreview.ts` 只缓存确定性失败），
  清缓存的唯一入口就是 `onFileDone` 的 `invalidateFilePreview`。不触发 ⇒ 文件早好好躺在
  磁盘上、气泡却永远空白，重启前不会自己好回来。

**② 是进度事件被静默丢弃**：`updateTransferProgress` 在 `transfers` 里找不到该 id 时直接
no-op 且无重放，而多选发送是"每个文件各自 `void refreshTransfers()`"的并发 IPC ——
后发先至的**旧快照**里没有刚建的那条 transfer，覆盖回来后所有进度事件都落空，
进度条就永久停在 0%（此前怀疑的 FileCompleteAck 丢失是错的假设，已推翻）。

- `refreshTransfers` 加请求序号，旧快照不得覆盖新状态；两次 IPC 改 `Promise.all`。
- 进度/done 事件找不到行时不再静默丢弃，改为防抖一次补拉（400ms，不放大 IPC）。

验证：npm test 489 全绿（新增 2 条）、cargo test --features bluetooth 570 全绿、
vite build + vue-tsc 通过。

## [4.22.16] - 2026-09-20

### Fixed (点完大文件不再"卡一会儿" — 气泡前不做整文件扫描；导入复制不再占住 async worker)

真机（Mac 发送端）：点一个大文件后聊天框里要过一会儿才出现「发送中」气泡。这与用户的产品
设计直接冲突 —— 任何操作点击后必须立刻乐观响应。

链路上挂在气泡前面的是两次 O(体积) 整读：
- `commands/files.rs::build_file_message` 在建发送记录时整读文件算 sha256；
  而**投递任务待会儿还会为 FileOffer 再整读一遍**（`network/file.rs` 里本来就是它算校验值）。
  现在建记录时 `sha256` 先留空，投递任务算完用 `db::fill_message_sha256` 回填同一份值
  （幂等：同值不再写；缺行静默通过 —— 内容补发复用同一个投递函数，那条路径没有 `file-*` 行）。
  少一次整读只是顺带的好处，真正的点是**气泡不再等任何 O(体积) 的活**。
- `import_picked_file`（Android `content://` → 缓存目录）用同步 `std::io::copy` 在 async
  命令里整复制，600MB 会占住一个 tokio worker，连带拖住同进程其它传输的进度事件与 DB 访问。
  改 `spawn_blocking` + `sync_all`。

顺带修掉一个同源并发缺陷：导入落盘名用 `gosslan-<毫秒>.<ext>` 兜底，而多选是并发的
（前端 CONCURRENCY=2），两个 URI 解不出名字的文件会在同一毫秒撞上同一个 `dest` ——
`rename` 到已存在路径是**静默覆盖**，此时第一份字节可能正在被哈希/分片发送。
改用 `file::unique_path` 保证目标名不存在（真机形状：一次发 9-10 张总有 1-2 张预览不出，
单独发同一张必成功）。

测试：`fill_message_sha256_backfills_once_and_keeps_other_fields`（用 AFTER UPDATE 触发器
数真实写入次数：同值必须 no-op、空值与缺行不得产生写入、只补一个字段不得丢其余键）；
源码守卫 `no_whole_file_scan_before_the_file_bubble`（钉住"建记录前不得整读文件"这条
很容易顺手写回去的不变量）。

## [4.22.15] - 2026-09-20

### Fixed (verify.mjs 在 macOS/Linux 上从第 1 步就 ENOENT — 后面 14 步从未跑过)

`NODE_EXE` 无条件把 `process.execPath` 包在双引号里（Windows 的 `shell:true` 需要，
因为 Program Files 有空格），而 macOS/Linux 走 `shell:false` ⇒ **引号成为文件名的
一部分** ⇒ 第 1 步 `spawnSync ENOENT`，fail-fast 之下后面 14 步（含 clippy -D warnings、
Rust 单测、护栏非空转、Android 编译门禁）在 unix 上从来没执行过。CI 自己重列步骤，
所以这条空转只在本地可见 —— 正是审计结论「CI 与 verify.mjs 双份清单必然漂移」的实例。
改为只在 Windows 加引号。

## [4.22.14] - 2026-09-20

### Fixed (文件分片流钉死单条链路 — 修多文件并发时大文件必失败)

真机（用户 2026-09-19）：单聊一次多选里 500-600MB 的文件跑到 100% 报
「文件分片顺序错误 / 接收失败」、发送端同步判失败；群里连发 9-10 张图总有 1-2 张收不全。
**单独发同一个文件/同一张图必定成功** —— 典型的"只在并发下才成立"的形状。

根因是文件分片用了普通消息的投递路径 `try_send`，而它是**逐条**重算选路的：
一个 peer 可以同时持有 LAN + Routed(+BLE) 多条独立 TCP 连接，第一轮全用非阻塞
`try_send`，**队列满就算这条链路"现在不行"并 failover 到下一条**（这是 M3-b 给普通
消息设计的正确行为）。文件分片是有序字节流：接收端 `write_chunk` 要求 seq 严格递增、
追加写且不 seek，一片跨了连接就永久失序 ⇒ `ChunkSeq::Gap` 判死整条传输。
空闲时队列不满、永远走第一条链路，所以单发看不出问题；两个流共用一条 1024 槽的
Low 队列（每连接一套），满了才第一次真正触发 failover。

- 新增 `transport::resolve_stream_link` + `send_on_link`：一条分片流在开始时**钉住
  一条链路**，队列满时原地等背压而不是换链路；不设局部超时（链路真死时 writer 循环
  退出会 drop Receiver，`send` 立刻 Err；短超时放弃反而会留下在途旧分片，与续传尝试
  的 seq 从 0 重数混进同一条追加写的流）。整体上限由 4.22.13 的体积自适应 deadline 兜。
- **FileDone 一并钉住**（单聊与群聊）：它与分片同为 Low 优先级，走 `try_send` 时
  会在队列满的那一刻被路由到空闲连接，于是完成帧**超过仍在路上的最多 1024 片
  （≈262MB）**先到，接收端判「文件传输未完成」打死传输 —— 这正是"跑到 100% 才失败"。
- 分块大小改为读**钉住那条链路**的 `path_kind`（原来读 `inbound_path_kind` 的另一次
  选路结果，两者在多条连接时可能不是同一条）。
- 群发路径补齐 4.22.13 漏做的一处：群文件仍用固定 10min 窗口，500-600MB 必被误判超时。
- 测试：新增 3 条（满队列原地背压不乱序、链路关闭立刻 Err、三级通道不被绕过）；
  源码守卫 `ble_file_transfer_respects_link_limits` 升级为同时钉住"分片与 Done 同链路
  + 不得逐片 try_send"两条不变量。

## [4.22.13] - 2026-09-19

### Fixed (文件消息体验链：群图片预览、视频退化成 JSON、大文件超时误判)

真机反馈的四联问题（群照片预览不了 / MP4 显示成 JSON / 几百 MB 大文件必失败），
根因是**同一条链的四个断点**：

- **接收端按文件名重新猜 kind**（单聊 FileOffer / 群 offer / 完成回填三处）：
  4.22.1 只收口了发送端，接收端仍把 mp4 写成 `kind="video"`——前端渲染链只认
  text/code/image/file/…，视频消息整个退化成一段裸 JSON。三处统一为 `image|file`，
  细分留在 `content.subtype`。
- **历史行兼容（不回写数据）**：`protocol::display_kind` 在 DB 读出口把 legacy
  `video/audio` 归一为 `file`（已入库的旧消息不再显示 JSON）；`code` 有歧义
  （真代码块同为 kind=code）刻意不映射。测试 ×1。
- **群图片预览打不开的真根因**：群文件完成时后端会**重发带本地 path 的消息记录**
  （applyIncoming 注释也这么宣称），但 store 的合成循环对已有 msg_id 一律丢弃——
  path 回填永远到不了 UI，气泡停在无 path 形态。`applyIncoming` 改为 upsert：
  同 msg_id 以 DB 记录刷新内容、送达状态仍只前进（重复投递的正常路径行为不变）。
- **大文件固定 10min 期限必败**：600MB 在 Wi-Fi/中继链路跑不完固定窗口 ⇒ 判超时、
  重试又从头开始。`send_deadline_for(size)` 按 512KiB/s 保守吞吐伸缩（下限 10min、
  上限 2h，再大交给断点续传），超时报案文案同步动态值并明说「将从断点自动续传」。
  测试 ×1。
- **已知未闭环（需要真机日志）**：「对方已收到甚至已读、我方仍显示发送中 0%」——
  发送侧 delivered 推进链本身是齐的（file.rs 完成段 set status + emit），最可疑的是
  **FileCompleteAck 在拥塞链路上丢失/迟到**（ack 走一次 `try_send` 不重试）。
  请复现一次并抓 `[FILE]` 前缀日志（4.22.2 起 logcat 已放行），下一轮据此收口。

## [4.22.12] - 2026-09-19

### Fixed (存储：自动缓存清理真正接线 + 递归扫描 + favorites 纳入配额)

README 承诺的「自动缓存清理（3/7/30 天/永久 + 磁盘配额）」此前**只有设置页手动按钮**：

- 新增 6h 周期自动清理任务（启动 6h 后第一轮，spawn_blocking 里跑，不占 tokio worker、
  不跨 await 持 db 锁）；**只有真删了文件才 VACUUM** —— 旧实现每次手动清理都无条件
  持全局 db 锁 VACUUM，大库时整个应用冻住（评审 P1 同源）。
- `walk_files` 递归扫描：图片/文件按子目录分层落盘（todo-paste/ 等），旧的单层
  read_dir 让整棵子树**既不占配额也永不回收**——配额判断用的是一半的真实数。
- `favorites_dir`（收藏的独立媒体副本）纳入媒体目录：此前收藏越多越接近无限增长。
- 手动「立即清理」行为不变（仍 VACUUM，用户主动等待可接受）。

## [4.22.11] - 2026-09-19

### Fixed (内容重试链真正封顶 + 共享目录帧双好友校验)

code review 第三批（评审必改#1 + 安全 P1）：

- **`MAX_CONTENT_RETRIES` 此前实际永不触发**（4.22.1 只接了判定，链路是断的）：
  ① receive 起点的 upsert 每次带 `attempts: 0`，SQL 无条件覆盖 → 计数被压回；
  ② 要修的「对端重装/文件已删」场景里 ContentRequest **无人应答**，那条路径上
  根本没有 record_failure 调用点 → 计数停在 0，每次 Hello 无限重发。
  现在：`attempts = MAX(旧, 新)`（计数语义「这条内容对这个 peer 试过几次」只增不减），
  且每次到期重发本身就记一次失败（Timeout/LinkDown）→ 8 次窗口后收口 Rejected、
  退出可恢复列表。回归测试 ×2。
- **共享目录 ShareTreeRequest / ShareFileRequest 补双好友判定**：`from` 是自报字段，
  旧校验单查它 ⇒ 一个好友可冒用另一好友的 id 浏览/下载别人的共享目录。
  不收紧成 `from == peer_id` 的原因：借一跳中继的合法帧到达时链路的对端是转投邻居。
  残余风险（中继好友可代转他人请求）在注释中显式登记——两跳均须在信任圈内，
  且 4.22.9 已让转发本身吃上中继授权。

## [4.22.10] - 2026-09-19

### Fixed (sweeper 三条队列统一离线保留 + 队列选择回归分类表单一来源 + v8 索引)

code review 第二批（评审 agent 与自查共同命中，5e/6/4 三条建议）：

- **群 outbox 补上离线保留**（4.21.2 只修了单聊，群是同型缺陷）：行级返回后按成员
  分类——某成员离线 ⇒ 他的行保留到 7 天窗口（上线补发），**所有行都该放弃**整条消息
  才置 failed。旧行为：一个成员离线 2 分钟，群消息对所有人一起被判死。
- **文件 outbox 同样接入离线判据**：离线接收方的文件此前 30 分钟一律判 failed
  （「关机一晚回来收不到大文件」与单聊被修的 P0#2 同型）。判定收进通用纯函数
  `should_fail_expired(reachable, age, deadline, hold)`，三条队列一个判据。
- **队列选择旁路收口**：心跳循环与握手回发 Hello 曾恒走 Normal（分类表判 High）、
  `broadcast_gossip`/`update_profile`/`broadcast_chat_style` 各自手写降级判据——
  现在四处全部现场调用 `message_priority`，「单一事实来源」从口号变成真（评审建议#6）。
- **sweeper 锁开销**：每 tick 一次 links 键集快照复用（旧写法每个候选行抢一次
  links 锁，500 离线行=500 次/tick）；配套 **v8 迁移**给 outbox/group_outbox/file_outbox
  建 `created_at` 索引（离线 7 天保留窗会让行数上来了，扫描成本必须跟着），
  SCHEMA/schema.sql 同步，软失败风格。

## [4.22.9] - 2026-09-19

### Fixed (P0：数据面转发接上中继授权 —— 设置开关从此管到文件与外部帧)

- **控制面/数据面两套真相收口**：中继授权（ADR-0016）此前只管 gossip；
  `handle_message` 的定向借道（共享目录三件套 + RelayFileOffer）、`RelayChunk`
  转投、`OpaqueExternal` 转投三个数据面路径**完全不吃策略** —— 用户把中继设为
  「关闭/仅好友」，陌生邻居照样能借本机一跳一跳地跑文件流量（带宽与隐私双重失信）。
  新增 `relay_policy::decide_relay_from_peer`（真值表与 gossip 同源；授权主体取
  **经 Hello 验签的链路对端**，不用可自报伪造的帧内 `from` 字段），三处全部接入。
  默认策略 `All` 行为逐字节不变，属安全开关真正生效。
- **静默丢片留痕**：三处 `let _ = try_send()` 全部改为 `Err → log_throttled warn`
  （relay_drop/relay_deny，10s 限频）—— 分片丢弃以前在本机日志里不可见，
  「文件传一半失败重来」无从归因（INV-005）。
- 测试：`peer_relay_respects_policy_matrix` 真值表（含 All/Off 零查库断言）；
  源码护栏 `relay_data_plane_respects_policy`（三个转发点少接一处即红）。

## [4.22.8] - 2026-09-19

### Fixed (code review 修正：v7 迁移口径数据事故 + upsert_peer 热路径)

- **v7 孤儿清理的 DELETE 口径是错的（自审必改#2）**：消息/墓碑两条用了
  `conv_id NOT IN conversations` 单锚，而「用户删掉群会话」是合法操作且 groups 行还在
  —— 按旧口径升级会把**整段在册群历史不可逆清空**，与自家注释承诺的「只清 groups 表
  没有的」直接矛盾。改为**双锚**（groups 与 conversations 都不在册才算孤儿），
  并在 `migration_v7_purges...` 加陷阱用例（群在册+会话被删 ⇒ 一条不清）。
- **v7 风格对齐 v3-v5**：清理语句逐条 eprintln 容错，不再用 `?` 上抛 ——
  一条 DELETE 失败（BUSY/磁盘满）不该让 `db::init` 变 Err 把应用锁死在启动页。
- **`upsert_peer` 的 friends 锚查询挪到「条目确实不存在」时**（自审必改#3）：
  4.21.5 的实现只要 announce 带公钥就抢全局 db 锁跑 SELECT，
  5s×N 节点在千节点规模 ≈ 200 次/秒与所有写路径争锁；peers 已存在的条目纯白读。
  预检 `contains_key` 与插入之间的竞态无害（Some 分支同样有冲突判定）。

## [4.22.7] - 2026-09-19

### Fixed (P0：删除级联收口——删会话/退群不再留「幽灵数据」)

审计坐实的三处漏口一次收口（schema 无 FK，级联全靠手写，此前只覆盖了单条消息删除）：

- **`delete_conversation` 不清投递队列**：被删会话的 `outbox`/`group_outbox`/
  `file_outbox` 行留在库里 → 下次建链/心跳把**已删除的消息**补发回去（「删了又冒出来」），
  还会给不存在的会话发 FileOffer。现在同事务清空在途队列与撤回墓碑；
  Card/Silent（公告、待办、置顶）维持既有的「不属于聊天历史」保护不动。
- **`delete_group`/退群不删消息与回执**：退群后整段历史成为孤儿——仍命中全局搜索、
  点又点不开、挤占「共 N 条」。现在单事务级联清理 messages/group_outbox/file_outbox/
  group_reads/pending_group_reads/撤回墓碑/clear_boundary 键。
  **刻意保留** `conversation_clocks`（逻辑序号只增不减，删了会撞历史序号）；
  `content_transfers` 按 cid 跨会话共享、需引用计数，登记为已知限制未动。
- **`search_history` 只搜在册会话**（`conv_id IN conversations`）：兜住存量孤儿与
  未来任何漏网路径。
- **DB v6→v7 迁移**：一次性清掉收口之前入库的孤儿群消息/投递/水位行
  （保守口径：只清 groups/conversations 双不在册的数据）。
  降级拒绝机制不变（v7 库在旧版 App 上开不了，属设计行为）。
- 测试：`db::cascade_tests` 4 条（队列清空/Card 保留/时钟保留/搜索孤儿/迁移边界）；
  既有 4 条搜索/撤回测试补 conversations fixture——它们此前隐式依赖「孤儿可搜」，
  正是本次收口的对象。

## [4.22.6] - 2026-09-19

### Fixed (P0：gossip 转发候选从「知识集」换成「可达集」，跨网段中继不再是空转)

- **`choose_fanout` 的三个转发调用点（gossip 广播分支 / 定向帧洪泛兜底 /
  OpaqueExternal）此前都拿 `peers.keys()` 当候选**：`peers` 是知识集——Presence/announce
  跨跳登记，异网段节点在里面但**没有 TCP 链路**。跨网段场景下扇出全部拨向不可达节点，
  `try_send` 失败又被 `let _ =` 静默吞掉——「节点互相帮转发」这条产品核心承诺恰好在
  最需要它的场景空转，且界面上零异常（本地收发全正常），只有跨网段压测才暴露。
  现在统一收进 `reachable_neighbors`（`links` 中非空链路的邻居），与源发侧
  `broadcast_gossip` 一直使用的口径一致。BLE 链路同表登记 ⇒ mesh 桥接路径一并生效。
- 新增源码护栏 `gossip_fanout_targets_reachable_links`（candidates 再退回 peers 即红；
  写护栏当天就抓到一处漏网调用点）。

## [4.22.5] - 2026-09-19

### Fixed (移动端体验批量收口：多选并发 / 预览返回栈 / TabBar 恢复 / 输入区细节)

- **一次选多个文件发送**（用户 2026-09-19：「只能一个一个发」）：`attachFile` 改
  `multiple: true`，string|string[] 统一成数组；**并发上限 2**（Android ART heap
  256MB，一个文件 import+sha256+send 峰值 ~40MB，5 张并发真机 FATAL OOM——
  WhatsApp 国内版同款保守值）；单个失败不阻塞其余（走 sendFileTo 的 failed 态）。
- **图片预览返回栈**：`ImageLightbox` 自己注册 `useBackLayer` —— 此前 Android
  侧滑返回先把 ChatWindow 层弹掉、预览还挂着（「预览页还在但背后聊天回到列表」）；
  注册顺序天然正确（子组件后 mount → 先弹出）。新增 scale≠1 时的「还原缩放」按钮。
- **收藏页 TabBar 消失修复**：`closeFavorites` 不再改 `mobileView`（改回 'list'
  会触发 useBackLayer #1 的 release、多退一次历史条目）；可见性只由
  favoritesOpen 的 translate 条件表达。
- 输入区 safe-bottom 6px→8px + Composer 垂直 padding 对齐 Material 3 基线；
  FriendProfile「发消息」按钮 `shrink-0 whitespace-nowrap`（长昵称窄屏挤压换行）；
  未知 kind 兜底气泡加 `break-all`（超长无空格 token 撑破气泡宽度）。

## [4.22.4] - 2026-09-19

## [4.22.3] - 2026-09-19

### Added (Android 媒体兼容：HEIC/HEVC/动态照片进聊天)

**症状**：一加 15（Android 15）拍的图/视频进 Gosslan 后，桌面端黑屏或裂图——
iPhone 系 HEIC/HEVC 在 Windows/macOS 浏览器渲染链上都不支持；此前移动端选图
按扩展名猜 MIME，`content://` URI 没有扩展名可猜。

- **发送端自动转码**（`OpenWith.kt` + `android_open.rs` 三座桥）：
  `convertHeicToJpeg`（ImageDecoder→JPEG 85）、`isHevcVideo`（MediaExtractor 探
  hevc 轨）、`isMotionPhoto`（Google Motion Photo 容器检测）；API 31+ 视频走
  **系统级自动转码**：新增 `res/xml/media_capabilities.xml` 声明本 App
  「不支持 HEVC/HDR10/HDR10Plus」，Android 12+ 经 ContentResolver 读取时系统直接
  给 H.264（骁龙硬件转码，1 分钟视频约 5s、零 CPU 占用）。
  **已知限制（注释如实登记）**：API<31 老设备无视频兜底转码，HEVC 仍会原样发出；
  HEIC 图片转码失败时保留原文件发送（可当文件下载，不静默丢）。
  Motion Photo 主动降级为静态封面（与微信/QQ 同口径——跨端动效需端到端重构，不做）。
- **JNI 桥修根**：`native_attach` 收到的 class 参数是 `OpenWithKt`（顶层函数类），
  而所有业务方法在 `object OpenWith`（@JvmStatic）——旧代码把前者当类缓存，
  方法查找恒失败；改为 find_class 钉住 object 类。Kotlin 方法登记护栏同步
  （3 → 6 个方法，漏登记即红）。
- **`sniff_media_ext` 魔数嗅探补齐**（日志查看器的媒体内嵌预览复用同一条判定）：
  HEIC 家族（ftyp heic/heix/mif1/msf1/hevc）、MP4/MOV/3gp（各 ftyp 变体）、
  WebM（EBML）、AVI（RIFF…AVI 且校验第二槽）、FLV；27 条新用例覆盖全部盒子变体。
  前端 `filePreview.ts` 保留 heic/avif/bmp 的 MIME 兜底映射并注释「渲染不可靠，
  发送端已转码」的真实语义。
- **proguard**：OpenWith 三方法 + companion 的 keep 规则同步（release 包 JNI 反射入口）。

## [4.22.2] - 2026-09-19

### Fixed (macOS 辅助窗口 EXC_BAD_ACCESS + logcat 诊断链路)

- **辅助窗口命令必须回主线程**：`open_log_window`/`open_settings_window`/
  `open_group_todos_window`/`open_link_window` 此前是 async 命令（跑在 Tokio worker），
  而 macOS 的 AppKit 调用（`ns_window().setHasShadow` 等）**必须在主线程** ⇒ 真机
  EXC_BAD_ACCESS。改为同步命令（Tauri 在 wry 主线程 IPC 回调内联执行）；
  async/同步对窗口创建耗时**没有差别**（`build()` 本身就同步等主线程返回），
  真正的阻塞风险是 db 锁——窗口路径内所有 `db.lock()` 改 `try_lock` + 安全降级
  （背景色兜底浅色/群名空串），`blocking_commands_run_off_the_main_thread` 护栏
  登记 4 条例外并注明理由（源码扫描天然会把它们报出来，例外必须交代理由）。
- **Android logcat 诊断白名单**：info 级放行 `boot/ble/dispatch/file/content/transport`
  六类 target（release 包只有 logcat 可查，这几条是消息「卡在哪」的唯一观测面）；
  warn/error 行为不变。
- `send_message` 投递决策三态日志（has_link / try_send 结果 / NO-LINK→broadcast），
  排障「消息发出去了但对端没影」时不再靠猜。

## [4.22.1] - 2026-09-19

### Fixed (文件与内容可靠性：崩溃恢复 / 重试封顶 / 进度收尾 / 会话名与 kind)

- **file_outbox 崩溃卡死**：`flush_pending_files` 先标 `sending` 再发送，进程若在
  发送中途被杀（真机：80 张图并发 OOM/ANR），这些行永远卡在 `sending`——
  `list_pending_file_outbox` 只捞 `pending`，它们被彻底遗忘（「发了一半的图重启后再也没到」）。
  现在 AppState 初始化时 `reset_sending_to_pending` 全部重置回 pending，下次 Hello 触发
  flush 重投；重复传输由对端幂等 FileOffer 吸收。
- **content 自动重试无封顶**：`MAX_CONTENT_RETRIES=8` 定义了却没接进 `on_failure`——
  取不到的内容（对端重装/文件已删）会以 60s 周期无限重试。现在到达上限即收口
  Rejected（`retry_cap_turns_resumable_failure_terminal`）。
- **群文件/中继文件进度收尾**：最后一片可能因 250ms 节流不发 progress，前端卡
  「发送中 0%」；两条路径完成时补发 100% progress + file-done 事件。
- **会话名被文件名覆盖**：`touch_conversation` 的 INSERT 分支不查真实群名，
  群文件消息会把会话名写成文件名。group 类型 INSERT 前先从 groups 表取真名
  （UPDATE 分支早有 CASE WHEN 保护，补齐 INSERT）。
- **文件消息 kind 收敛**：`send_file`/`send_group_file` 此前把 subtype（video/audio/…）
  直接写进 kind，前端渲染链只认 image/file——非图片一律归 `file`，细分留在 content.subtype。

## [4.22.0] - 2026-09-19

### Changed (三级通道调度收口：分类单一来源 + BLE writer 忙轮询修复)

**收编并补完前几轮散在多处的工作（high/normal/low 三级发送通道）**：

- `Link` 的 bulk/priority 双队列升级为 **High/Normal/Low 三队列**；分类表收进新模块
  `network/dispatch.rs::message_priority`（唯一事实来源），`try_send`/`send_over_order`
  按分类选道。旧 `is_bulk_message` 删除，其真值表测试原样迁移
  （`bulk_messages_are_only_large_chunks` 名字不变，verify-guards 锚点同步搬到 dispatch.rs）。
- **同名常量两份不同值消除**（INV-P23）：dispatch 曾自带 `CONTROL_AVATAR_MAX_BYTES=256KiB` /
  `BULK_GOSSIP_PAYLOAD_MAX_BYTES=4KiB`，与 transport 真机验证值（2KiB / 16KiB）漂移——
  现在 dispatch 直接 `use` transport 的常量，单一来源、行为不变。
- **`FileCompleteAck`/`GroupFileCompleteAck` 归位 Normal**：曾被划进 Low，
  完成回执会排到 ≤1024 个分片后面（整文件重传或 30min 误判 failed 的入口）；
  它与本机上行分片流无顺序耦合，维持旧 priority 语义。
- **BLE writer 忙轮询修复（P0）**：`ble_writer_loop` 的 `high_open` 漏 `mut` 且 None 分支
  从不判 `high_rx.is_closed()`——链路被摘后该 select 臂每轮空转命中，100% CPU 且永不退出。
  feature 门控代码，clippy 静默；已在 `--features bluetooth` 构建下修正三臂关闭判定。
- **注释诚实化**：dispatch/BLE writer 曾宣称「fragment 级 yield」——实现只存在于测试
  模拟器；真实抢占粒度是**帧**（biased select，High 最坏等待 = 单条 Low 帧分片时长）。
  文档改为如实声明，片间 yield 需要四个平台 FrameSink 同步改造，列为后续工作。
- 删除无消费者的影子定义：`DispatchState`、`RETRY_BACKOFF_SECS`（与 `content::policy`
  真退避表冲突）、`FILE_OFFER_TIMEOUT_SECS`（transport 内联 15s 才是执行值）。

## [4.21.5] - 2026-09-19

### Security (P0：Gossip 信任链锚定到 friends 表，堵死「冒充缺席好友」)

- **两处缺口同源**：`peers` 是内存态（重启后为空、好友离线时缺席），而
  `handle_gossip` 的信任判定和 `upsert_peer` 的新建条目都只看 `peers`——
  「不在 peers」被当成了「陌生节点」。攻击者因此可以：
  ① 用自签信封发 `Presence`/`FriendRequest`/`FriendAccept`（unknown-sender 的 TOFU
  白名单），抢先把好友 id 的公钥绑成自己的；② 之后自己的 `Chat` 信封通过
  「已认识」校验，**伪造任意内容冒充好友**，`update_friend_pubkeys` 的 COALESCE
  还会把攻击者公钥写穿 friends 表的 NULL 位。
- **修复（对齐 ADR-0011 / INV-P11 / INV-P21）**：
  `gossip_trust_for_unpeer_sender`——friends 在册且绑过键的 id 一律只认绑定值，
  任何 kind 不再 TOFU；`new_peer_conflicts_with_friend`——`upsert_peer` 新建条目前
  先对 friends 锚做冲突检查，冲突走与既有 `key_conflict` 同一出口（不落库 + 诊断事件 +
  「可能被人冒名顶替」系统消息）。真正的陌生节点与加好友流程不受影响（与 ADR-0011
  的 TOFU 边界一致）；键列为 NULL 的旧好友行维持原宽容。
  回归测试 ×2（`gossip_trust_for_known_friend_never_tofus`、
  `new_peer_entry_respects_friend_key_anchor`）。
- **已知边界（登记在案，下一轮）**：`FriendAccept` 消费分支仍无「本机发过申请」
  前置校验（陌生 id 的伪造同意仍能加陌生人好友）；Gossip `Chat` 消费解密用信封自带
  `sender_pubkey` 而非绑定值，建议改为消费前与 peers/friends 锚核对。

## [4.21.4] - 2026-09-19

### Changed (工程：修 main 上既有的 CI 红 —— fmt 漏跑)

- **`cargo fmt --check` 自 `5e31ebe` 起在 main 上持续失败**：`db.rs` 与 `db/migration_tests.rs`
  入库时漏了格式化，把 Rust job 的 fmt 门禁整段挡红（macOS/Windows 两条腿同挂，
  后续所有 push 连带变红）。本次对这两个文件补跑 `cargo fmt --all`，不改任何行为。
  测试基线：`cargo test --lib --features bluetooth` 540/540。

## [4.21.3] - 2026-09-19

### Fixed (P0：message-failed 等状态事件前端无人接，气泡永久「发送中」)

- **后端在发、前端没听**（违反 INV-006「UI 必须反映真实状态」）：outbox sweeper 判 failed /
  用户取消 / 用户重发三条路径分别 emit `message-failed` / `message-cancelled` /
  `message-resending`，但 `bindEvents` 没有消费者——DB 已是 failed，界面气泡却永远停在
  sending/delivered，只有重开会话才纠正。现在三个事件统一接到 `onMessageStatusChanged`
  （从 DB 重查该会话，状态的唯一真相源是 `messages.status`，不再维护第二套内存状态机）。
- **例外清单失修**：`file-failed`/`file-cancelled` 早已监听，白名单仍挂「待接」谎报。
  本次清掉 5 条入账条目，并给 `events.test.ts` 加反向检查：**已监听的事件不得留在
  例外清单**，防止白名单再次说谎。

## [4.21.2] - 2026-09-19

### Fixed (P0：outbox sweeper 把「对端离线」当「发送失败」，离线补发承诺被击穿)

- **好友关机 2 分钟，消息就永久判 failed**：`spawn_outbox_sweeper` 对所有超过
  `OUTBOX_FAIL_DEADLINE_MS`（120s）的行一律删 outbox + 置 failed，不区分
  「发出去没收到 Ack」和「对端根本不在网上」——后者正是 README/INV-P04 承诺的
  「离线暂存、上线自动补发」场景。现在：候选行带回 `peer_id`/`created_at`，判定收进
  纯函数 `db::should_fail_expired_outbox`——有链路（可达）仍 120s 放弃；无链路按
  `OUTBOX_OFFLINE_HOLD_MS`（7 天）保留，到期才当僵尸行清理。补 2 个单测
  （判定语义 + 行数据完整性）。**已知边界**：群 outbox 维持原 120s 语义（判定按
  msg_id 粒度、删除整组行，需要单独设计，登记在案）。

## [4.21.1] - 2026-09-19

### Fixed (P0：单聊重发把明文推上线，重发实际是 no-op)

- **`resend_message` 重建 outbox 时直接用了库内明文**（`messages` 表存明文是设计如此，见
  send_message「本地落库（明文）」），没有 `enc1:` 前缀 ⇒ 接收端 `open_direct_content`
  一律拒收（不落库、不 Ack），2 分钟后又被 sweeper 判 failed —— 用户症状是「点重发没反应」；
  同时 `seq: 0` 若真被收进会排到会话最前（排序按 seq，INV-P09），两端顺序分裂。
  现在：重发路径与 send_message 同口径——查对端当前公钥（friends → peers）、`crypto::seal`
  重封后再入队，沿用原 `ts`/`seq`；拿不到公钥时回滚状态并明确报错，不再写一条接收端
  永远拒收的明文行；补「只能重发自己发出的消息」校验。新增源码护栏 `resend_reseals_before_enqueue`。

## [4.21.0] - 2026-09-19

### Changed (群任务/群管理/窗口外观/公告 —— 用户 2026-09-17 第二轮)

**群任务**：
- 表单图片区支持**粘贴与拖入**（此前只有文件选择器）：虚线投放区 + 提示文案「点击添加，或直接粘贴 / 拖入图片」；
  粘贴的位图没有本地路径 ⇒ 新增 Rust 命令 `save_todo_image_bytes`（raw IPC 收字节 → 按 sha256 落盘
  `cache_dir/todo-paste/` → 返回路径，之后与选图同一条路）；拖放用 webview 级 `onDragDropEvent` 命中投放区，
  并通过 `app.boardDropActive` 让 ChatWindow 的聊天拖放**让位**（否则同一份文件既进任务又被当聊天附件发出）。
- **列表行内直接切状态**：有权限的行，状态胶囊点开小菜单即可切换（同一时间只开一个），与详情共用
  同一份 `foldTodos` 数据源 ⇒ 两处状态天然同步。
- **创建/完成时间回显**：详情新增「创建于 {time}」；`TodoItem.createdAt` 由 `foldTodos` 从
  **创建那条记录**（`kind === "todo"` 的 ts）单独收集（LWW 折叠保留的是最新定义，两者是不同的记录）。

**群管理**：群名称修改并入「成员管理」弹窗（群主可改、非群主只读），独立的 `RenameGroupModal` 删除。

**窗口外观**：
- Windows/Linux 窗口按钮按 **Win11 Fluent 细线造型自绘**（lucide 的双箭头/短横线与系统明显不像）；
  `titleBarIcons.test.ts` 与 verify-guards 锚点随之重定位到 `data-win-glyph` 标记。
- **聊天输入框加 1px 描边**（此前只靠底色区分，与聊天背景糊在一起）。
- 侧边栏「链接」图标改**指南针**造型。

**侧边栏与窗口生命周期**：设置/日志收进底部「更多」二级菜单（互斥弹层 + 键盘可达），
且这两个窗口改**关闭即销毁**（`AUX_WINDOWS_RESIDENT = false`；外链窗口保持常驻并独立成
`AUX_LINK_RESIDENT`）；窗口状态插件对 settings/logs 加拒绝列表，避免销毁重建与"恢复旧几何"打架；
ADR-0018 §2.3 同步改写。

**群公告**：
- 横幅样式优化（浅警告底 + 描边 + 圆角，从"贴着背景的一条线"变成有承载面的横幅）；
  点正文打开**全文弹窗**（替代 toast）。
- 支持**删除公告**（仅群主）：新增 Rust 命令 `delete_group_announcement`（发 `announcement_delete` 墓碑，
  接收端 owner-only 校验已有）；前端两段式确认。
- **会话列表提醒**：有生效公告的群在列表行显示 📢 标记 —— 新增 Rust 查询
  `list_active_group_announcements`（一条 SQL 全量折叠公告+墓碑；不能从 `chat.messages` 折叠，
  那是 ~4 个会话的 LRU），store 缓存成 map 并在发布/删除/收到公告事件时刷新。

### Fixed (日志主题色 / 预览 JSON / 群任务图片 / 通讯录「自己」 —— 用户 2026-09-17)

- **日志窗口不跟随主题色**：日志入口此前完全不初始化 app store，`theme-boot.js` 只在首帧按
  localStorage 设一次主色 —— 之后在设置里改主题色，常驻的日志窗口收不到（没订阅 `settings-changed`），
  操作按钮一直用旧色。现在入口跑 `app.init()`（与设置窗口同口径：只读 + 订阅）。
- **会话列表/通知显示一段 JSON**：`poll` / `announcement` 是非静默 kind，Rust 的 `preview` /
  `preview_content` 与前端的 `previewText` 都没处理 ⇒ 直接把载荷吐出去。预览文案现在**唯一事实源**
  收进 `protocol::preview_text`（投票取问题、公告取正文、回应/撤回/置顶/删公告全部给人话），
  Rust 两侧委托它，前端 `previewText` 同口径。
- **群任务图片大概率打不开**：三处叠加 —— ① 只有一个人的群里发图直接报错（"群内没有其他成员"），
  图片元数据已进定义、字节却没有；② 缩略图首次读取失败（字节还没落地）被按 cid **缓存**，永不重试；
  ③ 字节后到没有任何重读触发。现在：solo 群发待办图片只登记本机内容、不报错；
  `loadContentPreview` **失败不缓存**；缩略图带退避重试（1.5s → 30s）追回后到的字节。
- **通讯录新增「自己」**：与「新的朋友」同款行（本机头像 + 昵称 + 「和自己聊天」副标），
  点开直接进入与自己聊天（复用 `openSelfChat`）。

### Changed (辅助窗口统一自绘标题栏 + 群任务列表重做 —— 用户 2026-09-17)

**用户症状**：①「新窗口（设置/日志）好像用的是系统的样式？标题栏和整体窗口的背景颜色有明显的界限，
没有融合」；②「有的窗口应该不支持最大化、最小化吧」；③「群任务的样式还是太丑，至少参考飞书/微信/钉钉」。

**窗口外壳抽象成一份**：

- **所有应用窗口一律 `decorations(false)`**（此前辅助窗口没设，于是留着系统标题栏，那条底色由系统决定、
  应用改不了 —— 接缝就是这么来的）。设置 / 日志 / 群任务窗口现在与主窗口**共用** `TitleBar.vue`
  （它加了 `title` / `showMaximize` / `showMinimize` / `closeToTray` / `isMobile` 几个 props，
  并去掉了对 app store 的依赖，辅助窗口入口才能 import 它），顶部 caption 与内容同属一套配色。
- 新增 `window/AuxWindowShell.vue`：`decorations:false` 的两条后果（caption 底色 + 1px inset ring）
  只在**这一处**表达，主窗口与辅助窗口不再各写一遍。
- **窗口按钮作用于调用窗口**：`window_minimize` / `window_toggle_maximize` / `window_is_maximized` /
  `window_toggle_fullscreen` / `window_close` 此前全写死 `main`（辅助窗口点最小化会把**主窗口**最小化）。
  现在接收 `tauri::WebviewWindow`；`window_close` 主窗口=隐藏（托盘语义）、辅助窗口=`close()`
  （常驻的转隐藏、群任务的销毁）。顺带修掉"外链窗口若能调到 close 会藏起主窗口"的隐患。
- **按窗口区分能力**：设置/日志/群任务 = 可调整大小 + 可最小化 + 可关闭、**不可最大化**
  （builder `.maximizable(false)` + caption `:show-maximize="false"`，macOS 绿灯也不画）；
  外链窗口保留最小/最大化。另在 `apply_aux_geometry` 里 `unmaximize()`，防止 `tauri_plugin_window_state`
  按 label 把辅助窗口恢复成最大化。
- macOS：新增 `decorate_aux_window`（`set_closable(true)` 让 ⌘W 仍可用 + `disable_shadow` 去掉与圆角冲突的
  矩形阴影），三个 builder 调用；`AuxWindowShell` 在 `onMounted` 调 `apply_macos_window_shape`
  补圆角/主题背景（该命令本来就作用于调用窗口，此前只有主窗口调）。
- **外链窗口是唯一例外**（保留系统标题栏）：它加载第三方网页，我们的文档不在那个窗口里，
  套自绘栏只能改用 iframe，而大量站点有 `X-Frame-Options` 会白屏；且它下方是网页本身，不存在撞色问题。

**群任务列表改「微信式极简」**：

- 无卡片无边框，纯列表行 + 分组小标题（状态图标 + 名称 + 计数）+ 分割线；行内只留
  **序号 + 标题 + 元信息 + 状态胶囊 + 快捷「完成」**（行高统一，不再"行高参差、满屏 pill"）。
- **描述与图片、以及状态切换/编辑/删除/归档/恢复全部收进新增的 `TodoDetailDialog`**，点行打开。
- 筛选从自创圆胶囊改成**分段控件**（与应用设置页同一套配方），仍带计数。
- **列表有了自己的承载面**（用户 2026-09-17：「背景和任务条目颜色混在一起」）：此前行是**透明**的、
  直接坐在窗口底色上，只有 hover 才有底色 ⇒ 条目与背景同色。现在整份列表包进一张卡片
  （`rounded-lg` + `border` + `bg-[var(--gosslan-panel)]`，与设置页分组卡片同配方），
  分组小标题改成**带底色的条**（`bg-[var(--gosslan-bg)]`），分组之间一眼分得开。
- **状态切换改成显式两步**（用户 2026-09-17：「一不小心就把状态改了」）：详情里此前是一排 4 个
  分段按钮、**一点即写库**，而且与看板顶部的**筛选**分段控件长得一样，用户当成"切视图"就顺手点了。
  现在当前状态是一个胶囊（看清现状），改动要点开 `.gosslan-menu` 菜单再选，**当前项置灰不可点**；
  状态一变（完成 / 菜单切状态 / 归档 / 恢复）就**关掉详情**回到列表看它的新分组 —— 口径一致
  （用户 2026-09-17：「改『完成』也关闭弹窗保持一致」）。
- **新建/编辑改成表单弹窗**（用户 2026-09-17：「新建任务怎么还能操作列表筛选？」+「新建和详情样式统一」）：
  表单从列表内联块挪进 `BaseModal`（与详情同一套分区/字段/操作行样式），列表不再被表单推来推去，
  表单打开时看板被遮罩挡住 ⇒ 筛选自然不可操作；**编辑**表单里补上状态分段控件
  （表单内的改动到「保存」才写库，没有误触问题；新建恒为「待办」，不显示该行）；筛选的计数改成小圆片。
- **状态配色收敛成单一事实源**：`utils/todos.ts` 新增 `TODO_STATUS_PILL`，并把
  `TODO_STATUS_CLASS.overdue` 从 `danger-ink`（红）改成 `warning-ink`（橙）—— 同一个「延期」此前在看板里是橙、
  在成员面板里是红；看板与任务卡气泡不再各抄一份，并加单测锁死"延期不得再用 danger"。

**验证**：`cargo test --lib` **484 全绿**；`npm test` 除 2 条 zh-CN 环境既有失败外全绿（470 项）；
`npx vue-tsc --noEmit` 零错误、`npm run build` ✅。真机需人工确认见下（macOS 圆角/⌘W、Windows 边缘缩放、
嵌套弹窗层级）。

### Added (群任务独立窗口 + 左栏「外部链接」视图 —— 用户 2026-09-17)

**需求**：① 把群待办做成独立窗口；② 左边栏加一个可配置外部链接的地方，点开在独立窗口里渲染。

**群任务独立窗口**：

- **每群一个窗口**（label = `todo-<groupId>`，`WINDOW_GROUP_TODOS_PREFIX` + `src/utils/auxWindowLabels.ts`），
  窗口从**自己的 label** 解析群 ID —— 一个窗口只服务一个群，不做窗口内切群。桌面端用它替代应用内弹窗；
  移动端仍是弹窗（独立窗口是桌面能力），桌面创建失败也回退到弹窗。
- **关闭即销毁**（`AUX_GROUP_TODOS_RESIDENT = false`）：每群一个窗口，常驻会无界增长；销毁重建顺带保证
  每次打开都是新数据。设置/日志仍是常驻（`ensure_aux_window` 新增 `resident` 参数）。
- 看板抽成 `GroupTasksBoard.vue`（无壳），弹窗（`GroupTasksPanel`）与窗口（`GroupTodosWindow`）共用；
  `ToastHud.vue` 从 `ResponsiveLayout` 抽出，独立窗口也有 toast（否则窗口里的失败是静默的）。
- **列表观感对齐应用**（用户 2026-09-17 反馈"样式有点丑"）：任务行改成与群文件/群成员列表同一套
  扁平行（`hover:bg-[var(--gosslan-hover)]` + `text-[11px]` 的「·」分隔元信息），去掉厚重的卡片边框；
  每行加**序号**（按当前显示顺序 1..N，切筛选/切分组后重排）。
- **新增筛选胶囊**（用户 2026-09-17）：「全部 / 给我的 / 我创建的 / 已归档」+ 计数。
  「归档」从"列表底部一段容易错过的区块"改成**一眼可见的筛选**（用户反馈"归档在哪里"）；
  已归档视图带说明「完成之后可手动归档；完成后满 7 天自动归档」+ 逐条「恢复」。
- **归档改为「完成之后手动归档」**（用户 2026-09-17）：完成只记 `done_at`（**首次**完成才记，
  重复保存不改 —— 否则改个标题就把 7 天计时重置），**不再自动归档**；完成的任务留在活动列表的
  「完成」分组里，行内多一个「归档」按钮；未手动归档的满 7 天仍会由前端按 `done_at` 自动归档。
  后端 `update_group_todo` 新增显式 `archived: Option<bool>` 参数（`None` = 保留原值），
  推导逻辑抽成纯函数 `resolve_done_archive` + 单测（非完成态一律取消归档，杜绝"进行中却被归档"）。
- **绝不跑聊天 store 的初始化入口**：窗口用新增的 `chat.loadGroupTodos(groupId)`（只读一个群，不发
  群已读回执、不写共享 localStorage）与 `watchGroupTodos`（只订阅本会话的 `message-received`）。
  守门测试把「aux 入口不得 `chat.init(`/`bindEvents(`」从"禁 useChatStore"这一弱代理，改成**直接**禁这条。

**左栏「外部链接」**：

- 图标栏新增「链接」视图（`NavRail` → `LinksList`）：列表 + 增/改/删；点某条在独立窗口里加载它。
- 数据走**独立命令**（`list/add/update/remove_external_link`，镜像 `RoutedEndpoint`）而不是 `Settings`：
  后端能返回**真实校验错误**（`save_settings` 对脏值是静默忽略，用户会"以为保存了"）。
- **URL 协议白名单**（只放行 http/https 且 host 非空）：这个网址会被 `WebviewUrl::External` 直接加载，
  `javascript:`/`data:`/`file:`/`tauri:` 会带来本机攻击面；前后端各有一份同口径校验 + 两条表驱动用例。
- 链接窗口 label `link`：**刻意不进 `WINDOW_LABELS`、也不进 capabilities**（远端页面拿不到任何命令权限），
  并有 `link_window_is_not_capability_covered` 反向锁死；`on_navigation` 只放行 http/https；
  复用同一窗口（第二次打开是 `navigate` + 改标题，不会越开越多）。这是本仓库唯一一处在本机渲染第三方网页。

**守卫/架构**：新增窗口照 ADR-0018 补齐（HTML/入口/vite 输入/骨架 CSS/`dismissBoot`/capabilities/`WINDOW_LABELS`）；
Rust 侧 `aux_windows_open_their_own_document`、`aux_window_open_is_singleton_serialized_and_resident`、
`capability_covers_every_window_label`、几何用例均扩展到新窗口，并新增
`link_window_is_not_capability_covered`、`external_link_rejects_non_http_schemes`、
`group_todos_window_label_derives_from_group_id`（含与前端前缀的交叉核对）；`scripts/verify-guards.py`
更新了窗口单例注入锚点并新增 7 条非空转用例。

**验证**：`cargo test --lib` **483 全绿**；`npm test` 除 2 条 zh-CN 环境既有失败外全绿（469 项）；
`npx vue-tsc --noEmit` 零错误、`npm run build` ✅。真机需人工确认：桌面开某群任务窗口（标题带群名、
关闭即销毁、重开数据新鲜）、移动端仍是弹窗、链接增删改 + 窗口内加载。

### Changed (群任务优化：创建者 / 完整描述 / 图片 / 归档 / 滚动约束 —— 用户 2026-09-17)

**需求（7 条）**：① 标出任务发起人，创建者与 @ 到的人都能改指派人；② 约束面板内部滚动
（此前会越过内层窗口）；③ 归档（完成即归档 + 满 7 天自动归档 + 可恢复）；④ 指派成员时在群里
显式 @ 提醒；⑤ 完整长描述可看、可加图片（跨端同步）；⑥ 任务消息在聊天列表 / 通知里不再显示
原始 JSON；⑦ 搜索框**外框**（上一条"去掉输入框焦点方框"改动的回退）。

**协议 / 后端**：

- `TodoPayload` 增 `description` / `images[{id,name,size,sha256,subtype}]` / `archived` / `done_at`。
- `update_group_todo` 拆三档判权（**结构 > 指派人 > 状态**，顺序不能反），描述/图片传 `None`
  时**保留库中原值**（不让"只改状态"的请求把它们清空）。
- `may_update_todo`：改指派人 = 创建者 / 群主 / 当前被指派人（用户口径："被 @ 的人也能转派"）；
  改标题/删除仍是创建者或群主；改状态是创建者或被指派人。
- 完成（`status=="done"`）⇒ 后端记权威 `done_at`（**首次**完成才记，重复保存不改，不接受客户端自报）；
  `archived` 只由显式请求决定 —— **完成之后手动归档**，未手动归档的满 7 天自动归档（见下方 2026-09-17 追记）。
- 描述图片复用群文件管线：`send_todo_image`（`scope="todo"` ⇒ 不进时间线、不弹气泡，字节按
  `sha256` 登记 content store）；新增 `todo_image_meta`（选图只取元数据，不投递字节）、
  `read_content_preview`（按 cid 读回字节渲染缩略图，安全边界同 `read_file_preview`）。

**前端**：

- `utils/todos.ts`：解析新字段 + `isEffectivelyArchived`（显式归档 **或** 完成满 7 天）+
  `canEditAssignees` + `todoMentionsMe`（被指派 = 被 @，按 device id）。
- `GroupTasksPanel`：标创建者、展示完整描述（超长内部滚动）、图片缩略图、归档区 +
  「完成」即归档 + 「恢复」、内部滚动约束 `max-h-[60vh]`；表单新增描述与图片（选图 → 元数据随
  定义同步 + 字节走管线投递）。
- 时间线：`todo` 以新增的 `TodoCardBubble` 卡片渲染（此前落到"未知 kind 兜底"吐出原始 JSON），
  卡片含「查看任务」直接开面板；`messageHeight` 同步 `todo` 高度估算。
- 会话列表 / 通知：`previewText` 取标题（`[任务] 标题`）；指派给我时通知前缀「[任务@你]」，
  并点亮会话列表的「有人@我」红点。
- **任务被完成时给创建人提示**（用户追加）：识别「`creator` == 我 && `status` == done」的
  `todo_update`（本是静默事件），弹系统通知；正看着该会话时系统通知会被抑制，改用应用内 toast；
  按 `msg_id` 去重，避免重复投递反复提示。
- 聊天头部「局域网直连」徽标从 WiFi 扇形改为 `Router` 图标（用户追加：WiFi 图标让人以为是无线上网，
  而这里表达的是"同一局域网内直连"）。
- 修复 `ConversationList` / `ChatSearchDialog`：上一条焦点改动里的 `focus-within:border-transparent`
  会让**整个搜索外框**在聚焦时消失（只该去掉内部输入框的黑框，不该动外框）。

**验证**：`cargo test --lib` **480 全绿**；`npm test` 除 2 条 zh-CN 环境既有失败外全绿
（新增 `todos` 用例覆盖新字段 / 归档判定 / 改指派人权限 / 任务提及）；
`npx vue-tsc --noEmit` 零错误、`npm run build` ✅。
## [4.20.0] - 2026-09-17
## [4.19.0] - 2026-09-17

### Added (收藏 · 外部 PR by Ha1fice —— 2026-09-17)

PR #17 合入。微信式独立收藏：消息被删、会话被清理后收藏仍可访问。

- **独立 `favorites` 表**，`UNIQUE(msg_id)` 保证同一条消息重复收藏幂等
- **存 content 快照 + 媒体副本**：图片/文件的收藏副本复制到 `favorites_dir`，路径改写进 content
- **取源一律以库里消息为准**：前端只传 `msg_id`，Rust 端从 DB 取 content，防前端造假
- **路径越权防护**：`canonicalize + starts_with` 边界检查，收藏和删除都走同一套判据

### Added (桌面端未读角标 + 任务栏闪烁 · 外部 PR by Ha1fice —— 2026-09-17)

PR #18 合入。收到新消息时：

- 桌面端：系统托盘图标显示未读角标数字，窗口任务栏闪烁提醒
- 移动端：参数桩（`set_unread_badge` 签名保留，实际由系统通知通道负责）
- **失败静默**：角标是锦上添花，不影响消息收发主流程

### Added (Change Budget 守门:改动半径分级 + 重复犯案检测器 —— 2026-09-17)

Phase 6。核心认识:**小 diff 不等于安全** —— `4.18.7→4.18.10` 连着四个版本修同一个 BLE
分片问题,每版 2~4 文件 / +24~+136 行,全都"很小",每一个都在修上一个。所以判据有三个,
不是一个:

**`scripts/check-change-budget.mjs` 三判据**:

| 判据 | 规则 | 依据 |
|---|---|---|
| 变更分级 | L1(≤5 文件/≤200 行/1 领域)放行;L2(≤10/≤500/≤2 领域)需 `[plan]`;L3 或碰敏感文件需 `[impact]` | 9/10 真实修复落在 L1 内 |
| 敏感文件 | 碰 `protocol.rs` / `crypto.rs` / `schema.sql` **无论多小**直接 L3 | 一错就是安全/全库数据问题 |
| 重复犯案 | 同领域在最近 5 个 `fix` 中出现 ≥3 次 → FAIL | `4.18.7→4.18.10` 是 4 次;第 3 次就拦 |

- **豁免**:纯文档/工程文件(`*.md`/`*.txt`/`docs/`/`scripts/`/`.github/`)不计入预算;
  `chore(release)` 的版本五件套(package.json / Cargo.toml / Cargo.lock / tauri.conf.json /
  package-lock.json)豁免 —— 否则每次发版撞门。
- **范围语义 = 门禁向前看**:CI 用 push event 的 `before..sha`,本地用"未推送 commit"
  (`origin/<branch>..HEAD`)。**不重查已推送的历史** —— 那会把门禁变成对历史的审判。
- **测试接缝**:守门读真实 git 历史,没法"改坏源文件"验证 ⇒ 留 `--from-json`,非空转用例喂
  `scripts/fixtures/change-budget.json`(默认状态全 PASS,四条用例各破坏一个条件验证对应判据会红)。
- 接入 `npm run verify`(步骤 6)与 verify.yml frontend job(checkout 加 `fetch-depth: 0`,
  merge-base 与 push 范围探测需要)。
- **实测校准**:对历史 commit 的判定与预期一致 —— `fix(ble) d6e5c82`(3 文件/55 行)→ L1;
  `docs 240ccd8` → 豁免;`feat(chat) 7c03341`(42 文件/+2578/碰 protocol.rs)→ L3 无标记 FAIL,
  与计划预言吻合。当前分支犯案窗口 transport 2 次 < 3,第一版全绿。

**已知边界(诚实)**:管不了语义(+10 行能让整条链路发不出消息,那靠测试与不变量);
拆 commit gaming 靠犯案判据兜底;把 fix 写成 feat 属于"门禁被绕过"的流程问题,review 兜底。

### Changed (零风险债:消掉 `relay` 命名撞车 + 收敛一份双状态 —— 2026-09-17)

两笔之前悬而未决的零风险债一起勾掉。

**`relay_manager.rs` → `file_relay.rs`**：

`gosslan` 一直有两组同名"relay"的文件切片中继（`relay_manager.rs`，BitTorrent 式分发）
与 `mesh::router`（路由转发）—— 命名撞到一份 `lib.rs:19` 的注释化石上：
> `mod relay_manager; // 文件切片中继（BitTorrent 式分发），与 mesh::router 无关`

本轮 `git mv` + 改 4 处代码引用 + 删掉那条化石注释，模块名自带语义。
更新面：
- 代码 4 处（`lib.rs:19` / `state.rs:23` / `commands.rs:4999` / `network/file.rs:296`）
- 测试基线 8 行（`test-baseline.macos.txt:422-425` + `test-baseline.windows.txt:416-419`）
- `scripts/verify-guards.py:207` mock 文件路径
- docs：`domains.data.mjs`(routing 域 paths + persistence notes)、`migration-ledger.md`(关注点 9
  行 + 命名撞车表改"✅ 已消解" + 收口顺序前两项标 strike-through)、
  `ARCHITECTURE-EXPLAINED.md` 2 处、`audit-2026-09-13-mesh-ble-efficiency.md`、
  `README.md` 2 处、`AI_PROJECT_HANDOFF.md`。
**零行为改动**：测试名从 `relay_manager::tests::*` 改为 `file_relay::tests::*`，但函数体不动。

---

### Added (跨领域依赖守门 —— 把「每条 use 受 consumes 约束」机器化 —— 2026-09-17)

Phase 5 只完成了"看得见"(领域图 + 迁移台账),没完成"守得住"—— `scripts/check-domain-map.mjs`
只守图的形式(路径/不重叠/enforce 开关),不守图的依赖方向。本轮补足这块。

**做了什么**：

- **`docs/domains.data.mjs` 的 11 个领域新增 `consumes: [domainId]` 字段**，基于实测
  `use crate::xxx`（排除 `#[cfg(test)] mod tests`）推导：`transport` consumes `identity /
  persistence / files / messaging / routing / presence / platform` 七个 —— 多才正常，正是
  台账里说的「`db::` 穿透传输层」「一个关注点三个家」在代码层的具体形态。
- **`scripts/check-domain-deps.mjs`**（判据 G / H / I 三条）：
  - **G**：每个领域 `paths` 下的 `.rs` 文件（生产代码），`use crate::xxx` 落到另一领域时
    必须在那条 `consumes` 列表里（落地坐标 `file:line`，给"补 consumes 还是删 use"的修法）；
  - **H**：`consumes` 不能引用不存在的领域（typo 第一天就该红）；
  - **I**：`consumes` 不能引自己。
- 与 `check-domain-map.mjs` **分工互补**：图的形式 vs 图的依赖方向，两套都过 = 自洽；
  只过一套 = 要么补 `consumes` 要么删 `use`，绝不悄悄改写边界。
- 接入 `scripts/verify.mjs`（步骤 5，"领域依赖方向守门"）与 `.github/workflows/verify.yml`
  (frontend job 紧跟"领域图守门")，CI 无条件跑。
- `scripts/verify-guards.py` 新增 **3 条非空转验证**（`--only domain-deps`）：
  ① 故意加一条不在 consumes 的 use → FAIL；
  ② 把 consumes 误删成空 → 用现有 use 立刻穿帮；
  ③ 把 consumes 写成不存在的域 id → typo 当天拦下。

**为什么有了它不等于可以打开 `enforce`**：`enforce: true` 与 `consumes` 是两套独立开关，
前者管"图的形态对不对"（路径/重叠/enforce 与 secondHome 互斥），后者管"图的内核该怎么用
才对"。`enforce` 仍保持全 false（边界收口完成一个打开一个，Phase 6 的事）；`consumes`
是**随时打开的**——新增一条 use 立刻要 sign up，否则守门 FAIL。

**没有做也不会做的事**：
- 不扫 `#[cfg(test)] mod tests` 内的引用 —— 测试需要 mock / 接触内部状态，被圈进域约束
  反而会让测试改写得难看（状态机在 `findTestModuleRanges`）。
- 不扫前端 `src/*` —— `presentation` 域的边界另算（utils/api/composables 的相互 import
  模式与后端 crate 不同），而且 `domains.data.mjs` 里已注明。
- 不守 `activeHome` 是否属实 —— 那层是 `check-domain-map.mjs` 的边界，靠人诚实 + 台账
  里的 `file:line` 证据。

### Changed (修正 3 处过期的「未接线」声明，并收窄 2 处 allow(dead_code) —— 2026-09-16)

Phase 5 的台账上报了「文档与代码冲突，不得静默择一」的两条，动手核对后又找到第三条。
本轮**只改注释与 allow 的位置，不改任何行为**。

**修掉的过期声明**：

| 位置 | 原声明 | 实测 |
|---|---|---|
| `transport/bluetooth.rs:1-12` | 「当前实现提供了完整的 `Transport` 接口契约……**需要引入平台专用后端**」+ 一节「接入真实蓝牙后端的步骤」 | 那套步骤**早已做完**（`Cargo.toml` 已有 `bluetooth` feature、btleplug 已集成、driver 已实现并接线）。另有一节「接线时要做的（按顺序）」同样过期 —— 它列的扫描 → 候选 → 连接 → Hello → 登记 `state.links` **已在 `network/ble.rs` 完成**，只是没做在本模块内 |
| `transport/bluetooth.rs:37` | 「## 状态：**已实现、尚未接线**（7-e 的一半）」 | `network/ble.rs` 有 **5 处**真实调用（`driver::adapter` / `scan_peers` / `connect` / `BleConnection` / `writer.send_frame`）⇒ **已接线** |
| `transport/tcp.rs:11-14` | 「Phase 4 当前只落地 bytes 原语，**不改变任何现有收发路径**」+ 文件级 `#![allow(dead_code)] // 旁路阶段：待接线后移除` | 帧原语（`write_bytes` / `read_bytes` / `read_bytes_capped`）与 `TcpReceiver` / `TcpSender` 都**已接线**，且被 `network/transport.rs:57,62,69,1231,1232,1529,1613,2368,2369` 自述为"单一真相源（P-A03）"。未接线的只有 `TcpTransport` **一个结构体** |

三处都换成了**接线状态表 + 调用点 file:line**，并写明"动这一带代码前请先核对调用点，别信注释"。

**为什么顺手把 allow 收窄**：这三处声明都挂着 `#[allow(dead_code)]`，**把编译器本会给出的提示一起静音了** ——
这正是它们能存活很久的原因。所以：

- `transport/bluetooth.rs` 的 `driver`：模块级 `#[allow(dead_code)]` → 逐个标注
- `transport/tcp.rs`：文件级 `#![allow(dead_code)]` → 只有 `TcpTransport` 与其 `impl` 带 allow

**收窄后浮出 6 个真没用的项**（原先被文件级/模块级 allow 一起掩护着）：

| 位置 | 项 | 处理 |
|---|---|---|
| `bluetooth.rs` driver | `BleConnection.next_msg_id` 字段 | 逐个标注。**⚠️ 顺带发现潜在真问题**：该字段从没被读过，而 `BleWriter::send_frame` 用的是它**自己**的 `next_msg_id` ⇒ 「连接内消息号」有**两份状态、一份是死的**。已注释要求接线下一半时二选一收敛 |
| `bluetooth.rs` driver | `remote_id` / `is_connected` / `disconnect` | 逐个标注 + 说明"接线哪一处时会用上" |
| `transport/tcp.rs` | `TcpSender::send_bytes` / `TcpReceiver::receive_bytes` | 逐个标注：生产走 `write_frame` / `read_frame`（经 `AsyncWrite` / `AsyncRead` 实现），这两个直接方法只有本文件测试在用 |

**收益**：现在编译器重新能回答"这一层还有没有人在用" —— 若哪天 `network/ble.rs` 不再调
`driver::scan_peers`，会立刻 warning，而不是继续静音。

**验证**：`cargo check --lib`（默认与 `--features bluetooth`）均 0 warning。

### Added (领域图 + 迁移台账：先回答「这个关注点有几个家、哪个在跑数据」 —— 2026-09-16)

**这是一次只读审计**（没有改任何业务代码），产出两份文件 + 一个守门脚本。

**背景**：本仓库处在一次**半完成的 ADR 迁移**中（`network/` 老栈 → `transport/` +
`discovery/` + `mesh/` 新栈）。此时「域 = 某个目录」是**错的地图** ——
AI 会照着它去改那个"看起来更对但没在跑"的新家，而真跑数据的是老家，
于是症状不变或换个形态，出现「改完这个 bug 又冒那个」。

**产出**：

- `docs/domains.data.mjs` —— 领域图（11 个领域：identity / presence / friendship /
  messaging / routing / transport / files / persistence / platform / observability /
  presentation）。每个领域强制带 `activeHome`（**哪个家在跑数据**）与 `enforce`。
- `docs/migration-ledger.md` —— 迁移台账，逐条给出 `file:line` 证据。
- `scripts/check-domain-map.mjs` —— 守门（见下）。
- 两份文件挂进 `AI_ENGINEERING_INDEX.md` 的必读清单**第 4、5 位**（在 protocol-invariants 之前）——
  否则它们又是孤儿文档。

**⚠️ 为什么是 `.mjs` 而不是计划里的 `domains.yml`**：实测**工具链里没有 YAML 解析器**
（Node 无 `yaml`/`js-yaml`、Python 无 `pyyaml`，只有 Ruby 有）。为一个守门脚本引入 Ruby 依赖
不合适，而**在守门脚本里手写 YAML 子集解析器更糟** —— 解析错了会让守门静默失效，
那比没有守门更危险。`.mjs` 零解析、支持注释，且 Phase 6 的 Change Budget 能直接 `import` 它拿 `tier`。

**台账的核心结论**（11 个关注点）：

| 状态 | 数量 | 明细 |
|---|---:|---|
| ✅ 已收口 / 单家 | 6 | TCP 帧原语、BLE 外设、BLE 载荷预算、mesh、content、storage |
| ⚠️ **真双家未收口** | 1 | **局域网发现**：`network/discovery.rs`（**活**，`network/mod.rs:54` 起 spawn）vs `discovery/`（`DiscoveryManager`/`LanDiscovery` **无任何生产调用点**） |
| ℹ️ 三家但属正常分层 | 1 | **BLE 中央**：`network/ble.rs`（策略）+ `transport/bluetooth.rs::driver`（字节级）—— 有 7 处真实调用，不要合并 |
| ⚠️ 部分未接线 | 2 | 传输抽象（`route()` 分流）、文件切片中继（`ChunkData`/`RelayPlan`/`impl RelayManager`） |

「传输」一个关注点今天有**三个家**：TCP 数据面走 `network/`、BLE 数据面走
`transport/bluetooth.rs::driver` + 三个外设模块、控制面（开关/状态/分流）走 `transport/mod.rs`。

**命名撞车 3 处**（未消解，且作者已不得不用注释区分）：
两个 `transport.rs`（`network/` vs `transport/`）、两个 "relay"
（`file_relay.rs` 是**文件切片**、`mesh::router` 是**路由**，命名撞车已于 2026-09-17 通过 git mv `relay_manager.rs → file_relay.rs` 处理）、
两个 "discovery"（活的那个名字更难猜）。

**上报 2 条过期的「未接线」声明**（按 `AI_ENGINEERING_INDEX` 的规矩：文档与代码冲突不得静默择一）：
① `transport/bluetooth.rs:37`「已实现、**尚未接线**」——实测 `network/ble.rs` 有 7 处调用，**已接线**
（未接线的只是同文件的 `BluetoothTransport` 占位实现与 `route()`）；
② `transport/tcp.rs:14`「旁路阶段：待接线后移除」——实测 `network/transport.rs:40,57,62` 已用它的
帧原语并自述为"单一真相源"。**本轮只上报、不改**（Phase 5 是只读审计）。
这与 Phase 3 修掉的 `ble_framing.rs` 那句是同一个病：**"待接线"的注释在接线之后没人回头改**，
而且都挂着 `#[allow(dead_code)]`，把编译器本来会给出的提示一起静音了。

**新守门 `check-domain-map.mjs` 的 6 条判据**：A 结构（字段齐/id 唯一/tier 合法）·
B 路径必须存在 · C 一个文件最多属一个领域 · D `coverageRoots` 下不许有无主文件（必须显式列进
`unmapped`，不能靠"没提到"蒙混）· E `activeHome` 必须是自己的路径之一 ·
**F `enforce: true` 只能开在已收口的单家领域** —— 这条把「边界收口完成一个，打开一个」
从口号变成机器判定，也正是 Phase 6 的前置。

**守门脚本立刻抓出了我自己地图里的 8 个问题**（1 个不存在的路径 + 5 个文件被两域认领 +
2 个无主文件 + 1 个 `activeHome` 不在自己的 paths 里）—— 这就是「地图错了比没有地图更危险」
的现场实例。其中"两域认领"暴露了我一个建模错误：`mesh/` 的文件被 presence 与 routing 同时认领，
而正确的说法是 **presence 只是「消费」`PeerCandidate`，不认领 mesh 的文件** ⇒ 已改为 `consumes` 字段。

**验证**：`npm run verify` **11 步全绿**（132s）；领域图守门 **219 个文件无重叠、无遗漏**；
新增 3 条非空转用例（改坏即 FAIL、恢复即 PASS）；护栏 **97 → 100**。

### Fixed (Android 外设的载荷预算自己算了一遍 —— 硬编码 512/20 且放行装不下分片头的值 —— 2026-09-16)

**这是 Phase 3「BLE 单一事实来源」漏掉的第三处**，由 Phase 4 的 Android 编译门禁当场抓出。

```rust
// ble_android.rs::payload_mtu —— 改前
/// 该对端一次通知能收多少字节（未知 ⇒ 20，与 macOS 侧同口径）。   ← 又一句不成立的"同口径"
pub fn payload_mtu(&self, central: &str) -> usize {
    call_static_int("payloadMtu", central)
        .map(|v| if (1..=512).contains(&v) { v as usize } else { 20 })
        .unwrap_or(20)
}
```

两个问题：

1. **第三份实现**：硬编码 `512` / `20`，并自己写区间判断 —— 既没重新定义常量（逃过
   `check-ble-constants.mjs` 判据 B），也不是"重新实现具名函数"（逃过判据 A）。
2. **真 bug**：有效性判据是 `1..=512`，于是 `payloadMtu = 1..6` 这类**装不下 6 字节分片头**
   的值被原样接受 ⇒ `fragment(payload, 3, _)` 直接返回 `None` ⇒ **整条链路发不出任何消息**，
   而日志只说"帧无法分片"。规范函数要求 ≥ 分片头 + 1（7），否则退回默认 20。

改法：`payload_mtu` 只做 `i32 → usize` 的安全转换，换算交给
`ble_framing::notify_payload_budget`（与 macOS 同一份）。

**顺带**（也是 Phase 3 的遗留）：

- `ble_framing.rs` 的 `in_flight` 只被本模块测试使用 ⇒ 标 `#[cfg(test)]`。
  它此前靠模块级**无条件** `allow(dead_code)` 蒙混 —— 那条 allow 一删，Android 的
  `cargo check` 立刻报 `never used`。**`cargo check` 比 `cargo test` 更容易看见
  "只在测试里活着的项"**（测试构建里它们是被使用的），这正是 `check-mobile.sh` 的价值。
- 关掉 `bluetooth` feature 时 `ble_framing` 整个模块都是死代码（所有调用点都在该 feature 之下），
  `cargo check` 会刷 17 条 `never used`。改为按 feature **条件化**静音：
  `#![cfg_attr(not(feature = "bluetooth"), allow(dead_code))]` ——
  feature 关时整模块惰性（不静音就全是噪声）；**feature 开时不允许死代码**（这才抓得到
  "以为接线了其实没接"）。

**新增守门判据 C**（`scripts/check-ble-constants.mjs`）：三个外设平台
（macOS / Windows / Android）必须**委托**给规范换算，不许自己算一遍。A/B 判据都只盯"定义"，
而这处是"把换算内联进平台实现"，只有"这个文件必须出现规范调用"这一层能拦住。
配套 `verify-guards.py` 非空转用例（把它改回原样 ⇒ 必须 FAIL）。

**顺便标注一处未验证的语义**（不改行为）：Windows 的 `MaxNotificationSize` 按现有注释
"**已含** ATT 头"因而用 `att_payload_budget`（减 3），而 macOS / Android 的来源本身已是载荷
（不减）。该说法**尚未在真机确认**；若实际不含，我们每片会少发 3 字节 —— 属**偏保守**方向
（吞吐略降），不会像 Android 这个缺陷那样"直接发不出去"。已写进该文件注释。

### Added (Android 编译门禁接入 CI —— 2026-09-16)

**问题**：`scripts/check-mobile.sh` 早就写好了（`cargo check --target aarch64-linux-android`
+ 0 warning 判定），注释里也记着真实事故（2026-09-12：Android 目标 **8 个 E0433**、
整个安卓包打不出来）—— 但它**从未接进任何 CI**，纯手工门禁。于是 Android 专属代码
（`ble_android.rs` 等）在 macOS / Windows 两条腿上都不编译，等于没人看。

**做法**：

- `verify.yml` 新增 `android` job（`ubuntu-latest`）：装 JDK 21 + Android SDK + NDK +
  `aarch64-linux-android` target，跑 `check-mobile.sh --bluetooth`。
  ⚠️ 沿用 `build-android.yml` 的那个坑：`setup-android` 的默认 `packages` 含 `tools`，
  而 Google 已于 2026-09-15 把它下架 ⇒ 必须显式覆盖为 `'platform-tools'`。
- `npm run verify` 增加第 10 步「移动端编译门禁（Android）」。本地缺 NDK / rust target 时
  **显式跳过并打印原因**（记进汇总、不影响退出码）—— CI 上无条件跑，所以本地跳过不影响覆盖；
  结尾会提示"别把本地的 ⏭ 当成通过"。
- 失败诊断仍走 `ci-run.sh`（注解是唯一可匿名读取的渠道）。

**引入当天就抓到一个真 bug**（即上一条 Fix）。这正是这条腿存在的意义。

**验证**：`npm run verify` **10 步全绿**（169s）；`check-mobile.sh --bluetooth`
两种 feature 配置各自 **PASS / 0 warning**；macOS 的 `cargo check`（默认与 `--features bluetooth`）
均 **0 warning**；`cargo test --features bluetooth` **505 全绿**；护栏 **96 → 97**。

### Changed (BLE 单一事实来源：常量与换算收敛到一处 —— 2026-09-16)

**问题**：「一片能装多少字节」这件事，常量与换算此前有**多份**：

| 位置 | 内容 | 状态 |
|---|---|---|
| `transport/ble_framing.rs` | `att_payload_budget` + 函数内匿名 `BLE_DEFAULT_MTU=23` / `ATT_HEADER_LEN=3` / `GATT_MAX_ATTR_LEN=512` | 正典（但常量是函数局部的） |
| `transport/bluetooth.rs:110,112` | `pub const BLE_DEFAULT_MTU` / `ATT_HEADER_LEN` | **重复** |
| `transport/bluetooth_peripheral.rs`（macOS 外设） | `const DEFAULT = 20` / `const MAX = 512`，**自己实现一遍** | **第二份实现** |
| `transport/bluetooth_peripheral_windows.rs` | 调共享函数 | ✅ |

于是 `transport/bluetooth.rs` 里那句文档断言「**外设侧用的是同一个函数**」
**只对 Windows 成立** —— macOS 是另一份实现，数值恰好一致所以从未发作。

病史：CHANGELOG `4.18.7 → 4.18.10` **连着四个版本**修同一个分片预算问题
（4.18.7 没减 ATT 头 → 4.18.8「上一版修复生效但不够」→ 4.18.9 每片 514 > 512）。
根因不是某一行写错，而是同一个概念多处各算一遍。

**做法**：

- 常量提升为 `ble_framing.rs` 的 `pub const`（`BLE_DEFAULT_MTU` / `ATT_HEADER_LEN` /
  `GATT_MAX_ATTR_LEN` / `DEFAULT_PAYLOAD_BUDGET`），并**只在那儿定义一处**。
- 新增外设侧入口 `notify_payload_budget`（与 `att_payload_budget` 并列、语义不同：
  输入本身已是载荷、**不再减 ATT 头**），让 macOS 也能共用而不是自己算。
- `bluetooth_peripheral.rs::central_payload_mtu` 改为**纯转发**到共享函数（删掉两个匿名常量）。
- `bluetooth.rs` 删掉重复常量；那句只在 Windows 成立的文档断言**改正**。
- 删掉 `ble_framing.rs` 上过期的 `#![allow(dead_code)]`（模块早已接线：macOS/Windows/Android
  三个外设 + central driver + `network/ble.rs` 都在调它；那条注释与它静音的警告在接线后就过期了）。
- 新增 `docs/protocol-invariants.md` §23 `INV-P23 — One Budget, One Place`，并把
  「两侧必须推出同一个数」写进必测矩阵。

**新增守门** `scripts/check-ble-constants.mjs` + 3 条非空转用例。判据刻意避开"扫裸数字"：

- 判据 A：六个规范名字（4 常量 + 2 换算函数）在整个 crate 里**各有且仅有一处定义**。
- 判据 B：BLE 领域内 `const/static` **同时**满足「名字按 `_` 分词命中 `MTU/ATT/GATT/PAYLOAD/
  NOTIFY/CHUNK` 或本身是 `DEFAULT`/`MAX` 这类语义空名」**且**「值恰好是受保护字面量」⇒ FAIL。
  这条规则改了两版才定：只看值会误伤 `const CONNECT_ATTEMPTS = 3`（重试次数）、
  只看名字会漏掉真正的目标（`DEFAULT`/`MAX` 名字里没有概念词）。

**新增两条测试**（此前 macOS 侧**没有**任何两侧一致性校验，而那正是唯一没走共享换算的一侧）：

- `both_sides_agree_on_the_same_link_budget` —— `notify_payload_budget(att_payload_budget(m)) ==
  att_payload_budget(m)`；注入「外设侧也减一次 ATT 头」（4.18.7 的形态）即红。
- `notify_payload_budget_clamps_and_never_returns_zero` —— 边界与"绝不返回 0"。

**顺带修好 3 条失活的护栏用例**（`verify-guards.py --only ble` 此前是红的）：

1. `BLE 分片 MTU 异常值绝不返回 0` —— 锚点就在我删掉的 macOS 实现里（**本轮我自己造成的**）。
   重新指向 `ble_framing.rs` 并**去掉平台限制**（该模块无平台门控，现在三平台都有效）。
2. `蓝牙启动不得阻塞在 CoreBluetooth 状态回执上` —— 锚点的 cfg 仍是旧的两平台列表，
   而源码后来加入了 `target_os = "windows"` ⇒ 锚点 0 次命中。**既有腐烂**。
3. `BLE 拨号退避必须 1 分钟内恢复` —— 实现已被有意重设计（改为「前 3 次不退避，之后
   5s→10s→20s 封顶」），值、测试名都变了。**既有腐烂**。修的时候发现原注入方式本身是
   **空转**的：单改 `MAX_MS` 到 600_000 根本到不了分钟级（`step.min(2)` 已把增长压到 3 档），
   改为注入「去掉封顶」。这三条都属于"护栏静默腐烂、只有跑起来才知道"。

**基线更新**：macOS `503 → 505`（+2 条新测试）、Windows `493 → 495`。

**验证**：`npm run verify` **9 步全绿**（362s）；`cargo test --features bluetooth` **505 全绿**、
**零警告**；`python3 scripts/verify-guards.py --only ble` **23/23** 非空转通过；
护栏总数 **93 → 96**。

### Added (Windows 测试通道：让 Windows 专属的 BLE 外设代码被真正编译与执行 —— 2026-09-16)

**问题**：一部分代码是 **Windows 专属**的 ——
`transport/mod.rs:24` 的 `#[cfg(all(feature = "bluetooth", target_os = "windows"))]`
即 `bluetooth_peripheral_windows.rs`（Windows BLE **外设**角色 / WinRT `GattServiceProvider`，
**2 条用例**）。macOS 上它**不编译**，那 2 条一条都不跑，而 CI 照样全绿。

这不是假想的风险：`transport/bluetooth.rs` 里写着「外设侧用的是同一个函数」，
而实测**只有 Windows** 走了共享的 `att_payload_budget` —— macOS 那一侧自己抄了一份
`central_payload_mtu`（硬编码 `20` / `512`）。**这类跨平台漂移只有在两侧都被编译时才看得见。**
（Windows 那 2 条里恰好有一条就是 `peripheral_and_central_agree_on_payload_budget`。）

**做法**：

- `rust` job 改成 **matrix**：`os: [macos-latest, windows-latest]`，一个定义两条腿，
  `fail-fast: false`（两条都跑完，一次就能看到两个平台的情况）。
- **每个平台一个基线**：`test-baseline.macos.txt`（503）/ `test-baseline.windows.txt`（500）。
  基线必须按平台分 —— 两侧是互斥的 `#[cfg]`，拿 macOS 的基线去比 Windows 会把 5 条
  平台门控用例误判成「静默跳过」（纯误报）。
- `check-test-manifest.mjs` 增加**引导模式**：本平台基线缺失时，打印与其它平台基线的
  **差集**（新平台没法在别的机器上生成自己的基线）。差集通常只有几条，小到能塞进
  一条 CI 注解 —— 首次引导 Windows 基线正是这么做的。
- 新增 `scripts/ci-run.sh`：把「tee + 失败时合成一条多行 check 注解」抽成可复用脚本，
  Rust 腿与清单守卫共用（原先内联在 workflow 里，两份会漂）。

**⚠️ 坦白一处**：`test-baseline.windows.txt` 的 500 条是**按平台互斥的 `cfg` 推导出来的**
（= macOS 基线 − 5 条 macOS 专属 + 2 条 Windows 专属），**不是**在 Windows 上跑出来的。
Windows 腿第一次跑就会验证这个推导：对得上则绿；对不上则清单守卫会打出真实差集
（缺名 FAIL / 多名 WARN）。这是刻意选的路径 —— 推导错了不会静默通过。

**本地验证**：`npm run verify` 8 步全绿；`bash scripts/ci-run.sh` 成功/失败两条路径都实跑过
（成功不透传注解、失败以原退出码退出并合成单条多行注解）；引导模式的差集报告用一个
伪造的 `test-baseline.windows.txt` 演练过（正确报出 `+5` / `-2`）。

### Fixed (CI 首次跑测试就红：一条广播测试在 runner 上必然失败 —— 2026-09-16)

**这是本项目第一次在 CI 里跑 `cargo test`，结果是 502 通过 / 1 失败。**
红的那条与门禁本身无关，是它**在 CI 环境里本来就不可能通过**：

```text
test network::discovery::tests::discovery_recv_socket_actually_receives_broadcast ... FAILED
    panicked at src/network/discovery.rs:985
test result: FAILED. 502 passed; 1 failed
```

它是 2026-09-12 真机事故（「Mac 与手机同 Wi‑Fi 却互相搜不到」）的回归护栏，做一次
**真实的 UDP 广播收发**。原先的通吃条件是"没有可用 LAN 接口就跳过（纯 CI/容器）"——
但 GitHub 的 macOS runner **有** LAN 接口（`find_lan_interface()` 返回 `Some`），
却收不到自己发的 `255.255.255.255`，于是没跳过、直接 panic。

**修法**：把跳过条件从"有没有接口"升级为"这个环境能不能做本机广播"，用**证据**判定 ——
新增 `loopback_broadcast_works()`，收端**硬编码绑 `0.0.0.0`**（不是
`discovery_recv_bind_ip()`）。这个解耦是刻意的：探测必须与"被测代码的绑定选择"无关，
否则它区分不了"环境不支持广播"与"我们把绑定写错了"；拿已知正确的绑定去问环境，
失败就只可能是环境问题，**绝不会掩盖真正的回归**。

没有采用"检测到 `CI` 环境变量就跳过"：那会把碰巧跑在 CI 上的真机也一起漏掉。

**非空转验证**（按项目纪律）：把 `discovery_recv_bind_ip()` 改成 `Ipv4Addr::LOCALHOST`
（模拟事故原形态）⇒ 测试立刻红，并打出原始提示
「接收 socket（bind=127.0.0.1）收不到 255.255.255.255 广播」；恢复即绿。
本地（能广播）该测试**真的执行断言**（`--nocapture` 下无跳过信息），不是被探测误跳过。

### Changed (门禁：分支推送也触发 CI，按分支名去重 —— 2026-09-16)

原先 `verify.yml` 只挂 `pull_request` + `push: [main]`，于是**推一条分支上去什么都不会跑** ——
必须先开 PR 才有结果（2026-09-16 实测：推了 `chore/eng-hardening` 后 API 里一个 run 都没有）。

改成 `push: branches: ["**"]` + `pull_request` 两个都挂，并把 concurrency 的组键从
`github.ref` 换成 `github.head_ref || github.ref_name`：push 事件取到 `ref_name`、
PR 事件取到 `head_ref`，两者都是**分支名** ⇒ 「推分支」与「开/更新 PR」落进同一个组，
后启动的取消先启动的，**不会双跑**（原先 `refs/heads/X` 与 `refs/pull/N/merge` 会被分成两组）。

本仓库是 public ⇒ Actions 分钟数（含 macOS）免费，所以"每次推送都跑"没有成本顾虑。
⚠️ 已写进 workflow 注释：将来若启用 branch protection，建议改回只挂 `pull_request` ——
去重是"后者取消前者"的竞态，被取消的那次在 PR 上会显示 cancelled，branch protection 会判不通过。

**顺带解决一个真实的运维盲区**：Rust 单测步骤失败时，现在会把**诊断信息**提升为
**check 注解**。起因是 2026-09-16 第一次真跑 CI 就红了，但三条路都拿不到原因：

| 渠道 | 结果 |
|---|---|
| `actions/jobs/{id}/logs` API | 匿名 **403**（"Must have admin rights to Repository"） |
| 浏览器打开 job 日志页 | **"Sign in to view logs"** —— 公开仓库也要登录 |
| check-run 注解 | 只有一句 `Process completed with exit code 101` |

结论：**注解是唯一的公开渠道**（匿名 API 可取 `.../check-runs/{id}/annotations`），
所以把诊断放进注解。两个细节是踩出来的：

- **只挑关键行**（`test result:` / `error` / `failures:` / `panicked` / 信号 / 磁盘），
  不整段照搬 —— GitHub 每个 step 只保留约 **10 条**注解，照搬 30 行会被截断
  （第一次就踩了：拿到的全是 `... ok`，真正的错误落在截断之外）；
- **合并成一条多行注解**（换行编码为 `%0A`），进一步避开那个数量上限。

顺带打印 `df -h /`：冷编译 tauri 很占空间，runner 磁盘打满是 exit 101 的常见成因
（首次 CI 已用它排除了这个可能：余量 92Gi）。

### Added (统一验证入口 + 把护栏真正跑起来 —— 2026-09-16)

**背景**：上一条给 CI 装了门禁（PR 上跑测试），但验证手段仍然散在四处：CI 记一份、
`build-windows-release.ps1` 里手串一份、开发者脑子里再记一份。**没有单一入口，就没有
统一的验证标准** —— 谁记得跑什么就跑什么。本轮把顺序固定下来，并让 CI / 发布脚本 /
本地开发跑同一套。

**做法**：

- 新增 `scripts/verify.mjs` + `npm run verify`：按顺序跑
  ① 测试清单守卫（前端）② 不变量例外守卫 ③ CHANGELOG 结构 ④ `npm test`
  ⑤ `npm run build` ⑥ `cargo test --features bluetooth` ⑦ 测试清单守卫（Rust）
  ⑧ 护栏非空转（前端子集）。顺序有硬依赖，不能随手调换 —— ⑤ 必须在 ⑥ 之前，
  因为 `dist/` 不进版本库而 Tauri 的 `build.rs` 要读它（全新 clone 上顺序错了
  `cargo test` 根本编不过）。失败即停（fail-fast），末尾给逐步耗时汇总。
  另有 `--full`（加跑全量护栏）、`--no-guards`、`--list`。
- **`.github/workflows/verify.yml` 加跑护栏前端子集**。这是本轮最该进 CI 的一步：
  护栏失效时**不会有任何信号**（测试全绿、构建正常，只是它不再守任何东西），
  只有把它跑起来才知道。此前 92 条护栏全靠人工记得跑。
- 同时把「不变量例外守卫」与「CHANGELOG 结构」两个静态检查接进 CI。

**顺手修掉两条「空转」的护栏**（各自一条提交）——它们不是代码坏了，而是护栏自己失效了：

| 护栏 | 失效原因 | 修法 |
|---|---|---|
| 焦点可见 | 注入点所在的 `MessageComposer.vue` 被加了**文件级**逃生阀 ⇒ 整文件跳过，改坏也不报 | 注入点换到 `TitleBar.vue` 的关闭按钮（未被豁免、且是键盘可聚焦的 `button`）；再把逃生阀降到**元素级**（`data-focus-ring-ok`），让该文件其余元素恢复保护 |
| CHANGELOG 结构 | 它拿 `npm run version:check` 当命令，而那条命令的版本记账部分在**攒提交期间本来就该是红的** ⇒ 永远进不了「恢复即 PASS」 | 拆出 `npm run version:changelog`（只跑本就独立导出的 `changelogProblems()`）：结构是结构、记账是记账 |

**踩到的坑（写进代码注释与单测）**：元素级的 `data-focus-ring-ok` **含有** `focus-ring-ok`
这个子串，所以文件级判据不能还写 `src.includes("focus-ring-ok")` —— 否则「只豁免一个元素」
会被判成「整文件豁免」，元素级标记形同虚设。文件级令牌因此改为 `focus-ring-ok:file`，
并加了一条单测同时钉住「豁免不外溢」与「令牌不为子串」两个坑。

**有意没做**：`npm run version:check`（当前因两个历史提交缺 `Version-Bump:` 声明而红）、
`cargo fmt --check`（517 处差异）、`clippy`（未安装）都**不进**统一入口 ——
第一版入口必须全绿，否则大家会立刻开始绕过它。

**验证**：`npm run verify` **8 步全绿（106.6s）**；`npm test` **457 全绿**；
`cargo test --features bluetooth` **503 全绿**；前端护栏子集 **40/40** 通过非空转验证。

### Added (不变量例外登记：让「照文档误修」不再可能 —— 2026-09-16)

**问题（这是活的，不是假设）**：「和自己聊天」（`7c03341`）给两条核心不变量开了**正当**的例外 ——
落库即终态 `read`（跳过 INV-P03 的 `queued → sending → waiting_ack → delivered`），
且不写 outbox（INV-P04）。理由充分、注释也写得很清楚，落在三处：`commands.rs` 的
`insert_self_message` 文档注释、`verify-guards.py` 的「自聊消息必须留在本地」用例、
`src/utils/selfChat.ts` 的文件头。

但这三处**都不在 AI 的必读清单里**。`docs/AI_ENGINEERING_INDEX.md` 只指向
`protocol-invariants.md` 与 `AI_RULES.md`，而这两份当时**一个字都没提**这个例外。于是：

```text
AI 读 INV-P04「发送可靠消息 → insert message + insert outbox」
        ↓
看到 insert_self_message 只 insert_message、没有 outbox
        ↓
按文档判定这是 bug 并「修」它
        ↓
自聊消息进入 outbox ⇒ 永远等不到对端 Ack ⇒ flush_outbox 每次心跳重发
        ↓
「outbox 必然排空」被真的破掉 —— 而这次回归是「照文档修」造成的
```

**结论：局部注释不能替代规范文档。** 同一条知识写在实现旁边，对读实现的人有用、
对读不变量的人没用；而 AI 读的是不变量。

**做法**：

- `docs/protocol-invariants.md` 新增 §22 `INV-P22 — Exceptions Must Be Registered`，
  含一张**机器可解析**的例外登记表（用 `<!-- BEGIN/END EXCEPTION REGISTRY -->` 划边界 ——
  那份文档正文本来就到处是 `INV-Pxx`，不划边界就分不清「正文提到」与「登记为例外」）。
- `commands.rs` 的 `insert_self_message` 上方加 `// INV-EXCEPTION: INV-P03, INV-P04 — …` 标记。
- 新增 `scripts/check-invariant-exceptions.mjs`，**双向**校验：代码标了文档没登记 ⇒ FAIL
  （下一个人会被文档误导）；文档登记了代码没标 ⇒ FAIL（文档在说谎）；登记了文档未定义的 id
  ⇒ FAIL（笔误凭空造出一条不存在的例外）。
- `AI_RULES.md` §8 增「Exceptions Must Be Registered」，把规矩放进必读文件本身。
- `scripts/verify-guards.py` 补三条非空转用例（`--only invariant`）；新守卫接入
  `.github/workflows/verify.yml` 的前端 job（纯静态扫描，不需要编译）。

**顺带修正了我自己的一处判断**：最初以为自聊也破了 INV-P07（Gossip）与 INV-P10（E2EE）。
精读原文后**不是** —— INV-P07 只要求 TTL/去重/扇出有界、并不要求广播；INV-P10 管的是
「解密失败不得静默退明文」，而自聊压根没有密文。所以登记表**只登记 INV-P03 与 INV-P04**。
刻意不把未被违反的不变量塞进登记表 —— 那会让「例外」这个机制失去信号价值。

**验证**：`npm test` **455 全绿**；`cargo test --features bluetooth` **503 全绿**；
三条新护栏全部通过非空转验证（改坏即 FAIL、恢复即 PASS）；护栏总数 **87 → 92**。

### Added (工程门禁：让「测试静默不跑」不再可能 —— 2026-09-16)

**背景**：本仓库此前三个 workflow（`build` / `build-macos` / `build-android`）**全是打包**，
没有任何一个跑测试；git 钩子只有 AI 追踪器、没有 pre-commit。也就是说 455 条前端断言 +
503 条 Rust 用例 + 87 条非空转护栏，全部依赖**人工记得跑**。纪律很强，但没有门禁 ——
一次漏跑就能把回归合进 main。

**真实代价（实测数字）**：`bluetooth` 是**非默认** feature（`src-tauri/Cargo.toml` 的
`[features]`）。漏掉 `--features bluetooth` 时用例数从 **503 掉到 487** —— **16 条静默消失**：

| 模块 | 消失的用例数 |
|---|---:|
| `network::ble` | 8 |
| `transport::bluetooth_peripheral` | 5 |
| `transport::bluetooth::driver` | 2 |
| `commands` | 1 |

而 `cargo test` 依然**全绿、退出码 0**。这 16 条盯的正是 CHANGELOG `4.18.7 → 4.18.10`
连着四个版本边修边冒的那个子系统 —— 最需要护栏的地方，恰恰是"忘了加 feature 就静默不测"的地方。

**做法**：

- 新增 `.github/workflows/verify.yml`：**PR 触发**（只挂 `push: [main]` 等于合并之后才查，太晚），
  两个并行 job（前端 / Rust），跑 `npm test` + `cargo test --features bluetooth` + 前端构建 + 清单守卫。
- 新增 `scripts/check-test-manifest.mjs` 与 `src-tauri/test-baseline.<平台>.txt`：比对
  「基线名单 ⋈ 实际 `--list`」，**缺名即红**。刻意**比名字不比数量** —— 数量阈值（`>= 503`）
  会催生"为凑数保留已无价值的测试"，而删除一个过时测试反而要去改阈值；名字比对没有这个问题。
  判据方向刻意不对称：**缺名 FAIL**（那是静默跳过），**多名只 WARN**（新测试跑得好好的，不是故障）。
- 前端那一半守的是另一处同类脆弱：`npm test` 的脚本里是**手工枚举**的 48 条路径，
  新增 `.test.ts` 若忘了加进那串字符串，新文件不会跑而 `npm test` 依然全绿。
- 基线**按平台分文件**（`test-baseline.macos.txt` / `.windows.txt`）：`transport/bluetooth_peripheral.rs`
  与 `transport/bluetooth_peripheral_windows.rs` 是互斥的 `#[cfg]`，拿 macOS 的基线去比 Windows
  会把 5 条平台门控用例误判成"静默跳过" —— 纯误报。`verify-guards.py` 的 `platforms` 字段
  就是为同一个坑加的，其注释写着"不要留一堆假失败把真失败淹掉"。拿到本平台没有基线时**不猜、不退化**，
  直接报错让用 `--update` 生成（缺失时静默生成等于"没有基线也算通过"，正是要消灭的空转）。
- `scripts/verify-guards.py` 补两条非空转用例（`--only manifest`）：改坏即 FAIL、恢复即 PASS。

**没做的（有意）**：`cargo fmt --check`（当前 517 处差异）与 `clippy`（未安装）**不进门禁**。
第一版门禁必须**全绿** —— 现在加进来会让每次 PR 立刻全红，结果是所有人开始用 `--no-verify` 绕过，
而门禁一旦被绕过一次就永久失效。这两项作为独立技术债单独还。

**踩到的坑（实测，已写进 workflow 注释）**：`dist/` 在 `.gitignore` 里、不进版本库，
而 Tauri 的 `build.rs` 要读它。全新 clone 上 `cargo test` **根本编不过**：

```text
error: proc macro panicked
  --> src/lib.rs:387:16
  = help: message: The `frontendDist` configuration is set to `"../dist"` but this path doesn't exist
error: could not compile `gosslan` (lib test)
```

所以 Rust job 必须先 `npm run build` 再 `cargo test`，顺序不能调换。**顺带发现**：
`scripts/build-windows-release.ps1` 的 Step 2（`cargo test --lib --features bluetooth`）
在干净机器上会因此失败（Step 1 的 `npm test` 不产出 `dist/`）—— 记录在案，未在本轮修。

**验证**：`npm test` **455 全绿**；`cargo test --features bluetooth` **503 全绿**；
两个 job 的每一步都在本地按 CI 顺序实跑通过；清单守卫两个方向都做过非空转验证
（基线注入假名 ⇒ FAIL；抽掉 `--features bluetooth` ⇒ 精确报出那 16 条 ⇒ FAIL；恢复即 PASS）。
### Added (群任务列表：指派 + 四态状态 —— 用户 2026-09-16)

**需求**：「群里需要能够支持列一些任务列表，每个任务可以给一个或多个人，任务要能区分出
进行中、延期、完成等这些状态」。

**背景**：协议层与折叠逻辑在 `[4.17.0]` 就已经交付（`todo` kind + `send_group_todo` +
`utils/todos.ts` 的折叠 + 单测），当时的"未完成"清单第一条就是"任务卡片 UI、创建入口、聚合面板"。
本轮把它接到界面上，并按用户口径**改了数据模型**。

**数据模型改动（按用户口径）**：

- **状态是任务级的单一字段**（`TodoPayload.status`，四态 `todo / doing / overdue / done`），
  **手动选、不做截止时间** —— 所以删掉了 `due_ts`，也删掉了原来的第二层 `todo_done`
  （"每人各自一格完成"）：状态改成单值后它就是多余的第二份真相，且 `todo` 从未发布过，
  没有兼容包袱（`WIRE_KINDS` / `SILENT_KINDS` / `MsgKind` / 折叠 / 单测同步收敛）。
  合并规则：**LWW per `todo_id`，版本 `(seq, msg_id)`**，与前端 `newer()` 同规则。
- **拆成两个 kind，只为通知口径**：`todo`（Card）= **创建**（该计未读、该弹通知 —— 被指派的人
  得知道自己被派了活）；`todo_update`（Silent）= 改状态 / 改标题 / 换指派人 / 删除。
  两者**载荷同构、同属一条 LWW 序列**（折叠一视同仁）。若改动也用 Card，用户每拖一次状态
  全群就多一条未读 + 一条通知（实现时先按单 kind 做过，发现这条才拆开）。
- **指派人至少一人且必须是群成员**（用户明确"可以给一个或多个人"；指派人同时是改状态的鉴权依据，
  放进非成员会让这条任务对谁都改不了状态）。

**权限（前后端同一口径）**：

| 改动 | 允许谁 |
|---|---|
| 改状态 | 创建者 **或** 被指派人 |
| 改标题 / 换指派人 / 删除 | 创建者 **或** 群主 |

- 鉴权在**命令层**（`commands::may_update_todo` + `latest_todo_def` 从消息日志里取最新定义）；
  `creator` 由服务端回填，**不接受客户端自报**（否则传一个别人的 creator 就能改别人的任务）。
- 前端 `utils/todos.ts` 的 `canUpdateTodo` 是**显示用的镜像**（决定给不给按钮），
  两侧各有一条独立的授权矩阵用例表（`todo_update_permission_matrix` / 同名前端用例）。
- 一个命令 `update_group_todo` 覆盖"改状态 / 改标题与指派人 / 删除"：三者都是"重新发一份定义"，
  拆三个命令就是三份重复的构造与校验。

**界面**：

- 新增 `GroupTasksPanel.vue`：按状态分组（空组不占位，完成组标题带删除线）、
  每行显示标题 + 指派人（含「我」）+ 状态下拉（有权限时）+ 编辑/删除（有权限时）；
  新建与编辑走**面板内联表单**（标题 + 成员多选，不开第二层弹窗）；删除有二次确认。
- 入口两个：群聊头部 `ListChecks` 按钮（与"群成员 / 群文件 / 改名"同一排）、
  群成员面板里的「群任务」分区（三态计数 + 「查看全部 / 新建」）。
- `todo` 是 `Card` kind：**不进消息时间线**，只在这个面板里折叠展示（与群公告同一条口径）。
- 状态色走语义 token 且彩色文字用 `*-ink` 档（待办=中性 / 进行中=主题色 / 延期=危险 / 完成=成功）。

**验证**：`cargo test --lib` **480 全绿**（新增 `latest_todo_def_follows_the_same_lww_rule_as_the_frontend`、
`todo_update_permission_matrix`）；`npm test` 454 项（新增 `selfChat` + 重写的 `todos` 共 13 条用例、
`messageKinds` 新增状态表跨语言契约），仅剩 2 条与本轮无关的 zh-CN 环境既有失败；
`npx vue-tsc --noEmit` 零错误、`npm run build` ✅。真机两人互测仍需人工确认。

### Added (和自己聊天 —— 用户 2026-09-16)

**需求**：要一个能和自己聊天的功能（当备忘录用）。

**做法**：会话 id = **本机 device_id** 的本地会话，消息**只落本机**。

⚠️ **为什么必须是独立路径**（`commands::insert_self_message` 的文档里写了三条理由）：
`send_message` 要求"对方是好友 + 拿得到对方 X25519 公钥"，自己两条都不满足；即便绕过，
消息会进 outbox 而**永远等不到 Ack**（没有对端），那一行被每次心跳/建链的 `flush_outbox`
无限重发 —— 直接破坏"outbox 必然排空"这条不变量。所以自聊：**不进 outbox、不发 gossip、
不加密**（E2EE 保护传输；本地消息与其它消息一样是 SQLite 明文，不是"加密失败退明文"）。

- `send_message` 开头按 `friend_id == 自己` 分流 ⇒ **前端 `send()` 一行都没改**（乐观气泡 +
  返回记录替换的既有路径照用）。
- 状态直接给 `"read"`（不存在"在途"阶段）；前端**不给自聊消息挂回执**
  （否则会出现"绿勾已读"或永远转圈）。
- 自己的显示名/头像：`resolve_nickname` / `ensure_conversation` / `conversation_meta` 各加 self 分支，
  否则会话名会显示成一串 `gosslan-xxxx`。
- `mark_read` 对自聊**不发已读回执**（否则 `pending_reads` 里会永久堆积一条排不掉的记录）。
- 只支持**文本**（用户选的口径）：附件按钮在自聊里隐藏，粘贴图片/文件给明确提示，
  `send_file` 对自聊直接返回明确原因（不是那句对不上场景的"对方不是好友"）。
- 入口：会话列表「＋」菜单 →「和自己聊天」（用户选的"点过就有"）；列表行不显示在线状态点、
  头像不按离线灰掉（`isOnline` 对自聊返回 `null`）。

**验证**：`lib.rs` 新增护栏 `self_chat_stays_local`（函数体里**必须**是 `db::insert_message(`，
**不得**出现 `insert_message_and_outbox(` / `broadcast_gossip(` / `try_send(` / `crypto::seal(`），
并已通过 `scripts/verify-guards.py --only 自聊消息必须留在本地` 的**非空转**验证（改坏即 FAIL、
恢复即 PASS）；`utils/selfChat.ts` + 单测覆盖三处界面判据。手工验收（自己发 10 条 → 无未读角标、
无通知、无回执、重启仍在）仍需人工确认。

### Changed (输入框焦点态不再出现主题色方框 —— 用户 2026-09-16)

**用户症状**：整个应用的输入框一获得焦点就出现一个**主题色的方框**，很难看。

**根因**：全局焦点环把**文本输入类**也算进去了：

```css
:where(button, a, input, textarea, select, [tabindex], [contenteditable]):focus-visible {
  outline: 2px solid var(--gosslan-focus-ring);
}
```

浏览器对文本类控件一律把 `:focus-visible` 判成**"永远成立"**（点一下就成立，不需要键盘
Tab 遍历）—— 所以这不是"只有键盘用户才看到"的环，而是**每次点击输入框都会冒出来**的外圈
方框。消息输入框最难看的那个形态就是这么来的：它的矩形只是卡片里一块**透明的编辑区**
（`div[contenteditable]`，不是整张卡片），框出来像卡片内部浮着一个方框。
另外 `ProfileSection` 的昵称输入框自己还写了一个 `focus:ring-2 focus:ring-primary`，
那是**实心主题色方框**，同一个毛病的第二个来源。

**修法**：焦点提示按控件类型分开（观感沿用本项目既有的写法：`.gosslan-select:focus`
与各输入框的 `focus:border-[var(--gosslan-primary)]`，不引入新语言）。

| 控件 | 焦点提示 |
|---|---|
| `button` / `a` / `[tabindex]` | 键盘焦点环（`:focus-visible`，**保持原样**） |
| `input` / `textarea` / `select` | 边线变主题色（全局规则；本来没有边框的字段自己补 `border border-transparent`） |
| 消息输入框编辑区 | 焦点提示挂在**卡片边框**上（新增 `gosslan-composer` 钩子 + `:focus-within`） |

- 顺手补上了原本"只能靠那条外圈方框"才有点击焦点提示的字段（弹窗里的搜索/命名输入框、
  日志过滤框、主题自定义色块）：给它们常驻一个 `border border-transparent`，
  聚焦时由全局规则变成主题色边框；
- `ProfileSection` 的 `focus:ring-2 focus:ring-primary` → 常驻的 `border border-transparent`
  + `focus:border-[var(--gosslan-primary)]`（**不能**聚焦时才加边框：那会改尺寸、文字跳一下）；
- **没有**顺手去掉焦点提示本身：WCAG 2.4.7 要求可见焦点，`outline-none` 必须自带替代提示
  （既有护栏 `findOutlineNoneWithoutFocusRing` 仍在管）。

**验证**：新增护栏 `designGuards.checkTextFieldFocusRing` + 5 条用例（改坏即报、修好即过、
删掉替代提示要报、卡片钩子被摘掉要报、真实 `style.css` + `MessageComposer.vue` 通过）；
`npm test` 仅剩 2 条与本轮无关的既有失败（zh-CN 环境下 i18n 默认语言用例）、`npm run build` ✅。
⚠️ 观感只能真机确认：本机没有浏览器自动化（无 Playwright/Puppeteer），
`docs/design-guidelines.md` §2.4 已把规则写死。

### Fixed (群成员变更之类的系统提示显示英文 —— 用户 2026-09-16)

**用户症状**：界面是中文，但「谁加入了群聊」这类系统提示是**英文**。

**根因**：后端自己生成的文案由 `AppState::is_zh()` 选语言，而它的「跟随系统」分支只读
POSIX 环境变量（`LANG` / `LC_ALL` / `LC_MESSAGES`）——**Windows 上这些变量根本不存在**，
于是恒判为「不是中文」。默认设置就是「跟随系统」，所以中文 Windows 用户看到的
群成员变更 / 文件下载 / 托盘提示 / 窗口标题全变英文，而界面本身是中文。

（这条限制在 `state.rs` 里原本就被写成了注释，当时判断"影响有限，只有托盘提示与日志窗口
标题"；但系统消息也走这条判定，直接落在聊天内容里 ⇒ 影响并不有限。）

**修法**：让后端使用前端解析出的语言 —— 「跟随系统」的解析规则（`navigator.language`）
只在前端有一份，后端不该自己猜。优先级：
**显式偏好（`settings.language`）> 前端推来的解析结果 > 环境变量兜底**。

- `AppState` 新增内存字段 `ui_lang`（三态，不落库：它是解析结果的缓存，落库会多出一份
  与"偏好"不一致的状态）；`set_ui_language` 命令除了重建 macOS 菜单栏，也记下这个结果；
- 前端 `app.init()` 里**无条件**推一次解析后的语言 —— 这一条是关键：此前只有
  `if (has("language"))` 那条分支会推，而**从未改过语言**的用户库里根本没有这个键，
  于是后端永远收不到；
- ⚠️ 残留：一份群提示仍然带着**发送方**的语言 —— 「谁加入了群聊」是唯一一条经消息管道
  广播（而不是各端本地生成）的群内提示，所以群里其他人看到的是发起人界面语言的文案。
  （踢人 / 退群 / 群主转让三条都是各端本地生成，天然跟随各自语言。）彻底解决需要把这条
  改成「结构化载荷 + 各端自行渲染」，本轮未做。

**验证**：新增 `state::tests::ui_language_prefers_preference_then_frontend_hint_then_env`
（钉住三级优先级与脏值行为）；新增前端接线护栏
`i18n/index.test.ts`「app.init() 必须把解析后的语言推给后端」——它先改坏再修好过一遍，
确认非空转。

### Fixed (NSIS 安装包图标是默认的 —— 用户 2026-09-16)

**用户症状**：构建出来的安装包（setup.exe）图标是默认的，不是应用图标。
（应用本身的 exe 图标是正常的 —— 已验证六个尺寸帧都嵌在 `target/debug/gosslan.exe` 里。）

**根因**：`tauri.conf.json` 的 `bundle.windows.nsis` 里**没有 `installerIcon`**。
Tauri 的 NSIS 模板只在 `installerIcon` 非空时才 `!define MUI_ICON`，**没有兜底分支、
也不会回退到应用图标**（`crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi`
里就是 `!if "${INSTALLERICON}" != ""`）⇒ 留空就等于用 NSIS/MUI2 自带的默认安装程序图标。

**修法**：补上 `installerIcon` 与 `uninstallerIcon`（都指向 `icons/icon.ico`）——
同一份 `tauri icon` 产物，安装程序图标与卸载项图标都跟着应用图标走。

**验证**：图标文件本身无需改动（结构已核对：6 帧 PNG-compressed，是 `tauri icon` 的标准
输出）。⚠️ **安装包图标无法在本机验证**（没装 NSIS 工具链，需整包 release 构建），
请在下次打包后确认 setup.exe 与「应用和功能」里的图标。若仍是默认图标，`tauri build`
会明确报 `failed to resolve ... installerIcon`（路径解析失败），据此可判断是路径问题。

### Fixed (设置 / 日志窗口不居中、而且比主窗口还大 —— 用户 2026-09-16)

**用户症状**：打开设置、个人资料、日志这些新窗口时**没有居中**，而且窗口
**比主窗口还大很多**。

**根因**：两个独立窗口的尺寸在 Rust 里写死（设置 780×600、日志 760×560），
位置完全交给系统默认 —— 既没有 `center()`，也没有跟主窗口的任何关系。
主窗口是**可缩放**的（默认 1000×680），用户把它拉小之后这两条同时命中：
子窗口比它大，而且落在屏幕默认位置、离它该贴着的那个窗口很远。

**修法**：新增 `commands::aux_window_geometry`，按**主窗口**算几何：

- 尺寸：装得下就用设计尺寸，装不下按主窗口缩到「两侧各留 24px」；
- 位置：在主窗口**外框**内居中（主窗口是无边框自绘标题栏，外框≈内尺寸；子窗口带系统标题栏，
  所以位置在尺寸确定之后按**子窗口自己的真实外框**算，避免差半个标题栏）；
- 最小尺寸跟着收敛（不能大于实际尺寸，否则系统会把窗口顶回最小值，"缩小"等于白做）；
- 拿不到主窗口（还没建出来）时退回设计尺寸 + 系统默认摆位。

**⚠️ 同一轮的多屏修复（用户 2026-09-16 追加反馈「多屏时子窗口弹到另一个屏幕上、位置也没居中」）**

第一版把几何交给了 builder 的 `.position(x, y)` / `.inner_size(w, h)` —— **这两个 API 只收逻辑坐标**，
而 `tao` 创建窗口时会把逻辑坐标**逐个显示器**地按该显示器自己的缩放换回物理，取第一个"换算结果
落在自己范围内"的显示器；一个都没命中就退回 `CW_USEDEFAULT`（主屏层叠位置）
（`tao/src/platform_impl/windows/window.rs`）：

- 主窗口在 150% 的副屏、另一块屏 100% 时：按副屏缩放算出的逻辑坐标，再按 100% 换回来，
  正好落进那块屏 ⇒ **子窗口跑到另一块屏幕上**；
- 尺寸走同一条换算（按"选中显示器"的缩放）⇒ 大小同样不对。

改成**全程物理像素**：

- 窗口以 `.visible(false)` 创建，`build()` 之后用 `set_min_size` / `set_size` / `set_position`
  下发**物理**值，再交给 `ensure_aux_window` 的 `show()` —— 用户看不到中间态；
- 常驻窗口每次**重新打开**时重新居中（只动位置、不动尺寸，留住用户自己拉过的大小）：
  主窗口被拖到另一块屏之后，留在原地就等于"又开在另一块屏幕上"。

护栏：`aux_window_open_is_singleton_serialized_and_resident` 现在同时断言
"必须 `apply_aux_geometry` + `visible(false)`"、"**不得**出现 `.position(`"（这条缺陷在单屏上完全看不出来）。

⚠️ 尺寸/位置只在**创建**时算一次（重开只重新居中）：用户自己挪过/拉过的窗口不会被反复重置。

**验证**：`cargo test --lib` **475 项全绿** —— 几何不变式覆盖 7 档主窗口（含 125% / 150% 缩放）
× 4 个主窗口原点（含 1920 起的副屏与**负坐标**的左侧副屏）× 两组真实窗口参数，
断言"永不大于主窗口 / 最小尺寸不反超 / 按真实外框居中"；并断言 150% 屏上
780×600 逻辑 = 1170×900 物理、最小尺寸同样换算。真机多屏外观仍需人工确认。

### Fixed (撤回 / 文件被下载 / 群主变更在时间线上仍像一条普通消息 —— 用户 2026-09-16)

**用户症状**：消息撤回、文件被下载、群主转移这类通知**直接按普通文本消息那样提示**，
希望改成微信那样——只是居中一行提示。

**根因**：三处各有一半问题。

1. 渲染：居中灰字**已经存在**，但它被放在"头像行"**内部** —— 于是系统消息照样带
   36×36 头像，还被 `max-w-[72%]` 的列宽挤在一侧、居中后仍偏，看着就是一条普通消息。
2. 高度估算：`messageHeight` 只给 `system` 记了 28px，`recalled` 落到 default 按空文本
   气泡估 35px，且**照样加 18px 的昵称行** —— 群聊里一条"对方撤回"会把它下面那条推偏。
3. 群主转让**根本没有系统消息**：成员表里的「群主」标记悄悄换人，群里一声不响
   （而加人 / 踢人 / 退群三种成员变更都是有提示的）。

**修法**：

- 抽出 `messageKinds.TIP_KINDS` / `isTipKind` 作为提示行的**唯一判定点**；
  `MessageItem` 把它移出头像行、做成通栏居中小灰字（无头像、无气泡、无昵称、无菜单），
  `messageHeight` 按同一份清单估高（`recalled` → 28px，提示行不计昵称行）；
- `transfer_group_creator`（发起方）与 `handle_group_creator_changed`（接收方）各补一条
  群内系统消息，文案走 `is_zh()` 双语，与既有三条成员变更同口径。

**验证**：新增 `messageKinds.test.ts` 的「提示行判定只有一个来源」——直接读
`MessageItem.vue` / `messageHeight.ts` 源码，禁止再自己写 `kind === "system"`
（那正是会漏掉 `recalled` 的写法）。`npm test`、`npm run build` 全绿。

### Fixed (点系统通知没反应，不能定位到会话 —— 用户 2026-09-16)

**用户症状**：消息到系统通知了，但**点通知没反应**，无法定位到会话。

**根因**：桌面端的 `notify-rust` handle 被立刻丢掉：

```rust
notification.show().map(|_| ())   // handle 是点击响应的唯一通道，丢在这里
```

于是点击永远送不进进程。同时前端 `onAction` 监听的是**插件**的
`plugin:notification:actionPerformed` —— 那条事件只有移动端会发，桌面端从头到尾没有
点击来源。另外 `notify_desktop` 命令**没有带 `conv_id`**，即便拿到点击也无从定位。

**修法**：

- 新增 `notifications::show_click_if_enabled`：Windows 上把 handle 留在**独立线程**等
  `wait_for_response`，命中 `Default`（点正文）才回调（`Closed(..)` 是超时/被划掉，
  不该抢焦点）；点中后 `tray::show_main_window` + 广播 `notification-clicked`；
- 载荷 `{type, conv_id}` 与移动端插件通知的 `extra` **同形**，前端因此只有一条路由
  （`routeNotificationClick`），桌面事件与移动端 `actionPerformed` 共用；
- `notify_desktop` 增加 `conv_id`；好友申请通知（Rust 直发的那两条路径）也接上同一条路，
  点击跳到「新的朋友」；
- macOS / Linux 行为**与改动前完全一致**（点击不由 handle 送出，`on_click` 被丢弃）——
  macOS 的 `notify-rust` 实现是"handle drop 时才发送"，改成阻塞式发送会影响"通知能不能
  弹出"这件更基本的事，在无法真机验证前不动它。

**验证**：`npm test`（事件契约守卫要求"前端监听的事件必须真有人发"，
`notification-clicked` 两端都在，已通过）、`cargo check --lib`、`cargo test --lib`。
⚠️ **Windows 的真机点击链路未验证**：toast 的激活事件由 winrt 的进程内事件送达，
而 `notify-rust` 只保留 handle（不保留 `ToastNotification` 对象），是否稳定送达需要在
已安装的构建上实测；若实测点不动，日志里会有"已发送系统通知"但没有后续跳转，届时再评估
（退路是单实例 + AUMID 激活，或 COM 通知激活器）。

## [4.18.10] - 2026-09-16

### Fixed (链接复制保真：省略的应当只是界面，不是数据 —— 用户 2026-09-16)

**用户症状**：消息里的长链接在气泡里做中间省略，点击能正常打开，
但**选中复制拿到的是残缺 URL**（`https://very-long-domai…`）；应当复制原始数据。

**根因**：`displayUrl()` 截断后的字符串被**直接当成 DOM 文本渲染**
（`MessageTextBubble` / `MessageContentModal`），而**选中复制取的就是选区文本**。
右键菜单与复制按钮走的是 `message.content` 原文，所以只有"划选复制"这一条路是坏的
—— 这正是一开始没被发现的原因。

**修法**：DOM 里保留完整 URL 的字符序列，视觉省略只由 CSS 表达。

- `displayUrl()` → `splitUrl()`，切出 `head / mid / tail`，契约 `head+mid+tail === 原 URL`；
- 新增 `MessageLinkText.vue`，三段 DOM 顺序固定 `head → mid → 省略号 → tail`
  （选区按 **DOM 顺序**拼接，mid 必须夹在中间才能还原）；
- `mid` 用 `font-size: 0` 隐藏。⚠️ **不能**换成 `display:none` / `visibility:hidden` /
  `user-select:none` —— 那三种会把文字踢出选区，缺陷原样复现；
- 省略号是**纯 CSS 画的三个点、不含任何文本节点**（写成 `"…"` 会被一起复制进 URL）；
- `.gosslan-url-dots` 的几何按真实 `…` 量出来（14px 下宽 11.8px、点距 0.28em），
  全部用 em 表达，随 `--gosslan-msg-size` 缩放。

**验证**：Chromium 实测全选链接 → `Selection.toString()` 精确等于原始 URL（无 `…`、无空格）；
全选整条消息 → 完整原文；`mid` 计算宽度 0、字号 0px；省略号元素子节点数 0。
14/16/20px 三档下与真实 `…` 逐行比对，点距/基线/下划线连续性都对上。

### Fixed (链接切分：中文句读吞掉后半句 + 带括号的 URL 被从中间切断)

两条同属正文链接渲染，一起修的：

| 输入 | 修改前 | 修改后 |
|---|---|---|
| `看 https://a.com。然后呢` | 整句成链接，点开必然失败 | `https://a.com` + 文本 `。然后呢` |
| `https://zh.wikipedia.org/wiki/Foo_(bar)` | 切成 `...Foo_` + 文本 `(bar)`，点开 404 | 完整链接 |

- **中文句读**：`URL_RE` 的排除集原先只有 ASCII，`TRAILING_PUNCT` 也只有 ASCII，
  于是「。然后呢」整段被吃进 URL。现在中文句读**同时**出现在排除集与尾标点集里
  （只做后一半不够 —— 正则已经先把它吃进去了）。汉字仍允许出现在 URL 中
  （中文域名/路径是合法 URL，排除它们会把链接切断）。
- **括号**：`(` `)` 原先被直接排除出 URL 字符集。现在允许进 URL，改为**按是否配对**
  决定归属：`Foo_(bar)` 里的 `)` 配对 ⇒ 属于 URL；`（见 https://a.com/x)` 里多出来的
  `)` ⇒ 当句末标点剥掉。

### Fixed (聊天其余收口：@提及边界、引用消息、选区、搜索高亮)

- **@提及：输入框插入的 @ 没有前导边界** —— 中文里「你好@张三」不敲空格是常态，
  触发端刻意不设限，但插入端也没补边界，于是**发送端看到蓝色 chip、接收端既不通知也不高亮**
  （静默失效）。现在 `applyMention` 在 `@` 前不是边界时补一个 nbsp，
  与检测端共用同一个 `MENTION_BEFORE`。
- **@提及：表情紧邻时高亮与通知分叉** —— 气泡把正文按表情 token 切段后**逐段**跑 linkify，
  文本段段首天然命中 `^`，而检测端对完整正文跑正则。`MENTION_BEFORE` 现在把
  表情 token 的收尾 `]` 也算作边界，两边不可能再分叉。
- **「选择文字」模式下点链接会直接拉起系统浏览器** —— 那一模式下手指落在正文上是在挪选区。
  现在该模式下链接点击不生效（这一下点击会把选区收起来、自动退出选择模式，再点即正常打开）。
- **引用消息：同一个气泡两条复制路径结果不一致** —— 「选择文字」全选复制只拿到正文，
  而操作条的「复制」给的是带引用头的完整原文。全选范围改挂到「引用块 + 正文」的容器上；
  引用块本身是 `<button>`（会吃到全局 `user-select: none`），显式放开可选性。
  触屏下仍与正文**同进退**（默认不可选，长按归消息菜单），否则在引用块上长按会弹不出菜单。
- **引用消息：正文正好 5 行时凭空多出「展开」条** —— 截断判定吃的是含引用头的完整 content，
  而真正被 clamp 的只有正文。`textNeedsClamp` / `textBubbleHeight` 现在只看正文，
  引用头按它自己的排版（12px / 行高 16 / py-1 / mb-1.5 = 30px）单独计。
  解析逻辑收敛到新增的 `utils/quote.ts`（气泡、截断、复制三处共用，原先各写一份）。
- **引用消息：复制的内容里混进内部 `|msg_id`** —— 用户看到的是「引用 张三：片段」，
  复制出来却是 `…片段|msg_ab12」`。现在只在**复制**路径剥掉；转发原样保留
  （接收方靠它跳转到被引用的那条消息）。
- **`selectionchange` 监听泄漏** —— `MessageItem` 把它挂在 `document` 上，
  而 `onBeforeUnmount` 只摘了 window 上的两条。选择模式下滚出虚拟列表就永久留一个，
  之后在输入框/搜索框选字都会白跑 N 次 `getSelection()`。
- **搜索高亮撞上 HTML 实体** —— 旧实现先 `escHtml` 再在转义结果上替换，关键词撞上
  `amp`/`lt`/`gt`/`quot` 就会命中实体内部：搜 `amp` 会把 `&amp;` 渲染成字面的 `&amp;`。
  改为**先在原文上切片、再逐段转义**。

**新增两条护栏**（`designGuards` 的选区契约 ⑨⑩）：链接省略号的三段顺序/空白/无文本，
以及引用块可选性的两个半边（桌面放开 / 触屏默认收回）必须成对存在。
这两类改坏了都不报错、只会静默复制出错，必须由机器盯住。

### Fixed (BLE 可诊断性：外设启动失败的原因被丢掉 + 「开始连接」谎报 + 重复 `-conn`)

纯蓝牙实测一轮后按日志逐条核对出来的，三条都属于"功能没坏、但日志把人带偏"。
另附一条**从 4.18.9 起就红着的测试**。

**① 安卓「外设角色不可用」的真正原因被代码丢掉了（最要紧的一条）**

日志里只有一句自指的：
`蓝牙外设角色不可用（central 角色不受影响）：Android BLE 外设未能启动（详见日志中的具体原因）`
—— 而日志里并没有那个"具体原因"。这是一条三方接力、最后一棒掉了：

1. Kotlin 侧每种失败都明确上报了原因（`BlePeripheral.kt`：权限缺失 / 蓝牙没开 /
   本机不支持广播 / GATT server 打不开 …）→ `nativeOnWarning` → 进 `EVENTS` 队列；
2. 这条队列**唯一**的消费点是外设接收循环，而**启动失败时那个循环根本不会起来**；
3. `start_peripheral` 的失败分支只打那句自指文案，紧接着 `server.stop()` 把队列连同
   接收端一起丢掉 —— 原因已经躺在队列里，没人读就被扔了。

旁证：`start()` 里在调用 Kotlin 之前发的「Android BLE 外设桥已就绪」同样一条都没出现。
现在失败路径会先排空队列再停服务，且 `stop()` **前后各排一次**
（后一次是必要的：`stop()` 自身失败时会往同一条队列塞 Warning，那条以前也没人读过）。
影响面：用户无法判断"这台安卓当不了外设"是本机不支持、权限没给、还是代码 bug。

**② `候选可拨 ⇒ 开始连接` 之后什么都没发生，日志在谎报**

会话建好之后，扫描仍每轮打一句「候选可拨 … ⇒ 开始连接（GATT central）」——
真机日志里连着 18 轮，后面没有任何后续行。根因是 `dial_and_register` 里两条跳过路径
只有一条有日志：`DialGuard` 失败会打「已有在途拨号」，而
`has_endpoint_addr` 为真时**静默 `return Ok(())`**（隔壁注释明明写着"跳过"）。

修法：把「已建链」判定**提到打「开始连接」之前**，日志换成同频次的
「跳过候选 id=… 原因=已建链（不重复拨号）」—— 一换一，**日志量不增反降**，
还顺带省掉了那次读广播属性的平台调用；`dial_and_register` 里保留为竞态兜底并补了日志。

⚠️ 同一处还补回了一件事：原先那条静默跳过是走 `Ok(())` 分支、会**顺带
`clear_ble_dial_failure`**，提前 `continue` 后这个清除会丢 —— 后果是"已建链"的地址
留着一条过期退避，链路断掉要重拨时被拖住（最长 20s）。已在新分支里显式补上。

**③ 同一条链路被打了两条 `-conn … conns=0`**

`unregister_connection` 无条件记日志，而 `MeshManager::remove_connection` 是**有返回值**的
（注释就写着"返回是否真的移除"），返回值被丢掉了。同一条链路有两条拆除路径
（BLE 侧的 `teardown_link` 与传输层的统一拆除）都会走到这里，于是打出两条一模一样的行，
读起来像"同时断了两条链路"。现在没真移除就直接返回。

**④ 顺带：Rust 测试从 4.18.9 起就是红的**

`payload_mtu_handles_normal_and_bogus_values` 断言 `payload_mtu(517) == 514`，
而 4.18.9 那次「封顶到 AOSP 上限 512」改了 `att_payload_budget` 却没同步改这条断言
—— **上一个 release 是在测试红着的情况下发出去的**。已更新为 512，并把这条边界钉成契约
（515/514/513 三档都覆盖）。
注意：`ble.rs` 整个模块在 `feature = "bluetooth"` 后面，**默认 `cargo check` / `cargo test`
根本不编译 BLE 代码** —— 这大概就是它能一路漏过去的原因。

**验证**：`cargo test --features bluetooth --lib` 497 passed / 0 failed（修前 496/1）；
`cargo check --features bluetooth --all-targets` 无 error 无 warning。

## [4.18.9] - 2026-09-16

### Fixed (BLE 每片 514 字节 > AOSP 硬上限 512 ⇒ 多分片帧永远发不出去)

**查证过程（按用户要求：不猜，先查源码再改）**。

前两次都是猜的，第二次方向对但**幅度不够**，而且**改错了地方**（只改了日志变量，
`-3` 从未作用于真正的分片）。这次直接读 AOSP 源码
`android-35/.../bluetooth/BluetoothGatt.java`：

```java
private static final int GATT_MAX_ATTR_LEN = 512;      // L101
public int writeCharacteristic(BluetoothGattCharacteristic c, byte[] value, int writeType) {
    if (value.length > GATT_MAX_ATTR_LEN) {            // L1562
        throw new IllegalArgumentException(
            "value should not be longer than max length of an attribute value");
```

**这个上限是硬编码常量，与协商 MTU 完全无关。**

而本项目的分片预算是 `att_payload_budget(peripheral.mtu())` = `mtu - 3`。
**btleplug 在 Android 上 `Peripheral::mtu()` 返回的是请求值 517**（不是协商结果）
⇒ `517 - 3 = 514 > 512` ⇒ 每片 514 字节**必被框架抛异常**。

对照真机日志，症状完全吻合：

| 帧 | 分片 | 每片字节 | 对比 512 | 结果 |
|---|---|---|---|---|
| 聊天 272B | 1 片 | 272 | ≤ 512 | ✅ 正常 |
| 好友申请 738B | 2 片 | **514** | **> 512** | ❌ 永远失败 → 重试 4 次 → 拆链重连 |

**修法**：在**唯一的换算点** `att_payload_budget()` 里封顶到 512
（外设侧原本就已有 `(1..=512)` 的封顶，所以 Mac 侧一直正常 —— 这也解释了为什么
只有安卓→Mac 方向失败）。

**同时撤掉上一版加在日志变量上的 `-3`** —— 它只让日志显示 511、分片实际仍是 514，
属于"让日志说谎"的改动。

护栏：`att_payload_budget(517) == 512`、`(1024) == 512`、`(515) == 512`
（既有断言 23→20 / 185→182 / 4→1 均低于上限，不受影响）。

验证：cargo test --lib 480 passed · scripts/e2e-dev.sh 30 passed / 0 failed。

## [4.18.8] - 2026-09-16

### Fixed (BLE 写入失败日志补上帧长 —— 上一版修复生效但不够，先让它可精确诊断)

**4.18.7 的 `payload_mtu() - 3` 已确认生效**：真机日志里两侧的分片预算从 514/512 降到
**511/509**。**但写入仍然失败**：安卓（central）侧 `FriendRequest`（738B / 2 片）依然报
`value should not be longer than max length of an attribute value`。

也就是说 `MTU - 3` 仍不是真正的上限。而现有日志只写「写失败」+ 错误串，**看不出
"到底写了多少字节"**，无法判断是分片仍偏大、还是别的原因（例如对端特征的声明长度）。

本次只做一件事：**在写失败日志里补上帧长**（`帧长={ bytes.len() }`）。纯诊断、零行为改动。
下一次真机日志就能直接给出「写了多少字节失败」，从而精确定位差多少 ——
而不是再猜一次数字。

**为什么不再猜一次**：本次 BLE 问题我已经猜错两次（先怀疑广播地址、再怀疑分片尺寸），
第二次虽然方向对（确实少了 ATT 头）但幅度不够。继续盲调数字既不可靠，
也可能把已经能用的单分片路径弄坏 —— 先把度量补上，再按数据改。

验证：cargo test --lib 480 passed。

## [4.18.7] - 2026-09-16

### Fixed (BLE 分片预算没减 ATT 头 ⇒ 多分片帧写不出去，"好友申请永远发不出")

**用户真机症状**：蓝牙连着、加好友对方也同意了，但发起方列表里始终没有对方；
打开局域网后**立刻**就加上了；随后关掉局域网，蓝牙**又能聊天**（只是慢）。

**日志给出了决定性对比**：

| 帧 | 大小 | 分片 | 结果 |
|---|---|---|---|
| `FriendRequest` / `FriendAccept` | 738 B | **2 片** | ❌ 发不出去 |
| `chat_message` | 272–284 B | **1 片** | ✅ 正常 |

「关掉局域网后蓝牙还能聊」正是这条对比的另一半：**不是蓝牙不能聊，是超过一片的帧写不进去**。

**根因**：`mtu_budget = writer.payload_mtu()`。btleplug 在 Android 上返回的是
**协商到的 ATT MTU 本身**（日志里 514），而 GATT 单次写入的真实上限是 `MTU - 3`
（3 字节 ATT 头 = 511）。代码自己的注释早就写着「每片有效载荷 = MTU-3-6」，
**但实现里没减那 3**。

于是每片写 514B > 511B 上限 → `value should not be longer than max length of an
attribute value` → 重试 4 次 → 拆链重连（这正是那个反复拆链循环的来源）。
单分片帧只有 278B，**远低于上限，所以一直正常** —— 掩盖了问题，直到有超过一片的帧。

**修法**：`payload_mtu().saturating_sub(3)`（central 与外设侧两处，同口径）。
macOS 上 `payload_mtu()` 已是正确值，再减 3 只会让分片略小（无害）。

**这一条同时解释了之前所有症状**：好友申请发不出、单方面成功、反复拆链重连、
蓝牙下"能聊但慢"（慢是因为大帧全部重试失败）。

验证：cargo test --lib 480 passed · scripts/e2e-dev.sh 30 passed / 0 failed。

## [4.18.6] - 2026-09-16

### Fixed (好友申请无限重发 —— 登记只在「收到回执」时解除，而回执可能永远送不到)

真机日志（09:28 那次）显示 Mac 侧每 5 秒成对出现：

```
补发好友申请 peer=...（此前链路抖动丢过）   ×2
```

而**全程没有 `收到跨跳好友同意`** —— 同意回执本身是**没有 ACK 的定向帧**，链路抖动时
静默丢失，于是那条「我方已发出、等对方确认」的登记**永不解除**，链路每建立一次就重发一次。

**修法**：补发前先看「**本地好友表里有没有他**」——只要已经是好友，这条待发申请就失去意义，
直接清掉登记并留一行 `已是好友，停止补发申请`。

判据刻意用「本地好友表」而不是「有没有收到那个回执」：无论友谊是通过哪条路径建立的
（对方同意 / 我方同意 / 自动同意），只要已经是好友，就不该再重发。

**这没有从根本上补上缺失的 ACK** —— `FriendRequest` / `FriendAccept` 这对控制消息
至今没有任何回执机制（普通消息、群消息、群文件都有）。真正的解法是照 `ChatAck` 加一条
定向 ACK，让发送方在收到回执时才删待发记录。那是协议层改动，需要单独一轮做并充分验证；
本轮先用一个**不需要改协议、也不可能把已稳定的连接搞坏**的判据把无限循环掐掉。

验证：cargo test --lib 480 passed · scripts/e2e-dev.sh 30 passed / 0 failed。

## [4.18.5] - 2026-09-16

### Fixed (子网广播的成功在日志里不可见 + 预期失败每轮刷告警)

真机日志复核发现：**4.18.2 的子网广播修复其实是生效的**（两端都建起了 `path=lan`
链路并互相收到 announce），但**日志无法证明这一点** ——

```rust
diag_event_from_send_result(&directed, "broadcast_sent", &res);
```

`broadcast_sent` 恰好在 `push_diag_event` 的 `DROPPED` 名单里（高频心跳类），
**成功时什么都不打**，只有失败才留痕。于是「子网广播到底发出去没有」这条
排查"局域网只通一半"最需要的证据，在日志里完全不可见。

- 子网广播改用**独立事件名 `bc_directed`**，并在 `DROPPED` 判定里单独放行其成功 ——
  它能区分「发不出去」与「发出去了但对方没收到」。
- **子网广播成功时不再为 limited / multicast 的失败刷告警**：macOS 上它们本就发不出去
  （socket 绑具体网卡 IP 时 EHOSTUNREACH），而已有一条能用的路径 ——
  原先每 5 秒两条 WARN 持续数分钟，把真正有用的信息淹掉了。
  只有**三条路全失败**时才留痕：那才是"本机在局域网上发不出声"的真信号。

### 复核确认已生效的两处（真机日志）

- **好友同意去重（4.18.2）**：`收到跨跳好友同意` 只在首次出现并发一次通知，
  之后全部记为 `重复的好友同意（已忽略）`；`补发好友同意回执` 到第 3 次后
  `窗口/次数用尽` 自动停止（有界，设计如此）。
- **局域网连通**：两端均有 `建链 path=lan` + `conv=… path=lan hop=0` + `announce_verified`。

验证：cargo test --lib 480 passed · scripts/e2e-dev.sh 30 passed / 0 failed。

**仍未修（真机上仍可见）**：安卓 → Mac 的 BLE 写入失败
（`value should not be longer than max length of an attribute value`，738B 帧）导致
中央侧反复拆链重连；局域网已连通所以不影响使用，但持续耗电与刷日志。

## [4.18.4] - 2026-09-16

### Fixed (置顶条不刷新 + 无法就地取消 + 多条的样式边界)

**① 置顶后界面不刷新，必须重进会话**（用户真机反馈）。

根因在**命令层**：`pin_group_message` 返回 `()`，把置顶事件记录丢掉了 ——
而置顶条的呈现是 `foldPinned(该会话全部消息)` 折叠出来的，**事件不进前端 store，
折叠就看不到它**。前端即使想 enqueue 也无从拿到（我最初写的 `void rec` 正源于此，
那是症状不是原因）。

已改为与 `send_group_reaction` 同口径：返回 `MessageRecord`，前端 `enqueueMessage`。

**② 取消置顶必须先跳到原消息再右键** → 置顶条上每条加一个 **✕ 就地取消**
（悬停显示，触屏由 `hover-reveal-op` 常显兜底）。

**③ 多条置顶的样式边界**（用户明确要求"要有一个边界"）：
- **条数上限按端给**：移动端一行只放 **1 条**（放三个的话每个都被压成省略号，
  等于三个都读不出来）；桌面最多 3 条。
- 超出部分用 `+N` **就地展开成纵向列表**（`max-h-32` + 滚动），
  而不是继续往同一行里挤 —— 否则置顶一多，置顶条自己就把消息区吃掉了。
- 切换会话时自动收起展开态，不把上一次的状态带过来。
- ✕ 的 `title` 挂在**带 `truncate` 的那个元素**上（`designGuards` 的
  「截断文本必须有 title」护栏当场抓到我把 title 放到了外层按钮上）。

验证：npm test 424 passed · npm run build 通过 · cargo test --lib 481 passed ·
scripts/e2e-dev.sh 30 passed / 0 failed。

## [4.18.3] - 2026-09-16

### Fixed (撤回入口对所有人可见 —— `ComputedRef` 当真值用)

**用户真机反馈**：群聊里右键**别人的**消息，菜单里也有「撤回」。

根因是我 4.13.0 写的这一行：

```js
const canRecall = computed(() => !!props.isGroup && mine && props.message.kind !== "recalled");
```

`mine` 从 `useMessageDisplay` 解构而来，是 **`ComputedRef<boolean>`**（本文件其它三处都写
`mine.value`）。在**模板**里 Vue 会自动解包，但在 **script 的 `computed` 内部不会** ——
裸写 `mine` 是个对象，**恒为真值**。于是 `canRecall` 退化成了「是群聊 && 未撤回」，
对任何人的消息都显示撤回入口。

已改为 `mine.value`。

**顺带修掉同一处的第二个实例**：移动端长按面板的撤回项用的是内联条件
`v-if="mine && message.kind !== 'recalled'"` —— 漏了 `isGroup`，导致**单聊长按也显示撤回**，
而后端只实现了群撤回 ⇒ 点了确认后什么都不发生（静默 return）。
现在两个入口共用同一个 `canRecall`，不会再各自漂移。

### Changed (表情回应条：尺寸与对齐)

- **对齐**：回应条是消息行的**兄弟节点**，默认会从「头像」那一列起排，
  看起来像挂在头像下面而不是气泡下面。左右各让出「头像 40px + 行间距 8px」= 48px，
  与气泡对齐（纯排版补偿，不改行为）。
- **尺寸**：chip 高度 24px → **28px**（达到可点面积），表情 14px → 16px，
  计数加大并加粗；快捷表情按钮同步放大到 28px，与 chip 同高。
- 自己的回应条靠右（与气泡朝向一致），并补了 hover 文字色与阴影，层次更清楚。

验证：npm test 424 passed · npm run build 通过 · cargo test --lib 481 passed。

## [4.18.2] - 2026-09-16

### Fixed (好友同意重复通知 + 手动模式下无子网广播 —— 真机日志定位)

**① 好友同意被反复处理，每次都弹系统通知**（用户可见刷屏）。

真机日志里同一秒内出现三次：
```
收到跨跳好友同意 peer=...   已发送系统通知：好友申请已通过   ×3
```

根因是**两个 bug 相乘**：
- **接收侧**：`GossipKind::FriendAccept` 的处理**没有任何去重**。`add_friend` 是幂等的，
  但**通知与留痕每次都执行**。
- **发送侧**：`FriendAccept` 没有 ACK 机制，发送方只能靠"链路建立时重发"来保证送达
  （`补发好友同意回执`）—— 而真机上蓝牙每 2 秒断一次重连一次，于是每 2 秒重发一次。

它同时是「单方面成功」观感的来源：一方在无限重发，另一方被反复打扰。

**修法**：以「这一次是否真的从**不是好友**变成好友」为幂等判据 ——
只有首次才通知与留痕，重复投递只记一行便于排查的日志。
`emit("friend-accepted")` **仍然每次都发**：前端 store 只是据此重拉好友列表（幂等），
而漏发会让「首次那个 emit 恰好没被界面收到」时界面永远不刷新。

**② 手动选网卡时不算子网广播 ⇒ 局域网"只通一半"**。

上一版广播修复（4.18.1）在真机日志里**没有生效** —— 日志里从未出现
`target=192.168.31.255`。原因在 `resolve_bind_ip`：只有 auto 模式会调
`find_lan_interface` 拿广播地址，**手动模式直接返回 `None`**：

```rust
} else { Ok((ip, None)) }   // ← broadcast 丢了
```

于是只能发 limited broadcast（macOS 上 socket 绑定具体网卡 IP 时会 EHOSTUNREACH）。
**表现是"我收得到别人，别人找不到我"** —— 任一端发不出广播，对端就只能退回蓝牙。
已补 `broadcast_for_ip()`：按选中的本机 IP 找到对应网卡的子网广播地址。

验证：cargo test --lib 480 passed · scripts/e2e-dev.sh 30 passed / 0 failed。

**未修（记录在案）**：`FriendAccept` 没有 ACK，发送方只能靠链路建立时重发，
在蓝牙频繁重连时会持续重发（现在不会再打扰用户，但仍是无效流量）。

## [4.18.1] - 2026-09-16

### Fixed (局域网广播从未真正发出 —— 真机「只能走蓝牙 / 加不上好友」的上游根因)

**用户真机日志显示**：Mac 上每一轮广播都是
`broadcast_error: target=255.255.255.255:59991, error=No route to host (os error 65)`
—— Mac **从不在局域网上出现**，于是两端只能靠蓝牙发现对方，全部流量挤在那条通道上。

**根因**：`find_lan_interface()` 会算出**子网广播地址**（如 `192.168.31.255`），一路传进
`broadcast()` —— 然后被丢掉。参数名是 `_lan_broadcast`（下划线 = 刻意未使用），
函数体内永远只用 `255.255.255.255`。而该函数上方的注释白纸黑字写着
`broadcast_addr`「用于将 UDP 广播发到精确子网地址……确保广播不会因默认路由进入
VPN 适配器」—— 这个能力建好了，**但从未接上**。

在 macOS 上，socket 绑定到具体网卡 IP 时向 `255.255.255.255` 发送会返回
`EHOSTUNREACH`；精确子网地址才是能用的那条路。

**修法：两种广播都发**（不是替换）—— 各自覆盖对方的短板：
- `255.255.255.255`：Windows 默认禁用 directed broadcast，只有它能穿透；
- 子网广播：macOS 上真正可用的就是它。

接收端按 `device_id` + 消息去重，多收到一份是幂等的。`who_has` 探测（「打开添加好友」
时触发）同样补上 —— 否则 macOS 上打开添加好友照样发现不了对方。

**同时修正一句此前的判断**：我曾说「通道优先级未被触碰，所以连接没问题」——
那条只覆盖了**排序逻辑**，没覆盖**某一通道本身能不能用**。这次的根因恰好是后者。

验证：cargo test --lib 480 passed · scripts/e2e-dev.sh 30 passed / 0 failed。
（沙箱无真实局域网 ⇒ `lan_broadcast = None`，只走 limited broadcast，与预期一致；
用户 Mac 的 `send_bind=192.168.31.113` 来自 `find_lan_interface`，自动模式下
`lan_broadcast` 必为 `Some(192.168.31.255)`，修复会真正生效。）

## [4.18.0] - 2026-09-16

### Changed (日志：补齐连接生命周期，消除排查盲区)

排查实机连接问题（「加不上好友」「一会儿在线一会儿不在线」）所需的关键信息**此前完全
没有日志** —— 网络层只记了 BLE 扫描、跨跳好友同意、握手验签失败、网卡绑定失败。
用户给日志也只能看到"连上/没连上"，看不到"为什么"。

新增四类（全部落在 `link` target，便于过滤）：

| 日志 | 为什么需要 |
|---|---|
| `建链 peer=… path=… ep=…` | 何时连上、走的哪条通道（lan/routed/bluetooth）。原先完全没有 |
| `掉线 peer=…（链路断开）` | 与下面那条是**两条不同路径**，只有日志能区分 |
| `超时清理 N 个节点：…` | 45s 超时清掉的节点在界面上同样显示"离线"，但原因完全不同（链路断了 vs 我们没再收到它的 announce/Presence） |
| `gossip 扇出队列满，丢弃本条` | 评审指出的不可观测路径：`broadcast_gossip` 超时丢弃原先完全静默（限频 30s，与既有 heartbeat 拥塞告警同口径） |

**已确认这些日志确实可用**：E2E 实跑日志里出现了
`[info] [link] 建链 peer=e2e-peer path=lan ep=Tcp(127.0.0.1:61249)`。

**同时确认了两套机制其实是一套**（原以为要统一）：`push_diag_event` 本身就会写
`logger.info/warn("discovery", …)`，诊断面板与日志**不是两个地方**。
`DROPPED` 跳过名单里的每一项都核实过理由成立（`hello_rejected` 在**两处**调用点
都另有 `logger.warn`，带更完整的上下文）—— **没有需要删除的冗余日志**。

新增 `log_throttled(key, ms)`：模块级静态限频助手。项目原有的限频是循环内局部变量
（heartbeat 拥塞告警），自由函数用不上。用静态而非 `AppState` 字段 —— 它只服务日志，
不值得为诊断辅助引入需要清理的可增长状态；key 全是编译期字面量，表的规模天然有界。

**未做**：`ensure_link` 的拨号失败原因仍无日志（它的失败分支较多，需要逐个读后
决定哪些值得记，不宜仓促加）。

验证：cargo test --lib 480 passed · npm test 424 passed ·
scripts/e2e-dev.sh 30 passed / 0 failed，且实跑日志确认新日志生效。

## [4.17.2] - 2026-09-16

### Fixed (全面评审后的缺陷收口 —— 其中三条是本轮我自己引入的)

**🔴 所有静默事件在时间线上渲染成裸 JSON 气泡**（我自己引入，最严重）。
`ChatWindow` 把该会话的**全部**消息记录交给虚拟列表，没有任何 kind 过滤 ——
`KindClass::Silent`（「不进时间线」）这条契约在**唯一真正重要的地方**（渲染）从没落地。
后果：回一次表情 / 置顶 / 撤回，时间线上就多一条
`{"target":"...","emoji":"[赞]","add":true}`，而且与正确的聚合视图（气泡下方的 chip、
顶部置顶条）**同时出现**。已按 `kindClass` 过滤（card 一并过滤：公告已有独立横幅）。

**🟠 静默事件在前端仍然计未读、改预览、把会话顶到最前**（我自己引入）。
后端已按 `is_non_notifying_kind` 分支，前端 `applyIncomingToConversations` 漏了 ——
两边未读从此不一致（DB 里 0、界面 1），直到切会话重拉才纠正。
已按同一口径过滤，并补 3 条回归测试。

**🟠 撤回的「先撤后到」保护被自己的前置判定封死**（我自己引入）。
接收侧先查「发送者是否是被撤回消息的作者」，而目标尚未落库时该查询返回 None
⇒ 整条撤回被静默丢弃、**且不写入权威集合** —— 随后消息带着完整正文落库，撤回永久失效。
这与 `group_recalled_messages` 存在的意义正好相反。已改为：目标不存在时以
「发送者是本群成员」为准（能解开群消息即持群密钥），目标存在时仍严格要求作者本人。

**🟠 群公告接收侧无授权校验**。发送侧校验了 `creator == 我`，接收侧没有 ——
任何持群密钥的成员构造一条 `kind="announcement"` 即可改掉所有人的公告横幅，
墓碑事件同构。与同一批刚修的「群名劫持」是同一类漏洞、同一批改动里两处口径不一致。
已在接收侧加「sender == 本地已存 creator」判定并记 warn。

**🟠 `ChatWindow.vue` 用了未导入的 `Megaphone`**（我自己引入）。`vue-tsc` **查不出来**，
只在运行期报 "Failed to resolve component" 并渲染成空标签 —— 公告条图标缺失。
已补导入。

**🟠 单聊里的「撤回」是死入口**（我自己引入）。`canRecall` 没判 `isGroup`，
但 `confirmRecall` 里对非群会话静默 return —— 用户点完确认后**什么都没发生**
（无 toast、无日志）。已加 `isGroup` 判定。

### 评审确认无问题的区域

通道优先级与多路径（`route_order` / `send_over_order` / `try_send` / `best_link_kind` /
`should_accept_inbound` / `ensure_link`）**未被本轮触碰**，局域网 > 跨网段 > 蓝牙的优先级
与 failover 语义完整。`verify_hello` 对好友 / 非好友 / BLE 三条路径均能建链。
`upsert_peer` 的「只补不覆盖」、撤回物化与幂等、已读水位与搜索排除静默类、
清空边界只挡 Bubble、前端 emit 接线（无断链）—— 均按设计工作。

### 已知未修（记录在案）

- `mark_peer_keys_verified` 只在 TCP 入站一处调用，BLE / 出站路径不打标 ⇒ `keys_verified`
  实践中极难为真（功能上不致命：好友表优先、非好友走 TOFU，但注释与实现不符）。
- 中继态 TTL 不随分片刷新：BLE 上约 3.6MB 以上的传输跨过 1 小时会被误清。
- `broadcast_gossip` 超时丢弃无日志（真机排查「发不出去」时不可观测）。
- 置顶条重启后只显示已加载分页内的置顶；导出/`search_messages` 未过滤静默类。
- 连接生命周期（建链 / 拆链 / 拨号结果 / sweep 清理）缺日志 —— 排查实机连接问题所需。

验证：cargo test --lib 480 passed · npm test 424 passed（新增 3 项静态过滤回归）·
npm run build 通过 · scripts/e2e-dev.sh 30 passed / 0 failed。

## [4.17.1] - 2026-09-16

## [4.17.0] - 2026-09-16

### Added (群协作阶段 3 · 第一批：群任务与投票的**协议层与折叠逻辑**)

> ⚠️ **本批只交付协议、命令与折叠逻辑，UI 卡片尚未接入** —— 用户暂时无法从界面
> 创建任务/发起投票（见文末"未完成"）。这样切分是为了让最需要验证的部分
> （收敛语义）先被测住，UI 属纯展示、可独立补齐。

**群任务**：`todo`（Card，定义层）+ `todo_done`（Silent，完成层）。

**为什么必须两层**：朴素做法是一个 `{todo_id, title, assignees, done_by[]}` 寄存器，
**A 和 B 同时勾完成时后到的整体覆盖前者，B 的勾被吞掉**（经典丢更新）。
拆成「每人只写自己那一格」后不存在这个问题，`done: false`（取消勾选）也天然支持。

**投票**：`poll`（Card）+ `poll_vote`（Silent），结构与任务同构。

- **`options` 创建后不可变**：否则选项下标错位，`poll_vote` 的 choices 会指向错误的选项。
  要改选项就新建一个投票（已写进协议注释）。
- **撤票 = `choices: []` 的普通更新**，不是单独的删除事件。
- **不做匿名投票**：群密钥全员共享 + 选票必然带签名身份，"匿名"只能是界面隐藏、
  无权可验 —— 那比不做更糟（给人虚假的安全感）。

**收敛**：两层都是 LWW，版本号一律 `(seq, msg_id)` 元组 —— 只看 seq 会让不同副本
算出不同结果（Lamport 时钟两端离线后可能撞号）。

验证：npm test 421 passed（新增 7 项：畸形载荷、**不丢更新**（两人各自勾都保留）、
取消勾选只影响本人、完成层的 LWW 与到达顺序无关、定义层取版本最大、
多任务互不干扰）· cargo test --lib 480 passed · npm run build 通过 ·
scripts/e2e-dev.sh 30 passed / 0 failed。

**未完成**：任务卡片 / 投票卡片的 UI、创建入口（群聊头部的「+」菜单）、
`GroupMemberPanel` 里的任务与投票聚合面板。判定点（`WIRE_KINDS`）已接入，
`CARD_KINDS`/`SILENT_KINDS` 与跨语言契约测试已同步。

## [4.16.0] - 2026-09-16

### Added (群协作阶段 2 · 第三项：群公告，阶段 2 完成)

- **协议**：新 kind `announcement`（Card）+ `announcement_delete`（静默墓碑）。
  新增 `KindClass::Card` —— **进时间线**（发布是一条事件，该计未读、该通知），
  但**不属于"聊天历史"**。这两点是它与 Bubble 的全部差别，也正是它必须单独成档的原因。
- **权限：仅群主**（与 `handle_group_rename` 的 `creator == 我` 逐字同构）。
  公告是发给全群的权威信息，人人可发就失去了"公告"的意义。群主离线时发不了 ——
  无中心即无中心授权，不做"降级为任何人可发"。
- **「当前公告」按 `(seq, msg_id)` 取最大**，不按墙上时间：只有群主能发、
  而群主的 Lamport 时钟单调，自己两条公告不可能同 seq，tie-break 只是防御。

#### 两处全局语义改动（本次真正的风险点）

1. **`delete_conversation` 只删 Bubble**。原先 `DELETE FROM messages WHERE conv_id=?`
   会把公告一起删掉 ——「清空聊天记录」顺手清掉群公告是错误语义（与群文件同理：
   那是群资产，不是聊天记录）。清单从 `WIRE_KINDS` 派生，不手写。
2. **`group_message_blocked_by_boundary` 只对 Bubble 生效**（新增 `kind` 参数）。
   水位是**聊天历史**的水位。若它连公告一起挡，一个离线成员的公告
   （seq ≤ 本机 boundary）会被丢弃 ⇒ **各成员看到的公告不一致**，
   而公告恰恰是要求"所有人都看到同一份"的东西。

两处都有专门测试（`clearing_history_keeps_group_level_artifacts`、
`clear_boundary_only_blocks_bubble_kinds`）。

- **UI**：公告条常驻聊天头部下方（点击看全文），群主额外有「发布/修改」入口
  （移动端同样可点，弹窗与桌面一致）。上限 500 字 —— 公告是横幅里的一段短文本，
  长文该发消息；同时也是对广播体积的限制。

**未做**：`state_sync`（新成员入群时补发置顶/公告）。当前新成员看不到入群前的公告，
与「历史不回填」是同一个已知边界，已在 CHANGELOG 与代码注释里注明；
公告可以随时由群主重发一次作为绕过。

验证：cargo test --lib 480 passed（新增 2 项：清空历史保留群级产物、边界只挡 Bubble）·
npm test 414 passed（跨语言契约测试已扩展到 Card 档）· npm run build 通过 ·
scripts/e2e-dev.sh 30 passed / 0 failed。

## [4.15.0] - 2026-09-16

### Added (群协作阶段 2 · 第二项：消息置顶)

- **协议**：新 kind `pin`（静默事件），载荷 `{target, pinned}` —— 用一个 bool 而不是
  两个 kind，取消置顶就是同一条事件的反向。与回应/撤回同构：一串独立事件，
  每个 `target` 是一个按 **`(seq, msg_id)`** 定序的 LWW 寄存器。
- **权限：任意群成员**（可逆、低风险）。与「仅群主可改名」那类不可逆操作不同 ——
  置顶错了再取消即可，不必为此引入管理员角色。
- **置顶条**：挂在聊天头部下方（钉钉/飞书同款位置），点击跳到那条消息（复用既有的
  `locateMessageInConv`）。只列出**还能在本机找到的**置顶消息 —— 历史已被清空的
  不列，避免点了没反应。超过 3 条显示 `+N`。
- **折叠在会话层算一次**（与表情回应同理）：放进每条消息各自算就是 O(n²)。
- **两个入口都有**：桌面右键菜单与移动端长按面板一致；文案随当前状态切换
  「置顶 / 取消置顶」。

验证：npm test 414 passed（新增 7 项：畸形载荷、LWW 与到达顺序无关、
同 seq 时用 msg_id 决胜、多 target 排序、isPinned）· cargo test --lib 478 passed ·
npm run build 通过 · scripts/e2e-dev.sh 30 passed / 0 failed。

## [4.14.0] - 2026-09-16

### Added (群协作阶段 2 · 第一项：加人通知)

**此前加人是完全静默的** —— 靠 GroupKey 重发携带新成员表自愈，群里其他人根本不知道
多了一个成员。而踢人（`GroupMemberRemoved`）与退群（`GroupMemberLeft`）都有系统消息，
同一类事件两种待遇。

- 新增 `group_member_added_text()`（跟随本机语言，与既有两条同口径），
  加人成功后广播一条 `system` 群消息。
- **走消息管道而不是本地插入**：踢人/退群用的是 `insert_group_system_message`（只写本机），
  因为它们本来就有专用控制帧广播；而加人**没有**控制帧，本地插入就失去了"通知全体"的意义。
  所以借用 `send_group_payload`（群密钥 E2EE + gossip + 每个成员的 outbox + 离线补发）。

**顺带厘清一档此前隐式的语义**：`is_non_notifying_kind()` = 静默类 + `system`。
系统消息此前只由 `insert_system_message` 在本机插入，而它**不碰未读与会话预览** ——
所以"进时间线但不打扰"一直是既有事实，只是从没被写下来。加人通知改走消息管道后，
若不做这个归类，「X 加入了群聊」会给每个成员推一条系统通知、还会把会话顶到列表最前。
接收路径（群/单聊两条）与发送路径三处已统一按它分支。

验证：cargo test --lib 478 passed · scripts/e2e-dev.sh 30 passed / 0 failed。

## [4.13.0] - 2026-09-15

### Added (群协作阶段 1 · 第二批：消息撤回)

- **协议**：新 kind `recall`（事件）+ `recalled`（被撤回的消息本体，content 清空）。
  撤回与表情回应同构，是**一串独立事件**而非"改一个字段"—— `message_id` 绑定了 payload，
  同一条消息不可能带不同 content 重发。
- **权威集合 + 物化视图**（新表 `group_recalled_messages`，G-Set 只增不减）：
  `messages` 行的 content 置空只是它的**物化**。两者都必需 —— 撤回事件可能**先于**
  被撤回的消息到达（Gossip 泛洪与 outbox 直发是两条无顺序保证的路径），
  只靠 UPDATE 会打到 0 行、随后消息带着完整正文落库 ⇒ 撤回失效。
  落库前查权威集合即可解决「先撤后到」。
- **content 清空带来的连锁正确性**：搜索、导出、会话预览、已读水位
  **一行都不用改**就自动正确（没有正文可命中、可导出）。
  唯一的例外是已读水位 —— 那条走 `last_message_from_sender`，已在上一批由
  `WIRE_KINDS` 派生的静默清单排除。
- **权限：仅原作者**（信封被 Ed25519 签名，sender_id 不可伪造），接收端会再核对一次
  `sender_id == 被撤回消息的作者`。不做"群主撤他人"—— 那需要管理员角色，
  而没有中心权威就没有中心授权。
- **时间窗只在发送端强制**（2 分钟）。接收端**不校验**：它无法验证发送方的墙上时钟
  （`env.ts` 不参与排序也不可信），做出来的校验是假的。这是产品规则，不是安全边界；
  真正不可伪造的是作者身份。
- **UI（各端一致）**：桌面右键菜单与移动端长按面板**都有撤回入口**，且都走同一个
  二次确认弹窗（破坏性且不可逆）。已撤回的消息渲染为居中灰条「消息已撤回」，
  刻意不给气泡/头像 —— 它与系统提示同为状态行，给气泡会让人误以为还能点开。
  撤回入口只对**自己发的、未撤回的**消息显示（后端也只在作者本人时接受，
  前端隐藏是为了不让用户白点一次）。
- **前端事件消费**：`message-recalled` 有监听（`events.test.ts` 的护栏会拦住
  「后端在发、前端没人听」的情况，本次正是它先报出来的）。收到后把本地那一行改成
  已撤回形态，与后端物化保持一致。

**关于结构化引用**：原计划要求「引用必须结构化，否则撤回清不掉别人消息里嵌的 snippet」。
实测现有引用格式**已经带了 msg_id**（`「引用 X：snippet|msgId」`），渲染端据此就能在被引
消息已撤回时改渲染 —— 用更小的改动达到同样的撤回正确性。本次按此实现，未改引用格式；
结构化引用（能引用图片缩略图等）留作后续。

验证：cargo test --lib 478 passed（新增 2 项：撤回幂等 + 物化让搜索自动正确、
权威集合可先于消息存在）· npm test 407 passed · npm run build 通过 ·
scripts/e2e-dev.sh 30 passed / 0 failed。

## [4.12.0] - 2026-09-15

### Added (群协作阶段 1 · 第一批：kind 判定点 + 表情回应)

**地基：`kind` 语义的唯一判定点。** 此前「这个 kind 算不算内容」这个知识散在多处各写
一串 match，接收路径、会话预览、未读、通知、搜索、已读水位都要各问一遍 ——
每加一个新 kind 就要同时改所有地方，漏一处就是**静默的行为不一致**（最典型的症状：
回个表情把会话顶到列表最前、还弹一条系统通知）。

- `protocol::KindClass { Bubble, Silent }` + `WIRE_KINDS` 表 + `kind_class()`；
  **未知 kind 一律按 `Bubble`**，与 `MsgKind::from_str` 回退到 `Text` 同语义 ——
  宁可多显示一条，也不要把不认识的内容静默吞掉（对端版本更新时不丢消息）。
- SQL 里的 kind 清单**从 `WIRE_KINDS` 派生**（`sql_kind_list`），不手写：
  加了新 kind 而忘了同步 SQL 就是一条只在下次有人用那个功能时才暴露的漏判。
  已接入 `search_history`（静默类没有可搜正文）与 `last_message_from_sender`
  （否则回个表情就把该发送者的群已读水位顶到最新）。
- 接收路径按分类分支：静默类**不计未读、不改预览**（只 `ensure_conversation`），
  但 `observe_clock` **照常执行** —— 漏掉它本机后续 seq 会落后、新消息排到历史前面。
  单聊与群聊两条 Gossip 落库路径同口径。
- 前端 `utils/messageKinds.ts` 是 TS 侧的唯一判定点，配**跨语言契约测试**：
  读 `protocol.rs` 源码逐项比对两张表，防止 Rust/TS 判定漂移
  （漂移的后果是「服务端算静默、前端照常弹通知」，只在真机跑起来才看得见）。

**表情回应**（钉钉/飞书里使用频率最高的群功能之一，也是"降噪"的核心手段）：

- **协议**：新 kind `reaction`，载荷 `{target, emoji, add}`。建模成**一串独立事件**
  而不是「给消息加一个可变字段」—— `message_id` 是 `SHA-256(sender_id+nonce+payload)`，
  同一条业务消息不可能带不同 content 重发（`gossip_engine` 的回归测试钉死了这一点）。
- **复用整条可靠管道**：发送内核抽成 `send_group_payload()`，表情回应与文本/代码走
  **同一条路**（群密钥 E2EE + outbox + GroupAck + 四层幂等去重 + 离线补发）。
  若各写一份，任何一处修 bug 都只会修到其中一条。
- **收敛**：每个 `(target, actor, emoji)` 是 LWW 寄存器，每人只写自己那一格 ⇒ 无丢更新；
  版本号用 **`(seq, msg_id)` 元组**，不能只看 seq —— seq 是 Lamport 时钟，两端离线后
  各发一条都可能拿到同一个 seq，只看 seq 会让不同副本算出不同结果。
- **UI**：气泡下方的回应条（飞书/微信同款位置），已点过的高亮，点击切换 add/remove。
  折叠在**会话层算一次**再按 msg_id 分发（放进每条消息各自算就是 O(n²)，群聊一屏几十条
  时是实打实的卡顿）。

**顺带修正**：快捷表情按钮原本写成 `hidden` + `group-hover/msg:flex`，两处都错 ——
组名 `msg` 根本不存在（消息行用的是 `group/row`，而回应条是它的**兄弟节点**、
不在其作用域内），且没有触屏兜底（`designGuards` 的 P0-1 复现：Android 上永远够不到）。
已加 `group/msg` 到外层容器 + `hover-reveal` 兜底类。另外**移除了「更多表情」按钮** ——
它没有接实现，而项目原则是不放没有实现的功能按钮。

验证：cargo test --lib 476 passed（新增 4 项：静默类不入检索/不顶已读水位、
kind 清单派生一致性、表情 token 形态校验、回应载荷往返）· npm test 407 passed
（新增 11 项：折叠收敛 7 项 + 跨语言契约 3 项 + 形态校验）· npm run build 通过 ·
scripts/e2e-dev.sh 30 passed / 0 failed。

## [4.11.0] - 2026-09-15

### Added (TOFU 第二步：安全码核对)

首次接触（TOFU）的**唯一解法**是带外核对 —— 广播/Hello 里的公钥即便自签名，也只证明
「持有该私钥」，不证明「他就是那个 device_id」；攻击者抢先冒充时，协议层面无从分辨。
但只要双方在带外（当面、电话、另一条已知可信的信道）比对一串由**公钥派生**的数字，
中间人就藏不住了。

- **`crypto::safety_number()`**：由双方身份（device_id + 两把公钥，共 6 个字段）派生
  6 组 × 5 位十进制（30 位 ≈ 100 bit）。
  - **对称**：两方按 `device_id` 字典序排列后再哈希，因此 A 与 B 算出**同一个码** ——
    这是能核对的前提（按位置喂进去会得到两个不同的串，当面比对无从比起）。
  - **域前缀** `gosslan-safety-v1` 与其它哈希隔离；字段间插分隔符，
    避免 `("ab","c")` 与 `("a","bc")` 撞成同一码。
- **`get_safety_number` 命令**：对端公钥取 peers 优先、friends 回落（与
  `resolve_member_x25519` 同一口径）。**缺公钥时返回 `None` 而非拿 device_id 凑一个** ——
  凑出来的码在真正的攻击下与真实对端不同，用户核对后会以为"对得上"，比不给更糟。
- **好友资料页**：新增安全码区块（六段等宽显示 + 一键复制 + 一段使用说明）。
  **同时删掉了原先那个「指纹尾码」**——它取的是 `device_id` 的后 8 位，而 device_id
  是随机串、**与密钥无关**，冒充者伪造同一个 ID 就能得出同样的尾码：看着"对得上"
  却毫无防护作用，留着会让人误以为已经核对过。

**这一步补齐了什么**：4.10.0 的 announce 自签名让「广播的密钥」可归因，本步让用户能
**验证**那对密钥确实属于对方。两者合起来，中间人攻击从"无法察觉"变为"带外一比对即暴露"。

验证：cargo test --lib 472 passed（新增 4 项：对称性、确定性与格式、任一字段被替换码必变
（含换我自己的 X25519）、拼接歧义由分隔符消除）· npm test 396 passed ·
npm run build 通过 · scripts/e2e-dev.sh 30 passed / 0 failed。

## [4.10.0] - 2026-09-15

### Added (TOFU 第一步：announce 自签名)

背景：`UdpPacket` 此前携带 `device_id` 与两把公钥却**没有任何签名字段** —— 一条完全
无认证的信道。4.9.1 修的「伪造广播替换好友公钥」正是它的直接后果。

- **协议**：`UdpPacket` 增加 `nonce` / `sig`（均 `serde(default)`，旧端互通）；
  新增 `announce_signing_bytes()`（域前缀 `gosslan-announce-v1`，与 Hello 的签名材料
  域分离）与纯函数 `verify_announce()`，判定三档：
  - `Verified`：签名有效 —— 广播者持有其声明 Ed25519 公钥的私钥；
  - `Legacy`：无签名（旧端）→ **放行**。它只能驱动「拨号」，而身份绑定一律由 Hello
    验签决定（announce 来的公钥恒为 `keys_verified = false`）；硬拒会让旧端在局域网内
    彻底不可见，代价大于收益；
  - `Invalid`：带签名却验不过 → **在它影响任何状态之前丢弃**（篡改或伪造）。
- **发送侧**：每次广播生成新 nonce 并签名（广播周期 5s，nonce 每轮都换）。
- **签名范围只含安全相关字段**：device_id / tcp_port / 两把公钥 / nonce。
  **刻意不含 nickname** —— 它是展示信息且随改名变化，纳入签名会让「改个昵称 → 旧签名
  全部失效」。有单测钉死这一点。

**这条能做到什么、不能做到什么（重要）**：能防篡改、防重放、让每条广播可归因到某个
密钥持有者；**仍不能**阻止攻击者用自己的私钥签一个「自称是某人」的包 —— 那是首次接触
（TOFU）的固有限制，需要带外指纹核对（本步为其打基础：签名让「广播的密钥」与
「建链时 Hello 的密钥」可被关联比对）。

**顺带修复**：`examples/mirror_dial.rs` 与 `examples/dual_link.rs` 在 HEAD 上就已编译失败
（`Message::Hello` 缺 `content_features`，4.8.1 只补了 `e2e_peer`）。两个示例已修好，
`cargo check --all-targets` 现在 **0 错误**；`mirror_dial` 的 announce 也改为真签名。

验证：cargo test --lib 468 passed（新增 6 项 announce 单测：自签名通过、逐字段篡改被拒、
冒用他人公钥被拒、旧端放行、带签名但缺 nonce/公钥被拒、签名材料域分离）·
cargo check --all-targets 0 错误 · scripts/e2e-dev.sh 30 passed / 0 failed
（发现与建链正常 ⇒ 签名未改变线格式）。

**未覆盖**：接收侧 `Verified` 分支只有单测覆盖 —— E2E 是单实例 + 协议级对端，
对端不参与验签；`announce_*` 属诊断环形缓冲、不落日志文件，故本次未做双节点真机确认。

## [4.9.2] - 2026-09-15

- **`resume_receive` 为给哈希器播种而整读 `.part` 前缀**。`.part` 最长等于整个文件，
  于是「几个 GB 的文件传到 90% 断链、对端续传」会让进程瞬间占用 ≈ 文件大小 ——
  而续传本身正是为这种大文件场景设计的。改为分块喂哈希器，峰值只剩一个分片；
  同时把「磁盘前缀长度 ≠ 声明 from_bytes」记为一条 warn（**只记录不改行为**：
  `received` 仍以 from_bytes 为准，单方面改动会让两端 seq 对不上）。
- **`send_file_via_relay` 整读源文件再切片**。中继发送的是共享目录里的文件，
  整读后逐片 base64（×1.33）内存峰值远超文件本身。改为按需 `seek` + `read_exact`，
  峰值只剩一个分片。附带修正：`total` 改用 metadata 的 `size`（与 Offer 声明一致），
  文件在发送途中被截断时 `read_exact` 会**报错**而不是静默发一份短的。

**平台兼容性**：本次改动未引入任何平台相关代码（无 `#[cfg(...)]` / `target_os` /
平台专用 crate）。用到的 `std::fs::canonicalize`、`std::fs::File::{seek,read_exact}`、
`tokio::time::timeout`、HashMap `retain`、SQLite `CASE` 均为跨平台。
`delete_file` 的路径比较**两侧都做 canonicalize**，因此 Windows 上
`\\?\` 扩展长度前缀是两边一致的（该写法与既有 `resolve_media_path` 同源）。
前端唯一平台敏感处 `utils/localFile.ts` 是把原有的 `isAndroid` 分支**抽出共用**，
Android 的 SAF 另存为行为原样保留，且群文件面板也一并获得该行为。

验证：cargo test --lib 462 passed · cargo check 零警告 ·
scripts/e2e-dev.sh 30 passed / 0 failed（其中「下载方向文件传输」走的正是
`send_file_via_relay`，断言内容逐字节一致）· npm test 396 passed。
Android 交叉编译在本机无法执行（缺 NDK 的 `aarch64-linux-android-clang`，
失败发生在 `cc-rs` 构建 rusqlite 阶段、早于本次改动的任何代码）。

## [4.9.2] - 2026-09-15

### Fixed (后端审计续：5 条中低危缺陷收口)

- **`delete_file` 接受任意路径**。读取侧早有边界（`resolve_media_path`：canonicalize 后
  必须落在 downloads 内，或该消息确由本机发出），删除侧却没有。当前唯一调用方只清理
  `save_outgoing_image` 刚写进 downloads 的孤儿图片（注释原话「避免 downloads 目录堆积垃圾」），
  所以按 downloads 目录设限**不影响任何既有功能**；但没有它，任何一处 UI 把它接到消息里的
  `path`（该字段由对端控制）就会变成「对端点一下按钮删掉本机任意文件」。
- **`broadcast_gossip` 扇出用无超时的 `send().await`**。目标是有界队列（1024），对端僵死时
  会永久挂起 —— 而它被 `handle_gossip` 内联 await，后者由 reader_loop 调用 ⇒
  **另一个对端的读循环被卡住**，其后续帧（含心跳）全部排队直至被判不健康而拆链，
  即"一条拥塞链路伪造出全网链路故障"。补上与 `send_over_order` 同一口径的有界等待。
- **Hello nonce 在验签之前就被消费**。nonce 缓存是一条有界 FIFO（512 条），先消费等于给
  任何**未通过验签**的连接发了一张污染缓存的入场券：洪泛者可持续占用/挤出槽位，把合法对端的
  nonce 顶掉，或在窗口内让合法 Hello 被误判为「重放」而拒（表现为"好友时连时断"）。
  改为验签通过后再消费 —— 重放的 Hello 签名本就有效，依旧会被同一判拦下，只是不再占槽位。
- **四张按对端可控键索引的内存表无界增长**。`relay_file_keys` 与 `RelayManager::reassemblies`
  以 `transfer_id` 为键、插入于收到 `RelayFileOffer` 时，清除点却只在「重组完成/失败」——
  对端（只需是好友）持续发新 id 的 Offer 却永不发分片，两张表就只增不减直至 OOM；
  两者补 `created_at` 并接入既有的每小时定时任务（TTL 1h，与 `.part` 的 24h 同源但内存态更短）。
  `peer_content_features` / `key_conflict_warned` 原先只在「删好友」时清，而节点进出比删好友
  频繁得多，改为挂在 `sweep_peers` 已有的节点淘汰点上（同一个回收点，节点已不在 peers 表）。
- **锁中毒处理写法统一**（15 处生产代码的 `.lock().unwrap()` → `unwrap_or_else(|e| e.into_inner())`，
  与 `state.rs` 声明的约定一致）。**测试代码保持 `.unwrap()` 不动**：测试里中毒应当大声失败，
  改成静默自愈反而会掩盖问题。

护栏：`sweep_stale_reassemblies_keeps_active_and_drops_expired`（只清过期、保留进行中、幂等）。

验证：cargo test --lib 462 passed · cargo check 零警告 ·
scripts/e2e-dev.sh 30 passed / 0 failed（日志零 nonce 重放、零身份拒绝，握手正常）。

## [4.9.1] - 2026-09-15

### Fixed (后端审计：4 个 High 缺陷收口 —— 其中一条可击穿 E2EE)

审计范围 `src-tauri/src/**`（约 5.4 万行），全部结论均读过源码确认并补了回归测试。

- **未签名的 UDP announce 可永久替换好友公钥 ⇒ E2EE 被击穿（最严重）**。
  `UdpPacket` 携带 `device_id` 与公钥却**没有签名字段**；`upsert_peer` 对某个
  `device_id` 首次见到即绑定该公钥，此后遇到不同公钥（哪怕来自**验签通过**的 Hello）
  只标记冲突并拒绝写入；绑定值还会写进持久化的 `friends` 表。于是局域网内一个伪造
  announce 即可：覆盖好友真实公钥 → 我发给该好友的消息改用攻击者公钥加密
  （消息广播给所有已连接节点，攻击者用自己的私钥即可解开）→ 并用自己的 Ed25519
  冒充该好友（绑定的就是他的公钥，验签必然通过）；真实好友反而永远连不上。
  根因是**未认证信道的绑定赢过了认证信道**，而 `verify_hello` 的注释把这条设计
  写在了明处（「密钥绑定在后续 announce/upsert 中固化」）。修复分三处：
  `Peer` 增加 `keys_verified`（只有 Hello 验签通过才由 `mark_peer_keys_verified` 置位）；
  `verify_hello` 的身份绑定回落**只采信已验签条目**（抽成 `bound_ed25519_from_peer`
  并加单测）；写 `friends` 表的路径一律加验签闸门，且
  `db::update_friend_pubkeys` 改为**只填空位、绝不覆盖已有值**（最坏后果从
  「E2EE 被击穿」降级为「公钥为空时被抢先填一次」）。
  遗留：首次接触（TOFU）仍可被抢先冒充 —— 那需要带外指纹核对，
  与本仓库路线图里的「好友指纹安全码 / QR 校验」是同一件事，未在本次范围内。
- **`update_profile` / `broadcast_chat_style` 持全局 `links` 锁跨 `.await`**。
  发送目标都是有界队列（1024），对端僵死时 `send().await` 会永久挂起却握着全局
  links 锁 ⇒ try_send、心跳、`get_peers`、`mark_peer_offline`、`teardown_link`
  以及看门狗全部阻塞。看门狗恰恰是唯一能发 cancel 拆掉那条卡死连接、让队列排空的
  机制，被同一把锁挡住即形成**自锁死循环**，只能靠用户手动重开局域网。
  改为锁内只克隆发送端快照、发送在锁外做（与心跳发送同一纪律），并加源码断言护栏。
- **网络下发的 `transfer_id` 未校验即拼进落盘路径（CWE-22）**。接收端用对端完全可控的
  `transfer_id` 直接构造 `{id}.part` 并 `File::create`（创建或**截断**），
  一个 `../../../../Users/me/Documents/x` 即可逃出下载目录，失败收尾路径还会
  `remove_file` 它。显示名早已有 `safe_file_name` 消毒，`transfer_id` 这一半漏了。
  新增 `safe_transfer_id`（白名单 `[A-Za-z0-9_-]{1,64}`），在 `make_receiver` 与
  `resume_receive` 两处入口同时校验 —— 单聊与群文件、首传与续传全部覆盖。
- **群消息路径绕过「仅群主可改名」**。专用 `GroupRename` 帧严格要求群主，
  但群 Gossip 消息携带的 `group_name` 由 `upsert_group` **无条件覆盖**，
  任何成员都能改掉所有人的群名（可伪造成「系统通知」做社工），且这条路没有长度上限。
  修复分两层：`upsert_group` 只允许 creator 一致时更新名字（持久层兜底）；
  群消息分支再校验 `env.sender_id == creator`（否则成员填真群主的 id 就能对上 creator），
  并补上 `MAX_GROUP_NAME_LEN` 截断。

护栏：`hello_binding_ignores_unverified_announced_keys`（含反证断言，锁住「为什么必须过滤」）、
`announced_attacker_key_cannot_bind_and_impersonate`（完整攻击链回归）、
`update_friend_pubkeys_never_overwrites_a_bound_key`、
`rejects_path_traversal_transfer_ids` / `accepts_real_world_transfer_ids`、
`group_name_only_updates_for_the_recorded_creator`、
`never_awaits_while_holding_the_links_lock`。

验证：cargo test --lib 461 passed（新增 7 项）· cargo check 零警告 ·
scripts/e2e-dev.sh 30 passed / 0 failed（身份、建链、群聊、文件全链路无回归，
应用日志零身份拒绝与零密钥冲突）。

## [4.9.0] - 2026-09-15

### Added (群协作能力 · 阶段 0：群文件列表 + 会话置顶 + @所有人)

群聊补上三项「高频但一直缺」的能力。三项均为**零协议改动**：不新增 `Message` /
`GossipKind` 变体（新增会让旧端反序列化失败而断链），因此完全不涉及加密与可靠性管道。

- **群文件列表**：新增 `list_group_files(group_id)` 命令 + 群聊头部入口 + `GroupFilesPanel.vue`。
  把该群共享过的文件汇总成清单（名称/大小/发送者/时间 + 本机持有状态 + 对全群投递进度），
  支持点开已持有文件、对未取到的文件一键重新获取。此前只能顺着聊天记录往回翻。
  - **本机持有状态必须看磁盘**：路径在 DB 里存在不代表文件还在（存储清理会删媒体文件），
    只信 DB 会列出一堆点了打不开的条目。
  - `group_files` 表**不随「删除聊天记录」清空**（群文件是群级资产，同钉盘语义）。
  - 「重新获取」直接复用消息气泡既有的 `request_content`（ADR-0019 拥有即授权）——
    群文件 msg_id 恒为 `gfile-{transfer_id}`，面板凭一行元数据即可构造同一请求。
- **会话置顶**：`conversations` 增加 `pinned` 列（纯本地偏好，不广播不同步），
  右键菜单新增置顶/取消置顶，列表项名字左侧显示置顶图标。
  - 抽出 `sortConversations()` 作为**唯一排序口径**：置顶优先、其次按 last_ts 倒序。
    此前排序散在 `applyIncomingToConversations` 与 store 的多处，会出现
    「置顶了但被新消息挤下去」的不一致。
  - `ensure_conversation` 改为回读已存在的行：原先凭空造 `pinned=false`，
    会让前端把已置顶的会话当成未置顶。
- **@所有人**：群聊输入 `@` 后菜单首项为「所有人」（图标区别于真人头像），
  接收端按同一套边界规则（@ 前须行首/空白）识别并标红 `[有人@我]`，与点名的判定共用一个入口。
  - 落到正文的永远是固定字面量 `@所有人`，**不随界面语言变化** —— 它是一条发给所有人的文本，
    英文界面发出去的必须同样能被识别。

### Changed (抽出共用实现，不新增行为)

- `utils/fileKind.ts`：文件类型识别与配色从此有唯一判定点。
  原先这段 switch + 配色表内联在 `MessageFileBubble.vue`，群文件面板需要同一套视觉语言，
  复制一份会让同一种文件在列表和气泡里长得不一样。气泡已改为引用该模块。
- `utils/localFile.ts`：打开/另存本机文件的唯一路径。Android 上系统常没有能"打开"
  这类文件的应用，规则是一律改走系统保存对话框（SAF）——这条平台差异此前只写在
  `useMessageFile`，群文件面板若各自实现会退化成"点了直接报错"。

## [4.8.2] - 2026-09-14

### Fixed (审计 §7 风险收口：续传对账 + 过期 .part 定期清扫)

- **风险 1（发送端全量重试 vs 接收端续传撞车）**：让接收端成为"我有什么"的**唯一权威** ——
  没有活跃接收器时，若本地保留的 .part 前缀长度 ≠ 发送端的 from_bytes，回
  FileReject.received = 真实前缀长度；发送端据此**从断点续发**而不是重头覆盖
  （send_file_from_path_at 内最多对账 3 次，之后才报"对方未接受"）。
  活跃接收器的"重复 offer 幂等 accept"**保持不变**（那条修过真机的大图收不全缺陷）。
- **风险 2（过期 .part 无清扫）**：新增 sweep_stale_parts —— 启动时 + 之后每小时一次，
  只删"超过 24h 且当前不在接收中"的 .part，绝不碰活跃接收。
- 协议：FileReject 增加 received（serde default，旧端缺省 0 ⇒ 退化为整份重传，互通）。

护栏：源码断言（retained_part_len + received: retained + sweep_stale_parts）。

## [4.8.1] - 2026-09-14

## [4.8.0] - 2026-09-14

### Added (断点续传：弱网 / 大文件从断点继续，不再整份重来 —— ADR-0019 Phase 2)

- **协议**：FileOffer 增加 from_seq / from_bytes；ContentRequest 增加
  transfer_id / from_seq / from_bytes（serde default，旧端互通）。
- **发送端**：send_file_from_path_at 从 from_bytes 偏移读文件、seq 从 from_seq 编号
  （stream_file 用 AsyncSeekExt 定位）；自动重试与手动重取都带上 transfer_id + 已收字节。
- **接收端**：fail_receive / fail_group_receive **保留 .part**（不再删除）；新增
  resume_receive —— 读入已有前缀播种 SHA-256 hasher、received 接上、next_seq 归零，
  然后以 append 方式继续收。
- **服务端**：ContentRequest 处理沿用原 transfer_id，并按 from_bytes 续发。
- **安全兜底**：前缀长度 / TTL / 边界任何不一致 ⇒ resume_receive 返回 Err ⇒ FileReject
  ⇒ 发送端整份重传（绝不比今天更差）。TTL 24h，避免 .part 无穷增长。
- **护栏**：源码断言（resume_receive + send_file_from_path_at 必须在）+
  content::policy::resume_from_seq 单测（分片边界 / 尾部半片 / 非法分片大小）。

> 设计说明：续传段内 seq 归零重编（hasher 是字节级、与 seq 无关），因此**不需要**
> 持久化 next_seq；接收端只依赖 from_bytes。ADR-0019 §5 已更新为"已实现"。

## [4.7.5] - 2026-09-14

## [4.7.4] - 2026-09-14

## [4.7.3] - 2026-09-14

### Changed (断点续传前置：接收进度持久化 + 续传起点纯函数)

断点续传（Phase 2）的两块前置，先单独落地并测好，避免一次性改热路径：

- content::policy::resume_from_seq(received, chunk)：把已收字节折算成**分片序号**
  （向下取整到分片边界；尾部半片必须丢弃重传，否则 hasher 与 seq 对不齐，最终 SHA 必错）。
- write_chunk 每 500ms 把 received 落库（content_transfers.received，只前进）：
  这是续传的起点，也让统一状态能拿到真实进度。锁顺序保持
  file_receivers -> 释放 -> db（不嵌套）。

## [4.7.2] - 2026-09-14

### Fixed (群聊收文件/图片同样进入统一状态并能自动重试)

- begin_group_receive 一开始就登记 content_transfers（Active + cid）；
  fail_group_receive（中途断链/失败）由 record_failure 标 **Incomplete**。
- 于是群聊里"某个人看不到图"也走同一条自愈路径：状态可见、建链自动重取、
  点一下「重新获取」；且**已收完的成员是种子**（前一条已实现），原发送方不在时也能从群友取。

护栏沿用 incomplete_content_is_auto_retried_behind_capability_gate。

## [4.7.1] - 2026-09-14

### Fixed (中途失败/断链的接收不再停在 Active：记为 Incomplete 并自动重试)

- fail_receive（超时 / 断链 / 坏片清理）现在把该内容记为 **Incomplete**（可恢复），
  于是建链时 retry_incomplete_content 会按退避自动重取 —— 此前这类记录会停在 Active，
  should_retry_now 判不过，**自动重试实际不会触发**。
- 新增 **ADR-0019**（统一可靠内容传输）：完整记录分层、内容寻址、拉取式补取、能力协商、
  自动重试，以及 Phase 2 断点续传 From(seq) 的设计与难点（保留半成品 + 分片边界对齐 +
  复用 hasher + TTL 清理）。

护栏：incomplete_content_is_auto_retried_behind_capability_gate 增补 file.rs 断言。

## [4.7.0] - 2026-09-14

### Added (Phase 1 UI：统一内容状态呈现在文件卡片上)

- get_content_transfers 接通前端：store 持有 contentTransfers（随 refreshTransfers 一起刷新，
  不必额外 IPC 通道）。
- 文件卡片：当该内容在统一状态里是「未完成 / 校验失败」时，右侧按钮从「下载」变成
  **「重新获取」**（按 cid 拉一份，对方无需确认）；图片气泡此前已支持点击重取。
- MessageItem 用消息内容里的 sha256 关联到对应的内容记录（旧消息没有 sha256 ⇒ 不显示，
  不影响任何现有行为）。

护栏：前端用例（api / store / 文件卡片接线）。

## [4.6.0] - 2026-09-14

### Added (Phase 1：未完成内容自动重试 + 统一状态查询)

- **建链自动重试**：Hello / 建链时把该 peer 名下未完成的**接收**重新拉一遍
  （只对声明了 CONTENT_FEATURE_PULL 的对端发 ContentRequest；Incomplete 按退避到点、
  Active 超过 60s 没动也重试 —— 中途丢链不一定有机会写失败记录，不能让卡住的 Active
  永远不重试）。
- **接收一开始就登记** content_transfers（Active + cid）：于是"卡住 / 失败"有据可查；
  落盘后由 record_local 转 Complete，SHA 校验失败由 record_failure 转 Rejected。
- **统一状态查询** get_content_transfers：TransferRecord 现在可序列化给前端，
  供气泡显示「发送中 / 等待对方在线 / 网络不佳 / 未完成·点击重试 / 完成」
  （前端展示这批的后半段，下一提交接）。

护栏：incomplete_content_is_auto_retried_behind_capability_gate。

## [4.5.0] - 2026-09-14

### Added (内容拉取补全：接收方也能做种 + 群成员可拉 + 授权收紧)

- 接收落盘（单聊 FileDone / 群聊 GroupFileDone）后，接收方同样登记为一颗**种子**
  （content_transfers: cid → 本地 path）⇒ **群聊里 A→B 成功后，没拿到的 C 可以直接从
  已收完的 B 拉**，不再依赖 A 在线。这正是"某个群友看不到图"的自愈路径。
- 拉取授权从"仅好友"放宽到"好友 **或** 该内容所属群的成员"（并仍按 from == 链路对端
  防冒名）；其余一律拒绝并记日志。
- 单聊收到的文件消息内容补上 sha256（cid）：本机副本日后被清理时也能按 cid 重取。

护栏：content_pull_requires_capability_negotiation 增补"群成员可拉"断言；
content::store 新增种子记录单测（cid → path，带群上下文）。

## [4.4.0] - 2026-09-14

### Added (内容拉取：点一下，对方自动再发一份 —— ADR-0019 Phase 3)

消息系统稳定化的第一批可感知能力：图片 / 文件没拿到时，**点一下**就会自动从对方重新
取一份，**对方不需要确认**（拥有即授权）。仍然遵循分层与"能扩展"：

- **网络层 · 能力协商**（向后兼容的硬前提）：Hello 增加 content_features 位图，
  **不参与签名** ⇒ 老端忽略、新端可读；对端没声明 CONTENT_FEATURE_PULL 就**不发新帧**，
  自动退化成今天的推送式。对端能力位存 state.peer_content_features。
- **逻辑层 · 内容寻址**：cid = 明文 SHA-256；发送方把它写进文件消息内容，接收方据此
  知道要拉什么。发送成功即写 content_transfers（cid → 本地 path）；
  find_source 按 cid 找可服务的完整字节（**内容可用性与投递状态解耦**）。
- **业务层 · 服务端**：新帧 Message::ContentRequest { from, cid, name, size }；校验
  「from 就是这条链路的对端、且是好友」后，直接复用 send_file_from_path 回发一份
  FileOffer（Chunk / Done / CompleteAck 整套复用，零新传输逻辑）。
- **功能层 · 点击重取**：命令 request_content(peer_id, msg_id)；图片气泡加载失败点一下、
  以及「图片已被清理 / 可向对方重新索取」占位，都会触发重取。

护栏：content_pull_requires_capability_negotiation（Rust：能力位**不得**进入签名材料）
+ 前端用例（api / 气泡接线）。

> 遗留（下一批）：接收侧完成后的内容索引（让"已收完的群友"也能当种子）、断点续传
> From(seq)、以及把 content_transfers 状态统一呈现在文件气泡上。

## [4.3.23] - 2026-09-14

## [4.3.22] - 2026-09-14

### Added (内容传输逻辑层：统一生命周期 / 状态机 / 重试策略 / 持久化)

为「消息系统稳定化」（ADR-0019，Phase 1/3）打地基，新增 src-tauri/src/content/：

- model.rs：统一词汇 —— cid = sha256(明文)（内容寻址，任何持有完整字节的端都能当种子）、
  Direction、TransferStatus(queued/active/verifying/complete/incomplete/rejected)、
  FailReason（显式区分可恢复与终态）。
- policy.rs：纯函数状态机 + 指数退避（2s 起、60s 封顶）+ 失败分类；可脱离网络单测。
- store.rs：content_transfers 表，显式保存 received / attempts / next_attempt_at /
  last_error —— 这是「断网重启后还能继续」的事实依据；schema 归本层所有（分层）。
- 分层约定（用户 2026-09-15 要求）：逻辑层不依赖网络层，网络能力后续以 trait 注入；
  各层只通过能力函数调用，且都能扩展。详见 content/mod.rs 顶部。

本提交只是地基（尚未接线）：业务层接线、能力协商、ContentRequest 拉取与前端统一状态
在后续提交落地（ADR-0019 有分阶段表）。

## [4.3.21] - 2026-09-14

### Fixed (🔴 群聊收图时好时坏：在途文件被当成「已被清理」并永久缓存)

用户 2026-09-14：群里收到别人的图片有时加载失败，点几次 / 等一会儿 / 重发才出来；
三个人里有时是这个看不到、有时是另一个。

根因（"消息先到、字节后到"的竞态）：
- 接收方在 FileDone 之前写的是 <transfer_id>.part，**final 路径还不存在**；此时
  read_file_preview 走 resolve_media_path 的 canonicalize 失败分支，直接报 Gone
  =「文件不存在」，前端把它当成「已被清理」。
- 更糟的是 filePreview 把这次失败**按 msg_id 永久缓存**，而且图片气泡的预览不会随
  "传输完成"重读 ⇒ 文件明明已经落盘，界面也永远不再读（点几次也没用，只能重发——
  那是新的 msg_id）。

修法：
- 后端 resolve_media_path：final 文件缺失时先判**在途接收**（file_receivers /
  group_file_receivers，transfer_id 由 msg_id 反推）；在途报 Unknown("仍在接收")，
  只有确实不在途才报 Gone（= 真被清理）。
- 前端 filePreview：只有**确定性**失败（已被清理 / 文件过大）才缓存；新增
  invalidateFilePreview(msgId)。
- useMessageFile：预览 watcher 增加 transfer.status 依赖。
- useChatStore.onFileDone：失效 file-/gfile- 两条消息的预览缓存 ⇒ 字节一到就自动重读。

护栏：in_flight_media_is_not_reported_as_deleted（Rust）+ channelState.test.ts 前端用例 +
verify-guards.py 非空转用例。

## [4.3.20] - 2026-09-14

### Fixed (链路徽标与好友在线状态不实时：全局域网却显示「已桥接」)

用户 2026-09-14 真机：手机之前只用蓝牙、加好友加了一半；打开局域网、两边都直连后，
聊天窗口仍显示「已桥接」，好友在线状态也不实时。

两处根因：
1. 聊天头的链路来自**上一条消息的快照**（conv_link），链路从蓝牙/中继切回局域网后不会
   自动更新，只有再发一条消息才纠正 ⇒ 长期显示「桥接」。
2. 前端 onPeers 只按"在不在节点表"判在线，忽略了「有活跃链路但广播没收到（防火墙/组播
   限制）或刚被 sweep 清理」的情况 —— 而后端 get_friends 的 friend_is_online 是
   "最近 15s 见过 或 有活跃链路"，两边口径不一致 ⇒ 连上了却显示离线。

修法：
- get_conv_link 改为 async 实时计算：**有直连 ⇒ hop=0 + 当前选路（LAN > Routed > 蓝牙）**；
  无直连才回落到消息快照。（Tauri 要求带引用输入的 async 命令返回 Result，Ok 自动解包，
  前端拿到的仍是 LinkState | null，契约不变。）
- peers-updated 事件带上每个节点的活跃链路（link 字段，try_lock links）；前端 onPeers 把
  "有链路的节点"也算在线，与后端 friend_is_online 同口径。
- 聊天头在活跃对端的 link 变化时立即刷新链路状态，不再等新消息。

护栏：link_badge_and_presence_are_live（Rust 源码断言）+ channelState.test.ts 前端用例 +
verify-guards.py 非空转用例。

## [4.3.19] - 2026-09-14

### Fixed (🔴 Windows CI 打包失败 —— Windows 专用分支的编译错误本地拦不住)

用户 2026-09-14：GitHub Actions 的 Windows 两个 job（x64 + arm64）都卡在
"Build Windows installer (NSIS)" 步骤失败。

根因：4.3.18 新增的 notifications.rs 里，Windows 专用分支写成了
curr_dir.ends_with(format!("{SEP}target{SEP}debug"))。str::ends_with 需要 Pattern，
而 String 没有实现它（只实现了 &String）⇒ Rust 编译失败。这段在 macOS 上被
#[cfg(windows)] 掉，本地 cargo check 完全看不到 —— 于是两个 Windows job 同时挂，
而 mac/安卓 CI 照样绿。

修法：补 .as_str()（与 tauri-plugin-notification 上游的写法一致）。
护栏：新增单测 windows_only_branch_is_source_checkable_for_pattern_bounds ——
扫描生产代码里每个 ends_with(format!( 必须以 .as_str()) 收尾（跳过注释行、排除测试模块，
避免“护栏被自己的说明误伤”）；verify-guards.py 加了对应非空转用例。

## [4.3.18] - 2026-09-14

### Fixed (🔴 Windows 收不到系统通知 + 通知开关对好友申请无效)

用户 2026-09-14：Windows 同事反馈收不到任何消息的系统通知；排查时发现整条通知链路
**完全不可观测**（失败既不报错也不记日志）。

根因（三处叠在一起）：
1. tauri-plugin-notification 的桌面 show() 把真正的一次 toast 放进 spawn 然后丢掉结果
   （spawn 里 let _ = notification.show();）⇒ 失败时日志里连"有没有尝试发"都没有。
   插件自己的平台说明也写着 "Only works for installed apps." —— Windows 上未安装的 exe /
   未注册 AUMID / 专注助手（勿扰）都会静默失败。
2. 前端的桌面分支虽然写了 WebView 原生 new Notification 并挂 onclick，但插件会把
   window.Notification 换成"转发到 plugin:notification|notify"的实现 ⇒ onclick 永远不触发
   （点击无法定位会话），而且同样是静默失败。
3. 好友申请 / 好友通过这两类**不经前端**的通知（Rust 直接发）完全没判 notify_enabled
   ⇒ 用户关掉通知后仍会被弹。

修法：
- 新增 src-tauri/src/notifications.rs：桌面直接用 notify-rust（与插件同一底层库），
  **把错误返回出来并记 info/warn 日志**；移动端仍走插件（保留动作按钮与点击回调）。
- 后端也判一次 notify_enabled；好友申请/好友通过改走 notifications（开关生效）。
- 新增 notify_desktop 命令：前端桌面消息通知统一走后端（失败可返回/记录）。
- 设置页「通知」新增「发送测试通知」按钮（send_test_notification）：如实返回失败原因与
  平台排查说明 —— Windows 静默失败时终于有自检手段。
- 顺带修一处漏通知：窗口隐藏/最小化时 WebView 的 document.hasFocus() 仍可能为 true，
  maybeNotify 现在同时要求 !document.hidden。

护栏：notifications_are_observable_and_respect_the_switch（Rust 源码断言）+
channelState.test.ts 新增前端用例 + verify-guards.py 非空转用例。

## [4.3.17] - 2026-09-14

### Fixed (🔴 桌面端关掉蓝牙后，退出重进又被自动打开)

用户 2026-09-14 真机：设置里把蓝牙通道关掉，退出重进又变成开着的。
根因不是偏好没写（set_channel_enabled 确实写了 bt_enabled=0），而是**前端启动后无条件
自动拉起蓝牙**：ensureBluetoothOn（首帧后 2s、以及打开「添加好友」/网络设置时都会调）
用 channels[bluetooth].enabled 判"要不要拉起"，而快照里 enabled 等于**运行时是否在跑**
—— 刚启动 BLE 还没拉起，必然是 false ⇒ 它把"用户明确关掉"当成"还没启动"，重新打开。

修法：
- 后端：ChannelStatus 新增 preferred 字段（持久化偏好），与 running 分开。
  build_runtime_snapshot 从 lan_enabled / bt_enabled 填充；enabled 保持
  "运行时是否在跑"的原义（两处 UI 的开关值语义不变）。
- 前端：ensureBluetoothOn 先判 preferred，为 false 直接返回（尊重用户关闭）；
  并给 ChannelStatus 类型补上该字段。
- 行为不变的部分：首次安装 bt_enabled 缺省为开 ⇒ 仍然默认自动开启蓝牙。

护栏：channel_status_exposes_persisted_preference（Rust 源码断言）+
channelState.test.ts 新增前端用例（偏好判据必须在启用调用之前）+
verify-guards.py 对应非空转用例（删掉偏好判断 ⇒ 必须 FAIL）。

## [4.3.16] - 2026-09-14

### Tests (新增 6 条非空转护栏)

把本轮改动里最容易「静默退化、且没有编译期信号」的几处固化成
`scripts/verify-guards.py` 的注入式护栏，并做了非空转验证
（改坏 → 必须 FAIL → 恢复 → 必须 PASS，实测 6/6 通过）：

- TitleBar 图标 import：缺 Maximize2/Minimize2 会让 Windows 最大化按钮整颗消失。
- Presence / UserInfo 内联大头像必须降到 bulk 通道，不能堵住优先通道。
- directed_relay_target：共享目录/中继文件在无直连时借一跳邻居转发。
- begin_reassemble 幂等：重复 RelayFileOffer 不得清空已收到的切片。
- BLE MTU 吞吐估算必须扣 6 字节分片头，避免再报错一个量级。

## [4.3.15] - 2026-09-14

### Fixed (e2e 示例编译)

分享消息新增可选 to 字段后，examples/e2e_peer.rs 的 ShareFileRequest 少了 to ⇒
cargo build --example e2e_peer 编译不过（AI_RULES §29 要求网络/协议改动后必须跑）。
补上 to: None（e2e 走直连、不做中继）。

## [4.3.14] - 2026-09-14

### Fixed (共享目录在中继/桥接下不可用)

真机 2026-09-14 全 Windows 局域网：A 与 B 只能经中继通信时，A 打不开 B 的共享目录。
原因是共享目录三件套直接用 try_send（只支持直连），且 ShareTreeRequest/ShareFileRequest
的处理器要求 from == peer_id，中继转发会被丢弃；ShareTreeResponse 甚至没有 to 字段，
无法送回。聊天有 broadcast_gossip 兜底，所以聊天能过、共享目录不能。

修法：
- ShareTreeResponse / ShareFileRequest 增加可选 to（serde default，旧端兼容）。
- handle_message 顶部新增**定向中继**：不是给我的 ShareTree/ShareFile/RelayFileOffer
  借邻居的直连转投给 to（一跳）。
- 发送侧无直连时改走 relay_send_to_neighbors；新增 send_file_via_relay 用既有
  RelayFileOffer/RelayChunk 发送共享文件（E2EE 与直传一致，中继只透传密文）。
- 中继接收路径幂等：重复的 RelayFileOffer 不再清空已收到的切片/重置 hasher。

### Fixed (🔴 全 Windows 局域网：同一网段却走桥接 / 共享目录打不开)

真机 2026-09-14：三台 Windows、同一网段、蓝牙都开。A 与 B 之间显示「桥接 · 1」，
A 打不开 B 的共享目录（C 能打开）。根因不是选路优先级（LAN > Routed > Bluetooth 是对的），
而是 **A 根本没有到 B 的直连**：

1. 新学到的**跨跳**节点（只有 ip 空的 Presence）永远不会触发 LAN 拨号 ——
   ensure_link 只由 UDP announce 驱动。若 B 的 UDP 广播没被 A 收到（防火墙 / 虚拟网卡），
   A 就只能一直走中继。
2. 入站 TCP 被**无条件记成 LAN**。若对端是从 Clash TUN / VPN / Tailscale 地址拨进来的，
   它会被当成 LAN，has_lan_path 永真 ⇒ 本机再也不拨对端的真实 LAN 地址。
3. announce 的源地址未过滤：虚拟地址（Clash fake-ip / Tailscale CGNAT / link-local）
   也会被当作 LAN 去拨，同样堵死真实 LAN 直连。
4. broadcast_gossip 按**插入顺序**取第一条链路（v.first），同一 peer 同时有 LAN 与 BLE 时，
   控制帧/聊天回退可能走 BLE。
5. conv_link 是「上一条消息」的快照，直连建好后仍显示「桥接」直到再发一条消息。

修法：
- 学到**新的跨跳节点**时主动喊一轮 who_has：同网段节点用单播回 announce ⇒ 立刻建直连。
- 入站 TCP 按对端地址分类：虚拟地址记 Routed，其余才记 LAN。
- ensure_link 跳过虚拟源地址（不拨假 LAN）；真实 LAN 的 announce 会再来一轮。
- broadcast_gossip 按路径优先级（LAN > Routed > Bluetooth）选链路，不再用插入顺序。
- 链路登记时把该会话的「桥接」快照纠正为直连（hop=0）。

### Fixed (Windows 聊天窗口最大化/还原按钮消失)

0e07dd4 把 macOS 红绿灯改成自绘后删掉了 Maximize2/Minimize2 的 import，
但 Windows/Linux 分支仍在用它们 ⇒ 整颗「最大化/还原」按钮渲染为空。
恢复 import，并加护栏测试（模板用到就必须在 script 里 import）。

## [4.3.13] - 2026-09-14

### Fixed (🔴 安卓蓝牙：多片帧永远发不出去 —— 好友申请/同意 2 片必挂)

真机 2026-09-14（4.3.12，安卓 central ↔ Mac 外设）：
单片的聊天（272B）能发出去，**2 片的 FriendRequest/FriendAccept（738B）永远失败**，
日志是 BLE 写入失败：Unable to write characteristic；写失败 4 次即拆链路，
于是每 2–4s 自拆重连一次，好友永远同步不了（安卓显示还不是好友、Mac 列表没反应）。

根因在 btleplug 的 Android Java 实现：上一次 GATT 操作的 onCharacteristicWrite 回调
里就直接发起下一次 writeCharacteristic，而 Android 的 mDeviceBusy 此刻还没清
⇒ 第 2 片起一律返回 false。

修法（本地补丁，见 scripts/android/btleplug-java/README.md）：
- runNextCommand 改为 post 到主线程，等当前回调返回后再发下一跳；
- Android 13+ 改用 writeCharacteristic(characteristic, value, writeType) 新重载；
- Rust 侧兜底：no-response 被拒时，若该特征支持带响应写，就用 WithResponse 重试同一片。

影响：单聊/好友/文件所有帧长 > 1 片的 BLE 发送都受这条修复覆盖。

## [4.3.12] - 2026-09-14

### Fixed (🔴 蓝牙优先通道被大头像污染：加好友/消息被堵几分钟)

继续 4.3.11 之后的真机现象：蓝牙下「加好友要等几分钟」「消息一直发送中」。
根因不是链路，而是**优先通道里塞了大头像** —— 一张 400KB 头像要分上千片，
聊天与好友请求全排在它后面：

- broadcast_presence 每 10s 广播一次、走**优先通道**，却把整张 state.avatar 原样内联；
- update_profile 的 UserInfo、好友申请的 from_avatar 同样原样走优先通道。

修法：

- is_bulk_message 新增两类大而可晚到的帧走 **bulk**：大头像 UserInfo、大载荷 Gossip；
  文字/好友/回执/握手仍全部 priority（BLE 写循环 biased 先消费 priority）。
- broadcast_presence 过 hello_avatar_for_wire（2KiB）闸门，超限整个字段不带
  （接收侧 upsert_peer 只在 Some 时更新头像，缺失不会清空）；好友申请 from_avatar 同样过闸。
- 新增 send_user_info_to：建链后定向同步一次完整资料，大头像只在每次新建链路同步一次。

### Fixed (在途拨号令牌可能永不释放)

driver::connect 内部的 peripheral.connect() 没有超时：系统调用一旦挂住，拨号任务与
DialGuard 会一直存活 ⇒ 该对端在整个进程生命周期内再也不被拨号（真机「怎么等都连不上」）。
现在整条连接建立包 20s 超时（BLE_CONNECT_TIMEOUT）。

### Fixed (安卓：非图片文件点开报错 → 改为系统另存为)

- 收到的文件：点一下弹系统**另存为**（SAF ACTION_CREATE_DOCUMENT），不再 ACTION_VIEW。
- 自己发的文件：点一下**无任何响应**；长按菜单（另存/复制）保持不变。
- 新增 Kotlin OpenWith.saveWith / writeBytesWith + Rust JNI 桥；copy_file / save_data_file
  在 content:// 目标上改走 ContentResolver（原 std::fs 写 content:// 必然失败）。
- 归一化保存返回值（桌面=字符串、安卓={file:content://}）；取消不再误报「保存失败」。

### Changed

- BLE MTU 日志改为「净数据 + 按 12ms/片估算 KB/s」（旧文案 MTU=载荷+3+6 多算 6 字节）；
  蓝牙速度提示改为实测量级（约 30～40 KB/s），并明说「头像等大资料可能延迟同步」。

## [4.3.11] - 2026-09-14

### Fixed (🔴 BLE 握手永远成不了：Hello 帧里带了整张头像，一张图 424KB)

用户 2026-09-14 三端日志（Mac + 安卓，同场）：

- Mac：`[DISCOVERY] 候选可拨 id=674ff944-… ⇒ 开始连接` → `[GATT] 已就绪` + MTU 512 →
  **`[DISCONNECT] 候选 … 未建立链路：握手超时：对端未回 Hello`**；
  外设侧同一条链路上：`外设侧 MTU 协商结果 … 每片有效载荷=20 字节` →
  **`外设侧未建链 … 回 Hello 失败：帧无法分片（过大或 MTU 非法：len=424303 mtu=20）`**；
- 安卓：`[GATT] 已就绪` + MTU 514 → **`[DISCONNECT] … BLE 写入失败：Unable to write characteristic`**。
- 用户体感：**"搜得到、连不上、发不出消息"**（三台都在广播、都能互相发现）。

**根因（一条）**：`build_signed_hello` 把 `state.avatar` **原样**放进 `Hello` ——
用户头像是 base64 图片时，这个**握手帧**会到 **几百 KB**（日志里 424303 字节）。
BLE 上后果是双重的：
· central 侧：424303 ÷ 514 字节/片 ≈ **826 片 × 12ms ≈ 10s** ⇒ 正好撞上 `HANDSHAKE_TIMEOUT`
  ⇒ 对端看到的是"握手超时：对端未回 Hello"；
· 外设侧：`maximumUpdateValueLength` 在某些时序还没更新（MTU 23 ⇒ 每片 20 字节）⇒ 需要
  **3 万多片** > `MAX_BLE_CHUNKS_PER_MESSAGE`(8192) ⇒ `fragment()` 直接 `None`
  ⇒ "帧无法分片"（日志里的 `len=424303 mtu=20` 与这条完全对上）。

**修法**：给握手帧加**头像尺寸闸门** `HELLO_AVATAR_MAX_BYTES = 2048`（纯函数
`hello_avatar_for_wire`）：超过就不放进 Hello，并打一条 warn（附实际字节数）。
头像本来就有专门的 `Message::UserInfo` 通道同步，握手帧必须小到能秒过。

**护栏**：单测 `hello_avatar_is_capped_for_the_handshake_frame`（None/空串/正常/超限/正好等于上限
五种情形 + 源码断言 `build_signed_hello` 真的用了这个闸门）。
⚠️ 写这条测试时踩了个坑并已修：源码里有大量中文，**不能**按"起始 + 2000 字节"硬切字符串
（会切在多字节字符中间 panic），改成按顶层函数结尾的 `\n}\n` 取切片（与 `rust_fn_body` 同一判据）。

## [4.3.10] - 2026-09-14

### Fixed (🔴 BLE 链路"能收不能发"的僵尸态 —— 写失败一次就把写循环结束掉，只能重启)

用户 2026-09-13（安卓真机日志）：安卓与 Mac/Windows 的 BLE 会话都 `[SESSION] 已就绪`
（MTU 协商到 514 字节载荷），随后一阵 group gossip 进来，紧接着两条链路各出现一次
`[SEND] 写失败 ⇒ 结束该链路写循环`；**从那以后就再也发不出去**（界面报「发送失败，连接已关闭」），
而**读**还在持续正常收 —— 看门狗按**读**活性判健康（15s × 3 = 45s）⇒ 永远不拆这条链路
⇒ 只能重启应用才恢复。

**根因**：`ble_writer_loop` 在第一次写失败时直接 `break`，只结束了**写**循环，
链路仍登记在表里、**读**循环还活着 ⇒ 留下"能收不能发"的僵尸链路。
而 BLE 的写失败大多是**瞬态**的（对端 GATT 通知队列满 / 链路忙 / 连发被拒）。
旧日志还只打 `type=?`，连失败原因都没有，真机上完全无法定位。

**修法**（三处，都在 `ble_writer_loop`）：
1. **退避重试**：失败后 120ms 重试，最多 4 次（可被停机/取消打断）；只有"帧无法分片"
   这种**帧自身**的问题才不重试。
2. **最终失败 → 拆链路**：按端点去链路表取这一条的 `cancel` 并 `send(true)`，
   让**读**循环收尾执行 `teardown_link`（清链路 + 清该地址退避 + `wake_scan`）
   ⇒ 下一轮扫描即可重拨，不再留下半死链路。
3. **日志带上真实原因**（`原因={e}`）与重试次数。

**护栏**：`ble_write_failure_retries_then_tears_the_link_down`（源码断言：必须有重试上限、
最终失败必须走链路表的 `cancel.send(true)`、失败日志必须带原因）。

## [4.3.9] - 2026-09-14

### Fixed (Mac 主窗口 ⌘W 只会「滴滴滴」—— 关不掉，设置/日志窗口却正常)

用户 2026-09-13（Mac 真机）：「Command+W 关闭主窗口的功能失效了，它就一直"滴滴滴"。
设置、日志窗口 ⌘W 还是能关的，⌘Q 也正常，就是聊天主窗口关不掉。」

**根因**：窗口菜单里用的是**系统预定义**的关闭项（`PredefinedMenuItem::close_window`），
它的动作是 AppKit 的 `performClose:`，由系统**按窗口的 `Closable` 样式位校验可用性**。
而本项目为了自绘标题栏用了 `decorations: false` ⇒ 窗口是 Borderless（不含 `Closable`）
⇒ 这一项被判为**不可用** ⇒ 按下只有系统提示音，**而且没有任何日志**。
设置/日志窗口是有边框的普通窗口，`Closable` 位本来就在，所以它们一直正常 ——
这也解释了"为什么只有主窗口坏"。
（先前 `ce49e1f` 靠 setup 里 `win.set_closable(true)` 补位修复过同一症状；
那条依赖"AppKit 认补出来的样式位"，一旦不成立就退回"滴滴滴"且无从察觉。）

**修法**：窗口菜单改成**我们自己的**菜单项（`id=close-window`、`CmdOrCtrl+W`），
由 `on_menu_event` 直接处理 —— 不再经过 AppKit 的可用性校验，因此不可能再"被系统判为不可用"。
行为与「×」按钮、托盘一致：关掉**当前聚焦**的窗口（回落到主窗口），
各窗口自己的 `CloseRequested → 隐藏` 处理器照旧生效（设置/日志常驻窗口不会被销毁）。

**护栏**：`cmd_w_is_handled_by_our_own_menu_item`（源码断言：不许再用预定义关闭项、
必须有 `CmdOrCtrl+W` 的自定义项、事件处理必须关"当前聚焦窗口"并走 `close()`；
⚠️ 断言前先剥注释 —— 那段解释里恰好写着 `PredefinedMenuItem::close_window` 这个名字，
不剥注释会把"解释这个坑"误判成"又踩了这个坑"，本项目踩过这种假阳性）
+ `scripts/verify-guards.py` 对应用例（改回预定义项 ⇒ 必须 FAIL、恢复即 PASS）。

## [4.3.8] - 2026-09-14

### Changed (合并评审三项：vendor 用 `[patch.crates-io]`、退避改为缓增封顶、日志措辞纠错)

**① vendor 的接线方式改成 `[patch.crates-io]`**（评审建议）。
主依赖处恢复上游语义 `btleplug = { version = "0.13" }`，补丁来源挪到文件末尾的
`[patch.crates-io]` —— 这样版本约束仍然表达"我要 0.13 这条线"，将来升级只改主依赖的版本号，
不必在一堆注释里找那行 `path`。同时**裁掉不参与编译的部分**：
`.github/`、`docs/`、`scripts/`、`CLAUDE.md`、`Cargo.lock`、`Cargo.toml.orig`、
`.cargo_vcs_info.json`、`.cargo-ok`、`.gitignore`、以及 57.5 KB 的 `gradle-wrapper.jar`
（**176 → 152 文件，0.84 → 0.67 MB**）。
`examples/`、`tests/`、`test-peripheral/` **有意保留**：上游 `Cargo.toml` 里
`autoexamples = false` / `autotests = false` 且逐个显式声明了目标（4 个 `[[example]]`、
33 个 `[[test]]`），删掉目录而不重写这 37 处声明会让 Cargo 直接拒绝解析清单 ——
为了 0.2 MB 去改 37 行上游声明，维护风险大于收益。

⚠️ **构建缓存的代价（评审实测）**：换成 patch 会让 `btleplug` 的 fingerprint 变化 ⇒
**首次构建整包重编**（评审实测 `cargo check --features bluetooth` **13m28s**，
之前约 30s）。已写进 `Cargo.toml` 的注释里，免得 CI 上被当成"突然变慢的回归"。

**② 拨号退避改为缓增封顶 20s**（评审建议）。
上一版是"前 3 次不退、之后**固定 5s**"—— 评审判定为偏激进：对端长期不在（或根本不是
Gosslan 端）时会一直每轮都敲，而射频/功耗的代价**没有用户可见反馈**，只在电量上体现。
新形状：`1–3 次 ⇒ 0`（不退）、`第 4 次 ⇒ 5s`、`第 5 次 ⇒ 10s`、`第 6 次起 ⇒ 20s 封顶`。
20s 与 ~4s 的扫描周期同量级 ⇒ 最多跳 5 轮必定重试，不会重演"被退避锁到分钟级"那次故障。
护栏同步改为钉死这三段 + 封顶 + **单调不降**（防止冷却忽长忽短让日志里的剩余时间来回跳）。

**③ 日志措辞纠错**（评审指出）：
`[SESSION] 已就绪` 原写「**这个 ep 就是该设备对应的蓝牙地址**」—— 这句在
**Windows 外设链路**上不成立：那里的标识是 WinRT 的 `BluetoothDeviceId`（不是 MAC），
而 macOS 侧两个角色给的也一直是 UUID（central 侧小写、外设侧大写）。
新措辞：「ep 是**本机这一侧的链路标识**，central 侧=对端外设标识 / 外设侧=对端 central 标识，
两者不一定同串」，并说明它是排障用的坐标而非地址。

**关于"同一对端两个角色看到不同串"是否会造成重复端点**（评审提问，已查证）：
会形成两个不同的 `BleEndpoint` 字符串，但**不会因此多出一条 BLE 链路** ——
入站去重 `should_accept_inbound` 是按 **`PathKind`** 判的（不参考地址串），
指定的拨号方会拒掉镜像入站。真正的代价是**记账粒度**：拆链/去重要按串逐个对
（`detach_by_endpoint`），且 `MAX_LINKS_PER_PEER` 的余量会被多占一格。
本次不改判据（改它会动到 D6-2/D6-3 那条已被真机验证过的对称性），仅在此留档。

### Fixed (🔴 对方重装换过公钥后"必须重启"才能重新加好友 —— 现在删好友重新加即可)

用户 2026-09-13（两台电脑，局域网）：「Windows 清空数据重装后再加 Mac，聊天框提示对方换过公钥；
这时既收不到消息、也收不到好友申请；把好友删掉重新加也收不到，**必须重启一下**才能收到。」

**根因**（两处叠加，缺一不可）：
1. 身份表**只补空、不覆盖**（INV-P11：公钥冲突不静默覆盖）——这是**对的**，不该改；
2. 但 `verify_hello` 的绑定来源有**两条腿**：`friends` 表 + 内存 `peers` 表（广播里学来的、
   **未经验签**的公钥）。删好友只断了第一条腿，**内存那条旧公钥还在当信任根用**
   ⇒ Hello 继续被硬拒 ⇒ 消息与好友申请全都进不来；**只有重启**（内存清空）才回落到 TOFU。

**修法**：**解除关系就解除身份绑定**。
· 新增 `network::transport::forget_peer_identity()`（清 `peers` 表与 mesh `PeerManager` 里的公钥，
  **不动链路/昵称/IP**，并允许下次再提示一次）；
· 在 `remove_friend` 与收到 `FriendRemove`（对方删了我）**两条**路径上都调用；
· 密钥变化的系统提示与 Hello 被拒的原因都改成**可行动**：
  「若对方刚重装过应用：删掉这个好友再重新添加即可（聊天记录会保留、不用重启）；
  如果不是本人操作，就别继续」。
· 规格同步：`docs/protocol-invariants.md` 的 **INV-P11** 写下了这次的决定
  （好友表 = 硬拒；内存绑定 = 可解除），并列出两个更好的后续方案（非好友按验签结果重新绑定、
  并排展示旧/新指纹的"重新配对"对话框）。

**安全说明**：没有放松任何判据 —— 好友表那条硬绑定照旧（换了公钥必须先由用户显式解除关系），
只是让"用户显式解除"这件事真的生效。风险与缓解写在 INV-P11 与审计 §10。

**护栏**：`PeerManager::forget_identity` 单测（只清身份、链路不动、清完能重新绑定、未知 id 不 panic）+
`removing_a_friend_also_drops_the_in_memory_identity_binding`（源码断言：两条解除路径都要调、
提示必须写出可行动路径）。

## [4.3.7] - 2026-09-13

## [4.3.6] - 2026-09-13

### Fixed (已装的安卓仍然带着 `dev-` 前缀的旧 device_id —— 现在就地迁移，不必清数据)

Windows 那边把 `state.rs` 兜底路径里多套的那层 `dev-` 前缀去掉了（`dev-gosslan-…` ⇒
`gosslan-…`），但**只治新装设备**：已经写过库的设备里存的还是旧值，而 device_id 一旦
落库就持久化了。继续带着旧值的后果（真机 2026-09-13）：
`'d' < 'g'` ⇒ 那台安卓在三端里**恒为最小 id** ⇒ 按「大 id 拨、小 id 只接受」的镜像规则
**它永远不主动拨号** —— 只能等别人来连它，自己搜不到、也拨不动。
当时的结论是"让用户清一次应用数据"，而那会丢掉聊天与好友。

**修法**：读取时做**一次性就地迁移** —— `device::strip_legacy_dev_prefix()` 只剥这一层
已知前缀（剥完必须还是合法 `gosslan-…`，否则不动，避免把用户自己的 id 改坏），
迁移后立刻写回库。护栏：单测 `legacy_dev_prefix_is_stripped_exactly_once`
（含"只剥一层"与"非法就不动"两个负例）。

## [4.3.5] - 2026-09-13

### Fixed (Windows 的日常包一直没有蓝牙 —— 一键入口 `npm run dist` 漏了 `--features bluetooth`)

2026-09-13 合并评审（把 Windows 那边的 10 个提交 rebase 进来之后逐条 code review）发现的。

BLE 在 Cargo 里是**可选 feature**（ADR-0015 §2：不开时依赖不下载、代码不编译）。
Windows 那边已经给 `dist:win` / `dist:win:arm64` / `dist:win:msi` / 便携版脚本补了
`--features bluetooth`，**但一键入口 `scripts/package.mjs`（= `npm run dist`）漏了** ——
而按用户定的规则，`npm run dist` 才是日常出包的那条路（Windows 上只出当前环境的包）。
后果是**静默**的：构建成功、产物正常、只是那个包完全没有蓝牙。

**修法**：`scripts/package.mjs` 的 win32 分支补上 `--features bluetooth`。
**护栏**：`src/utils/buildConfig.test.ts` 新增一条 —— 扫描 `package.json` 里所有
`tauri build` 命令与 `scripts/package.mjs` 里的每一条 build 命令，少一个 feature 就 FAIL；
并进了 `scripts/verify-guards.py` 的非空转验证（改坏即 FAIL、恢复即 PASS）。

## [4.3.4] - 2026-09-13

### Fixed (安卓长按面板：点「引用」「转发」后面板还挂着 —— 现在点任何一项都收起)

用户 2026-09-13（Android）：「点击文字『引用』，这个 sheet 应该自动隐藏；点击『转发』应该也是
自动隐藏，因为它会跳转到界面内去操作聊天。」

**根因**：`MessageItem` 的 `doQuote` / `doForward` 只调了 `closeContextMenu()`（桌面右键菜单），
**没调** `closeActionSheet()`；而"点完收起"这件事原来是**每个按钮各写一遍**的
（复制 / 选择文字那两项写了），于是漏一个就漏一个 —— 同一批里「复制图片」「保存图片」
「保存文件」「复制文件」四项同样不会收起。

**修法**：把"点一项即收起"提到 `ActionSheet` 的**面板层**
（`<DialogPanel @click="emit('close')">`）—— 这是成熟产品（iOS ActionSheet / 微信 / Telegram
底部菜单）的通行行为，一处覆盖所有入口，以后新增入口也不会再漏。
另外给「引用」「转发」在 handler 里加了**第二道保险**（`closeActionSheet()`）：
它们是"跳到别处去操作"（引用草稿 / 转发弹窗），即使以后面板的通用规则变了，
也不该让面板留在跳转后的界面上面。

⚠️ 踩坑记录：面板层那段说明注释**必须写在 `TransitionChild as="template"` 外面** ——
放插槽里会多出一个注释节点，HeadlessUI 立刻抛 `Passing props on template!`
（`designGuards` 里现成的护栏在本次编辑时就抓到了，没有进到提交里）。

**护栏**：`src/utils/longPress.test.ts` 新增一条（面板**开标签**必须含收起、引用/转发 handler
必须自己收）；`scripts/verify-guards.py` 加了对应非空转用例。
⚠️ 这条用例第一次跑就**抓出护栏本身是空转的**：原来的断言拿整段
`<DialogPanel>…</DialogPanel>` 去匹配，而"取消"按钮自己也有 `@click="emit('close')"`
⇒ 把面板上的收起删掉照样通过。已改成只看**开标签**，重跑确认"改坏即 FAIL、恢复即 PASS"。

## [4.3.3] - 2026-09-13

### Fixed (🔴 外设侧重连后第一条消息会丢 —— 分片重组器带着上一轮连接的残留)

框架审计（`docs/notes/audit-2026-09-13-mesh-ble-efficiency.md`，用户优先级 ①「蓝牙设备加入
mesh 的稳定性」）里「§6-3 外设重连不清重组器」这一条。

**证据**：`bluetooth_peripheral.rs::did_unsubscribe` 会按 central 清掉它的分片重组器，
但 **`did_subscribe` 不会** —— 而 macOS 外设角色**没有** didDisconnect 回调、
`didUnsubscribe` 也不保证在断连时到达。对端的 `msg_id` 又**每条连接都从 1 重新开始**
⇒ 重连后第一帧的分片会和上一轮残留的半截消息撞在同一个 `msg_id` 上（分片数/序号对不上）
⇒ 那条帧被当坏片丢弃。`BleReassembler` 自带 30s TTL 能兜底，但真机重连通常就在**几秒内**
发生 —— 来不及。真机体感：**"断一下再连上，第一条消息发了对方收不到。"**

**修法**：`did_subscribe` 里**每一次订阅都视作新的"连接世代"**，按 central id `remove` 它的
重组器（⚠️ 只 remove 这一个，**不整体 clear** —— 那会误伤其它正在线的对端）。
Android 侧本来就在 `onUnlinked`（Kotlin 会真的回调）与启动时清，不受影响。

**护栏**：Rust 结构护栏 `peripheral_subscribe_resets_that_centrals_reassembler`
（`did_subscribe` 必须含 `reassemblers` 与 `.remove(&id)`），并进了
`scripts/verify-guards.py` 的非空转验证（改坏即 FAIL、恢复即 PASS）。

## [4.3.2] - 2026-09-13

### Fixed (🔴 安卓长按气泡：有的地方弹不出复制/转发面板，弹出来一放手又缩回去)

用户 2026-09-13（Android 实测）：「长按那个聊天的文字内容的气泡，有的时候弹不出来那个
复制/转发的 sheet，有的时候又能弹出来，然后你一放手，立马就缩回去了。」

两个独立的缺陷，各自都会让"长按 → 复制/转发"不可用：

1. **按在文字上长按完全不弹**（"有的时候弹不出来"）。
   正文那层 `<div>` 带着 `.gosslan-selectable`（"可选文本"标记，桌面端靠它选字），
   而 `onTouchStart` 的老判据是"命中 `.gosslan-selectable` 就不起长按定时器"
   —— 那个类在 DOM 上一直都在，于是**气泡绝大部分面积（文字）按下去毫无反应**，
   只有按到 `px-3 py-1.5` 那圈内边距才弹得出来。
   触屏下这块正文早已被 `@media (pointer: coarse)` 关掉选中（4.3.0 起"部分选字"改走
   菜单里的「选择文字」二级入口），所以这条"让路"在触屏上已经没有意义。
   **修法**：判据抽成纯函数 `utils/longPress.ts::shouldStartLongPress`，
   只有"**当前真的还能选字**的可选区域"（例如代码块）才继续让路；
   正文气泡（`.gosslan-bubble-text` 内的 `.gosslan-selectable`）不再让路。

2. **弹出来一放手就缩回去**（"你一放手立马就缩回去了"）。
   面板是 HeadlessUI `Dialog`，它的 `useOutsideClick` 在 **document 捕获阶段**挂了
   `touchend`，判据是"`touchend` 的 target 在不在对话框容器里"；而 touch 事件的 target
   在 **`touchstart` 那一刻就固定**成那条消息了 ⇒ **手指一抬必被判成"点了外面"** ⇒
   立刻 `@close`。所以它其实每次都会缩，只是"弹出来那一下"用户才看得见。
   **修法**：面板展开期间在 **window 捕获阶段**拦下这次 `touchend` 并 `preventDefault()`
   （HeadlessUI 的判据里有 `if (e.defaultPrevented) return`，这就够）。
   ⚠️ 必须挂 `window`：它挂的是 `document` 捕获，同阶段按注册顺序执行（它先注册），
   而捕获路径是 `window → document → … → target`，只有 `window` 抢得到它前面；
   顺带也杀掉了这次 tap 的合成 `click`（不会误触气泡里的链接）。
   规则同样抽成纯函数 `shouldSwallowLongPressRelease`（只有"面板是这次按压弹出的 +
   面板还开着"才吞，否则用户点遮罩关面板会被误吞）。面板关闭/组件卸载时摘掉监听。

**护栏**：新增 `src/utils/longPress.test.ts`（判据真值表 + `MessageItem` 的两条结构护栏：
必须走纯函数并传全语境、必须在 window 捕获阶段吞掉抬手且能摘掉监听）；
`scripts/verify-guards.py` 加一条非空转用例（把吞掉那段改成 `if (false)` ⇒ 必须 FAIL）。
顺带修掉 `designGuards.ts` 里一句已经过期的注释（`.gosslan-selectable` 不再是
"移动端长按让路给原生选字"的标记）。

## [4.3.1] - 2026-09-13

### Fixed (🔴 蓝牙开关点一下要等好几秒才动 —— 前端在等后端，后端在等 CoreBluetooth)

用户 2026-09-13（Mac 实测）：「蓝牙的开关是可以开和关的，但是点起来很卡。点了一下，
过了好一会儿才会关；再点一下，过了好一会儿才会开。」用户同时重申了本项目的一贯规则：
**所有这类操作都以"乐观更新"优先响应用户需求，再去底层执行；失败才 loading → 提示 → 回退数据。**

**为什么慢**（三段时间叠在一起，全都发生在"用户点下去"到"开关动起来"之间）：

1. **前端等 IPC**：开关的值取自通道状态，而 `setChannelEnabled` 是
   `applyRuntimeSnapshot(await api.setChannelEnabled(...))` —— 后端不返回，开关就不动。
2. **后端在等蓝牙栈**：`ble::start` 里 `start_peripheral` 要等 CoreBluetooth 回报状态
   （`peripheral::STATE_WAIT = 3s`）；`ble::stop` 要等扫描任务退出
   （`STOP_TIMEOUT = 2s`，而 `scan_peers` 一轮就是 `SCAN_WINDOW = 3s`），再逐条拆链路。
3. **3s 冷却会"丢弃"新意图**：冷却期内到达的请求只记一条 warn 就返回
   （`忽略高频蓝牙通道切换请求`）—— 用户"关一下马上又开"时第二次点击被静默吞掉，
   表现进一步恶化成"点了没反应"。

**修法**：

- **前端乐观更新**（`stores/useAppStore.ts`）：开关顺序固定成
  **先按用户意图改状态 → 再让后端执行 → 成功用权威快照收尾 / 失败回退并抛错**，
  期间挂 `channelPending`（开关滑杆上一枚小转圈 + `aria-busy`），失败由调用方 toast。
  同一个规则也补到 `startNetwork` / `stopNetwork`（网卡切换那条路径）。
  `SettingsToggle` 新增 `pending` 属性；设置页与「添加好友」页都接上；
  「添加好友」页不再用 `disabled` 表达 busy（`disabled` 会让开关停在旧值上，等于把乐观又抹掉）。
- **后端不再阻塞命令返回**（`network/ble.rs`）：外设角色本来"独立失败"（起不来只影响
  别人连我们），所以 `start()` 里改成 `tokio::spawn(start_peripheral(...))`，
  不再 `await` 那最多 3s 的状态回执。⚠️ 句柄改为**先写进 `state.ble` 再 spawn** ——
  否则"刚开就关"时 `stop()` 拿不到 handle、发不出停机信号，那个外设任务会永远活着。
- **冷却从"丢弃意图"改成"排队 + 最后一次意图胜出"**（`commands.rs::bt_switch_plan`）：
  决策抽成纯函数并单测 —— ① 已被更新的意图取代 ⇒ 什么都不做（那次会做）；
  ② 运行状态已是目标状态 ⇒ 幂等跳过（2026-09-12 那个"每秒十几次启停把蓝牙栈打满、
  整个应用顿卡"的抖动护栏照旧）；③ 距上次真实启停不足冷却 ⇒ **等够了再做**（`wait_ms`），
  不再丢弃。启停本身用一把 `tokio::sync::Mutex` 串行化，所以"关了又马上开"一定会在
  冷却结束后执行到开，不会丢。

**护栏**：`channelState.test.ts` 新增"通道开关必须乐观更新"（按源码顺序断言
乐观写入在 `await` 之前 + 失败回退 + pending 清理）；Rust 侧新增
`bt_switch_plan_coalesces_intent_and_keeps_the_cooldown`（6 个真值分支）与
`ble_start_does_not_block_on_the_peripheral_state_wait`（结构护栏）。
两条都进了 `scripts/verify-guards.py` 的非空转验证（改坏即 FAIL、恢复即 PASS）。
顺带修掉 `verify-guards.py` 一个坑：`--only` 写错时**一条都不跑却打印 ✅**，现在直接报错退出。

## [4.3.0] - 2026-09-13

### Fixed (🔴 外设侧握手失败后不解除"握手中"标记 ⇒ 那台设备再也加入不进 mesh)

2026-09-13 审计抓到的"加入不了 mesh"缺陷。

`network/ble.rs` 的外设事件循环用一个 `handshaking: HashSet<central>` 防止同一个 central
触发多次并行握手；但只有**两条路径**会把它摘掉：`RouteCtl::Add`（握手**成功**）与
`Unlinked`（对端退订）。**握手失败**（对端根本不是 Gosslan 端、Hello 验签不过、首帧异常…）
时**不摘** ⇒ 该 central 之后发来的**真 Hello 会被「已在握手」静默丢弃**
（`if handshaking.insert(...)` 返回 false 就不再起握手任务）⇒ **设备再也连不进来**。

真机上表现为：手机第一次连 Mac 没连上（或连上又断），之后**无论怎么重试都连不上**，
除非对端退订触发 `Unlinked`。而 **macOS 外设没有断连回调**，这个条目可能**永久残留**。

**修法**：新增 `RouteCtl::HandshakeFailed`，`accept_handshake` 改为返回
"是否真的建链成功"，**只在失败时**回传该控制消息，由事件循环摘掉标记。
（成功路径仍由 `Add` 清理 —— 若成功也回传，会与"刚起来的第二次握手"抢同一个标记，
把新握手的 `handshaking` 误清 ⇒ 同一 central 叠起多条握手。）

### Fixed (🔴 非成员中继不转发群消息 ⇒ 多跳 mesh 上「群聊永远不通、单聊却正常」)

2026-09-13 审计抓到的 blocker：`handle_gossip` 里"群信封只能被群成员消费"的判据被写成了
**提前 `return`**，位置在**第 4 步转发之前** ⇒ 只要中继节点不在这个群里，
A 发的群消息到它这里就被丢掉，永远到不了 C。

真机形态：**BLE-only 三个设备串成 A—B—C（手机↔电脑↔手机）时，群聊不通，而同一条链路上的单聊完全正常**
（单聊走定向 `target` 分支，不经过这条判据）—— 这正是"手机↔手机 mesh"在群聊上失效的原因。

**修法**：把判据抽成纯函数 `group_envelope_consumable`，**只决定"要不要本地消费"**；
非成员照样走第 4 步的转发，只是跳过第 5 步的本地处理（群密钥本来就不在手上，也解不开）。

**为什么"非成员转发"是安全的**：
- 群正文用群密钥对称加密，非成员只有密文（`plaintext = None`，不泄露任何内容）；
- 愿不愿意替别人转发由**中继授权（M4，`decide_forward`）**决定，不由这条判据决定；
- `sender` 必须也在成员表里 —— 签名只证明"是谁发的"，不证明"他有权把人拉进群"，
  所以伪造者广播的群信封本机仍然不消费。

**护栏**：真值表单测 `group_envelope_consumption_rule`（成员/非成员/伪造 sender/旧端空成员表/非群种类）
+ **结构护栏** `handle_gossip_does_not_bail_out_for_non_members`（源码断言：那句早期 `return`
一旦被加回来，多跳群聊会静默失效而**没有任何测试会失败**）。

### Fixed (🔴 BLE 上 >20KB 的文件永远传不完 ——「等 30s 确认」其实从第 1 秒就开始倒计时)

用户此前实测「手机给电脑发 500KB 图片，传很久，最后报分片相关的错」。2026-09-13 审计定位到
三个叠加的缺陷，本条一次修完：

1. **固定 30s 墙钟等错了对象**（`file.rs::stream_file`）。分块是**一次性全部入队**的
   （mpsc 容量 1024），而 `FileDone` 排在所有分块**后面**：1MB 文件在 BLE 上把 256 个分块
   在 **1 秒内**塞满队列，30s 只走得掉约 30KB ⇒ **必然超时** ⇒ `retryable` ⇒
   每 5s 心跳从头重传。
   **修法**：新增 `file_wire_progress`（transfer_id → 最近一次分块**真的离开链路**的时刻，
   由 TCP/BLE 两条 `writer_loop` 在写成功时刷新），把等待改成**安静 30s 才算失败** ——
   只要还有分块在往链路上走就一直等；真断链/真丢包仍然 30s 后失败，可靠性判据没有放松。
   判据必须落在"写出"而不是"入队"上：队列能装 1024 帧，入队 1 秒就完成，而链路上要跑几分钟。
2. **重传时接收方直接拒收**（`file.rs::make_receiver`）。同一个 `transfer_id` 再来一次 Offer
   = 发送方在重试，旧实现返回「重复的文件传输」⇒ 发送方 15s 等 accept 超时 ⇒ 再重试 ——
   **死循环**。**修法**：丢掉旧接收状态、从零重新开始（临时文件按 `transfer_id` 命名，
   `File::create` 会截断，不会与新 attempt 混写）。
3. **旧 attempt 的迟到分片会把整单打死**（`file.rs::write_chunk`）。旧实现 `seq != next_seq`
   一律报「**文件分片顺序错误**」—— 这正是用户看到的那条文案。**修法**：`seq < next_seq`
   （重复/迟到）**忽略**，只有 `seq > next_seq`（真跳号）才报错；整份字节仍由文件级
   SHA-256 兜底校验。规则抽成纯函数 `chunk_seq_decision` + 单测钉住。

### Fixed (群文件块大小没按链路选 ⇒ 群文件在 BLE 上 0 字节可达；BLE 断链后不立刻重拨)

两条都来自 2026-09-13 框架审计（`docs/notes/audit-2026-09-13-mesh-ble-efficiency.md`）：

1. **群文件在 BLE 上等于发不出去。** `commands.rs` 的群文件投递（`dispatch_group_file_to_peer`）
   一直用固定 `FILE_CHUNK = 256 KiB` 分块，而 256KiB 经 AEAD + base64 后约 350KB，
   在 MTU=23 的 BLE 上需要 ≈25000 片 > `MAX_BLE_CHUNKS_PER_MESSAGE`(8192)
   ⇒ `fragment()` 返回 `None` ⇒ **整帧被丢弃**（只留一条 warn），
   而发送方界面照旧显示"已发送"。单聊路径早已用 `chunk_size_for_path` 修掉同一个坑，
   这次把群文件补齐：按**该接收者的实际链路**（`inbound_path_kind`）选 4KiB / 256KiB。
2. **BLE 断链后不立刻重拨。** `network/ble.rs::teardown_link` 原来只清理链路，
   不唤醒扫描 ⇒ "重新发现对端"要等下一轮扫描：前台最多 5s、**后台最多 30s**，
   体感就是"断开后几十秒没反应"。现在断链即 `wake_scan`，并清掉该 BLE 地址的
   **失败退避**（退避是给"连不上"用的，刚断的这条本来是通的，不该被旧计数拖住）。

### Security (.gitignore 补上"签名私钥副本"—— 它此前可被 `git add -A` 提交)

`src-tauri/gen/android/app/release.keystore` 是注入脚本从仓库那把 keystore **解出来的副本**，
AGP 实际用它签名，而它**不在任何 `.gitignore` 里**（`git check-ignore` 返回"未忽略"）。
一次 `git add -A` 就会把**签名私钥**提交进公开仓库。已在根 `.gitignore` 补
`src-tauri/gen/android/app/release.keystore` 与 `app/*.keystore`，`git status` 里不再出现。
（只动 `.gitignore`，没有碰任何签名逻辑。）

### Changed (移动端长按改成成熟产品的模型：长按=菜单，「选择文字」是菜单里的二级入口)

用户 2026-09-13：「移动端长按气泡时，'没选文字'和'弹菜单'有点冲突，可以参考成熟产品怎么设计的」。

现状确实是"两头都不灵"：气泡正文既然是可选文本，长按就被系统抢去弹它自己的选择工具条；
而我们的长按定时器又要求在 500ms 内手指几乎不动 —— 真机上很难两全。

**改成成熟产品的通行模型**（微信 / Telegram / WhatsApp / iMessage 都是这个思路）：

| 产品 | 长按气泡 | 部分选字 |
|---|---|---|
| 微信 / WhatsApp | 弹菜单（复制=整条） | 不提供 |
| **Telegram** | 弹菜单 | 菜单里的 **Select Text** 进入选择模式 |
| iMessage | 弹菜单 | 再长按 / 双击进入 |

**本项目的做法（= Telegram 模型）**：

- **触屏下气泡默认不可选**（`@media (pointer: coarse)` 里 `.gosslan-bubble-text { user-select: none }`）
  ⇒ 长按**必定**是我们的消息菜单，不再和系统争手势；
- 菜单里新增 **「选择文字」**（仅文本消息）→ 本气泡切成 `.gosslan-selecting`（重新开放原生选字 +
  `-webkit-touch-callout: default`）并**自动全选**，系统工具条（复制/全选）随即弹出，
  用户再拖手柄精确调整；
- 选区一消失（点了别处 / 收起手柄）自动退出选择模式，避免状态残留导致"长按又弹不出菜单"；
- **桌面鼠标不受影响**：`pointer: fine` 命中不了那段媒体查询，仍是默认可拖选
  （并保留上一轮的内边距锚点 + 表情不可拖修复）。

i18n：新增 `common.selectText`（选择文字 / Select Text）。
`MessageTextBubble` 新增 `selectMode` 属性（进入时自动选中正文）；
`MessageItem` 新增 `textSelecting` 状态与 `selectionchange` 退出监听。

### Fixed (🔴 BLE 健康链路每 45s 被看门狗自己拆掉 —— 蓝牙"时好时坏"的根因)

2026-09-13 框架审计（`docs/notes/audit-2026-09-13-mesh-ble-efficiency.md`）抓到的最严重缺陷。

**证据链**：`ConnectionHealth` 的**读活性**只在建链时播种一次
（`transport.rs::register_connection` → `seed_connection_read_seen`），此后**只由读循环刷新** ——
TCP 侧确实每次都刷（`transport.rs` 的 `reader_loop` → `mark_conn_seen`），
而 **BLE 读循环（`network/ble.rs::ble_reader_loop`）一次都没调**。

**后果**：任何**健康**的蓝牙链路 —— 15s 后 `is_healthy` 判假（选路与镜像去重都按"不健康"处理）、
45s 被健康看门狗 `stale_connections` 当死链路**拆掉**，对端再拨回来、45s 后再拆，无限循环。
真机体感正是：**蓝牙时好时坏、加好友/消息过一会儿才到、大图传到一半失败**。

**修法**：读循环收到帧即回灌读活性。该函数同时服务 central（`BleReader`）与外设
（`ChannelSource`）两条路径，**一处调用覆盖两个方向**。

**顺带**：两侧各加一条 **MTU 协商结果日志**（central：`[GATT] MTU 协商结果 …`；
外设：`[GATT] 外设侧 MTU 协商结果 …`）。审计发现文档里的"MTU=23 ⇒ 1KB/s"一直是**猜测** ——
btleplug 实测是 macOS `maximumWriteValueLength+3`（≈185）、Android `requestMtu(517)`，
即真实载荷本应 182~512 字节；没有这条日志，"蓝牙到底多慢"根本无从判断。

**护栏**：Rust 单测 `ble_reader_loop_refreshes_read_activity`（源码断言：读循环里必须有
`mark_conn_seen`，漏了必 FAIL —— 这种退化不会编译失败、只会让链路自断）+
`verify-guards.py` 新增非空转用例。

### Added (框架审计报告 + 真机测试计划)

- `docs/notes/audit-2026-09-13-mesh-ble-efficiency.md`：按用户新优先级
  （BLE 加入 mesh 稳定性 / 聊天高效 / 手机↔手机 mesh）逐项审核，带 `文件:行号` 证据。
  结论摘要：框架齐备（三链路、多跳、中继授权、外部帧流水线都在主干），
  缺口集中在 ① BLE 链路生命周期 ② 弱链路吞吐的可观测性与节流 ③ 文件传输的固定超时
  ④ 群消息中转与跨跳补发。
- `docs/notes/device-test-plan-2026-09-13.md`：8 条真机测试，每条都写明"看哪个日志/数字"；
  T1（链路是否活过 45s）与 T2（真实 MTU/吞吐基准）是前提。

## [4.2.20] - 2026-09-13

### Fixed (🔴 安卓包签名不稳定 ⇒ `INSTALL_FAILED_UPDATE_INCOMPATIBLE`：不再回退 debug，签名配死)

用户真机：`adb install` 报 `INSTALL_FAILED_UPDATE_INCOMPATIBLE: signatures do not match`。
查证（`apksigner verify --print-certs` + `dumpsys package`）：我打出来的 4.2.19 APK 是
**Android Debug 签名**（`CN=Android Debug`，SHA-256 `D2:27:81:F7…`），而手机上已装的包是
**另一把钥匙**（`signatures=[7f46ed86]` ⇒ SHA-256 `86:ED:46:7F…`）。

**根因**：`scripts/inject-android-signing.mjs` 在缺少 `ANDROID_KEYSTORE_BASE64` 时会**静默
回退 Android debug 签名**，而 debug keystore 的位置随 `$HOME`/`$ANDROID_USER_HOME` 变化
⇒ 不同会话/机器打出来的包签名不同，以前只是恰好一致才没暴露。

**修法**（用户要求「算法配死，跟以前一样」）：

1. **固定本地 keystore**：`scripts/android/keystore/gosslan-release.keystore`（首次 `keytool`
   自动生成；路径与凭据**写死在脚本里**，与 `HOME`/`ANDROID_USER_HOME` 无关）⇒ 所有构建共用
   同一把钥匙；CI 仍可用 `ANDROID_KEYSTORE_BASE64` 覆盖；
2. **不再静默回退 debug**：只有显式 `--allow-debug-signing` 才允许；
3. **构建脚本把证书钉住**：打印 `DN` + `SHA-256`，检测到 `CN=Android Debug` 直接让构建失败
   （除非 `GOSSLAN_ALLOW_DEBUG_SIGNING=1`）——这类退化以前只会以"装不上"的形式暴露；
4. `.gitignore` 排除 keystore 目录（**私钥绝不入库**）。

实测：重建后 APK 证书 `DN=CN=Gosslan, O=Gosslan, C=CN`、SHA-256 `05:EC:D5:40…`（稳定）。
⚠️ 手机上当前装的是**旧钥匙**的包：要么提供原来的 `ANDROID_KEYSTORE_BASE64`（我固化到固定路径），
要么**卸载一次**（丢本机数据）后再装 —— 之后不会再变。

### Fixed (文本选择：PC 拖选气泡不再"刚选中就取消"；移动端选中文字能弹「复制」；头像不可选)

用户 2026-09-13 报了三件事：

1. **PC**：右键气泡能复制整条文本，但**鼠标拖选不行 —— 刚选中立刻被取消**。
2. **移动端**：(a) 长按菜单（右键气泡）不好用；(b) 选中文本后**不弹「复制/全选」工具条**。
3. **头像不该被选中**（将来会有点击事件，但依然不能选择）；移动端**长按与"长按选字"互相打架**。

逐条根因与修法：

| 现象 | 根因 | 修法 |
|---|---|---|
| 拖选刚选中就取消 | 正文被 `px-3 py-1.5` 内边距包着，从内边距/气泡边缘起拖时**选区锚点落在不可选区域**，WebKit 立刻收敛选区 | `MessageTextBubble` 气泡根加 `select-text`（只让"可选中"，**不**加 `.gosslan-selectable`——那是长按让路标记） |
| 拖选划过表情就中断 | 正文表情是 `<img>`，浏览器**默认允许拖图**，拖选划过去就变成拖图片 | `.emoji-img` 加 `-webkit-user-drag: none` + 模板 `draggable="false"`。⚠️ 刻意**不加** `user-select: none`：那会让复制选区时丢掉表情（`alt` 是用户可见文本） |
| 移动端选中后没有「复制」工具条 | ① `button,[role=button]` 的 `-webkit-touch-callout: none` 会盖到正文；② 全局 `contextmenu` **无条件** `preventDefault()`，把选区的系统菜单也吃了（Android WebView 的选择工具条依赖它的默认行为） | `.gosslan-selectable` 显式 `-webkit-touch-callout: default`；`App.vue` 的 `contextmenu` 改为**有非空选区时放行系统菜单**，其余仍屏蔽 |
| 头像能被拖进选区 | 头像容器没有 `user-select` 约束（`button,[role=button]` 只覆盖按钮形态） | `.gosslan-avatar-box`（13 个头像调用点都带这个类）加 `user-select: none`；头像 `<img>` 加 `draggable="false"` |
| 移动端长按"不太灵" | `@touchmove` **直接绑 `cancelLongPress`** —— 手指动 1px 就取消，真机上几乎不可能"完全不动地按住 500ms" | 改绑 `onTouchMove`：**12px 抖动容差**；到点加一次 `haptic("heavy")` 触觉反馈（"到点了"必须有明确反馈） |

**护栏**：新增 `checkSelectionContract`（`utils/designGuards.ts`）+ 真值对用例（`designGuards.test.ts`：
"同一份源码既要有 `gosslan-selectable`，又**不能**把气泡根标成它"，以及"`.emoji-img` 不许出现
`user-select: none`"这类反向约束）。`verify-guards.py` 新增用例「聊天区文本选择契约」并已验证
**改坏即 FAIL、恢复即 PASS**。

⚠️ 仍待用户确认：「PC 上拖选立刻取消」我只复现到了**结构性**成因（内边距锚点 + 图片可拖），
如果修完仍复现，需要知道①系统是 Windows 还是 macOS、②纯文字（无表情）消息是否同样复现 ——
macOS 15 的 WKWebView 有一条已知的选区回归，Windows 上则可能是 `tauri.conf.json` 里
`dragDropEnabled: true` 的原生拖放注册在抢手势。

### Changed (打包提速：一键 `npm run dist`，mac 上安卓+mac **并行**、Windows 只出当前环境的包)

用户 2026-09-13：「Mac 端要打一个安卓包和一个 Mac 包，**要并行、不要串行**，尽可能优化打包时间；
Windows 端就只打当前环境适配的那个包。」

新增 `scripts/package.mjs`（`npm run dist`，平台自动判定）与 `scripts/frontend-build.mjs`。
原来慢在 5 处，逐个消掉：

| 优化 | 原来 | 现在 |
|---|---|---|
| 前端构建 | mac + 每个 ABI 各跑一遍 `vue-tsc + vite`（2~3 遍） | **只跑一遍**；其余 tauri 进程由 `GOSSLAN_SKIP_FRONTEND=1` 短路钩子 |
| macOS bundling | `targets: "all"` ⇒ 每次做 DMG（分钟级） | 默认只出 `.app` + zip；`--dmg` 才出 |
| Android ABI | 每次两个 ABI（两次完整 release 构建） | 默认只 arm64-v8a；`--all-abis` 才两个 |
| 并行 | mac 与安卓串行 —— cargo 对 target 目录加**独占锁**（实测并发时打印 `Blocking waiting for file lock on build directory`） | 安卓用独立 `CARGO_TARGET_DIR=src-tauri/target-android`，两个构建**真正并行** |
| release profile | `lto = true` + `codegen-units = 1`（体积最优、编译最慢） | 默认 `thin` LTO + 16 CGU；`--fat-lto` 回到发布级 |

其它：`--dry-run` 可先看命令；`--serial` 回退串行（复用旧缓存）；`--migrate-cache` 一次性把旧
`target/` 里的安卓产物搬进 `target-android/`（同盘 rename，秒级）；结束时打印每个任务的用时汇总。
`npm run build` 改为走 `scripts/frontend-build.mjs`（保留 `vue-tsc + vite` 两步与耗时输出，
只是多了一个"跳过"开关）。`.gitignore` 加 `src-tauri/target-android`。

**本机实测（macOS arm64 / 8 核，`npm run dist`）**：

| 场景 | mac | android | 总耗时 |
|---|---|---|---|
| 首次（profile 换了 ⇒ 两个 target 全量重编） | 819.1s | 744.8s（Rust 已完成） | **820.9s**（串行约 1564s） |
| 稳态（缓存都在、只改了前端） | 133.6s | 223.4s | **234.6s**（前端只跑一次 11.2s） |

产物：`release-artifacts/macos/gosslan-4.2.19-aarch64-apple-darwin.app.zip`（6.6M）与
`release-artifacts/android/gosslan-4.2.19-arm64-v8a-release.apk`（13M，含 apksigner / 单 ABI /
btleplug Java 类 / 包内前端一致性四项既有校验）。日志里能看到
`[frontend] GOSSLAN_SKIP_FRONTEND=1 ⇒ 复用已构建的 dist/` —— 前端确实只构建了一次。

### Added (默认头像取字规则升级：英文取前 4 字母、中文取首字、中英混排有明确截断)

用户 2026-09-13 提出：个人信息页等所有「默认头像」都是用户名生成的，取字规则应当更明确。
旧规则只有一条「取首字符大写」（`avatarInitial`），英文名只出一个字母，和用户预期不符。

**新规则（`src/utils/color.ts::avatarInitial`，全部按码点取、字母转大写、空名兜底 `?`）**：

| 用户名 | 结果 | 规则 |
|---|---|---|
| `zhou` | `ZHOU` | 纯英文 ⇒ 前 4 个字母 |
| `周工` | `周` | 纯中文 ⇒ 首字 |
| `周san` | `周` | 中文开头，后面是英文 ⇒ 仍是首字 |
| `a中` | `A中` | 字母 + 中文，字母 1 个 ⇒ **字母 + 一个中文**（用户澄清） |
| `ab中` | `AB中` | 字母 + 中文，字母 2 个 ⇒ 两个字母 + 一个中文 |
| `abc中` | `ABC` | 字母 + 中文，字母 3 个 ⇒ 不加中文，只截字母 |
| `abcde中` | `ABCD` | 字母超过 4 个 ⇒ 前 4 个字母 |
| `John Smith` / `lee_2` | `JOHN` / `LEE` | 只对「中英混排」特判，其余归入英文那一档 |
| `👍周工` | `👍` | 非 ASCII 字母开头（emoji / 数字 / 符号）⇒ 首字符 |

**渲染适配**：3~4 个字（如 `ZHOU`）在旧字号下会撑破头像圆。新增 `.gosslan-avatar-box`
（`container-type: inline-size`）与 `.gosslan-avatar-initial[data-len]`，用 `cqw` 让字号
随**头像框宽度**缩放 —— 16px 的已读小头像与 64px 的资料页大头像自动各自合适，
不必在 13 个调用点各写一份字号；1~2 个字沿用原字号（零视觉变化），
不支持容器查询的旧 WebView 回落到原字号（顶多略挤，不会看不见）。
13 个渲染点全部接上（消息流 / 会话列表 / 通讯录 / 群成员 / 转发弹窗 / 已读回执 /
@提及 / 侧栏 / 资料页 / 添加好友 / 群九宫格）。

**护栏**：`color.test.ts` 新增中英混排真值表 + `avatarInitialLen`（按**渲染结果**数字数，
不是原始用户名长度）用例。

### Added (蓝牙链路聊天框加传输速度提示)

用户 2026-09-13 实测「电脑给手机发图片，500K 传了很久」。真因是 BLE 分片载荷受
20 字节 MTU 限制，实测吞吐只有 **~1 KB/s** 量级（见 4.2.17 的 CHANGELOG），
一张 500 KB 的图片要几分钟 —— 用户不知道这个量级，只会以为卡死。

`ChatWindow.vue` 在**当前单聊真的走在蓝牙链路**时，于头部下方显示一条可关闭的提示
（文案进 i18n）：`蓝牙直连较慢（约 1 KB/s），大图片/文件可能要几分钟；传大文件建议双方连同一个 Wi-Fi。`
判据取**在线节点表的实时链路**（`peer.link`），而不是 `get_conv_link`（那是"最近一条消息
走的路径"的快照，可能早已切链路）；切会话后提示重新出现。

### Added (BLE 坏分片不再静默：丢了几片、最近原因是什么，进日志)

用户 2026-09-13 真机（手机→电脑发图片）提到过"分片顺序错误"。`BleReassembler::push`
对重复 / 越界 / 分片数不一致 / 超上限的坏片只返回 `Dropped(&str)`，**central 侧这一路原来是
静默 `continue`** —— 真机上只看到"图片没到"，看不到"到了、但被分片层丢了、原因是什么"。
现在 `BleReader` 记下累计丢弃条数与最近原因，读循环在计数增加时打一条
`[FRAG] 丢弃分片 N 片（新增 M，最近原因：…）`（只增才打，不会刷屏）。
外设侧（帧在各自驱动里重组）暂未覆盖 —— 那里拿不到这个计数，`FrameSource::frag_drops`
返回 `None` 时读循环什么都不打。

### Changed (网络诊断重做：蓝牙有自己的状态，不再被判 offline；网卡候选加入蓝牙)

用户 2026-09-13：「网络诊断里，如果是蓝牙用户（纯蓝牙或局域网+蓝牙），应该有相应的信息
可以看出来并标注，不像现在还是 offline。网卡-候选其实也可以加上蓝牙。这个组件重新设计一下。」

- **后端**：`DiscoveryDiag` 新增 `bluetooth: BleDiag`（feature_compiled / enabled / available /
  running / peers / **当前扫描节奏** / 扫描窗口与间隔 / 最近一轮扫描的 `收到广播数 / 本应用数` /
  失败退避明细 / 不拨名单）。扫码统计由 `scan_loop` 写入 `state.ble_scan`。
- **候选链路**：`InterfaceCandidate` 增加 `kind`（`lan` / `bluetooth`）与 `detail`；蓝牙作为
  **一条候选**进同一张表，状态用一句人话说清（`运行中 · 前台节奏（3s 扫描 / 5s 间隔）· 1 个对端`），
  网卡的 IP/广播/RFC1918/虚拟网卡字段对蓝牙一律不适用。
- **前端**：`DevDiagPanel.vue` 整块重做 —— 一条通道一张卡（局域网 / 蓝牙各自说自己的状态）、
  候选链路按 `kind` 渲染成卡片列表（窄屏不横向滚动）、局域网没开时明确写「局域网未开启
  （不影响蓝牙通道）」并隐藏发现细节、新增蓝牙退避明细区。

### Changed (「最近事件」合并进运行日志，诊断面板不再单列)

用户 2026-09-13：「最近事件这块其实可以移到日志里……如果没办法让我们 debug、没什么意义
的话也可以去掉。」

`AppState::push_diag_event` 从「写 50 条内存环形缓冲」改为**直接进运行日志**
（可搜索 / 可复制 / 可落盘），`DiscoveryDiag.recent_events` 与面板的事件区一并删除。
**不是无脑全打**（日志规范明确不记高频循环）：`discovery_started` / `*_error` 落日志，
纯心跳的 `announce_recv` / `broadcast_sent` / `multicast_sent` / `who_has_sent` 直接丢弃 ——
它们每 5~10s 一条，打进去几分钟就把 500 条内存缓冲冲干净，真问题反而被淹没。
`hello_rejected` / `hello_mismatch` / `identity_key_conflict` 也**不重复打**：它们的每个调用点
旁边本来就有一条上下文更完整的 `logger.warn`（用户的诉求是"别单开一块"，而这些早已在日志里）。

### Changed (蓝牙扫描按前台/失焦分级刷新率；点「添加好友」立刻补扫)

用户 2026-09-13：「APP 在前台可以提高刷新率，后台降低扫描率，被杀掉直接关掉；
PC 端窗口聚焦就提高刷新率，窗口关闭或不在聚焦那一层就降低。」

- 扫描节奏从固定 10s 改为**自适应**：前台/聚焦 **5s** 一轮（发现更快、加好友不用干等），
  后台/失焦 **30s** 一轮（射频占空比 3/5 → 3/30，省电）；「被杀掉直接关掉」不需要代码 ——
  进程没了扫描任务自然不存在。
- 新增命令 `set_app_active`：`App.vue` 在 `visibilitychange` / `focus` / `blur` 时上报
  （**移动端不看 focus/blur**：软键盘与系统弹框会误触发 blur）；从后台切回前台时后端
  `wake` 一次扫描，立刻补一轮而不是等完慢周期。
- `search_nearby_peers`（打开「添加好友」）顺带唤醒蓝牙扫描一轮；**不等待** BLE 结果
  （一轮扫描窗口 3s，等它会把弹窗卡住），新对端通过 `peers-updated` 自己冒出来。
- 诊断面板实时显示当前节奏（前台/后台、5s/30s），用户看得到策略在生效。

## [4.2.19] - 2026-09-13


### Changed (连接信息按链路类型显示：蓝牙不再显示"IP 地址：—"，设备类型不再显示英文原值)

用户 2026-09-13 提出：「个人信息里的设备类型、IP 地址这一块，如果是蓝牙的话，你看怎么样
显示比较合适？不同网络连接进来的设备，应该标注的信息是不一样的。」

现状确实不对：资料页**无论什么链路**都有一行 `IP 地址：—`（蓝牙链路上根本没有 IP 这个概念），
设备类型直接显示后端的 `desktop` / `mobile` 英文原值；「添加好友」列表里也是"有 IP 就显示
IP、否则显示 —"。

**约定（一条链路只说它真有的事实）** —— 新增 `src/utils/peerConnectionInfo.ts`（纯函数）：

| 链路 | 连接方式 | 地址行 |
|---|---|---|
| 蓝牙直连 | 蓝牙直连（近距离） | **不显示**（蓝牙没有 IP 概念） |
| 同一局域网 | 同一局域网 | `192.168.31.32:59992` |
| 跨网段 / VPN | 跨网段 / VPN | `100.101.221.60:59992` |
| 经中继（hop ≥ 1） | 经 N 跳中继 | **不显示**（只有跳数，没有直连地址；写了就是编） |
| 只发现未建链 | 已发现（还没建链） | 有真实 IP 才显示 |
| 设备类型 | desktop → 电脑、mobile → 手机、其余 → 未知设备 | |

**接线**：`FriendProfile.vue`（头部"连接方式 · 地址"、地址行 `v-if`、设备类型本地化）与
`AddFriendModal.vue`（列表文案）**共用同一份判据** —— 两处不可能再各说各话。
中继跳数取自与聊天头部徽标**同一个来源** `get_conv_link`，不另造一份状态。

**护栏**：`peerConnectionInfo.test.ts`（6 条真值表：蓝牙隐藏 IP／LAN 给 ip:port／
Routed 标签／中继只报跳数／未建链／设备类型映射）+ 更新 ⑤「不得用『没有 IP』反推蓝牙」到
新的唯一判据 + `verify-guards.py` 新用例（把蓝牙分支改成 `if (false)` 必须 FAIL），
前端子集 28/28 通过。

**门禁**：`npm test` 356/356 · `npx vue-tsc --noEmit` 通过 · `npx vite build` 通过 ·
`verify-guards --only frontend` 28/28。

## [4.2.18] - 2026-09-13

### Fixed (🔴 大图片「两边都显示已发送/已读、对方列表里却没有」：分块超出 BLE 分片上限，一帧打死整条链路)

用户 4.2.17 真机（纯蓝牙）：**文字聊天与加好友都通了**，但大图片两边都显示成功、双方都看到
已读，接收侧列表里却没有这张图。日志把三个结构性缺陷一次暴露：

```
[SEND] type=file_chunk transfer=…… bytes=282900 分片=…      ← 一块 256 KiB
[SEND] 写失败 ⇒ 结束该链路写循环 … type=file_chunk transfer=…  ← 链路被这一帧打死
[RECV] type=file_offer → [file] 接收文件初始化失败: 重复的文件传输
[SEND] type=file_reject                                       ← 重发被拒 ⇒ 对端停止重试
```

**根因（三条，缺一条都不会好）**：

1. **分块大小与链路能力不匹配**：一对一文件流每块默认 **256 KiB**，在 MTU=23 的 BLE 上需要
   ⌈262144/14⌉ = **18725 个分片**，而 BLE 分片层上限是 `MAX_BLE_CHUNKS_PER_MESSAGE` = 8192
   ⇒ `fragment()` 返回 `None` ⇒ 写循环把它当**写失败**并**拆掉整条链路**（连带把好友请求、
   消息一起打断）。
2. **一帧的问题被升级成链路问题**：上面那次拆分让同一条连接上的其它传输全部失败。
3. **重复的 `FileOffer` 被 reject**：对端没收到 accept 会重发同一个 `transfer_id`，而接收侧
   回 `FileReject("重复的文件传输")` ⇒ 对端判定失败、**停止重试** ⇒ 文件永远到不了。
   加上「文件流只要最终校验失败就整份重来」，而大图在 BLE 上要几分钟 ⇒ 表面"成功"、
   实际永远差一块。

**修法**（只改文件接入与 BLE 写循环，Frozen Core 语义零改动）：

- `file::chunk_size_for_path(path_kind)`：按**实际选路结果**决定分块 —— Bluetooth 用
  `BLE_FILE_CHUNK = 4 KiB`（293 片，距上限 28× 余量；非蓝牙仍用 256 KiB 保吞吐）；
  `stream_file` 用它切块；
- BLE 写循环遇到 `帧无法分片` **只丢这一帧**并留 warn，不拆链路（真正的写失败仍然拆）；
- 接收侧遇到重复 `FileOffer` **幂等回 `FileAccept`**（不再 reject），让对端把剩下的分片发完。

**护栏**：行为级 `ble_file_chunk_actually_fits_the_ble_fragment_layer`（把两种分块大小真的
喂给 `fragment()`：4 KiB 必须成功且余量 ≥4×，256 KiB 必须失败 —— 把 bug 成因钉在测试里）+
源码级 `ble_file_transfer_respects_link_limits`（分块按选路、丢帧不拆链、重复 offer 幂等）+
`verify-guards.py` 对应用例，现共 **60** 条。

**门禁**：`cargo test --lib --features bluetooth` 429/429 · `npm test` 350/350 ·
`check-mobile --bluetooth` PASS(0 warning) · `verify-guards` 60/60。

> 仍未解决：BLE 的 MTU 只有 23（20 字节载荷）⇒ 1.2 KB/s 级别的吞吐，大图仍需数分钟；
> 下一步是让 Mac 侧把 MTU 谈大（或按 `onNotificationSent` 做流控替代固定 12ms 节流）。

## [4.2.17] - 2026-09-13

### Fixed (🔴 好友申请到了安卓、Mac 却什么都收不到：Android 外设**连发通知丢片**)

用户 4.2.16 真机：Mac 加安卓 → **安卓收到了、好友也加上了**，但 Mac 侧没反应。日志给出了
算术级的证据：

```
Mac：[SESSION] 已就绪 peer=dev-gosslan-…             ← 连接/握手都成功了
     [SEND] type=gossip kind=FriendRequest bytes=742
     [FRAG] 收到通知 38 条 / 747 字节（非本特征 0 条）  ← 分片到了，但永远拼不出完整帧
     （整段日志里一个 [RECV] 都没有）
```

一个 **742 字节**的帧，在 MTU=23（ATT 头 3 + 分片头 6 ⇒ 每片 14 字节载荷）下需要
**⌈742/14⌉ = 53 片**；而 Mac 只收到 **38 片** ⇒ **丢 15 片** ⇒ 重组器永远等不到完整帧
⇒ `handle_message` 从不执行 ⇒ Mac 既看不到好友、也看不到任何消息。

**根因**：Android 的 `notifyCharacteristicChanged` **连发会被协议栈丢包**（发送缓冲有限），
而 `send()` 返回 `true` 只代表**调用被接受**，不代表已上线 —— 所以安卓侧日志全是"成功"。

**修法**：`transport/ble_android.rs` 的 `PeripheralWriter::send_frame` 在**每片之间**
`sleep(NOTIFY_CHUNK_INTERVAL = 12ms)`（≈ 一个连接间隔；最后一片不等）。
配套把**发出的分片数**写进日志（`[SEND] … 分片=53`），与对端的 `[FRAG] 收到通知 N 条`
一比即可判定"是发少了还是收丢了"——这次正是靠这两个数字对不上才定位到的。

**护栏**：`android_peripheral_paces_its_notifications`（常量存在 + 循环里真的 sleep +
最后一片不再等 + 发送侧必须打分片数）+ `verify-guards.py` 对应用例，现共 **59** 条。

**门禁**：`cargo test --lib --features bluetooth` 427/427 · `npm test` 350/350 ·
`check-mobile --bluetooth` PASS(0 warning) · `verify-guards` 59/59。

## [4.2.16] - 2026-09-13

### Fixed (🔴 Mac↔Android BLE「安卓发出去了、Mac 一个字节都收不到」：同一对端叠了多条连接)

用户 4.2.15 真机（只开蓝牙）双方日志对照，问题被夹到唯一一条链路上：

```
安卓侧：对端已订阅通知（50:A6:D8:AE:B2:69）
        [SESSION] 已就绪（外设侧）peer=gosslan-0672402a0eef460a
        [SEND] type=gossip kind=FriendRequest … bytes=738      ← 反复发，全部"成功"
Mac 侧：10:30:25 [GATT] 已就绪 → 10:30:41 [DISCONNECT] 握手超时：对端未回 Hello（丢掉了 0 个前导帧）
        10:30:38 [GATT] 已就绪 → 10:30:50 同上                    ← 一个 [RECV]/[FRAME] 都没有
```

**根因**：**同一个对端上叠了多条 BLE 连接**。

- 扫描每 10s 一轮，而一次连接+握手最长 10s ⇒ 每一轮扫描都会为**同一个**外设再起一个
  `dial_and_register` 任务（原先只按"已登记的端点"去重，**在途拨号不去重**）；
- 握手失败后**从不 `disconnect()`** —— drop 一个 btleplug `Peripheral` **不会**断开
  CoreBluetooth 连接 ⇒ 每失败一次就多留一条"已经没人读"的连接 + 一个通知流订阅；
- 于是 Mac 侧同时挂着 2~3 条连接、2~3 个通知订阅。Android 的 GATT server 对同一地址
  **只保留最后一条连接**，通知被投给那条时，读它的任务可能早已 `返回`（超时退出）
  ⇒ Mac 收不到任何分片，而安卓侧 `notifyCharacteristicChanged` 全部返回成功。

**修法**（最小、只碰 BLE 接入链）：

1. **在途拨号去重**：复用 TCP 侧已有的 `DialGuard`（RAII，Drop 即释放），键
   `ble:<外设 id>` ⇒ 同一外设同时只允许一个拨号任务；
2. **复用旧连接前先断开**：`is_connected() == true` 时先 `disconnect()` 再连，
   避免 `connect()` 复用幽灵连接（新订阅的通知流收不到任何东西）；
3. **失败路径显式断开**：握手/登记阶段拆成 `finish_dial()`，`Err` 时统一
   `peripheral.disconnect()`；
4. **分片级可见性**（诊断，用户要求）：`BleReader` 统计收到的通知条数/字节数，
   读循环空闲窗口打 `[FRAG] 收到通知 N 条 / M 字节`；安卓侧新增
   **notify 调用/协议栈确认/MTU** 三类日志（`onNotificationSent` + `onMtuChanged`），
   一眼区分"没发出去"与"发出去没收到"。

**护栏**：源码级 `ble_dial_is_deduplicated_and_disconnects_on_failure`（DialGuard 去重、
复用前断开、失败显式断开、分片统计必须存在）+ `verify-guards.py` 对应用例，现共 **58** 条。

**门禁**：`cargo test --lib --features bluetooth` 426/426 · `npm test` 350/350 ·
`check-mobile --bluetooth` PASS(0 warning) · `verify-guards` 58/58。

## [4.2.15] - 2026-09-13

### Fixed (🔴 Mac↔Android BLE「能发现、连不上」的真因：新连接的第一帧是**上一条链路的残留帧**)

用户 4.2.14 真机（只开蓝牙）新日志给出了决定性证据 —— Mac 侧反复出现：

```
00:53:55 [GATT] 已就绪 ep=368f5c3f-… → [DISCONNECT] 对端首帧不是 Hello（收到 chat_message）
01:05:07 [GATT] 已就绪 ep=ec59937a-… → [DISCONNECT] 对端首帧不是 Hello（收到 chat_message）
00:56:18 [SESSION] 已就绪（同一对端）→ [SEND] gossip + chat_message → [RECV] chat_message
```

**根因**：Android 的 `notifyCharacteristicChanged` 是**按 central 地址**投递的 —— 上一条链路
排队待发的帧（outbox flush / 心跳）会落在**新连接**上。而两边握手都要求"**第一帧必须是
Hello**"，于是这些残留帧把**本来能建起来的链路**全部打死：Mac 放弃 → 重拨 → 对端又在新连接上
先吐出残留帧 → 再次放弃，双方互相打断，好友申请与消息因此全部过期或延迟数分钟。

注意这不是"握手机制错了"：它是一道**安全边界**（身份只能来自 Hello 的签名验证）。
所以修法是**容忍前导帧、但绝不提前处理它们**：

- central 侧新增 `read_hello_frame()`：窗口内继续读，**丢弃**非 Hello 前导帧（不喂给
  `handle_message` —— 验签之前它只是字节），读到 Hello 再握手；额度
  `MAX_HANDSHAKE_PREAMBLE_FRAMES = 32`，用尽仍明确报错并带上最后一帧类型；
- 外设侧同一条语义：没有活路由时收到非 Hello 帧 ⇒ 记一条 `[SESSION] 丢弃外设侧握手前导帧`
  并继续等 Hello（既不投旧路由，也不当握手首帧）；
- 判定抽成纯函数 `preamble_action(dropped, is_hello)`（可单测 + 护栏）。

### Fixed (Android 日志：**每条日志 fork 一个进程** ⇒ 一边跑 GATT 一边 fork 风暴)

真机抓到的第二个问题：`logging.rs` 原来用 `Command::new("log").spawn()` 镜像到 logcat ——
**每行一次 fork+exec**（`adb logcat -s gosslan` 里每行 PID 都不同就是铁证）。
BLE 生命周期日志一多（每帧一条 `[SEND]/[RECV]`），真机上就变成"跑 GATT 的同时疯狂 fork"，
直接拖慢 Rust 运行时与蓝牙时序。现在改成**队列 + 常驻线程 + 200ms 合批**，
每批只 fork 一次（整批作为一条多行消息发出），logcat 里依然逐行可见；
行级精确时间戳仍完整保存在落盘日志与内存日志里。

### Changed (诊断：Gossip 帧必须打出 kind)

`frame_trace` 以前对 `Message::Gossip` 只打 `type=gossip`，**分不清好友申请到底发出去没有**
（真机排查正是卡在这里）。现在打 `type=gossip kind=FriendRequest/FriendAccept/...`。

**护栏**：`handshake_tolerates_leading_non_hello_frames_but_is_bounded` +
源码级 `ble_handshake_skips_leading_frames_without_processing_them`（必须走
`read_hello_frame`、前导帧不得进 `handle_message`、外设侧也要丢）+ `verify-guards.py`
对应非空转用例，现共 **57** 条。

**门禁**：`cargo test --lib --features bluetooth` 425/425 · `npm test` 350/350 ·
`check-mobile --bluetooth` PASS(0 warning) · `verify-guards` 57/57。

## [4.2.14] - 2026-09-13

### Fixed (BLE 首轮稳定性：好友申请等 5～6 分钟、同意后对端状态不同步)

用户真机（Mac ↔ Android、**只开蓝牙**）：发现要等一会儿、发现后"未连接"；
最严重一次**好友申请 5～6 分钟才到**；Android 接受后 **Mac 端好友状态一直没同步**；
之后发消息长期停在"发送中"。

**审计结论**（完整版见 `docs/notes/ble-audit-2026-09-13.md`，含 20 问逐条答案）：

1. **5～6 分钟的主因 = 拨号退避上限 10 分钟**。`ble_dial_backoff_ms` 旧值是
   `60s→120s→240s→480s→600s`，按 **BLE 地址**记账，只有"我们拨成功"才清零；
   而 `should_dial_ble` 的规则是"大 id 拨、小 id 只接受"⇒ **只有一侧会拨**——
   唯一的拨号通道一旦连续失败（BLE 上"连过去被拒"是常态），就被自己的退避锁死到分钟级。
   而日志里只写"候选 X 未建立链路"，**完全看不出是被退避锁住了**。
2. **"接受好友后对端不同步"= `FriendAccept` 只发一次且没有回执/重发**：
   旧 `accept_friend_request` 发完就 `Ok(())` 并清 pending，广播在"没有直连"时还是静默无操作
   ⇒ 一次丢帧 = **永久单边好友**（我这儿有他、他那儿没我）。
3. **"消息一直发送中"不是消息丢了**：`send_message` 一律先落 outbox，UI 的"发送中"只是
   **没收到 ACK**；接收方对**重复投递会再回 ACK**（`transport.rs` 的 `exists` 分支），
   所以它不会永久卡死——**持续时间 = 链路恢复时间**，要修的仍是链路可用性/恢复速度。

**修法**（只动 BLE 接入链路，线格式/E2EE/outbox/ACK 语义零改动）：

- 退避改成 **5s→10s→20s→40s→60s 封顶**（`ble_dial_backoff_ms`）；
- **对端拨我们成功时也清零退避**（`try_accept_handshake`）——对端能连上，
  说明"我拨不上它"的历史已过期，否则唯一拨号方会被自己的退避锁住；
- 跳过退避时**留痕**（含剩余毫秒）：`[DISCOVERY] 跳过候选 id=… 原因=退避中 剩余=…ms`；
- **`FriendAccept` 有界补发**：新增 `state::pending_out_accepts` +
  `commands::send_friend_accept_via_link`（抽出唯一发送实现）+
  `transport::flush_pending_friend_accept`（2 分钟窗口 / 最多 3 次 / 两次至少隔 5s，
  策略抽成纯函数 `friend_accept_flush_decision`）；
- 好友申请与同意回执**也在心跳时补发**（原来只在建链时补发）。

**日志**（用户要求，全部可 grep）：`[DISCOVERY]`（可拨/被退避跳过）、`[CONNECT]`、
`[GATT] 已就绪`（连接+服务发现+订阅）、`[SESSION] 已就绪`（双向 Hello 验签）、
`[SEND]/[RECV] type=… msg_id=…`、`[ACK] 已持久化 ⇒ 回执` / `[ACK] 重复消息仍回执`、
`[DISCONNECT]`。`Heartbeat/Presence/UserInfo` 过滤掉（每 5s 一条会刷满）。

**护栏**：`dial_backoff_recovers_within_a_minute`、`friend_accept_flush_is_bounded_and_spaced` +
两条 `verify-guards.py` 非空转用例（改 `MAX_MS`/把窗口判据改成 `if false` 都必须 FAIL），
现共 **56** 条。

**门禁**：`cargo test --lib --features bluetooth` 423/423 · `npm test` 350/350 ·
`check-mobile --bluetooth` PASS(0 warning) · `verify-guards` 56/56。

> 与 4.2.12/4.2.13 一起才完整：4.2.12 修"重连时 Hello 投给旧链路"、4.2.13 修"掉线即删节点"、
> 4.2.14 修"退避锁死 + 同意回执不重发 + 可观测性"。P0 真机测试清单见审计文档 §7。

## [4.2.13] - 2026-09-13

### Fixed (🔴 只开蓝牙时「Mac 搜不到安卓 / 安卓显示已发现未建联」的真因：链路一断就删节点)

用户 4.2.11 真机（两端**只开蓝牙**、关掉局域网）：安卓能看到 Mac（"已发现未建联"），
**Mac 里安卓什么都不显示**；安卓点「加好友」提示"已发送，等待对方确认"，Mac 没有任何反应。

Mac 日志给出了完整链路：

```
15:12:43 候选 8474c5dd-… 未建立链路：对端首帧不是 Hello        ← Mac 拨号失败（4.2.12 修）
15:12:56 +conn peer=dev-gosslan-… path=Bluetooth new_peer=true  ← 手机拨进来、握手成功、学到身份
15:12:57 外设侧对端取消订阅（视为断开）                          ← 手机按"指定拨号方"退让，1s 后断开
15:12:57 -conn conns=0
```

**根因**：链路全断时会调 `mark_peer_offline`，它把节点**从节点表里删掉** ——
而「添加好友」列表的数据源**就是这张表**。BLE 上"连上 → 被对端按指定拨号方退让 → 断开"
是**常态**，于是 Mac 每次刚学到手机身份就被删，列表里只闪一下，用户根本点不到；
小 id 那一侧（手机）自退让时从未登记过链路、自然不会调到那里，所以它反而能一直显示
"已发现未建联" —— 两端行为不一致就是这么来的。

**修法**（两件事必须一起做，缺一个就会引出旧缺陷）：

1. `mark_peer_offline` **不再删节点条目**，只清链路快照（`conv_link`，它表示"当前可达路径"）；
   条目交给 `sweep_peers` 的 45s 超时收割 ⇒ 「添加好友」有 45s 窗口能看到刚见过的节点，
   待发的好友申请也能随下一次建链补发（`flush_pending_friend_request`）。
2. 「在线」判据改成 **`last_seen` 新鲜度（15s）或 有活链路**（新增纯函数 `friend_is_online`）。
   只"在节点表里"不再等于在线 —— 否则就是 2026-09-12 复核抓到的那个 High 缺陷
   （一次"连过又掉线"的节点**永久显示在线**）。

**护栏**：`friend_online_needs_freshness_or_an_active_link`（真值表 + 边界）+
源码级 `offline_peer_stays_listed_but_is_not_online`（不删条目、必须清 conv_link、
`get_friends` 必须走 `friend_is_online`、旧的 presence 判据不得复活）+
`verify-guards.py` 对应非空转用例，现共 **54** 条。

> 与 4.2.12 配合才完整：4.2.12 修"Mac 拨号时新连接的 Hello 被投给旧链路"（上面日志第一行），
> 4.2.13 修"学到身份后立刻把节点删掉"。两端都要 ≥4.2.13 才能稳定建链。

## [4.2.12] - 2026-09-13

### Fixed (BLE 重连：新连接的 Hello 被投给**旧链路** ⇒ 对端报「首帧不是 Hello / 未回 Hello」)

真机（用户 4.2.11 会话）Mac 日志反复出现两种失败，指不到原因：

```
[ble] 候选 8474c5dd-… 未建立链路：对端首帧不是 Hello
[ble] 候选 8474c5dd-… 未建立链路：握手超时：对端未回 Hello
```

**根因**：BLE 上同一个 central 的地址在**重连**时会被复用（macOS 侧是 CoreBluetooth 给同一台
手机分配的 UUID，Android 侧是同一个 MAC）。外设侧收到一帧时只按"这个 central 有没有活路由"
决定投递，于是**新连接发来的 Hello 被投进了旧链路的管道**：旧链路的写句柄指向**旧连接**
⇒ 新连接永远收不到 Hello 回应（对端报"握手超时"）；旧链路把这条 Hello 当普通帧消费掉
⇒ 对端报"对端首帧不是 Hello"。用户侧看到的就是"蓝牙时好时坏、加好友没反应"。

**修法**：外设侧新增唯一判据 `peripheral_route_action(has_route, frame_is_hello)` ——
**没有活路由 或 这帧是 Hello ⇒ 换路由并重新握手**（有活路由 + Hello ⇒ 一定是重连），
其余才投已有链路；判据是纯函数，真值表由 `ble::tests` 钉住。为了不给 256 KiB 的分片白烧一次
解析，只对 ≤ `HELLO_PEEK_MAX_BYTES`(1024) 的帧做"是不是 Hello"的轻量判断。

**顺带把诊断补上**（这一轮排查卡在"日志只说不是 Hello，没说是什么"）：central 与外设**两侧**
的握手失败都改成 `…（收到 {wire_kind()}）`；`Message::wire_kind()` 从**序列化结果**反读
`type` 字段（与 serde tag 同一份事实来源 —— 36 个变体手写 match 漏一个就会打出**错的**类型名，
比没有日志更坏），并有单测 `wire_kind_matches_the_serde_tag` 对齐真实 tag。

**护栏**：`reconnect_hello_must_not_go_to_the_stale_route`（真值表）+ 源码级
`peripheral_reconnect_hello_replaces_the_stale_route`（判据存在、接收循环**真的调用**它、
两处错误都带类型名）；`verify-guards.py` 两条对应非空转用例（把判据改成 `if false`、
把 central 侧的类型名去掉，都必须 FAIL），现共 **53** 条。

> ⚠️ 这一版没有改任何**线格式**（`Message` 变体零改动）⇒ 与 4.2.10/4.2.11 混用不会断链；
> 上面那份"未知 type 即硬解析错误"的契约（ADR-0017）依然成立：升级要整批进行。

## [4.2.11] - 2026-09-12

### Fixed (🔴「同一个 Wi‑Fi 里互相搜不到」的真因：发现 socket 绑了**具体 IP** ⇒ macOS 收不到广播)

用户 4.2.9 真机：手机与 Mac 在同一个 Wi‑Fi、两端都开了局域网与蓝牙，
**安卓能稳定搜到 Mac，Mac 里安卓只闪一下就没了**，互相加不上好友。

**根因（本机实测，不是推断）**：发现用的 UDP socket 绑定到**具体网卡地址**
（为了把出口钉在真实 LAN、躲开 VPN 默认路由）。而 **macOS 上绑具体地址的 socket
收不到 `255.255.255.255` 广播、也收不到组播** —— 在空闲端口上做的干净对照实验：

| 接收方 bind | 收到广播 | 收到组播 |
|---|---|---|
| `192.168.31.113:60001`（= 我们的做法） | **0** | **0** |
| `0.0.0.0:60001` | 3 | 3 |

旁听 Mac 的 59991 端口也一致：绑 `0.0.0.0` 时能听到手机每 ~5s 的 announce
（`dev-gosslan-f3d6b7dddf73aab2`，tcp_port 59992），绑具体地址时**一个包都收不到**。
⇒ Mac 的 announce **发得出去**（所以手机看得到 Mac），却**收不到任何 announce**
⇒ 局域网里"单向可见"。手机侧（Linux）绑具体地址仍能收广播，所以只有 Mac 瞎。

**修法**：收发拆成**两个 socket** —— 收的绑 `0.0.0.0`（`discovery_recv_bind_ip()`），
发的仍绑具体 LAN IP（保住"出口钉在 LAN 接口"这个来之不易的修复）；
两个 socket **都进接收循环**读取（SO_REUSEPORT 会把同一份数据报只投给其中一个，
只读一个会漏一半发现包），出包一律走发送 socket（对端拿 `src.ip()` 回连我们，
源地址必须是真实 LAN IP）。

**护栏**：`discovery_recv_socket_actually_receives_broadcast` —— 在本机做一次真实收发
（接收方按 `discovery_recv_bind_ip()` 绑、发送方绑 LAN IP 往 `255.255.255.255` 发），
把接收绑定改回具体 IP 就**在 macOS 上当场红**；`verify-guards.py` 有对应非空转用例。

**顺带记录（还没修）**：手机 GATT server 在 `dumpsys bluetooth_manager` 里显示
**22:20:01~22:21:51 处于未注册状态**，而 Mac 恰好在 22:20:23 拨号并报
`对端没有 Gosslan 的接收特征`；同时两端 BLE 的"镜像链路自退让"工作正常
（各自主动断开自己多拨的那条）。⇒ BLE 通道的注册/重启抖动是**下一个**要查的问题，
本轮局域网修好后 Mac 与手机会走 LAN，不再依赖这条不稳的 BLE 链路。

## [4.2.10] - 2026-09-12

### Docs (⑨ 社区仓库复看：BitChat 的 mesh + 多窗口取舍)

新增两份笔记（用户点名四个仓库，逐条给证据）：

- `docs/notes/bitchat-comparison.md` —— 拿用户手机上已装的 `com.bitchat.droid` **1.7.4**
  （`classes.dex` 字符串 + `AndroidManifest.xml`）与上游 `permissionlesstech/bitchat-android`
  （HEAD = 2.0.2）源码，对照「传输层 / BLE 身份 / 包格式 / 分片 / TTL / 去重 / 离线暂存」，
  每条都标了证据来源（APK 实测 vs 源码常量）；
- `docs/notes/community-repos-review.md` —— 四个仓库的总览结论：`tauri` 2.11.5
  （多 webview 挂在 `unstable` 后面，我们不开）、`lencx-ChatGPT`（`windows: []` + Rust 建窗与我们一致；
  单窗多 webview 的平台分支代价；`open_settings` 的 check-then-build 正是护栏 #34 拦的 TOCTOU；
  多窗口共用 `index.html` 正是护栏 #33/#35 拦的入口反例）、`clash-verge-rev`（见 ADR-0018 §2.5）。

**结论**（对应用户裁定「可以参考 bitchat 协议，最终结果可以帮 bitchat 做中间节点，但不用兼容它的消息协议」）：

1. 我们**已经**具备当中继的全部语义 —— `Message::OpaqueExternal` 去重 + TTL 递减 + fan-out，
   且不解析载荷；
2. 但**现在收不到任何 BitChat 帧**：BLE 层互相看不见（它只认 `F47B5E2D-…` / `A1B2C3D4-…`，
   我们广播 `6b1a7e60-…`）。要真当中继需做**双栈 BLE 外设**（同时注册两套 GATT 服务 +
   扫描同时匹配两个 UUID + 广播兼容），**不是改协议**；代价是复杂度/功耗/身份合规，
   建议默认关闭、设置里显式打开 ⇒ 本轮**不实现**（等用户决定）；
3. 可抄且**直接命中当前 BLE 痛点**的一条：BitChat 把 **peerID 放进广播的服务数据**，
   扫描方不连接就知道"对面是谁"，可用来做去重键（BLE 地址会轮换）并在广播层决定拨不拨
   —— 我们广播里只有 UUID（`bluetooth_peripheral.rs:17,588`），身份要连上后 Hello 才拿到；
4. 另一条加固：BitChat 的分片有**跨消息全局字节上限**（4MiB 全局/1MiB 每条/256 片/64 组），
   我们只有 `MAX_INFLIGHT_MESSAGES = 8` + 单条 512KiB ⇒ 峰值仍可达 MiB 级；
5. 纠正一个印象：BitChat **不是纯 BLE**（1.7.4 就有 Wi‑Fi Aware + Nostr + 可选 Tor），
   我们是 LAN + BLE —— 方向同类，通道组合不同。

本轮**只改文档、护栏脚本与版本号，不动产品代码**，故 4.2.9 的两台产物在功能上与 4.2.10 等价。

### Fixed (`verify-guards.py` 两条护栏的注入锚点失效 ⇒ 门禁假绿)

`store 契约` 与 `R8 keep (JNI)` 两条护栏的注入锚点在 4.2.x 重构后**匹配 0 次**，
脚本以"验证过程出错"报 FAIL（不是静默跳过，这点是对的），但会让整轮门禁红：
- `store 契约` 锚点 `refreshChannels` 已改名 `refreshRuntime`（用在 `NetworkSection.vue` /
  `AddFriendModal.vue`）⇒ 改用新名字，并加注释说明**注入的必须是界面真在用的导出**；
- `R8 keep` 锚点还是实例方法 `public boolean send(...)`，而 4.2.7 之后 keep 规则
  **必须是 `public static`**（Kotlin `@JvmStatic` 的静态桥，见 proguard 文件里的说明）⇒ 同步锚点。

两条都已重新做非空转验证（改坏即 FAIL、恢复即 PASS）。

## [4.2.9] - 2026-09-12

### Fixed (🔴 BLE「每 13s 重拨一次」的真因：端点身份比较**大小写敏感**)
用户 4.2.6 真机：两端能互相搜到 ✓、但点「加好友」对面没反应；Mac 关掉局域网只留蓝牙后
手机能搜到 Mac、**Mac 搜不到手机**。手机 logcat 显示 Mac 每 ~13s 重订阅一次通知，
Mac 日志对应地每 ~13s 一条 `候选 8474c5dd-… 未建立链路：握手超时`。

**根因**：同一台对端在不同角色下拿到的地址**大小写不同** ——
macOS 外设角色（CoreBluetooth 回调）给**大写** UUID（`8474C5DD-…`，链路登记时用的就是它），
而 btleplug central 的 `PeripheralId::to_string()` 给**小写**（`8474c5dd-…`，拨号去重时算出来的）。
而 `MeshEndpoint::Ble` 的相等性是**大小写敏感**的 ⇒ `has_endpoint_addr()` 的"这个端点已经连上了"
**永远不命中** ⇒ 每轮扫描都重新拨号 ⇒ 每次连接都替换对端 GATT server 上的旧连接
⇒ 把它拨过来的那条好链路反复打断（45s 无入站帧 ⇒ 看门狗拆链）⇒ 好友申请正好在死链窗口里发出
⇒ 静默丢失（界面照样显示「已发送」）。

**修法**：`BleEndpoint` 的 `PartialEq` / `Hash` 改为**忽略大小写**（原始字符串保持不变 ——
Android 侧还要拿它去查 Kotlin 的连接表）。一处修改覆盖所有比较点：去重、拆链、端点快照。
顺带这一版也包含上一版的镜像放行 + 失败退避 + 好友申请补发。

**护栏**：`ble_endpoint_equality_ignores_case`（大小写不同的同一地址必须相等、哈希一致、
原始字符串不变）+ `verify-guards.py` 对应非空转用例。

### Fixed (BLE 镜像互拨：4.2.6 的修复没生效 —— 被拒的那一侧永远学不到对端 id)
用户 4.2.6 复测：两端能互相搜到了 ✓，但「点加好友对面没反应」；Mac 关掉局域网、只留蓝牙后
**手机能搜到 Mac，Mac 搜不到手机**。Mac 日志给出了答案：
```
13:23:00 候选 8474c5dd-… 未建立链路：握手超时：对端未回 Hello     ← 每 13s 重复一次，一直没停
```

**根因**：4.2.6 加的"指定拨号方"判据放在**握手成功之后**（因为那时才学到对端 device_id）。
但外设侧（手机）会按"镜像链路"规则**在回 Hello 之前**就把 Mac 的拨入拒掉
⇒ Mac **永远握不上手** ⇒ 永远学不到 id ⇒ 永远记不进"不要再拨"名单 ⇒ 每 13s 重拨一次，
而**每次连接都会打断手机拨过去的那条好链路**（Android GATT server 替换同 central 的旧连接）
⇒ 好链路 45s 无帧被看门狗拆掉 ⇒ 好友申请正好在那个窗口发出 ⇒ **静默丢失**
（好友申请没有回执，界面照样显示「已发送」）。

**修法（两处）**：
1. 外设侧**放行镜像链路**（只保留 `MAX_LINKS_PER_PEER` 上限）：让它把握手走完，
   拨号那一侧拿到 device_id 后会自己判"我比你小 ⇒ 该你拨我"，记入名单并**显式
   `disconnect()`** 收掉这条镜像 —— 一次性打扰，换来永久安静（Mac 即使还是 4.2.6 也能自愈）。
2. **候选失败退避**：连续失败按 60s → 120s → 240s → 480s（上限 10 分钟）冷却，
   连上即清零。BLE 上"连过去被拒"是常态，而每次尝试都会打扰对端 —— 不设冷却就是 13s 一轮的抖动。

### Fixed (好友申请"已发送"但对方没收到 —— 没有回执的帧丢了就永远丢了)
- 命令**先登记再发**（`pending_out_requests`）；任何传输**建链/Hello 补全**时补发一次
  （`flush_pending_friend_request`）；收到**同意或拒绝**后清除（走 `forget_pending_request`）
  ⇒ 丢了会自动补上，也不会无限重发；
- 发送逻辑抽成 `send_friend_request_via_link`，命令与补发共用同一实现。

护栏：`friend_request_survives_a_dropped_link`（先登记 / 有补发入口 / Hello 里真的调用 /
同意或拒绝后清除）+ `verify-guards.py` 对应非空转用例。

## [4.2.8] - 2026-09-12

### Fixed (④ 安卓：非聊天页不得判已读/发回执)
用户 2026-09-12 实测：「虽然我之前点开过聊天界面，但随后在聊天界面点到了设置页面，
当前页面应该不是聊天界面……此时我明明没有看到那条消息，但对方发过来的消息，我这边却判定为已读返回去了。」

**根因**：去抖标记已读只判了「是这个会话 + 应用在前台（`document.hidden`）」，
**没判「聊天视图此刻真的可见」**。移动端设置/运行日志/新的朋友/好友资料都是**整页内容**，
盖在聊天之上时 `mobileView` 仍是 `chat` ⇒ 判定照旧成立。
**修法**：新增 store 级唯一判据 `chatVisible`（桌面恒真；移动端要求 `mobileView === "chat"`
且**没有整页浮层盖住**），由 `ResponsiveLayout` 用 `watchEffect` 把
`settingsOpen / logsOpen / showRequests / profileFriend / shareOpen` 同步成
`mobileChatObscured`；去抖标记已读与"回到前台补发回执"两处都改用它。

### Fixed (⑤ Mac「添加好友」把 Tailscale 同网段设备误标成「蓝牙直连」)
用户 2026-09-12 实测：「他和 tailscale 在同一个网段的设备，其实不是蓝牙直连，但上面写着『蓝牙直连』。」
**根因**：界面用 `p.ip || 蓝牙直连` **反推**链路 —— 没有 IP 就当蓝牙。而跨网段（Routed/Tailscale）
的 peer 同样可能没有 LAN IP。**链路类型只有后端知道**（`Link::path_kind` 由**来路**决定，
不能从 IP 段反推），所以新增 `Peer::link`：由 `get_peers` / `search_nearby_peers` 在返回时
按 `best_link_kind`（LAN > Routed > Bluetooth，与选路同优先级）填上。
界面只在后端确认 `link === "bluetooth"` 时才写「蓝牙直连」，否则显示 IP / 「跨网段·VPN」/「已发现」。

### Changed (⑥ 会话列表：输入不改列表，回车进弹窗搜；弹窗加 loading；清空回初始态)
用户 2026-09-12 要求：「现在已经有聊天的列表，已经有搜索记录了。在上面输入，列表就不要有变化了。
回车弹窗之后，在弹窗里面搜就行了。」「搜索的过程也要加上 loading，我感觉显示得比较慢，当前状态没有提示。」
「当字清空的时候，列表应该恢复初始的空状态。」
- `useConversationSearch` → **`useSearchKeyword`**：只提供 `keyword` + 延迟镜像 `query`，
  **删掉**消息内容检索与"按输入过滤会话"的整套逻辑；会话列表恒为全量（`listConversations = chat.conversations`）；
- 会话列表项不再有"搜索结果态"（`snippet` / 关键词高亮 / v-memo 里的 results 依赖全部移除），
  点开会话就是打开会话 —— 跳转到具体命中那一条由弹窗自己负责；
- 联系人的**姓名过滤**保留（那里就是要实时筛人，仍走延迟镜像，连发粘贴不卡）；
- 搜索弹窗补上**可见的 loading**（转圈 + 「正在搜索…」），清空关键词回到初始空态（原本已清结果，
  现在有明确的空态文案与 loading，界面不再像卡住）。

护栏：`非聊天页不得判已读`、`「蓝牙直连」不得用『没有 IP』反推`、`会话列表不随输入变化`、
`搜索弹窗有 loading 且清空回初始态` 四条前端契约测试 + `verify-guards.py` 对应三条非空转用例。

## [4.2.7] - 2026-09-12

### Fixed (BLE 链路被"镜像互拨"打断 —— 点加好友报「发送失败，连接已关闭」)
用户 2026-09-12 复测：**能搜到人**了，但点「加好友」报「发送失败，连接已关闭」。
Mac 侧日志把因果链完整暴露出来：
```
13:02:43 +ble-link(外设) peer=dev-gosslan-…（双向 Hello 已验签）   ← 手机(central) 连上 Mac 的外设
13:02:58 候选 8474c5dd-… 未建立链路：握手超时：对端未回 Hello    ← Mac 也在拨手机 ⇒ 被手机按镜像规则拒掉
13:03:32 -conn … 读活性超过 45s 无入站帧 ⇒ 拆除死链路并等待重拨    ← 那条好链路被反复打断 ⇒ 45s 无一帧 ⇒ 拆链
```

**根因**：两端都同时跑 central + peripheral，于是**互相拨号**形成镜像链路。
镜像本身会被拒（对端不回 Hello ✓ 规则是对的），但**每次连接都会打断对端拨过来的那条好链路**
（Android 的 GATT server 对同一 central 的新连接会替换旧连接）⇒ 好链路收不到帧 ⇒
被 45s 读活性看门狗拆掉 ⇒ 再重来。用户点按钮的时刻正好落在"链路已死但还没重拨"的窗口里，
于是 `try_send` 走到 `连接已关闭`（链路在表里、writer 通道已关闭）。

**修法**（与 TCP 的 `should_dial` 同一条规则：**大 id 拨、小 id 只接受**）：
- 新增 `should_dial_ble(my_id, peer_id)`：小 id 在**握手验签后**（此时才学到对端 id）
  把该外设记进 `ble_no_dial`，并主动放弃这条镜像链路；
- 扫描循环跳过 `ble_no_dial` 里的外设 —— 不再反复去打扰那条好的链路；
- 蓝牙通道停止时清空该名单（下次开启重新学）。

### Changed (我的在线状态：任一通道在跑 = 在线，两个都关才是离线)
用户 2026-09-12 规则：「蓝牙手机端是自动启动的，此时我的状态应该是在线；能搜到人就说明我在线；
只有用户主动关了蓝牙/两个通道都关了，才是离线」。
- 运行状态快照新增 `present`：`channels.any(running)`（用 `running` 而不是 `enabled` ——
  开关打开但起不来不算在线）；
- 「设置 → 个人资料」与侧栏头像状态点改读 `present`；
  **局域网专属文案**（"局域网：N 个节点"）仍读 `online`（它是"局域网在跑"，两件事不能混）。

护栏：`ble_link_has_a_designated_dialer` + 前端『在线语义』契约测试；
`verify-guards.py` 各加一条非空转用例（`--only ble` 5/5、`--only ipc` 7/7）。

## [4.2.6] - 2026-09-12

### Fixed (🔴 蓝牙「互相搜不到」第二层原因：平台级服务过滤 + 只看连接后的服务)
接上一条（`services()` 复核）之后真机复测**仍然搜不到**，于是把两件事分开测：
`4.2.3/4.2.4` 的日志给出了决定性数据 —— 手机每 13s 扫一次，但
**`BLE 扫描到 0 个候选`**（Mac 那边却一直能连上手机）。

**根因（两条，都是"过滤条件用错了地方"）**：
1. **平台级用服务 UUID 过滤**：`start_scan(ScanFilter{services})` 在 Android 上走的是
   **硬件/固件过滤，只匹配主广播包**；而 macOS 的 `CBAdvertisementDataServiceUUIDsKey`
   会把 128 位 UUID 放进**扫描响应（scan response）** ⇒ 手机**永远收不到 Mac 的广播**。
   （反向没问题：Android 的广播把 UUID 放在主包里，所以 Mac 能找到手机 —— 这个不对称
   正是"一边能发现、一边不能"的原因。）
2. `Peripheral::services()` 在 Android 上**只有连接并 `discover_services()` 之后**才有值，
   未连接时恒为空 —— 上一条已修，但当时没意识到第 ① 条，所以仍然收不到任何东西。

**修法**：`scan_peers()` 改为**扫全部设备**（`ScanFilter::default()`），
再在 Rust 侧按**广播内容**判定（`peripheral.properties().services` —— 这是 btleplug 从
广播/扫描响应里解析出来的，两端都可靠）；"对方不是 Gosslan 端"由 `connect()` 的特征校验兜住。

**诊断日志**（以后这类问题一眼可见）：每次扫描都打
`BLE 扫描：收到 N 个广播，其中 M 个是本应用服务` ——
`N=0` 是扫描/权限/硬件问题，`N>0 且 M=0` 是对端没在广播或广播里没有我们的 UUID。

## [4.2.5] - 2026-09-12

## [4.2.4] - 2026-09-12

### Changed (蓝牙日志上 logcat：`ble` 通道的 info 也镜像出去)
真机排查 BLE 时，缺的正是 info 级那几条（"扫描到几个候选 / 哪个候选没连上、为什么"）——
它们以前只写应用内日志文件，而 release 包既不能 `run-as`、logcat 里也看不到，
于是用户能贴给我们的只有 warn/error，"互相搜不到"只能靠猜（这一轮的根因就是被 `services()`
过滤掉，日志里**一个字都没有**）。
现在 Android 上 `ble` 通道的 info 与 `boot` 一样镜像到 logcat
（`adb logcat -s gosslan`），频率很低（每 10s 最多几行），不会刷屏。

## [4.2.3] - 2026-09-12

## [4.2.2] - 2026-09-12

### Fixed (🔴 蓝牙「互相搜不到」的真因：扫描结果被未连接的 `services()` 复核掉了)
用户 2026-09-12 实测：手机（4.2.1）与 Mac（4.1.17）蓝牙都开着、双方都在广播、
`dumpsys bluetooth_manager` 里能看到手机**每次扫描命中 2–3 个带我们服务 UUID 的广播**，
但**两台设备的「添加好友」列表里始终没有对方**。

**根因**：`scan_peers()` 在拿到扫描结果后又按 `Peripheral::services()` 复核了一遍
（注释里写的意图是"部分平台会忽略 ScanFilter，所以复核一次"）。但
`Peripheral::services()` 在 **Android 上只有 `discover_services()`（= 连接）之后才有值**，
**未连接时恒为空集合** ⇒ 所有候选都被过滤掉 ⇒ 扫描循环一个都不去连
（`scan_loop` 里"连接失败"是 info 级、成功才打 `+ble-link`，所以日志里**一个字都没有**，
症状就是"扫描明明有结果、却永远搜不到"）。

**修法**：
- `scan_peers()` **原样返回**平台层已经过滤好的结果（`start_scan(ScanFilter{services})`
  就是系统级过滤，`dumpsys` 的 GATT Scanner Map 能直接看到命中数）；
  "对方不是 Gosslan 端"由 `connect()` 里的**特征校验**兜住 —— 那一步本来就要连上；
- 扫描循环补一条诊断日志：`BLE 扫描到 N 个候选，开始逐个连接` ——
  这类"发现了却没去连"的缺陷以后在日志里一眼可见。

**护栏**：`scan_results_are_not_filtered_by_unconnected_services`
（断言 `scan_peers` 里不再出现 `services()` 过滤、且必须有解释性注释与"扫到候选"的日志）
+ `verify-guards.py` 对应非空转用例（把过滤加回去 ⇒ 必须 FAIL）。

## [4.2.1] - 2026-09-12

### Docs (③ 窗口架构 ADR-0018：把「一窗一入口 / 后端真相源 / 事件带载荷」定下来)
用户要求的第 ③ 项：这三条是本轮 ① ② 的根据，写进 ADR 以免以后被改回去。

`docs/adr/0018-window-architecture.md` 记录：
- **一窗一入口**：每个窗口自己的 HTML + 入口（共享的只有 `boot.ts` 与 `style.css`），
  禁止"一个文档 + 前端按 label 换布局"；
- **后端是唯一真相源**：跨窗口可见的事实只能存后端一份；同一事实只有一个读命令、
  前端只有一个写入入口（举证：`get_channel_status`/`get_network_status`/`NetworkStatus` 已被删除）；
- **常驻单例窗口**：懒创建、只隐藏不销毁；代价是常驻窗口必须自己刷新（焦点时
  `refreshEnvironment()` 并行拉取）；
- **事件带载荷 + 定向发送**：`settings-changed`（补丁）/ `runtime-changed`（快照）/
  `data-cleared`（破坏性操作）都带载荷、都用 `emit_filter` 排除发起窗口，发起窗口改用**命令返回值**；
  由此删除了 `settingsDirty` 一整套防回灌状态机；
- **为什么不用 BroadcastChannel**（5 条理由：绕过后端⇒第二真相源、无法与后端原子、
  到不了原生侧、没有目标过滤与类型约束、不解决首帧），并对照 `clash-verge-rev` 说明了
  我们采纳什么、在哪一点上刻意做得不同。

## [4.2.0] - 2026-09-12


### Changed (② 运行状态合并成「一个快照 + 一个带载荷的事件」)
用户批准的三项窗口架构改造的第 ② 项（① 已在 4.1.13 交付，③ 窗口架构 ADR 随后）。

**旧状态**：同一件事（局域网到底开着没有）在前端有**两份**表示 ——
`channels[lan].enabled`（来自 `get_channel_status`）与 `online`（来自 `get_network_status`），
由两个命令 + 两个事件各自维护；`runtime-changed` 还是**无载荷**广播，每个窗口收到后都要
自己重拉一半状态。用户实测过它的必然结果：「添加好友里把局域网打开，设置里还是关的」。

**新状态**：
- 后端只有**一个采集点** `build_runtime_snapshot()`：通道（lan/bluetooth 的
  enabled/available/running/peers）、局域网是否在线 + 绑定地址、蓝牙事实
  （本次构建是否编译了蓝牙特性）、在线节点数，一次读全；
- 只有**一个命令** `get_runtime_snapshot`，只有**一个事件** `runtime-changed`，
  且事件**带完整快照**、用 `emit_filter` **不回发发起窗口**（发起窗口从命令返回值里拿）；
- `set_channel_enabled` / `start_network` / `stop_network` 都**返回新快照** ⇒
  发起的窗口零额外 IPC、也没有"拉回来的是旧值"的竞态；
- 前端只保留**一个写入入口** `applyRuntimeSnapshot()`；`refreshChannels` /
  `refreshNetworkStatus` 与其"两半各自刷新"的写法**删除**；
- `get_channel_status` / `get_network_status` 两个命令与 `NetworkStatus` 结构**删除**
  （它们的全部信息都已在快照里）。

`peers` 仍然走 `peers-updated`（它最多 3/s、只在脏时推，见 `emit_peers`）：把完整节点列表塞进
快照会让每次通道开关都搬一遍全表；快照里只带 `peerCount`（空态文案要用）。

**护栏**（都已在 `verify-guards.py` 证明"改坏即 FAIL、恢复即 PASS"）：
- Rust `runtime_state_has_a_single_source`：旧的两个"半份状态"命令必须不存在、
  必须有唯一采集点、事件必须带 `RuntimeSnapshot` 且用 `emit_filter` 排除发起窗口；
- 前端 `events.test.ts` 同名契约检查（含"前端不得再调那两个半份命令"）；
- `channelState.test.ts` 的旧契约（"必须同时刷新两半"）改写成"只应用返回的快照"。

门禁：`cargo test --lib` 403/0；`npm test` 346/0；`vue-tsc` 0；`vite build` 通过；
`verify-guards.py --only ipc` 6/6 非空转通过。

## [4.1.17] - 2026-09-12

### Fixed (安卓蓝牙外设起不来：`BlePeripheral.start()` 漏了 `@JvmStatic` + keep 规则只 keep 了实例方法)
真机日志（4.1.16 装上后崩溃已消失、btleplug 也初始化成功，接着暴露出来的两条）：
```
[warn] [ble] 蓝牙外设角色不可用（central 角色不受影响）：JNI 调用失败：Method not found: start ()Z
[warn] [ble] 扫描失败：启动扫描失败：Runtime Error: Need android.permission.BLUETOOTH_SCAN permission
```

**根因（两处，都是"Rust 按静态方法调、Kotlin/keep 只给了实例方法"）**：
1. `BlePeripheral.kt` 里 `fun start(): Boolean` **漏了 `@JvmStatic`**（它旁边 6 个兄弟都有）。
   `object` 里的成员只有加了 `@JvmStatic` 才生成**静态桥**；Rust 侧用的是
   `call_static_method("start", "()Z")` ⇒ 没有静态桥就是 `Method not found`。
2. proguard 的 keep 规则写的是 `public boolean start();`（没有 `static`）⇒ R8 只保住了
   **实例方法**（dex 里 `PUBLIC FINAL start()Z`），静态桥被当死代码删掉。
   实测把规则改成 `public static boolean start();` 后，dex 里 `send/stop/isConnected/payloadMtu`
   立刻变成 `PUBLIC STATIC FINAL` —— 只剩 `start` 还是实例形态，于是顺着它查到了 ①。

**修法**：给 `start()` 补 `@JvmStatic`；keep 规则里所有 JNI 方法都改成 `public static …`；
顺手把一处**误挂在 `onMainSync` 上的 `@JvmStatic`** 和 `start` 的 KDoc 归位。

**护栏（两条，都是被这次真机日志证明必需的）**：
- `android_jni_signatures_match_kotlin` 扩展：Rust 用 `call_static_method` 调的每个
  Kotlin **成员**函数，声明上方必须有 `@JvmStatic`（顶层函数天然 static，不要求）；
- `release_keeps_every_kotlin_method_called_from_rust` 扩展：keep 块里每个方法都必须是
  `public static …`（只 keep 实例方法 = 静态桥被删 = `Method not found`）；
- `verify-guards.py` 各加一条非空转用例（去掉 `@JvmStatic` / 去掉 `static` ⇒ 必须 FAIL）。

> 说明：`BLUETOOTH_SCAN` 那条是**权限没授予**（设备上此前被拒/未授），不是代码缺陷；
> 前端已有"去系统设置打开附近设备权限"的提示路径，本次未改。

## [4.1.16] - 2026-09-12

### Fixed (🔴 安卓启动闪退的真因：`nativeAttachOpenWith` 被按"实例方法"注册，ART 直接 abort)
真机 logcat（4.1.15 实测，终于抓到崩溃栈，而不是只有一行 `gosslan` 日志）：
```
Abort message: 'Native method '"nativeAttachOpenWith"' was registered as instance
                 but called as static method'
  #12 … (Java_com_gosslan_app_OpenWithKt_nativeAttachOpenWith__+24)
  #15 … com.gosslan.app.MainActivity.onCreate+492
```

**根因**（4.1.12 引入 FileProvider 时我没注意的一处细节）：`OpenWith.kt` 里的
`nativeAttachOpenWith()` 是**文件级（顶层）函数** ⇒ Kotlin 把它编译成 `OpenWithKt` 的
**static** 方法；而 Rust 侧的 `native_method!` 少了 `static` 关键字 ⇒ 宏按**实例方法**注册。
ART 在第一次调用时判定不一致，**直接 abort 整个进程**（SIGABRT）。
对照：`BlePeripheral` 里的 `external fun nativeBootstrap()` 在 **object 内部** ⇒ 实例方法 ⇒
那边的宏**不加** `static`（一直是对的）。

**修法**：`static extern fn native_attach_open_with()` + 形参由 `JObject this` 改为
`JClass class`（static 方法拿到的第二个参数就是类引用本身，不再需要 `get_object_class`）。

**护栏**（这条正是被这次闪退证明必需的 —— 它编译、单测、构建全绿，只在真机启动时炸）：
`jni_static_matches_kotlin_toplevel`：解析 Rust 每个 `native_method!` 是否带 `static`，
与 Kotlin 侧同名 `fun` 的**缩进**对照（顶格 = 顶层 = static；缩进在 `object`/`class` 里 = 实例），
两者必须一致；`verify-guards.py` 增加对应非空转用例（删掉 `static` ⇒ 必须 FAIL）。

## [4.1.15] - 2026-09-12

### Fixed (安卓蓝牙起不来的真因：btleplug 的 `io.github.gedgygedgy.**` 从未进过包)
真机 logcat（4.1.13 实测）：
`btleplug droidplug 初始化失败（蓝牙通道将不可用）：failed to resolve Java class
'io/github/gedgygedgy/rust/future/Future' (class not found or linkage error)` → 随后闪退。

**根因**（`tar` + `dexdump` + 上游源码三方对照，不是猜的）：
1. **真正让它静默的是 R8 keep 规则把包名拼错了**：`scripts/android/proguard-gosslan.pro` 写的是
   `-keep class io.github.gedgygeddy.**`（**多一个 `d`、少一个 `g`**）—— 那是个**不存在的包**，
   于是 R8 把真正的 `io.github.gedgygedgy.**` 当死代码**整包删掉**；配套的
   `-dontwarn io.github.gedgygeddy.**` 又把"这个包不存在"的警告**吞掉**，构建期一个字都不报。
   4.1.10 那次"修好了"是**假象**：注入目录里恰好有源码、编是编了，但 dex 里没有 ——
   当时只验证了 `com.nonpolynomial.**` 在不在，**没验证另一半**。
2. 发布到 crates.io 的 **btleplug-0.13.0 里也没有 `io/github/gedgygedgy/**`**：
   `tar tzf btleplug-0.13.0.crate | grep gedgy` = 0（只有 14 个 `com/nonpolynomial/**`），
   这 18 个 `.java` 只在它的 git 仓库里（**目录名与包名都是 `gedgygedgy`**）。
3. 输入还不稳：那 18 个 .java 此前只存在于 CARGO_HOME 的**提取目录**，而它是**易失**的
   （换一个 CARGO_HOME、或 cargo 重新解包就没了）⇒ 连"能编进去"都时有时无。

**修法**（三处，缺一不可）：
- 包名统一改正：`proguard-gosslan.pro` / 注入脚本 / 构建脚本里的 `gedgygeddy` → `gedgygedgy`
  （R8 于是真的 keep 住这 18 个类）；
- 那 18 个 `.java` **随仓库入库**（`scripts/android/btleplug-java/`），与 crate 自带的
  `com/nonpolynomial/**` 一起挂到 Gradle 的 `sourceSets`；
- 注入脚本对**两个目录**都做硬检查（缺任何一个直接 `throw`，工作区文件缺失时从 git 自愈），
  打完包再**反查 dex**：`Lcom/nonpolynomial/btleplug/android/impl/Adapter;` 与
  `Lio/github/gedgygedgy/rust/future/Future;` 必须都在，否则这次构建直接失败
  （这条检查本该两轮前就有 —— 它就是被这个坑证明必需的那一条）。

顺带把 `useAppStore` 里一句已经过时的注释（提到已删除的 `settingsDirty`）改正。

## [4.1.14] - 2026-09-12

### Fixed (A 加不上 B：B 那边已有 A，A 这边是重置过的账号 —— 申请被"已是好友"过滤掉了)
用户实测（局域网内 A、B 互相发现）：
- B 的好友列表里有 A，而 **A 是重置过的账号**、列表里没有 B；
- A 去加 B → **B 的「新朋友」里没有他**，两边都加不上；
- 用户唯一能走通的路是：**先把 B 里的 A 删掉**，再重新加一次。

**根因**（两条规则各自都对，叠在一起就成了死锁）：
1. 收到好友申请时，旧实现**无条件**往 `pending_requests` 里插一条；
2. 而上一条修复（"已经是好友了、申请还挂着"）又让 `get_pending_requests` 把
   **申请人是已是好友**的条目**过滤掉**。
⇒ 这条申请在 B 的界面上永远不可见，可它又占着 pending；A 那边也一直在等一个永远不会来的同意。

**修法（按用户给的规则）**：既然 B 那边已经把 A 当好友，就等于 B **已经同意了** ——
收到这种申请时直接走**完整的同意路径**（落库 + 回执 + 清 pending + 通知 UI），双方关系立刻收敛：
- 新增 `auto_accept_if_already_friend()`：直连（`Message::FriendRequest`）与跨跳
  （`GossipKind::FriendRequest`）**两条入口都先调它**，不再插 pending；
- 把"同意好友"抽成**唯一实现** `accept_friend_request()`，用户手动同意与自动同意共用一个函数 ——
  上一类缺陷（单边好友关系）的成因正是"同一件事两套路径行为不一致"。

**护栏**：`friend_request_from_existing_friend_auto_accepts`（两条路径都必须先自动同意、
且"同意"只有一份实现），`verify-guards.py` 新增对应非空转用例（改坏即 FAIL、恢复即 PASS）。

## [4.1.13] - 2026-09-12

### Changed / Fixed (① 设置事件带补丁 + 不回发发起窗口：消灭"每个窗口全量重拉"与事件乒乓)
用户批准的三项窗口架构改造，这是**第①项**（②运行状态单一快照、③窗口架构 ADR 在本条之后）。

**旧实现**：`emit(EVENT_SETTINGS_CHANGED, ())` —— 无载荷、广播给所有窗口。三个后果都真实发生过：
1. **白拉**：改一次主题，每个窗口都要 `get_settings + get_device_info + get_share_dir` 三连重拉；
2. **回灌**：发起窗口会收到**自己**的事件，读到的却是写入前的旧快照 ⇒ "点了主题又跳回去"
   （为此额外养了 `settingsDirty` / grace 窗口一整套守卫）；
3. **事件乒乓**：重拉路径末尾会 `pushUiLanguage()` → `set_ui_language()`，而那条命令当时**也发**
   `settings-changed` ⇒ 两个窗口互相触发，形成高频 IPC 环（"界面响应速度高于一切"最怕的东西）。

**现在**（`SettingsPatch`）：
- 载荷是 `{changed, origin, settings}`：`changed` 说明变了哪些键，`settings` 只带**这些键的新值**
  ⇒ 接收方**零 IPC** 直接应用（`applySettingsPatch` + `applySettingsSnapshot(..., {partial:true})`）；
- 后端用 **`emit_filter`** 按窗口标签过滤，**发起窗口收不到**这个事件 ⇒ 回灌与乒乓一起消失；
- `shouldResyncFromBackend` / `settingsDirty` / `lastLocalWriteAt` **整套删除**（连同它们的单测）——
  少一套需要长期维护的状态机；
- `applySettingsSnapshot` 支持 `partial`：缺的键一律**不动**（以前 `preferredIp.value = s.bindIp`
  会把"选中的网卡"清掉），样式副作用也只在与它相关的键真的变了时才跑；
- `set_ui_language` **不再发**设置事件（它只负责重建 macOS 菜单栏）—— 这是打断乒乓的关键一刀。
- 唯一保留的"全量重拉"是 `changed: ["*"]`（「恢复默认」把键整体删掉了，逐键送 patch 容易漏）；
  资料/目录（`nickname`/`avatar`/`shareDir`）只做一次**定向**补拉。

### Fixed (Mac 4.1.10：清了缓存/目录/聊天记录，主界面毫无反应)
**根因**：`clear_all_data` **不发任何事件**，而「清除聊天数据」按钮在独立设置窗口里，
它调的是**那个窗口**的 `chat.clearAllData()` + `refreshFriends()`；主窗口是另一个 WebView，
手里的会话列表/消息一条都没变 —— 看起来就像"没清掉"。
**修法**：新增 `data-cleared` 事件（同样不回发发起窗口），主窗口收到后
`resetAfterDataCleared()`：先清空本地视图（消息/会话/群/待处理申请/传输单），再重拉还在的那些。
好友**不清**（清数据不等于断交，`clear_all_data` 也不动好友表）。

**护栏**（都已在 `verify-guards.py` 里证明"改坏即 FAIL、恢复即 PASS"）：
- 设置事件必须 `emit_filter` + 过滤掉发起窗口；载荷必须带 `changed` 与 `settings`；
- **每个**改设置的后端命令都必须传 `origin`（写成 `None` 就报错）；
- `clear_all_data` 必须广播 `data-cleared`，且前端必须监听它；
- `settingsDirty` 那套守卫不得复活（防止有人无意中把回灌问题带回来）；
- Rust 单测 `settings_patch_carries_only_changed_keys_with_camel_case_names`：键名必须是 camelCase、
  只带被点名的键、`dark_mode` 要转布尔、通知两项缺省是"开"、资料/目录不进 patch。

门禁：`cargo test --lib` 400/0；`npm test` 345/0；`vue-tsc` 0；`vite build` 通过；
`verify-guards.py` 新增 3 条用例（+ 既有 36 条用例的注入锚点全部复核为唯一）。

## [4.1.12] - 2026-09-12

### Fixed (安卓「打开文件」失败的真因：应用私有文件不能以 `file://` 交给别的应用)
用户实测 4.1.9：点已收到的文件 → 「文件打开失败，系统或者网络暂不可用」。

**真因**（读上游源码确认）：`tauri-plugin-opener` 在 Android 上只有一句
`Intent(ACTION_VIEW, url.toUri())`（`OpenerPlugin.kt`），而我们交给它的是
`file:///data/user/0/com.gosslan.app/downloads/…` —— **Android 7.0+ 禁止把应用私有文件以
`file://` 暴露给别的应用**，`startActivity` 当场抛 `FileUriExposedException`，
前端只能显示一句笼统的失败。桌面端不存在这个限制，所以它只会在真机上现形。

**修法**：改走 FileProvider —— 把私有文件映射成 `content://com.gosslan.app.fileprovider/…`
再交给用户选中的应用，intent 上带 `FLAG_GRANT_READ_URI_PERMISSION`（只授这一个 URI 的临时
读权限：不申请任何存储权限，也不暴露目录）。
- Kotlin 侧新增 `OpenWith.kt`：`FileProvider.getUriForFile` + `ACTION_VIEW`，MIME 交给系统
  `MimeTypeMap` 推断；**所有异常都翻译成能行动的中文原因**（文件不存在 / 没有能打开它的应用 /
  具体异常类型），并且与 `BlePeripheral` 一样**跳回主线程**执行（非主线程的 Java 未捕获异常
  会直接杀进程，连 panic hook 都抓不到）。
- `res/xml/file_paths.xml` 补一条 `root-path`：Tauri 在 Android 上的 data 目录是
  `Context.getDataDir()` **本身**，收到的文件在 `dataDir/downloads`，模板原有的
  `cache-path` / `external-path` 覆盖不到它（不补的话 `getUriForFile` 直接抛
  `IllegalArgumentException`）。
- Rust 侧新增 `android_open.rs`（JNI 桥：JavaVM 与类引用由 `MainActivity` →
  `OpenWith.bootstrap` 带进来，与 BLE 同套路）；`open_file_native` 在 Android 上走它，
  macOS / Windows / Linux 行为不变。
- **`jni` 依赖从 `bluetooth` feature 里摘出来**（改成 Android 目标必带）：打开文件与蓝牙无关，
  挂在 feature 上等于"没开蓝牙就开不了文件"。
- `macos_open.rs` → `open_path.rs`：它现在管四个平台（macOS / Android / Windows / Linux），
  存在性检查也收进同一个入口。

**顺带修**（记账脚本把 CHANGELOG 劈坏了）：`scripts/version.mjs` 用
`includes("## [Unreleased]")` + 字符串 `replace` 找发布锚点，正文里出现同样文字就会被误命中 ——
真实后果是 4.1.1~4.1.11 全被插进 4.1.0 小节的半句话里、`## [Unreleased]` 锚点被吞掉。现已改成
**按行锚定**的正则，并修复了受影响的 CHANGELOG（补回锚点、还原被劈开的句子、把 4.1.0 移回
正确的"新在前"位置）。`npm run version:check` 同时新增 **CHANGELOG 结构检查**（锚点唯一 +
标题格式 + 版本小节严格降序），这类破坏从此会在门禁里被拦住。

**护栏**：JNI 签名护栏与 R8 keep 护栏各自扩展到第二座桥（`OpenWith.openWith` /
`nativeAttachOpenWith`），JNI 类型映射支持可空标记（`String?` 与 `String` 描述符相同）；
`scripts/verify-guards.py` 新增 3 条非空转用例 —— Kotlin 少写 `: String?` ⇒ 必须报
「描述符不一致」；keep 规则漏 `openWith` ⇒ 必须报「缺少 `openWith`」；CHANGELOG 丢了
`[Unreleased]` 锚点 ⇒ `version:check` 必须失败。

## [4.1.11] - 2026-09-12

### Fixed (蓝牙"没有默认开启"其实是重试被冷却挡掉了；图片"先失败后成功"；灯箱按钮压状态栏)
用户 4.1.9 实测（**闪退已不再复现** ✓，以下三条是新问题）：

1. **蓝牙没有默认开启** —— 日志里是 `切换：已停止 → 开启` 紧跟 `忽略高频蓝牙通道切换请求（3s 冷却内）`。
   根因：第一次 start **失败**（4.1.9 还没有 btleplug 的 Java 类，见上一条），前端的自动重试落进
   3 秒冷却被丢掉。修法：**冷却只统计成功的切换** —— 失败时立刻把冷却清零，放行重试。
   （4.1.10 已把 Java 类编进包，配合这条，蓝牙应能真正起来。）
2. **图片"先显示加载失败、过一会儿才出来"** —— `@error` 把状态钉死成 failed，而收到图片时
   文件可能还在传输/落盘。现在：出错先按退避重试（0.4/0.8/1.6/2.4/3.2s，共约 10s，带防缓存参数），
   期间保持"加载中"；**只有重试全部失败才显示"加载失败"**，而且点一下可以手动重试 ——
   与用户要求一致："能加载了才弹出来，失败是不可逆的最终结果，中间给个加载中"。
3. **安卓图片预览右上角「保存 / ×」压住状态栏** —— 该操作区与底部页码都改用
   `env(safe-area-inset-top/bottom)`；桌面端 `env()` 为 0，视觉不变。

⚠️ **仍未修**（下一轮，需要 FileProvider + Kotlin intent，无法在无设备环境验证）：
安卓端点击"打开文件"报「文件打开失败」—— 根因是 `tauri-plugin-opener` 在 Android 上只处理
**URL**（`Intent(ACTION_VIEW, url.toUri())`），而我们的文件在应用私有目录里，Android 必须用
`content://`（FileProvider）并显式授予读权限才能交给其它应用。

## [4.1.10] - 2026-09-12

### Fixed (🔴 安卓闪退真因：btleplug 的 Android Java 部分**从未编译进 App**)
用户 4.1.7 的 logcat 复现同一条 panic（我上一轮"初始化 droidplug"的修法因此无效）：

```
panic @ btleplug-0.13.0/src/droidplug/mod.rs:20:26：
  Droidplug has not been initialized. Please initialize it with btleplug::platform::init().
```

**真正的根因**（用 `dexdump` 反查 release APK 确认）：btleplug 在 Android 上依赖它自带的
**Java 实现**（`com.nonpolynomial.**` + `io.github.gedgygedgy.**`，共 28 个 `.java`），
而 Tauri 的 Gradle 工程里**根本没有这个模块** ⇒ `platform::init()` 里的 `find_class` 失败
⇒ 之后 `Manager::new()` 在 crate 内 panic ⇒ 安卓 release `panic = "abort"` **整进程消失**。
（debug 包同样没有这些类，只是表现为"蓝牙通道打不开"而不是闪退 —— 与此前那条反馈也对得上。）

修法（两处，都是**构建期注入**，因为 `gen/android` 会被 `tauri android init` 重生）：
1. `inject-android-signing.mjs`：把 crate 自带的 Java 源码目录挂到 App 的
   `sourceSets["main"].java.srcDirs(...)`（比引 Gradle 子模块简单，且不受 AGP 版本差异影响）；
2. `proguard-gosslan.pro`：按 btleplug 官方 README 的要求 keep
   `com.nonpolynomial.**` 与 `io.github.gedgygedgy.**`（它的 Java 代码只被 native 按类名调用，
   R8 会当死代码整包删掉）。
3. 另外把"初始化失败"从**静默**改成**可见**（stderr + logcat），并在 `driver::adapter()` 前
   查一个就绪标志：万一哪天又退化，**降级成"蓝牙不可用"，绝不再 panic 闪退**。

**验证**（可复现）：重建后 `strings classes*.dex | grep nonpolynomial` 必须非空 —— 这是我这一轮
唯一能在这台机器上做完的端到端验证。

## [4.1.9] - 2026-09-12

### Fixed (蓝牙启停加了 3 秒冷却：无论谁在抖动，都不再拆蓝牙栈)
幂等闸门只能挡住"重复同一状态"，挡不住**交替**请求（日志里正是 `启动→停止→启动`）。
现在再加一道冷却：距上次真实启停不足 3 秒的切换请求**只记一条 warning**（含"运行中/请求"两个状态），
不再真的启停。于是即使调用方在抖，用户也只会看到日志，不会被拖卡。
同时每次真实切换都打一条 `蓝牙通道切换：X → Y`，下次日志能直接指出是谁在抖。

## [4.1.8] - 2026-09-12

### Fixed (Mac 4.1.5 实测：蓝牙在"启动→停止"之间每秒抖动，把整个应用拖卡)
用户 Mac 日志实测：`蓝牙外设角色已启动 → 蓝牙通道已启动 → 已停止广播 → 已启动 …`
每秒循环十几次，持续十几秒；现象是**设置窗口顿卡、主界面慢、局域网消息也变慢**。

- **幂等闸门**：`set_channel_enabled("bluetooth", …)` 现在先比较"目标状态 vs 运行状态"，
  相同就**只同步偏好、绝不碰蓝牙栈**。上层 UI 再怎么抖动，也不会再把 CoreBluetooth 的
  GATT server + 广播拆掉重建 ⇒ 卡顿与"局域网变慢"的放大器被摘掉。
  （抖动本身的调用方我还在查；这条闸门让"是谁在抖"不再影响用户。）
- **蓝牙缺省三端都是开**（用户规则：「有蓝牙就默认开，不用手动开关」）。
  之前是手机默认开、桌面默认关 —— 不仅 Mac 上要多点一次，更会让"偏好=关 而运行时=开"
  互相回灌，正是上面那种抖动的温床。
- **启动 2s 后自动确保一次**（首帧之后、不在启动关键路径上；幂等、失败不抛），
  三端一致：装完就有蓝牙通道。

### Fixed (设置窗口渲染异常：`null is not an object (evaluating 'g.themeColor')`)
用户 Mac 日志里的前端 rejection：设置窗口读设置快照时快照为 `null`。
**一次渲染期异常会让那一页再也 patch 不动**，表现正是"点设置顿顿的、过一会儿才突然弹出来"。
两处修：`applySettingsSnapshot` 加空值防御；顺手删掉 `themeColor` 那行的重复赋值。

## [4.1.7] - 2026-09-12

### Fixed (安卓闪退真凶：btleplug 的 droidplug 后端从未初始化 —— 由真机 logcat 定位)
用户按提示跑出 logcat，拿到**原始 panic**：

```
panic @ btleplug-0.13.0/src/droidplug/mod.rs:20:26：
  Droidplug has not been initialized. Please initialize it with btleplug::platform::init().
```

即：Android 上 `Manager::new()` → `global_adapter()` 时发现 droidplug 没初始化 ⇒ panic；
而安卓 release 强制 `panic = "abort"` ⇒ **进程直接消失**（进「添加好友」/「设置」时按需拉起
蓝牙通道，正好走到这里）。此前我们**从没调用过** `btleplug::platform::init()`。

修法：在 `nativeBootstrap`（由 `MainActivity.onCreate` 同步调用、手上有 `Env`）里做一次
`btleplug::platform::init(env)` —— droidplug 需要一个已 attach 的线程来种下 JavaVM 单例与
Adapter 类。失败不致命：只打一条 stderr，蓝牙通道随后以明确错误返回（绝不 panic）。

顺带印证了两件事：① 上一提交加的**日志进 logcat** 让这次定位成为可能（panic 直接出现在
`adb logcat -s gosslan` 里）；② panic hook 在 abort 之前确实执行了（那条 `[panic]` 就是它写的）。

### Changed (出一个 Mac 生产包也纳入常规流程；并加一条只打 .app 的脚本)
用户要求：「后面每次打完安卓的包，再打一个 Mac 的生产包，我本地测试」。

- 新增 `npm run dist:mac:app`：只出 `.app`（`--bundles app`）。
  为什么要这条：`npm run dist:mac` 会继续打 `.dmg`，而 dmg 那步要跑 `hdiutil` + AppleScript
  设置窗口布局 —— 在受管沙箱里会失败（本轮实测：`bundle_dmg.sh` 退出码非 0），
  虽然 `.app` 其实已经产出成功。以后本机自测用 `dist:mac:app`，要发布 dmg 时在**自己的终端**里跑
  `npm run dist:mac`。
- 产物：`src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Gosslan.app`
  （release + `bluetooth` feature；`open` 或拖进 /Applications 即可测）。

## [4.1.6] - 2026-09-12

## [4.1.5] - 2026-09-12

### Fixed (安卓「点『添加好友』/『设置』立刻闪退」—— Android 框架 API 被从非主线程调用)
用户实测 4.1.3：**打开应用不闪，一进「添加好友」或「设置」立刻闪退**。

两个入口的唯一共同新代码是 `ensureBluetoothOn()`（打开这些界面时按需申请「附近的设备」权限）。
根因形态：Rust 命令跑在 **tokio 工作线程**上，JNI 直接调进 Kotlin 后，
`ActivityCompat.requestPermissions` / `openGattServer` / `startAdvertising` 这些
**Android 框架 API 只能在主线程（有 Looper 的线程）调用**；从工作线程调用会抛 Java 异常，
而 Java 层的未捕获异常由系统处理器**直接杀掉进程** —— 它不是 Rust panic，
所以 panic hook 也抓不到、日志里什么都没有（与"什么都拿不到"的现象完全吻合）。

修法（Kotlin 侧，`BlePeripheral.kt`）：新增主线程跳板
- `onMainSync { }`（带返回值、最多等 3s）用于 `start()`，保留它原来的 `Boolean` 契约；
- `onMain { }`（异步 post）用于 `requestAllPermissions()`、`stop()`；
- 全部 try/catch 兜底并 `nativeOnWarning(...)` ⇒ 平台调用失败**最多是"通道没开"，绝不让应用消失**。

教训（写进注释）：**任何触碰 Android 框架/Activity 的 JNI 入口都必须回到主线程**。
顺带把"日志进 logcat + 启动路标"（上一提交）保留，下次即使还有别的崩点也能自己浮出来。

门禁：`npm test` 344/0；`vue-tsc` 0；`check-mobile.sh --bluetooth` PASS / 0 warning；
APK 构建通过（Kotlin 编译是该改动的实际验证）。

## [4.1.4] - 2026-09-12

### Added / Changed (安卓崩溃可诊断：日志进 logcat + 启动路标；启动路径彻底不申请权限)
用户实测 v4.1.2「打开还是闪退，连日志都拿不到」。在拿到 logcat 之前，先把"能自己缩小范围"的
两件事做掉：

- **日志同时写 logcat**（`log -t gosslan ...`）：release 包既不能 `run-as`、Rust 的 stdout/stderr
  也不进 logcat，此前崩溃现场对用户和我们都是黑的。现在 `adb logcat -s gosslan` 就能看到
  应用自己的日志（含 panic hook 那条 `panic @ 文件:行:列：消息`）。
  开销控制：warn/error 一律打，info 只打 `boot` 通道（启动路标）。
- **启动路标**：`AppState::init` 前后、`tray::setup` 前后各打一行 `boot` 日志 ——
  下次"打开就闪退"时，最后一条路标直接告诉我们崩在哪一步。
- **移动端启动路径彻底不申请权限**（上一轮已把蓝牙运行时改成按需，这一轮连权限申请也改成按需）：
  打开「添加好友」或网络设置时才申请。启动路径至此**不含任何平台专有调用**。

顺带修一处**只有安卓会现形**的编译问题：给 `tray::setup` 加路标时把 `#[cfg(desktop)]`
拆开了（属性只作用于紧跟其后的**一条**语句），导致 `tray::setup` 掉出 cfg ⇒ 移动端 E0433。
已改成整块包 `#[cfg(desktop)]`。`check-mobile.sh --bluetooth` 正是为这类问题存在的门禁。

门禁：`cargo test --lib` 399/0；`npm test` 344/0；`vue-tsc` 0；`vite build` 通过；
`check-mobile.sh --bluetooth` **PASS / 0 warning**。

## [4.1.3] - 2026-09-12

## [4.1.2] - 2026-09-12

## [4.1.1] - 2026-09-12

### Fixed
- 安卓闪退修复（启动路径不再碰蓝牙）；通道状态单一真相源；手机端蓝牙不给手动开关（有蓝牙即默认开）；
  设置项按端裁剪。**本节原本的内容被发布脚本吞掉**（见 4.1.0 小节的说明），明细见
  tag `v4.1.0...v4.1.1` 的提交记录（`4f3eb93`）。

## [4.1.0] - 2026-09-12
### Changed (默认昵称：不再用设备用户名，改为「形容词 + 动物 + 设备短码」的英文名)
用户要求：「默认用户名可以不用设备的用户名吗？用一串英文，可以加设备识别号的前几位或后几位；
长度合适，让用户不改也好看，也有想改的欲望。」

- **规则**：`<Adjective> <Animal> <3 位 base36>`，例如 `Lively Puma W1U`。由 `device_id` 的 SHA-256
  **确定性派生**：同一台设备每次启动同名（随机数会让重启后名字变化，好友列表就认不出谁是谁）；
  短码来自设备标识的哈希，**不含设备信息**（旧规则直接拿 hostname 当昵称：既不好看，也把设备名写了出去）。
- **长度**：词表只用 ≤6 字母的词 ⇒ 总长 ≤17 字符，列表里不会被截断成省略号。
- **一次性迁移**：仅当已存名字是"旧默认的产物"（空串 / "Gosslan 用户" / "Gosslan User" /
  恰好等于本机 hostname）才替换；**用户自取的名字一律不动**。
- **"恢复默认"走同一条规则**：新增命令 `default_nickname`，前端不再写死 i18n 文案
  （否则"恢复默认"与首次安装得到的名字会不一致）；顺带删掉两处写死的默认名。

**护栏**：`nickname.rs` 4 条单测（确定性 + 三段式/纯 ASCII/长度上限并断言词表无长词、不同设备短码不同、
base36 补零大写、旧默认名识别且不误伤自取名字）。

**顺带修**：`scripts/version.mjs` 发布后**补回 `## [Unreleased]`**（此前发布一次就把这一节吃掉，
下一次记账无处可写 —— 这个坑本轮咬了我两次）。

## [4.0.0] - 2026-09-12

### Added (Phase 8：BitChat 中继 —— 外部 mesh 的不透明帧，Gosslan 只当中继)
依据 ADR-0017（用户裁定：本版不做旧版兼容 ⇒ 不需要能力门控/双读），验收只有三条：
**收得到 · 去得掉重 · TTL 递减后转发**。

- **线格式**新增 `Message::OpaqueExternal { id, ttl, payload }`（原样字节 base64），
  去重用它自己的 `id`，**不进** Gosslan 的 `message_id` 体系。
- **收到即喂同一条流水线**：`handle_message` 新分支 → `MeshRouter::on_receive`
  （全局去重 + TTL 递减 + 源节点排除，与业务帧同一套；路由器不解析载荷，P-A03）
  → `Forward{frame}` 时用**路由器给出的 ttl**（已递减）按 fan-out 发给邻居（排除来源）。
- **不做的事**（照 ADR 写死）：不解密、不落库、不建 BitChat 用户/channel、不进 gossip 引擎。
- **健壮性底线**：新增纯函数 `validate_opaque_external`（id ≤128 且字符安全、ttl ∈ 1..=16、
  payload 合法 base64 且解码后 1..=256 KiB）—— 畸形/超限帧**只丢这一帧、不断链**。

**护栏**：`phase8_acceptance_receive_dedup_and_ttl_forward`（验收三条一次跑通）、
`opaque_external_validation_bounds`、`opaque_external_round_trips_through_wire_format`。
验证：`cargo test --lib` **395 / 0**；`npm test` **344 / 0**；`vue-tsc` 0；`vite build` 通过。

## [3.0.1] - 2026-09-12
### Changed
- 版本发布 v3.0.1（本次未预先填写更新说明，明细见 tag v3.0.0...v3.0.1 的提交记录）

## [3.0.0] - 2026-09-12

### Fixed (安卓发附件/图片总是失败 —— 选择器给的是 `content://` URI，不是文件路径)
用户实测：「发文件总是失败，但文字、代码都能发」。

**根因（读上游源码确认）**：Android 的文件选择器返回 **`content://` URI**；
`tauri-plugin-dialog` 的 Kotlin 侧直接把它交给前端（插件里的 `getPathFromUri` 是**没人调用**的死代码），
于是 `std::fs::metadata("content://…")` 必然失败 ⇒ 发送失败。桌面端本来就是真实路径 —— 只有安卓会这样。

**修法**：新增命令 `import_picked_file`，选择器返回值**统一先过它**：
不是 URI 就原样返回（桌面零开销）；是 URI 就经 `tauri-plugin-fs`（安卓走 ContentResolver）
**流式复制**进 `cache/imports/` 再发送 —— 后面的图片预览/缩略图/断点重传都不用改。
文件名优先取 URI 里编码的真实名字（`…%3ADownload%2Freport.pdf` → `report.pdf`），
相册那种只有数字 id 的按**文件头嗅探**补类型（jpg/png/gif/webp/bmp/pdf/zip，其余 bin），
所以"从相册选图片"仍会作为**图片**消息发出；名字一律消毒（阻断路径穿越）。

**护栏**：三个纯函数单测（URI 解名 / 内容嗅探 / 消毒）+ 前端接线守卫（必须先 `api.importPickedFile`）。
验证：`cargo test --lib` **392/0**、`npm test` **337/0**、`vue-tsc` 0、`check-mobile.sh` PASS/0 warning。
⚠️ 仍需真机确认：相册图片 + 下载里的文档各发一次；"另存为/打开收到的文件"是同一个选择器的**保存方向**，
可能需要同样处理（下一步）。

### Fixed (好友申请：双方互加后，那条申请还挂在「新朋友」里)
用户 2026-09-12 真机实测：「如果两个人已经互相加上好友了（可能双方都给对方发送了加好友申请），
其中一个人点了确定，另一个人点进『新朋友』列表……如果该好友已在好友列表的话，那条好友申请
就应该自动清除掉」。

**根因**：同一件事（同意好友 ⇒ 忘掉这条申请）在**两条路径**上行为不一致 ——
跨跳路径 `GossipKind::FriendAccept` 清了 `pending_requests`，而**直连路径
`Message::FriendAccept` 只加了好友、忘了清**。于是"有时候会清、有时候不清"，
全看这条回执走的是哪条路（同一局域网内直连时必现）。

**修法（三层，缺一层都可能再漏）**：
1. **路径统一**：抽出 `transport::forget_pending_request(state, peer)`，直连 / 跨跳 /
   `respond_friend_request` 的同意路径**全部**走它（同一个助手，不可能再各写一遍）。
2. **兜底判据**：`get_pending_requests` 按 friends 表过滤并顺手收敛内存态
   （`is_actionable_request`：人已经是好友 ⇒ 申请不再"待处理"）。判据抽成**纯函数并有单测** ——
   它原先散落在各条路径里，正是漏清的原因。
3. **前端按事实过滤**：`chat.pendingRequests` 改为 computed，用新的纯函数
   `actionableRequests(原始列表, 好友 id 集合)` 过滤。这样**四个读它的地方**
   （会话列表红点、通讯录「新的朋友」、窄导航徽标、添加好友页的「同意/拒绝」行）一处生效，
   而且无论这条申请是"我同意的 / 对方同意的 / 重启后重新拉取的 / 对方走别的消息把我加上的"，
   只要 `friends` 里有这个人，那一行就立刻消失 —— 不依赖某条回执有没有送达。

**护栏（都做过非空转验证，`verify-guards.py` 现 **27** 条）**：
新增 `src/utils/friendRequests.test.ts`（纯函数 4 例 + "store 必须走它"的接线守卫）、
`commands::tests::pending_request_from_an_existing_friend_is_not_actionable`、
Rust 源码规则 `every_friend_accept_path_forgets_the_pending_request`（两条路径各一次，少一条即 FAIL）
与 `pending_requests_exclude_existing_friends`。

验证：`cargo test --lib` **389 / 0**；`npm test` **334 / 0**；`vue-tsc` 0；`vite build` 通过；
`cargo check --all-targets` 0 warning；`check-mobile.sh` PASS / 0 warning。

### Fixed (安卓真机实测四处：触屏定位 / 通道不同步 / 新的朋友点不开 / 蓝牙要手动开)
用户 2026-09-12 安卓实测报告：①「回到最新」按钮不在右下角；② 添加好友里的局域网开关与设置页的
不同步；③ 收到好友申请后点「新的朋友」打不开界面；④ 蓝牙通道打不开、且希望手机上默认就开着
（参考 BitChat：不用配对、不用配置、进去就能连）。

- **① 「回到最新」按钮在触屏上掉出右下角** —— 根因是 **CSS 特异性**：按钮写的是
  `tap-safe absolute bottom-4 right-5`，而 `src/style.css` 在 `@tailwind utilities` **之后**、
  `@media (pointer: coarse)` 里的 `.tap-safe { position: relative }` 与 Tailwind 的 `.absolute`
  **特异性相同** ⇒ 触屏设备上 position 被改成 `relative`，按钮回到文档流。
  桌面 `pointer: fine` 不走这条媒体查询，所以**只有安卓/触屏复现**（这就是它看起来像随机 bug 的原因）。
  修法：把 `.tap-safe` 的规则包进 `:where()`（特异性 0），任何定位工具类都能正常生效；
  并给 `checkStyleCascade` 加了第 ③ 条级联判据（该块内声明 `position` 的选择器必须是 `:where(...)`），
  配一个复现用的坏样例单测 —— 面向未来：以后往这个块里加任何"只扩大命中区"的类都不会再压掉别人。
- **② 局域网开关两处不同步** —— 同一个概念有两份前端状态：`channels[lan].enabled`（后端真实运行
  状态）与 `app.online`（另一份快照）。「添加好友」页改的是前者，设置页显示的却是后者，
  而且没人去刷新它。修法：两处 UI 一律走 `app.setChannelEnabled`，且**设置页的开关值也改用通道
  状态**；store 的 `setChannelEnabled` 现在**同时**刷新 `refreshChannels()` 与
  `refreshNetworkStatus()`（`online`/`boundIp`）。新增 `channelState.test.ts` 把这三条路径钉住。
- **③ 安卓点「新的朋友」没反应** —— 申请页渲染在**右侧主面板**里，而移动端靠 `mobileView` 平移
  切换面板；`openRequests()` 没切过去，用户还停在会话列表上，于是"点了像没反应"。
  修法：移动端打开申请页时 `mobileView = "chat"`，关闭时回到 `"list"`（与 `openFriendProfile` /
  `openSearchHistory` 同一处理）。
- **④ 蓝牙：状态是假的 + 手机上要手动开** ——
  - **假状态**：`get_channel_status` 里的蓝牙 `running`/`peers` 取自 `TransportManager` 中那个
    "尚未接线"的占位 `BluetoothTransport`（running 恒 `false`、peers 恒 0）⇒ 界面永远显示未运行，
    用户点了开关也看不出变化。现在改为读**真实运行时**（`network::ble::runtime_state`），
    并且 **起不来就显示为关**（不再是"偏好写了就算开"，用户能再点一次重试）。
  - **手机默认开启、零配置**：新增 `db::get_bt_enabled`（缺省值 = `cfg!(mobile)`，
    与 `get_lan_enabled` 一样立刻持久化；桌面维持默认关，不悄悄开射频），
    且移动端启动拿到「附近的设备」权限后会自动把蓝牙通道真正拉起来（失败退避重试一次，
    仍失败则交给「添加好友」页的就地开关，那里有明确的失败原因与权限指引）。
- **顺手把「找不到设备」变成可自诊**：「添加好友」的空态现在直接说事实 ——
  两条通道都关 / 局域网在跑但没人应答（附"同网段、别开 VPN/访客网络"）/ 蓝牙正在扫描（首次要等几秒，
  列表会自己刷新）；蓝牙直连的节点（无 IP，来自双向 Hello 验签）显示「蓝牙直连」而不是留一行空白；
  通道开关改用开关给出的**目标值**（原先用 `!ch.enabled` 取反，状态过期时会反向操作）。

**仍未验证（需要真机环境）**：局域网在你们网络下到底能不能发现（组播是否被 AP 拦、是否同一网段）、
蓝牙射频行为（能否扫到/连上/握手）、以及手机默认开启后的实际观感。这些只能由你在设备上确认；
出问题时「运行日志」里会有 `ble`/`discovery` 通道的记录（`who_has_sent`、`+ble-link` 等）。

### Fixed (桌面独立窗口：慢、会闪成聊天界面、连点会开出第二个 —— 架构上把三个窗口彻底分开)
用户实测三连：「点设置/日志，窗口出来得很慢，像卡了一下」「第二次打开设置，窗口会先刷成主聊天
窗口、再立马变成设置界面」「按钮没防抖，连点几下不该开出第二个，它应该还是那一个」。

**根因（读代码确认，不是一个 bug 而是三个叠在一起）**：
1. **三个窗口共用一个 `index.html` + 一个 Vue 应用**，由 `App.vue` 按窗口 label 决定渲染哪一屏。
   它的模板是 `…v-else-if="isSettingsWindow && settingsReady"` / `v-else` ⇒ 设置窗口在
   "数据就绪之前"的窗口期**落到了 `v-else`，也就是把整棵聊天三栏布局挂了起来** ——
   这就是"先闪成主聊天窗口"。而且每次打开都要白等一整棵聊天组件树（dev 下是几百个模块请求）。
2. **窗口关闭即销毁**：每次打开都要重建 WebView + 重新加载前端 + 重跑 `app.init()`。
3. **并发打开没有串行**：`WebviewWindowBuilder::build()` 的重复 label 检查在
   `tauri/src/manager/window.rs::prepare_window` 里做，而窗口被登记进 manager 是在主线程创建
   **完成之后** —— 两个并发调用（连点）会双双通过检查，后者还会覆盖 manager 的记录。
   前端也只有 `void api.openSettingsWindow()`，没有单飞/防抖。

**修法（一次做干净，不留分支与拷贝）**：
- **一个窗口一个文档 + 一个入口**：`index.html` → `src/entries/main.ts`（聊天）、
  `settings.html` → `src/entries/settings.ts`、`logs.html` → `src/entries/logs.ts`。
  设置/日志窗口**从第一帧到结束都不会碰到聊天代码**（构建产物实测：`settings.html`
  不再引用 `assets/main-*.js`）。Rust 侧 `WebviewUrl::App("settings.html"|"logs.html")`，
  不再注入 `__GOSSLAN_WINDOW__`。
  实测收益：主入口 bundle **458KB → 310KB**，设置窗口只额外加载 3.25KB 的入口 chunk。
- **共用启动逻辑抽成一份**：`src/boot/boot.ts`（错误上报、骨架撤除、标题、装配、挂载顺序）
  + `src/boot/theme-boot.js`（首帧主题/语言/平台）+ `src/boot/skeleton.css`（骨架样式），
  后两者由 `vite.config.ts` 的 `gosslan:inline-boot` 插件内联进三个 HTML —— **三份拷贝变一份事实来源**。
  每个窗口的骨架写在各自的 HTML 里（设置窗口只有设置骨架），标题由各自的
  `data-title-zh/en` 声明（Tauri 会把 document title 同步到窗口标题，Rust 不再维护第二份文案）。
- **`App.vue` 只服务主窗口**：窗口 label 分支、`settingsReady` 占位 hack 全部删除 ——
  "设置窗口渲染成聊天界面"这个 bug 在结构上不可能再发生。
- **后端单例 + 串行创建**：新增 `ensure_aux_window`（快路径 show+focus；慢路径拿
  `AUX_WINDOW_CREATE_LOCK` 后**再查一次**才 build），所有独立窗口都必须走它。
- **关闭即隐藏（常驻）**：`install_hide_on_close` 把标题栏 × 与 `close_*_window` 都拦成
  `hide()` ⇒ 第二次起打开是 `show()`，也就是用户要的"点一下立马就开"。
  开关是 `commands.rs` 里的 `AUX_WINDOWS_RESIDENT`（改成 `false` 即回到关闭销毁）。
- **前端单飞 + 防抖 + pending 反馈**：新增 `src/utils/windowLaunch.ts`（纯判据）
  与 `src/composables/useWindowLauncher.ts`（模块级单例状态，窄导航 / 移动端底栏 / 原生菜单
  三处共用）；按钮在打开期间显示 `aria-busy` + 半透明，冷启动那一下用户能立刻看到"点到了"。

**护栏（都是主机可跑，且逐条做过非空转验证）**：
- `aux_windows_open_their_own_document`：Rust 里每个 `WebviewUrl::App(...)` 目标文件必须存在、
  必须指向自己的入口，且**独立窗口不得再共用 `index.html`**；
- `aux_window_open_is_singleton_serialized_and_resident`：打开命令必须走 `ensure_aux_window`、
  不得自己查窗口存在性，helper 必须双重检查 + 接上 hide-on-close；
- 前端 `windowEntries.test.ts`：三个 HTML ↔ 三个入口 ↔ 三套骨架一一对应（设置/日志不得带
  聊天骨架、不得 import 聊天代码），`App.vue` 不得再有 label 分支；
- 前端 `windowLaunch.test.ts`：单飞/防抖判据 + "两个开窗按钮必须走 `launchAuxWindow`"接线守卫。
- `scripts/verify-guards.py`：`--only window` 4 条新用例（现共 **19 条**）。

### Fixed (Android **release** 包：三个"只有 release 才现形"的问题 —— 之前那份包是装不上 / 蓝牙会废的)
- **① R8 把 Rust 按名字调用的 Kotlin 方法改名了**：`isMinifyEnabled = true` 时，`BlePeripheral` 的
  `stop/start/send/isConnected/payloadMtu/requestAllPermissions/hasRequiredPermissions` 全被改名成
  `a/b/c/d/e`（`dexdump` 实测），而 JNI 只按「名字 + 签名」查找 ⇒ release 真机包上**蓝牙外设整条
  路径会在运行期 `NoSuchMethodError`**；debug 包不混淆，所以开发期完全看不见。
  修法：新增 `scripts/android/proguard-gosslan.pro`（版本库里的单一事实来源），构建前由
  `inject-android-signing.mjs` 注入 `app/proguard-rules.pro`；**新增主机可跑护栏
  `release_keeps_every_kotlin_method_called_from_rust`** —— 规则漏方法 / 多留废弃方法 / 两处规则漂移
  三种漂移都会 FAIL，且已逐条做非空转验证（删 `send` 行 → FAIL 并指名；塞 `legacyMethodGone` → FAIL；
  恢复 → PASS）。修完实测 `dexdump`：7 个名字全部保留。
- **② release 包根本没有签名**（真机上是"应用未安装"）：`app/build.gradle.kts` 里没有任何
  signingConfig，AGP 对 release 产出的就是未签名 APK —— 而发布脚本此前**没有**跑
  `inject-android-signing.mjs`（只有旧的 `android:build` 跑了）。修法：脚本在构建前强制注入
  （有 `ANDROID_KEYSTORE_BASE64` 用真 keystore，否则回退 debug 签名保证内测可装），并在打包后
  **硬校验** `apksigner verify` + 包里只有目标 ABI 的 `.so`，不通过就整条构建红掉。
- **③ GitHub Actions 会把上面的坑原样发出去**：CI 只装 `platforms;android-34`，而 `tauri android init`
  生成的工程是 `compileSdk = 36`（必然失败）；且 CI 走同一个发布脚本（同样没注入签名/清单）。
  修法：CI 装 `platforms;android-36` + `build-tools;36.0.0` + `ndk;27.1.12297006`（与本机验证过的一致）、
  JDK 升 21、删掉重复的 python 权限注入（统一由注入脚本负责）、产物连 `.sha256` 一起上传/发布。

### Changed (Android 出包链路：产物位置、命名、校验)
- 产物从 `dist/android/` 改到 **`release-artifacts/android/`**：安卓构建会先跑 `vite build`，而它会
  **清空 `dist/`**（第一版就踩过：`mkdir` 完紧接着被删，`cp` 报 "No such file or directory"）。
- 文件名带构建类型：`gosslan-<版本>-<abi>-<release|debug>.apk`（此前 release/debug 同名，分不清手上
  装的是哪一份；实测 release **12MB** vs debug **216MB**），并在旁边生成同名 `.sha256`。
- 只认**本次构建新产出**的 APK（marker 时间戳 + `find -print -quit`），不再 `ls -t | head -1` 去赌
  构建目录里没有残留的 universal / 另一个 ABI 的旧包。
- 新增三条产物校验（都在出包脚本里，失败即整条构建红掉）：
  - **包内前端 = 当前 `dist`**：前端资源是被嵌进 `libgosslan_lib.so` 的，所以"前端改了但 Rust
    没重编"在产物层面完全看不出来 —— 用 bundle 的内容哈希文件名在 `.so` 里搜一遍（分块搜，debug
    的 `.so` 有 200MB+），对不上就报"包里会是旧界面"。
  - Gradle 判定"输入内容未变"而跳过打包时**允许复用**已有产物，但会打印一行说明并照常做上面的校验
    （实测：只改前端压缩配置、`.so` 内容一致时 `packageRelease` 是 UP-TO-DATE，APK 的 mtime 不变；
    旧写法会误报"本次构建没有新产出 APK"）。
  - **体积异常自检**：Gradle 增量打包偶尔在 APK 里留下**未被中央目录引用**的旧数据
    （实测 debug 包 226MB → **444MB**，多出的 218MB 是上一版 `.so` 残骸，能装但白胖一倍）→ 超过
    5MB 就警告并给出处置办法（删 `build/outputs/apk` 后重打）。
- 支持只出单个 ABI：`bash scripts/build-android-releases.sh --abi arm64-v8a`
  （或 `npm run android:build:test -- --abi arm64-v8a`）。
- 构建会**改脏工作区**的问题一并解决：`MainActivity.kt`（运行时权限申请）、`AndroidManifest.xml`
  （竖屏 + 权限清单）、`build.gradle.kts`（release 签名）与 `proguard-rules.pro`（R8 keep）的注入结果
  都落到版本库；注入脚本保持幂等，专门兜底 `tauri android init` 重生工程之后的 CI / 新机器。

### Changed (打包策略：按架构分别出包，不再打 universal)
- **Android 按 ABI 出两份包**（GitHub 发布就挂这两份）：`arm64-v8a` 给现代手机、`armeabi-v7a` 给老设备。新增 `scripts/build-android-releases.sh`（`npm run android:build:test` / `android:build:release`），对每个 ABI 各跑一次 `tauri android build --target <abi>`，产物按 ABI 改名落到 `release-artifacts/android/`。
  - universal 包把两/四份 `.so` 拼在一起，而真机只用到一份（实测 universal debug **423MB**）；单 ABI 包体积约为其 1/3。
  - ⚠️ **不用** Gradle 的 `splits.abi`：Tauri 的 Android 插件会给每个 ABI 设 `ndk.abiFilters`，AGP 禁止两者并存，配置阶段直接失败（`Conflicting configuration … in ndk abiFilters cannot be present when splits abi filters are set`）。
- **macOS 分架构出包**：`npm run dist:mac`（`aarch64-apple-darwin`，Apple 芯片 —— 日常开发/自测/打包都用它）与 `npm run dist:mac:intel`（`x86_64-apple-darwin`，发布给老 Intel Mac 时才需要）。**不打 universal**（会把两份二进制拼起来，体积翻倍）。
- **Windows 不需要拆**：`npm run dist:win`（NSIS，x86_64）本来就小，维持现状。
- 测试口径：**一律用最新版本，不为旧版本做任何兼容**。

### Fixed (Android：关掉蓝牙开关后手机仍在广播 —— JNI 签名写错)
- **真实缺陷**（本轮 code review 抓到，`d2fad5e`）：Kotlin 的 `fun stop()` 是 **Unit** 方法（JNI 描述符 `()V`），Rust 侧却按 `()Z` 调用。**JNI 不做任何编译期检查** —— 这只会在运行期抛 `NoSuchMethodError`，且只有真机才现形：用户关掉「蓝牙通道」后手机**仍在广播**（耗电 + 隐私），日志里一个字都没有。
- 修法：`stop()` 改走 `()V` 的 void 调用，失败时**主动上报 Warning**（"停止 BLE 外设失败（可能仍在广播）"）；引入 `kotlin_method!("名字", "描述符")` 登记宏把两者写在一处；顺带修正一条**永远发不出来的日志**（桥就绪的 Notice 原先在 `bootstrap` 里发，而那时 events 通道还没建立，现改在 `start()` 里发）。
- **新增主机可跑护栏 `android_jni_signatures_match_kotlin`**：解析 `BlePeripheral.kt` 里 `fun` 的形参/返回类型推出 JNI 描述符，与 Rust 侧登记**逐字比对**；并检查每个 `extern fn` 在 Kotlin 里确有同名 `external fun`（否则 JVM 会 `UnsatisfiedLinkError`）。
- **非空转验证**：把 `stop` 改回 `()Z` → 护栏 FAIL 并给出具体差异；恢复 → 全绿。已加入 `scripts/verify-guards.py`（现覆盖 **10 条**护栏，一条命令全跑）。
- 验证：`cargo test --lib` **379 passed / 0 fail / 0 warning**；`cargo check --all-targets` 0 warning；Android `check-mobile.sh --bluetooth` PASS / 0 warning。
- ⚠️ 仍未验证：真机射频行为（需你的设备）。另外本轮确认了 E2E 两个前置能编出来（`cargo build` + `build --example e2e_peer`），但**没有替你跑 `scripts/e2e-dev.sh`** —— 它会真的启动 GUI 实例（在你桌面上弹窗口），该由你决定何时跑（手册 §0.5）。

### Added (Android 外设角色的 Rust↔Kotlin 桥 —— 7-f 完成，手机也能"被连"了)
- **Rust 侧 JNI 桥**（`transport/ble_android.rs`）：缓存 `JavaVM` 与 Kotlin 类的全局引用、把 Kotlin 回调上来的**分片**重组成整帧（复用 `ble_framing`，与 macOS 同一份实现）、把网络层要发的帧按 MTU 分片后调 Kotlin 的 `send`。
  - **`bootstrap` 的鸡生蛋问题**：JNI 的 `FindClass` 依赖"调用方的类加载器"，从 tokio 线程里找不到 App 的类 ⇒ `MainActivity.onCreate` 调一次 `BlePeripheral.bootstrap(context)`，由它在 App 代码还在栈上时把 `JavaVM` + 类引用交给 Rust。
  - **符号 + 注册双保险**：`native_method!` 的 `extern` 直接导出 JNI 符号（`bootstrap` 靠名字解析），其余四个再 `register_native_methods` 显式注册 —— 签名写错会**当场**以 `NoSuchMethodError` 暴露，而不是真机收发时静默失效。
  - **没开 feature 必须安全**：Kotlin 用 `try/catch UnsatisfiedLinkError` 包住 `nativeBootstrap()`，否则默认构建会在启动路径崩溃。
- **接口同形**：`ble_android.rs` 的 `start/stop/PeripheralServer/PeripheralWriter/PeripheralEvent` 与 macOS 版逐一对应 ⇒ `network/ble.rs` 的事件循环/握手/路由/读写循环**两平台共用一份**，只有 import 按平台切换。依赖只加了 `jni = "0.22"`（optional，Android 专属；btleplug 的 droidplug 本来就用同一个版本，**没有引入新的第三方 crate**）。
- 验证：`cargo test --lib` 378 / 0 warning；`cargo check --all-targets` 0 warning；**Android target `--features bluetooth` 0 warning**；`--features bluetooth` 的完整 APK 构建通过（JNI 符号链接进 `.so`、Kotlin 一并编译）。
- ⚠️ **仍未验证**：真机上的广播/连接/GATT 读写（需用户设备）；iOS 侧同类实现（`CBPeripheralManager`，与 macOS 同款代码）尚未接；Windows 做外设（WinRT `GattServiceProvider`）未做。

### Fixed (🔴 卡死：61 个命令仍在 macOS 主线程上跑 —— 清除数据/恢复/添加好友时整个应用冻住)
- **用户反馈**：「点设置里的清除数据或恢复，整个设置窗口就卡死；点加号 → 添加好友，主窗口卡死」，并重申**渲染与响应速度高于一切**（`6324d05`）。
- **根因（读上游源码确认）**：`tauri-macros` 的 `body_blocking` 把**同步**命令**内联调用**在 IPC 处理器里，只有 `ExecutionContext::Async` 才走 `respond_async_serialized` → `async_runtime::spawn`；而 wry 的 IPC 回调跑在 **AppKit 消息循环（macOS 主线程）**。所以同步命令 = 在 UI 主线程执行，**卡的是整个进程、所有窗口**。而「清除数据」是一次长事务（全表 `DELETE`，可能数秒）并一直握着 `db` 互斥锁 ⇒ **长事务持锁 → 同步读在主线程等锁 → 全部窗口冻住**。`clear_all_data` 本身早就是 async，但它的**读者不是**（`get_settings`/`get_friends`/`get_pending_requests`/`get_transfers`/`get_logs`/`list_interfaces`/`reset_settings`/`open_settings_window`…）。
- **修法**：把所有会碰重资源的命令改成 `#[tauri::command(async)]` / `async fn`，共 **61 个**（数据库、文件系统、日志、剪贴板、网卡枚举、阻塞睡眠）；只保留纯窗口操作（minimize/maximize/fullscreen/close/圆角）同步。其中 19 个（`send_message`/`mark_read`/`send_file`/`respond_friend_request`/`start_network`/`stop_network`/`set_channel_enabled`…）是**被新守卫逼出来的**，全是高频路径。
- **守卫从"名字清单"改成"规则"**：上一轮的 `heavy_commands_run_off_the_main_thread` 是名字清单，只能盯住写清单时的 12 个 —— 正因为如此这 61 个才漏了过去。新增 `blocking_commands_run_off_the_main_thread` 直接解析 `commands.rs`，逐个命令取函数体（手写扫描跳过字符串/注释/生命周期，避免 `format!("{}")` 造成花括号错配），碰标记即要求 off-main-thread，**一次报出全部违规并附原因**。
- **非空转验证**：去掉真实命令 `get_settings` 的 `(async)` → 守卫 FAIL 并点名；恢复后全绿、无残留。
- 验证：`cargo test --lib` 378 / 0 warning；`--features bluetooth` 385 / 0 warning；`cargo check --all-targets` 0 warning；Android `check-mobile.sh --bluetooth` PASS / 0 warning。前端无需改动（命令名与返回类型未变，async 对 `invoke` 透明）。
  ⚠️ 说明：清除大量数据时**读操作会排在长事务后面**（界面保持可交互，数据在操作完成后刷新）—— 这是刻意的原子性取舍（全清或全不清），不是卡顿。

### Fixed (设置窗口的头像与名字显示空白/默认值)
- **根因是挂载与取数的顺序**（`9d4d055`）：子组件在 `setup` 阶段就把 store 的值**快照**进 ref（`ProfileSection` 的 `watch(..., { immediate: true })` 读 `app.device`），而 `app.init()` 是在 `App.vue` 的 `onMounted` 里才 `await` 的 —— **先挂载、后拿数据**。主窗口看不出问题（设置分区打开时才挂载），独立设置窗口一开场就把**空昵称 / null 头像**写进了 ref，数据到位后没人再同步，于是头像与名字一直是空白/默认。
- 修法两处互补：① `App.vue` 新增 `settingsReady`，独立设置窗口的内容**等 `app.init()` 完成后再挂载**（这一处同时修掉外观/语言/网络/存储等分区的同类问题；等待期间显示窗口自己的首屏骨架）；② `ProfileSection` 补对 `app.device.nickname/avatar` 的 watch，覆盖**运行中**的变更（「恢复默认」会把昵称恢复默认、头像清空并广播）。用户正在输入时 `device` 不变，故不会覆盖未保存的编辑。
- 验证：`npm test` 291 / 0 fail；`vue-tsc` 0 错误；`vite build` 通过。

### Added (Android 外设角色的 Kotlin 侧 —— 手机也能"被连"了，7-f 第一步)
- **新增 `BlePeripheral.kt`**（`gen/android/app/src/main/java/com/gosslan/app/`）：`BluetoothLeAdvertiser` + `BluetoothGattServer` 的完整实现（`e66fd8b`）。只做 central 的手机**永远不可能被发现**（btleplug 只能主动连，ADR-0015 §3.1）；手机做了外设之后 `Windows(central) ──BLE──▶ 手机(peripheral)` 才成立，手机与 Windows 之间不必再经 Mac 中转。
- 与 macOS 实现（`bluetooth_peripheral.rs`）**行为契约一致**：同一套 UUID、同样"广播里只放服务 UUID"、同样的写/通知语义与 native 回调（frame / unlinked / notice / warning）。两处**有意的差异**：① Android 的 `onConnectionStateChange` 会**真的**告诉我们对端断开（CoreBluetooth 外设角色没有这个回调）；② 必须显式给 TX 挂 **CCCD 描述符**，客户端才能开启通知（CoreBluetooth 隐式处理）。
- 🔴 **补 `BLUETOOTH_ADVERTISE` 权限**（真实缺口）：Android 12+ 把蓝牙拆成 SCAN / CONNECT / **ADVERTISE** 三个运行时权限，缺 ADVERTISE 时 `startAdvertising` 直接抛 `SecurityException` —— 现象正是"手机能扫别人、别人永远发现不了手机"。Kotlin 侧在用户打开「蓝牙通道」时申请并给出可操作提示。
- **本机真的把 APK 建出来了**（不只 `cargo check`）：`ANDROID_USER_HOME=<workspace>/target/android-home npm run android:build:debug`（沙盒不能写 `~/.android`，重定向后 Gradle 8.14 + AGP 出包）。第一次构建**抓到一处 Kotlin 编译错误**（API 33 的 `notifyCharacteristicChanged` 返回状态码 `Int`，与旧重载的 `Boolean` 不同 → 两分支类型不一致），已修；`aapt2 dump permissions` 确认最终 APK 含 `BLUETOOTH_ADVERTISE` / `SCAN` / `CONNECT`。
- ⚠️ **现状**：Kotlin 侧就绪并通过编译，**Rust 侧 JNI 桥接尚未实现**（注册 native 回调 + 调用 `start`/`send`），因此本类在真机上还不会被触发；真机广播/连接/GATT 读写与 iOS 侧同类实现均待做。

### Fixed (BLE 外设：蓝牙被关掉/广播失败不再静默)
- 两处"看代码看不出来"的静默故障（`81606cd`，自查上一轮落地的外设代码时发现）：
  1. **系统蓝牙被关 / 权限被撤时订阅状态会一直是旧的**：CoreBluetooth 会清空本地 GATT 数据库并断开所有 central，但**不会**回调 `didUnsubscribeFromCharacteristic:` ⇒ `is_subscribed` 仍返回 true，写任务要等 `updateValue` 失败（**最长 8s**）才收尾，而且**日志里一个字都没有**。现在：离开 `PoweredOn` 即作废全部订阅与半截消息、唤醒等待中的写任务，并为每个已订阅的 central 各发一条 `Unlinked`，让网络层**立刻**拆链路。
  2. **广播启动失败只在首次状态回调时才会被报出来**：那个 oneshot 在第一次 `peripheralManagerDidUpdateState:` 里就被 `take()`，之后（如用户关掉蓝牙再打开、重新广播失败）错误**被静默丢弃** —— 现象正是"蓝牙开着却没人能发现我们"，与手册排障表里"看日志"完全对不上。现在改走常驻的 `events` 通道，**每次**都记 `logger.warn` 并给出可操作建议。
- 同时：回到 `PoweredOn` 时**重新** `addService` + `startAdvertising`（CoreBluetooth 清过库，不重新发布就再也不会有人能连上我们）；新增纯函数 `state_label`（"蓝牙已关闭" vs "未授权（去系统设置…）"，两者的处理建议完全不同）与 `detach_targets`（离开 `PoweredOn` ⇒ 全部订阅视为失效）；`did_unsubscribe` 拆开两层锁。
- 验证：`cargo test --lib --features bluetooth` **385 passed / 0 fail / 0 warning**（+2 护栏）；`cargo test --lib` 378 / 0 warning；`cargo check --all-targets` 0 warning；Android `check-mobile.sh --bluetooth` PASS / 0 warning；前端 291 / vue-tsc 0 / vite build 通过。
- **非空转验证**：把 `detach_targets` 改成永远返回空 ⇒ 新护栏 FAIL；恢复后全绿、无残留标记。
  ⚠️ 真机上"关蓝牙 → 链路立刻消失且日志里有原因"仍需实测（本机无法触发 CoreBluetooth 状态切换）。

### Fixed (8 处点按目标 < 44pt 的触屏隐患 + 护栏 ⑧)
- iOS HIG 的最小点按目标是 **44×44pt**，而项目里图标按钮普遍 24–32px（桌面鼠标没问题，**手指容易点不中甚至误触相邻项**）。项目早有 `.tap-safe`（`:pointer: coarse` 下把热区垂直撑 +16px），但靠自觉使用；本轮实测 35 个小尺寸可交互元素里**仍有 8 处漏网**（`54123e0`）：
  - `FriendProfile` 移动端**返回键**（32×32，手机上最主要的返回入口）；
  - `NetworkSection` 删除「跨网段端点」（28×28，破坏性操作且与整行相邻）；
  - `AppearanceSection` 自定义主题**取色控件**（24×28）；
  - `LogViewer` 4 个工具按钮 + `SettingsWindow` 折叠项（桌面为主，但 Windows 触屏笔记本属粗指针）。
- **新增静态护栏 ⑧ `findSmallTapTargets`**：可交互元素（原生可交互标签或带 `@click`）+ `h-5..h-8`/`w-5..w-8` + 无 `tap-safe` → 报出；提示里写清能力边界（`tap-safe` 只补垂直 ±8px ⇒ h-7→44、h-8→48；`h-5`→36 仍不达标，必须调大）。逃生阀 `tap-target-ok`。
- **非空转验证**：去掉真实移动端返回键的 `tap-safe` → 全库扫描用例 FAIL 并精确指到 `FriendProfile.vue:50`；恢复后全绿。
- 至此触屏/键盘三件套齐备：⑥ 能点（键盘够得着）+ ⑦ 看得见（焦点环不被静默覆盖）+ ⑧ 点得中（≥44pt）。
- 验证：`npm test` **291 passed / 0 fail**（284 → 291）；`vue-tsc` 0 错误；`vite build` 通过；后端未改。
  ⚠️ `.tap-safe` 的实际手感（热区够不够、会不会与相邻按钮重叠）只能在真机触屏上确认。

### Fixed (7 处输入框的焦点环被 `outline-none` 静默盖掉 + 护栏 ⑦)
- **全局焦点环其实一条都没生效**：`style.css` 的焦点环写在 `:where(button, a, input, textarea, select, [tabindex], [contenteditable]):focus-visible` 里，而 `:where()` 让整条选择器**特异性变成 0**；Tailwind 的 `.outline-none`（`outline: 2px solid transparent; outline-offset: 2px`）是 0,1,0 ⇒ **只要元素带 `outline-none`，焦点环必定被覆盖**（透明 2px = 看不见）。源码注释里"必须包含 `[contenteditable]`，否则消息输入框看不到焦点"的**本意是对的，但因为特异性加了也不生效**（`754167b`）。
- **修掉 7 处**（全是 `outline-none` 且无替代指示）：`MessageComposer`（最高频的消息输入框）、`ConversationList` 搜索、`LogViewer` 搜索、`ChatSearchDialog` 搜索、`GroupCreateModal` 群名、`RenameGroupModal` 群名、`AddFriendModal` 搜索 —— 统一**删掉 `outline-none`**，让项目本来就设计好的全局焦点环生效。
- **两处合法例外显式声明**：`ContextMenu`（`role="menu" tabindex="-1"`）与 `ImageLightbox`（`role="dialog" tabindex="-1"`）的**容器**，焦点由内部条目承担，给弹出菜单/全屏遮罩画环只会变噪声 ⇒ 加 `focus-ring-ok` 文件级逃生阀并写明理由。（`style.css` 的 `.gosslan-select` 同为 `outline: none`，但自带 `.gosslan-select:focus { border-color }` 替代指示，合规。）
- **新增静态护栏 ⑦ `findOutlineNoneWithoutFocusRing`**：带 `outline-none` 的开标签必须自带 `focus:` / `focus-visible:` 的 `ring|border|outline|bg|shadow` 之一。
- **非空转验证**：把 `outline-none` 加回真实的消息输入框 → 全库扫描用例 FAIL 并精确指到 `MessageComposer.vue:598`；移除后全绿。
- 验证：`npm test` **284 passed / 0 fail**（278 → 284）；`vue-tsc` 0 错误；`vite build` 通过；后端未改。
  ⚠️ 桌面端点击这些输入框时会开始出现焦点环（`<input>` 聚焦即匹配 `:focus-visible`）—— 这是补上"设计好但没生效"的可见焦点，观感需真机确认（手册键盘验收那条已覆盖）。

### Fixed (5 处「能点但键盘够不着」的元素 + 静态护栏)
- **`div @click` = 只有鼠标/手指能用的按钮**：触屏能用、鼠标能用，但**键盘 Tab 不到、回车没反应**，读屏软件也只念成一段普通文本 —— 与"为 iOS 上架铺路 / 去网页感"直接冲突（原生控件天生带这些语义）。用静态扫描复核出 5 处真缺陷并全修（`16d8e91`）：
  - `GroupCreateModal` 好友选择行、`GroupMemberPanel` 可添加好友行 → 改**真按钮**（前者带 `aria-pressed` 开关语义）；
  - `MessageFileBubble`（点开文件）、`MessageImageBubble`（点开大图）→ 补 `:role`/`:tabindex`（**可用时才可聚焦**，避免把加载中的气泡做成"Tab 得到却点不动"的空按钮）+ 回车/空格处理；图片气泡另加 `aria-label`；
  - `AboutSection` 指纹（点击复制）→ 补 `role`/`tabindex`/键盘处理，**保留 `select-text`** 因此不换成 `<button>`。
  模态里的两行统一加 `type="button"`，不会误触发表单提交。
- **新增静态护栏 ⑥ `findTappableWithoutKeyboard`**：扫非交互标签上的**真实动作型** `@click`，要求同标签内有 `role` / `tabindex` / `@keydown|@keyup`（静态与 `:` 绑定都认）。两个**刻意排除**的写法（真实存在，非臆测）：`aria-hidden="true"` 的遮罩层（ActionSheet 的遮罩，Escape 由 Headless UI 的 Dialog 负责）、只有修饰符的 `@click.stop`（EmojiPicker 用来阻止冒泡，不是按钮）。
- **非空转验证**：往真实组件注入 `<div class="cursor-pointer" @click="…">` → 全库扫描用例 FAIL 并精确指到行号；移除后全绿、无残留标记。
- 验证：`npm test` **278 passed / 0 fail**（270 → 278）；`vue-tsc` 0 错误；`vite build` 通过；后端未改。
  ⚠️ 键盘可达性需真键盘走一遍（手册新增验收步骤）；静态护栏只能保证"语义补上了"，不能保证焦点顺序与视觉焦点环在所有页面都好看。

### Fixed (macOS 沙盒：用户选的目录重启后失访 —— 共享目录"变空"、收到的文件写不进去)
- **两处用户自选目录改用 security-scoped bookmark 保活**（`bce66f0` + `50cdc0d`）。App Sandbox 下用户在目录选择器里挑的目录，系统**只把访问权授予本次进程**；我们此前只把**路径字符串**存进数据库 ⇒ 重启后路径还在、权限没了：
  - **共享目录**：`read_dir` 失败 ⇒ 「共享目录」列表直接变空、对方拉不到文件；
  - **文件接收目录**：收到的文件**写不进去**（用户把接收目录改到自定义位置后重启即触发）。
  两者都**没有任何报错弹窗**，是最容易被误判成"网络问题"的一类故障。
- **修法**：存 security-scoped bookmark，启动时 `URLByResolvingBookmarkData:` 解析（**解析即隐式开始访问**，因此不再额外调 `startAccessingSecurityScopedResource` 免得引用计数只加不减），过期则用解析出来的 URL 续期并写回；**书签优先于数据库里的路径**（书签记的是"资源"，用户在 Finder 里移动/重命名目录后解析出的路径比旧路径更新），书签坏了则退回路径 —— 绝不能因为书签失效就让用户重选一次。
- **两级书签策略**：先试安全作用域书签，被拒则退回**普通书签**（不带沙盒授权，但能跟踪目录移动，且未沙盒环境里它就是完整可用的），两者都失败才退回"只存路径"。解析侧对称（选项必须与创建时一致）。这条兜底同时让本机的未沙盒测试二进制能**真实验证** objc2 调用姿势。
- 🔴 **补 `com.apple.security.files.bookmarks.app-scope`**：沙盒里缺这条权限，`NSURLBookmarkCreationWithSecurityScope` 会被系统拒绝 —— 也就是说"书签代码写了"在真机上依然不生效。⚠️ 已装旧版的 Mac 必须**重装**这一版。
- 新文件 `macos_bookmark.rs`（书签读写）、`user_dirs.rs`（两个目录共用的持久化 + 纯决策函数 `pick`）；`state.rs` 启动、`commands::set_share_dir` / `set_downloads_dir` 接线。
- 单测 +12：书签**真往返**（本机未沙盒也走通创建→解析并比对路径）、坏输入干净报错不 panic、目录被删后解析不得当成可用、端到端 `store → load` 两种模式、坏书签被清理且退回路径、书签优先/路径兜底/空路径拒绝、**两个目录互不串键**。
- **非空转验证**：① `pick` 改成"路径优先" → 2 条 FAIL；② 书签建失败时连路径也不落库 → 2 条 FAIL；③ `RECEIVE` 的键改成与 `SHARE` 相同 → 串键护栏 FAIL；恢复后全绿、无残留标记。
- 验证：`cargo test --lib` **378 passed / 0 failed / 0 warning**；`--features bluetooth` 383 passed / 0 warning；`cargo check --all-targets` 0 warning；`scripts/check-mobile.sh [--bluetooth]` Android 双 PASS / 0 warning；前端未改（npm 270 / vue-tsc 0 / vite build 通过）。
  ⚠️ 沙盒授权本身只能在**打包版**上验：手册 §4 顶部新增"选共享目录/接收目录 → ⌘Q 完全退出 → 重开 → 目录仍然可用"的验收步骤（`npm run tauri dev` 未沙盒，测不出区别）。

### Added (BLE 外设角色：手机不必与 Mac 同一 Wi-Fi 也能连入)
- **macOS 上新增 BLE peripheral（GATT server）角色**（`b317c27`）。`btleplug` 只能当 central（其 README 原文 "host/central mode only"），只能主动扫/连、**不能**被连 —— 所以只做 central 的 Mac 在蓝牙上永远不可被发现，"手机与电脑不在同一局域网也能加入"这条产品目标根本无法落地。现在 Mac 同时具备两种角色：

  ```text
  手机（Android/iOS，central）──BLE──▶ Mac（peripheral）──局域网──▶ Windows
  ```

  手机侧**零新增原生代码**（仍走 `btleplug` central 路径），Mac 负责把消息中继给同局域网的 PC。
- **线格式零改动**：广播里只放服务 UUID，特征 UUID、分片/重组、Hello 握手、验签、去重判据全部复用 central 侧那一套 —— 没有新增任何线上协议，因此不需要 ADR-0017 的能力门控。新增依赖 `objc2-core-bluetooth`（macOS target 专属 + optional，纳入 `bluetooth` feature），默认构建与其它平台完全不受影响（`cargo metadata`/Android `cargo check` 均已验证）。
- **`network/ble.rs` 里只多了一个"谁先连谁"的分支**：外设侧首帧就是对端的 Hello，验签通过后再回我们的 Hello。写/读循环泛型化为 `FrameSink`/`FrameSource` 两个私有 trait（`BleWriter`/`BleReader` 与新增的 `PeripheralSink`/`ChannelSource` 各实现一次），于是「取消息 → 序列化 → 发送 → 失败即收尾」与「收帧 → 解析 → `handle_message`」各**只有一份**实现，去重判据仍是同一个 `should_accept_inbound_public`。
- **三处必须写清的细节**（否则真机上必踩）：① BLE 外设角色**没有** "central 断开" 回调（只有取消订阅），旧链路可能早就死了而我们不知道 ⇒ 同一 BLE 端点的旧链路必须让位给新连接，否则该设备重连时会永远撞在去重判据上形成黑洞；② 路由先就位、再回 Hello（对端收到 Hello 会立刻冲刷待发队列，通道先挂上这批帧才不会被"注册还没完成"的缝隙吞掉）；③ `updateValue` 返回 `false`（对端接收窗口满）不是错误，等 `peripheralManagerIsReadyToUpdateSubscribers:` 再重试（8s 上限），未订阅时等订阅信号 —— 等待一律 `timeout + 50ms` 兜底，**绝不忙等**。
- **不阻断渲染**：delegate 回调走主队列，但回调里只做「拷字节 + 查表 + 发通道」，验签/写库/加解密全在 tokio 侧；CoreBluetooth 对象通过显式 `SendObj` 断言跨线程使用（依据 Apple 文档：manager 方法可从任意线程调用、回调串行派发到构造时给的队列），`Retained<NSData>` 等非 `Send` 对象一律在任何 `await` 之前析构，`start()` 全程同步（否则 future 会被染成 `!Send`，一路炸到 `#[tauri::command]`）。
- 单测 +3：广播净荷受 31 字节 legacy 上限约束且**故意不放**本地名；对端 `maximumUpdateValueLength` 异常值必须退回默认而**绝不返回 0**；用 clamp 出的 MTU 分片能被对端同一套重组器逐字节还原。**非空转验证**：把 MTU 下限判据改成 1 → FAIL；把 128 位 UUID 记成 16 字节 → FAIL；恢复后 PASS。
- 验证：`cargo test --lib --features bluetooth` **371 passed / 0 failed / 0 warning**；`cargo test --lib` 366 passed / 0 warning；`cargo check --all-targets` 干净；`bash scripts/check-mobile.sh --bluetooth` Android target **PASS / 0 warning**；前端未改（npm 270 / vue-tsc 0 / vite build 通过）。
  ⚠️ **射频行为未验证**（本机无第二台设备、无头运行会被 CoreBluetooth 授权弹窗挡住）：广播能否被手机发现、真实吞吐必须在真机跑，步骤见 `.workbuddy/audit/2026-09-12-真机测试手册.md` §5。手机/Windows 自己做外设（ADR-0015 的 7-f）仍未做，因此手机 ↔ Windows 之间只能经 Mac 中转。

### Fixed (macOS 沙盒缺蓝牙权限：开了开关却一个设备都发现不了)
- **`entitlements.plist` 补 `com.apple.security.device.bluetooth`，并显式声明 `bundle.macOS.infoPlist`**（`42c1108`）。两个都是"运行期才暴露、且现象具有误导性"的打包缺口：App Sandbox 下没有蓝牙权限时，CoreBluetooth 的 manager 状态会一直停在 Unauthorized —— **central（扫描/连接）也一起失效**，现象是"打开了蓝牙开关、一个设备也发现不了"，极易被误判成"对面没开蓝牙"；macOS 11+ 同样要求 Info.plist 里有 `NSBluetoothAlwaysUsageDescription`，之前只有 iOS 那一侧显式配置，现在 macOS 侧也显式指向同一个 `Info.plist`，不再依赖"自动探测同名文件"这种隐式行为。⚠️ 已装过旧版本的设备必须**重装**这一版才会带上新 entitlement。

### Changed (窄导航栏通讯录图标与选中态 + 输入框工具栏对齐)
- **通讯录图标换成 `Contact`**（用户反馈「最左侧那一栏通讯录的图标跟上面的聊天图标不像是一整套」）：原 `Users` 是「宽而扁」的双人剪影，与近正方形的聊天气泡并排时外接框与视觉重量都不一致；`Contact`（通讯录卡片）同为方形容器，两者并排才像一套。
- **导航栏选中态改为「图标 + 底色块」双通道**：原先只有图标变色、选中时还把图标填色（`fill=currentColor`）—— 填色对双人图标会变成两块墨团，且只靠颜色表达「现在在哪一栏」。现在用既有但从未被使用的 `--gosslan-rail-active`（浅色 `#cbd5e1` / 深色 `#253246`）作底色块 + `--gosslan-rail-text-active` 作图标色，图标**保持线性不填充**，与底部工具图标同一套描边语言。
- **导航栏上下两组按钮统一为 44px / 20px 图标**：中部导航原本 44px、底部工具原本 40px，两组点击热区与视觉重量不一致 → 统一为 44px（图标 20px、线宽 1.9）。
- **输入框工具栏三处修复**（用户反馈「底下工具栏这一行的内容，左右两边在视觉上不在同一条线上」「很像个网页」）：
  1. **垂直对齐**：行改 `h-7 items-center`，左右两组同高。原先发送键（`h-7 + px-4 + 13px` 文本）与图标按钮（28px 见方 + 18px 图标）是两种不同高度的行盒，`items-center` 居中两个不同行盒 ⇒ 看不出同一条中线。
  2. **两侧留白对称**：卡片 `px-3` → `px-4`，并在工具栏行加 `-mx-1`（4px）—— 编辑器文字左边缘与左侧第一个图标的**热区**边缘取同一起点，右侧发送键边缘与文字右边界对称，同时 28px 按钮的热区不越出卡片。
  3. **统一规格去"网页感"**：图标按钮 28×28 / 图标 16px / 线宽 1.75（原 18px + 默认 2，偏粗偏大）；按钮间距 8 网格（`gap-1.5`）；**发送键改为圆角实心主按钮**（`rounded-full` + `bg-primary` + 白字 + `font-medium`），无草稿时是低对比占位态。
- 验证：`npm test` **229 passed / 0 failed**；`npx vue-tsc --noEmit` 0 错误；`npx vite build` 通过。
  ⚠️ 均为观感变更，**需真机目视**（本机无头浏览器不可用）：重点看导航栏两组图标的整体感与选中态是否清楚、工具栏左右是否在同一中线、发送键有/无草稿两态。

### Changed (聊天气泡更紧凑 + 正文更清晰，含度量联动护栏)
- **气泡高度收敛、正文字重提高**（用户 2026-09-12 反馈：「气泡高度太高了，不如微信里的和谐；字重又太细了，一眼看上去不够清晰」）：文本气泡从 `px-3 py-2 leading-relaxed`（上下内边距 16px、行高 1.625）改为 `px-3 py-1.5 leading-normal`（12px、1.5），正文加 `font-medium`（500）。若隐若现的"太细"来自 400 字重在浅色画布上的笔画对比不足，500 提升辨识度又不会像 600 那样变成标题感。
- **同步虚拟列表高度度量**（关键，否则相邻消息会互相遮挡）：`previewMetrics.ts` 的 `TEXT_LINE_RATIO` 1.625 → 1.5、`TEXT_BUBBLE_PADDING` 16 → 12；`MessageItem` 里未知 kind 的兜底气泡同样改为 `py-1.5 leading-normal`（它与 `MessageTextBubble` 共用同一套高度常量）。
- **新增护栏 ⑤ `checkBubbleMetricsCoupling`**：把「组件真实排版」与「虚拟列表估算常量」这一对**必须成对演化**的值钉在一起 —— 解析气泡根元素的 `leading-*` / `py-*`，与度量文件里的 `TEXT_LINE_RATIO` / `TEXT_BUBBLE_PADDING` 交叉核对，不一致就报出并**给出应改的数值**。Tailwind 未覆盖 leadings（已确认 `tailwind.config.js` 只 extend 了 colors/fontFamily），故可静态判定。
- **非空转验证**：把 `TEXT_LINE_RATIO` 临时改回 1.625（marker 已删）→ 全库扫描用例**精确 FAIL**，恢复后 229 全绿。
- 验证：`npm test` **229 passed / 0 failed**（224 → 229）；`npx vue-tsc --noEmit` 0 错误。
  ⚠️ 属观感改动，**需真机目视**（本机无头浏览器不可用，见 §七之十二）：重点看长消息滚动时相邻气泡不遮挡、6 套配色下正文可读性。

### Fixed (M3-0b：连接健康拆开「读活性」与「写活性」)
- **半开 TCP 不再永久被判健康**（ADR-0014 §3.1 / §7 的前置补丁）。M3-0 的健康记录只有一个 `last_seen_ms`，**写成功与读成功写同一个字段**，而心跳每 5s 会给每条连接写成功一次 ⇒ 一条对端已消失、本机内核仍接受写入的链路会**永久保持「健康」**；选路若据此过滤，就会一直选中这条死路 —— 正是 ADR-0014 §7 列的失效场景。
- **拆成两个字段**：`last_write_seen_ms`（诊断口径，**不参与**判定）与 `last_read_seen_ms`（唯一「对端活着」的证据）。`is_healthy` 只看读活性 + 连续失败。`writer_loop` 写成功只刷写活性；`reader_loop` 每读到一帧刷读活性。
- **建链播种读活性**（`seed_read_seen`）：刚建好、尚未收到任何帧的连接必须算健康，否则 `should_dial` 会反复重拨（ADR-0014 §3.1 注意 ①）。播种是**一次性**的，超过阈值同样过期 —— 不是永久豁免。
- **行为零变化**：`online_state()` 在生产路径仍只用于 `[mesh] +conn` 的 `online=` 日志字段，没有任何决策读它，因此本补丁不影响现有收发。
- ⚠️ **阈值提醒（留给 M3-b）**：`PeerManager::new(10_000, 3)` 的读活性阈值是 10s，而双向心跳 5s 一次 ⇒ 容错仅一个心跳周期。今天无影响（只有日志在读），但 M3-b 一开始用健康信号做选路/在线判定，建议放宽到 ≥3 个心跳周期，否则抖动会被误判成链路故障并触发无谓换路（ADR-0014 §3.1 已记）。
- 单测 +5：**只写不读不算健康**（核心不变量）、写 19 次仍因读活性过期而判不健康、建链播种即健康且会过期、边界内仍健康、连续失败阈值；Peer 层再加 2 条（只写不读 → Offline、播种 → Online）。
- 验证：`cargo test --lib` **322 passed / 0 failed**（317 → 322）；`cargo check --all-targets` **0 warning**。

### Fixed (已读回执不再压小图片 + 选中会话可删除)
- **图片消息尺寸固定，已读头像不再把它压小**（用户 2026-09-12 反馈：「群聊里对方已读，后面会有一个已读列表和已读的小头像，那个头像会让图片稍微缩小一下……图片发出来之后，大小应该是固定的」）。根因：`MessageImageBubble` 的容器原先只有 `max-w-full`，**没有定宽** —— 图片宽度于是变成「父容器剩余宽度」的函数；而图片**没有内在宽度下限**，被压缩后不会回流，就永久变小。群聊已读回执（最多 3 个头像 + `+N`）是同一 flex 行的兄弟节点，因此**回执一出现就占宽、把图片压小**。改为容器**定宽 `w-52`（13rem，与加载骨架同宽）**：图片尺寸从此与兄弟节点无关，`max-w-full` 仅作为窄窗口下的安全下限保留；`img` 同宽 + 骨架 `w-full`，加载前后也不跳变。代价（已知并接受）：竖长图会在 13rem 框内留白，换「发出后尺寸恒定」。
- **选中的会话无法删除**（用户反馈：「选中的聊天框没法删除，自己应该是可以删除的」）。根因：删除按钮写成 `v-if="!active"` —— **选中态整个按钮不渲染**。改为**选中行常显**（`active ? 'flex' : 'hidden group-hover/conv:flex'`），并给它不透明底色 + 面板描边，避免压在摘要文字上糊在一起。删除后「顺位到下一条 / 无会话时兜底界面」**原本已由既有逻辑处理**：`deleteConversation` 会清空 `activeConv`，`ResponsiveLayout` 已有 watcher「`activeConv` 为空且列表非空 → 打开第一条」，列表为空时由 `ConversationList` 的空态提示接管。

### Fixed (群文件进度条改为「在线成员」口径)
- **在线成员都收到 = 100%，离线成员不再拖住进度条**（用户 2026-09-12 反馈：3 人群里 1 人离线，两个文件都发完却有一个「一直卡在 50%」）。此前发送方气泡进度取的是**全体 recipient 的 `max(progress)`**，离线成员永远停在 0 ⇒ 进度被永久冻在某个百分比；完成确认又把「有任意一人完成」直接写成进度 `1.0`，与离线成员数无关。
- **新口径**：进度 = **发送那一刻在线的成员**各自的字节进度的**平均**。分母在发送时**冻结**（`AppState::group_file_online_targets`），因此离线成员之后上线补发**不会**让进度条倒退（用户明确要求「补发不算在进度条里」）；发送时无人在线则恒为 0。
- **离线成员仍然可见、可补发**：气泡上的「已发送给 N 人 · M 人待上线」照旧按全体 recipient 统计，`flush_pending_group_files` 的离线补发链路一个字未改 —— 变的只是**进度条**这一项展示口径。
- 内核抽成**纯函数** `group_file_progress_from(recipients, online, fallback)`（+5 单测）：在线全到 → 1.0；**对照组**同一数据在全体口径下是 2/3（证明差异来自分母而非巧合）；离线者补发到 0.5 → 仍 1.0（不回退）；在线未全到 → 取平均；发送时无人在线 → 0（`fallback` 也被压到 0）；快照丢失 → 退回全体口径；`fallback` 越界夹紧。
- ⚠️ 调用顺序约束（已写进注释）：聚合函数**自己取 `state.db` 锁**，调用方必须在**未持有 db 锁**时调用（std Mutex 不可重入，注释里标了死锁原因）—— 完成确认路径为此显式 `drop(dbc)` 后再算。
- 验证：`cargo test --lib` **317 passed / 0 failed**（312 → 317）；`cargo check --all-targets` **0 warning**。
  ⚠️ E2E 未跑：本机会话的沙箱不允许写 `~/Library/Application Support/...`（`sqlite3` 打不开测试库），`scripts/e2e-dev.sh` 在第 3 步即报「数据库尚未初始化」。

### Fixed (截断文本补齐 hover title)
- **被 `truncate` 截断的文本补 `title`**（用户反馈「名字显示不下变成 `...`，鼠标悬停看不到完整名字」）：会话列表（名字 + 摘要）、好友列表（名字 + 在线态）、好友申请、群成员面板（成员名 + 可添加好友）、好友资料页（头部 + 大标题）、添加好友搜索结果、分享目录文件名、聊天头部标题、输入框 @候选、转发目标列表、文件气泡备注、已读成员列表、引用预览条、运行日志标题、路由端点地址、诊断面板事件。
- **新增静态护栏**（`designGuards` ④ `findTruncationWithoutTitle`）：全库扫描「带 `truncate` 类的真实 `class` 属性、同一开标签内既无 `title` / `:title` 也无 `aria-label` / `:aria-label`」的元素，精确报 `文件:行号`。这类缺陷**编译通过、测试全绿、代码看着正常**，只有真去 hover 才发现 —— 属最该由机器盯住的一类。支持跨行开标签；逃生阀为文件内 `truncate-title-ok` 注释（如父级已有整行 `aria-label` 且文案短到不可能截断）。**非空转验证**：截断但无 title → 报出；补 `:title`/`aria-label` → 通过；注释里提到 `title` → 仍报出（与 ③ 同源的假通过陷阱）；跨行 → 抓到；`truncate-title-ok` → 跳过。

### Fixed (跨网段中继稳定性)
- **定向 Gossip 帧到达目标后不再转发**：`handle_gossip` 第 4 步转发前新增 `is_target` 判定——`env.target == 本机` 时只消费不转发。此前目标节点会把自己是目标的定向帧（FriendRequest/FriendAccept 等）再洪泛给其他邻居，邻居又按 target 定向转发回来，形成冗余中转与回环，真机表现为「同网段好友申请一直中转、清掉还冒出来」。
- **单聊送达确认跨跳（`GossipKind::ChatAck`）**：接收方在 `handle_gossip` 单聊分支持久化后回发定向 Gossip 送达确认（明文 `{"msg_id":...}`，`target`=原始发送方）。此前单聊消息走 Gossip 多跳到达，但 Ack 只走 `try_send` 直连，跨 Tailscale 无直连时送达确认永远到不了发送方，消息状态卡在 `sent` 一直转圈。发送方按 `outbox(msg_id, sender)` 命中才接受，防伪造送达。
- **单聊已读回执跨跳（`GossipKind::ChatReadReceipt`）**：`mark_read` / `flush_pending_reads` 改走 `send_read_receipt_route`——有直连走 `Message::ReadReceipt`，无直连（跨跳）走定向 Gossip。修复跨网段聊天「双方都看到了却始终没有已读回执」。
- **拒绝好友申请不再被发送失败阻塞**：`respond_friend_request` 拒绝分支 `try_send(...).await?` 改为 `let _ = ...`——此前跨跳无直连时拒绝回执发不出去会导致 `pending_requests` 不删除、申请「清掉又冒出来」；现在本地清理与回执发送解耦，并补发 `friend-rejected` 事件。
- **回执/确认的身份绑定**：`sender_trusted` 对 `ChatAck` / `ChatReadReceipt` 不允许 TOFU，未在 peers 表时回退到 friends 表持久化的 ed25519 公钥做身份绑定（进程重启后 peers 内存态为空时不误拒跨跳回执）。

### Changed (Phase 6 网络层演进)
- **Routed 端点 `device_id` 改为可选**：`RoutedEndpoint` 的 `device_id` 由 `String` 改为 `Option<String>`；JSON 序列化时 `None` 不写入该键（`skip_serializing_if`）；旧格式 `{"device_id":"...","address":"..."}` 完全兼容，可直接被新代码反序列化。`scripts/t2-learn-id.sh` 端到端验证（向后兼容见 `discovery/routed::tests::device_id_is_optional_and_backward_compatible` 单测）。
- **主动拨号时无 peer_id 不再要求预配置身份**：`connect_to_peer` 接受 `known_id: Option<&str>`。`None` 路径遵循 §8 的 `IP:PORT → TCP → Hello → Node ID → Identity → 建立 Peer`：先发自身 Hello → 读对端回发的 Hello（被动方在「握手补全」中负责回发）→ 验签 → 学到真实身份后再登记链路 / 注册 mesh Connection / flush 待发队列。`HANDSHAKE_TIMEOUT = 5s`（大于正常握手，但覆盖「对端是未升级的旧版本、不会回发 Hello」兜底）。
- **`AppState::has_endpoint_addr(&SocketAddr)`**：身份未知时只能按端点判「要不要拨号」，否则 10s 周期重试会重复建链。方向性说明：主动方记录的 endpoint 是**对端的监听地址**（与配置一致），被动方记录的是**临时源端口**，故不会误判。
- **`add_routed_endpoint` 接受 `device_id: Option<String>`、`remove_routed_endpoint` 仅按地址匹配**：去重按地址而非 `(device_id, address)` —— 同一物理地址无论是否带 id 都是同一个端点；空字符串与 `None` 等价。
- **Routed 拨号任务的去重**：`spawn` 的拨号循环原本在循环内做 `has_endpoint` 检查；现在统一移到 `connect_to_peer` 里（按端点去重，且 `Some(id)` 时按 peer+endpoint、`None` 时按 endpoint）。理由：避免「同一判断两处实现、行为不一致」（项目踩过的坑）。

### Verification (Phase 6 Step 2)
- **T2-A 决定性验证（`scripts/t2-learn-id.sh`）**：实例1 (`--instance 1`) 配置 `[{"address":"127.0.0.1:60012"}]`（无 device_id），实例2 (`--instance 2`) 标准启动 → **7/7 PASS**：实例1 日志含 `[transport] 握手学到对端身份 peer=...-i2` + `[routed] 已连上 peer=<握手学>` + 实例1 DB `conversation_clocks` 出现 `...-i2` 行 + 实例2 DB 出现 `...-i1` 行（双向 observe_clock）+ 实例2 日志含 `[transport] 握手补全`（被动方回 Hello）+ 实例1 mesh 层 `[mesh] +conn peer=...-i2`。
- **护栏非空转验证**：在 `connect_to_peer` 的 `None` 分支临时注入「身份未知直接返回 Failed」回到旧行为，重跑 T2-A → **6/6 核心判据全部按预期 FAIL**（仅配置验证 PASS），证明判据**不是空转**。判定包括对话时钟表（已加 `DELETE FROM conversation_clocks WHERE conv_id LIKE '%-i1' OR '%-i2'` 防上次残留）。（基线 marker 已删。）
- **全门**：`cargo test --lib` 288 passed / 0 failed / 0 warning；`bash scripts/e2e-dev.sh` 30/0/1（功能零回归）；`npm test` 196 pass / 0 fail。
- **未触碰 Frozen Core**：msg_id / E2EE / Outbox / Ack / SQLite / 好友 / 文件 / 通知 / Chat UI 全部零改动。
- **后续**：「Routed 配置 UI」（让用户从好友列表选人 + 只填地址）排期独立；BLE 跨网段发现独立推进。

### Added (运行日志系统)
- **应用级运行日志**（`src-tauri/src/logging.rs`）：内存有界 ring buffer（500 条）+ 落盘文件（`logs/gosslan.log`，单文件 512 KB 超限轮转 `.old.log`，磁盘上界约 1 MB，惰性清理不另起后台任务）。生产环境（Windows release 无控制台）此前关键诊断日志只走 `eprintln!` 到 stderr 而全部丢失，现在统一进日志系统。
- **「运行日志」页**（`LogViewer.vue`）：桌面端走独立窗口（`open_log_window` 动态创建，label="logs"，系统标题栏、关闭即销毁），移动端走全屏页面（带返回）。支持滑动浏览、按级别着色（INFO/WARN/ERROR）、一键复制（时间正序）、清空（两段式确认）、自动刷新（2s 可关）。
- **入口**：桌面 NavRail 底部 + 移动端底部导航各加「日志」按钮（`ScrollText` 图标）。
- **日志规范**：写进 `logging.rs` 模块头注释——只记「可能出错」与关键状态跃迁，Info/Warn/Error 三档；不记消息正文 / 密钥等敏感内容；target 用子系统名（transport / lan / routed / mesh / friend / presence / link …）。
- **迁移现有诊断日志**：transport（握手/连接/拨号/presence/friend/link）与 network / commands / lib 启动阶段的关键 `eprintln!` 统一迁到 logger（级别、target 归一）。无 `state` 上下文的边界处（`set_abortive_close`、`await_tasks`、`tray::setup`）保留 `eprintln!`。

### Changed (P1 单聊定向化)
- **单聊消息定向投递**（`send_message`）：单聊 `GossipKind::Chat` 加 `target = 接收方`（参与签名，重签），投递改为「目标直连 → 只发它；否则广播靠中间节点按 target 定向转发」。此前单聊消息无条件 `broadcast_gossip` 全网广播，直连场景也放大到全网。投递失败**不返回 Err**（消息已落 outbox 兜底，链路竞态由 flush_outbox 补发），避免前端误判「发送失败」而重发。
- **接收端单聊消费加 target 判断**（`handle_gossip`）：`target` 存在且非本机 → 中间节点只转发不消费（防御性；即便不判断，中间节点也因 ECDH 解不开而不会落库，但明确判断语义更清晰）。
- 送达确认（ChatAck）与已读回执（ChatReadReceipt）此前已定向，本次对齐；outbox 补发走 `Message::ChatMessage` 直发、群聊无 target 广播，均不受影响。

### Changed (P1 M2 双向建链兜底)
- **小 ID 兜底拨号**（`ensure_link`）：此前只由「device_id 字典序较大」的一方拨号，小 ID 一方被动等。若大 ID 一方因单向可达（不对称 NAT/防火墙）拨不过来、或长期离线，小 ID 永远连不上。现在小 ID 在「对端在线却迟迟连不上」（首次发现超过 10s 仍无连接）时兜底主动拨号，补齐「谁能连上谁建链」的对等性；对称场景仍是大 ID 先拨（避免两端同时拨号产生重复连接）。
- 抽纯函数 `should_dial(my_id, peer_id, first_seen, now)` + 三个单测钉住「大 ID 恒拨 / 小 ID 阈值内等待 / 小 ID 超阈值兜底」，护栏非空转验证（临时禁用兜底 → 测试 FAIL）。
- `Peer.connected_since` 语义修正重命名为 `first_seen`（其值本就是「首次发现时间」而非「建链时间」，此前从未被读取）；前端 types 同步。

### Fixed (P1-2 镜像重复连接修正)
- **同一对节点不再稳定停留 2 条镜像 TCP**（`ensure_link`）：拨号判据从「**这个端点**连上了吗」提升为「和这个 peer **有连接吗**」。根因是端点表示不对称 —— 接受侧 `handle_incoming` 记录的 `Link.endpoint` 是 TCP **源地址（临时端口）**，而 `ensure_link` 拿到的是 announce 自报的**监听地址**，两者永不相等 ⇒ 被动方（小 ID）永远认为「没连上」，10s 后兜底拨号反向再拨一条，形成镜像重复连接（连接与读写任务翻倍、心跳双份，并让「断一条仍在线」的多路径判据变成假阳性）。镜像连接**不带来任何送达补偿**：`try_send` 只把消息交给 mpsc（返回 Ok 不代表 TCP 写出成功），所以它纯属浪费。
- **语义边界明确化**：`ensure_link` 只负责**连通性**（和看得见的 peer 建立联系），不负责**多路径** —— 多路径由各 Transport 自己的驱动产生（Routed 由配置驱动直接走 `connect_to_peer`、BLE 由 BLE 发现驱动，都不经过 `ensure_link`）。将来若需要「同一路径的多条连接」（如多网卡冗余），按**连接健康度**收敛，而不是放宽这一条。
- 抽纯函数 `should_dial(my_id, peer_id, has_endpoint, has_any_link, first_seen, now)`，决策顺序「已连该端点 → 已有任意连接 → 大 ID 恒拨 → 小 ID 超阈值兜底」。新增单测 `should_dial_skips_when_any_connection_already_exists` 钉住核心场景；**护栏非空转验证**：临时退回旧判据（只按端点）→ 该测试 FAIL（3 passed / 1 failed），证明它精确钉住了 P1-2 行为（marker 已删）。

### Fixed (未读徽标数字未垂直居中)
- **徽标数字在圆内偏下**（用户反馈「上宽下窄」）：2x 截图逐像素测量，圆 32 设备像素、数字墨迹 15–16、**上间隙 10 / 下间隙 7**（导航栏与会话列表三处徽标结果一致）⇒ 字形偏下 1.5 设备像素 = 0.75 CSS px。根因是 `items-center` 居中的是**行盒**，而字体的 ascent/descent 不对称、数字又没有下伸部，字形天然不在行盒正中。修法用**布局补偿**而非 transform（后者会让小字号文本在变换空间里栅格化而发虚）：固定高度盒加 `padding-bottom: 1.5px`，把行盒上移 0.75px（`(16 − 1.5 − 11) / 2 = 1.75`，未补偿时 2.5），正好抵消。
- **收敛为唯一实现 `UnreadBadge.vue`**：原先 5 处手写副本，其中 `ResponsiveLayout.vue` 的 2 处**漏了 `leading-none`** —— 同一种徽标在不同位置基线不一致（宏观「数字没居中」肉眼可见，读代码却看不出来）。同时消掉 3 份重复的「99+ 上限」逻辑。
- **新增静态护栏**（`designGuards`）：① 全库扫描禁止再手写徽标（带 `min-w-4` + danger 底色的 class 即为手写，精确报 `文件:行号`）；② 组件必须保留 `pb-[1.5px]` 与配套的 `leading-none`。**两条都做了非空转验证**（改坏 → 测试按预期 FAIL）。护栏刻意只解析真实 `class` 属性：首版用 `src.includes()` 扫全文，结果"因为注释里提到类名"而假通过，已修复并把该假通过固化成反面用例。

### Added (运行日志文本过滤)
- **日志界面文本过滤**：工具栏下方新增独立过滤条（不塞进工具栏 —— 那里已有 4 个按钮，移动端会被挤爆）。**字面包含**匹配、大小写不敏感、不做模糊/分词；命中处用与会话搜索同一套 `highlightText` 加颜色标记（`<mark>`，明暗主题各一套配色）。过滤时显示「匹配 n / 总数」，带一键清除；「没有匹配」与「暂无日志」是两种不同的空态文案。
- **判据是「所见即所匹配」**：只在界面上真实渲染出来的文本（时间 HH:MM:SS · 级别 · target · 消息）上匹配，不把未显示的日期部分纳入 —— 否则会出现「保留了这一行但整行没有高亮」的困惑。
- **过滤逻辑抽为可测纯函数 `utils/logFilter.ts`**：新增 9 条单测钉住语义（空词不过滤 / 子串命中 / 大小写不敏感 / **跨词不连续不算命中** / 正则元字符按字面处理 / 保持顺序）；另补 6 条 `utils/highlight.ts` 单测（转义安全 + 正则元字符不是模式 + 每处都标记）。

### Changed (M3-0 连接级健康信号，ADR-0014 §3.1)
- **`ConnectionHealth` 首次真正被喂上数据**：此前 `mark_seen` / `mark_failure` 只有 `mesh/peer.rs` 内部与单测在调用、`register_connection` 只 `merge` 出 `default()` 健康值，于是 `PeerManager::online_state()` 在生产路径**恒返回 Offline**（模型在、数据空 —— 与 Phase 2 review 抓到的 `upsert_connection` health 覆盖 bug 属同一类陷阱）。本步把它接上，且**纯旁路、行为零变化**：只写不读，选路仍照旧。
- **三个成功打点 + 一个失败打点**（全部复用现有帧，**零新协议**）：① 建链即打一次（否则「已建立但还没收发」的连接会被健康判据算作不健康，M3 选路会因此退化成「按固定顺序挑」甚至反复重拨 —— ADR-0014 §3.1 硬性注意 ①）；② `writer_loop` 每次写帧成功（心跳每 5s 一次 ⇒ 无业务消息时也至少每 5s 刷新）；③ `reader_loop` 每收到一帧（比「写成功」**更强**：对端确实活着，是半开 TCP 下唯一能区分真活/假活的信号）；④ 写失败记一次失败。RTT 恒为 `None` —— `Heartbeat` 是单向的、无回包，ADR-0014 明确本阶段不做 RTT，这里也不假装有数据。
- **`[mesh] +conn` 日志新增 `online=` 字段**：`ConnectionHealth` 是内存态，这是 mesh 健康信号在生产路径**唯一的外部可观测点**；没有它就只能靠读代码相信「信号接上了」。
- **验证（决定性 + 非空转）**：`bash scripts/t4-mirror-dial.sh` 实跑，三条连接（含真实局域网节点）全部 `online=1`；临时去掉建链打点 → 三条全部 `online=0`。后者同时**实测证实**了「M3-0 之前 `online_state()` 恒 Offline」这一 review 结论。
- `docs/adr/0014-multi-path-connection-selection.md` 状态 Proposed → **Accepted**（用户 2026-09-12 审核通过）。

### Added (M3-a 选路纯函数，ADR-0014 §3.2)
- **`mesh::selection::pick_link`**：多路径选路的**纯函数**（候选连接 → 该用哪一条）。策略：① 按活性过滤不健康连接 ② 路径优先级 **LAN > Routed > Bluetooth** ③ 同优先级用**建链顺序**打破平局（稳定可复现，不引入随机性）④ 全部不健康时**退回第一条**而非返回 `None`（保持可用优于报错，与改造前「首个成功即返回」的兜底一致）。
- **本步行为零变化**：函数先就位，只被单测调用，**没有接进 `try_send`** —— 按 ADR-0014 §9 Risks 把「策略」与「接线」拆开提交，接线（M3-b）出问题时可二分定位。
- **不做 RTT 排序**：`Heartbeat` 单向、无可靠往返测量来源，而 LAN 与 Tailscale 的差距由路径优先级已能区分（ADR-0014 §2）。
- `path_rank` 用**显式 match** 而非枚举声明顺序 —— 枚举顺序是巧合，以后往中间插一个变体就会静默改变选路优先级（已加单测钉住语义顺序）。
- `PeerManager` 新增 `health_timeout_ms()` / `max_failures()` 访问器：让选路复用**同一个**健康阈值，避免阈值散落两处（本项目踩过「同一判断两处实现、行为不一致」的坑）。
- 单测 +11（空集合 / 单条 / 全不健康兜底 / LAN>Routed>Bluetooth / 顺序打乱仍选 LAN / 不健康 LAN 不阻塞健康 Routed（failover 核心）/ 同优先级取先出现 / 过期不算健康 / 连续失败超阈值不算健康 / 恰好等于阈值仍算健康 / 优先级语义顺序）。**非空转验证**：临时把 LAN 降级 → 4 条优先级护栏按预期 FAIL。
- 验证：`cargo test --lib` **311 passed** / 0 warning；E2E 30/0/1（行为零变化）。

### Fixed (群成员变更不同步 + 缺系统消息)
- **群主移人后，其余成员的成员表不变小、也看不到任何提示**（用户反馈）。根因是**两头都断**：发送侧 `group_remove_member` 只把 `GroupMemberRemoved` 发给**被移除者本人**；接收侧 `handle_group_member_removed` 开头就是 `if to != 本机 { return }` —— 压根没有「别人被移出」这个分支。
  - 发送侧：现在**同时广播给其余成员**（被移除者本人仍单独通知以便清理本地群）；
  - 接收侧：补齐 `RemoveOther` 分支 —— 校验发起方确为群创建者后，同步本地成员表、清掉指向该成员的待补发群消息、落一条群内系统消息；
  - 群主自己也插一条系统消息（别人各自插入），文案「「X」已被移出群聊」/ “X” has been removed from the group。
- **主动退群也补了群内系统消息**：`leave_group` 原本已正确广播 `GroupMemberLeft`（成员表能同步），但接收端只更新成员表、不插系统消息，群里看不到「「X」退出了群聊」。现在两侧都有（同时清掉指向他的待补发群消息）。
- 成员变更的分支选择抽成纯函数 `member_removed_action`（三分支真值表），**非空转验证**：把 `RemoveOther` 改回 `Ignore` → 护栏按预期 FAIL（marker 已删）。
- 待测边界：**「其余成员是否真的收到并同步」需要 ≥3 台设备**（群主 + 被移出者 + 另一个成员），本机 E2E 只有 1 实例 + 1 对端，覆盖不到；已用单测钉住分支选择，集成留给真机回归。

## [2.1.2] - 2026-09-11

### Fixed
- **macOS 圆角外"淡淡一层颜色"（白主题淡白 / 黑主题淡黑）**：窗口背景色误用了 `--gosslan-bg`（浅 `#f1f5f9` / 深 `#0f172a`），而 body 实际底色是 `--gosslan-app-bg`（浅 `#edf1f6` / 深 `#0b1220`），两者差一档 → 圆角外透出与内容不同色的"淡淡一层"。修复：窗口背景色改用 `--gosslan-app-bg`，与内容零色差。真透明（透出桌面）需 `macos-private-api` 私有 API、会失去 App Store 上架资格，用户确认**保持可上架**，故不采用。
- **Windows 冷启动暗色下"闪一下白"**：窗口以 `visible: false` 创建，由前端挂载后调 `focus_window` 显示；但 `focus_window` 只 `show()` 没动背景色，`show()` 的第一帧会露出 WebView2 的默认背景色（`tauri.conf.json` 的 `backgroundColor` 写死浅色 `#edf1f6`），暗色主题用户在骨架合成前看到"骨架之前还有一帧白色"。修复：`focus_window` 在 `show()` 之前读后端 SQLite 的 `dark_mode`（"解析后的结果"，跟随系统时已按系统偏好算好），用 `set_background_color` 把窗口底色改成跟随主题（浅 `#edf1f6` / 深 `#0b1220`，与 body 的 `--gosslan-app-bg` 一致），第一帧即正确底色。命令消息 FIFO 顺序保证「先设色、后 show」，冷启动不再露浅色。
- **macOS 窗口圆角仍不生效 + 暗色下露白角**：此前在 `setup` 里给 contentView 设圆角，但 wry 在**窗口显示时才**用 `WryWebViewParent` 替换 NSWindow 的 contentView，setup 阶段的圆角被替换丢失；且窗口背景色写死浅色 `#edf1f6`，暗色主题下圆角外露出浅色边。修复：① 圆角改到 **WebView 加载完成后**设置（前端 `App.vue` onMounted 调新命令 `apply_macos_window_shape`），此时 contentView 已是 wry 的 parent_view；② 窗口背景色**运行时跟随主题**，消除暗色露白；③ `setHasShadow(false)` 留在 setup（NSWindow 级、不被替换）。新增 `macos_window::disable_shadow` / `apply_rounded_corners` 两函数。

## [2.1.1] - 2026-09-10

### Fixed
- **粘贴超长文本卡死输入框**：粘贴大段文字时 `execCommand("insertText")` 把整段（可能几十万字符）塞进 contenteditable，随后 input 事件里的 `innerText` 读取又强制同步 reflow，界面卡死。修复：① 粘贴前先 `slice(0, 50000)` 截断到硬上限（新增 `MAX_INPUT_LENGTH = 50_000`，与发送时的兜底截断共用同一常量）；② `syncDraftState` / `normalizeEmpty` 改用 `textContent` 替代 `innerText`（`innerText` 每次读取都触发 reflow，`textContent` 不触发布局）。发送序列化仍保留 `innerText`（只在发送时读一次，需保留 `<br>`→`\n` 换行语义）。
- **macOS 打开文件失败（App Sandbox 拦截 `/usr/bin/open`）**：`tauri-plugin-opener` 在 macOS 底层走 `open` crate → `Command::new("/usr/bin/open")`，而 App Sandbox 禁止沙盒应用 fork 外部可执行文件，故 Mac 端点开文件一律失败（Windows 端无沙盒正常）。修复：新增 `src-tauri/src/macos_open.rs`，macOS 改用 `NSWorkspace.openURL`（纯 Foundation API，沙盒允许），Windows/Linux 回落 opener；新增 `open_file_native` 命令并加 `path.exists()` 前置检查，文件不存在时返回明确错误（区分「文件不存在」与「无默认应用」）。
- **macOS 窗口四周无圆角**：跨平台用 `decorations: false` 自绘标题栏，关掉了 macOS 系统装饰（Windows 11 仍由 DWM 画圆角，所以 Mac 看起来直角、Win 看起来圆角，跨平台割裂）。修复：新增 `src-tauri/src/macos_window.rs`，运行时给窗口 `contentView` 的 layer 设 `cornerRadius: 10.0` + `masksToBounds`（公开 API；⚠️ NSWindow **没有** `setCornerRadius:`，容易误以为有——cornerRadius 是 CALayer 的属性）；同时 `setHasShadow: false`（系统阴影画在窗口外、是矩形，与圆角冲突）。**没**改 tauri.conf.json 的 `decorations`（单平台共用字段，改了会破坏 Windows 自绘）。

### Changed
- **PC 端窗口可缩到移动端宽度**：窗口最小宽度 `minWidth` 从 920 降到 **360**。`isMobile` 本就是响应式判定（`matchMedia("(max-width: 767px)")`，非平台判定），此前被 920 的 `minWidth` 挡住、PC 上永远触发不了移动端布局；现在缩窗口到 767px 以下即切换成移动端 UI（单列抽屉 + 底部导航），PC 上也能体验移动端形态。

## [2.1.0] - 2026-09-10

> 本轮为 **Apple HIG（2026 版）体验审计后的修复**，分三批 + 一次「App Store 上架前置」：
> **第一批**（改动小、收益确定）：辅助功能媒体适配、触摸端删除会话、Toast 可达性、`tap-safe`、图片 `alt`；
> **第二批**（需碰数据/结构）：搜索命中定位到消息、破坏性操作二次确认、空态行动入口；
> **第三批**（功能变更）：外观三态、macOS 原生菜单栏 + 快捷键、窗口控件系统行为、通知设置；
> **第四批**（为 iOS/macOS 上架铺路）：隐私清单、iOS 目的字符串、macOS 权限，以及第三批逻辑的单测覆盖。
> 未改任何消息投递、加密与存储语义。
> 审计报告见 `.workbuddy/artifacts/apple-hig-ux-audit-2026-09-10.md`，交付报告见 `p0-fixes-report-2026-09-10.md`。

### Added
- **macOS 原生菜单栏 + 跨平台快捷键**：自绘标题栏 + `decorations:false` 导致 macOS **没有系统菜单栏**，而 HIG 明确菜单栏是 Mac 应用的基础（⌘Q / ⌘, / ⌘W / ⌘M / 标准「编辑」项都靠它）。现在 `src-tauri/src/menu.rs` 在 macOS 建立应用菜单（关于 / 偏好设置 ⌘, / 服务 / 隐藏 / 退出 ⌘Q）、编辑（撤销/重做/剪切/复制/粘贴/全选）、会话（添加好友 ⌘N / 搜索 ⌘F）、窗口（最小化/关闭/缩放/全屏，`set_as_windows_menu_for_nsapp` 交给系统接管）。设计要点：
  - 自定义项**只发事件**、由前端执行，与快捷键走**同一条路径**（`window` 事件广播，见 `api/index.ts` 的 `APP_ACTION`），保证菜单与快捷键行为一致；
  - 菜单属"锦上添花"，**初始化失败不阻断启动**（与托盘不同，只打印日志）；
  - 跨平台快捷键 `useShortcuts`：⌘/Ctrl + `,`（设置）/ `F`（搜索）/ `N`（添加好友），**绝不碰** ⌘C/⌘V/⌘A/⌘X（那是系统编辑键）；组合态（中文输入法）放行。
- **通知设置**：新增后端键 `notify_enabled` / `notify_show_content`。设置页加「通知」分组：桌面通知开关（**打开时在用户动作上下文里请求权限**，被拒则保持关闭并提示，不再等某条消息到达才弹权限）+ 「通知显示消息内容」隐私开关（关掉后只提示"收到新消息"，锁屏/通知中心不泄正文）。
- **搜索命中可直接跳到那条消息**：此前搜到会话后点进去，用户还得自己在会话里翻——搜索只完成了一半。现在：
  - 后端 `SearchResult` 增加 `match_msg_id`（`db::search_messages_in_conv` 本就返回完整消息，只是此前没往外传）；
  - store 新增 `locateMessageInConv(convId, msgId)`：打开会话后若命中不在已加载窗口（默认最近 100 条），**逐页往前找**，上限沿用 `MAX_PAGES`（与手动上翻一致，不会为一句话翻遍整库）；找到后置 `locateRequest`，由 `ChatWindow` 滚动 + 高亮（复用既有的引用定位渲染路径）；
  - ⚠️ `locateRequest` **刻意不复用** `unreadJump`：后者会画「以下是未读消息」分割线，语义不同，复用会画错东西；
  - 返回**三态**（`found` / `not-found` / `error`）而不是布尔：翻到顶没找到、与翻页中途出错是两回事，要给不同的话——不能静默，也不能说错原因。
- **外观跟随系统（三态：跟随系统 / 浅色 / 深色）**：此前只有"深色模式"布尔开关，用户在 macOS / Android 系统里切换外观时 App **不跟随**——这是最容易被感知的"不像原生"之处（Apple HIG *Dark Mode* 要求 Respect the system appearance）。现在：
  - 新增后端设置键 `appearance_mode`（`system` | `light` | `dark`，缺省即 `system`），与既有 `dark_mode` **并存不冲突**：前者是**用户意图**，后者是**解析后的结果**（跟随系统时由前端按系统偏好算出来回写）。`save_settings` 对非法值直接忽略（宁可回落"跟随系统"也不写脏值），并已加入 `SETTINGS_KEYS` 使"恢复默认"能清干净
  - store 里 `dark` 由可写 ref 改为 **computed**（`appearance === "system" ? 系统偏好 : 强制值`）——既有大量 `app.dark` 读取处**零改动**，写入统一收敛到唯一入口 `applyAppearance()`
  - 跟随系统模式下监听 `prefers-color-scheme` 变化，系统切外观 App **即时**跟随，无需重启；强制模式下系统怎么变都不影响用户选择
  - **旧数据不丢**：≤2.0.3 只写了 `gosslan.dark` 布尔值，那是用户的一次**显式**选择；升级后按 `1→dark / 0→light` 迁移成显式模式，不会被静默改成"跟随系统"
  - `index.html` 首屏骨架的判定同步（骨架先于 bundle 执行，判定逻辑必须与 store 完全一致，否则启动瞬间会看到"骨架浅色 → 界面深色"闪一下）
- **错误文案收敛模块 `utils/errors.ts`**：把 IPC 抛出的异常转成「能读懂 + 可行动」的一句话。⚠️ 实测前提：本项目 Rust 侧**大部分错误本来就是写好的中文说明**（如「对方不是好友，请先扫描添加好友之后再继续聊天」），所以策略不是"一律换成通用文案"（那会抹掉有用信息），而是：命中需额外解释的模式 → 换成更有帮助的说法（如公钥缺失时说明"消息已保留、对方上线会自动补发"）；看起来已是给人读的 → 原样保留；其余（`os error 2` 这类 IO/库英文串）→ 换通用文案，**原文只进 console**，既不给用户看转储也不丢排查线索
- **自动化护栏 `utils/a11yLabels.ts` + 测试（含全库扫描）**：扫描 `src` 下全部 `.vue`，报出"纯图标且没有任何名字来源"的 `<button>`。判据刻意收紧（已有 `aria-label`/`aria-labelledby`（含 `:`/`v-bind:` 动态绑定）/`v-html`/可见文本或插值/带非空 `alt` 的 `<img>` 都跳过），避免误报
- **App Store 上架前置配置（macOS / iOS，均为 App Store 强制项，此前完全缺失）**：
  - **隐私清单 `src-tauri/PrivacyInfo.xcprivacy`**：声明不追踪、不采集数据（本应用纯 P2P、无服务器、无统计 SDK），并登记 Tauri 运行时用到的 required-reason API（`UserDefaults` / `FileTimestamp` / `SystemBootTime` / `DiskSpace`，各带正确的 reason 码），通过 `bundle.resources` 打入产物；
  - **iOS 目的字符串 `src-tauri/Info.plist`**：`NSLocalNetworkUsageDescription` —— 本应用靠 UDP 广播发现 + TCP 直连局域网，iOS 14+ 缺了它会在访问本地网络时被系统拦截甚至崩溃；通过 `bundle.iOS.infoPlist` 合并进默认 Info.plist（iOS 工程尚未生成，此为预置）；
  - **macOS 权限 `src-tauri/entitlements.plist`**：App Sandbox + `network.client/server` + 用户自选文件读写 + Downloads 读写，通过 `bundle.macOS.entitlements` 接入。
- **纯函数模块 `utils/appActions.ts` / `utils/shortcuts.ts` / `utils/notifications.ts`**：把「应用级动作名」「快捷键命中判定」「通知正文拼装」从 api/composable/store 里抽成零 `@/` 依赖的纯函数（Node 单测无法解析 `@/` 别名），并补齐单测。
- **状态恢复（各端统一，Apple HIG *State Restoration*）**：
  - **上次会话恢复**：打开会话即记 `gosslan.lastConv`（localStorage），重启后若该会话仍存在则自动打开——三端一致，回到上次离开的地方。
  - **窗口尺寸/位置恢复**（桌面 macOS/Windows）：接入官方 `tauri-plugin-window-state`，只持久化 `SIZE/POSITION/MAXIMIZED/FULLSCREEN`。⚠️ **刻意排除 `VISIBLE`**——本应用「关闭=隐藏到托盘」，若把可见性也持久化，会记成"关闭后是隐藏态"、重启就不显示窗口了；`DECORATIONS` 也排除（自绘标题栏由本项目管理）。
  - **移动端方向统一竖屏**：iOS `Info.plist` 补 `UISupportedInterfaceOrientations=Portrait`，与 Android 既有的 `screenOrientation="portrait"`（`scripts/inject-android-signing.mjs`）对齐——当前移动端横屏布局尚未适配（会破坏安全区/导航），等横屏就绪后再放开 iPad 多方向。
- **本地化骨架（App Store 全球上架阻断项）**：此前全中文硬编码、无任何 i18n。本轮建立：
  - `src/i18n/`：轻量字典（`zh-CN`/`en-US`）+ 响应式 `t()`（支持 `{name}` 插值）+ `applyLocale`/`isLocale`，自写而非引入 vue-i18n（文案量有限，避免新依赖）。
  - 后端新增 `language` 设置键（脏值忽略，回落中文），store 加 `language`/`setLanguage`（切语言即时生效 + 持久化）。
  - 设置页加「语言」切换入口（简体中文 / English），**覆盖系统 UI（导航栏）与设置页主框架 + 各分组标题/footer + 通知/共享/重置/清除 + 清除确认弹窗**。
  - 护栏 `src/i18n/index.test.ts`：**断言中英字典 key 集合完全一致**（漏翻译会变红）+ t() 翻译/插值/缺 key 回退。
  - ⚠️ 各分组**内部字段文案**（外观三态、气泡配色预设名、存储清理策略、网卡列表等）仍为增量待迁——切语言后这些字段暂不随动，后续批量补。
- **通知「标记已读」动作（移动端）**：Android/iOS 通知增加「标记已读」按钮（`registerActionTypes` + `actionTypeId`），点按不唤起窗口、直接标记该会话已读（发已读回执 + 清未读角标）。桌面端 Web Notification 不支持按钮，保持「点击打开」。
- **⌘/Ctrl + = / − 调整消息字号（Dynamic Type 精神）**：在 小/标准/大 三档间切换，直接改 store（`useShortcuts` 里处理，不走 window 事件），与设置页「字体大小」共用同一套 `CHAT_FONT_SIZES`。
- **macOS 滚动条恢复系统 overlay**：`html.platform-mac`（store init 按 `isMac` 标记）+ CSS 覆盖，macOS 上滚动条回到「滚动才浮出、不占布局」，Windows/Android 保留 6px 常显细滚动条。
- **移动端消息长按 → 底部 Action Sheet**：新建 `ActionSheet.vue`（底部滑出、遮罩、取消按钮、安全区），移动端长按消息唤出「复制/保存/引用/转发」等操作——此前移动端**没有右键、也没有长按**，消息操作在触屏上完全不可用（真实功能缺失）。
- **语言跟随系统（三态：跟随系统 / 简体中文 / English）**：此前语言默认写死中文。现在：
  - 后端 `language` 键扩展为 `system` | `zh-CN` | `en-US`（缺省即 `system`，脏值忽略）；
  - `src/i18n` 抽出 `detectSystemLocale()`（系统 `zh*` → 中文，其余 → 英文）+ `LanguagePreference` 三态，`locale` 改为 computed（`preference === "system" ? 系统语言 : 偏好`）；
  - 设置页语言切换改为三选一（「跟随系统」随当前语言翻译，「简体中文 / English」按国际惯例不自翻译）；
  - `index.html` 首帧脚本同步检测系统语言设置 `lang`（避免启动瞬间静态 `lang="zh-CN"` 与真界面不一致）；
  - 护栏 `src/i18n/index.test.ts` 扩展：`detectSystemLocale` 纯函数覆盖 + `refreshSystemLocale()` 重解析 + 显式偏好不受系统语言变化影响。
- **应用名本地化「相闻」/ "Gosslan"**：应用中文名「相闻」、英文名 "Gosslan"，桌面/主屏图标名按系统语言显示：
  - macOS：`src-tauri/infoplist/{en,zh-Hans}.lproj/InfoPlist.strings` 本地化 `CFBundleDisplayName`，经 `bundle.macOS.files` 精确放入 `Contents/Resources/<lang>.lproj/`（Tauri 不自动生成 InfoPlist.strings；官方推荐的 `resources` glob 也可行，这里用 `files` 显式映射更精确）；
  - iOS：`Info.plist` 设 `CFBundleDisplayName = "相闻"`（中文主市场默认），英文系统的本地化待 iOS 工程生成后补 `en.lproj/InfoPlist.strings`；
  - ⚠️ Windows 桌面快捷方式名 = `productName`（NSIS 不支持按系统语言），保持 "Gosslan" 不变（改 productName 会连带数据目录/bundle id，不推荐）。
- **英文翻译按 Apple 规范润色**：Title Case 一致性（`Mark as Read` / `Show Message Content`）、`&`→`and`、全大写强调 `NOT`→`not`、语法修正（`switch interface`→`switch the interface`）、描述用 sentence case、无障碍 label 更清晰（`My profile, {status}. Open settings.`）。
- **英文样式适配**：语言分段控件加 `flex-wrap + whitespace-nowrap`（英文「Follow System」较长，放不下换行而非溢出）；设置页 label/footer 均 flex + 自动换行，英文长文案安全。
- **README 参与贡献模块**：顶部加 release / contributors / license 徽章，License 前加「参与贡献」区块，用 `contrib.rocks` 动态展示提交量前 10 位贡献者头像。
- **聊天以外全库文案 i18n 全覆盖**：把上一轮只覆盖「系统 UI + 设置页主框架」的本地化，扩展到**除聊天消息内容外的全部用户可见文案**（44 个文件、约 500 个字典 key）：
  - 设置页 7 个 Section 内部字段（外观三态、主题色、字体、气泡配色预设名、字号档位、存储策略、网卡列表、资料、安全、关于）；
  - 所有 `title` / `aria-label`（标题栏窗口按钮、聊天头部、消息操作、回执、图片预览、文件气泡、输入区、会话/好友列表等）；
  - 所有弹窗 / 菜单 / toast / 空态 / placeholder / 状态标签（群组、好友、会话删除、共享目录、转发、诊断面板、移动端导航）；
  - `chatStyle.ts` 的气泡预设 `label` 与字号 `label` 改为 i18n key（纯数据模块零依赖，组件 `t(label)` 翻译）；
  - store/composable 的直接 toast（发送失败、权限、文件操作）统一走 `toastError` / `t()`。

### Fixed
- **触摸端无法删除会话**（真实功能缺失）：会话行的删除键写成 `hidden` + `group-hover:flex`，而 **Android 没有 hover 事件 → 该按钮永远不显示**，表现为"桌面能删、手机删不掉"（同一层的"删除好友"有长按兜底，聊天记录却没有）。新增全局工具类 `.hover-reveal` / `.hover-reveal-op`（`@media (hover: none)` 下退化为常显），并把「凡用 `group-hover` / `opacity-0` 揭示的元素都必须加其中之一」写进设计规范
- **读屏用户收不到失败反馈**：toast 是**唯一的失败反馈通道**（发送失败 / 删除失败都靠它），但容器没有 live region → 失败被静默。补 `role="status"` + `aria-live="polite"`（每条 `aria-atomic` 保证整句播报），装饰图标加 `aria-hidden`；错误停留时长 **3s → 6s**（读屏播报比扫一眼慢得多，原值常常没播完就消失）
- **错误提示直接暴露原始异常串**：31 处 `catch` 里写成 ``toast(`发送失败：${e}`)`` / `toast(String(e))`，会把 Rust 侧 `Err(String)` 原文（含 device_id、`os error 2` 之类）直接给用户看。统一改走 `app.toastError(e, "发送失败")`（14 个文件、31 处；按所在函数给了语义化前缀）
- **图标按钮在读屏下等于"无名按钮"**：全库 60 余处只写了 `title`，而 `title` 是**鼠标工具提示**、不是可访问名（触屏 VoiceOver/TalkBack 基本读不到）。为 **31 个纯图标按钮**补 `aria-label`（与 `title` 并存：一个给读屏、一个给鼠标），覆盖导航栏、标题栏窗口按钮、聊天头部、图片预览、群成员面板、输入区工具栏、申请列表等；动态值用 `:aria-label` 保持跟随
- **发送状态对读屏不可见**：回执是纯图标（转圈 / 空心圆 / 绿勾 / 红叉），读屏什么也读不到——而"发送中 / 已送达 / 已读"是聊天最核心的状态。给回执容器加 `role="img"` + 可访问名
- **列表行键盘不可达**：会话行 / 好友行是 `<div class="cursor-pointer">`，键盘用户无法 Tab 进入（`style.css` 注释里此前已自认"属后续项"）。补 `tabindex="0"` + `role="button"` + `:aria-label` + Enter/Space 激活；焦点环复用既有的全局 `:focus-visible` 规则，无需新增样式
- **未适配辅助功能媒体**：Apple 2026 HIG 明确要求适配「降低透明度 / 提高对比度」（macOS 27 另有 "Show Borders"），而项目大量使用毛玻璃且**完全没有**对应的降级。补两段媒体查询：`prefers-reduced-transparency: reduce` 时 `.frost` 退回不透明实底、`.glass`（模态遮罩）去模糊并加深压暗、`.vel-modal` 同理；`prefers-contrast: more` 时把**边界类** token（`border` / `divider` / `hover` / `window-ring`）提档。**刻意只动表面与边界、不动文字色**——文字可读性已由 `tokenContrast` 契约在亮/暗两套外观下逐对保证，在此再改会绕过那道护栏
- **44pt 最小点按目标规则"定义了却从未使用"**：`style.css` 早有 `.tap-safe`，但全库 **0 处引用**；而多数独立小图标按钮只有 24~32px，低于 Apple 的最小点按目标。本轮给 **15 处**补上（会话删除键、列表头的加号、聊天头部 4 个、申请行的同意/拒绝、共享目录的刷新/下载、群成员面板的转让/移出、文件气泡的下载、回执的重发、图片预览的关闭）。**刻意不为所有小按钮都加**，判据是「垂直方向有没有紧邻另一个可交互元素」：
  - ✗ **导航栏的图标栈**（`gap-2` 紧凑堆叠）：扩 8px 会盖到相邻按钮；
  - ✗ **输入区工具栏**（正下方是 `contenteditable` 正文区）：扩 8px 会抢走"点正文最后一行"的点击，反而更难用。
  这正是 `style.css` 里"只扩垂直、不给横向相邻控件加"那条告诫的延伸（触屏下多扩的命中区落在**非交互**空间才安全）。
- **`<img>` 全部声明 alt**：此前全库 17 处 `<img>` 无一带 alt，读屏可能念出文件名/URL。**分两类处理、不一刀切**：
  - **装饰性头像 15 处** → `alt=""`（HTML 规范里"这是装饰、读屏请跳过"的**正确**写法）——它们所在的行/按钮**已有可访问名或可见姓名**，再写一次人名只会让读屏重复念；
  - **内容图片 2 处** → 真实 alt：消息内图片 `alt="图片消息"`，图片预览用 `:alt="current?.name || '图片预览'"`（用文件名，取不到时回退）。
  - 判据是「**出现过** alt」而不是「alt 非空」——`alt=""` 合法，要禁的是"忘记写"。
- **破坏性操作的确认行为不一致**（同类的三处三种待遇）：
  - **右键删除好友此前"单击即删"**，而「删除聊天记录」和资料页的「删除好友」都有确认弹窗 → 右键那条最容易误触。现补二次确认（讲清"聊天记录保留 / 对方无法再发消息 / 可重新添加"）。
  - 这里**刻意不做"撤销"**：删除好友在后端不是可本地回滚的操作（对方可能已同步移除，重建关系要走一次好友申请），给一个做不到的"撤销"比不给更糟——所以选确认，而不是选一个假的甜点。
  - **「清除聊天数据」原本用 `window.confirm`**：那是 WebView 的系统对话框，样式与 App 完全脱节，在无边框窗口里尤其突兀。改为应用内 `BaseModal`（与其它破坏性确认同一套样式，并逐条列出"删什么 / 不删什么"）。
- **平台判定把 iOS 误判成 macOS**（iOS 上架前必须修掉）：`isMac` 用 `/Macintosh|Mac OS X/` 匹配 UA，而 iOS 的 UA 形如 `... (iPhone; CPU iPhone OS 17_0 like Mac OS X) ...`，其中 `like Mac OS X` 会命中 → iPhone/iPad 被当成 Mac（移动端错误显示红绿灯、快捷键误用 ⌘ 而非 ctrl）。改为只匹配桌面 macOS 独有的 `Macintosh`，并加 `navigator` 守卫（供 Node 单测 import）。
- **群成员面板两处仍用 `window.confirm`**（转让群主 / 退出群聊）：改为应用内 `BaseModal` 二次确认，与「清除聊天数据 / 删除好友」统一。⚠️ 剩余一处 `StorageSection` 的缓存策略确认仍用 `window.confirm` —— 它在 `watch` 里依赖**同步**弹窗 + 立即回滚的时序，改异步弹窗需重排该回滚逻辑，风险较高，留作后续。
- **截图粘贴时好时坏**：Windows 11 截图（Win+Shift+S）的剪贴板**同时**带一个临时文件引用（CF_HDROP 指向 Temp 下的 PNG），原逻辑「文件路径优先」会把截图误判成文件、去发那个可能已被清理的临时路径，导致"有时发得出去、有时发不出去"。修复两处：① `classifyPaste` 改为**图片优先于文件路径**；② `onPaste` 在**任何 `await` 之前**同步捕获图片 `File`（`files` 优先、`items.getAsFile()` 兜底）——Chromium/WebKit 会在 paste 事件返回后清空 clipboardData，先 `await` 再读 `items` 会拿到 `null`。
- **发送文件（尤其 .md）被渲染成代码块**：文件消息此前按 `subtype=code` 渲染成**内联代码预览块**（把 .js/.md 内容拉出来高亮显示），而不是文件卡片。现在**文件一律按文件卡片渲染**——代码块只来自「代码消息」（kind=code，输入框粘贴/发送的文本），文件不再依据扩展名变代码块。同时 `.md`（Markdown 是文档而非代码）从 `classify_file_subtype` 与文件卡片图标的 `code` 分类中移除，归为普通 `file`/文档图标。
- **群关系同步会凭空重建群聊会话**（产品语义错误）：`handle_group_key`（收到 GroupKey 后的群关系同步路径）在 `upsert_group` 之后**多调了一次 `ensure_conversation`**，导致用户清库/重装后仅凭群关系同步（群名/群成员/群密钥），之前加入过的群聊就会自动重新出现在聊天列表。这混淆了「群关系」与「聊天会话」——conversation 是**聊天活动驱动的会话索引**，只有收到新消息（`insert_message` + `touch_conversation`）时才应创建。修复：删掉这一处 `ensure_conversation`，保留 `upsert_group`（写 groups/group_members）与 `observe_clock`（群时钟推进）；群消息接收路径的 `touch_conversation` 不动，因此新群消息仍会正常创建会话。新增 db 层测试锁定不变量（群关系同步不建会话 / 新消息建会话 / 已有会话不被删除或重复创建）。

### Changed
- **空态只有陈述、没有下一步**：主聊天区、会话列表的「暂无会话 / 暂无好友」此前都只有一句话。新用户最常卡在"怎么加人"，现在空态直接给「添加好友」按钮（**搜索无结果时不给**——那是"换个词"的场景，不是"去加人"）。
- **`docs/design-guidelines.md` 新增 §10「系统与辅助功能跟随」**：把本轮确立的三条硬规则写进规范（辅助功能媒体必须响应、可访问名是准入项、悬停不能是唯一入口），并更新 §7.2 的"已知偏差"——「未新增跟随系统外观」已由本轮修复，从偏差表移除
- 设置页「外观」由二态开关改为**三选一分段控件**（跟随系统 / 浅色 / 深色）；导航栏的太阳/月亮按钮保留为快捷开关（语义明确为"切成显式的浅/深"，不再在城市与系统之间来回）
- **macOS 窗口控件补齐系统行为**：绿灯 option-click 进入/退出全屏（此前只有缩放，新增 `window_toggle_fullscreen`）；双击标题栏 = 缩放（仅 macOS，Windows 由 tao 原生处理）。⚠️ 系统偏好「双击标题栏的动作」无法从 WebView 读取，这里用系统默认的"缩放"，若需完全跟随需改 `decorations + titleBarStyle: Overlay`（属后续项）。
- **前端不再在 macOS 兜底 ⌘W**：此前靠 `TitleBar` 的 keydown 兜底，现由原生「窗口 → 关闭」菜单（配合 lib.rs 恢复的 NSWindow `Closable` 位）接管，避免双触发；Windows/Linux 的 Ctrl+W 兜底保留。
- **输入框移动端键盘提示**：`MessageComposer` 的 contenteditable 补 `enterkeyhint="send"`（回车即发送，iOS/Android 键盘显示"发送"而非"换行"），并按代码模式切换 `spellcheck` / `autocorrect` / `autocapitalize`（代码模式关闭纠错，避免改坏粘贴的代码）。

### 校验
- `npm test` **196/196**（原 121；新增 `errors` 8 例、`a11yLabels` 12 例、`templateBranches` 3 例、`appearance` 10 例、`designGuards` 12 例、`platform` 4 例、`shortcuts` 6 例、`notifications` 5 例、`clipboard` 1 例（图片优先回归）、`i18n` 14 例（含 `detectSystemLocale` 纯函数 + 跟随系统重解析））
- `npx vue-tsc --noEmit` 0 错误；`cargo check` 0 error / 0 warning；`cargo test --lib` **219/219**（新增 `markdown_is_a_document_not_code`）
- **降级 CSS 用无头浏览器实测计算值**（不靠推理）：`prefers-reduced-transparency` 下 `.frost` / `.glass` / `.vel-modal` 的 `backdrop-filter` 均为 `none`、`.glass` 背景变为 `rgba(0,0,0,0.62)`；`.hover-reveal` 的 `display` 确为 `flex`（证明 `!important` 压过了 Tailwind 的 `hidden`）；`prefers-contrast: more` 下 `--gosslan-border` = `#94a3b8`。⚠️ 这三段媒体查询**必须留在 `style.css` 末尾**——`.glass` / `.frost` 的定义在文件中更靠后，同优先级下"后定义者胜"，写在前面会被直接覆盖（首版即踩，已实测确认）

> **三批的完成情况**：第一批（辅助功能媒体适配、触摸端删除会话、Toast 可达性、`tap-safe`、图片 `alt`）✅；
> 第二批（搜索定位到消息、破坏性操作二次确认、空态行动入口）✅；
> 第三批（外观三态、macOS 原生菜单栏 + 快捷键、窗口控件系统行为、通知设置）✅。
> 第一批里的「抽 `IconButton` 组件」**有意未做**：图标按钮的可访问名已用 inline `aria-label` 补齐，
> 并加了全库扫描护栏；此情此景引入一个组件抽象违反 `AI_RULES §33`（不要无必要的抽象），
> 且会让 31 处改动的 diff 变大、收益为零。
> 路线图（P2 形态类：侧栏贴边/图标着色、字号覆盖整套 `--gosslan-text-*`、常显滚动条）未动。

## [2.0.3] - 2026-09-10

### Added
- **首屏骨架屏（消除启动白屏）**：此前 `index.html` 只有一个空 `#app`，样式与脚本要等 `main.ts` 执行后才生效——WebView 加载期间用户看到的是一整片纯白，容易误以为"卡了"。现在 `index.html` 内联了首屏骨架：**关键底色 + 三栏布局骨架（caption 38 / rail 64 / 列表 250 / 聊天区 / 输入条）+ 轻微呼吸动画**，并内联脚本在样式加载前先按 localStorage 恢复**亮暗主题与主色**（顺带消除"先白后暗"的闪烁）。真实数据就绪后淡出移除；另有 5s 兜底定时器，初始化异常也不会把骨架永久挡在界面上
- **聊天记录导出（纯文字单文件）**：此前**没有任何导出/备份入口**——磁盘吃紧或换机时只能看着聊天记录丢，「存储与缓存」页的清理又只管媒体，用户没有任何自救手段。现在设置页「存储」新增**导出聊天记录**：把全部会话的文字导出成**一个 Markdown 文件**（会话按最近活跃排序，逐条带时间与发送者；图片/文件消息只保留文件名，媒体本体不在里面）。刻意**不产出 HTML**：正文来自对端，导出成 HTML 再用浏览器打开等于自己造一条本机 XSS 通道；Markdown 在任何编辑器里都能读且没有执行语义。同时**不需要新增任何依赖**（`AI_RULES §25`），时区换算由前端给出偏移、Rust 侧纯整数运算

### Changed
- **深色模式按 Apple HIG 全局校准（主题关联性 + 文字可读性）**：此前深色只是"把浅色变量翻一遍"，有 11 处组件仍在写死 `emerald/neutral` 色阶、若干"与底色同值"的 token 让控件在暗色下隐形（对比 1.00 = 完全看不见）。本次逐项按可测对比度收敛到语义 token：
  - **新增暗色语义 token**：`--gosslan-success` / `--gosslan-success-ink`（填充与文字分开，绿在白底只有 2.5，文字必须深一档）/ `--gosslan-success-soft` / `--gosslan-status-offline` / `--gosslan-field`（输入框底）/ `--gosslan-hud`（toast 底）/ `--gosslan-card` `-ink` `-line`（中性文件/代码气泡）/ `--gosslan-accent-ink`（主题色当文字用）
  - **文字可读性（实测 WCAG 比值，改前 → 改后）**：绿色状态字 3.88 → **7.24**（暗）/ 3.77 → **5.48**（亮）；主题色文字 3.98 → **5.90**（暗）/ 3.68 → **6.30**（亮）；危险色文字 4.29 → **4.63**（暗）；代码块工具栏文字 2.79 → **4.59**（亮）；已读勾图标 2.32 → **5.01**（亮）。六套预置主题色在两种外观下都 ≥ 4.5
  - **主题关联性**：`--gosslan-accent-ink` 用 `color-mix` 派生（亮色压暗 28% 黑、暗色提亮 28% 白）——因为主色是用户可选、且由 `applyTheme` 以 inline style 写在 `<html>` 上，**暗色档无法用 `.dark { --gosslan-primary: … }` 覆盖**（inline 优先级更高）
  - **暗色下隐形/糊住的层**：搜索框底原本与列表栏同值（1.00）、右键菜单/表情面板的毛玻璃浮层与列表栏同值（1.00）、组件边框贴列表只有 1.28、toast 贴画布只有 1.15 → 分别抬到 **1.27 / 1.32 / 1.64 / 1.81**（对齐 Apple 深色 `tertiarySystemFill` 约 1.35 的"填充差"量级）；toast 底色在暗色下由"深灰"改为"抬亮一档"的中性灰（白字仍有 9.1:1）
  - **原生控件跟随主题**：补 `color-scheme: light/dark`，否则暗色下 `<select>` 仍弹白底下拉、滚动条/取色器永远是浅色
  - **硬编码色清零**：11 处 `bg-emerald-500` / `bg-neutral-400` / `text-emerald-500(600)` / `bg-emerald-500/10` 与 toast 的 `bg-neutral-800/90`、`useMessageDisplay` 的中性卡片色全部改为 token；`index.html` 首屏骨架的 `--boot-line` 同步更新（骨架无法用 CSS 变量，见规范 §4）
  - 校验方式：`npm test` 99/99、`npm run build` 通过，并用无头浏览器渲染 light/dark 双栏逐像素采样，确认每个 token 的**实际渲染值**与设计值一致（曾因此发现 `--gosslan-accent-ink` 的暗色档漏写）

- **亮色模式可读性按 Apple 标准校准（与上一轮暗色校准对称）**：上一轮只把暗色收敛到可测对比度，亮色沿用旧值未复核。本轮逐对实测后发现四处不达标，且都是**日常高频位置**：
  - **次要文字 `--gosslan-text-2` 不够**：设置页说明、会话摘要、占位符大量用 11~13px 小字，原 `#64748b` 落在卡片底只有 **4.11**、列表底 **4.34**（都低于正文要求的 4.5）→ 按项目既有 Slate 灰阶下走一档到 `#475569`：卡片 **6.54** / 列表 **6.92** / 面板 **7.58**
  - **危险色被当文字用**：11px 的「发送失败」、删除按钮、`[有人@我]`、错误提示在原 `#ff3b30` 上只有 **3.06~3.55** → 新增 **`--gosslan-danger-ink: #cc2418`**（面板 **5.48** / 列表 **5.00** / 卡片 **4.73**）；**填充档 `#ff3b30` 保持不变**，未读徽标、危险按钮实底、hover 底照旧走系统红
  - **警告色被当图标用**：群主皇冠、文件夹图标在原 `#ff9500` 上只有 **2.20**，连非文字的 3:1 都不到 → 新增 **`--gosslan-warning-ink: #c67600`**（面板 **3.50** / 列表 **3.20** / 卡片 **3.02**）；填充档不改
  - **离线状态点太淡**：`#a3a3a3` 在白底 **2.52** → 取 Apple `systemGray` **`#8e8e93`**（面板 **3.26**），并顺带与暗色档统一为同一个灰
  - 共 **20 处** `text-[var(--gosslan-danger/warning)]`（11 个文件）改走 `-ink` 档；10 处 `bg-[var(--gosslan-danger)]` 等**填充**用法原样保留 —— 这正是上一轮确立的「填充档 / 文字档分开」原则的对称落地
  - **新增自动化护栏**：`utils/tokenContrast.ts` + 测试直接读 `src/style.css`，按带 `why` 说明的契约表逐对核算 `:root` 与 `.dark`，并断言每个 `-ink` 档在两套外观都成对定义（漏写 `.dark` 会立刻变红）。测试数 114 → **121**
  - 校验：`npm test` 121/121；无头浏览器渲染「改前/改后」双栏逐像素采样，确认实际渲染值与设计值一致（改前 `#ff9500`/`#ff3b30`，改后 `#c67600`/`#cc2418`）
  - **有意保留的偏差已逐条登记**在 `docs/design-guidelines.md §3.3`：在线绿点 **2.54**（Apple 自家 `systemGreen` on white 仅 2.22，本应用已更亮）、未读徽标白字约 **3.5**（与 Apple 计数徽标同取舍）、亮色边框 **1.23**（亮色靠阴影表达层级，与暗色判据不同、不可互搬）

### Fixed
- **每条消息都被渲染两遍（文本/代码消息整条重复）**：`MessageItem` 的「图片已被清理」占位块写成了 `v-if`，而 `v-if / v-else-if / v-else` 是**按同层相邻性成链**的——这个 `v-if` 把「文本 → 代码 → 图片 → 文件 → 兜底」那条链**切断**了，于是链尾的兜底分支 `<div v-else>{{ message.content }}</div>` 变成一条独立链的 `v-else`，对**任何非图片/文件的消息都成立**。表现就是每条文本消息出现两个气泡；带表情的消息更明显：一个是表情气泡（`MessageTextBubble` 把 `[摊手]` 解析成表情图），另一个是兜底气泡（原样显示 `[摊手][害羞][色]` 文本）。现在把该块改回 `v-else-if`，让 7 个分支回到同一条链上，非图片/文件消息只命中第一支（渲染实测：一条消息 2 个气泡 → 1 个）
  - 该缺陷由提交 `fd02f62` 引入（当时为「媒体已被清理」加占位块时顺手写成了 `v-if`），**尚未发版**，仅存在于本地提交
  - **新增静态护栏防复发**：这类问题 `vue-tsc` 与既有单测都覆盖不到（模板分支结构不在二者覆盖面内），且修复只是一个词的差别、极易复发。新增 `src/utils/templateBranches.ts` + `templateBranches.test.ts`：解析 `<template>` 的兄弟分支链，报出「链被切断且新链末尾 `v-else` 会误命中旧链分支」与「孤儿 `v-else`」两类问题；判据刻意收紧（只在被切断的链本身是多分支时才报），因此 `v-if="mine"` 的回执、`v-if="fileDragOver"` 的拖拽提示层等**彼此独立的合法写法不会误报**。测试会用该检测器扫描 `src` 下全部 46 个 `.vue`，测试数 108 → **114**
- **「存储与缓存」整块是死 UI，数字永远为 0**：统计与清理策略都指向 `cache/` 目录，但 **P1 重构后图片/文件改落 downloads，已无任何代码往 `cache/` 写入**——于是「当前缓存 N 个文件」恒为 0、两个下拉与「立即清理」操作的都是空目录（连 footer 承诺的"删除最旧的图片/文件"也没真正发生过）。现在：
- **设置页底部文案过期**：仍写「清除聊天数据仅删除本机消息与会话」，但该操作还会**退出所有群聊**（此前只更正了二次确认弹窗里的文案，这行漏了）
- **气泡内表情偏小**：源图多为 96×96、少数 108×96，`object-contain` 下**实际墨迹比盒子还小**，1.15em 时与正文几乎同高，高分屏更显小。放大到 **1.3em**（14px 正文下 18.2px），混排更和谐
- **消息过长会被静默截断**：`send_message` / `send_group_message` 都用 `chars().take(50000)` **悄悄切掉超出部分**——用户以为整段发出去了，对方只收到前半段，本机也不留任何痕迹（违反 `AI_RULES` INV-005「不允许静默丢失」）。现在改为**超限直接报错拒发**，错误文案给出实际字符数与上限并建议分段/改用文件，与 `MAX_OUTGOING_IMAGE_BYTES`「超限一律报错拒发，绝不静默截断」的既有约定一致。上限仍是既有的 **5 万字符**（按字符计，UTF-8 下最大约 150 KB 落库），即**对正常消息零行为变化**
- **媒体被「存储清理」删掉后 UI 显示成坏图**：清理只删文件、不动消息，于是历史消息里的图片变成一个永远转圈/裂开的框、文件点开只报"打开文件失败"——用户会以为是对端发来的文件本身有问题，也不知道可以重新索取。现在新增 `media_present` 命令（复用 `read_file_preview` 的路径解析与安全边界，**只有确知文件已被删除**时才判定"已清理"），图片显示「图片已被清理 · 可向对方重新索取」占位，文件卡片显示「已被清理」并**去掉下载入口**（文件不会"等对方上线"自己回来）。在途的乐观消息一律按"存在"处理，绝不会被误标
- **暗色主题下启动仍有一片亮色闪**：窗口的 `backgroundColor` 是**静态**色值，只能在浅色/深色里二选一——改深色会让浅色用户闪一下黑。改为窗口以 `visible: false` 创建，由前端在挂载完成后立刻显示：此刻 `index.html` 的内联骨架（含主题判断）已在 DOM 且样式生效，**窗口露出来的第一帧就是骨架**。Rust 侧另有 4s 兜底（仅在确定处于隐藏态时才强制显示），前端初始化异常也不会留下"应用启动了却没有窗口"的死局
- **「添加好友」的扫描列表看不到第 201 个之后的节点**：列表此前硬截断在 200 行，超出的节点只能靠搜索找到——对设计规模（500–1000 节点）等于不可见。现在行上加了 `content-visibility: auto`（离屏行不参与布局与绘制），上限提到设计规模本身；保留上限只为挡住异常膨胀的节点表

---

## [2.0.2] - 2026-09-10

### Fixed

### Fixed
  - 统计改为**真实媒体目录**（接收的图片/文件，含历史遗留 cache 目录）+ **聊天数据库占用**，字段更名 `media_count/media_bytes/db_bytes`；「文件存储目录」行不再把长路径塞进副标题（改为截断 + 悬浮看全）
  - 保留时长 / 占用上限**真正作用于图片与文件**（恢复文案原本承诺的语义），并在改为非「永久/无限制」前**二次确认**，明确告知"历史消息里的对应图片/文件将无法再打开"；默认值不变（永久 + 无限制 = 不自动删除任何东西）
  - 「立即清理」结果文案区分情况：无可清理时明确说"当前设置下无需删除"，而不是丢一句"删除 0 个文件"
  - 已实测 13/14/16px 三档字号下，1.15em 与 1.3em 的**气泡渲染高度完全一致**（38.8 / 42.0 / 37.1px），因此不影响 VirtualList 的高度估算
- **Windows 标题栏窗口按钮的 hover 底与窗口圆角之间夹出一条缝**：上一版试图"对齐全局圆角"，给最小化/最大化/关闭三键加了 8px 圆角——结果**按钮自己的圆角曲线与窗口边界（外框 `rounded-xl` 12px）曲线不重合**，右上/右下角就夹出一条浅色月牙缝。现在**按钮不加任何圆角**，完全交给外框容器的 `rounded-xl + overflow-hidden` 裁切：两者曲线重合，hover 底与窗口边界严丝合缝（原生 Windows 的窗口按钮同样是整块矩形、由窗口圆角裁切）。组件注释已写明"不要在这里加圆角"及原因
- **表情面板里的表情太挤**：面板是 8 列 36px 格子，但**间隙只有 4px**（表情宽度的 1/9），整片网格看着"贴死"。现在间隙提到 **8px**（留白翻倍），面板宽度 340 → **360px** 与之精确配合；**表情尺寸与可视行数都不变**（8px 间隙下 300px 高度仍正好是 7 行）
  - ⚠️ 面板宽度与网格是**精确配合**的：内宽 `360 − 16(p-2) = 344 = 8 列 × 36(h-9) + 7 间隙 × 8(gap-2)`。列宽由 `1fr` 决定、格子却是固定 px，改格子或间隙时必须同步改面板宽度，否则格子会溢出列宽导致表情互相重叠
- **群里「@我」不高亮，且与「@其他人」样式不一致**：渲染端 @ 高亮用的成员名列表取自 `nicknameOf(我的 device_id)`——而我既不在自己的好友表、也不在 peers（那是"别的节点"），于是**回退成设备指纹**，别人 `@我的昵称` 匹配不上、只剩纯文本，跟 `@其他人` 的色块并列就很割裂。改为「自己」这一项直接取本机昵称
- **「完整文本」弹窗里的 @ 完全不高亮**：`linkify` 没传成员名 → 同一条消息在气泡里高亮、在弹窗里是纯文本。弹窗已接收成员名并复用同一套提及配色
- **@提及不够明显**：浅色气泡上 16% 淡底不够显眼，但**不能**靠加大底色——底色本身是 `currentColor` 的低透明度，加大它等于压低「文字/底色」对比度、跌破 `chatStyle.mentionHighlightColor` 守住的 ≥4.5:1 护栏。改为加一圈同色 1px 内描边：只强化边界、不动填充，对比度不受影响
- **气泡内表情显得拥挤**：表情 1.25em（14px 正文下 17.5px）比正文大一圈，气泡上下几乎没留白、连排表情还彼此贴死。改为 **1.15em + 横向 1px 间隙**
  - 选"缩小表情"而非"加大气泡内边距"：内边距会改变**所有**文本气泡的实际高度，必须同步 `previewMetrics.TEXT_BUBBLE_PADDING`，否则相邻消息错位；而表情尺寸受 `line-height` 约束，**不影响气泡高度与 VirtualList 的估算**
- **右键菜单可以同时展开多个（浮层未全局互斥）**：消息右键菜单的展开态原本由**每个消息项各自维护**，而右键触发的是 `contextmenu`、**不会触发 `click`**——靠 document click 关闭的兜底收不到通知，于是「先右键 A 再右键 B」会出现两个菜单并存；同类还有「消息菜单 + 好友菜单」「消息菜单 + 已读弹层」「消息菜单 + 表情面板」。现在把展开态收敛到全局注册表（`utils/popupRegistry.ts`：同一时刻只允许一个浮层展开），并统一接入**消息右键菜单 / 好友右键菜单 / 会话列表「+」下拉 / 群聊「已读成员」弹层 / 输入框表情面板**——任意一个打开都会自动收起其它浮层
  - 顺带修掉「点另一条消息的已读头像时，上一条的已读成员弹层不收起」
- **Windows 标题栏窗口按钮的 hover 底与窗口圆角之间夹出一条缝**：上一版试图"对齐全局圆角"，给最小化/最大化/关闭三键加了 8px 圆角——结果**按钮自己的圆角曲线与窗口边界（外框 `rounded-xl` 12px）曲线不重合**，右上/右下角就夹出一条浅色月牙缝。现在**按钮不加任何圆角**，完全交给外框容器的 `rounded-xl + overflow-hidden` 裁切：两者曲线重合，hover 底与窗口边界严丝合缝（原生 Windows 的窗口按钮同样是整块矩形、由窗口圆角裁切）。组件注释已写明"不要在这里加圆角"及原因
- **代码气泡的描边与尖角对不上**：代码卡片原来四边都有 1px 描边，右侧那条竖线正好贴在尖角旁，尖角像"贴上去的"。现在消息流里的代码卡片**去掉描边**——`CodeBlock` 新增 `attached` 语义（无描边 + 下圆角抹平，与下方操作条拼成一整块，操作条只留一条上分隔线）；尖角改取 `toolbar` 的等效色（尖角整段都落在 32px toolbar 高度内，取卡片本体色会比相邻 toolbar 亮一档、仍能看出台阶），做到融合无感。独立的「全文」弹窗仍是带描边的独立卡片
  - 同步修正 `previewMetrics` 的高度常量（卡片不再计边框）：截断高度 145→144、完整高度减 2px，保证 VirtualList 的估算与真实渲染仍然一致
- **群密钥不再因群主离线而死（群聊可用性）**：`group_add_member` 原先在加人时轮换群密钥，而 `handle_group_key` 只接受**群主**分发的密钥（这道校验用于防止成员伪造密钥劫持群聊，不能放宽），于是新密钥只存在于群主本机——群主一旦离线，其余成员永远拿不到，整群消息都无法解密。现在**加人不再轮换**（加人本无前向保密收益：新成员本来就没有旧密钥），改用当前密钥重发；轮换只保留在「移除成员」路径（撤权必需，且该时机群主必然在线）
- **身份密钥冲突改为用户可见告警**：同一 `device_id` 报出与已绑定值不同的公钥时（对方重装 / 有人冒名顶替），原先只在开发者诊断面板留痕，用户无感知。现在会在与该好友的会话里写入一条系统消息提示，并按 `device_id` 去重（announce 每 5s 一次，不去重会刷屏）。安全行为不变：**绝不覆盖**已绑定公钥
- **下载目录重名兜底会静默覆盖用户文件**：`unique_path` 在同名文件已达 999 个时直接返回原路径（必定已存在）→ 改为退回随机后缀，保证任何分支都不返回已存在的路径
- **误杀合法局域网地址**：删除 `handle_incoming` 中 `octets()[0] != 169 && octets()[1] != 254` 这处判断——它把「169.x 且 x.254」错写成两个独立条件（会误杀 `10.0.254.x`），且与 `is_virtual_ip()`（已覆盖 `169.254.0.0/16`）完全冗余

### Changed
- **互斥量中毒不再级联崩溃**：`std::sync::Mutex` 一律改用 `lock().unwrap_or_else(|e| e.into_inner())`（共 340 处）。此前任一处**持锁 panic** 会让互斥量中毒，之后所有加锁点都会跟着 panic，整个应用不可用；现在取回内部数据继续运行，把影响限制在最初那次 panic
- **前端消息缓存加上界**：消息缓存按会话累积且只增不减，聊天对象一多内存会持续增长。现在只保留活跃会话 + 最近使用的至多 3 个会话的内存副本（切回时从 SQLite 重新加载最新一页），淘汰判定抽为纯函数 `selectCachedConversations` 并有单测锁定
- **左右两栏头部的分隔线对不齐**：左栏列表头是 `px-3 py-2` + `h-9` 搜索框（**52px 且无底边线**），右栏 `ChatHeader` 是 `--gosslan-header-h`（56px）+ `border-b` → 永远差 4px，且右栏那条线在左栏没有对应物。左栏改用同一个 `--gosslan-header-h` 与同款 `border-b`，两栏分隔线合成一条（渲染实测同在 y=93.00）
- **群聊昵称与头像的视觉不在一条线上**：外层 flex 无 `items-*`（头像顶边 = 昵称行盒顶边），而 `text-[11px]` 继承行高 1.5 → **半行距把墨迹往下推约 3.7px**，看着"名字偏低"（中英文都有，实测 +3.0/+4.0px）。昵称行改 `leading-none` + `mb-[7px]`（总高仍 18px = `messageHeight.NICKNAME_ROW`，不影响虚拟列表估算），墨迹贴住行盒顶
- **切会话会先闪一句"暂无消息"**：消息尚未加载完时 `messages[convId]` 是空数组 → 走了空态分支。现在渲染加载骨架（过渡态而非空态），并给 `loadMessages` 补失败终态，避免骨架永久停留
- **设计系统 token 化（圆角 / 状态色 / 排版）**：圆角统一到 `--gosslan-radius-xs|sm|md|lg|xl|pill`（99 处 Tailwind 字面值 1:1 换算，零观感变化），并落实 iOS 的**同心圆角**规则（内圆角 = 外圆角 − 内边距；修掉菜单项 6px、表情格 6px、代码卡片顶部 8px 三处"内角外翻"）；危险/警告色改用语义 token（Apple 系统色，浅深两套，替换 61 处硬编码 `#e81123`/`red-*`/`amber-*`）
- **原生手感基线（触摸 / 按压 / 动效）**：关掉点击高亮、`touch-action: manipulation`（保留捏合缩放）、`overscroll-behavior` 防整页回弹；`:active` 里把过渡时长归零 → **按下瞬时反馈、松手平滑回落**（并列的可点 `<div>` 行也通过 `.cursor-pointer:active` 补上反馈）；新增 `prefers-reduced-motion` 支持与 `.tap-safe`（触屏小按钮垂直扩到 44px）
- **新增触觉反馈 `utils/haptics.ts`**：按 Apple 触觉词汇分档（light / selection / heavy / success / warning / error），**按下即反馈且不滥用**；接入发送消息、切换会话、长按菜单。Android 走 `navigator.vibrate`（已加 `VIBRATE` 权限），iOS/桌面自动 no-op
- **排版标尺（对齐 Apple HIG Text Styles）**：定 `--gosslan-text-title|body|callout|footnote|caption`（15/14/13/12/11px），**11px 为正文下限**；把 8/9/10px、13.5px 共 20 处散值收进标尺
- **新增 `docs/design-guidelines.md` 并接入 `AI_RULES.md` §42**：圆角 / 交互态 / 触觉 / 配色 / 排版 / UI 优先（不可阻断渲染）全部成文，新功能默认遵循
## [2.0.0] - 2026-09-10

> **2.0 版本：安全加固版 —— 修复「任意节点可冒用他人身份建链」的高危漏洞，并补齐群聊生命周期管理。**
> 单聊/群聊消息内容一直受 E2EE 保护，但**握手身份此前未做密码学校验**：局域网内任意设备只要
> 知道群主/好友的 `device_id`（announce 广播即可获得），就能冒名建立 TCP 链路，进而伪造
> 明文控制消息（如 `GroupMemberRemoved`）把别人的群从本地删除。本次为 `Hello` 握手加入
> Ed25519 签名与防重放校验，在建立链路前完成身份认证。同时新增「群主转让」与「退出群聊」，
> 解决群主换设备后群永久无法管理的问题。
>
> ⚠️ **破坏性变更（与 1.x 不兼容）**：`Hello` 握手新增签名要求，**2.0 与 1.x 无法互通**，
> 互聊设备需升级到同一版本。按 pre-release 规则不保留旧协议兼容分支。

### Added
- **群主转让与退出群聊**：新增命令 `transfer_group_creator`（仅当前群主，目标须为群成员）与 `leave_group`（群主须先转让，非群主可退并清理本地群记录/会话/群密钥）；新增协议 `GroupCreatorChanged` / `GroupMemberLeft` 及对应处理器（校验 `from` 为本地记录的当前群主/成员）；群成员面板新增「转让群主」与「退出群聊」入口。修复「群主换设备/卸载后群永久无法改名、加人、踢人」的僵尸群问题

### Security
- **TCP 握手身份认证（`Hello` 签名）**：`Hello` 新增 `nonce` 与 Ed25519 `sig`，签名覆盖 `device_id | tcp_port | nonce | x25519_pubkey | ed25519_pubkey`（`protocol::hello_signing_bytes`，经 serde_json 序列化避免字段拼接歧义）。接收方在**建立链路之前**验签：用 `friends`（权威）/ `peers` 中该 `device_id` 已绑定的 Ed25519 公钥校验，自报公钥必须与绑定值一致；两者皆无时才走 TOFU 用自报公钥验签。新增有界 FIFO 防重放（不依赖墙上时钟），验签失败直接拒绝连接。
  - **修复的漏洞**：此前 `Hello` 里的 `device_id` 完全未经验证即被用作链路标识，且 `handle_message` 对 `GroupMemberRemoved` / `GroupRename` / `FriendRemove` / `FriendAccept` 等**明文控制消息**只做 `from == peer_id` 的绑定校验 —— 局域网内任意设备只要在 `Hello` 中冒用群主/好友的 `device_id`（announce 广播即可获得），就能伪造 `GroupMemberRemoved` **把别人的群从本地删除**，或恶意改名、删除好友关系，并抢占链路造成失联。
  - ⚠️ **破坏性协议变更**：新旧版本的 `Hello` 互不兼容（新版本要求签名）。按 pre-release 规则不保留兼容分支；请确保互聊设备升级到同一版本。

### Fixed
- **设置页「清除数据」提示与实现不符**：提示文案仍写「不会退出群聊」，但 `clear_all_data` 早已会删除群记录/成员/群密钥（即退出所有群聊）。文案已更正，并明确列出「退出所有群聊（群聊会从列表移除）」。

## [1.2.0] - 2026-09-10

> **1.2 版本：抖音表情 + 图片相册预览 + 设置页 iOS 化重构，并修复局域网发现、群图片接收等真机回归。** 表情不再依赖 Unicode、改用 214 张抖音图片统一三端渲染；消息图片支持相册式浏览；设置页统一为 iOS 分组卡片风格；同时根治「头像撑爆 UDP 广播导致搜不到设备」与「群图片接收方加载失败」两个顽疾。

### Added
- **抖音表情**：214 个抖音评论区表情以图片形式统一三端渲染（不依赖 Unicode），输入框新增抖音式表情弹窗（上下滚动网格），选中插入 `[名字]` 语法；正文中 `[名字]` 与文字自然混合排版，正方形副格子 + `object-contain` 保持原图比例（兼容非正方形源图），只匹配已知表情名、不误吞 `[图片]`/`[代码]` 占位
- **图片相册预览**：点击消息里的图片，打开会话内全部图片的相册浏览，左右箭头 / 键盘 ←→ 切换，保留滚轮缩放 / 拖拽 / 保存 / Esc

### Changed
- **设置页 UI 重构（iOS 分组卡片风）**：抽出 `SettingsGroup` / `SettingsRow` 标准组件统一七个分区，灰底白卡 + 行分隔，分区按「个人资料 → 外观 → 聊天显示 → 网络 → 共享目录 → 存储 → 安全 → 关于 → 重置」优先级重排；头像改为可点 + hover 相机遮罩
- **标题栏平台化**：macOS 左侧红黄绿「红绿灯」（模拟系统样式，悬停整组显示符号），Windows/Linux 右侧三键（关闭键顶到窗口右缘、去掉空隙）

### Fixed
- **局域网发现失效（Message too long / EMSGSIZE）**：UDP announce 曾携带完整 base64 头像，超过 UDP 报文上限导致广播发送失败、节点互相搜不到；现从 announce 移除头像（发现只需 device_id/nickname/公钥/tcp_port），头像改由 TCP 建链后的 UserInfo 同步，且 `upsert_peer` 对 None 头像不覆盖、双向兼容旧版本
- **群图片接收方「加载失败」**：群文件 `GroupFileDone` 只 emit 带本地 path 的记录、从不更新 messages 表，`read_file_preview` 按 msg_id 反查 content 拿到 Offer 阶段无 path 的旧内容而失败；现 Done 阶段显式回填 content 的 path 与 status（与单聊 FileDone 一致）
- **头像存储爆炸风险**：头像此前无大小限制地以 base64 落库并广播；现前端限制 2MB + 中心裁剪 + 512×512 PNG 无损压缩，后端 `update_profile` 兜底拒绝超限头像
- **Cmd+W / Ctrl+W 关闭窗口**：macOS Cmd+W、Windows/Linux Ctrl+W 均可关闭窗口（隐藏到托盘），前端 keydown 兜底，不依赖系统原生菜单对无边框窗口的 Cmd+W 支持是否生效

## [1.1.0] - 2026-09-09

> **1.1 版本：夜间模式 + 微信式聊天 UI 全面优化，以及图片消息彻底移出 SQLite（P1）。** 图片不再以 base64 内联存储——粘贴/接收的图片经「本地文件 + 文件传输链路」流转，`kind` 保持 `image`，协议与 schema 零变更；同时修掉 macOS 图片粘贴无反应、附件图片无预览、发送方图片无回显三个真机回归。

### Added
- **夜间模式（深色主题）**：全项目暗色适配；暗色下「我的气泡」经 softenMineForDark 压暗，气泡/画布/文字对比度护栏卡死 ≥4.5:1
- **群聊 @提及**：输入框升级 contenteditable，@成员转为内联原子 token（主题色高亮、退格整删、光标原生管理）；新增 @成员选择器（模糊过滤/键盘导航）；被 @ 的会话在列表显示红色 [有人@我]
- **图片消息链路重构（P1）**：粘贴/拖拽的图片 data URL 仅作前端临时输入 → Rust `save_outgoing_image` 解码落盘本地文件（MIME 校验 + ≤8MiB + UUID 文件名）→ 复用既有 1:1 / 群文件传输链路 → SQLite 只存 JSON 元数据 `{name,path,size,subtype:"image"}`；新增 `delete_file` 清理发送失败孤儿文件
- **文件交互（微信式）**：文件气泡右键「复制文件」把 CF_HDROP 写入系统剪贴板（资源管理器可直接粘贴）；输入框粘贴真实文件直接发送（位图回退图片分支）

### Changed
- **微信式聊天 UI 全面优化**：气泡头像侧 CSS 尖角；文件气泡按类型着色图标（8 类）、整卡点击打开、下载态切换；图片气泡 loading/failed 占位；智能时间分割线（今天/昨天/星期/跨年）并删除消息底部常驻时间行
- **配色体系重构**：亮色改 luma 定标（二分 HSL 让任意主题色气泡亮度稳定在 207±2）、饱和度上限 0.62 雾感；派生统一走 HSL；头像颜色/取字全项目按昵称哈希统一

### Fixed
- **macOS Ctrl+V 图片粘贴无反应**：WKWebView/Safari 的 paste 事件里 items 为空、位图只经 `clipboardData.files` 暴露，此前按 items 判断误判为纯文本；现 `classifyPaste` 优先 files、items 兜底
- **附件图片无预览（仅图标）**：`read_file_preview` 在 macOS 上把 raw 字节 JSON 序列化成 `number[]`，`new Blob([number[]])` 被强转成 "137,80,78,…" 字符串导致图片损坏；现统一 `new Uint8Array(raw)` 归一成字节再消费
- **图片发送方无回显**：图片气泡 `<img loading="lazy">` 叠加 `hidden`（display:none）——懒加载图片在隐藏态永远进不了视口懒加载距离，`@load` 永不触发、骨架占位永不解除；现移除 `loading="lazy"`，图片即时解码显示
- **代码模式 Enter 无法发送**：代码消息按钮 `@mousedown` 抢占输入框焦点，Enter 激活按钮而非触发编辑器发送；现 `@mousedown.prevent` 保持编辑器焦点
- **macOS Cmd+W 无法关闭窗口**：`decorations:false` 使窗口缺失 Closable 位，系统「关闭窗口」菜单项（Cmd+W / performClose:）不可用；现窗口创建后补回 Closable 位，Cmd+W 恢复且关闭动作仍走托盘隐藏路径
- **e2e 测试基建修复**：新增 ensure_test_friend 用本次运行身份真实公钥预置好友关系；ensure_test_group 清理 group_files 残留；恢复图片粘贴回归用例

## [1.0.1] - 2026-09-09

> **1.0.0 之后的稳定性修复集合。** 聚焦两件事：Windows 上"重启/退出后再打开就永久掉线"的 LAN 顽疾，以及 1.0.0 UI 收敛留下的配色/头像一致性问题。

### Fixed
- **Windows 重启/退出后 TCP 59992 无法重新绑定导致永久掉线**：退出时监听 socket 与已建立连接没有及时释放，端口处于残留占用状态，重新启动后 `bind` 失败且无法自愈。现关闭流程显式 shutdown/close 监听与连接，并在绑定失败时按 `AddrInUse` 做退避重试与诊断上报（`transport.rs` / `mod.rs` / `tray.rs`），重启后可正常重新入网
- **Auto 模式下 UDP Discovery 绑定 `0.0.0.0` 导致对端收不到 announce**：多网卡（VPN / 虚拟适配器）环境下，广播与组播出口会被默认路由劫持到错误网卡，表现为"本机在线但节点列表恒为 0"，而 TCP 59992 却双向可达。现 Auto 模式绑定探测到的真实 LAN IP；同时不再吞掉 `set_multicast_if_v4` / `join_multicast_v4` 失败，启动即报错；`send_to` 失败记录 `broadcast_error` 事件，不再伪造 `broadcast_sent`；诊断中新增 `bound_ip` 展示实际 UDP 绑定地址（TCP 仍监听 `0.0.0.0:59992`，行为不变）
- **停止网络时端口释放不及时**：shutdown 信号在 accept / reader / discovery 的 `select!` 中不占优，停止后线程仍阻塞在读写上，端口要等超时才释放。现让 shutdown 分支优先，停止即时生效
- **文件气泡按钮配色错乱**：文件气泡中的"打开/另存"等按钮没有跟随气泡配色预设，深色/自定义配色下对比度不足。现统一走气泡配色变量
- **消息区对端头像不刷新**：会话头像更新后聊天区气泡仍用旧头像。现消息区与列表共用同一头像来源
- **默认头像底色不统一**：无头像用户此前每次渲染可能得到不同底色。现按名字哈希稳定取色，同一用户全局一致（连带会话列表、好友列表、群成员面板、转发弹窗、个人资料页）
- **聊天正文链接未自动识别**：纯文本 URL 不可点击。新增 `utils/linkify.ts`（含单测）自动识别并渲染为链接；引用块（`>` 引用）配色随之统一，跨气泡配色一致
- **文件气泡图标方块不跟随配色**：图标底板写死颜色，改用 `currentColor`，跨气泡配色自动继承

## [1.0.0] - 2026-09-08

> **正式版 1.0.0 发布。** 本版本在 UI 收敛之上，完成了 LAN Chat 稳定性与跨设备一致性：
> 群消息离线补发、文件离线补发与完成确认、每会话逻辑序号（Lamport）、传输优先级双队列、
> Android 安全区/竖屏/运行时权限、最新消息优先分页、文件进度条修复。

### Changed
- **视觉收敛：会话列表与聊天区分层、选中态改浅灰、去刺眼主色填充**（在上轮纯白极窄版之上纠偏）：
  - 三栏恢复**层次感**：侧栏浅灰 `--gosslan-rail #f2f3f5`、会话列表 `--gosslan-list #f4f5f7`、聊天区 `--gosslan-chat #fff`，告别"三块纯白靠细线"的空旷感
  - **选中态彻底去掉主题色填充**（用户反馈太丑）：`--gosslan-list-active` 由主色改为中性浅灰 `#e2e4e8`（深色 `#38393d`），会话/好友选中行不再整行变蓝/白字，文字保持深灰；侧栏激活项由主色整块改为**浅色胶囊 + 主色图标**（`--gosslan-rail-active` 改白/浅灰）
  - 侧栏加宽 52→**64px**，图标/头像加大（44px 触控块、图标 22px、本人头像 40px），激活项 `rounded-xl` + 轻投影，不再逼仄空旷
  - 列表头搜索框改**白底 + 细边框胶囊**（在浅灰栏上清晰可辨），聚焦描主色
  - 头像圆角 4→6px、气泡圆角 4→6px（更协调不锋利）
- **视觉改版：对齐微信 4.0 Windows 桌面版（按截图精确还原，自定义能力全部保留）**：此前在 `f223dbb` / `b2d6e0e` 基础上按用户截图校准的过渡版本（纯白三栏 + 52px 极窄栏 + 选中主色填充），已在本版本收敛为上述更耐看的层次化设计。**气泡配色预设、主题色取色器与色板、字号、紧凑模式等自定义项全部保留，出厂默认保持经典蓝**
- **顶部窗口条 & 聊天输入区再校准（微信 4.0）**：
  - 顶部拖拽条背景由纯白改为浅灰 app 底色 `--gosslan-app-bg`，高度收敛为 `--gosslan-title-h:30px`；盖在浅灰列表上自然融合、盖在白聊天区上呈微信那种"浅灰 caption 浮于内容上方"，不再是一条割裂的白色横杠；窗口按钮加宽到 44px 触控，关闭键 hover 走系统红 `#e81123`
  - 输入区图标改 `rounded-md`、放大到 19px、去 30px 空高挤得更紧；文本域去掉固定 72px 初高/`rows`，改 `min-h-7` 随内容自然生长（发送/清空仍即时自适应）；发送键由"大号纯白文字+高填充胶囊"改为微信式小圆角主色按钮，未输入时降为灰块禁用（主题色仍可自定义，语义不变）
- **前端组件拆分（纯结构重构，交互与视觉不变）**：`MessageItem`（668 行）拆为 `components/message/` 下的 Avatar / TextBubble / CodeBubble / FileBubble / ImageBubble / Receipt / ContentModal，并把「气泡配色与连续消息合并判定」「文件与附件预览」「复制反馈」抽成 `composables/useMessageDisplay`、`useMessageFile`、`useClipboard`；`ConversationList` 拆出会话行 / 好友行 / 好友申请 / 右键菜单，搜索抽为 `useConversationSearch`；`ChatWindow` 拆出 `ChatHeader` / `MessageComposer` / `RenameGroupModal`，行高估算抽为 `utils/messageHeight.ts`（与 `previewMetrics` 同源）；`SettingsPanel` 拆为 Profile / Appearance / ChatStyle / Network / Storage / Security / About 七个分区，并新增基础开关 `SettingsToggle`。行高估算与渲染仍共用同一套常量，虚拟列表定位与气泡高度不受影响

### Fixed
- **移动端列表↔聊天切换挤压内容**：原先靠宽度动画（`w-0` ↔ `w-full`）切换，动画期间列表内容被横向压缩。改为整屏抽屉 + `translate-x` 滑动
- **移动端底部导航与内容区未适配安全区**：导航本身有 `env(safe-area-inset-bottom)`，内容区却固定 `pb-16`，全面屏机型底部会被导航遮住。现内容区按 `calc(4rem + env(safe-area-inset-bottom))` 留位
- **移动端软键盘弹出时底部导航浮在键盘上方遮挡输入**：监听 `visualViewport`，键盘弹出即收起底部导航并收紧内容区留白
- **移动端进入好友资料页后无返回入口**：资料页加移动端返回条，可回到列表
- **移动端无法删除好友**：`contextmenu` 在触屏不会触发，现长按好友 500ms 呼出同一菜单（桌面右键行为不变）

## [0.13.1] - 2026-09-08
### Fixed
- **Android APK 构建失败（v0.13.0 起 CI 连续失败）**：自绘标题栏的窗口控制命令 `window_minimize` / `window_toggle_maximize` 调用了 Tauri 仅桌面端提供的 `WebviewWindow::minimize` / `maximize` / `unmaximize`，交叉编译到 `*-linux-android` 时报 E0599（`is_maximized` 与 `hide` 两端都有，因此只有这两个命令受影响）。现沿用仓库既有的 `focus_window` 做法补 `#[cfg(mobile)]` 实现：移动端无独立窗口概念，`window_minimize` 为 no-op、`window_toggle_maximize` 返回 `false`；两个命令在桌面/移动两端仍然都注册，桌面实现逐字未改。前端 `TitleBar.vue` 本身以 `v-if="!app.isMobile"` 只在桌面渲染，移动端行为无任何变化，桌面端功能不受影响

## [0.13.0] - 2026-09-07

### Fixed
- **聊天气泡辨识度与连续消息间距**：对方气泡统一使用更明显的蓝灰底色并增加边界；连续消息的时间标签移动到发送者消息组末尾，避免对方首条与第二条之间被额外撑开；虚拟列表通过真实 DOM 高度和 `ResizeObserver` 校正文本、代码、图片、文件及群聊回执布局，避免消息互相遮挡
- **未上线版本继续兼容明文聊天**：删除明文 `GroupMessage` 线路，直连聊天载荷不带 `enc1:` 时直接拒绝；用户聊天不再存在旧版本明文兼容分支
- **群聊已读只显示单一状态的问题**：新增成员级 `GroupReadReceipt` 和本地 `group_reads` 持久化；自己发送的群消息显示已读成员头像，最多展示 3 个头像，超出显示 `+N`，点击可展开完整成员列表
- **前端首屏包体过大**：改用 Highlight.js core + 按需语言注册，图片查看器改为异步加载，并按 Vue、Tauri、图标、代码高亮和通用依赖拆分 Rollup chunks；入口 JS 从约 1.27MB 降到约 124KB
- **文件接收的完整性与路径安全**：拒绝穿越文件名、非法/乱序/越界分片、超出声明大小的数据和不完整连接；断链会清理临时文件并报告失败，完成前同步临时文件后再原子改名
- **局域网消息身份与权限校验**：TCP 帧中的设备身份、好友关系、文件/共享目录请求和 Ack 目标均与当前连接绑定；Gossip 先验签再去重，避免伪造消息污染去重缓存；检测到已知设备公钥冲突时不再静默覆盖
- **Gossip 信封身份冒充与元数据篡改**：签名现在覆盖不可变信封字段（TTL 除外），并要求信封公钥与 Discovery/Hello 绑定的 device_id 一致；旧版仅签 message_id 的信封不再接受
- **持久化与前端竞态**：文本发送的消息和 outbox 进入同一事务；启动时损坏的身份密钥会重新持久化；文件失败事件、事件监听注册竞态和 Ack 先于文件气泡落地的状态竞态已修复
- **中继与共享目录边界**：拒绝空文件/非法中继分片，校验中继文件大小和落盘错误；共享目录枚举不再跟随符号链接
- **输入约束**：好友请求、群成员/群名、消息类型、分页范围、预览大小和文件类型增加服务端校验

## [0.12.0] - 2026-09-07
### Fixed
- **同一消息经 Direct + Gossip 双路径到达时重复计未读、重复弹通知**：`handle_gossip` 只做网络层内存去重（`gossip.is_new`），从不查业务库；而 `touch_conversation` 的 `unread = unread + ?` 是无条件累加。于是「Direct 先落库 → Gossip 后到」这一顺序下，数据库靠 `INSERT OR IGNORE` 只留一行，但**未读又 +1、`message-received` 又发一次**（前端 `mergeMessages` 会丢掉重复气泡，却仍会重复计未读并重复发系统通知）。现两条路径统一由新增的 `db::insert_message_if_new` 裁决——判定与插入合并在同一条 `INSERT OR IGNORE` 的受影响行数里完成（不是先查后写），只有 `fresh=true` 的一方执行 `touch_conversation(+1)` 与 `emit("message-received")`；Direct 的 Ack 与 fresh 无关（消息已在库中即代表成功接收），Gossip 的 TTL fan-out 在落库判定之前、完全不受本地是否已有影响，两层去重保持独立。新增 5 条回归测试覆盖 Direct→Gossip、Gossip→Direct、同一 msg_id 重复投递、8 线程并发同一 msg_id 只有一个首次插入者，以及「业务层判为重复」与「传播层仍判为首次见到、可继续 fan-out」两条件同时成立
- **落库裁决三态化：数据库真故障不再被误当成「重复」而照常 Ack**（与上一条同批修复的加固）：`insert_message_if_new` 明确区分 `Ok(true)` 新插入 / `Ok(false)` 唯一约束命中 / `Err` 数据库故障，调用方不得再用 `unwrap_or(false)` 把 `Err` 折叠成 `false`。Direct 分支据此在 `Err` 时**既不投递也绝不 Ack**——Ack 会让发送方删除 outbox 行，一次临时 SQLite 故障就此变成永久丢消息（与 P0-2 同源的红线）。Gossip 的单聊与群聊共用同一落库块，`GossipKind::Chat` 与 `GossipKind::Group` 均已分别覆盖测试。累计 9 条回归测试。另把 `Message::GroupMessage` 的 TCP 落库分支也套上同一裁决，至此三条落库路径（单聊直发 / Gossip / 群聊直发）副作用判定完全一致，不再存在无条件计未读并投递的群聊形态代码
- **E2EE 解密失败永久烧掉真实 msg_id（消息内容不可恢复）**：直连单聊 `ChatMessage` 解不开时（缺对方公钥 / 对方已换身份 / 密文损坏），旧实现把 `[加密消息] …` 占位文本**以原始 `msg_id` 写入 messages 并照常回 Ack**。后果是双重的：① Ack 让发送方删除 outbox 行，重发机会就此终止；② 之后携带同一 `msg_id` 的正确副本被 `INSERT OR IGNORE` 静默吞掉，明文永久不可恢复——即「对方重装/换密钥后发来的第一条消息永远看不到」。现改为：解不开即**不落库、不 Ack**（Ack 严格保持「已成功接收并持久化」语义），`message_exists` 判定提前到解密之前；outbox 行保留由 Hello/心跳继续补发，且补发前用**当前最新公钥**从本地明文重新密封（`msg_id` 保持不变），使「暂时缺公钥」「发送方换身份」「接收方换身份」三类场景全部自愈。新增 11 条回归测试覆盖正常解密、缺公钥、双方各自轮换、密文损坏/篡改、重复投递、Direct×Gossip 竞态、以及 outbox 身份只认 msg_id
- **局域网发现退化为广播热循环（三端 CPU/流量异常，疑似「界面卡顿」主因之一）**：自适应广播周期（5/10/20s）此前从未真正生效——广播循环每轮都重建 `tokio::time::Interval`，而新建 Interval 的首个 tick 立即就绪，等于把等待清零，`announce` 以约 1ms/轮的速率连发（实测同版本 tokio 复现）。后果是 UDP 广播+组播风暴、单核打满，以及与 UI 线程争抢 `peers` 互斥锁。现改为按固定 deadline 等待（`sleep_until`），周期只在一次广播真正发出后按节点数重算，`who_has` 探测分支不再改变节拍；`shutdown` / `probe` 分支语义保持不变。自适应分档与抖动边界、以及「不得在循环内重建计时器」这一时序前提均已补单测
- **快速连续发送时最后几条永久停在「发送中」（Ack / 真实记录丢失）**：`send()` 上屏的乐观记录用 `tmp-*` 占位 msg_id，真实 msg_id 只在 `invoke` 返回后由 `replaceMessage` 就地替换；而该记录走的是 rAF 批量队列，**invoke 完全可能先于批次落地返回**——此时 `messages.value[convId]` 里还找不到 `tmp-*`，旧实现直接 `return` 把真实记录**整条丢弃**，气泡就此停在 `sending`（转圈），后端库里却早已是 delivered/read：只有重开会话读库才会恢复正常。UI 繁忙时（连发 5 条）批次被推后，因此表现为「最后 1–2 条一直转圈」。现新增 `pendingReplace` 挂起未落地的替换，由 `applyIncoming` 在批次落地的唯一入口处经 `applyReplacements` 换成真实记录（不新增气泡、不改批量管线本身）。同时 `send_message` 改为**先写 outbox 再广播**（INV-003 原本就是这一顺序）：旧顺序下心跳的 `flush_outbox` 若正好落在广播与插队之间，这一轮直发缺席、Ack 要等下一个心跳（+5s）。另修两处状态回退：`loadMessages` 的会话快照可能早于 Ack/peer-read 落库，旧实现无条件覆盖会把已推进的状态退回「发送中」，现按 `preserveDeliveryStatus` 取两者更靠后者；重投消息带回的迟到 Ack 也不再能把 `read` 改回 `delivered`（`set_message_status` 与前端 `onMessageAcked` 同时收紧）。新增 5 条前端纯函数测试 + 1 条 DB 测试
- **已读回执只在用户再次点击会话时才生效（非实时）**：`mark_read` 的 `ReadReceipt` 是**一次性即时发送**——`let _ = try_send(...)`，未建链或半开链路时直接失败并被静默丢弃，此后没有任何补发路径，对方的绿勾只能等下一次 `mark_read`（即用户再点一次会话）才会更新。现按消息补发的既有机制对称处理：失败的回执暂存到 `state.pending_reads`（同一 peer 只保留最大 `last_read_ts`，已读是单调状态），由 `flush_pending_reads` 在建链 / Hello / 心跳这三个既有触发点冲刷，不新增表、不新增协议消息、不做定时全量刷新。会话内的自动标记已读（收到消息 → 防抖 600ms → `mark_read`）与窗口重新可见补标记链路经复核无误，未改动

### Changed
- **局域网通道默认开启（桌面端）**：此前只有设置 `GOSSLAN_AUTOSTART=1` 才在启动时联网，首次安装必须手动进设置页打开开关才能收发。现启动即按偏好自动开启，偏好复用现有 `settings` 表新增键 `lan_enabled`（**不引入新的配置系统**）：键不存在即视为开启 ⇒ 新装与从旧版本升级都自动联网；用户在设置页关闭后持久化为 `0`，重启保持关闭；「恢复默认设置」清除该键 ⇒ 回到默认开启。绑定地址沿用用户已选网卡 `settings.bind_ip`（自动开启**不**改写该选择），仅当该网卡已不存在（换网络）时回落 `0.0.0.0`；`set_channel_enabled("lan")` 原先硬编码 `0.0.0.0`（会覆盖用户已选网卡）也改走同一路径。`GOSSLAN_AUTOSTART=1` 行为保持不变（强制 `0.0.0.0`、且不改动已持久化的偏好），`examples/e2e_peer.rs` 依赖不受影响。移动端本轮保持手动开启不变

## [0.11.2] - 2026-09-05
### Fixed
- **消息排序错乱（同发送者消息堆叠，Mac↔Windows 时钟偏差）**：接收方落库直接采用发送方时钟的时间戳，设备间时钟不一致时消息会插到错误位置（视觉上同一人的消息堆在一起）。现接收侧对每条消息做双向钳制——不晚于本地当前时间（防对方时钟快、消息出现在「未来」）、不早于会话内最后一条消息（防对方时钟慢、消息插到历史之前）；毫秒级精度，同毫秒由自增 id 稳定排序兜底。直发单聊 / 群聊 / Gossip 三条落库路径统一处理
- **设置面板滚动区被滚动条挤压**：滚动容器右侧留白 12px→16px 并补左侧 4px，6px 宽滚动条不再遮挡内容 1–2 像素

### Changed
- `mark_read` 的会话最新时间查询复用新增的 `db::last_message_ts`（消除重复 SQL）

## [0.11.1] - 2026-09-05
### Changed
- **文档体系重构**：`AI_PROJECT_HANDOFF.md` 全面重写为「项目全景与开发指南」（面向 AI 编程 / 源码阅读 / fork 二次开发：完整功能清单、架构代码导读、E2EE 状态机、工程约定、测试口径、v0.1→v0.11 演进时间线、演进设想与已知限制）；README 功能特性表补齐 v0.5–v0.11 全部功能（回执/托盘/删除会话/E2EE 恒开等）、修正过时的 npm 脚本说明与版本示例

## [0.11.0] - 2026-09-05
### Changed
- **端到端加密（E2EE）恒开且不可关闭**：单聊 / 群聊消息始终经 X25519 + ChaCha20-Poly1305 加密，聊天窗口顶部恒显绿锁徽标；设置面板「安全」区的开关移除，改为说明文字（加密原理 + 需先获取对方公钥的行为说明）。发送侧不再读取 `e2ee_enabled` 设置（旧库残留键在「恢复默认」时清理），并消除「对端需同样开启才能互通」的限制——接收方解密只需发送方公钥，旧版关闭 E2EE 的对端也能正常解密

## [0.10.0] - 2026-09-05
### Added
- **删除聊天记录**：消息列表每条会话项右下角新增「×」按钮（hover 行时浮现），点击弹出二次确认弹窗（提示仅本地清理、不影响对方、不影响好友关系），确认后删除该会话全部本地消息与会话行；为乐观交互（先更新 UI，失败回滚 + toast），同时若被删会话是当前打开的会话则关闭并置空 `activeConv`，避免空窗口

### Fixed
- **E2EE 公钥缺失发送失败**：原逻辑「好友表无公钥即报错『未获取对方公钥』」会让 Mac→Windows 等跨网首次加密消息直接失败；现 E2EE 开启且好友表无公钥时，自动触发一次节点探测（who_has）等待 1.2 秒后重试（让对方/中继 announce 落库），仍缺失则返回更明确的错误指引（对方离线 / 临时关闭 E2EE 后明文发送）
- **E2EE 解密失败静默丢消息**：开启 E2EE 的一方发来的 `enc1:` 消息若接收方缺公钥或公钥已更新，原逻辑 `return` 直接静默吞掉消息；现改为写入一条系统消息「[加密消息] 尚未获取 {id} 的公钥... / 解密失败...」，让用户看到失败原因而非消息凭空消失
- **E2EE 关闭时仍要求对端公钥**：原代码无论 E2EE 开关都要求对方 X25519 公钥，导致关闭 E2EE 时也报「未获取对方公钥」；现已修正为关闭时不查公钥、不派生共享密钥（性能优先，明文路径无密码学开销）

### Changed
- **消息回执移到气泡左侧**：我发出的消息回执（转圈/空心圆/绿勾/红叉）从气泡**右侧**改到**左侧**，贴近头像方向更直观（与聊天人头像同侧）
- **消息时间默认显示 `MM-DD HH:mm`**：从「HH:mm」改为更显眼的日期+时分格式（hover 时仍切换到秒级 `YYYY-MM-DD HH:mm:ss`），同分钟合并的连续消息仍仅在 hover 时显示完整时间
- **移除文本右键「复制文本」菜单**：气泡 hover 已自带复制按钮，再加右键菜单冗余；现禁用文本右键自定义菜单，恢复系统默认行为

## [0.9.0] - 2026-09-05
### Added
- **系统托盘（关闭即最小化到托盘）**：Windows / macOS 点击窗口「×」不再退出进程，改为隐藏主窗口并驻留右下角（Windows）/ 菜单栏（macOS）托盘，后台继续收发消息与通知；托盘菜单提供「显示主窗口」与「退出」，**只有选择「退出」才真正结束进程**；左键单击托盘图标也可恢复窗口；macOS 点击 Dock 图标在无可见窗口时同样恢复窗口。托盘为桌面端专属（`#[cfg(desktop)]`，Cargo 开启 `tray-icon` feature）

## [0.8.0] - 2026-09-05
### Added
- **端到端加密（E2EE）开关 + 可视化**：设置面板新增「安全」区，E2EE 开关默认**关闭**（局域网可信场景性能优先）；开启后单聊/群聊载荷经 X25519 + ChaCha20-Poly1305 加密（信封新增 `encrypted` 标志，缺失时按已加密兼容旧版本）；开关影响说明内嵌设置页；聊天窗口顶部新增锁形徽标（绿锁「端到端加密」/ 灰开锁「未加密」）实时反映状态
- **修复直发链路明文不一致**：此前只有 Gossip 路径加密、直发 ChatMessage 帧为明文；现直发内容统一受 E2EE 开关控制（加密内容带 `enc1:` 前缀，接收方用发送方 X25519 公钥解密），关闭时两条路径同为明文

### Changed
- **设备指纹前缀改为 `gosslan-`**（原 `dev-`），设置页「关于」完整显示指纹（不再省略号截断，等宽字体可选中复制）。⚠️ 破坏性：升级后设备 ID 变化，需重新添加好友（本地聊天记录保留）
- 设置面板滚动条与内容间距优化（内容区加内边距，不再被滚动条挤压）

## [0.7.0] - 2026-09-05
### Added
- **代码块渲染升级**：highlight.js 自动检测语言（常用语言子集 + 置信度阈值），无法识别时兜底纯文本；代码自动换行、**移除横向滚动条**；超过 7 行自动折叠（渐隐 + 「展开全部 N 行」按钮）；标题栏显示语言与行数
- **悬浮完整时间戳**：鼠标悬停消息时，时间行从「HH:mm」切换为「YYYY-MM-DD HH:mm:ss」；同分钟合并的消息悬浮也可查看完整时间
- **协议演进文档 `docs/protocol-design.md`**：对标 BeeBEEP（发现冗余/每连接会话密钥/断点续传）与 bitchat（Noise XX/BLE 无配对 Mesh/Store-and-Forward）的差距分析与演进路线（安全/完整/及时三维度），为蓝牙第二通道与协议抽象化提供依据

### Fixed
- **消息可靠性（Mac→Windows 丢消息根因）**：`send_message` 改为**一律写 outbox 兜底**（原逻辑仅「无直连链路」入队，链路存在但已失效/半开 TCP 时 broadcast 静默丢包且无补发）；`flush_outbox` 改为**只补发、不删行**（原「try_send 返回 Ok 即删」在半开链路上同样丢消息），outbox 行仅在收到对方 Ack 时删除，接收方按 msg_id 幂等去重
- **消息排序**：乐观消息时间戳用「发送时刻」并以会话内最新消息时间做下限钳制，消除设备间时钟偏差导致的排序错乱
- **后台消息滞留**：消息合并批处理的 rAF 调度在窗口不可见时会被浏览器暂停，改为不可见时退回 setTimeout；窗口重新可见时冲刷滞留批次
- **已读回执时机**：窗口从后台恢复时自动补发当前会话已读回执（对齐 bitchat「send read receipts on focus」实践），消除「只有长时间后的第一条消息有回执」的现象
- 文本气泡悬停复制按钮加 `whitespace-nowrap`，修复短气泡内图标换行挤压

### Changed
- **乐观交互推广**：好友同意/拒绝、删除好友改为「先更新界面、失败回滚 + toast」；文件发送拿到 transfer_id 后立即乐观上屏气泡（大小/进度随传输记录回填），消息发送此前已乐观化——所有聊天与局域网交互优先「先假定成功、失败可感知」
- **输入区重排**：代码模式/发送文件按钮与「Enter 发送 · Shift+Enter 换行 · 支持粘贴图片」提示移到输入框下方一行，发送按钮改为「图标+文字」置于行尾；输入区底部间距加宽（pb-4/px-4）
- 消息回执（转圈/空心圆/绿勾/红叉）从时间行移到**气泡侧面**挂载
- 同一分钟内同一发送者的连续消息自动合并紧凑显示（不依赖紧凑模式开关）
- 首屏分页由 300 条调整为 **100 条**，上滑按需加载更多（最多 10 页）

## [0.6.0] - 2026-09-05
### Added
- **消息发送状态回执链路**：发送中转圈 → 空心圆（对方已收到/未读）→ 绿色对勾（对方已读），失败红叉 + toast 提示；协议新增 `ReadReceipt(from, to, last_read_ts)`，`mark_read` 改为 async 并向单聊对端发送已读回执；对端将「我发出的、ts ≤ last_read_ts」的消息置为已读并推送 `peer-read` 事件；会话打开期间收到新消息自动去抖标记已读并发送回执
- **文件消息内嵌传输进度条**：发送/接收中实时显示百分比，完成后自动消失（消除"发送像卡死"的感知）
- **对方在线状态全链路可视**：会话列表单聊头像角标（绿点=在线可连接 / 灰点=离线）、聊天窗口头部对方头像 + 同款角标与「对方在线/离线」文字、联系人列表离线头像置灰
- **本人在线状态迁位**：从原聊天区顶部拓扑栏移入「设置 → 个人资料」与左侧导航头像角标（绿点），明确标识是"我"的状态

### Changed
- 移除聊天区顶部拓扑栏（节点数等信息保留在「设置 → 网络通道」；消除平均延迟"—"占位符的误读）
- 输入框体验：内容换行自动增高（上限约 5 行后滚动）、底部留白修正、打开会话自动聚焦输入框（移动端不弹软键盘）
- 消息发送改为**乐观上屏**：点击发送立即显示，后端确认后替换为真实记录

## [0.5.1] - 2026-09-05
### Fixed
- **Mac 上发送/接收消息不刷新（关键）**：消息合并走 Web Worker，在 Tauri 生产构建（WKWebView 自定义协议）下 Worker 可能加载失败——`mergeInWorker` 的 Promise 永不 resolve，发送与接收的消息全部卡在合并步骤不显示（须重开会话走查库路径才恢复）。修复：移除消息路径上的 Worker（合并为 O(n) Set 去重，微秒级），改主线程同步合并，保留 rAF 批量节流
- **发送失败静默无反馈**（表现为「点发送没反应」）：`sendMsg` 增加错误捕获与 toast 提示，失败时保留草稿

## [0.5.0] - 2026-09-05
### Added
- **聊天滚动与定位**：消息区仅纵向滚动（禁横向溢出）；滚动事件 rAF 节流 + passive 监听；`scrollToIndex` 支持跳到任意消息（任意方向无布局抖动）；打开有未读的会话自动定位到**第一条未读**（显示「以下是未读消息」分割线，向上加载历史时分割线索引随偏移）；离开底部显示「回到最新」悬浮按钮
- **长文本折叠**：超过 280 字符的消息默认折叠为 5 行，底部带「展开全文 / 收起」与「复制」按钮
- **聊天样式配置（即点即存 + 跨设备同步）**：6 套可读性配色预设（经典蓝 / 薄荷绿 / 暖阳橙 / 青瓷 / 樱花粉 / 石墨灰，明暗双主题、对比度 ≥ 4.5:1）+ 3 档字体大小 + 消息合并开关；改动立即生效并持久化（重启保留）；变更时经 `ChatStyle` 消息广播到所有已连接节点，**对方按「我的配色」渲染我发的消息**（持久化对端样式表）
- **删除好友**：联系人列表右键 → 删除好友（保留聊天记录，公钥随行移除）；对方仍出现在扫描列表可重新添加；新增 `remove_friend` 命令
- 群聊优化：连续消息合并（5 分钟内同发送者省略头像/昵称/时间戳）、≥5 分钟显示时间分割线、群消息显示发送者昵称
- E2E 新增「聊天样式同步」协议用例；VirtualList 向上加载历史时锚定旧首条消息（视口不跳动）

### Changed
- 消息气泡颜色 / 文字颜色改为样式预设驱动（覆盖原 `--gosslan-bubble-*` 变量的用法）
- `reset_settings` 额外清除聊天样式与对端样式表

## [0.4.2] - 2026-09-05
### Added
- **单机 dev 全功能验证流程**：`scripts/e2e-dev.sh` 一键脚本——无需第二台设备、不依赖 UDP 广播（who_has 单播 + TCP loopback），启动 headless 实例后运行协议级对端，覆盖除网络发现外的全部聊天功能（29 项断言全通过）
- `e2e_peer --full` 扩展模式：代码 / 图片 / 1MB 大文本消息、乱序消息、群消息、心跳保活、UserInfo 资料同步、好友申请（等待 UI 人工同意，SKIP 语义）、共享目录树、**下载方向文件传输**（app 主动发送路径，即「发送文件卡死」修复的回归验证），并补齐对应 SQLite 落库校验（ts 保真、长度完整、群会话行）
- 测试报告支持 PASS / FAIL / SKIP 三态（人工交互项不计失败）

### Fixed
- e2e-dev.sh 在 pkill 后旧进程未退完时 sqlite3 预置 share_dir 偶发失败——加重试

## [0.4.1] - 2026-09-05
### Fixed
- **UDP 发现 socket 阻塞（关键）**：`bind_udp_reusable` 把 socket2 创建的**阻塞** socket 直接交给 tokio——debug 构建直接 panic（发现任务静默死亡），release 构建虽不 panic 但阻塞 fd 挂在 kqueue/epoll 上会卡死 worker 线程（界面卡顿帮凶）。修复：转 tokio 前显式 `set_nonblocking(true)`
- **macOS/Linux 多开互发现失败**：UDP 同端口多绑定在 unix 上必须 `SO_REUSEPORT`（`SO_REUSEADDR` 仅 Windows 有效），socket2 需开 `all` feature；缺它第二个实例 network 启动即报 "Address already in use"
- **跨路径消息重复隐患**：Gossip 落库的 msg_id 带 `g-` 前缀而 outbox 补发为 UUID，两者不同导致极端竞态下重复消息。统一为 Gossip 信封的确定性 SHA-256 ID（本地记录 / Gossip 投递 / outbox 补发三处共用），接收方 `message_exists` 跨路径去重

### Added
- **协议级 E2E 验证工具**：`src-tauri/examples/e2e_peer.rs`——无 GUI 直连真实运行实例，覆盖 UDP 发现、TCP 建链、直连消息 + 去重、Gossip E2EE（X25519+AEAD+Ed25519）、文件传输全链路、outbox 离线补发，并校验 SQLite 落库与文件落盘（15 项断言，全部通过）
- `GOSSLAN_AUTOSTART=1` 环境变量：启动即自动开启局域网通道，支持 headless 多实例联调
- `lib.rs` 开放 `crypto` / `protocol` 模块供测试对端复用

## [0.4.0] - 2026-09-04
### Added
- **统一文件发送（自动路由）**：新增 `send_file_auto` 命令——有直连 TCP 链路走直连分片流，无直连自动切换切片中继，均不可达给出明确错误；前端「直接发送 / 中继发送」合并为一个按钮，无需用户选择路线
- **消息触顶分页加载**：虚拟列表滚动到顶部自动加载更早历史（每页 300 条，上限 10 页），带会话切换竞态守卫
- **会话选中态（飞书式）**：会话列表 / 联系人列表选中项左侧主色指示条 + 名称高亮；新增 `ensure_conversation` 命令，打开新好友会话时后端自动补建会话行（修复左侧无高亮项）
- **离线消息队列激活**：全网无连接时消息进入 SQLite outbox，对方上线建链后自动补发（原 `insert_outbox` 为死代码从未启用）
- **TCP 心跳探活**：每 5s 向所有已建链节点发送 Heartbeat，静默断连及时清理（在线状态修正）

### Fixed
- **文字消息发送失败**：好友公钥在加好友后才落库导致 ECDH 失败——`announce`/`FriendAccept` 路径始终同步好友公钥，`send_message` 回退在线节点表取公钥
- **发送文件界面卡死**：切片读取改为 `spawn_blocking`（不再阻塞 async runtime）；发送/接收两侧进度事件 250ms 节流；前端进度事件不再全量 `refreshTransfers()`（消除大文件 IPC 风暴）
- **间歇性连通性故障**：旧连接断开时误删新连接的发送端（按 key 而非按 channel 身份移除），现用 `same_channel` 精确清理；`connect_to_peer` 移除占位 channel 竞态
- **新会话不显示**：收到未知会话的消息时自动从后端刷新会话列表（原逻辑直接跳过）
- **历史遗留单测失败**：`outbox.msg_id` 补唯一索引（含旧库迁移 + 重复行清理）、`reassemble_out_of_order` 改用手工切片绕开最小分片钳制，31 个 Rust 单测全绿

### Changed
- **设置即点即保存**：昵称失焦 / 回车自动保存，缓存策略（保留时长 / 磁盘上限）改动即时持久化；移除「保存资料」「保存策略」按钮
- CI 注入的 `scripts/android/permissions.xml` 移除误带的 XML 声明（曾致 Android Gradle 清单解析失败、APK 打包全红）
- 会话 / 联系人列表行增加 `v-memo`，长列表渲染仅在相关字段变化时更新

## [0.3.0] - 2026-09-04
### Added
- 好友搜索流程：按需 `who_has` 群发探测（仅在打开「添加好友」时触发，启动不持续扫描）；昵称 / IP / 设备 ID 实时过滤；已是好友显示「已加好友」禁用态；500–1000 节点下列表截断渲染 + 搜索
- 全平台系统通知：应用处于后台或非当前会话时触发原生通知（含昵称 + 摘要）；点击通知唤起并聚焦窗口，自动跳转到发送者会话并清零未读
- 新增后端命令 `search_nearby_peers`、`focus_window`
- **双通道聚合传输**：`transport/` 定义 `Transport` 抽象接口 + `TransportManager`（局域网 / 蓝牙独立开关、智能分流、状态汇总），`lan.rs` 适配现有网络层，`bluetooth.rs` 提供 BLE/RFCOMM 接口契约（后端待接入）
- **异构 Mesh 中继**：`relay/mesh_router.rs` 跨链路桥接寻址、TTL 衰减、有界 RingBuffer 限流暂存（可独立测试）
- **轻量存储与缓存清理**：`storage/cache_cleaner.rs` 保留时长（3/7/30 天/永久）+ 磁盘配额自动清理 + SQLite VACUUM；缓存策略设置与「立即清理」命令
- 新增命令 `get_channel_status`、`set_channel_enabled`、`get_cache_info`、`set_cache_policy`、`clean_cache_now`

### Changed
- 聊天/群聊/文件消息的原生通知由 Rust 侧迁移到前端统一做 gating（后台 / 非当前会话），避免重复通知；好友申请类通知仍由 Rust 直接触发
- 新增 `--instance N` 多开启动参数（独立数据库/TCP 端口/设备指纹，UDP 共享 + SO_REUSEADDR），支持单机模拟多节点压测
- 新增跨平台环境配置指南 `docs/setup-windows.md` 与一键脚本（`env:check` / `env:install` / `android:build` / `dist:win:portable` / `multi:run`）

## [0.2.0] - 2026-09-04
### Added
- E2EE 端到端加密：X25519（ECDH 密钥交换）+ Ed25519（签名校验）+ ChaCha20-Poly1305（AEAD）
- Gossip 去中心化广播：Bloom Filter 概率去重 + LRU 精确去重 + Epidemic fan-out + TTL
- 大文件切片中继：64KB–512KB Chunk，BitTorrent 式 Mesh 并行分发、乱序重组
- 群聊群密钥机制：组密钥对称加密，密钥用各成员公钥 ECDH 单独加密分发
- 响应式布局：PC 三栏 / 移动端单栏滑动切换，虚拟滚动，Web Worker 异步解密
- 局域网拓扑状态栏：节点数 / 中继数 / 平均时延
- 项目更名 Lanct → **Gosslan**

### Changed
- UI 组件库由 Ant Design Vue 全面替换为 Tailwind CSS + Headless UI + Lucide
- 面向 500–1000 节点规模做性能优化：节点列表事件节流合并、自适应广播周期、去重参数适配、SQLite WAL 调优

## [0.1.0] - 2026-09-03
### Added
- 基础 P2P 局域网即时通讯：UDP 广播发现、TCP 分帧传输、好友关系、群聊、文件直传、共享目录
