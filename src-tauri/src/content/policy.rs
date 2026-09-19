//! 传输状态机与重试策略（**纯函数**，无 IO）。
//!
//! 单独成文件的原因：这两件事是整套可靠性设计的"判据"，必须能脱离网络/DB 单测，
//! 也便于 ADR-0010 的失败注入（断片、超时、校验不符）逐条钉死。

use crate::content::model::{FailReason, TransferStatus};

/// 首次重试等待（ms）。
pub const RETRY_BASE_MS: i64 = 2_000;
/// 退避封顶（ms）。
pub const RETRY_MAX_MS: i64 = 60_000;
/// 最大自动重试次数：超过后标记 Rejected，不再自动重试。
/// 退避序列 (2,4,8,16,32,60,60,60)s × 8 ≈ 前 8 次累计 ≈ 4 分钟后彻底放弃。
pub const MAX_CONTENT_RETRIES: u32 = 8;

/// 第 attempts 次失败后，距下次重试的间隔：
/// attempts==0 ⇒ 0（首次立即）；1 ⇒ base；2 ⇒ 2*base … 封顶 max。
pub fn backoff_ms(attempts: u32) -> i64 {
    if attempts == 0 {
        return 0;
    }
    let shift = (attempts - 1).min(5);
    RETRY_BASE_MS
        .saturating_mul(1i64 << shift)
        .min(RETRY_MAX_MS)
}

/// 断点续传的起点：把"已收到的字节数"折算成**分片序号**（向下取整到分片边界）。
///
/// 只能从分片边界续传 —— 尾部若有一个半片，它对应不上任何完整分片的 seq，
/// 必须丢弃并重传整片（否则 hasher 与 seq 会对不齐，最终 SHA 必错）。
pub fn resume_from_seq(received_bytes: u64, chunk_size: u64) -> u32 {
    if chunk_size == 0 {
        return 0;
    }
    (received_bytes / chunk_size).min(u32::MAX as u64) as u32
}

/// 失败 ⇒ 下一个状态：可恢复 ⇒ Incomplete，否则 ⇒ Rejected。
pub fn status_after_failure(reason: FailReason) -> TransferStatus {
    if reason.retryable() {
        TransferStatus::Incomplete
    } else {
        TransferStatus::Rejected
    }
}

/// 状态机允许的跃迁。
///
/// 只允许"向前"或"恢复性回退"：Complete / Rejected 是终态，不再出去；
/// Incomplete 可被重试重新拉回 Active。
pub fn can_transition(from: TransferStatus, to: TransferStatus) -> bool {
    use TransferStatus::*;
    if from == to {
        return true;
    }
    match (from, to) {
        (Complete, _) | (Rejected, _) => false,
        (Queued, Active) | (Queued, Incomplete) | (Queued, Rejected) => true,
        (Active, Verifying) | (Active, Complete) | (Active, Incomplete) | (Active, Rejected) => {
            true
        }
        (Verifying, Complete) | (Verifying, Incomplete) | (Verifying, Rejected) => true,
        (Incomplete, Queued) | (Incomplete, Active) | (Incomplete, Rejected) => true,
        _ => false,
    }
}

/// 现在是否该自动重试（到点了且状态可恢复）。
pub fn should_retry_now(status: TransferStatus, now_ms: i64, next_attempt_at: i64) -> bool {
    status == TransferStatus::Incomplete && now_ms >= next_attempt_at
}

/// 记一次失败后的 (新状态, 新 attempts, next_attempt_at)。
///
/// 自动重试有封顶（`MAX_CONTENT_RETRIES`）：可恢复的失败超过上限后按 Rejected 收口。
/// 没有这道闸，一块永远取不到的内容（对端重装 / 文件已删）会以 60s 周期无限重试，
/// 在 Android 后台表现为持续的蓝牙/网络唤醒（真机 80 张图并发场景下放大为耗电与 OOM 风险）。
pub fn on_failure(attempts: u32, reason: FailReason, now_ms: i64) -> (TransferStatus, u32, i64) {
    let next_attempts = attempts.saturating_add(1);
    let status = if next_attempts >= MAX_CONTENT_RETRIES {
        TransferStatus::Rejected
    } else {
        status_after_failure(reason)
    };
    let next_at = if status == TransferStatus::Incomplete {
        now_ms.saturating_add(backoff_ms(next_attempts))
    } else {
        0
    };
    (status, next_attempts, next_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_monotonic_and_capped() {
        assert_eq!(backoff_ms(0), 0, "首次重试立即，不退避");
        assert_eq!(backoff_ms(1), RETRY_BASE_MS);
        assert_eq!(backoff_ms(2), RETRY_BASE_MS * 2);
        let mut last = -1;
        for a in 0..20u32 {
            let d = backoff_ms(a);
            assert!(d >= last, "退避必须单调不减（a={a}）");
            assert!(d <= RETRY_MAX_MS, "退避必须封顶（a={a} d={d}）");
            last = d;
        }
        assert_eq!(backoff_ms(100), RETRY_MAX_MS);
    }

    #[test]
    fn terminal_states_never_transition_out() {
        // 终态不能去**别的**状态；同态是幂等更新（可重复落库），允许。
        for to in [
            TransferStatus::Queued,
            TransferStatus::Active,
            TransferStatus::Verifying,
            TransferStatus::Incomplete,
        ] {
            assert!(!can_transition(TransferStatus::Complete, to));
            assert!(!can_transition(TransferStatus::Rejected, to));
        }
        assert!(can_transition(
            TransferStatus::Complete,
            TransferStatus::Complete
        ));
        assert!(can_transition(
            TransferStatus::Rejected,
            TransferStatus::Rejected
        ));
        assert!(can_transition(
            TransferStatus::Incomplete,
            TransferStatus::Active
        ));
        assert!(can_transition(
            TransferStatus::Active,
            TransferStatus::Incomplete
        ));
        assert!(!can_transition(
            TransferStatus::Queued,
            TransferStatus::Verifying
        ));
    }

    #[test]
    fn resume_only_from_chunk_boundaries() {
        assert_eq!(resume_from_seq(0, 256), 0);
        assert_eq!(resume_from_seq(256, 256), 1);
        assert_eq!(resume_from_seq(300, 256), 1, "尾部半片必须丢弃（向下取整）");
        assert_eq!(resume_from_seq(512, 256), 2);
        assert_eq!(
            resume_from_seq(999, 0),
            0,
            "非法分片大小回退到 0（整份重来）"
        );
    }

    #[test]
    fn failure_maps_to_resumable_or_terminal() {
        let (s, a, _) = on_failure(0, FailReason::LinkDown, 1_000);
        assert_eq!(s, TransferStatus::Incomplete);
        assert_eq!(a, 1);
        let (s2, _, next_at) = on_failure(0, FailReason::Timeout, 1_000);
        assert_eq!(s2, TransferStatus::Incomplete);
        assert_eq!(next_at, 1_000 + RETRY_BASE_MS);
        let (s3, _, next_at3) = on_failure(0, FailReason::HashMismatch, 1_000);
        assert_eq!(s3, TransferStatus::Rejected);
        assert_eq!(next_at3, 0, "终态不应有下次重试时间");
        assert!(should_retry_now(
            TransferStatus::Incomplete,
            next_at,
            next_at
        ));
    }

    /// 自动重试必须封顶：第 `MAX_CONTENT_RETRIES` 次失败后，可恢复的失败也收口为 Rejected。
    #[test]
    fn retry_cap_turns_resumable_failure_terminal() {
        let (s, a, next_at) = on_failure(MAX_CONTENT_RETRIES - 1, FailReason::LinkDown, 1_000);
        assert_eq!(a, MAX_CONTENT_RETRIES);
        assert_eq!(
            s,
            TransferStatus::Rejected,
            "到达重试上限后，链路类可恢复失败也必须终止自动重试"
        );
        assert_eq!(next_at, 0, "终态不带下次重试时间 ⇒ should_retry_now 永假");
        // 上限之前一步仍可续
        let (s_prev, _, _) = on_failure(MAX_CONTENT_RETRIES - 2, FailReason::LinkDown, 1_000);
        assert_eq!(s_prev, TransferStatus::Incomplete);
        assert!(!should_retry_now(
            TransferStatus::Incomplete,
            next_at - 1,
            next_at
        ));
        assert!(!should_retry_now(
            TransferStatus::Active,
            next_at + 1,
            next_at
        ));
    }
}
