# Changelog

本项目遵循[语义化版本 SemVer](https://semver.org/lang/zh-CN/)：

- **major**：破坏性变更 / 架构级重构（不向后兼容）
- **minor**：新增功能（向下兼容）
- **patch**：Bug 修复与细节优化

版本号统一由 `npm run version:patch|minor|major` 维护，一次改动同步 `package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/tauri.conf.json` 五处，并把本文件 `[Unreleased]` 小节落为带日期的版本小节。

> 台账分卷（2026-10-07）：4.31.41 及更早的版本小节已整段搬到 [`docs/notes/changelog-archive.md`](docs/notes/changelog-archive.md)（只搬不改，条目文字一字未动）。
> 本文件继续只收未发布小节与 4.32 及以后的版本 —— 新的更新说明一律写进上面那一节，不往归档追加。

## [Unreleased]

## [4.33.21] - 2026-10-07

### Fixed

- ★ **表情回应条与气泡对不齐（左右各差 12px）、右列 hover 的「谁点的」名单被窗口右缘裁掉、
  气泡与胶囊之间偏松**（用户 2026-10-07 两条观感：「电脑端 右边聊天 hover 表情 人员列表 展开遮挡」
  与「表情和气泡对齐：左右两条表情回复边缘都与上面气泡对齐，并且间距紧凑点」）。
  三处根因都量出来了，不是猜的：
  1. **补偿值算错**：回应条是消息行的**兄弟节点**，要让出「行的 `px-4`(16) + 头像 `w-9`(**36**) +
     行 `gap-2`(8)」= **60px** 才压到气泡边上；册里写的是 48（注释按"头像 40 + 间距 8"算，
     而头像其实是 36，还整个漏掉了行的 16px）⇒ 真浏览器现读 `chip.right - col.right = +12px`、左列 −12px。
  2. **同属性两档 class 互相覆盖**：外层挂 `pr-1`(4px)、册内挂 `pr-12`(48px)，两档都落在**同一个元素**上
     ⇒ 生效的只有 48 那一档（实测 `padding-right: 48px`，那 4px 静默消失）。
     这条形状比算术错更阴：两处各自看都对，合起来少一层。
  3. **名单锚点不分朝向**：两侧都写 `left-0` ⇒ 名单永远从胶囊左缘往右长；右列的胶囊本来就贴着右缘，
     于是整块越界（现读：宽 128px 的名单**右溢 37px**，正是截图里被切掉那一截）。
  修法：补偿收成**一个** `pl-[60px]` / `pr-[60px]`（不再叠第二档）、去掉条自己的 `mt-1`
  ⇒ 竖向间距 10px ⇒ **6px**（剩下那 6px 是消息行自己的 `pb-1.5`，它同时是选中底色的下沿，刻意不动）。
  ⚠️ 这一轮给名单写的"按 `mine` 翻转锚点"**当天就被下面第二、三轮换掉了**（按消息朝向写死 ≠ 按位置现算），
  条目留着是为了记下那个判据为什么不够。

- ★ **同一批里第二、三轮：名单浮层仍被遮挡 ⇒ 换成 Teleport + fixed，四个方向都按位置现算；
  宽度改成按名字伸缩；去掉「最新」标签**（用户 2026-10-07 连着追的两轮：「这个列表还是会被遮挡」
  「左边右边不能特别固定的就往左偏或者往右偏，而是看这个图标的位置靠近哪边就往反方向偏」
  「名字很长很长的话也要做省略」「框框长度是固定长度的，名字不够长后面有空白…希望是伸缩长度，
  又有一个最大长度」「最新的那个标签去掉」）。三个根因各自独立：
  1. **`position: absolute` 逃不出 `overflow: hidden`**：第一轮的锚点翻转只解决了"往哪边长"，
     没解决"被谁裁"—— 名单和胶囊都在消息列表那个 `overflow-hidden` 容器里
     （`ChatWindow.vue` 的消息区），只要浮层越出容器边就一定被切。
     表情选择器与已读成员弹层早就为同一个原因改成 Teleport + fixed，这一处当时只修了朝向 ⇒
     **同一个坑两处各修一次**。
  2. **朝向 ≠ 位置**：一群回应有十几个时胶囊会**折行**，同一侧（同一朝向）的一排里既有贴左缘的
     也有贴右缘的 ⇒ 按 `mine` 翻转必然在某一侧再撞一次边界。改成按**这颗胶囊中心离视口哪半近**现算，
     纵向同理按视口中线翻。
  3. **写死宽度 = 短名字留白**：原来 `min-w-[8rem] max-w-[16rem]` 加固定锚点，名字短时右边一截空白。
     改成只给 `max-width` ⇒ 盒子按内容撑开，超出上限才交给行内 `truncate` 出省略号（`title` 兜全名）。
     ⚠️ 这一步带出一个不显然的连带约束：**宽度一伸缩，就必须钉住"会靠住的那条边"**。
     往左长那一支若仍发 CSS `left`，内容一窄右缘就够不到胶囊右缘 ⇒ 浮层与锚点脱开；
     所以那一支改发 CSS `right`（夹位仍按最宽那一档算，内容只会更窄 ⇒ 只会更往里，不可能外溢）。
     这条光测"不越界"抓不到，`hoverCard.test.ts` 里单独钉了"右缘正好落在锚点右缘"。
  顺带：名单不再在末位标「最新」—— 顺序本身就是追加序（先点的在上、后点的在下），
  末位再挂标签是重复表达；两个语言包里那个键随之删掉。

### Changed

- **浮层摆位收成通用的一套**：新增 `utils/hoverCard.placeCard`（纯几何：四向翻 + 夹进视口 +
  `max-width`）与 `composables/useHoverCard`（开/关 + "一滚就收、但忽略浮层自己的内部滚动"），
  表情名单与**已读成员弹层**都改为消费它 ⇒ 那段 scroll/resize 监听与"忽略自己滚动"的判断从两份变一份。
  已读弹层顺带拿到左右翻转（此前写死"永远右对齐"）。
  ⚠️ **表情选择器（`MessageItem.positionReactionPicker`）本轮刻意不迁**：它是点击展开、
  要配合 `useExclusivePopup` 与 document click 一起收，且固定 360px 左对齐是它自己注释里写明的取舍；
  为省十几行样板去动一条有独立生命周期管理的通路，收益不抵风险。它是这套判据现存的第二个家。

### Added

- **UI 运行时探针新增 `roster` 段**（`node scripts/check-ui-runtime.mjs --only=roster`，
  现跑 **25/25 绿**；整条链 `node scripts/check-ui-runtime.mjs` 现跑 **88/88 绿**）：
  左右两列各量「胶囊边 vs 气泡边」（容差 1px）、竖向间距、hover 展开的名单**左右溢出像素**，
  外加反面对照 —— 把条朝外侧挪 8px ⇒ 对齐判据必须不再成立；把名单 fixed 坐标推到视口右缘外
  ⇒ 右溢必须重新出现；两把都验了可逆。多颗那一组还钉"贴左那颗往右长 / 贴右那颗往左长"
  "长名字顶到 256 上限"「短名字那颗必须明显窄于长名字那颗」（两颗互证 ⇒ 宽度写死就当场红，
  不需要额外的反面对照）与"超长昵称那行真被省略号截断且 title 里留了全名"。`--shot=` 存取证帧。
  ⚠️ 这一段自己踩到两条方法论坑，记在这里：
  ① **第一版量的是"回应条那个盒子"的边**，而盒子是通栏的、padding 长在盒子里面
     ⇒ 读出来两侧恒差 60px，差点照着这个假数去改补偿值。用户那句"边缘对齐"判的是**看得见的胶囊**，
     量具必须换成 `chip` 的边（读数随即从 60 变成真实的 12）。
  ② **整段顺序跑时真鼠标投递不过来**：名单一次都没弹出，而单独跑 `--only=roster` 全绿。
     二分（新增 `--only=a,b` 支持）定位到是 `emoji` 那一段污染了后续，派发一次真 `mouseenter`
     能正常打开 ⇒ 红的是探针的鼠标投递，不是产品代码。处置是"先试 3 次真鼠标、不行再派发
     `mouseenter` 兜底"，并把**走的哪条路打印出来**（不静默）。代价要写清楚：摆位/裁切/省略号
     判据照样成立（同一元素、同一处理器、同一套 rect），但"浏览器会不会投递 mouseenter"
     这一层在整段跑里没被覆盖，由那 3 次真鼠标 + 静态接线判据兜着。



## [4.33.20] - 2026-10-07

### Changed

- ★ **代码复审那一轮的落地清单（9 条里做了 7 条，两条按实测判为不动）**。逐条都挂了现算的复跑，
  没有一条是"读过觉得没问题"：
  1. **内联耦合棘轮上限按领域口径重钉：13 ⇒ 6**（`scripts/check-domain-deps.mjs`）。
     上限是从"按物理文件"那一版沿下来的，而 2026-10-07 把可见性改成按领域判之后**存量自己掉到了 6**，
     留着 13 等于把三对的 slack 永久留在判据里。反证（不改文件就能红）：
     `GOSSLAN_DOMAIN_INLINE_PAIR_MAX=5 node scripts/check-domain-deps.mjs` ⇒ RC=1；
     注入一条新的内联耦合 ⇒ 打印"去重后 7 对"并判红。恒等：同一份新尺子在拆前/拆后两棵树都报 **30 处 / 6 对**。
  2. ⑥ **花括号 `use crate::{a, b};` 两条扫描都不看** —— `scanInlineCrate` 把 use 行整行让给
     `scanUseCrate`，而后者只认单条路径 ⇒ 这是一条**能绕开上面那个上限**的写法。
     现算今天 0 处 ⇒ 补上是纯收紧（改前改后本脚本输出**逐字节相同**）。
     ⚠️ 我自己的第一版修法就是错的：没先去行内注释，`use crate::{a::b}; // 说明` 照样看不见，
     注入实测 RC=0 —— 是那条注入把它照出来的，不是读代码读出来的。现在三种形状各有反证：
     带注释的花括号 / `pub use crate::{a, b};` / 嵌套 `use crate::{a::{b, c}}`（削成模块前缀）⇒ 全部 RC=1。
  3. ⑦ **BLE 匿名重述判据的扫描面只扩到 `network/ble/`，兄弟册仍然看不见**。
     先量再扩：把面临时扩到**全部生产 rs** 跑真实判据 ⇒ 命中 **0 处**（`grep -c '^  ✗ '` = 0），
     所以按 `network/` + `transport/` 两个目录扩面（点名 6 + 动态展开 ⇒ 现算 47 本）今天不改任何结论。
     不扩到全仓的理由写在脚本注释里：值清单含 `3 / 20`，`MAX_CHUNK_* = 3` 这类**同名不同义**的常量
     会被拉进误伤面，而这条守卫一旦被加白名单就形同虚设。反证：往 `network/file/receive.rs` 注入
     `const FAKE_ATT_HEADER_LEN: u16 = 3;` ⇒ 旧面 RC=0 / 新面 RC=1 且点名那一行。
  4. `verify-guards.py` **起跑前要求每条用例声明 `expect_fail_hint`**（现算 202/202 都有 ⇒ 这条前置
     今天不改结论，改的是"以后加一条没 hint 的用例"那种形状），`base.py` 的字段注释同步改成"必填"。
  5. `ble.rs` 两条读循环守卫的锚点唯一性断言：`body.starts_with("async fn x")` 是**同义反复**
     （`body` 就是从那个 `find` 命中点切的，永远以它开头 ⇒ 判据恒真）⇒ 换成
     `assert_eq!(src.matches("\nasync fn x").count(), 1)`，钉"锚点在整棵视图里恰好一次"。
  6. `lib_delivery_shape_tests.rs` 的 `assert!(uses >= 4)` ⇒ `assert_eq!(uses, 4)`：
     多了=有新的转发点没走可达集，少了=视图或被测试文本垫数，`>=` 两边都放行。
  7. **两条新的"形状对不对"判据**（复审点出的那个新口子该钉的钉）：
     `dispatch_arms_and_volume_arms_agree`（分发臂 ↔ 五册入口的臂表，两个方向 + 五族互不重叠 + 台账 6/6/11/8/4=35，
     分发区今天共 38 个变体 = 35 委托 + Gossip + OpaqueExternal + Unknown 三条原地）；
     `count_guard_views()` 发现式对账（按名字数"有几份守卫视图" ↔ 对账用例里写了几条 case ⇒ 新加一份视图而没登记，
     以前是沉默的）。**用例数 789 ⇒ 790**，两份平台基线同步（`--update` + `--sync-baselines` ⇒ macos 790 / windows 778）。
     非空转证据：把 `FileDone` 从 file 组的分发臂挪到 share 组（**编译合法、运行时静默丢帧**——分册自己那层有 `_ => {}`）
     ⇒ 恰好这一条红、其余 789 条全绿；还原后全绿。
     ⚠️ 这条守卫自己的第一版有两处自造解析缺陷，都被它自己红出来：按字节 `rfind("m @ (")` 切出**半行**
     （8 格缩进被切掉）⇒ 每组第一个变体永远漏数（实测 FileOffer 消失、6 报成 5）；
     而 `find("=> handler(")` 接不住 identity 族那种 `=> {\n handler(…)`。现在按行扫。
- **一条搬家时掉的日志文本空格**：`transport/relay_file.rs` 的 `RelayChunk 转投 tid=…` 少了前导空格。
  行多重集恒等判据看不见这一类（它只比代码行），是逐行读 diff 时抓到的。
- **⑨ 已知边界，写在这里而不是藏在注释里**：202 条用例里有 **6 条**的 `expect_fail_hint` 就是它自己的测试名。
  这类仍然"改坏即红"（`cmd` 只跑那一条测试；编译不过时 cargo 不印测试名 ⇒ 也不会误纳），
  弱的是**归因**（同一条测试里另一处断言坏了它也认）。**本轮不改**：换 6 条 hint 要逐条重写并复跑，
  而 4.33.18 那条已经证明"hint 取源码帧里的标识符"会在 CI 上长出专属红 —— 断言正文不是免费的。

### Notes
- 本地/E2E 层（`--group local`，21 步）在这一批改动之前已在 `1917d0a` 整层绿（结论行 `✓ 21 步 / ✗ 0 / ⏭ 1 跳过`），
  ⚠️ 但那次我把日志文件连同链尾的清理一起删了 ⇒ 现在**只剩结论行这一句可引**，逐步正文无从复核。
  教训记在这里：门禁日志留到提交之后再删，不在同一条链里 `rm`。


## [4.33.19] - 2026-10-07

### Changed
- 自查今天三处搬家时清掉一条**悬空的分节横幅**：`transport.rs` 分发段里那句
  `// ---- 中继文件传输 ----` 原来是贴在 `Message::RelayFileOffer` 那条臂上面的，
  臂体搬进 `transport/handle_share.rs` 之后 `cargo fmt` 把它挤到了上一条臂的右花括号后面
  （`} // ---- 中继文件传输 ----`）—— 它现在指着一条 Gossip 臂，说的是不在这儿的代码。
  同一句话在 `handle_share.rs` 的册头与分发臂上方的注释里都有，所以删掉而不是挪走。
- **这条是本轮机械自查抓到的，不是门禁**：恒等判据只比"行多重集"，比出"拆前拆后各 38 个变体、
  丢 0 / 多 0 / 无重复处理"与 `pub` 项数 `file 65→65`、`ble 8→8`，但注释位置不在它的射程里
  （唯一少掉的那一行就是这条注释）。⇒ 记下来：**搬家后按行多重集对账只能证明代码没丢，
  不能证明注释还指着它讲的那段**。


## [4.33.18] - 2026-10-07

### Changed
- ★ **CI 立刻替我复证了那条新判据的一个副作用，并已修**：推送上去的那 34 条在
  「前端测试 / 清单 / 构建」这一步红 —— `❌ 注入后是红了，但失败输出里**没有**声明的那句判据关键词
  「onFocusChanged」`。根因不是代码：那条 hint 取的是**标识符**，本机 node 会把失败那一行的源码打进输出、
  CI 那台不会 ⇒ 一个"只在本地成立"的 hint 在新判据下变成了 CI 专属红。
  修法两条：① hint 换成断言消息本体「设置窗口要监听重新获得焦点」（任何平台都会打印）；
  ② 红时打印的输出尾部从 600 字符加到 2500 —— 600 不够挑新 hint，这正是我这次定位慢的原因。
  复跑该条：`python3 scripts/verify-guards.py --only "常驻窗口"` ⇒ ✅。
  ⚠️ 同一形状的风险还剩几条（现算 `hint 总数 175 / 纯标识符形状的 40`，其中多数是**测试名**——
  cargo 的 failures 列表一定会打印测试名，那类是稳的；不稳的是只出现在源码帧里的标识符）。
  我不去逐条猜，交给下一次 CI 复证：**这条新判据的价值恰恰在于它会把这类脆弱点在真实环境里照出来。**

### Changed
- ★ **UI 运行时探针现在会核"这台浏览器起得来"，而不是只看文件在不在**（负责人点头"用我自己的浏览器跑"）：
  实测那台 playwright 缓存里的 Chrome for Testing 只剩 `Helpers/Libraries/Resources`，
  `Versions/Current/… Framework` 那个文件不见了 ⇒ dlopen 失败、进程一触即溃，
  而探针只报 `CDP /json/list 里一直没有 page target` —— **一条环境缺陷被伪装成代码回归**，
  在这台机器上连着两轮红在同一个位置，措辞一字不差。
  解析顺序三档：① `GOSSLAN_CHROME`；② 缓存那台（**加一次可加载性核对**）；③ 系统里已装的 Chromium 系
  （Brave / Chrome / Edge / Chromium，macOS + Linux 路径）。**三档全空仍然判红**，不许把没跑写成 PASS。
  走 ③ 时会把"实际用的是谁"打在 `· 来源：…` 那一行 ⇒ 换内核属于**可追溯的降级**，不是静默换尺子。
- 两条实测：真跑 ⇒ `· 来源：系统浏览器回退（playwright 缓存里那台存在但装坏了：框架二进制缺失）` +
  **63/63 条判据绿**；反证（把系统回退名单临时清空的那份副本）⇒ 退 1 报
  「环境：找得到可用浏览器（GOSSLAN_CHROME / playwright 缓存 / 系统 Chromium 系）」这条红。
  环境侧的正解仍是把那台浏览器装回来（`npx playwright install chromium`），那是在他机器上，不在本仓。



### Documentation（认知层第二批：把前两轮拆分的欠账补齐）
- ★ 认知条目 **531 ⇒ 587 条**（新增 56 条：`scripts/e2e/core.mjs` + 19 本轮次分册、`scripts/guard_cases/` 9 本、
  `lib_*_tests.rs` 11 本、`transport/*_tests.rs` 15 本、`CHANGELOG.md` 本体），基线文件 893 ⇒ **948**；
  复跑 `grep -c ': F:' aoci.code.txt` 与 `aoci status`。另把 3 条**正文已失真**的条目改口：
  harness 驱动那条（旗标与分发留在驱动、轮次体搬去 core/rounds）、拆分计划那条（不再是在途地图，
  而是计划+两轮结果+11 行尺子表）、探针那条（"文件在 ≠ 起得来"）。
  `aoci --json check` 现读：`code_missing / code_stale / code_unbaselined` **三格都归 0**，`structure_valid=true`。
- ⚠️ 剩下的 `governance_aligned=false` 不是缺条目，而是** observe 证据要人工复核**（`aoci scope preview` 现读
  「阶段 observed_evidence_review_required｜人工review 155」）。`aoci scope acknowledge` 的字面意思是"记录复核"，
  我跑它就等于替负责人宣称"这些我看过并认可了"——AGENTS 里"人工裁决/审批边界不得忽略"就是这一格 ⇒ **不自行越过**，
  如实报给他：要么他复核一次，要么在带 `aoci_maintain` 的会话里走正式 maintain 通道。
- 机器事实两条（写进记忆，别再当"只有 MCP 能写"）：① **相同文本重传会被拒**（"重复批次: 正式索引零写入"）
  ⇒ 过期条目不能靠原文重绑，必须真的改内容或走 maintain 签发的候选；② 整行有预算上限
  （`entry_field_budget_exceeded` 实测：本条 S 到 435 字节被拒、422 字节通过 ⇒ 按字节而非字符数收）。

### Documentation（认知层第三轮：全局体检剩下的两格）
- 全局漂移分类现读（`aoci --json check` + `aoci --json scope preview`）：`code_missing / code_stale /
  code_unbaselined / orphan` **四格全 0**，条目 587、基线 948 文件、整索引 88,470 tokens（目标档 200,000）。
  本轮补的最后 3 条：`verify-guards.py`（判据化的代价与 5 条不符的现场）、`guard_cases/frontend_ui.py`
  （CI 抓到的那条标识符 hint）、`CHANGELOG.md` 本体（**写清它天然一直过期**：每记一次账就改一次本文件，
  所以它的 code_stale 不算欠账，别为它反复重绑）。
- ⚠️ 剩两格需要他动作，都不在 agent 权限内：
  ① `src/utils/designGuards.test.ts` 的 observe 指纹变了而我今天确实改过它（豁免从一条字面路径改成点名三本），
     但 `observe_change_policy = review_required` ⇒ 要 `aoci scope acknowledge`（字面含义"记录复核"）；
     我不自跑——那等于替他宣称复核过。
  ② `docs/notes/changelog-archive.md` 报 unbaselined（它是超 1 MiB 的免条目对象 ⇒ 属报告噪声）；
     只有他愿意把当前源码整体承认为新起点时才动 `aoci scan --force`，而**那一步会把未复核的 observe 漂移一起洗白**
     （工具自己写的原话就是"防止一键洗白未处理的漂移"）⇒ 我没跑。

## [4.33.17] - 2026-10-07

### Documentation
- ★ **AOCI 认知层按命令行通道补写本轮的 26 条对象行**（他授权"硬走一遍"）：**新增 17 条**
  （`transport/handle_*` 5 册、`network/file/*` 5 册、`network/ble/*` 4 册、`i18n/locales/{dict,zh-cn,en-us}` 3 册）
  **+ 重写 9 条**（`transport.rs` / `network/mod.rs` / `file.rs` / `ble.rs` / `locales.ts` / `file_tests.rs` /
  `check-domain-deps.mjs` / `check-ble-constants.mjs` / `verify-guards.py`）。
  索引条目 514 ⇒ **531**（复跑 `grep -c ': F:' aoci.code.txt`），基线文件 876 ⇒ 893。
  机器事实（修正旧口径"写条目只有 MCP 一条通道"）：`aoci update-entry` 可用，但 `--entry` 必须是
  **完整规范对象行**（含 `basename[TAG]: ` 键前缀；只给 `F:…|R:…` 会被判 `canonical_object_line=false`），
  且要带 `--source-sha256`（自己按文件算）；`--preview` 只给人读文本、**不吐 JSON** ⇒ 批量脚本以裸退码为第一信号
  （我第一版按 JSON 解析，把 26 条成功全报成失败 = 又是"整批读数一模一样 ⇒ 探针读错对象"）。
- ⚠️ **同一台机器上那条 UI 运行时探针的红已定位成环境缺陷**：playwright 缓存里那台 Chrome for Testing 的
  Framework 二进制不见了 ⇒ 浏览器起不来、探针报"CDP 里没有 page target"。换个完好的 Chromium 内核跑同一条探针
  **63/63 全绿**（复跑：`GOSSLAN_CHROME="/Applications/Brave Browser.app/Contents/MacOS/Brave Browser" node scripts/check-ui-runtime.mjs`）
  ⇒ 两条结论：这条红不是本轮回归；界面文案那侧（locales 拆册）在真浏览器里渲染正常。现象与复跑写进
  `docs/large-file-split-plan.md` §6-bis，**没改探针、没放宽判据**（装回浏览器属于他机器上的动作）。
- **AOCI 剩下的欠账不是我本轮造的**（现读 `aoci --json check`）：缺条目 56 条、条目过期 25 条、未入基线 55 个
  —— 绝大多数是 10-06/10-07 那些拆分册（`scripts/e2e/core.mjs`、`e2e/rounds/*`、`guard_cases/*`、`lib_*_tests.rs` 等）
  一直没补认知。`structure_valid=true`、`governance_aligned=false`、门禁各层不读它 ⇒ 补多少条等他拍板（见工单）。


## [4.33.16] - 2026-10-07

### Changed
- ★ **护栏的 `expect_fail_hint` 从"提示"改成"判据"**（`scripts/verify-guards.py`）：现算 **202/202 条 Case 都声明了它**，
  所以它就是契约的一部分。旧写法只在红的时候附一句括号 ⇒ 一句永远匹配不上的 hint 只存在于没人读的 detail 里
  （第 15 趟已经抓到过 local 层一条这样的，那次是手工改措辞）。现在不匹配就 FAIL，并附实际失败输出尾部供挑新 hint。
  判据本身有反证：临时把一条 cargo 用例的 hint 换成 `ZZZ-这条判据措辞不存在` ⇒ 退 1 并原样打印那句关键词找不到；
  删掉注入 ⇒ 回到 ✅。**改坏即 FAIL、恢复即 PASS 两条半边都保留**（先恢复再判 hint，所以红不会把现场留在树里）。
- **整跑 202 条的实测结果：5 条不符合预期，其中 2 条是我今天自己造出来的假绿**（不是过期措辞，是"注入照样绿"）：
  · `ble_reader_loop_refreshes_read_activity`、`peripheral_handshake_failure_clears_the_handshaking_mark`
    这两条读的是 `ble_src_for_guards()` 聚合文本，而**判据自己的字面量也在文本里** ——
    `ble.rs` 的内联测试模块排在视图第一段、函数本体被我搬进最后一段的 `ble/io_loops.rs`，
    于是 `find("async fn ble_reader_loop")` 先撞上测试里那个字符串，取到的"窗口"是 13 行测试文本，
    里面当然有 `mark_conn_seen(` ⇒ 断言恒真。**这条守卫在我拆完 BLE 那一刻起就不守东西了**，
    而快速层/全量层那天都是绿的 —— 只有逐条注入才看得见。
    修法照 `transport/gossip_tests.rs` 的先例：检索式带行首换行（`"\nasync fn x"`；**不加左括号**，
    这两个签名带泛型 `<S: FrameSource + 'static>`，加了就找不到 —— 我第一次就改成了带括号，测试当场 panic
    在 `.expect(...)`，那是响亮的好形状），再加 `body.starts_with("async fn x")` 钉住窗口真的是函数本体。
    复证两条各退 0（`--only "BLE 读循环必须回灌读活性"` / `--only "外设握手失败必须解除"`）。
  · 另 3 条是措辞过期：`有一条群消息补发…` 实际打印的是`有一处…`；`成键` 换成那条测试的名字
    `file_send_progress_counts_wire_not_queue`（红时必打印，而纯编译错不会）；`只有后端说 bluetooth` 换成
    失败用例标题 `连接图标名与文案同源`。三条逐条复跑都回到 ✅。
- ⚠️ 一处**测量工具自己的缺陷**顺手记下：红时打印的输出尾部只有 600 字符，而 cargo 尾部是编译噪声 ⇒
  挑新 hint 时看不到真正的断言消息。这次靠"再单跑一次 + 读测试源码"绕过去了，没改工具（不为本轮范围加东西）。
- §7 那张尺子表加到 **11 行**：新增那一行就是上面的"视图里带内联测试模块 ⇒ find 自匹配"，
  后果标的是**静默失明**而不是响亮报错 —— 这是本轮搬家唯一一次"守卫被搬家弄瞎、而所有门禁都绿"。


## [4.33.15] - 2026-10-07

### Changed
- ★ **`src-tauri/src/network/ble.rs` 按角色切成四册（tier-2 第三轮，也是 2,000–3,000 那一档的最后一本）**：
  2,392 行 ⇒ 主文件 **745 行**（常量 + `start`/`stop` + 链路拆除 + 帧诊断 + 那份 274 行的内联测试模块）+
  `ble/central.rs` 777（扫描→该不该拨→拨号→握手验签→登记链路，含拨号退避）+
  `ble/peripheral.rs` 536（起广播→收 central→首帧判路由→握手→建链与顶替旧链）+
  `ble/io_loops.rs` 275（三级抢占写循环 + 读活性判据，两条角色跑同一对循环）+
  `ble/frame_io.rs` 112（`FrameSink`/`FrameSource` 两个 trait、四个 impl、两个薄封装）。
  边界依据是模块文档自己那句："这一层只补两件 BLE 专属的事：**扫描/连接** 与 **分片收发**"。
  分册不各写 `#[cfg(feature = "bluetooth")]` —— `network/mod.rs:10` 门控整个模块，`include!` 继承它。
- **自证同一套**：`cargo test --features bluetooth --lib` 789 passed / 0 failed；`-- --list` 789 条用例名
  与基线**差集 0 行**；clippy（门禁形态）干净；`verify-guards.py --list` 仍是"202 条锚点各命中一次"；
  `check-ble-constants` 扫描面点名 6 + 目录动态展开 4 = 10 个文件；`check-domain-deps` 在这一刀之后
  仍报 **30 处 / 6 对**（与拆分前同一个数 ⇒ 上一条口径改准是搬家不变的，不是把洞说小）。
- **两处尺子跟着搬家改口（都是响亮的那种，靠现跑抓到）**：
  · `network/mod.rs::ble_src_for_guards()` 立成第三个家 ⇒ 11 处读 `ble.rs` 文本的形状守卫
    （`lib_ble_tests.rs` 9 处 + `ble.rs` 自己测试模块里 2 处 `include_str!("ble.rs")`）改读它；
    `lib_source_view_tests.rs` 的登记对账 4 个用例 ⇒ **5 个**（canary = `ble/central.rs`）。
  · `scripts/check-ble-constants.mjs` 的 `BLE_DOMAIN_FILES` 是**硬编码文件清单**：新册落在
    `network/ble/` 里它不会红、只会永远看不见 ⇒ 判据 B 的扫描面改成"点名 6 + `network/ble/` 目录动态展开"，
    分母由脚本自己打印。反证真跑：往 `ble/central.rs` 塞一行 `const GATT_CHUNK_PROBE: usize = 512;`
    ⇒ `✗ network/ble/central.rs:2 const GATT_CHUNK_PROBE = 512`（退 1），删掉后回到 10 个文件 / 退 0。
- ⚠️ **一条我自己数错的分母，记在这里因为它是流程缺陷而不是笔误**：动手前我把"读 ble.rs 文本的守卫"
  数成 11 处（`include_str!("network/ble.rs")` + `include_str!("ble.rs")` 两种拼法），实际**还有第 12 处**
  写成 `include_str!("../ble.rs")`（在 `network/transport/queue_tests.rs`，跨目录相对拼法）。
  它是靠 `cargo test` 真跑红才现形的（`应为「定义 1 处 + 四个建链点各 1 处」left: 3 right: 5`），
  而红报的形状是"少了建链点"——那会把下一个人推向"补一个并不缺的建链点"。
  复跑口径：`grep -rnoE 'include_str!("[^"]*ble\.rs")' src-tauri/src scripts`（最宽形状，先修尺子再下结论）。
- 契约图 `docs/ARCHITECTURE-MAP.html`：transport 与 files 两个节点的 `paths` 各补一条**目录**认领
  （`network/ble`、`network/file`），并在 notes 写明"逐册名单的唯一来源是 `docs/domains.data.mjs`，
  这张图不抄第二份"。改完按老规矩 `node --check` 那段内嵌 script（1 段，语法通过）。


## [4.33.14] - 2026-10-07

### Changed
- ★ **`src-tauri/src/network/file.rs` 按角色切成五册（tier-2 第二轮）**：2,495 行 ⇒ 主文件 **398 行** +
  `file/send.rs` 689（offer→accept→分片流式→收尾，含 `WireLedger` 记账与 `wait_complete_ack`）+
  `file/receive.rs` 845（offer 决策→`.part` 接收器→逐片解密→校验改名→失败与接管）+
  `file/relay_push.rs` 261（借道中继：`RelayFileOffer`/`RelayChunk`，收件人不是链路对端）+
  `file/group_receive.rs` 142（一个群文件对多成员各建一个接收器）+ `file/share_walk.rs` 224
  （共享目录遍历与落盘命名 —— 纯函数、不碰网络也不碰会话状态）。
  边界依据是文件自己那段模块文档：它开头就写了"发送方 / 接收方 / 中继路径"三条生命周期。
  机制与 transport 那五刀同一条：`include!` 是文本粘贴 ⇒ 同一模块、同一 `use`、同一可见性，
  **测试全名一字不变**。
- **恒等与自证**：`cargo test --features bluetooth --lib` 789 passed / 0 failed；
  `-- --list` 的 789 条用例名与 `test-baseline.macos.txt` **差集 0 行**；clippy（门禁那条形态，不带
  `--all-targets`）`-D warnings` 干净；`verify-guards.py --list` 报"202 条用例的注入锚点都在各自文件里
  恰好命中一次" ⇒ 锚点由 runner 沿 `include!` 树自动解析，指向 `file.rs` 的那批 Case 的 `file=` **一字未改**（对账就是 `--list` 那一句"202 条…恰好命中一次"）；
  `check-domain-map.mjs` 认领数 347 ⇒ 352；`check-doc-citations.mjs` 31 处全落真行。
- **新登记的第五把尺子 + 一处旧尺子改严**：
  · `network/mod.rs::file_src_for_guards()` —— `file` 模块的"生产码全集"视图（`#[cfg(test)]`，与
    transport 那份同形）；形状守卫原先 9 处 `include_str!("network/file.rs")` 全部改成读这个家
    （读单个文件在搬家后只看见主册：搬走会红，但**搬进来的坏形状看不见** = 假绿）。
  · `lib_source_view_tests.rs` 的登记对账守卫从 3 个用例加到 **4 个**（新用例 canary = `file/send.rs`）
    ⇒ 以后少登记一册、或多登记一个不在 `include!` 闭包里的文件都会红，两侧都钉。
  · 同批把 `file_tests.rs` 里那条用 `std::fs::read_to_string("<crate>/src/network/file.rs")` 读源码的
    判据也接进视图 —— 它是本次唯一"跑起来才知道"的红（`✖ 单聊 peer-wide 收尾改名了`），
    红是对的方向：它读的是单个文件。
  · `docs/domains.data.mjs` 的文件传输领域按**目录**认领 `src-tauri/src/network/file`（不是逐册点名），
    并写明两边严格程度的差异按各自失效方式定：视图漏一册是假绿，所以逐册；领域漏一行是无主文件，
    判据 D 会当场红，所以按目录。
- **一条自己踩到的计数错，如实记**：动手前我把"读 `file.rs` 文本的守卫"数成 6 处（只在两个
  `lib_*_tests.rs` 里 grep），实际是 **9 处 / 5 个文件**（还有 `lib_ble_tests` / `lib_compat_gating_tests`
  / `lib_relay_data_tests` 各一处）。按文件去 grep 就是把"检索面"当成了"全集"。
  这次没造成损失是因为批量替换脚本对每个文件写了 `assert count == 预期`，第一个文件就断言失败、
  一个字都没写下去 ⇒ 分母是错的这件事被工具替我说出来了。
- 顺带 4 处指路句改口到真位置：`state.rs`（`file/send.rs::wait_complete_ack`）、
  `commands/files.rs`（`file/send.rs::send_file_from_path_at`）、`file_relay.rs`
  （`file/share_walk.rs::safe_transfer_id`）、`db/messages.rs`（`file/receive.rs::resume_receive`）。

### Changed（同批的第六把尺子：领域依赖方向守门的口径改准，**上限一格没动**）
- ★ `scripts/check-domain-deps.mjs` 里"判据看不见的内联跨域引用"从**按物理文件**判改成**按领域**判。
  起因：`file.rs` 切册后这里 13 对 ⇒ 16 对并报红，而棘轮写明"不许直接调大上限"。
  现量核对（不是解释）：把同一份新脚本在 `567694b`（拆分前）与现树各跑一遍 ⇒
  **两边都是 30 处 / 6 对**；旧口径在两棵树分别是 13 / 16 ⇒ 红来自"`use crate::db;` 留在模块根文件、
  内联的 `crate::db::now_ms` 搬进了分册"这种**归属假象**：include! 让"use 落在哪个物理文件"变成任意事实，
  而 `consumes` 与判据 G 本来就是**领域粒度**的。
- 反证（证明口径没被改成瞎）：往 `files` 域塞一句该域任何文件都不曾 `use` 过的内联
  `crate::notifications::…` ⇒ 6 对变 7 对，且 `GOSSLAN_DOMAIN_INLINE_PAIR_MAX=6 node scripts/check-domain-deps.mjs`
  当场退 1（`✗ 内联跨域引用对数 7 超过棘轮上限 6`）；撤掉注入后回到 30 处 / 6 对。
- ⚠️ 这条口径买不到的那一半写在脚本注释里：同一条**已声明**依赖在另一个物理文件里被内联使用，
  现在不再单独报出来（按领域问就只看领域）。取舍：宁可少报一格，也不拿"文件归属"这种会随搬家漂移的事实当分母。

## [4.33.13] - 2026-10-07

### Changed
- ★ **`src/i18n/locales.ts` 按语言拆册（大文件拆分第二轮，tier-2 里"确有内聚边界"的第一本）**：
  2,010 行 ⇒ 门面 `locales.ts` **16 行** + `locales/zh-cn.ts` 1,024 + `locales/en-us.ts` 1,000 +
  `locales/dict.ts` 7（`MessageDict` 的形状，两册与门面共用，放门面里会形成类型层面的自我 re-export）。
  行数以 `wc -l src/i18n/locales.ts src/i18n/locales/*.ts` 现算为准。
  边界依据是文件自己的第一句：那本字典装的是**两门语言**，各自 903 个键 —— 改中文要在英文的行号上面插话。
- **恒等判据是产物而不是文本**：拆前后各 import 一次 `locales.ts`、把 `{zh, en}` 两个对象
  `JSON.stringify`（键序也进串）后取 sha256 ⇒ 两边都是 **`bf4fee1c6837b3bc`**、键数 903/903。
  三条字典护栏（中英 key 一一对应、值非空、所有 `t("…")` 字面量调用点命中真实 key）读的是**对象**
  而不是文件文本 ⇒ 搬家对它们透明，一字未改；`node --test` 那四本相关测试 148 passed / 0 failed。
- **同批改一处尺子**：`designGuards.test.ts` 里"查看者视角的 `@你` 只许存在于 i18n 名单里"那条，
  豁免从 `rel === "i18n/locales.ts"` 改成**点名三本**（门面 + 中英两册）。没有改成
  `startsWith("i18n/locales")` —— 那样落在该目录下的任何新文件都会**静默豁免**，而这条判据要的
  恰恰是"只有字典本体能带这些字面量"。反证真跑：临时放一本未登记的 `locales/xx-lie.ts`（内含 `@你`）
  ⇒ `✖ 这些生产码把查看者视角的文案写死了：i18n/locales/xx-lie.ts（含 @你）`，删掉后 101 passed。
- **顺手补一条上一轮的漏账**：`check-domain-map.mjs` 判据 D 报 `handle_message` 拆出的五册没登记
  （前五个族提交各少一行 paths），已在 `1736744` 补齐并让该层认领数从 342 变 347。
  另注：4.33.12 那条里写的"主文件 ⇒ 1,391"是搬家脚本在 `cargo fmt` **之前**打印的数，`cargo fmt` 后
  现量 `wc -l src-tauri/src/network/transport.rs` = **1,394** ⇒ 以现算为准（§5.1-bis 已按 1,394 写）。

## [4.33.12] - 2026-10-07
### Changed
- ★ **`handle_message` 第五族：消息与送达回执 4 个变体搬进 `transport/handle_messaging.rs`**（235 行 → 分册 253），
  主文件 1,613 ⇒ 1,391。并一族的理由是这条送达链要一起读才看得见谁点亮谁：落库（ChatMessage）→
  对端确认（Ack 清 outbox）→ 已读（ReadReceipt 只推游标、不改会话摘要）→ 气泡样式（ChatStyle）。
- **五族搬完后 `handle_message` 的实际形状**（现量）：见提交正文与 `docs/large-file-split-plan.md` §5.1 的改口。
  自证同一套：789 passed / 0 failed、用例名差集 0、clippy 与 fmt 干净、202 条锚点各命中一次。

## [4.33.11] - 2026-10-07
### Changed
- ★ **`handle_message` 第四族：身份握手与好友关系 8 个变体搬进 `transport/handle_identity.rs`**（266 行 → 分册 288），
  主文件 1,866 ⇒ 1,613。这一族带着全仓最硬的两条顺序守卫（「身份锚点打标点必须留在 Hello 分支」、
  「打标必须排在 `upsert_peer` 之后 —— 反了不报错，只会静默绑不上」），它们原来与臂体隔着几百行互相看不见。
  两条按函数名开窗的守卫**跟着臂改家**（不跟着改会当场红，不是静默变弱 —— 这条红就是搬家自己报的）。
  ⚠️ 顺手记一处我在这次改里造的自伤：第一版把窗口变量直接遮蔽了全集视图 `tr`，害到同一测试后面
  那条「密钥冲突提示」的断言找不到锚点（`必须还有密钥冲突提示` 红）。正解是给窗口另起名字，
  **全集视图不许被窗口变量顶掉**。

## [4.33.10] - 2026-10-07
### Changed
- ★ **`handle_message` 第三族：群聊与群文件 11 个变体整块搬进 `transport/handle_group.rs`**（167 行 → 分册 185 行），
  主文件 2,029 ⇒ 1,866。并成一族的理由留在分册头部：这 11 个变体共用同一套群前提
  （密钥必须先到、受众按群算的 G-Set 语义、群文件走另一条 outbox），留在原处时这些前提散在 11 个臂中间。
  自证同一套：`cargo test --lib` 789 passed / 0 failed、用例名差集 0 行、clippy 与 fmt 干净、
  `verify-guards --list` 202 条锚点各命中一次。

## [4.33.9] - 2026-10-07
### Changed
- ★ **`handle_message` 拆出两族，主文件 2,743 ⇒ 2,029 行**（第一、二族并一个提交：两批改动都落在同一个函数的同一张 match 上，
  事后拆提交只会让每次的恒等判据更难核）。12 个变体的处理体整块搬进两个新分册：
  `transport/handle_file.rs`（1:1 文件收发 6 臂 / 500 行）与 `transport/handle_share.rs`（共享目录 + 单跳中继 6 臂 / 239 行）。
  **臂体逐字未改**：分册里那层 match 与原函数同一层（臂仍 8 格缩进）⇒ 不需要重排；分发臂用
  `m @ (Message::FileOffer { .. } | …) => handle_file_messages(state, peer_id, m).await`，
  `{ .. }` 不绑字段 ⇒ 绑定全部留在分册里那条同形状的模式上。
- 为什么这一刀算机械搬家而不是改控制流：AST 现量过 **match 之前没有裸 `let` 绑定、分支之间不共享局部量**，
  且 `match msg` 是函数最后一条语句 ⇒ 臂里的 `return` 从 helper 返回与原来从 `handle_message` 返回**等价**
  （若 match 之后还有代码，这个等价就不成立 —— 那是这条搬家能做的判据，写在分册头部）。
- **搬家自己报出的一处真耦合**：一条按函数名开窗的形状守卫
  （`duplicate_file_done_still_answers_with_an_ack`）跟着臂搬家后窗口里没有了那段文本 ⇒ 它当场红，
  已把开窗锚点改指 `handle_file_messages` 并同步那段"为什么是源码守卫"的说明。
  ★ 而护栏用例那 11 条注入锚点**一条都不用改**：`verify-guards.py` 自己顺着 `include!` 树解析锚点所在文件
  （`--list` 现算 202 条各恰好命中一次）⇒ 按文件路径硬改 `file=` 反而是错的。
- 恒等与自证（两族一起）：`cargo test --lib` 789 passed / 0 failed、用例名与 `test-baseline.macos.txt` **差集 0 行**、
  clippy `-D warnings` 干净、`cargo fmt --check` 干净、`verify-guards --list` 202 条锚点全命中。

## [4.33.8] - 2026-10-07
### Fixed
- ★ **数据面中继授权闸从"抄三遍"收成"一个家"，同时把替那个缺陷形状把关的判据改严**：
  `decide_relay_from_peer(` 原来在三个转发点各写一遍（定向借道 / `OpaqueExternal` 转投 / `RelayChunk` 转投），
  三段的策略判定、db 锁取好友、节流日志形状完全相同，只差"为什么被拒"那句文案。
  现在唯一的家是 `transport.rs::relay_denied(state, peer_id, why)`，三处各自 `if relay_denied(...) { return; }`。
  **行为一字未变**（同一张真值表、同一个节流键 `relay_deny`、同一句日志文本），789 条用例全过、用例名与基线差集 0 行。
- 判据 `relay_data_plane_respects_policy` 原来钉的是「`decide_relay_from_peer(` 出现 ≥3 次」⇒
  它要求的正是"重复三遍"这个形状：谁收成 helper 让三处共调，计数掉到 1、判据当场红，而代码其实变好了；
  最顺手的消红动作是再抄第四遍。**现在钉形状不钉次数**：策略判定恰好 1 处（在家里）+ `fn relay_denied(` 恰好 1 处 +
  `if relay_denied(` 恰好 3 处。两条反证都真跑过（不是设计意图）：
  摘掉 `RelayChunk` 那一处消费者 ⇒ `FAILED … 都要走 relay_denied，实际 2 处`；
  在 helper 旁再抄一处判定 ⇒ `FAILED … 必须只有 relay_denied 一个家，实际 2 处`。两次都当场还原。
- 顺手止住一处热路径小浪费：被拒日志的文案以前在**每一跳**都无条件 `format!`（中继在文件分片的热路径上），
  现在 `why` 是 `impl FnOnce() -> String`，只在真被拒且节流放行时才求值。

- ★ **CHANGELOG 分卷（方案 A，他 2026-10-07 点头后执行）**：台账主文件 **13,592 ⇒ 720 行**，
  4.31.41 及更早的 **303 个版本小节 / 12,888 行**整段搬到 `docs/notes/changelog-archive.md`。
  切点取「保留未发布小节 + 4.32 及以后」，依据是现算的分布：最近 10 节只占全文 6%，未发布小节 242 行 ⇒
  **天天要读的那一块本来就只 700 行，剩下 94% 是冷的历史**（复跑 `grep -c '^## \[' CHANGELOG.md` ⇒ 9、
  `grep -c '^## \[' docs/notes/changelog-archive.md` ⇒ 303，两份之和 312 = 分卷前那份的小节数）。
- **只搬不改，恒等判据是字节级**：把两段新增说明文字摘掉后，主文件 + 归档重拼回原文，**SHA 相同**
  （`570410fb47cd…`）。分卷脚本里六道断言（小节数守恒、锚点仍在主文件且唯一、归档不含锚点、
  归档首节确实是 4.31.41、主文件仍是新在前降序、重拼逐字节相同）**任何一条不过就不写盘**。
- ★ **这一刀没有改任何一把尺子，三条理由都是当场读过的**（与前面几刀"判据必须同批改口"形成对照 —— 区别不在大小，
  而在**判据读的是不是这份文件的哪一段**）：
  ① 结构判据与发版脚本只认**行首**的未发布小节锚点 ⇒ 锚点留在主文件且仍唯一；
  ② 条目归属判据只读**当前这一版**那一小节 ⇒ 后半段住哪儿与它无关，跑出来仍然「归属可证」
  （`npm run version:changelog` 当场退 0）；
  ③ 引用与数字两把文档量具用**显式清单**（台账本就不在其中），而命令名探针的跳过规则整段放过
  `docs/notes/` 与 `CHANGELOG.md` ⇒ **归档落 `docs/notes/` 是刻意的**：这样不需要为它新开一条豁免，
  也不会让 12,888 行历史里的旧命令名被当成"文档引用了不存在的脚本"。
- **非空转复证**：那条专门往台账注入、证明结构判据会红的护栏用例（`CHANGELOG 结构（[Unreleased] 锚点缺失/顺序错乱必须报出）`）
  分卷后仍报「✅ 改坏即 FAIL、恢复即 PASS」⇒ 锚点没被搬丢；`check-doc-numbers` / `check-doc-citations` /
  `check-invariant-hooks` / `check-scripts-parse` 全 0。
- 归档顶部写死两条规矩：**不追加、不改写**。新的更新说明一律写主文件的未发布小节；历史小节里那些
  "当时的行号 / 当时的条数"是证据，不是待更新的字段 —— 改了就把记录换成了传闻。
- 零应用码改动（一份台账 + 一份新归档 + 一处文档改口）⇒ 不提版本。


- ★ **验真时发现并修掉一条永远匹配不上的护栏 hint**（顺带把这类洞的量法记下来）：
  `guard_cases/toolchain.py` 那条「门禁 local 层必须逐条点名 harness 的正向注入轮」写的是 `expect_fail_hint="local 层"`，
  而这条用例跑的 `node scripts/check-doc-numbers.mjs` **从不打印这个短语**（它打印的是
  「门禁层点名了 harness 里不存在的注入 …」和「harness 有正向轮次 --fault=kill-mid，但 verify.mjs 的门禁层没有它」——
  那句 `local 层` 只存在于该文件第 299 行的**注释**里）。⇒ 改成判据真会打印的那句，跑完 detail 从
  「失败输出里没看到 …」变成「✅ 改坏即 FAIL、恢复即 PASS」；再反向证一次匹配机制本身不是空转：
  故意把 hint 换成一个不存在的短语 ⇒ 提醒立刻出现（退码仍 0）。
  ⚠️ 由此量到的**系统性形状**：202 条用例全带 `expect_fail_hint`，而 runner 对"匹配不上"的处理是
  **写进 detail 后照样判绿**（`verify-guards.py:270`）⇒ hint 是 attribution 提示、不是守卫。
  静态代理只能筛出 8 条"整个仓里没有任何代码会打印这个短语"的（这条 `local 层` 反而不在其中 ——
  别的脚本会打印它，**但这条用例跑的那条命令不会**），真实数量必须按 `case.cmd` 分组跑一遍才知道
  ⇒ 已开一条工单（要不要把 runner 改成「声明了 hint 却没匹配上就 FAIL」= 一次 202 条整跑，约 45–55 分钟）。
- 纯文档一记（无码改动 ⇒ 不提版本）：拆分计划补 **§2-bis「其余五个文件由什么构成」** —— 把"具体分析它们在干嘛"这半条目标
  从一句话描述升级为**现算的内部构成表**：护栏 runner 202 条 Case 的七个域分布（39/32/30/29/27/26/19）、
  `transport/*_tests.rs` **96 个**测试的分布、`lib_*_tests.rs` **75 个**、harness 19 册 **175 条 `check("`**
  （+ 驱动里默认轮那 18 条 ⇒ 每轮真数 = 18 + 本册数，与判据 C 是两条独立量法、互点对上了）、
  台账 **312 个版本小节**（平均一节 43 行）。表里每条都挂了复跑命令，并逐条在本机原样跑过（BSD grep 也吃那个形状）。
  ★ 数出第一版时我自己的尺子是坏的：`re.findall` 忘了开 `re.M` ⇒ `^    Case\(` 对七册全数 0，
  而 202 是我知道的非零值 —— "已知非零的量具读出 0"就当场否掉了它，补 `re.M` 后又漏数 `#[tokio::test]`（`listen_tests` 4 个被数成 0）。
  两个错都修在落笔之前，表里现在是修完的读数。
- ★ 拆分管线段的验证口径补一条**会咬人的运维警示**（落在 `docs/large-file-split-plan.md` harness 那一节）：
  **锁屏期间不许跑 `selfproof:sync`**。实测本轮整层跑的每一次报红，红名聚起来只有一种（「报告带两张全屏帧」⇒ 环境不是搬家），
  但反证档的红**条数**会被这一格顶上去 —— 而 `scripts/fixtures/selfproof-baseline.json` 存的正是每档红条数，
  这时候 `--sync` 等于把 +1 悄悄写成契约。正确顺序：解锁 → 单档复跑对数 → 需要时才 sync；跑不动就记「未跑」，不许记绿（总指令§十）。
- 写这条警示时**被自家判据 A 当场逮到一次**：我在文档里写「`verify:e2e` 前 14 步」，判据把任何「N 步」都当门禁步数声明，
  报「手写 14，现算只有 快速 17 / 全量 23」⇒ 本地层的步数不许写成「N 步」（换量词，或引脚本自己打印的那行）。
  这条形状约束已进长期记忆，判据本身一字未改 —— 它这次的红是**对的**。
- 纯文档一记（无码改动 ⇒ 不提版本）：把三处**会指挥下一步动作**的旧指路句按搬家后的真位置改口 ——
  ① `docs/acceptance/stability-smoke-matrix.md` 顶部那句"判据 C 从 `e2e-multi-instance.mjs` 现算每轮条数"
  改成两路归堆（驱动的块 + `rounds/*.mjs` 按册里 `export const MODE`），并写清"同一 MODE 两处家 = 红"；
  ② 同一份矩阵里点名的 `openWhileOff` / `openWhenOn` / `probeControl` 三条判据，落点从主文件改到
  `scripts/e2e/rounds/lanoff.mjs`（现读确认这三条只住在那儿了）；
  ③ 拆分计划 §4「已完成」补第 5 条（harness 那一刀的形状、三条必须同批改的判据、惰性 import 那个洞），
  标题计数改口为五个文件。
  ★ 有一条**没改**并说清为什么：矩阵 §213 那句"`e2e-multi-instance.mjs` 起跑前 `renameSync(i.db, …)`"仍然成立 ——
  库备份/还原那段确实还留在驱动里（现读 693 / 757 行），把它跟着别的指针一起改掉就是制造假修正。
  复跑：`node scripts/check-doc-numbers.mjs`（20 轮数字仍逐轮一致）/ `check-doc-citations` 0 / `check-invariant-hooks` 30 条全绑定。
- ★ **harness 第八刀第三段：剩下 9 族 13 个块（2,108 行）也搬完了 —— 至此 `scripts/e2e-multi-instance.mjs` 4,845 ⇒ 驱动 910**，
  `scripts/e2e/core.mjs` 679，`scripts/e2e/rounds/` **19 册 / 3,664 行**（最大 `task.mjs` 535）。驱动里顶格轮次块现为 **0**
  （复跑 `grep -cE '^if \([A-Z_]+\) \{$' scripts/e2e-multi-instance.mjs`），只剩 26 条分发行 + 默认轮 + 报告。
  ⇒ **本轮目标的"人写的代码文件都进 3,000 行"这条已经达成**：§1 那条现算命令现在只印 4 行，全都不是代码
  （`CHANGELOG.md` 台账 + `Cargo.lock` / `package-lock.json` 两份生成物 + `.aoci/baseline.json` 工具基线）。
- ★ **抓到一类"只有文件存在性能看见"的洞**，而且当场就抓到：块体里有惰性 `await import("./ax-tree.mjs")`
  （群聊轮读 AX 树那两处）。**静态 import 表看不见它**（它不是 import 语句）、**ESM 链接测试也看不见它**
  （19 册逐个 `await import()` 全过 —— 那一行还没被执行）⇒ 唯一报红的是 `check-scripts-parse.mjs`
  那条「被引用但文件不存在」。修法一行：按新目录重定基成 `../../ax-tree.mjs`。
  ⇒ 记下这条区别：**"能不能解析"与"路径落不落得到真文件"是两把判据，后者不能被前者替代**；本轮没为此新加工具（已有的那把买到了）。
- **生成器自己的两个缺陷都被当场抓住，各记一条形状**：
  ① 取"声明自己的注释"不能用 `getFullStart()` 的前导 trivia —— 同批被搬走的轮次块也算"上一条语句"，
     于是**群聊那一族的预置横幅落进了 `task.mjs` 的私有段**（同一段话在两个分册各出现一次、还挂错对象）。
     改成只认**紧邻上一行**的连续注释后解决；
  ② 修完 ① 之后那段横幅**仍然**在 task.mjs 里 —— 因为它是**驱动里本来就挂错对象的注释**（内容说的是 `if (GROUP)` 那一段，
     却坐在任务 id 声明上面；`if (GROUP) {` 自己头上是空的）。已把它归位到 `group.mjs` 的 preset 上。
     这条不是搬家用具的错，是搬家把一处旧缺陷照出来给你看了 —— 系统性扫法：逐册列"注释里提到的 `--round=` / `--fault=` 名字"与本族对照
     （复跑：把 `docs/large-file-split-plan.md` §1 那段 python 换成按 `export const MODE` 分组即可），其余命中都是**正当交叉引用**
     （如续发轮里那句"`--round=groupcrash` 判的是崩溃恢复，不是新建群能不能送达"）。
- **恒等与行为**：判据 C 现算的 20 个轮次标签**在 13 个块搬完 + 注释补完之后仍与搬前一字不差**；真跑这 9 轮
  `group` 36 / `task` 49 / `posttext` 39 / `dmreaction` 24 / `gossip3` 25 / `gossip-late` 23 / `lanoff` 26 / `groupcrash` 26 / `gfile` 28，
  **每轮恰好 1 条红且 9 条都是同一格**「报告带两张全屏帧」（会话锁定 ⇒ 环境判不了，**没放宽截图判据**），条数与现算逐轮吻合。
  契约图那格依赖的现读命令也当场复跑过：`grep -oE 'ROUND === "[a-z0-9-]+"' scripts/e2e-multi-instance.mjs | sort -u` ⇒ 仍 19 个轮次名
  （分发留在驱动 ⇒ 判据 D 与这条命令一字未改）。
- 一次性工具的一条自我伤害记下来（不留红、但值得记形状）：argv 里带 `=族名=阶段名` 覆盖写法时被我的过滤器当成"不是族名"，
  于是**只搬走了 6 个单块族**；而更早一次失败（`dmreact` 的 `ROUND` 没家）在被拒写盘之前已经把 group/task/posttext 三册落过一次盘 ⇒
  **驱动最后写盘**这个顺序保住了不一致不会发生，但孤儿分册确实存在过、也确实清掉了。⇒ 生成器该"全部成功才落盘"，
  而"引用没家就拒绝写盘"那道断言第一次发挥了作用（它抓到的是真洞：漏搬内置件与 `ROUND`）。
- 零应用码改动 ⇒ 不提版本。护栏/文档侧欠账只剩两件：新落的 19 册 + core/驱动的 **AOCI 认知维护**（要一个 aoci MCP 工具在册的会话），
  以及 `CHANGELOG.md` 那一本的 A/B 决定（等你拍板）。
- ★ **harness 第八刀第二段（下）：10 个注入族 13 个块整块搬进 `scripts/e2e/rounds/`** —— 主文件 4,209 ⇒ **3,146**，
  10 册共 1,266 行（最大 `sendkill.mjs` 153），core 679。搬走 1,038 行；驱动里还剩 **13 块 / 2,108 行**（群聊与任务那 9 族）
  ⇒ **再搬任意一族就进 3,000 阈值**。
- **这一刀一行都没重排，也没改成入参**：块体本来就是 2 格缩进（顶层 `if` 之内）⇒ 去掉旗标行与它顶格的 `}` 之后**正好是函数体缩进**，
  逐字即所得；而**旗标与分发留在驱动**（`if (POISON) await poison.preset();`）⇒ 判据 D 与契约图那条 `ROUND === "…"` 现读命令的输入没搬家，**一字未改**。
  这条切法推翻了我自己 10-06 写进拆分计划的那句结论（「块读的就是那 29 个顶部旗标 ⇒ 搬出去必须改成显式入参，那是重写而不是搬家」）：
  按族取 AST 引用集合后，**块体里没有一处读旗标**，旗标只出现在 `if (FLAG) {` 那一行。文档里那两处（连同「`peerTo` 被十族都写」→ 实为**十族都读**）已按读数改口。
- **搬家前先证的两条前提**（决定"盲加前缀会不会静默改语义"）：7 个跨块可变名在全文件除顶层那条 `let` 外**零绑定**（无解构 / 无形参 / 无内层 `let`）、
  **0 处写成对象简写属性**；再加一条 AST 硬断言：**每册正文引用的名字必须有归属**（自带 import / 族私有 / 全局），没家就拒绝写盘 ——
  这条当场抓到过一次真洞：第一版生成器没搬 `fs` / `path` / `createHash` 这些**内置件 import**，`node --check` 全过而跑起来才 ReferenceError。
- **尺子跟着搬，并且当场证它不是空转**（判据 C 四条 lie，一条比一条狠）：
  ① 把一册的 MODE 改成未登记名 ⇒ 红（沿用原有那条 unknown 口径）；② **把整册挪走** ⇒ 红「MODE_LABEL 登记了 POISON，但驱动与 rounds/ 里都没有它的家」
  —— 这条是新加的，挡的正是"删分册 ⇒ 该轮断言数静默变 0 而文档跟着改 0 就绿"；③ 同一 MODE 出现两份家 ⇒ 红；
  ④ 摘掉册里一条 `check("` ⇒ 文档那句「脏前缀轮 22 条断言」与现算 21 对不上 ⇒ 红。四条恢复后全部退 0。
  恒等判据：判据 C 现算的 **20 个轮次标签在搬前搬后一字不差**（`node scripts/check-doc-numbers.mjs | grep 现算 E2E 断言数` 与搬前那份 diff 为空）。
- **另外两处"检索面必须跟着搬家"的守卫，都是实测必需而不是预防**：
  · `check-invariant-hooks` 的 `e2e:` 片段原来只读驱动 ⇒ 加宽到驱动 + `e2e/core.mjs` + `e2e/rounds/*.mjs`（12 份）。
    反证做过：把检索面收回只读驱动 ⇒ `INV-P06` 的 `e2e:故障注入判据③` / `…⑧` 两条当场报红（退 1）——
    因为它们搬完后只存在于 `rounds/kill.mjs` / `rounds/rot.mjs`。**不加这条的话表现是"这两条不变量没有钩子"**，会指挥下一个人去补一条本来就活着的钩子。
  · `check-scripts-parse` 的 import 闭包原来只跟 `./x.mjs`，不跟 `../x.mjs` ⇒ 分册引用上一级那条边一直是漏的；
    放松后**当场多查一个文件**（41 ⇒ 52，新增的是 `docs/domains.data.mjs`，此前从未被查过）。
- **行为复证（真跑搬过的族，屏幕锁定态）**：`--fault=kill-mid` 25 条 / `--fault=recv-dir-rotted` 24 条 / `--fault=multi-file` 24 条，
  **每轮恰好 1 条红，且都是同一格**「报告带两张全屏帧」（loginwindow 盖屏 ⇒ 环境判不了；**没放宽截图判据**），条数与判据 C 现算逐轮一致。
  便宜量具：`verify-guards.py --list` 退 0、`check-domain-map` / `check-domain-deps` / `check-test-manifest --only rust` / `check-doc-numbers` / `check-doc-citations` 全 0。
- ★ 顺手记一条**量具坏了的形状**（已写进拆分计划正文）：那行数块的 python 只匹配 `^if \([A-Z_]+`，
  分发行出现后它把 13 条 `if (X) await …` 也当块开头，而分发行没有顶格 `}` ⇒ 一路吞到下一个真闭合，
  实测读出「26 块 / 3,497 行」这种**比全文还大**的废数。形状规则收成 `^if \([A-Z_]+\) \{$` 之后回到 13 块 / 2,108 行。
- 零应用码改动 ⇒ 不提版本。下一段：群聊与任务那 9 族（13 块 / 2,108 行）同法搬走，主文件落到 ~1,050。

- ★ **harness 第八刀第二段（上）：先把「谁都能碰的那份共享量」收成一个家** —— 主文件 4,321 ⇒ 4,209，`scripts/e2e/core.mjs` 602 ⇒ 676。
  搬去 core 的是 12 条 argv/env 派生声明（`FAULT` / `ROUND` / `LIE` / `LIE_SHA` / `GROUP_ID` / `GROUP_NAME` / `GROUP_KEY_B64` 连它的填充循环 /
  `GROUP_KEY_STR` / `SIZE_ARG` / `FILE_MB` / `FILE_BYTES`）；7 个「写在驱动、读在轮次块」的可变量（`idA` `idB` `msgId` `peerTo` `xferId` `srcFile` `srcSha`）
  并进已有的状态对象 `S`。**26 个轮次块一个都没动** —— 这一段是下一步的前提：轮次块要搬出去，就得先拿到同一份共享量。
- **为什么前一段说「43 个模块级标识符」不是拦路石，而共享量只有 7 个**：用 `node_modules` 里的 TypeScript 解析器现数
  （一次性脚本在 /tmp、不入库，所以这两个数是**本轮读到的、不是读者能复跑的**；能复跑的是下面那两条恒等判据）⇒
  **89 个名字只在某个块里出现**（各轮的旗标、`*_LIE`、字节数、`xferIdN` 这类族内 id —— 它们跟着块走，不需要任何家），
  **只有 9 个跨越块或跨块/驱动**（7 个可变量 + `FILE_BYTES` + `GROUP_KEY_B64`）。上一段那句「43 个标识符 ⇒ 拆不得」是把"块引用过模块级名字"
  误读成"必须共享"，实际绝大多数是**一族私有的**。
- **盲加 `S.` 前缀会不会静默改语义 —— 先证两件事再动手**：这 7 个名字在块内**没有任何局部绑定**（无解构、无形参、无内层 `let`，AST 全文件扫：
  除顶层那一条声明外零绑定），也**没有一处被写成对象简写属性**（`{ idA }` 这种加前缀会变成语法错或改语义的形状，实测 0 处）
  —— 这两条同样是 AST 现数、随本次一次性脚本作废，但它们正是「234 处插入」的前提，所以写在这；后果由下面两把判据兜住 ⇒ 234 处插入
  （`idA`97 / `idB`75 / `peerTo`23 / `xferId`17 / `msgId`16 / `srcFile`3 / `srcSha`3）全部按 AST 位置做，不靠正则。
- **恒等与行为两把都交了**：判据 C 现算的 20 轮断言数**逐轮与搬前一字不差**（`node scripts/check-doc-numbers.mjs | grep 现算 E2E 断言数` 前后 diff 空）；
  真跑默认轮 18 条 = 1 红（锁屏那一格，与拆前同形）；再跑 `--fault=poison-part-lie` 得 **22 条 3 红**，其中 **2 条是设计要红的脏前缀判据**
  （『坏内容不许冒充成功』『两侧都不许 done』）—— 这两条亮着就是 `LIE`/`LIE_SHA` 从 core 读到的活证据，第 3 条才是锁屏。
  便宜量具：`check-scripts-parse` 0（41 个脚本）、`check-invariant-hooks` 0（30 条钩子全绑定）、`check-doc-citations` 0（34 处 file:line）、
  `verify-guards.py --list` 0。
- **顺手拔掉一处会说谎的指路命令**：`docs/stability-roadmap.md` 那条「`grep -n GOSSLAN_AUTOSTART scripts/e2e-multi-instance.mjs` ⇒ 第 554 行」
  在上一段搬家后就已经空了（那份 env 现在由 `scripts/e2e/core.mjs` 的 `launch()` 塞）—— 它是写给读者的复跑指令，不是历史读数 ⇒ 改成指向 core 并去掉硬行号；
  同一条目里 `lib.rs:335/342` 那两处**留着不动**（那是 09-27 那次现场的记录，且本仓已把 `forced || enabled` 改掉，改了反而把历史覆盖掉）。
- 零应用码改动 ⇒ 不提版本。下一段是真搬家：10 个文件注入族（POISON/RESUME/KILL/FREEZE/SENDKILL/STALL/DISK/ROT/SHRINK/MULTI，共 1,088 行）
  整块搬进 `scripts/e2e/rounds/*.mjs` —— 块体缩进本来就是 2 格，正好是函数体的缩进 ⇒ **去掉 `if (FLAG) {` 与它顶格的 `}` 之后逐字即所得，一行都不必重排**；
  旗标与分发留在驱动（判据 D 与契约图那条 `ROUND === "…"` 的现读命令因此一字不用改），判据 C 改成按分册里一行 `export const MODE` 归堆。

- ★ **harness 第八刀第一段：`scripts/e2e-multi-instance.mjs` 4,845 ⇒ 主文件 4,321 + 新家 `scripts/e2e/core.mjs` 602 行**（55 个导出：常量、共享量、可复用的工具函数）。搬的段全是模块级声明（本来就在第 0 列）⇒ 唯一的文本改动是给声明行加 export 前缀；另有两处是**必须**改的：① `ROOT` 由本文件位置推导，搬进子目录要退两层（不改就静默指到 `scripts/`）；② 三个会被赋值的标量（`curStep` / `curStepIdx` / `stashSeq`）改由状态对象 `S` 承载 —— 6 个读写点，逐处数过。
- ★ 这一段把「JS 里为什么不像 Rust 那样能纯搬」量成三条硬约束（每条都是本轮真跑出来的，已写进 core 册头）：
  ① **ESM 不给 import 绑定赋值** —— 第一版搬完真跑就报 Assignment to constant variable；
  ② **搬进 core 的函数不许回头引用留在主文件的声明** —— 链接期不报，`writeReport` 引用了驱动侧 8 个标量，
     跑起来才炸 ⇒ 生成器现在把这条做成**写盘前**的断言（B 类洞会点名到行）；
  ③ **位置相关量必须单独核** —— `import.meta.dirname` 派生的 `ROOT` 换目录就换含义（实测 pre-flight 以「没有二进制」拒跑、退 2）。
- **复证不靠读代码**：`node --check` 两份都过；core 单独 import 无顶层副作用（`test-results` 目录数前后都 299、  无残留进程）；真跑三轮 —— `--round=dmreaction` 24 条、`task` 49 条、`group` 36 条，  **每轮恰好 1 条红，且是同一格**「报告带两张全屏帧」（屏幕锁定 ⇒ 只抓到 1 张），与拆前的读数同形；  判据 C 现算的 20 轮断言数**逐轮与拆前一致**；`verify-guards.py --list` 0（202 条锚点各恰好命中一次）、  `check-scripts-parse` 0（41 个脚本，新 core 已被 import 闭包吃进去）、`check-domain-map` 0、  `check-doc-numbers` 0；快速层 16 绿 + 第 14 步 Change Budget（既有红，非本次）。顺带量到：**每轮 13–39 秒**。
- 零应用码改动（都在 `scripts/` 与 `docs/`）⇒ 不提版本。下一段是把 26 个 `if (FLAG) {…}`（3,157 行）按族拆进 `scripts/e2e/rounds/`，同批改判据 C/D 的读法 —— 那一段做完主文件才落到 ~1,200 行。

- 纯文档一记（无码改动 ⇒ 不提版本）：拆分计划补 **§5「更优雅的写法实测到哪一步」**，把**函数级**这一半从印象变成现算 —— `handle_message` 函数体 **1562 行 / 38 个顶层分支**，`match` 之前的 `let`（`allowed` / `cfg` / `dbc`）全在一条**早退分支内部**、分支体里的同名 `let` 各分支自己有（现算 7 处）⇒ **「一个消息族一个 handler」在这里是机械活不是重写**（新函数只要 `(&AppState, &str peer_id)` + 自己的解构字段）。但这一刀**不让任何文件变小**，改的是控制流 ⇒ 按既有口径仍属「真重构」，且 2026-09-25 复审已把这一处判为「短期只降稳定」而暂缓 ⇒ **等他点头再动**，动就一族一提交、每步跑同一套恒等判据。文档里那条复跑命令在仓库根原样跑过，读数 `span 1562 arms 38` 与正文一致。
- ★ 顺手抓到一处**真缺陷**（不是风格问题）：中继授权闸在生产码里有 **3 个家**（`transport.rs:1003`、`transport.rs:1278`、`transport/relay_file.rs:459`），而判据 `relay_data_plane_respects_policy` 钉的是 `wired >= 3` ⇒ **这条判据现在要求的就是「重复三遍」这个形状**：谁把闸收成一处 helper 让三个消费者共调，`wired` 掉到 1、判据当场红，而代码其实变好了。文档里写清了唯一正确的改法（两件一起做）：抽一个家 + **同批把判据改严而不是改松**（「策略调用恰好 1 处 + helper 调用点恰好 3 处」，配两条反证：摘掉一个消费者必红、把闸内联回去必红）。不同批改判据的话，最顺手的消红动作是再复制一份第四遍 —— 那才是把缺陷真钉死。
- 证据归属：全量层（23 步）在 tip `8b77d33` 上整跑过一遍 —— 21 绿 / 2 红 / 1 跳过，两条红仍是同一根因（`Change Budget` 窗口里那三条 transport 补丁形状提交，以及被它顶住的「force push 后 before 不可达不得判空转」那一条护栏用例的「恢复后即 PASS」半边），与上一版全量层（`fe35db0`）比**差集为 0**（没有新增红）；`移动端编译门禁（Android）` 那一步本层跳过、按既有口径记「未跑」不记 ✅。另把契约图那两格也现算核了一遍：`字面量事件名` 27 与 `arch-map-stats.mjs` 读数一致、`护栏非空转用例` 202 由判据 E 对账 —— 这轮搬家**没有让图上任何一格悄悄变数**。
- ★ **护栏 runner 第七刀：`scripts/verify-guards.py` 4,176 ⇒ runner 385 行 + `scripts/guard_cases/` 7 个域分册
  （最大 771 行）** —— 202 条 `Case` 按**锚定的被守物**分册，runner 只剩注入/还原/报告那一套。
  这一刀与前面几刀不同类：它**必须同批改判据本身**（判据 E 的计数范围跟着搬家），所以两件事各补了非空转对照。
- **恒等判据换了形状，但仍是硬的那种**：`python3 scripts/verify-guards.py --list` 拆前拆后都是 624 行、
  **排序后逐字节差集 0 行**（202 条用例名与说明一字未变）；运行次序按 `MODULES` 分段变了 ——
  只影响 `[n/m]` 进度号，而**没有判据读那个序号**（现算：代码里没有 `CASES[` / `CASES.index`；
  文档里 `[197/197]` 那种写的是**总数**、与次序无关）。
  ⚠️ 搬完第一跑就抓到**我自己造的新噪声**：册头里写了一句带反斜杠括号的正则，Python 报 7 条
  `SyntaxWarning: invalid escape sequence` ⇒ 改成中文原样描述后 stderr 0 行。
- ★ **三处判据跟着搬家改口径，每一处都当场证明没空转**：
  ① `check-doc-numbers.mjs` 判据 E 现在 glob `scripts/guard_cases/*.py` 数 `^    Case\(`（仍故意「数到 0 就 throw」）
     —— 摘掉一条 `Case(` ⇒ 它报「现算 201 与图上那格 202 对不上」；还原 ⇒ 退 0；
  ② `guard_cases/__init__.py` 加了一条**起跑前就炸**的对账：目录里的分册集合 ≠ `MODULES` 名单 ⇒ `AssertionError`
     点名差集。这条守的是 E **买不到的那一半**：E 按目录 glob 数，「漏点名一册」在它眼里条数不变（＝假绿），
     只有装配表自己知道自己少装了谁。实测：临时丢进一册 `zzz_probe.py` ⇒ `--list` 退 1 并打印差集，删掉即复绿；
  ③ `check-scripts-parse.mjs` 的 import 闭包原来只走 `.mjs`（`node --check` 不解析 import ⇒ 共用件坏了没人知道），
     现在补上 Python 的 `from <pkg> import` ⇒ 分册里的语法错落在秒级层被抓。实测：`= [` 改成 `= [)` ⇒ 该步报红，还原即绿。
- **路由规则踩过一次 substring 假命中，已改**：第一版按裸 substring 判域，于是 `TodoCardBubble.vue`（含「ble」）
  被分进蓝牙册、`FriendProfile.vue`（含「file」）被分进文件册 —— 各 2 条。改成按**路径段/文件名主干**匹配后归位；
  这条教训写进了每一册的册头，因为下一个加用例的人会同样手滑。
- **`ROOT` 的深度换了，配一条当场断言**：分册在 `scripts/guard_cases/` 下 ⇒ `ROOT` 必须是 `parents[2]`；
  数错时 `src-tauri/Cargo.toml` 就不在 ⇒ `base.py` 直接抛，而不是让 202 条用例去找不着的文件。
  同批让 `verify-guards.py` 自己把脚本目录插进 `sys.path`：`docs/final-architecture-review.md` 里有一条用
  `spec_from_file_location` 数 `CASES` 的复跑命令，那种加载方式下 `from guard_cases import` 会
  `ModuleNotFoundError`（**改之前实测整条命令已作废**）⇒ 指路句必须在读者站的位置原样跑得通，不能只写「应该能跑」。
  ⇒ 修的是**模块自己**（不把 `sys.path` 的責任推给每个读者），文档那条命令一字未改、现在原样返回 202。
- 零应用码改动（都在 `scripts/` 与 `docs/`）⇒ **不提版本**。快速层第 10 步「护栏注入锚点静态核对」正是这一刀的
  常驻回归（它 import 整个包 ⇒ 装配表少点名会当场红）；
- ★ **第四处指名点被我漏了，是快速层当场抓出来的**：`check-invariant-hooks.mjs` 也在**按用例名**解析
  `guards:` 钩子（`docs/protocol-invariants.md` 里 11 条不变量各钉着一具名 Case）⇒ Case 一搬家，
  那 11 条钩子当场「解析失败」（现算读数：`形态 … guard-case:11`）。同批把取名范围改成
  `verify-guards.py` + `scripts/guard_cases/*.py`，并照例做非空转对照：把一个 Case 的名字改坏 ⇒
  该守卫退 1 并把两处都点名（还原 ⇒ 退 0）。
  ⚠️ 这条流程欠账记下来：**我是先提交、后跑快速层**才看到它的。前面逐个跑的 `check-doc-numbers` /
  `check-scripts-parse` / `check-domain-map` 都是我自己列的清单，而权威清单是**那一层的 17 步**；
  清单少列一个判据，就等于那个判据没被跑过 ⇒ 以后每一刀在提交前先跑快速层，不靠"我想起有哪几处"。
  快速层在这条链里已经是第二次当唯一目击者（另一次是 09-29 那趟 rust 锚点）。第 14 步 `Change Budget` 那条红与本刀无关
  （窗口里仍是 a1c4301/615c659/339b8ae 三条 transport 补丁形状提交）。
- ★ 顺手改掉上一刀里我自己抄错的一格：那句写成了「`[61/66] force push 后 …`」—— 用例**序号**会随 `MODULES`
  分段而漂，正是本轮要避免的抄法 ⇒ 改成只点用例名、不点序号（同一条纪律第二次用上）。

- ★ **`lib_tests.rs` 第六刀（4,009 ⇒ 壳 205 行 + 11 个 `lib_<concern>_tests.rs`，最大一册 614 行）
  ⇒ `src-tauri/src` 下所有 `.rs`（含分册）今天已经没有 > 3,000 行的文件**
  （量法见 `docs/large-file-split-plan.md` §1 那条现算命令；逐册 `wc -l src-tauri/src/lib_*.rs`）。
- **分册与 `lib_tests.rs` 同级平铺，不是随手放的**：这个文件近 50 处 `include_str!("commands.rs")`、
  `include_str!("../gen/android/…")` 是**相对本文件**解析的，挪进 `lib_tests/` 子目录会让它们整体偏移一位。
  编译器会拒（不静默），但那就不再是"逐字未变"的搬家 ⇒ 恒等判据从"块文本逐字相同"降级成
  "我读过觉得没变"。这条规矩本来就读得到：`lib_tests.rs` 头部 2026-09-28 那段注释写的正是同一条，
  并拿 `network/transport/tests.rs` 那次换目录必须改 `../ble.rs` 当反例 ⇒ 这次是照它做。
  留在壳里的只有三份守卫视图（`all_commands_src` / `all_db_src`）与解析器 ——
  登记对账那条守卫读的是 `include_str!("lib_tests.rs")` 并按函数名切体，把视图搬走就是弄丢它自己的锚点。
- **搬家用的跨度规则换了（这一步是必须的，不是偏好）**：前一版按字符数 `{`/`}` 算函数跨度，
  在 `lib_tests.rs` 上把 96 项只认出 **57 项** —— 那个文件里就有 `'{' => depth += 1` 这种字符字面量，
  按字符数括号会把它当成真的花括号，于是把 29 个项当成 1 个项（顺带把某项算成 1,126 行，实测 29 行）。
  改成"声明行 `^    fn` ⇒ 收口行 `^    }`"（rustfmt 保证嵌套收口更深），并把
  **认出项数 == `fn` 声明数** 当断言跑（96 == 96 才继续）。同一条断言在上一刀里是 113 == 113。
- **恒等判据同前一批且全绿**：`cargo test --features bluetooth --lib -- --list` 拆前/拆后各 789 条、
  排序后差集 **0 行**；`cargo test --lib` **789 passed / 0 failed**；`cargo fmt --check --all` 0、
  `cargo clippy --features bluetooth -- -D warnings` 0、`verify-guards.py --list` 0
  （202 条锚点各恰好命中一次）、`check-test-manifest --only rust` 0、`check-domain-map` 0
  （11 个新册同批登记进 `unmapped` —— 判据 D「coverageRoots 下不许有无主文件」靠的就是这份登记，
  而它们跨业务领域，不该塞进某个领域的 paths）、`check-domain-deps` / `check-doc-numbers` /
  `check-doc-citations` 0。
- **搬走的守卫自己还会咬（现跑对照，不是推断）**：摘掉 `network/mod.rs` 里 `transport/dial.rs` 那一行登记
  ⇒ 搬进 `lib_source_view_tests.rs` 的 `guard_source_views_register_every_include_subfile` 当场 `FAILED`
  并指名缺哪一个分册（输出 `1 failed; 788 filtered out` ⇒ 过滤确实命中，不是打空退 0 那种假绿）；
  还原后 `1 passed`。摘动的是判据的输入、不是判据本身，跑完 `git checkout` 复原，工作树只留下本次搬家。
- 顺带把**下一刀该动谁**用现算定下来（`docs/large-file-split-plan.md` §1/§4 已改口）：
  `verify-guards.py` 4,176 那条 Case 清单可以先切，但**必须同批改判据 E** —— 它只在主文件这一个文本里
  按 `^    Case\(` 数条数，分册后现算会得 0，而那条判据故意"数到 0 就 throw"（这是设计，不是 bug）；
  `e2e-multi-instance.mjs` 4,845 排最后 —— 现算顶格 `if (MODE)` 轮次块 3,146 行、非轮次部分 1,699 行，
  光搬引擎最多落到 ~3,100（还在阈值边上），而判据 C/D 的尺子就长在这一个文件上 ⇒ 改尺子的风险大于收益。
- 零应用码改动（`isAppCodePath` 把 `lib_*_tests.rs` 判成测试）⇒ **不提版本**。
  全量层 23 步这次跑出 2 条红，两条同一根因、且都早于本刀：`Change Budget 守门`（窗口里是
  a1c4301/615c659/339b8ae 三条带 `Version-Bump: patch` 的 transport 提交）与
  `护栏非空转（前端子集）` 里那条「force push 后 before 不可达不得判空转」——
  后者的"恢复源码后即 PASS"半边正好被前者顶住（受检范围内一直是红的），不是新的坏。

- ★ **`transport/tests.rs` 第五刀（3,227 ⇒ 壳 29 行 + 15 个 `<concern>_tests.rs`，最大一册 336 行）
  ⇒ Rust 侧今天已经没有 > 3,000 行的文件**（量法见 `docs/large-file-split-plan.md` §1 那条现算命令；
  逐册行数 `wc -l src-tauri/src/network/transport/*.rs` 现数）。
- **计划初稿那句"测试归位到各生产分册"被否掉了**，两条理由都是机器定的：
  ① `network::transport_src_for_guards()` 那份"生产码全集"视图**只许装生产码** —— 测试字面量掺进去
  会把按窗口取段的守卫飘到测试文本上（4.33.6 已经为此修过一次，再并进去就是把那个 bug 请回来）；
  ② `lib_tests.rs` 的 `guard_source_views_register_every_include_subfile` 按**文件名后缀 `_tests.rs`**
  豁免测试分册 ⇒ 新册名必须以此结尾。叫 `tests_*.rs` 会被判"漏登记"，而消红最顺手的做法
  （把测试并进视图）正好是**假绿的形状** —— 所以册名是被判据选定的，不是我起的。
  册内仍用 `include!` 拼回**同一个 `mod tests`** ⇒ 模块路径、`use`、可见性、**测试全名一字未变**。
- **恒等判据与前四批同一套，且这次用到最硬的那一种**：拆前/拆后 `cargo test --features bluetooth
  --lib -- --list` 各 789 条、**排序后差集 0 行**（其中 95 条 `network::transport::tests::*` 逐字相同），
  `cargo test --features bluetooth --lib` **789 passed / 0 failed**；`cargo fmt --check --all` 退 0、
  `cargo clippy --features bluetooth -- -D warnings` 退 0（`--all-targets` 那 37 条测试码既有异味
  拆前拆后同为 37，这次搬家没改动它）；`python3 scripts/verify-guards.py --list` 退 0
  ——「202 条用例的注入锚点都在各自文件里恰好命中一次」；`check-test-manifest --only rust` 退 0
  （基线 789 条全部在跑）；`check-domain-map` / `check-domain-deps` 退 0（15 个新册已登记进
  `docs/domains.data.mjs` 的 transport 领域，判据 D「coverageRoots 下不许有无主文件」靠的就是这份登记）。
- **搬家手法本身也自证了一次**：脚本按花括号深度切出 113 个顶层项、块与块之间**无缝覆盖全文件**
  （`covered == span` 断言），分配阶段断言"未分配 0 项 / 重复分配 0 项"（第一次跑就逮到 1 项漏分：
  `gossip_trust_for_known_friend_never_tofus`），末尾再把块按原序拼回原文逐字对账 ⇒
  "内容没动"是脚本算出来的，不是我读出来的。
- 文档两处随批：`docs/large-file-split-plan.md` §1 表改成**首量快照**并标注去向（数字一律以 §1 那条
  现算命令为准），§4 那句"预期主文件落到 <300"按实测改口 —— 没达成也没必要达成：差的正是
  `startup 1,281` / `dispatch 1,658`（含 `handle_message` 1,562 行）两段，**文件级已进阈值，
  再搬只是壳里套壳**，该动的是函数级拆分，那是改控制流的重构、不能和"只搬不改"混在一次提交里 ⇒ 另案。
- 零应用码改动（`isAppCodePath` 把 `*_tests.rs` 与 `tests.rs` 判成测试）⇒ **不提版本**；
  快速层那一步「CHANGELOG 结构」判据读的是本文件的形状，不是这里的条数。

## [4.33.7] - 2026-10-06

### refactor(network): transport.rs 第三、四批 —— 主文件进 3,000 阈值，并推翻"最大单函数 59 行"这个错结论
- **`transport.rs` 第三、四批：再搬 2,531 行进八个分册，主文件 3,941 ⇒ 2,742 行 —— 进 3,000 阈值**。
  这批新增 6 个分册 `transport/{group_file 839, dial 625, handshake 581, peer_registry 244,
  group_membership 145, e2ee_payload 113}`；连同前两批共 13 个分册
  （复跑 `ls src-tauri/src/network/transport/`、`wc -l src-tauri/src/network/transport*.rs`）。
  恒等判据与前两批同一套：`cargo test --features bluetooth --lib` **789 / 0**、
  **逐条用例名与拆前基线差集 0 行**、clippy/fmt/`verify-guards --list`/测试清单守卫/领域图/快速层全退 0；
  26 条护栏锚点现在解析分布在 7 个文件（14 主文件 + group_file 2 + handshake 2 + queue_policy 5 +
  relay_file 1 + outbox_sweep 1 + dial 1），**失败 0 条**。
- ★ **推翻我自己前一轮写进文档的一个结论**：那句"最大单函数仅 59 行"是算错的——
  我第一版跨度函数在遇到尚未开 `{` 的行就返回。按正确的括号深度重量：`handle_message` **1,562 行**、
  `handle_gossip` 840、`handle_group_file_done` 335、`spawn` 304、`connect_to_peer` 247。
  ⇒ 「更优雅的写法」不只是文件级聚合：**函数级拆分是真重构**，与本轮"只搬不改"是两类活，
  必须一 handler 一提交、每步重跑同一套恒等判据，所以另案处理，文档已按实测改口。
- ★ 两次搬家尝试被自己的机器拦下来（这两条记下来是给下一次省事）：一次把 `include!` 那行本身卷进
  待搬段 ⇒ 子目录里的相对路径变成 `transport/transport/…` 编不过；一次按"墙"碎切成 26 段 ⇒
  同一批项被复制进两处，`E0428 defined multiple times`。都是先编译再提交，工作树干净回退，
  最终做法回到**一刀一整段连续块 + 块内断言无 `include!`/无分节横幅 + 结尾必须正好是该item的右花括号**。

## [4.33.6] - 2026-10-06

### refactor(network): transport.rs 第二批 —— 再搬 1,388 行进五个分册，并修掉「视图掺测试文本」这个潜伏守卫 bug
- **`transport.rs` 第二批：再搬 1,388 行进五个 `include!` 分册，主文件 6,648 ⇒ 5,266 行**。
  新分册 `transport/{queue_policy 264, relay_file 535, outbox_sweep 201, read_receipt 118, link_state 296}`
  （复跑 `wc -l src-tauri/src/network/transport*.rs src-tauri/src/network/transport/*.rs`）。
  恒等判据同第一条：`cargo test --features bluetooth --lib` 仍 **789 passed / 0 failed**、
  **逐条用例名与基线差集 0 行**，`cargo fmt --check --all` 与 `cargo clippy --features bluetooth -- -D warnings` 退 0，
  快速层 17 步退 0，测试清单守卫与领域图退 0。
- ★ **搬完 26 条护栏锚点的实际归属是现算出来的**：19 条仍在主文件、5 条跟着进了 `queue_policy.rs`、
  1 条进 `outbox_sweep.rs`、1 条进 `relay_file.rs`，解析失败 0 条 ⇒ `include!` 分册下**锚点自动跟随**
  这件事第一次拿到正对照（不是只读代码读出来的结论）。
- ★ **顺带修掉一个潜伏的守卫设计 bug（早于本轮存在，是这次搬家把它逼出来的）**：
  `transport_src_for_guards()` 那份"生产码全集"视图里一直拼着 `transport/tests.rs`。
  搬家前生产码都在主文件、排在前面，所以没人发现；搬完之后视图里**先撞上 tests.rs 中的字符串字面量**
  （`"async fn reader_loop("` 这类），于是按窗口取段的守卫飘到测试文本上 ⇒ 4 条守卫假红
  （`writer_loop_splits_local_from_socket_failure_exactly_once`、
  `peer_wide_receiver_cleanup_is_gated_on_total_link_loss`、`peer_offline_rule_has_one_home`、
  `peer_offline_group_cleanup_takes_before_finalizing`）。修法是按本仓自己写在 `mod.rs` 注释里的规矩来：
  **视图只装生产码**，把 tests 分册移出视图，登记对账用例的过滤条件同步补上"恰好名为 `tests.rs`"这一种；
  同时把 4 处"直读 transport.rs 单文件文本"的守卫改指全集视图（原来只读主文件 ⇒ 下一次搬家必然再假红一次）。
  这条属于「守卫看着绿、其实读错了文本」那一类，比少一条用例危险。

## [4.33.5] - 2026-10-06

### refactor(network): `transport.rs` 第一刀搬出四个 `include!` 分册（同批：AOCI 认知层接入、安卓 gradle 相对路径修复、大文件拆分计划与两份文档守卫登记）
- **`transport.rs` 的第一刀：446 行搬进四个 `include!` 分册，行为不变的证据是「测试用例名逐字相同」**。
  主文件 7,087 ⇒ **6,647 行**（复跑 `wc -l src-tauri/src/network/transport.rs`），新增
  `transport/{pending_keys 194, member_notices 131, peer_state 92, outbox_flush 41}.rs`。
  手法沿用本仓先例（`include!` 回同一模块 ⇒ 模块路径、可见性、`use`、测试全名都不变），
  所以恒等判据能用最硬的那种：`cargo test --features bluetooth --lib` 拆前 **789 passed / 0 failed**，
  拆后仍 **789 / 0**，**逐条用例名排序后与基线差集 0 行**（不是"条数相等"这种弱判据）；
  `cargo clippy --features bluetooth -- -D warnings` 退 0、`cargo fmt --check --all` 退 0、
  `python3 scripts/verify-guards.py --list` 退 0。
- ★ **三条守卫立刻报红，且三条都是"我漏登记"而不是"代码坏了"** —— 这仓按源码形状钉住的东西必须同批登记，
  这次被机器当场逮到：① `guard_source_views_register_every_include_subfile` 断言**每个 `include!` 子文件
  都要进守卫视图**（视图＝`network/mod.rs` 的 `transport_src_for_guards()`；漏登记是**假绿**，
  relay.rs 在 4.25.0 接线时漏过两次）；② `offline_peer_stays_listed_but_is_not_online` 在源码文本里
  找不到 `pub(crate) async fn mark_peer_offline(` —— 因为我把它按横幅错分进了 `pending_keys.rs`，
  **它按内聚该在 `peer_state.rs`**（同一条证据：作者留的分节横幅不等于内聚，照横幅切会切出杂糅文件）；
  ③ `friend_identity_anchor_has_one_binding_rule` 的计数从 3 掉到 2 —— 视图少看一册，
  「成为好友的三条路径各绑一次公钥」这条判据就少看见一条路。补登记 + 重归位后三条全回绿。
  同批还要在 `docs/domains.data.mjs` 的分册清单里登记这 4 个文件（`check-domain-map` 退 0）。
- **锚点会跟着 `include!` 走，不必改 26 条 Case 的 `file=`**（推翻拆分计划初稿的推断，已改口）：
  `verify-guards.py` 的 `_list_includes()` + `_resolve_anchor_file()` 会递归展开子模块树、把注入写回
  真正含锚点的文件。正对照是活的：**全仓 202 条 Case 里已有 18 条的锚点本来就落在 include 子文件中**
  （chat.rs 4、settings.rs 2、friends.rs 2、favorites.rs 2、logs.rs 2…），且 202/202 解析零失败。
  真实约束因此是另一条：**同一锚点必须在整棵树里恰好出现一次**。
- 拆分进度（阈值 3,000）：`transport.rs` 还差 **3,647 行**才达标，锚点分布已现算排好序——
  下一刀按 `outbox_sweep 316` → `queue_policy 263` → `peers 251` → `mesh_sync 297` → `dial 624` →
  `relay_file 534` → `group_keys 1,094` → `startup 1,281` → `dispatch 1,658`（含 10 条锚点、最后做）。
  计划与安全网见 `docs/large-file-split-plan.md`。

- 新增 `docs/large-file-split-plan.md` —— **>3000 行大文件的清点与拆分计划**（分析交付，未动一行业务码）。
  现算结论：12 个超阈值文件里 3 个是图标按换行数误算、3 个是生成物 ⇒ **真正要处理 6 个**；
  `transport.rs` 7,087 行由 15 条作者横幅分成 13 个关注点，**最大单函数仅 59 行** ⇒ 病症在文件聚合
  不在函数粒度，所以「更优雅的写法」＝沿横幅切 Rust 子模块 + `pub use` 再导出（调用点零改动），
  而不是顺手重写。安全网先建：全仓 202 条护栏 Case 里 **26 条的 `file=` 指着这个文件**，搬家会静默
  弄死锚点 ⇒ 每刀同批改路径并跑 `verify-guards.py --list`；行为不变的证据是产物恒等判据
  （`cargo test --features bluetooth` 断言条数逐字相同 + clippy/fmt 退 0 + 测试基线现算对账），
  不是「读过觉得没变」。`CHANGELOG.md` 自己 13,268 行的归档与否**交他拍板**，`state.rs`/useChatStore
  沿用他 2026-09-25 的「不拆」决定，本计划不碰。

- ★ **跨轮清理以前只看"绿不绿、超没超上限"，不看这份产物有没有被文档指名** ⇒ 被台账点名的"实测锚点"
  会被当普通绿轮淘汰掉。这次盘查里量到：**文档共点名 19 个 `run-…Z`，其中 9 个已经不在盘上**（不可恢复），
  而新加的 `check-doc-citations.mjs` 按设计**跳过 `test-results`** ⇒ 这件事发生时没有任何机器会报，
  只能靠临时脚本现算才看得见（临时脚本只证明"我看到了什么"，不证明"工具管不管"）。
  现在：被点名的绿轮**不进删除名额**；**引用名单拿不到（`git ls-files` 失败）时一个都不删** ——
  "扫不到引用"和"没有引用"是两件事，只有后者允许删。
- **这条判据原来长在收尾的 `finally` 里** ⇒ 要证明新加的两格真会失败，只能先跑满一轮（30–45 分钟）。
  按 `--logtail-selfcheck` / `--shot-selfcheck` / `--report-contract-selfcheck` 三个先例，把
  `prunePlan` / `selfcheckPrune` / `citedRunIds` 提到模块级并给一条 `--prune-selfcheck` 秒级入口
  （不起实例、不碰任何目录，并顺手现印"点名几个 / 在盘上几个 / 已缺失几个"及逐个名字）。
  ⚠️ **起跑前不 throw**：清理逻辑坏了不该让一整轮跑不起来，立场与原来一致 —— 收尾自证不过就
  "一个都不删 + 本轮记一条不合格"。判据只留一个家，收尾那段改成调用同一份函数。
- 自证六格 ⇒ **八格**，新增的一对同一批输入只换一个输入；两条单点变异各自只红对应那一格：
  摘掉引用过滤 ⇒ 只有"被点名的绿轮不删"红（其余 7 格不受影响 ⇒ 说明"留下那个"确实只由引用决定，不是排序巧合）；
  把"拿不到名单"写成"没有引用" ⇒ 只有 fail-closed 那一格红。`docs/stability-roadmap.md` 里那句
  "先跑六格自证"同步改成八格（散文抄条数正是这次的漂法），收尾注释的"三条硬约束"补成四条。
- 回归：`check-scripts-parse` 0 / `check-doc-numbers` 0（**E2E 各轮断言数逐字与改动前相同**、反证档仍 20/20 对齐，
  这次没动任何断言也没加 npm 入口）/ `verify-guards.py --list` 0（代码搬家没弄死注入锚点）/
  `check-domain-map` 0 / `check-doc-citations` 0（28 处 `file:line` 全部落在真文件真行）/
  `--prune-selfcheck` 改前 0、两次变异各恰好红 1 条、还原后复绿；`test-results/` 目录数在前后两次核对都是 298 项，
  这条入口一个目录都没删。零应用码改动 ⇒ 不提版本。
- 顺带（与本条无关的清理，无码改动）：删掉可再生的构建缓存共约 10.8 GB —— Rust debug 产物 7.8 GB、
  安卓/依赖下载缓存 2.7 GB、安卓目标产物 371 MB，`git gc` 把 `.git` 从 166 MB 收到 23 MB
  （`git fsck` 裸退码 0；**`src-tauri/target/release` 特意留着**，它是让下次 `cargo build --release`
  走增量而不是冷编的那半）。仓库 16 GB ⇒ 5.2 GB。
- ★ **入库的那份安卓 gradle 里写的是这台机器的绝对路径**（`src-tauri/gen/android/app/build.gradle.kts`
  第 22 行那条 btleplug Java 源目录，由 `scripts/inject-android-signing.mjs` 注入）。不只是难看 ——
  它是一条**静默坏包**通道：Gradle 对**不存在的 `srcDir` 一个字都不报** ⇒ 别人机器/CI 上那个目录解析为空，
  btleplug 的两半 Java 不进 dex，构建全绿，只在真机 logcat 里现形
  （`failed to resolve Java class 'io/github/gedgygedgy/rust/future/Future'`，正是该文件注释里那条事故）。
  现在注入的是**相对 App 模块目录**的 `file("../../../../scripts/android/btleplug-java")`
  （与同文件里 `file("release.keystore")` 同一套解析口径），脚本另加一条折回核对：算出的相对路径
  resolve 回来不是那个目录就**拒绝写入** ⇒ 生产通道再也产不出带绝对路径的那一行。
  机器证明（`./gradlew --offline -I <打印 main java srcDirs 的 init 脚本> help`，正反对照同一份判据）：
  相对写法 ⇒ `:app` 那个目录 `exists=true`、`.java` 28 个（`com/nonpolynomial` 10 + `io/github/gedgygedgy` 18，
  正是 `build-android-releases.sh` ⓪b 反查 dex 要的那两半）；把它换成"别人机器上的绝对路径"这一形状 ⇒
  **退码仍是 0，而 `exists=false`、计数掉到 0** —— 这类坏法第一次被当场看到，而不是靠下次真机报错。
  同批（AOCI 索引收尾）：`scrape-douyin-emoji.py` 那条 Entry 的 R 原来指着 `code:src/assets`（目录、
  非托管对象，全库唯一一处 dangling），现按现读到的真实关系链改成 `code:src/data/emojis.ts`
  （它首行写明「由 scripts/douyin_comments_emoji/emojis.json 生成」，而那份产物目录不入库）；
  `logs.rs` 那条 Entry 里一个中英夹生词已由机器的 cognition_optimization 批次重写，重写时把
  ⚠️ 一处**注释与代码不符**登记了进去 —— `prewarm_aux_windows` 上方那段"只预热预览这一扇"是过期的，
  函数体现在预览与群任务两扇都建。改那段注释要动应用码 ⇒ 不在本轮，只登记。
  回归：`verify-guards.py --list` 0（那份 gradle 没被任何锚点钉着，改它不会改坏注入锚点）/
  `aoci check` 0（五净）/ `aoci verify` structure_valid+governance_aligned 双 true /
  `aoci index agent guide` stage=aligned、complete=true、next_action=none /
  全库 497 条 Entry 的 R 项逐条复审（存在性 + 是否跟踪 + 是否目录）异常 0 处。零应用码改动 ⇒ 不提版本。
- 新增 `docs/aoci-usage.md` —— **AOCI 认知层使用手册**（工具用法：哪些文件入库、换机器怎么接宿主、
  谁在什么时候必须调它、9 个 MCP 工具与 CLI 只读命令速查、一条 Entry 的格式与 `S` 的两层配额、
  收尾三件套）。写的时候现跑了一遍自查，抓到两处我自己想当然的假命令：**CLI 没有 `aoci header`**
  （字典的现读口是 MCP `aoci_header` 或 `aoci_maintain` 响应里的 `authoring_meta`），以及
  `aoci guide` 不存在 —— Guide 的真位置是 `aoci index agent guide` 且 **`--agent` 必填**。
  已按"每条命令都要在读者站的位置跑得通"逐条改口，并在文档里登记这两个坑。
  导航同步：README 的「AI 开发必读」表加一行（★ 按需）、`docs/AI_ENGINEERING_INDEX.md` 的
  「其余在 docs/ 里」加一条 —— 新建文档不进导航＝约束失效。零代码改动 ⇒ 不提版本。

## [4.33.4] - 2026-10-03

### fix(scripts): `$var` 紧跟中文时 bash 会吞掉变量值 —— 修 10 处 + 加门禁

- **实测确认的坏法**（不是理论推断）：
  ```console
  $ bash -c 'SRC=/tmp/app; echo "A: $SRC（后续）"'   →   A: ��后续）      ← /tmp/app 整个没了
  $ bash -c 'B=x; echo "C=$B）"'                      →   C=��
  ```
  bash 把 `$name` 之后**紧邻**的中文字节算进变量名解析，值整段丢失。
  代价不是崩，而是**错误消息里最该看清的那个值不见了** —— 而这些行几乎都在错误分支上
  （"[错误] 未找到 $BIN，请先…"、"缺少 Rust 目标 $TARGET"），正是排障时唯一要看的那一行。
- **修了 10 处**（`check-mobile.sh` / `e2e-dev.sh` / `pack-macos-app.sh` /
  `t2-learn-id.sh` / `t3-presence-relay.sh` / `t4-mirror-dial.sh`），一律写 `${var}`。
  逐行复核过 diff：只改**紧跟中文的那一个**，同一行里前面是空格的 `$TARGET` 等保持原样。
- **新增门禁 `scripts/check-shell-var-cjk.mjs`**（已接进 `verify.mjs`，`group: frontend`
  ⇒ CI 自动覆盖）。三类误报必须排除，否则它立刻变噪音：
  ① **注释里引用这个坑当反面案例**（`ci-run.sh:23` 写着 `` `$status（` `` 并解释为什么要写
  `${var}`）—— 那是**文档**不是代码，走**按行号登记**的 `EXEMPT`（不接受通配）；
  ② `.yml` 里的 **PowerShell 块**（pwsh 不做这种解析）—— 靠 `run: |` 块内的 pwsh cmdlet
  标记**预扫描整块**；⚠️ 第一版写成"从当前行往上找最近的 `run: |`"，结果
  `build-windows-webview2.yml:96` 没被排除（实测才发现），已改成先标记整块；
  ③ 已是 `${name}` 的形状。
  另有覆盖面自证：扫到 <5 个文件直接红（路径挪了/正则失配 ⇒ 静默空转）。
- **非空转**：注入一处裸写 ⇒ 退出码 1；还原 ⇒ 0。
- 起因是 2026-10-03 一天内**两处**踩到同一件事：`ci-run.sh` 的注释早记着它，
  我在 `publish-release-assets.sh` 写新守卫时又犯了一次（`--label=$label（` 直接
  `unbound variable`）。⇒ 这类"看起来是风格问题、实际会静默吃掉值"的坑值得机器钉。
- ⚠️ 顺带记一条工具事实：**macOS 的 `grep` 不支持 `-P`**，这类 Unicode 扫描用 Python 正则写。

## [4.33.3] - 2026-10-03

### fix(release): 内置 WebView2 那一档在 Release 上被默认档**覆盖**了（用户实报）

- **症状**：Release 的 Assets 里只有 2 个 Windows 安装包（`Gosslan_…_x64-setup.exe` /
  `…_arm64-setup.exe`），**没有**"内置 WebView2"那个包；而
  `Build Windows + 内置 WebView2` 那个 run **Status Success**、两档产物都在
  （arm64 176MB / x64 187MB，与默认档大小逐字相同）。
- **机制（静默覆盖）**：tauri 生成的 exe 文件名只由 `productName` + version 决定，
  **两档的 exe 逐字同名**。webview2 那档用 `merge-multiple: true` 把产物下载到
  同一个 `assets/` 目录 ⇒ **同名文件互相覆盖**，只剩一份，且不带任何 webview2 标识
  ⇒ 这一档在 Release 上彻底消失。artifact 名（带 `webview2-bundled`）是分开的，
  但 `merge-multiple` 恰恰把那个区分**抹掉**了 —— 注释里写"附件名自带 webview2-bundled
  ⇒ 与默认档一眼可分"，而事实是附件名来自 exe 自身，压根没带。
  为什么"产物大小与默认档相同"是**预期**而不是"没换包"：这一档的差别只在内嵌 runtime
  怎么装（`webviewInstallMode`），产物路径与命名都照旧 ⇒ 靠大小分不出，只能靠文件名。
- **修法**：
  ① `build-windows-webview2.yml` 在下载后、上传前给每个 exe 加 `-webview2-bundled`
     后缀（`.sha256` 同步改名）⇒ 与默认档**文件名**不再冲突，两档能同挂一个 Release；
  ② `publish-release-assets.sh` 新增 `--label`（`base` / `webview2`）与**档位守卫**：
     webview2 档出现裸名 exe ⇒ 硬失败并直接告诉人该改成什么名字；
     一个带标识的都没有 ⇒ 也红（这一档等于没发出来）；未知 label ⇒ 红
     （**不默认当 base** —— 默默放行正是这次 bug 的形状）。
- **判据**：`scripts/publishRelease.test.ts` 6 条新用例（15/15 绿），
  两向非空转都验过：把守卫短路 ⇒ 3 条变红、还原 ⇒ 15/15。
  含两条**反面对照**：默认档的裸名 exe 是基线、不许被自己的守卫拦下；
  macOS/Android 档（dmg/apk，不含 `setup.exe`）不受影响。
- 顺带修一个**我自己写出来又当场被测试抓住**的隐患：守卫里 `echo "… --label=$label（…）"`
  的 `$label` 紧跟全角括号 ⇒ bash 报 `label?: unbound variable`
  （本仓 `ci-run.sh` 早前也踩过同一个，注释里记着）。已写 `${label}`。
  ⇒ 这类"变量紧跟全角字符"目前**没有护栏**，今天两处各中一次，值得补。

## [4.33.2] - 2026-10-03

### fix(guard): force push 后 Change Budget 不再自己判自己空转
- **真实事故（CI run 37106836002）**：发 `v4.33.0` 时用了 amend + `--force-with-lease`
  改写提交声明（patch → minor），于是 GitHub 事件里的 `event.before` 指向的是
  **刚被改写掉的那个提交**（`1231aa2`）。它不在任何 ref 上，而 CI 是从 GitHub
  全新克隆（`fetch-depth: 0` 只拉 ref 上的对象）⇒ `git cat-file -e <before>^{commit}`
  必然失败 ⇒ 脚本拿不到 `before..sha`，退到 `HEAD~1..HEAD`。
- **矛盾在哪**：那条兜底范围**确实判到了 1 个 commit 并把三条判据全跑完**，
  却仅因为「来源不是 `before..sha`」被判 `exit 1`。门禁自己兜住了范围，又自己判这个
  兜底无效 —— 自相矛盾。而 `docs/VERSIONING.md` §3 明确允许改写已推送历史
  （那条流程第一步就是 amend + force push），所以这不是该拦的形态。
- **修法**：把"env 压根没喂"与"喂了但 `before` 不可达"分成两种情形。
  只有前者硬失败（那正是这条判据 2026-09-28 立起来要拦的洞）；后者打一条
  **如实说明覆盖局限**的警告后放行 —— 不写"全部提交都判到了"（实测那次 force push
  引入 3 个提交而 `HEAD~1..HEAD` 只判到最后一个，固定深度猜不出这次改写动了几个），
  而是打印判到了几个、判不到的是什么、需要全覆盖时该在本地跑什么。
- **判据**：`verify-guards.py` 新增一条非空转 Case「force push 后 before 不可达
  不得判空转（改回去必须红）」，与既有的「真·env 没喂必须红」互为对照 ——
  两条都守住，才既不误红又不放松。非空转实测：注入 `if (false && …)` ⇒ `exit 1`；
  还原 ⇒ `exit 0`。
- 顺带把 `docs/ARCHITECTURE-MAP.html` 里手写的护栏非空转用例数从 201 改成 202
  （由 `check-doc-numbers.mjs` 对账现算，不是手改数字了事）。

## [4.33.0] - 2026-10-03

> **为什么是 minor 而不是 patch**：提交原本声明 `patch`，但这一批里有一条
> `feat(guard): 新增「文档 file:line 引用对账」门禁` —— 按 `docs/VERSIONING.md` §1
> 「`feat` ⇒ 中」且**「改动规模与线索词不参与定档」**，用户可感知的新能力落在 minor。
> 声明与版本位已一并改正（`Version-Bump: minor`），不是"声明 patch 却发 minor"的错配。
> 另三条 `fix` 是用户可见的缺陷修复（重发重复气泡、破坏性确认框显示裸 key、英文界面
> 显示中文 toast），按 SemVer 属向后兼容的改进，不构成 MAJOR。

### fix(send): 重发不再产生第二条气泡 —— 旧 failed 气泡原地复用
- **缺陷**：`MessageItem.vue::retrySend` 走 `chat.send(...)`，而 `send()` 每次都新建一条
  `msg_id = tmp-${Date.now()}-…` 的乐观记录并 `enqueueMessage`。于是列表里同时留着
  「发送失败的原文」与「重发中的原文」两条，而**失败那条永远不会被删**：
  全库没有任何删除类函数（`messages.ts` 导出的 19 个里没有 remove/delete/purge），
  且 `appendLocalOnly` 把 `tmp-*` 视为"只存在于内存"，每次 `loadMessages` 都会把它
  **重新追加**到快照尾部 ⇒ 用户点一次「重发」看到两条一样的文字，切会话重开也一样。
- **修法**：`send()` 新增可选 `retryOfMsgId`。传它就**不再新建乐观记录**，而是按
  `msg_id` 找到那条 failed 气泡、原地转回 `sending`，成功后 `replaceMessage(convId,
  retryOfMsgId, …)` 替换**同一条**；再失败则退回 `failed`。找不到那条（切了会话/
  已被快照吞掉）时**退回新建** —— 宁可多一条也不静默丢消息。
- ★ **为什么不能字面"复用同一 msg_id"**（INV-001 的另一半在本项目做不到）：
  后端 msg_id = `SHA-256(sender_id + nonce + payload)`（`protocol.rs::compute_message_id`），
  nonce 每条新消息都不同 ⇒ 重发**必然**是新 msg_id；且 `send_message` 的签名只有
  `(friend_id, content, kind)`，**没有 msg_id 参数**可传。所以 INV-001 的"重试复用同一
  msg_id"在本项目落在**接收侧幂等**（`message_exists` 按 msg_id 去重），前端这一侧
  只能做成"旧气泡原地复用"—— 逻辑消息在界面上仍是同一条。
- 顺带把单聊/群聊的 invoke 分流抽成 `sendToBackend`（重发与首次发送共用），
  避免两条路径各写一份 `startsWith("group:")` 分流。
- **撞上一条既有守卫并正面处理**：`storeContract.test.ts` 的 #82 形状守卫
  （"返回 MessageRecord 的命令，调用点必须 enqueueMessage"）在 `sendToBackend` 上变红。
  它不是漏 enqueue —— 记录去哪儿**由调用方决定**（新建 vs 重发两条路径不同），
  转发层擅自 enqueue 反而会把重发路径那条旧气泡顶掉。为此给该守卫加了
  **带理由的白名单**，并配两条自证：① 白名单里的函数必须**真的存在且仍在调**返回记录的
  命令（否则函数改名后豁免会静默留着，"看起来有豁免、实际不豁免任何东西"）；
  ② 白名单**上限 3 条**（膨胀到一半说明它被当成万能洞用，该问"能不能不抽这一层"）。
  两条自证都做过非空转（指向不存在函数 / 塞满 4 条 ⇒ 都变红）。
- 判据 `src/utils/retrySend.test.ts`（5 条，含形状断言与后端前提对账），
  **两种非空转都验过**：把 `retrySend` 退回不传 msg_id ⇒ 红；整个重发分支删掉 ⇒ 3 条红。
  其中一条钉住后端"msg_id 不可由客户端指定"这个前提 —— 哪天 `send_message` 加了可选
  `msg_id`，前端就该改成真复用，本文件的推理与注释都要重写。
### fix(i18n): 取消发送的三条结果提示不再硬编码中文
- `MessageItem.vue::doCancelSend` 的三条用户可见文案（"已请求取消发送" /
  "标记为已取消（传输可能已结束）" / 错误前缀）全是中文字面量 ⇒ 英文界面下显示中文 toast。
  同一函数里 `@cancel` 的入口文案走了 `t()`，自相矛盾。
- 补 `msg.cancelSend.requested` / `.alreadyEnded` / `.fail` 中英各一条。
  **不复用**已有的 `msg.canceled`：那条是"已取消发送"（`onFileCancelled` 的事件提示），
  而这三条讲的是"用户点了取消之后发生了什么"，语义与语气都不同。
- 全仓复查：`app.toast(...)` / `toastError(...)` 已无硬编码中文。
### refactor(store): 在线判定收成唯一一份实现（`utils/friendOnline.ts`）
- `friends[].online` 的判定此前被抄成两份：`onPeers` 那条链带
  「或持有活跃链路也算在线」，`searchNearbyPeers` 那份**漏了**。后者是"添加好友"弹窗
  的探测路径（`commands/network.rs::search_nearby_peers`），漏了那层意味着一次
  `who_has` 探测回来后，正在保持 TCP 链路的好友被判离线 ⇒ `ChatHeader` 的
  `v-if="!isGroup && online && linkState"` 整块链路信息凭空消失。
  同一份语义的两个家必然漂移（AI_RULES §32），现收进 `utils/friendOnline.ts`，
  两个调用点共用。抽成纯函数而不是留在 store 私有函数里，是为了**能测** ——
  藏在 store 里的口径护栏无从下手，这正是它漂了没人发现的原因。
- 判据随实现一起搬家：`channelState.test.ts` 那条 2026-09-14 的用例（用户实测
  "局域网都连上了，在线状态却不实时"）原先正则匹配 store 里那一行，代码搬走后会失效；
  `scripts/verify-guards.py` 的**注入锚点**同步跟到新文件。
  并加一条**调用点个数**断言（必须恰好 2 处调用）—— 否则将来任一路径改回自己算 online，
  `linkedIds` 那层会被悄悄丢掉，而这正是本条判据当年钉住的缺陷。
- ★ **顺带查清一件被误报的事**：`linkedIds` 那一层在当前后端数据形状下是**冗余**的。
  `state.rs::emit_peers_now` 与 `commands/network.rs::fill_peer_links` 这两个填 `link`
  的入口**都只给「已经在 peers 表里」的节点填**（都是先收 `peers`、再
  `for p in peers.iter_mut()` 填），恒有 `linkedIds ⊆ onlineIds`，删掉行为不变。
  审计报告的 F2（"`searchNearbyPeers` 漏 linkedIds ⇒ 好友被闪成离线"）**据此撤回**：
  节点不在 peers 表里，后端根本不会给它 `link` 字段，前端拿到的 list 里不可能有它。
  `state.rs:1770-1774` 那段注释描述的痛点是真的，但**不是前端能修的** ——
  该修的是后端让 peers 表覆盖"有链路但广播没到"（或让 `fill_peer_links` 额外接受一组
  待填 id），不在本次范围。**这一层仍然保留**：它无害，且后端哪天真的支持了表外节点带
  link 就会自动生效；但推导过程写进了注释与新判据，避免下一个人误以为它在兜什么。
- **非空转实测**（本仓铁律，两种都验过）：
  ① 删掉 `|| linkedIds.has(...)` ⇒ `friendOnline.test.ts` 的形状断言与
  `channelState.test.ts` 的用例**都**变红；
  ② 把后端 `fill_peer_links` 改成不再只遍历 peers 表 ⇒ 钉着数据形状假设的那条判据变红。
  ② 是必要的：它防的是"后端改了数据形状而前端注释/推导没跟上"。
- 过程里踩了一次**判据自等于**并当场抓到：第一版新测试把设备同时放进了 list，
  于是"看起来在测 linkedIds、实际 onlineIds 就够了"，删掉 linkedIds 层 6 条用例全绿。
  改法是把那条改成**形状断言**并显式写明"删掉它行为不变，但它是 2026-09-14 那条判据
  钉着的契约" —— 而不是留一条看起来在测、实际测不到东西的用例。
- ★ **「版本号同步几处」在文档里有三个不同的数，而实现是五处**（纯文档/注释口径修复，零应用码改动）：
  `scripts/version.mjs` 头部注释写"三处"、`README.md` 与 `AI_PROJECT_HANDOFF.md` 各写"4 处"，而本文件开头的
  前言一直写的是五处 —— 第 4、5 步（`package-lock.json` / `Cargo.lock`）落地时只改了实现和本文件，
  那三份副本没被带上。现在统一成五处，并修掉 version.mjs 里重复编号的第二个 `// 5)`（→ `// 6)`）。
- **判据 4 的一条理由与实现相反**：`check-change-budget.mjs` 注释写着 `Cargo.lock`"由 cargo 构建时同步、
  可以合法地晚一版"，而 `version.mjs` 第 5 步现在把晚一版直接 `exit 1` 拒掉。对账集仍是那四个清单文件
  （没有放宽任何判定），理由换成"第五处已由 version.mjs 自己断言，不在判据里重复数一遍"。
  留着错理由的代价是具体的：下一个人会据此认为 `Cargo.lock` 不一致是可接受的。
- **README 手抄的 IPC 命令数漂了一格**（写 129、注册表 130）。契约图自己早就为这个形状改过口
  （那张表的行数由页头 `CMDS.length` 现数，图里还点名说"以前这里抄过 129、同页另一行写 130，两条都是手抄"），
  但 README 那份副本没跟着走，而 `check-doc-numbers` 只覆盖门禁步数与 E2E 断言数 ⇒ 属于静默漂移那一类。
  README 现在不带这个数，改指向 `CMDS.length` 与 `check-key-boundary.mjs` 自己打印的那行"注册表 N 条"。
- **两处"用现在时命名过去读数"的表头**：`AI_PROJECT_HANDOFF.md` §6 那张表的列名从"现状"改成
  "当时读数（v1.0.0 · 2026-09-08，今天不作数）"，并在表下给出三把现成量具
  （`check-test-manifest.mjs` / `cargo test --lib` 末行 / `e2e_peer` 自己打印的结果块），
  同时写明别把读数抄回表里 —— 那张表正是这么坏的；`docs/VERSIONING.md` §4 的标题从"现在的台账与数字"
  改成"台账与数字怎么取"，184 / 23 / 45 / 116 与 `2.1.2 → 3.0.0` 就地标为 3.0.0 那一次的读数。
- **约束导航漏了 5 份文档**。`docs/AI_ENGINEERING_INDEX.md` 作为"约束文档导航"没提它们，其中
  `docs/VERSIONING.md`（自题"从 2026-09-12 起强制执行"）和受硬数字守卫的活文档
  `docs/acceptance/stability-smoke-matrix.md` 在 README / 索引 / AI_RULES 三处入口全是零引用
  ⇒ 只从 README 找约束的人根本碰不到它们；`docs/P1-image-out-of-sqlite-overview.md` 此前全仓零引用。
  索引新增一节逐份写明角色与时效（含 `ARCHITECTURE-EXPLAINED.md` 自带的 v4.2.7 快照戳已漂），
  README 的 `docs/` 目录树同步补齐。
- 没动的两处附理由：`probe-doc-command-names.mjs` 报的 2 条"断链"是它咬到自己（`docs/final-architecture-review.md`
  里一个在讲"命令名断掉是响亮失败"、一个在记录往 VERSIONING 注入假脚本名的非空转实测），文档原文不改、
  也不给量具开白名单 —— 它本来就不是门禁步（#131 判过），而整份排除又会把它另外 15 个真引用的覆盖一起丢掉。
  契约图 `ARCHITECTURE-MAP.html` 一字未改：它的 130 与自数口径现读就是对的。
### docs(ledger,tcp): 台账的失效引用与过期判定，注释的腐烂行号
- `docs/migration-ledger.md` 5 处 `docs/domains.yml` → `docs/domains.data.mjs`、2 处 `active_home`
  与 3 处 `second_home` → 驼峰 `activeHome`/`secondHome`。那个 `.yml` **从不存在**
  （`.yml`→`.mjs` 是有意改名，工具链没有 YAML 解析器），而台账 §5「维护规则」是写给接手者的
  **操作指令**：照着改会去打开一个不存在的文件、用错字段名，且 `check-domain-map.mjs` 读的是
  `.data.mjs` ⇒ **门禁照亮绿灯**。
- 9 条行数证据改成自指表述（`network/transport.rs` 声称 8836 实测 7086、`file_relay.rs` 声称 86
  实测 504、`mesh/` 声称 10 文件 2432 行实测 11 文件 3276 行…）。行数在这份文件里是**论据**不是修辞：
  §4 用"最大最活"把它排在收口顺序**最后一位**，这个优先级就建立在那个行数上；而 §1 又声明
  "行数不是判据"，口径自相矛盾。统计段两处 `db::` 引用数（13 / 43）同样自相矛盾且都错（实测 12 / 56）。
- §3「过期声明上报」第 1、2 条判定**作废**：源码 2026-09-16 就改成「已接线」了
  （`transport/bluetooth.rs:44`、`transport/tcp.rs:12`），台账却一直挂着红牌，让下一个人重新调查
  当天就解决完的事。
- `src-tauri/src/transport/tcp.rs` 模块头两行「接线状态」表的 `file:line` 全部重钉到真符号：
  帧原语在 `network/transport/outbound.rs:45,105`；`TcpReceiver`/`TcpSender` 在
  `network/transport/relay.rs:392,393`。原写的 `network/transport.rs:57,62,69` 与
  `:1231,1232,1529,1613,2368,2369` 逐个核对**全部指向无关代码**（`"transport"` / `format!(` /
  `high_open` …）—— `network/transport.rs` 已目录化、数据面搬进了 `network/transport/*.rs`。
  **之所以危险**：注释说"已接线"是对的、**行号是烂的**，这比"注释撒谎"更隐蔽 —— 顺行号核对的人会
  看到无关代码，从而误判"注释在骗人"而把好的接线拆掉。这是本仓「待接线声明腐烂」族的第三次
  （`ble_framing.rs` Phase 3、`transport/bluetooth.rs` Phase 5、此处），本次由 2026-09-16 那次核过、
  到今天又烂掉亲自坐实。零行为变更（`src-tauri/` 侧 diff 过滤后为空）。
- 验证：快速层 15 步全绿；`cargo fmt --check` exit 0；clippy 警告数 42，与改动前基线
  （`git stash` 对比）逐条一致、零新增。
### fix(i18n): 破坏性确认弹窗标题显示裸 key —— 补「t() 调用点必须命中词典」护栏
- ★ **新护栏：`t()` 调用点引用的 key 必须在中英词典里命中**（`src/i18n/index.test.ts`）。
  一次只读审计（2026-10-03）查出**破坏性确认弹窗的标题显示裸 key**：自动删除策略那个
  `BaseModal` 写的是 `t('settings.storage.limit')`，而这个 key 中英两侧都不存在 ——
  `t()` 的回落是 `dict.value[key] ?? key`（`i18n/index.ts:93`），于是用户点开"要永久删除历史
  消息引用的图片/文件"的确认框，标题直接写着 `settings.storage.limit`。同文件正文用的
  `settings.storage.confirm.body` 是存在的 ⇒ 漏了一条，不是有意为之。
- **为什么已有两条字典护栏抓不到**：它们只查「中英 key 集合互相一致」和「值非空」，
  对"调用点引用了不存在的 key"完全无感 ⇒ 假绿。新护栏扫全仓 `.vue`/`.ts` 的 `t("字面量")` 调用点。
  两条设计取舍写进注释：① **只查字面量**（`t(\`prefix.${kind}\`)` 那类模板字符串由各自专项用例
  钉住，如 relay.rs `as_str` ↔ TS 联合类型 ↔ 两种语言那条三向断言）；② **`.test.ts` 整个跳过**
  （测试本来就要引用不存在的 key 来验回落，扫它们得到的红是假的 —— 假红会让人养成
  "先加白名单再跑"的习惯，那比漏报更危险）。
- 护栏上线当场抓到**另外两个真漏项**（都不是审计报告里列的，是护栏自己发现的）：
  ① `BaseModal.vue:150` 用 `t('common.close')`（关闭按钮的 `title` + `aria-label`），词典里只有
  `closeEsc` 没有 `close` ⇒ 关闭按钮的悬浮提示与**无障碍标签**两种语言下都是裸 key；
  ② `utils/storeContract.test.ts:84` 期望真实代码用 `t('msg.fail')`，词典里也没有 ⇒ 错误提示显示裸 key。
  三处都补齐（`common.close` 中英、`settings.storage.confirm.title` 中英），`common.close` 那条
  顺带把"此前只写了 closeEsc"的原因记在旁边，免得下次又被当成多余条目删掉。
- `settings.storage.limit` 改名为 `settings.storage.confirm.title`：调用点与词典一起改，跟同族
  `settings.storage.confirm.body/.cap/.keepDays` 对齐 —— 原来的名字既不在词典里，也不跟任何兄弟条目同族。
- **护栏的非空转实测**（本仓铁律：新护栏必须证明它不是空转）：删掉 `common.close` 的英文条目
  ⇒ 两条判据同时变红；只删中文、保留英文 ⇒ 中英一致性那条与新护栏**都**变红且新护栏点名
  `zh-CN 缺 common.close（src/components/BaseModal.vue）`。两种形态都验过，验证完已还原。
  顺带确认了旧护栏的真实盲区：它抓"漏译"（一侧有一侧没有），抓不到"两侧都没有" ——
  而后者才是这次事故的形状。
- `scanned > 50` 自检：一条调用点都扫不到时判据直接失败（正则失配或目录搬迁会让护栏悄悄空转）。
### feat(guard): 新增「文档 file:line 引用对账」门禁
- `scripts/check-doc-citations.mjs`（已接进 `verify.mjs` 第 7 步，`group: frontend`
  ⇒ CI 的 `verify.mjs --group frontend` 自动覆盖）。三条判据：文件存在（按 `SOURCE_ROOTS`
  解析，裸名按全仓唯一同名）／行号落在文件范围内／**扫到 <20 处引用判 INCONCLUSIVE 而非绿**。
- **上线当场抓到本台账两处真断裂**：一处引的 `transport/mod.rs` 行号**超出该文件行数**，
  一处引的 `lib.rs` 行号**超出全文五倍**。两处都已重钉到真符号。
- ⚠️ **刻意不判「那一行是不是文档说的那个符号」**：那要解析 Rust/TS 语法，解析错了门禁会
  静默失效 —— 比没有门禁更危险。这条边界同时写进脚本头与台账（§4 末尾），
  免得下一个人以为它能抓符号腐烂。后果举例：台账 §3 第 3 条引的 `route()` 已被 0-A2
  整体删除，行号仍在范围内、门禁判绿，但那一行今天根本不是 `route()`。
- **故意不管**：带日期的复审快照（`ARCHITECTURE-REVIEW-*` / `final-architecture-review`）
  与 `CHANGELOG` —— 它们记录的是**当时**的读数，拿今天的事实源去判必然误报。
  与 `check-doc-numbers.mjs` 同一立场（能被现算的东西不许留第二份手抄），只是那条管「数字」、
  这条管「指向」。
- 非空转三种形态都验过并已还原：超界行号／不存在的文件／正则失配（判据 C）。
- 过程中自己踩了两次同一个坑：**在文档里「提到」一个已知坏行号**（叙述历史）时，门禁把它
  当「断言」判红。⇒ **提到行号**与**断言行号**必须分开写；复述一个已知坏行号 = 新造一条断裂。
### docs(invariants): 新增 INV-P30「前端自洽」一节
- `docs/protocol-invariants.md` §30（5 条规则 + 3 行反向判据速查表）把 2026-10-03 那三个
  补丁收敛成明文契约：同一份派生状态只有一个家／`t()` key 必须中英两侧都存在／重发复用
  失败那条气泡／派生值不粘住上一轮／同一份判据不许被抄成两份。
- 钩子 30 条全部解析成功（`check-invariant-hooks.mjs` 现场验）。
- 这一节是对 Change Budget 判据 3 的正式回答：它把 `presentation` 域连打三个补丁
  识别为 4.18.7→4.18.10 那种「每个补丁都很小、但它们在互相修」的犯案形态，
  并要求「① 该领域的不变量补了吗」—— 现在补上了。
- 第 1 条特意钉「必须恰好 2 处调用」：只钉「判定含 `linkedIds`」不够 ——
  有人把某个调用点改回自己算 `online`，那条断言照样绿。数调用点个数才关得住第二个家。

## [4.32.0] - 2026-09-30

### 工具口径修正（只动 `scripts/` 与 `docs/` ⇒ 按 `isAppCodePath` 不占版本号）

- **`scripts/sign-readiness.mjs` 那句「接在哪」原先是错的，现读改掉**：它叫人在 `tauri build` 之后另写 `codesign --sign` 与 `notarytool submit --wait + stapler staple` 两步，而本仓装的 **tauri-cli 2.11.4 自己就做签名与公证**。现读三处：`./node_modules/.bin/tauri --version` ⇒ `tauri-cli 2.11.4`；`tauri build --help` 里有 `--no-sign` 与 `--skip-stapling`（后者说明默认它会等公证完并 staple）；`config.schema.json` 里有 `bundle > macOS > signingIdentity` 与 `bundle > windows > signCommand`（Windows 的钩子是这条带 `%1` 的自定义命令，不是内置 signtool）；`cli.darwin-arm64.node` 的字符串表里出现 `APPLE_SIGNING_IDENTITY`、`APPLE_CERTIFICATE` / `APPLE_CERTIFICATE_PASSWORD`、`APPLE_API_KEY` / `APPLE_API_KEY_PATH` / `APPLE_API_ISSUER`、`APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID`、`APPLE_DEVELOPMENT_TEAM`、`APPLE_PROVIDER_SHORT_NAME` ⇒ 该挂的是**那一步的 env 与配置键**，不是再加两步 shell。⚠️ 边界：以上全是版本固定的**读数**，本仓没有凭据、**一次都没实跑过签名或公证**；换 CLI 版本这段要重读。
- 验收矩阵加 `Smoke-13` 一行（标题栏遮挡 / 表情入口 / 任务编号列的真机走查），写明机器只判到浏览器内、等级不往上抬；维护规则第 4 条那句「当前只有第 14 行满足」改成内容锚 + 现读 `verify.yml` 的口径。

### 真窗口读数进了仓 + 内置 WebView2 那一档开始挂 Release

- ★ **`npm run verify:ax` 多了两档"读数前置"**：`--fixture todo` 先给一个**本次新建**的隔离实例库写一条
  生产形状的群任务定义行，再读真实 WKWebView 的无障碍树；`--fixture todo-nonumber` 是同一条但 `number=0`。
  现跑读数（真 App，不是浏览器）：带号档 `AXStaticText=R2345` 与 `AXGroup=任务编号 R12345` **各 1 个**，
  无号档带编号字样的名字 **0 个** ⇒ 那两条不是探针自己造的。
  **有意不判红**：这一段只打印，不调 `ok()/bad()` ⇒ 该入口那四条判据与退码一字不改（这一屏要的是
  "读屏用户实际念到什么"，文案改了期望就该跟着改，进任何一层都会变成"改文案要改门禁"）。
  新增 `scripts/axTodoFixture.mjs`（两档 + 写库）与 `scripts/axTodoFixture.test.ts` 五条用例，其中一条
  **现读 `src-tauri/src/protocol.rs` 的 `TodoPayload` 字段名**当分母 ⇒ 生产加字段而夹具没跟上就红
  （这条一落地就抓到我今天那份一次性探针**漏了 `done_at`**）；`seedTodoFixture` 两档各占独立 `msg_id`
  （同 id 会互相顶掉）。参数只认空格写法：`--fixture=todo` 现在**拒跑**（退 2）而不是被静默忽略后
  给出一份"看着像夹具"的读数。矩阵 `Smoke-13` 那一格同步改口（编号列已有真窗口读数，遮挡与 hover 仍无）。
- **`build-windows-webview2.yml` 加了一个 `release` 任务**（负责人选 A）：这一档以前只 `upload-artifact`、
  从不挂 Release ⇒ Release 的下载列表里永远看不见"内置 WebView2 运行时"那个包，产物只在 Actions 里活着会过期。
  现在与另三条同构（取那一份 `scripts/publish-release-assets.sh` → 取本 run 的两档包 → 挂上同一个 Release），
  附件名自带 `webview2-bundled` ⇒ 与默认档一眼可分。⚠️ 本机没有任何 YAML 解析器，这份文件**没被一层判据读过**
  （现读：`verify-guards.py` 里没有一条用例的注入目标是 workflow 文件），第一次真跑只能等下一个 tag。
- 顺手记一条今天的事实：`v4.31.41` 的 Release 页已由 CI 建好，**8 个附件**（两份 APK + 两份 `.sha256`、
  两份 dmg、两份 exe），四条腿全 success ⇒ 这就是那条 `--repo` 修复的第一次真跑；
  ⚠️ **只有 Android 带校验文件**，mac 的 dmg 与 win 的 exe 都没有 `.sha256`（要不要补齐是另一个决定）。
### 修一条远程红灯：macOS 的 DMG 打包步加了"只认偶发"的有界重试

- **红在哪一步（负责人贴的日志，run `36714419638`）**：`build-macos (x86_64-apple-darwin, x86_64)` 的
  「Build .app + .dmg (x86_64)」，日志顺序是 vite build ✅ → cargo `Finished release profile in 4m40s` ✅ →
  `Built application at .../x86_64-apple-darwin/release/gosslan` ✅ → `Bundling Gosslan.app` ✅ →
  `Bundling Gosslan_4.31.41_x64.dmg` → `Running bundle_dmg.sh` →
  `failed to bundle project: error running bundle_dmg.sh` → `Process completed with exit code 1.`，
  后面 `Upload .dmg artifact` 被跳过。**同一条 run 的 arm64 腿绿**，而**同一个 x86_64 腿 1 小时前
  （v4.31.41 那趟）也是绿的** —— 中间那次提交没动应用码也没动这份 workflow ⇒ 输入一字未变、结局不同，
  这是 `bundle_dmg.sh`（hdiutil attach + Finder 排版）在 macOS runner 上的偶发，不是回归。
  ⚠️ 我一开始想自己读逐 step 结论，匿名 API 额度 20:50 打到 0（`remaining=0`/403，21:31 才回），
  是负责人把日志页贴出来才定到位；Actions 的 HTML 列表页我没当证据用 —— 它把 `v4.31.40` 的 macOS 也
  显示成 Success，而那次红在发布步是 API 逐 step 证实过的。
- **修法：有界重试，但只认"偶发"那一种形状**（`build-macos.yml` 那一步现在自己判）：
  二进制没产出 ⇒ 判编译/链接问题，**不重试**直接红；`.dmg` 已经在 ⇒ 挂在打包之后那一步，**不重试**直接红；
  `.app 出了、.dmg 没出` ⇒ 才重试，最多三次，三次仍无 `.dmg` 就红。
  **绝不加 `continue-on-error`** —— 那正是把"Release 少一档安装包"静音成绿的做法（v4.31.37 丢 Windows x64 就是这么丢的）。
- 五条分支本机现跑（`/tmp/retrytest`，桩 `npm` 造出各档结局，`sleep` 缩到 0）：
  编译失败 ⇒ 退 1 且不重试；打包失败且无 `.dmg` ⇒ 重试到第 3 次退 1；失败但 `.dmg` 已在 ⇒ 退 1 并说"重试没有意义"；
  一次成功 ⇒ 退 0；**前两次挂第三次成 ⇒ 退 0（这条才是"偶发被接住"的正面证据）**。
- 边界：这份 workflow **不被任何一层判据读**（现读 `verify-guards.py` 无一条用例的注入目标是 workflow 文件，
  `verify.mjs` 不解析 YAML），本机也没有 YAML 解析器 ⇒ 这段重试在真 runner 上的第一次执行只能等下一个 tag；
  本地能证的只有"抽出来的那段 shell `bash -n` 退 0 + 上面五条分支"。
