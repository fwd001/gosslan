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
| 4,845 | `scripts/e2e-multi-instance.mjs` | 双/多实例 E2E harness（按 `--round` / `--fault` 分轮次） | ✅ **可拆，排下一刀**（原写「不划算」的两个前提都被实测推翻，见本节末）：现算 顶格 `if (MODE)` 轮次块共 **3,146 行**、非轮次部分 **1,699 行** ⇒ 光搬引擎/夹具最多落到 ~3,100；而判据 C 是按"顶格 `}` 收块"从**这一个文件**里数每轮断言数的，判据 D 再拿这份数与门禁 local 层互点 ⇒ 分册必然同批改这两条判据（改尺子的风险大于收益）。**先做 verify-guards.py**，这一条排最后 |
| 4,176 | `scripts/verify-guards.py` | 护栏非空转 runner（**202 条 Case**） | ✅ **已拆完**（2026-10-07：runner 385 行 + `scripts/guard_cases/` 7 册，最大 771 行）。判据 E 同批改了计数范围（现在 glob 分册目录数），`guard_cases/__init__.py` 另加一条「目录里的分册 ≠ `MODULES` 名单就当场 ImportError」的对账 —— 那条正是 E 自己买不到的那一半 |
| 4,004 | `src-tauri/src/lib_tests.rs` |  crate 层测试 | ✅ **已拆完**（2026-10-06：壳 205 行 + 11 个 `lib_<concern>_tests.rs`，最大 614） |
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

**已完成（截至 2026-10-07，四个文件切完）**

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
3. `lib_tests.rs` 4,009 ⇒ **壳 205 行 + 11 个 `lib_<concern>_tests.rs`（最大 614）**。
   ⚠️ 这一刀的分册**必须与 `lib_tests.rs` 同级平铺**、不许挪进 `lib_tests/` 子目录：册里近 50 处
   `include_str!("commands.rs")` / `include_str!("../gen/android/…")` 是**相对本文件**解析的，
   换目录就整体偏移一位（编译器会拒、不静默 —— 但那就不再是"逐字未变"的搬家，恒等判据要降级）。
   这条规矩不是我发明的：`lib_tests.rs` 头部 2026-09-28 那段注释就写着"同目录 ⇒ 零路径风险"，
   并拿 `network/transport/tests.rs` 那次换目录必须改 `../ble.rs` 作反例。同目录先例 `protocol_tests.rs`。
   留在壳里的只有三份守卫视图（`all_commands_src` / `all_db_src`）与它的解析器 —— 登记对账那条守卫读的是
   `include_str!("lib_tests.rs")` 并按函数名切体，把视图搬走就是弄丢它自己的锚点。
   **非空转对照（现跑，不是推断）**：摘掉 `network/mod.rs` 里 `transport/dial.rs` 那一行登记 ⇒ 搬进
   `lib_source_view_tests.rs` 的那条守卫当场 `FAILED` 并指名缺哪一个分册（`1 failed; 788 filtered out`
   ⇒ 过滤确实命中，不是打空退 0）；还原后 `1 passed`。

4. `scripts/verify-guards.py` 4,176 ⇒ **runner 385 行 + `scripts/guard_cases/` 7 个域分册（最大 771）**。
   202 条 `Case` 一块没丢：`--list` 输出**排序后与拆前逐字节相同**（两边都是 624 行 / 202 条）。
   ⚠️ 运行次序按 `MODULES` 分段变了 —— 只有 `[n/m]` 进度号受影响，而**没有判据读那个序号**
   （现算：代码里没有 `CASES[` / `CASES.index`；文档里 `[197/197]` 那种写的是**总数**、与次序无关）。
   两条**新对账**各自当场验过非空转：① 摘掉一条 `Case(` ⇒ 判据 E 报「现算 201 与图上那格 202 对不上」，
   还原即复绿；② 往目录里丢一册却不写进 `MODULES` ⇒ `--list` 起跑前 `AssertionError` 点名差集。
   ② 守的正是 E 买不到的那一半：E 按目录 glob 数条数，所以「漏点名一册」在它眼里条数不变（＝假绿）。
   另：`check-scripts-parse.mjs` 的 import 闭包原来只走 `.mjs`，现在补上 Python 的 `from <pkg> import`
   ⇒ 分册里的语法错落到秒级层（实测把 `= [` 改成 `= [)` 该步报红，还原即绿）。
每批的恒等判据都是同一套：`cargo test --features bluetooth --lib` **789 passed / 0 failed** 且
`-- --list` 那 789 条用例名与拆前**差集 0 行**（比"条数相等"硬）+ clippy/fmt/`verify-guards --list`
（202 条锚点各恰好命中一次）/测试清单守卫（基线 789 条全在跑）/领域图/领域依赖/快速层全退 0。
复跑：`wc -l src-tauri/src/network/transport.rs src-tauri/src/network/transport/*.rs src-tauri/src/lib_tests.rs src-tauri/src/lib_*_tests.rs`。

**剩余（用 §1 那条命令现算，2026-10-07 晚）**：Rust 侧与护栏 runner 都进阈值了。还超的只剩两个（其中台账那一本等你拍板 A/B）：
`scripts/verify-guards.py` 4,176 ⇒ **已做**（见本节末第 4 条）。留一句口径出处：判据 E 原来只在
这一个文本里按 Case 构造行现算条数，分册后会得 0，而那条判据故意「数到 0 就 throw」
⇒ 它必须与搬家**同批**改，不能先搬后补。→
`scripts/e2e-multi-instance.mjs` 4,845（现算：顶格 `if (MODE)` 轮次块共 3,146 行、非轮次部分 1,699 行
⇒ 光搬引擎最多落到 ~3,100，而判据 C/D 的尺子就长在这一个文件上 ⇒ 拆的第一动作就是同批改这两条尺子）→
`CHANGELOG.md` 13,352 等 A/B 决定（harness 那一刀的成本核算见下面专门那一节）。
### 关于 `scripts/e2e-multi-instance.mjs`（4,845）：量完作用点后的结论是**可以拆，下一刀就拆它**（先前写的「本轮不拆」被自己的实测推翻，见本节末）

```bash
python3 - <<'PY'   # 顶格 if (MODE) { … } 以顶格 } 收尾（与判据 C 同一套形状规则）
import io,re
L=io.open('scripts/e2e-multi-instance.mjs',encoding='utf-8').read().split(chr(10))
s=[i for i,l in enumerate(L) if re.match(r'^if \([A-Z_]+',l)]
tot=0
for i in s:
    j=i
    while j<len(L) and L[j]!='}': j+=1
    tot+=j-i+1
print('轮次块',tot,'非轮次',len(L)-tot,'合计',len(L))
PY
```

现算：**轮次块 3,146 行 / 非轮次部分 1,699 行**。⇒ 只把引擎与夹具搬走，主文件还剩 ~3,150，
**过不了阈值**（复跑旗标数：`grep -cE '^const [A-Z_]+(_LIE)? = (FAULT|ROUND) === ' scripts/e2e-multi-instance.mjs` ⇒ 29 个，形如 `const POISON = FAULT === "poison-part"`）；
要过就得把轮次块本身搬走，而那些块读的就是这 29 个**文件顶部旗标** ⇒ 搬出去必须改成显式入参，
**那是重写而不是搬家**。
★ 2026-10-07 用 AST 把这件事量到底了（不再靠「引用了 43 个名字」这种聚合说法）：
  模块级声明 **168 个**，其中**会被重新赋值的 51 个**；顶格轮次块 **26 个 / 3146 行**，
  其中 **21 个块会往至少一个模块级可变名里写东西** —— 而对 ESM 的 import 绑定赋值是 SyntaxError，
  ⇒ 这些写必须改成经由一个共享状态对象（如 `S.peerTo = …`）。
  好消息是这 51 个名字**绝大多数是单轮自己的**（只被一个轮次块写：20 个，
  如 `xferId2..8` / `srcFileN` / `taskCreateId` / `gMentionId`）—— 它们跟着自己那一轮走就行；
  而按**轮次族**分组后真相是：**只有 1 个名字真被多个族写入** —— `peerTo`（POISON/RESUME/KILL/FREEZE/
  SENDKILL/STALL/DISK/ROT/SHRINK/MULTI 十族都写它）；其余 **43 个只在各自那一族内共享**
  （`xferId2..8` / `srcFileN` / `task*Id` / `gMention*` 等）⇒ 它们跟着自己那一族搬走就行。
  ⇒ 所以这一刀的难点不是共享状态，而是「把 26 个 `if (FLAG) {…}` 包成 async 函数 + 一个 `peerTo` 的家」。  复跑（审计脚本是一次性的，不放仓里；这份文件里的每个数都出自它）：
  ```bash
  node /tmp/audit-harness-scope.mjs scripts/e2e-multi-instance.mjs
  ```
  （它用 `@babel/parser` 走 AST：统计每个顶格区域引用了哪些模块级名、其中哪些会被重新赋值。
  口径：读共享量在 ESM 里是安全的（import 是活绑定，`const` 数组/对象照样能改内容），
  **只有「往模块级可变名里写」才是障碍** ⇒ 判可拆性要数的是后者，不是前者。）

★ 复证成本也说清（原来这里写的「每轮 30–45 分钟」是我从记忆里抄的，没当场量）：
  当场量的一次（`--round=dmreaction`，release 二进制刚重编）：**18 秒跑完 24 条断言，23 过 1 红**，
  而那条红正是下面 ② 的环境格（「报告带两张全屏帧」）⇒ **每轮几十秒**，不是我原先抄的几十分钟；
  全部 20 轮的真跑复证因此是**十几分钟量级**、不是隔夜工程。这条改口的直接后果：
  本节下面「本轮不动」的判断**不再成立**，改完判据 C/D 之后按轮次族拆走。
  既有读数是本地层 21 步（含全部轮次）一整层跑完的量 —— 见 CHANGELOG 里各趟「全量/本地层」结论行，
  那一层的墙钟本来就是现算印出来的，所以这一格**不抄固定分钟数**。
  真正卡住「能不能廉价复证」的不是分钟数，是这两条实测前置：
  ① pre-flight 要 release 二进制**晚于**最后一次改 `src-tauri/src` 的提交（实测拒跑、退 2，
     解法就一条：`cd src-tauri && cargo build --release --features bluetooth`）；
  ② 屏幕锁定状态下「全屏帧」那一格必红，且 macOS 拒绝给不在当前 Space 的窗口出图
     （实测：5 扇窗全拿不到图 ⇒ 判为环境限制，**不许为此放宽截图判据**）。
  ⇒ 搬家期间可廉价验的：`node --check`、**ESM 链接期就会把漏 export 的名字报成 SyntaxError**、
     判据 C 的每轮断言数（改造成 glob 轮次模块后要求与拆前**逐轮相同**）、`verify-guards.py --list`
     （★ 现算：202 条护栏 Case 里**没有一条**把注入锚点打在这个文件上；锚在 `scripts/` 下的共 19 条，     指向的是 `verify.mjs` / `check-change-budget.mjs` / fixture 等 ⇒ 搬这个文件不会弄死护栏锚点，     真正的间接判据是：判据 C 静态数每轮 `check(` 条数、判据 D 拿它与门禁两层互点、     `npm run selfproof:check` 那 20 档反证跑的就是这个 harness，外加 4 条 `--*-selfcheck` 秒级入口。  ⇒ 换句话说：这一刀的**结构**能被廉价证，**行为**要靠真跑轮次；跑轮次要屏幕解锁 + 现编二进制。
  同时判据 C 与判据 D 的尺子就长在这一个文件上（C 按顶格 `}` 数每轮 `check(` 条数，
D 拿这份数与门禁 local/release 两层互点）⇒ 一次改动要同时动「harness + 两条判据 + 20 轮的对账」。
而这一层的复证成本是**每轮 30–45 分钟、全链路 20 轮**，搬家出错的表现不是编译错而是某一轮的断言数悄悄变了。
⇒ 按「先量作用点、再决定动不动」这条既有纪律，**本轮把它记为已量、不动**，
真要动的时候应当连着「让轮次块可参数化」这件设计一起做（那是 `docs/stability-roadmap.md` 级别的活），
而不是为了凑阈值改一把正在承重尺子。

然后是**函数级**的 `handle_message`（1,562 行，`transport.rs:993-2554`）/ `handle_gossip`
（840 行，`transport/gossip.rs:124-963`）拆分 —— 那是真重构、另案提交。

## 5. 「更优雅的写法」实测到哪一步（函数级，不动文件大小）

### 5.1 先量拆分前提：`handle_message` **没有跨分支共享局部量**（现算）

```bash
python3 - <<'PY'
import io,re
L=io.open('src-tauri/src/network/transport.rs',encoding='utf-8').read().split('\n')
s=next(i for i,l in enumerate(L) if l.startswith('pub async fn handle_message('))
e=s
while L[e] != '}': e += 1
print('span', e-s+1, 'arms', sum(1 for i in range(s+1,e) if re.match('^        Message::', L[i])))
PY
```

实测：函数体 **1562 行、38 个顶层分支**；`match` 之前唯一被 `let` 出来的名字是
`allowed` / `cfg` / `dbc`，而它们全在 `if let Some(to) = directed_relay_target(…)` 那个**早退分支的内部**
（那条分支跑不到 `match`）。分支体里出现的 `allowed` / `cfg` / `now` 也都是各分支自己 `let` 的
（现算 7 处）。⇒ **「一个消息族一个 handler」在这里是机械活而不是重写**：
新函数只需要 `(&AppState, &str peer_id)` 加上自己那几个解构字段，不需要造上下文结构体。

⚠️ 但这一刀**不让任何文件变小**（`transport.rs` 早已在阈值内），改的又是控制流 ⇒
按本仓既有口径属于「真重构」，而 2026-09-25 复审时这一处已被判过「纯搬家、短期只降稳定」而暂缓 ⇒
**等他点头再动**。真要动：一族一提交，每步跑与前五刀同一套恒等判据
（`cargo test --lib -- --list` 的 789 条差集 0 行 + `verify-guards.py --list` 的 202 条 + clippy/fmt/快速层）。

### 5.2 顺手抓到一处真缺陷：中继授权闸有 **3 个家**，而现有判据正替这个形状把关

现算 `decide_relay_from_peer(` 的生产调用点：`src-tauri/src/network/transport.rs:1003`、`src-tauri/src/network/transport.rs:1278`、`src-tauri/src/network/transport/relay_file.rs:459`

`lib_relay_data_tests.rs` 的 `relay_data_plane_respects_policy` 判的是 `wired >= 3`，
语义是「三个转发点都接过闸」。⇒ **这条判据现在要求的就是"重复三遍"这个形状**：
谁把闸收成一处 helper、三个消费者共调，`wired` 会掉到 1 ⇒ 判据当场红，而代码其实变好了。
这正是本仓反复踩的两件事叠在一起：「同一个判据长在多处」+「守卫把缺陷钉成契约」。

⇒ 修法必须**一件事两件一起做**（谁也别先动）：
① 抽 `relay_gate_allows_forward(state, peer_id) -> bool` 当唯一的家，内联处改成调用它；
② 判据同步**改严而不是改松**：从「`decide_relay_from_peer(` ≥3 处」改成
   「策略函数调用恰好 1 处（在家里）+ helper 的调用点恰好 3 处（三个消费者）」，
   并配两条反证：摘掉任意一个消费者 ⇒ 红；把闸内联回某一处 ⇒ 红。
不同批改 ② 的话，"消红最顺手"的动作就是再复制一份第四遍 —— 那才是把缺陷真正钉死。
