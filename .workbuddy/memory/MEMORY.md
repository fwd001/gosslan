# Gosslan 项目长期备忘

## 开发流程（用户明确要求，2026-09-05 起）
- **新功能一律：先计划 → 再设计测试 → 后实现 → 用测试验证**（计划要简短列出任务分解与测试设计）
- **每次交付推送 = version bump + CHANGELOG + tag + push tag**，缺一不可：
  1. 先把变更写进 CHANGELOG `## [Unreleased]`，再 `node scripts/version.mjs minor|patch`
     （同步 package.json / Cargo.toml / tauri.conf.json / package-lock；Cargo.lock 用 cargo check 刷新）
  2. `git tag vX.Y.Z && git push origin main && git push origin vX.Y.Z`
     （注意：本沙箱 git && 链有时中断，push 后务必 `git ls-remote` 核对 main 与 tag 同 commit）
  3. tag 触发三端 workflow 并自动创建 GitHub Release（三件套 apk/dmg/exe）
- 版本选择：新功能=minor，修复=patch；破坏性变更在 0.x 允许进 minor，CHANGELOG 标注 ⚠️

## 关键事实
- 仓库 public：fwd001/gosslan，SSH 已认证；本机 macOS arm64，SDK/NDK27.1 齐全
- CI 三端全绿基线：v0.5.1 起；v0.8.0 起 Release 均带 apk/dmg/exe 三件套
- 匿名 GitHub API 有 60/h 限流，超限时用 WebFetch 读网页版 releases 页
- E2EE：v0.8.0 起可开关（默认关，设置 `e2ee_enabled`）；**两端必须同时开启才能互通**；
  直发内容加密带 `enc1:` 前缀；GossipEnvelope.encrypted 缺省 true（兼容旧版）
- 设备指纹前缀 `gosslan-`（v0.8.0 破坏性变更，旧 dev- 好友关系需重建）
- 机器码依赖 machine-uid 仅桌面目标（Cargo.toml target 门控）；`enc1:` 解密查公钥顺序：friends 表 → peers 表
- version.mjs 只认 `[Unreleased]`→落日志；npm 命令在沙箱需 `env -u NODE_OPTIONS -u CODEBUDDY_BROKERED_FS_HOOK_ENABLED` 前缀
- Bash 的 grep 常返回空，一律用 Grep 工具
- 并发会话常见：push 前先 fetch；git commit/push 链式命令偶发「nothing to commit」假输出，以 git log/ls-remote 实际状态为准
