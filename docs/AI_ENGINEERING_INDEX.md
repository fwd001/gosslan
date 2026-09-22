# Gosslan AI Engineering Index

## Required before coding

1. `AI_RULES.md`
2. `docs/acceptance/1.0-release.md` — current goal and acceptance bar
3. `AI_PROJECT_HANDOFF.md`
4. `docs/domains.data.mjs` — **领域图**：先确认「我改的是哪个领域、它的边界在哪」
5. `docs/migration-ledger.md` — **迁移台账**：这个关注点**有几个家、哪个在跑数据**
6. `docs/protocol-invariants.md` — when touching protocol / network / crypto / DB
7. `docs/design-guidelines.md` — when touching UI (圆角 / hover / 配色 / 窗口边界)
8. Relevant ADR
9. Relevant tests
10. `CHANGELOG.md` history when touching a previously-fixed area

> ⚠️ **第 4、5 条为什么必须排这么前**（2026-09-16 加入）：本仓库处在一次**半完成的 ADR
> 迁移**中，同一个关注点常常有**两个家**（老的还在跑数据、新的部分接线）。不先确认
> 「哪个家在跑数据」就直接 grep 改代码，会改到那个**看起来更对但没在跑**的新家上 ——
> 症状不变或换个形态，于是出现「改完这个 bug 又冒那个」。
>
> 历史代价：`4.18.7→4.18.10` 连着四版修同一个 BLE 分片预算问题（三份实现）；
> macOS 与 Android 各自藏着一份独立实现，分别到 Phase 3 / Phase 4 才被发现。
>
> 由 `scripts/check-domain-map.mjs` 守门（路径存在、一文件不属两域、无文件漏归属、
> `enforce` 只能开在已收口的单家领域）。

## Templates

- `docs/templates/BUG_FIX.md`
- `docs/templates/ADR.md`

## ADRs

- `0007-protocol-versioning.md`（**Accepted 2026-09-20**：`protocol_version` + capability
  门控 + 灰度顺序 + 三项决策，是跨版本兼容的权威出处）
- `0008-state-machine-boundaries.md`
- `0009-rust-typescript-contract.md`
- `0010-failure-injection-testing.md`
- `0011-gossip-envelope-authentication.md`
- `0012-logical-sequence-ordering.md`
- `0013-transport-priority-queues.md`
- `0014-multi-path-connection-selection.md`
- `0015-ble-transport.md`
- `0016-relay-authorization.md`
- `0017-opaque-external-wire-frame.md`
- `0018-window-architecture.md`
- `0019-content-transfer.md`
- `0020-blind-circuit-relay.md`（**Accepted 2026-09-21**：可选自托管的公网哑管道中继。
  服务器代码**不在本仓库** → <https://github.com/fwd001/gosslan-relay-server>；
  本仓只有客户端侧的 `src-tauri/src/transport/relay_seal.rs`。
  ⚠️ 改到中继线格式要**同一轮改两个仓库**，两边各有一份规格文字，漂移没有测试提醒）
- `0021-device-id-is-generated-not-derived.md`（**Accepted 2026-09-22**：设备 ID 改为首启随机生成
  + 设备属性混合、此后只认持久化值。真机事故：克隆镜像的机器码 / 新机的默认主机名相同 ⇒ 两台设备
  同一个 id ⇒ `peers`/`links`/`friends` 互相顶 + Hello 密钥冲突硬拒 + 镜像规则在 id 相等时退化。
  含「已装设备不迁移、撞号那台换 id 并重加好友、旧会话只读」的迁移决定与自定义后缀的字符集约束）

> Earlier ADRs `0001`–`0006` (message idempotency, outbox+ACK, E2EE, transport, no-Web-Worker,
> device fingerprint) were removed; their normative content now lives in
> `docs/protocol-invariants.md` (INV-P01…P24) and `AI_RULES.md` (INV-001…008).
> Do not re-create them as a second source of truth.

## Rule

If a document conflicts with executable code or tests, do not silently choose one. Report the conflict and determine whether the documentation or implementation is stale.
