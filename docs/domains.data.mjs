/**
 * Gosslan 领域图（machine-readable）。
 *
 * ## ⚠️ 为什么是 `.mjs` 而不是 `domains.yml`
 *
 * 原计划写 YAML，实测**工具链里没有 YAML 解析器**：Node 没有（`node_modules` 里无
 * `yaml`/`js-yaml`）、Python 没有（无 `pyyaml`），只有 Ruby 有 —— 为一个守门脚本引入
 * Ruby 依赖不合适，而**在守门脚本里手写 YAML 子集解析器更糟**：解析错了会让守门静默失效，
 * 那比没有守门更危险。
 *
 * `.mjs` 的额外好处：零解析（直接 `import`）、支持注释（就是本文件）、
 * 后续 Phase 6 的 Change Budget 也能 import 它拿 `tier`。
 *
 * ## 这份文件是干什么的
 *
 * 它把「领域 → 路径 → 层级 → 不变量 → **活路径在哪个家**」写成机器可读的形式，供：
 *
 *   ① `scripts/check-domain-map.mjs` 守门（路径必须存在、一文件不许属两域、不许有文件没归属）；
 *   ② Phase 6 的 Change Budget / 修改半径检查（读 `tier`）；
 *   ③ 人（与 AI）在动手前回答「我改的是哪个领域、它的边界在哪」。
 *
 * ## ⚠️ 为什么必须有 `active_home`（本文件最重要的字段）
 *
 * 本仓库处在一次**半完成的 ADR 迁移**中：同一个关注点常常有**两个家**
 * （老的还在跑数据、新的部分接线）。此时"域 = 某个目录"是**错的地图** ——
 * AI 会照着它去改新家，而真跑数据的是老家，于是「改完这个 bug 又冒那个」。
 *
 * 所以每个领域强制回答：**这个关注点有几个家？哪个在跑数据？**
 * 逐条证据（file:line）在 `docs/migration-ledger.md`。
 *
 * 历史教训：Phase 2 我按「用例上方的 #[cfg]」推导 Windows 测试基线、漏掉**模块级** cfg，
 * 被 CI 当场打回；Phase 3/4 又先后发现 macOS、Android 两侧各有一份"载荷预算"的独立实现。
 * **地图错了比没有地图更危险** —— 它会被当成事实执行。
 *
 * ## `consumes` 字段（为什么它跟 `enforce` 一样重要）
 *
 * 每个领域有一个可选的 `consumes: [domainId]` 列表，**声明本领域合法依赖的
 * 其他领域**。`scripts/check-domain-deps.mjs` 会扫描域内每个 `.rs` 文件的
 * `use crate::xxx`（**排除 `#[cfg(test)] mod tests` 内**），对每条引用解析
 * 它的归属：
 *
 *   · 落到别的领域 → 那个领域必须在 `consumes` 里，否则 FAIL；
 *   · 落到装配层（`commands` / `state` / `network` / `lib` / `main`）→ 放行；
 *   · 落到一个既没在任何领域 paths、也不是装配的模块 → 放行（unmapped）。
 *
 * 这一层的关键判断要诚实 —— `consumes` 太严，CI 第一天全红；`consumes`
 * 太松，啥也拦不住。当前值基于实测 `use crate::xxx` 清单（**生产代码**，
 * 不是测试），逐条对应 `docs/migration-ledger.md` 的事实。
 *
 * **不写 `consumes` 也能跑守门**（守门只在引用落到已声明的别的领域时 FAIL），
 * 但 `consumes: []` 跟"我没想清楚"是两件事 —— 至少写个空数组，强迫自己
 * 决定。
 *
 * ## `enforce` 为什么全是 false（别急着打开）
 *
 * `enforce: true` 表示"该领域边界已是事实、可由机器守住"（例如禁止跨领域直接引用）。
 * 现在**一个都不能开**：多数领域边界还在迁移中，开了第一天就全红，而红门禁会催生绕过。
 * 规则：**边界收口完成一个，打开一个**（Phase 6）。
 *
 * ⚠️ **`enforce` 当前只守"开关"**（F 判据：`enforce:true` 与 `secondHome` 互斥），
 * 不守"跨领域是否合法" —— 那层守门就是 `check-domain-deps.mjs`，
 * 由每个领域自己写 `consumes` 决定，所以**两个守门各司其职**：
 * `check-domain-map.mjs` 守图的形式，`check-domain-deps.mjs` 守图的依赖方向。
 */

export default {
  version: 1,

  /**
   * 覆盖检查的根：`check-domain-map.mjs` 会要求这些根下的**每个文件**都被某个领域认领，
   * 或显式列进 `unmapped`。
   */
  coverageRoots: ["src-tauri/src", "src"],

  /**
   * 不属于任何业务领域的**文件级**清单（显式列出，而不是"没提到就算漏"）。
   * 加新条目前请先想清楚：它真不属于任何领域吗？
   */
  unmapped: [
    ["src-tauri/src/main.rs", "6 行，只有 tauri::Builder 的入口调用"],
    ["src-tauri/src/lib.rs", "2168 行：模块注册 + 窗口管理 + 命令注册表（跨领域组装点，Phase 7）"],
    [
      "src-tauri/src/commands.rs",
      "142 行 + 25 个子模块 / 26 个 tauri command：**前后端边界**，按领域拆是 Phase 7 的事",
    ],
    [
      "src-tauri/src/commands",
      "commands.rs 的 25 个子模块（include! 物理拆分、逻辑仍属装配层）：",
    ],
    ["src-tauri/src/state.rs", "1494 行 / 284 个 pub 项：跨领域共享状态（耦合热点）"],
    ["src/vite-env.d.ts", "类型声明"],
    ["src/types.ts", "跨领域共享类型"],
    ["src/style.css", "全局样式（令牌化设计体系的落点）"],
    ["src/i18n", "文案（横切）"],
    ["src/data", "内置静态数据（emoji / 快捷键等）"],
    ["src/boot", "启动骨架（窗口 / 主题，横切）"],
    ["src/entries", "各窗口的入口桩（3 文件 64 行）"],
    ["src/layouts", "布局外壳"],
    ["src/assets", "静态资源（图片 / 字体）"],
    [
      "src-tauri/src/network/mod.rs",
      "老栈的组装点：起 TCP 监听 + UDP 发现（跨 transport / presence 两个领域，与 lib.rs 同类）",
    ],
  ],

  domains: [
    // -------------------------------------------------------------------------
    //  1. 身份 —— 我是谁
    // -------------------------------------------------------------------------
    {
      id: "identity",
      name: "身份",
      tier: "L3", // 密钥/身份一错就是安全问题
      paths: ["src-tauri/src/device.rs", "src-tauri/src/crypto.rs", "src-tauri/src/nickname.rs"],
      invariants: ["INV-P11", "INV-P12", "INV-P18", "INV-007", "INV-P22"],
      activeHome: "src-tauri/src/crypto.rs", // 单家，无迁移
      enforce: false,
      consumes: [], // 叶子域：crypto.rs/device.rs/nickname.rs 不 use crate::xxx（外部仅用 Identity）
      notes:
        "「我是谁」不负责「我怎么找到别人」。E2EE 密钥派生与 Hello 验签都在这。私钥永不过 UI 边界（INV-P18）。",
    },

    // -------------------------------------------------------------------------
    //  2. 在线状态与发现 —— 谁能被看见   ⚠️ 双家，未收口
    // -------------------------------------------------------------------------
    {
      id: "presence",
      name: "在线状态与发现",
      tier: "L3",
      paths: [
        "src-tauri/src/network/discovery.rs", // 旧家（活）
        "src-tauri/src/discovery", // 新家（未接线）
      ],
      invariants: ["INV-P21"],
      // ⚠️ 双家：**旧家是活路径**，新家整体未接线。
      activeHome: "src-tauri/src/network/discovery.rs",
      secondHome: "src-tauri/src/discovery",
      secondHomeStatus: "未接线",
      enforce: false,
      consumes: ["routing", "messaging", "transport"], // 实测:discovery/* 用 mesh(→routing) + protocol(→messaging)；旧家 network/discovery.rs 还用 network::transport(→transport)
      notes:
        "用户可见症状（2026-09-14）：「局域网直连上了，好友在线状态却不实时」——已有非空转护栏（「在线状态必须包含有活跃链路的节点」）。迁移详见 docs/migration-ledger.md 的「局域网发现」条。",
    },

    // -------------------------------------------------------------------------
    //  3. 好友关系
    // -------------------------------------------------------------------------
    {
      id: "friendship",
      name: "好友关系",
      tier: "L3",
      // 目前住在 commands.rs + db.rs 里，**没有独立文件** —— 这正是它最容易跨界污染的原因
      paths: [],
      invariants: ["INV-P12"],
      activeHome: null,
      activeHomeNote: "暂无独立文件：逻辑在 commands.rs 的 friend 命令与 db/friends.rs 的表 CRUD",
      enforce: false,
      consumes: [], // 暂无独立代码；待 Phase 7 提取后再补
      notes:
        "⚠️ **Friend ≠ Peer**：`Peer` 是「发现到了」，`Friend` 是「关系已建立」。混同是 Presence 类 bug 的常见来源。",
    },

    // -------------------------------------------------------------------------
    //  4. 消息（Gosslan 最核心）
    // -------------------------------------------------------------------------
    {
      id: "messaging",
      name: "消息",
      tier: "L3",
      paths: ["src-tauri/src/protocol.rs", "src-tauri/src/gossip_engine.rs"],
      invariants: [
        "INV-001",
        "INV-002",
        "INV-003",
        "INV-004",
        "INV-005",
        "INV-006",
        "INV-P01",
        "INV-P02",
        "INV-P03",
        "INV-P04",
        "INV-P07",
        "INV-P08",
        "INV-P19",
        "INV-P22",
      ],
      activeHome: "src-tauri/src/protocol.rs",
      enforce: false,
      consumes: ["identity"], // gossip_engine.rs 用 crypto::Identity（生产代码）
      notes:
        "直发 + Gossip + 离线补发统一 msg_id 幂等去重；消息先落 outbox，ACK 到达才删。**已知例外**（protocol-invariants §22）：「和自己聊天」不进 outbox、不 gossip、不加密（INV-P03/P04 例外）。消息语义的**实际执行点**仍在 network/transport.rs（约 9600 行，正按 include! 分册中）—— 那是 Phase 7 的拆分目标。",
    },

    // -------------------------------------------------------------------------
    //  5. 路由 / Mesh
    // -------------------------------------------------------------------------
    {
      id: "routing",
      name: "路由与中继",
      tier: "L3",
      paths: [
        "src-tauri/src/mesh",
        "src-tauri/src/file_relay.rs", // 原 relay_manager.rs 重命名 —— `lib.rs:19` 那句「与 mesh::router 无关」注释随之消失,命名撞车处理了一处
      ],
      invariants: ["INV-P20"],
      activeHome: "src-tauri/src/mesh",
      enforce: false,
      consumes: ["messaging"], // mesh/router.rs 生产代码用 gossip_engine::{BloomFilter, LruSet}（测试内引用 transport 的 HEARTBEAT_INTERVAL_SECS 已被 check-domain-deps.mjs 排除，不需写入）
      notes:
        "`mesh/` 是**本仓库最成型的领域模块**（candidate / connection / endpoint / manager / path / peer / relay_policy / router / selection 各自独立文件，有 ADR-0013/0014 背书）。⚠️ relay_manager.rs 属于**文件切片中继**（BitTorrent 式分发），不是路由 —— 命名撞车，lib.rs:19 那句注释就是为它写的。",
    },

    // -------------------------------------------------------------------------
    //  6. 传输 —— 怎么送字节   ⚠️ 三家 + 命名撞车
    // -------------------------------------------------------------------------
    {
      id: "transport",
      name: "传输",
      tier: "L3",
      paths: [
        "src-tauri/src/network/ble.rs", // BLE 中央角色（活）
        "src-tauri/src/network/transport.rs", // TCP 数据面（活，主文件；正按 include! 分册中）
        "src-tauri/src/network/transport/outbound.rs", // 分册：出站投递 + 链路选路（同模块，非新领域）
        "src-tauri/src/network/dispatch.rs", // 三优先级调度 + BLE yield + 发送状态定义（新建，2026-09）
        "src-tauri/src/transport", // 新栈（部分接线）
      ],
      invariants: ["INV-P20", "INV-P22", "INV-P23"],
      // ⚠️ 三个家：TCP 数据面在 network/、BLE 数据面在 network/ble.rs + transport/、控制面在 transport/mod.rs
      activeHome: "src-tauri/src/network/transport.rs", // 活路径的具体文件（network/ 整体是活的家）
      secondHome: "src-tauri/src/transport",
      secondHomeStatus: "部分接线",
      enforce: false,
      consumes: [
        "identity", // network/transport.rs 全文 use crypto
        "persistence", // network/transport.rs 全文 use db（已被台账标记为「db:: 穿透传输层」）
        "files", // network/transport.rs use network::file
        "messaging", // network/transport.rs + ble.rs + tcp.rs 用 protocol
        "routing", // network/transport.rs + ble.rs 用 mesh::*
        "presence", // network/transport.rs 用 discovery::routed
        "platform", // transport/ble_android.rs 用 jni_method::kotlin_method
      ],
      notes:
        "⚠️ **两个同名 transport.rs**：network/transport.rs（约 9600 行，活；已开始 include! 分册）与 transport/{mod,tcp}.rs（新栈）。「传输」这一个关注点今天有**三个家**：TCP 数据面走 network/、BLE 数据面走 transport/bluetooth.rs::driver + 三个外设模块、控制面（开关/状态/分流）走 transport/mod.rs。这是「改完这个 bug 又冒那个」的结构性来源，逐条证据见 docs/migration-ledger.md。BLE 载荷预算已收敛为单一事实来源（INV-P23，Phase 3/4）。",
    },

    // -------------------------------------------------------------------------
    //  7. 文件与内容
    // -------------------------------------------------------------------------
    {
      id: "files",
      name: "文件与内容",
      tier: "L3",
      paths: [
        "src-tauri/src/network/file.rs",
        "src-tauri/src/content", // 内容传输逻辑层
        "src-tauri/src/storage", // 二进制落盘 + 缓存清理
      ],
      invariants: ["INV-P17", "INV-P22"],
      activeHome: "src-tauri/src/network/file.rs",
      enforce: false,
      consumes: [
        "identity", // network/file.rs 用 crypto
        "persistence", // network/file.rs 用 db（与 transport 同病）
        "transport", // network/file.rs 用 network::transport
        "messaging", // network/file.rs 用 protocol::{Message, ShareEntry, FILE_CHUNK}
      ],
      notes:
        "原则（storage/mod.rs）：**SQLite 不存 BLOB**，二进制全部落盘、前端异步懒加载。content/ 是「内容生命周期」逻辑层，network/file.rs 是它的网络侧执行点。",
    },

    // -------------------------------------------------------------------------
    //  8. 持久化
    // -------------------------------------------------------------------------
    {
      id: "persistence",
      name: "持久化",
      tier: "L3",
      paths: [
        "src-tauri/src/db.rs", // 438 行 / Schema + 16 include!
        "src-tauri/src/db", // 16 个子模块：settings / clocks / friends / messages / conversations / offline_queue / file_transfer / favorites(含 tests) / ...
        "src-tauri/src/schema.sql",
        "src-tauri/src/export.rs", // 聊天记录导出（磁盘满 / 迁机时的自救）
      ],
      invariants: ["INV-P05", "INV-P14", "INV-008"],
      activeHome: "src-tauri/src/db", // 16 个子模块（include! 物理拆分）
      enforce: false,
      consumes: [], // 叶子域：db.rs / export.rs / schema.sql 在生产代码内只被装配层 use(state/commands),对其他领域无生产代码引用；tests 内 use protocol 已被 check-domain-deps.mjs 排除
      notes:
        "⚠️ **耦合热点**：`db::` 被 13 个文件引用，横跨 network/、transport/、mesh/、content/、file_relay、notifications、export、commands、state —— 即**传输层直接写库**。架构上最值得收口的一处（Phase 7）。",
    },

    // -------------------------------------------------------------------------
    //  9. 平台外壳
    // -------------------------------------------------------------------------
    {
      id: "platform",
      name: "平台外壳",
      tier: "L2",
      paths: [
        "src-tauri/src/menu.rs", // macOS 专属（模块级 cfg，lib.rs:29）
        "src-tauri/src/tray.rs",
        "src-tauri/src/macos_window.rs", // macOS 专属（lib.rs:40）
        "src-tauri/src/macos_bookmark.rs", // macOS 专属（lib.rs:46）
        "src-tauri/src/open_path.rs",
        "src-tauri/src/android_open.rs", // Android 专属
        "src-tauri/src/jni_method.rs", // Android 专属
        "src-tauri/src/notifications.rs",
        "src-tauri/src/user_dirs.rs",
      ],
      invariants: [],
      activeHome: "src-tauri/src/menu.rs",
      enforce: false,
      consumes: ["persistence"], // user_dirs.rs 用 db；notifications.rs 仅引用 state(装配)；android_open/jni_method 互引(同域)
      notes:
        "按平台分：menu / macos_bookmark / macos_window 是 macOS 专属（模块级 `#[cfg(target_os = \"macos\")]`）；android_open / jni_method 是 Android 专属。这些平台差异正是 Phase 2/4 两条腿存在的原因。",
    },

    // -------------------------------------------------------------------------
    //  10. 可观测性
    // -------------------------------------------------------------------------
    {
      id: "observability",
      name: "可观测性",
      tier: "L2",
      // 只认领后端：它的 UI（LogViewer / DevDiagPanel）物理上住在 presentation 的地盘（src/components）
      paths: ["src-tauri/src/logging.rs"],
      invariants: ["INV-P16"],
      activeHome: "src-tauri/src/logging.rs",
      enforce: false,
      consumes: [], // 叶子域：logging.rs 在 src-tauri/src 内无 use crate::xxx（被 state.rs / commands.rs 引用）
      notes:
        "本项目最强项（结构化日志 + 日志窗口 + 诊断面板 + CHANGELOG 里大量「真机日志定位」）。Phase 8 要补的是**按 msg_id 串起一条消息的生命周期**（Trace）。另有 logs.html 作为独立日志窗口入口。",
    },

    // -------------------------------------------------------------------------
    //  11. 呈现层
    // -------------------------------------------------------------------------
    {
      id: "presentation",
      name: "呈现层",
      tier: "L2",
      paths: [
        "src/App.vue",
        "src/stores",
        "src/components",
        "src/composables",
        "src/api",
        "src/utils",
      ],
      invariants: [],
      activeHome: "src/stores",
      enforce: false,
      consumes: [], // 前端域:本守门不扫 src/*(避跨 parser,且 utils/Api/组件相互 import 模式与后端 crate 不同),见 check-domain-deps.mjs 头部注释
      notes:
        "⚠️ **没有领域边界**：stores/ 只有 2 个文件却是 2346 行（useChatStore.ts 1421 行 / 86 个导出成员），utils/ 92 个文件平铺。前端状态串味最容易产生 UI 回归 —— 拆分优先级高于后端大文件（Phase 7）。前端已有一整套源码扫描护栏（utils/designGuards.ts）。",
    },
  ],
};
