#!/usr/bin/env python3
"""护栏非空转用例分册：前端呈现层（components / layouts / entries / 样式与打包配置里的界面判据）。

本册 30 条 / 485 行，2026-10-07 从 `scripts/verify-guards.py`（原 4,176 行、202 条挤在一份
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
    # ---------------- 本地新增护栏（2026-09-14）----------------

    Case(
        name="Win11 窗口字形都在（删掉它最大化按钮渲染为空）",
        why="2026-09-17 起窗口按钮按 Win11 细线造型自绘（data-win-glyph 标记）；"
            "此前 0e07dd4 删过图标 import 而 Windows 分支仍在用 ⇒ 按钮渲染为空。"
            "这类退化不报错、不影响构建，只有这条守卫能拦住",
        file=ROOT / "src" / "components" / "TitleBar.vue",
        injections=[(
            'data-win-glyph="maximize"',
            'data-win-glyph="maximize-x"',
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/utils/titleBarIcons.test.ts"],
        cwd=ROOT,
        expect_fail_hint="字形",
        tags=["frontend", "new-guards"],
    ),
    Case(
        name="@提及 兜底色不许被抄回组件（底色事实只有一份）",
        why="v4.23.2 删掉的两处硬编码兜底（气泡 #1c2434/#eeeef0、全文弹窗 #1e293b/#ffffff）"
            "抄的是主题 token 的值、且两处不一致 ⇒ token 一改组件静默失准。"
            "这类退化不报错、不影响构建，只有扫源码的断言能拦住",
        file=ROOT / "src" / "components" / "message" / "MessageTextBubble.vue",
        injections=[(
            "mentionHighlightColor(app.themeColor, app.dark, bg)",
            'mentionHighlightColor(app.themeColor, app.dark, bg || "#1c2434")',
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/utils/chatStyle.test.ts"],
        cwd=ROOT,
        expect_fail_hint="兜底色",
        tags=["frontend", "new-guards"],
    ),
    # ---------------- 前端：静态设计护栏 ----------------
    Case(
        name="键盘可达（div @click 必须报出）",
        why="`div @click` 触屏/鼠标能用但键盘够不着、读屏念成普通文本",
        file=ROOT / "src" / "components" / "settings" / "AboutSection.vue",
        injections=[(
            "<div class=\"px-4 py-3\">",
            "<div class=\"px-4 py-3\">\n      <div class=\"cursor-pointer\" @click=\"onFingerprintTap\">x</div>",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="键盘够不着",
        tags=["frontend", "a11y"],
    ),
    Case(
        name="跨窗口的定时器必须有卸载出口（VirtualList 落定轮询）",
        why="PR #24 给 `scrollToIndex` 加了 1.5s 的落定轮询（100ms 一次），而自动收口只写在\n"
        "     `applyJump` 内部，它开头是 `if (!j || !el) return;` —— `el` 是容器 ref，组件卸载后\n"
        "     Vue 把它置 null ⇒ 那条过窗自清的分支永远走不到，定时器以 10Hz 常驻在已死的组件上，\n"
        "     且不会自愈（关掉带列表的辅助窗口、切会话正好落在窗口里就漏一条）。\n"
        "     注入方式：把 `onBeforeUnmount` 里那三行显式清理缩成一句赋值 —— 行为照常、类型照过，\n"
        "     只有静态守卫会红（它钉的是「卸载必须有出口」这件事，不是那段轮询代码怎么写）",
        file=ROOT / "src" / "components" / "VirtualList.vue",
        injections=[(
            "  if (jumpTimer) {\n"
            "    window.clearInterval(jumpTimer);\n"
            "    jumpTimer = 0;\n"
            "  }",
            "  jumpTimer = jumpTimer;",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="100ms 轮询不会停",
        tags=["frontend", "lifecycle"],
    ),
    Case(
        name="预览 objectURL 的消费者不得就地 revoke（⑭守卫必须真能抓回来）",
        why="PR #24 把 revoke 从四个消费组件里删掉，并在 `designGuards.test.ts` ⑭ 钉住：\n"
        "     预览 URL 是缓存里同 cid/msg_id **共用的同一个字符串**，任何一处卸载 revoke 都会\n"
        "     把别处的图一起打裂，而且缓存里那个 URL 已死仍被命中 ⇒ 连退避重试都救不回来。\n"
        "     那条测试本身是他写的、当场能红，但它没进非空转用例集 —— 按本仓库的规矩\n"
        "     （v4.22.31 那条假证明之后）新守卫必须证明「改坏一定 FAIL」。这里补上：\n"
        "     注入 = 把历史上那行原样放回 TodoImageThumb 的卸载钩子，必须被 ⑭ 抓住。",
        file=ROOT / "src" / "components" / "TodoImageThumb.vue",
        injections=[(
            "onBeforeUnmount(cancelRetry);",
            "onBeforeUnmount(() => {\n"
            "  cancelRetry();\n"
            "  if (url.value) URL.revokeObjectURL(url.value);\n"
            "});",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="里出现了 revokeObjectURL",
        tags=["frontend", "lifecycle", "image"],
    ),
    Case(
        name="常驻群任务窗口换群必须靠 :key 重挂（护栏早就在，这是第一次证明它会红）",
        why="`channelState.test.ts` 里那条 `assert.match(win, /<GroupTasksBoard\\s+:key=\"groupId\"/)`\n"
        "     不是新写的，它一直都在 —— 但它**从没进过本脚本** ⇒ 按本仓的规矩（「每条护栏都要被\n"
        "     证明会失败」）它当时只是一条没被证过的锁，和 v4.22.31 那次假证明同形。\n"
        "     这一条为什么值得钉：看板自己那份「清 draft/detailId/pendingDelete + 按来源收预览」的\n"
        "     watch 键在 `props.open` 的翻转上（`GroupTasksBoard.vue:688`），而常驻窗口这条调用\n"
        "     **根本不传 open**（props 里是 `open?: boolean`）⇒ 那条重置在独立窗口里恒不触发，\n"
        "     换群清状态只剩 `:key` 一条路。摘掉它的表现是「在新群里点新建，草稿是上一个群的」，\n"
        "     而 `submitDraft` 读的是当前 `props.groupId` ⇒ 上一个群的半成品会被真的建进这个群。",
        file=ROOT / "src" / "components" / "GroupTodosWindow.vue",
        injections=[(
            '<GroupTasksBoard :key="groupId" ref="boardRef" :group-id="groupId" standalone />',
            '<GroupTasksBoard ref="boardRef" :group-id="groupId" standalone />',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="看板必须按群重挂",
        tags=["frontend", "window", "todo"],
    ),
    Case(
        name="任务缩略图的调用点必须给 clickable（漏一个 prop 的形状是图点不动）",
        why="`TodoImageThumb` 的 `clickable` 默认 **false** ⇒ 调用点漏给它的表现不是报错，而是"
        "「图渲染出来了但点不动」：编译过、类型过、单测也过，只有真人去点才发现。"
        "本仓真发生过两处：收藏那处（已修 `a077850`）、以及**修完那一轮之后聊天时间线的任务卡"
        "仍然漏着的那一面** —— 复审 §12 那一行当时的原话就写着「卡片图 clickable 默认 false」，"
        "说明这一类是被知道的，只是没人把调用点数完。\n"
        "     新护栏按形状数每个 `<TodoImageThumb` 调用点（判据输入由代码自己声明，不手抄名单），"
        "它的 RED 是拿**真缺陷态**跑出来的（修复前那条点名 components/TodoCardBubble.vue）；"
        "本用例把这处破坏登记成可重跑的形态：摘掉卡片调用点的 `clickable` ⇒ 必须红。",
        file=ROOT / "src" / "components" / "TodoCardBubble.vue",
        injections=[(
            "        :image=\"img\"\n        clickable\n        @open=\"openCardImage(i)\"",
            "        :image=\"img\"\n        @open=\"openCardImage(i)\"",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="缩略图渲染出来但点不动",
        tags=["frontend", "image", "todo"],
    ),
    Case(
        name="任务描述的 @ 必须走统一渲染件（退回裸插值必须被点名）",
        why="第二阶段 §9 要的是**同一份 @ 渲染覆盖聊天和任务**。语义早就单源在 `utils/linkify`，"
        "缺的是覆盖面：任务卡与任务详情里是 `{{ todo.description }}` 这种裸插值 ⇒ 描述里 @ 到我时"
        "既不高亮、也不显示「@你」，与同一条消息在聊天里的表现不一致（§6① 那一类：同一个判断两处各画一遍）。"
        "新护栏按形状数任务这一族的描述渲染点（归属由代码自己声明：引用 TodoItem/parseTodo），"
        "它的 RED 是拿**真缺陷态**跑出来的（迁移前点名 components/TodoCardBubble.vue 与 TodoDetailDialog.vue）。"
        "本用例把那处破坏登记成可重跑形态：把卡片的渲染件退回裸插值 ⇒ 必须红。",
        file=ROOT / "src" / "components" / "TodoCardBubble.vue",
        injections=[(
            '      <MentionText\n'
            '        :text="todo.description"\n'
            '        :mention-names="mentionNames"\n'
            '        :self-mention="selfMention"\n'
            '      />',
            '      {{ todo.description }}',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="这些任务描述是裸插值",
        tags=["frontend", "mention", "todo"],
    ),
    Case(
        name="查看者视角的 @ 文案不许写进生产码（写死就是把自我视角烧进公共文案）",
        why="「@你」有两种相反方向的破坏：① 渲染端图省事直接把文案写死（换语言/换视角就错）；"
        "② 更坏的在**发送或落库侧替换文案**，那等于把我的视角烧进公共数据，对端与历史跟着错 —— "
        "方向②由群聊轮那条跨进程判据钉（库里那串字节必须逐字等于原文），本条钉方向①："
        "文案唯一来源是 `src/i18n/locales.ts`，生产码只能 `t(\"mention.self\")` 取。"
        "注入形态是真会写出来的那种：渲染件里对 `mention-self` 段直接三元写死字面量。",
        file=ROOT / "src" / "components" / "message" / "MentionText.vue",
        injections=[(
            '    <span v-else>{{ seg.value }}</span>',
            '    <span v-else>{{ seg.kind === "mention-self" ? "@你" : seg.value }}</span>',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="把查看者视角的文案写死了",
        tags=["frontend", "mention", "i18n"],
    ),
    Case(
        name="焦点可见（outline-none 必须有自己的焦点指示）",
        why="全局焦点环写在 `:where()` 里（特异性 0），会被 `.outline-none`（特异性 0,1,0）"
        "静默覆盖 —— 7 处输入框（含最高频的消息输入框）因此完全没有焦点指示，"
        "而代码看起来「有全局规则在管」（真实缺陷 2026-09-12）。"
        "⚠️ 2026-09-16 换了注入点：原先注入 `MessageComposer.vue`，而该文件后来因用户要求"
        "（消息输入框不画焦点环）加了**文件级** `focus-ring-ok` 逃生阀 ⇒ 整个文件被跳过、"
        "本用例退化成空转（改坏也不报，2026-09-16 由 verify-guards 全量跑发现）。"
        "改注入 `TitleBar.vue` 的关闭按钮：未被豁免，且它是**键盘可聚焦的 button** —— "
        "正是这条护栏真正要保护的场景（键盘用户看不到焦点在哪）。"
        "MessageComposer.vue 整文件失去本条覆盖的问题，另行按元素级逃生阀处理。",
        file=ROOT / "src" / "components" / "TitleBar.vue",
        injections=[(
            "flex w-11 items-center justify-center text-[var(--gosslan-rail-text)] "
            "transition hover:bg-[var(--gosslan-danger)] hover:text-white",
            "flex w-11 items-center justify-center text-[var(--gosslan-rail-text)] "
            "transition hover:bg-[var(--gosslan-danger)] hover:text-white outline-none",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="静默覆盖",
        tags=["frontend", "a11y"],
    ),
    Case(
        name="焦点可见：MessageComposer 里未豁免的元素也必须被守到（豁免不得外溢）",
        why="2026-09-16 发现：该文件为「消息输入框不画焦点环」这**一个元素**的需求用了**文件级**"
        "逃生阀 ⇒ 整个文件（含「取消引用」按钮等键盘可聚焦元素）一起失去本条保护，"
        "而注入该文件的旧用例因此退化成空转（改坏也不报）。改成元素级 `data-focus-ring-ok` 后，"
        "本用例把**未**打标记的那个按钮改坏，必须报出来 —— 它同时钉住两个坑："
        "① 元素级豁免不得外溢到同文件其它元素；② 文件级令牌不能是元素级令牌的子串"
        "（`data-focus-ring-ok` 含有 `focus-ring-ok`，所以文件级必须写成 `focus-ring-ok:file`，"
        "否则「只豁免一个元素」会被判成「整文件豁免」）。",
        file=ROOT / "src" / "components" / "chat" / "MessageComposer.vue",
        injections=[(
            "flex h-5 w-5 shrink-0 items-center justify-center "
            "rounded-[var(--gosslan-radius-xs)] transition hover:bg-[var(--gosslan-hover)]",
            "flex h-5 w-5 shrink-0 items-center justify-center "
            "rounded-[var(--gosslan-radius-xs)] transition hover:bg-[var(--gosslan-hover)] outline-none",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="静默覆盖",
        tags=["frontend", "a11y", "new-guards"],
    ),
    Case(
        name="触屏点按目标（小按钮必须有 tap-safe）",
        why="HIG 最小 44pt；小图标按钮手指容易点不中或误触相邻项",
        file=ROOT / "src" / "components" / "FriendProfile.vue",
        injections=[("class=\"tap-safe flex h-8 w-8", "class=\"flex h-8 w-8")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="tap-safe",
        tags=["frontend", "mobile"],
    ),
    Case(
        name="截断文本要有可访问名（truncate 必须有 title）",
        why="被截断的完整内容鼠标悬停拿不到、读屏也可能拿不到",
        file=ROOT / "src" / "components" / "settings" / "AboutSection.vue",
        injections=[(
            '<div class="px-4 py-3">',
            '<div class="px-4 py-3">\n      <div class="truncate">完整名字很长很长</div>',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="truncate",
        tags=["frontend", "a11y"],
    ),
    Case(
        name="聊天区文本选择契约（可选中 / 头像不可选 / 长按容差）",
        why="用户 2026-09-13 实测的三件静默退化：PC 上拖选气泡「刚选中立马取消」、"
        "移动端选中文字后不弹「复制」工具条、头像能被拖进选区。这些都是"
        "「代码看着对、用户一用就不对」，改坏了不会报错，只能靠护栏盯住",
        file=ROOT / "src" / "App.vue",
        injections=[(
            "    const sel = window.getSelection();",
            "    // 回归：不再检查选区（有选中文字时也 preventDefault）",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="contextmenu",
        tags=["frontend", "selection"],
    ),
    Case(
        name="操作面板：点任何一项都要收起（引用/转发会跳到别处）",
        why="用户 2026-09-13 安卓实测：「点『引用』这个 sheet 应该自动隐藏；点『转发』也应该"
        "自动隐藏，因为它会跳转到界面内去操作聊天」。不在 ActionSheet 面板层统一收，就得每个"
        "入口各写一遍（漏一个：点『保存图片』这类也一样挂着），而且跳转后的界面会被它挡住",
        file=ROOT / "src" / "components" / "ActionSheet.vue",
        injections=[(
            "            @click=\"emit('close')\"\n          >",
            "          >",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="必须在点击时收起",
        tags=["frontend", "mobile", "selection"],
    ),
    Case(
        name="长按面板：正文气泡里必须弹得出、抬手指不许把它关掉",
        why="用户 2026-09-13 Android 实测两条：「长按气泡有的时候弹不出来、有的时候能弹出来」"
        "（命中 `.gosslan-selectable` 就不起长按 ⇒ 只有按到气泡内边距才弹）与"
        "「弹出 sheet 之后一放手立马就缩回去了」（HeadlessUI 的 outside-click 在 document "
        "捕获阶段挂 `touchend`，而 touch 的 target 在 touchstart 就定死成那条消息 ⇒ 抬手被"
        "判成点了外面）。两条都是「换个位置按/按慢一点就正常」，只能靠判据单测 + 结构护栏",
        file=ROOT / "src" / "components" / "MessageItem.vue",
        injections=[(
            "  if (!shouldSwallowLongPressRelease({ openedByHeldPress: longPressHeld, sheetOpen: sheetOpen.value })) {\n",
            "  if (false) {\n",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="吞不吞要走纯判据",
        tags=["frontend", "mobile", "selection"],
    ),
    # ---------------- 前端：会话列表不随输入变化（⑥） ----------------
    Case(
        name="会话列表不随输入变化（⑥）",
        why="用户 2026-09-12 要求：「在上面输入，列表就不要有变化了。回车弹窗之后，在弹窗里面搜就行」",
        file=ROOT / "src" / "components" / "ConversationList.vue",
        injections=[(
            "const listConversations = computed(() => chat.conversations);",
            "const listConversations = computed(() => chat.conversations.filter((c) => c.name.includes(query.value)));",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="列表必须是全量",
        tags=["frontend", "search"],
    ),
    # ---------------- 前端：Headless UI 模板插槽 ----------------
    Case(
        name="as=template 插槽不得有注释（dev 保留注释 ⇒ 渲染抛错 ⇒ 窗口卡死）",
        why="用户实测：点「+ → 添加好友」整个窗口卡死的真因 —— BaseModal 在 TransitionChild 插槽里放了注释",
        file=ROOT / "src" / "components" / "BaseModal.vue",
        injections=[(
            '<TransitionChild\n            as="template"',
            '<TransitionChild\n            as="template"\n          >\n            <!-- 注入的注释 -->',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="HTML 注释",
        tags=["frontend", "render"],
    ),
    Case(
        name="窗口骨架（设置窗口必须带自己的骨架类）",
        why="三个窗口共用一份骨架 CSS，靠 `<html class=\"boot-settings\">` 决定显示哪一套；"
        "类名漏了那个窗口就只剩白屏骨架（功能正常、但启动那一下很难看）",
        file=ROOT / "settings.html",
        injections=[('<html lang="zh-CN" class="boot-settings" ', '<html lang="zh-CN" ')],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="boot-settings",
        tags=["frontend", "window"],
    ),
    Case(
        name="开窗接线（按钮必须走单飞入口）",
        why="三处入口（窄导航头像 / 移动端底栏 / 原生菜单）必须共用同一份单飞+防抖状态；"
        "退回成按钮里直接 invoke 就是用户报的「连点会开出第二个窗口」",
        file=ROOT / "src" / "layouts" / "ResponsiveLayout.vue",
        injections=[
            (
                'void launchAuxWindow("settings", () => api.openSettingsWindow())',
                "void api.openSettingsWindow()",
            )
        ],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="launchAuxWindow",
        tags=["frontend", "window"],
    ),
    Case(
        name="常驻窗口（设置窗口重新显示必须刷新环境数据）",
        why="独立设置窗口改成常驻（关闭=隐藏）之后不再重新加载；若只在首次加载时取一次数据，"
        "用户切了 Wi-Fi/换了共享目录再打开设置会看到旧快照 —— 这是'常驻'引入的新退化面",
        file=ROOT / "src" / "entries" / "settings.ts",
        injections=[
            (
                "  void getCurrentWindow().onFocusChanged(({ payload: focused }) => {\n"
                "    if (focused) void useAppStore().refreshEnvironment();\n"
                "  });",
                "  // （非空转验证：这一块被临时移除）",
            )
        ],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="设置窗口要监听重新获得焦点",
        tags=["frontend", "window"],
    ),
    Case(
        name="窗口骨架（群任务窗口必须带自己的骨架类）",
        why="三个窗口共用一份骨架 CSS，靠 `<html class=\"boot-todos\">` 决定显示哪一套；"
        "类名漏了那个窗口就只剩白屏骨架（功能正常、但启动那一下很难看）",
        file=ROOT / "todos.html",
        injections=[('<html lang="zh-CN" class="boot-todos" ', '<html lang="zh-CN" ')],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="boot-todos",
        tags=["frontend", "window"],
    ),
    # ── #27 移动端首屏布局（2026-09-25）：四条各盯一个「少了它手机就当桌面用」的形状 ──
    Case(
        name="移动布局判据必须写进 html.is-mobile（CSS 侧唯一的读端）",
        why="isMobile 只活在 JS 里的话，结构级断点就无从让路 —— 而手机首帧的视口宽度会说谎"
        "（启动时系统权限弹框盖住 WebView，那一刻读到 980px 兜底档）。那个类是 JS 与 CSS 之间"
        "唯一的接缝，写的人少一句，CSS 侧全部 desktop: 叠加同时失效，且不产生任何编译错误",
        file=ROOT / "src" / "stores" / "useAppStore.ts",
        injections=[(
            '      document.documentElement.classList.toggle("is-mobile", isMobile.value);\n',
            "",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="html.is-mobile",
        tags=["frontend", "mobile-layout", "ui"],
    ),
    Case(
        name="结构级断点必须叠 desktop:（导航栏那条）",
        why="导航栏只写 `hidden md:flex`：手机首帧读到兜底视口宽度时导航栏直接回来，"
        "与 JS 判出的移动布局互相打脸。叠成 desktop:md:flex 才等于「桌面且够宽」；"
        "少叠一次就是一个「手机上多出桌面导航栏」的现场",
        file=ROOT / "src" / "components" / "NavRail.vue",
        injections=[("py-3 desktop:md:flex", "py-3 md:flex")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="desktop:",
        tags=["frontend", "mobile-layout", "ui"],
    ),
    Case(
        name="desktop 变体必须在 tailwind 注册（拼错的后缀会静默不生效）",
        why="Tailwind 对不认识的变体是静默丢掉整条工具类：不报错、不生成规则，界面只是"
        "「少了一点样式」。所以写 desktop: 的人必须有人回头查注册还在不在",
        file=ROOT / "tailwind.config.js",
        injections=[('addVariant("desktop", "html:not(.is-mobile) &");', "// removed by 非空转验证")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="desktop",
        tags=["frontend", "mobile-layout", "ui"],
    ),
    Case(
        name="peers 必须走 mergePeerList（不许退回整表直写）",
        why="peers-updated 最多每秒 3 次，整表直写会换掉数组和里面每个对象 ⇒ 凡读过 peers 的"
        "渲染（消息行模板里的 nicknameOf）每 333ms 全部作废一次，与这一拍有没有真的变化无关。"
        "合并函数把「没变就不赋值」变成可单测的判据，绕开它就是再把那次重画请回来",
        file=ROOT / "src" / "stores" / "useChatStore.ts",
        injections=[
            (
                "        const merged = mergePeerList(peers.value, p);\n"
                "        if (merged) peers.value = merged;\n",
                "        peers.value = p;\n",
            )
        ],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="mergePeerList",
        tags=["frontend", "mobile-layout", "ui"],
    ),
    Case(
        name="群任务窗口不得初始化聊天事件（否则重复通知/未读/回执）",
        why="独立窗口跑聊天 store 的 init 会注册第二套后端事件监听 —— 与主窗口重复，用户会收到"
        "重复通知、未读数翻倍、群已读回执重复发（见 src/App.vue 顶部说明）。"
        "2026-09-24 取数搬进根组件后，入口连 `useChatStore` 都不该出现（判据同步收紧），"
        "所以锚点从原来的 `const chat = useChatStore();` 换成挂载那一行。",
        file=ROOT / "src" / "entries" / "todos.ts",
        injections=[
            (
                "void mountAuxWindow(createWindowApp(GroupTodosWindow), () => useAppStore().init());",
                "void useChatStore().init();\n"
                "void mountAuxWindow(createWindowApp(GroupTodosWindow), () => useAppStore().init());",
            )
        ],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="不该引用 useChatStore",
        tags=["frontend", "window"],
    ),
    Case(
        name="外链开窗必须走单飞入口（不得裸 invoke）",
        why="与设置/日志同一条：绕过 `launchAuxWindow` 就退化成『连点发多次 IPC』，"
        "而后端是单例复用 —— 第二次点击会把已打开的窗口 navigate 到同一网址，用户看到闪一下",
        file=ROOT / "src" / "layouts" / "ResponsiveLayout.vue",
        injections=[
            (
                'launchAuxWindow("link", () => api.openLinkWindow(link.url, link.name))',
                "api.openLinkWindow(link.url, link.name)",
            )
        ],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="launchAuxWindow",
        tags=["frontend", "window"],
    ),
    Case(
        name="应用样式（三个窗口都必须加载 style.css）",
        why="真实缺陷：一窗一入口重构时漏掉了 `import \"./style.css\"`，dev 起来整个界面\"像没有 CSS\"，"
        "而且不报错、不影响任何测试 —— 只有这条守卫能拦住",
        file=ROOT / "src" / "boot" / "boot.ts",
        injections=[('import "@/style.css";', "// （非空转验证：这一行被临时移除）")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="style.css",
        tags=["frontend", "style"],
    ),
    Case(
        name="输入框不再把 @ 名单随 send 交出去（第①段接缝断掉）",
        why="选择器插进 DOM 的 `data-mention-id` 是「@ 的就是这个人」唯一的权威来源；\n"
        "     emit 时漏掉它，后面五段全都白接。这一条与上一条同形（跨语言链的两端），\n"
        "     两条都钉住才谈得上「断在哪一段就报哪一段」。",
        file=ROOT / "src" / "components" / "chat" / "MessageComposer.vue",
        injections=[(
            'emit("send", { content: text, kind: k, mentionIds });',
            'emit("send", { content: text, kind: k });',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="① 输入框没把名单随 send 交出去",
        tags=["frontend", "group", "stability", "new-guards", "mention-identity"],
    ),]
