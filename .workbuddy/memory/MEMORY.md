# Gosslan 项目长期备忘

## 发版流程（用户明确要求，2026-09-05 起）
- **每次交付推送 = version bump + CHANGELOG + tag + push tag**，缺一不可：
  1. `node scripts/version.mjs minor|patch`（同步 package.json / Cargo.toml / tauri.conf.json / package-lock / Cargo.lock 需 cargo check 刷新 / CHANGELOG）
  2. CHANGELOG：预先把本轮变更写进 `## [Unreleased]`；version.mjs 已增强——没有该小节会自动补占位，但**内容应手写具体变更**
  3. `git tag vX.Y.Z && git push origin main && git push origin vX.Y.Z`
- tag 推送触发三端 workflow（build.yml / build-macos.yml / build-android.yml），各自把产物挂到同名 GitHub Release（softprops 支持多 workflow 并发向同一 Release 追加资产，已验证 v0.4.2~v0.6.0 均有 exe/dmg/apk 三件套）
- 版本选择：新功能=minor，修复=patch（SemVer）
- 此约定覆盖早前"不自动创建 Release"的旧习惯：Gosslan 项目就是要自动发版

## 关键事实
- 仓库 public：fwd001/gosslan，SSH 已认证；本机 macOS arm64，SDK/NDK27.1 齐全
- CI 三端全绿基线：v0.5.1 起（Android 曾因 machine-uid/focus_window 等编译错误全挂，已修）
- version.mjs 只认 `[Unreleased]`→落日志；Cargo.lock 里 gosslan 版本需 cargo check 刷新
- npm 命令在沙箱需 `env -u NODE_OPTIONS -u CODEBUDDY_BROKERED_FS_HOOK_ENABLED` 前缀
- Bash 的 grep 常返回空，一律用 Grep 工具
- 并发会话常见：用户或另一 AI 会同时改这个仓库，push 前先 fetch，冲突多半在 Cargo.toml/device.rs/CHANGELOG
