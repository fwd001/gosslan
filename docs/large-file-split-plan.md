# 大文件拆分计划（>3000 行）

> 目标：把超过 3000 行的代码分析清楚 → 判断能不能拆 → 按架构级手法安全拆到 3,000 行以内。
> 规矩来源：`AI_RULES.md:121` 明确「拆文件、挪模块属于允许的整理，不改变行为」⇒ 本计划的每一次切分
> 都必须**行为不变**，验收靠产物恒等而不是靠读代码觉得没变。
> ⚠️ 本文所有数字都挂现算命令；重跑一遍再说数。

## 1. 谁真的超了 3000 行（现算）

```bash
python3 - <<'PY'
import subprocess,re
fs=[f for f in subprocess.run(['git','ls-files','-z'],capture_output=True).stdout.decode().split('\0') if f]
for f in fs:
    try: b=open(f,'rb').read()
    except Exception: continue
    if b'\x00' in b[:8000]: continue          # 二进制按 AOCI 同一条口径剔除
    n=b.count(b'\n')+1
    if n>3000: print(n,f)
PY
```

第一次数出 12 个，其中 **3 个是图标（`.icns`/`.png`）被按换行数误算**、3 个是生成物
（`Cargo.lock`、`package-lock.json`、`.aoci/baseline.json`）⇒ **真正要处理的是 6 个**：

> 这张表是**首量快照**（2026-10-06 那一轮）；已完成的行标了去向，但**行数一律以 §1 上面那条命令现算为准**
> —— 快照里的数字只会腐烂，那条命令不会。

| 行数 | 文件 | 是什么 | 拆的判定 |
|---|---|---|---|
| 13,268 | `CHANGELOG.md` | 版本记账台账（`scripts/version.mjs` 读写、快速层有「CHANGELOG 结构」判据） | **不自行拆**：拆＝改记账口径，交他拍板（A 归档分卷 / B 不动） |
| **7,087** | `src-tauri/src/network/transport.rs` | 网络传输层本体（启动/选路/分发/中继/群密钥/清扫） | ✅ **已拆完**（2026-10-06 四批 ⇒ 主文件进阈值，见 §4 末） |
| 4,845 | `scripts/e2e-multi-instance.mjs` | 双/多实例 E2E harness（按 `--round` / `--fault` 分轮次） | ✅ 按轮次族切 + 一张注册表；⚠️ selfproof 档位名口径会受影响 |
| 4,176 | `scripts/verify-guards.py` | 护栏非空转 runner（**202 条 Case**） | ✅ 按 `tags` 域切 Case 清单，runner 逻辑只留一份 |
| 4,004 | `src-tauri/src/lib_tests.rs` |  crate 层测试 | ✅ 按域切分册 |
| 3,227 | `src-tauri/src/network/transport/tests.rs` | 传输层测试 | ✅ **已拆完**（2026-10-06：壳 + 15 个 `<concern>_tests.rs`，见 §4 末） |

2,000–3,000 那一档（不在本轮范围，作为下一步候选）：`network/file.rs` 2,495、`network/ble.rs` 2,392、
`src/stores/useChatStore.ts` 2,351、`i18n/locales.ts` 2,010、`state.rs` 1,962、`protocol.rs` 1,814、
`components/MessageItem.vue` 1,536。⚠️ `state.rs`（AppState）与 useChatStore 在 2026-09-25 复审里
被判为「拆了短期只降稳定」且他已定过「动 AppState 选 B 不拆」⇒ **本计划不碰**，除非他改主意。

## 2. 「它们在干嘛」——transport.rs 的内部构成（现读）

它的作者已经留了地图：文件里 15 条分节横幅，各节跨度：

| 起始行 | 行数 | 分节 |
|---|---|---|
| 177 | **1,281** | 服务启动 |
| 1458 | 263 | 链路队列的容量策略（第 2 步 · P3） |
| 1721 | 297 | 传输层 ↔ mesh 层同步（6b-3） |
| 2018 | 624 | 主动建链（小 ID 拨号） |
| 2642 | **1,658** | 消息分发 |
| 4300 | 116 | 直连 E2EE 载荷 |
| 4416 | 20 | 落库裁决 → 副作用策略 |
| 4436 | 4 | Gossip 处理（消费判据 / handle_gossip / 载荷还原） |
| 4440 | 534 | 中继文件传输 |
| 4974 | **1,094** | 群密钥 |
| 6068 | 251 | 节点与好友辅助 |
| 6319 | 452 | 待发群密钥登记表（横幅自述：纯逻辑、不涉网络与 AppState） |
| 6771 | 316 | Outbox 超时清扫 |

顶层项统计：`fn` 54 / `const` 13 / `enum` 5 / `use` 22 / 内联 `mod` 1（L1983 mesh_sync_tests）。

**结论（这条被实测推翻过一次，现在的版本是重新量出来的）**：初稿写"最大单函数只有 59 行"
是**算错的**——那版跨度函数在遇到第一个无 `{` 的行就提前返回。按正确的括号深度重量（复跑见下）：

```bash
python3 - <<'EOF'          # 按顶层 fn 的花括号深度算跨度
import io,re,glob
def spans(f):
    L=io.open(f,encoding='utf-8').read().split('\n'); res=[]; i=0
    while i<len(L):
        m=re.match(r'^(?:pub(?:\([^)]*\))? )?(?:async )?fn\s+([A-Za-z_]\w*)',L[i])
        if m:
            d=0; st=False
            for j in range(i,len(L)):
                d+=L[j].count('{')-L[j].count('}')
                if '{' in L[j]: st=True
                if st and d==0: res.append((j-i+1,m.group(1))); i=j; break
        i+=1
    return res
top=sorted(sum([spans(f) for f in ['src-tauri/src/network/transport.rs']+glob.glob('src-tauri/src/network/transport/*.rs')],[]),reverse=True)
for n,nm in top[:8]: print(n,nm)
EOF
```

⇒ 真实结论是**两层都有病**：文件级聚合（13 个关注点共处一文件，本轮已按内聚切完）**并且**存在巨型函数
（`handle_message` 1,562 行、`handle_gossip` 840、`handle_group_file_done` 335、`spawn` 304、
`connect_to_peer` 247）。文件级搬家已完成、行为可由"用例名差集 0"证明；**函数级拆分是真重构**，
必须一 handler 一提交、每步都过同一套恒等判据，不能和搬家混在一次提交里。

⚠️ 两条**推翻本文初稿**的现读事实（都靠读实现/读守卫拿到，不是推理）：

1. **横幅不等于内聚**：照横幅原样切会切出杂糅文件——`待发群密钥登记表`（L6319-6770）这一节里
   同时住着群密钥登记表、成员变动文案、昵称/群名解析、离线补发四种关注点 ⇒ 下刀按**内聚**重分组（§4 已据此改过）。
2. **这仓的分册手法是 `include!` 而不是 `mod`**：`transport.rs` 已经用
   `include!("transport/{outbound,relay,gossip,tests}.rs")` 拆过四次，文件头 L45 写明"同一模块、
   零 `use` 改动" ⇒ 模块路径、可见性、测试全名一字不变。先例：`transport/{gossip 964, outbound 538,
   relay 845, tests 3226}`。继续用同名目录 + `include!`，属于顺势而非发明。

## 3. 安全网（先建网，再动刀）

1. **锚点风险实测比初稿小一个量级**——初稿写「26 条会被静默弄死」，那是一条没读实现的推断，已作废：
   `verify-guards.py` 自带 `_list_includes()` + `_resolve_anchor_file()`，它从根文件**递归展开
   `include!` 子模块树**，把注入写回真正含该锚点的那个文件 ⇒ 搬进分册时**锚点自动跟随，
   不需要改那 26 条 Case 的 `file=`**。真正的约束是另一条：**同一锚点必须在整棵树里恰好出现一次**
   （否则它自己报「在多个文件里都出现了」）。每刀仍跑 `python3 scripts/verify-guards.py --list`
   退 0，并抽一条做反证：把某锚点从分册里删掉必须报红——不报红就是这条守卫空转。
2. **产物恒等判据**（行为没变的证据，不是"我读过觉得没变"）：
   - `cargo test --features bluetooth` 的**断言条数逐字相同**（清单：`src-tauri/test-baseline.macos.txt`，
     其中 transport 出现 153 次；拆完必须现算对账、不许净减）；
   - `cargo clippy --features bluetooth -- -D warnings` 退 0；`cargo fmt --check --all` 退 0；
   - 快速层 17 步绿；动 Rust 的那一层要跑含 Rust 的层（快速层不含 clippy ⇒ 只跑快速层会把红推上去）。
3. **机械手法固定**：把连续区间整段搬进 `transport/<concern>.rs`，原位置换成一行
   `include!("transport/<concern>.rs");` ⇒ **同一模块、同一命名空间**：调用点、`use`、可见性、
   测试全名一行都不变（这正是它比 `mod` + 再导出更适合本轮"只搬不改"的原因）。
4. **同批改口的指名点**：`docs/migration-ledger.md`（"几个家"要重数）、`docs/ARCHITECTURE-MAP.html`
   （LIVE_DOCS，图上有现算统计）、`docs/domains.data.mjs`、ADR-0014 / ADR-0020 等的 file 指名、
   两份复审文档（不在 LIVE_DOCS＝数字无守卫，只改路径不改数字）。
5. **每刀一个提交**：动应用码 ⇒ 同提交写 `Version-Bump: patch` 并把五处版本号一起提；
   标题带半径标记（push→main 时 CI 判的是本次推送范围，标记**必须在 push 前补好**）；
   CHANGELOG `[Unreleased]` 记账；收尾跑 `aoci_maintain`（新文件是新的受管理对象）。

## 4. 下刀顺序

按内聚重分组后的**第一刀**（都落在 L6319-6769 这段，锚点 0 条）：
`transport/pending_keys.rs`（L6319-6555 登记表与重发）→ `transport/peer_state.rs`
（L6556-6601 链路快照 + 好友/成员公钥）→ `transport/member_notices.rs`（L6603-6730 成员变动
系统消息与文案 + 昵称/群名解析）→ `transport/outbox_flush.rs`（L6732-6769 离线补发）。
之后：`outbox_sweep 316` → `queue_policy 263`（5 条锚点）→ `peers 251` → `mesh_sync 297`
→ `dial 624` → `relay_file 534` → `group_keys 1,094`（2 条）→ `startup 1,281`（3 条）
→ `dispatch 1,658`（**10 条锚点、含 handle_message**，最后做）。
小段落（E2EE 载荷 116 / 副作用策略 20 / gossip 头 4）留在主文件或并入相邻分册。
~~预期：transport.rs 落到 <300，各分册 ≤1,700 ⇒ 全部在阈值内。~~
**这条预期没达成，也没必要达成**（2026-10-06 实测：主文件停在 **2,742**）。差在两段最厚的：
`startup 1,281` 与 `dispatch 1,658`（含 `handle_message` 那 1,562 行）留在主文件里 ——
把它们再搬出去只剩"壳里再套一层壳"，而**文件级已经进阈值**；真正该动的是**函数级**拆分，
那是会改控制流的重构、和"只搬不改"不能混在一次提交里 ⇒ 另案（见本节末"剩余"）。

锚点按节分布是**现算**的（import 那份守卫脚本读它自己的 `CASES` 列表，不另写一份解析）：

| 分节 | 锚点条数 |
|---|---|
| 消息分发 | 10 |
| 队列容量策略 | 5 |
| 服务启动 | 3 |
| 头部注释 / 群密钥 | 各 2 |
| 主动建链 / 中继文件传输 / Outbox 超时清扫 | 各 1 |
| 待发群密钥登记表及其后（第一刀的四节） | **0** |

**已完成（2026-10-06 五批）**

1. `transport.rs` 7,087 ⇒ **2,742 行，进阈值**；产出 13 个 `include!` 生产分册
   （最大四册 `group_file 839` / `dial 625` / `handshake 581` / `relay_file 535`）。
2. `transport/tests.rs` 3,227 ⇒ **壳 29 行 + 15 个 `<concern>_tests.rs`**（最大 336 行）。
   ⚠️ **本节初稿写的"测试归位进各生产分册"被否掉了**，两条理由都是机器定的、不是我挑好看：
   - `transport_src_for_guards()` 那份"生产码全集"视图**只许装生产码**——测试字面量掺进去会把
     按窗口取段的守卫飘到测试文本上（2026-10-06 实测假红 4 条，见 CHANGELOG 4.33.6）；
   - `lib_tests.rs` 的 `guard_source_views_register_every_include_subfile` 按**文件名后缀 `_tests.rs`**
     豁免测试分册 ⇒ 新册名必须以 `_tests.rs` 结尾。叫 `tests_*.rs` 会被判成"漏登记"，而消红的唯一
     "顺手"办法就是把测试并进视图 —— 那正好是假绿的形状。
   册内仍是 `include!` 回**同一个 `mod tests`** ⇒ 95 条 `network::transport::tests::*` 的**全名一字未变**。

每批的恒等判据都是同一套：`cargo test --features bluetooth --lib` **789 passed / 0 failed** 且
`-- --list` 那 789 条用例名与拆前**差集 0 行**（比"条数相等"硬）+ clippy/fmt/`verify-guards --list`
（202 条锚点各恰好命中一次）/测试清单守卫（基线 789 条全在跑）/领域图/领域依赖/快速层全退 0。
复跑：`wc -l src-tauri/src/network/transport.rs src-tauri/src/network/transport/*.rs`。

**剩余（用 §1 那条命令现算，2026-10-06 晚）**：Rust 侧**已经没有 >3,000 行的文件**。还超的是
`lib_tests.rs` 4,009（按域切）→ `scripts/e2e-multi-instance.mjs` 4,845（按轮次切）→
`scripts/verify-guards.py` 4,176（按 `tags` 切）→ `CHANGELOG.md` 13,352 等 A/B 决定。
然后是**函数级**的 `handle_message`（1,562 行）/ `handle_gossip`（840）拆分 —— 那是真重构、另案提交。
