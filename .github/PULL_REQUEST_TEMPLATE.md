<!-- Gosslan PR 模板
     分级由 CI 自动计算（check-change-budget.mjs），但你需要手动填下面三个 section。
     L1 改动（≤5 文件 / ≤200 LOC / 单领域）三个 section 都填 N/A 即可。
     L2 改动必须填 "[plan]" section。
     L3 改动（或碰了 crypto.rs / protocol.rs）必须填 "[impact]" section。
     不填 → CI 里 check-change-budget 会红。 -->

## 改动摘要

<!-- 一句话改了什么、为什么。例："fix(ble): 外设侧 DEFAULT_PAYLOAD_BUDGET 漏减 ATT 头，导致 514 > 512" -->

## [plan] — 你打算怎么验证

<!-- L2/L3 必须填；L1 填 N/A -->
<!-- 列出你做过/要做的验证步骤。至少一条；依赖真机/第二台设备的写清楚。 -->
- [ ] `npm run verify` 本地全绿（13 步）
- [ ] `cd src-tauri && cargo test --features bluetooth`
- [ ] `npm test`
- [ ] （真机/第二台设备）
- [ ] （verify-guards 非空转：python3 scripts/verify-guards.py）

## [impact] — 安全/数据/协议影响

<!-- L3 必须填（碰了 crypto.rs / protocol.rs 无论多小都要）；L2 填 N/A -->
<!-- 回答：这次改动会影响谁？什么场景出问题？回滚路径是什么？ -->

**影响范围**： <!-- 例：单聊消息发送路径 / 全库 schema / E2EE 握手 -->
**安全影响**： <!-- 例：无 / 可能导致中继节点能解密 / X25519 密钥派生变化 -->
**数据影响**： <!-- 例：无 / schema 变更需要迁移 / outbox 格式变化 -->
**回滚**： <!-- 例：revert 这个 commit / 需要数据迁移回滚脚本 -->

## 测试门禁自检

<!-- 每一项勾一下，证明你跑过了（或说明为什么跳过） -->

| 检查 | 命令 | 结果 |
|---|---|---|
| 前端 457 测试 | `npm test` | ✅ / ❌ / ⏭（理由） |
| Rust 单测 489 | `cargo test --features bluetooth` | ✅ / ❌ / ⏭ |
| verify.mjs 全量 | `npm run verify` | ✅ / ❌ / ⏭ |
| 非空转护栏 | `python3 scripts/verify-guards.py` | ✅ / ❌ / ⏭ |

## 涉及的领域（从 domains.data.mjs 选）

<!-- 选一个或多个，方便 change-budget 计算同领域重复犯案 -->
- [ ] transport（TCP 帧/建链/心跳）
- [ ] ble（扫描/连接/载荷预算/分片）
- [ ] mesh-router（路由/选路/中继）
- [ ] discovery（UDP 广播/组播/Routed）
- [ ] friendship（好友/申请/安全码）
- [ ] content-transfer（消息/文件/图片/代码块）
- [ ] storage（SQLite schema/缓存清理）
- [ ] e2ee（X25519/Ed25519/ChaCha20）
- [ ] protocol（线格式/消息枚举）
- [ ] ui（前端/窗口/主题/i18n）
- [ ] other（说明：____）

## 关联 issue / CHANGELOG 条目

<!-- fix #123 或 refs #456；CHANGELOG 条目建议写在 commit message 里，格式：fix(scope): 摘要 -->
