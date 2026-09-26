# Gosslan 视觉与交互规范（UI Design Guidelines）

> Version: 1.0 · 2026-09-10  
> 依据：Apple Human Interface Guidelines 与 WWDC25「Get to know the new design system」  
> 的 **Shape & Concentricity**、语义色与交互态取向（iOS 26 / Liquid Glass 时代）。
>
> **强制条款**：**后续所有新功能，如果没有特殊要求，一律按本规范执行**；  
> code review 时按本文末尾的自查清单核对。要偏离必须显式说明理由并写进改动描述。

---

## 0. 三条总原则（与上面对齐）

1. **形状表达密度与层级**，不是装饰。同一层级用同一档圆角；不要到处都圆。
2. **同心**：嵌套面的内圆角必须与外圆角同源（见 §2.3）。不允许内角被夹扁或外翻。
3. **系统拥有窗口形状**：应用**不自己画窗口圆角**（见 §5）。

---

## 1. 圆角规范

### 1.1 三类形状（先选类，再选值）

| 形状                    | 用在哪                   | 本应用对应                  |
| --------------------- | --------------------- | ---------------------- |
| **固定半径**              | 紧凑、密集的控件（默认选择）        | 列表行、导航项、按钮、菜单项、气泡      |
| **胶囊 `rounded-full`** | 半径 = 高度一半；高强调、需要"可点"感 | 头像、在线状态点、未读徽标、圆形图标按钮   |
| **同心**                | 嵌套面：内圆角 = 外圆角 − 内外间距  | 菜单项在菜单里、格子在选择器里、卡片在面板里 |

> Apple 对 macOS 的补充：**小/中等密度的控件保持圆角矩形即可**，不必强行胶囊化。  
> 因此本应用**没有**把普通按钮改成胶囊——这是有意保留，不是遗漏。

### 1.2 圆角梯度（唯一来源：`src/style.css`）

| Token                   | 值      | 用途                             |
| ----------------------- | ------ | ------------------------------ |
| `--gosslan-radius-xs`   | 4px    | 内联小色块：@提及、行内代码、搜索高亮            |
| `--gosslan-radius-sm`   | 6px    | 紧凑控件：下拉项、工具条按钮、气泡、头像           |
| `--gosslan-radius-md`   | 8px    | 标准控件：列表行、导航项、菜单容器、输入框、设置行、代码卡片 |
| `--gosslan-radius-lg`   | 12px   | 浮层与卡片：弹层、设置分组、大卡片              |
| `--gosslan-radius-xl`   | 16px   | 大面板：模态、资料卡                     |
| `--gosslan-radius-pill` | 9999px | 胶囊（等同 `rounded-full`）          |

语义别名（保留旧名，值统一来自上表）：`--gosslan-avatar-radius`=sm、`--gosslan-item-radius`=md、`--gosslan-bubble-radius`=sm。

**用法**：一律写 `rounded-[var(--gosslan-radius-md)]`，不要写 `rounded-lg` 这类 Tailwind 字面值——  
字面值无法全局调，也看不出"这一档是给谁用的"。方向变体同理：`rounded-t-[var(--gosslan-radius-md)]`。

### 1.3 同心公式（最容易出错的一条）

```
内圆角 = 外圆角 − 内外间距          （结果 < 0 时取 0）
```

| 场景        | 外                 | 间距        | 内（正确）                           |
| --------- | ----------------- | --------- | ------------------------------- |
| 右键菜单项在菜单里 | `rounded-lg` 8px  | `p-1` 4px | **4px（xs）**                     |
| 表情格子在面板里  | `rounded-xl` 12px | `p-2` 8px | **4px（xs）**                     |
| 代码卡片在气泡里  | 气泡 6px（sm）        | 0（卡片贴边）   | **6px（sm）**，即 `rounded-t-[…sm]` |

> 若容器设了 `overflow-hidden`，子元素不画圆角也可以（容器会裁）；但只要子元素自己画了圆角，  
> 就必须按公式取值——否则会出现 Apple 明确禁止的"**内角外翻**"（内比外还圆）。

### 1.4 文字与图标的"视觉对齐"（光学对齐）

排版盒对齐 ≠ 看起来对齐。**半行距会把文字墨迹往下推**：
`font-size: 11px` 的行盒在 `line-height: 1.5` 时是 16.5px，墨迹顶边落在行盒顶下方约 **3.7px**
（中英文同样存在，实测 3.0–4.0px）。当文字与一个**图标/头像并排**时，这个偏移就会读作"文字低了一截"。

规则：

- 并排的图标与文字，**行盒顶 = 图标顶**（flex 行默认就是如此，不要额外加 `items-center` 去"居中"文字行）。
- 需要"贴顶"时，用 **`leading-none`**（行盒 = 字号）消除半行距，而不是用负 margin 硬拽。
- 改行高必然改变行盒高度 → **同时把高度估算法里的常量改掉**（`utils/messageHeight.ts`），
  否则虚拟列表会错位。示例：群聊昵称行 `leading-none`(11) + `mb-[7px]` = 18 = `NICKNAME_ROW` ✓。

### 1.5 红线

- ❌ 新增字面值圆角（`rounded-lg` / `border-radius: 8px`）——必须用 token。
- ❌ 内圆角 ≥ 外圆角（外翻）。反例（本次已修）：菜单容器 8px + `p-1`，菜单项却用 6px。
- ❌ 给**窗口根容器**加圆角（见 §5）。
- ❌ 同一层级混用不同档位（例如列表里既有 6px 又有 8px 的同类行）。

---

## 2. 交互态规范（hover / press / focus）

iOS 的取向：**hover 必须克制**（只做轻微加深/提亮，**不做位移与缩放**——密集列表里位移会让"行在跳"），  
而且**必须有 press 态**，否则按下去像没反应。

| 态         | 规则                           | 实现                                                                        |
| --------- | ---------------------------- | ------------------------------------------------------------------------- |
| **hover** | 中性面加深；危险/警告用语义 token         | `hover:bg-[var(--gosslan-hover)]`、`hover:bg-[var(--gosslan-danger-soft)]` |
| **press** | 全局统一压暗/提亮，不需要逐个写             | `src/style.css` 的 `button:active` / `[role=button]:active` 全局规则           |
| **focus** | 见 §2.4：**非文本控件**用键盘焦点环；**文本输入类**用边线变色 | 全局规则 + `--gosslan-focus-ring`                                             |

### 2.4 焦点提示：文本输入类不许画外圈方框

用户 2026-09-16 反馈：「整个应用的输入框在焦点态会默认有个主题色的方框，很难看」。
根因是全局焦点环的选择器里含 `input` / `textarea` / `select` / `[contenteditable]` ——
浏览器对**文本类控件**一律把 `:focus-visible` 判成"永远成立"（**点一下就成立**，不需要键盘
Tab），于是每次点击输入框都会冒出一个 2px 的外圈方框。消息输入框尤其难看：它的矩形只是
卡片里一块**透明的编辑区**（不是整张卡片），框出来像卡片内部浮着一个方框。

规则（两条分开，判据由 `designGuards.checkTextFieldFocusRing` 守着）：

| 控件 | 焦点提示 | 实现 |
|---|---|---|
| `button` / `a` / `[tabindex]`（非文本控件） | 键盘焦点环（`:focus-visible`） | `:where(button, a, [tabindex]):focus-visible { outline: … }` |
| `input` / `textarea` / `select` | **边线变主题色** | `input:focus, textarea:focus, select:focus { border-color: … }` |
| 消息输入框（`div[contenteditable]`） | **卡片边框**变主题色 | `.gosslan-composer:focus-within { border-color: … }` |

- 文本输入类**不得**出现在全局 `:focus-visible` 选择器里（含 `:where()` 的写法也算）。
- 焦点提示一律画在**边线**上，与既有的 `.gosslan-select:focus`、`focus:border-[var(--gosslan-primary)]` 同一套观感。
- **本来没有边框**的字段（弹窗输入框、过滤条那种自带底色的）自己补一个**常驻的**
  `border border-transparent`，就能吃到上面那条全局规则 ——
  不要等聚焦时才加边框（会改尺寸、文字跳一下），也不要再画外圈方框。
- **去掉外圈方框 ≠ 可以没有焦点提示**：WCAG 2.4.7 要求可见焦点，别顺手把替代提示一起删了。
  这条由 `designGuards.checkTextFieldFocusRing` 守着（改坏即报）。

### 2.1 允许的 hover 取值（白名单）

- 中性面：`--gosslan-hover`（面板/列表/菜单项通用）
- 列表与侧栏的专用层：`--gosslan-list-hover`、`--gosslan-rail-hover`（保持三档灰阶层次，不要混用）
- 语义：`--gosslan-danger-soft`、`--gosslan-warning-soft`
- 彩色面上的"提亮"蒙层：`hover:bg-white/15`、`hover:bg-white/20`、`hover:bg-black/5`（**仅限**彩底/图片上的覆盖层）
- 不透明度类：`hover:opacity-80/90/100`（用于图标按钮，不得用于整行）

### 2.2 红线

- ❌ 写死颜色：`hover:bg-[#e81123]`、`hover:bg-red-500/10`、`hover:text-amber-500`（本次已全量替换为语义 token）
- ❌ 只有 hover 没有 press（全局规则已兜底，但自定义组件不得用 `filter` 以外的方式覆盖掉它）
- ❌ hover 做 `translate` / `scale` / 阴影大幅变化

### 2.3 触摸与触觉（"原生手感"）

WebView 默认自带"网页感"，来源就四件事，`src/style.css` 的「原生手感基线」已逐条处理：

| 网页感来源 | 处理方式 |
|---|---|
| 点击时闪一块半透明高亮（Android 最明显） | `-webkit-tap-highlight-color: transparent` |
| 双击缩放判定吃掉首次点击的即时反馈 | `touch-action: manipulation`（**保留捏合缩放**，不禁用缩放本身） |
| 内容滚到头还把整页拖出去再弹回来 | `overscroll-behavior: none`（页面）/ `contain`（滚动容器） |
| hover 的过渡把"按下"拖慢约 150ms | `:active` 里 `transition-duration: 0s` → **按下瞬时、松手平滑** |

**按下反馈的两条铁律**（对应 iOS 的 "press feedback on touch-down"）：

1. **按下必须立即变色**，不能等过渡跑完；松手再平滑回落（`transition-duration` 只在 `:active` 归零）。
2. **`<div>` 形态的可点行同样要有反馈**：本仓约定「写了 `cursor-pointer` 就是可点」，全局规则已覆盖。

**触觉（`src/utils/haptics.ts`）** —— 按 Apple 的触觉词汇与时机，**不是每个点击都给**：

| 时机 | 类型 | 依据（Apple 对应） |
|---|---|---|
| 发送消息 | `light` | 按下即反馈，不等网络结果 |
| 切换会话 | `selection` | 离散选择变化（UISelectionFeedbackGenerator） |
| 长按菜单弹出 | `heavy` | 菜单出现时的重反馈 |
| 成功 / 警告 / 失败 | `success` / `warning` / `error` | 语义结果（对应两段 / 两段 / 三连震） |

平台限制（**不要把它当成 iOS 级触觉**）：

- Android WebView：走 `navigator.vibrate`，**需在 AndroidManifest 声明 `VIBRATE` 权限**；
  粒度只有时长/节奏，拿不到 iOS 的 light/medium/heavy 质感。
- iOS（WKWebView）：**不支持** `navigator.vibrate` → 自动 no-op；真触觉须走原生
  `UIFeedbackGenerator`（Tauri 插件或平台代码，属后续项）。
- 桌面：无此 API → no-op。`prefers-reduced-motion` 下也不触发。

**动效令牌**：`--gosslan-ease`（强 ease-out：起步快、收尾稳，比 Tailwind 默认的 standard
曲线更"跟手"）、`--gosslan-duration-fast`(150ms，轻反馈) / `--gosslan-duration`(240ms，结构性变化)。
Tailwind `.transition` 系列的曲线由本文件统一覆盖。

**点按目标**：触屏上小于 44px 的独立小按钮加 `class="tap-safe"`——只在**垂直**方向扩到 44px，
横向不扩（横向相邻控件彼此会抢点击）。

---

## 3. 配色规范

1. **语义色优先**，且必须同时提供浅/深两套（本应用：`:root` + `.dark`）。

### 3.1 语义色总表

| Token                    | Light                 | Dark                   | 用途                  |
| ------------------------ | --------------------- | ---------------------- | ------------------- |
| `--gosslan-danger`       | `#d43d43`             | `#d43d43`              | **填充档**：危险按钮实底、未读徽标底、错误 toast。⚠️ 这一档**上面永远压着白字**，所以它和文字档受同一条 4.5 约束：2026-09-26 无障碍实测系统红 `#ff3b30` 只有 **3.55**（暗色 `#ff5548` **3.16**，hover 混白后更低）⇒ 压深一档，白字 **4.62**。不承担白字的填充（`--gosslan-warning`）仍保持系统色 |
| `--gosslan-danger-ink`   | `#cc2418`             | `#ff5548`              | **文字/图标档**：删除、失败、错误提示（系统红当文字只有 3.55，不够 4.5） |
| `--gosslan-danger-soft`  | `rgba(212,61,67,.12)` | `rgba(255,85,72,.18)`  | 危险的浅底 hover         |
| `--gosslan-warning`      | `#ff9500`             | `#ff9f0a`              | **填充档**：警告实底（Apple 系统橙） |
| `--gosslan-warning-ink`  | `#c67600`             | `#ff9f0a`              | **图标档**：群主皇冠、文件夹图标（琥珀当图标只有 2.20，连图形档 3.0 都不到） |
| `--gosslan-warning-soft` | `rgba(255,149,0,.12)` | `rgba(255,159,10,.18)` | 警告的浅底 hover         |
| `--gosslan-success`      | `#10b981`             | `#30d158`              | **填充/状态点**：在线点、已读勾、进度条 |
| `--gosslan-success-ink`  | `#047857`             | `#30d158`              | **绿色文字**（绿在浅底天然难达标，必须比状态点深一档） |
| `--gosslan-success-soft` | `rgba(16,185,129,.12)`| `rgba(48,209,88,.18)`  | 绿色的浅底（「已启用」徽标等） |
| `--gosslan-status-offline`| `#8e8e93`            | `#8e8e93`              | 离线状态点（Apple systemGray；中性，不是错误、也不是禁用） |
| `--gosslan-accent-ink`   | 主题色 72% + 28% 黑   | 主题色 72% + 28% 白    | **主题色当文字/图标用**的版本 |
| `--gosslan-field`        | `#ffffff`             | `#2a3a52`              | 输入类控件底（搜索框），须与所在栏拉开一档 |
| `--gosslan-card` / `-ink` / `-line` | `#eeeef0` / `#0f172a` / `rgba(0,0,0,.06)` | `#1c2434` / `#e2e8f0` / `rgba(255,255,255,.08)` | 中性卡片气泡（文件 / 代码 / 无类型兜底） |
| `--gosslan-text-2`       | `#475569`             | `#94a3b8`              | 次要文字：说明文案、会话摘要、占位符、辅助图标（11~13px 小字按正文 4.5 判） |
| `--gosslan-hud`          | `rgba(38,38,38,.9)`   | `rgba(72,72,74,.92)`   | Toast 底（与主题色解耦的中性 HUD，白字） |

> **填充档与文字/图标档成对**是这张表的主线（`danger` / `-ink`、`warning` / `-ink`、
> `success` / `-ink`）。改色时先问「这一处是 fill 还是 ink」，再选列。

### 3.2 三条硬约束

1. **品牌色只作填充，当文字用必须走 `--gosslan-accent-ink`。**
   `text-primary` 在面板上只有 3.98（暗）/ 3.68（亮），够不到正文 4.5。
   ⚠️ 暗色档**不能**写成 `.dark { --gosslan-primary: … }`——主色是用户可选、由 `applyTheme`
   以 **inline style 写在 `<html>` 上**，inline 优先级高于任何选择器；只能 `color-mix` 派生独立 token。
2. **填充与文字分开取档。** 绿/橙/红这类高饱和色，"当状态点"和"当文字"需要的亮度不同
   （`#10b981` 在白底只有 2.5）。新增一处彩色文字前先问：这是 fill 还是 ink？
3. **暗色分层靠明度，且不允许倒置**：
   `app-bg #0b1220 < chat/bg #0f172a < caption/rail #16202f < list/panel #1e293b < list-active #334155`。
   **任何"浮在另一层之上"的面必须比它亮**（浮层、输入框、卡片、toast 都适用）——
   暗色下"与底色同值"等于这个元素不存在（本项目踩过 4 次：搜索框、毛玻璃菜单、边框、toast）。

### 3.3 可读性护栏（改色必测）

- 文字与底 ≥ **4.5:1**（含 11px 小字）；图形 / 图标 / 状态点这类非文字 ≥ **3:1**
  （`utils/chatStyle.ts` 已有气泡的运行时校验，不要绕过）。
- **自动化护栏**：`utils/tokenContrast.ts` + `tokenContrast.test.ts` 直接读 `src/style.css`，
  按**契约表**逐对核算 `:root` 与 `.dark`。契约里每条都标注了它对应界面上的哪个组合（`why`）与适用档位。
  → **改 token 值、或新增 token 忘了在 `.dark` 成对定义，`npm test` 立刻变红。**
  契约表只收「必须达标」的组合；有意低于标准的一律不写进去，而是登记在下面的「已知偏差」。
- 量测方式：Brave 无头截图 + Pillow 取像素，或直接算 WCAG 比值
  （`0.2126R+0.7152G+0.0722B` 线性化后 `(L1+0.05)/(L2+0.05)`）。
  **别靠肉眼判断"应该够亮"**——本项目已多次只用推理就误判。

**已知偏差（有意不达标，逐条记明理由）**

| 组合 | 实测 | 为何不拉到达标 |
|---|---|---|
| 在线绿点 `--gosslan-success` `#10b981` on 白底 | 2.54 | Apple 自家 `systemGreen #34C759` on white 只有 2.22 —— **已经比 Apple 更亮**。拉到 3:1 需要显著更深，绿点就不再是"在线"的语义色。离线点则已提到 systemGray（3.26 on 面板） |
| 亮色边框 `--gosslan-border` on 面板 | 1.23 | 亮色**靠阴影表达层级**，分隔线本就该淡（Apple 亮色分隔线同量级）。暗色没有有效阴影，才需要把边框拉到 1.64 —— 两套外观的判据不同，不要互搬 |

★ **撤销过一条"已知偏差"（2026-09-26）**：这一族里原本还有一行「亮色 `--gosslan-danger` 填充档白字 3.55 ——
Apple 的计数徽标同样如此，属短数字+高强调的既有取舍」。**它被推翻并改成了判据**，理由三条：
① 徽标里的数字是**信息载体**（有几个没读、有几个任务），读不出就等于没有提醒，不是纯装饰；
② Apple 自己也这么干不构成理由 —— 本项目的契约是自己定的 4.5，不能拿别人的破绽当自己的依据；
③ 实测压深到 `#d43d43` 后白字 **4.62**，观感只是"红得沉一档"，仍是原生红，没有牺牲辨识语义。
⇒ 现在 `tokenContrast` 的契约表里 `#ffffff on --gosslan-danger` 与 `#ffffff on --gosslan-danger-hover`
两档（亮/暗各二）都是**会红的判据**，不再是注释里的取舍。hover 的方向也一并纠正：原来 `color-mix` 朝白混
（实测把白字从 3.55 再拖到 **3.11**，混出来是 `#ff584f`），现在朝暗混（混出 `#bb363b`，白字 **5.67**）。

### 3.4 原生控件跟随外观

`:root` 写 `color-scheme: light`、`.dark` 写 `color-scheme: dark`。缺了它，
`<select>` 在暗色下仍弹白底白字下拉、滚动条与取色器永远是浅色（"主题没关联上"最典型的破绽）。

**品牌色可被用户改**（`--gosslan-primary`），所以任何地方都不要硬编码主题蓝；
需要"跟随主题色"的派生色用 `color-mix(in srgb, var(--gosslan-primary) …)`（见 `--gosslan-accent-ink`）
或 `utils/chatStyle.ts` 里已有的派生函数（它们自带对比度校验）。

---

## 4. 骨架屏（首屏）同步规则

`index.html` 的内联骨架样式跑在 bundle 之前，**无法引用 CSS 变量**，因此它是**唯一**允许写字面值的地方：

- 骨架用到的圆角/底色必须在注释里注明对应的 token，**改 token 时同步这里**；
- 骨架**不画窗口圆角**（与 §5 一致），否则启动瞬间会看到"角落跳一下"；
- 骨架的三栏宽度/高度必须与真实布局一致（参见该文件内已有注释）。

---

## 5. 窗口与系统边界（本次修复的根因）

**窗口形状由系统负责，应用不自绘。**

- Windows：DWM 提供圆角（`tauri.conf.json` 的 `shadow: true`…）；**最大化时系统不再圆角**。
- macOS：系统窗口圆角。

因此**不要给根容器加 `rounded-xl`**：应用再画一层，就会与系统边界错位——容器圆角之外那圈  
露出 `body` 底色（亮色 `#edf1f6` ≈ 白），表现为"窗口角上有一道白缝"，hover 成红色后尤其刺眼。  
同理，窗口内的**标题栏按钮不要自己画圆角**：让它被窗口边界裁切，曲线才唯一。

> 这条同时满足 iOS 的"窗口/面板边缘由系统或容器统一负责"取向，也避免了 §1.3 的同心外翻问题。

**窗口顶部一律自绘（用户 2026-09-17：「新窗口用的是系统样式？标题栏与内容有明显界限」）。**

- **所有应用自己的窗口**（主窗口 + 设置 / 日志 / 群任务）都是 `decorations:false`，
  顶部那条 caption 由前端画（`src/components/TitleBar.vue`），底色是 `--gosslan-caption`
  ⇒ 与内容同属一套配色，不会出现系统标题栏那种改不了的撞色接缝。
- 辅助窗口用 `src/components/window/AuxWindowShell.vue` 套壳（它同时负责 1px inset ring）；
  **不要把系统标题栏与自绘标题栏叠成两层**。日志窗口因为兼移动端整页形态，直接用 `TitleBar`。
- 小窗口（设置 / 日志 / 群任务）**不给最大化**：builder 上 `.maximizable(false)`，
  caption 传 `:show-maximize="false"`（macOS 绿灯也不画）；可调整大小 / 最小化 / 关闭。
- **唯一例外：外部链接窗口**保留系统标题栏 —— 它加载的是第三方网页，我们自己的文档不在那个窗口里，
  套自绘栏只能改用 iframe 包一层（大量站点有 `X-Frame-Options`，会直接白屏）。

**独立窗口（设置 / 日志 / 群任务）的位置与尺寸：跟随主窗口，且必须用物理像素。**

- **参照物是主窗口，不是屏幕**：子窗口装不下就按主窗口缩（两侧留 24 逻辑像素），位置在主窗口
  外框内居中。主窗口是可缩放的 —— 按屏幕算会让子窗口比主窗口还大、还离它很远。
- **禁止把几何交给 `WebviewWindowBuilder::position/inner_size`**：它们只有**逻辑**坐标，`tao`
  创建窗口时会按"逐个显示器试算"换算，多屏不同缩放时会**选错屏**（`tao` 的
  `available_monitors().find_map(..)`，一个都没命中就退回主屏 `CW_USEDEFAULT`）。
  正确做法：`.visible(false)` 创建 → `build()` 之后用 `set_size` / `set_position` 下发物理值 → 再 `show()`。
- 常驻窗口（关闭即隐藏）每次打开要**重新居中**，否则主窗口被拖到另一块屏后，它留在原地。

实现与护栏：`src-tauri/src/commands.rs` 的 `aux_window_geometry` / `apply_aux_geometry`，
以及 `lib.rs` 的 `aux_window_open_is_singleton_serialized_and_resident`。

---

## 6. 自查清单（提交前逐条打勾）

- [ ] 新增/修改的圆角全部用 `--gosslan-radius-*` token，没有字面值
- [ ] 嵌套面按 §1.3 公式取内圆角（内 < 外）
- [ ] hover 只用 §2.1 白名单里的值；没有硬编码十六进制
- [ ] 交互元素有 press 反馈（全局规则已覆盖，确认没有被 `filter`/背景覆盖掉）
- [ ] **焦点提示按 §2.4**：文本输入类用**边线变色**（不画外圈方框），非文本控件保留键盘焦点环
- [ ] 新增的可点元素要么是 `<button>`，要么带 `cursor-pointer`（否则按下去没反应）
- [ ] 关键动作按 §2.3 给了对应触觉，且**没有每个点击都给**
- [ ] 触屏上小于 44px 的独立小按钮加了 `tap-safe`
- [ ] 新增动效使用 `--gosslan-ease` 与时长令牌，没有另写曲线
- [ ] 危险/警告色用语义 token，且浅深两套都定义了
- [ ] 彩色**文字**走 `*-ink` 档（`--gosslan-accent-ink` / `--gosslan-success-ink`），没有直接用 `text-primary` 或填充色
- [ ] **深色下每个浮起的面都比它所在层亮**（浮层 / 输入框 / 卡片 / toast 不得与底色同值）
- [ ] 改过 token 值的话，`index.html` 骨架里对应的 `--boot-*` 已同步（§4）
- [ ] 没有给窗口根容器或标题栏按钮加圆角
- [ ] 新增的辅助窗口用 `AuxWindowShell`（或直接 `TitleBar`）拿顶部 chrome，**没有自绘第二层标题栏**；
      小窗口没有最大化入口（`.maximizable(false)` + `:show-maximize="false"`）
- [ ] **字号取自 §8 的五档标尺（≥11px），字重只用 400/500/600**
- [ ] **交互先改 UI 再 `await`（§9）；数据未到渲染骨架，不闪"空态"；异步路径必有终态**
- [ ] 若改了 token 或布局尺寸，`index.html` 骨架同步更新
- [ ] 布局尺寸变了的话，同步 `utils/previewMetrics.ts` / `messageHeight.ts`（虚拟列表估算）
- [ ] **图标按钮补了 `aria-label`**（只有 `title` 不算——那是工具提示；§10.2）
- [ ] **每个 `<img>` 都有 `alt`**：装饰性写 `alt=""`，内容图写真描述（§10.2）
- [ ] **没有把操作藏在 hover 后面**：凡用 `group-hover:*` / `opacity-0` 揭示的元素都加了 `.hover-reveal` / `.hover-reveal-op`（§10.3）
- [ ] 新增毛玻璃（`backdrop-filter`）时确认在 `prefers-reduced-transparency` 下有回退（§10.1）
- [ ] 错误提示走 `app.toastError(e, "…")`，没有自己拼 `：${e}`（§9.5）

---

## 7. 本次审计结论（2026-09-10）

已修复：

1. **窗口角白缝**（根因 + 修法见 §5）：根容器 `rounded-xl` 与系统圆角错位；已移除，并同步去掉骨架里的 `12px`。
2. **硬编码危险色**：`#e81123`（关闭键）、`red-500/600`、`amber-500` 等 61 处 → 语义 token（浅深两套）。
3. **同心外翻**：右键菜单项 6px → 4px、表情格子 6px → 4px、代码卡片顶部 8px → 6px。
4. **缺 press 态**：新增全局压暗/提亮规则 + 键盘焦点环。
5. **圆角双词汇表**：99 处 Tailwind 字面圆角类 → token（值 1:1，**零观感变化**，已用产物 CSS 实测核对）。

已知偏差（有意保留，后续按需处理）：

- 高强调按钮未改胶囊（Apple 对 macOS 中等密度控件允许保持圆角矩形）。
- `ConversationList` 的加号下拉菜单项没有圆角（容器 `overflow-hidden` 会裁，功能上正确），  
  若要 hover 有"胶囊项"质感，可补 `rounded-[var(--gosslan-radius-xs)]`。
- 骨架屏仍是字面值（受限于 CSS 变量不可用，见 §4）。

### 7.1 第二轮审计（同日晚）

| 发现 | 处理 |
|---|---|
| **左右两栏头部分隔线不齐**：左栏列表头是 `px-3 py-2` + `h-9` 搜索框 = **52px 且无底边线**，右栏 `ChatHeader` 是 **56px + `border-b`** | 左栏改用同一个 `--gosslan-header-h` + 同款 `border-b` → 分隔线合为一条（渲染实测：两栏同在 y=93.00、亮度一致） |
| 圆角字面值残留 1 处（`MessageCodeBubble` 组件内的 `border-radius: 0 0 8px 8px`） | 改用 `--gosslan-radius-md` |
| 排版散值：`8/9/10px`（低于最小正文档）、`13.5px`（半步值）共 20 处 | 收进标尺：→ `11px` / `13px`（见 §8） |
| **交互审计**：全量核对 store 的 `await api.*`，多数已是乐观更新（`send` 先插 `tmp-*`、`openConversation` 先切 `activeConv`、`removeFriend`/`deleteConversation` 先改本地） | 确认合规；`sendFileTo` 不加乐观气泡是**有意例外**（见 §9.3） |
| **切会话会先闪一句"暂无消息"**（`messages[convId]` 未加载完时是空数组 → 走了空态分支） | 新增加载骨架 + `loadMessages` 失败终态 → 渲染过渡态而非空态（§9.2、§9.4） |

### 7.2 第三轮审计：深色模式全局校准（同日晚）

主题：**主题关联性**（暗色不能是"浅色变量翻一遍"）+ **文字可读性**（实测比值，不靠感觉）。

| 发现 | 处理（改前 → 改后对比度） |
|---|---|
| **11 处硬编码状态色**：`bg-emerald-500` / `bg-neutral-400`（在线点，7 处）、`text-emerald-500/600`、`bg-emerald-500/10` | 收敛到 `--gosslan-success` / `--gosslan-success-ink` / `--gosslan-success-soft` / `--gosslan-status-offline`。绿色文字 **3.88 → 7.24**（暗）/ **3.77 → 5.48**（亮） |
| **`text-primary` 当文字用**（8 处）：面板上只有 3.98（暗）/ 3.68（亮） | 新增 `--gosslan-accent-ink`（`color-mix` 派生：亮色 −28% 黑、暗色 +28% 白）→ **5.90**（暗）/ **6.30**（亮） |
| **暗色危险色文字** `#ff453a` 在列表底上 4.29 | → `#ff5548`，**4.63**（兼顾"白字压红底"的未读徽标：3.16） |
| **暗色搜索框与列表栏同值**（panel = list = `#1e293b`，对比 **1.00** = 输入框消失） | 新增 `--gosslan-field`（暗 `#2a3a52`）→ **1.27** |
| **暗色毛玻璃浮层与列表栏同值**（`rgba(30,41,59,.85)` 复合后 1.00，右键菜单/表情面板等于隐形） | `--gosslan-panel-frost` → `rgba(48,62,88,.9)`，复合 **1.32** |
| **暗色组件边框过淡**（贴列表 1.28，输入框/按钮的边"看不见"） | `--gosslan-border` → `#3a4a66`，**1.64**；`--gosslan-divider` 保持更淡（分隔线是分栏、边框是控件） |
| **暗色 toast 与画布糊在一起**（1.15；亮色下是深灰 HUD，暗色下也深就"沉下去"了） | 新增 `--gosslan-hud`（暗色抬亮一档中性灰）→ **1.81**，白字仍有 9.1:1 |
| **原生控件不跟主题**：`<select>` 暗色下弹白底下拉、滚动条/取色器永远浅色 | 补 `color-scheme: light/dark` |
| **中性卡片气泡色写死在组件里**（`#1c2434` / `#eeeef0`） | 收敛到 `--gosslan-card` / `-ink` / `-line`（**值 1:1，零观感变化**，仅"脱离主题"变"跟随主题"） |
| **代码块工具栏文字亮色不达标**（`rgba(0,0,0,.4)` 在 `#eaeef3` 上 2.79） | 提到 `.55` → **4.59** |

**已知偏差（有意保留）**：

- ~~未新增"跟随系统"外观（需贯通 `darkMode: bool` 的后端契约 → 属功能变更，不在本次范围）。~~
  **已修复（2026-09-10 第五轮）**：外观改为三态 `system / light / dark`，默认跟随系统；
  新增后端键 `appearance_mode`（与既有 `dark_mode` 并用：前者是用户意图、后者是解析结果），
  旧数据按 `gosslan.dark` 迁移为显式模式，`index.html` 骨架判定同步。详见 §10.4 与 CHANGELOG。
- 未读徽标（白字 on `--gosslan-danger`）两套主题都约 3.5——Apple 的计数徽标同样如此，
  属"短数字 + 高强调"的既有取舍，未改；正文位置一律走 `-ink` 档（见 §3.3 偏差表）。
- 亮色危险色**文字**已于第四轮收紧为 `#cc2418`（见 §7.3）。

### 7.3 第四轮审计：亮色（白天）模式可读性校准（同日晚）

主题：把第三轮的**暗色**校准对称地做一遍到**亮色**。方法与第三轮同源（逐对算 WCAG），
但**判据按亮色的实际形态取**，不照搬暗色结论。

| 发现 | 处理（改前 → 改后） |
|---|---|
| **`--gosslan-text-2` 当小字不达标**：说明文案 / 会话摘要 / 占位符大量用 11~13px，落在卡片底 **4.11**、列表底 **4.34** | → Slate-600 `#475569`（按项目既有灰阶**下走一档**）：卡片 **6.54** / 列表 **6.92** / 面板 **7.58** |
| **`--gosslan-danger` 被当文字用**：11px 的「发送失败」、删除按钮、「[有人@我]」、错误提示落在 **3.06~3.55** | 新增 `--gosslan-danger-ink: #cc2418`（面板 **5.48** / 列表 **5.00** / 卡片 **4.73**）；**填充档 `#ff3b30` 不动**（未读徽标 / 危险实底 / hover 底） |
| **`--gosslan-warning` 被当图标用**：群主皇冠、文件夹图标只有 **2.20**，连图形档 3.0 都不到 | 新增 `--gosslan-warning-ink: #c67600`（面板 **3.50** / 列表 **3.20** / 卡片 **3.02**）；填充档 `#ff9500` 不动 |
| **`--gosslan-status-offline` 太淡**：`#a3a3a3` 在白底 **2.52**，低于图形档 | → Apple `systemGray` `#8e8e93`（面板 **3.26**），并与暗色档**同值**（同一个"离线"语义两套外观同一个灰） |
| 共 **20 处** `text-[var(--gosslan-danger/warning)]` 分散在 11 个文件 | 全部改走 `-ink` 档；`bg-[var(--gosslan-danger)]` 等 10 处**填充**用法原样保留 |
| 气泡配色（自己/对方、明暗 × 全主题、@提及） | **已由 `chatStyle.test.ts` 长期覆盖 ≥4.5，本轮无需改动**（亮色自己气泡本就是"浅底深字"，不是白字压主题色） |

**同时新增护栏**：`utils/tokenContrast.ts` + 测试（读真实 `style.css` 核算契约表，测试数 114 → **121**）。

**这轮**校验方式：`npm test` 121/121、全库 `.vue` 分支链扫描干净，
并用无头浏览器渲染"改前/改后"双栏逐像素采样，确认实际渲染色 == 设计值
（改前 `#ff9500`/`#ff3b30`，改后 `#c67600`/`#cc2418`）。

### 7.4 第五轮：Apple HIG 2026 审计后的 P0 修复（同日晚）

主题：**可达性 / 系统跟随 / 错误恢复**。前三轮把"看得见的颜色"校准得不错，
本轮补的是前三轮**照不到的三类**：能用鼠标的人之外的人、系统的辅助功能开关、以及出错时的反馈。

| 发现 | 处理 |
|---|---|
| **触摸端删不掉会话**：删除键 `hidden + group-hover:flex`，Android 无 hover → 永远不显示 | 新增 `.hover-reveal`（`@media (hover: none)` 下常显），并把"CAA 不能只靠悬停"写进 §10.3 |
| **读屏收不到失败**：toast 无 live region；错误只停留 3s | `role="status"` + `aria-live="polite"` + 每条 `aria-atomic`；错误停留 **3s → 6s** |
| **错误文案 = 原始异常**：31 处 `toast(\`…：${e}\`)` | 统一走 `app.toastError(e, "…")` → `utils/errors.ts` 收敛（原文只进 console） |
| **图标按钮无名**：60 余处只有 `title` | 补 `aria-label` **31 个**；新增护栏 `utils/a11yLabels.ts`（全库扫描，§10.2） |
| **发送状态读屏不可见**：回执是纯图标 | 回执容器 `role="img"` + 可访问名 |
| **列表行键盘不可达**：`<div class="cursor-pointer">` | 补 `tabindex` + `role="button"` + Enter/Space（焦点环复用既有全局规则） |
| **未适配降低透明度 / 提高对比度** | 两段媒体查询（§10.1），**必须放文件末尾**否则被后面的 `.glass`/`.frost` 覆盖 |
| **`.tap-safe` 定义了却 0 处使用** | 给 **15 处**独立小图标按钮补上（24~32px）；**判据是"垂直方向有没有紧邻另一个可交互元素"**——导航栏图标栈、输入区工具栏**刻意不加**（会抢走相邻按钮 / 正文区的点击，§2.3） |
| **17 处 `<img>` 全无 alt** | 装饰性头像 15 处 `alt=""`、内容图片 2 处给真实描述（§10.2.1）；护栏同步扩展到 `<img>` |
| **不跟随系统外观** | 三态 `system/light/dark`（§10.4），旧数据迁移不丢偏好 |

**校验方式**：`npm test` 144/144（新增 errors 8 例 + a11yLabels 12 例（按钮 + 图片，均含全库扫描）+ templateBranches 3 例）、
`vue-tsc` 0 错误、`cargo check` 0 error/0 warning；降级 CSS **用无头浏览器读计算值实测**
（`backdrop-filter: none`、`.glass` 背景 `rgba(0,0,0,0.62)`、`.hover-reveal` 的 `display: flex`
证明 `!important` 压过 Tailwind 的 `hidden`、`--gosslan-border: #94a3b8`）——这类"写对了但被覆盖"
的问题靠读代码判断不出来。

---

## 8. 排版标尺（Text Styles）

按 Apple HIG 的**语义档位**映射（Subheadline / Body / Footnote / Caption1 / Caption2），
**不允许随手写中间值**：

| token | 值 | Apple 档位 | 用在哪 |
|---|---|---|---|
| `--gosslan-text-title` | 15px | Subheadline | 会话标题、群名（最大一级） |
| `--gosslan-text-body` | 14px | Body | 消息正文（= `--gosslan-msg-size`，**用户在设置里可改**） |
| `--gosslan-text-callout` | 13px | Footnote | 列表行标题、输入框、次级正文 |
| `--gosslan-text-footnote` | 12px | Caption1 | 摘要、引用块、辅助说明 |
| `--gosslan-text-caption` | 11px | Caption2 | **正文最小可用字号**：昵称、时间、徽标 |

现有代码里用 Tailwind 名的对应关系：`text-sm` = body(14)、`text-xs` = footnote(12)。

**红线**

- ❌ 字号不在五档内。本次审计已把 `8px / 9px / 10px → 11px`、`13.5px → 13px` 全部收进标尺。
- ❌ 小于 11px 的正文（那是 Apple 的最小正文档；徽标里的数字同样按 11px 处理）。
- ❌ 用 Bold/Black 做正文或列表强调——只用 **Regular(400，默认) / Medium(500) / Semibold(600)**。
- ❌ 负 letter-spacing。
- ❌ 字号随窗口宽度缩放（不要 `vw` 字号）。
- ⚠️ 改字号必须同步行高（成对：11↔16 / 12↔20 / 13↔20 / 14↔22 / 15↔24）以及相关高度估算常量。

**合规现状（本次审计）**：字重只有 medium/semibold 两级 ✓；未使用 tracking 负值 ✓；
`--gosslan-msg-size` 被设置页覆盖属于**有意例外**（用户的阅读偏好优先于标尺）。

### 8.1 提示行（系统消息 / 已撤回）

时间线里有一类**不是消息**的条目：消息撤回、文件被下载、群成员变更（加入 / 移出 / 退群 /
群主转让）。微信式的形态是**居中一行小灰字**，与"一条普通消息"必须一眼可分：

| 性质 | 值 | 为什么 |
|---|---|---|
| 字号 / 颜色 | `text-xs`(12) + `--gosslan-text-2` | 状态行不是内容，不能和正文抢注意力 |
| 宽度 | **通栏**（`px-4 text-center`） | 放进消息行会被 `max-w-[72%]` 挤偏，"居中"看着还是像消息 |
| 头像 / 气泡 / 昵称行 | **都没有** | 有头像就成了一条消息；有气泡会让人以为能点开/复制 |
| 交互 | 无右键 / 无长按 / 无表情回应 | 没有可操作内容 |

**判定只有一个来源**：`src/utils/messageKinds.ts` 的 `TIP_KINDS` / `isTipKind()`。
`MessageItem.vue`（渲染）与 `messageHeight.ts`（虚拟列表估高）都必须读它 ——
两边各写一份 `kind === "system"` 就会漏掉 `recalled`，估高与渲染差一行即互相遮挡。
守护测试：`src/utils/messageKinds.test.ts`「提示行的判定只有一个来源」。

---

## 9. UI 优先：不可阻断渲染

**规则：交互事件首先更新 UI，所有 IO 异步且不阻断渲染。数据可以晚到，界面不能等。**

1. **点击 → 同帧出反馈。** 按下有 press（§2.3），选中态/打开态**同步**改，不要 `await` 之后再改。
   范例：`openConversation` 第一行就是 `activeConv.value = id`，之后才异步取消息。
2. **数据未到时渲染"过渡态"，不是"空态"。** 切会话时 `messages[convId]` 还不存在 →
   渲染加载骨架（`ChatWindow` 的 `messagesLoading`），**绝不能先闪一句"暂无消息"**。
3. **乐观更新 + 失败回滚。** 本地状态先改，IPC 失败再回滚并提示。
   范例：发送消息先插入 `tmp-*` 气泡；删除好友先移除行、失败再加回。
   **有意的例外**：文件/图片发送**不加**手工乐观气泡——后端会在返回前同步 emit 完整元数据，
   手工拼的"只有文件名"的记录会与真实记录按 `msg_id` 去重竞态而丢元数据（见 `sendFileTo` 注释）。
4. **异步路径必须有终态。** 成功或失败都要落到一个可渲染的状态，
   否则骨架/加载指示器会永久停留（`loadMessages` 的 `catch` 就是这个作用）。

**性能配套**：反馈只用 paint 级属性（`filter` / 背景色 / `transform`），不触发重排；
长列表一律走 `VirtualList`；语法高亮等重活不得放在点击的同步路径上。

### 9.5 错误提示：走统一出口，不要自己拼异常

失败反馈一律 `app.toastError(e, "发送失败")`，**不要**写 ``toast(`发送失败：${e}`)`` 或 `toast(String(e))`。

原因：IPC 抛出的是 Rust 的 `Err(String)` 原文，可能含 device_id、也可能是 `os error 2` 这类英文库错误。
`utils/errors.ts` 会把它收敛成「能读懂 + 可行动」的一句话（并保证原文进 console、不丢排查线索）。
⚠️ 它**刻意不覆盖**已经写好的中文说明（本项目 Rust 侧大量如此，如「对方不是好友，请先扫描添加好友」）
——"一律换通用文案"会把有用信息抹掉，那是另一种错误。

---

## 10. 系统与辅助功能跟随（2026-09-10 第五轮审计新增）

**总原则：跟随系统，而不是要求用户去改 App 的开关。** Apple 2026 版 HIG 把
*Flexibility*（适应不同环境与需求）重新列为设计原则，并明确要求 Liquid Glass 适配
「降低透明度 / 提高对比度」（macOS 27 另有 "Show Borders"）。以下三条为**硬规则**。

### 10.1 辅助功能媒体必须响应

| 系统设置 | 本项目必须做的事 | 实现位置 |
|---|---|---|
| 降低透明度 | 毛玻璃退化为**不透明实底**并去掉 `backdrop-filter` | `style.css` 末尾的 `prefers-reduced-transparency` 块 |
| 提高对比度 | **边界类** token 提档（`border` / `divider` / `hover` / `window-ring`） | `style.css` 末尾的 `prefers-contrast` 块 |
| 减弱动态效果 | 关掉过渡与动画（已有） | `prefers-reduced-motion` 块 |

两条注意：

1. **只动表面与边界，绝不在这些媒体查询里改文字色** —— 文字可读性由 `utils/tokenContrast.ts`
   的契约表在亮/暗下逐对保证，在这里改会绕过那道护栏。
2. ⚠️ **这些块必须放在 `style.css` 最末尾**：`.glass` / `.frost` 的定义在文件中更靠后，
   CSS 同优先级下"后定义者胜"，写在前面会被**直接覆盖**（首版即踩，已用无头浏览器实测确认）。

### 10.2 可访问名是准入项（不再是"有 title 就行"）

图标按钮**必须**有 `aria-label`；`title` 只是鼠标工具提示，触屏与读屏都不保证读得到。
两者可以并存（各服务一种输入方式）。

- **护栏**：`utils/a11yLabels.ts` + `a11yLabels.test.ts`（`npm test` 会扫描 `src` 下全部 `.vue`），两组断言：
  - `findUnlabeledButtons` —— 报"纯图标且没有任何名字来源"的 `<button>`；
  - `findUnnamedImages` —— 报"没有 alt"的 `<img>`。
  **新增无名按钮或漏写 alt 都会让测试变红。**
- 组件若自身只有图形（如 `SettingsToggle`），把名字做成**必填 prop**（`label`），
  让"无名开关"在编译期就过不去，而不是靠自觉。
- 纯图标状态（如消息回执的转圈/绿勾）用 `role="img"` + 可访问名 —— 否则读屏用户
  完全不知道消息发出去了没有。

> ⚠️ 护栏的已知盲区：内部含 `{{ 表达式 }}`（如未读徽标、头像首字母）的按钮会被当作"有文本"而跳过。
> 导航栏的「聊天 / 通讯录」正是这类，它们的名字会退化成裸数字，**仍需人工判断**。

### 10.2.1 图片 alt：装饰性与内容要分开

判据是「**出现过** `alt` 属性」而不是「`alt` 非空」——`alt=""` 是 HTML 规范里
"这是装饰、读屏请跳过"的**正确**写法，真正要禁的是"忘记写 alt"。

| 类型 | 写法 | 例 |
|---|---|---|
| 装饰性头像（旁边已有姓名，或所在行已有可访问名） | `alt=""` | 会话行 / 好友行 / 申请行 / 成员列表里的头像 |
| 内容图片（图片本身就是信息） | 真实描述 | 消息内图片 `alt="图片消息"`；预览用 `:alt="current?.name \|\| '图片预览'"` |

❌ 不要给装饰性头像写人名 —— 读屏会把同一个名字念两遍（一次来自行标签、一次来自 alt）。

### 10.3 悬停不能是唯一入口

任何用 `group-hover:*` 或 `opacity-0` 揭示的元素，都必须同时加 `.hover-reveal`（显示）
或 `.hover-reveal-op`（不透明）——这两个类在 `@media (hover: none)` 下退化为常显。

真实事故：会话行的删除键写成 `hidden` + `group-hover:flex`，**Android 上永远不显示**
——桌面能删、手机删不掉（见 §7.4 / CHANGELOG）。同层的"删除好友"因为有长按兜底才幸免。

配套：触屏上小于 44px 的独立小按钮加 `tap-safe`（见 §2.3）。注意该规则 2026-09-10 之前
**定义了但全库 0 处引用**，属"护栏写了没人用"的典型，code review 时一并核对。

### 10.4 外观三态：跟随系统是默认

- 用户意图存 `settings.appearance_mode`（`system` | `light` | `dark`），
  `settings.dark_mode` 是**解析后的结果**（跟随系统时由前端按系统偏好算出后回写），二者并存不冲突。
- store 里 `dark` 是 **computed**（`system → 系统偏好，否则强制值`）：既有 `app.dark` 读取处零改动，
  写入统一走 `applyAppearance()` 这一个出口。
- **`index.html` 的骨架判定必须与 store 完全一致**（骨架先于 bundle 执行，不一致就会看到
  "骨架浅色 → 界面深色"闪一下）。改外观逻辑时两处一起改。
- 强制模式下系统外观变化**不得**影响界面；跟随模式下系统变化要**即时**生效（监听 `prefers-color-scheme`）。
