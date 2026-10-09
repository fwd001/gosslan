# perf/ —— 长会话压测台（手动运行）

用**真实的 `VirtualList` 组件**与**真实的高度估算函数**灌入合成消息，采集滚动帧间隔与定位准确度，
回答"长会话会不会卡、整表重算多少次"。

## 怎么跑（三个进程）

```bash
# 1) dev server（压测页只是普通 Vite 页面）
npx vite --port 5199 --strictPort

# 2) 带调试端口的 headless 浏览器。**端口必须是 9223** —— perf/run.mjs 里写死了它自己开页的端口，
#    换端口只会让 probe 能连、run.mjs 报「找不到 CDP 目标页」。
"/Applications/Brave Browser.app/Contents/MacOS/Brave Browser" \
  --headless=new --disable-gpu --no-sandbox --user-data-dir=/tmp/brave-perf \
  --remote-debugging-port=9223 --window-size=1280,900 \
  "http://127.0.0.1:5199/perf/vlist.html?n=20000"

# 3) 先断言，再采数（node 22+ 自带 WebSocket，无需装依赖）
node perf/probe.mjs 9223        # 必须同时看到 scrollerReal: true 与 virtualizationOK: true
node perf/run.mjs 20000         # 再采帧间隔与整表重算次数
```

## ⚠️ 采数之前先读两条自检（这一条是 2026-10-09 加进来的，原因是量具自己坏过）

`perf/probe.mjs` 现在打印两个布尔量，**两个都为 true 才准采数据**：

| 自检 | 它挡住的是哪种假数据 |
|---|---|
| `scrollerReal` | 页面**没加载应用样式**时，`overflow-y-auto` 只是个死类名 ⇒ 容器根本不是滚动容器 ⇒ **`scrollTop` 写入被浏览器整轮忽略**。当时的表现是"滚动帧间隔 p50 = 16.7ms 满帧"，而 16.7ms 正是一个**什么都不做的空闲帧**——一帧都没滚过。 |
| `virtualizationOK` | 渲染行数 ≈ 总条数 ⇒ 虚拟化没生效，帧数据测的是"全量渲染"这种线上不存在的情形。 |

> **历史代价（写在这里防止再犯）**：2026-09-10 那两份报告（本文件旧版 + `.workbuddy/artifacts/vlist-perf-selfcheck.md`）
> 里的一切帧间隔与"`estimateHeight` 调用次数 = 0，稳态滚动没有整表重算 ✓"，都是在 `scrollerReal` 为假时测的——
> **调用数为 0 不是因为不重算，是因为根本没滚动**。那两个数字从今天起作废，别再引。

## 今天（2026-10-09）量具修好后的第一批真数据

复跑命令：上面三步（`?n=20000`）。同一台机器、headless Brave、`--disable-gpu`、dev 模式构建。

| 项 | 值 | 读法 |
|---|---|---|
| `renderedRows` / `clientHeight` / `scrollHeight` | 12 / 700 / 2,545,466 | 虚拟化生效（12 行 ≠ 2 万行） |
| 滚动 120 帧 × 800px（共 96,000px） | avg **16.7** / p50 **16.7** / p95 **17.6** / max **18.2** ms | 满帧，无一帧掉出 16.7ms 一档 |
| `longFramesOver50ms` | **0** | 这一档规模下没有肉眼可见的卡顿 |
| `estimateCalls` | **2,340,312** | 见下面那颗地雷 |
| `offsetsRebuilds` | **117.02** | **≈ 每帧一次整表前缀和重算**（2,340,312 ÷ 20,000） |
| n=100,000 那一档 | `run.mjs` 在 `Runtime.evaluate` 180s 超时 | **未归因**：可能是重算风暴随规模线性放大（117 × 10 万），也可能是 harness 自己又开了一个 10 万行的页。**别把这句读成"10 万会卡"，它现在只是一条没跑完的记录** |

**为什么帧间隔仍然是满帧，而重算已经每帧一次**：估算结果有 `bubbleCache`（`utils/messageHeight.ts`，键 = 字号 + `msg_id`），
所以重算里那 2 万次调用退化成哈希查找，单帧成本还没浮出来。**这不代表安全**——它意味着成本随条数线性增长，
而目前挡住它的是缓存而不是算法。10 万档没跑完正好是这条假设该被证实或证伪的地方。

## 与真实聊天页的差异

- 行模板用等价结构（头像 + 昵称 + 气泡），未挂 `MessageItem` 的交互/右键/引用逻辑；
- 图片/文件行用等高占位块（与估算假设对齐）；
- 没有 ChatHeader / 输入框，视口略大；
- ⚠️ 合成数据是"每条都实测过一次"的稳态，而真实聊天里**图片加载完成会让同一行高度突变**，
  那才是重算风暴最凶的时刻——本台的数字因此**偏乐观**。

## 还开着的一格（下一刀的位置）

`offsets` 是全表前缀和，`commitHeight` 每有一次实测高度与已知值差 >1px 就 `heightVersion += 1` ⇒ 整表重算
（`VirtualList.vue` 的 `offsets` computed）。改法在 `.workbuddy/artifacts/vlist-perf-selfcheck.md` §三 里写过三条
（批量到一帧一次 / 变更点之后的增量修正 / Fenwick），**本次没动**：本轮的收口判据是"虚拟化输入正确 + 量具自证"，
重算成本已经有数字了，动算法要单独一次改动 + 单独一次门禁，不与这次同车。
