//! 默认昵称规则（用户 2026-09-12 要求）。
//!
//! 四条约束：
//! ① **不用设备的用户名/hostname**（那既不好看，也把设备信息写在了公屏上）；
//! ② 一串**英文** + 一小段由**设备标识派生**的短码 —— 长度合适，不改也好看；
//! ③ **稳定**：同一台设备每次启动都是同一个名字（用 `device_id` 派生，不用随机数，
//!    否则每次重启名字都变，好友列表里根本认不出谁是谁）；
//! ④ 让人**有想改的欲望**：形容词 + 动物，好念、好认、有性格。
//!
//! 生成形如 `Swift Otter 4K7` 的名字：`<Adjective> <Animal> <3 位 base36>`。
//! 词表刻意只用 ≤6 个字母的词，名字总长 ≤17 字符（列表里不会被截断成省略号）。
use sha2::{Digest, Sha256};

/// 形容词（全 ≤6 字母）。
pub const ADJECTIVES: &[&str] = &[
    "Brave", "Bright", "Calm", "Clever", "Cosmic", "Daring", "Eager", "Gentle", "Happy", "Keen",
    "Kind", "Lively", "Lucky", "Merry", "Nimble", "Noble", "Proud", "Quiet", "Rapid", "Sharp",
    "Silent", "Swift", "Warm", "Wise",
];

/// 动物（全 ≤6 字母）。
pub const ANIMALS: &[&str] = &[
    "Badger", "Bison", "Condor", "Eagle", "Falcon", "Fox", "Gecko", "Heron", "Ibis", "Koala",
    "Lemur", "Lynx", "Marten", "Otter", "Owl", "Panda", "Puma", "Raven", "Robin", "Seal", "Tapir",
    "Wolf", "Wren", "Yak",
];

/// base36 短码的位数（3 位 = 46656 种，配合 24×24 的词表组合，千级局域网内撞名概率可忽略；
/// 真正的身份始终是 `device_id`，名字只用于display）。
const SUFFIX_LEN: usize = 3;

/// 由 `device_id` **确定性地**生成默认昵称（纯函数，主机可单测）。
pub fn default_nickname(device_id: &str) -> String {
    let h = Sha256::digest(device_id.as_bytes());
    let adj = ADJECTIVES[(h[0] as usize) % ADJECTIVES.len()];
    let animal = ANIMALS[(h[1] as usize) % ANIMALS.len()];
    let n = u16::from_be_bytes([h[2], h[3]]) as u32;
    let suffix = base36(n, SUFFIX_LEN);
    format!("{adj} {animal} {suffix}")
}

/// 把 `n` 转成**固定长度**的 base36（大写、左侧补 0）。
fn base36(mut n: u32, width: usize) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut out = vec![b'0'; width];
    for i in (0..width).rev() {
        out[i] = DIGITS[(n % 36) as usize];
        n /= 36;
    }
    String::from_utf8(out).unwrap_or_else(|_| "000".to_string())
}

/// 这个昵称是不是"旧版默认名"（需要在升级时一次性换成新规则）。
///
/// 认三种：空串、旧的字面默认值、以及**恰好等于本机 hostname** 的值 ——
/// 后者正是旧规则（`hostname::get()`）的产物。用户自己填过的名字一律不动。
pub fn is_legacy_default(name: &str, hostname: &str) -> bool {
    let n = name.trim();
    n.is_empty()
        || n == "Gosslan 用户"
        || n == "Gosslan User"
        || (!hostname.is_empty() && n.eq_ignore_ascii_case(hostname.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_nickname_is_deterministic_and_looks_right() {
        let a = default_nickname("dev-abc-123");
        let b = default_nickname("dev-abc-123");
        assert_eq!(a, b, "同一设备必须每次得到同一个名字（重启后名字不能变）");

        // 形如 `<Adj> <Animal> <3 位 base36>`：三段、纯 ASCII、长度合适
        let parts: Vec<&str> = a.split(' ').collect();
        assert_eq!(parts.len(), 3, "应当是三段：{a}");
        assert!(parts[0].chars().all(|c| c.is_ascii_alphabetic()), "{a}");
        assert!(parts[1].chars().all(|c| c.is_ascii_alphabetic()), "{a}");
        assert_eq!(parts[2].len(), SUFFIX_LEN, "{a}");
        assert!(parts[2].chars().all(|c| c.is_ascii_alphanumeric()), "{a}");
        assert!(a.is_ascii(), "默认名必须是纯 ASCII 英文（{a}）");
        assert!(a.len() <= 17, "太长会在列表里被截断：{a}（{} 字符）", a.len());

        // 词表本身也不能有长词（保证上面那条长度断言不是碰巧成立）
        assert!(ADJECTIVES.iter().all(|w| w.len() <= 6));
        assert!(ANIMALS.iter().all(|w| w.len() <= 6));
    }

    #[test]
    fn different_devices_get_different_suffixes() {
        let ids = ["a", "b", "c", "dev-1", "dev-2", "9f8e7d6c", "phone-1"];
        let names: Vec<String> = ids.iter().map(|i| default_nickname(i)).collect();
        let suffixes: std::collections::HashSet<&str> =
            names.iter().map(|n| n.rsplit(' ').next().unwrap()).collect();
        assert!(
            suffixes.len() >= ids.len() - 1,
            "不同设备的短码应当基本都不同：{names:?}"
        );
    }

    #[test]
    fn base36_is_zero_padded_and_uppercase() {
        assert_eq!(base36(0, 3), "000");
        assert_eq!(base36(35, 3), "00Z");
        assert_eq!(base36(36, 3), "010");
        assert_eq!(base36(46655, 3), "ZZZ");
        // 越界（生产上不会发生：输入是 u16）时取**低位**，长度恒定
        assert_eq!(base36(46656, 3), "000");
        assert_eq!(base36(46657, 3), "001");
    }

    #[test]
    fn legacy_defaults_are_recognised_but_chosen_names_are_not() {
        assert!(is_legacy_default("", "MacBook-Pro"));
        assert!(is_legacy_default("Gosslan 用户", "MacBook-Pro"));
        assert!(is_legacy_default("Gosslan User", "MacBook-Pro"));
        assert!(is_legacy_default("macbook-pro", "MacBook-Pro"));
        assert!(is_legacy_default("  MacBook-Pro  ", "MacBook-Pro"));
        // 用户自己取的名字（哪怕和默认规则很像）绝不能被改
        assert!(!is_legacy_default("Swift Otter 4K7", "MacBook-Pro"));
        assert!(!is_legacy_default("老王的手机", "MacBook-Pro"));
    }
}
