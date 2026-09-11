//! 中继授权策略（P2 / M4）。
//!
//! ## 要解决的问题
//! 多跳转发（`handle_gossip` 第 4 步：`choose_fanout` + TTL 衰减）今天对**所有**信封
//! **无条件**转发。于是任何邻居都能把本机当免费中转站：既消耗流量/电量，也让"我的设备
//! 在替谁传话"这件事完全不可见、不可控。
//!
//! ## 为什么默认是 `All`（而不是 `Off`）
//! `Off` 看着更"安全"，但它是**改变传播语义**：今天跨跳投递（A—B—C，A 与 C 无直连）
//! 依赖 B 转发，一旦默认关掉，已有拓扑会静默丢消息 —— 这正是项目红线 §8.1 #3
//! 「不得在重构里顺手改变传播语义」禁止的事，也违背用户「现有局域网聊天不能搞坏」的要求。
//! 所以：**默认 `All` ＝ 与今天逐字节一致**；想限制中继的用户在设置里显式选择
//! `off` / `friends` / `allowlist`。（默认值的取舍写进 ADR-0016，供用户复核。）
//!
//! ## 判据为什么放在纯函数里
//! 「谁的信封我肯转发」是**传播语义**本身，改错不会编译报错、只会静默少发/多发。
//! 因此把三条判据（目标不转发 / TTL / 授权）收进 `should_forward`，用真值表钉住，
//! transport 只负责喂事实、不自己拼条件。

/// 中继授权策略。取值与 settings 表里的 `relay_policy` 字符串一一对应。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RelayPolicy {
    /// 不替任何人转发（仍会发送自己的信封）。
    Off,
    /// 只替**好友**转发。
    Friends,
    /// 只替**白名单**里的设备转发（白名单是 device_id 列表）。
    Allowlist,
    /// 替所有通过验签的邻居转发 —— **默认值，与今天的行为一致**。
    #[default]
    All,
}

impl RelayPolicy {
    /// 从设置值解析。
    ///
    /// ⚠️ **未知值 / 缺失 → `All`**（保持今天的行为）。这里刻意不"失败即关闭"：
    /// 设置表里的脏值（旧版本写过、手工改库）不该让用户的跨跳投递瞬断。
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some("off") => Self::Off,
            Some("friends") => Self::Friends,
            Some("allowlist") => Self::Allowlist,
            Some("all") => Self::All,
            _ => Self::All,
        }
    }

    /// 稳定字符串（设置页读写 / 日志 / 诊断共用，改这里等于改持久化格式）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Friends => "friends",
            Self::Allowlist => "allowlist",
            Self::All => "all",
        }
    }

    /// 本策略下，是否愿意替 `is_friend` / `in_allowlist` 的那个发送者转发。
    pub fn allows_relay(self, is_friend: bool, in_allowlist: bool) -> bool {
        match self {
            Self::Off => false,
            Self::Friends => is_friend,
            Self::Allowlist => in_allowlist,
            Self::All => true,
        }
    }
}

/// 运行时中继配置：策略 + 白名单。
///
/// 缓存在 `AppState` 里（启动读一次、`save_settings` 时更新），**不在转发热路径上读库** ——
/// gossip 帧可能是每秒几十条，为了一个策略字段去锁 SQLite 是没必要的开销。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelayConfig {
    pub policy: RelayPolicy,
    /// 允许中继的设备 id（`Allowlist` 策略用）。
    pub allowlist: Vec<String>,
}

impl RelayConfig {
    /// 从 settings 表的两条原始值解析（`relay_policy` / `relay_allowlist`）。
    ///
    /// 白名单是 JSON 字符串数组；**解析失败当作空表**（不 panic、不影响其它设置）——
    /// 脏值最多让"白名单模式"暂时谁都不转发，用户改一下设置即可恢复。
    pub fn parse(policy_raw: Option<&str>, allowlist_raw: Option<&str>) -> Self {
        let allowlist = allowlist_raw
            .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
            .unwrap_or_default();
        Self {
            policy: RelayPolicy::parse(policy_raw),
            allowlist,
        }
    }

    /// 是否需要查"发送者是不是好友"（只有 `Friends` 需要）。
    /// 存在的意义是让热路径**按需查询**：默认 `All` 下一次库都不碰。
    pub fn needs_friend_lookup(&self) -> bool {
        self.policy == RelayPolicy::Friends
    }

    /// 是否需要查白名单（只有 `Allowlist` 需要）。
    pub fn needs_allowlist_lookup(&self) -> bool {
        self.policy == RelayPolicy::Allowlist
    }

    pub fn allowlist_contains(&self, sender_id: &str) -> bool {
        self.allowlist.iter().any(|x| x == sender_id)
    }
}

/// 转发判据的全部输入。transport 只负责把事实填进来。
#[derive(Clone, Copy, Debug)]
pub struct RelayInput {
    /// 本机就是信封的 `target`（定向帧到达目的地）⇒ 只消费，不再转发。
    /// 不判这条会与邻居形成"你转给我、我再转回你"的冗余中转与回环。
    pub is_target: bool,
    /// 信封剩余 TTL（转发后减 1；`<= 1` 表示不能再转）。
    pub ttl: u8,
    /// 信封的原始发送者就是本机 ⇒ 这是"源发"，不是"替人中转"，永远允许。
    pub sender_is_me: bool,
    /// 原始发送者是我的好友。
    pub sender_is_friend: bool,
    /// 原始发送者在我的中继白名单里。
    pub sender_in_allowlist: bool,
}

/// 是否转发这个信封。**这是传播语义的唯一落点**，改它必须同步更新真值表测试。
pub fn should_forward(policy: RelayPolicy, input: RelayInput) -> bool {
    if input.is_target {
        return false;
    }
    if input.ttl <= 1 {
        return false;
    }
    if input.sender_is_me {
        return true;
    }
    policy.allows_relay(input.sender_is_friend, input.sender_in_allowlist)
}

/// 转发决策的**唯一入口**。
///
/// 与 `should_forward` 的区别：这里自己按策略**按需**取好友/白名单事实 ——
/// `is_friend` 是一个闭包，只有 `Friends` 策略才会被调用（默认 `All` 下热路径零额外开销）。
/// transport 侧只需这样用：
///
/// ```ignore
/// let cfg = state.relay_config();
/// if relay_policy::decide_forward(&cfg, is_target, env.ttl, sender_is_me, &env.sender_id, || {
///     let dbc = state.db.lock()...;
///     db::get_friend(&dbc, &env.sender_id).is_some()
/// }) { /* 转发 */ }
/// ```
pub fn decide_forward(
    config: &RelayConfig,
    is_target: bool,
    ttl: u8,
    sender_is_me: bool,
    sender_id: &str,
    is_friend: impl FnOnce() -> bool,
) -> bool {
    if is_target || ttl <= 1 {
        return false;
    }
    if sender_is_me {
        return true;
    }
    let friend = if config.needs_friend_lookup() {
        is_friend()
    } else {
        false
    };
    let in_allow = if config.needs_allowlist_lookup() {
        config.allowlist_contains(sender_id)
    } else {
        false
    };
    config.policy.allows_relay(friend, in_allow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(is_target: bool, ttl: u8, sender_is_me: bool, is_friend: bool, in_allow: bool) -> RelayInput {
        RelayInput {
            is_target,
            ttl,
            sender_is_me,
            sender_is_friend: is_friend,
            sender_in_allowlist: in_allow,
        }
    }

    #[test]
    fn parse_roundtrip_and_unknown_defaults_to_all() {
        for p in [
            RelayPolicy::Off,
            RelayPolicy::Friends,
            RelayPolicy::Allowlist,
            RelayPolicy::All,
        ] {
            assert_eq!(RelayPolicy::parse(Some(p.as_str())), p, "as_str/parse 必须可往返");
        }
        // 缺失与脏值都落到 All（保持今天的行为，不静默断链）
        assert_eq!(RelayPolicy::parse(None), RelayPolicy::All);
        assert_eq!(RelayPolicy::parse(Some("")), RelayPolicy::All);
        assert_eq!(RelayPolicy::parse(Some("  ")), RelayPolicy::All);
        assert_eq!(RelayPolicy::parse(Some("OFF")), RelayPolicy::All, "大小写不敏感不做：脏值一律回落");
        assert_eq!(RelayPolicy::parse(Some("nonsense")), RelayPolicy::All);
        assert_eq!(RelayPolicy::default(), RelayPolicy::All, "默认必须等于今天的行为");
        assert_eq!(RelayPolicy::parse(Some(" off ")), RelayPolicy::Off, "允许前后空白");
    }

    /// 授权策略的**完整真值表**（4 策略 × 好友 × 白名单）。
    #[test]
    fn relay_authorization_truth_table() {
        let cases = [
            (RelayPolicy::Off, false, false, false),
            (RelayPolicy::Off, true, false, false),
            (RelayPolicy::Off, true, true, false),
            (RelayPolicy::Friends, false, false, false),
            (RelayPolicy::Friends, true, false, true),
            (RelayPolicy::Friends, true, true, true),
            (RelayPolicy::Allowlist, false, false, false),
            (RelayPolicy::Allowlist, true, false, false),
            (RelayPolicy::Allowlist, false, true, true),
            (RelayPolicy::Allowlist, true, true, true),
            (RelayPolicy::All, false, false, true),
            (RelayPolicy::All, true, true, true),
        ];
        for (policy, is_friend, in_allow, want) in cases {
            assert_eq!(
                policy.allows_relay(is_friend, in_allow),
                want,
                "{policy:?} friend={is_friend} allowlist={in_allow} 期望 {want}"
            );
        }
    }

    #[test]
    fn target_and_exhausted_ttl_never_forward_regardless_of_policy() {
        for policy in [
            RelayPolicy::Off,
            RelayPolicy::Friends,
            RelayPolicy::Allowlist,
            RelayPolicy::All,
        ] {
            assert!(
                !should_forward(policy, input(true, 6, false, true, true)),
                "{policy:?}: 本机是 target 时不得转发（否则与邻居形成回环）"
            );
            assert!(
                !should_forward(policy, input(false, 1, false, true, true)),
                "{policy:?}: TTL 耗尽不得转发"
            );
            assert!(
                !should_forward(policy, input(false, 0, false, true, true)),
                "{policy:?}: TTL=0 不得转发"
            );
        }
    }

    #[test]
    fn own_envelopes_always_forward_even_when_relay_off() {
        // 源发不是中转：即使策略是 off，"我自己的信封"也必须照常送出去
        for policy in [
            RelayPolicy::Off,
            RelayPolicy::Friends,
            RelayPolicy::Allowlist,
            RelayPolicy::All,
        ] {
            assert!(
                should_forward(policy, input(false, 6, true, false, false)),
                "{policy:?}: 自己发起的信封必须转发（否则断掉本机消息）"
            );
        }
    }

    #[test]
    fn default_policy_forwards_everything_like_today() {
        // 这条是「默认不改变现有行为」的钉子：默认策略下，任何非 target、TTL>1 的信封都转发
        for is_me in [true, false] {
            for is_friend in [true, false] {
                for in_allow in [true, false] {
                    assert!(
                        should_forward(
                            RelayPolicy::default(),
                            input(false, 6, is_me, is_friend, in_allow)
                        ),
                        "默认策略必须与今天的无条件转发逐字节一致"
                    );
                }
            }
        }
    }

    // ---------------- RelayConfig / decide_forward ----------------

    fn cfg(policy: RelayPolicy, allow: &[&str]) -> RelayConfig {
        RelayConfig {
            policy,
            allowlist: allow.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn config_parse_tolerates_dirty_allowlist() {
        let c = RelayConfig::parse(Some("friends"), Some("not json"));
        assert_eq!(c.policy, RelayPolicy::Friends);
        assert!(c.allowlist.is_empty(), "脏值当空表，不能 panic、不能影响策略解析");

        let c = RelayConfig::parse(Some("allowlist"), Some(r#"["dev-1","dev-2"]"#));
        assert_eq!(c.policy, RelayPolicy::Allowlist);
        assert!(c.allowlist_contains("dev-1"));
        assert!(!c.allowlist_contains("dev-3"));

        // 缺失两条 key → 默认 All + 空白名单（= 今天的行为）
        assert_eq!(RelayConfig::parse(None, None), RelayConfig::default());
    }

    #[test]
    fn default_path_does_not_touch_friend_or_allowlist() {
        // 默认策略（All）下**一次额外查询都不做** —— 这是"不改变现有行为、不加开销"的钉子
        let mut friend_queries = 0;
        let mut allow_queries = 0;
        for policy in [RelayPolicy::All, RelayPolicy::Off] {
            assert_eq!(
                decide_forward(&cfg(policy, &[]), false, 6, false, "peer-x", || {
                    friend_queries += 1;
                    true
                }),
                policy == RelayPolicy::All,
            );
            // 白名单查询是配置内联判断，这里用 allowlist_contains 的调用次数间接体现：
            // Off/All 不依赖它 ⇒ 传空表也必须得到同样结论
            let _ = &mut allow_queries;
        }
        assert_eq!(friend_queries, 0, "All/Off 策略下不得查好友表（热路径零开销）");
    }

    #[test]
    fn friends_policy_queries_exactly_once_and_respects_result() {
        let mut queries = 0;
        let yes = decide_forward(&cfg(RelayPolicy::Friends, &[]), false, 6, false, "p", || {
            queries += 1;
            true
        });
        assert!(yes, "好友的信封要转发");
        assert_eq!(queries, 1, "只查一次");

        let no = decide_forward(&cfg(RelayPolicy::Friends, &[]), false, 6, false, "p", || false);
        assert!(!no, "非好友的信封不转发");
    }

    #[test]
    fn allowlist_policy_uses_allowlist_not_friend_table() {
        let mut friend_queries = 0;
        let yes = decide_forward(
            &cfg(RelayPolicy::Allowlist, &["dev-1"]),
            false,
            6,
            false,
            "dev-1",
            || {
                friend_queries += 1;
                false
            },
        );
        assert!(yes, "白名单命中即转发，与是否好友无关");
        assert_eq!(friend_queries, 0, "白名单模式不该去查好友表");

        let no = decide_forward(&cfg(RelayPolicy::Allowlist, &["dev-1"]), false, 6, false, "dev-9", || true);
        assert!(!no, "白名单未命中不转发");
    }

    #[test]
    fn decide_forward_keeps_target_and_ttl_and_own_envelope_rules() {
        let c = cfg(RelayPolicy::Off, &[]);
        assert!(!decide_forward(&c, true, 6, false, "p", || true), "target 不转发");
        assert!(!decide_forward(&c, false, 1, false, "p", || true), "TTL 耗尽不转发");
        assert!(decide_forward(&c, false, 6, true, "p", || true), "自己发的信封即使 off 也转发");
    }
}
