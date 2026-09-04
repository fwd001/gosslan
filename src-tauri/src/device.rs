//! 设备指纹：用机器码（MachineGuid / machine-id / IOPlatformUUID）生成稳定的设备 ID。
//! 同一台电脑重启后仍是同一 ID，从而在“无登录”前提下识别同一用户。

use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 由硬件指纹派生的稳定设备 ID（取哈希前 16 位）。
/// 无法获取机器码时返回 None，由上层回退为持久化的 UUID。
pub fn hardware_fingerprint() -> Option<String> {
    // machine-uid 0.5 不支持 Android（其 `machine_id` 模块无 android 分支），
    // 故该依赖仅对非 Android 目标引入（见 Cargo.toml），Android 上直接返回 None，
    // 由 state.rs 回退到持久化的 device_id / 主机名指纹。
    #[cfg(target_os = "android")]
    {
        return None;
    }

    #[cfg(not(target_os = "android"))]
    {
        let uid = machine_uid::get().ok()?;
        let uid = uid.trim();
        if uid.is_empty() {
            return None;
        }
        let mut h = Sha256::new();
        h.update(b"gosslan-machine:");
        h.update(uid.as_bytes());
        Some(format!("dev-{}", &hex(&h.finalize())[..16]))
    }
}

/// 回退：基于主机名派生（稳定性弱于机器码，仅作兜底）。
pub fn hostname_fingerprint() -> String {
    let host = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut h = Sha256::new();
    h.update(b"gosslan-host:");
    h.update(host.as_bytes());
    format!("dev-{}", &hex(&h.finalize())[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_has_prefix() {
        if let Some(id) = hardware_fingerprint() {
            assert!(id.starts_with("dev-"));
            assert_eq!(id.len(), 20); // "dev-" + 16 hex
        }
    }
}
