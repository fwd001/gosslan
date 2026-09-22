# 按「最终设计目标」做的全面审计 + 方案计划（2026-09-22）

> 判据来源：用户 2026-09-22 定案的优先级序
> **稳定 > 流畅 > 正确 > 原生体验 > 安全 > 可维护性 > 扩展性 > 新功能**，
> 以及同批给出的两条约束：**当前阶段不新增任何功能**、**允许重构核心架构（"最小改动"不再是最高原则）**。
>
> 取证方式：下面每一条结论都由我本人 `sed`/`grep` 复跑过原文并给出 `文件:行号` + 逐字引用。
> 并行子审计给出的、我复跑不成立的结论已删除；只成立但推断的部分标 `?`。§11 列出这次**没验到**的边界。
>
> 一句话结论：**最危险的三件事不是"缺功能"，而是三种"看起来正常"** ——
> ① 中继文件推送的进度和"完成"是本地循环计数推出来的（A1）；
> ② 安全码可能是在核对一把从未加密过任何字节的钥匙（E1）；
> ③ 一堆"用户操作被后台拖住"的根因是同一把全局 db 锁（B1）。
> 这三类都不需要新架构就能修，但都需要**先承认口径错了**才能修对。

---

# 第一部分：审计

## 0. 先解决目标本身：同一个项目里有三套优先级

这不是一条"文档洁癖"，它直接决定后面每一处取舍。当前并存：

| 位置 | 声明 | 与新目标的关系 |
|---|---|---|
| `AI_RULES.md:6` | `Primary goal: 稳定 · 速度 · 可达 · 安全 · 可解释（见 §1）` | 缺"正确""原生体验"，"可达"未归位 |
| `AI_RULES.md:22-34` | 五档竖排 `稳定 ↓ 速度 ↓ 可达 ↓ 安全 ↓ 可解释` | 同上 |
| `docs/acceptance/1.0-release.md:13` | `四条验收维度，按此顺序取舍：稳定 → 速度 → 可达 → 安全` | **少了"可解释"**（而同一文件 14 行又把它作为横切要求加回来） |
| 本次指令 | `稳定 > 流畅 > 正确 > 原生体验 > 安全 > 可维护性 > 扩展性 > 新功能` | 新增"正确""原生体验"两档，删掉"可达"这一独立档 |

还有一条**正面冲突**：`docs/acceptance/1.0-release.md:62` 写着
「判断根因，做**最小**修复（改不动就报告，别自己扩大方案）」，
而新目标是「允许重构核心架构，不以最小改动为最高原则…如果当前实现从根上就是错误的，应优先修正设计」。
按 `acceptance:4` 自述的"权威版本是 AI_RULES §1/§2/§3"，这句话必须跟着改，否则**下一个 AI 每次开工都会先被误导一遍**
（这正是 `acceptance:1-3` 开头抱怨的那件事本身）。

同时要注意：`AI_RULES.md:38` 早就把「点击即乐观响应：任何操作在界面上必须立刻有反馈，慢的工作放后面做」
写成产品判断标准 —— 新目标里的"流畅"档**不是新要求**，是把一条既有要求升成硬排序。
所以 §2 的修法不需要发明新原则，只需要把已有原则变成可判据的东西。

---

## 1. 稳定（优先级 1）：八条已证实的"永不收敛 / 假成功 / 静默丢失"

### A1 【最严重】中继文件推送：进度和"完成"是本地算出来的，发送结果整个被丢弃

```rust
// src-tauri/src/network/file.rs:606
crate::network::transport::relay_send_to_neighbors(state, peer_id, &msg).await;
// src-tauri/src/network/file.rs:607
let sent = end as u64;
```

`relay_send_to_neighbors` 返回 `()`，内部对每个邻居 `let _ = try_send(...)`：

```rust
// src-tauri/src/network/transport/outbound.rs:395
pub(crate) async fn relay_send_to_neighbors(state: &AppState, to: &str, msg: &Message) {
// src-tauri/src/network/transport/outbound.rs:401
        let _ = try_send(state, &p, msg).await;
```

循环跑完后无条件写终态并广播"完成"：

```rust
// src-tauri/src/network/file.rs:642-651
db::upsert_transfer(&dbc, transfer_id, peer_id, &name, size, "send", "done", ..., 1.0)
```

进度 = 文件读到哪，不是对端收到哪；这条链路**没有握手也没有回执**（`file.rs:188` 自己写着
"relay 没有 FileAccept 握手和 FileCompleteAck"）。后果：一条链路都没有、或三条队列全满时，
**一个字节都没出门，界面照样 100% + 完成**。
这直接命中 `docs/acceptance/1.0-release.md:54` 的禁止项
（"界面显示成功"和"对端实际收到"不一致，0% 却已读、100% 却失败，都算缺陷）和 :52（不要静默吞掉）。

### A2 中继接收侧：分片找不到会话就人间蒸发，且没有终态出口

```rust
// src-tauri/src/network/transport.rs:4316-4323
let Some(rs) = keys.get_mut(&transfer_id) else { return; };
let Ok(sealed) = STANDARD.decode(&data) else { return; };
let Some(bytes) = crypto::open_symmetric(&rs.file_key, &sealed) else { return; };
```

回收侧只 `retain` 并返回条数，不发任何事件、不写 `failed`：

```rust
// src-tauri/src/network/transport.rs:63-69
const RELAY_STATE_TTL_MS: i64 = 60 * 60 * 1000;
pub fn sweep_stale_relay(state: &AppState) -> usize {
```

后果：与 A1 正好相反方向的两边都错 —— 发送端显示完成，接收端卡在 X%，最多一小时后**静默消失**。
`transport.rs:55-58` 的注释说明这张表此前连回收都没有（可被单个好友刷到 OOM），
说明这条状态机的"失败出口"是后补的、补的时候只补了内存回收没补用户可见终态。

### A3 outbox 清扫器：先宣布"失败"，再在乎有没有写进库

```rust
// src-tauri/src/network/transport.rs:6353-6359
if let Ok(dbc) = state.db.lock() {
    let _ = db::set_message_status(&dbc, &msg_id, "failed");
    let _ = db::delete_outbox_by_msg_id(&dbc, &msg_id);
}
let _ = state.app.emit("message-failed", &msg_id);
```

`emit` 在 `if let` **外面**。锁中毒或任一写失败时：界面显示失败、`outbox` 行还在 ⇒
下一次 Hello/心跳把它再投一遍 ⇒ 用户看到"我以为失败了的消息又发出去了"（重复投递）。

更糟的是它的取数语句：

```rust
// src-tauri/src/network/transport.rs:6342
let Ok(dbc) = state.db.lock() else { continue };
```

同一把锁中毒时**清扫器整体空转且不留一行日志**。而这个清扫器是"没有任何链路的消息"唯一的终态出口
（它自己的文档注释 :6295-6304 就这么说），它空转 = 消息永久停在"发送中"。

### A4 回执与否定确认全靠 `let _ = try_send`，且这条路上没有任何可观察痕迹

`transport.rs` 里 `let _ = try_send(` 有几十处，其中判定终态语义的包括 :3677、:3710 一带
（`FileCompleteAck{success:false}` 与 `FileReject`）。队列满 ⇒ 帧被丢 ⇒ 发送端只能等
`FILE_ACK_IDLE` 静默窗口再重试。**功能上能自愈，代价是每次白等一个静默窗口 + 整段重推**，
而运维侧完全看不出是哪一层丢的（这正是 A1/A2 反复难定位的原因）。
此项即历史任务 `#19A`，本次复跑确认仍然存在。

### A5 重复 `FileDone` 且本机没记成功 ⇒ 一帧都不回

```rust
// src-tauri/src/network/transport.rs:3710-3723
Ok(None) => {
    let already_done = { ... .any(|t| t.id == transfer_id && t.status == "done") };
    if already_done {
        let _ = try_send(state, peer_id, &Message::FileCompleteAck { transfer_id, success: true, ...
```

`if already_done` 之后**没有 `else`**。接收器已被 `fail_receives_for_peer` 清掉、
且 `file_transfers.status != "done"` 时，发送端拿不到任何答复，只能等静默窗口。
按 `acceptance:14`（任何异常都必须可解释），"没做完"本身是一种可解释状态，必须回回去。

### A6 「已经收完」这件事不在续传判据的输入里 ⇒ 重投会落第二份副本

```rust
// src-tauri/src/network/transport.rs:4335-4340（`decide_offer` 的全部输入）
let active = file::receiver_progress(state, &transfer_id);
let disk_retained = file::retained_part_len(state, &transfer_id);
let decided = file::decide_offer(active.is_some(), active.unwrap_or(0), disk_retained, from_bytes);
```

三个输入是：内存活跃接收器、`.part` 前缀、发送端声明的位置。**没有"这个 transfer_id 本机已经收完并改名"**。
收完之后 `.part` 已不存在 ⇒ `disk_retained = 0` ⇒ 与 `from_bytes = 0` 的重复 Offer 判成 `Accept`
⇒ 重新建接收器 ⇒ 整份重推 ⇒ `unique_path` 写出 `名字(1)`。
触发链完整成立：A4 丢掉成功回执 → outbox 重试 → 整份重推。
（结构由代码路径确认，**未在真机上观察到**，见 §11。）

### A7 群文件根本没有续传能力（协议层就没有）

1:1 有：`protocol.rs:1050-1072` `FileOffer{ from_seq, from_bytes }` + `FileReject{ received }`。
群侧只有 `GroupFileOffer / GroupFileChunk / GroupFileDone / GroupFileCompleteAck`
（`protocol.rs:1146/1167/1177/1186`）—— **Offer 里没有位置字段，也没有 Accept 帧**，
重复 Offer 直接静默忽略：

```rust
// src-tauri/src/network/transport.rs:4701-4708
if state.group_file_keys.lock()...contains_key(&transfer_id) { return; }
```

⇒ 上周修 160MB 时把 1:1 那条判据收成"一份"（`decide_offer`），群那条**仍然是没有这条能力的状态**。
这是同一个缺陷类的第二个实例，而且比第一处更基础：不是判据写错，是协议少一半。

### A8 三处 `.lock().unwrap()` 混在一堆 `unwrap_or_else(|e| e.into_inner())` 里，而全仓没有 `catch_unwind`

```rust
// src-tauri/src/network/transport.rs:3582-3584
.pending_file_accept
.lock()
.unwrap()
```

`grep -rn catch_unwind src-tauri/src | wc -l` = **0**。⇒ 任何一次中毒 panic 会带走 `reader_loop`，
并跳过它紧接着的收尾（`transport.rs:1510` 起：`file::fail_receives_for_peer`、群接收器清理、
`links.remove`、`mark_peer_offline`）⇒ 那条连接的对端在界面上永久"在线"。
锁纪律在同一个函数里都不统一，说明它靠自觉而不靠守门。

---

## 2. 流畅（优先级 2）

### B1 结构根因：一条 `Mutex<Connection>` 服务全进程，298 个手写锁点

```rust
// src-tauri/src/state.rs:749
pub db: Mutex<Connection>,
```

`grep -rn "db.lock()" src-tauri/src | wc -l` = **298**，散布在 commands / network / db / storage 各处。
而 SQLite **已经开了 WAL**：

```rust
// src-tauri/src/db.rs:573-576
// 开启 WAL，提升并发读写
conn.pragma_update(None, "journal_mode", "WAL").ok();
conn.pragma_update(None, "synchronous", "NORMAL").ok();
```

也就是说：数据库层具备"一写多读"的并发能力，被上面那把互斥锁**主动抵消了**。
于是本轮 A 类里那些"持锁做慢活"的缺陷（遍历目录、VACUUM、全表扫描）不是七个孤立 bug，
而是同一个架构假设的必然产物 —— 任何一处忘记，代价都由**全 App 的其它所有 db 使用者**付。
这正是新目标要求复审的那类"错误的架构假设"。

附带两条同源事实：pragma 全部 `.ok()`（WAL 若因只读文件系统等失败 ⇒ 静默退回 DELETE 模式，
并发能力凭空少一档且无人知道）；`synchronous=NORMAL` 换来写入吞吐，
但它对"断电"的保证弱于 `FULL`（进程崩溃是安全的），这与验收项 11「SQLite 持久化 + 重启恢复」
之间的取舍从没被写进任何文档 —— 需要写，不需要改。

### B2 【当前仍在代码里】`clean_cache_now` 持全局 db 锁遍历磁盘，而自动清理那条路径早就避开了

```rust
// src-tauri/src/commands/channel.rs:416-417（HEAD b2fda12，已复跑确认）
let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
cache_cleaner::clean(&dirs, policy, &dbc)
```

`cache_cleaner::clean` 内部既递归遍历三个媒体目录删文件（`cache_cleaner.rs:96` 起
`std::fs::read_dir`）又做 VACUUM（`cache_cleaner.rs:123` 签名带 `&Connection`）⇒
**用户点一下「立即清理」，期间所有消息落库、发送、好友写入一起排队**。
讽刺的是同一件事的**自动**路径早就是对的（`channel.rs:339` 用 `clean_files` 不碰锁，
只在真删了东西时短暂拿锁 VACUUM）—— 两条路径一条改了另一条没改，是本仓库反复付钱的那类缺陷。

> 记账：本轮我曾把这处改成与自动路径同形并配了守卫
> （`cache_clean_must_not_hold_the_db_lock_across_the_walk` + `verify-guards.py` 变异用例），
> 源码测试 642/0 通过。**但工作区被并行操作回退，改动已全部丢失**（HEAD 未动，已推送的提交都在）。
> 所以这里按"缺陷仍在"登记，修法见 §9 阶段 3.1。

### B3 「保存资料」：会自锁的那一半已修，"卡在点击路径上"的那一半没修

已修的有守卫：

```rust
// src-tauri/src/lib.rs:2152-2156
fn never_awaits_while_holding_the_links_lock() {
    let cmds = all_commands_src();
    for f in [
        "pub async fn update_profile(",
```

剩下的半步在 `commands/system.rs:90-92`：

```rust
for tx in &targets {
    let _ = tx.send(msg.clone()).await;
}
```

发送端是有界队列（1024）：对端僵死 ⇒ `send().await` 无限挂起 ⇒ **这次"保存资料"永远不返回**（不再拖垮全局，但用户那颗按钮一直转）；
队列已关闭 ⇒ `let _ =` 把失败吞了 ⇒ 昵称/头像改动了但某些对端永远收不到，直到下一次 announce。
**修法不需要新机制**：`tokio::time::timeout` + 把失败计数交给既有 diag/日志。

> ⚠️ 顺带一条对**任务清单本身**的审计结论：`#25` 记的五项里，
> 第 2 项（`update_profile` 无超时 send 握着 links 锁）与第 5 项（`export_chat_text` 全库扫描持锁）
> **都已经不成立了** —— 后者在 `commands/favorites.rs:357-359` 明确写了"只把读库放进锁里"。
> 也就是说清单是上一轮审计的快照，不是当前事实。§9 的计划因此全部按**代码现状**重排，不按清单序号推。

### B4 前端乐观更新：主路径已经做对了，次级动作还没做

主发送路径是乐观的（`src/stores/useChatStore.ts:820-850`：先 `enqueueMessage(optimistic)`，
invoke 成功后 `replaceMessage`），未读清零也是（:623-629）。没做的是次级动作，举两个已复跑的例子：

```ts
// src/components/GroupTasksBoard.vue:489-493
async function setStatus(x: TodoItem, status: string) {
  await chat.updateTodo(props.groupId, x, { status });
  closeDetail();
```
详情抽屉要等一次完整往返（含群 gossip 投递）才关；点"完成"没任何即时反馈。

```ts
// src/components/settings/ResetSection.vue:51-56
clearConfirmOpen.value = false;
try { await chat.clearAllData(); await chat.refreshFriends(); await chat.refreshPending();
```
三次串行 IPC，全程无 loading 无 skeleton。

### B5 渲染账：每行每次渲染重新算一遍，而消息行没有 memo

```ts
// src/stores/useChatStore.ts:507-513
function groupReaderIds(groupId: string, messageTs: number): string[] {
  const members = new Set(groups.value.find(...)?.members ?? []);
  return Object.entries(groupReads.value[groupId] ?? {}).filter(...).map(...)
}
```

```vue
<!-- src/components/ChatWindow.vue:1110-1116 -->
:sender-name="isGroup ? chat.nicknameOf(item.sender_id) : ''"
:group-reader-ids="isGroup && activeGroupId ? chat.groupReaderIds(activeGroupId, item.ts) : []"
:pinned="pinnedIds.includes(item.msg_id)"
```

每行每次渲染：新建一个 `Set` + 一次 `Object.entries` + 一次 `filter/map`，**返回的是新数组引用**
⇒ 子组件 props 恒不等。`grep -c v-memo`：`MessageItem.vue` **0**、`ChatWindow.vue` **0**
（全仓 `v-memo` 只在 `ConversationList.vue` 与 `LogViewer.vue`）。
另：`pinnedIds.includes(...)` 是 O(行数 × 置顶数)。
缓解因素（也是事实）：列表本身**已经虚拟化了**（`src/components/VirtualList.vue` 自研 493 行，
前缀和 + 二分 + ResizeObserver 实测高度），单屏只渲染几十行 ⇒ 这一项是"每帧常数偏大"，
不是"O(全部历史)"。优先级因此低于 B1/B4，但不能算没有。

### B6 【主线程，最严重的一档】未读角标是同步命令，在主线程上逐像素重画托盘图标

```rust
// src-tauri/src/commands/network.rs:236-239
#[cfg(desktop)]
#[tauri::command]
pub fn set_unread_badge(app: tauri::AppHandle, count: u32) -> Result<(), String> {
    crate::tray::set_unread_badge(&app, count);
```

没有 `(async)`、是同步 `fn` ⇒ 按仓库自己读过的上游源码（`lib.rs:716-723`）它就是
**在 macOS 主线程的 IPC 回调里内联执行**，而它做的活是：

```rust
// src-tauri/src/tray.rs:104-108
for y in 0..height {
    for x in 0..width {
        let dx = x as f32 + 0.5 - cx;
        let dy = y as f32 + 0.5 - cy;
        let d = (dx * dx + dy * dy).sqrt();
```

外加 `tray.rs:255` `let _ = tray.set_icon(Some(icon));`。**每一次未读数变化都在 UI 主线程上跑一遍整张图标的浮点像素合成。**

更值得记一笔的是**为什么现有守卫没抓到它**：`blocking_commands_run_off_the_main_thread`
（`lib.rs:733`）已经是"规则式"守卫，但它的规则是 **9 个字符串 marker 去匹配命令体**
（`lib.rs:743-753`：`.db` / `db::` / `std::fs` / `logger.` / `Clipboard` / `list_interfaces` /
`if_addrs` / `thread::sleep` / `block_on`）。`set_unread_badge` 的函数体里**一个都不含**——
重活在一次跨模块调用后面。⇒ 这是守卫视图的第二个盲点（第一个是 include! 分册漏登记）：
**"规则式守卫"如果规则本身仍是字面量清单，就只是把盲点从"名字"挪到了"调用层级"。**

### B7 群操作把整条 gossip fan-out 串在点击路径上：代价 = N 个成员 × 500ms

```rust
// src-tauri/src/commands/window.rs:220
broadcast_gossip(s, env).await;
```

```rust
// src-tauri/src/network/transport/outbound.rs:148 与 :450,459
const SEND_QUEUE_FULL_TIMEOUT: Duration = Duration::from_millis(500);
for (tx, _peer_id, _endpoint) in &targets {
    if tokio::time::timeout(SEND_QUEUE_FULL_TIMEOUT, tx.send(msg.clone())).await.is_err()
```

有界等待本身是**对的**（`outbound.rs:451-457` 写清了为什么必须有限、必须留痕），
问题只在它**内联在用户动作里**：群越大、越有人拥塞，"改一条待办状态 / 置顶 / 发公告 / 加表情"
的响应就越接近 `N×500ms`。这类命令没有一条是"必须等发完才能返回"的 —— 落库成功就该返回。

### B8 「加收藏」等一整份媒体文件复制完才变星

```rust
// src-tauri/src/commands/favorites.rs:142
std::fs::copy(&src, &dst).map_err(|e| format!("复制到收藏目录失败：{e}"))?;
```

```ts
// src/components/ChatWindow.vue:631
const added = await chat.addFavorite(msgId, convId);
```

点星 ⇒ 等一次完整文件复制 ⇒ 才有 toast。收藏一张 200MB 的视频就是干等。
批量那条更明显：`ChatWindow.vue:797` 只发一次 `chat.addFavorites(ids)`，
但 store 里是**串行 await 循环**（`useChatStore.ts:1106-1108` `for (const id of msgIds) { if (await addFavorite(...)) }`）
⇒ N 次整文件复制一张接一张，且中途没有任何进度。

### B9 群公告把整文件 SHA-256 放在气泡之前 —— 同一个缺陷，1:1 早已修过

```rust
// src-tauri/src/commands/group_announcements.rs:210-213
let sha256 = tokio::task::spawn_blocking(move || file::sha256_file_hex(&p_sha))
    .await
    .map_err(|e| e.to_string())??;
```

而 1:1 路径**专门为此写过判据**：

```rust
// src-tauri/src/commands/files.rs:243-252
// ⚠️ **这里不计算**：整文件哈希是 O(体积) 的，放在建记录这一步就等于把气泡挡在哈希之后
// （真机 Mac 发 600MB：点完要"卡一会儿"才出现发送中气泡）…
"sha256": "",
```

⇒ 结论有两层：① 已修的教训**只覆盖了 1:1**；② 守这条的守卫
`no_whole_file_scan_before_the_file_bubble`（`lib.rs:1369`）**只读 `commands/files.rs`**，
所以群公告这条路径**结构上不可能被抓到**。这与 `#19C`「守卫名不符实」是同一类问题：
守卫的范围没跟着它命名的那条不变量走。

### B10 其余三处"等活干完才反馈"（同一形状，各一行）

| 位置 | 活 | 备注 |
|---|---|---|
| `ResetSection.vue:51-56` + `useChatStore.ts:1332` | 清空数据：三次串行 IPC | 无 `clearing` 态；后端 `favorites.rs:534` 起是 `read_dir` + `remove_dir_all` 的 O(文件数) 同步拆除 |
| `StorageSection.vue:44` `await api.getCacheInfo()` | 后端要遍历全部媒体目录算占用 | 该面板只有"清理/导出"有 loading（:21/:23），**加载没有** |
| `GroupCreateModal.vue:49` → `useChatStore.ts:1158-1163` | 建群：`createGroup` → `distributeGroupKey` → `refreshGroups` → `refreshConversations` **四次串行 await**，中间无 pending 态 | `refreshGroups` 内的 `:466-468` 是 N+1 次 `getGroupReads`，但用 `Promise.all` **并发**发 ⇒ 代价是 N 次 IPC 而非 N 次延迟；真正卡住的是那四步串行 |

（`ChatWindow.vue:857` 的 `importPickedFile` 也是"复制完才出气泡"，但它已在
`mobile_picker.rs:146-149` 的 `spawn_blocking` 里 ⇒ 只是延迟反馈，不占 async worker，等级低一档。）

### B11 一条登记性质的：`send_message` 里有 1.2 秒的探测等待

```rust
// src-tauri/src/commands/chat.rs:92
tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
```

只在"缺对端公钥、已触发 who_has"时走，且乐观气泡已在 ⇒ **不是阻塞缺陷**，
但它把"真实记录/失败原因"晚了 1.2 秒。登记是为了别让它被误当成需要修的那一类。

---

## 3. 正确（优先级 3）：三条口径

1. **进度口径**：`file.rs:607` 的 `sent = end` 是"读到哪"，网络层的 wire 进度是"刷出去哪"，
   对端口径是"收到并合成完哪"。三个都在用同一个 `progress` 字段与同一条 `file-progress` 事件。
   正确性要求不是"必须用最严的那个"，而是**界面必须说清它显示的是哪一个**（A1 的病根是拿最松的当最严的报）。
2. **终态口径**：`done/failed` 必须能指回一次真实证据 —— 要么落库成功（A3），要么对端回执（A4/A5/A6）。
   现在两边都不是硬要求。建议升为不变量 **INV-P25**（§9 阶段 1）。
3. **身份口径**：同一件事（"这个 device_id 的钥匙是哪把"）在加密侧、安全码侧、群密钥侧有三套取值顺序（见 E1/E2）。
   这一条同时是安全主项，细则在 §5。

---

## 4. 原生体验（优先级 4）

先记账：**已经做到的比"自研 Web 套壳"这个刻板印象多**。已复跑确认存在的机制：
自研虚拟列表（B5）、`env(safe-area-inset-*)` 11 个文件、Android 返回层用 pushState/popstate
（`src/composables/useBackLayer.ts:37,61`）、键盘避让走 `visualViewport`（`useAppStore.ts:266-278`）、
`overscroll-behavior: none`（`style.css:295,298`）、macOS 手绘红绿灯 + Windows 自绘画廊
（`TitleBar.vue:174-200,257`）、拖拽区绕开 WebView2 的 `data-tauri-drag-region` 缺陷改为显式 `startDragging()`、
长按（移动）与右键（桌面）双入口、按指针类型分别放行文本选择。

缺口（对"接近原生"有实际影响，且**都不算新增功能**）：

| 编号 | 缺口 | 证据 | 为什么值得做 |
|---|---|---|---|
| D1 | 上下文菜单是自研 HTML 浮层，不是平台菜单 | `src/components/ContextMenu.vue:124-130`（`:style="pos"` + 自写键盘处理）；而 `@headlessui/vue` 只被 2 个文件用到 | 焦点陷阱/键盘/读屏全靠自研补齐；成熟件已解决，属"用成熟能力替换自研" |
| D2 | 日志文件只按大小轮转，无年龄/份数之外的清理 | `logging.rs:128` `MAX_LOG_FILE_BYTES = 512*1024`，且 `logs/` 不在 `media_dirs`（`commands/channel.rs:322`）内 | 与 E4 同项：长期挂机留敏感明文，属稳定性/安全而不是新功能 |
| D3 | 桌面端无单实例 | `grep single-instance src-tauri/Cargo.toml` = 空 | 双开会互抢 db 文件与端口，是**稳定性**缺口（用户已定调"多开只用于开发"） |
| D4 | iOS 侧滑返回按设计不接管；haptics 只有 `navigator.vibrate`（Android/web） | `useBackLayer.ts:21-22` 自述；`src/utils/haptics.ts:57` | 记录在案；当前四端目标不含 iOS，**不做** |
| D5 | 滚动容器无 `will-change`/合成层策略 | `grep -rn will-change src` = 0 | 只在真机确认掉帧后再动，**不预先加** |

判定原则：**凡是"换机制、不加能力"的才做**（D1/D2/D3），
凡是"加一个原来没有的能力"的都不做（下拉刷新、深链、桌面宠物等一律排除）。

---

## 5. 安全（优先级 5，但用户点名的两条核心必须真成立）

### E1 【最高危】安全码可能在核对一把从未加密过任何字节的钥匙

**加密侧**取钥匙：好友表优先、节点表回落。

```rust
// src-tauri/src/commands/chat.rs:69-79
let from_db = { ... db::get_friend_x25519(&dbc, &friend_id) };
let from_peers = s.peers...get(&friend_id).and_then(|p| p.x25519_pubkey.clone());
match from_db.or(from_peers) {
```

**安全码侧**取值：**恰好相反**，而且两把钥匙各走各的回落。

```rust
// src-tauri/src/commands/friends.rs:34-47
let peers = s.peers.lock()...; let p = peers.get(&peer_id);
(p.and_then(|p| p.x25519_pubkey.clone()), p.and_then(|p| p.ed25519_pubkey.clone()))
...
their_x.or_else(|| db::get_friend_x25519(&dbc, &peer_id)),
their_e.or_else(|| db::get_friend_ed25519(&dbc, &peer_id)),
```

三个后果，都成立：
1. 两表都有该好友但 X25519 不同时，**屏幕上那串数字来自 `peers`，实际加密用的是 `friends`** —— 人工核对通过，
   核对的对象却是错的（用户 2026-09-22 的原话正是"人工核对就好了，重点是要确认人工核对的准确性"）。
2. `x25519` 来自内存 `peers`、`ed25519` 来自 `friends` ⇒ **混源**，这串码不代表任何一对真实存在的钥匙。
3. `peers` 里的钥匙可以是**未验签**来源：`network/discovery.rs:504-505` 自己写着
   "announce 来的公钥恒为 keys_verified=false"，而 `get_safety_number` **不看这个标志** ——
   同仓库其它信任判定都过这道闸（`network/transport.rs:611-613` `bound_ed25519_from_peer` 用 `peer.filter(|p| p.keys_verified)`）。

`crypto::safety_number`（`crypto.rs:148`）本身没问题：它忠实地把 6 个字段排序后哈希 —— 喂进去什么就印什么。
缺陷是**喂进去的东西没有单一来源**。这就是历史任务 `#17`，本次复跑确认仍在，且比记录里更具体（取值方向相反）。

### E2 同一类缺陷的第二处：群密钥分发不过身份闸

```rust
// src-tauri/src/network/transport.rs:6098-6115
fn pick_member_x25519(peers_key: Option<String>, friends_key: Option<String>) -> Option<String> {
    peers_key.or(friends_key)
}
pub(crate) fn resolve_member_x25519(state: &AppState, member_id: &str) -> Option<String> {
```

注释说明它存在的理由是"避免 friends 表公钥缺失导致 GroupKey 被静默跳过"—— 这个动机是对的，
但它把**未验签的内存钥匙**优先塞进了群密钥分发。⇒ 与 E1 是同一个根因的第二个实例：
"钥匙从哪来"没有唯一判据。修法与 `#32` 已定的范式一致：一个 helper，三处调用（1:1 加密、安全码、群分发）。

### E3 E2EE 无前向保密：调用点全部用长期身份密钥直接 ECDH，原始输出直接当对称密钥

```rust
// src-tauri/src/crypto.rs:75-84
Some(*my_secret.diffie_hellman(&their).as_bytes())     // ECDH 原始输出
...
let cipher = ChaCha20Poly1305::new(Key::from_slice(shared));   // 直接当 AEAD key
```

`grep` 全部调用点：`transport.rs:3409/4096/4111/4246/4630/5951`、`file.rs:304/525`、`gossip.rs:178/208`
**无一例外传的是 `state.identity.x25519_secret`**（长期身份私钥）。
全仓 `grep -i "ratchet|hkdf|chain_key|root_key"` = **0 命中**：没有 KDF 域分离，没有棘轮，没有临时密钥。
随机 nonce（`crypto.rs:86`）+ 重发重新加密是满足的，所以"链路明文"和"重放可解密"这两条没问题。
**没有**的是：长期私钥一旦泄露（本机 SQLite 就是明文，见 E4），全部历史消息可解 —— 即无前向保密。

⇒ 审计立场：**不要谎称有前向保密**（当前文案与文档没有明说，但"端到端加密强制开启"会被读者理解成包含它）。
是否补协商层（X3DH / Noise）是一个**要用户拍板的架构决定**，因为它是协议 breaking，
必须走 ADR-0007 的 capability 门控 + INV-P24 跨版本降级，成本远超它在本排序（第 5 档）里的位置。
§9 阶段 6 只安排"出 ADR + 风险说明"，不动代码。

### E4 明文残留：机制层是干净的，持久层有三处

**干净（逐条复跑确认，不需要动）**：
- 中继不落盘：`src-tauri/src/transport/` 下 grep 磁盘写 = **0 命中**；ADR-0020 的哑管道立场成立。
- 中继上的元数据：`relay_seal.rs` 把整帧（含 type/device_id/from/to/文件名/大小）封进密文，
  只有帧长与时序可读 —— 这条比 LAN 上的形状好，是对的。
- `.part` 分片临时文件：`file.rs:1391` 创建，`file.rs:1071` `TTL_MS = 24h` 定期清扫（`lib.rs:343` 挂时）。
- BLE 分片重组：`ble_framing.rs:233` 只在内存、有 `MAX_INFLIGHT_MESSAGES` 上界 + TTL `gc`，不落盘。
- 日志/诊断里**没有**消息正文、文件名、公钥、安全码（定向 grep 无命中）；中继口令已有守卫
  （`lib.rs:2652` `relay_token_never_reaches_logs_or_diagnostics`）。

**三处不必要的明文**：

1. **长期身份私钥明文入库**：
   ```rust
   // src-tauri/src/state.rs:1184-1185
   db::set_setting(&conn, "x25519_secret", &id.x25519_secret_b64()).ok();
   db::set_setting(&conn, "ed25519_secret", &id.ed25519_secret_b64()).ok();
   ```
   而 `rusqlite` 无 SQLCipher（`Cargo.toml:41` 只有 `bundled`），消息正文/昵称/路径全表明文
   （`schema.sql` 的 `messages.content`、`friends.nickname`、`file_transfers.path`、`outbox.payload`）。
   ⇒ 结论要诚实：**这个产品的存储设计本来就是"本机明文"**（README 的卖点就是"数据只存本机"），
   拿到磁盘 = 读到一切。这不是"漏了一个加密开关"，是一个需要写下来的**既定安全边界**：
   机密性依赖 OS 级全盘加密 + 账户隔离。要改变它就得引 SQLCipher（新的密钥托管问题 + 交付体积），
   在第 5 档且"不新增功能"的前提下**不建议现在做**，但**必须写进文档**，因为它决定用户该把它用在什么数据上。
2. **日志里有可关联身份**：`gossip.rs:352-354` 把 `peer=<device_id> nickname=<昵称>` 写进
   `logs/gosslan.log`；`transport.rs:2301-2303/2391` 写对端 endpoint 与中继服务器地址。
   昵称+设备号+时间是身份指纹，而日志**不在任何清理策略覆盖目录里**（见 D2）。
   这与"要求 7（口令不落日志）"是同一类问题，只是没人给它定判据。
3. **接收媒体默认永久保留**：`load_policy` 在两项都为 0/未设时返回"无策略"
   （`commands/channel.rs:298-309`：`.filter(|&d| d > 0)` ⇒ 0 = 永不清理），
   前端默认值 `retentionDays = ref(0)`（`StorageSection.vue:19`），
   而 UI 对 0 的文案正是 `keepForever`。**这不是 bug**（静默删用户聊天媒体才是数据丢失），
   但它是"传输路径不留长期残留"这句话的**边界**：机制残留会被回收，用户内容不会 ⇒ 要在文档里写明。

---

## 6. 可维护 / 扩展（优先级 6、7）

| 编号 | 事实 | 数字（本次实测） |
|---|---|---|
| F1 | 主链单文件过大 | `src-tauri/src/network/transport.rs` = **9110 行**；`network/` 合计 18919 行；`lib.rs` = 3326 行 |
| F2 | 锁纪律靠自觉 | `db.lock()` **298** 处手写；同一函数内 `unwrap()` 与 `unwrap_or_else(into_inner)` 混用（A8） |
| F3 | 变更热区集中 | 近 200 次提交里 `transport.rs` 被改 **43** 次、`state.rs` 18、`network/file.rs` 18 —— 稳定性风险与它们的重写频率同源 |
| F4 | 守门体系规模已不小 | `verify-guards.py` 用例 **148** 条；`lib.rs` 内 `#[test]` **55** 个 |
| F5 | 基线不对称 | macOS 基线 642 条；Windows 基线不完整（历史项 `#10`），意味着 Windows 侧的回归概率更高 |

F4 值得单独说一句：**148 条变异用例 + 55 个源码守卫**这套机制本身已经是这个仓库最值钱的东西之一，
"允许重构核心架构"的前提恰恰是它有非空转验证 —— 所以 §9 的每个阶段都要求"守卫 + 变异用例"同批落地，
而不是把守卫推迟到"重构完再补"。

---

## 7. 第三方成熟能力重估（对应指令第 6 条）

现有直接依赖（本次读 `Cargo.toml` / `package.json` 得出）：
Rust = tauri 2 + 5 个官方插件、`tokio`(full)、`rusqlite`(bundled)、`socket2`、serde、
`x25519-dalek`/`ed25519-dalek`/`chacha20poly1305`/`sha2`/`rand_core`、`btleplug`(vendored patch)、
`objc2*`/`windows`/`jni`、`notify-rust`、`machine-uid`、`if-addrs`、`hostname`、`uuid`、`url`。
前端 = vue3 + pinia + tailwind + headlessui + dayjs + highlight.js + lucide + vue-easy-lightbox + clsx/tailwind-merge。

| 自研件 | 规模 | 成熟替代 | 判定 | 理由 |
|---|---|---|---|---|
| 组网/发现/多路径/选路/故障切换 | `network/` 18.9k 行 | `libp2p` / `iroh` / `quinn`(QUIC) | **不换** | 换 = 重写主链。与"无服务器 + BLE + 四端安装包"三个硬约束正面冲突，而第 1 优先级正是稳定 |
| 应用层线协议（帧/能力协商/gossip/信封） | `protocol.rs` 2395 行 | 同上（libp2p 的 gossipsub 等） | **不换**，保留 | 语义（群、待办、合并卡片）本身就是自研的，协议壳换不掉收益 |
| 加密原语 | — | RustCrypto / dalek | **已经是成熟件** ✓ | 唯一缺口是**协商层**：`snow`(Noise) / X3DH —— 这是"我们根本没有的那一层"，§9 阶段 6 |
| 文件传输（Offer/Chunk/Done/续传/去重/校验） | `file.rs` 3308 行 | 无合适的成熟件（要嵌进既有 E2EE 帧与三级队列） | **保留**，但两条实现要合一 | A7 暴露的真问题不是自研，是**同一件事写了两遍**（1:1 有位置字段，群没有） |
| SQLite 访问层 | `Mutex<Connection>` + 298 锁点 | `r2d2` 连接池 / `sqlx`；或"单写多读 + 消息传递" | **该换 —— 本表唯一的高收益项** | WAL 已开（B1），并发能力是现成的，只是被自研的"一把锁"锁住了 |
| schema 迁移 | 手写 SCHEMA + `migration-ledger.md` | `refinery` / `rusqlite_migration` | **不换**（低优先） | 未正式发布、允许 breaking change，收益小于风险 |
| 虚拟列表 | `VirtualList.vue` 493 行 + 设计守卫测试 | `vue-virtual-scroller` / `@vueuse/virtual` | **保留** | 消息流的"底部钉住 + 向上前插保持锚点"恰是这些库的弱项，且这里已有测试覆盖 |
| 状态管理 / 通知 / 图标 / 灯箱 | — | pinia / notify-rust / lucide / easy-lightbox | **已经是成熟件** ✓ | 通知双轨（plugin + notify-rust）是有意为之，理由在 `notifications.rs` 头注释 |
| 上下文菜单 / 弹层 | `ContextMenu.vue` 等自研 | 已在依赖里的 `@headlessui/vue`（目前只有 2 个文件用） | **该用没用它** | 零新依赖成本，命中 D1/原生体验 |
| 日志 | 自研轮转 | `tracing` + 年龄清理 | **不换库，补清理** | 换库收益小；缺的是 D2/E4 那条"年龄清理" |

一句话总结这张表：**绝大多数自研不需要换**，真正该做的是
(a) 把已经付了钱的成熟能力用起来（SQLite WAL 的并发、Headless UI），
(b) 补上我们**根本不存在**的那一层（密钥协商），
(c) 把同一件事的两份实现合成一份（文件续传判据、钥匙来源判据）。
这三条都不属于"造轮子 vs 换轮子"，属于"轮子没装全 / 装了两套"。

---

## 8. 历史任务按新目标重估（指令第 7 条：重新评估历史任务）

| 任务 | 新目标下的处置 | 依据 |
|---|---|---|
| `#25` 五处阻塞 | **作废重建**：第 2、5 项已不成立（B3），第 1 项已完成待收尾（B2），第 3 项仍成立（中继整文件哈希），第 4 项降级为"次级动作乐观化"（B4） | 代码现状 ≠ 清单快照 |
| `#17` 安全码同源 | **升为安全主项，排在稳定批次之后**（E1/E2 合并做，一个 helper 三处调用） | 直接命中用户点名的"能确认对端不是中间人" |
| `#19` 三处 | **保留并扩为 A4/A5**（回执可丢、无 else 不回帧、deadline 漏算 1.334） | 稳定 = 第 1 档 |
| `#8` 真机验证 | **保留且必须保留** | 它是"稳定闭环"唯一能拿到的正面证据，源码永远证明不了 A6/A1 |
| `#20`/`#22`/`#23`/`#24` 四项审计 | **本次合并交付**：§1-§7 就是 A/C/D 的取证结果 + §7 的第三方重估 | 避免再拆四轮 |
| `#9` 事件名常量化 | **降到最末**（纯可维护性，第 6 档） | 新排序 |
| `#10` Windows 基线 | 并入可维护批次（F5），不单列 | 同上 |
| `#16` 设备身份 | "撞号检测⇒提示换新 ID" **保留**（正确性，真机撞过）；自定义后缀 **不做**（新功能） | 已在记忆定案，本次不改判 |
| 新增：**INV-P25 终态必须真实** | 建议立为新不变量，把 A1-A6 收敛成一条可判据的规则 | 见 §9 阶段 1 |
| 新增：**群文件续传**（A7） | 新拆一项，排在阶段 4（协议 breaking，需 capability 门控） | 缺陷类第二实例 |
| 新增：**中继口令/日志年龄清理** | 并入阶段 5（D2/E4-2） | 明文残留的最后一处机制性缺口 |

---

# 第二部分：方案计划

排序严格照新优先级：**先假成功（稳定），再身份同源（安全的核心那条），再流畅的结构性账，
再群文件续传，最后才是原生体验的收尾**。第三方重估（§7）不占阶段 —— 它只有一个可执行结论，
挂在阶段 6。每个阶段内部的每个改动仍守既有纪律：**一个改动 = 一个 commit + 一次版本提升**，
守门测试与 `verify-guards.py` 变异用例**同批**落地，不为让测试通过而改期望。

## 阶段 0 —— 目标唯一化（0 处代码，先做，因为它决定后面每一处取舍）

| 改 | 内容 |
|---|---|
| `AI_RULES.md:6` / `:22-34` | 把五档换成用户定案的八档序，并明确写"稳定 > … > 新功能"与"当前阶段不新增功能" |
| `AI_RULES.md` 新增一条 | "允许重构核心架构：根因在设计层时，先修设计；但一次一个 issue、守卫同批、测完才提交" |
| `docs/acceptance/1.0-release.md:13` | 删掉本地那份排序，改成指向 `AI_RULES §1`（消灭第二份事实来源） |
| `docs/acceptance/1.0-release.md:62` | "做**最小**修复"改为"判断根因层级：现象层能修则最小修，**根因在设计层则修设计**，不得顺手扩大" |
| 本文件要不要进索引 | **不进**。`grep -rn "docs/notes/" README.md docs/AI_ENGINEERING_INDEX.md AI_RULES.md` 现在 = 0 命中：既有审计笔记一律不占必读位（`audit-2026-09-13-mesh-ble-efficiency.md` 同例）。要进索引的是**目标声明本身**（改 `AI_RULES §1`），不是这份取证记录 —— 否则每轮审计都给必读清单加一行，索引迟早没人读 |

**退出判据**：`grep -rn "稳定.*速度.*可达.*安全" AI_RULES.md docs/` 只剩指向性引用，不再有第二份独立声明。
**版本**：docs-only，不提版本（与既有文档提交同形）。

## 阶段 1 —— 稳定：把"终态"变成只能由真实证据驱动（A1 A2 A3 A4 A5 A8，6 个 patch）

先立不变量 **INV-P25「终态必须可追溯」**，写进 `docs/protocol-invariants.md`：

> 任何面向用户的终态（`done` / `failed` / `delivered` / `100%`）必须能指回**至少一条真实证据**：
> 一次成功且未忽略返回值的落库，或一个对端回执，或一次实际写出的字节。
> 由本地循环计数、队列入队成功、或"没报错"推导出的终态一律算缺陷。
> 反之任何"可恢复等待"不得伪装成终态（A5 那条缺 `else` 就是这个反例）。

按这个判据逐条修，顺序 = 用户可感知度：

| # | 目标 | 改动形状（**设计层，不是补丁**） | 版本 |
|---|---|---|---|
| 1.1 | **A3** 清扫器先宣布后写库 | 把"写库"与"通知"的因果反过来：`set_message_status` + `delete_outbox` 的返回值参与判定，**写库失败就不 emit**，改为 `warn!` + 保留 outbox 行；`let Ok(dbc) = lock() else { continue }` 改成 `unwrap_or_else(into_inner)` + 计数日志（锁中毒必须可见） | patch |
| 1.2 | **A1** 中继推送假成功 | `relay_send_to_neighbors` 改为返回 `SendOutcome{delivered_bytes, failed_targets}`（**它的返回值就是这条链路的证据来源**）；`file.rs` 进度改为 `sent = 累计真实入队成功的字节`，循环结束按"是否有任何一个邻居收到"决定 `done` 还是 `failed`。这条链路本来就没有对端回执，所以判据下限是"确实进了某条活链路的队列"，**不能再是"循环跑完了"** | patch |
| 1.3 | **A2** 中继接收静默丢 + 无终态 | 三处 `else { return }` 合并成一处带诊断的拒绝（`push_diag_event` + 节流日志）；`sweep_stale_relay` 对"有会话但未完成"的条目补 `upsert_transfer(failed)` + `file-failed`，让超时变成用户看得懂的一句话 | patch |
| 1.4 | **A5** 重复 FileDone 不回帧 | `if already_done` 补 `else`：回 `FileCompleteAck{success:false}`（或新增"未完成，请重推"语义）。**注意**：这一步必须与 1.5 同批，否则否定确认会被队列丢弃（A4）而白改 | patch |
| 1.5 | **A4** 否定确认可被静默丢 | 给"终态语义"的帧（Ack / Reject / CompleteAck）单独一条**有界等待**发送路径（`SEND_QUEUE_FULL_TIMEOUT` 已有先例，`outbound.rs:148` 定义、`:459` 用于 gossip），失败时落一条节流 warn。**不新增队列、不新增优先级**，只把 `let _ =` 换成有判据的发送 | patch |
| 1.6 | **A8** 锁纪律不统一 | 三处 `.lock().unwrap()` 统一成 `unwrap_or_else(\|e\| e.into_inner())`，并加一条全仓守卫：**`network/` 与 `commands/` 里不得出现 `.lock().unwrap()`**（守卫写进 `lib.rs`，变异用例进 `verify-guards.py`） | patch |

**A6（成功后重投落第二份副本）不单列一次修**：它的根因是 A4/A5，1.5 落地后触发链断开。
但要在 1.5 的测试里**同时钉住**"已完成 → 重复 Offer 不再新建接收器"这一条，
判据是把 `file_transfers.status == "done"` 作为 `decide_offer` 的第四个输入（一个纯函数参数 + 一条用例）。

**退出判据**：
1. 每条都有"先红后绿"的回归用例；`verify-guards.py --only <标签>` 全绿（证明守卫非空转）。
2. `npm run verify -- --full` 15 步全绿；`cargo test --features bluetooth --lib` 用例数只增不减（当前基线 642）。
3. **可观测判据交给真机**（用户执行）：拔掉所有直连、只留中继，发 160MB ——
   期望看到"要么进度按真实字节推进并最终 `file-failed` 并说明原因，要么真的完成"，
   **不接受"100% + 完成但对端没有文件"**；日志前缀 `relayfile` 应能指认每一片的去向。

## 阶段 2 —— 安全的用户点名项：钥匙来源只留一个判据（E1 + E2，1 个 patch + 1 个 minor）

做法与 `decide_offer` 完全同构：**把"这把钥匙能不能用、该用哪把"收成一份纯函数**，三处调用。

| # | 内容 |
|---|---|
| 2.1 | 新增 `identity::friend_key(state, peer_id) -> Option<(x25519, ed25519, Trusted)>`：**内部只认 `keys_verified` 闸门**，返回**成对**的钥匙（禁止两把各走各的回落）。取值优先级必须与加密侧一致（`friends` 优先、`peers` 回落且**只在过闸时**可用） |
| 2.2 | 三处改调用它：`commands/chat.rs:69-79`（加密）、`commands/friends.rs:34-47`（安全码）、`network/transport.rs:6104`（群密钥分发 `resolve_member_x25519`） |
| 2.3 | `get_safety_number` 在**混源或未过闸**时返回 `None`，前端把"算不出来"显示成可解释状态（现有 `None` 语义已经写明"绝不拿 device_id 凑一个"，沿用） |
| 2.4 | 守卫：`lib.rs` 里钉"这三处都必须经过那一个 helper"（`upsert_peer`/`get_friend_x25519` 的直调只许出现在 helper 内部），并加**次序断言**（防 B3 那类"两表优先级反了"再次发生） |

**为什么排第二而不是跟在阶段 1 之后随大流**：它命中用户原话"能够确认聊天对端确实是目标用户"。
而它是**当前唯一一条"用户按了、通过了、但结论可能是假的"**的安全项 —— 假绿比没做更危险。
**退出判据**：一条纯函数用例覆盖"两表不同"与"混源"两种形状；真机判据 = 好友资料页安全码与对端当面核对一致，
且人为往 `peers` 塞一把不同的钥匙时，界面显示"无法核对"而不是另一个数字。

## 阶段 3 —— 流畅：按"会不会冻住整个进程"排序（B6 → B1）

| # | 目标 | 改动形状 | 版本 |
|---|---|---|---|
| 3.1 | **B2** 清理缓存持锁遍历 | 把 `clean_cache_now` 改成与自动路径同形（`clean_files` 不碰锁 → 只为 VACUUM 拿锁），删掉 `cache_cleaner::clean` 那份带 `&Connection` 的重载，守卫 `cache_clean_must_not_hold_the_db_lock_across_the_walk`（含"拿锁必须排在遍历之后"的次序断言）+ 变异用例。（**本轮做过一次，工作区被回退，需重做**） | patch |
| 3.2 | **B6** 角标在主线程 | 命令改 `#[tauri::command(async)]`，图标合成挪出主线程。**并且同批改守卫的形状**：把"9 个 marker 匹配命令体"倒过来 —— 凡**未标 async** 的同步命令必须出现在一份**显式白名单**里并写明"只碰内存"的理由，不在名单里即 FAIL。理由：字面量清单永远追不上"重活在一次函数调用后面"这种情况，而倒装判据能 | patch |
| 3.3 | **B7** gossip 出点击路径 | 命令在**事务提交后立刻返回**，`broadcast_gossip` 交给后台任务（`tauri::async_runtime::spawn`）。判据 = 用户动作的耗时上界不再与群成员数相关 | patch |
| 3.4 | **B8** 收藏整文件复制 | 先落收藏行 + 返回，复制进 `favorites_dir` 交后台任务，完成后按既有 `settings-changed`/专用事件推进 UI（与阶段 1.2 同一个判据：**入队/建记录 ≠ 完成**）。批量那条把串行 await 循环换成一次 IPC | patch |
| 3.5 | **B9** 群公告哈希 | 群公告改成"空串占位 → 投递任务算 → 回填"（照 `files.rs:243-252` 已有的那份设计搬），**并把 `no_whole_file_scan_before_the_file_bubble` 的读取范围从 `commands/files.rs` 扩到 `commands/group_announcements.rs`**（守卫范围必须等于它命名的那条不变量，否则同一条缺陷会在没被盯的地方复活） | patch |
| 3.6 | **B3** 保存资料 | 发送循环加 `tokio::time::timeout` + 失败计数进 `push_diag_event`；`broadcast_chat_style`（`settings.rs:402-404`）同形 | patch |
| 3.7 | **B10** 三处无反馈 | `ResetSection` 加 `clearing` 态；`StorageSection.loadCache` 加 loading；`createGroup` 加 pending 且把四步串行压成"建群 + 后台分发密钥 + 一次刷新" | patch |
| 3.8 | **B4** 次级动作乐观化 | `GroupTasksBoard.setStatus` 先本地置位、失败回滚 + toast；`:507` 归档 / `:526` 删除同形 | patch |
| 3.9 | **B5** 渲染账 | `groupReaderIds` 记忆化（或传引用稳定的 `Map`）、`pinnedIds` 改 `Set`，然后才给消息行加 `v-memo`（引用不稳定时加了也不生效） | patch |
| 3.10 | **3.3 的前置债**：298 个锁点的"锁内做慢活"分类清扫 | 脚本辅助分类，只改一类（锁内出现 IO / 哈希 / 遍历 / VACUUM）；产出一条**通用守卫**：同一函数体内 `db.lock()` 与 `std::fs::` / `sha256` / `VACUUM` 不得共存 | 每点 patch |
| 3.11 | **B1 结构账（本阶段唯一大改）** | 一进程一把 `Mutex<Connection>` → **单写 + 多读**（WAL 已支持；写路径专用连接 + 读路径 `r2d2` 池）。**必须放最后**：它会改变 3.10 的判据（改完"持锁做慢活"的爆炸半径自动变小），先做会让 3.10 白干 | minor |

**退出判据（每条都要能测）**：给 `dev_diag` 加一条"命令耗时直方图"（或复用现有计时），
四个场景各跑一次并给数字：① 清理缓存期间连发 20 条消息；② 传 160MB 时切会话/开设置；
③ 保存资料时对端正在休眠；④ 导出几十万条聊天时收消息。
**判据 = 四个场景里没有任何一次用户操作 >200ms 无响应**（数字可与用户再定，但不能没有）。
3.11 的风险与回退：它碰 DB 层 ⇒ 单独分支、单独 minor、CHANGELOG 写清"可回退 = revert 该 commit"，
且必须跑 `check-mobile.sh` 与 Android 编译（DB 初始化在移动端路径不同）。

## 阶段 4 —— 群文件续传（A7，1 个 minor，需 ADR）

协议层补齐，形状照 1:1（**不另立第二套判据**）：`GroupFileOffer` 加 `from_bytes`（`#[serde(default)]`），
新增 `GroupFileProgress`（或复用 `GroupFileCompleteAck` 加字段）回真实位置，接收侧复用 `decide_offer` + `.part` 前缀。
**必须先出 ADR**（编号续 0021 之后），写清：
① 旧端不发 `from_bytes` ⇒ 新端按 0 处理，语义与今天一致（INV-P24 / ADR-0007 的 capability 门控）；
② 群是 1→N，"位置"是**按收件人**各算各的（今天 `group_file_recipient_status` 已经是按人存的，正好对得上）；
③ 与 `#8` 的"群文件停滞判定按人"合并验证，避免两条按人状态机各写一遍。
**退出判据**：两端版本不一致时群文件仍能传完（旧↔新双向各一次）；同版本下断链重连后日志出现"从 N 字节续发"。

## 阶段 5 —— 原生体验收尾（D1 D2 D3，各 1 个 patch）

1. **D1** `ContextMenu` 迁到已在依赖里的 `@headlessui/vue`（`Menu`/`Popover`），删自研焦点与键盘分支 —— 零新依赖、命中"成熟件替换自研"。
2. **D2** 日志加"份数/年龄"上限，并把 `logs/` 纳入 `cache_info` 的可见占用；同时把 `gossip.rs:352` 那行 `nickname` 从日志里去掉（保留 `peer`，昵称不是诊断必需）—— 这一条同时是 E4-2。
3. **D3** 引入 `tauri-plugin-single-instance`：第二实例聚焦已有窗口。这是**稳定性**（双开互抢 db/端口）而非新功能。

**明确不做**（按"不加能力、只换机制"的边界）：下拉刷新、深链、iOS 侧滑、预加 `will-change`、
事件名常量化（`#9`，第 6 档排最末）、SQLCipher（除非用户改判）、QUIC/libp2p 替换、虚拟列表换库。

## 阶段 6 —— E2EE 协商层：只出文档，等拍板（0 处代码）

产出 `docs/adr/0022-…`：现状（无临时密钥、无 KDF、无棘轮 ⇒ 无前向保密）、
威胁模型（本机明文 DB + 长期私钥 ⇒ 磁盘访问即可解密历史）、
三条可选路线（Noise `snow` / X3DH+双棘轮 / 只加 HKDF 域分离不改协商）各自的
**breaking 程度、跨版本门控方案、包体与性能代价、要重测哪些真机场景**。
**不动代码**：它是第 5 档，且是协议 breaking；决定权在用户。

---

## 11. 这份审计的边界（哪些没验、哪些我复跑后不采信）

**取证性质**：以上全部来自**读代码 + 逐条 `sed`/`grep` 复跑**。
一条真机验证都没做 —— 双机、只走中继、BLE 三条路径的实际行为仍是 `#8` 的未验项，
所以 §9 每个阶段都硬写了"可观测判据交给真机"，而不是"改完就算稳定"。

**平台边界**：本机是 macOS。Windows / Android 侧的行为未验（且 Windows 基线本身不完整，见 F5）。
本次 `git fetch` 失败（远程不可达），"与远端比对"只做到本地 `origin/main` = `b2fda12` 为止；**推之前必须重跑一次 fetch 比对**。

**并行子审计里，我复跑后不成立或被高估的 4 条**（记下来，是为了让这些结论不会被下一轮当成事实捡回去）：

| 子报告的说法 | 复跑结果 |
|---|---|
| `clean_cache_now` 已改成不持锁遍历 | **不成立**：我本轮改过，但工作区被并行操作回退，HEAD 上仍是 `cache_cleaner::clean(&dirs, policy, &dbc)` ⇒ 它引的是我未提交时的快照 |
| `importPickedFile` 整文件复制会占住 async worker | **高估**：复制已在 `mobile_picker.rs:146` 的 `spawn_blocking` 里 ⇒ 只剩"延迟反馈"，低一档 |
| `refreshGroups` 的 `getGroupReads` 是 N+1 **串行** | **高估**：`useChatStore.ts:466-468` 外面套着 `Promise.all` ⇒ 是 N 次并发 IPC |
| `#25` 的"第 2、5 项待修" | **不成立**：两处都已修（`never_awaits_while_holding_the_links_lock` 守卫 / `favorites.rs:357-359` 只把读库放锁内），剩下的只有 B3 那半步 |

**留在备查、未写进正文的推断项**（标 `?`，因为在阶段 1 动手时必须顺带证实或证伪，不能现在当事实卖）：

1. **迟到的成功回执会被下一轮消费**：`FileAccept` / `FileCompleteAck` 只带 `transfer_id`
   （`protocol.rs:1064-1073`），等待表也只按 `transfer_id` 键 ⇒ deadline 触发后的重试
   可能取到上一轮那张 `success:true` 而提前判 `delivered`。结构可疑，**触发链我没构造出来**。
2. `file_sending`（按 peer 键的 HashSet）只在 spawn 循环正常结束时清 ⇒ 若中途 panic（见 A8），
   该 peer 的 outbox flush 永久挡住。依赖 A8 真的发生。
3. Offer 走 Normal 队列、Chunk 走 Low 队列，`try_send` 的 failover 可能让两者落到不同链路。
   群侧已有守卫钉住"三类帧同链路"（`group_file_dispatch.rs`），**1:1 侧没钉**。

**A6 的触发链**（回执丢 ⇒ 整份重推 ⇒ 落第二份副本）是代码路径推断，**未在真机看到过第二份文件**。

**行号会漂**：本文件的 `文件:行号` 对的是 HEAD `b2fda12`。任何人（包括并行的另一个会话）提交之后行号就会移动 ——
定位请认**函数名 + 逐字引用**，行号只是加速用的。这也是为什么每条结论都附了原文。
