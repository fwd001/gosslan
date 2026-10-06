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

| 行数 | 文件 | 是什么 | 拆的判定 |
|---|---|---|---|
| 13,268 | `CHANGELOG.md` | 版本记账台账（`scripts/version.mjs` 读写、快速层有「CHANGELOG 结构」判据） | **不自行拆**：拆＝改记账口径，交他拍板（A 归档分卷 / B 不动） |
| **7,087** | `src-tauri/src/network/transport.rs` | 网络传输层本体（启动/选路/分发/中继/群密钥/清扫） | ✅ **可拆，且已有先例**（见 §2） |
| 4,845 | `scripts/e2e-multi-instance.mjs` | 双/多实例 E2E harness（按 `--round` / `--fault` 分轮次） | ✅ 按轮次族切 + 一张注册表；⚠️ selfproof 档位名口径会受影响 |
| 4,176 | `scripts/verify-guards.py` | 护栏非空转 runner（**202 条 Case**） | ✅ 按 `tags` 域切 Case 清单，runner 逻辑只留一份 |
| 4,004 | `src-tauri/src/lib_tests.rs` |  crate 层测试 | ✅ 按域切分册 |
| 3,227 | `src-tauri/src/network/transport/tests.rs` | 传输层测试 | ✅ **归位到各分册**（测试贴着被测物） |

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

**结论（这一条决定了「更优雅的写法」往哪使）**：全文件**最大的单个函数只有 59 行**
（复跑：按顶层 `fn` 的花括号跨度排序取前 25）。也就是说——**这文件的问题不在函数级写得烂，
全在文件级聚合**：13 个关注点被横幅隔开却共处一文件。所以最佳实践就是
**沿横幅把关注点切成 Rust 子模块**，而不是顺手重写逻辑（重写会让"行为不变"再也证不出来）。
同目录 `transport/{gossip 964, outbound 538, relay 845, tests 3226}` 就是这仓已经在用的分册手法
⇒ 继续用同名目录切，属于顺势而非发明。

## 3. 安全网（先建网，再动刀）

1. **锚点是最大的静默风险**：全仓 202 条护栏 Case 里 **26 条的 `file=` 指着 transport.rs**。
   把代码搬走 ⇒ 这些注入锚点在原文件里找不到字面量、护栏**空转但报绿**（本仓踩过）。
   每刀必须同批改 `file=` 路径，并跑秒级核对：`python3 scripts/verify-guards.py --list`（退 0＝锚点都活着）。
2. **产物恒等判据**（行为没变的证据，不是"我读过觉得没变"）：
   - `cargo test --features bluetooth` 的**断言条数逐字相同**（清单：`src-tauri/test-baseline.macos.txt`，
     其中 transport 出现 153 次；拆完必须现算对账、不许净减）；
   - `cargo clippy --features bluetooth -- -D warnings` 退 0；`cargo fmt --check --all` 退 0；
   - 快速层 17 步绿；动 Rust 的那一层要跑含 Rust 的层（快速层不含 clippy ⇒ 只跑快速层会把红推上去）。
3. **机械手法固定**：新分册 + 主文件 `mod x;` + `pub use x::*;` 再导出
   ⇒ `crate::network::transport::*` 的**所有调用点零改动**，编译器会当场把私有项跨模块的漏改暴露出来。
4. **同批改口的指名点**：`docs/migration-ledger.md`（"几个家"要重数）、`docs/ARCHITECTURE-MAP.html`
   （LIVE_DOCS，图上有现算统计）、`docs/domains.data.mjs`、ADR-0014 / ADR-0020 等的 file 指名、
   两份复审文档（不在 LIVE_DOCS＝数字无守卫，只改路径不改数字）。
5. **每刀一个提交**：动应用码 ⇒ 同提交写 `Version-Bump: patch` 并把五处版本号一起提；
   标题带半径标记（push→main 时 CI 判的是本次推送范围，标记**必须在 push 前补好**）；
   CHANGELOG `[Unreleased]` 记账；收尾跑 `aoci_maintain`（新文件是新的受管理对象）。

## 4. 下刀顺序

耦合从低到高，先用最薄的一刀把机械流程跑通，再啃大的：

`pending_group_keys 452`（横幅自述零 AppState 依赖）→ `outbox_sweep 316` → `queue_policy 263`
→ `peers 251` → `mesh_sync 297` → `dial 624` → `relay_file 534` → `group_keys 1,094`
→ `startup 1,281` → `dispatch 1,658`（含 handle_message，最后做）。
小段落（E2EE 载荷 116 / 副作用策略 20 / gossip 头 4）留在主文件或并入相邻分册。
预期：transport.rs 落到 <300，各分册 ≤1,700 ⇒ 全部在阈值内。

之后依次：`transport/tests.rs` 归位 → `lib_tests.rs` 按域切 → `e2e-multi-instance.mjs` 按轮次切 →
`verify-guards.py` 按 tags 切。`CHANGELOG.md` 等 A/B 决定。
