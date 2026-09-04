# Changelog

本项目遵循[语义化版本 SemVer](https://semver.org/lang/zh-CN/)：

- **major**：破坏性变更 / 架构级重构（不向后兼容）
- **minor**：新增功能（向下兼容）
- **patch**：Bug 修复与细节优化

版本号统一由 `npm run version:patch|minor|major` 维护，一次改动同步 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 三处，并把本文件 `[Unreleased]` 小节落为带日期的版本小节。

## [0.4.0] - 2026-09-04
### Added
- **统一文件发送（自动路由）**：新增 `send_file_auto` 命令——有直连 TCP 链路走直连分片流，无直连自动切换切片中继，均不可达给出明确错误；前端「直接发送 / 中继发送」合并为一个按钮，无需用户选择路线
- **消息触顶分页加载**：虚拟列表滚动到顶部自动加载更早历史（每页 300 条，上限 10 页），带会话切换竞态守卫
- **会话选中态（飞书式）**：会话列表 / 联系人列表选中项左侧主色指示条 + 名称高亮；新增 `ensure_conversation` 命令，打开新好友会话时后端自动补建会话行（修复左侧无高亮项）
- **离线消息队列激活**：全网无连接时消息进入 SQLite outbox，对方上线建链后自动补发（原 `insert_outbox` 为死代码从未启用）
- **TCP 心跳探活**：每 5s 向所有已建链节点发送 Heartbeat，静默断连及时清理（在线状态修正）

### Fixed
- **文字消息发送失败**：好友公钥在加好友后才落库导致 ECDH 失败——`announce`/`FriendAccept` 路径始终同步好友公钥，`send_message` 回退在线节点表取公钥
- **发送文件界面卡死**：切片读取改为 `spawn_blocking`（不再阻塞 async runtime）；发送/接收两侧进度事件 250ms 节流；前端进度事件不再全量 `refreshTransfers()`（消除大文件 IPC 风暴）
- **间歇性连通性故障**：旧连接断开时误删新连接的发送端（按 key 而非按 channel 身份移除），现用 `same_channel` 精确清理；`connect_to_peer` 移除占位 channel 竞态
- **新会话不显示**：收到未知会话的消息时自动从后端刷新会话列表（原逻辑直接跳过）
- **历史遗留单测失败**：`outbox.msg_id` 补唯一索引（含旧库迁移 + 重复行清理）、`reassemble_out_of_order` 改用手工切片绕开最小分片钳制，31 个 Rust 单测全绿

### Changed
- **设置即点即保存**：昵称失焦 / 回车自动保存，缓存策略（保留时长 / 磁盘上限）改动即时持久化；移除「保存资料」「保存策略」按钮
- CI 注入的 `scripts/android/permissions.xml` 移除误带的 XML 声明（曾致 Android Gradle 清单解析失败、APK 打包全红）
- 会话 / 联系人列表行增加 `v-memo`，长列表渲染仅在相关字段变化时更新

## [0.3.0] - 2026-09-04
### Added
- 好友搜索流程：按需 `who_has` 群发探测（仅在打开「添加好友」时触发，启动不持续扫描）；昵称 / IP / 设备 ID 实时过滤；已是好友显示「已加好友」禁用态；500–1000 节点下列表截断渲染 + 搜索
- 全平台系统通知：应用处于后台或非当前会话时触发原生通知（含昵称 + 摘要）；点击通知唤起并聚焦窗口，自动跳转到发送者会话并清零未读
- 新增后端命令 `search_nearby_peers`、`focus_window`
- **双通道聚合传输**：`transport/` 定义 `Transport` 抽象接口 + `TransportManager`（局域网 / 蓝牙独立开关、智能分流、状态汇总），`lan.rs` 适配现有网络层，`bluetooth.rs` 提供 BLE/RFCOMM 接口契约（后端待接入）
- **异构 Mesh 中继**：`relay/mesh_router.rs` 跨链路桥接寻址、TTL 衰减、有界 RingBuffer 限流暂存（可独立测试）
- **轻量存储与缓存清理**：`storage/cache_cleaner.rs` 保留时长（3/7/30 天/永久）+ 磁盘配额自动清理 + SQLite VACUUM；缓存策略设置与「立即清理」命令
- 新增命令 `get_channel_status`、`set_channel_enabled`、`get_cache_info`、`set_cache_policy`、`clean_cache_now`

### Changed
- 聊天/群聊/文件消息的原生通知由 Rust 侧迁移到前端统一做 gating（后台 / 非当前会话），避免重复通知；好友申请类通知仍由 Rust 直接触发
- 新增 `--instance N` 多开启动参数（独立数据库/TCP 端口/设备指纹，UDP 共享 + SO_REUSEADDR），支持单机模拟多节点压测
- 新增跨平台环境配置指南 `docs/setup-windows.md` 与一键脚本（`env:check` / `env:install` / `android:build` / `dist:win:portable` / `multi:run`）

## [0.2.0] - 2026-09-04
### Added
- E2EE 端到端加密：X25519（ECDH 密钥交换）+ Ed25519（签名校验）+ ChaCha20-Poly1305（AEAD）
- Gossip 去中心化广播：Bloom Filter 概率去重 + LRU 精确去重 + Epidemic fan-out + TTL
- 大文件切片中继：64KB–512KB Chunk，BitTorrent 式 Mesh 并行分发、乱序重组
- 群聊群密钥机制：组密钥对称加密，密钥用各成员公钥 ECDH 单独加密分发
- 响应式布局：PC 三栏 / 移动端单栏滑动切换，虚拟滚动，Web Worker 异步解密
- 局域网拓扑状态栏：节点数 / 中继数 / 平均时延
- 项目更名 Lanct → **Gosslan**

### Changed
- UI 组件库由 Ant Design Vue 全面替换为 Tailwind CSS + Headless UI + Lucide
- 面向 500–1000 节点规模做性能优化：节点列表事件节流合并、自适应广播周期、去重参数适配、SQLite WAL 调优

## [0.1.0] - 2026-09-03
### Added
- 基础 P2P 局域网即时通讯：UDP 广播发现、TCP 分帧传输、好友关系、群聊、文件直传、共享目录
