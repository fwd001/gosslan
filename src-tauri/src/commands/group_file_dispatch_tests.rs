// 职责边界：
// - 群文件投递失败后的**终态裁决**（GroupFileSendErr → 该写回哪个 recipient 状态）
// - 以及"留在 sending 就再也进不了重试队列"这件事本身的数据库断言
#[cfg(test)]
mod group_file_dispatch_tests {
    use super::{group_file_status_after_fail, GroupFileSendErr};

    /// ★ 钉的是形状本身：**没有任何一种失败允许把行留在 `sending`** ——
    /// 重试查询（`db::list_pending_group_files_for_recipient`）只捞 `pending`，
    /// 所以留在 `sending` 等于"既不重试，也没人告诉用户它死了"（#154-4 的全部机制）。
    #[test]
    fn no_failure_leaves_the_row_in_sending() {
        let all = [
            GroupFileSendErr::Unrecoverable("群文件记录不存在".into()),
            GroupFileSendErr::Retryable("未建立连接".into()),
            GroupFileSendErr::Cancelled,
        ];
        for e in all.iter() {
            let v = group_file_status_after_fail(e);
            assert_ne!(v, Some("sending"), "{e:?} 之后不许停在 sending");
            assert!(
                matches!(v, None | Some("pending") | Some("failed") | Some("cancelled")),
                "{e:?} 的终态必须是这张表真在用的取值，不许新造词"
            );
        }
    }

    /// 给不了货的那一类（群文件行本身没了 / 源文件不在了）必须判死：
    /// 用户看得见"失败"，面板也不会再去问一个答不出货的人。
    #[test]
    fn unrecoverable_becomes_failed() {
        assert_eq!(
            group_file_status_after_fail(&GroupFileSendErr::Unrecoverable("x".into())),
            Some("failed")
        );
    }

    /// "现在给不了、以后可能给得了"（没链路、密钥还没到、超时、分片写失败）
    /// ⇒ 回到 `pending`，让下一次建链 / Hello / 心跳把它重新拾起来。
    #[test]
    fn retryable_goes_back_to_pending() {
        assert_eq!(
            group_file_status_after_fail(&GroupFileSendErr::Retryable("x".into())),
            Some("pending")
        );
    }

    /// 用户主动取消记 `cancelled`，**不许记 failed**（INV-P26 同一口径：
    /// 取消是用户的动作，自动失败才是 failed）。
    #[test]
    fn cancel_is_recorded_as_cancelled_not_failed() {
        assert_eq!(group_file_status_after_fail(&GroupFileSendErr::Cancelled), Some("cancelled"));
    }

    /// 数据库侧的机制断言：`sending` 的行确实**捞不到**，回到 `pending` 才捞得到
    /// ⇒ 上面那三条裁决不是纸面功夫。
    #[test]
    fn sending_rows_are_invisible_to_the_retry_query_and_pending_ones_are_not() {
        use crate::db::{self, SCHEMA};
        use rusqlite::{params, Connection};
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO group_files(transfer_id, group_id, sender_id, name, size, sha256, status, created_at, scope, todo_id)
             VALUES('gf-stuck','g1','dev-a','a.png',10,'x','sending',0,'chat','')",
            [],
        )
        .unwrap();
        for (recipient, status) in [("dev-b", "sending"), ("dev-c", "pending")] {
            conn.execute(
                "INSERT INTO group_file_recipients(transfer_id, recipient_id, status, progress, updated_at)
                 VALUES('gf-stuck',?1,?2,0.0,0)",
                params![recipient, status],
            )
            .unwrap();
        }
        assert_eq!(
            db::list_pending_group_files_for_recipient(&conn, "dev-b").unwrap().len(),
            0,
            "sending 的行不在重试查询里 —— 这句是「留在 sending 就是死档」的前提"
        );
        assert_eq!(
            db::list_pending_group_files_for_recipient(&conn, "dev-c").unwrap().len(),
            1,
            "同一张表里 pending 的行必须捞得到，否则回到 pending 也没有意义"
        );
        // 裁决真的应用到库上之后，dev-b 就从死档回到可重试
        let v = group_file_status_after_fail(&GroupFileSendErr::Retryable("x".into())).unwrap();
        conn.execute(
            "UPDATE group_file_recipients SET status = ?1 WHERE transfer_id = 'gf-stuck' AND recipient_id = 'dev-b'",
            params![v],
        )
        .unwrap();
        assert_eq!(
            db::list_pending_group_files_for_recipient(&conn, "dev-b").unwrap().len(),
            1,
            "应用裁决之后必须重新可重试（这就是 #154-4 的用户可见修复）"
        );
    }
}
