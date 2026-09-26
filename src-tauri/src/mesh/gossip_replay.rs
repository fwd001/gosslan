//! 替**别人**转发过的群信封的一份有界缓存 —— 为的是 #77：晚加入群的成员
//! 能拿到"我不在时别人托我转发的历史消息"。
//!
//! 为什么需要它（而不是"加个 if"）：`handle_gossip` 把信封扇给的是**收到那一瞬间**
//! 的 `reachable_neighbors`，转发过的信封本身不留存 ⇒ 那一瞬间还没建链的成员
//! 永远补不到（真机形状：A 与 C 都连着 B，C 掉线期间 A 发的群消息，C 重连后看不到）。
//!
//! 语义边界（必须写清，不许读成"群消息最终一定一致"）：
//! - 缓存是**进程内**的 ⇒ 中间人重启后那一段仍然补不到；
//! - 每组只留最近 `PER_GROUP_LIMIT` 条、只留 `RELAY_WINDOW_MS` 窗口内的 ⇒
//!   掉线超过窗口或消息数超过上限的成员拿到的是**部分**历史；
//! - 只记 `kind == Group` 且成员表非空的信封：成员表为空是旧端信封
//!   （口径同 `network::transport::gossip::group_envelope_consumable`），
//!   没有成员表就无从判别该补递给谁。
//!
//! 为什么窗口量的是**本机收到的时刻**而不是 `env.ts`：`ts` 由发送方时钟写，
//! 跨设备不可信（快钟会让记录永不生效，慢钟会让记录秒过期）；补递策略是
//! 本机的局部决定，用本机的单调读到的时间才与"我还记不记得它"这件事对齐。

use std::collections::HashMap;

use crate::protocol::{GossipEnvelope, GossipKind};

/// 本机现在的毫秒时间（窗口用它量，不用 `env.ts` —— 见文件头注释）。
/// 放在这里而不是拉 `chrono`：这个模块的判据全部是纯函数 + 显式传时刻。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// 每个群保留的条数上限（超出时丢最旧的）。
pub const PER_GROUP_LIMIT: usize = 16;
/// 一条转发记录的可补递窗口（毫秒）。
pub const RELAY_WINDOW_MS: i64 = 10 * 60 * 1000;
/// 补递还要**再吃掉一跳**（对端收到后 `ttl` 归零就不再往下转），
/// 所以 `ttl < 2` 的信封补了也白补 —— 与 `decide_forward` 的"TTL 耗尽不转发"同口径。
pub const MIN_TTL_FOR_RELAY: u8 = 2;

#[derive(Clone, Debug)]
struct Entry {
    env: GossipEnvelope,
    /// 本机收到并转发的时刻（毫秒，本地时钟）
    seen_at: i64,
}

/// 进程内有界的"已转发群信封"缓存。
#[derive(Default)]
pub struct GossipRelayCache {
    /// group_id → 记录，**新在前**（`eligible` 直接按这个顺序补递）
    by_group: HashMap<String, Vec<Entry>>,
}

impl GossipRelayCache {
    /// 记下一条刚转发出去的信封；返回是否真的记了（判据要能区分"没记"与"记了但递不到"）。
    pub fn remember(&mut self, env: &GossipEnvelope, now_ms: i64) -> bool {
        if !recordable(env) {
            return false;
        }
        let group = env.group_id.clone().unwrap_or_default();
        let bucket = self.by_group.entry(group).or_default();
        // 同一条从两条链路各收到一次（接收侧本就按 message_id 去重）⇒ 只留一份，
        // 否则补递会把同一条原样发两遍。
        if bucket.iter().any(|e| e.env.message_id == env.message_id) {
            return true;
        }
        bucket.insert(
            0,
            Entry {
                env: env.clone(),
                seen_at: now_ms,
            },
        );
        // 先按窗口清，再按条数裁 —— 顺序反了会让"新来的"把"还能递的旧的"挤掉。
        bucket.retain(|e| in_window(e.seen_at, now_ms));
        bucket.truncate(PER_GROUP_LIMIT);
        true
    }

    /// 当前对 `peer` 可补递的信封（跨所有群，新在前）。
    pub fn eligible(&self, peer: &str, me: &str, now_ms: i64) -> Vec<GossipEnvelope> {
        let mut out = Vec::new();
        for bucket in self.by_group.values() {
            for e in bucket {
                if worth_replaying(&e.env, e.seen_at, peer, me, now_ms) {
                    out.push(e.env.clone());
                }
            }
        }
        // 分桶遍历的顺序不稳定，而补递顺序要稳定（新在前）：按 seen_at 倒序排。
        out.sort_by(|a, b| b.ts.cmp(&a.ts));
        out
    }

    /// 缓存里还剩多少条（判据用它证明"过窗口的被物理清掉"而不是只被过滤）
    pub fn len(&self) -> usize {
        self.by_group.values().map(|v| v.len()).sum()
    }
}

/// 这条信封有没有补递语义（值不值得占缓存的一格）。
fn recordable(env: &GossipEnvelope) -> bool {
    env.kind == GossipKind::Group
        && env.group_id.as_deref().is_some_and(|g| !g.is_empty())
        && !env.group_members.is_empty()
}

/// 窗口判定：本机记下它还没过 `RELAY_WINDOW_MS`。
fn in_window(seen_at: i64, now_ms: i64) -> bool {
    now_ms - seen_at < RELAY_WINDOW_MS
}

/// 这条信封此刻该不该补递给 `peer`（纯函数，真值表钉死）。
pub fn worth_replaying(
    env: &GossipEnvelope,
    seen_at: i64,
    peer: &str,
    me: &str,
    now_ms: i64,
) -> bool {
    if !recordable(env) {
        return false;
    }
    // 补递还要再吃掉一跳
    if env.ttl < MIN_TTL_FOR_RELAY {
        return false;
    }
    if !in_window(seen_at, now_ms) {
        return false;
    }
    // 发送方本机早就有、本机自己就是收件人 —— 都不必递
    if peer == env.sender_id || peer == me {
        return false;
    }
    env.group_members.iter().any(|m| m == peer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GossipKind;

    fn frame(
        kind: GossipKind,
        group: Option<&str>,
        members: &[&str],
        sender: &str,
        ttl: u8,
    ) -> GossipEnvelope {
        GossipEnvelope {
            message_id: format!("m-{}", members.len()),
            sender_id: sender.to_string(),
            nonce: "n".to_string(),
            sender_pubkey: "p".to_string(),
            sender_ed25519: "e".to_string(),
            sender_sig: "s".to_string(),
            ttl,
            kind,
            group_id: group.map(str::to_string),
            group_name: Some("g".to_string()),
            group_creator: Some("a".to_string()),
            group_members: members.iter().map(|m| m.to_string()).collect(),
            payload: "ciphertext".to_string(),
            ts: 1_700_000_000_000,
            seq: 1,
            encrypted: true,
            target: None,
        }
    }

    /// 具名 id：a 发送、b 是中间人（本机）、c 是晚到的成员
    fn group_frame(ttl: u8) -> GossipEnvelope {
        frame(GossipKind::Group, Some("g1"), &["a", "b", "c"], "a", ttl)
    }

    #[test]
    fn worth_replaying_requires_ttl_to_survive_one_more_hop() {
        let now = 1_000_000_000_i64;
        assert!(worth_replaying(&group_frame(2), now, "c", "b", now));
        assert!(worth_replaying(&group_frame(7), now, "c", "b", now));
        // ttl=1 ⇒ 补递这一跳就把它吃光，对端再也转不出去
        assert!(!worth_replaying(&group_frame(1), now, "c", "b", now));
        assert!(!worth_replaying(&group_frame(0), now, "c", "b", now));
    }

    #[test]
    fn worth_replaying_requires_the_peer_to_be_a_member() {
        let now = 1_000_000_000_i64;
        assert!(worth_replaying(&group_frame(5), now, "c", "b", now));
        // 不在成员表里 ⇒ 递过去它也解不开（群密钥），还白白占一次链路写
        assert!(!worth_replaying(&group_frame(5), now, "d", "b", now));
    }

    #[test]
    fn worth_replaying_never_replays_to_the_sender_or_to_me() {
        let now = 1_000_000_000_i64;
        let f = group_frame(5);
        assert!(!worth_replaying(&f, now, "a", "b", now), "发送方本来就有");
        assert!(!worth_replaying(&f, now, "b", "b", now), "不能递给自己");
    }

    #[test]
    fn worth_replaying_drops_frames_outside_the_window() {
        let now = 1_000_000_000_i64;
        let f = group_frame(5);
        assert!(worth_replaying(
            &f,
            now - (RELAY_WINDOW_MS - 1),
            "c",
            "b",
            now
        ));
        assert!(
            !worth_replaying(&f, now - RELAY_WINDOW_MS, "c", "b", now),
            "刚好到窗口边界就不许再递 —— 窗口是这条缓存唯一的'遗忘'机制"
        );
        assert!(!worth_replaying(
            &f,
            now - RELAY_WINDOW_MS - 1,
            "c",
            "b",
            now
        ));
    }

    #[test]
    fn remember_skips_non_group_and_legacy_frames() {
        let now = 1_000_000_000_i64;
        let mut c = GossipRelayCache::default();
        // 单聊 / Presence / 好友申请这些不是群消息，缓存了也没有补递语义
        assert!(!c.remember(&frame(GossipKind::Chat, None, &[], "a", 5), now));
        assert!(!c.remember(&frame(GossipKind::Presence, None, &[], "a", 5), now));
        // 成员表为空的群信封 = 旧端，无法判别该递给谁
        assert!(!c.remember(&frame(GossipKind::Group, Some("g1"), &[], "a", 5), now));
        // 没有群 id 的同样不记（按 group_id 分桶，没有桶就进不了任何桶）
        assert!(!c.remember(
            &frame(GossipKind::Group, None, &["a", "b", "c"], "a", 5),
            now
        ));
        assert_eq!(c.eligible("c", "b", now).len(), 0);
        // 正常的才记
        assert!(c.remember(&group_frame(5), now));
    }

    #[test]
    fn remembered_frame_reaches_the_late_joiner_and_nobody_else() {
        let now = 1_000_000_000_i64;
        let mut c = GossipRelayCache::default();
        assert!(c.remember(&group_frame(5), now));
        let got = c.eligible("c", "b", now);
        assert_eq!(got.len(), 1, "晚到的 c 应当拿到那一条");
        assert_eq!(got[0].message_id, group_frame(5).message_id);
        assert_eq!(c.eligible("a", "b", now).len(), 0, "发送方不用补");
        assert_eq!(c.eligible("b", "b", now).len(), 0, "本机不用补");
        assert_eq!(c.eligible("d", "b", now).len(), 0, "非成员不用补");
    }

    #[test]
    fn same_message_id_arriving_twice_is_stored_once() {
        // 中间人从两条链路各收到同一条（接收侧按 message_id 去重，缓存同理），
        // 否则补递会把同一条重复发两遍。
        let now = 1_000_000_000_i64;
        let mut c = GossipRelayCache::default();
        let f = group_frame(5);
        assert!(c.remember(&f, now));
        assert!(c.remember(&f, now + 10));
        assert_eq!(c.eligible("c", "b", now + 20).len(), 1);
    }

    #[test]
    fn per_group_limit_keeps_the_newest_and_drops_the_oldest() {
        let now = 1_000_000_000_i64;
        let mut c = GossipRelayCache::default();
        let total = PER_GROUP_LIMIT + 4;
        for i in 0..total {
            let mut f = group_frame(5);
            // message_id 要各不相同，否则会被去重成一条
            f.message_id = format!("m{i}");
            assert!(c.remember(&f, now + i as i64));
        }
        let got = c.eligible("c", "b", now);
        assert_eq!(
            got.len(),
            PER_GROUP_LIMIT,
            "每组最多留 {PER_GROUP_LIMIT} 条"
        );
        assert_eq!(got[0].message_id, format!("m{}", total - 1), "新的在前");
        assert_eq!(
            got.last().unwrap().message_id,
            format!("m{}", total - PER_GROUP_LIMIT),
            "丢的必须是最旧的那批，不能随机丢"
        );
    }

    #[test]
    fn expired_frames_are_dropped_from_the_cache_not_just_filtered() {
        let now = 1_000_000_000_i64;
        let mut c = GossipRelayCache::default();
        let mut f = group_frame(5);
        f.message_id = "old".to_string();
        assert!(c.remember(&f, now - RELAY_WINDOW_MS - 1));
        assert_eq!(c.eligible("c", "b", now).len(), 0, "过窗口的不许补递");
        // 再记一条新的 ⇒ 顺带把过窗口的清掉（否则缓存只增不减，是内存泄漏）
        let mut g = group_frame(5);
        g.message_id = "new".to_string();
        assert!(c.remember(&g, now));
        let kept = c.eligible("c", "b", now);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].message_id, "new");
        assert_eq!(c.len(), 1, "过窗口的那条要被物理清掉");
    }
}
