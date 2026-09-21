//! 网络协议层：定义 UDP 发现包与 TCP 帧的线格式，以及与前端交互的公开类型。
//!
//! 设计要点：
//! - 所有消息均为 `{ "type": "...", ... }` 形态的 JSON，便于未来在 QUIC / WebSocket 中继上复用。
//! - TCP 帧 = 4 字节大端长度前缀 + JSON 负载，最大 64MB（足以承载 256KB 文件的 base64 分片）。

use serde::{Deserialize, Serialize};

/// UDP 发现端口（局域网广播）
pub const UDP_PORT: u16 = 59991;
/// TCP 消息/文件传输端口
pub const TCP_PORT: u16 = 59992;
/// 单帧最大字节数（64MB）
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// **预认证阶段**的单帧上限（Hello 帧远小于此：device_id + 公钥 + 签名 ≈ 数百字节）。
///
/// 为什么需要单独一个更小的上限：首帧由**任何**能连上 TCP 端口的主机发送，
/// 而 `read_bytes` 会先 `vec![0u8; len]` 再读——声明 64MiB 只发 4 字节头即可让本机
/// 先分配缓冲，且（在加超时之前）可以无限期挂在那里。预认证阶段收紧到 64KiB，
/// 把这种"未验签就吃内存"的路子堵住；验签之后才按 `MAX_FRAME` 收。
///
/// 这是"我们拒收更大帧"，不改我们发出的字节 ⇒ 无 wire 兼容问题。
pub const MAX_PREAUTH_FRAME: usize = 64 * 1024;

/// 首帧（Hello）等待上限。超时即断开：对端 accept 后一个字节都不发、
/// 或对端断电导致的半开连接，都不会再永久占着任务与 socket。
pub const FIRST_FRAME_TIMEOUT_SECS: u64 = 10;
/// 文件分片原始大小（256KB，base64 后约 342KB）
pub const FILE_CHUNK: usize = 256 * 1024;
/// 广播/发现周期（秒）
pub const ANNOUNCE_INTERVAL_SECS: u64 = 5;
/// 跨跳（无直连）节点离线判定阈值（秒）。
///
/// 跨跳节点没有直连 TCP，`last_seen` 只能靠 Presence（10s 周期）经中继转发刷新。
/// 若沿用 15s，10s 周期只留 5s 余量，Tailscale 等高延迟中继一旦抖动，某次 Presence
/// 迟到超过 15s 就被 `sweep_peers` 误删 → 在线状态「一会儿绿一会儿灰」。
/// 45s ≈ 4.5 个 Presence 周期，给中继延迟留足余量；代价是跨跳节点真正离线后
/// 最多约 45s 才判离线（可接受）。
///
/// 有直连 TCP 的节点不依赖本阈值：`sweep_peers` 用 `active_links` 直接豁免，
/// 且连接断开时由 `mark_peer_offline` 立即移除（无需超时兜底）。
pub const RELAY_PEER_TIMEOUT_SECS: i64 = 45;

/// 当前平台的设备类型标识（"desktop" / "mobile"）。
///
/// 供 Hello / UserInfo / Presence 携带，让对端知道「我是电脑还是手机」。
/// 它是**展示信息**（不参与签名、不绑定身份），旧端缺省时按空串处理。
#[cfg(desktop)]
pub fn current_device_type() -> &'static str {
    "desktop"
}
#[cfg(mobile)]
pub fn current_device_type() -> &'static str {
    "mobile"
}

/// 本机**线格式**版本（ADR-0007 决策 1）。
///
/// 只在**破坏兼容**的协议变更时 +1；加可选字段、加新帧类型都**不** bump
/// （新帧由"对端声明的 `protocol_version` 够高才发"做 capability 门控，见 INV-P24）。
/// 今天所有已发布版本（v4.8.2 起）都是 1。
pub const PROTOCOL_VERSION: u32 = 1;

/// 本机应用版本串（取自 `Cargo.toml`，与 `package.json` 由 `scripts/version.mjs` 同步）。
///
/// ⚠️ **只用于给人看**：诊断面板、日志、"对方版本较新"提示。
/// 绝不用它做兼容判断 —— `4.22.10` 与 `4.22.9` 的线格式完全相同，
/// 而字符串比较会把它们排出高低（这正是本 ADR "不把 app version 当 protocol version" 那条）。
pub fn current_app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 对端**线格式版本是否比本机高**（INV-P24：对端更高必须成为可解释的状态）。
///
/// 这是全仓唯一的兼容判定处，结果直接以 `Friend::peer_version_newer` 发给前端 ——
/// 前端不许自己比数字，否则 `PROTOCOL_VERSION` 就有了第二份真相源。
///
/// `None`（老版本没声明这个字段）判 **false**：没声明 ≠ 版本 0，也 ≠ 版本 1。
/// 把"未知"当成"更高"会在满屏好友上刷出一排"对方版本较新"的错提示；当成"更低"
/// 则会藏掉真实存在的差异。所以老实例就是**什么都不提示**，与"未声明"在诊断里
/// 显示成"未声明"是同一个口径。
pub fn peer_protocol_is_newer(declared: Option<u32>) -> bool {
    declared.is_some_and(|v| v > PROTOCOL_VERSION)
}

/// 内容能力位：支持按 cid 拉取（ContentRequest / 拥有即授权服务）。
pub const CONTENT_FEATURE_PULL: u32 = 1 << 0;

/// 能力位：**能收 `kind:"merge"`（合并转发卡片）**。
///
/// 为什么需要单独一个位而不是"看 protocol_version"：`merge` 是在 V1 期间
/// （`9b26006`，最早出现在 v4.22.30 这条线上）才加进 `MsgKind` 的，而
/// v4.8.2 / v4.18.10 / v4.20.0 这些**已发布**的包里根本没有这个变体。它们的
/// `ChatMessage.kind` 还是嵌套枚举 ⇒ 收到 `kind:"merge"` 时整帧解析失败被丢掉
/// （INV-P24 第 2 条那个 bug 我们只修了自己这一侧，老版本里它仍然成立），
/// 于是发送方 outbox 反复重投、最后显示"发送失败"，双方都不知道为什么。
/// 这段历史也是"V1 内部并不单调"的证据：**别把 protocol_version 当能力清单用**。
pub const CONTENT_FEATURE_MERGE: u32 = 1 << 1;

/// 本机支持的内容能力位图。**不参与 Hello 签名**（见 hello_signing_bytes）：
/// 老端忽略该字段、新端据此决定能不能对它发拉取帧。
pub fn content_features() -> u32 {
    CONTENT_FEATURE_PULL | CONTENT_FEATURE_MERGE
}

/// 「哪个 kind 需要对端具备哪一点能力」的**唯一**答案。`None` = V1 词表内、对所有对端安全。
///
/// 新增 kind 时如果它不是所有已发布版本都认得，**必须**在这里登记 ——
/// `every_gated_kind_is_advertised_by_us` 会盯着"加了门控却忘了声明能力"这个组合。
pub fn kind_required_feature(kind: &str) -> Option<u32> {
    match kind {
        "merge" => Some(CONTENT_FEATURE_MERGE),
        _ => None,
    }
}

/// INV-P24 第 4 条的判据本身：**不门控不许发**。
///
/// `peer_features` 是**对端声明过**的位图；从没交换过 Hello / 节点已离线时，调用方传 `0`
/// —— 也就是"不知道就当不支持"。这个默认方向是刻意的：宁可少发一条新类型消息，
/// 也不要让老对端整帧丢掉、发送方还以为是网络问题。
/// （"离线时先入队、等 Hello 到货再决定"是更好的体验，但它要求 outbox 能改载荷，
/// 那是另一件事，见待办里的 flush 期降级。）
pub fn kind_allowed_by_features(kind: &str, peer_features: u32) -> bool {
    match kind_required_feature(kind) {
        None => true,
        Some(bit) => peer_features & bit != 0,
    }
}

/// 被门控挡下时给用户的那句话（与判据放在一起，免得文案与规则分两处腐烂）。
///
/// 刻意**不报具体版本号**：能力位才是事实来源，版本号只是它恰好对应的现象；
/// 而且"多少版以上"这种话一旦写死就会腐烂。要查对方到底什么版本，
/// 联系人详情页的「对方版本」那一行（v4.22.35）已经有。
pub fn kind_unsupported_hint(kind: &str) -> String {
    format!(
        "对方的 Gosslan 版本较旧，不支持「{kind}」类型的消息，已停止发送。\
         请让对方升级后再发 —— 硬发过去会被对方的程序整条丢掉，\
         最后只会显示「发送失败」，两边都看不出原因。"
    )
}

/// 群里的受众分布：三态分开数，**不要复用** `kind_allowed_by_features`。
///
/// 那个函数为了"宁可少发也不让老对端整帧丢"，刻意把"不知道"并进"不支持"（调用方传 0）。
/// 同样的口径搬到群聊提示上是错的：群成员**离线是常态**，而 `peer_content_features` 只在
/// 内存里、重启即空 ⇒ "不知道"会占满全场，于是每次发合并转发都弹一句"有人可能看不到" ——
/// 一条永远在响的提示等于没有提示，还会顺手把真正要紧的那部分（确知的旧版本成员）淹掉。
///
/// 所以这里：
///   · `unsupported` = 该 kind 需要能力位，且这个成员的位图**确实**没有该位；
///   · `unknown`     = 从没交换过 Hello / 已离线 ⇒ 不并进上面，只在文案里说"版本未知"。
/// 自己不计（自己是发送方）。`gated == false` 时两个数都是 0（这个 kind 对所有版本都安全）。
pub fn kind_audience(
    kind: &str,
    features_of: impl Fn(&str) -> Option<u32>,
    member_ids: &[String],
    self_id: &str,
) -> (Vec<String>, usize, bool) {
    let Some(bit) = kind_required_feature(kind) else {
        return (Vec::new(), 0, false);
    };
    let mut unsupported = Vec::new();
    let mut unknown = 0usize;
    for id in member_ids {
        if id == self_id {
            continue;
        }
        match features_of(id) {
            None => unknown += 1,
            Some(features) if features & bit == 0 => unsupported.push(id.clone()),
            Some(_) => {}
        }
    }
    (unsupported, unknown, true)
}

/// 群受众的**那句话**（与 `kind_audience` 放同一处：文案和它的触发条件是同一件事）。
///
/// 措辞上刻意说清"不会丢消息"：群 kind 不在 `MsgKind` 里，接收端按自由字符串解析
/// （本轮考古实测 v2.1.2 / v4.3.9 / v4.8.2 / v4.20.0 同一份实现），老成员看到的是**渲染退化**
/// 成原始文本，而不是整帧丢掉 —— 这句提示的价值全在"只报渲染、不制造丢消息的恐慌"。
pub fn kind_audience_hint(kind: &str, unsupported_names: &[String], unknown: usize) -> String {
    if unsupported_names.is_empty() {
        return String::new();
    }
    const SHOW_MAX: usize = 5;
    let who = if unsupported_names.len() > SHOW_MAX {
        format!(
            "{} 等 {} 名成员",
            unsupported_names[..SHOW_MAX].join("、"),
            unsupported_names.len()
        )
    } else {
        unsupported_names.join("、")
    };
    let mut hint = format!(
        "{who}的 Gosslan 版本较旧，「{kind}」消息在这些人那里会显示成一段原始文本\
         （消息本身不会丢，其他成员正常）。"
    );
    if unknown > 0 {
        hint.push_str(&format!("另有 {unknown} 位成员当前不在线、版本未知。"));
    }
    hint
}

/// 消息内容类型
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MsgKind {
    Text,
    Code,
    Image,
    File,
    System,
    /// 合并转发的聊天记录（微信式：多条消息合成一张卡片）。
    ///
    /// ⚠️ 必须在这里也列一份（而不是只加 `WIRE_KINDS`）：`MsgKind` 是**发送侧的词表**，
    /// 漏在这里就发不出这个 kind（`as_str` 编译不过）。
    /// 接收侧不再依赖它 —— `ChatMessage.kind` 从 v4.22.34 起是字符串，未知 kind 原样入库
    /// 由前端按 `is_known_kind` 显示占位（INV-P24 第 2 条），所以"漏一个变体"的代价从
    /// "对方看到一坨裸 JSON"降级成"我们发不出这种消息"。
    Merge,
}

impl MsgKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MsgKind::Text => "text",
            MsgKind::Code => "code",
            MsgKind::Image => "image",
            MsgKind::File => "file",
            MsgKind::System => "system",
            MsgKind::Merge => "merge",
        }
    }

    pub fn from_wire_str(s: &str) -> MsgKind {
        match s {
            "code" => MsgKind::Code,
            "image" => MsgKind::Image,
            "file" => MsgKind::File,
            "system" => MsgKind::System,
            "merge" => MsgKind::Merge,
            _ => MsgKind::Text,
        }
    }
}

/// 一种 wire kind 在**接收侧**的语义分类。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KindClass {
    /// 时间线内容：计未读、进会话预览、弹通知、可搜索。渲染成气泡或卡片。
    Bubble,
    /// 静默状态事件（表情回应、撤回、置顶 …）：**不进时间线** ——
    /// 不计未读、不改会话预览、不弹通知。前端按它驱动聚合视图
    /// （回应显示为气泡下方的 chip，而不是时间线上的一条）。
    Silent,
    /// 群级沉淀物（群公告）：**进时间线**（是一条发布事件，该计未读、该通知），
    /// 但**不属于"聊天历史"** —— 清空聊天记录时不得被删、清空边界也不得拦它。
    /// 这两点正是它与 Bubble 的全部差别（见 `delete_conversation` 与
    /// `group_message_blocked_by_boundary`）。
    Card,
}

/// kind 语义的**唯一判定点**。
///
/// 为什么必须收敛到一处：接收路径、会话预览、未读、通知、搜索、已读水位、
/// 清空边界、导出 —— 全都要问「这个 kind 算不算内容」。此前这个知识散在多处、
/// 各写一串 match；每加一个新 kind 就要同时改所有地方，漏一处就是静默的行为不一致
/// （最典型的症状：回个表情把会话顶到列表最前、还弹一条系统通知）。
pub const WIRE_KINDS: &[(&str, KindClass)] = &[
    ("text", KindClass::Bubble),
    ("code", KindClass::Bubble),
    ("image", KindClass::Bubble),
    ("file", KindClass::Bubble),
    ("system", KindClass::Bubble),
    // 阶段 1：表情回应 / 撤回
    ("reaction", KindClass::Silent),
    ("recall", KindClass::Silent),
    // 撤回后的消息本体：仍在时间线上占位（居中灰条「消息已撤回」），故是 Bubble。
    // 它的 content 已被清空 —— 搜索、导出、已读水位因此自动正确，无需各自过滤。
    ("recalled", KindClass::Bubble),
    // 消息置顶：与表情回应同构的静默状态事件（不进时间线，只在置顶条里体现）
    ("pin", KindClass::Silent),
    // 阶段 2：群公告
    ("announcement", KindClass::Card),
    ("announcement_delete", KindClass::Silent),
    // 阶段 3：群任务 / 投票
    //
    // 任务拆成**两个 kind**，但**共用一个载荷结构**（`TodoPayload`），合并规则也相同
    // （LWW per `todo_id`）。拆的理由**只是通知口径**，不是合并语义：
    //   · `todo`（Card）：**创建**任务 —— 这是一次"发布"，该计未读、该弹通知
    //     （被指派的人得知道自己被派了活）。
    //   · `todo_update`（Silent）：改状态 / 改标题 / 换指派人 / 删除 —— 这些是**状态微调**，
    //     与 `pin` / `reaction` 同类。若也算 Card，用户每拖一次状态全群就多一条未读 + 一条通知
    //     （2026-09-16 实现时先按单 kind 做过，发现这条才拆开）。
    ("todo", KindClass::Card),
    ("todo_update", KindClass::Silent),
    ("poll", KindClass::Card),
    ("poll_vote", KindClass::Silent),
    // 合并转发的聊天记录（微信式）：一张卡片，但它是**一条聊天内容** ——
    // 该计未读、该弹通知、该进搜索、清空聊天记录时该被删、也应受清空边界约束，
    // 所以是 Bubble 而不是 Card（Card 是"群级沉淀物"，清空边界不拦它，见 KindClass）。
    ("merge", KindClass::Bubble),
];

/// 未知 kind 一律按 `Bubble` 处理 —— 与 `MsgKind::from_wire_str` 回退到 `Text` 同语义：
/// 宁可多显示一条，也不要把不认识的内容**静默吞掉**（对端版本更新时不丢消息）。
/// 读出口的显示 kind 归一化（**不回写数据**，行里保留原样）。
///
/// 背景（真机 2026-09-19）：4.22.1 之前接收端把消息 kind 写成了文件分类
/// （mp4 ⇒ kind="video"、m4a ⇒ kind="audio"），前端渲染链只认 text/code/image/
/// file/system/merge —— 视频消息整个退化成一段裸 JSON。发送端已收口，历史行靠这层
/// 兼容：video/audio 一律按 file 渲染（文件卡片 + subtype 图标），数据不动。
/// code 有歧义（真代码块消息同为 kind="code"），故不映射 —— 旧「代码文件」卡片
/// 的渲染退化可接受，误伤真代码块不可接受。
pub fn display_kind(kind: &str) -> String {
    match kind {
        "video" | "audio" => "file".into(),
        other => other.into(),
    }
}

pub fn kind_class(kind: &str) -> KindClass {
    WIRE_KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, c)| *c)
        .unwrap_or(KindClass::Bubble)
}

/// 这个 kind **在本机的词表里**吗。
///
/// 与 `kind_class` 的区别就是这条判据存在的理由：`kind_class` 对未知值**回落到 Bubble**
/// （宁可多显示一条），于是"未知"这件事在类型上看不出来。而渲染层必须能区分
/// "知道怎么显示但没专门分支" 与 "压根不认识"（INV-P24 第 2 条）⇒ 判据只能查这张表，
/// **不允许再维护第二份 kind 清单**（清单会漂移，本项目已有多次教训）。
pub fn is_known_kind(kind: &str) -> bool {
    WIRE_KINDS.iter().any(|(k, _)| *k == kind)
}

/// 未知 kind 的预览/占位文案（INV-P24 第 2 条：绝不把载荷原样甩给用户）。
///
/// 前端有一份同样的字符串（`utils/messageKinds.ts`）—— 会话列表与通知说的必须是同一句话，
/// 所以 `messageKinds.test.ts` 会直接读本常量逐项比对，防两侧漂成两个词。
pub const UNSUPPORTED_PREVIEW_LABEL: &str = "[不支持的消息]";

/// 「进时间线，但**不打扰**」—— 不计未读、不改会话预览、不弹通知。
///
/// 与 `Silent` 的区别：静默事件**根本不进时间线**（表情回应/撤回是状态，不是内容），
/// 而 `system` 要在时间线上占一行（居中灰条）。但两者都不该把会话顶起来或弹通知：
/// 此前系统消息只由 `insert_system_message` 在**本机**插入（它不碰未读与预览），
/// 所以"不打扰"是既有事实；现在加人通知要经消息管道广播给全体成员，
/// 必须把这条口径显式化，否则「X 加入了群聊」会给每个人推一条通知。
pub fn is_non_notifying_kind(kind: &str) -> bool {
    is_silent_kind(kind) || kind == "system"
}

pub fn is_silent_kind(kind: &str) -> bool {
    kind_class(kind) == KindClass::Silent
}

/// 生成 `kind NOT IN (...)` 用的 SQL 字面量列表（**从 `WIRE_KINDS` 派生**）。
///
/// 不允许在 SQL 里手写这份清单：加了新 kind 而忘了同步 SQL，就是一条静默漏判 ——
/// 而它偏偏只在下一次有人用那个功能时才暴露。
pub fn sql_kind_list(extra: &[&str], pick: impl Fn(KindClass) -> bool) -> String {
    let mut names: Vec<String> = WIRE_KINDS
        .iter()
        .filter(|(_, c)| pick(*c))
        .map(|(k, _)| format!("'{k}'"))
        .collect();
    names.extend(extra.iter().map(|k| format!("'{k}'")));
    names.join(",")
}

/// 合并转发卡片里的一条内容（`kind = "merge"` 的载荷元素）。
///
/// ## 为什么把每条消息**整份快照**进来，而不是存 msg_id 引用
///
/// 合并转发是**独立内容**：原消息被删、会话被清空之后，卡片展开仍要能看到当时的内容
/// （与收藏同一套理由）。存引用的话，接收方一清空会话，卡片就变成一串"消息不存在"。
///
/// ## 媒体只带元信息
///
/// `kind` 为 `image`/`file` 时，`content` 仍是那条消息原本的元信息 JSON
/// （`name/size/subtype/sha256`），**不复制文件本体** —— 卡片里以 `[图片] 名字` 这样的
/// 占位行展示。真要把媒体也带过去，需要"一条消息携带 N 个附件 + 逐条回源"的内容传输，
/// 是另一个量级；在那之前，**逐条转发**（会真的重发文件）就是它的补充路径。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MergedItem {
    /// 发送者显示名。发送侧拼好（卡片要写"谁说的"，而接收方未必解析得出对方的好友昵称）。
    pub sender: String,
    pub kind: String,
    pub content: String,
    pub ts: i64,
}

/// 合并转发的载荷（微信式"聊天记录"卡片）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MergePayload {
    /// 卡片标题（如「群聊的聊天记录」），由发送侧按会话类型拼。
    pub title: String,
    pub items: Vec<MergedItem>,
}

/// 合并转发的条数上限。取 100 与微信一致 —— 再多既超出卡片的信息承载，也容易撞
/// [`crate::commands`] 的单条消息长度上限（5 万字符）。
pub const MAX_MERGE_ITEMS: usize = 100;

/// 解析并校验合并转发载荷。
///
/// 畸形/超限**一律报错**，不做静默截断（AI_RULES INV-005）：条数被悄悄砍掉，
/// 用户看到的是"我明明选了 12 条，对方只收到 8 条"，而没有任何提示。
pub fn parse_merge_payload(content: &str) -> Result<MergePayload, String> {
    let p: MergePayload =
        serde_json::from_str(content).map_err(|_| "合并转发的载荷不是合法 JSON".to_string())?;
    if p.items.is_empty() {
        return Err("合并转发至少要包含 1 条消息".to_string());
    }
    if p.items.len() > MAX_MERGE_ITEMS {
        return Err(format!(
            "合并转发最多 {MAX_MERGE_ITEMS} 条，当前 {} 条",
            p.items.len()
        ));
    }
    Ok(p)
}

/// 合并转发在**会话预览**里的摘要文案（解析失败也要给一句人话，不能把裸 JSON 顶到列表上）。
pub fn merge_summary(content: &str) -> String {
    match parse_merge_payload(content) {
        Ok(p) => format!("[聊天记录] {} 条", p.items.len()),
        Err(_) => "[聊天记录]".to_string(),
    }
}

// 消息在**会话列表预览**里的文案（`kind` → 人话）。
//
// 收敛到 protocol.rs 的 reason：它要同时被两处用 —— 发送路径（`commands.rs` 写会话摘要）
// 与**删消息后的重算**（`db.rs` 要把末条重建成预览文案）。这两处各写一份 match 时，
// 加一个 kind 只改一处，结果是"删掉末条后列表预览与发送时的口径不一致"。
//
// 文本类截前 30 字符：会话列表只显示一行，超出部分给省略号（**不是**截断内容本身）。
// ---------------- 会话/通知的预览文案（唯一事实源） ----------------
//
// 会话列表摘要与系统通知的正文都来自这里（Rust 侧 `protocol::preview_text` / `transport::preview_content`、
// 前端 `utils/messages.ts` 的 `previewText`）。JSON 载荷的 kind 若不给**人话**，就会把
// 「会话列表/通知显示一段 JSON」暴露给用户（用户 2026-09-17）。
//
// ⚠️ 前端 `previewText` 与本函数必须**逐项一致**（哪些 kind 给什么文案）——
// 由 `src/utils/messageKinds.test.ts` 读 `WIRE_KINDS` 与 `UNSUPPORTED_PREVIEW_LABEL` 机器比对
// （2026-09-20 起，此前只靠注释提醒"两处一起改"，那是迟早会漂的）。

/// 截断到 30 字符（与前端 `previewText` 的 default 分支同口径）。
fn truncate_preview(content: &str) -> String {
    let count = content.chars().count();
    let c: String = content.chars().take(30).collect();
    if count > 30 {
        format!("{c}…")
    } else {
        c
    }
}

/// 从 JSON 载荷里取一个字符串字段（缺失/类型不对 → `None`）。
fn json_str_field(content: &str, field: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()?
        .get(field)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 待办预览：取标题，失败回落「[任务]」。
pub fn todo_preview(content: &str) -> String {
    match json_str_field(content, "title") {
        Some(t) => format!("[任务] {t}"),
        None => "[任务]".to_string(),
    }
}

/// 投票预览：取问题，失败回落「[投票]」。
fn poll_preview(content: &str) -> String {
    match json_str_field(content, "question") {
        Some(q) => format!("[投票] {q}"),
        None => "[投票]".to_string(),
    }
}

/// 公告预览：取正文，失败回落「[公告]」。
fn announcement_preview(content: &str) -> String {
    match json_str_field(content, "text") {
        Some(t) => format!("[公告] {t}"),
        None => "[公告]".to_string(),
    }
}

/// 会话列表摘要 / 通知正文的预览文案。
///
/// 静默类（reaction/recall/pin/poll_vote/announcement_delete/todo_update）照理到不了预览
/// （两侧都按 `is_non_notifying_kind` 过滤），这里仍兜一层——防"某一侧的过滤条件日后变了"
/// 再把 JSON 露出去。
///
/// ⚠️ 默认分支**不再无条件透传正文**（INV-P24 第 2 条）：`text`/`system` 的正文本来就是给
/// 人看的 ⇒ 照旧截断；**表里没有的 kind**（对端版本比本机新，或 4.22.1 之前写坏的形态）
/// 载荷多半是解不开的 JSON ⇒ 原样截断就等于"界面上出现一串 JSON 字符串"（用户明确要求禁止）。
pub fn preview_text(kind: &str, content: &str) -> String {
    // 先过一层显示归一化：库里存着 4.22.1 之前接收端写坏的 `kind="video"/"audio"` 历史行，
    // 不归一化就会被当成"未知种类" ⇒ 一条真实视频消息在列表里显示成「[不支持的消息]」。
    // `display_kind` 与气泡渲染共用同一个函数，两侧口径不会分叉。
    let kind = display_kind(kind);
    match kind.as_str() {
        "file" => "[文件]".to_string(),
        "image" => "[图片]".to_string(),
        "code" => "[代码]".to_string(),
        "merge" => merge_summary(content),
        "todo" | "todo_update" => todo_preview(content),
        "poll" | "poll_vote" => poll_preview(content),
        "announcement" => announcement_preview(content),
        "announcement_delete" => "[公告]".to_string(),
        "reaction" => "[回应]".to_string(),
        "recall" | "recalled" => "[撤回]".to_string(),
        "pin" => "[置顶]".to_string(),
        other if is_known_kind(other) => truncate_preview(content),
        _ => UNSUPPORTED_PREVIEW_LABEL.to_string(),
    }
}

/// 表情回应的事件载荷（`kind = "reaction"`）。
///
/// 建模成**一串独立消息**而不是「给消息加一个可变字段」：`message_id` 是
/// `SHA-256(sender_id + nonce + payload)`，同一条业务消息不可能带不同 content 重发
/// （`gossip_engine` 的回归测试钉死了这一点）。所以「状态变化」只能是一串新事件，
/// 「当前值」由接收端按 `(seq, msg_id)` 折叠出来。
///
/// 收敛性：每个 `(target, actor, emoji)` 三元组是一个 LWW 寄存器、值为 bool。
/// 每个用户只写自己那一格 ⇒ 不存在丢更新；`(seq, msg_id)` 是全序 ⇒ 任意到达顺序收敛。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReactionPayload {
    /// 被回应的消息 msg_id
    pub target: String,
    /// 表情 token（如 `[赞]`），与正文里的表情语法同源
    pub emoji: String,
    /// true = 添加，false = 取消
    pub add: bool,
}

/// 校验一个表情 token 的**形态**（`[名字]`），拒掉空串、超长与控制字符。
///
/// 注意这里**不校验表情是否真的存在** —— 表情目录的唯一来源是前端的
/// `data/emojis.ts`（`EmojiPicker` 与正文渲染都从那里取）。在后端再维护一份名单
/// 就是第二个真相源，加一个表情要改两处、漏一处就出现「能选但发不出去」。
/// 后端只负责挡住畸形与超长输入，语义有效性交给前端。
pub fn is_valid_emoji_token(s: &str) -> bool {
    if !s.starts_with('[') || !s.ends_with(']') {
        return false;
    }
    if s.len() < 3 || s.len() > 32 {
        return false;
    }
    let inner = &s[1..s.len() - 1];
    // 内层不得再出现方括号（否则 `[[x]` 这类畸形会被当成合法 token）
    !inner.is_empty() && !inner.contains(['[', ']']) && !s.contains(char::is_control)
}

/// 群任务（`kind = "todo"`）。
///
/// 任务是**单一寄存器**：一条任务 = 一个 `todo_id` + 它的标题/指派人/状态，
/// 合并规则 **LWW per `todo_id`，版本 `(seq, msg_id)`**。
///
/// 谁写：任意成员创建；**被指派人或创建者**可改状态；**创建者或群主**可改标题/指派人/删除。
/// 授权在命令层判定（`commands::may_update_todo`）—— 载荷里的 `creator` 由服务端从库里
/// 读原值回填，客户端不能自己填，否则"谁有权改"就成了客户端说了算。
///
/// ⚠️ 状态是**任务级**的（一条任务一个状态），不是"每人各自一格"。用户 2026-09-16 定了四态
/// 且**手动选**（不做截止时间）：代价是并发改状态时按 LWW 收敛，后写者胜 ——
/// 对"一条任务当前处于什么阶段"这种单值语义，LWW 就是期望行为（看板类工具都这样）。
/// 待办描述里附带的图片（**仅元数据**，真实字节走群文件管线投递，见 `commands.rs`）。
///
/// 与消息图片同源：用 `sha256`（= cid）作为跨端去重与本地落盘的文件名，
/// 接收方按 `sha256` 在本地 `todo_images/` 目录解析；缺失则触发补取。
/// 所有字段 `#[serde(default)]` ⇒ 旧版只发 `todo_id/title/...` 的载荷也能解析（向后兼容）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TodoImage {
    /// 跨端唯一 id（= 文件 sha256 / cid），落盘文件名与去重键
    #[serde(default)]
    pub id: String,
    /// 原始文件名（展示用）
    #[serde(default)]
    pub name: String,
    /// 字节大小
    #[serde(default)]
    pub size: u64,
    /// 文件 sha256（与 `id` 同值，落盘/校验用）
    #[serde(default)]
    pub sha256: String,
    /// 子类型（image / 其它），决定聊天卡片里是缩略图还是文件块
    #[serde(default)]
    pub subtype: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TodoPayload {
    pub todo_id: String,
    pub title: String,
    /// 指派的成员 device_id（**至少一人**：用户明确「可以给一个或多个人」，
    /// 由命令层校验"非空且都是群成员"）
    #[serde(default)]
    pub assignees: Vec<String>,
    /// 任务状态，取值见 [`TODO_STATUSES`]。缺省 = 待办 ⇒ 历史/异常载荷也能解析成一条合法任务。
    #[serde(default = "default_todo_status")]
    pub status: String,
    /// 创建者（改/删的授权判据；由命令层回填，不接受客户端自报）
    #[serde(default)]
    pub creator: String,
    /// 删除标记（墓碑）：定义层的 LWW 值为它
    #[serde(default)]
    pub deleted: bool,
    /// 长文本描述（用户 2026-09-17 优化：待办要能写详细说明）。缺省空串 ⇒ 旧载荷兼容。
    #[serde(default)]
    pub description: String,
    /// 描述里附带的图片（仅元数据；真实字节走群文件管线）。缺省空 ⇒ 旧载荷兼容。
    #[serde(default)]
    pub images: Vec<TodoImage>,
    /// 是否已归档（用户 2026-09-17：完成/过期/不用的任务可归档）。缺省 false。
    #[serde(default)]
    pub archived: bool,
    /// 状态变为「完成」的**权威**时间戳（ms）。由 `send_group_payload` 在 `status=="done"`
    /// 时填 `db::now_ms()`，不接受客户端自报 ⇒ 7 天自动归档的计时起点可信。
    /// `None` = 从未完成过（或旧载荷）。
    #[serde(default)]
    pub done_at: Option<i64>,
}

/// 任务状态的**唯一取值表**（用户 2026-09-16 定的四态）。
///
/// 与前端 `src/utils/todos.ts` 的 `TODO_STATUSES` **必须一致**，
/// 由 `src/utils/messageKinds.test.ts` 读本文件逐项比对（与 `WIRE_KINDS` 同一套跨语言契约）。
///
/// ⚠️ 状态是**手动选**的：没有截止时间、也没有"过了日期自动变延期"这回事 ——
/// 「延期」就是人手动标出来的一个状态。
pub const TODO_STATUSES: [&str; 4] = ["todo", "doing", "overdue", "done"];

/// 缺省状态（新建的任务 = 待办）。也是 `status` 字段缺失时的解析回落值。
pub fn default_todo_status() -> String {
    "todo".to_string()
}

/// 状态取值是否合法（命令层校验用；未知值一律拒收，避免脏状态流进群里）。
pub fn todo_status_is_valid(s: &str) -> bool {
    TODO_STATUSES.contains(&s)
}

/// 投票的**定义**层（`kind = "poll"`）。结构与任务同构。
///
/// ⚠️ **`options` 一旦创建不可变**：否则选项下标会错位，`poll_vote` 的 choices
/// 会指向错误的选项。要改选项就新建一个投票。
///
/// **不做匿名投票**：群密钥全员共享 + 选票必然带签名身份，"匿名"只能是界面隐藏，
/// 无权可验 —— 那比不做更糟（给人虚假的安全感）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PollPayload {
    pub poll_id: String,
    pub question: String,
    pub options: Vec<String>,
    /// 是否多选
    #[serde(default)]
    pub multi: bool,
    /// 是否已关闭（关闭后拒绝新票）
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub creator: String,
}

/// 投票的**选票**层（`kind = "poll_vote"`）：每人只写自己那一格。
/// **撤票 = `choices: []` 的普通更新**，不是单独的删除事件。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PollVotePayload {
    pub poll_id: String,
    /// 选中的选项下标（空 = 撤票）
    pub choices: Vec<u32>,
}

/// 群公告的载荷（`kind = "announcement"`）。
///
/// 「当前公告」= 按 `(seq, msg_id)` 取最大的那条（**不是**墙上时间）——
/// 只有群主能发，而群主的 Lamport 时钟单调，自己两条公告不可能同 seq，
/// tie-break 只是防御。`ann_id` 取发布事件自身的 msg_id，不需要额外的生成器。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AnnouncementPayload {
    pub text: String,
}

/// 公告删除（`kind = "announcement_delete"`）：墓碑，携带被删公告的 ann_id。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AnnouncementDeletePayload {
    pub ann_id: String,
}

/// 消息置顶的事件载荷（`kind = "pin"`）。
///
/// 与撤回/回应同构：一串独立事件，每个 `target` 是一个按 `(seq, msg_id)` 定序的
/// LWW 寄存器（值 = 是否置顶）。任意成员都能置顶/取消（可逆、低风险），
/// 与「仅群主可改名」那类不可逆操作不同。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PinPayload {
    /// 被置顶的消息 msg_id
    pub target: String,
    /// true = 置顶，false = 取消置顶
    pub pinned: bool,
}

/// 撤回的事件载荷（`kind = "recall"`）。
///
/// 与表情回应同构：撤回也是**一串独立事件**而非"改一个字段"，
/// 因为 `message_id` 绑定了 payload，同一条消息不可能带不同 content 重发。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RecallPayload {
    /// 被撤回的消息 msg_id
    pub target: String,
}

/// 撤回后消息本体的 `kind`。`content` 被清空、`sender_id`/`ts`/`seq`/`msg_id` 保留 ——
/// 这样搜索、导出、已读水位**一行都不用改就自动正确**（没有正文可命中、可导出）。
pub const KIND_RECALLED: &str = "recalled";

/// 群公告发布 / 删除的 `kind`（接收侧授权判定要用）。
pub const KIND_ANNOUNCEMENT: &str = "announcement";
pub const KIND_ANNOUNCEMENT_DELETE: &str = "announcement_delete";

/// 撤回事件本身的 `kind`。
pub const KIND_RECALL: &str = "recall";

/// 共享目录条目
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ShareEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Gossip 消息类型
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GossipKind {
    /// 单聊（点对点 E2EE，仅接收方可解密）
    Chat,
    /// 群聊（群密钥对称加密）
    Group,
    /// 好友关系拦截通知（明文 JSON payload，携带 original_sender）
    FriendMessageBlocked,
    /// 节点通告：周期广播自身身份，跨跳传播让全网节点互相可见（TOFU 语义）。
    /// 明文（encrypted=false），payload 为 JSON（昵称 / 头像）。
    Presence,
    /// 好友申请（定向跨跳）：payload 为 E2EE 密文（用目标 X25519 公钥加密），
    /// `target` 指定接收方 device_id；中间节点按 target 定向转发（一跳精确，
    /// 无路由表时洪泛兜底）。
    FriendRequest,
    /// 好友申请同意（定向跨跳）：方向与 FriendRequest 相反，其余同理。
    FriendAccept,
    /// 单聊送达确认（定向跨跳）：接收方成功持久化某条单聊 Gossip 后回给原始发送方。
    /// 明文（encrypted=false），payload 为 JSON `{"msg_id":"..."}`，`target` = 原始发送方。
    /// 与直连 `Message::Ack` 语义一致，但可跨跳（跨 Tailscale 无直连时 Ack 到不了发送方）。
    ChatAck,
    /// 单聊已读回执（定向跨跳）：接收方读到某发送方消息后回执。明文，
    /// payload 为 JSON `{"last_read_ts":n,"last_read_msg_id":"..."}`，`target` = 原始发送方。
    /// 与直连 `Message::ReadReceipt` 语义一致，但可跨跳。
    ChatReadReceipt,
}

/// Gossip 广播信封（Epidemic 协议消息体）。
/// - `message_id`：SHA-256 十六进制（去重键）
/// - `sender_pubkey` / `sender_ed25519`：发送方 X25519 / Ed25519 公钥
/// - `sender_sig`：对信封不可变字段的 Ed25519 签名（身份校验；TTL 不签名，因为转发会递减）
/// - `ttl`：生存时间，每转发一次减一，归零丢弃
/// - `payload`：base64（用户聊天 `encrypted=true` 时为 `nonce || ChaCha20-Poly1305 密文`）
/// - `encrypted`：载荷是否加密；用户聊天必须为 true，内部拒绝通知可使用明文控制载荷
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GossipEnvelope {
    pub message_id: String,
    pub sender_id: String,
    /// 每条消息的随机 nonce：message_id = SHA-256(sender_id + nonce + payload)。
    /// 不再用本地时间戳参与消息身份，避免同毫秒碰撞，也避免业务身份依赖系统时间。
    #[serde(default)]
    pub nonce: String,
    pub sender_pubkey: String,
    pub sender_ed25519: String,
    pub sender_sig: String,
    pub ttl: u8,
    pub kind: GossipKind,
    pub group_id: Option<String>,
    /// 群名快照：随消息广播，接收方本地无群记录时可直接展示正确群名
    /// （不参与 `compute_message_id` 哈希，不影响跨路径去重）。
    #[serde(default)]
    pub group_name: Option<String>,
    /// 群创建者 ID + 当前成员列表：随群消息广播，使只收到群消息、
    /// 从未收到 GroupKey 的成员也能据此在本地建立/刷新群记录（含成员）。
    /// 与 `group_name` 同理，不参与 message_id 哈希。
    #[serde(default)]
    pub group_creator: Option<String>,
    #[serde(default)]
    pub group_members: Vec<String>,
    pub payload: String,
    pub ts: i64,
    /// 会话逻辑序号（Lamport），签名覆盖；接收方按此排序。
    #[serde(default)]
    pub seq: i64,
    pub encrypted: bool,
    /// 定向目标 device_id（仅 `FriendRequest` 使用；`None` = 广播）。
    /// 参与签名，中间节点不可篡改目标；不参与 message_id（nonce 已保证唯一）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl GossipEnvelope {
    /// 计算并填充 message_id（SHA-256 of sender_id + nonce + payload）。
    /// 用随机 nonce 而非时间戳：消息身份不依赖本地时钟，也不存在同毫秒碰撞。
    pub fn compute_message_id(&mut self) {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.sender_id.as_bytes());
        h.update(self.nonce.as_bytes());
        h.update(self.payload.as_bytes());
        self.message_id = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    }

    /// 生成签名材料。TTL 是唯一允许中继节点修改的字段；其余路由、身份、
    /// 群成员和载荷字段都必须被签名，避免“签名仍有效但把 Chat 改成 Group”之类的
    /// 元数据篡改。
    pub fn signing_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            &self.message_id,
            &self.sender_id,
            &self.nonce,
            &self.sender_pubkey,
            &self.sender_ed25519,
            &self.kind,
            &self.group_id,
            &self.group_name,
            &self.group_creator,
            &self.group_members,
            &self.payload,
            &self.ts,
            &self.seq,
            &self.encrypted,
            &self.target,
        ))
        .unwrap_or_default()
    }
}

/// TCP 帧消息（P2P 节点间传输）
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// 连接建立后首先发送的握手包。
    ///
    /// `nonce` + `sig` 是**连接身份认证**：只有持有 `device_id` 绑定私钥的一方
    /// 能对 `hello_signing_bytes()` 产出合法签名。接收方在建立链路前用它确认
    /// 「这个 TCP 对端确实是 device_id 本人」，杜绝任意节点冒用他人（好友/群主）
    /// device_id 建链后伪造明文控制消息（GroupMemberRemoved / GroupRename 等）。
    Hello {
        device_id: String,
        nickname: String,
        avatar: Option<String>,
        /// 设备类型（"desktop" / "mobile"，空串 = 旧端/未知）。展示信息，不参与签名。
        #[serde(default)]
        device_type: String,
        /// 内容能力位图（见 CONTENT_FEATURE_PULL）。**不参与签名**：老端忽略、新端可读。
        #[serde(default)]
        content_features: u32,
        /// 对端**线格式**版本（`PROTOCOL_VERSION`，ADR-0007 决策 1）。
        ///
        /// `None` = 老端没发这个字段 ⇒ 按最低版本处理（能力门控一律当"不支持新帧"）。
        /// **不参与签名**，与 `content_features` 同理：加字段不能弄坏老端验签，
        /// 否则"报版本"这件事本身就成了一次破坏性变更。
        #[serde(default)]
        protocol_version: Option<u32>,
        /// 对端应用版本串（如 `"4.22.27"`）。**只给人看**（诊断面板 / 日志 /
        /// "对方版本较新"提示），绝不用它做兼容判断 —— 见 `current_app_version`。
        /// 同样不参与签名、老端缺省为 `None`。
        #[serde(default)]
        app_version: Option<String>,
        tcp_port: u16,
        x25519_pubkey: String,
        ed25519_pubkey: String,
        /// 与对方单聊会话的本地逻辑时钟：用于建链时快速对齐，
        /// 避免双方时钟长期不同步导致新消息序号偏小。
        #[serde(default)]
        conv_clock: i64,
        /// 每次握手新生成的随机串（base64），参与签名并供接收方防重放去重。
        #[serde(default)]
        nonce: String,
        /// Ed25519 签名（base64），覆盖 `hello_signing_bytes()` 的全部字段。
        #[serde(default)]
        sig: String,
    },
    /// 心跳
    Heartbeat {
        device_id: String,
    },
    /// 用户资料变更同步（昵称/头像）
    UserInfo {
        device_id: String,
        nickname: String,
        avatar: Option<String>,
        /// 设备类型（"desktop" / "mobile"，空串 = 旧端/未知）。
        #[serde(default)]
        device_type: String,
    },
    /// 聊天样式同步：发送方广播自己的气泡/字体偏好，接收方持久化并按其偏好渲染该发送者的消息
    ChatStyle {
        from: String,
        /// 目标节点（None = 广播给所有已连接节点）
        to: Option<String>,
        /// 样式 JSON，如 {"preset":"classic","fontSize":"md","compact":true}
        style: String,
    },
    /// **按 cid 拉取内容**（ADR-0019 Phase 3）。
    ///
    /// 收到方若持有该 cid 的完整字节（content_transfers: status=complete + path），
    /// 直接回一份 FileOffer（复用既有 Chunk/Done/CompleteAck 流程）——
    /// **拥有即授权，无需人工确认**。只应发给 Hello 里声明了 CONTENT_FEATURE_PULL 的对端。
    ContentRequest {
        from: String,
        /// 明文 SHA-256（hex），与 FileOffer.file_sha256 同一口径。
        cid: String,
        name: String,
        size: u64,
        /// 断点续传：原 transfer_id（服务端要用它回发，接收端才找得到 <tid>.part）。
        #[serde(default)]
        transfer_id: String,
        /// 断点续传：接收端期望的下一片序号。
        #[serde(default)]
        from_seq: u32,
        /// 断点续传：接收端已持有的前缀字节数。
        #[serde(default)]
        from_bytes: u64,
    },
    /// 加好友申请
    FriendRequest {
        from: String,
        from_nickname: String,
        from_avatar: Option<String>,
        to: String,
        ts: i64,
    },
    FriendAccept {
        from: String,
        to: String,
    },
    FriendReject {
        from: String,
        to: String,
    },
    FriendRemove {
        from: String,
        to: String,
    },
    FriendMessageBlocked {
        from: String,
        to: String,
        original_sender: String,
    },
    /// 单聊消息
    ChatMessage {
        msg_id: String,
        from: String,
        to: String,
        /// **线格式是字符串，不是枚举**（INV-P24 第 2 条）。
        ///
        /// 曾经这里是 `MsgKind`：对端 Gosslan 比本机新、发来一个本机不认识的 kind 时，
        /// serde 会报 `unknown variant` —— 而 `decode_frame` 无法区分"未知**帧**类型"与
        /// "未知**嵌套枚举值**"，于是整条 `chat_message` 被降级成 `Message::Unknown` 丢弃。
        /// 后果不是"少显示一个占位"，而是**消息根本进不了库**：接收方什么都不知道，
        /// 发送方拿不到 Ack，最后显示「发送失败」。
        ///
        /// 改成字符串后未知 kind 原样入库，前端按 `isKnownKind` 显示可解释的占位
        /// （见 `UnsupportedKindBubble`）。发送侧仍然只产出 `MsgKind` 的词表
        /// （`commands::send_message` 会先归一化），所以"本机不会发出乱码 kind"不变。
        kind: String,
        content: String,
        ts: i64,
        /// 会话逻辑序号（Lamport），接收方按此排序，而非发送方墙上时钟。
        #[serde(default)]
        seq: i64,
    },
    /// 送达确认（用于离线补发去重）
    Ack {
        msg_id: String,
    },
    /// 已读回执：接收方打开会话时告知发送方「读到 last_read_ts 为止的消息都看了」。
    /// `last_read_msg_id` 指向接收方最近读到的一条**发送方消息**，发送方用它
    /// 换算回自己的本地时间戳，避免设备间时钟偏差导致回执失效。
    ReadReceipt {
        from: String,
        to: String,
        last_read_ts: i64,
        #[serde(default)]
        last_read_msg_id: Option<String>,
    },
    /// 群聊成员级已读回执：接收方读到群消息的时间点。
    /// `last_read_msg_id` 与单聊回执同理，指向该成员最近读到的一条群消息。
    GroupReadReceipt {
        from: String,
        group_id: String,
        last_read_ts: i64,
        #[serde(default)]
        last_read_msg_id: Option<String>,
    },
    /// 群消息送达确认：接收方成功持久化某条群消息后回给原始发送者，
    /// 发送方据此删除对应 `group_outbox(msg_id, peer_id)` 行。
    GroupAck {
        group_id: String,
        msg_id: String,
        from: String,
    },
    // ---- 文件传输 ----
    /// 发起文件传输。`sealed_file_key`：发送方为本 transfer 生成的随机
    /// 32B 文件会话密钥，用接收方 X25519 公钥 ECDH + AEAD 封装——
    /// 只有接收方能解封；后续 FileChunk.data 均用该密钥加密。
    /// `file_sha256`：整个原文件的 SHA-256（64 位小写 hex），仅用于
    /// 文件级完整性验证；分片级防篡改由 AEAD 承担。
    FileOffer {
        transfer_id: String,
        from: String,
        name: String,
        size: u64,
        sealed_file_key: String,
        file_sha256: String,
        /// 断点续传：从第几片开始发（缺省 0 = 整份）。
        #[serde(default)]
        from_seq: u32,
        /// 断点续传：从第几字节开始发（接收端已持有的前缀字节数）。
        #[serde(default)]
        from_bytes: u64,
    },
    FileAccept {
        transfer_id: String,
    },
    FileReject {
        transfer_id: String,
        /// 断点续传：接收端**已持有的字节数**（0 = 没有前缀）。发送端据此偏移续发。
        #[serde(default)]
        received: u64,
    },
    /// `data`：文件会话密钥 AEAD 加密后的 base64（nonce || ciphertext），
    /// 密文在 TCP / 中继上均不透明。
    FileChunk {
        transfer_id: String,
        seq: u32,
        data: String,
    },
    FileDone {
        transfer_id: String,
    },
    /// 接收方对文件传输的最终确认（成功持久化并校验完成后才允许回 success=true）。
    /// 发送方只有收到 success=true 才能把本地文件消息推进到 delivered。
    FileCompleteAck {
        transfer_id: String,
        success: bool,
    },
    // ---- 共享目录 ----
    ShareTreeRequest {
        request_id: String,
        from: String,
        to: String,
    },
    ShareTreeResponse {
        request_id: String,
        from: String,
        /// 目标节点（None = 旧端直连回复）。有它才能在无直连时借中继一站送回。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        entries: Vec<ShareEntry>,
    },
    /// 请求对方共享目录中的文件（触发对方向我方发起文件传输）
    ShareFileRequest {
        transfer_id: String,
        from: String,
        path: String,
        /// 目标节点（None = 旧端直连发送）。见 ShareTreeResponse.to。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
    },
    /// Gossip 广播信封（去中心化消息分发）
    Gossip {
        envelope: GossipEnvelope,
    },
    /// 大文件切片中继转发（BitTorrent 式 Mesh 分发）
    RelayChunk {
        transfer_id: String,
        seq: u32,
        data: String,
        from: String,
        to: String,
        ttl: u8,
    },
    /// 群密钥分发（用成员公钥 ECDH 加密的群密钥）。
    /// 同时携带群名与成员列表：成员端据此在本地建群记录，
    /// 否则收到首条群消息时只能兜底成「群聊 g-xxxx」。
    GroupKey {
        group_id: String,
        from: String,
        to: String,
        key: String,
        #[serde(default)]
        group_name: String,
        #[serde(default)]
        members: Vec<String>,
        /// 该群的当前逻辑时钟：成员上线拿到密钥时同步本地时钟，
        /// 保证其后续新消息序号大于清空边界等本地水位。
        #[serde(default)]
        clock: i64,
    },
    /// 群文件发起（不含文件内容）。`sealed_file_key`：发送方为本 transfer
    /// 生成的随机 32B 文件会话密钥，用**群密钥** AEAD 封装（seal_symmetric）——
    /// 群内成员用本地 GroupKey 解封，群外与中继无法解开。
    /// 后续群文件分片均以该 file_key 加密（下一阶段实现）。
    GroupFileOffer {
        transfer_id: String,
        group_id: String,
        sender_id: String,
        name: String,
        size: u64,
        sha256: String,
        sealed_file_key: String,
        /// 归属场景：`chat` = 普通群文件（进聊天时间线）；`todo` = 待办描述图片
        /// （只走传输管线把字节投递给全员，不进时间线、不弹气泡）。
        /// ⚠️ `default`：**未升级端发出的 offer 没有这两个键** —— 缺了会反序列化失败
        /// （升级后的端拒收旧端的群文件）。缺省 "" 走普通文件分支，与旧端语义一致。
        #[serde(default)]
        scope: String,
        /// `scope == "todo"` 时关联的 `todo_id`；其余场景为空。
        #[serde(default)]
        todo_id: String,
    },
    /// 群文件分片。`data` = Base64(nonce || AEAD(file_key, plaintext))，
    /// file_key 仅存在于收发双方内存（AppState.group_file_keys），
    /// 群密钥只负责封装 file_key，绝不直接加密文件内容。
    GroupFileChunk {
        transfer_id: String,
        group_id: String,
        sender_id: String,
        seq: u32,
        data: String,
    },
    /// 群文件发送完毕（发送方全部分片已发出）。
    /// 接收端据此做最终校验（size + SHA-256）并落盘正式文件；
    /// 接收完成与否以接收端本地校验结果为准，本消息不是完成确认。
    GroupFileDone {
        transfer_id: String,
        group_id: String,
        sender_id: String,
    },
    /// 群文件接收完成确认（receiver → 原始 sender）。
    /// `sender_id` = ACK 发送者（即原 recipient），发送方必须校验
    /// sender_id == TCP peer_id，且 transfer 的 group_file.sender_id 是本机。
    /// success = 本地 size/SHA-256 校验通过并已 rename 落盘。
    GroupFileCompleteAck {
        transfer_id: String,
        group_id: String,
        sender_id: String,
        success: bool,
    },
    /// 群名变更广播（创建者改名后通知各成员同步本地群名）
    GroupRename {
        group_id: String,
        from: String,
        name: String,
    },
    /// 成员被移出群：仅群创建者发起，发给被移除的成员本人。
    /// 接收方删除本地群记录与会话，并撤销群密钥。
    GroupMemberRemoved {
        group_id: String,
        from: String,
        to: String,
    },
    /// 群主转让：仅**当前**创建者可发起。接收方校验 `from` 是本地记录的创建者、
    /// `to` 是群成员后，把本地群创建者改为 `to`。用于群主更换设备/卸载前移交
    /// 管理权，避免群永久失去改名/加人/踢人能力。
    GroupCreatorChanged {
        group_id: String,
        from: String,
        to: String,
    },
    /// 成员主动退群（非群主）。接收方把 `from` 从本地群成员中移除。
    /// 群主退出前必须先转让（由 `leave_group` 命令强制）。
    GroupMemberLeft {
        group_id: String,
        from: String,
    },
    /// 中继文件传输元数据（切片总数等，先于 RelayChunk）。
    /// `sealed_file_key`：与 FileOffer 同义——用接收方公钥封装的文件会话密钥，
    /// 中继节点不持有也不解封，仅接收方能解开。
    /// `file_sha256`：原文件 SHA-256（hex），中继不解密不校验，仅透传给接收方。
    RelayFileOffer {
        transfer_id: String,
        from: String,
        to: String,
        name: String,
        size: u64,
        total_chunks: u32,
        sealed_file_key: String,
        file_sha256: String,
    },
    // ---- Phase 8（ADR-0017）：外部 mesh（BitChat）的不透明帧 ----
    /// 外部 mesh 的包：Gosslan **只当中继** —— 收得到 / 去得掉重 / TTL 递减后转发，
    /// 不解密、不落库、不建用户/channel。`payload` 是原样字节的 base64。
    ///
    /// ⚠️ 决策更新（ADR-0017，用户裁定）：本版**不考虑旧版兼容**，
    /// 所以不需要能力门控/双读窗口；但保留健壮性底线 —— 畸形/超限帧**只丢这一帧、不断链**
    /// （见 [`validate_opaque_external`]）。
    OpaqueExternal {
        /// 外部帧的自有 id（仅用于去重；不进 Gosslan 的 message_id 体系）
        id: String,
        /// 剩余跳数（路由器会按自己的上限再裁剪一次）
        ttl: u8,
        /// 原样载荷（base64）
        payload: String,
    },
    /// **跨版本兜底**（INV-P24）：本机不认识的 `type` 解析成这个变体，
    /// 于是"收到未来版本的新帧"= 忽略这一条 + 记一条节流日志，
    /// 而不是反序列化失败 ⇒ 连接错误 ⇒ **拆链**（旧行为，会让老设备跟新设备连不上）。
    ///
    /// 只由 `read_frame` 产生，**发送侧永远不会构造它**（所以不需要为它定义线格式语义）。
    /// 已知 `type` 但字段畸形**不算**这里 —— 那是我们自己的 bug，必须继续报错，
    /// 否则就是把真实的协议错误静默吞掉（违反 INV-005 的精神）。
    Unknown {
        /// 线格式里那个我们不认识的 `type` 值（仅用于日志与诊断面板）
        wire_type: String,
    },
}

impl Message {
    /// 诊断用：这条消息的**线格式类型名**（= `#[serde(tag = "type")]` 里的那个值）。
    ///
    /// 为什么从序列化结果反读、而不是手写一遍 `match`：`Message` 有 36 个变体，
    /// 手写映射就是给协议加了**第二份事实来源** —— 将来新增变体时忘了同步，
    /// 日志里会出现**错误的类型名**，比没有日志更坏（真机排查会被带偏）。
    /// 这里永远与 serde 一致，代价是序列化一次；**只在错误/诊断路径**调用。
    pub fn wire_kind(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
            .unwrap_or_else(|| "未知类型".to_string())
    }
}

/// 单个不透明外部帧的载荷上限（解码后字节）。取 256 KiB：足够装下 BitChat 的典型包
/// （其 MTU 是几十~几百字节），又远小于 `MAX_FRAME`，不会成为内存放大入口。
pub const MAX_OPAQUE_PAYLOAD: usize = 256 * 1024;
/// 外部帧允许声明的最大 TTL（路由器另有自己的 `max_ttl` 再裁剪一层）。
pub const MAX_OPAQUE_TTL: u8 = 16;
/// 外部帧 id 的长度上限。
pub const MAX_OPAQUE_ID: usize = 128;

/// 校验不透明外部帧（**纯函数，主机可单测**）。
///
/// 返回解码后的原样字节；任何不合规都返回 `Err(原因)`，调用方**只丢这一帧**并记日志
/// （这是 ADR-0017 决策更新里保留的那条底线：健壮性，不是兼容性）。
pub fn validate_opaque_external(id: &str, ttl: u8, payload_b64: &str) -> Result<Vec<u8>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    if id.is_empty() || id.len() > MAX_OPAQUE_ID {
        return Err(format!("id 长度非法（{}）", id.len()));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return Err("id 含非法字符".to_string());
    }
    if ttl == 0 || ttl > MAX_OPAQUE_TTL {
        return Err(format!("ttl 非法（{ttl}）"));
    }
    let bytes = STANDARD
        .decode(payload_b64)
        .map_err(|e| format!("payload 不是合法 base64：{e}"))?;
    if bytes.is_empty() {
        return Err("payload 为空".to_string());
    }
    if bytes.len() > MAX_OPAQUE_PAYLOAD {
        return Err(format!("payload 过大（{} 字节）", bytes.len()));
    }
    Ok(bytes)
}

/// Hello 帧的签名材料（版本前缀 + 全部连接身份字段）。
///
/// 用 `serde_json` 序列化元组而非手写字符串拼接：避免字段里出现分隔符时产生
/// 「不同字段组合出同一段字节」的歧义（长度前缀/分隔符逃逸问题）。
/// 接收方以 `device_id` 绑定的 Ed25519 公钥验签，从而确认 peer_id 不可冒充。
pub fn hello_signing_bytes(
    device_id: &str,
    tcp_port: u16,
    nonce: &str,
    x25519_pubkey: &str,
    ed25519_pubkey: &str,
) -> Vec<u8> {
    serde_json::to_vec(&(
        "gosslan-hello-v1",
        device_id,
        tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
    ))
    .unwrap_or_default()
}

/// announce 的签名材料。
///
/// **只覆盖安全相关字段**：device_id / tcp_port / 两把公钥 / nonce。
/// 刻意**不含 nickname**：它随用户改名变化、且纯属展示信息，纳入签名会让
/// 「改个昵称 → 旧签名全部失效」；也不含 `kind`，由调用方保证是 announce。
pub fn announce_signing_bytes(
    device_id: &str,
    tcp_port: u16,
    nonce: &str,
    x25519_pubkey: &str,
    ed25519_pubkey: &str,
) -> Vec<u8> {
    serde_json::to_vec(&(
        "gosslan-announce-v1",
        device_id,
        tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
    ))
    .unwrap_or_default()
}

/// UDP 广播/回复包
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UdpPacket {
    /// "announce"（主动广播自身） | "who_has"（询问局域网内谁在线）
    pub kind: String,
    pub device_id: String,
    pub nickname: String,
    pub tcp_port: u16,
    /// X25519 公钥（base64，用于 ECDH）
    pub x25519_pubkey: Option<String>,
    /// Ed25519 公钥（base64，用于验签）
    pub ed25519_pubkey: Option<String>,
    /// 每次广播新生成的随机串（base64），参与签名 —— 防重放。
    /// 旧端不发送（`serde(default)`），判定见 `verify_announce`。
    #[serde(default)]
    pub nonce: String,
    /// Ed25519 签名（base64），覆盖 `announce_signing_bytes()` 的全部字段。
    ///
    /// 历史背景：本字段缺失时，`announce` 是一条**完全无认证**的信道 ——
    /// 包里的 device_id 与公钥都是明文，任何人都能伪造。曾经的后果是
    /// 「未签名的广播绑定了身份，且写进持久化的 friends 表」（见 CHANGELOG 4.9.1）。
    /// 现在补上签名后，**能**做到的：防篡改、防重放、让每条广播可归因到某个密钥持有者。
    /// **仍做不到**的：阻止攻击者用自己的密钥签一个「自称是某人」的包 ——
    /// 那是首次接触（TOFU）的固有限制，要靠带外指纹核对（见 `Peer.keys_verified` 的说明）。
    #[serde(default)]
    pub sig: String,
}

/// `announce` 的认证判定结果。
#[derive(Debug, PartialEq, Eq)]
pub enum AnnounceAuth {
    /// 签名有效：广播者持有其所声明 Ed25519 公钥的私钥。
    /// **注意这仍不等于「他就是那个 device_id」** —— 见 `UdpPacket::sig` 的说明。
    Verified,
    /// 无签名（旧端）。**仍然接受用于发现**：它只驱动「拨号」，
    /// 而真正的身份绑定由 Hello 验签决定（announce 来的公钥一律 `keys_verified = false`）。
    /// 硬拒会让旧端在局域网内彻底不可见 —— 代价大于收益。
    Legacy,
    /// 带签名但验不过：包被篡改或伪造，必须丢弃。
    Invalid(String),
}

/// 校验一条 `announce`/`who_has` 包的自签名。**纯函数**，便于单测。
pub fn verify_announce(pkt: &UdpPacket) -> AnnounceAuth {
    let (Some(x), Some(e)) = (
        pkt.x25519_pubkey.as_deref().filter(|s| !s.is_empty()),
        pkt.ed25519_pubkey.as_deref().filter(|s| !s.is_empty()),
    ) else {
        // who_has 不带公钥也不带签名，属正常形态
        return if pkt.sig.is_empty() {
            AnnounceAuth::Legacy
        } else {
            AnnounceAuth::Invalid("带签名但缺少公钥".to_string())
        };
    };
    if pkt.sig.is_empty() {
        return AnnounceAuth::Legacy;
    }
    if pkt.nonce.is_empty() {
        return AnnounceAuth::Invalid("带签名但缺少 nonce（无法防重放）".to_string());
    }
    let data = announce_signing_bytes(&pkt.device_id, pkt.tcp_port, &pkt.nonce, x, e);
    if crate::crypto::verify_signature(e, &data, &pkt.sig) {
        AnnounceAuth::Verified
    } else {
        AnnounceAuth::Invalid("签名校验失败".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 历史 video/audio 行在**读出口**归一为文件卡片；其余 kind 原样（数据不回写）。
    #[test]
    fn display_kind_maps_legacy_media_to_file() {
        assert_eq!(display_kind("video"), "file");
        assert_eq!(display_kind("audio"), "file");
        assert_eq!(display_kind("image"), "image");
        assert_eq!(display_kind("file"), "file");
        assert_eq!(display_kind("text"), "text");
        // code 有歧义（真代码块同为 kind=code），刻意不映射
        assert_eq!(display_kind("code"), "code");
    }

    /// **未知 kind 的预览必须是占位文案，绝不能是载荷**（INV-P24 第 2 条）。
    ///
    /// 为什么值得单独钉：`preview_text` 的默认分支过去**无条件透传正文**，于是对端版本比
    /// 本机新时（或 4.22.1 之前写坏的 kind 形态），会话列表与系统通知会直接显示一串裸 JSON。
    /// 而它的行为是"少显示点东西"，没有任何测试会因为缺少它而失败。
    /// 三个对照分支同样重要：`text`/`system` 必须照旧透传（否则"全都塞进占位"也能让第一条
    /// 断言通过 = 空转），历史 `video` 行必须归一成 `[文件]`（把真实消息判成"不支持"同样是错）。
    #[test]
    fn preview_text_hides_payload_for_unknown_kind() {
        let payload = r#"{"question":"周五前交","options":["A","B"]}"#;
        assert_eq!(preview_text("sticker", payload), UNSUPPORTED_PREVIEW_LABEL);
        assert!(
            !preview_text("sticker", payload).contains('周'),
            "未知 kind 的预览不得外泄载荷内容"
        );
        // 对照 1：已知但没有专门分支的 kind ⇒ 正文本来就是给人看的，照旧截断
        assert_eq!(preview_text("text", "hello"), "hello");
        assert_eq!(preview_text("system", "X 加入了群聊"), "X 加入了群聊");
        // 对照 2：4.22.1 之前写坏的历史行 ⇒ 归一化成 [文件]，不是"不支持"
        assert_eq!(
            preview_text("video", r#"{"name":"a.mp4","path":"/x/a.mp4","size":1}"#),
            "[文件]"
        );
    }

    /// `is_known_kind` 与 `kind_class` 的区别必须成立（前者能认出"未知"）。
    ///
    /// 只钉一条判据：`kind_class` 对未知值回落到 Bubble（设计上就看不出未知），
    /// 所以渲染层判"未知"必须走 `is_known_kind` —— 这条测试挡住有人把两者混用一个。
    #[test]
    fn only_is_known_kind_can_tell_unrecognized_apart() {
        assert!(!is_known_kind("sticker"), "表里没有的 kind 必须判为未知");
        assert_eq!(kind_class("sticker"), KindClass::Bubble, "回落仍是 Bubble");
        for (k, _) in WIRE_KINDS {
            assert!(is_known_kind(k), "{k} 在表里却判成未知");
        }
    }

    use crate::crypto::Identity;

    // ---------------- announce 自签名 ----------------

    /// 按线上形态构造一条自签名 announce。
    fn signed_announce(id: &Identity, device_id: &str, port: u16, nonce: &str) -> super::UdpPacket {
        let x = id.x25519_public_b64();
        let e = id.ed25519_public_b64();
        let sig = id.sign_b64(&super::announce_signing_bytes(
            device_id, port, nonce, &x, &e,
        ));
        super::UdpPacket {
            kind: "announce".to_string(),
            device_id: device_id.to_string(),
            nickname: "nick".to_string(),
            tcp_port: port,
            x25519_pubkey: Some(x),
            ed25519_pubkey: Some(e),
            nonce: nonce.to_string(),
            sig,
        }
    }

    #[test]
    fn announce_verified_when_self_signed() {
        let id = Identity::generate();
        let pkt = signed_announce(&id, "dev-a", 59992, "n1");
        assert_eq!(super::verify_announce(&pkt), super::AnnounceAuth::Verified);
    }

    /// 篡改任何**被签名覆盖**的字段都必须失败 —— 这是「防篡改」的全部内容。
    #[test]
    fn announce_rejects_tampering_on_every_signed_field() {
        let id = Identity::generate();
        let base = signed_announce(&id, "dev-a", 59992, "n1");

        let mut p = base.clone();
        p.device_id = "victim".to_string();
        assert!(
            matches!(super::verify_announce(&p), super::AnnounceAuth::Invalid(_)),
            "改 device_id"
        );

        let mut p = base.clone();
        p.tcp_port = 1;
        assert!(
            matches!(super::verify_announce(&p), super::AnnounceAuth::Invalid(_)),
            "改 tcp_port"
        );

        let mut p = base.clone();
        p.nonce = "n2".to_string();
        assert!(
            matches!(super::verify_announce(&p), super::AnnounceAuth::Invalid(_)),
            "改 nonce"
        );

        // 换成攻击者自己的公钥（想把绑定指向自己的密钥）
        let attacker = Identity::generate();
        let mut p = base.clone();
        p.ed25519_pubkey = Some(attacker.ed25519_public_b64());
        p.x25519_pubkey = Some(attacker.x25519_public_b64());
        assert!(
            matches!(super::verify_announce(&p), super::AnnounceAuth::Invalid(_)),
            "换公钥"
        );

        // nickname **不在**签名范围内（改名不该让签名失效），故意不测它
        let mut p = base.clone();
        p.nickname = "换个昵称".to_string();
        assert_eq!(
            super::verify_announce(&p),
            super::AnnounceAuth::Verified,
            "nickname 不参与签名"
        );
    }

    /// 用别人的公钥声称自己是对方：签名一定对不上（攻击者没有对方私钥）。
    #[test]
    fn announce_rejects_forged_signature_with_victim_pubkey() {
        let attacker = Identity::generate();
        let victim = Identity::generate();
        let vk = victim.ed25519_public_b64();
        let vx = victim.x25519_public_b64();
        // 攻击者用**自己的**私钥签，却声明受害者的公钥
        let sig = attacker.sign_b64(&super::announce_signing_bytes(
            "victim", 59992, "n1", &vx, &vk,
        ));
        let pkt = super::UdpPacket {
            kind: "announce".to_string(),
            device_id: "victim".to_string(),
            nickname: String::new(),
            tcp_port: 59992,
            x25519_pubkey: Some(vx),
            ed25519_pubkey: Some(vk),
            nonce: "n1".to_string(),
            sig,
        };
        assert!(matches!(
            super::verify_announce(&pkt),
            super::AnnounceAuth::Invalid(_)
        ));
    }

    /// 旧端不签名 → 放行（Legacy）。硬拒会让旧端在局域网内彻底不可见，
    /// 而 announce 本就不能用于身份绑定（公钥恒为 keys_verified=false），放行的风险可控。
    #[test]
    fn announce_without_signature_is_legacy_not_rejected() {
        let id = Identity::generate();
        let mut pkt = signed_announce(&id, "dev-a", 59992, "n1");
        pkt.sig = String::new();
        assert_eq!(super::verify_announce(&pkt), super::AnnounceAuth::Legacy);

        // who_has：不带公钥也不带签名，是正常形态
        let probe = super::UdpPacket {
            kind: "who_has".to_string(),
            device_id: "dev-a".to_string(),
            nickname: String::new(),
            tcp_port: 59992,
            x25519_pubkey: None,
            ed25519_pubkey: None,
            nonce: String::new(),
            sig: String::new(),
        };
        assert_eq!(super::verify_announce(&probe), super::AnnounceAuth::Legacy);
    }

    /// 带签名却缺 nonce / 缺公钥 → 无法防重放或无法验签，必须拒。
    #[test]
    fn announce_rejects_signed_but_incomplete_packets() {
        let id = Identity::generate();

        let mut p = signed_announce(&id, "dev-a", 59992, "n1");
        p.nonce = String::new();
        assert!(matches!(
            super::verify_announce(&p),
            super::AnnounceAuth::Invalid(_)
        ));

        let mut p = signed_announce(&id, "dev-a", 59992, "n1");
        p.x25519_pubkey = None;
        assert!(matches!(
            super::verify_announce(&p),
            super::AnnounceAuth::Invalid(_)
        ));

        let mut p = signed_announce(&id, "dev-a", 59992, "n1");
        p.ed25519_pubkey = Some(String::new());
        assert!(matches!(
            super::verify_announce(&p),
            super::AnnounceAuth::Invalid(_)
        ));
    }

    /// 签名材料对每个字段敏感（防止将来有人漏字段导致"改了也能过"）。
    #[test]
    fn announce_signing_bytes_sensitive_to_every_field() {
        let base = super::announce_signing_bytes("a", 1, "n", "x", "e");
        assert_ne!(base, super::announce_signing_bytes("b", 1, "n", "x", "e"));
        assert_ne!(base, super::announce_signing_bytes("a", 2, "n", "x", "e"));
        assert_ne!(base, super::announce_signing_bytes("a", 1, "m", "x", "e"));
        assert_ne!(base, super::announce_signing_bytes("a", 1, "n", "y", "e"));
        assert_ne!(base, super::announce_signing_bytes("a", 1, "n", "x", "f"));
        // 与 Hello 的材料必须不同域（前缀不同），否则一个协议的签名能拿到另一个用
        assert_ne!(
            base,
            super::hello_signing_bytes("a", 1, "n", "x", "e"),
            "announce 与 Hello 的签名材料必须域分离"
        );
    }

    /// 表情 token 的**形态**校验：挡畸形与超长，但**不判断表情是否存在**
    /// （目录的唯一来源是前端，后端再存一份就是第二个真相源）。
    #[test]
    fn emoji_token_shape_is_validated_but_not_the_catalogue() {
        assert!(super::is_valid_emoji_token("[赞]"));
        assert!(super::is_valid_emoji_token("[微笑]"));
        // 后端不认识的名字也必须放行 —— 前端加了新表情不该需要同时改后端
        assert!(super::is_valid_emoji_token("[后端不认识的表情]"));
        for bad in [
            "",
            "[",
            "]",
            "[]",
            "赞",
            "[赞",
            "赞]",
            "[[赞]]",
            "[赞][踩]",
            "[a\nb]",
        ] {
            assert!(!super::is_valid_emoji_token(bad), "{bad:?} 应被拒");
        }
        assert!(
            !super::is_valid_emoji_token(&format!("[{}]", "很".repeat(20))),
            "超长应被拒"
        );
    }

    /// 回应载荷的线上往返（发送端序列化 → 接收端反序列化）。
    #[test]
    fn reaction_payload_roundtrips() {
        let p = super::ReactionPayload {
            target: "msg-1".to_string(),
            emoji: "[赞]".to_string(),
            add: true,
        };
        let wire = serde_json::to_string(&p).unwrap();
        let back: super::ReactionPayload = serde_json::from_str(&wire).unwrap();
        assert_eq!(back.target, "msg-1");
        assert_eq!(back.emoji, "[赞]");
        assert!(back.add);
        // 与群消息 payload 同形（{"kind","content"} 里的 content 就是它）
        assert!(wire.contains("\"add\":true"));
    }

    /// Phase 8（ADR-0017）：不透明外部帧的边界校验 —— **畸形/超限只丢该帧，不断链**。
    #[test]
    fn opaque_external_validation_bounds() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let ok = STANDARD.encode(b"bitchat-packet");
        assert_eq!(
            super::validate_opaque_external("pkt-1", 3, &ok).unwrap(),
            b"bitchat-packet"
        );
        assert!(super::validate_opaque_external("pkt-1", 0, &ok).is_err());
        assert!(super::validate_opaque_external("pkt-1", super::MAX_OPAQUE_TTL + 1, &ok).is_err());
        assert!(super::validate_opaque_external("", 3, &ok).is_err());
        assert!(
            super::validate_opaque_external(&"x".repeat(super::MAX_OPAQUE_ID + 1), 3, &ok).is_err()
        );
        assert!(super::validate_opaque_external("bad id!", 3, &ok).is_err());
        assert!(super::validate_opaque_external("pkt-1", 3, "not base64!!").is_err());
        assert!(super::validate_opaque_external("pkt-1", 3, "").is_err());
        let huge = STANDARD.encode(vec![0u8; super::MAX_OPAQUE_PAYLOAD + 1]);
        assert!(super::validate_opaque_external("pkt-1", 3, &huge).is_err());
    }

    /// 线格式必须能原样往返（Gosslan 不解码载荷，只透传）。
    #[test]
    fn opaque_external_round_trips_through_wire_format() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let payload = STANDARD.encode(vec![0u8, 1, 2, 250, 255]);
        let msg = super::Message::OpaqueExternal {
            id: "pkt-9".to_string(),
            ttl: 5,
            payload: payload.clone(),
        };
        let json = serde_json::to_vec(&msg).unwrap();
        let back: super::Message = serde_json::from_slice(&json).unwrap();
        match back {
            super::Message::OpaqueExternal {
                id,
                ttl,
                payload: p,
            } => {
                assert_eq!(id, "pkt-9");
                assert_eq!(ttl, 5);
                assert_eq!(p, payload);
            }
            other => panic!("往返后类型变了：{other:?}"),
        }
    }

    use super::*;

    fn env() -> GossipEnvelope {
        GossipEnvelope {
            message_id: String::new(),
            sender_id: "dev-a".into(),
            nonce: "nonce-1".into(),
            sender_pubkey: "xk".into(),
            sender_ed25519: "ek".into(),
            sender_sig: "sig".into(),
            ttl: 6,
            kind: GossipKind::Chat,
            group_id: None,
            group_name: None,
            group_creator: None,
            group_members: Vec::new(),
            payload: "ciphertext".into(),
            ts: 123456,
            seq: 1,
            encrypted: true,
            target: None,
        }
    }

    /// 好友申请（定向）信封：加密、签名、验签、解密、target 完整性。
    #[test]
    fn friend_request_envelope_encrypt_sign_decrypt_and_target_integrity() {
        use crate::crypto::Identity;
        use crate::gossip_engine::GossipEngine;
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

        let a = Identity::generate();
        let c = Identity::generate();
        let engine = GossipEngine::new(100, 10, 4, 6);

        // A 构造 FriendRequest（target=C，用 C 的 X25519 公钥加密内容）
        let payload = r#"{"from_nickname":"Alice","from_avatar":null}"#;
        let shared =
            crate::crypto::shared_secret(&a.x25519_secret, &c.x25519_public_b64()).unwrap();
        let sealed = crate::crypto::seal(&shared, payload.as_bytes()).unwrap();
        let payload_b64 = B64.encode(&sealed);
        let mut env = engine.build_envelope(
            &a,
            "dev-a",
            GossipKind::FriendRequest,
            None,
            None,
            &payload_b64,
            123456,
            0,
        );
        env.target = Some("dev-c".into());
        env.sender_sig = a.sign_b64(&env.signing_bytes());

        // 验签通过（target 参与签名）
        assert!(engine.verify_envelope(&env));

        // C 用自己的私钥解开内容
        let shared2 = crate::crypto::shared_secret(&c.x25519_secret, &env.sender_pubkey).unwrap();
        let pt = crate::crypto::open(&shared2, &B64.decode(&env.payload).unwrap()).unwrap();
        assert_eq!(String::from_utf8(pt).unwrap(), payload);

        // 中间节点篡改 target → 验签失败（target 不可篡改）
        let mut tampered = env.clone();
        tampered.target = Some("dev-eve".into());
        assert!(!engine.verify_envelope(&tampered));

        // target 序列化：None 不写键，Some 写入
        let json_none = serde_json::to_string(&GossipEnvelope {
            target: None,
            ..env.clone()
        })
        .unwrap();
        assert!(
            !json_none.contains("target"),
            "None 不应写 target 键: {json_none}"
        );
        let json_some = serde_json::to_string(&env).unwrap();
        assert!(json_some.contains("dev-c"), "Some 应写 target: {json_some}");
    }

    /// ChatAck / ChatReadReceipt（定向、明文）信封：签名、验签、target 完整性、明文往返。
    #[test]
    fn chat_ack_and_read_receipt_plaintext_directed_envelope_integrity() {
        use crate::crypto::Identity;
        use crate::gossip_engine::GossipEngine;
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

        let c = Identity::generate();
        let engine = GossipEngine::new(100, 10, 4, 6);

        // ChatAck：接收方 C 回给原始发送方 A，明文 { msg_id }
        let ack_payload = r#"{"msg_id":"deadbeef"}"#;
        let mut ack = engine.build_envelope(
            &c,
            "dev-c",
            GossipKind::ChatAck,
            None,
            None,
            &B64.encode(ack_payload.as_bytes()),
            123456,
            0,
        );
        ack.encrypted = false;
        ack.target = Some("dev-a".into());
        ack.sender_sig = c.sign_b64(&ack.signing_bytes());

        // 验签通过（target 参与签名）
        assert!(engine.verify_envelope(&ack));
        // 明文：直接 base64 解码即可得到原始 JSON，无需解密
        assert_eq!(B64.decode(&ack.payload).unwrap(), ack_payload.as_bytes());
        // 篡改 target → 验签失败
        let mut tampered = ack.clone();
        tampered.target = Some("dev-eve".into());
        assert!(!engine.verify_envelope(&tampered));

        // ChatReadReceipt：定向明文，payload 含 last_read_ts / last_read_msg_id
        let rr_payload = r#"{"last_read_ts":99,"last_read_msg_id":"m-1"}"#;
        let mut rr = engine.build_envelope(
            &c,
            "dev-c",
            GossipKind::ChatReadReceipt,
            None,
            None,
            &B64.encode(rr_payload.as_bytes()),
            123456,
            0,
        );
        rr.encrypted = false;
        rr.target = Some("dev-a".into());
        rr.sender_sig = c.sign_b64(&rr.signing_bytes());
        assert!(engine.verify_envelope(&rr));
        assert_eq!(B64.decode(&rr.payload).unwrap(), rr_payload.as_bytes());
    }

    #[test]
    fn envelope_encrypted_flag_roundtrip() {
        // 显式 false 往返保持 false
        let mut e = env();
        e.encrypted = false;
        e.compute_message_id();
        let json = serde_json::to_string(&Message::Gossip {
            envelope: e.clone(),
        })
        .unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::Gossip { envelope } => assert!(!envelope.encrypted),
            _ => panic!("expect gossip"),
        }

        // 未声明加密标志的旧信封直接拒绝，不再兼容旧协议。
        let legacy = r#"{"type":"gossip","envelope":{"message_id":"m","sender_id":"a","sender_pubkey":"x","sender_ed25519":"e","sender_sig":"s","ttl":6,"kind":"chat","group_id":null,"payload":"p","ts":1}}"#;
        assert!(serde_json::from_str::<Message>(legacy).is_err());
    }

    #[test]
    fn gossip_message_id_deterministic_and_sensitive_to_payload() {
        let mut e1 = env();
        e1.compute_message_id();
        let id1 = e1.message_id.clone();
        assert_eq!(id1.len(), 64); // SHA-256 hex

        let mut e2 = e1.clone();
        e2.compute_message_id();
        assert_eq!(id1, e2.message_id); // 同内容同 id

        e2.payload = "tampered".into();
        e2.compute_message_id();
        assert_ne!(id1, e2.message_id); // 篡改 payload → id 变化

        // 时间戳不参与消息身份：改变 ts 不应改变 message_id。
        let mut e3 = e1.clone();
        e3.ts = 999_999;
        e3.compute_message_id();
        assert_eq!(id1, e3.message_id);

        // nonce 参与消息身份：改变 nonce 必须改变 message_id。
        let mut e4 = e1.clone();
        e4.nonce = "nonce-2".into();
        e4.compute_message_id();
        assert_ne!(id1, e4.message_id);
    }

    #[test]
    fn message_json_roundtrip() {
        let mut e = env();
        e.compute_message_id();
        let msg = Message::Gossip {
            envelope: e.clone(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::Gossip { envelope } => {
                assert_eq!(envelope.message_id, e.message_id);
                assert_eq!(envelope.sender_id, "dev-a");
            }
            _ => panic!("应还原为 Gossip 消息"),
        }
    }

    #[test]
    fn msg_kind_mapping() {
        assert_eq!(MsgKind::from_wire_str("code"), MsgKind::Code);
        assert_eq!(MsgKind::from_wire_str("unknown"), MsgKind::Text);
        assert_eq!(MsgKind::Code.as_str(), "code");
    }

    /// 诊断用的类型名必须与**线格式**一致（真机排查只认日志里这个词）。
    ///
    /// 为什么这条测试值得存在：BLE 握手失败时日志现在会写「对端首帧不是 Hello（收到 xxx）」，
    /// `xxx` 就是 `wire_kind()` 的输出。若哪天有人把它改成手写 match 又漏了变体，
    /// 这里会立刻红 —— 而不是等到真机上看着一个错误的类型名猜半天。
    #[test]
    fn wire_kind_matches_the_serde_tag() {
        let hello = Message::Hello {
            device_id: "dev-a".into(),
            nickname: "A".into(),
            avatar: None,
            device_type: "desktop".into(),
            content_features: super::content_features(),
            protocol_version: Some(super::PROTOCOL_VERSION),
            app_version: Some(super::current_app_version().to_string()),
            tcp_port: 59992,
            x25519_pubkey: "xk".into(),
            ed25519_pubkey: "ek".into(),
            conv_clock: 0,
            nonce: "n1".into(),
            sig: "sig".into(),
        };
        assert_eq!(hello.wire_kind(), "hello");
        // 与真实序列化结果的 `type` 字段逐字一致（不是"看起来差不多"）
        let v: serde_json::Value = serde_json::to_value(&hello).unwrap();
        assert_eq!(v["type"], serde_json::json!("hello"));

        assert_eq!(
            Message::Heartbeat {
                device_id: "dev-a".into()
            }
            .wire_kind(),
            "heartbeat"
        );
    }

    /// **serde 事实**：`Message` 是 `#[serde(tag = "type")]` 枚举，未知 `type` 在
    /// `from_slice` 这一层**就是硬错误**。
    ///
    /// ⚠️ 立场已变（ADR-0007 Accepted 2026-09-20 + INV-P24 第 1 条）：这个错误**不允许**
    /// 一路冒到 `reader_loop` —— 旧行为是 `read_frame` 返回 `InvalidData` ⇒ 断开整条连接，
    /// 于是新版本只要上线一种新帧，老设备就不是"少收一条"而是"跟这台设备连不上"，
    /// 还伴随重连-再拆的死循环。现在由 `network::transport::decode_frame` 在帧层降级成
    /// `Message::Unknown`（忽略 + 节流日志，链路保持），并由守卫
    /// `unknown_wire_frame_is_tolerated_after_auth` 钉住。
    ///
    /// 这条测试保留的理由：它钉的是 serde 层的既有事实（降级逻辑正是依赖
    /// "未知变体报 unknown variant"这个措辞来判别），并带一条**防空转对照**
    /// （已知变体必须能解析，否则"全都失败"也会让断言看起来通过）。
    #[test]
    fn unknown_message_type_is_a_hard_parse_error() {
        let unknown = br#"{"type":"some_future_kind","id":"x"}"#;
        assert!(
            serde_json::from_slice::<Message>(unknown).is_err(),
            "serde 层必须仍然报未知 type —— decode_frame 靠这个错误把未知帧降级，而不是当畸形帧"
        );
        // 对照：已知变体必须能解析（否则上面那条断言会因为"全都解析失败"而变成空转）。
        // `Heartbeat` 需要 `device_id`，这里给全字段。
        let known = br#"{"type":"heartbeat","device_id":"dev-a"}"#;
        assert!(
            serde_json::from_slice::<Message>(known).is_ok(),
            "对照用例必须能解析，否则上面的断言是空转（全都失败也算通过）"
        );
    }

    #[test]
    fn hello_signing_bytes_sensitive_to_every_field() {
        let base = hello_signing_bytes("dev-a", 59992, "n1", "xk", "ek");
        // 相同输入必须产出相同字节（签名可复现）
        assert_eq!(base, hello_signing_bytes("dev-a", 59992, "n1", "xk", "ek"));
        // 任一字段变化都必须改变签名材料：否则攻击者可平移字段伪造身份
        assert_ne!(base, hello_signing_bytes("dev-b", 59992, "n1", "xk", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 1, "n1", "xk", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 59992, "n2", "xk", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 59992, "n1", "xk2", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 59992, "n1", "xk", "ek2"));
        // 拼接歧义防护：把不同字段切成另一种组合不应撞车
        assert_ne!(
            hello_signing_bytes("ab", 1, "c", "d", "e"),
            hello_signing_bytes("a", 1, "bc", "d", "e")
        );
    }

    #[test]
    fn hello_carries_nonce_and_sig_roundtrip() {
        let hello = Message::Hello {
            device_id: "dev-a".into(),
            nickname: "A".into(),
            avatar: None,
            device_type: "desktop".into(),
            content_features: super::content_features(),
            protocol_version: None,
            app_version: None,
            tcp_port: 59992,
            x25519_pubkey: "xk".into(),
            ed25519_pubkey: "ek".into(),
            conv_clock: 7,
            nonce: "n1".into(),
            sig: "sig".into(),
        };
        let json = serde_json::to_string(&hello).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::Hello { nonce, sig, .. } => {
                assert_eq!(nonce, "n1");
                assert_eq!(sig, "sig");
            }
            _ => panic!("expect hello"),
        }
        // 不带 nonce/sig 的旧 Hello 仍可解析（serde default），但会在验证层被拒
        let legacy = r#"{"type":"hello","device_id":"a","nickname":"A","avatar":null,"tcp_port":1,"x25519_pubkey":"x","ed25519_pubkey":"e","conv_clock":0}"#;
        match serde_json::from_str::<Message>(legacy).unwrap() {
            Message::Hello { nonce, sig, .. } => {
                assert!(nonce.is_empty() && sig.is_empty());
            }
            _ => panic!("expect hello"),
        }
    }

    /// device_type：序列化往返保持，旧 Hello 缺省为空串（旧端兼容，不参与签名）。
    #[test]
    fn hello_device_type_roundtrip_and_legacy_default() {
        let hello = Message::Hello {
            device_id: "a".into(),
            nickname: "A".into(),
            avatar: None,
            device_type: "mobile".into(),
            content_features: super::content_features(),
            protocol_version: None,
            app_version: None,
            tcp_port: 1,
            x25519_pubkey: "x".into(),
            ed25519_pubkey: "e".into(),
            conv_clock: 0,
            nonce: "n".into(),
            sig: "s".into(),
        };
        let json = serde_json::to_string(&hello).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::Hello { device_type, .. } => assert_eq!(device_type, "mobile"),
            _ => panic!("expect hello"),
        }
        // 旧 Hello（无 device_type 字段）→ 缺省空串
        let legacy = r#"{"type":"hello","device_id":"a","nickname":"A","avatar":null,"tcp_port":1,"x25519_pubkey":"x","ed25519_pubkey":"e","conv_clock":0}"#;
        match serde_json::from_str::<Message>(legacy).unwrap() {
            Message::Hello { device_type, .. } => assert!(device_type.is_empty()),
            _ => panic!("expect hello"),
        }
    }

    /// 版本声明（ADR-0007 决策 1）：新端往返保持，**老 Hello 缺省为 `None`**。
    ///
    /// 这一步的全部价值来自"加这两个字段不断老版本互通"，所以两个方向都要钉：
    /// - 老→新：缺字段的 Hello 必须照样解析（报错就是老设备**连不上**，不是"少个信息"）；
    /// - 新→老：我们多带的字段必须被忽略 —— 判据就是这里**没有** `deny_unknown_fields`，
    ///   一旦有人加上，"给帧加字段"这件事本身会变成一次破坏性变更。
    #[test]
    fn hello_version_fields_roundtrip_and_old_peer_declares_nothing() {
        let hello = Message::Hello {
            device_id: "a".into(),
            nickname: "A".into(),
            avatar: None,
            device_type: "desktop".into(),
            content_features: super::content_features(),
            protocol_version: Some(2),
            app_version: Some("9.9.9".into()),
            tcp_port: 1,
            x25519_pubkey: "x".into(),
            ed25519_pubkey: "e".into(),
            conv_clock: 0,
            nonce: "n".into(),
            sig: "s".into(),
        };
        let json = serde_json::to_string(&hello).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::Hello {
                protocol_version,
                app_version,
                ..
            } => {
                assert_eq!(protocol_version, Some(2));
                assert_eq!(app_version.as_deref(), Some("9.9.9"));
            }
            _ => panic!("expect hello"),
        }
        // 老端（4.22.27 之前）的 Hello：没有这两个字段 ⇒ 必须是 `None`。
        // 不能回落成 Some(1)：诊断面板要能区分"对方是没报版本的老版本"和"对方报了 1"。
        let legacy = r#"{"type":"hello","device_id":"a","nickname":"A","avatar":null,"tcp_port":1,"x25519_pubkey":"x","ed25519_pubkey":"e","conv_clock":0}"#;
        match serde_json::from_str::<Message>(legacy).unwrap() {
            Message::Hello {
                protocol_version,
                app_version,
                ..
            } => {
                assert_eq!(protocol_version, None);
                assert_eq!(app_version, None);
            }
            _ => panic!("expect hello"),
        }
        // 新→老：未知**字段**必须被忽略（这条断言就是"不许 deny_unknown_fields"的哨兵）。
        let newer = r#"{"type":"hello","device_id":"a","nickname":"A","avatar":null,"tcp_port":1,"x25519_pubkey":"x","ed25519_pubkey":"e","conv_clock":0,"protocol_version":9,"some_future_field":true}"#;
        assert!(
            serde_json::from_str::<Message>(newer).is_ok(),
            "Hello 遇到未知字段必须照单收下，否则加字段就等于破坏性变更"
        );
    }

    /// **未知 kind 不得把整条 `chat_message` 带崩**（INV-P24 第 2 条）。
    ///
    /// 这条钉的是"`ChatMessage.kind` 为什么是 String 而不是 `MsgKind`"：只要它是枚举，
    /// `kind:"sticker"` 就会让 serde 报 `unknown variant`，而 `decode_frame` 分不清
    /// "未知**帧**类型"与"未知**嵌套枚举值**" ⇒ 整帧被降级成 `Message::Unknown` 丢弃
    /// ⇒ 消息根本进不了库，前端连「不支持的消息类型」都来不及显示，发送方永远等不到 Ack。
    /// 所以正确的判据是：**同一个未知值放在 kind 上必须还能解析，放在 type 上才该降级**。
    #[test]
    fn unknown_message_kind_still_decodes_as_a_chat_message() {
        let frame = br#"{"type":"chat_message","msg_id":"m1","from":"a","to":"b","kind":"sticker","content":"enc1:AAA","ts":1,"seq":1}"#;
        match serde_json::from_slice::<Message>(frame)
            .expect("未知 kind 必须仍能解析成 ChatMessage（否则整条消息会被静默丢弃）")
        {
            Message::ChatMessage { kind, msg_id, .. } => {
                assert_eq!(kind, "sticker", "kind 必须原样保留，不能回落成 text");
                assert_eq!(msg_id, "m1");
            }
            other => panic!("应还原为 ChatMessage，实得 {other:?}"),
        }
        // 对照（防空转）：未知值放在 **type** 上时仍是硬解析错误 —— 那才是
        // `decode_frame` 该降级成 Unknown 的场景（见 unknown_message_type_is_a_hard_parse_error）。
        assert!(serde_json::from_slice::<Message>(br#"{"type":"sticker"}"#).is_err());
    }

    /// 兼容判定的四种输入 —— `Some(PROTOCOL_VERSION)` 那条是**防空转的关键**：
    /// 写成 `>=` 就会在每个同版本好友上刷"对方版本较新"，而真网里同版本才是常态。
    #[test]
    fn peer_protocol_newer_only_when_declared_higher() {
        assert!(!peer_protocol_is_newer(None), "未声明 ≠ 更高（也 ≠ 更低）");
        assert!(
            !peer_protocol_is_newer(Some(PROTOCOL_VERSION)),
            "同版本必须判不高 —— 写成 >= 就会满屏误报"
        );
        assert!(!peer_protocol_is_newer(Some(0)), "0 也不猜成更高");
        assert!(peer_protocol_is_newer(Some(PROTOCOL_VERSION + 1)));
    }

    /// 发送侧门控的四种输入 —— 关键是 `0`（没交换过 Hello / 对端已离线）判**不许发**。
    /// 方向选"不知道就当不支持"是刻意的：宁可少发一条新类型，也不要让老对端整帧丢掉、
    /// 发送方还以为是网络问题。
    #[test]
    fn gated_kind_needs_the_peers_own_declaration() {
        assert!(
            kind_allowed_by_features("text", 0),
            "V1 词表内的 kind 对所有对端都安全，不该被门控挡住"
        );
        assert!(
            !kind_allowed_by_features("merge", 0),
            "对端没声明能力 ⇒ 不许发 merge"
        );
        assert!(kind_allowed_by_features("merge", CONTENT_FEATURE_MERGE));
        // 只认自己那一位：对方有别的 capability 不算数（防"位图非零就放行"这种糊法）
        assert!(
            !kind_allowed_by_features("merge", CONTENT_FEATURE_PULL),
            "按位判定，不是按非零判定"
        );
    }

    /// 加了门控却忘了在 Hello 里声明 ⇒ 这种消息永远发不出去，而且是静默的。
    /// 这条测试盯的就是"门控表与广播的能力对不上"这个组合。
    #[test]
    fn every_gated_kind_is_advertised_by_us() {
        let ours = content_features();
        let mut gated = Vec::new();
        for (kind, _) in WIRE_KINDS {
            if let Some(bit) = kind_required_feature(kind) {
                assert_ne!(
                    ours & bit,
                    0,
                    "kind `{kind}` 要求能力位 {bit:#b}，但本机 Hello 没声明它 —— 门控会把我们自己的功能锁死"
                );
                gated.push(bit);
            }
        }
        assert!(
            gated.contains(&CONTENT_FEATURE_MERGE),
            "merge 必须仍在被门控之列：v4.20.0 及更早的 MsgKind 里没有这个变体"
        );
    }

    /// 群受众统计必须**三态分开**：确知不支持 / 版本未知 / 支持。
    ///
    /// 为什么单独立一条：把"未知"并进"不支持"在 1:1 发送口是对的方向（宁可少发），
    /// 在群提示上是错的（离线成员是常态 ⇒ 提示永远在响 ⇒ 没人再看）。这条测试钉的就是
    /// 两个函数**不该共用一个默认值**。
    #[test]
    fn group_audience_keeps_unknown_apart_from_unsupported() {
        let members: Vec<String> = ["a", "b", "c", "me"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // a 声明过但没有 merge 位；b 声明了 merge 位；c 从没交换过 Hello（未知）
        let features = |id: &str| match id {
            "a" => Some(CONTENT_FEATURE_PULL),
            "b" => Some(CONTENT_FEATURE_MERGE),
            "c" => None,
            _ => Some(content_features()),
        };
        let (unsupported, unknown, gated) = kind_audience("merge", features, &members, "me");
        assert!(gated, "merge 是受能力位约束的 kind");
        assert_eq!(unsupported, vec!["a".to_string()], "只数确知缺位的");
        assert_eq!(unknown, 1, "没交换过 Hello 的算未知，不算缺位");

        // 不受约束的 kind：一个都不该报，也不该弹提示
        let (u2, k2, g2) = kind_audience("text", features, &members, "me");
        assert!(!g2 && u2.is_empty() && k2 == 0, "text 对所有版本安全");

        // 只有未知、没有确知缺位 ⇒ 句子为空（宁可不说话，也不说一条永远在响的话）
        let only_unknown: Vec<String> = ["c".to_string()].into_iter().collect();
        let (u3, k3, _) = kind_audience("merge", features, &only_unknown, "me");
        assert!(u3.is_empty() && k3 == 1);
        assert_eq!(kind_audience_hint("merge", &[], only_unknown.len()), "");

        let hint = kind_audience_hint("merge", &["小李".to_string(), "阿强".to_string()], 3);
        assert!(
            hint.contains("小李") && hint.contains("阿强"),
            "要列得出是谁"
        );
        assert!(hint.contains("不会丢"), "必须说清只是渲染退化，不是丢消息");
        assert!(
            hint.contains("3 位成员") && hint.contains("版本未知"),
            "未知的另计，不冒充缺位"
        );
    }

    /// 缺位人数很多时不刷屏：列出前几个 + 「等 N 名成员」。
    #[test]
    fn audience_hint_collapses_long_name_lists() {
        let names: Vec<String> = (0..9).map(|i| format!("成员{i}")).collect();
        let hint = kind_audience_hint("merge", &names, 0);
        assert!(hint.contains("等 9 名成员"), "{hint}");
        assert!(
            hint.contains("成员0") && !hint.contains("成员8"),
            "只展示前几个"
        );
    }

    #[test]
    fn group_lifecycle_messages_roundtrip() {
        let changed = Message::GroupCreatorChanged {
            group_id: "g1".into(),
            from: "old".into(),
            to: "new".into(),
        };
        let json = serde_json::to_string(&changed).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::GroupCreatorChanged { group_id, from, to } => {
                assert_eq!(
                    (group_id.as_str(), from.as_str(), to.as_str()),
                    ("g1", "old", "new")
                );
            }
            _ => panic!("expect group_creator_changed"),
        }

        let left = Message::GroupMemberLeft {
            group_id: "g1".into(),
            from: "dev-a".into(),
        };
        let json = serde_json::to_string(&left).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::GroupMemberLeft { group_id, from } => {
                assert_eq!((group_id.as_str(), from.as_str()), ("g1", "dev-a"));
            }
            _ => panic!("expect group_member_left"),
        }
    }

    #[test]
    fn group_ack_and_file_complete_ack_roundtrip() {
        let group_ack = Message::GroupAck {
            group_id: "g1".into(),
            msg_id: "m1".into(),
            from: "dev-a".into(),
        };
        let json = serde_json::to_string(&group_ack).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::GroupAck {
                group_id,
                msg_id,
                from,
            } => {
                assert_eq!(group_id, "g1");
                assert_eq!(msg_id, "m1");
                assert_eq!(from, "dev-a");
            }
            _ => panic!("expect group_ack"),
        }

        let file_ack = Message::FileCompleteAck {
            transfer_id: "t1".into(),
            success: true,
        };
        let json = serde_json::to_string(&file_ack).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::FileCompleteAck {
                transfer_id,
                success,
            } => {
                assert_eq!(transfer_id, "t1");
                assert!(success);
            }
            _ => panic!("expect file_complete_ack"),
        }
    }
}
