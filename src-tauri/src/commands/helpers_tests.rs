// 职责边界：
// - peer 投递槽位闸门（mark_peer_sending / release_peer_send / PeerSendGuard）的行为断言
// - 源码守卫：两条投递循环都必须走这个家，不许再把 remove 手写在任务体末尾
#[cfg(test)]
mod helpers_gate_tests {
    use super::{mark_peer_sending, release_peer_send, PeerSendGuard};
    use std::sync::Mutex;

    type Gate = Mutex<std::collections::HashSet<String>>;

    fn empty_gate() -> Gate {
        Mutex::new(std::collections::HashSet::new())
    }

    /// 抢占的互斥面：同一 peer 第二次必须抢不到（这就是"串行"的全部含义），
    /// 不同 peer 之间必须并行（闸门的键是 peer，不是全局）。
    #[test]
    fn second_claim_for_the_same_peer_is_refused() {
        let set = empty_gate();
        assert!(
            mark_peer_sending(&set, "peer-a"),
            "空闲时必须抢得到 peer-a"
        );
        assert!(
            !mark_peer_sending(&set, "peer-a"),
            "同一个 peer 不能同时跑两个投递任务"
        );
        assert!(
            mark_peer_sending(&set, "peer-b"),
            "不同 peer 之间必须并行"
        );
    }

    /// 正常离开作用域要归还，否则下一次连接事件永远抢不到 ⇒ 文件永远卡在「发送中 0%」。
    #[test]
    fn guard_releases_when_the_scope_ends() {
        let set = empty_gate();
        assert!(mark_peer_sending(&set, "peer-a"));
        {
            let _guard = PeerSendGuard::new(&set, "peer-a");
            assert!(!mark_peer_sending(&set, "peer-a"), "持有期间不许被抢走");
        }
        assert!(
            mark_peer_sending(&set, "peer-a"),
            "守卫离开作用域后必须能再次抢占"
        );
    }

    /// ★ 这条才是本轮修的东西：投递体里一次 panic 之后槽位必须跟着展开一起归还。
    /// 旧写法把 `remove` 手写在 spawn 体的**末尾**，panic 会跳过它 ⇒
    /// 这个 peer 之后所有文件永远发不出去，而且不报任何错。
    #[test]
    fn guard_releases_when_the_body_panics() {
        let set = empty_gate();
        assert!(mark_peer_sending(&set, "peer-a"));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = PeerSendGuard::new(&set, "peer-a");
            panic!("投递体里的一次意外崩溃");
        }));
        assert!(r.is_err(), "夹具必须真的 panic，否则这条断言是空转的");
        assert!(
            mark_peer_sending(&set, "peer-a"),
            "panic 展开后槽位必须已归还，否则这个 peer 之后所有文件永远发不出去"
        );
    }

    /// 锁中毒（别的任务持锁时 panic）不许让闸门卡死：取与放两侧都抗中毒。
    #[test]
    fn guard_releases_after_the_lock_is_poisoned() {
        let set = empty_gate();
        assert!(mark_peer_sending(&set, "peer-a"));
        let guard = PeerSendGuard::new(&set, "peer-a");
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = set.lock().unwrap(); // 持锁时 panic ⇒ 锁中毒
            panic!("持锁时崩");
        }));
        assert!(poisoned.is_err(), "夹具必须真的把锁弄中毒");
        drop(guard);
        assert!(mark_peer_sending(&set, "peer-a"), "中毒锁也要能取放槽位");
    }

    /// 手动归还仍是早退路径的出口之一，但它和 Drop 走的是同一个家。
    #[test]
    fn release_helper_uses_the_same_set() {
        let set = empty_gate();
        assert!(mark_peer_sending(&set, "peer-a"));
        release_peer_send(&set, "peer-a");
        assert!(mark_peer_sending(&set, "peer-a"));
    }

    // ---------------- 源码守卫 ----------------

    /// 两条投递循环（1:1 与群文件）都必须把槽位交给 `PeerSendGuard`。
    ///
    /// 这条是"同一个开关的另一半会继续漏"的钉子：本轮只修 1:1 那条是不够的 ——
    /// 群文件那条是逐字复制出来的同一个形状，两个都得走这个家。
    /// 反向钉子：任务体末尾手写 `remove` 的那种写法不许回来（它挡不住 panic）。
    #[test]
    fn both_dispatch_loops_use_the_guard() {
        let direct = include_str!("files.rs");
        let group = include_str!("group_file_keys.rs");
        for (name, src, field) in [
            ("files.rs", direct, "file_sending"),
            ("group_file_keys.rs", group, "group_file_sending"),
        ] {
            assert!(
                src.contains(&format!("PeerSendGuard::new(&st.{field}, &peer)")),
                "{name} 的投递任务必须用 PeerSendGuard 归还槽位（panic 也要收得回）"
            );
            assert!(
                src.contains("mark_peer_sending("),
                "{name} 的抢占必须走 helpers 的那一个家，不许再手写 insert"
            );
            assert!(
                !src.contains("remove(peer_id)") && !src.contains("remove(&peer)"),
                "{name} 里不许再手写归还槽位（早退走 release_peer_send，任务体走守卫的 Drop）"
            );
        }
    }
}
