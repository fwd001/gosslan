#!/usr/bin/env node
// 双实例 E2E harness（稳定版第二阶段 A-1 / v0）
//
// 设计判据见 docs/stability-roadmap.md §11：**零生产码改动** ——
// 用「停机预置 SQLite」表达用户动作（enqueue-before-deliver 保证入队即事实源），
// 用「进程信号 + 日志 + 磁盘」表达故障与断言，绝不新增任何可驱动面。
//
//   node scripts/e2e-multi-instance.mjs            # 跑 J1（文本消息 A→B 全链路）
//   node scripts/e2e-multi-instance.mjs --negative # 反向自证：B 不在，判据必须报红
//   GOSSLAN_E2E_BIN=<二进制> node scripts/e2e-multi-instance.mjs
//
// 退出码：0=全部 PASS，1=有 FAIL，2=环境不满足（没编译产物 / 平台不认识）。
// --negative 是**这条测试自己的非空转证明**（总指令§十四的「错误行为测试」）：
// 两边照常起、照常建链，只把这条消息的收件人换成幽灵 id —— 链路是好的，投递注定失败。
// 于是"报红"只能来自投递断言本身，而不是来自"B 没启动"这种基础设施噪声。
//
// 三种模式（每一个"正向绿"都要配一个"反向能红"，否则判据可能只是空转）：
//   默认                      → J1 文本 + J2 文件 + 重启，预期全绿
//   --negative                → 收件人换幽灵 id，投递断言预期报红
//   E2E_NO_ROUTED=1           → 只清掉两端预置的 routed_endpoints（改的是**判据读的那个输入**）：
//                                「Routed 端点拨出过链路」那条必须红，而旧的那圈 peer= 判据照常过
//   --fault=poison-part       → 注入脏 .part 前缀，预期全绿（产品须自愈或明确失败）
//   --fault=poison-part-lie   → 同样的注入，只把比对摘要换成必定不相等的值 ⇒ 预期报红
//   --fault=resume-prefix     → 注入②：真前缀必须被续传复用，预期全绿
//   --fault=resume-prefix-lie → 同样的注入，只把"期望已收字节数/期望摘要"换成错值 ⇒ 预期报红
//   --fault=kill-mid          → 注入③：接收中真 SIGKILL 对端，预期全绿
//   --fault=kill-mid-lie      → 同样的注入，只把"期望续发字节数/期望摘要"换成错值 ⇒ 预期报红
//   --fault=peer-freeze       → 注入④：对端被 SIGSTOP 冻住（有写无 ACK）⇒ 不许宣布送达，解冻后补齐
//   --fault=peer-freeze-lie   → 同样的注入，只换期望摘要 ⇒ 预期报红
//   --fault=src-shrunk        → 注入⑥：入队后源文件被改小 ⇒ 按磁盘真值收发，两侧终态一致
//   --fault=src-shrunk-lie    → 同样的注入，只换期望摘要 ⇒ 预期报红
//   --fault=multi-file        → 注入⑦：三单一起排队、其中两单同名 ⇒ 一张都不许丢、内容不许串味
//   --fault=multi-file-lie    → 同样的注入，只把其中一份的期望摘要换掉 ⇒ 预期报红
//   --fault=recv-readonly     → 注入⑤：接收目录只读（磁盘写不进去）⇒ 必须明确失败并止步，不许假 done、不许无限重试
//   --fault=recv-readonly-lie → 同样的注入，只把"该落到哪个终态"换成 done ⇒ 预期报红
//   --fault=recv-dir-rotted    → 注入⑧：预置可写 .part 后把目录改只读 ⇒ 只塌在最后那次 rename，
//                                「报 done 就必须有整份正确的文件」（A-5 那一格：⑤证不到的"半途才失败"）
//   --fault=recv-dir-rotted-lie→ 同样的注入，只把"接收侧不许假 done"换成"必须 done" ⇒ 预期报红
//   （每轮各几条断言**不在这里写**：`check-doc-numbers.mjs` 从下面的 check(" 调用点现算，
//    写进文档时要以「<轮次名> N 断言」的形式，否则不会被对账）
//   （kill 轮的文件尺寸用 E2E_KILL_MB 调，默认 100 —— 回环上实测 ~0.78s 走完，窗口够打）
//   （freeze 轮用 E2E_FREEZE_S 选调结长度，默认 30：**<45s** 是"链路还活着、只是没回执"，
//    **>45s** 越过 watchdog（健康阈值 15s×3）⇒ 真的拆链 + 重拨 + 重试，两种都是同一组结局判据）

const NEGATIVE = process.argv.includes("--negative");
// 判据自证先跑，**在任何实例启动之前**：这套 harness 用「日志里有没有某行」当就绪/投递证据，
// 而那层读法今天真的塌过一次（10 MB 轮的 boot 行被 512 KB 轮转藏进 .old.log ⇒ 20s 超时红，
// 连带 fail-fast 跳掉后面 9 步）。判据自己坏了的时候，必须以「判据坏了」退出，
// 不能留一个人去猜超时是谁的错。`--logtail-selfcheck` 是同一条检查的独立入口（不需要 release 产物）。
{
  const fails = selfcheckLogtail();
  if (process.argv.includes("--logtail-selfcheck")) {
    for (const f of fails) console.error(`  ❌ ${f}`);
    console.log(fails.length ? `✗ 日志判据自证红 ${fails.length} 条` : "✅ 日志判据自证 5 格全部成立");
    process.exit(fails.length ? 1 : 0);
  }
  if (fails.length) throw new Error(`日志判据自证不成立，先修判据再跑轮：\n  ${fails.join("\n  ")}`);
}
// §十六 的截图判据同理：它自己也要能被秒级证明"既能真也能红"（红 = 空图/没落盘也判得过）。
let envBlockReason = null;
{
  const { fails, notes } = selfcheckShot();
  for (const n of notes) console.log(`  · ${n}`);
  if (process.argv.includes("--shot-selfcheck")) {
    for (const f of fails) console.error(`  ❌ ${f}`);
    console.log(fails.length
      ? `✗ 截图判据自证红 ${fails.length} 条`
      : notes.length
        ? "✅ 截图判据自证成立（能判假）；「能判真」这一格今天没证 —— 原因见上面每条 note"
        : "✅ 截图判据自证成立（能判假 + 能判真）");
    process.exit(fails.length ? 1 : 0);
  }
  if (fails.length) throw new Error(`截图判据自证不成立，先修判据再跑轮：\n  ${fails.join("\n  ")}`);
  // 屏幕锁着**不再拦停整轮**（2026-09-27 自我推翻上一条提交）。理由有两条：
  // ① 锁屏只让"截图"这一格判不了，其余各格读的是两端 DB / 日志 / 磁盘，与屏幕状态无关 ——
  //    拦停等于把十几格可用证据一起扔掉，还要人等着；
  // ② 本仓既有口径（Windows 那条腿没有采集器时）就是"该格明着红 + 说清是环境"，不是整轮不跑。
  // 上一版把"别让八分钟白烧后才红在最后一步"当成拦停的理由，但代价是 13 格根本不跑；
  // 真正的解法是**红得有名**：把环境原因带进那一格的判定文本里。
  envBlockReason = screenBlockedReason();
  if (envBlockReason) {
    console.log(`  ⚠️ 环境不完整：${envBlockReason.split("\n").join(" ")}`);
    console.log("     ⇒ 本轮「报告带全屏帧」那一格会红（红在环境，不在产品），其余各格照常跑");
  }
}
// §十六 那份报告契约同理：契约里点名的字段（步骤/预期/实际/PASS-FAIL/耗时/日志关联/msg_id+transfer_id）
// 以前只有人肉核对过一次，机器一句都没管 —— 谁把 `writeReport` 里的一行删掉，报告就少一格而没人报红。
// 所以判据先跑，再启动实例；`--report-contract-selfcheck` 用合成夹具证明它「能判真也能判假」，
// `--report-contract=<目录>` 拿这份判据读一份**已经落盘**的报告。
{
  const { fails, notes } = selfcheckReportContract();
  if (process.argv.includes("--report-contract-selfcheck")) {
    for (const f of fails) console.error(`  ❌ ${f}`);
    console.log(fails.length ? `✗ 报告契约判据自证红 ${fails.length} 条` : `✅ 报告契约判据自证成立（${notes}）`);
    process.exit(fails.length ? 1 : 0);
  }
  const one = (process.argv.find((a) => a.startsWith("--report-contract=")) || "").slice("--report-contract=".length);
  if (one) {
    const gaps = readReportContract(one);
    for (const g of gaps) console.error(`  ❌ ${g}`);
    console.log(gaps.length ? `✗ ${one} 的报告不合格（${gaps.length} 条）` : `✅ ${one} 的报告符合 §十六 契约`);
    process.exit(gaps.length ? 1 : 0);
  }
  if (fails.length) throw new Error(`报告契约判据自证不成立，先修判据再跑轮：\n  ${fails.join("\n  ")}`);
}
//   --shot-selfcheck         → 只跑 §十六 截图判据的三格自证（假 PNG 判假 / 缺文件判假 / 真截图判真），
//                             不起实例、不需要 release 产物：npm run test:e2e:shot-selfproof
//   --prune-selfcheck        → 只跑跨轮保留的 8 格自证（含"被文档点名的绿轮不许删"那一对），
//                             不起实例、不碰任何目录：node scripts/e2e-multi-instance.mjs --prune-selfcheck
//   E2E_NO_CAPTURE=1         → 把采集器关掉（= 截图判据读的那个输入）⇒ 那条截图判据必须红
const POISON = FAULT === "poison-part" || FAULT === "poison-part-lie";
/// 注入②：接收端已有**真实前缀** ⇒ 必须按前缀续传，不许从 0 重灌整份。
const RESUME = FAULT === "resume-prefix" || FAULT === "resume-prefix-lie";
/// 注入③：接收中**真杀进程**。窗口是实测的，不是猜的：100 MB 在回环上 ~0.78s 走完
/// （.part 每 ~52ms 涨 6.5MB）⇒ 20%~100% 之间有 ~0.6s 可打，所以这条不是掷骰子。
const KILL = FAULT === "kill-mid" || FAULT === "kill-mid-lie";
/// 注入④：对端**失联但没死** —— SIGSTOP 冻住 B。这一格钉的是
/// 「对端没回执期间两侧都不许假成功，对端回来必须自己补齐」。
/// ⚠️ 为什么不是"传输中途冻"：实测 100 MB 回环 0.78s 传完，而拆一条静默链路要 **45s**
/// （watchdog = 健康阈值 15s × 3）⇒ 在飞窗口等不到冻结生效。时序只能是
/// 「先冻 → 再入队 → 冻 N 秒 → 解冻」，N 决定落在哪个 regime（见 FREEZE_MS）。
/// ⚠️ 上面那句"等不到"在写它之后就被**推翻了一半**：那是"手动掐时机"等不到，
/// 「判据自己入队 + 自旋等 .part 涨起来」是等得到的（③ 杀进程轮先做到，⑨ 停滞轮第二个做到）。
/// ⇒ ④ 只覆盖了"先冻再入队"这个 regime；"在飞时被冻"是 ⑨，别把两轮的结论互相冒充。
/// ⚠️ 实测撞出的产品现状（30s / 60s 两跑相同）：**失联期间这一单一次都没被尝试过**
/// —— `file_outbox` 的重投只被入站事件触发，没有定时器 ⇒ 这一格**没覆盖**"到点重投"，
/// 也**没覆盖**"write 成功 ≠ 已送达"。已按 A 类风险登记在 roadmap，改前别把话说满。
const FREEZE = FAULT === "peer-freeze" || FAULT === "peer-freeze-lie";
/// 注入⑩：**发送端在飞时被 SIGKILL，然后重启** —— §七「发送过程中杀进程」×「重启后继续」、
/// §八「本端重启」、§九「数据生命周期：运行→退出→重新启动」的交叉格。
/// ⚠️ 这一格此前**零跨实例判据**：③ 杀的是**接收端**，全仓没有任何一轮从"发送端崩了"这一侧看过。
/// 它钉的不是"能不能续传"（③/② 已证），而是**崩溃不许把"已入队"这个事实抹掉**：
/// 队列行是"先入队再投递"那条物理定律的载体，如果一次崩溃能让它变成 done 或让它消失，
/// 这一单就永久没人再发了。而**用户侧还有第二个结局**：气泡行（`messages`）也没了的话，
/// 用户连"曾经发过这一单"都看不见 —— 两个结局各自钉一条，所以入队照⑥ 复刻产品的三行。
const SENDKILL = FAULT === "sender-kill-mid" || FAULT === "sender-kill-mid-lie";
/// 注入⑨：**字节已经在飞**的时候把接收端冻住 —— §七「发送方提前放弃 / 接收方仍在线」
/// 与 §19「文件·断线」的交叉格。冻结轮（④）**没有**覆盖它：④ 的时序是"先冻 → 再入队"，
/// 而 ④ 自己实测出那一轮 A **一次都没尝试**（A-9：队列只被入站帧带动，没有定时器）
/// ⇒ ④ 里根本不存在在飞字节。③ 杀进程轮则证明了在飞窗口**抓得住**
/// （判据自己入队 + 自旋等 `.part` 涨到 786432 字节才动手），所以 ④ 那句
/// "在飞窗口等不到冻结生效"写的是它当时的时序，不是这一轮的障碍。
/// ⚠️ 判据写成**结局空间**，不写"我预期它失败"：这里本来以为有两个计时器在赛 ——
///   链路 watchdog 45 s（健康阈值 15 s × 3，越过就拆链）vs 发送侧停滞放弃 60 s
///   （`FILE_STALL_ABORT_MS`，file.rs:208）。**2026-09-26 实测：watchdog 先赢，而且赢得干净**
///   （`run-…T05-31-30-478Z`：`05:31:36 → start streaming` → 冻 B → `05:32:21 COMPLETED ok=false`
///   = 45 s 整），60 s 那一档在"对端完全冻死"下**结构上不可达**；全量 grep 156 份归档 run
///   的 A 侧日志，`[STALL] transfer=` 命中 **0 行**（同批语料 `[FILE] COMPLETED` 4,230 行）
///   ⇒ 登记为 roadmap **A-13**（含"15s 那一档只 emit 不写日志，所以今天无法回答它有没有出现过"）。
///   仍然只钉"两侧都不许假成功"，把走哪条只打印出来 —— **一条从未出现过的出口不能当判据**
///   （"日志里没有"写进断言就是同义反复，见 A-13 那格最后一段）。
const STALL = FAULT === "stall-mid" || FAULT === "stall-mid-lie";
/// 注入⑤：接收端**磁盘写不进去**（用户把接收目录设到只读盘 / 磁盘满 / 外接盘被拔）。
/// 与冻结轮正好成对：冻结轮里 A **一次都没尝试**（没有入站帧 ⇒ 没人触发 flush）；
/// 这里 B 活着、照常发心跳 ⇒ A 一定尝试、一定被拒，于是真正走的是
/// 「重试到上限 → GiveUp → 唯一出口记 failed」这条链（也是 outbox 第一次被真进程跑到 GiveUp）。
/// ⚠️ 注入落在 **offer 期**（`File::create` 就 EACCES，file.rs:1604）⇒ 走的是
///   `transport.rs:3720` 那条分支：只发 `FileReject{received:0}` + 记一条 error 日志，
///   **不写任何接收侧 DB 行**。所以这一轮**不许**断言"B 侧记了 failed"（那行根本不存在）；
///   B 侧可证的事实只有「日志里出现初始化失败」与「盘上没有这个 transfer 的任何东西」。
///   要测"收到一半才写失败 ⇒ 接收侧 upsert failed"得另开一条（预置可写 `.part` 再把目录改只读），
///   那是 A-5 的形状，不并进这一格。
const DISK = FAULT === "recv-readonly" || FAULT === "recv-readonly-lie";
/// 注入⑧：A-5 的形状 —— **预置可写 `.part`，再把接收目录改成只读**。
/// 与⑤成对但不重复：⑤ 打在 offer 期（`File::create` 就 EACCES ⇒ 只发 FileReject、
/// **接收侧不写任何行**，见上面那条注释），所以⑤**证明不了**"半途才失败时接收侧怎么收口"。
/// 这一轮把注落下得晚：真前缀已存在 ⇒ 往已存在的 inode 里写**不需要目录写权限**，
/// 于是字节照常流进来，唯一会 EACCES 的是**收尾那次 rename**（还有清理时的 unlink）。
/// ⇒ 这是「rename 才算完成」这条不变量第一次被活实例检验：报 done 就必须有整份正确的文件。
/// ⚠️ 判据刻意不预设"产品必须失败"（那等于替产品做决定）：只判**终态与磁盘自洽**。
/// 解除只读之后会不会自愈 —— 只打印实测，不设判据（没量过的事不写进断言）。
const ROT = FAULT === "recv-dir-rotted" || FAULT === "recv-dir-rotted-lie";
/// 注入⑥：§七「错误 size」的**真实用户形状** —— 不是线上收到一个谎报的 size（那一格协议层
/// 已经用 hash+length 判死了），而是**入队之后、真正发出去之前，磁盘上的原件被改小了**。
/// 现实触发：离线排队期间用户在原路径上裁掉/覆盖了同一个文件（视频剪完再发、同步盘回写）。
/// 窗口为什么是确定的：A 只在**收到对端某一帧**时才读盘（A-9 实测），所以
/// 「先冻住 B → 入队 → 截断 → 解冻」保证 A 一定读到截断后的版本，不靠运气。
/// ⚠️ 这里**入队三行**（messages 气泡 / file_transfers / file_outbox），与前五轮只插 outbox
///   不同：这一格的疑点正好在"入队时按磁盘算的那份 size 事后会不会被更正"，
///   少插一行就测不到它（`send_file_from_path_at` 的 offer size 来自 `meta.len()`，
///   而 `upsert_transfer` 的 ON CONFLICT 只改 status/path/progress，**不改 size**）。
const SHRINK = FAULT === "src-shrunk" || FAULT === "src-shrunk-lie";
/// 注入⑦：§七「连续多文件」×「磁盘已有同名文件」这两格合起来测 —— 三个 transfer 一起排队，
/// 其中**两个文件名完全相同、内容不同**（真机形状：一次选两张同名截图、或连发两版同名文档）。
/// ⚠️ 为什么这一格值一次运行：接收端的落地名在 **offer 那一刻**由 `unique_path` 决定
///   （`file.rs:1598`，规则 `a.bin → a (1).bin`）。同名两单若在这之前都还没 rename，
///   两者会拿到**同一个 final_path** ⇒ 后一次 rename 直接覆盖前一次 = **静默数据丢失**
///   （`file.rs:1599-1602` 那段注释只解决了 `.part` 交错，没解决 final 撞名）。
///   少一个文件、或落地内容集合少一份，就是这条被判红 —— 不是"看着不顺眼"，是丢东西。
const MULTI = FAULT === "multi-file" || FAULT === "multi-file-lie";
const GROUP = ROUND === "group" || ROUND === "group-lie" || ROUND === "group-targets-lie";

/// 链式轮（`--round=gossip3`，用户 2026-09-26 拍板＝建，但只挂在**发版前**那一层，不进日常本地门禁）。
/// 钉的是 §五 点名的 `群聊 + gossip` 交叉里唯一没被跨实例判着的那一半：**经中间人转发的收敛**。
/// 拓扑是 A—B—C 一条链：A 与 C **互相不是好友、也不给任何端点、C 侧关 LAN** ⇒ 它们之间不可能有链路
/// ⇒ C 若收到 A 的群消息，只可能是 B 转发的（不是"直连也能过"的假绿）。
/// ⚠️ 前置判据（"C 侧没有与 A 的建链行"）必须**先**成立，否则后面所有格都失去意义 ——
///    没有它，这一轮测的就不是转发。生产侧的对应形状：gossip 扇出候选取
///    `reachable_neighbors`（有活链路的邻居）而不是 `peers`（知识集），见 lib.rs 的
///    `gossip_fanout_targets_reachable_links`（审计 P0#7）；那一半钉源码，这一半钉真跑。
const CHAIN = ROUND === "gossip3" || ROUND === "gossip3-lie";
/// **#89 的那一半今天还缺的判据**：用户把"局域网发现"关掉之后，别的实例**真的学不到他**
/// （而不是靠 `GOSSLAN_AUTOSTART` 替他重新打开）。
/// 为什么必须单独一轮：2026-09-26 我据两轮实测写下"关掉了还能被学到"这个产品结论，
/// 第二天复跑发现**那个"关掉"从未生效**（harness 给每个实例塞了强制联网的 env，
/// 而产品码里它优先于用户那个键）。根因已拔掉（`lib.rs` 现在只看 `lan_enabled`），
/// 但"键能说话"这件事本身还没有跨实例判据 —— 没有它，#76 仍然是"我以为对了"。
/// ⚠️ 这一轮**故意继续带着那个 env 跑**（`launch()` 塞的）：要证的正是
///    "预置说关 ⇒ 连强制联网的环境变量都不该把它打开"，摘掉 env 就等于把考题擦掉。
const LANOFF = ROUND === "lanoff" || ROUND === "lanoff-lie";
/// **#77 的判据轮**（`--round=gossip-late`）：晚加入群的成员上线后，中间人把窗口内
/// 替别人转发过的那条**重新递给它**。
/// 与链式轮**同一套拓扑**（A—B—C 一条链：A 与 C 互相不是好友、互不给端点），唯一区别是
/// 启动时序反过来 —— 链式轮先建好 B↔C 再让 A 发（证明"当时能扇到"），这一轮让 A 在
/// **C 还不存在**时就发完（证明"以后还能补到"）。
/// ⚠️ 生产侧的边界（必须与 `mesh/gossip_replay.rs` 的文件头一致，不许读成"最终一定一致"）：
/// 缓存是进程内的、每组只留 `PER_GROUP_LIMIT` 条、只留 `RELAY_WINDOW_MS` 窗口 ——
/// 中间人重启过、或 C 掉线超过窗口/超过 16 条，补到的就是**部分**历史。
const LATE = ROUND === "gossip-late" || ROUND === "gossip-late-lie";

/// 任务专项轮（第二阶段 §22）：一条任务「创建 → 被指派者收到 → 改成完成」跨两个真实进程。
/// 判的是这一族在 SQLite 里**真的存在**的事实；徽标那个数字是前端 store 算的，不在本层判（见 roadmap §13.3 的层次修正）。
const TASK = ROUND === "task" || ROUND === "task-lie";
/// #121（`--round=groupcrash`）：**正在建群时被 SIGKILL** —— §28「链路失效」那一族里
/// 唯一今天做得成的格子。钉的是"群名册才是事实源、重递不许依赖内存登记"，
/// 完整动机与为什么不预置队列行写在那一步的注释里。
const GCRASH = ROUND === "groupcrash" || ROUND === "groupcrash-lie";
/// `--round=gfile`（群文件跨实例那一格：§七-4 点名的「文件 + 群聊」＋§六 群聊清单里的「群文件」）：
/// A 只把**货**备在盘上（群文件行 / 每个成员一行 / 密封好的会话密钥 / 源文件路径），B 一行都不知；
/// B 上线后由生产 `flush_pending_group_files` 自己把它投出去 ⇒ 判的是**对端自己落盘那份内容**。
/// 为什么不照抄 1:1 那三条手写 DB 行：群文件在此之外多一份「用群密钥封装的 file_key」，
/// 而接收端要用**它自己的群密钥**解开才肯建会话 —— 密封这一步只能过生产 crypto。
const GFILE = ROUND === "gfile" || ROUND === "gfile-lie";
/// `--round=dmreaction`（需求汇总三点名「群聊 + 1:1 都要」那一格的跨实例那一半）：
/// 1:1 的表情回应今天只有单元级判据（位图门控、载荷校验、前端接线各自绿），
/// 而**两个真实进程之间送一条静默事件**从没被判过 —— 这一轮补的就是那一格。
const DMREACT = ROUND === "dmreaction" || ROUND === "dmreaction-lie";
// §三 点名的「大文件之后继续发送普通消息」与 §五 那张"文件层失败/走完都不许波及消息层"。
// 默认轮的顺序是**先文本（J1）再文件（J2）** ⇒ 它判的是"文件之前聊天能用"，
// 从来没有一条判据把那条文本放到**一份文件走完之后**再投。这一轮补的就是那一半。
const POSTTEXT = ROUND === "posttext" || ROUND === "posttext-lie";

import { spawn, spawnSync } from "node:child_process";
import { createHash, createCipheriv, createPrivateKey, randomBytes, randomUUID, sign } from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { BOOT_LINE, bootBaseline, bootReady, countLog, readLogTail, selfcheckLogtail, stashLogs } from "./e2e-logtail.mjs";
import { captureShot, captureWindowShot, describeShotDir, screenBlockedReason, selfcheckShot } from "./e2e-shot.mjs";
// 引擎层（常量 + 共享量 + 工具函数）在 scripts/e2e/core.mjs：整段搬过去、只在声明行加了 export；
import {
  ALL_INST, APPDATA, BIN, FAULT, FILE_BYTES, GROUP_ID, GROUP_KEY_B64, GROUP_KEY_STR, GROUP_NAME, GROUP_TTL, INSTANCES, INST_C, ISO, LIE, LIE_SHA, MINTED_IDS, NO_ROUTED, PKCS8, ROOT, ROUND, RUN_DIR, S, TCP_BASE, appDataDir, assertions, binaryPath, bootAndStop, bootBaseOf, buildGroupEnvelope, buildMentionTargets, check, citedRunIds, ed25519Priv, eid, launch, newestMtime, noteId, nowMs, openDb, procs, procsLeft, prunePlan, pubFromSecret, readIdentity, readReportContract, reportContractGaps, runStep, seed, seedPair, selfcheckPrune, selfcheckReportContract, shotFiles, shotSources, sleep, step, stepBadge, steps, stopAll, stopOne, tailLog, takeShot, tcpOpen, traceExcerpt, waitFor, waitSendTerminal,
} from "./e2e/core.mjs";
import * as group from "./e2e/rounds/group.mjs";
import * as task from "./e2e/rounds/task.mjs";
import * as posttext from "./e2e/rounds/posttext.mjs";
import * as dmreact from "./e2e/rounds/dmreact.mjs";
import * as chain from "./e2e/rounds/chain.mjs";
import * as late from "./e2e/rounds/late.mjs";
import * as lanoff from "./e2e/rounds/lanoff.mjs";
import * as gcrash from "./e2e/rounds/gcrash.mjs";
import * as gfile from "./e2e/rounds/gfile.mjs";
import * as poison from "./e2e/rounds/poison.mjs";
import * as resume from "./e2e/rounds/resume.mjs";
import * as kill from "./e2e/rounds/kill.mjs";
import * as freeze from "./e2e/rounds/freeze.mjs";
import * as sendkill from "./e2e/rounds/sendkill.mjs";
import * as stall from "./e2e/rounds/stall.mjs";
import * as disk from "./e2e/rounds/disk.mjs";
import * as rot from "./e2e/rounds/rot.mjs";
import * as shrink from "./e2e/rounds/shrink.mjs";
import * as multi from "./e2e/rounds/multi.mjs";



// ── 跨轮保留判据（§十六 产物保留的第二半：历史攒多少轮）──────────────
/// 为什么在模块级、而不是留在收尾的 `finally` 里：原来它长在最后一段，要证明"那几格断言
/// 真会失败"就得先跑满一轮（30–45 分钟）—— 而 `--logtail-selfcheck` / `--shot-selfcheck` /
/// `--report-contract-selfcheck` 这三个先例存在的原因就是同一个：**判据必须能秒级自证**。
/// 收尾那一段现在调的就是这里的同一份函数，不留第二个家。
/**
 * 被文档点名的 run 目录名（那些"实测锚点 `run-…Z`"就是证据本体）。
 * ⚠️ 拿不到名单时必须返回 **null**，不能返回空集合 —— "扫不到引用"和"没有引用"是两件事，
 * 只有后者才允许删（同一形状：`--locked` 读不到 lock 时宁可报错，不许当成"没有依赖"）。
 * 扫的范围只到 `*.md` / `*.html`：证据锚点写在文档里；自证夹具用的是 `run-a` 这种合成名，
 * 不匹配下面那个 ISO 形状，不会被误当引用。
 */
/** 自证：每格只换一个输入。不这么写的话"上限生效"可以只是"绿轮恰好都被留着"。 */
{
  if (process.argv.includes("--prune-selfcheck")) {
    const fails = selfcheckPrune();
    for (const f of fails) console.error(`  ❌ ${f}`);
    console.log(fails.length ? `✗ 跨轮保留判据自证红 ${fails.length} 条` : "✅ 跨轮保留判据自证成立（8 格）");
    // 引用普查：这条入口顺手回答"文档点名了几个 / 盘上还缺几个"。那 9 个已缺失的锚点以前只能靠
    // 临时脚本现算才看见（而现写的脚本只证明"我看到了什么"，不证明"工具管不管"）⇒ 数交给判据自己印。
    const cited = citedRunIds();
    if (!cited) {
      console.error("⚠️ 引用名单拿不到（`git ls-files` 失败）⇒ 真实一轮会一个都不删");
    } else {
      const root = path.join(ROOT, "test-results");
      const present = new Set(fs.existsSync(root) ? fs.readdirSync(root).filter((n) => n.startsWith("run-")) : []);
      const missing = [...cited].filter((n) => !present.has(n)).sort();
      console.log(`引用普查：文档点名 ${cited.size} 个 run-* · 仍在盘上 ${cited.size - missing.length} 个 · 已缺失 ${missing.length} 个`);
      for (const m of missing) console.log(`  · 已缺失（历史淘汰，不可恢复）：${m}`);
    }
    process.exit(fails.length ? 1 : 0);
  }
  // ⚠️ 这里**不**在起跑前 throw：清理逻辑坏了不该让一整轮（30–45 分钟）根本跑不起来。
  //   立场与原来一致 —— 收尾那一跑先自证，自证不过就"一个都不删 + 本轮记一条不合格"。
}


if (!APPDATA || !fs.existsSync(APPDATA)) {
  console.error(`✗ 找不到 app data 目录：${APPDATA ?? "(本平台不认识)"} —— 先跑过一次应用再说`);
  process.exit(2);
}
if (!fs.existsSync(BIN)) {
  console.error(
    `✗ 没有二进制：${BIN}\n  先 npm run build && (cd src-tauri && cargo build --release --features bluetooth)\n`
    + "  ★ 必须带 --features bluetooth：打包五条命令全都带，门禁若测不带蓝牙的那份 ⇒ 「测的那份 ≠ 发的那份」（#126）",
  );
  process.exit(2);
}
// 测旧代码 = 白测（本项目真踩过）。判据：二进制必须不比源码新文件更旧。
//
// 但 **mtime 变新 ≠ 源码变了**：`verify-guards` 是「改坏 → 跑测试 → 原样写回」，
// 写回会把 `.rs` 的 mtime 推到当下，内容却一个字节都没动。真按 mtime 一刀切，
// 这条守卫会在每次护栏运行期间把 E2E 全部拒掉 —— 而它拒绝的理由是假的。
// 所以「变新了」必须再问一层：内容到底和 HEAD 一样吗？二进制是不是比那次提交更新？
// 两条都成立 ⇒ 只是 mtime 抖动，放行并说明；否则 ⇒ 真的可能在测旧码，红。
{
  const binM = fs.statSync(BIN).mtimeMs;
  const newest = newestMtime(path.join(ROOT, "src-tauri", "src"), 20);
  if (newest > binM + 1000) {
    const dirty = spawnSync("git", ["status", "--porcelain", "--", "src-tauri/src"],
      { cwd: ROOT, encoding: "utf8" }).stdout.trim();
    // 比的是**最后一次动过 src-tauri/src 的提交**，不是 HEAD。
    // 踩过的坑：只改文档/脚本的提交会把 HEAD 推到二进制之后，于是这条守卫把
    // "内容完全没变的二进制"判成过期 —— 守卫自己造假红，和被它挡住的旧二进制一样有害。
    const srcHeadIso = spawnSync("git", ["log", "-1", "--format=%cI", "--", "src-tauri/src"],
      { cwd: ROOT, encoding: "utf8" }).stdout.trim();
    const srcHeadMs = Date.parse(srcHeadIso);
    if (!dirty && Number.isFinite(srcHeadMs) && binM > srcHeadMs) {
      console.warn(`⚠️ 有 .rs 的 mtime 比二进制新，但 src-tauri/src 与 HEAD 内容完全一致，`
        + `且二进制晚于最后一次改动 src-tauri/src 的提交 —— 判定为 mtime 抖动，继续测当前内容。`);
    } else {
      // ⚠️ 文案必须是**归因准确**的：2026-09-27 我自己在"先 build 再 commit"这个最自然的顺序上
      //   撞了它一次，屏幕上写的是"你正在测旧代码"—— 那是假指控（内容正是这份二进制编出来的）。
      //   这一格分不出下面两种成因，所以两种都拒（保守方向 = 不跑，而不是跑完再说）：
      const iso = (ms) => new Date(ms).toISOString();
      console.error(`✗ 拒跑：无法证明这份 release 二进制是从当前这份源码内容构建的`);
      console.error(`  二进制 ${iso(binM)} · 最新的 .rs mtime ${iso(newest)} · `
        + `最后一次改动 src-tauri/src 的提交 ${srcHeadIso || "?"}`);
      console.error(`  成因可能是这两种之一：`);
      console.error(`   ① 源码真的动过而没重编（src-tauri/src 未提交改动：${dirty ? "有" : "无"}）；`);
      console.error(`   ② **先 build 再 commit** —— 内容没错，只是提交时间戳晚于构建。`);
      console.error(`      ②不改成放行，是因为"提交晚于构建"和"pull/checkout 带进新内容"在时间戳上长得一样，`);
      console.error(`      而后者真的该拒（那种情况下测的是旧码）。⇒ 顺序纪律：**改完 Rust 先提交，再重编，再跑 E2E。**`);
      console.error(`  两种情况的处置相同且只要一分半：cd src-tauri && cargo build --release --features bluetooth（#126：与打包同 feature 集）`);
      process.exit(2);
    }
  }
}



/** 停机时写库：WAL 要先 checkpoint 再关，否则 -wal 里的内容下一次启动才被吸收（本项目实测口径）。 */






/** 每次 launch 前把该实例的历史日志移进 run 目录（不删）；本轮写的行因此必然在当前档里。 */


/** §十六 报告要的界面截图（真实文件路径，落 RUN_DIR/screenshots/）。 */
/** #91：每张帧**从哪来的**逐张记下来 —— 窗口帧（属于某个实例进程的某扇窗）还是整屏兜底（带原因）。
 *  这一格存在的意义就是"报告不许说自己拍到了界面，除非它真的拍到了那一扇窗"。 */

/** 先按**被测实例自己的 pid** 抓那一扇窗；抓不到再回落整屏，两条路都如实记来源。
 *  ⚠️ 刻意不把"必须抓到窗口帧"写成通过条件：macOS 对不在当前 Space / 已最小化的窗口直接拒绝出图
 *  （实测 `could not create image from window`、不落盘），那是环境状态，写成通过条件会把它
 *  乘成本层十几条同因红（与 roadmap §十六 那条"锁屏乘成 14 条同因红"同形）。
 *  这一格的机器证明在 `selfcheckShot` 的 ⑦⑧ 两格（每轮起跑前跑，判据坏了就当场停轮）。 */

/// 本轮总判据**只能**由断言账本推出来。
/// 之前的写法是「没抛异常 = 成功」，而 `check()` 返回 false 并不抛 ⇒
/// 一条真回归会被写成 summary.json 里的 PASS，只有人盯着控制台才看得见。
/// 机器判据不能依赖人读日志。
const anyFail = () => assertions.some((a) => a.verdict === "FAIL");
/// 「起 A/B 并等链路真的建立」那一步的下标。反向自证要求红**落在它之后**：
/// 红若落在建链之前，那只是应用没起来，证明不了「断言依赖真实投递」。
const linkStepIdx = () => steps.findIndex((s) => s.name.startsWith("起 A/B"));let NODES;
step("停机预置：好友 + routed 端点 + 独立接收目录", () => seedPair(NODES));

step("L-A 入队：在 A 的库里留下「已入队待发送」的事实", () => {
  S.msgId = eid();
  // 反向模式：链路照建，只是注定送不到 —— 报红必须来自投递断言本身
  S.peerTo = NEGATIVE ? `${S.idB.runtimeId}-ghost` : S.idB.runtimeId;
  const ts = nowMs();
  const payload = JSON.stringify({
    type: "chat_message",
    msg_id: S.msgId,
    from: S.idA.runtimeId,
    to: S.peerTo,
    kind: "text",
    content: "enc1:harness-placeholder", // 占位；应用会 re-seal 成真密文
    ts,
    seq: 1,
  });
  seed(INSTANCES[0].db, (db) => {
    db.prepare("DELETE FROM messages WHERE msg_id=?1").run(S.msgId);
    db.prepare("DELETE FROM outbox WHERE msg_id=?1").run(S.msgId);
    // 陷阱 10：两行必须成对 —— 只插 outbox 则 re-seal 没有明文，只插 messages 则 Ack 找不到人
    db.prepare(
      `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
       VALUES(?1,?2,?3,?4,'text',?5,?6,1,'sending')`,
    ).run(S.msgId, S.idB.runtimeId, S.idA.runtimeId, S.peerTo, "hello from harness", ts);
    db.prepare("INSERT INTO outbox(msg_id,peer_id,payload,created_at) VALUES(?1,?2,?3,?4)")
      .run(S.msgId, S.peerTo, payload, ts);
  });
});

step("L-A 入队：A 的一个 1 MB 文件也排好队（停机窗口内）", () => {
  S.xferId = eid("x");
  const dir = path.join(RUN_DIR, "src");
  fs.mkdirSync(dir, { recursive: true });
  S.srcFile = path.join(dir, `${S.xferId}.bin`);
  // 真随机字节：全零会被任何"压缩/去重"路径悄悄改掉而断言看不出来
  const buf = Buffer.alloc(FILE_BYTES);
  for (let i = 0; i < buf.length; i += 32) buf.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
  fs.writeFileSync(S.srcFile, buf);
  S.srcSha = createHash("sha256").update(buf).digest("hex");
  seed(INSTANCES[0].db, (db) => {
    db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(S.xferId);
    // 陷阱 8：local_path 必须真实存在，否则每次重试白烧一个 attempts 配额
    db.prepare(
      `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
       VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
    ).run(S.xferId, S.peerTo, S.srcFile, `${S.xferId}.bin`, buf.length, nowMs());
  });
});
// 故障注入族的**停机预置**（块体逐字住在 scripts/e2e/rounds/）：旗标与顺序留在这里 —— 判据 D 与契约图
// 那条 ROUND/FAULT 现读命令都从本文件取，所以分发行不许跟着块搬走。
if (POISON) await poison.preset();
if (RESUME) await resume.preset();
if (KILL) await kill.preset();
if (TASK) await task.preset();
if (GROUP) await group.preset();
if (POSTTEXT) await posttext.preset();

step("起 A/B 并等链路真的建立（routed 拨号一轮 10s）", async () => {
  for (const i of INSTANCES) launch(i);
  for (const i of INSTANCES) {
    await waitFor(() => tcpOpen(i.port), 60_000, `实例 ${i.label} TCP ${i.port} 可连`);
  }
  // 断言链路成立，而不是靠 sleep：日志里的 +conn peer= 必须出现「对端那个 id」
  const other = { A: { inst: INSTANCES[0], id: S.idB }, B: { inst: INSTANCES[1], id: S.idA } };
  for (const side of ["A", "B"]) {
    const { inst, id } = other[side];
    await waitFor(
      () => tailLog(inst.log, 4000).includes(`peer=${id.runtimeId}`),
      90_000,
      `${side} 侧日志出现与 ${id.runtimeId} 的链路`,
    );
  }
  // §19「网络」Routed 这一格第一次有跨实例判据。上面那圈 waitFor 只要求日志里出现
  // `peer=<对端 id>`，而 LAN 广播也能让它出现 ⇒ "两端预置的那个 Routed 端点真被拨通过"
  // 一直是被动发生却没人钉着的（把拨号循环改坏，这 12 轮照样全绿）。
  // 判据是"两侧合计 ≥1"而不是"每一侧各 1"：谁先拨到谁记 routed、另一侧只看到入站连接，
  // 11 轮实测里 A=routed/B=lan 与 A=lan/B=routed 两种都出现过 ⇒ 按侧断言会漂。
  const routedDialed = () =>
    countLog(INSTANCES[0].log, `建链 peer=${S.idB.runtimeId} path=routed`) +
    countLog(INSTANCES[1].log, `建链 peer=${S.idA.runtimeId} path=routed`);
  await waitFor(routedDialed, 45_000, "至少一侧打出与对端的 path=routed 建链行");
  check("这对外部以手动配置的 Routed 端点拨出过链路（§19 网络·Routed）",
    routedDialed() >= 1, "≥1 条 path=routed 建链", routedDialed());
  // 两个实例的窗口此刻都在这台机器的桌面上：留一张"链路真建立了"的界面证据
  takeShot("1-link-established", procs.get(INSTANCES[0].n)?.pid);
});

step("A→B 送达 + Ack 回收 + 无重复", async () => {
  const [aDb, bDb] = [openDb(INSTANCES[0].db, true), openDb(INSTANCES[1].db, true)];
  const peekB = () => bDb.prepare("SELECT * FROM messages WHERE msg_id=?1").all(S.msgId);
  const outboxLeft = () => aDb.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id=?1").get(S.msgId).c;
  await waitFor(async () => peekB().length > 0, 60_000, "B 侧出现这条消息");
  // 等条件而不是等时间：Ack 回来才继续（否则慢机器上会把"还没到"读成"丢了"）
  await waitFor(() => outboxLeft() === 0, 30_000, "A 侧 outbox 被 Ack 删除");
  await sleep(1500); // 多留一点窗口，让「重复投递」这种退化有机会显现

  const rowsB = peekB();
  check("B 侧恰好一条（重复投递 = 用户看到两条）", rowsB.length === 1, 1, rowsB.length);
  check("B 侧内容与应用解密结果一致",
    rowsB[0]?.content === "hello from harness", "hello from harness", rowsB[0]?.content);
  check("B 侧方向正确（sender 是 A）",
    rowsB[0]?.sender_id === S.idA.runtimeId, S.idA.runtimeId, rowsB[0]?.sender_id);

  const left = aDb.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id=?1").get(S.msgId).c;
  check("A 侧 outbox 被 Ack 清空", left === 0, 0, left);
  const st = aDb.prepare("SELECT status FROM messages WHERE msg_id=?1").get(S.msgId)?.status;
  check("A 侧状态前进过 sending", !["sending", "failed"].includes(st), "sent/delivered/read", st);
  const uniq = bDb.prepare(
    "SELECT COUNT(*) c FROM messages WHERE msg_id=?1",
  ).get(S.msgId).c;
  check("msg_id 在 B 侧唯一（INV-P01）", uniq === 1, 1, uniq);
  aDb.close(); bDb.close();
});

step("J2 文件 A→B：只有 rename 之后才算完成，且字节与 hash 一致", async () => {
  const recvB = path.join(RUN_DIR, "recv", "B");
  const landed = path.join(recvB, `${S.xferId}.bin`);
  await waitFor(() => fs.existsSync(landed), 120_000, `B 的接收目录出现 ${S.xferId}.bin（只认 rename 后的最终名）`);
  const bytes = fs.readFileSync(landed);
  check("B 侧字节数与发送端一致", bytes.length === FILE_BYTES, FILE_BYTES, bytes.length);
  const got = createHash("sha256").update(bytes).digest("hex");
  check("B 侧 sha256 与源文件一致（INV-P17 分片可验证）", got === S.srcSha, S.srcSha.slice(0, 12) + "…", got.slice(0, 12) + "…");

  // ⚠️ 这里以前是"看到 B 的终名文件就立刻读 A"⇒ 判据读在 ack 之前（见 waitSendTerminal 的注释）。
  //   默认轮一直绿只是因为 waitFor 的 500ms 轮询恰好盖住了 ack 的往返时间，不是这条链没有窗口。
  const sentJ2 = await waitSendTerminal(S.xferId);
  const left = sentJ2.queued;
  const aRow = sentJ2.row;
  check("A 侧 file_outbox 行已被收尾删除", left === 0, 0, left);
  // 发送侧生命周期：pending → active → **sent（只是「我写完了 socket」）→ done（对端 FileCompleteAck 之后）**。
  // 钉 done 而不是 sent，正是总指令那条「不要因 TCP write 成功就认为已送达」的机器形状：
  // 只要本端写完就写 sent 就红，必须等对端确认才绿。接收侧终态同样是 done。
  check("A 侧 send 记录终态是 done（对端确认过，不是「我写完了 socket」的 sent）",
    aRow && aRow.status === "done", "done", aRow ? aRow.status : "无行");
  check("A 侧 send 记录进度到位", aRow && aRow.progress === 1.0, 1, aRow?.progress);

  const bDb = openDb(INSTANCES[1].db, true);
  const bRow = bDb.prepare("SELECT status,path FROM file_transfers WHERE id=?1").get(S.xferId);
  const bDup = bDb.prepare("SELECT COUNT(*) c FROM file_transfers WHERE id=?1").get(S.xferId).c;
  bDb.close();
  check("B 侧 receive 记录终态是 done（不是 active/failed）", bRow && bRow.status === "done", "done", bRow ? bRow.status : "无行");
  check("B 侧同一条传输只记一次", bDup === 1, 1, bDup);
  const strays = fs.existsSync(recvB)
    ? fs.readdirSync(recvB).filter((f) => f.includes(S.xferId) && f !== `${S.xferId}.bin`)
    : [];
  check("接收目录没有 .part / 改名副本残留", strays.length === 0, 0, strays.join(", ") || 0);
});
// 旅程轮的**本体**（起实例之后跑；顺序就是这里的顺序，判据在各分册里）：
// 单聊表情 → 续发 → 任务 → 群聊。
if (DMREACT) await dmreact.run();
if (POSTTEXT) await posttext.run();
if (TASK) await task.run();
if (GROUP) await group.run();
// 故障注入族的**轮次本体**（起实例之后跑）。同样只有分发，判据在分册里。
if (POISON) await poison.run();
if (RESUME) await resume.run();
if (KILL) await kill.run();
if (FREEZE) await freeze.run();
if (SENDKILL) await sendkill.run();
if (STALL) await stall.run();
if (DISK) await disk.run();
if (ROT) await rot.run();
if (SHRINK) await shrink.run();
if (MULTI) await multi.run();

step("L-B 故障注入：两端重启后仍正确", async () => {
  await stopAll();
  await bootAndStop("重启");
  const bDb = openDb(INSTANCES[1].db, true);
  const rows = bDb.prepare("SELECT * FROM messages WHERE msg_id=?1").all(S.msgId);
  bDb.close();
  check("重启后 B 侧仍只有一条、内容不变",
    rows.length === 1 && rows[0].content === "hello from harness", 1, rows.length);
  const aDb = openDb(INSTANCES[0].db, true);
  const again = aDb.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id=?1").get(S.msgId).c;
  aDb.close();
  check("重启不复活已 Ack 的 outbox 行（不二次投递）", again === 0, 0, again);
  // §十六「报告要带 screenshots/」这一格第一次有产物。判据只管"截图真落盘、不是空图"，
  // **不管界面对不对**（没有像素级判据；那一半仍按 §12.6 记结构级/MANUAL，写成绿就是假证据）。
  // ⚠️ 非 macOS 没有采集器 ⇒ 这一条**是红，不是跳过**（§十禁止把没跑写成 PASS）：
  //    Windows 腿要自己实现采集器，在那之前这格就明着红着。
  //    同理 macOS 上**屏幕被锁**也红：`screencapture` 退 0 却截到一张整屏纯色，
  //    而 `shotIsReal` 现在会把纯色帧判掉（2026-09-27 实测到的洞）。报错里会说是哪一种。
  takeShot("2-after-restart", procs.get(INSTANCES[0].n)?.pid,
    "这一步开头 stopAll() 把两台都停了 ⇒ 此刻没有窗口可抓；这一张只是整屏现场，不是「界面在那个窗口里」的证据");
  const realShots = shotFiles.filter(Boolean);
  // ⚠️ 这条**只**证明"抓到了两帧、且不是纯色"。它证明不了"应用界面在那一帧里"：
  //   2026-09-27 实测 —— 屏幕锁着（loginwindow 以 layer 2004 盖住整屏）时全屏抓帧
  //   拿到的是**桌面壁纸**，壁纸有多种颜色 ⇒ 连"不是纯色"这关也过得去。
  //   要证明"界面渲染了"必须按窗口 id 抓（`screencapture -l<id>`），那是另一件事（已登记）。
  // ⚠️ 通过条件里**必须**有 `!envBlockReason`，这一半不是文案是判据：
  //   2026-09-27 实测——把"锁屏就在起跑前拦停"删掉之后，同一趟锁屏跑里
  //   `✅ 报告带两张全屏帧` 出现了 14 次、整层 0 红、`exit=0`。
  //   这条判据能看见的三样输入（落盘 / PNG 结构 / 不是纯色）在锁屏时**一个都不会变**
  //   （全屏抓到的是多色的桌面壁纸）⇒ 能挡住这个假绿的只有"环境信号进判据"这一条路。
  //   缺信息时的方向必须是红，不是"少一条 ✅"。
  const shotList = `${realShots.length} 张：`
    + (realShots.map((f) => path.basename(f)).join(", ") || describeShotDir(RUN_DIR));
  check("报告带两张全屏帧（链路建立后 / 两端重启后；只证『抓得到且不是纯色』）",
    realShots.length === 2 && !envBlockReason,
    "2 张全屏 PNG，结构成立且不是纯色帧" + (envBlockReason ? "，且起跑前环境自检未报缺 surface" : ""),
    envBlockReason
      ? `环境判不了（起跑前量到）：${envBlockReason.split("\n").join(" ")}\n     ${shotList}`
      : shotList);
});
// 重启之后那几轮（L-B 把两端杀过又起回来）：任务重启判据 → 三实例链式 → 补递 →
// 关发现 → 建群崩溃 → 群文件。
if (TASK) await task.recheck();
if (CHAIN) await chain.run();
if (LATE) await late.run();
if (LANOFF) await lanoff.run();
if (GCRASH) await gcrash.run();
if (GFILE) await gfile.run();
const REPORT_GAPS = [];
function writeReport(failed) {
  fs.mkdirSync(RUN_DIR, { recursive: true });
  for (const i of INSTANCES) {
    try { fs.copyFileSync(i.log, path.join(RUN_DIR, `instance-${i.label}.app.log`)); } catch { /* 还没生成 */ }
    for (const suffix of ["", "-wal", "-shm"]) {
      const src = i.db + suffix;
      if (fs.existsSync(src)) {
        try { fs.copyFileSync(src, path.join(RUN_DIR, `sqlite-${i.label}`, path.basename(src))); } catch { /* 占用中 */ }
      }
    }
  }
  const totalS = +(steps.reduce((n, s) => n + (s.ms ?? 0), 0) / 1000).toFixed(1);
  const sum = {
    run: ISO,
    // 只要账本里有一条红，报告就不许写 PASS —— 判据不许由调用方口头声明
    verdict: failed || anyFail() ? "FAIL" : "PASS",
    negative: NEGATIVE,
    binary: BIN, platform: process.platform, duration_s: totalS,
    instances: INSTANCES.map((i) => ({ label: i.label, n: i.n, port: i.port, runtimeId: (i.n === 1 ? S.idA : S.idB)?.runtimeId })),
    trace: { msg_id: S.msgId ?? null, transfer_id: S.xferId ?? null, ids: [...new Set(MINTED_IDS)] },
    shots: shotFiles.filter(Boolean).map((f) => path.relative(RUN_DIR, f)),
    // #91：那几张帧各自的来源（窗口帧带 pid / 窗口 id / 几何；整屏兜底带"为什么"）。
    shot_sources: shotSources,
    // §十六要的「步骤 + 耗时 + 日志关联」：把闭包剔掉，只留事实。
    // 没跑到的步骤（fail-fast 跳过的）必须自带终态 NOT-RUN —— 否则"没有 verdict"会在渲染时落到
    // 「其他都算通过」那一支，报告就把没跑过的一格标成 ✅。
    steps: steps.map(({ fn, ...rest }) => ({ verdict: "NOT-RUN", ...rest })),
    assertions,
  };
  fs.writeFileSync(path.join(RUN_DIR, "summary.json"), JSON.stringify(sum, null, 2));
  const esc = (x) => String(x ?? "").replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
  const stepRows = steps.map((s) =>
    `<tr><td>${stepBadge(s.verdict)}</td><td>${esc(s.name)}</td><td>${((s.ms ?? 0) / 1000).toFixed(1)}s</td><td>${s.checks ?? 0}</td></tr>`).join("\n");
  const logEx = steps.filter((s) => s.logs && Object.keys(s.logs).length)
    .map((s) => `<h4>❌ ${esc(s.name)} —— 日志里带这条 trace 的行</h4>` +
      Object.entries(s.logs).map(([label, lines]) =>
        `<p><code>instance-${label}.app.log</code></p><pre>${esc(lines.join("\n")) || "（没有匹配行 —— 说明这一侧根本没见过这条 trace）"}</pre>`).join("\n")).join("\n");
  const tr = assertions.map((a) =>
    `<tr><td>${a.verdict === "PASS" ? "✅" : "❌"}</td><td>${esc(a.step)}</td><td>${esc(a.name)}</td><td><code>${esc(a.expect)}</code></td><td><code>${esc(a.actual)}</code></td></tr>`).join("\n");
  fs.writeFileSync(path.join(RUN_DIR, "summary.html"),
`<!doctype html><meta charset=utf-8><title>Gosslan 多实例 E2E ${ISO}</title>
<body style="font:14px/1.6 system-ui;margin:32px;max-width:1100px">
<h1>${sum.verdict === "PASS" ? "✅ PASS" : "❌ FAIL"} — 双实例 E2E ${ISO}${NEGATIVE ? " · 反向自证" : ""}</h1>
<p>二进制 <code>${BIN}</code> · ${process.platform} · 总耗时 ${totalS}s · msg_id <code>${S.msgId ?? "-"}</code> · transfer_id <code>${S.xferId ?? "-"}</code></p>
<p>实例：${sum.instances.map((i) => `${i.label}=#${i.n} :${i.port} <code>${i.runtimeId ?? "-"}</code>`).join(" · ")}</p>
<h2>步骤</h2>
<table border=1 cellpadding=6 cellspacing=0 width=100%>
<tr><th></th><th>步骤</th><th>耗时</th><th>断言数</th></tr>
${stepRows}
</table>
<h2>断言</h2>
<table border=1 cellpadding=6 cellspacing=0 width=100%>
<tr><th></th><th>所属步骤</th><th>断言</th><th>预期</th><th>实际</th></tr>
${tr}
</table>
${logEx ? `<h2>失败步骤的日志关联</h2>${logEx}` : ""}
${sum.shots.length
  ? sum.shots.map((rel) => {
      // #91：图注必须说清这张是**谁的哪扇窗**，说不出就写"整屏兜底 + 原因" —— 不许只放一张图暗示"界面在里面"。
      const src = (sum.shot_sources ?? []).find((x) => x.file === rel);
      const note = !src ? "来源未记录"
        : src.source === "window"
          ? `按窗口 id 抓 · 实例 pid ${src.pid} · 窗口 ${src.window_id} · 该窗 ${src.bounds} · 图 ${src.png} · scale ${src.scale}`
          : `整屏兜底 · ${src.why ?? "原因未记"}`;
      return `<p><code>${rel}</code> · ${esc(note)}</p><img src="${rel}" width="1000" alt="${esc(rel)}">`;
    }).join("\n")
  : `<p>⚠️ 本轮没有截图：本平台没有采集器 ⇒ §十六 这一格在它上面仍未做，不算通过。</p>`}
<p style="color:#666">日志/DB 快照在本目录：<code>instance-*.app.log</code> · <code>sqlite-*/</code> · <code>recv/</code> · <code>after-*.db</code></p>
<p style="color:#666">⚠️ 标 ⚠️ NO-ASSERT 的步骤只靠「超时即抛」把关，本身没下断言 —— 覆盖度按红字算，不按步骤数算。<br>⛔ 未跑 = 前面的步骤报红后 fail-fast 跳过的，什么都没验过，不许算进通过格。</p>`);
  // 判据读的是**刚落盘的产物**，不是内存里的那个对象 —— 序列化会吞掉 undefined，
  // 而"字段在内存里有、落盘后没了"正是这类契约最容易漏的那一格。
  REPORT_GAPS.push(...readReportContract(RUN_DIR));
  console.log(`\n报告：${RUN_DIR}/summary.html`);
}

const backups = new Map();
try {
  fs.mkdirSync(RUN_DIR, { recursive: true });
  for (const i of ALL_INST) {
    if (fs.existsSync(i.db)) {
      const to = path.join(RUN_DIR, `backup-${i.label}`, path.basename(i.db));
      fs.mkdirSync(path.dirname(to), { recursive: true });
      fs.renameSync(i.db, to);
      backups.set(i.db, to);
      for (const s of ["-wal", "-shm"]) {
        if (fs.existsSync(i.db + s)) fs.renameSync(i.db + s, to + s);
      }
    }
  }
  console.log(`双实例 E2E · run ${ISO}\n  二进制 ${BIN}\n  appdata ${APPDATA}`);
  await bootAndStop("首启（生成身份密钥）");
  [S.idA, S.idB] = INSTANCES.map(readIdentity);
  NODES = INSTANCES.map((inst, i) => ({ ...[S.idA, S.idB][i], label: inst.label, port: inst.port }));
  console.log(`  A=${S.idA.runtimeId}\n  B=${S.idB.runtimeId}`);
  for (let i = 0; i < steps.length; i++) await runStep(i, steps[i]);
  // 总判据一律从断言账本推，不从「有没有抛异常」推。
  if (NEGATIVE) {
    const bad = assertions.filter((a) => a.verdict === "FAIL");
    const delivered = bad.some((a) => a.stepIdx > linkStepIdx());
    const linkBroke = steps.slice(0, linkStepIdx() + 1).some((s) => s.verdict === "FAIL");
    if (delivered && !linkBroke) {
      console.log("\n✅ 反向自证通过：链路成立而投递断言如期报红 —— 红不来自基础设施噪声");
      writeReport(true);
      process.exitCode = 0;
    } else {
      console.error(`\n✗ 反向自证不算通过：${
        linkBroke ? "红落在「建链」之前，那只是基础设施坏了"
                  : "送给一个不存在的对端却全绿 —— 这些断言不依赖真实投递，是空转"}`);
      writeReport(true);
      process.exitCode = 1;
    }
  } else {
    const bad = assertions.filter((a) => a.verdict === "FAIL");
    if (bad.length) {
      console.error(`\n✗ ${bad.length}/${assertions.length} 条断言报红：\n` +
        bad.map((a) => `  · [${a.step}] ${a.name} —— 预期 ${a.expect} / 实际 ${a.actual}`).join("\n"));
      writeReport(true);
      process.exitCode = 1;
    } else {
      writeReport(false);
      console.log(`\n✅ 多实例 E2E 全绿（${assertions.length} 条断言）`);
    }
  }
} catch (e) {
  // 反向模式：红必须落在「建链之后」才算自证成立 —— 否则「应用根本没起来」
  // 也会被判成「断言不依赖基础设施噪声」，这条自证就白写了。
  const deliveredFail = NEGATIVE && S.curStepIdx > linkStepIdx();
  if (deliveredFail) {
    console.log(`\n✅ 反向自证通过：链路正常但送不到时，投递断言如期报红 —— ${e.message}`);
    writeReport(true);
    process.exitCode = 0;
  } else {
    console.error(NEGATIVE
      ? `\n✗ 反向自证不算通过：红落在「建链」那一步之前，只是基础设施坏了 —— ${e.message}`
      : `\n✗ ${e.message}`);
    writeReport(true);
    process.exitCode = 1;
  }
} finally {
  await stopAll().catch((e) => console.error("清理失败：", e.message));
  for (const i of ALL_INST) {
    try { fs.copyFileSync(i.db, path.join(RUN_DIR, `after-${i.label}.db`)); } catch { /* 没有 */ }
  }
  // 用户原来的库必须回来 —— 覆盖掉本轮写出来的测试库
  for (const [dbFile, from] of backups) {
    for (const s of ["", "-wal", "-shm"]) {
      if (fs.existsSync(from + s)) fs.renameSync(from + s, dbFile + s);
      else if (fs.existsSync(dbFile + s)) fs.rmSync(dbFile + s);
    }
  }
  // §十六 产物保留（2026-09-26 拍板＝选项 C；**2026-09-27 二次拍板：失败轮也删这两份**）。
  // 删的只有 `src/` + `recv/` —— 可随时重生成的收发用大文件副本，字节数与哈希已写进 summary.json；
  // 日志 / DB 快照 / 截图 / summary 一律留，**红轮的整个目录仍然在跨轮上限之外（永不整轮删）**。
  // 为什么原来那道"判绿才删"的门要拆：实测 `test-results/` 5.1 GB / 150 轮里，占体积的几乎全是
  // **失败轮**（单轮 222 MB，其中 202 MB 就是这两份副本）⇒ 绿轮上限对红的那一大半根本不生效。
  // 判据仍是纯函数 + 五格自证：每格只换一个它读的输入，少一格成立就说明这条是空的。
  function retentionPlan({ green, negative, reportGaps }) {
    // 报告自己不合格 ⇒ 一件都不动（这不是体积问题，是"这台判据不可信"）
    if (reportGaps) return [];
    // 反向轮**居然判绿** = 按设计该红的东西没红 ⇒ 现场原样留着，别在追查判据失效时先烧掉一半
    if (negative && green) return [];
    return ["src", "recv"];
  }
  function selfcheckRetention() {
    const fails = [];
    const eq = (name, got, want) => {
      const a = JSON.stringify(got), b = JSON.stringify(want);
      if (a !== b) fails.push(`${name}：预期 ${b} / 实际 ${a}`);
    };
    eq("绿轮 ⇒ 删这两份", retentionPlan({ green: true, negative: false, reportGaps: false }), ["src", "recv"]);
    eq("红轮 ⇒ 只删那两份可再生副本（2026-09-27 拍板）",
      retentionPlan({ green: false, negative: false, reportGaps: false }), ["src", "recv"]);
    eq("反向轮按设计判红 ⇒ 同样只删可再生副本",
      retentionPlan({ green: false, negative: true, reportGaps: false }), ["src", "recv"]);
    eq("反向轮居然判绿（判据自己坏了）⇒ 一件都不删",
      retentionPlan({ green: true, negative: true, reportGaps: false }), []);
    eq("报告自己不合格 ⇒ 不删", retentionPlan({ green: true, negative: false, reportGaps: true }), []);
    return fails;
  }
  function dirBytes(d) {
    if (!fs.existsSync(d)) return 0;
    let n = 0;
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      n += e.isDirectory() ? dirBytes(p) : fs.statSync(p).size;
    }
    return n;
  }
  const retFails = selfcheckRetention();
  if (retFails.length) {
    console.error(`✗ 保留策略判据自证不成立 ⇒ 本轮一律不删（宁可留一堆，也不能删错）：\n  ${retFails.join("\n  ")}`);
    process.exitCode = 1;
  } else {
    const green = !process.exitCode && !REPORT_GAPS.length;
    const doomed = retFails.length ? [] : retentionPlan({ green, negative: NEGATIVE, reportGaps: false });
    let freed = 0;
    for (const name of doomed) {
      const target = path.join(RUN_DIR, name);
      if (!fs.existsSync(target)) continue;
      freed += dirBytes(target);
      fs.rmSync(target, { recursive: true, force: true });
    }
    if (doomed.length) {
      console.log(`产物保留（C）：${green ? "本轮判绿" : "本轮判红（含按设计判红的反向轮）"} ⇒ `
        + `已删可再生大文件副本 ${doomed.join(" + ")}`
        + `，释放 ${(freed / 1024 / 1024).toFixed(1)} MB；日志/DB/截图/summary 全留`);
    }
    // 把这件事**回写进刚落盘的 summary.json**，让"这轮删没删、省了多少"可被机器读回；
    // 回写之后**再用同一份 §十六 契约判一遍** —— 回写要是把契约字段弄丢了，必须当场判红。
    // 只报**回写新增**的那几条：`writeReport` 末尾已经判过一次并塞进 REPORT_GAPS，
    // 上一版这里不比较就复述全部 ⇒ 一次真·首次写盘的缺口会被写成"回写弄坏了产物"（误导归因），
    // 还会把同一条 gap 打印两遍。实测锚点：LIE-79 那一轮 3 条 gap 打印成 6 条。
    const sumPath = path.join(RUN_DIR, "summary.json");
    if (fs.existsSync(sumPath)) {
      try {
        const sum = JSON.parse(fs.readFileSync(sumPath, "utf8"));
        sum.retention = { policy: "C", freed_bytes: freed, deleted: doomed };
        fs.writeFileSync(sumPath, JSON.stringify(sum, null, 2));
        const before = new Set(REPORT_GAPS);
        const newly = readReportContract(RUN_DIR).filter((g) => !before.has(g));
        if (newly.length) {
          console.error(`✗ 回写 retention 之后 §十六 报告契约反而不合格 ⇒ 回写弄坏了产物：\n  ${newly.join("\n  ")}`);
        }
        REPORT_GAPS.push(...newly);
      } catch (e) {
        console.error(`✗ 回写 retention 到 summary.json 失败：${e.message}`);
        REPORT_GAPS.push(`回写 retention 失败：${e.message}`);
      }
    }
  }
  // 跨轮上限（2026-09-27 用户拍板"按你建议"）：绿轮只留最近 N 个，**红轮与跑不出结论的轮次一个都不删**。
  // 上面那条策略 C 只管"本轮内部"省空间，从没管过"历史攒多少" ⇒ `test-results/` 才会涨到几百轮十几 GB。
  // 删目录不可逆，所以四条硬约束：① 先自证（每格只换一个输入），自证不过 ⇒ 这一趟一个都不删并把本轮判红；
  // ② 只允许碰 `test-results/run-*`，且**跳过本轮自己**；③ 想多留用 GOSSLAN_KEEP_RUNS 调大上限；
  // ④ **被文档点名的 run-* 不进删除名额**，而名单拿不到时（`git ls-files` 失败）一个都不删（2026-10-03 加，
  //    起因：#65 那批"实测锚点 run-…Z"已经被淘汰掉几个，而当时没有任何机器会报这件事）。
  const KEEP_GREEN_RUNS = Number(process.env.GOSSLAN_KEEP_RUNS || 30);
  // `prunePlan` / `selfcheckPrune` / `citedRunIds` 都在模块级（连同 `--prune-selfcheck` 那条秒级入口，
  // 见 `RUN_DIR` 下面那一段）—— 判据只留一个家，收尾这里只按它给的名单删。
  function classifyRun(dir) {
    const sum = path.join(dir, "summary.json");
    if (!fs.existsSync(sum)) return "unknown"; // 崩在半路 ⇒ 现场比"省空间"值钱
    try {
      const v = JSON.parse(fs.readFileSync(sum, "utf8")).verdict;
      return v === "PASS" ? "green" : v === "FAIL" ? "red" : "unknown";
    } catch {
      return "unknown";
    }
  }
  function pruneOldRuns(cited) {
    const root = path.join(ROOT, "test-results");
    if (!fs.existsSync(root)) return { deleted: [], bytes: 0, left: 0, spared: 0, missingCited: 0, citedAvailable: !!cited };
    const cur = path.basename(RUN_DIR);
    const dirs = fs.readdirSync(root, { withFileTypes: true })
      .filter((e) => e.isDirectory() && e.name.startsWith("run-"))
      .map((e) => e.name);
    const present = new Set(dirs);
    const runs = dirs.filter((n) => n !== cur)
      .map((n) => ({ name: n, outcome: classifyRun(path.join(root, n)) }));
    const doomed = prunePlan({ runs, keep: KEEP_GREEN_RUNS, negative: NEGATIVE, cited });
    // `spared` = 被文档点名且还在盘上的目录数（红轮本来也不删，所以它是"证据正被保护"的分母，
    // 不等于"这一趟多救下几个"）。`missingCited` = 文档点名但盘上找不到 ⇒ 只报数，不改判。
    const spared = cited ? dirs.filter((n) => cited.has(n)).length : 0;
    const missingCited = cited ? [...cited].filter((n) => !present.has(n)).length : 0;
    let bytes = 0;
    const gone = [];
    for (const name of doomed) {
      const dir = path.join(root, name);
      if (name === cur || !dir.startsWith(root + path.sep)) continue; // 双保险：不碰本轮、不跑出根目录
      bytes += dirBytes(dir);
      fs.rmSync(dir, { recursive: true, force: true });
      gone.push(name);
    }
    const left = fs.readdirSync(root, { withFileTypes: true })
      .filter((e) => e.isDirectory() && e.name.startsWith("run-")).length;
    return { deleted: gone, bytes, left, spared, missingCited, citedAvailable: !!cited };
  }
  const pruneFails = selfcheckPrune();
  if (pruneFails.length) {
    console.error(`✗ 跨轮保留自证不成立 ⇒ 旧轮次一律不删（宁可留一堆，也不能删错）：\n  ${pruneFails.join("\n  ")}`);
    REPORT_GAPS.push(`跨轮保留自证不成立：${pruneFails.join(" / ")}`);
  } else {
    const pr = pruneOldRuns(citedRunIds());
    console.log(`跨轮保留：绿轮上限 ${KEEP_GREEN_RUNS} ⇒ 删最老的绿轮 ${pr.deleted.length} 个` +
      `（释放 ${(pr.bytes / 1024 / 1024).toFixed(1)} MB），现存 ${pr.left} 个 run-*；` +
      `红轮与没有 summary.json 的轮次一个都不删（GOSSLAN_KEEP_RUNS 可调上限）`);
    console.log(pr.citedAvailable
      ? `  引用保护：文档点名且仍在盘上的 run-* ${pr.spared} 个一律不删，只有名单外的绿轮才进删除名额` +
        (pr.missingCited ? `；⚠️ 另有 ${pr.missingCited} 个被文档点名的 id 在 test-results 里已经找不到` : "")
      : "  ⚠️ 引用名单没拿到（`git ls-files` 失败）⇒ 这一趟一个都没删");
  }
  if (backups.size) console.log(`已还原用户原有实例库 ${backups.size} 个`);
  // 报告本身不合格 ⇒ 这一轮不许以"跑完了"收场。放在 finally 最末（清理之后、退出之前），
  // 所以它盖得过上面任何一条 exitCode —— 包括反向模式那条"按设计退 0"。
  if (REPORT_GAPS.length) {
    console.error(`\n✗ §十六 报告契约不合格（缺的是**报告自己**的一格，不代表被测功能通过）：\n` +
      REPORT_GAPS.map((g) => `  · ${g}`).join("\n") + `\n  产物 ${RUN_DIR}`);
    process.exitCode = 1;
  }
}
