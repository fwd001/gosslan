# 本次改动概览（2026-09-04）

## 完成了什么

1. **项目重命名**：Lanct → **Gosslan**（gossip + LAN，全小写单字、无大小写分词）。
   - 包名 `gosslan`、Rust crate `gosslan_lib`、应用标识 `com.gosslan.app`、窗口标题 `Gosslan`、CSS 前缀 `--gosslan-*`、本地库 `gosslan.db`。
   - 复用脚本 `scripts/rename.mjs`，未来想换名可一键重做。

2. **版本号维护机制**（SemVer）：
   - `scripts/version.mjs` + npm 脚本 `version:show/patch/minor/major`，一次 bump 同步 `package.json` / `Cargo.toml` / `tauri.conf.json` 并落 `CHANGELOG.md` 日期。
   - 版本基线 `0.2.0`；产物命名 `gosslan_<版本>_x64-setup.exe`。

3. **500–1000 节点性能优化**：
   - 修复 `friends` 表缺失公钥列的运行 bug。
   - 节点表事件 300ms 节流合并（`spawn_peer_emitter`）。
   - UDP 广播自适应周期（<100→5s / ≥100→10s / ≥500→20s）+ 抖动。
   - 消除每条 announce 的冗余写库 / 群密钥遍历。
   - SQLite WAL + `synchronous=NORMAL`；加好友弹窗搜索 + 截断渲染。
   - 详见 `docs/performance.md`。

4. **Windows x64 打包链路**：
   - 生成完整图标集（`npm run icon`）。
   - `npm run dist:win`（NSIS 安装包）。
   - `.github/workflows/build.yml`：推送 `v*` 标签 → GitHub 自动构建并发布 win-x64 安装包。

## 验证状态
- ✅ 前端 `vue-tsc` 类型检查、`vite build`（1881 模块）、`npm test`（22/22）全部通过。
- ⚠️ 本沙箱无 Rust/MSVC/MinGW，**无法本地编译产出 .exe**；Rust 代码已按 Tauri v2 稳定 API 编写并自查，需在装好 Rust + VS Build Tools 的机器上 `cargo check`/`npm run dist:win` 验证。

## 下一步建议
- 本地或 GitHub Actions 出第一个 win-x64 安装包。
- 之后按「改多少 bump 多少」的节奏发版：`npm run version:patch|minor|major` → 提交 → `git tag vX.Y.Z` → 推送。
