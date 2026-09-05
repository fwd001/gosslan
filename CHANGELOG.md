# Changelog

本项目遵循[语义化版本 SemVer](https://semver.org/lang/zh-CN/)：

- **major**：破坏性变更 / 架构级重构（不向后兼容）
- **minor**：新增功能（向下兼容）
- **patch**：Bug 修复与细节优化

版本号统一由 `npm run version:patch|minor|major` 维护，一次改动同步 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 三处，并把本文件 `[Unreleased]` 小节落为带日期的版本小节。

## [0.5.0] - 2026-09-05
### Added
- **聊天滚动与定位**：消息区仅纵向滚动（禁横向溢出）；滚动事件 rAF 节流 + passive 监听；`scrollToIndex` 支持跳到任意消息（任意方向无布局抖动）；打开有未读的会话自动定位到**第一条未读**（显示「以下是未读消息」分割线，向上加载历史时分割线索引随偏移）；离开底部显示「回到最新」悬浮按钮
- **长文本折叠**：超过 280 字符的消息默认折叠为 5 行，底部带「展开全文 / 收起」与「复制」按钮
- **聊天样式配置（即点即存 + 跨设备同步）**：6 套可读性配色预设（经典蓝 / 薄荷绿 / 暖阳橙 / 青瓷 / 樱花粉 / 石墨灰，明暗双主题、对比度 ≥ 4.5:1）+ 3 档字体大小 + 消息合并开关；改动立即生效并持久化（重启保留）；变更时经 `ChatStyle` 消息广播到所有已连接节点，**对方按「我的配色」渲染我发的消息**（持久化对端样式表）
- **删除好友**：联系人列表右键 → 删除好友（保留聊天记录，公钥随行移除）；对方仍出现在扫描列表可重新添加；新增 `remove_friend` 命令
- 群聊优化：连续消息合并（5 分钟内同发送者省略头像/昵称/时间戳）、≥5 分钟显示时间分割线、群消息显示发送者昵称
- E2E 新增「聊天样式同步」协议用例；VirtualList 向上加载历史时锚定旧首条消息（视口不跳动）

### Changed
- 消息气泡颜色 / 文字颜色改为样式预设驱动（覆盖原 `--gosslan-bubble-*` 变量的用法）
- `reset_settings` 额外清除聊天样式与对端样式表

## [0.4.2] - 2026-09-05
### Added
- **单机 dev 全功能验证流程**：`scripts/e2e-dev.sh` 一键脚本——无需第二台设备、不依赖 UDP 广播（who_has 单播 + TCP loopback），启动 headless 实例后运行协议级对端，覆盖除网络发现外的全部聊天功能（29 项断言全通过）
- `e2e_peer --full` 扩展模式：代码 / 图片 / 1MB 大文本消息、乱序消息、群消息、心跳保活、UserInfo 资料同步、好友申请（等待 UI 人工同意，SKIP 语义）、共享目录树、**下载方向文件传输**（app 主动发送路径，即「发送文件卡死」修复的回归验证），并补齐对应 SQLite 落库校验（ts 保真、长度完整、群会话行）
- 测试报告支持 PASS / FAIL / SKIP 三态（人工交互项不计失败）

### Fixed
- e2e-dev.sh 在 pkill 后旧进程未退完时 sqlite3 预置 share_dir 偶发失败——加重试

## [0.4.1] - 2026-09-05
### Fixed
- **UDP 发现 socket 阻塞（关键）**：`bind_udp_reusable` 把 socket2 创建的**阻塞** socket 直接交给 tokio——debug 构建直接 panic（发现任务静默死亡），release 构建虽不 panic 但阻塞 fd 挂在 kqueue/epoll 上会卡死 worker 线程（界面卡顿帮凶）。修复：转 tokio 前显式 `set_nonblocking(true)`
- **macOS/Linux 多开互发现失败**：UDP 同端口多绑定在 unix 上必须 `SO_REUSEPORT`（`SO_REUSEADDR` 仅 Windows 有效），socket2 需开 `all` feature；缺它第二个实例 network 启动即报 "Address already in use"
- **跨路径消息重复隐患**：Gossip 落库的 msg_id 带 `g-` 前缀而 outbox 补发为 UUID，两者不同导致极端竞态下重复消息。统一为 Gossip 信封的确定性 SHA-256 ID（本地记录 / Gossip 投递 / outbox 补发三处共用），接收方 `message_exists` 跨路径去重

### Added
- **协议级 E2E 验证工具**：`src-tauri/examples/e2e_peer.rs`——无 GUI 直连真实运行实例，覆盖 UDP 发现、TCP 建链、直连消息 + 去重、Gossip E2EE（X25519+AEAD+Ed25519）、文件传输全链路、outbox 离线补发，并校验 SQLite 落库与文件落盘（15 项断言，全部通过）
- `GOSSLAN_AUTOSTART=1` 环境变量：启动即自动开启局域网通道，支持 headless 多实例联调
- `lib.rs` 开放 `crypto` / `protocol` 模块供测试对端复用

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
