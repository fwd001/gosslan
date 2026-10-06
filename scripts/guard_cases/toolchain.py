#!/usr/bin/env python3
"""护栏非空转用例分册：门禁与工程链路自身（scripts/*、docs/*、package.json、CHANGELOG 结构）。

本册 29 条 / 543 行，2026-10-07 从 `scripts/verify-guards.py`（原 4,176 行、202 条挤在一份
`CASES` 字面量里）按**锚定的被守物**切出来。块文本逐字未搬动过一字 ⇒
恒等判据＝`verify-guards.py --list` 的输出排序后与拆前**逐字节相同**（条数与用例名都不是"我觉得一样"）。

⚠️ 三条硬规矩（都是这仓自己踩出来的形状）：
1. 加一条护栏就加进**对应这一域**的本册；域由 `file=` 锚点决定，不按"谁方便找"。
2. 分册必须被 `guard_cases/__init__.py` 的 `MODULES` 点名 —— 那里有一条**起跑前就会炸**的对账：
   目录里的模块集合 != 名单 ⇒ `ImportError`。漏点名的后果不是报错而是**那几条护栏从此不跑**（假绿）。
3. `check-doc-numbers.mjs` 的判据 E 现在按 `scripts/guard_cases/*.py` 里的 Case 构造行现算条数，
   与契约图上那一格对账；数到 0 它自己 throw（尺子坏了要比被测物先响）。

路由规则按**路径段**匹配，不许用裸 substring：`TodoCardBubble.vue` 里含 "ble"、`FriendProfile.vue`
里含 "file" —— 第一版就是这么被错分进蓝牙册与文件册的（各 2 条），改成段/主干相等才干净。
"""

from __future__ import annotations

from .base import ROOT, TAURI, Case, cargo, npm

CASES: list[Case] = [
    # ---------------- INV-P24 第 4 条：发送侧门控 ----------------
    Case(
        name="1:1 发送路径必须真的问门控判据（不门控不许发）",
        why="门控判据函数写得再对，发送点不调用它就等于没有门控 —— 而这条不是假想需求：\n"
        "     MsgKind::Merge 是 V1 期间（9b26006）才加的，v4.8.2/v4.18.10/v4.20.0 三个已发布版本\n"
        "     的 ChatMessage.kind 仍是枚举，收到 kind:merge 会**整帧丢掉**（v4.22.34 只修了我们\n"
        "     这一侧），发送方 outbox 反复重投最后显示「发送失败」。这条用例证明「把发送点那行\n"
        "     门控删掉」一定会被抓 —— 注入方式就是删掉那次调用（不是改成 if false，那仍然算调用）",
        file=TAURI / "src" / "commands" / "chat.rs",
        injections=[(
            "        if !crate::protocol::dm_allowed_by_features(&kind, peer_features.unwrap_or(0)) {\n"
            "            return Err(crate::protocol::kind_blocked_hint(&kind, peer_features));\n"
            "        }",
            "        let _ = (&kind, peer_features);",
        )],
        cmd=cargo("test", "--lib", "new_message_kinds_are_gated_at_the_send_path"),
        cwd=TAURI,
        expect_fail_hint="必须问门控判据",
        tags=["rust", "protocol", "gating"],
    ),
    Case(
        name="1:1 挡下时的文案必须走三态入口（不许把\"不知道\"说成\"版本旧\"）",
        why="判据把\"从没交换过 Hello\"并进 0 是对的方向（宁可少发一条），但文案沿用\"对方的\n"
        "     Gosslan 版本较旧\"就成了一次假指控：`peer_content_features` 是内存表，对方一离线\n"
        "     就被 sweep 掉、本机重启即空 ⇒ 缺条目通常只代表\"此刻不知道\"，用户照着去催对方\n"
        "     升级，而真正要做的只是等对方上线（#37② 的\"小半\"）。\n"
        "     注入方式：把三态入口退回旧那句 —— 门控方向没变、编译没坏，只有接线断言会红",
        file=TAURI / "src" / "commands" / "chat.rs",
        injections=[(
            "            return Err(crate::protocol::kind_blocked_hint(&kind, peer_features));",
            "            return Err(crate::protocol::kind_unsupported_hint(&kind));",
        )],
        cmd=cargo("test", "--lib", "new_message_kinds_are_gated_at_the_send_path"),
        cwd=TAURI,
        expect_fail_hint="分两句说",
        tags=["rust", "protocol", "gating"],
    ),
    Case(
        name="门控表里的能力位必须本机自己声明（否则功能把自己锁死）",
        why="新增 kind 时登记了「对方需要什么能力」却忘了在 Hello 里声明同一个位 ⇒\n"
        "     所有对端看起来都不支持 ⇒ 这个功能我们自己永远发不出去，而且是静默的。\n"
        "     注入方式：把 CONTENT_FEATURE_MERGE 从本机广播的位图里摘掉",
        file=TAURI / "src" / "protocol.rs",
        injections=[(
            "pub fn content_features() -> u32 {\n    CONTENT_FEATURE_PULL\n        | CONTENT_FEATURE_MERGE\n        | CONTENT_FEATURE_FILE_EPOCH\n        | CONTENT_FEATURE_FLEX_DM_KIND\n}",
            "pub fn content_features() -> u32 {\n    CONTENT_FEATURE_PULL\n        | CONTENT_FEATURE_FILE_EPOCH\n        | CONTENT_FEATURE_FLEX_DM_KIND\n}",
        )],
        cmd=cargo("test", "--lib", "every_gated_kind_is_advertised_by_us"),
        cwd=TAURI,
        expect_fail_hint="没声明",
        tags=["rust", "protocol", "gating"],
    ),
    # ---------------- 门禁自己的分层自检 ----------------
    Case(
        name="门禁分层的自证不是装饰（漏归重门禁层必须当场红）",
        why="verify.mjs 的快速层靠 `isHeavyStep` 按**名字/命令**判断哪一步会碰工具链，"
        "旁边另有一条自证 `mayTouchToolchain`。两者判据不同正是重点：将来有人加一条 "
        "`{cmd:\"bash\", name:\"某项检查\"}` 而名字没带关键词 ⇒ 它**悄悄落进快速层**，"
        "表现是「快速层怎么突然三分钟」，没人会去查 —— 会让人想绕过的门禁等于没有门禁。\n"
        "     ⚠️ 这条用例是补 v4.22.31 欠下的账：当时我用「注入一条 cmd:cargo 的步骤」"
        "自证，那是**错的** —— cargo 步骤本来就被 isHeavyStep 归走，永远不会漏，"
        "所以那次『通过』什么也没证明。真正的注入必须造出"
        "「会碰工具链却没归层」的组合，也就是把一条 bash 步骤改名去掉关键词。\n"
        "     跑的是 `--list`：自证在 listOnly 分支**之前**执行，所以既不编译也不跑测试，"
        "秒级出结论，可以留在日常子集里",
        file=ROOT / "scripts" / "verify.mjs",
        injections=[(
            '  name: "移动端编译门禁（Android）",',
            '  name: "移动端检查（Android）",',
        )],
        cmd=["node", "scripts/verify.mjs", "--list"],
        cwd=ROOT,
        expect_fail_hint="分层自检失败",
        tags=["gates", "frontend", "new-guards"],
    ),
    Case(
        name="每个门禁步骤都必须声明 CI 归属（漏声明 = 这条门禁 CI 永远不跑）",
        why="「跑哪些检查」以前在 verify.mjs 的步骤表与 verify.yml 的 run: 列表里**各写一份**，\n"
        "     于是必然腐烂（verify.yml 头部那句「457 条前端断言 / 503 条 Rust 用例 / 93 条护栏」\n"
        "     早就对不上实际）。单源化后 CI 只说跑哪个组，所以每个步骤必须声明 group；\n"
        "     漏声明的静默后果比写错更糟：本地照跑、CI 从不跑，两边看都是绿的。\n"
        "     注入：摘掉某一条的 group ⇒ --list 必须退出码 1",
        file=ROOT / "scripts" / "verify.mjs",
        injections=[(
            '    group: "frontend",\n    name: "不变量例外守卫",',
            '    name: "不变量例外守卫",',
        )],
        cmd=["node", "scripts/verify.mjs", "--list"],
        cwd=ROOT,
        expect_fail_hint="没声明 CI 归属",
        tags=["gates", "frontend", "new-guards"],
    ),
    Case(
        name="门禁 local 层必须逐条点名 harness 的正向注入轮（少一步要能红）",
        why="§十五把多实例 E2E 的正向轮接进 verify 之后，静默面从「根本没接进来」变成「接进来又被删掉一步」：\n"
        "     删掉 `--group local` 里任意一条步骤，`verify:e2e` 会照样报「3 步全绿」，没有任何东西记得少了一步。\n"
        "     判据 D 拿 harness 自己的 `const X = FAULT === \"…\"` 当正向轮次清单，与门禁里点名的\n"
        "     `--fault=` 互相对账（不手抄第二份名单），并反向钉一条：`-lie` 模式不许进门禁 ——\n"
        "     预期红的东西进了门禁，整层就会被静音。\n"
        "     注入：把 kill-mid 那条改成一个 harness 里不存在的名字 ⇒ 文档守卫必须退出码 1",
        file=ROOT / "scripts" / "verify.mjs",
        injections=[(
            '"--fault=kill-mid"',
            '"--fault=kill-mid-gone"',
        )],
        cmd=["node", "scripts/check-doc-numbers.mjs"],
        cwd=ROOT,
        expect_fail_hint="但 verify.mjs 的门禁层没有它",
        tags=["gates", "frontend", "new-guards"],
    ),
    Case(
        name="打生产包必须带 --features bluetooth（否则产物静默地没有蓝牙）",
        why="BLE 是可选 feature，漏了 `--features bluetooth` 的后果是**静默**的：构建成功、"
        "产物正常、只是那个包完全没有蓝牙（开关起不来、搜不到设备）。"
        "真机代价是拿一个没有蓝牙的包去测 Windows ↔ Android，白跑一轮 —— "
        "这个坑在 dist:win 系列与**一键入口** `npm run dist`（scripts/package.mjs）上都出现过",
        file=ROOT / "package.json",
        injections=[(
            '"dist:win": "tauri build --features bluetooth --bundles nsis --target x86_64-pc-windows-msvc"',
            '"dist:win": "tauri build --bundles nsis --target x86_64-pc-windows-msvc"',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="没有蓝牙",
        tags=["frontend", "build"],
    ),
    Case(
        name="版本号规则（feat 必须算中档，否则台账与门禁一起失效）",
        why="版本分类是发布台账与 `version:check` 的唯一依据；把 feat 降成 patch 会让\"中功能\""
        "永远不提升中位，历史台账与门禁同时失真（这类退化不报错、也不影响功能）",
        file=ROOT / "scripts" / "semver.mjs",
        injections=[('feat: "minor",', 'feat: "patch",')],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="minor",
        tags=["frontend", "version"],
    ),
    Case(
        name="CHANGELOG 结构（[Unreleased] 锚点缺失/顺序错乱必须报出）",
        why="发布脚本按行首 `## [Unreleased]` 插入新小节。真实事故：它以前用 includes+replace "
        "找锚点，正文里出现同样文字就被误命中 ⇒ 4.1.1~4.1.11 全被插进 4.1.0 小节的半句话里、"
        "真正的锚点被吞掉（不报错、不影响功能，只有结构检查能拦住）。"
        "⚠️ 2026-09-16 修正：本用例原先拿 `npm run version:check` 当命令，而那条命令的 ①②"
        "（版本必须 ≥ 未发布提交要求的、每个提交要有自洽的 Version-Bump 声明）在**攒提交期间"
        "本来就该是红的** ⇒ 本用例永远进不了『恢复即 PASS』、被判成护栏失效。"
        "改成只跑结构检查的 `version:changelog`：结构是结构、记账是记账。"
        "（2026-09-28 #138/#123 更新：①② 的作用域都已缩小 ⇒ 上面那句「当时该红」今天不再成立，"
        "但这条拆分仍然要 —— 结构判据不许被记账口径牵动。）",
        file=ROOT / "CHANGELOG.md",
        injections=[("## [Unreleased]\n", "## [unreleased]\n")],
        cmd=npm("run", "version:changelog"),
        cwd=ROOT,
        expect_fail_hint="[Unreleased]",
        tags=["frontend", "version", "docs"],
    ),
    Case(
        name="测试清单：磁盘上的测试文件没登记进 package.json 必须报出来",
        why="`npm test` 的脚本里是**手工枚举**的 48 条路径。新增一个 .test.ts 时若忘了把它加进"
            "那串字符串，新文件不会被执行，而 `npm test` 依然**全绿** —— 与「漏 --features」"
            "是完全同类的东西：退出码 0 的空转。",
        file=ROOT / "package.json",
        injections=[(
            "src/utils/selfChat.test.ts src/utils/todos.test.ts ",
            "src/utils/selfChat.test.ts ",
        )],
        cmd=["node", "scripts/check-test-manifest.mjs", "--only", "frontend"],
        cwd=ROOT,
        expect_fail_hint="未登记",
        tags=["manifest", "frontend"],
    ),
    Case(
        name="不变量钩子：文档里的钩子名字解析不出来必须报出来（挡住「契约只有 prose」）",
        why="`docs/protocol-invariants.md` 是 AI 必读清单直接指向的『物理定律』登记处，而它过去每条"
            "只有一段伪码式的「验证」段 —— 没有任何**具名对象**在跑，所以改坏不变量时 CI 不会红。"
            "现在每条都有一行 `- 钩子：` 指向真实存在的用例/护栏脚本/夹具名，本用例证明那一行不是装饰："
            "把 INV-P01 的钩子换成一个不存在的用例名 ⇒ 必须红（否则「测试早已被删、文档还在指它」"
            "这种最典型的腐烂又会变成静默的）。",
        file=ROOT / "docs" / "protocol-invariants.md",
        injections=[(
            "`db::tests::message_dedup_by_unique_msg_id`",
            "`db::tests::a_test_that_was_deleted_but_the_doc_still_points_at`",
        )],
        cmd=["node", "scripts/check-invariant-hooks.mjs"],
        cwd=ROOT,
        expect_fail_hint="钩子解析失败",
        tags=["docs", "invariant", "new-guards"],
    ),
    # ---------------- 不变量例外登记（挡住「照文档误修」） ----------------
    # 守的是 `scripts/check-invariant-exceptions.mjs`：代码侧的 `INV-EXCEPTION:` 标记
    # 与 `docs/protocol-invariants.md` §22 登记区必须**双向**一致。
    Case(
        name="不变量例外：代码标了但文档没登记必须报出来（否则会被照文档误修）",
        why="AI 的必读清单（AI_ENGINEERING_INDEX.md）只指向 protocol-invariants.md 与本文件。"
            "一段**正当**的例外若只写在实现旁边、没写进那份文档，读文档的人就会把它当 bug 修掉 ——"
            "「和自己聊天」正是这种：它不进 outbox，「修」成进 outbox 会让那行永远等不到 Ack、"
            "被 flush_outbox 每次心跳重发，把「outbox 必然排空」真的破掉。",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "// INV-EXCEPTION: INV-P03, INV-P04 — 自聊收发双方都是本机",
            "// INV-EXCEPTION: INV-P03, INV-P04, INV-P20 — 自聊收发双方都是本机",
        )],
        cmd=["node", "scripts/check-invariant-exceptions.mjs"],
        cwd=ROOT,
        expect_fail_hint="没登记进不变量文档",
        tags=["invariant", "new-guards"],
    ),
    Case(
        name="不变量例外：文档登记了但代码标记没了必须报出来（否则文档在说谎）",
        why="登记的例外如果代码里已无人声明，要么这段代码的例外成了隐藏事实，"
            "要么例外早已不存在而登记忘了撤 —— 两种都会让文档变得不可信，"
            "而「文档不可信」比「没有文档」更糟：它会让所有不变量一起失效。",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "// INV-EXCEPTION: INV-P03, INV-P04 — 自聊收发双方都是本机，没有对端可等 Ack：",
            "// （标记被删）",
        )],
        cmd=["node", "scripts/check-invariant-exceptions.mjs"],
        cwd=ROOT,
        expect_fail_hint="找不到对应标记",
        tags=["invariant", "new-guards"],
    ),
    Case(
        name="不变量例外：登记里出现文档未定义的 id（笔误会登记出一条不存在的例外）",
        why="例外表里的 id 打错一个数字，就等于凭空登记了一条不存在的例外。"
            "这类笔误不会自己冒出来，只会在某次「照文档排查」时把人带沟里。",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "// INV-EXCEPTION: INV-P03, INV-P04 — 自聊收发双方都是本机",
            "// INV-EXCEPTION: INV-P03, INV-P04, INV-P99 — 自聊收发双方都是本机",
        )],
        # 同时改文档：把这个不存在的 id 也写进登记区，才能把「笔误」单独隔离出来
        # （否则会先以「代码标了没登记」失败，证明不了笔误这条判据本身有效）。
        extra_injections=[(
            ROOT / "docs" / "protocol-invariants.md",
            "<!-- END EXCEPTION REGISTRY -->",
            "| INV-P99 | 探针 | 无 | 探针 |\n\n<!-- END EXCEPTION REGISTRY -->",
        )],
        cmd=["node", "scripts/check-invariant-exceptions.mjs"],
        cwd=ROOT,
        expect_fail_hint="文档未定义",
        tags=["invariant", "new-guards"],
    ),
    # ---------------- 领域图（docs/domains.data.mjs + check-domain-map.mjs） ----------------
    # 地图错了比没有地图更危险 —— 它会被当成事实执行。下面三条守的是"地图不许说谎"里
    # **机器能守**的那部分（`activeHome` 是否属实只能靠人诚实 + 台账里的 file:line 证据）。
    Case(
        name="领域图：enforce 不能开在还有第二个家的领域（边界收口完成一个，打开一个）",
        why="`enforce: true` 表示「该领域边界已是事实、可由机器守住」（例如禁止跨领域直接引用）。"
        "若它还有第二个家（迁移中）就把闸门打开，第一天就会全红 —— 而红门禁会催生绕过，"
        "门禁一旦被绕过一次就永久失效（本项目铁律：第一版门禁必须全绿）。"
        "本用例把 presence 的 enforce 改成 true（它还有未接线的第二个家 discovery/），必须被拦下。"
        "这条把「边界收口完成一个，打开一个」从口号变成机器判定 —— 也是 Phase 6 的前置。",
        file=ROOT / "docs" / "domains.data.mjs",
        injections=[(
            '      secondHome: "src-tauri/src/discovery",\n'
            '      secondHomeStatus: "未接线",\n'
            '      enforce: false,',
            '      secondHome: "src-tauri/src/discovery",\n'
            '      secondHomeStatus: "未接线",\n'
            '      enforce: true,',
        )],
        cmd=["node", "scripts/check-domain-map.mjs"],
        cwd=ROOT,
        expect_fail_hint="还有第二个家",
        tags=["domain", "new-guards"],
    ),
    Case(
        name="领域图：一个文件不许被两个领域认领",
        why="一个文件被两个领域认领 ⇒ 改它时不知道该守谁的规则 ⇒ 规则的**适用范围**本身成了歧义。"
        "本用例把 transport 的活路径文件塞进 presence 的 paths，必须被拦下。",
        file=ROOT / "docs" / "domains.data.mjs",
        injections=[(
            '      paths: [\n'
            '        "src-tauri/src/network/discovery.rs", // 旧家（活）\n'
            '        "src-tauri/src/discovery", // 新家（未接线）\n'
            "      ],\n",
            '      paths: [\n'
            '        "src-tauri/src/network/discovery.rs", // 旧家（活）\n'
            '        "src-tauri/src/discovery", // 新家（未接线）\n'
            '        "src-tauri/src/network/transport.rs", // 注入：该文件已被 transport 认领\n'
            "      ],\n",
        )],
        cmd=["node", "scripts/check-domain-map.mjs"],
        cwd=ROOT,
        expect_fail_hint="被多个领域认领",
        tags=["domain", "new-guards"],
    ),
    Case(
        name="领域图：不许有文件既没归属也没列进 unmapped（无主之地最容易出跨界 bug）",
        why="「没被提到」与「确认不属于任何领域」是两回事：前者是无主之地（谁改都不守规则），"
        "后者是经过思考的豁免。本用例把 style.css 从 unmapped 里删掉（它不会被任何领域认领），"
        "必须报出来 —— 强制那条豁免是**显式**的。",
        file=ROOT / "docs" / "domains.data.mjs",
        injections=[('    ["src/style.css", "全局样式（令牌化设计体系的落点）"],\n', "")],
        cmd=["node", "scripts/check-domain-map.mjs"],
        cwd=ROOT,
        expect_fail_hint="既没被领域认领",
        tags=["domain", "new-guards"],
    ),
    Case(
        name="领域依赖方向：consumes 被误删成空 → 已有 use 立刻穿帮",
        why="messaging 域的 gossip_engine.rs 通过 `use crate::crypto::Identity;` 依赖 identity 域。"
        "本用例把 messaging 的 consumes 从 `[\"identity\"]` 改成 `[]`,守门必须报"
        "「messaging 想依赖 identity」 —— 因为「没声明」与「声明了不需要」是两回事,前者意味着"
        "依赖边界被悄悄擦掉了。判定 `consumes: []` 跟 `enforce: false` 是两套独立的开关:enforce"
        "控制图的形式,consumes 控制图的内容。",
        file=ROOT / "docs" / "domains.data.mjs",
        injections=[(
            'consumes: ["identity"], // gossip_engine.rs 用 crypto::Identity（生产代码）',
            'consumes: [], // TEMP-NON-VACUUM-TEST(messaging):该声明被误删,守门必须报',
        )],
        cmd=["node", "scripts/check-domain-deps.mjs"],
        cwd=ROOT,
        expect_fail_hint="想依赖「identity」域",
        tags=["domain-deps", "new-guards"],
    ),
    Case(
        name="领域依赖方向：consumes 引用了不存在的领域 → FAIL（typo 第一天就该红）",
        why="`consumes: [\"identity\"]` 写错成 `[\"identtity\"]` 这类 typo,在守门放松对"
        "consumes 字段自身合法性做检查时会**完全无害**地通过 —— 直到真正新增一条 use 触发"
        "「不存在的域」才被察觉,届时已经离 typo 隔了 N 个 PR。本用例在 transport 的 consumes"
        "中临时塞一个不存在的 id,直接验证判据 H 必红。修法:把不存在的 id 改回真名。",
        file=ROOT / "docs" / "domains.data.mjs",
        injections=[(
            '        "platform", // transport/ble_android.rs 用 jni_method::kotlin_method\n',
            '        "platform", // transport/ble_android.rs 用 jni_method::kotlin_method\n'
            '        "identtity_typo_will_fail", // TEMP-NON-VACUUM-TEST(transport):错字,守门必报\n',
        )],
        cmd=["node", "scripts/check-domain-deps.mjs"],
        cwd=ROOT,
        expect_fail_hint="引用了不存在的领域",
        tags=["domain-deps", "new-guards"],
    ),
    # ---------------- Change Budget(check-change-budget.mjs + fixture) ----------------
    Case(
        name="Change Budget:只动测试文件的提交不许凑满犯案窗口（#139）",
        why="领域归属原先不认「测试文件不算应用码」这条既有口径（semver.mjs 里为它写过注释：每次补用例"
        "都被迫提版本号 ⇒ 版本号会通胀）。真实后果 2026-09-28 实测到：一条只改 `src/utils/versioning.test.ts`"
        "的提交被算成 presentation 的第 3 票,把「同一领域反复打补丁」这条红推起来了 —— 而写回归正是本仓鼓励的事。"
        "fixture 默认状态是 2 条动应用代码的 transport fix + 1 条只动 `ble_tests.rs` 的 ⇒ 必须仍判 2 次并 PASS；"
        "本用例摘掉那层过滤 ⇒ 三条一起数,守门必须报「出现了 3 次」。",
        file=ROOT / "scripts" / "check-change-budget.mjs",
        injections=[(
            "        .filter((f) => !isExempt(f.path) && isAppCodePath(f.path))",
            "        .filter((f) => !isExempt(f.path))",
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="出现了 3 次",
        tags=["frontend", "change-budget", "new-guards"],
    ),
    Case(
        name="Change Budget:CI 的「空转硬失败」不许打在 fixture 接缝上(打回去必须红)",
        why="2026-09-28 CI 实测:护栏非空转那一步在 frontend 组红了,而**本地同一命令是绿的** —— "
        "成因是那段「拿不到 before..sha 就判空转」的硬失败由 `GOSSLAN_BUDGET_STRICT` + `GITHUB_EVENT_NAME` "
        "门控,只有 CI 有这两个 env;`--from-json` 本来就没有 git 范围(数据是喂进去的),于是每条 fixture "
        "用例的「恢复后即 PASS」半边都被它判成 exit 1。这段判据最近一次改动是 d056422(09-21)、"
        "不是本轮推送带进来的,而 Change Budget 排在护栏之前一步、一红就 fail-fast ⇒ 护栏那步整段标"
        "「未跑」,没人看见过它;直到 keepGoingOnFail 那次改动让它真的跑到。"
        "本用例把 `!fromJson` 摘掉=把缺陷装回去:注入态必须红且报「受检范围」,还原态必须 0。"
        "它同时是那条空转判据自身的非空转证明 —— 没有 env 入口之前,「只在 CI 红」这一类根本无法表达。",
        file=ROOT / "scripts" / "check-change-budget.mjs",
        injections=[(
            "if (ok && strict && ciPushMain && !fromJson && !rangeFromEventBefore) {",
            "if (ok && strict && ciPushMain && !rangeFromEventBefore) {",
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="受检范围",
        env={
            "GOSSLAN_BUDGET_STRICT": "1",
            "GITHUB_EVENT_NAME": "push",
            "GITHUB_REF_NAME": "main",
        },
        tags=["frontend", "change-budget", "new-guards"],
    ),
    # ---------------- force push 后的 before 不可达（2026-10-03 真实事故） ----------------
    Case(
        name="Change Budget:force push 后 before 不可达不得判空转（改回去必须红）",
        why="真实事故 2026-10-03（CI run 37106836002）：发 v4.33.0 时用了 amend + "
            "--force-with-lease，于是事件里的 event.before 是**刚被改写掉的那个提交**"
            "（1231aa2），它不在任何 ref 上，而 CI 是从 GitHub 全新克隆（fetch-depth: 0 "
            "只拉 ref 上的对象）⇒ `git cat-file -e <before>^{commit}` 必然失败 ⇒ "
            "拿不到 before..sha，脚本退到 `HEAD~1..HEAD`。**它确实判到了 1 个 commit 并把"
            "三条判据全跑完**，却仅因「来源不是 before..sha」被判 exit 1 ⇒ 门禁自己兜住了"
            "范围、又自己判这个兜底无效，自相矛盾。`docs/VERSIONING.md` §3 明确允许改写已推送"
            "历史（那条流程第一步就是 amend + force push），所以这不是该拦的形态。"
            "本用例把 force push 的例外分支摘掉=把缺陷装回去：注入态必须 exit 1 且报"
            "「受检范围」，还原态必须 0。与上一条（真·env 没喂必须红）互为对照："
            "**两条都守住，才既不误红又不放松**。",
        file=ROOT / "scripts" / "check-change-budget.mjs",
        injections=[(
            "  if (beforeGivenButUnreachable) {",
            "  if (false && beforeGivenButUnreachable) {",
        )],
        # ⚠️ **不能用 `--from-json`**（2026-10-03 实测踩到：这条 Case 头一次是空转的，
        #   是本文件自己的非空转扫描报出来的）。原因：被守的那段硬失败带 `!fromJson`
        #   条件（`check-change-budget.mjs:707`），而 `--from-json` 模式 `fromJson=true`
        #   ⇒ **整段不执行** ⇒ 注入之后测试照样绿。
        #   与上面那条 [60/66] 的差别也正在这里：它注入的是 `!fromJson` 条件**本身**
        #   （摘掉之后那段被执行，所以咬得住）；而这条摘的是**内层子分支**，
        #   外层的 `!fromJson` 仍然把整段挡掉 ⇒ 静默空转。
        # ⇒ 这条必须用**真实 git 范围**跑（不给 --from-json），整段才会执行。
        cmd=["node", "scripts/check-change-budget.mjs"],
        cwd=ROOT,
        expect_fail_hint="受检范围",
        env={
            "GOSSLAN_BUDGET_STRICT": "1",
            "GITHUB_EVENT_NAME": "push",
            "GITHUB_REF_NAME": "main",
            # ★ 给一个**不可达**的 before：模拟 force push 后的真实形态
            #   （事件里的 before 是刚被改写掉的那个提交，CI 从 GitHub 全新克隆里
            #   拿不到那个对象 ⇒ `git cat-file -e` 失败 ⇒ 拿不到 before..sha）。
            #   全 0 的 sha 保证在任何克隆里都不可达，连本地也一样 ——
            #   用真实的 before 值会**因地而异**（本地仓库里那个对象还在）。
            "GITHUB_EVENT_BEFORE": "0000000000000000000000000000000000000000",
            "GITHUB_SHA": "HEAD",
        },
        tags=["frontend", "change-budget", "new-guards"],
    ),
    # 守门读真实 git 历史,没法"改坏源文件"来验证 —— 所以脚本留了 --from-json 测试接缝,
    # 用 fixture 喂数据。fixture 的默认状态是全 PASS(每条判定路径都走到),下面四条用例
    # 各自破坏一个条件来验证对应判据会红。fixture 本身提交进仓库,是可以 review 的测试数据。
    Case(
        name="Change Budget:L2 改动丢了 [plan] 标记 → FAIL",
        why="超 L1(≤5 文件)但 ≤L2(≤10 文件)的改动,要求 commit message 带 [plan] 说明改动计划 ——"
        "『中改动必须被声明』是 Change Budget 的核心语义。fixture 里 a000002(8 文件/412 行)默认带"
        " [plan: 拆成三步…];本用例把 [plan] 从 message 里删掉,守门必须报「没有 [plan] 标记」。",
        file=ROOT / "scripts" / "fixtures" / "change-budget.json",
        injections=[(
            '"message": "feat(ui): 重构设置面板 [plan: 拆成三步 —— 先抽 store,再拆视图,最后迁 API]",',
            '"message": "feat(ui): 重构设置面板",',
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="没有 [plan] 标记",
        tags=["change-budget", "new-guards"],
    ),
    Case(
        name="Change Budget:L3 改动(含敏感文件)丢了 [impact] 标记 → FAIL",
        why="碰 protocol.rs / crypto.rs 的改动**无论多小**都是 L3(一错就是安全/全库数据问题),"
        "必须有 Impact Report 的最小形态 [impact] 标记。fixture 里 a000003 只改 2 个文件,但因碰了"
        " protocol.rs 直接 L3;本用例删掉 [impact],守门必须红 —— 证明『敏感文件不豁免于规模』。",
        file=ROOT / "scripts" / "fixtures" / "change-budget.json",
        injections=[(
            '"message": "refactor(protocol): 线格式 v2 [impact: 见 docs/protocol-invariants.md 新增小节;两侧同步升级;505 用例全绿]",',
            '"message": "refactor(protocol): 线格式 v2",',
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="没有 [impact] 标记",
        tags=["change-budget", "new-guards"],
    ),
    Case(
        name="Change Budget:同一领域连续 3 次 fix → FAIL(重复犯案检测器)",
        why="4.18.7→4.18.10 连着四个版本修同一个 BLE 分片问题,每个补丁都很小但它们在互相修 ——"
        "『改完这个冒出那个』的特征不是 diff 大,而是同一领域反复被打补丁。fixture 窗口里 transport"
        " 已有 2 次 fix(阈值 3);本用例注入第 3 条 transport fix,守门必须报「出现了 3 次」并提示"
        "先补不变量/收敛单一事实来源。",
        file=ROOT / "scripts" / "fixtures" / "change-budget.json",
        injections=[(
            '"message": "fix(ble): 写入失败日志补帧长",\n      "files": [{ "path": "src-tauri/src/transport/bluetooth.rs", "add": 24, "del": 2 }]\n    },',
            '"message": "fix(ble): 写入失败日志补帧长",\n      "files": [{ "path": "src-tauri/src/transport/bluetooth.rs", "add": 24, "del": 2 }]\n    },\n'
            '    {\n      "sha": "b000009",\n      "message": "fix(ble): 第三次打补丁",\n      "files": [{ "path": "src-tauri/src/transport/tcp.rs", "add": 5, "del": 1 }]\n    },',
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="出现了 3 次",
        tags=["change-budget", "new-guards"],
    ),
    Case(
        name="Change Budget:版本清单文件不吃改动半径预算(把豁免摘掉必须红)",
        why="2026-09-28 负责人拍板的口径：判据 4 **强制**每条动应用码的提交都同时改那四个版本清单文件,"
        "而判据 1 又把它们数进「改了几个文件」⇒ **同一道门禁的一条判据在制造另一条判据的红**。"
        "实测代价：近 12 条含产品码的提交里 3 条被这样顶到 L3(要求 [impact]),其中 c58c425 真实只动"
        " 3 个产品文件、计入却是 11 个(5 个版本清单 + 3 个测试) ⇒ CI 连着红了三趟。"
        "现在这五个文件(那四个 + Cargo.lock)**永不计入规模**;判据 4 照旧读它们,一条没少判。\n"
        "     注入：把规模判据里那条豁免摘掉 ⇒ fixture 里 a000004(发版提交,package-lock +250 行等)"
        "立刻被算进去 ⇒ 347 行超 L1 且标题没有 [plan] ⇒ FAIL。证明这条豁免是**承重的**,不是装饰。\n"
        "     ⚠️ 它顶替的是原来那条「chore(release) 换成普通类型 → 版本白名单失效 → FAIL」："
        "那条守的是「豁免是声明出来的、不是永远免检」,而**这个前提刚被负责人推翻** ⇒ 留着就是假守卫。"
        "它原本顺带守住的另一半(发版提交改名不许静默放行)由下一条判据 4 反向用例继续守。",
        file=ROOT / "scripts" / "check-change-budget.mjs",
        injections=[(
            '    (f) => !isExempt(f.path) && !VERSION_MANIFESTS.has(f.path.replaceAll("\\\\", "/")),',
            '    (f) => !isExempt(f.path),',
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="没有 [plan] 标记",
        tags=["change-budget", "new-guards"],
    ),
    Case(
        name="Change Budget:声明了 Version-Bump 却没动版本文件 → FAIL",
        why="真实缺口：PR #22/#23 的 e5770ce、f6bb81c 两条都写了 `Version-Bump:` trailer，"
        "但四个版本清单文件一个没动 —— 版本最后是 v4.23.0 手工补的账。trailer 从 4.23.2 起"
        "是判据 3 的输入（用它判「这次是不是修补」），一个可以随便写的声明 = 门禁读的是装饰。"
        "fixture 里 a000001 只动 3 个文件、没有 trailer ⇒ 默认 PASS；注入 trailer 后守门必须红。",
        file=ROOT / "scripts" / "fixtures" / "change-budget.json",
        injections=[(
            '"message": "fix(chat): 复制长链接",',
            '"message": "fix(chat): 复制长链接\\nVersion-Bump: patch",',
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="声明没落地",
        tags=["change-budget", "new-guards"],
    ),
    Case(
        name="Change Budget:四个版本文件都动了却没声明 → FAIL(反向也判)",
        why="只判『声明了没做』会留一个洞：不写 trailer 就能同时躲开判据 4 和判据 3 的窗口。"
        "所以反方向一起判。fixture 里 a000004 动满四个版本文件、靠 `chore(release)` 前缀豁免声明；"
        "本用例把它改成 `chore(deps)` + 带 [plan][impact] —— 规模判据此时仍然放行(它是 L2 且有标记)，"
        "因此红只可能来自『真 bump 缺声明』这一条，是判据 4 反向分支的干净判别器。",
        file=ROOT / "scripts" / "fixtures" / "change-budget.json",
        injections=[(
            '"message": "chore(release): v4.19.0",',
            '"message": "chore(deps): v4.19.0 [plan] [impact]",',
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="却没声明",
        tags=["change-budget", "new-guards"],
    ),
    Case(
        name="Change Budget:`feat` 前缀 + patch 声明照样进犯案窗口",
        why="窗口原先按 `--grep=^fix` 选提交 ⇒ 把一条修复写成 `feat(...)` 就永久看不见它"
        "(真实形状：`feat(ui): 主题色体系 + …… + 群任务编辑权限修复`)。现在按声明选："
        "`Version-Bump: patch` = 作者断言没有新能力 ⇒ 无论前缀是什么都算修补形状。"
        "本用例注入的第 3 条 ble 提交**故意写成 feat** —— 若有人把窗口改回只看前缀，"
        "上面那条 `fix(ble)` 注入用例照样会红，只有这条会不红，所以它守的是这条新口径本身。",
        file=ROOT / "scripts" / "fixtures" / "change-budget.json",
        injections=[(
            '"message": "fix(ble): 写入失败日志补帧长",\n      "files": [{ "path": "src-tauri/src/transport/bluetooth.rs", "add": 24, "del": 2 }]\n    },',
            '"message": "fix(ble): 写入失败日志补帧长",\n      "files": [{ "path": "src-tauri/src/transport/bluetooth.rs", "add": 24, "del": 2 }]\n    },\n'
            '    {\n      "sha": "b000004",\n      "message": "feat(ble): 又调了一档预算\\nVersion-Bump: patch",\n'
            '      "files": [{ "path": "src-tauri/src/transport/tcp.rs", "add": 5, "del": 1 }]\n    },',
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="出现了 3 次",
        tags=["change-budget", "new-guards"],
    ),]
