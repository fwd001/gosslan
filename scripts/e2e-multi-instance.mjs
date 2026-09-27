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
/// 反证开关（只喂给 `seedPair`）：清掉两端的手动 Routed 端点 ⇒ 那条 Routed 判据必须报红。
/// 它改的是**判据读的那个输入**（配了什么端点），不是判据本身。
const NO_ROUTED = process.env.E2E_NO_ROUTED === "1";
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
/// 故障注入模式（§八）。`--fault=poison-part` 见下方 preset 步骤的注释。
/// 另有**旅程轮** `--round=`（不是注入，是补一整条没测过的用户路径）：
///   --round=group    → 群聊这一族跨实例真跑：两端预置群 → A 排三条群消息（正文/撤回/正文）→
///                      对端上线后靠 flush_group_outbox 补发 → 判落库/解密/清队列/G-Set/不串味
///   --round=group-lie→ 预置与投递完全不动，只把判据读的 msg_id 换成不存在的值 ⇒ 预期按设计报红
///   断言条数不在这里写，由 check-doc-numbers 现算对账（同下面每一轮）。
const FAULT = (process.argv.find((a) => a.startsWith("--fault=")) || "").slice("--fault=".length);
//   --shot-selfcheck         → 只跑 §十六 截图判据的三格自证（假 PNG 判假 / 缺文件判假 / 真截图判真），
//                             不起实例、不需要 release 产物：npm run test:e2e:shot-selfproof
//   E2E_NO_CAPTURE=1         → 把采集器关掉（= 截图判据读的那个输入）⇒ 那条截图判据必须红
const POISON = FAULT === "poison-part" || FAULT === "poison-part-lie";
/// 注入②：接收端已有**真实前缀** ⇒ 必须按前缀续传，不许从 0 重灌整份。
const RESUME = FAULT === "resume-prefix" || FAULT === "resume-prefix-lie";
/// 注入③：接收中**真杀进程**。窗口是实测的，不是猜的：100 MB 在回环上 ~0.78s 走完
/// （.part 每 ~52ms 涨 6.5MB）⇒ 20%~100% 之间有 ~0.6s 可打，所以这条不是掷骰子。
const KILL = FAULT === "kill-mid" || FAULT === "kill-mid-lie";
/// 这一条注入专用的尺寸（与 J2 的 1 MB 分开，免得把默认轮也拖慢）。
const KILL_BYTES = Number(process.env.E2E_KILL_MB || 100) * 1024 * 1024;
let xferId4, srcFile4, srcSha4, partAtKill = 0;
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
/// 调结长度。**<45s**：链路还活着，只是对端不回话；
/// **>45s**：越过 watchdog ⇒ 拆链 + 重拨（解冻后由重拨/心跳重新触发 flush）。
/// 两种 regime 用同一组"结局空间"判据，不需要分叉。
const FREEZE_MS = Number(process.env.E2E_FREEZE_S || 30) * 1000;
const FREEZE_BYTES = Number(process.env.E2E_FREEZE_MB || 1) * 1024 * 1024;
/// 注入⑩：**发送端在飞时被 SIGKILL，然后重启** —— §七「发送过程中杀进程」×「重启后继续」、
/// §八「本端重启」、§九「数据生命周期：运行→退出→重新启动」的交叉格。
/// ⚠️ 这一格此前**零跨实例判据**：③ 杀的是**接收端**，全仓没有任何一轮从"发送端崩了"这一侧看过。
/// 它钉的不是"能不能续传"（③/② 已证），而是**崩溃不许把"已入队"这个事实抹掉**：
/// 队列行是"先入队再投递"那条物理定律的载体，如果一次崩溃能让它变成 done 或让它消失，
/// 这一单就永久没人再发了。而**用户侧还有第二个结局**：气泡行（`messages`）也没了的话，
/// 用户连"曾经发过这一单"都看不见 —— 两个结局各自钉一条，所以入队照⑥ 复刻产品的三行。
const SENDKILL = FAULT === "sender-kill-mid" || FAULT === "sender-kill-mid-lie";
/// 和 ③ 同档：100 MB 在回环上传 ~0.8 s，50 ms 自旋才抓得到在飞窗口。
const SENDKILL_BYTES = Number(process.env.E2E_SENDKILL_MB || 100) * 1024 * 1024;
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
const STALL_BYTES = Number(process.env.E2E_STALL_MB || 100) * 1024 * 1024;
/// 冻结时长：必须 **>60 s** 才越过 `FILE_STALL_ABORT_MS`；留 10 s 余量给 5 s 一跳的停滞检查。
const STALL_HOLD_MS = Number(process.env.E2E_STALL_S || 70) * 1000;
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
const DISK_BYTES = Number(process.env.E2E_DISK_MB || 1) * 1024 * 1024;
/// 发送侧重试上限（file.rs:259 MAX_FILE_OUTBOX_RETRIES）—— 判据用它钉"不许无限重试"。
const DISK_MAX_ATTEMPTS = 5;
let xferId6, term6 = null;
/// 注入⑧：A-5 的形状 —— **预置可写 `.part`，再把接收目录改成只读**。
/// 与⑤成对但不重复：⑤ 打在 offer 期（`File::create` 就 EACCES ⇒ 只发 FileReject、
/// **接收侧不写任何行**，见上面那条注释），所以⑤**证明不了**"半途才失败时接收侧怎么收口"。
/// 这一轮把注落下得晚：真前缀已存在 ⇒ 往已存在的 inode 里写**不需要目录写权限**，
/// 于是字节照常流进来，唯一会 EACCES 的是**收尾那次 rename**（还有清理时的 unlink）。
/// ⇒ 这是「rename 才算完成」这条不变量第一次被活实例检验：报 done 就必须有整份正确的文件。
/// ⚠️ 判据刻意不预设"产品必须失败"（那等于替产品做决定）：只判**终态与磁盘自洽**。
/// 解除只读之后会不会自愈 —— 只打印实测，不设判据（没量过的事不写进断言）。
const ROT = FAULT === "recv-dir-rotted" || FAULT === "recv-dir-rotted-lie";
const ROT_BYTES = Number(process.env.E2E_ROT_MB || 1) * 1024 * 1024;
const ROT_PREFIX = 64 * 1024;
let xferId8, srcFile8, srcSha8, term8 = null;
let xferId5, srcFile5, srcSha5;
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
const SHRINK_BYTES = Number(process.env.E2E_SHRINK_MB || 1) * 1024 * 1024;
const SHRINK_TO = Number(process.env.E2E_SHRINK_TO_BYTES || 4096);
let xferId7, srcFile7;
/// 注入⑦：§七「连续多文件」×「磁盘已有同名文件」这两格合起来测 —— 三个 transfer 一起排队，
/// 其中**两个文件名完全相同、内容不同**（真机形状：一次选两张同名截图、或连发两版同名文档）。
/// ⚠️ 为什么这一格值一次运行：接收端的落地名在 **offer 那一刻**由 `unique_path` 决定
///   （`file.rs:1598`，规则 `a.bin → a (1).bin`）。同名两单若在这之前都还没 rename，
///   两者会拿到**同一个 final_path** ⇒ 后一次 rename 直接覆盖前一次 = **静默数据丢失**
///   （`file.rs:1599-1602` 那段注释只解决了 `.part` 交错，没解决 final 撞名）。
///   少一个文件、或落地内容集合少一份，就是这条被判红 —— 不是"看着不顺眼"，是丢东西。
const MULTI = FAULT === "multi-file" || FAULT === "multi-file-lie";
let multiSpec = [];
/// 轮次（§九 旅程族，与 `--fault=` 的注入族并列）：`--round=group` = 群聊这一族跨实例真跑。
/// 为什么这一格值一轮：此前 harness **从未建过群** —— `grep -c group` 只命中 file_outbox 的
/// `group_id` 列名，§九「群聊：创建/同步/发送/成员离线/重新上线/撤回」在跨实例层面是零判据，
/// 而群消息走的是一条与 1:1 完全不同的管道（`group_outbox` 按成员一行 + Gossip 信封 +
/// `GroupAck` 删行 + G-Set 撤回）。
/// ⚠️ 与 1:1 的关键差异（决定了这一轮为什么要自己签名加密）：
///   `flush_group_outbox`（transport.rs:6689-6693）**不做 re-seal**，只是 `from_str` 之后原样
///   `try_send` —— 而 1:1 的 `flush_outbox` 每条都过 `reseal_for_send`。所以停机写入的那段
///   payload 必须**在写库那一刻就已经是合法、已密封、已签名的 Gossip 信封**，
///   放占位串只会得到"B 静默丢弃"（verify_envelope 不过 ⇒ handle_gossip 直接 return，
///   gossip.rs:61-99/194-204），那红的是脚本不是产品。
/// 这一轮顺带就是 §五 点名的两格组合：`群聊 + 离线成员重新上线`（入队时对端进程还没起，
/// 只能靠建链后的 flush 送达）与 `聊天 + 群聊 + 文件`（同一对实例同时背 1:1 与群两条管道，
/// 判据里专门有一格查两者互不串味）。
const ROUND = (process.argv.find((a) => a.startsWith("--round=")) || "").slice("--round=".length);
const GROUP = ROUND === "group" || ROUND === "group-lie";
/// 反向模式：注入与预置完全不动，只把**判据要去找的那个 msg_id** 换成一个必定不存在的值。
/// 报不出红 ⇒ 那几条断言读的不是真落库行。
const GROUP_LIE = ROUND === "group-lie";
/// 链式轮（`--round=gossip3`，用户 2026-09-26 拍板＝建，但只挂在**发版前**那一层，不进日常本地门禁）。
/// 钉的是 §五 点名的 `群聊 + gossip` 交叉里唯一没被跨实例判着的那一半：**经中间人转发的收敛**。
/// 拓扑是 A—B—C 一条链：A 与 C **互相不是好友、也不给任何端点、C 侧关 LAN** ⇒ 它们之间不可能有链路
/// ⇒ C 若收到 A 的群消息，只可能是 B 转发的（不是"直连也能过"的假绿）。
/// ⚠️ 前置判据（"C 侧没有与 A 的建链行"）必须**先**成立，否则后面所有格都失去意义 ——
///    没有它，这一轮测的就不是转发。生产侧的对应形状：gossip 扇出候选取
///    `reachable_neighbors`（有活链路的邻居）而不是 `peers`（知识集），见 lib.rs 的
///    `gossip_fanout_targets_reachable_links`（审计 P0#7）；那一半钉源码，这一半钉真跑。
const CHAIN = ROUND === "gossip3" || ROUND === "gossip3-lie";
/// 反向模式：拓扑、预置、投递全都一样，只把**判据要找的那条 msg_id** 换成必定不存在的值。
const CHAIN_LIE = ROUND === "gossip3-lie";
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
/// 反向模式：拓扑、预置、时序全都一样，只把判据要找的那条 msg_id 换成必定不存在的值。
const LATE_LIE = ROUND === "gossip-late-lie";
const LANOFF_LIE = ROUND === "lanoff-lie";

/// 任务专项轮（第二阶段 §22）：一条任务「创建 → 被指派者收到 → 改成完成」跨两个真实进程。
/// 判的是这一族在 SQLite 里**真的存在**的事实；徽标那个数字是前端 store 算的，不在本层判（见 roadmap §13.3 的层次修正）。
const TASK = ROUND === "task" || ROUND === "task-lie";
const TASK_LIE = ROUND === "task-lie";
/// 转发轮的正文：判据里既要看 C 解出的明文等于它，也要把它写进 A 的 messages（明文列）。
const CHAIN_TEXT = "two-hop group message via B";
// 补递轮用另一段正文：判据里的文本比对就只可能命中这一轮的落库行
const LATE_TEXT = "late joiner replayed by B";
const GROUP_ID = "g-e2e-harness";
const GROUP_NAME = "E2E-Group";
/// 群对称密钥：settings 表 `gk:{group_id}` = base64 的**正好 32 字节**
/// （transport.rs:5984-5986 解码后 `try_into::<[u8;32]>()`，长度不对直接 None ⇒ 永不解密）。
/// 先例：`e2e_peer.rs:62` 的 `GROUP_KEY_B64` 就是同一形状。
const GROUP_KEY_B64 = Buffer.alloc(32);
for (let i = 0; i < 32; i += 4) GROUP_KEY_B64.writeUInt32BE(0x6e00_0000 + i, i);
const GROUP_KEY_STR = GROUP_KEY_B64.toString("base64");
/// `GossipEngine` 的 ttl（`GossipEngine::new(bloom, lru, fanout, ttl)` 第四参，见
/// gossip_engine.rs:244 那组测试的形状）；转发每跳减一，写 0 会让对端直接丢。
const GROUP_TTL = 6;
let gTextId, gRecallId, gText2Id;
/// §8「存储永远保存真实身份」那一格：A 打出来的 @ 正文（用的是 A 自己给 B 存的昵称）。
/// 呈现层可以把它换成「@你」，**库里那串字节一个字都不许动** —— 所以文本与 id 都要留着当比对基准。
let gMentionId = "";
let gMentionText = "";
/// `--round=group` 里 #103（@ 绑身份）那两条线级判据读的 id：一条明文带 `mentions`，一条不带这个键。
let gMentionOnlyId = "";
let gLegacyShapeId = "";

/// `poison-part-lie` = 这组判据自己的**非空转证明**：注入完全一样，只把比对用的期望摘要
/// 换成一个必定不相等的值。产品没坏 ⇒ 判据必须报红；报不出红 ⇒ 那几条断言读的不是真字节。
const LIE = FAULT.endsWith("-lie");
const LIE_SHA = "0".repeat(64);
/// 传输尺寸（默认 1 MB，`E2E_FILE_MB=N` 或 `--size=N` 覆盖）。这个旋钮不是为了测"大文件"本身，
/// 而是先量出**一次传输在回环上真实耗时多久**：「接收中杀进程」这类注入能不能做成
/// 非竞态，取决于窗口有没有那么长。量不出来就老实标 SIMULATED，不许伪装 PASS（§十）。
/// `--size=` 是给**尺寸阶梯**用的：同一轮旅程（文本 + 文件 + 重启）换档位重跑，
/// 证明"换尺寸"不是一条只在一个尺寸上成立的测试。**故意不做成新的注入轮次** ——
/// 加轮次要同步四处登记（MODE_LABEL / 门禁 local 层 / 判据 C 的轮次声明 / 正反两跑），
/// 而阶梯要测的东西与故障无关，复用默认轮的断言才是这一格的正解。
/// ⚠️ 必须 `Math.round`（实测，不是猜的）：`Buffer.alloc(1048.576)` **不抛错**，它静默给一个
/// **1048 字节**的 buffer ⇒ 于是后面那条 `bytes.length === FILE_BYTES` 变成
/// "1048 === 1048.576" = 假 ⇒ 报出来的红长得像产品 bug（"B 侧字节数与发送端一致"失败），
/// 坏的实际是档位算术。本仓已多次踩"红的是脚本不是被保护的东西"，所以取整是这条阶梯的承重。
const SIZE_ARG = process.argv.find((a) => a.startsWith("--size="));
const FILE_MB = SIZE_ARG
  ? Number(SIZE_ARG.slice("--size=".length))
  : Number(process.env.E2E_FILE_MB || 1);
const FILE_BYTES = Math.round(FILE_MB * 1024 * 1024);

import { spawn, spawnSync } from "node:child_process";
import { createHash, createCipheriv, createPrivateKey, randomBytes, randomUUID, sign } from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { BOOT_LINE, bootBaseline, bootReady, countLog, readLogTail, selfcheckLogtail, stashLogs } from "./e2e-logtail.mjs";
import { captureShot, describeShotDir, screenBlockedReason, selfcheckShot } from "./e2e-shot.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const ISO = new Date().toISOString().replace(/[:.]/g, "-");

/// §十六「本轮每一单都要数得出来」：所有 trace id 一律从这里铸，铸出来就登记。
/// 为什么非要注册表而不是最后把变量名抄一遍 —— 同一个坑 #70 踩过：手写名单在注入轮出现后
/// 悄悄漏掉那些单，报告的 trace 指着别的一单，而没有任何一格会红。
const MINTED_IDS = [];
function eid(prefix = "", tail = Math.random().toString(36).slice(2, 8)) {
  const id = `e2e-${prefix ? `${prefix}-` : ""}${ISO}-${tail}`;
  MINTED_IDS.push(id);
  return id;
}
/// 反向轮会拿真 id 拼一个**故意不存在**的 decoy 去查库。它同样是"报告里出现过的本形状 id"，
/// 所以照样登记 —— 否则反向轮会因为报告契约不合格多红一次，把"红恰好落在断言上"那条证据搅浑。
function noteId(id) {
  MINTED_IDS.push(id);
  return id;
}
const RUN_DIR = path.join(ROOT, "test-results", `run-${ISO}`);

// ── 环境事实（不认识的平台直接退，不猜）────────────────────────────
function appDataDir() {
  const home = process.env.HOME || process.env.USERPROFILE;
  switch (process.platform) {
    case "darwin":
      return path.join(home, "Library", "Application Support", "com.gosslan.app");
    case "win32":
      return path.join(process.env.APPDATA, "com.gosslan.app");
    default:
      return null;
  }
}
function binaryPath() {
  if (process.env.GOSSLAN_E2E_BIN) return process.env.GOSSLAN_E2E_BIN;
  const exe = process.platform === "win32" ? "gosslan.exe" : "gosslan";
  return path.join(ROOT, "src-tauri", "target", "release", exe);
}

const APPDATA = appDataDir();
const BIN = binaryPath();
if (!APPDATA || !fs.existsSync(APPDATA)) {
  console.error(`✗ 找不到 app data 目录：${APPDATA ?? "(本平台不认识)"} —— 先跑过一次应用再说`);
  process.exit(2);
}
if (!fs.existsSync(BIN)) {
  console.error(`✗ 没有二进制：${BIN}\n  先 npm run build && (cd src-tauri && cargo build --release)`);
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
      console.error(`  两种情况的处置相同且只要一分半：cd src-tauri && cargo build --release`);
      process.exit(2);
    }
  }
}
function newestMtime(dir, depth) {
  let m = 0;
  if (depth <= 0) return m;
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    if (e.name === "target" || e.name.startsWith(".")) continue;
    const p = path.join(dir, e.name);
    m = Math.max(m, e.isDirectory() ? newestMtime(p, depth - 1) : fs.statSync(p).mtimeMs);
  }
  return m;
}

// ── 实例定义 ───────────────────────────────────────────────────────
const TCP_BASE = 59992; // protocol.rs TCP_PORT；instance>0 时端口 = TCP_PORT + N*10
const INSTANCES = [1, 2].map((n) => ({
  n,
  label: n === 1 ? "A" : "B",
  port: TCP_BASE + n * 10,
  db: path.join(APPDATA, `gosslan-${n}.db`),
  log: path.join(APPDATA, "logs", `gosslan-${n}.log`),
}));
/// 第三实例（只有 `--round=gossip3` 会真启动它）。产品侧对实例号没有上限，也没有特判：
/// 端口 = `TCP_PORT + instance*10`（state.rs:1282）、运行时身份 = `base-iN`（state.rs:1272）、
/// 库与日志各自一份（state.rs:1186/1207）—— 所以"第三个实例"不需要动产品码，只是把同一套隔离再套一份。
const INST_C = {
  n: 3,
  label: "C",
  port: TCP_BASE + 3 * 10,
  db: path.join(APPDATA, "gosslan-3.db"),
  log: path.join(APPDATA, "logs", "gosslan-3.log"),
};
/// **可能被本轮真启动过**的全部实例。备份/还原、`after-*.db`、失败时的日志关联一律按这份清单走：
/// 少算一格 = 把本轮写出来的测试库留在用户 appdata 里，而且第三实例红的时候报告里没有它的现场。
const ALL_INST = [...INSTANCES, INST_C];

// ── 小工具 ─────────────────────────────────────────────────────────
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const nowMs = () => Date.now();

function openDb(file, readonly = false) {
  return new DatabaseSync(file, { readOnly: readonly });
}
/** 停机时写库：WAL 要先 checkpoint 再关，否则 -wal 里的内容下一次启动才被吸收（本项目实测口径）。 */
function seed(file, fn) {
  const db = openDb(file);
  try {
    fn(db);
    db.exec("PRAGMA wal_checkpoint(TRUNCATE);");
  } finally {
    db.close();
  }
}
async function waitFor(cond, ms, what) {
  const until = nowMs() + ms;
  while (nowMs() < until) {
    if (await cond()) return true;
    await sleep(500);
  }
  throw new Error(`超时（${ms}ms）等 ${what}`);
}

/// 有界等**发送侧自己走到终态**，返回最后一次快照（不抛 —— 到点就把实际值交给 check 判红）。
///
/// 为什么需要它：**「B 的终名文件出现了」不是 A 的同步点**。A 的 `done` 与删 `file_outbox` 行
/// 发生在收到对端 `FileCompleteAck` 之后，而那条 ack 在接收端 rename **之后**才发
/// ⇒ 天然顺序是"先见 B 落地、后见 A 终态"。
/// 实测（2026-09-26 本地层第 12 步）：解冻→落地只 5.5s 的那一刻读 A ⇒ `{A:active, outbox:1}`
/// 被判成红，而同一步里 B 已 done、字节与 sha 全对、无 `.part` 残留 —— **红的是判据读早了，不是产品分叉**。
/// 窗口默认 60s：心跳 5s（transport.rs:1992）+ 文件队列退避 5s（file.rs:303）⇒ 该走得到的路最长也就几跳，
/// 60s 还停在 `active`/`pending` 才是真红（用户界面就是"永远转圈"）。
async function waitSendTerminal(id, timeoutMs = 60_000) {
  const t0 = nowMs();
  let row = null;
  let queued = -1;
  // 句柄只开一次：这一条循环最多要读 240 次，若每轮都 `openDb()`，
  // 撞上 SQLITE_BUSY 的概率被放大两个量级，而那时抛的是"技术错误"不是断言红（读的是别人进程的库）。
  const db = openDb(INSTANCES[0].db, true);
  try {
    const qRow = db.prepare("SELECT status,progress FROM file_transfers WHERE id=?1");
    const qQ = db.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1");
    for (;;) {
      row = qRow.get(id) ?? null;
      queued = qQ.get(id).c;
      if (row && (row.status === "done" || row.status === "failed" || row.status === "cancelled")) break;
      if (nowMs() - t0 >= timeoutMs) break;
      await sleep(250);
    }
  } finally {
    db.close();
  }
  return { row, queued, waitedMs: nowMs() - t0 };
}
function tcpOpen(port) {
  return new Promise((res) => {
    const s = net.connect({ host: "127.0.0.1", port });
    s.setTimeout(800);
    s.once("connect", () => { s.destroy(); res(true); });
    s.once("timeout", () => { s.destroy(); res(false); });
    s.once("error", () => res(false));
  });
}
// 读实例日志一律走 e2e-logtail：应用会把超 512 KB 的日志整份轮转成 `.old.log`，
// 只看当前档会让「这一单真跑过」这类判据在轮转瞬间凭空看不见（10 MB 轮实测踩过）。
const tailLog = (file, n = 400) => readLogTail(file, n);

// PKCS8 定长前缀 + 32 字节种子 → 导出公钥（Node 原生支持 X25519 / Ed25519）
const PKCS8 = { x25519: "302e020100300506032b656e04220420", ed25519: "302e020100300506032b657004220420" };
function pubFromSecret(kind, b64secret) {
  const der = Buffer.concat([Buffer.from(PKCS8[kind], "hex"), Buffer.from(b64secret, "base64")]);
  const jwk = createPrivateKey({ key: der, format: "der", type: "pkcs8" }).export({ format: "jwk" });
  return Buffer.from(jwk.x, "base64url").toString("base64"); // 应用侧统一标准 base64
}

/// 停机时刻自制一个**合法**的群 Gossip 信封（`--round=group` 专用）。
/// 为什么必须由 harness 签名加密，而不是像 1:1 那样丢给应用去 re-seal：
/// `flush_group_outbox`（transport.rs:6689-6693）只 `serde_json::from_str` 再原样 `try_send`，
/// **不过 `reseal_for_send`** ⇒ 写进 `group_outbox.payload` 的那段 JSON 会被逐字节发上线。
/// 三把材料 harness 全都有：A 的 ed25519 私钥（settings）、A 的两把公钥（readIdentity 已导出）、
/// 以及 harness 自己写进 `settings['gk:{gid}']` 的群对称密钥。
/// 每一处序列化都必须与 Rust 侧逐字对齐，错一处得到的就是"B 静默丢弃"（红在脚本）：
/// - `message_id` = SHA-256(sender_id ‖ nonce ‖ payload) 的**小写 hex**（protocol.rs:856-863）
/// - 签名材料 = 这 15 个字段的**紧凑 JSON 数组**，顺序照 `signing_bytes()`（protocol.rs:868-885），
///   `ttl` 不在里面（中继会递减），`target` 为 `null`
/// - 载荷 = base64(nonce12 ‖ ChaCha20-Poly1305(群密钥, {"kind":..,"content":..}))，无 AAD
///   （crypto.rs:83-97 `seal`，`encrypt` 不带 associated data）
/// - `kind` 恒为 `"group"`（GossipKind 的 snake_case），`encrypted` 恒 true
function buildGroupEnvelope(o) {
  const iv = randomBytes(12);
  const c = createCipheriv("chacha20-poly1305", o.groupKey, iv, { authTagLength: 16 });
  // 明文形状必须与产品侧 `protocol::gossip_plaintext` 逐字同形，**包括 mentions 的三态**：
  // 键不存在 = 旧版本发出来的样子；键存在（含空数组）= 发送方的权威回答。
  // 这一格要能在 harness 里分别造出这两种，所以由调用方"传不传 o.mentions"决定。
  const plain = { kind: o.kind, content: o.content };
  if (o.mentions !== undefined) plain.mentions = o.mentions;
  const sealed = Buffer.concat([
    c.update(JSON.stringify(plain), "utf8"),
    c.final(),
    c.getAuthTag(),
  ]);
  const payload = Buffer.concat([iv, sealed]).toString("base64");
  const nonce = randomUUID();
  const messageId = createHash("sha256")
    .update(Buffer.concat([
      Buffer.from(o.senderId, "utf8"),
      Buffer.from(nonce, "utf8"),
      Buffer.from(payload, "utf8"),
    ]))
    .digest("hex");
  const signing = JSON.stringify([
    messageId, o.senderId, nonce, o.x25519Pub, o.ed25519Pub,
    "group", o.groupId, o.groupName, o.creator, o.members, payload, o.ts, o.seq, true, null,
  ]);
  const env = {
    message_id: messageId,
    sender_id: o.senderId,
    nonce,
    sender_pubkey: o.x25519Pub,
    sender_ed25519: o.ed25519Pub,
    sender_sig: sign(null, Buffer.from(signing, "utf8"), o.priv).toString("base64"),
    ttl: GROUP_TTL,
    kind: "group",
    group_id: o.groupId,
    group_name: o.groupName,
    group_creator: o.creator,
    group_members: o.members,
    payload,
    ts: o.ts,
    seq: o.seq,
    encrypted: true,
  };
  return { messageId, wire: JSON.stringify({ type: "gossip", envelope: env }) };
}

/// 从实例库里取回 ed25519 私钥对象（只有 harness 需要，产品侧从不导出私钥）。
function ed25519Priv(inst) {
  const db = openDb(inst.db, true);
  try {
    const b64 = db.prepare("SELECT value FROM settings WHERE key='ed25519_secret'").get()?.value;
    if (!b64) throw new Error(`${inst.label} 库里没有 ed25519_secret`);
    const der = Buffer.concat([Buffer.from(PKCS8.ed25519, "hex"), Buffer.from(b64, "base64")]);
    return createPrivateKey({ key: der, format: "der", type: "pkcs8" });
  } finally { db.close(); }
}

// ── 生命周期 ───────────────────────────────────────────────────────
const procs = new Map();
let stashSeq = 0;
/** 每次 launch 前把该实例的历史日志移进 run 目录（不删）；本轮写的行因此必然在当前档里。 */
const bootBaseOf = new Map();
function launch(inst) {
  stashLogs(inst.log, RUN_DIR, `${inst.label}-${++stashSeq}`);
  bootBaseOf.set(inst.n, bootBaseline(inst.log, BOOT_LINE));
  const out = fs.openSync(path.join(RUN_DIR, `instance-${inst.label}.stdout.log`), "a");
  const p = spawn(BIN, [], {
    env: { ...process.env, GOSSLAN_INSTANCE: String(inst.n), GOSSLAN_AUTOSTART: "1" },
    stdio: ["ignore", out, out],
    detached: false,
  });
  procs.set(inst.n, p);
  return p;
}
async function stopAll() {
  for (const [n, p] of procs) {
    if (p.exitCode === null) p.kill("SIGTERM");
    try {
      await Promise.race([
        new Promise((r) => p.once("exit", r)),
        sleep(8000).then(() => { if (p.exitCode === null) p.kill("SIGKILL"); }),
      ]);
    } catch { /* 已经退了 */ }
    procs.delete(n);
  }
  // 残留必须是 0 —— 「没清理干净」本身就是失败，不许静默
  const left = procsLeft();
  if (left.length) throw new Error(`清理后仍有实例活着：${left.join(", ")}`);
}
/// 只停**一个**实例（`#89` 那一轮要用：观察者必须一直活着，被观察的那台重启）。
/// ⚠️ 别用 `p.exitCode === null` 判"还活着"：**被信号杀死时 exitCode 就是 null**（第一次跑这条
///    把自己判成了"SIGKILL 之后还没退"）。要看的是 pid 还在不在，与 `stopAll` 末尾那个残留扫描同一口径。
async function stopOne(inst) {
  const p = procs.get(inst.n);
  if (!p) throw new Error(`stopOne(${inst.label})：procs 里没有这个实例（它没被 launch 过？）`);
  const alive = () => {
    try { process.kill(p.pid, 0); return true; } catch { return false; }
  };
  if (alive()) p.kill("SIGTERM");
  await Promise.race([
    new Promise((r) => p.once("exit", r)),
    sleep(8000).then(() => { if (alive()) p.kill("SIGKILL"); }),
  ]);
  let gone = false;
  for (let i = 0; i < 40 && !gone; i++) {
    gone = !alive();
    if (!gone) await sleep(250);
  }
  procs.delete(inst.n);
  if (!gone) throw new Error(`stopOne(${inst.label})：SIGTERM + SIGKILL 之后 pid ${p.pid} 还在`);
}
function procsLeft() {
  if (process.platform === "win32") {
    const exe = path.basename(BIN);
    const r = spawnSync("tasklist", ["/FI", `IMAGENAME eq ${exe}`, "/FO", "CSV", "/NH"], { encoding: "utf8" });
    return ((r.stdout || "").includes(exe)) ? [exe] : [];
  }
  const r = spawnSync("pgrep", ["-f", `${BIN}`], { encoding: "utf8" });
  return (r.stdout || "").trim().split("\n").filter(Boolean);
}
async function bootAndStop(what) {
  for (const i of INSTANCES) launch(i); // launch 内部会先给该实例清档，基线随之一并重置
  for (const i of INSTANCES) {
    await waitFor(() => tcpOpen(i.port), 60_000, `${what}：实例 ${i.label} 的 TCP ${i.port} 可连`);
    await waitFor(() => bootReady(i.log, bootBaseOf.get(i.n), BOOT_LINE), 20_000,
      `${what}：实例 ${i.label} 打出 boot 完成行`);
  }
  await stopAll();
}

// ── 预置（L-A）─────────────────────────────────────────────────────
function readIdentity(inst) {
  const db = openDb(inst.db, true);
  try {
    const get = (k) => db.prepare("SELECT value FROM settings WHERE key=?1").get(k)?.value ?? null;
    const base = get("device_id");
    const xSec = get("x25519_secret");
    const eSec = get("ed25519_secret");
    if (!base || !xSec || !eSec) throw new Error(`${inst.label} 库里缺身份键（base=${base}）`);
    // 陷阱 4：secret 格式错时应用会**静默重新生成**，所以这里必须回读校验长度
    if (Buffer.from(xSec, "base64").length !== 32 || Buffer.from(eSec, "base64").length !== 32) {
      throw new Error(`${inst.label} 的密钥不是 32 字节 base64 —— 预置失败，别往下走`);
    }
    return {
      // 陷阱 1：instance>0 时运行时 id 是 base + "-iN"，settings.device_id 只是基名
      runtimeId: `${base}-i${inst.n}`,
      x25519Pub: pubFromSecret("x25519", xSec),
      ed25519Pub: pubFromSecret("ed25519", eSec),
    };
  } finally { db.close(); }
}
function seedPair(nodes) {
  INSTANCES.forEach((inst, idx) => {
    const peer = nodes[1 - idx];
    const recv = path.join(RUN_DIR, "recv", inst.label);
    fs.mkdirSync(recv, { recursive: true });
    seed(inst.db, (db) => {
      db.prepare("DELETE FROM friends WHERE device_id=?1").run(peer.runtimeId);
      // 陷阱 5：ed25519 留 NULL 交给 TOFU 自绑（预置错值会永久拒 Hello 且不可恢复）；
      // x25519 必须是真值，否则 re-seal 拿不到密钥 ⇒ 明文发出 ⇒ 对端静默丢弃。
      db.prepare(
        `INSERT INTO friends(device_id,nickname,avatar,x25519_pubkey,ed25519_pubkey,added_at)
         VALUES(?1,?2,NULL,?3,NULL,?4)`,
      ).run(peer.runtimeId, `e2e-${peer.label}`, peer.x25519Pub, Date.now());
      // E2E_NO_ROUTED=1 把这条预置清空 —— 给下面那条 Routed 判据当"只换判据的输入"的反证：
      // 端点没配 ⇒ 那一格必须报红，其余格照常（LAN 广播还在，投递不该受影响）。
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints',?1)")
        .run(NO_ROUTED ? "[]" : JSON.stringify([{ address: `127.0.0.1:${peer.port}` }]));
      // 陷阱：macOS 上 load() 优先信书签 ⇒ 只写路径，绝不写 downloads_dir_bookmark
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('downloads_dir',?1)").run(recv);
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('lan_enabled','true')").run();
    });
  });
}

// ── 断言账本 ───────────────────────────────────────────────────────
const steps = [];
/** §十六 报告要的界面截图（真实文件路径，落 RUN_DIR/screenshots/）。 */
const shotFiles = [];
const assertions = [];
let curStep = null;
let curStepIdx = -1;
function step(name, fn) { steps.push({ name, fn }); }
function check(name, pass, expect, actual) {
  // `undefined` 会被 JSON.stringify **整个键丢掉** —— 于是"读不到那一行"的断言一红，
  // summary.json 反而缺了 §十六 点名的「预期/实际」两栏（实测 run-2026-09-26T09-20-42-139Z：
  // 报告契约判红两条，理由正是"有条断言缺 预期/实际 之一"）。判红的那一格必须依然读得出预期与实际。
  const show = (v) => (v === undefined ? "<无此行/undefined>" : v);
  assertions.push({
    stepIdx: curStepIdx, step: curStep?.name ?? null,
    name, verdict: pass ? "PASS" : "FAIL", expect: show(expect), actual: show(actual),
  });
  console.log(`  ${pass ? "✅" : "❌"} ${name}${pass ? "" : `\n      预期 ${expect} / 实际 ${actual}`}`);
  return pass;
}
/// 本轮总判据**只能**由断言账本推出来。
/// 之前的写法是「没抛异常 = 成功」，而 `check()` 返回 false 并不抛 ⇒
/// 一条真回归会被写成 summary.json 里的 PASS，只有人盯着控制台才看得见。
/// 机器判据不能依赖人读日志。
const anyFail = () => assertions.some((a) => a.verdict === "FAIL");
/// 「起 A/B 并等链路真的建立」那一步的下标。反向自证要求红**落在它之后**：
/// 红若落在建链之前，那只是应用没起来，证明不了「断言依赖真实投递」。
const linkStepIdx = () => steps.findIndex((s) => s.name.startsWith("起 A/B"));
/// 失败时把两端日志里出现这条 trace 的行摘出来 —— §十六要的「日志关联」不是写个文件名，
/// 而是要能顺着 msg_id / transfer_id 直接看见对端说过什么。
// 失败时"沿链追踪"要看的是**本轮自己造的那些单**的日志。
// ⚠️ 以前这里手写四个变量名，于是注入轮（⑤⑥⑦⑧⑨⑩ 各自另造 id）的日志一行都摘不到，
// 而报告里 `transfer_id` 还指着同一轮里另一单（J2 那一单）⇒ 追踪会指到错的那一单。
// 实测锚点：run-2026-09-26T01-42-36-986Z —— 注入⑧ 的 `e2e-i-…-dqy5zx` 在 A 侧日志有 132 行，
// 报告摘出的行里含它 **0 行**，`trace.transfer_id` = `e2e-x-…`。
// 所有 id 都是 `e2e-…<本轮 ISO>-随机` 的形状 ⇒ 按形状匹配，新增轮次不必登记，也就不会再次漏。
function traceExcerpt() {
  const re = new RegExp(`e2e-\\S*${ISO.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`);
  const out = {};
  for (const i of ALL_INST) {
    const body = tailLog(i.log, 20000) || "";
    out[i.label] = body.split("\n").filter((l) => re.test(l)).slice(-8);
  }
  return out;
}
async function runStep(i, s) {
  curStep = s;
  curStepIdx = i;
  const t0 = nowMs();
  const before = assertions.length;
  process.stdout.write(`\n[${i + 1}/${steps.length}] ${s.name}\n`);
  let threw = null;
  try {
    await s.fn();
  } catch (e) {
    threw = e;
    check(s.name, false, "不抛错", String(e.message ?? e));
  }
  const made = assertions.slice(before);
  s.ms = nowMs() - t0;
  s.checks = made.length;
  s.verdict = made.some((a) => a.verdict === "FAIL") ? "FAIL" : made.length ? "PASS" : "NO-ASSERT";
  if (s.verdict === "FAIL") s.logs = traceExcerpt();
  console.log(`      ${(s.ms / 1000).toFixed(1)}s · ${s.checks} 断言 · ${s.verdict}`);
  if (threw) throw threw;
}

// ── J1：文本消息 A→B 全链路 ────────────────────────────────────────
let idA, idB, msgId, NODES, peerTo, xferId, srcFile, srcSha;
let xferId2, srcFile2, srcSha2, xferId3, srcFile3, srcSha3;
let chainMsgId; // 链式轮那条群消息的 msg_id（信封里是 sha256，由 buildGroupEnvelope 算出来）
step("停机预置：好友 + routed 端点 + 独立接收目录", () => seedPair(NODES));

step("L-A 入队：在 A 的库里留下「已入队待发送」的事实", () => {
  msgId = eid();
  // 反向模式：链路照建，只是注定送不到 —— 报红必须来自投递断言本身
  peerTo = NEGATIVE ? `${idB.runtimeId}-ghost` : idB.runtimeId;
  const ts = nowMs();
  const payload = JSON.stringify({
    type: "chat_message",
    msg_id: msgId,
    from: idA.runtimeId,
    to: peerTo,
    kind: "text",
    content: "enc1:harness-placeholder", // 占位；应用会 re-seal 成真密文
    ts,
    seq: 1,
  });
  seed(INSTANCES[0].db, (db) => {
    db.prepare("DELETE FROM messages WHERE msg_id=?1").run(msgId);
    db.prepare("DELETE FROM outbox WHERE msg_id=?1").run(msgId);
    // 陷阱 10：两行必须成对 —— 只插 outbox 则 re-seal 没有明文，只插 messages 则 Ack 找不到人
    db.prepare(
      `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
       VALUES(?1,?2,?3,?4,'text',?5,?6,1,'sending')`,
    ).run(msgId, idB.runtimeId, idA.runtimeId, peerTo, "hello from harness", ts);
    db.prepare("INSERT INTO outbox(msg_id,peer_id,payload,created_at) VALUES(?1,?2,?3,?4)")
      .run(msgId, peerTo, payload, ts);
  });
});

step("L-A 入队：A 的一个 1 MB 文件也排好队（停机窗口内）", () => {
  xferId = eid("x");
  const dir = path.join(RUN_DIR, "src");
  fs.mkdirSync(dir, { recursive: true });
  srcFile = path.join(dir, `${xferId}.bin`);
  // 真随机字节：全零会被任何"压缩/去重"路径悄悄改掉而断言看不出来
  const buf = Buffer.alloc(FILE_BYTES);
  for (let i = 0; i < buf.length; i += 32) buf.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
  fs.writeFileSync(srcFile, buf);
  srcSha = createHash("sha256").update(buf).digest("hex");
  seed(INSTANCES[0].db, (db) => {
    db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId);
    // 陷阱 8：local_path 必须真实存在，否则每次重试白烧一个 attempts 配额
    db.prepare(
      `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
       VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
    ).run(xferId, peerTo, srcFile, `${xferId}.bin`, buf.length, nowMs());
  });
});

// ── 故障注入（总指令§八 / roadmap A-2）：--fault=poison-part ─────────
// 为什么选「给接收端预置一段脏 .part」，而不是「改 A 库里的 sha256」：
//   发送侧的 hash 是 `sha256_file_hex()` **发送时从磁盘现算**的（network/file.rs:375/665），
//   DB 里那一份改了就等于没改 —— 那条注入只会红得莫名其妙（我差点就写成那样）。
//   而接收侧 `resume_receive` 明写「不再 truncate，用已有前缀播种 hasher」（file.rs:1317-1388），
//   所以一个**严格短于文件**的脏 .part = 确定性地让最终 sha256 不匹配，零生产码改动、无竞态。
if (POISON) {
  step("故障注入：A 再排一个 1 MB 文件，同时给 B 预置 4 KB 脏 .part 前缀", () => {
    xferId2 = eid("p");
    const dir = path.join(RUN_DIR, "src");
    fs.mkdirSync(dir, { recursive: true });
    srcFile2 = path.join(dir, `${xferId2}.bin`);
    const buf = Buffer.alloc(FILE_BYTES);
    for (let i = 0; i < buf.length; i += 32) buf.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
    fs.writeFileSync(srcFile2, buf);
    srcSha2 = createHash("sha256").update(buf).digest("hex");
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId2);
      db.prepare(
        `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
         VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
      ).run(xferId2, peerTo, srcFile2, `${xferId2}.bin`, buf.length, nowMs());
    });
    // 脏前缀必须严格短于文件：等长或更长会让接收端回 received >= size，那走的是
    // AlreadyHave 分支（合法地宣布"我早收完了"），就不是在测 hash 拒收这一格。
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const junk = Buffer.alloc(4096);
    for (let i = 0; i < junk.length; i += 32) junk.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
    fs.writeFileSync(path.join(dl, `${xferId2}.part`), junk);
    console.log(`      预置脏 .part = ${junk.length} 字节 / 真实文件 = ${buf.length} 字节`);
  });
}

// ── 注入②：--fault=resume-prefix ────────────────────────────────────
// 与"脏前缀"只差一件事：这里预置的 64 KiB 是**源文件自己的开头**。
// 于是接收端报出的已收字节是真值 ⇒ 发送端必须从 65536 续发；hasher 用真前缀播种后，
// 最终 sha256 必须仍然等于源。它钉的是 decide_offer 注释里那次 160MB 真机事故
// （"有活跃接收器时一律 Accept" ⇒ 每轮从 0 重灌 ⇒ 界面恒 0% ⇒ 最后判"分片失败"）：
// **对有效前缀不许重灌**是行为契约，不是性能偏好。
if (RESUME) {
  step("预置（注入②）：B 侧已有真前缀 64 KiB + A 侧待发一个 1 MB 文件", () => {
    xferId3 = eid("r");
    const dir = path.join(RUN_DIR, "src");
    fs.mkdirSync(dir, { recursive: true });
    srcFile3 = path.join(dir, `${xferId3}.bin`);
    const buf = Buffer.alloc(FILE_BYTES);
    for (let i = 0; i < buf.length; i += 32) buf.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
    fs.writeFileSync(srcFile3, buf);
    srcSha3 = createHash("sha256").update(buf).digest("hex");
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId3);
      db.prepare(
        `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
         VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
      ).run(xferId3, peerTo, srcFile3, `${xferId3}.bin`, buf.length, nowMs());
    });
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    fs.writeFileSync(path.join(dl, `${xferId3}.part`), buf.subarray(0, 64 * 1024));
    console.log(`      预置真前缀 65536 字节 / 全文件 ${buf.length} 字节（sha256=${srcSha3.slice(0, 12)}…）`);
  });
}

if (KILL) {
  step(`预置（注入③）：先生成一个 ${KILL_BYTES / 1024 / 1024} MB 源文件（行进库留到判据里，见下面那段注释）`, () => {
    xferId4 = eid("k");
    const dir = path.join(RUN_DIR, "src");
    fs.mkdirSync(dir, { recursive: true });
    srcFile4 = path.join(dir, `${xferId4}.bin`);
    const buf = Buffer.alloc(KILL_BYTES);
    for (let i = 0; i < buf.length; i += 32) buf.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
    fs.writeFileSync(srcFile4, buf);
    srcSha4 = createHash("sha256").update(buf).digest("hex");
    console.log(`      源文件 ${buf.length / 1024 / 1024} MB（sha256=${srcSha4.slice(0, 12)}…）`);
  });
}

/// 群聊这一族（`--round=group`）的预置。形状照 `e2e_peer.rs:300-338` 的 `ensure_test_group`
/// （仓内既有先例：停机给真实例写群记录），两端各写三行：
/// `settings['gk:{gid}']`（对称密钥，transport.rs:5981-5986 要求 base64 解出正好 32 字节）、
/// `groups` + `group_members`（发送侧 window.rs:133-143 两者缺一就 `Err`）、
/// `conversations`（e2e_peer 也写了；不写也能跑，但搜索谓词会漏掉无会话行的历史）。
/// A 侧再排两条群消息（正文 seq=1、撤回 seq=2）：**入队时对端进程还没起** ⇒
/// 这一轮只能靠建链后的 `flush_group_outbox` 送达，正是 §五「群聊 + 离线成员重新上线」那一格。
let taskCreateId = "";
let taskUpdateId = "";
let taskArchId = "";
let taskReopenId = "";
/// 对端发起的那两条（创建 / 改成完成）。以前这一轮只有 A→B 一个方向 ⇒
/// "任务只能由本端发起"这一半从头到尾没被判过，B 签名/对端 creator 这条授权输入也没人核。
let taskBCreateId = "";
let taskBDoneId = "";
/// 崩溃那一腿的两条（发送端在"还没送达"的时刻被 SIGKILL）。判的不是"能不能续传"
/// （②③⑩ 已证），而是**群任务这一族的「已入队」这个事实许不许被一次崩溃抹掉**。
let taskCrashCreateId = "";
let taskCrashDoneId = "";
/// 任务描述里带的那两张图片**引用**（真实字节走群文件管线，SQLite 不存 BLOB）。
/// 五个字段各有各的用途：`subtype` 决定卡片里是缩略图还是文件块、`sha256`/`id` 是接收方
/// 在本地目录里找字节的键、`size` 用于进度与完整性 —— 少一个都是**静默**的（不会报错，只会显示不对）。
let taskImages = [];
/// 任务轮预置：两端同一份群 + 群密钥（与群聊轮同形），A 排两条群载荷 ——
/// `todo`（创建，seq=1，指派给 B）与 `todo_update`（B 视角下的完成，seq=2）。
/// 载荷字段逐字对齐 `protocol.rs::TodoPayload`（todo_id/title/assignees/status/creator/
/// deleted/description/images/archived/done_at），**snake_case 无 rename**。
if (TASK) {
  step("任务预置：两端各写一份群 + 同一份群密钥，A 排「创建」与「完成」两条任务载荷", () => {
    const members = [idA.runtimeId, idB.runtimeId];
    const convId = `group:${GROUP_ID}`;
    const ts = nowMs();
    for (const inst of INSTANCES) {
      seed(inst.db, (db) => {
        db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)")
          .run(`gk:${GROUP_ID}`, GROUP_KEY_STR);
        db.prepare("INSERT OR REPLACE INTO groups(id,name,creator,created_at) VALUES(?1,?2,?3,?4)")
          .run(GROUP_ID, GROUP_NAME, idA.runtimeId, ts);
        db.prepare("DELETE FROM group_members WHERE group_id=?1").run(GROUP_ID);
        for (const m of members) {
          db.prepare("INSERT OR IGNORE INTO group_members(group_id,device_id) VALUES(?1,?2)")
            .run(GROUP_ID, m);
        }
        db.prepare(
          "INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at)"
          + " VALUES(?1,'group',?2,NULL,0,?3)",
        ).run(convId, GROUP_NAME, ts);
      });
    }
    const todoId = "todo-e2e-1";
    const mk = (over) => JSON.stringify({
      todo_id: todoId, title: "e2e task", assignees: [idB.runtimeId], status: "todo",
      creator: idA.runtimeId, deleted: false, description: "", images: [], archived: false,
      done_at: null, ...over,
    });
    const base = {
      groupKey: GROUP_KEY_B64, senderId: idA.runtimeId, priv: ed25519Priv(INSTANCES[0]),
      x25519Pub: idA.x25519Pub, ed25519Pub: idA.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: idA.runtimeId, members,
    };
    const c = buildGroupEnvelope({ ...base, kind: "todo", content: mk({}), ts, seq: 1 });
    const u = buildGroupEnvelope({
      ...base, kind: "todo_update", content: mk({ status: "done", done_at: ts + 5 }), ts: ts + 1, seq: 2,
    });
    // §7 那两条迁移（完成后归档 ⇒ 与我相关的数从 1 掉到 0；再重开 ⇒ 又回到 1）。
    // 载荷形状与命令层一致：`resolve_done_archive` 只允许 done 带 archived，
    // 重开则 status 回 doing、archived=false、done_at 清空 —— 照抄这个口径，不自创一套。
    const ar = buildGroupEnvelope({
      ...base, kind: "todo_update",
      content: mk({ status: "done", done_at: ts + 5, archived: true }), ts: ts + 2, seq: 3,
    });
    const re = buildGroupEnvelope({
      ...base, kind: "todo_update",
      content: mk({ status: "doing", done_at: null }), ts: ts + 3, seq: 4,
    });
    taskCreateId = c.messageId;
    taskUpdateId = u.messageId;
    taskArchId = ar.messageId;
    taskReopenId = re.messageId;
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM group_outbox WHERE group_id=?1").run(GROUP_ID);
      for (const [env, seq, kind, content, at] of [
        [c, 1, "todo", mk({}), ts],
        [u, 2, "todo_update", mk({ status: "done", done_at: ts + 5 }), ts + 1],
        [ar, 3, "todo_update", mk({ status: "done", done_at: ts + 5, archived: true }), ts + 2],
        [re, 4, "todo_update", mk({ status: "doing", done_at: null }), ts + 3],
      ]) {
        db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
        db.prepare(
          `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
           VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'sent')`,
        ).run(env.messageId, convId, idA.runtimeId, GROUP_ID, kind, content, at, seq);
        db.prepare(
          `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
           VALUES(?1,?2,?3,?4,?5)`,
        ).run(env.messageId, GROUP_ID, idB.runtimeId, env.wire, at);
      }
      db.prepare(
        "INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq",
      ).run(convId, 4);
    });

    // ★ 方向反过来再排一次：B 自己建一条任务、自己改成完成，A 只是收的一方。
    // 为什么这是**合法**形状而不是伪造：命令层的改/删授权是
    // `def.creator == actor || group_creator == actor`（commands/group_files.rs:181），
    // B 是这条任务自己的 creator ⇒ 走第一支。拿"非创建者改任务"来排这一腿会判到一条
    // 产品本来就不允许的输入上，红得没有意义。
    // 时钟这边预置到 6 不会挡住 A 的 1..4：接收侧走 `observe_clock`=`max(local,observed)`
    // （db/clocks.rs:41-48），只有发送侧的 `next_clock` 会分配新号 —— 已读源码确认，不是猜的。
    const todoId2 = "todo-e2e-2";
    taskImages = [
      {
        id: createHash("sha256").update("e2e-task-image-1").digest("hex"),
        name: "白板照片.jpg", size: 20480,
        sha256: createHash("sha256").update("e2e-task-image-1").digest("hex"),
        subtype: "image",
      },
      {
        id: createHash("sha256").update("e2e-task-file-1").digest("hex"),
        name: "合同.pdf", size: 98304,
        sha256: createHash("sha256").update("e2e-task-file-1").digest("hex"),
        subtype: "file",
      },
    ];
    const mk2 = (over) => JSON.stringify({
      todo_id: todoId2, title: "e2e task by B", assignees: [idA.runtimeId], status: "todo",
      creator: idB.runtimeId, deleted: false, description: "", images: taskImages,
      archived: false, done_at: null, ...over,
    });
    const baseB = {
      groupKey: GROUP_KEY_B64, senderId: idB.runtimeId, priv: ed25519Priv(INSTANCES[1]),
      x25519Pub: idB.x25519Pub, ed25519Pub: idB.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: idA.runtimeId, members,
    };
    const b1 = buildGroupEnvelope({ ...baseB, kind: "todo", content: mk2({}), ts: ts + 4, seq: 5 });
    const b2 = buildGroupEnvelope({
      ...baseB, kind: "todo_update",
      content: mk2({ status: "done", done_at: ts + 9 }), ts: ts + 5, seq: 6,
    });
    taskBCreateId = b1.messageId;
    taskBDoneId = b2.messageId;
    seed(INSTANCES[1].db, (db) => {
      db.prepare("DELETE FROM group_outbox WHERE group_id=?1").run(GROUP_ID);
      for (const [env, seq, kind, content, at] of [
        [b1, 5, "todo", mk2({}), ts + 4],
        [b2, 6, "todo_update", mk2({ status: "done", done_at: ts + 9 }), ts + 5],
      ]) {
        db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
        db.prepare(
          `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
           VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'sent')`,
        ).run(env.messageId, convId, idB.runtimeId, GROUP_ID, kind, content, at, seq);
        db.prepare(
          `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
           VALUES(?1,?2,?3,?4,?5)`,
        ).run(env.messageId, GROUP_ID, idA.runtimeId, env.wire, at);
      }
      db.prepare(
        "INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq",
      ).run(convId, 6);
    });
  });
}

if (GROUP) {
  step("群聊预置：两端各写一份群 + 同一份群密钥，A 再排两条群消息（正文 + 撤回）", () => {
    const members = [idA.runtimeId, idB.runtimeId];
    const convId = `group:${GROUP_ID}`;
    const ts = nowMs();
    for (const inst of INSTANCES) {
      seed(inst.db, (db) => {
        db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)")
          .run(`gk:${GROUP_ID}`, GROUP_KEY_STR);
        db.prepare("INSERT OR REPLACE INTO groups(id,name,creator,created_at) VALUES(?1,?2,?3,?4)")
          .run(GROUP_ID, GROUP_NAME, idA.runtimeId, ts);
        db.prepare("DELETE FROM group_members WHERE group_id=?1").run(GROUP_ID);
        for (const m of members) {
          db.prepare("INSERT OR IGNORE INTO group_members(group_id,device_id) VALUES(?1,?2)")
            .run(GROUP_ID, m);
        }
        db.prepare(
          "INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at)"
          + " VALUES(?1,'group',?2,NULL,0,?3)",
        ).run(convId, GROUP_NAME, ts);
      });
    }
    const base = {
      groupKey: GROUP_KEY_B64, senderId: idA.runtimeId, priv: ed25519Priv(INSTANCES[0]),
      x25519Pub: idA.x25519Pub, ed25519Pub: idA.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: idA.runtimeId, members,
    };
    const t = buildGroupEnvelope({ ...base, kind: "text", content: "hello from group harness", ts, seq: 1 });
    const r = buildGroupEnvelope({
      ...base, kind: "recall", content: JSON.stringify({ target: t.messageId }), ts: ts + 1, seq: 2,
    });
    // ⚠️ 第三条**不被撤回**的正文是必需的，不是为了凑数：第一轮实测（26/27 绿）只有 1 条正文
    // 时，「内容是解密后的明文」与「撤回把正文清空成 ""」这两格**读同一行的同一列**，
    // 于是先落的那格必红 —— 红在判据、不是产品（本项目第三次撞同一形状）。
    // 「真解密成功」这一格只能由一条**永远不会被物化覆盖**的行来证。
    const t2 = buildGroupEnvelope({ ...base, kind: "text", content: "second group message", ts: ts + 2, seq: 3 });
    // §8 那一格：A 打的一句 @B（用的是 A 库里给 B 存的昵称 —— `seedPair` 写的就是 `e2e-<label>`）。
    // 为什么这条值得排：把「@你」做成"存的时候替换"是这条规则最自然的破坏方式，
    // 而它的表现是**另一个人的屏幕上被烧进了我的视角**（同一条消息只能有一个正确存储形态）。
    gMentionText = `@e2e-${INSTANCES[1].label} 帮忙看这条`;
    const mt = buildGroupEnvelope({ ...base, kind: "text", content: gMentionText, ts: ts + 3, seq: 4 });
    // #103 的线级那一半（原来只有单元判据，跨进程没人证过）：**同一条管道**上排两封，
    // 一封明文带 `"mentions":["<B 的 id>"]`，一封**没有这个键**（旧版本的原样形状）。
    // 正文刻意不含任何 `@` ⇒ "B 被点名"这件事只能从名单里读到，按名字一律判不出 ——
    // 这正是"改过名字的人收不到历史上那些 @"的线上形态。
    const mo = buildGroupEnvelope({
      ...base, kind: "text", content: "这条只带身份号，正文里没有名字",
      mentions: [idB.runtimeId], ts: ts + 4, seq: 5,
    });
    const lg = buildGroupEnvelope({
      ...base, kind: "text", content: "旧形状的一条正文", ts: ts + 5, seq: 6,
    });
    gTextId = t.messageId;
    gRecallId = r.messageId;
    gText2Id = t2.messageId;
    gMentionId = mt.messageId;
    gMentionOnlyId = mo.messageId;
    gLegacyShapeId = lg.messageId;
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM group_outbox WHERE group_id=?1").run(GROUP_ID);
      for (const [env, seq, kind, content, at] of [
        [t, 1, "text", "hello from group harness", ts],
        [r, 2, "recall", JSON.stringify({ target: t.messageId }), ts + 1],
        [t2, 3, "text", "second group message", ts + 2],
        [mt, 4, "text", gMentionText, ts + 3],
        [mo, 5, "text", "这条只带身份号，正文里没有名字", ts + 4],
        [lg, 6, "text", "旧形状的一条正文", ts + 5],
      ]) {
        db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
        // 逐列照发送内核（window.rs:179-190）：receiver_id 是**裸 group_id**、初始 status 是
        // 'sent'（不是 1:1 的 'sending'），content 存**明文**（密文只在信封里）
        db.prepare(
          `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
           VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'sent')`,
        ).run(env.messageId, convId, idA.runtimeId, GROUP_ID, kind, content, at, seq);
        // 每个非自身成员一行（window.rs:210-216）；payload 就是那整条已签名帧
        db.prepare(
          `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
           VALUES(?1,?2,?3,?4,?5)`,
        ).run(env.messageId, GROUP_ID, idB.runtimeId, env.wire, at);
      }
      // 时钟必须一起推进，否则 A 之后自己发的消息会撞 seq（window.rs:127-130）
      db.prepare(
        "INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq",
      ).run(convId, 6);
    });
    console.log(`  · 群 ${GROUP_ID}：正文一 ${gTextId.slice(0, 12)}… / 撤回 ${gRecallId.slice(0, 12)}…`
      + ` / 正文二 ${gText2Id.slice(0, 12)}… / @ 那条 ${gMentionId.slice(0, 12)}…`
      + ` / 带名单 ${gMentionOnlyId.slice(0, 12)}… / 旧形状 ${gLegacyShapeId.slice(0, 12)}…`);
  });
}

step("起 A/B 并等链路真的建立（routed 拨号一轮 10s）", async () => {
  for (const i of INSTANCES) launch(i);
  for (const i of INSTANCES) {
    await waitFor(() => tcpOpen(i.port), 60_000, `实例 ${i.label} TCP ${i.port} 可连`);
  }
  // 断言链路成立，而不是靠 sleep：日志里的 +conn peer= 必须出现「对端那个 id」
  const other = { A: { inst: INSTANCES[0], id: idB }, B: { inst: INSTANCES[1], id: idA } };
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
    countLog(INSTANCES[0].log, `建链 peer=${idB.runtimeId} path=routed`) +
    countLog(INSTANCES[1].log, `建链 peer=${idA.runtimeId} path=routed`);
  await waitFor(routedDialed, 45_000, "至少一侧打出与对端的 path=routed 建链行");
  check("这对外部以手动配置的 Routed 端点拨出过链路（§19 网络·Routed）",
    routedDialed() >= 1, "≥1 条 path=routed 建链", routedDialed());
  // 两个实例的窗口此刻都在这台机器的桌面上：留一张"链路真建立了"的界面证据
  shotFiles.push(captureShot(RUN_DIR, "1-link-established"));
});

step("A→B 送达 + Ack 回收 + 无重复", async () => {
  const [aDb, bDb] = [openDb(INSTANCES[0].db, true), openDb(INSTANCES[1].db, true)];
  const peekB = () => bDb.prepare("SELECT * FROM messages WHERE msg_id=?1").all(msgId);
  const outboxLeft = () => aDb.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id=?1").get(msgId).c;
  await waitFor(async () => peekB().length > 0, 60_000, "B 侧出现这条消息");
  // 等条件而不是等时间：Ack 回来才继续（否则慢机器上会把"还没到"读成"丢了"）
  await waitFor(() => outboxLeft() === 0, 30_000, "A 侧 outbox 被 Ack 删除");
  await sleep(1500); // 多留一点窗口，让「重复投递」这种退化有机会显现

  const rowsB = peekB();
  check("B 侧恰好一条（重复投递 = 用户看到两条）", rowsB.length === 1, 1, rowsB.length);
  check("B 侧内容与应用解密结果一致",
    rowsB[0]?.content === "hello from harness", "hello from harness", rowsB[0]?.content);
  check("B 侧方向正确（sender 是 A）",
    rowsB[0]?.sender_id === idA.runtimeId, idA.runtimeId, rowsB[0]?.sender_id);

  const left = aDb.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id=?1").get(msgId).c;
  check("A 侧 outbox 被 Ack 清空", left === 0, 0, left);
  const st = aDb.prepare("SELECT status FROM messages WHERE msg_id=?1").get(msgId)?.status;
  check("A 侧状态前进过 sending", !["sending", "failed"].includes(st), "sent/delivered/read", st);
  const uniq = bDb.prepare(
    "SELECT COUNT(*) c FROM messages WHERE msg_id=?1",
  ).get(msgId).c;
  check("msg_id 在 B 侧唯一（INV-P01）", uniq === 1, 1, uniq);
  aDb.close(); bDb.close();
});

step("J2 文件 A→B：只有 rename 之后才算完成，且字节与 hash 一致", async () => {
  const recvB = path.join(RUN_DIR, "recv", "B");
  const landed = path.join(recvB, `${xferId}.bin`);
  await waitFor(() => fs.existsSync(landed), 120_000, `B 的接收目录出现 ${xferId}.bin（只认 rename 后的最终名）`);
  const bytes = fs.readFileSync(landed);
  check("B 侧字节数与发送端一致", bytes.length === FILE_BYTES, FILE_BYTES, bytes.length);
  const got = createHash("sha256").update(bytes).digest("hex");
  check("B 侧 sha256 与源文件一致（INV-P17 分片可验证）", got === srcSha, srcSha.slice(0, 12) + "…", got.slice(0, 12) + "…");

  // ⚠️ 这里以前是"看到 B 的终名文件就立刻读 A"⇒ 判据读在 ack 之前（见 waitSendTerminal 的注释）。
  //   默认轮一直绿只是因为 waitFor 的 500ms 轮询恰好盖住了 ack 的往返时间，不是这条链没有窗口。
  const sentJ2 = await waitSendTerminal(xferId);
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
  const bRow = bDb.prepare("SELECT status,path FROM file_transfers WHERE id=?1").get(xferId);
  const bDup = bDb.prepare("SELECT COUNT(*) c FROM file_transfers WHERE id=?1").get(xferId).c;
  bDb.close();
  check("B 侧 receive 记录终态是 done（不是 active/failed）", bRow && bRow.status === "done", "done", bRow ? bRow.status : "无行");
  check("B 侧同一条传输只记一次", bDup === 1, 1, bDup);
  const strays = fs.existsSync(recvB)
    ? fs.readdirSync(recvB).filter((f) => f.includes(xferId) && f !== `${xferId}.bin`)
    : [];
  check("接收目录没有 .part / 改名副本残留", strays.length === 0, 0, strays.join(", ") || 0);
});

// §十四要的「错误行为测试」+ §七/§八的「文件 hash 不一致 / .part 已存在」：
// 坏内容必须要么被拒收、要么被补齐成正确字节 —— 但绝不允许"报成功却没有正确文件"。

if (TASK) {
  step("任务判据：B 收到创建/完成/归档/重开四条、明文解得开、指派就是我、seq 决定 LWW 终态", async () => {
    const flip = (h) => h.slice(0, -1) + (h.endsWith("0") ? "1" : "0");
    const want = (id) => (TASK_LIE ? flip(id) : id);
    const convId = `group:${GROUP_ID}`;
    await waitFor(() => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return [taskCreateId, taskUpdateId, taskArchId, taskReopenId]
          .every((id) => db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(id).c > 0);
      } finally { db.close(); }
    }, 60_000, "B 侧四条任务载荷到齐（建链后 flush_group_outbox 送达）");

    const bDb = openDb(INSTANCES[1].db, true);
    const q = (id) => bDb.prepare(
      "SELECT m.conv_id,m.sender_id,m.kind,m.content,m.seq,m.status FROM messages m WHERE m.msg_id=?1",
    ).all(id);
    const rowC = q(want(taskCreateId));
    const rowU = q(want(taskUpdateId));
    const rowA = q(want(taskArchId));
    const rowR = q(want(taskReopenId));
    const leak1to1 = bDb.prepare(
      "SELECT COUNT(*) c FROM messages WHERE conv_id!=?1 AND msg_id IN (?2,?3,?4,?5)",
    ).get(convId, taskCreateId, taskUpdateId, taskArchId, taskReopenId).c;
    bDb.close();
    const aDb = openDb(INSTANCES[0].db, true);
    const queued = aDb.prepare(
      "SELECT COUNT(*) c FROM group_outbox WHERE msg_id IN (?1,?2,?3,?4)",
    ).get(taskCreateId, taskUpdateId, taskArchId, taskReopenId).c;
    const aKinds = aDb.prepare(
      "SELECT kind FROM messages WHERE msg_id IN (?1,?2) ORDER BY seq",
    ).all(taskCreateId, taskUpdateId).map((r) => r.kind);
    aDb.close();

    const parse = (rows) => {
      if (rows.length !== 1) return null;
      try { return JSON.parse(rows[0].content); } catch { return null; }
    };
    const pc = parse(rowC);
    const pu = parse(rowU);
    const pa = parse(rowA);
    const pr = parse(rowR);
    const uniq = new Set([taskCreateId, taskUpdateId, taskArchId, taskReopenId]).size;

    // 这条读的是**发送侧**（A 的库），lie 模式翻的是判据读的 id ⇒ 它在正向与反向两跑里都该绿：
    // 它是这一轮的"预置/投递没坏"控制项，不是被钉的那件事本身。
    check("A 侧两条载荷按 seq 排的 kind 依次是 todo / todo_update（发送侧预置控制项）",
      aKinds.join(",") === "todo,todo_update", "todo,todo_update", aKinds.join(","));
    check("B 侧四条各恰好一行（多行=重复投递，0 行=没送达）",
      [rowC, rowU, rowA, rowR].every((r) => r.length === 1), "1/1/1/1",
      [rowC, rowU, rowA, rowR].map((r) => r.length).join("/"));
    check("B 侧落库的必须是群会话行（不许串进 1:1）",
      rowC.length === 1 && rowC[0].conv_id === convId && leak1to1 === 0,
      `${convId} 且 1:1 里 0 条`, `${rowC[0]?.conv_id} / leak=${leak1to1}`);
    check("载荷必须是**解密后的明文 JSON**（拿到密文或空串都说明没真解密）",
      !!pc && !!pu, "两条都能 JSON.parse", `c=${pc === null ? "解析失败" : "ok"} u=${pu === null ? "解析失败" : "ok"}`);
    check("两条指的是**同一个任务**（todo_id 相同，不是两条无关消息）",
      !!pc && !!pu && pc.todo_id === pu.todo_id && pc.todo_id === "todo-e2e-1",
      "todo-e2e-1", `${pc?.todo_id} / ${pu?.todo_id}`);
    check("创建那条的指派里必须有 B 的 device_id（「与我相关」的输入就是这个）",
      Array.isArray(pc?.assignees) && pc.assignees.includes(idB.runtimeId),
      `含 ${idB.runtimeId}`, JSON.stringify(pc?.assignees));
    check("创建那条的 status 是 todo、完成那条是 done 且带 done_at",
      pc?.status === "todo" && pu?.status === "done" && !!pu?.done_at,
      "todo → done(+done_at)", `${pc?.status} → ${pu?.status} done_at=${pu?.done_at}`);
    check("seq 必须是创建 1 / 完成 2（LWW 靠 seq 定序，乱了折叠出的终态就不对）",
      rowC[0]?.seq === 1 && rowU[0]?.seq === 2, "1 / 2",
      `${rowC[0]?.seq} / ${rowU[0]?.seq}`);
    check("creator 由载荷带着，且必须是 A（改/删授权判据靠它）",
      pc?.creator === idA.runtimeId && pu?.creator === idA.runtimeId,
      idA.runtimeId, `${pc?.creator} / ${pu?.creator}`);
    check("发送方在 B 侧记为 A 的 runtimeId（不许被改写成接收者自己）",
      rowC[0]?.sender_id === idA.runtimeId, idA.runtimeId, rowC[0]?.sender_id);
    check("A 侧这两条的 group_outbox 必须被 GroupAck 清干净（队列残留=还会重发）",
      queued === 0, 0, queued);
    check("四条载荷的 msg_id 互不相同且各唯一（INV-P01 幂等的前提）",
      uniq === 4, 4, uniq);
    // §7 要的完整迁移：创建 → 完成 →（完成态才允许）归档 → 重开。
    // 徽标那条数在真实应用里就是靠这一串状态行的**先后**算出来的（done/archived 不算，
    // 重开回 doing 又要算回来），所以这里判的是四行的状态字段与 seq 顺序，不是像素。
    check("归档那条必须 archived=true 且 status=done（命令层 resolve_done_archive 只允许这个组合）",
      pa?.archived === true && pa?.status === "done", "done + archived=true",
      `${pa?.status} + archived=${pa?.archived}`);
    check("重开那条回到 doing 且清掉 archived/done_at（「与我相关」的数要从 0 又变回 1）",
      pr?.status === "doing" && pr?.archived === false && !pr?.done_at,
      "doing + archived=false + done_at=null",
      `${pr?.status} + archived=${pr?.archived} + done_at=${JSON.stringify(pr?.done_at)}`);
    check("四条的 seq 必须严格 1/2/3/4（到货顺序不等于因果顺序，LWW 全靠 seq）",
      [rowC[0]?.seq, rowU[0]?.seq, rowA[0]?.seq, rowR[0]?.seq].join(",") === "1,2,3,4",
      "1,2,3,4",
      [rowC[0]?.seq, rowU[0]?.seq, rowA[0]?.seq, rowR[0]?.seq].join(","));

    // ── 对端发起的那一半（B 建 → B 完成 → A 同步）──
    // 预置时 A/B 都停着，靠建链后的 flush_group_outbox 送达；这条 waitFor 用**真 id**，
    // 与上面同形：lie 只翻判据读的那份，不翻"等不等得到"，否则红会落在超时上、看不出是判据坏。
    await waitFor(() => {
      const db = openDb(INSTANCES[0].db, true);
      try {
        return [taskBCreateId, taskBDoneId]
          .every((id) => db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(id).c > 0);
      } finally { db.close(); }
    }, 60_000, "A 侧收到 B 发起的创建与完成两条（发起方换向的这一半以前没有判据）");

    const aDb2 = openDb(INSTANCES[0].db, true);
    const q2 = (id) => aDb2.prepare(
      "SELECT conv_id,sender_id,kind,content,seq FROM messages WHERE msg_id=?1",
    ).all(id);
    const bRowC = q2(want(taskBCreateId));
    const bRowU = q2(want(taskBDoneId));
    const bLeak = aDb2.prepare(
      "SELECT COUNT(*) c FROM messages WHERE conv_id!=?1 AND msg_id IN (?2,?3)",
    ).get(convId, taskBCreateId, taskBDoneId).c;
    aDb2.close();
    const bDb2 = openDb(INSTANCES[1].db, true);
    const bQueued = bDb2.prepare(
      "SELECT COUNT(*) c FROM group_outbox WHERE msg_id IN (?1,?2)",
    ).get(taskBCreateId, taskBDoneId).c;
    bDb2.close();
    const pbc = parse(bRowC);
    const pbu = parse(bRowU);

    check("B 发起的两条在 A 侧各恰好一行、且落在群会话里（换向这一腿真投递了）",
      bRowC.length === 1 && bRowU.length === 1 && bRowC[0]?.conv_id === convId && bLeak === 0,
      "1/1 行且 1:1 里 0 条",
      `${bRowC.length}/${bRowU.length} conv=${bRowC[0]?.conv_id} leak=${bLeak}`);
    check("A 侧解出来的明文指向 B 建的那条任务，creator 就是发送者 B（授权读的是这个字段）",
      !!pbc && !!pbu && pbc.todo_id === "todo-e2e-2" && pbu.todo_id === "todo-e2e-2"
      && pbc.creator === idB.runtimeId && pbu.creator === idB.runtimeId,
      `todo-e2e-2 + creator=${idB.runtimeId}`,
      `${pbc?.todo_id}/${pbu?.todo_id} creator=${pbc?.creator}/${pbu?.creator}`);
    check("创建那条指派的是 A、seq 5→6 且 todo→done 带 done_at（对端视角的「与我相关」输入）",
      Array.isArray(pbc?.assignees) && pbc.assignees.includes(idA.runtimeId)
      && pbc?.status === "todo" && pbu?.status === "done" && !!pbu?.done_at
      && bRowC[0]?.seq === 5 && bRowU[0]?.seq === 6,
      "assignees 含 A + 5/6 + todo→done(+done_at)",
      `${JSON.stringify(pbc?.assignees)} seq=${bRowC[0]?.seq}/${bRowU[0]?.seq} ${pbc?.status}→${pbu?.status} done_at=${pbu?.done_at}`);
    check("发送方在 A 侧记为 B 的 runtimeId（对端发起的不得被写成接收者自己）",
      bRowC[0]?.sender_id === idB.runtimeId, idB.runtimeId, bRowC[0]?.sender_id);
    check("B 侧这一单的 group_outbox 也被 Ack 清干净（发起方的队列残留=下次建链还会重发）",
      bQueued === 0, 0, bQueued);

    // §27「任务 + 图片」这一格：任务描述里带的图片**只有元数据过线**（真实字节走群文件管线，
    // SQLite 不存 BLOB —— 这条原则必须继续保持）。五个字段每一个掉了都是静默的：
    // `subtype` 错 ⇒ 卡片把图片渲染成文件块；`sha256`/`id` 错 ⇒ 接收方在本地目录里找不到字节；
    // `size` 错 ⇒ 进度与完整性对不上。所以逐字段比，不按字符串比（序列化顺序不是契约）。
    const imgSame = (a, b) => !!a && !!b && a.id === b.id && a.name === b.name
      && a.size === b.size && a.sha256 === b.sha256 && a.subtype === b.subtype;
    const imgsOk = (p) => {
      const got = p?.images;
      return Array.isArray(got) && got.length === taskImages.length
        && taskImages.every((im, k) => imgSame(im, got[k]));
    };
    check("任务带的两张图片引用跨进程**逐字段**原样到齐、顺序不变（创建那条与改成完成那条都要带住）",
      imgsOk(pbc) && imgsOk(pbu), `${taskImages.length} 条 × 5 字段全等`,
      `c=${JSON.stringify(pbc?.images)} u.ok=${imgsOk(pbu)}`);
    // 这一条钉的是产品侧写在注释里的一句等式："id 与 sha256 同值，接收方按它在本地解析"。
    // 它一旦分叉，接收方就永远取不到那张图的字节 —— 而线上看着完全正常。
    // ★ `every` 对**空数组恒为真** ⇒ 必须先钉"有一条以上"，否则对端一条都没到时这条照样绿
    //   （实测：lie 模式下只加 `.every` 那半边是 18 红，补上前半句才是 19 红）。
    const idsMatch = (p) => Array.isArray(p?.images) && p.images.length > 0
      && p.images.every((im) => im.id === im.sha256 && im.sha256.length === 64);
    check("每条图片引用的 id 必须等于 sha256（=64 位 hex）—— 接收方就是拿它在本地目录里找字节的",
      idsMatch(pbc) && idsMatch(pbu), "id==sha256 且 64 位",
      JSON.stringify((pbc?.images ?? []).map((im) => `${im.id === im.sha256}/${im.sha256?.length}`)));
  });

  // ── §28「正在任务同步时退出」这一格：群任务这一族的崩溃恢复（以前只有文件族有判据）──
  // 钉的核心**不是**"能不能续传"（②③⑩ 已证），而是**一次崩溃不许把「已入队」这个事实抹掉**：
  // `group_outbox` 行是"先入队再投递"这条不变量的载体。它若因发送端崩溃变成 0 行，
  // 这两条任务就永久没人再发，而 A 的聊天气泡还在（§30 说的"最难发现的一种丢法"）。
  // ★ 时序由判据自己造：停机入队 → **只起 A**（对端不存在 ⇒ 一条也送不出去）→ SIGKILL A →
  //   再起 A+B（建链事件带动 flush，这是 A-9 那条缺口的机制面，这里当机制用，不等于认可那个缺口）。
  step("任务崩溃判据：群任务还没送达时发送端被 SIGKILL ⇒ 队列行与气泡都不许消失，重启后必须自己送到", async () => {
    await stopAll();
    const ts = nowMs();
    const convId = `group:${GROUP_ID}`;
    const members = [idA.runtimeId, idB.runtimeId];
    const todoId = "todo-e2e-3";
    const mk = (over) => JSON.stringify({
      todo_id: todoId, title: "e2e task crash", assignees: [idB.runtimeId], status: "todo",
      creator: idA.runtimeId, deleted: false, description: "", images: [], archived: false,
      done_at: null, ...over,
    });
    const base = {
      groupKey: GROUP_KEY_B64, senderId: idA.runtimeId, priv: ed25519Priv(INSTANCES[0]),
      x25519Pub: idA.x25519Pub, ed25519Pub: idA.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: idA.runtimeId, members,
    };
    const c1 = buildGroupEnvelope({ ...base, kind: "todo", content: mk({}), ts, seq: 7 });
    const c2 = buildGroupEnvelope({
      ...base, kind: "todo_update", content: mk({ status: "done", done_at: ts + 3 }), ts: ts + 1, seq: 8,
    });
    taskCrashCreateId = c1.messageId;
    taskCrashDoneId = c2.messageId;
    seed(INSTANCES[0].db, (db) => {
      for (const [env, seq, kind, content, at] of [
        [c1, 7, "todo", mk({}), ts],
        [c2, 8, "todo_update", mk({ status: "done", done_at: ts + 3 }), ts + 1],
      ]) {
        db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
        db.prepare(
          `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
           VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'sent')`,
        ).run(env.messageId, convId, idA.runtimeId, GROUP_ID, kind, content, at, seq);
        db.prepare(
          `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
           VALUES(?1,?2,?3,?4,?5)`,
        ).run(env.messageId, GROUP_ID, idB.runtimeId, env.wire, at);
      }
      db.prepare(
        "INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq",
      ).run(convId, 8);
    });

    // 只起 A：这一格的坏状态是"对端整个不存在"，所以这两条**必须**只能待在队列里。
    launch(INSTANCES[0]);
    await waitFor(() => tcpOpen(INSTANCES[0].port), 60_000, `崩溃判据：A 的 TCP ${INSTANCES[0].port} 可连`);
    await waitFor(() => bootReady(INSTANCES[0].log, bootBaseOf.get(INSTANCES[0].n), BOOT_LINE), 30_000,
      "崩溃判据：A 打出 boot 完成行");
    await sleep(15_000); // 一段"对端完全缺席"的时间

    const readA = () => {
      const db = openDb(INSTANCES[0].db, true);
      try {
        return {
          queued: db.prepare("SELECT COUNT(*) c FROM group_outbox WHERE msg_id IN (?1,?2)")
            .get(taskCrashCreateId, taskCrashDoneId).c,
          bubbles: db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id IN (?1,?2)")
            .get(taskCrashCreateId, taskCrashDoneId).c,
        };
      } finally { db.close(); }
    };
    const readB = () => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id IN (?1,?2)")
          .get(taskCrashCreateId, taskCrashDoneId).c;
      } finally { db.close(); }
    };
    // 缺席那一条读的是**真 id**（翻过的 id 必然查不到 ⇒ 那条断言就成了永远为真的空转）。
    // 它的正向对照是同一步里后面那条"重启后 B 侧真收到" —— 缺席判据与到齐判据配对，才不是"没跑起来"。
    check("对端缺席 15s 期间 B 侧不许有这两条的任何一行（还没送达就是没送达）",
      readB() === 0, 0, readB());
    const q1 = readA();
    check("对端缺席期间 A 侧两条队列行都还在（「先入队再投递」的载体不许自己消失）",
      q1.queued === 2, 2, q1.queued);

    const pA = procs.get(INSTANCES[0].n);
    // ⚠️ 被信号杀死的子进程 `exitCode === null`、只有 `signalCode` 有值（③⑩ 踩过同一个坑）。
    const deadA = () => pA.exitCode !== null || pA.signalCode !== null;
    pA.kill("SIGKILL");
    await waitFor(deadA, 15_000, "崩溃判据：A 确认已死（SIGKILL 不给它收尾的机会）");
    await sleep(10_000); // 一段"发送端根本不存在"的时间

    const q2 = readA();
    check("发送端崩溃后队列行必须还是 2 行 —— 崩溃不许把它当成已送达、更不许抹掉「已入队」",
      q2.queued === 2, 2, q2.queued);
    check("发送端崩溃后 A 侧那两条气泡必须还在 —— 队列没了是永久没人再发，气泡没了是用户连「发过」都看不见",
      q2.bubbles === 2, 2, q2.bubbles);

    launch(INSTANCES[0]);
    launch(INSTANCES[1]);
    for (const i of INSTANCES) {
      await waitFor(() => tcpOpen(i.port), 60_000, `重启后实例 ${i.label} 的 TCP ${i.port} 可连`);
      await waitFor(() => bootReady(i.log, bootBaseOf.get(i.n), BOOT_LINE), 30_000,
        `重启后实例 ${i.label} 打出 boot 完成行`);
    }
    // 同步点选在**被断言那一侧自己到齐**：B 落库靠 A 建链后的 flush，A 的队列清空靠 B 的 GroupAck
    // （⑨ 的教训：拿另一侧的产物当这一侧的同步点会天然晚一步）。
    const flip = (h) => h.slice(0, -1) + (h.endsWith("0") ? "1" : "0");
    const want = (id) => (TASK_LIE ? flip(id) : id);
    await waitFor(() => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return [taskCrashCreateId, taskCrashDoneId]
          .every((id) => db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(id).c > 0);
      } finally { db.close(); }
    }, 90_000, "重启后这两条必须自己送到 B（建链带动 flush）");

    const bDb = openDb(INSTANCES[1].db, true);
    const rowsCrash = [want(taskCrashCreateId), want(taskCrashDoneId)].map((id) => bDb.prepare(
      "SELECT conv_id,sender_id,kind,content,seq FROM messages WHERE msg_id=?1",
    ).all(id));
    bDb.close();
    const safeParse = (rows) => {
      if (rows.length !== 1) return null;
      try { return JSON.parse(rows[0].content); } catch { return null; }
    };
    const pc = safeParse(rowsCrash[0]);
    const pd = safeParse(rowsCrash[1]);
    check("重启补送到的这两条在 B 侧各恰好一行、明文解得开、seq 7→8 且 todo→done（重放不许送坏内容）",
      rowsCrash.every((r) => r.length === 1) && rowsCrash[0][0]?.conv_id === convId
      && rowsCrash[0][0]?.sender_id === idA.runtimeId
      && pc?.todo_id === todoId && pd?.todo_id === todoId
      && pc?.status === "todo" && pd?.status === "done" && !!pd?.done_at
      && rowsCrash[0][0]?.seq === 7 && rowsCrash[1][0]?.seq === 8,
      "各 1 行 + 明文 todo→done + seq 7/8 + sender=A",
      `${rowsCrash.map((r) => r.length).join("/")} ${pc?.status}→${pd?.status}`
      + ` seq=${rowsCrash[0][0]?.seq}/${rowsCrash[1][0]?.seq} sender=${rowsCrash[0][0]?.sender_id}`);

    // A 的队列清空只能**有界地等**它自己发生（GroupAck 在 B 落库之后才发）；
    // 到不了终态就把最后一次读数交给 check 判红 —— 不写"兜底断言"（那种断言到不了就是死代码）。
    let qEnd = -1;
    const tEnd = nowMs();
    for (;;) {
      qEnd = readA().queued;
      if (qEnd === 0 || nowMs() - tEnd >= 60_000) break;
      await sleep(500);
    }
    check("重启后这两条的队列行最终被 GroupAck 清成 0（残留=下次建链还会重发一遍）",
      qEnd === 0, 0, qEnd);
  });
}

if (GROUP) {
  step("群聊判据：每条各只落一行、明文要真解得开、@ 的那串字节不许被改写、撤回只物化不删行、Ack 必须把队列清干净", async () => {
    // 反向模式在这里翻的**只有判据读的 id**（预置、信封、投递全都一模一样）：
    // 真投递已经完成，却拿必定不存在的 id 去比 ⇒ 红只能来自断言本身，不来自基础设施噪声。
    const flip = (h) => h.slice(0, -1) + (h.endsWith("0") ? "1" : "0");
    const want = (id) => (GROUP_LIE ? flip(id) : id);
    const convId = `group:${GROUP_ID}`;
    // 前提断言（等四条都到齐）：没有它，下面几条会因为"链路根本没跑"而集体假绿
    await waitFor(() => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return [gTextId, gRecallId, gText2Id, gMentionId, gMentionOnlyId, gLegacyShapeId]
          .every((id) => db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(id).c > 0);
      } finally { db.close(); }
    }, 60_000, "B 侧六条群消息到齐（建链后 flush_group_outbox 送达）");
    // 撤回物化与投递是两次独立写盘，给它一个**有界**的等待（不许无条件睡）
    await waitFor(() => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return db.prepare("SELECT kind FROM messages WHERE msg_id=?1").get(gTextId)?.kind
          === "recalled";
      } finally { db.close(); }
    }, 20_000, "B 侧那条正文被撤回物化成 recalled");

    const bDb = openDb(INSTANCES[1].db, true);
    const q = (id) => bDb.prepare(
      "SELECT m.conv_id,m.sender_id,m.kind,m.content,m.seq,m.status,c.kind conv_kind"
      + " FROM messages m LEFT JOIN conversations c ON c.id=m.conv_id WHERE m.msg_id=?1",
    ).all(id);
    const t1 = q(want(gTextId));
    const rec = q(want(gRecallId));
    const t2 = q(want(gText2Id));
    const mt = q(want(gMentionId));
    const mo = q(want(gMentionOnlyId));
    const lg = q(want(gLegacyShapeId));
    // 这一轮自己排的那六条（id 列表是唯一一处，条数由它推出来）：SQL 的占位符必须跟着长，
    // 写死 `IN (?2,?3,?4,?5)` 的话，加两条会让 node:sqlite 直接抛参数个数不符 —— 那是技术性红，
    // 不是判据红，读日志的人只会看见一个没头没尾的异常。
    const seededIds = [gTextId, gRecallId, gText2Id, gMentionId, gMentionOnlyId, gLegacyShapeId];
    const inList = (from) => seededIds.map((_, i) => `?${i + from}`).join(",");
    const leakedIntoOneToOne = bDb.prepare(
      `SELECT COUNT(*) c FROM messages WHERE conv_id!=?1 AND msg_id IN (${inList(2)})`,
    ).get(convId, ...seededIds).c;
    bDb.close();
    const aDb = openDb(INSTANCES[0].db, true);
    const stillQueued = aDb
      .prepare("SELECT COUNT(*) c FROM group_outbox WHERE group_id=?1").get(GROUP_ID).c;
    // 读的是**这一轮自己排的那四条**（按 id 取），不是"那个会话里现有多少行" ——
    // 后者会让"再加一条预置"变成改判据的分母，加一行就把这条判成红（实测：加完 @ 那条就红了，
    // 红的是判据不是产品）。id 列表在作用域里，条数由它推出来，不再写死。
    const aSeededIds = seededIds;
    const aStatus = aDb.prepare(
      `SELECT msg_id,status FROM messages WHERE msg_id IN (${inList(1)})`,
    ).all(...aSeededIds).map((r) => r.status);
    const aT1 = aDb.prepare("SELECT kind FROM messages WHERE msg_id=?1").get(gTextId);
    aDb.close();

    check("B 侧六条群消息各恰好一行（同 msg_id 多行=重复投递，少行=丢）",
      [t1, rec, t2, mt, mo, lg].every((rows) => rows.length === 1),
      "六条各 1 行", [t1, rec, t2, mt, mo, lg].map((r) => r.length).join("/"));
    // ★ §8 的存储侧不变量：**@ 的那串字节不许被任何一层改写**。
    // "被改写"有两种正好相反的死法 —— 替换成呈现层产物「@你」（把我的视角烧进公共数据），
    // 或"规范化"成 device id（把可读的那份弄没）。所以一条判"逐字等于打出去的原文"，
    // 一条判"这两个方向都没有出现"。前者是正向对照，缺了后者就不知道被换成了什么；
    // 反过来缺了前者，后者会因为"根本没投递"而假绿（同一形状见上面那条前提断言的注释）。
    check("A 打出的那句 @ 在 B 库里必须逐字等于原文（存储只保存真实昵称，「@你」是渲染时才换的）",
      mt.length === 1 && mt[0].content === gMentionText, gMentionText, JSON.stringify(mt[0]?.content));
    check("存储里既不许出现「@你」，也不许把昵称规范化成 device id（两个相反的破坏方向各钉一次）",
      mt.length === 1 && !mt[0].content.includes("@你") && !mt[0].content.includes(idB.runtimeId)
      && !mt[0].content.includes(idA.runtimeId),
      "只含真实昵称那一串", JSON.stringify(mt[0]?.content));
    // #103 的线级那一半：`mentions` 到底能不能穿过"seal → 网络 → 解密 → 解析"到达对端进程。
    // 读的是 **B 自己的日志**（不是 harness 写进去的东西）⇒ 判的是对端自己解出来的结果，
    // 不是"我发了"。两条成对：前一条红在"名单某一跳丢了"，后一条挡住
    // "把缺键也当成空名单"那种实现（那样旧对端发的 @ 会永久判不出来，比原缺陷更糟）。
    // ⚠️ 这一格判不到那枚红点：徽标活在 webview 的 store 里（不落库、也不进日志）
    //    ⇒ "界面上真的亮了"仍归 Smoke-10/11 人工（#117 记的就是这剩下的一半）。
    const mentionLineCount = countLog(
      INSTANCES[1].log, `群消息@输入 msg=${gMentionOnlyId} mentions=1`);
    const legacyLineCount = countLog(
      INSTANCES[1].log, `群消息@输入 msg=${gLegacyShapeId} mentions=none`);
    check("带 @ 名单那条：B 自己解密后读到 1 个身份号（名单穿过 seal→网络→解析没丢）",
      mentionLineCount >= 1, "≥1 行 mentions=1", mentionLineCount);
    check("没带这个键那条（旧形状）：B 判成「不知道」而不是「谁都没 @」——第三态跨进程成立",
      legacyLineCount >= 1, "≥1 行 mentions=none", legacyLineCount);
    check("落库的会话必须是群会话（conv_id 带 group: 前缀，且 conversations.kind='group'）",
      t2.length === 1 && t2[0].conv_id === convId && t2[0].conv_kind === "group",
      `${convId} / group`, `${t2[0]?.conv_id} / ${t2[0]?.conv_kind}`);
    check("未被撤回那条的正文必须是**解密后的明文**（拿到密文或空串都说明没真解密）",
      t2.length === 1 && t2[0].content === "second group message",
      "second group message", JSON.stringify(t2[0]?.content));
    check("发送方必须是 A 的 runtimeId、seq 必须照信封给（seq 是排序权威）",
      t2.length === 1 && t2[0].sender_id === idA.runtimeId && t2[0].seq === 3,
      `${idA.runtimeId} / seq=3`, `${t2[0]?.sender_id} / seq=${t2[0]?.seq}`);
    check("B 侧终态必须是 delivered（收到即记，不等 Ack 回传）",
      t2.length === 1 && t2[0].status === "delivered", "delivered", t2[0]?.status);
    check("撤回必须把目标**物化**成 kind=recalled + 空正文，而且**不许删行**（G-Set 语义）",
      t1.length === 1 && t1[0].kind === "recalled" && t1[0].content === "",
      "行还在 + kind=recalled + content=''",
      `${t1.length} 行 / ${t1[0]?.kind} / content=${JSON.stringify(t1[0]?.content)}`);
    check("撤回事件自身也必须留一行（历史只增不减，不许被当成一次性通知丢掉）",
      rec.length === 1, 1, rec.length);
    check("这几条群消息都不许串进 1:1 会话（两条管道共用同一对实例时的串味检查）",
      leakedIntoOneToOne === 0, 0, leakedIntoOneToOne);
    check("A 侧这个群的 group_outbox 必须被 GroupAck 清空（还留着=只发不认，重启会二次投递）",
      stillQueued === 0, 0, stillQueued);
    check("A 侧这一轮排的每条群气泡都不许被判成 failed（failed 的唯一裁决不能被群路径绕过）",
      aStatus.length === aSeededIds.length && !aStatus.includes("failed"),
      `${aSeededIds.length} 行都有状态且无 failed`, JSON.stringify(aStatus));
    check("A 侧那条正文此刻仍是 text —— ⚠️ **实测出来的边界**：harness 写的是入队形状，"
      + "没有执行产品的撤回命令 ⇒ 发送侧本地物化不在这一格的证明范围里",
      aT1?.kind === "text", "text（本格的已知边界）", aT1?.kind);
  });
}

if (POISON) {
  step("故障注入判据：脏 .part 前缀不许污染结局（拒收 或 补齐，二选一，不许交叉）", async () => {
    const recvB = path.join(RUN_DIR, "recv", "B");
    const landed2 = path.join(recvB, `${xferId2}.bin`);
    // 前提断言：B 真的动过这一单。没有它，下面几条会因为"链路根本没跑"而集体假绿 ——
    // 那正是最像成功的一种失败。
    let how = "";
    await waitFor(() => {
      const b = openDb(INSTANCES[1].db, true);
      const r = b.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId2);
      b.close();
      if (r) { how = `B 侧有行 status=${r.status}`; return true; }
      if ((tailLog(INSTANCES[1].log, 20000) || "").includes(xferId2)) { how = "B 日志提到过这个 transfer_id"; return true; }
      return false;
    }, 90_000, `B 侧出现对 ${xferId2} 的处理痕迹（先证明这一单真被投递过，再谈拒收）`);
    console.log(`      观察：${how}`);
    await new Promise((r) => setTimeout(r, 10_000)); // 让 hash 校验 / rename / 重试落定

    const exists = fs.existsSync(landed2);
    const gotSha = exists
      ? createHash("sha256").update(fs.readFileSync(landed2)).digest("hex")
      : null;
    // 脏前缀被丢掉、整份重新收齐并改名 ⇒ 这是"自愈完成"，此时 done 才是**正确**终态。
    const wantSha2 = LIE ? LIE_SHA : srcSha2;
    const clean = !!exists && gotSha === wantSha2;
    // ⚠️ 这一轮的"两侧 done"是**蕴含式**的后件 ⇒ 只在补齐分支才需要等 A 到终态
    //   （不补齐那一支本来就允许停在 pending/重试中，等满 60s 只会白烧门禁时间）。
    //   lie 模式下 clean 必为假 ⇒ 走的还是今天这条不等待的路，反向自证那 2 条红不受影响。
    if (clean) await waitSendTerminal(xferId2);

    const bDb = openDb(INSTANCES[1].db, true);
    const b2 = bDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId2);
    bDb.close();
    const aDb = openDb(INSTANCES[0].db, true);
    const a2 = aDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId2);
    const queued = aDb.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(xferId2).c;
    aDb.close();
    const strays2 = fs.existsSync(recvB)
      ? fs.readdirSync(recvB).filter((f) => f.includes(xferId2) && f !== `${xferId2}.bin`)
      : [];
    const both = `B=${b2?.status ?? "无行"} A=${a2?.status ?? "无行"} outbox=${queued}`;

    // 四条合起来 = "结局只允许两种，且不许交叉"：
    //   A) 补齐了 ⇒ 终名 sha256 == 源 + 两侧 done + outbox 已清（自愈完成）
    //   B) 没补齐 ⇒ 没有正确终名 + 两侧都不许 done（明确没成功、还能重试）
    // 交叉态才是真 bug：done 却没有正确文件 = 假成功；文件已正确落地却仍 failed = 恢复失败、界面永久转圈。
    // ⚠️ 上一版这里写的是"必须非 done"——被实跑证伪了：它写的是"我以为失败长什么样"，不是产品契约。
    check("坏内容不许冒充成功：终名要么不出现，出现则 sha256 必须等于源文件",
      !exists || clean,
      "不出现 或 " + wantSha2.slice(0, 12) + "…",
      exists ? gotSha.slice(0, 12) + "…" : "未出现");
    check("若脏前缀最终被补齐（字节正确）：两侧必须 done 且 outbox 已清 —— 不许停在中间态",
      !clean || (b2?.status === "done" && a2?.status === "done" && queued === 0),
      clean ? "双侧 done + outbox=0" : "不适用（未落地）", both);
    check("若终名未落地或字节不对：两侧都不许 done —— 绝不许对坏内容宣布完成",
      clean || (b2?.status !== "done" && a2?.status !== "done"),
      clean ? "不适用（本轮走补齐分支）" : "双侧非 done", both);
    check("污染过的前缀不许留在盘上：接收目录不得残留该 transfer 的 .part / 副本",
      strays2.length === 0, 0, strays2.join(", ") || 0);
  });
}

if (RESUME) {
  step("故障注入判据②：真前缀必须被续传复用，拼出来的字节要等于源文件", async () => {
    const recvB = path.join(RUN_DIR, "recv", "B");
    const landed3 = path.join(recvB, `${xferId3}.bin`);
    const PRE = 64 * 1024;
    // lie 模式同时换掉"期望已收字节数"和"期望摘要"两个输入 ⇒ 前两条必须报红。
    const wantPre = LIE ? PRE / 2 : PRE;
    const wantSha3 = LIE ? LIE_SHA : srcSha3;
    await waitFor(() => (tailLog(INSTANCES[0].log, 60000) || "").includes(xferId3),
      90_000, `A 侧出现对 ${xferId3} 的处理痕迹（先证明这一单真被投递过，再谈续传）`);
    await sleep(12_000); // 让续传 / rename / 可能的重试都落定
    const aLog = tailLog(INSTANCES[0].log, 60000) || "";
    const line = aLog.split("\n").filter((l) => l.includes(xferId3) && l.includes("接收端已有")).pop() || "";
    const m = line.match(/接收端已有 (\d+) 字节/);
    check("必须按对端已收的 65536 字节续传，不许从 0 重灌整份（160MB 事故那一格）",
      !!m && Number(m[1]) === wantPre, String(wantPre), m ? m[1] : "A 日志里没有针对这一单的续发行");
    const exists3 = fs.existsSync(landed3);
    const got3 = exists3 ? createHash("sha256").update(fs.readFileSync(landed3)).digest("hex") : null;
    check("续传拼出来的文件 sha256 必须等于源文件（前缀 + 尾段字节级正确）",
      exists3 && got3 === wantSha3, wantSha3.slice(0, 12) + "…",
      exists3 ? got3.slice(0, 12) + "…" : "未落地");
    const bDb = openDb(INSTANCES[1].db, true);
    const b3 = bDb.prepare("SELECT status FROM file_transfers WHERE id=?1").all(xferId3);
    bDb.close();
    // ⚠️ 上面那句 sleep 只是让**日志**落定，不等于 A 的终态到了：下面这条是无条件断"两侧 done"，
    //   所以必须等 A 自己被 ack 点亮（见 waitSendTerminal），否则第 12 秒读早了照样假红。
    const t3 = await waitSendTerminal(xferId3);
    const a3 = t3.row;
    const q3 = t3.queued;
    const both3 = `B=${b3.map((r) => r.status).join("/") || "无行"} A=${a3?.status ?? "无行"} outbox=${q3}`;
    check("同一条续传在 B 侧只记一次（不许一次传输落多行）", b3.length === 1, 1, b3.length);
    check("续传完成就是完成：两侧终态 done 且 outbox 已清（不许停在中间态）",
      b3.length === 1 && b3[0].status === "done" && a3?.status === "done" && q3 === 0,
      "1 行 + 双侧 done + outbox=0", both3);
    const kept3 = fs.existsSync(recvB) ? fs.readdirSync(recvB).filter((f) => f.includes(xferId3)) : [];
    check("前缀用完即弃：接收目录只剩 1 个终名文件，无 .part 残留、无副本",
      kept3.length === 1 && kept3[0] === `${xferId3}.bin`, `${xferId3}.bin`, kept3.join(", ") || "空");
  });
}

if (KILL) {
  step("故障注入判据③：接收中被 SIGKILL ⇒ 不许假成功，重启后按盘上真实字节续完", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    const partPath = path.join(dl, `${xferId4}.part`);
    const landed = path.join(dl, `${xferId4}.bin`);
    const partSize = () => {
      try {
        return fs.statSync(partPath).size;
      } catch {
        return 0;
      }
    };
    // 这一单**由我在判据里才入队**：前两版都在停机时预置，于是传输发生在"起 A/B 等链路"
    // 那一步里，等判据去看时早传完了 —— 打空的两轮报红全是我的时序问题，不是产品的。
    // ⚠️ 进程活着时写它的库是新用法：seed() 末尾的 wal_checkpoint(TRUNCATE) 撞上在写的
    //    连接会 SQLITE_BUSY ⇒ 重试几次；真进不去就该换成"停机入队 + 大文件"那条路。
    for (let i = 0; ; i++) {
      try {
        seed(INSTANCES[0].db, (db) => {
          db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId4);
          db.prepare(
            `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
             VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
          ).run(xferId4, peerTo, srcFile4, `${xferId4}.bin`, KILL_BYTES, nowMs());
        });
        break;
      } catch (e) {
        if (i >= 5) throw e;
        await sleep(300);
      }
    }
    // waitFor 是 500ms 粒度，而实测一次 100 MB 传输只有 ~0.78s ⇒ 会打空。这里用 50ms 自旋。
    // 打没打中窗口是**这条注入自己**的成败，必须红给看，不许悄悄当成"已通过"。
    const inFlight = () => {
      const n = partSize();
      return n > 0 && n < KILL_BYTES;
    };
    let miss = "";
    const until = nowMs() + 120_000;
    while (nowMs() < until && !inFlight()) await sleep(50);
    if (!inFlight()) {
      miss =
        `120s 内没出现"在飞"的 .part（实际 ${partSize()} 字节）—— ` +
        `要么 A 没有在飞行中把这单捡起来，要么传得太快/太大没抓着`;
    }
    partAtKill = partSize();
    const pB = procs.get(INSTANCES[1].n);
    // ⚠️ 被信号杀死的子进程：`exitCode === null` + `signalCode === "SIGKILL"`。
    //    拿 exitCode !== null 判"死了没有"永远等不到（第一版就在这里超时）。
    const dead = () => !!pB && (pB.exitCode !== null || pB.signalCode !== null);
    if (!dead()) pB.kill("SIGKILL");
    await waitFor(dead, 15_000, "B 进程确认已死（SIGKILL 不给它收尾的机会）");
    await sleep(2_000); // 让 A 把"写失败了"变成状态
    const landedAtKill = fs.existsSync(landed);
    // ★ 参考量取在**确认已死之后**，不取在按下 SIGKILL 之前：`.part` 只有 B 自己会写，
    //   所以此刻起它永久冻结 —— 这才是"死的那一刻盘上有多少字节"。
    //   原先拿 kill 前的快照当期望值，本地层实测把它判红了：快照 4,194,304，而重启后的 B 自己读到
    //   4,456,448 并要求从这里续（差恰好一个 256 KiB 片）⇒ **产品服从的是盘上真值，红的是判据自己的读数窗口**
    //   （从快照到真死 B 还在收片，且写入也要一会儿才在 stat 上显现；两种成因指向同一个修法）。
    //   与 waitSendTerminal 同一族：先问"这个数是靠谁定格的"。下面那行打印就是这扇窗的探针。
    const partAtDeath = partSize();
    console.log(`  · 实测：按下 SIGKILL 前读到 ${partAtKill} 字节，确认已死后冻结在 ${partAtDeath} 字节`
      + `（差 ${partAtDeath - partAtKill}，非零就是快照打早了 —— 期望值以冻结那个为准）`);

    check("窗口必须真打中：杀的那一刻 .part 在 0~全量之间",
      !miss && partAtKill > 0 && partAtKill < KILL_BYTES,
      `0 < .part < ${KILL_BYTES}`, miss || `${partAtKill} 字节`);
    check("rename 才算完成：B 死在半路时接收目录不许出现终名文件",
      !landedAtKill, "不存在", landedAtKill ? "已出现" : "不存在");
    const aMid = openDb(INSTANCES[0].db, true);
    const a4mid = aMid.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId4);
    aMid.close();
    check("发送端不许在对端没确认时宣布完成：对端被杀的那一刻 A 不能是 done",
      a4mid?.status !== "done", "非 done", a4mid?.status ?? "无行");

    // 重启 B：链路该自己回来、outbox 该自己重投，且必须**接着盘上那点字节**发
    launch(INSTANCES[1]);
    await waitFor(() => tcpOpen(INSTANCES[1].port), 60_000, `重启后的 B 的 TCP ${INSTANCES[1].port} 可连`);
    await waitFor(() => bootReady(INSTANCES[1].log, bootBaseOf.get(INSTANCES[1].n), BOOT_LINE), 30_000,
      "重启后的 B 打出 boot 完成行");
    await waitFor(() => fs.existsSync(landed) || partSize() > partAtDeath, 120_000,
      "重启后这一单被重新拾起（.part 比死时更长，或终名文件出现）");
    await sleep(15_000); // 让续传 / rename / 多轮重试都落定

    // 期望值 = **确认已死后冻结的那个字节数**（不是按下 SIGKILL 之前的快照，见上面那段注释）。
    const wantFrom = LIE ? partAtDeath + 1 : partAtDeath;
    const aLog = tailLog(INSTANCES[0].log, 60000) || "";
    const line = aLog.split("\n").filter((l) => l.includes(xferId4) && l.includes("接收端已有")).pop() || "";
    const m = line.match(/接收端已有 (\d+) 字节/);
    check("重启后必须从**盘上真实字节数**续发，不许从 0 重灌（接收端真实进度优先）",
      !!m && Number(m[1]) === wantFrom, String(wantFrom),
      m ? m[1] : "A 日志里没有针对这一单的续发行");
    const exists4 = fs.existsSync(landed);
    const got4 = exists4 ? createHash("sha256").update(fs.readFileSync(landed)).digest("hex") : null;
    const wantSha4 = LIE ? LIE_SHA : srcSha4;
    check("死前写的前缀 + 重启后续发的尾段 = 源文件（sha256 逐字节对得上）",
      exists4 && got4 === wantSha4, wantSha4.slice(0, 12) + "…",
      exists4 ? got4.slice(0, 12) + "…" : "未落地");
    const bDb4 = openDb(INSTANCES[1].db, true);
    const b4 = bDb4.prepare("SELECT status FROM file_transfers WHERE id=?1").all(xferId4);
    bDb4.close();
    // ⚠️ 上面那句 `sleep(15_000)` 只是把 ack 竞态**藏住**，不是解决它（读早了照样红）。
    //   改成有界等 A 自己到终态：既不再靠运气，也不用白等 15s。
    const t4 = await waitSendTerminal(xferId4);
    const a4 = t4.row;
    const q4 = t4.queued;
    const both4 = `B=${b4.map((r) => r.status).join("/") || "无行"} A=${a4?.status ?? "无行"} outbox=${q4}`;
    check("恢复的终局只有一个：两侧 done 且 outbox 已清（不许停在中间态，也不许弃单）",
      b4.length === 1 && b4[0].status === "done" && a4?.status === "done" && q4 === 0,
      "1 行 + 双侧 done + outbox=0", both4);
    const kept4 = fs.existsSync(dl) ? fs.readdirSync(dl).filter((f) => f.includes(xferId4)) : [];
    check("续完之后接收目录只剩 1 个终名文件：无 .part 残留、无半截副本",
      kept4.length === 1 && kept4[0] === `${xferId4}.bin`, `${xferId4}.bin`, kept4.join(", ") || "空");
  });
}

if (FREEZE) {
  step("故障注入判据④：对端失联（进程被冻住）⇒ 失联期间不许假成功，对端回来必须自己补齐", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId5 = eid("f");
    srcFile5 = path.join(srcDir, `${xferId5}.bin`);
    fs.writeFileSync(srcFile5, Buffer.alloc(FREEZE_BYTES));
    srcSha5 = createHash("sha256").update(fs.readFileSync(srcFile5)).digest("hex");
    const landed = path.join(dl, `${xferId5}.bin`);
    const partPath = path.join(dl, `${xferId5}.part`);
    const sizeOf = (p) => { try { return fs.statSync(p).size; } catch { return -1; } };
    // lie 模式：注入一模一样，只把"终局该等于哪个摘要"换掉 ⇒ 摘要那条必须红。
    const wantSha5 = LIE ? LIE_SHA : srcSha5;
    const pB = procs.get(INSTANCES[1].n);
    if (!pB) throw new Error("拿不到 B 的子进程句柄 —— 这条注入没有可冻结的对象");
    let thawed = false;
    // ⚠️ 解冻必须放进 finally：**被 SIGSTOP 停住的进程收不到 SIGTERM 的处理**（信号挂起），
    //    收尾的 stopAll() 会永久等一个不会来的 exit ⇒ 整条 harness 挂死、留一个僵尸。
    pB.kill("SIGSTOP");
    let mid = null;
    try {
      // 入队在冻结之后：与 kill 轮同一个教训 —— 注入时机必须在判据自己手里。
      for (let i = 0; ; i++) {
        try {
          seed(INSTANCES[0].db, (db) => {
            db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId5);
            db.prepare(
              `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
               VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
            ).run(xferId5, peerTo, srcFile5, `${xferId5}.bin`, FREEZE_BYTES, nowMs());
          });
          break;
        } catch (e) {
          if (i >= 5) throw e;
          await sleep(300);
        }
      }
      await sleep(FREEZE_MS);
      const aMid = openDb(INSTANCES[0].db, true);
      const st = aMid.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId5);
      const q = aMid.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(xferId5).c;
      aMid.close();
      const midLanded = sizeOf(landed);
      const bMid = openDb(INSTANCES[1].db, true);
      const b5mid = bMid.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId5);
      bMid.close();
      mid = {
        status: st?.status ?? "无行", queued: q, b: b5mid?.status ?? "无行",
        part: sizeOf(partPath), landed: midLanded,
      };
      // ⚠️ 实测到的**产品现状**（30s 与 60s 各跑一遍，结论相同；写在这里是防止下一个 AI 把这一轮
      //   当成它看起来像在测的东西）：失联期间 A 侧 `attempts` 一次没涨、`file_transfers` 连行都没有
      //   ⇒ **A 根本没有尝试过**。根因：`flush_pending_files` 只被建链 / Hello / 心跳 / BLE 这类
      //   **入站事件**触发（transport.rs:2580/2902/3015、ble.rs:1102/2110），没有任何定时器去兑现
      //   `file_outbox.next_attempt_at` 与那个 5s backoff；对端"活着但一句不回"时不会有入站事件。
      //   ⇒ 这一轮证明的是：失联期间两侧都不许假成功 + 对端回来自己补齐。
      //   它**没有证明**"write 成功 ≠ 已送达"（那需要一个真在飞的写），也**没覆盖**"失联期间到点重投"。
      //   后者已按 A 类风险登记在 roadmap；修好之前不许把下面三条改名成"已覆盖重试"。
      check("失联期间接收目录不许出现终名文件（没收下就没有完成可言）",
        midLanded < 0, "不存在", midLanded >= 0 ? `已出现 ${midLanded} 字节` : "不存在");
      check("失联期间发送侧不许记成 done", mid.status !== "done", "非 done", mid.status);
      check("失联期间接收侧也不许记成 done（它一次回执都没发过）",
        mid.b !== "done", "非 done", mid.b);
      console.log(`  · 实测（失联 ${FREEZE_MS / 1000}s）：${JSON.stringify(mid)}`);
      for (const l of (tailLog(INSTANCES[0].log, 60000) || "").split("\n")
        .filter((x) => x.includes(xferId5)).slice(-8)) console.log("      A│ " + l.slice(0, 220));
      pB.kill("SIGCONT");
      thawed = true;
      const t0 = nowMs();
      await waitFor(() => sizeOf(landed) >= 0, 120_000, "解冻后 B 该把这一单收完并 rename 成终名");
      console.log(`  · 实测：解冻 → 终名落地 ${(nowMs() - t0) / 1000}s`);
    } finally {
      if (!thawed) { try { pB.kill("SIGCONT"); } catch { /* 已经退了 */ } }
    }
    const got = sizeOf(landed) >= 0
      ? createHash("sha256").update(fs.readFileSync(landed)).digest("hex") : null;
    check("补齐之后的字节内容必须等于源文件（终局不许是坏内容）",
      got === wantSha5, wantSha5.slice(0, 12) + "…", got ? got.slice(0, 12) + "…" : "未落地");
    const bDb = openDb(INSTANCES[1].db, true);
    const b5 = bDb.prepare("SELECT status FROM file_transfers WHERE id=?1").all(xferId5);
    bDb.close();
    // ⚠️ 同 J2 /  kill 轮：解冻→落地 0.5s 太快，读 A 必须等它自己被 ack 点亮，不能拿 B 的 rename 当同步点。
    const t5 = await waitSendTerminal(xferId5);
    const a5 = t5.row;
    const q5 = t5.queued;
    const both5 = `B=${b5.map((r) => r.status).join("/") || "无行"} A=${a5?.status ?? "无行"} outbox=${q5}`;
    check("对端解冻后必须自己补到终态：两侧 done 且 outbox 已清（不许停在中间态、不许弃单）",
      b5.length === 1 && b5[0].status === "done" && a5?.status === "done" && q5 === 0,
      "1 行 + 双侧 done + outbox=0", both5);
    const kept5 = fs.existsSync(dl) ? fs.readdirSync(dl).filter((f) => f.includes(xferId5)) : [];
    check("解冻之后不许留下第二次成功的痕迹",
      kept5.length === 1 && kept5[0] === `${xferId5}.bin`, `${xferId5}.bin`, kept5.join(", ") || "空");
  });
}

if (SENDKILL) {
  step("故障注入判据⑩：发送端在飞时被 SIGKILL ⇒ 崩溃不许抹掉「已入队」，重启后必须自己续完", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(dl, { recursive: true });
    fs.mkdirSync(srcDir, { recursive: true });
    const idK = eid("k");
    const srcFileK = path.join(srcDir, `${idK}.bin`);
    fs.writeFileSync(srcFileK, Buffer.alloc(SENDKILL_BYTES));
    const srcShaK = createHash("sha256").update(fs.readFileSync(srcFileK)).digest("hex");
    const landedK = path.join(dl, `${idK}.bin`);
    const partK = path.join(dl, `${idK}.part`);
    const sizeK = (p) => { try { return fs.statSync(p).size; } catch { return -1; } };
    // lie：注入完全相同，只换"补完之后该等于哪个摘要"⇒ 第 4 条必须红。
    const wantShaK = LIE ? LIE_SHA : srcShaK;
    const pA = procs.get(INSTANCES[0].n);
    if (!pA) throw new Error("拿不到 A 的子进程句柄 —— 这一格要杀的正是发送端");

    // 入队 = 复刻 `send_file` 命令在点击那一刻写的**三行**（气泡 / 传输台账 / 队列），照⑥ 的同形状。
    // 只写队列那一行就证不到"用户看得见的那一单"：崩溃后气泡没了 = 用户以为发过、其实没人再发，
    // 而台账与队列都在时用户界面上至少还有个入口 —— 这两件事在 UI 上是两个不同的结局。
    // ⚠️ 进程活着时写它的库是新用法：seed() 末尾的 wal_checkpoint(TRUNCATE) 撞上在写的连接会
    //    SQLITE_BUSY ⇒ 有限重试；真进不去就该换成"停机入队 + 更大文件"那条路。
    for (let i = 0; ; i++) {
      try {
        seed(INSTANCES[0].db, (db) => {
          db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(idK);
          db.prepare("DELETE FROM file_transfers WHERE id=?1").run(idK);
          db.prepare("DELETE FROM messages WHERE msg_id=?1").run(`file-${idK}`);
          const ts = nowMs();
          const seq = db.prepare("SELECT COALESCE(MAX(seq),0)+1 s FROM messages WHERE conv_id=?1")
            .get(peerTo).s;
          db.prepare(
            `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
             VALUES(?1,?2,?3,?4,'file',?5,?6,?7,'sent')`,
          ).run(`file-${idK}`, peerTo, idA.runtimeId, peerTo,
            JSON.stringify({ name: `${idK}.bin`, path: srcFileK, size: SENDKILL_BYTES, sha256: "", subtype: "file" }),
            ts, seq);
          db.prepare(
            `INSERT INTO file_transfers(id,peer_id,name,size,direction,status,path,progress,created_at)
             VALUES(?1,?2,?3,?4,'send','pending',?5,0,?6)`,
          ).run(idK, peerTo, `${idK}.bin`, SENDKILL_BYTES, srcFileK, ts);
          db.prepare(
            `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
             VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
          ).run(idK, peerTo, srcFileK, `${idK}.bin`, SENDKILL_BYTES, ts);
        });
        break;
      } catch (e) {
        if (i >= 5) throw e;
        await sleep(300);
      }
    }

    // ★ 世界前提（抛异常，不设判据）：必须抓到"字节正在飞"。抓不到就等于什么都没注入，
    //   而后面几条"不许成功"会因为链路根本没跑而集体假绿 —— 那是最像成功的一种失败。
    //   50 ms 自旋：100 MB 在回环上 ~0.8 s 就传完了，waitFor 的 500 ms 粒度抓不住。
    let atKill = -1;
    const spinT0 = nowMs();
    for (;;) {
      atKill = sizeK(partK);
      if (atKill > 0 && atKill < SENDKILL_BYTES) break;
      if (sizeK(landedK) >= 0) throw new Error(`还没杀就已收完（${(nowMs() - spinT0) / 1000}s）—— 在飞窗口没抓到`);
      if (nowMs() - spinT0 > 120_000) throw new Error(`等 120s 仍没抓到在飞 .part（实际 ${atKill} 字节）`);
      await sleep(50);
    }
    // ⚠️ 被信号杀死的子进程 `exitCode === null`，只有 `signalCode` 有值（③ 踩过同一个坑）。
    const deadA = () => pA.exitCode !== null || pA.signalCode !== null;
    pA.kill("SIGKILL");
    await waitFor(deadA, 15_000, "A 进程确认已死（SIGKILL 不给它收尾的机会）");
    await sleep(20_000); // 一段"发送端根本不存在"的时间：这期间 B 不该收到任何东西

    const straysK = fs.existsSync(dl)
      ? fs.readdirSync(dl).filter((f) => f.includes(idK) && f !== `${idK}.part`)
      : [];
    check("发送端已死这 20s 内，接收目录不许出现终名文件（没人发 FileDone，完成无从谈起）",
      sizeK(landedK) < 0, "不存在", sizeK(landedK) >= 0 ? `已出现 ${sizeK(landedK)} 字节` : "不存在");
    const bMidDb = openDb(INSTANCES[1].db, true);
    const bMid = bMidDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(idK) ?? null;
    bMidDb.close();
    check("发送端已死这 20s 内，接收侧台账不许被记成 done",
      bMid?.status !== "done", "非 done", bMid?.status ?? "无行");
    // ★ 这一条是整轮的重点：「先入队再投递」的另一半 —— 崩溃**不许**把入队事实抹掉。
    //   队列行没了 = 这一单永久没人再发，而用户界面上的气泡还在（最难发现的一种丢法）。
    const aDeadDb = openDb(INSTANCES[0].db, true);
    const aDeadQ = aDeadDb.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(idK).c;
    const aDeadRow = aDeadDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(idK) ?? null;
    const aDeadBubble = aDeadDb
      .prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(`file-${idK}`).c;
    aDeadDb.close();
    check("发送端崩溃之后队列行必须还在（1 行，等待重启后重投）—— 崩溃不许当成已送达",
      aDeadQ === 1, 1, aDeadQ);
    // ★ 用户可见的那一半：崩溃不许把**会话里的那条气泡**一起带走。队列行没了 = 永久没人再发，
    //   气泡没了 = 用户连"曾经发过这一单"都看不见；两者在 UI 上是两个不同的结局，所以要各钉一条。
    check("发送端崩溃之后那条文件气泡必须还在（1 行）—— 崩溃不许把用户可见的这一单一起抹掉",
      aDeadBubble === 1, 1, aDeadBubble);
    check("发送端崩溃的这一刻自己不许记成 done（对端一个字节都没确认过）",
      aDeadRow?.status !== "done", "非 done", aDeadRow?.status ?? "无行");
    console.log(`  · 实测：在飞 .part=${atKill}B 时 SIGKILL A → 死透后等 20s`
      + `（B 台账=${bMid?.status ?? "无行"} · A 队列行=${aDeadQ} · A 台账=${aDeadRow?.status ?? "无行"}）`);

    launch(INSTANCES[0]);
    await waitFor(() => tcpOpen(INSTANCES[0].port), 60_000, `重启后的 A 的 TCP ${INSTANCES[0].port} 可连`);
    await waitFor(() => bootReady(INSTANCES[0].log, bootBaseOf.get(INSTANCES[0].n), BOOT_LINE), 30_000,
      "重启后的 A 打出 boot 完成行");
    const t0 = nowMs();
    await waitFor(() => sizeK(landedK) >= 0, 180_000, "重启后的 A 必须把这一单自己补完并 rename 成终名");
    const gotK = sizeK(landedK) >= 0
      ? createHash("sha256").update(fs.readFileSync(landedK)).digest("hex") : null;
    check("重启后补完的那份必须等于源文件（半途崩溃不许留下坏内容当成品）",
      gotK === wantShaK, wantShaK.slice(0, 12) + "…", gotK ? gotK.slice(0, 12) + "…" : "未落地");
    // 读序照⑨ 的教训：先等 A 自己走到终态（它的 done 由 B 的 ack 点亮，B 落地不是 A 的同步点），
    // 再读 B —— 接收侧的 done 由 rename 触发、ack 在其后 ⇒ B 一定不比 A 晚。
    const endK = await waitSendTerminal(idK);
    const aK = endK.row;
    const qK = endK.queued;
    const bEndDb = openDb(INSTANCES[1].db, true);
    const bK = bEndDb.prepare("SELECT status FROM file_transfers WHERE id=?1").all(idK);
    bEndDb.close();
    check("重启后的终局只有一个：两侧 done 且队列清零（不许停在中间态，也不许弃单）",
      bK.length === 1 && bK[0].status === "done" && aK?.status === "done" && qK === 0,
      `B=done A=done outbox=0`,
      `B=${bK.map((r) => r.status).join("/") || "无行"} A=${aK?.status ?? "无行"} outbox=${qK}`);
    const strays2K = fs.existsSync(dl)
      ? fs.readdirSync(dl).filter((f) => f.includes(idK) && f !== `${idK}.bin`) : [];
    check("补完之后接收目录不许留下半截 .part / 改名副本",
      strays2K.length === 0 && straysK.length === 0, "无残留",
      `崩溃窗口 ${straysK.length} 个 · 收尾 ${strays2K.length} 个`);
    console.log(`  · 实测：A 重启 → 终名落地 ${(nowMs() - t0) / 1000}s；`
      + `崩溃时 .part=${atKill}B → 落地 ${sizeK(landedK)}B（同一单从盘上真实进度续完）`);
  });
}

if (STALL) {
  step("故障注入判据⑨：字节在飞时冻住对端 ⇒ 冻结期间盘上进度一字不涨，解冻后必须补到源内容", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(dl, { recursive: true });
    fs.mkdirSync(srcDir, { recursive: true });
    const idS = eid("s");
    const srcFileS = path.join(srcDir, `${idS}.bin`);
    fs.writeFileSync(srcFileS, Buffer.alloc(STALL_BYTES));
    const srcShaS = createHash("sha256").update(fs.readFileSync(srcFileS)).digest("hex");
    const landedS = path.join(dl, `${idS}.bin`);
    const partS = path.join(dl, `${idS}.part`);
    const sizeOfS = (p) => { try { return fs.statSync(p).size; } catch { return -1; } };
    // lie：注入完全相同，只换"补完之后该等于哪个摘要"⇒ 第 7 条必须红。
    const wantShaS = LIE ? LIE_SHA : srcShaS;
    const pB = procs.get(INSTANCES[1].n);
    if (!pB) throw new Error("拿不到 B 的子进程句柄 —— 这条注入没有可冻结的对象");

    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(idS);
      db.prepare(
        `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
         VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
      ).run(idS, peerTo, srcFileS, `${idS}.bin`, STALL_BYTES, nowMs());
    });

    let frozen = false;
    let thawed = false;
    let atGrowth = -1;
    let atFreeze = -1;
    let mid = null;
    try {
      // ⚠️ 自旋等 `.part` 真的开始长 —— 这一条是整轮的**世界前提**：
      // 抓不到在飞字节，后面"不许假成功"那几条会因为链路根本没跑而集体假绿。
      // 50ms 粒度（不是 waitFor 的 500ms）：100 MB 在回环上 ~0.78s 就走完了。
      const spinT0 = nowMs();
      for (;;) {
        atGrowth = sizeOfS(partS);
        if (atGrowth > 0) break;
        if (sizeOfS(landedS) >= 0) throw new Error(`还没冻住就已经收完（${(nowMs() - spinT0) / 1000}s）—— 在飞窗口没抓到`);
        if (nowMs() - spinT0 > 20_000) throw new Error("等 20s 仍没有 .part：这一单根本没被投递");
        await sleep(50);
      }
      const freezeT0 = nowMs();
      pB.kill("SIGSTOP");
      frozen = true;
      // 快照取在**冻结之后**：冻结前那几毫秒 B 还在写，拿 atGrowth 当基准会虚涨。
      // ⚠️ 还要再等一下：`kill("SIGSTOP")` 是**异步生效**的，信号排到队上之后 B 仍可能写完一片。
      //   这一格的基准要是取早了，就会把"B 在信号生效前最后写的那片"算成"冻结期间涨了" ——
      //   与注入③ 同族的读数竞态（那里是 SIGKILL 前快照，已改成确认已死后再读）。
      await sleep(300);
      atFreeze = sizeOfS(partS);
      await sleep(STALL_HOLD_MS);

      const midDb = openDb(INSTANCES[0].db, true);
      // ⚠️ `attempts` 在 `file_outbox` 上，不在 `file_transfers` 上（第一版在这里写了
      //   `SELECT status,attempts FROM file_transfers` ⇒ no such column，红的是判据不是产品）。
      const midA = midDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(idS) ?? null;
      const midOut = midDb.prepare("SELECT status, attempts FROM file_outbox WHERE transfer_id=?1").get(idS) ?? null;
      midDb.close();
      const midBDb = openDb(INSTANCES[1].db, true);
      const midB = midBDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(idS) ?? null;
      midBDb.close();
      mid = {
        part: sizeOfS(partS), landed: sizeOfS(landedS),
        a: midA?.status ?? null, b: midB?.status ?? null,
        out: midOut ? `${midOut.status}/${midOut.attempts}` : "无行",
      };
      check("冻结期间接收目录不许出现终名文件（没写完就没有完成可言）",
        mid.landed < 0, "不存在", mid.landed >= 0 ? `已出现 ${mid.landed} 字节` : "不存在");
      check("冻结期间发送侧不许记成 done", mid.a?.status !== "done", "非 done", mid.a?.status ?? "无行");
      check("冻结期间接收侧不许记成 done（它一次回执都没发出去）",
        mid.b?.status !== "done", "非 done", mid.b?.status ?? "无行");
      // ★ 这一条钉的是不变量「接收端真实进度」的跨实例形状：对端停止写盘之后，
      //   盘上进度必须**一字不涨**。A 往 socket 里灌的字节、内核缓冲的字节都不算进度。
      check("对端被冻住期间，接收端盘上进度不许继续涨（进度只能来自真实写入）",
        mid.part === atFreeze, `冻结时刻的 ${atFreeze} 字节`,
        mid.part === atFreeze ? `${atFreeze} 字节（一字未涨）` : `涨到 ${mid.part} 字节`);

      // 走的是哪条分支只打印、不判：**2026-09-26 已量清** —— 先到的是 45 s 链路 watchdog（15 s × 3），
      // 60 s 的 `FILE_STALL_ABORT_MS` 在"对端完全冻死"下够不到（156 份归档 run 里 `[STALL]` 命中 0 行），
      // 所以把"看到 [STALL]"写成判据必然是永远不成立的空转 ⇒ 只打印。机制与那 0 行的账记在 roadmap A-13。
      const abandon = (tailLog(INSTANCES[0].log, 200000) || "").split("\n")
        .filter((x) => x.includes(idS) && /STALL|放弃|ok=false|失败|拒绝|error/i.test(x)).slice(-6);
      console.log(`  · 实测：抓到在飞 .part=${atGrowth}B → 冻结时 ${atFreeze}B → 冻后 ${mid.part}B`
        + `（冻结 ${(nowMs() - freezeT0) / 1000}s）`);
      console.log(`  · 实测：A 侧这一单 ${mid.a ?? "无行"} / B 侧 ${mid.b ?? "无行"}`
        + ` / 队列行 status/attempts=${mid.out}`);
      for (const l of abandon) console.log("      A│ " + l.slice(0, 220));
      if (!abandon.length) console.log("      · A 侧本轮没留下'这一单已结束'的日志痕迹（记下，待判是否 A-13）");

      pB.kill("SIGCONT");
      thawed = true;
      const t0 = nowMs();
      await waitFor(() => sizeOfS(landedS) >= 0, 180_000, "解冻后必须把这一单补完并 rename 成终名");
      console.log(`  · 实测：解冻 → 终名落地 ${(nowMs() - t0) / 1000}s`);
    } finally {
      // ⚠️ 被 SIGSTOP 停住的进程收不到 SIGTERM ⇒ 不解冻会把整条 harness 挂死（冻结轮的教训）。
      if (frozen && !thawed) { try { pB.kill("SIGCONT"); } catch { /* 已经退了 */ } }
    }

    const gotS = sizeOfS(landedS) >= 0
      ? createHash("sha256").update(fs.readFileSync(landedS)).digest("hex") : null;
    check("补完之后的字节内容必须等于源文件（半途放弃不许留下坏内容当成功）",
      gotS === wantShaS, wantShaS.slice(0, 12) + "…", gotS ? gotS.slice(0, 12) + "…" : "未落地");
    // ⚠️ 判终局之前必须**等 A 自己走到终态**，不能拿"B 的终名文件出现"当同步点
    //   （理由与窗口取值都写在 `waitSendTerminal` 上；这一轮就是它被本地层判红的现场）。
    //   读序：先等 A，再读 B —— 接收侧的 done 由 rename 触发、ack 在其后 ⇒ B 一定不比 A 晚。
    const endS = await waitSendTerminal(idS);
    const aS = endS.row;
    const qS = endS.queued;
    const bDbS = openDb(INSTANCES[1].db, true);
    const bS = bDbS.prepare("SELECT status FROM file_transfers WHERE id=?1").all(idS);
    bDbS.close();
    console.log(`  · 实测：B 落地 → A 终态 ${(endS.waitedMs / 1000).toFixed(1)}s`
      + `（A=${aS?.status ?? "无行"} outbox=${qS}）`);
    check("终局只有一个：两侧 done 且 outbox 已清（不许停在中间态，也不许悄悄弃单）",
      bS.length === 1 && bS[0].status === "done" && aS?.status === "done" && qS === 0,
      "1 行 + 双侧 done + outbox=0",
      `B=${bS.map((r) => r.status).join("/") || "无行"} A=${aS?.status ?? "无行"} outbox=${qS}`);
    const keptS = fs.existsSync(dl) ? fs.readdirSync(dl).filter((f) => f.includes(idS)) : [];
    check("补完之后接收目录只剩终名文件：那份半截 .part 不许残留",
      keptS.length === 1 && keptS[0] === `${idS}.bin`, `${idS}.bin`, keptS.join(", ") || "空");
  });
}

if (DISK) {
  step("故障注入判据⑤：接收目录写不进去 ⇒ 必须明确失败并止步，不许假 done、不许无限重试", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId6 = eid("g");
    const srcFile6 = path.join(srcDir, `${xferId6}.bin`);
    fs.writeFileSync(srcFile6, Buffer.alloc(DISK_BYTES));
    const t0 = nowMs();
    // 注入：接收目录整个改成只读 —— 建 `.part` 与最终 rename 都需要目录写权限。
    fs.chmodSync(dl, 0o500);
    try {
      for (let i = 0; ; i++) {
        try {
          seed(INSTANCES[0].db, (db) => {
            db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId6);
            db.prepare(
              `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
               VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
            ).run(xferId6, peerTo, srcFile6, `${xferId6}.bin`, DISK_BYTES, nowMs());
          });
          break;
        } catch (e) {
          if (i >= 5) throw e;
          await sleep(300);
        }
      }
      term6 = null;
      await waitFor(() => {
        const db = openDb(INSTANCES[0].db, true);
        const r = db
          .prepare("SELECT status,attempts FROM file_outbox WHERE transfer_id=?1")
          .get(xferId6);
        db.close();
        // 终态 = 队列行落到 failed/cancelled。
        // ⚠️ 不要把 `sending` 当终态：它是"正在投递"的中间态（mark_file_outbox_sending 顺手 +1 attempts），
        //    第一版就是这么判的，结果第 4 次尝试的 33.2 s 处抓到 `{status:'sending',attempts:4}` 判红。
        //    （停在 sending 会不会永久卡住？不会 —— AppState 初始化有 reset_sending_to_pending，
        //    file_offline.rs:135-146 的注释正是为这件事写的。）
        if (r && (r.status === "failed" || r.status === "cancelled")) term6 = r;
        return !!term6;
      }, 180_000, "A 侧这一单要在重试上限内落到明确终态（不许静静挂着）");
      console.log(
        `  · 实测：入队 → 明确终态 ${((nowMs() - t0) / 1000).toFixed(1)}s ${JSON.stringify(term6)}`,
      );
      for (const l of (tailLog(INSTANCES[0].log, 60000) || "").split("\n")
        .filter((x) => x.includes(xferId6)).slice(-6)) console.log("      A│ " + l.slice(0, 220));
      for (const l of (tailLog(INSTANCES[1].log, 60000) || "").split("\n")
        .filter((x) => x.includes("初始化失败")).slice(-3)) console.log("      B│ " + l.slice(0, 220));
    } finally {
      // ⚠️ 必须还原：否则这一轮的接收目录连同后续清理都带着只读位，
      //    而且下一个模式会被这条注入的残留状态污染。
      fs.chmodSync(dl, 0o700);
    }
    const seen = fs.readdirSync(dl).filter((f) => f.includes(xferId6));
    // 反空转前提（冻结轮的教训：先量"我以为已经成立的前提"）：
    // B 的日志必须真说过"初始化失败" ⇒ 注入确实生效、A 确实试过，而不是"这一单没跑"带来的假绿。
    const bLog = tailLog(INSTANCES[1].log, 200000) || "";
    check("注入真的生效：接收端日志必须出现「接收文件初始化失败」",
      bLog.includes("接收文件初始化失败"), "≥1 次", (bLog.match(/接收文件初始化失败/g) || []).length);
    check("重试真的发生过（与冻结轮的分水岭：这里对端活着、有入站帧）",
      !!term6 && term6.attempts >= 2, "≥2", term6 ? term6.attempts : "无终态");
    check("重试不许失控：次数不得超过 MAX_FILE_OUTBOX_RETRIES",
      !!term6 && term6.attempts <= DISK_MAX_ATTEMPTS, `≤${DISK_MAX_ATTEMPTS}`,
      term6 ? term6.attempts : "无终态");
    // lie 模式：注入完全一样，只把"该落到哪个终态"换成 done ⇒ 这一条必须红。
    const wantTerm = LIE ? "done" : "failed";
    check("接收端写不进去时，发送侧必须落到明确终态（不许停在 pending/active，也不许假 done）",
      !!term6 && term6.status === wantTerm, wantTerm, term6 ? term6.status : "始终没到终态");
    const aDb = openDb(INSTANCES[0].db, true);
    const a6 = aDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId6);
    aDb.close();
    check("发送侧台账不许是 done（唯一出口的判定）",
      a6?.status !== "done", "非 done", a6?.status ?? "无行");
    check("写不进去就不许在接收目录留下这个 transfer 的任何东西",
      seen.length === 0, "无文件", seen.join(", ") || "无");
  });
}

if (ROT) {
  step("故障注入判据⑧：预置可写 .part 后把接收目录改成只读 ⇒ 半路失败不许被当成完成（rename 才算完成）", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId8 = eid("i");
    const name8 = `${xferId8}.bin`;
    const part8 = path.join(dl, `${xferId8}.part`);
    const final8 = path.join(dl, name8);
    srcFile8 = path.join(srcDir, name8);
    const buf8 = Buffer.alloc(ROT_BYTES);
    for (let i = 0; i < buf8.length; i += 32) buf8.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
    fs.writeFileSync(srcFile8, buf8);
    srcSha8 = createHash("sha256").update(buf8).digest("hex");
    // 注入的形状刻意选成「前缀已经在那儿了，之后的每一步都不许改口」：
    //   1) 预置**真前缀** `.part` ⇒ 接收端走的是 `resume_receive`（播种 hasher、不 truncate），
    //      于是 offer 期不会 EACCES，A 一定把剩下的字节发过来 —— 与⑤（offer 期就写不进）分道。
    //   2) 再把**目录**改成只读 ⇒ 往已存在的文件里写仍然合法（写权限看的是 inode），
    //      但 create / rename / unlink 全部 EACCES ⇒ 唯一会塌下来的动作就是收尾那次 rename。
    //      这正是⑤的注释里点名"要另开一条"的 A-5 形状。
    fs.writeFileSync(part8, buf8.subarray(0, ROT_PREFIX));
    const t0 = nowMs();
    fs.chmodSync(dl, 0o500);
    let bTerminal = null;
    let aGrew = false;
    let partGrew = 0;
    try {
      for (let i = 0; ; i++) {
        try {
          seed(INSTANCES[0].db, (db) => {
            db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId8);
            db.prepare(
              `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
               VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
            ).run(xferId8, peerTo, srcFile8, name8, ROT_BYTES, nowMs());
          });
          break;
        } catch (e) {
          if (i >= 5) throw e;
          await sleep(300);
        }
      }
      const sizeOf = (p) => {
        try {
          return fs.statSync(p).size;
        } catch {
          return -1;
        }
      };
      term8 = null;
      // ⚠️ 「A 侧 outbox 行被删掉」**就是终态**（成功收尾的判据，见 J2 那条
      //    「A 侧 file_outbox 行已被收尾删除」）。第一版我把它当成"还没到终态"继续等 ⇒
      //    跑满 180 s 超时，把"A 已经宣布完成"这件事实读成了"卡住"。行没了必须立刻收，
      //    否则这条判据会把**成功**判成**超时**，而超时恰恰是这条判据最不该混淆的信号。
      let sawRow = false;
      await waitFor(() => {
        const g = sizeOf(part8);
        if (g > ROT_PREFIX) {
          partGrew = g;
          aGrew = true;
        }
        const ad = openDb(INSTANCES[0].db, true);
        const r = ad.prepare("SELECT status,attempts FROM file_outbox WHERE transfer_id=?1").get(xferId8);
        const aT = ad.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId8);
        ad.close();
        const bd = openDb(INSTANCES[1].db, true);
        const bT = bd.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId8);
        bd.close();
        bTerminal = bT?.status ?? null;
        if (r) sawRow = true;
        if (!r && sawRow) term8 = { status: "gone", attempts: 0, aT: aT?.status ?? null };
        else if (r && (r.status === "failed" || r.status === "cancelled" || r.status === "done"))
          term8 = { ...r, aT: aT?.status ?? null };
        return !!term8;
      }, 180_000, "A 侧队列要落到明确终态（行被收尾删除 / failed / cancelled / done 都算，不许静静挂着）");
      console.log(
        `  · 实测：入队 → A 终态 ${((nowMs() - t0) / 1000).toFixed(1)}s ${JSON.stringify(term8)} B=${bTerminal ?? "无行"} .part 峰值=${partGrew}`,
      );
      for (const l of (tailLog(INSTANCES[1].log, 200000) || "").split("\n")
        .filter((x) => x.includes(xferId8) || /rename|重命名|写失败|finalize|终态/i.test(x)).slice(-8)) console.log("      B│ " + l.slice(0, 220));
    } finally {
      fs.chmodSync(dl, 0o700);
    }
    // ── 反空转前提（⑤的教训：先量"我以为已经成立的前提"，再判结论）──
    // 前缀真的被续写 ⇒ 这一轮走的是"收到一半才失败"，而不是⑤那条"offer 期就被拒"。
    // 这条判红不表示产品坏了，表示**注入没落地**，必须分开说，否则后面每一条都是空转。
    check("注入真的落地：.part 必须被续写过（超过预置前缀）", aGrew, `>${ROT_PREFIX}`, partGrew);
    check("A 侧队列必须落到明确终态（行被收尾删除/failed/cancelled/done 都算，停在 pending/sending=界面永远转圈）",
      !!term8, "终态", "180s 内没到终态");
    check("重试不许失控：A 侧 attempts 有界", !!term8 && term8.attempts <= DISK_MAX_ATTEMPTS,
      `≤${DISK_MAX_ATTEMPTS}`, term8 ? term8.attempts : "无终态");
    const existsFinal = fs.existsSync(final8);
    const finalSha = existsFinal ? createHash("sha256").update(fs.readFileSync(final8)).digest("hex") : null;
    const landedWhole = existsFinal && finalSha === srcSha8;
    const claimedDone = !!term8 && (term8.status === "gone" || term8.status === "done" || term8.aT === "done");
    // ★ A-12 修完之后加回来的那条交叉自洽判据（原文照抄 roadmap A-12 那格，一字未改）
    check("发送侧宣布完成（队列行被收尾删除或台账 done）⇒ 接收侧必须有整份且 sha256 相等的 final 文件",
      !claimedDone || landedWhole,
      claimedDone ? "整份 final" : "不声称完成（前件不成立）",
      `A 队列=${term8?.status ?? "无"} / A 台账=${term8?.aT ?? "无"} / final=${existsFinal ? (landedWhole ? "整份" : "内容不符") : "不存在"} / .part=${partGrew}`);
    // 上面那条是**蕴含式**，前件不成立时它自己永远红不了 ⇒ 必须再钉一条"本轮 rename 恒失败 ⇒
    // 发送侧只能落到明确失败"。没有这条，上一条就会退化成 A-9 那次救过我的"永远为真的空转"；
    // lie 轮翻的也正是这一条的期望值。
    const wantA = LIE ? "done" : "failed";
    check("改名永远做不成时，发送侧只能落到明确失败（不许 done，更不许把队列行删掉当收尾）",
      !!term8 && term8.status === wantA, wantA, term8 ? term8.status : "180s 内没到终态");
    // 接收侧台账不许假装成功：B 侧有行时只能停在非 done。
    check("接收侧台账不许假装成功（唯一的失败出口）",
      landedWhole || bTerminal !== "done", "非 done", bTerminal ?? "B 侧无行");
    console.log(`      注：解除只读后 B=${bTerminal ?? "无行"} / .part=${fs.existsSync(part8) ? fs.statSync(part8).size : "已清"} —— 自愈与否只打印，不设判据`);
  });
}

if (SHRINK) {
  step("故障注入判据⑥：入队后源文件被改小 ⇒ 只许按磁盘上那份真值收发，两侧终态一致", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId7 = eid("h");
    const name7 = `${xferId7}.bin`;
    srcFile7 = path.join(srcDir, name7);
    fs.writeFileSync(srcFile7, Buffer.alloc(SHRINK_BYTES));
    const landed = path.join(dl, name7);
    const sizeOf = (p) => { try { return fs.statSync(p).size; } catch { return -1; } };
    const pB = procs.get(INSTANCES[1].n);
    if (!pB) throw new Error("拿不到 B 的子进程句柄 —— 这一轮的窗口要靠冻结 B 来保证");
    let thawed = false;
    let sent = null;
    try {
      // 顺序不能换：先冻住 B，A 才有"不读盘"的确定窗口（A 只在收到入站帧时才 flush，见 A-9）。
      pB.kill("SIGSTOP");
      // 入队 = 复刻 `send_file` 命令在点击那一刻写的三行（气泡 / 传输台账 / 队列）。
      // 少写一行就测不到这一格的疑点：气泡与台账的 size 都是**按当时磁盘**算出来的。
      for (let i = 0; ; i++) {
        try {
          seed(INSTANCES[0].db, (db) => {
            db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId7);
            db.prepare("DELETE FROM file_transfers WHERE id=?1").run(xferId7);
            db.prepare("DELETE FROM messages WHERE msg_id=?1").run(`file-${xferId7}`);
            const ts = nowMs();
            const seq = db.prepare("SELECT COALESCE(MAX(seq),0)+1 s FROM messages WHERE conv_id=?1")
              .get(peerTo).s;
            db.prepare(
              `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
               VALUES(?1,?2,?3,?4,'file',?5,?6,?7,'sent')`,
            ).run(`file-${xferId7}`, peerTo, idA.runtimeId, peerTo,
              JSON.stringify({ name: name7, path: srcFile7, size: SHRINK_BYTES, sha256: "", subtype: "file" }),
              ts, seq);
            db.prepare(
              `INSERT INTO file_transfers(id,peer_id,name,size,direction,status,path,progress,created_at)
               VALUES(?1,?2,?3,?4,'send','pending',?5,0,?6)`,
            ).run(xferId7, peerTo, name7, SHRINK_BYTES, srcFile7, ts);
            db.prepare(
              `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
               VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
            ).run(xferId7, peerTo, srcFile7, name7, SHRINK_BYTES, ts);
          });
          break;
        } catch (e) {
          if (i >= 5) throw e;
          await sleep(300);
        }
      }
      // 注入：入队之后把原件改小（用户在同一批"等着对方上线"的单子还没发出去时改了那个文件）。
      fs.truncateSync(srcFile7, SHRINK_TO);
      const shrunkSha = createHash("sha256").update(fs.readFileSync(srcFile7)).digest("hex");
      pB.kill("SIGCONT");
      thawed = true;
      const t0 = nowMs();
      await waitFor(() => {
        const aDb = openDb(INSTANCES[0].db, true);
        const a = aDb.prepare("SELECT status,size FROM file_transfers WHERE id=?1").get(xferId7);
        const q = aDb.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(xferId7).c;
        const bubble = aDb.prepare("SELECT content FROM messages WHERE msg_id=?1").get(`file-${xferId7}`);
        aDb.close();
        const bDb = openDb(INSTANCES[1].db, true);
        const b = bDb.prepare("SELECT status,size FROM file_transfers WHERE id=?1").get(xferId7);
        bDb.close();
        // 收完的判据用"队列行已关 + 两侧 done"，不用 landed 存在 —— rename 之后 A 还要等回执才落 done。
        if (a && b && a.status === "done" && b.status === "done" && q === 0) {
          sent = {
            a, q, b, landed: sizeOf(landed),
            bubbleSize: bubble ? JSON.parse(bubble.content).size : null,
            bubbleSha: bubble ? JSON.parse(bubble.content).sha256 : null,
            shrunkSha,
          };
        }
        return !!sent;
      }, 120_000, "改小的原件要按新 size 走完 offer→chunk→rename→done");
      console.log(`  · 实测：解冻 → 两侧 done ${(nowMs() - t0) / 1000}s ${JSON.stringify(sent)}`);
      for (const l of (tailLog(INSTANCES[0].log, 60000) || "").split("\n")
        .filter((x) => x.includes(xferId7)).slice(-6)) console.log("      A│ " + l.slice(0, 220));
    } finally {
      // 被 SIGSTOP 停住的进程收不到 SIGTERM ⇒ 不解冻会把整条 harness 挂死（冻结轮的教训）。
      if (!thawed) { try { pB.kill("SIGCONT"); } catch { /* 已经退了 */ } }
    }
    const landedBytes = sizeOf(landed);
    const got = landedBytes >= 0
      ? createHash("sha256").update(fs.readFileSync(landed)).digest("hex") : null;
    check("落地字节数必须等于截断后的磁盘大小（说明这一单按真值重算，不是按入队那份）",
      sent.landed === SHRINK_TO && landedBytes === SHRINK_TO, SHRINK_TO,
      `offer时队列=${sent?.landed} 盘上=${landedBytes}`);
    check("落地内容必须等于截断后的源文件（不许把半截当完成，也不许多给旧字节）",
      got === (LIE ? LIE_SHA : sent.shrunkSha), (LIE ? LIE_SHA : sent.shrunkSha).slice(0, 12) + "…",
      got ? got.slice(0, 12) + "…" : "未落地");
    check("两侧台账必须同时 done 且队列行已关（跨设备终态不许分叉）",
      sent.a.status === "done" && sent.b.status === "done" && sent.q === 0,
      "A=done B=done outbox=0", `A=${sent.a.status} B=${sent.b.status} outbox=${sent.q}`);
    check("接收目录只许有终名那一个文件（无 .part 残留、无第二份）",
      fs.readdirSync(dl).filter((f) => f.includes(xferId7)).join(",") === name7, name7,
      fs.readdirSync(dl).filter((f) => f.includes(xferId7)).join(", ") || "空");
    check("接收端台账的 size 必须等于盘上真实字节数（接收端说真话）",
      sent.b.size === landedBytes, landedBytes, sent.b.size);
    check("发送端气泡回填的 sha256 必须等于落地文件摘要（内容寻址 cid 不许撒谎）",
      sent.bubbleSha === got, got?.slice(0, 12) + "…", sent.bubbleSha?.slice(0, 12) ?? "空");
    // ⚠️ 这一行**不是断言**，是这一轮照出来的**分歧证据**（已按 A-11 登记 roadmap）：
    //   气泡与发送台账的 size 来自点击那一刻的磁盘，`upsert_transfer` 的 ON CONFLICT 只改
    //   status/path/progress、不改 size ⇒ 原件事后变小时，A 自己看到的"多大"和真正发出去、
    //   B 收到的那份就不是一个数。修（回填 size）之前不许把它写成断言，也不许删这条打印。
    console.log(
      `  · 实测 size 分歧：入队 ${SHRINK_BYTES} → 实发 ${landedBytes}` +
      ` | A 气泡 ${sent.bubbleSize} · A 台账 ${sent.a.size} · B 台账 ${sent.b.size}`,
    );
  });
}

if (MULTI) {
  step("故障注入判据⑦：三个文件一起排队、其中两个同名 ⇒ 一张都不许丢、内容不许串味", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const token = Math.random().toString(36).slice(2, 8);
    const photoName = `photo-${token}.bin`;
    const noteName = `note-${token}.bin`;
    // 同名的两张必须放在**不同目录**（同一路径放不下两个文件）：offer 的 name 取自路径的
    // file_name（`file.rs:358`），所以"同名不同内容"只能这样造。
    const layout = [
      { dir: "p1", name: photoName, bytes: 1024 * 1024, fill: 0xa1 },
      { dir: "p2", name: photoName, bytes: 64 * 1024, fill: 0xb2 },
      { dir: "p1", name: noteName, bytes: 256 * 1024, fill: 0xc3 },
    ];
    multiSpec = layout.map((l, i) => {
      const d = path.join(RUN_DIR, "src", l.dir);
      fs.mkdirSync(d, { recursive: true });
      const src = path.join(d, l.name);
      fs.writeFileSync(src, Buffer.alloc(l.bytes, l.fill));
      return {
        tid: eid("m", `${token}-${i}`),
        src,
        name: l.name,
        bytes: l.bytes,
        sha: createHash("sha256").update(fs.readFileSync(src)).digest("hex"),
      };
    });
    // lie：注入完全一样，只把**其中一张**的期望摘要换掉 ⇒ 那条多重集合判据必须红。
    // 用集合而不是"随便挑一张比对"，就是为了验证"每份内容各自对上了"，不是"对上了三份里的任意一份"。
    const wantShas = multiSpec.map((s) => s.sha).sort();
    if (LIE) wantShas[0] = LIE_SHA;
    for (let i = 0; ; i++) {
      try {
        seed(INSTANCES[0].db, (db) => {
          const ins = db.prepare(
            `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
             VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
          );
          for (const s of multiSpec) {
            db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(s.tid);
            ins.run(s.tid, peerTo, s.src, s.name, s.bytes, nowMs());
          }
        });
        break;
      } catch (e) {
        if (i >= 5) throw e;
        await sleep(300);
      }
    }
    const t0 = nowMs();
    let snap = null;
    await waitFor(() => {
      const aDb = openDb(INSTANCES[0].db, true);
      const rows = aDb
        .prepare(`SELECT id,status FROM file_transfers WHERE id IN (${multiSpec.map(() => "?").join(",")})`)
        .all(...multiSpec.map((s) => s.tid));
      const queued = aDb
        .prepare(`SELECT COUNT(*) c FROM file_outbox WHERE transfer_id IN (${multiSpec.map(() => "?").join(",")})`)
        .get(...multiSpec.map((s) => s.tid)).c;
      aDb.close();
      const landed = fs.readdirSync(dl).filter((f) => f.includes(token));
      const parts = landed.filter((f) => f.endsWith(".part"));
      const files = landed.filter((f) => !f.endsWith(".part"));
      if (rows.length === multiSpec.length && rows.every((r) => r.status === "done")
        && queued === 0 && files.length === multiSpec.length) {
        snap = { rows, queued, landed, parts };
      }
      return !!snap;
    }, 180_000, "三单（含两张同名）要在同一批里全部投递完成");
    console.log(
      `  · 实测：入队 3 单 → 全部终态 ${((nowMs() - t0) / 1000).toFixed(1)}s · `
      + `落地 ${JSON.stringify(snap.landed.sort())}`,
    );
    const files = snap.landed.filter((f) => !f.endsWith(".part"));
    const gotShas = files
      .map((f) => createHash("sha256").update(fs.readFileSync(path.join(dl, f))).digest("hex"))
      .sort();
    const photoLanded = files.filter((f) => f.includes(photoName.replace(".bin", "")));
    check("一张都不许丢：三单必须各自落到一行 done 且队列已清空",
      snap.rows.length === multiSpec.length && snap.rows.every((r) => r.status === "done")
      && snap.queued === 0,
      "3 行 done + outbox=0", JSON.stringify(snap.rows) + ` outbox=${snap.queued}`);
    check("同名不许互相覆盖：接收目录里这张名字必须出现两次（少一次就是静默丢数据）",
      photoLanded.length === 2, 2, `${photoLanded.length} → ${photoLanded.join(", ")}`);
    check("每一张都必须是完整、各自对得上的内容（不许交错、不许串味、不许被顶掉）",
      gotShas.join(",") === wantShas.join(","),
      multiSpec.map((s) => s.sha.slice(0, 8)).join(","), gotShas.map((h) => h.slice(0, 8)).join(","));
    check("不许留下 .part 半成品（串行里每一单都得收尾）",
      snap.parts.length === 0, "无 .part", snap.parts.join(", ") || "无");
    const bDb = openDb(INSTANCES[1].db, true);
    const bRows = bDb
      .prepare(`SELECT id,status,size FROM file_transfers WHERE id IN (${multiSpec.map(() => "?").join(",")})`)
      .all(...multiSpec.map((s) => s.tid));
    bDb.close();
    check("接收侧每一单各记一行、都是 done（不重复记账、不把三单并成一条）",
      bRows.length === 3 && bRows.every((r) => r.status === "done"),
      "3 行 done", JSON.stringify(bRows));
    check("接收侧每单记的字节数必须等于它自己那张源（串味在这里也会露出来）",
      bRows.every((r) => multiSpec.some((s) => s.tid === r.id && s.bytes === r.size)),
      "逐单相等", JSON.stringify(bRows.map((r) => `${r.id.slice(-1)}=${r.size}`))
      + ` 期望 ${JSON.stringify(multiSpec.map((s) => s.bytes))}`);
  });
}

step("L-B 故障注入：两端重启后仍正确", async () => {
  await stopAll();
  await bootAndStop("重启");
  const bDb = openDb(INSTANCES[1].db, true);
  const rows = bDb.prepare("SELECT * FROM messages WHERE msg_id=?1").all(msgId);
  bDb.close();
  check("重启后 B 侧仍只有一条、内容不变",
    rows.length === 1 && rows[0].content === "hello from harness", 1, rows.length);
  const aDb = openDb(INSTANCES[0].db, true);
  const again = aDb.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id=?1").get(msgId).c;
  aDb.close();
  check("重启不复活已 Ack 的 outbox 行（不二次投递）", again === 0, 0, again);
  // §十六「报告要带 screenshots/」这一格第一次有产物。判据只管"截图真落盘、不是空图"，
  // **不管界面对不对**（没有像素级判据；那一半仍按 §12.6 记结构级/MANUAL，写成绿就是假证据）。
  // ⚠️ 非 macOS 没有采集器 ⇒ 这一条**是红，不是跳过**（§十禁止把没跑写成 PASS）：
  //    Windows 腿要自己实现采集器，在那之前这格就明着红着。
  //    同理 macOS 上**屏幕被锁**也红：`screencapture` 退 0 却截到一张整屏纯色，
  //    而 `shotIsReal` 现在会把纯色帧判掉（2026-09-27 实测到的洞）。报错里会说是哪一种。
  shotFiles.push(captureShot(RUN_DIR, "2-after-restart"));
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

/// §22「A 重启后 badge 仍然正确」里**这一层能判的那一半**：徽标数字是前端算的（runtime 层，
/// 见 roadmap §13.3 的层次修正），但它是从这几行状态折叠出来的 —— 所以真正要钉的是
/// 「重启之后这些行还在、没被改写成旧状态、也没被再投一遍」。判据读的是**真 id**：
/// 这一格要证的是持久性，拿翻过的 id 去读只会得到"0 行"，那种红分不清"没送达"和"没留住"。
/// 独立成一个 `if (TASK)` 块而不是塞进上面那一步：上面那一步的判据全按 `check-doc-numbers`
/// 归到「默认轮」，往里塞 TASK 专属断言会让默认轮少算几条、任务轮多算几条（现算守卫会当场判红）。
if (TASK) {
  step("任务重启判据：六条状态行两端都活得过重启，且已 Ack 的队列不被点亮成二次投递", async () => {
    // 上一步（L-B）已经 stopAll + bootAndStop 走完一轮真实重启，此刻两端都是停机库 ⇒ 直接读。
    const bDb = openDb(INSTANCES[1].db, true);
    const bRows = bDb.prepare(
      "SELECT msg_id,kind,seq,status FROM messages WHERE msg_id IN (?1,?2,?3,?4)"
      + " ORDER BY seq",
    ).all(taskCreateId, taskUpdateId, taskArchId, taskReopenId)
      .map((r) => `${r.kind}:${r.seq}`);
    bDb.close();
    const aDb = openDb(INSTANCES[0].db, true);
    const aRows = aDb.prepare(
      "SELECT msg_id,kind,seq FROM messages WHERE msg_id IN (?1,?2) ORDER BY seq",
    ).all(taskBCreateId, taskBDoneId).map((r) => `${r.kind}:${r.seq}`);
    const aLeft = aDb.prepare(
      "SELECT COUNT(*) c FROM group_outbox WHERE msg_id IN (?1,?2,?3,?4)",
    ).get(taskCreateId, taskUpdateId, taskArchId, taskReopenId).c;
    aDb.close();
    const bLeft = (() => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return db.prepare("SELECT COUNT(*) c FROM group_outbox WHERE msg_id IN (?1,?2)")
          .get(taskBCreateId, taskBDoneId).c;
      } finally { db.close(); }
    })();

    check("重启后 B 侧四条状态行仍各一行、kind:seq 一字不变（折叠出的终态不许退）",
      bRows.join(",") === "todo:1,todo_update:2,todo_update:3,todo_update:4",
      "todo:1,todo_update:2,todo_update:3,todo_update:4", bRows.join(","));
    check("重启后 A 侧对端发起的两条仍各一行（不二次投递 = 已 Ack 的队列没被点亮）",
      aRows.join(",") === "todo:5,todo_update:6", "todo:5,todo_update:6", aRows.join(","));
    check("重启后两侧的 group_outbox 对这六条都是 0 行（残留=下次建链会再发一遍）",
      aLeft === 0 && bLeft === 0, "0 / 0", `${aLeft} / ${bLeft}`);
  });
}

// §五「群聊 + gossip」这一族里最后一格：成员**不是 A 的直发对象**，只能靠中间人把 gossip 带给它。
// 两实例的群轮（`--round=group`）里 A→B 是直发（`group_outbox` 一发就中），
// 「收到 gossip 之后再扇给自己当时可达的邻居」这条路径从头到尾没被走过（**晚到的那一半现在由
// 下面的 `--round=gossip-late` 判**：中间人把窗口内转发过的信封在建链时重递一次） —— 那一半只有第三个实例能测。
//
// ⚠️ 边界一：这一格**证不了**"A 与 C 之间没有链路"（实测两次，都是红的）：
//   run-2026-09-26T09-20-42-139Z 与 run-2026-09-26T09-24-08-894Z 里，C 库内 `lan_enabled='false'`、
//   好友只有 B、端点只有 B，A 的日志仍然出现 `diag/announce_verified: from=<C 的 id>`
//   ⇒ **关掉局域网发现并没有停止广播，也没有停止接收侧的 announce 验证**，同机三实例必然互相建链。
//   所以拿"拓扑隔离"当前提会让这一轮常红。发现本身另立条目待拍板，不在测试任务里顺手改产品码。
//
// ★ 于是判据换成一条**机器可判定、且不依赖拓扑**的陈述：**A 的逐成员直发队列里从来没有面向 C 的行**
//   （见下面那条 `group_outbox ... peer_id=C` 必须 0 行）。这才是"C 收到的不是直发"的正身。
//
// ⚠️ 边界二：这一格判的是**多跳收敛**，不是"晚到成员补拉"（补拉那一半另有 `--round=gossip-late`）。
//   历史读数留档：实测（run-2026-09-26T09-24-08-894Z 与
//   同形的一次晚到构造）里让 C 在 A 发完之后才第一次上线 ⇒ 90 s 内 C 库里 0 行 ——
//   **那是 #77 落地之前的产品行为**，现在的形状是 B 除了"当时那一瞬间的扇出"之外，
//   还会在为新成员登记链路时把窗口内转发过的信封重递一次（有界：每组 16 条 / 10 分钟 / 30s 间隔）：
//   递不到补推。
//   那是产品行为，已按实测记进 roadmap 待拍板 —— 在这一轮里写成绿就是替产品许愿。
//   ⇒ 所以 **C 必须先于 A 起、且 B↔C 链路先建好再起 A**（2026-09-27：三端同时起会让这一格偶发红，
//     同一份二进制一次 91.2s 四条红、一次 1.1s 全绿 ⇒ 判据当时不可复现，改法见下面启动那一段）。
if (CHAIN) {
  step("链式三实例：A 从没直发给 C 的那条群消息，C 仍收敛到了（中间人在收到的一瞬间扇出）", async () => {
    // 上一步（L-B）收尾时 A/B 已被 stopAll 停干净 ⇒ 下面写的都是**停机库**。
    // C 第一次拉起只为自建身份与库，而且单独拉（新库会广播 announce，别让它在这个窗口里被别人学到）。
    launch(INST_C);
    await waitFor(() => tcpOpen(INST_C.port), 60_000, "C 首启（只为建身份与库）：TCP 可连");
    await waitFor(() => bootReady(INST_C.log, bootBaseOf.get(INST_C.n), BOOT_LINE),
      30_000, "C 首启：打出 boot 完成行");
    await stopAll(); // 此刻 procs 里只有 C —— stopAll 顺带保证"没清理干净"当场炸

    const idC = readIdentity(INST_C);
    const recvC = path.join(RUN_DIR, "recv", INST_C.label);
    fs.mkdirSync(recvC, { recursive: true });
    seed(INST_C.db, (db) => {
      db.prepare(
        `INSERT INTO friends(device_id,nickname,avatar,x25519_pubkey,ed25519_pubkey,added_at)
         VALUES(?1,?2,NULL,?3,NULL,?4)`,
      ).run(idB.runtimeId, `e2e-${INSTANCES[1].label}`, idB.x25519Pub, Date.now());
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints',?1)")
        .run(JSON.stringify([{ address: `127.0.0.1:${INSTANCES[1].port}` }]));
      // 陷阱：macOS 上 load() 优先信书签 ⇒ 只写路径（与 seedPair 同口径）
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('downloads_dir',?1)").run(recvC);
    });
    // B：把 C 追加进好友与端点。⚠️ 端点必须**先读回再追加** —— 直接覆盖会把 A 的端点清没，
    // 那样连 A-B 都断，整轮退化成「B 谁也没连上」的假红。
    seed(INSTANCES[1].db, (db) => {
      db.prepare(
        `INSERT OR IGNORE INTO friends(device_id,nickname,avatar,x25519_pubkey,ed25519_pubkey,added_at)
         VALUES(?1,?2,NULL,?3,NULL,?4)`,
      ).run(idC.runtimeId, `e2e-${INST_C.label}`, idC.x25519Pub, Date.now());
      const cur = db.prepare("SELECT value FROM settings WHERE key='routed_endpoints'").get();
      const eps = JSON.parse(cur?.value || "[]");
      const addr = `127.0.0.1:${INST_C.port}`;
      if (!eps.some((e) => e.address === addr)) eps.push({ address: addr });
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints',?1)")
        .run(JSON.stringify(eps));
    });
    // 三端各写一份群 + 同一份群密钥（逐列形状照群轮：content 存明文、receiver_id 是裸 group_id、
    // 初始 status='sent'、时钟一起推进否则撞 seq）。
    const members = [idA.runtimeId, idB.runtimeId, idC.runtimeId];
    const convId = `group:${GROUP_ID}`;
    const ts = nowMs();
    for (const inst of ALL_INST) {
      seed(inst.db, (db) => {
        db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)")
          .run(`gk:${GROUP_ID}`, GROUP_KEY_STR);
        db.prepare("INSERT OR REPLACE INTO groups(id,name,creator,created_at) VALUES(?1,?2,?3,?4)")
          .run(GROUP_ID, GROUP_NAME, idA.runtimeId, ts);
        db.prepare("DELETE FROM group_members WHERE group_id=?1").run(GROUP_ID);
        for (const m of members) {
          db.prepare("INSERT OR IGNORE INTO group_members(group_id,device_id) VALUES(?1,?2)")
            .run(GROUP_ID, m);
        }
        db.prepare(
          "INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at)"
          + " VALUES(?1,'group',?2,NULL,0,?3)",
        ).run(convId, GROUP_NAME, ts);
      });
    }
    const env = buildGroupEnvelope({
      groupKey: GROUP_KEY_B64, senderId: idA.runtimeId, priv: ed25519Priv(INSTANCES[0]),
      x25519Pub: idA.x25519Pub, ed25519Pub: idA.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: idA.runtimeId, members,
      kind: "text", content: CHAIN_TEXT, ts, seq: 1,
    });
    chainMsgId = env.messageId;
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM group_outbox WHERE group_id=?1").run(GROUP_ID);
      db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
      db.prepare(
        `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
         VALUES(?1,?2,?3,?4,'text',?5,?6,1,'sent')`,
      ).run(env.messageId, convId, idA.runtimeId, GROUP_ID, CHAIN_TEXT, ts);
      // 只给 B 一行：C 从始至终不是 A 的直发对象（这一条本身就是判据，见下面 aDb 那格）。
      db.prepare(
        `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
         VALUES(?1,?2,?3,?4,?5)`,
      ).run(env.messageId, GROUP_ID, idB.runtimeId, env.wire, ts);
      db.prepare("INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq").run(convId, 1);
    });

    // 三端同场起，但**时序有讲究**（2026-09-27 被一次实测推翻后改的）：
    // 先起 B 与 C、等 B↔C 链路真的建成，**最后**才起 A。
    // 为什么：中间人只在"收到的那一瞬间"把 gossip 扇给**当时可达**的邻居（这正是 #77 那一格的产品行为）。
    // 旧写法三端同时起 ⇒ A 的排队群消息可能在 C 的链路建好之前就到 B ⇒ B 无处可扇，C 永远收不到。
    // 现场：run-2026-09-26T18-33-35-042Z 里 C 侧四条断言全红、白等 91.2s，
    //      而同一份二进制换个启动时序 7/7 只花 1.1s（run-2026-09-26T18-37-54-706Z）
    //      ⇒ 这条判据当时**不可复现**，而"偶尔绿的门禁"比"没跑"更坏。
    // ⚠️ 改的只是**测试的同步点**，产品行为一字未动；晚到的邻居那一半现在由 #77 补递判（ #77。
    for (const inst of [INSTANCES[1], INST_C]) launch(inst);
    for (const inst of [INSTANCES[1], INST_C]) {
      await waitFor(() => tcpOpen(inst.port), 60_000, `链式轮：实例 ${inst.label} 的 TCP ${inst.port} 可连`);
      await waitFor(() => bootReady(inst.log, bootBaseOf.get(inst.n), BOOT_LINE), 30_000,
        `链式轮：实例 ${inst.label} 打出 boot 完成行`);
    }
    // 投递的同步点：C 必须先与 B 建成链路，否则"C 没收到"只是链路没建起来，判不到产品头上。
    await waitFor(() => countLog(INST_C.log, `建链 peer=${idB.runtimeId}`) > 0,
      60_000, "前置：C 与 B 先建成链路（这一步不过就不起 A）");
    launch(INSTANCES[0]);
    await waitFor(() => tcpOpen(INSTANCES[0].port), 60_000, "链式轮：A 的 TCP 可连");
    await waitFor(() => bootReady(INSTANCES[0].log, bootBaseOf.get(INSTANCES[0].n), BOOT_LINE),
      30_000, "链式轮：A 打出 boot 完成行");
    const bDb = openDb(INSTANCES[1].db, true);
    let bRows = [];
    try {
      const q = bDb.prepare("SELECT content,sender_id,conv_id FROM messages WHERE msg_id=?1");
      const until = nowMs() + 60_000;
      for (;;) {
        bRows = q.all(chainMsgId);
        if (bRows.length || nowMs() >= until) break;
        await sleep(1000);
      }
    } finally { bDb.close(); }
    check("前置：A 排的那条群消息先真到了 B（B 手里有过它，后面才谈得上转发）",
      bRows.length === 1 && bRows[0]?.content === CHAIN_TEXT,
      `1 行 / ${CHAIN_TEXT}`, `${bRows.length} 行 / ${bRows[0]?.content}`);

    // （B↔C 链路这一前置已经上移到"起 A 之前"，这里不再等第二次。）
    // 反向模式（§十四「错误行为测试」）：上面全部照跑，只把判据要去找的那个 msg_id 换成必定不存在的值
    // ⇒ 报不出红就说明下面几条读的不是真落库行。
    const judgedId = CHAIN_LIE ? noteId(`${chainMsgId}-lie`) : chainMsgId;
    const cDb = openDb(INST_C.db, true);
    let rows = [];
    try {
      const q = cDb.prepare("SELECT content,seq,sender_id,conv_id FROM messages WHERE msg_id=?1");
      const until = nowMs() + 90_000;
      for (;;) {
        rows = q.all(judgedId);
        if (rows.length || nowMs() >= until) break;
        await sleep(1000);
      }
    } finally { cDb.close(); } // 句柄只开一次：这条循环最多读 90 遍，每遍重开会放大 BUSY 概率

    check("★ C 的库里落了那条消息，且只有一行（A 从没直发给 C ⇒ 只能是中间人的 gossip 收敛）",
      rows.length === 1, 1, rows.length);
    check("C 侧解出的是明文正文（解密发生在 C 自己身上，不是谁代解后送明文）",
      rows[0]?.content === CHAIN_TEXT, CHAIN_TEXT, rows[0]?.content);
    check("C 侧记的发送者仍是 A（经手不改归属）",
      rows[0]?.sender_id === idA.runtimeId, idA.runtimeId, rows[0]?.sender_id);
    check("C 侧落在群会话、seq 与信封一致",
      rows[0]?.conv_id === convId && rows[0]?.seq === 1, `${convId}/seq=1`,
      `${rows[0]?.conv_id}/seq=${rows[0]?.seq}`);
    const aDb = openDb(INSTANCES[0].db, true);
    let outPeers = [];
    try {
      outPeers = aDb.prepare("SELECT peer_id FROM group_outbox WHERE msg_id=?1 AND peer_id=?2")
        .all(chainMsgId, idC.runtimeId).map((r) => r.peer_id);
    } finally { aDb.close(); }
    check("A 的逐成员直发队列里**没有任何面向 C 的行**（C 从来不是 A 的直发对象）",
      outPeers.length === 0, 0, JSON.stringify(outPeers));
    const bAgain = openDb(INSTANCES[1].db, true);
    let bCount = -1;
    try {
      bCount = bAgain.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(chainMsgId).c;
    } finally { bAgain.close(); }
    check("B 自己仍只有一行（把消息递给下游不会让自己重复落库）", bCount, 1, bCount);

    // C 的日志与库进产物：报告按 label 走（#74 之后判据是形状匹配），第三实例的现场不能只留在 appdata。
    try {
      fs.copyFileSync(INST_C.log, path.join(RUN_DIR, `instance-${INST_C.label}.app.log`));
      fs.mkdirSync(path.join(RUN_DIR, `sqlite-${INST_C.label}`), { recursive: true });
      for (const s of ["", "-wal", "-shm"]) {
        if (fs.existsSync(INST_C.db + s)) {
          fs.copyFileSync(INST_C.db + s, path.join(RUN_DIR, `sqlite-${INST_C.label}`, path.basename(INST_C.db + s)));
        }
      }
    } catch { /* 产物复制失败不改判定（判定只看库与日志里的真事实） */ }
  });
}


// ── #77：晚到成员的补递轮 ────────────────────────────────────────────
// 这一轮证明的是"以后还能补到"，与链式轮的"当时能扇到"是两条独立的判据（拓扑相同、时序相反）。
if (LATE) {
  step("晚到成员补递：A 在 C 上线之前就发完，C 与中间人建链之后仍拿到了那条（#77）", async () => {
    // C 第一次拉起只为自建身份与库，而且单独拉（新库会广播 announce，别让它在这个窗口里被别人学到）。
    launch(INST_C);
    await waitFor(() => tcpOpen(INST_C.port), 60_000, "C 首启（只为建身份与库）：TCP 可连");
    await waitFor(() => bootReady(INST_C.log, bootBaseOf.get(INST_C.n), BOOT_LINE),
      30_000, "C 首启：打出 boot 完成行");
    await stopAll();

    const idC = readIdentity(INST_C);
    const recvC = path.join(RUN_DIR, "recv", INST_C.label);
    fs.mkdirSync(recvC, { recursive: true });
    seed(INST_C.db, (db) => {
      db.prepare(
        `INSERT INTO friends(device_id,nickname,avatar,x25519_pubkey,ed25519_pubkey,added_at)
         VALUES(?1,?2,NULL,?3,NULL,?4)`,
      ).run(idB.runtimeId, `e2e-${INSTANCES[1].label}`, idB.x25519Pub, Date.now());
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints',?1)")
        .run(JSON.stringify([{ address: `127.0.0.1:${INSTANCES[1].port}` }]));
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('downloads_dir',?1)").run(recvC);
    });
    // B：把 C 追加进好友与端点。⚠️ 端点必须**先读回再追加**（同链式轮）—— 直接覆盖会把 A 的端点清没。
    seed(INSTANCES[1].db, (db) => {
      db.prepare(
        `INSERT OR IGNORE INTO friends(device_id,nickname,avatar,x25519_pubkey,ed25519_pubkey,added_at)
         VALUES(?1,?2,NULL,?3,NULL,?4)`,
      ).run(idC.runtimeId, `e2e-${INST_C.label}`, idC.x25519Pub, Date.now());
      const cur = db.prepare("SELECT value FROM settings WHERE key='routed_endpoints'").get();
      const eps = JSON.parse(cur?.value || "[]");
      const addr = `127.0.0.1:${INST_C.port}`;
      if (!eps.some((e) => e.address === addr)) eps.push({ address: addr });
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints',?1)")
        .run(JSON.stringify(eps));
    });
    const members = [idA.runtimeId, idB.runtimeId, idC.runtimeId];
    const convId = `group:${GROUP_ID}`;
    const ts = nowMs();
    for (const inst of ALL_INST) {
      seed(inst.db, (db) => {
        db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)")
          .run(`gk:${GROUP_ID}`, GROUP_KEY_STR);
        db.prepare("INSERT OR REPLACE INTO groups(id,name,creator,created_at) VALUES(?1,?2,?3,?4)")
          .run(GROUP_ID, GROUP_NAME, idA.runtimeId, ts);
        db.prepare("DELETE FROM group_members WHERE group_id=?1").run(GROUP_ID);
        for (const m of members) {
          db.prepare("INSERT OR IGNORE INTO group_members(group_id,device_id) VALUES(?1,?2)")
            .run(GROUP_ID, m);
        }
        db.prepare(
          "INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at)"
          + " VALUES(?1,'group',?2,NULL,0,?3)",
        ).run(convId, GROUP_NAME, ts);
      });
    }
    const env = buildGroupEnvelope({
      groupKey: GROUP_KEY_B64, senderId: idA.runtimeId, priv: ed25519Priv(INSTANCES[0]),
      x25519Pub: idA.x25519Pub, ed25519Pub: idA.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: idA.runtimeId, members,
      kind: "text", content: LATE_TEXT, ts, seq: 1,
    });
    const lateMsgId = env.messageId;
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM group_outbox WHERE group_id=?1").run(GROUP_ID);
      db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
      db.prepare(
        `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
         VALUES(?1,?2,?3,?4,'text',?5,?6,1,'sent')`,
      ).run(env.messageId, convId, idA.runtimeId, GROUP_ID, LATE_TEXT, ts);
      // 只给 B 一行：C 从始至终不是 A 的直发对象（下面 A 侧那一格钉的就是这个）。
      db.prepare(
        `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
         VALUES(?1,?2,?3,?4,?5)`,
      ).run(env.messageId, GROUP_ID, idB.runtimeId, env.wire, ts);
      db.prepare("INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq").run(convId, 1);
    });

    // ★ 时序：先起 A 与 B、让那条**走完 A→B**，最后才起 C。
    // 反过来（三端同场）就退化成链式轮 —— 那一刻 C 的链路已经在了，"补递"根本没有发生的必要。
    for (const inst of [INSTANCES[0], INSTANCES[1]]) launch(inst);
    for (const inst of [INSTANCES[0], INSTANCES[1]]) {
      await waitFor(() => tcpOpen(inst.port), 60_000, `补递轮：实例 ${inst.label} 的 TCP ${inst.port} 可连`);
      await waitFor(() => bootReady(inst.log, bootBaseOf.get(inst.n), BOOT_LINE), 30_000,
        `补递轮：实例 ${inst.label} 打出 boot 完成行`);
    }
    const bDb = openDb(INSTANCES[1].db, true);
    let bRows = [];
    try {
      const q = bDb.prepare("SELECT content,sender_id FROM messages WHERE msg_id=?1");
      const until = nowMs() + 60_000;
      for (;;) {
        bRows = q.all(lateMsgId);
        if (bRows.length || nowMs() >= until) break;
        await sleep(1000);
      }
    } finally { bDb.close(); }
    check("前置：C 还没上线，那条群消息就已经落在 B 的库里（B 手里有过它，才谈得上'以后补'）",
      bRows.length === 1 && bRows[0]?.content === LATE_TEXT,
      `1 行 / ${LATE_TEXT}`, `${bRows.length} 行 / ${bRows[0]?.content}`);

    launch(INST_C);
    await waitFor(() => tcpOpen(INST_C.port), 60_000, "补递轮：C 上线后 TCP 可连");
    await waitFor(() => bootReady(INST_C.log, bootBaseOf.get(INST_C.n), BOOT_LINE),
      30_000, "补递轮：C 打出 boot 完成行");
    // 补递的触发点 = B 为 C 登记链路的那一刻（C 拨 B ⇒ B 侧走入站 accept）。
    await waitFor(() => countLog(INST_C.log, `建链 peer=${idB.runtimeId}`) > 0,
      60_000, "前置：C 与 B 建成链路（这一步不过就没有补递的触发点）");
    // ⚠️ 这里**不设断言**（2026-09-27 自己抓到的一条 flaky）：曾经写成
    //   "C 侧从来没有与 A 的建链行" ⇒ 判 `countLog(...) === 0`。同一台机器上三实例是**能**经局域网
    //   互相发现的（这条边界早就写在链式轮那一格的 why 里：同机造不出"A-C 无链路"），
    //   所以那个 0 只是"A 还没轮到拨 C"的瞬时读数 —— 采到的那一刻是 0、下一次跑就是 1
    //   （实测：run-…20-53 里 1.7s 报红"实际 1"，而 run-…20-16 与 20-39 两次都是 0）。
    //   ⇒ 拿"某一瞬间没发生"当判据 = 竞态判据。归因不靠它也能立：
    //     ① A 的逐成员直发队列里没有面向 C 的行（下面那条断言）⇒ 这一帧不是 A 直发的；
    //     ② B 的日志里有指向 C 的补递行 ⇒ 这一帧是 B 补的。
    //   读数继续打印，进报告产物，只是不当判据。
    console.log(`     · [只记录，不判] C 侧与 A 的建链行数=${countLog(INST_C.log, `建链 peer=${idA.runtimeId}`)}`
      + "（同机局域网能互达 ⇒ 这个数不是判据，见上面注释）");

    // 归因这一格：C 收到这一条**只能**来自补递 —— B 的日志里那一行是本机自己打的，
    // 没有它就没有"哪条路径递的"这个问题的答案（链式轮那格证明不了这一轮，反之亦然）。
    let replayLines = 0;
    {
      const until = nowMs() + 30_000;
      for (;;) {
        replayLines = countLog(INSTANCES[1].log, `补递群消息 peer=${idC.runtimeId}`);
        if (replayLines > 0 || nowMs() >= until) break;
        await sleep(1000);
      }
    }
    check("★ B 的日志里出现补递那一行，且指向的正是刚上线的 C（证明这一条是「以后补的」而不是「当时扇的」）",
      replayLines > 0, "至少 1 行 peer=C 的补递", `${replayLines} 行`);

    const judgedId = LATE_LIE ? noteId(`${lateMsgId}-lie`) : lateMsgId;
    const cDb = openDb(INST_C.db, true);
    let rows = [];
    try {
      const q = cDb.prepare("SELECT content,seq,sender_id,conv_id FROM messages WHERE msg_id=?1");
      const until = nowMs() + 90_000;
      for (;;) {
        rows = q.all(judgedId);
        if (rows.length || nowMs() >= until) break;
        await sleep(1000);
      }
    } finally { cDb.close(); } // 句柄只开一次：这条循环最多读 90 遍，每遍重开会放大 BUSY 概率
    check("★ C 的库里落了那条消息，且只有一行（它上线时 A 早发完了 ⇒ 只可能是中间人补的）",
      rows.length === 1, 1, rows.length);
    check("C 侧解出明文正文、发送者仍是 A、落在群会话且 seq 与信封一致",
      rows[0]?.content === LATE_TEXT && rows[0]?.sender_id === idA.runtimeId
      && rows[0]?.conv_id === convId && rows[0]?.seq === 1,
      `${LATE_TEXT} / A / ${convId} / seq=1`,
      `${rows[0]?.content} / ${rows[0]?.sender_id} / ${rows[0]?.conv_id} / seq=${rows[0]?.seq}`);
    const aDb = openDb(INSTANCES[0].db, true);
    let outPeers = [];
    try {
      outPeers = aDb.prepare("SELECT peer_id FROM group_outbox WHERE msg_id=?1 AND peer_id=?2")
        .all(lateMsgId, idC.runtimeId).map((r) => r.peer_id);
    } finally { aDb.close(); }
    check("A 的逐成员直发队列里**没有任何面向 C 的行**（C 从来不是 A 的直发对象）",
      outPeers.length === 0, 0, JSON.stringify(outPeers));

    try {
      fs.copyFileSync(INST_C.log, path.join(RUN_DIR, `instance-${INST_C.label}.app.log`));
      fs.mkdirSync(path.join(RUN_DIR, `sqlite-${INST_C.label}`), { recursive: true });
      for (const suffix of ["", "-wal", "-shm"]) {
        if (fs.existsSync(INST_C.db + suffix)) {
          fs.copyFileSync(INST_C.db + suffix,
            path.join(RUN_DIR, `sqlite-${INST_C.label}`, path.basename(INST_C.db + suffix)));
        }
      }
    } catch { /* 产物复制失败不改判定（判定只看库与日志里的真事实） */ }
  });
}

// ── #89：局域网开关的跨实例隔离轮 ───────────────────────────────────
// 三条腿：开着先学到 → 关掉之后学不到（核心）→ 翻回来又学得到（正向对照，证明中间那条不是空转）。
// 观察者 A **全程不重启**：如果 A 也被停掉，"计数不涨"就变成"没人再看"的同义反复（那条假判据的形状）。
if (LANOFF) {
  step("局域网开关：关掉之后对端学不到我，翻回来必须重新学得到（env 不许替用户表态）", async () => {
    /// 不是抛错的 waitFor：这一轮的"等不到"必须是**断言红**，不是步骤崩。
    const within = async (fn, ms) => {
      const until = nowMs() + ms;
      for (;;) {
        if (fn()) return true;
        if (nowMs() >= until) return false;
        await sleep(1000);
      }
    };
    const B = INSTANCES[1];
    const learnedNeedle = (id) => `announce_verified: from=${id}`;
    const seedLan = (on) => seed(B.db, (db) => {
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('lan_enabled',?1)")
        .run(on ? "1" : "0");
    });
    const bootOf = async (inst) => {
      await within(() => bootReady(inst.log, bootBaseOf.get(inst.n), BOOT_LINE), 30_000);
    };

    // 上一轮（默认旅程）收尾时 A/B 已被 stopAll 停干净 ⇒ 这里自己起。
    launch(INSTANCES[0]);
    launch(B);
    await bootOf(INSTANCES[0]);
    await bootOf(B);
    // ① 前置：两边都开着（L-A 预置显式写了 lan_enabled='true'）时，A 要真学到过 B。
    const sawFirst = await within(() => countLog(INSTANCES[0].log, learnedNeedle(idB.runtimeId)) > 0, 45_000);
    check("前置：B 开着的时候 A 的日志里出现过它的 announce（否则下面那条「不涨」没有对照物）",
      sawFirst, ">0 次", countLog(INSTANCES[0].log, learnedNeedle(idB.runtimeId)));

    // ② 停机把 B 的键显式写成"关"，再带着 GOSSLAN_AUTOSTART=1 起它 —— 这一轮要的就是这一对。
    await stopOne(B);
    seedLan(false);
    launch(B);
    await bootOf(B);
    check("关掉后 B 自己没起网：它的日志里 discovery_started 计数为 0",
      countLog(B.log, "discovery_started") === 0, 0, countLog(B.log, "discovery_started"));

    // 关着的那段窗口里先把监听口的读数取走（**只打印**，理由见文件末尾那三行对照的注释）
    const listenerPids = () => {
      const r = spawnSync("lsof", ["-nP", "-tiTCP:" + B.port, "-sTCP:LISTEN"], { encoding: "utf8" });
      return (r.stdout || "").trim().split("\n").filter(Boolean);
    };
    const psOf = (pid) => pid
      ? (spawnSync("ps", ["-o", "pid=,comm=,lstart=", "-p", String(pid)], { encoding: "utf8" }).stdout.trim()
         || "(ps 查不到这个 pid)")
      : "(没有 pid)";
    const openWhileOff = await tcpOpen(B.port);
    const whoHolds = listenerPids();
    check("★ 关掉之后连 TCP 监听口都不存在（别人拨不进来，不只是「我不再广播」）",
      openWhileOff === false, "连不上（false）",
      `连得上=${openWhileOff}；监听者=[${whoHolds.map(psOf).join(" | ")}]；`
      + `本轮 launch 起的 B 的 pid=${procs.get(B.n)?.pid}
       ⚠️ 这条 2026-09-27 凌晨先被判成"红"过一次，原因是**探针放错窗口**：那一刻量的是
       键翻回「开」之后重起的那个 B（日志尾巴上还留着它的 bc_directed 与 routed 建链），
       于是三个读数互相矛盾。挪进关着的那段窗口之后：监听者列表是空的。
       ⇒ 教训是「归因不清」和「探针读错了对象」长得一模一样，区别只在有没有把读数钉在事件上。`);


    // ③ 核心：A 活着且一直在听，两个广播周期内"学到 B"的次数一字不涨。
    const base = countLog(INSTANCES[0].log, learnedNeedle(idB.runtimeId));
    await sleep(26_000);
    const after = countLog(INSTANCES[0].log, learnedNeedle(idB.runtimeId));
    // 反向模式：注入、时序、读的东西全都一样，**只把这一条的期望翻成"该涨"**
    // ⇒ 产品没错时它必须红；报不出红就说明这条读的不是真日志行。
    check("★ 关掉之后 A 再也学不到 B（announce 计数一字不涨；env 没能把它偷偷打开）",
      LANOFF_LIE ? after > base : after === base,
      LANOFF_LIE ? "> 基线（这是反向模式的期望，正常应当红）" : base, after);

    // ④ 正向对照：翻回"开"并重启 B ⇒ 同一套读法必须立刻重新数得到 announce。
    //    没有这一条，第 ③ 条可以由"A 瞎了/日志格式变了/needle 拼错"来冒充成功。
    await stopOne(B);
    seedLan(true);
    launch(B);
    await bootOf(B);
    const grewBack = await within(
      () => countLog(INSTANCES[0].log, learnedNeedle(idB.runtimeId)) > after, 45_000);
    check("对照：把键翻回「开」并重启 B，A 的 announce 计数重新开始涨（证明第 ③ 条不是空转）",
      grewBack, "> 上一段读数", countLog(INSTANCES[0].log, learnedNeedle(idB.runtimeId)));
    check("对照：翻回「开」之后 B 自己的日志里 discovery_started 又出现（与上面那个 0 成对）",
      countLog(B.log, "discovery_started") > 0, ">0", countLog(B.log, "discovery_started"));
    // #95 的探针与它的两条对照（三条读数全设断言：①开=连得上、②闲口=连不上、③关=连不上）。
    // ②是①③的非空转证明；①是③的对照物 —— 缺任何一条，剩下的那条都可能是半个守卫。
    //   ① 开着的时候监听口在（正向，设断言）；
    //   ② 谁也没占的那个口连不上（探针自己的负对照，设断言 —— 没有它，①与③都是半个守卫：
    //      一个永远回 true 的 `tcpOpen` 能让①假绿、让③"看起来红在环境"）；
    //   ③ 键为关时监听口不在（这一条才是 #95 要的答案）。
    //   ⚠️ 它今晚**先被判错过一次**：探针最初放在"翻回开、重起 B"之后，量的是开着的那个进程，
    //   于是出现"③=true 但 discovery_started=0、日志里还有 bc_directed 与 routed 建链"这种
    //   三个读数互不相容的假矛盾；把读数挪回关着的那段窗口，监听者列表就是空的。
    //   ⇒ 「归因不清」与「探针读错了对象」在报告里长得一模一样，区别只有读数钉没钉在事件上。
    const openWhenOn = await tcpOpen(B.port);
    check("对照：翻回「开」之后 B 的 TCP 监听口连得上（这个键开着时端口确实在听）",
      openWhenOn === true, "连得上（true）", String(openWhenOn));
    // 探针的负对照：挑一个本轮任何实例都不用的口，必须连不上。
    const probeControl = await tcpOpen(65501);
    check("对照：`tcpOpen` 读得出不存在的口（探针自己不是恒真机 —— 上面两条读数因此才算数）",
      probeControl === false, "连不上（false）", String(probeControl));
  });
}

// ── 主流程 ─────────────────────────────────────────────────────────
// §十六 报告契约：把「报告至少显示」那几条点名翻译成对**产物**的判据，不是对源码字面量的存在性检查。
// 判据只吃一个已经落盘的 summary.json —— 所以「改坏报告生成器」和「手工改坏一份报告」走的是同一条判据。
function reportContractGaps(s) {
  /// 本轮铸出来的 trace id 的**形状**（`e2e-` + 可选单字母段 + 本轮 ISO + 尾巴）。
  /// 按形状认，不按变量名点名 —— 理由同 `traceExcerpt()`：手写名单会漏新轮次。
  /// 写成函数内的字面量而不是模块级 `const`：`selfcheckReportContract()` 在文件**第 82 行**就被调用，
  /// 模块级常量那时还没初始化（TDZ 会直接抛，第一次跑就把整层判据打挂）。
  const RUN_ID_RE = /e2e-(?:[a-z]-)?\d{4}-\d{2}-\d{2}T[\d-]+Z-[A-Za-z0-9-]+/g;
  const gaps = [];
  if (!s || typeof s !== "object") return ["报告不是一个对象"];
  if (!["PASS", "FAIL"].includes(s.verdict)) gaps.push("总 verdict 不是 PASS/FAIL");
  if (!s.trace || !("msg_id" in s.trace) || !("transfer_id" in s.trace))
    gaps.push("缺 trace 里的 msg_id / transfer_id（§十六 要求这两个 id 贯穿整轮）");
  // ★ §十六 的另一半：一轮里**每一单**都要有自己那条 trace。多文件轮同时有 3 个 transfer_id，
  // 只报一个标量就等于"报告说这轮只发过一单"。所以判：**报告里出现的每个本形状 id，都必须已登记**。
  // 不登记的那一格会在追故障时凭空消失 —— 而它恰好就是"报告指到错的那一单"的形状。
  if (!Array.isArray(s.trace?.ids)) {
    gaps.push("trace.ids 不是数组（本轮造过的每一单都要在报告里数得出来）");
  } else {
    const reg = new Set(s.trace.ids);
    const hay = JSON.stringify({ ...s, trace: { ...(s.trace ?? {}), ids: [] } });
    for (const id of new Set(hay.match(RUN_ID_RE) ?? [])) {
      if (!reg.has(id)) gaps.push(`报告里出现没登记进 trace.ids 的 id：${id}`);
    }
  }
  if (typeof s.duration_s !== "number") gaps.push("缺总耗时 duration_s");
  if (!Array.isArray(s.steps) || !s.steps.length) gaps.push("缺「步骤」表");
  for (const st of s.steps ?? []) {
    if (!st.name) gaps.push("有条步骤连名字都没有 —— 报告读不出这是哪个功能");
    if (!["PASS", "FAIL", "NO-ASSERT", "NOT-RUN"].includes(st.verdict))
      gaps.push(`步骤「${String(st.name ?? "?").slice(0, 24)}」没有终态 verdict`);
  }
  if (!Array.isArray(s.assertions) || !s.assertions.length) gaps.push("缺「预期/实际」账本（assertions 为空）");
  for (const a of s.assertions ?? []) {
    if (!a.step || !a.name || !("expect" in a) || !("actual" in a) || !["PASS", "FAIL"].includes(a.verdict)) {
      gaps.push(`有条断言缺 步骤/预期/实际/PASS-FAIL 之一：${JSON.stringify(a).slice(0, 90)}`);
      break;
    }
  }
  const failSteps = (s.steps ?? []).filter((x) => x.verdict === "FAIL");
  if (s.verdict === "FAIL" && failSteps.length && !failSteps.some((x) => x.logs && Object.keys(x.logs).length))
    gaps.push("报了 FAIL 却没有一步带「日志关联」—— §十六 要求失败能追到实例日志");
  return gaps;
}
function readReportContract(dir) {
  const p = path.join(dir, "summary.json");
  if (!fs.existsSync(p)) return [`没有 ${p}`];
  if (!fs.existsSync(path.join(dir, "summary.html"))) return ["§十六 要求 summary.html，但没落盘"];
  let s;
  try { s = JSON.parse(fs.readFileSync(p, "utf8")); } catch (e) { return [`summary.json 解析失败：${e.message}`]; }
  return reportContractGaps(s);
}
// 步骤徽章：以前是写在模板里的三元式，「没有 verdict」直接落到 else ⇒ 没跑的步骤显示成 ✅。
// 报告把没跑标成通过，和被它标成通过的那些格一起算进覆盖度 —— 这正是 §十 禁止的「把没有测试伪装成 PASS」。
function stepBadge(v) {
  return v === "FAIL" ? "❌"
    : v === "NO-ASSERT" ? "⚠️"
    : v == null || v === "NOT-RUN" ? "⛔ 未跑"
    : "✅";
}
function selfcheckReportContract() {
  const base = () => JSON.parse(JSON.stringify({
    verdict: "PASS", duration_s: 1.2,
    trace: { msg_id: "m1", transfer_id: "t1", ids: ["e2e-2026-01-01T00-00-00-000Z-aaaaaa"] },
    steps: [{ name: "跑通的一步", ms: 1, verdict: "PASS", checks: 1 }, { name: "没跑到的步骤", verdict: "NOT-RUN" }],
    assertions: [{ step: "跑通的一步", name: "判据", expect: 1, actual: "e2e-2026-01-01T00-00-00-000Z-aaaaaa", verdict: "PASS" }],
  }));
  const mut = (f) => { const c = base(); f(c); return c; };
  const cases = [
    ["真：字段齐全判得出合格", reportContractGaps(base()), 0],
    ["假：缺 msg_id 判得出", reportContractGaps(mut((c) => delete c.trace.msg_id)), 1],
    ["假：trace.ids 不是数组判得出", reportContractGaps(mut((c) => delete c.trace.ids)), 1],
    // ★ 这一条是"本轮有一单没被登记进 trace"的形状：断言里出现了本形状 id，但 trace.ids 里没有它。
    ["假：报告里出现没登记的 id 判得出",
      reportContractGaps(mut((c) => { c.trace.ids = []; })), 1],
    ["真：多单全部登记判得出合格",
      reportContractGaps(mut((c) => {
        c.trace.ids = ["e2e-2026-01-01T00-00-00-000Z-aaaaaa", "e2e-x-2026-01-01T00-00-00-000Z-bbbbbb"];
        c.assertions.push({ step: "跑通的一步", name: "第二单", expect: 1, actual: "e2e-x-2026-01-01T00-00-00-000Z-bbbbbb", verdict: "PASS" });
      })), 0],
    ["假：缺耗时判得出", reportContractGaps(mut((c) => delete c.duration_s)), 1],
    ["假：步骤没有终态判得出", reportContractGaps(mut((c) => delete c.steps[1].verdict)), 1],
    ["假：断言少了「实际值」判得出", reportContractGaps(mut((c) => delete c.assertions[0].actual)), 1],
    ["假：报 FAIL 却没日志关联判得出", reportContractGaps(mut((c) => { c.verdict = "FAIL"; c.steps[0].verdict = "FAIL"; })), 1],
    ["真：FAIL 且带日志关联判得出合格", reportContractGaps(mut((c) => {
      c.verdict = "FAIL"; c.steps[0].verdict = "FAIL"; c.steps[0].logs = { A: ["一行日志"] };
    })), 0],
    ["假：断言账本整体为空判得出", reportContractGaps(mut((c) => { c.assertions = []; })), 1],
  ];
  const fails = [];
  for (const [name, got, want] of cases)
    if (got.length !== want) fails.push(`${name} —— 期望判出 ${want} 条，实际 ${got.length} 条${got.length ? `（首条：${got[0].slice(0, 50)}）` : ""}`);
  const badges = [
    ["PASS 才是 ✅", stepBadge("PASS") === "✅"],
    ["没终态不许是 ✅", stepBadge(undefined) !== "✅"],
    ["NOT-RUN 要写明未跑", stepBadge("NOT-RUN").includes("未跑")],
  ];
  for (const [name, ok] of badges) if (!ok) fails.push(`步骤徽章：${name} 不成立`);
  return { fails, notes: `${cases.length + badges.length} 格` };
}
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
    instances: INSTANCES.map((i) => ({ label: i.label, n: i.n, port: i.port, runtimeId: (i.n === 1 ? idA : idB)?.runtimeId })),
    trace: { msg_id: msgId ?? null, transfer_id: xferId ?? null, ids: [...new Set(MINTED_IDS)] },
    shots: shotFiles.filter(Boolean).map((f) => path.relative(RUN_DIR, f)),
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
<p>二进制 <code>${BIN}</code> · ${process.platform} · 总耗时 ${totalS}s · msg_id <code>${msgId ?? "-"}</code> · transfer_id <code>${xferId ?? "-"}</code></p>
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
  ? sum.shots.map((rel) => `<p><code>${rel}</code></p><img src="${rel}" width="1000" alt="${esc(rel)}">`).join("\n")
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
  [idA, idB] = INSTANCES.map(readIdentity);
  NODES = INSTANCES.map((inst, i) => ({ ...[idA, idB][i], label: inst.label, port: inst.port }));
  console.log(`  A=${idA.runtimeId}\n  B=${idB.runtimeId}`);
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
  const deliveredFail = NEGATIVE && curStepIdx > linkStepIdx();
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
  // 删目录不可逆，所以三条硬约束：① 先自证（每格只换一个输入），自证不过 ⇒ 这一趟一个都不删并把本轮判红；
  // ② 只允许碰 `test-results/run-*`，且**跳过本轮自己**；③ 想多留用 GOSSLAN_KEEP_RUNS 调大上限。
  const KEEP_GREEN_RUNS = Number(process.env.GOSSLAN_KEEP_RUNS || 30);
  function prunePlan({ runs, keep, negative }) {
    if (negative) return [];
    if (!Number.isFinite(keep) || keep < 0) return [];
    const greens = runs
      .filter((r) => r.outcome === "green")
      .sort((a, b) => (a.name < b.name ? 1 : -1)); // 目录名是 ISO 时间戳 ⇒ 字典序倒排 = 新的在前
    // 返回**按名字升序**（= 从最老的删起），让调用侧与自证都不依赖 sort 的方向
    return greens.slice(keep).map((r) => r.name).sort();
  }
  /** 自证：每格只换一个输入。不这么写的话"上限生效"可以只是"绿轮恰好都被留着"。 */
  function selfcheckPrune() {
    const fails = [];
    const eq = (name, got, want) => {
      const a = JSON.stringify(got), b = JSON.stringify(want);
      if (a !== b) fails.push(`${name}：预期 ${b} / 实际 ${a}`);
    };
    const mk = (tag, outcome) => (outcome ? { name: "run-" + tag, outcome } : { name: "run-" + tag });
    const greens = [mk("a", "green"), mk("b", "green"), mk("c", "green")]; // c 最新
    eq("绿轮超上限 ⇒ 删最老的那几个", prunePlan({ runs: greens, keep: 1, negative: false }), ["run-a", "run-b"]);
    eq("没超上限 ⇒ 一个都不删", prunePlan({ runs: greens, keep: 9, negative: false }), []);
    eq("红轮永远保留（哪怕上限 0）", prunePlan({ runs: [mk("x", "green"), mk("y", "red")], keep: 0, negative: false }), ["run-x"]);
    eq("跑不出 summary ⇒ 按红处理", prunePlan({ runs: [mk("z", "unknown")], keep: 0, negative: false }), []);
    eq("反向模式 ⇒ 一趟都不删", prunePlan({ runs: greens, keep: 1, negative: true }), []);
    eq("上限写坏了（NaN/负数）⇒ 不删", prunePlan({ runs: greens, keep: Number.NaN, negative: false }), []);
    return fails;
  }
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
  function pruneOldRuns() {
    const root = path.join(ROOT, "test-results");
    if (!fs.existsSync(root)) return { deleted: [], bytes: 0, left: 0 };
    const cur = path.basename(RUN_DIR);
    const runs = fs.readdirSync(root, { withFileTypes: true })
      .filter((e) => e.isDirectory() && e.name.startsWith("run-") && e.name !== cur)
      .map((e) => ({ name: e.name, outcome: classifyRun(path.join(root, e.name)) }));
    const doomed = prunePlan({ runs, keep: KEEP_GREEN_RUNS, negative: NEGATIVE });
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
    return { deleted: gone, bytes, left };
  }
  const pruneFails = selfcheckPrune();
  if (pruneFails.length) {
    console.error(`✗ 跨轮保留自证不成立 ⇒ 旧轮次一律不删（宁可留一堆，也不能删错）：\n  ${pruneFails.join("\n  ")}`);
    REPORT_GAPS.push(`跨轮保留自证不成立：${pruneFails.join(" / ")}`);
  } else {
    const pr = pruneOldRuns();
    console.log(`跨轮保留：绿轮上限 ${KEEP_GREEN_RUNS} ⇒ 删最老的绿轮 ${pr.deleted.length} 个` +
      `（释放 ${(pr.bytes / 1024 / 1024).toFixed(1)} MB），现存 ${pr.left} 个 run-*；` +
      `红轮与没有 summary.json 的轮次一个都不删（GOSSLAN_KEEP_RUNS 可调上限）`);
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
