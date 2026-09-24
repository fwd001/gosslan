//! Routed（已路由 IP）端点的配置层。
//!
//! ⚠️ 这里**没有**发现机制。曾经的目标结构（Phase 3：`trait.rs` 定义 `Discovery`、
//! `manager.rs` 聚合多源、`lan.rs` 把 announce 转成候选、`routed.rs` 产 Routed 候选）
//! 三个文件全部零生产调用点，2026-09-24 架构复审 0-A2 删除。
//!
//! **真正在跑数据的发现在 `network/discovery.rs`**（UDP 广播 + 组播 + 网卡选择 + 过期清扫），
//! Routed 拨号在 `network/transport.rs` 的 routed_task。留着那套未接线的抽象，代价不是冗余，
//! 而是下一个人照着它改、改在一个不跑数据的家上（本仓库为这件事专门建了
//! `docs/domains.data.mjs` 的 `activeHome` 字段与 `docs/migration-ledger.md`）。

pub mod routed;

pub use routed::{
    encode_endpoints, parse_endpoint_addr, parse_endpoint_addr_on, parse_endpoints, RoutedEndpoint,
    RELAY_DEFAULT_PORT, ROUTED_ENDPOINTS_KEY,
};
