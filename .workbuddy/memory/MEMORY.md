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
- **E2EE（v0.11.0 起恒开且不可关闭）**：单聊/群聊始终 X25519 + ChaCha20-Poly1305 加密；
  `e2ee_enabled` 设置已废弃（Settings 结构无此字段，旧库残留键由 reset 清理）；
  直发内容加密带 `enc1:` 前缀；GossipEnvelope.encrypted 缺省 true（兼容旧版）；
  接收方解密只需发送方公钥（信封携带），无「两端同时开启」限制
- **E2EE send（v0.11.0）**：发送必须有对端公钥——好友表 → peers 表 → 探测 who_has 等 1.2s 重试 → 仍缺报错指引；
  enc1: 解密失败写系统消息而非 return（v0.10.0）
- 设备指纹前缀 `gosslan-`（v0.8.0 破坏性变更，旧 dev- 好友关系需重建）
- 机器码依赖 machine-uid 仅桌面目标（Cargo.toml target 门控）；`enc1:` 解密查公钥顺序：friends 表 → peers 表
- version.mjs 只认 `[Unreleased]`→落日志；npm 命令在沙箱需 `env -u NODE_OPTIONS -u CODEBUDDY_BROKERED_FS_HOOK_ENABLED` 前缀
- Bash 的 grep 常返回空，一律用 Grep 工具
- 并发会话常见：push 前先 fetch；git commit/push 链式命令偶发「nothing to commit」假输出，以 git log/ls-remote 实际状态为准
- **删除聊天记录**：db::delete_conversation 事务删 messages+conversations 行（幂等）；前端 ConversationList 右下角 X 按钮 + 二次确认 Modal
- **消息回执位置**：v0.10.0 起 mine 时固定在气泡左侧（行容器第一子元素）
- **消息时间**：默认 MM-DD HH:mm，hover 切秒级；同分钟合并消息仅 hover 时显示完整时间
- **文本右键菜单**：v0.10.0 起禁用自定义复制菜单（hover 按钮已够）
