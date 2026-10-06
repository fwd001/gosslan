// 职责边界：
// - transport 这组行为测试的**模块壳**：只剩 `#[cfg(test)] mod tests`、`use` 和 15 行 `include!`
// - 用例本体在同目录的 `<concern>_tests.rs` 分册里（测试贴着被测物，一册一个关注点）
// 为什么拆：这一段原来 3,227 行、113 个顶层项挤在一个文件里，15 个关注点只靠注释横幅分隔。
// ⚠️ 分册名必须以 `_tests.rs` 结尾：`lib_tests.rs` 的 `guard_source_views_register_every_include_subfile`
// 按这个后缀把测试分册豁免在「生产码全集」视图之外（视图只装生产码 —— 掺进测试字面量会让按窗口
// 取段的守卫飘到测试文本上，2026-10-06 实测假红 4 条）。改名或换成 `tests_*.rs` 都会逼出假绿。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gossip_engine::GossipEngine;


    include!("handshake_tests.rs"); // 1/15 握手帧的头像预算 + Hello 验签决策表 + 公钥锚点（TOFU / 好友锚）
    include!("peer_state_tests.rs"); // 2/15 peers 表在线判定 + 成员 X25519 公钥解析与好友接受时的补绑
    include!("dial_tests.rs"); // 3/15 主动建链的拨号判据（大小 ID 对称、退避、同路已连通则不拨）
    include!("route_tests.rs"); // 4/15 多链路选路次序（LAN / routed / BLE 优先级、健康度过滤、陈旧代次）
    include!("outbound_tests.rs"); // 5/15 出站投递：按链路顺序发送、failover、背压、超时不留残、优先级通道互不阻塞
    include!("queue_tests.rs"); // 6/15 链路队列的字节预算与「只有一个创建点」（queue_policy）
    include!("framing_tests.rs"); // 7/15 帧编解码与读写循环：未知 wire type 降级、超长帧、本地错与 socket 错分型
    include!("relay_tests.rs"); // 8/15 公网中继：定向转发 share/offer、中继节点不本地处理 Ack、转发保留原字段
    include!("gossip_tests.rs"); // 9/15 Gossip 消费判据与传播层：非成员也转发、大块分级、传播层不依赖本地落库
    include!("e2ee_tests.rs"); // 10/15 直连 E2EE 载荷：解封成功/失败/自愈、换钥后重密封、明文与篡改一律拒
    include!("group_keys_tests.rs"); // 11/15 待发群密钥登记表 + 群成员变动后的重发与文案判据
    include!("dispatch_tests.rs"); // 12/15 分发与落库裁决：入站去重真值表、徽章归属、副作用策略、好友权限门
    include!("outbox_tests.rs"); // 13/15 离线队列与过期终态：过期文件不许改写已完成传输、Ack 点亮送达并按 msg_id 删行
    include!("read_receipt_tests.rs"); // 14/15 已读回执：只前进不后退、写失败时待发队列不丢最大时间戳
    include!("listen_tests.rs"); // 15/15 监听口平台语义：accept 后重绑、Windows 断离式关闭、启停循环仍服务
}
