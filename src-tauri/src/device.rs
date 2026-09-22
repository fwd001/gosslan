//! 设备 ID（界面与日志里那行"设备指纹"）。
//!
//! ## 为什么不是从设备属性算出来的（2026-09-22 真机事故后重写）
//!
//! 旧实现是 `SHA256(机器码)` 或 `SHA256(主机名)`，而且**机器码优先于库里已存的值**。
//! 两台设备因此会拿到同一个 ID，现场是"局域网里互相打架"：`peers` / `links` / `friends` /
//! `conv_id` 全按 `device_id` 索引 ⇒ 两个节点互相覆盖对方的条目；Hello 是
//! "`device_id` → 绑定 Ed25519"，于是第二台来连就是一次 INV-P11 的**密钥冲突 ⇒ 硬拒**；
//! 而「大 id 主动拨、小 id 只接受」的镜像规则在 id **相等**时直接退化。撞号的来源有两类，
//! 都不是"熵不够"：① 克隆/未 sysprep 的 Windows 镜像、VM 模板、迁移助理保下来的
//! `IOPlatformUUID` ⇒ 机器码相同；② 安卓走主机名兜底，而两台新机的默认主机名常常一样。
//!
//! ## 现在的规则
//!
//! `id = gosslan- + hex( SHA256( 16 字节首启随机 ‖ 设备属性快照 ) )[..13]`（共 21 字符）
//!
//! - **唯一性由那次随机数保证**：属性全一样也不撞（克隆镜像正是属性全一样的场景）；
//! - **属性参与计算**（按用户要求）：机器码 / 主机名 / 网卡名，作为熵与"同镜像可识别"的线索；
//! - **稳定性由"只认持久化值"保证**：只在第一次启动生成一次，此后绝不重新派生 ——
//!   换网卡、开随机 MAC、升级系统都不会悄悄改掉身份。
//!
//! MAC 与蓝牙地址**刻意不进哈希**：`if-addrs` 不跨平台提供 MAC（要写 Win/BSD/Linux 三套
//! 系统调用 + Android JNI），而 Android/iOS/Win11 默认开 MAC 随机化 ⇒ 它加不了唯一性，
//! 只会加不稳定与"换网络就变身份"。将来要加也只是往属性快照里添一项。
//!
//! 长度与形状（21 字符 = `gosslan-` + 13 位小写 hex）保持稳定：`nickname.rs` 由 id 派生默认昵称、
//! 镜像规则要 ASCII 可排序、UI 与日志宽度都按这个形状写着。**已装设备保持它原来的 24 字符**
//! （只认持久化值），所以系统里长短两种 id 会并存 —— 没有任何一处按等长假设写过。

use rand_core::RngCore;
use sha2::{Digest, Sha256};

/// ID 前缀。全链路只有这一个前缀（多套一层会把排序压成"恒最小 id"，见 `strip_legacy_dev_prefix`）。
pub const DEVICE_ID_PREFIX: &str = "gosslan-";
/// 前缀之后的十六进制位数。
///
/// **13 位 = 52 bit**（用户 2026-09-22 定的形状：整机 21 字符，自定义后缀上限 +3 正好回到 24）。
/// 比旧的 16 位少 12 bit，这是**明知故犯**：唯一性本来就由首启那 16 字节随机数提供，
/// 52 bit 在"百万台设备"量级下的生日碰撞概率仍在 1e-4 以下，而这个产品的对手不是注册机。
/// 换来的是界面与日志里那一行短三个字符，以及"默认长度 + 自定义 3 位"这条规格自洽。
pub const DEVICE_ID_HEX_LEN: usize = 13;
/// 整机 ID 的固定长度 = 前缀 + hex，**只在测试里当形状判据用**。
///
/// 刻意写成 `#[cfg(test)]` 而不是 `pub`：`cargo clippy -- -D warnings` 只编 lib（不带
/// `--tests`，见 `.github/workflows/verify.yml` 的 rust 组），一个只给测试用的 `pub` 常量
/// 在生产构建里就是 dead code ⇒ 直接把 CI 打红。上一条 4.25.1 修的就是同一形状的坑。
#[cfg(test)]
pub(crate) const DEVICE_ID_LEN: usize = DEVICE_ID_PREFIX.len() + DEVICE_ID_HEX_LEN;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 参与派生的设备属性快照。
///
/// ⚠️ 这些只是**熵的来源**，不是唯一性的来源：拿不到（容器、移动端、权限受限）就留空，
/// 生成的 ID 依然随机、依然唯一。
pub struct DeviceAttrs {
    pub machine_uid: Option<String>,
    pub hostname: String,
    /// 网卡名（排序后），不含地址
    pub interfaces: Vec<String>,
}

/// 尽力采集设备属性；任何一项失败都只是少一项，不影响生成。
pub fn collect_device_attrs() -> DeviceAttrs {
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let interfaces = if_addrs::get_if_addrs()
        .map(|list| {
            let mut names: Vec<String> = list.iter().map(|i| i.name.clone()).collect();
            names.sort();
            names.dedup();
            names
        })
        .unwrap_or_default();
    DeviceAttrs {
        machine_uid: machine_uid_opt(),
        hostname,
        interfaces,
    }
}

/// 机器码（Windows `MachineGuid` / Linux `machine-id` / macOS `IOPlatformUUID`）。
///
/// 移动端上游 `machine-uid` 没有 android/ios 分支 ⇒ 该依赖只对非移动目标引入，这里整体返回
/// `None`，让移动设备只靠"随机数 + 主机名 + 网卡名"派生。
fn machine_uid_opt() -> Option<String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        None
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        machine_uid::get()
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

/// 生成设备 ID：首启随机数主导，设备属性混入。
pub fn generate_device_id(attrs: &DeviceAttrs) -> String {
    let mut nonce = [0u8; 16];
    rand_core::OsRng.fill_bytes(&mut nonce);
    let mut h = Sha256::new();
    h.update(b"gosslan-device-v2:");
    h.update(nonce);
    for field in attrs.machine_uid.iter().map(|s| s.as_str()) {
        h.update(b"m");
        h.update(field.as_bytes());
        h.update([0u8]);
    }
    h.update(b"h");
    h.update(attrs.hostname.as_bytes());
    h.update([0u8]);
    for name in &attrs.interfaces {
        h.update(b"i");
        h.update(name.as_bytes());
        h.update([0u8]);
    }
    format!(
        "{DEVICE_ID_PREFIX}{}",
        &hex(&h.finalize())[..DEVICE_ID_HEX_LEN]
    )
}

/// 剥掉历史遗留的 `dev-` 前缀（返回 `None` = 不需要迁移）。
///
/// 为什么需要它（2026-09-13 合并评审）：`state.rs` 的兜底路径曾经写成
/// `format!("dev-{}", hostname_fingerprint())`，而 `hostname_fingerprint()` 本身已带
/// `gosslan-` 前缀 ⇒ 已装的安卓库里存的是 **`dev-gosslan-…`**。
/// 改成不再套前缀只治"新装设备"；**已经装上并写过库的设备**必须就地迁移，
/// 否则它仍然是"三端里恒最小 id、永远不主动拨号"的那一个
/// （现象：它只能等别人连它，自己永远搜不到、也拨不动）。
///
/// 只剥这一个已知前缀，不做任何其它"美化"（id 是身份，不能顺手改）。
pub fn strip_legacy_dev_prefix(id: &str) -> Option<&str> {
    let rest = id.strip_prefix("dev-")?;
    // 只处理"套在合法指纹外面"的那一层：剥完必须还是一个合法指纹，
    // 否则宁可不动（例如用户/测试库里恰好有个叫 `dev-xxx` 的 id）。
    rest.starts_with("gosslan-").then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 形状与前缀不许变（2026-09-13 那轮的教训还在）：多套一层 `dev-` 会让 id 在 ASCII 排序里
    /// 恒为最小，而镜像规则是「大 id 拨、小 id 只接受」⇒ 那台设备永远不主动拨任何人。
    #[test]
    fn generated_id_keeps_the_one_prefix_and_length() {
        let id = generate_device_id(&collect_device_attrs());
        assert!(
            id.starts_with(DEVICE_ID_PREFIX),
            "必须有且只有一个 gosslan- 前缀，实际：{id}"
        );
        assert!(
            !id.starts_with("dev-"),
            "不许再套 dev- 前缀（会让排序恒最小、永远不主动拨号）：{id}"
        );
        assert_eq!(
            id.len(),
            DEVICE_ID_LEN,
            "长度必须固定：昵称由 id 派生、UI 与日志都按这个形状写着：{id}"
        );
        // 这个字面量是**故意的**，不许换成常量：换成常量后"把 hex 位数从 13 改到 8"
        // 就再也报不出红（上面那条断言会跟着一起变绿）。形状变更必须显式改这里。
        assert_eq!(
            DEVICE_ID_LEN, 21,
            "整机 id 长度是 21 字符（用户 2026-09-22 定的规格）"
        );
        let body = id.strip_prefix(DEVICE_ID_PREFIX).unwrap();
        assert!(
            body.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "十六进制部分必须是小写 hex，且只含 hex：{body}"
        );
    }

    /// **本次事故的正向回归**：两台克隆出来的设备（机器码、主机名、网卡名全都一样）必须拿到
    /// 不同的 ID。旧实现恰恰在这里 100% 撞 —— `SHA256(机器码)` 是确定的。
    #[test]
    fn identical_attributes_still_produce_different_ids() {
        let attrs = DeviceAttrs {
            machine_uid: Some("the-cloned-image-guid".into()),
            hostname: "DESKTOP-SAME".into(),
            interfaces: vec!["eth0".into(), "wlan0".into()],
        };
        // 64 bit 随机下 200 次里撞一次的概率约 1e-12 —— 这条不是碰运气的摆设。
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            assert!(
                seen.insert(generate_device_id(&attrs)),
                "属性完全相同时撞了 ⇒ 唯一性又去依赖设备属性了"
            );
        }
    }

    /// 属性拿不到（容器、移动端、权限受限）不是失败：留空也要能生成合法且互不相同的 ID。
    #[test]
    fn empty_attributes_still_yield_valid_distinct_ids() {
        let empty = DeviceAttrs {
            machine_uid: None,
            hostname: String::new(),
            interfaces: Vec::new(),
        };
        let a = generate_device_id(&empty);
        let b = generate_device_id(&empty);
        assert!(
            a.starts_with(DEVICE_ID_PREFIX) && a.len() == DEVICE_ID_LEN,
            "{a}"
        );
        assert_ne!(a, b, "属性全空时也必须靠随机数分开（否则移动端回到撞号）");
    }

    /// 历史 `dev-` 前缀必须能被就地剥掉（否则已装设备永远是最小 id）。
    #[test]
    fn legacy_dev_prefix_is_stripped_exactly_once() {
        assert_eq!(
            strip_legacy_dev_prefix("dev-gosslan-f3d6b7dddf73aab2"),
            Some("gosslan-f3d6b7dddf73aab2"),
            "带 dev- 的老 id 必须迁移成 gosslan-…"
        );
        // 正常 id：不动
        assert_eq!(strip_legacy_dev_prefix("gosslan-f3d6b7dddf73aab2"), None);
        // 只剥**一层**，且剥完必须是合法指纹：不合法就不动（别把用户自己的 id 改坏）
        assert_eq!(strip_legacy_dev_prefix("dev-dev-gosslan-abc"), None);
        assert_eq!(strip_legacy_dev_prefix("dev-whatever"), None);
        assert_eq!(strip_legacy_dev_prefix(""), None);
    }
}
