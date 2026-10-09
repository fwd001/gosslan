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
- **任何内容浮层都不许盖住 caption（用户 2026-09-30：「图片预览独立窗口会遮住头部标题栏
  关闭最小化那块，应该只在下面内容区，标题栏优先度最高」）**：`TitleBar` 自带 `z-[85]` ——
  高于弹窗 `65` / 右键菜单 `70` / 操作面板与图片预览 `80`，只有 Toast（`90`，`pointer-events-none`）
  允许浮在它上面（失败提示被压住等于没有反馈）。完整阶梯只写在 `src/components/BaseModal.vue`
  那段注释里，**新增浮层请把层级留在 80 及以下**：超过 85 会被设计判据 ㉕
  （`src/utils/designGuards.test.ts`，从源码现算，不抄数字）当场判红。
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

### 8.2 点名（@）的视角规则（用户 2026-09-26 提，#85）

**同一条消息里同一个 @，自己看和别人看不一样**：

| 谁在看 | 看到什么 | 为什么 |
|---|---|---|
| 被点到的本人 | **`@你`**，且**加重**（`.mention-token--self`） | 名字对自己没有信息量，"在叫我"才是；扫一眼就要能分出来 |
| 其他人 | 仍然是 `@张三` | 别人需要知道点的是谁，折成"你"就废了 |

规则只有**一个来源**：`src/utils/linkify.ts` 产出 `kind: "mention" | "mention-self"`，
渲染端（消息气泡与内容详情弹窗两处）按 kind 挂类名，**不在组件里再比一次名字**。

⚠️ 一条写过的判据差点埋掉的坑：**同前缀昵称**。`张三` 是自己、`张三丰` 是别人时，
`@张三丰` 必须还是 `@张三丰`，不能被前缀匹配折成 `@你` —— 这条有专门用例钉住
（`linkify.test.ts`「@张三丰 @张三」那组）。

**修饰类必须有定义**（本轮新加的护栏）：`designGuards::findModifierClassWithoutStyle`
扫全部 `.vue`，凡是模板上挂了含 `--` 的修饰类而全局 `style.css` 与本文件 `<style>` 里都没有定义 ⇒ 报红。
理由：这类退化的形态是**界面静默变朴素** —— 渲染不报错、类型不报错、测试不报错，
只有用户看得见"高亮没了"。

覆盖面是**模板 class 属性上写死的类名**，三种真实绑定写法各认一处：静态 `class="a b--c"`、
对象 `:class="{ 'b--c': x }"`、三元与数组 `:class="ok ? 'b--c' : ''"` / `:class="['b--c']"`。
**不覆盖在脚本里拼出来的类名**（`:class="[rowLevelBg(r.level)]"` 那类函数返回值）—— 静态扫不出来。
这一格"到底认出了几枚"不写进文档（写了就会漂），由 `designGuards.test.ts`
「全库扫到的修饰类清单要非空转」现算：少于下限即红，红消息里直接打印认出的清单。

⚠️ **这条护栏第一版就漏过一枚**：它当时只认静态与对象两种写法，而筛选 chip 的选中态
（`.gosslan-filter-chip--on`，`ChatSearchDialog.vue`）写的是三元形式 ⇒ 那枚类挂上去也没人守，
且全库扫描**照样绿**（因为该类的定义本来就在）。是"把文档里的枚数拿去现算"这一步抓到它的，
不是那条判据自己报的 —— 通用一条：**判据的覆盖面要靠现算输入清单来核，不能只信判据没红**。
两处实测（都一正一反）：把 `.mention-token--self` / `.gosslan-filter-chip--on` 的定义改名
⇒ 全库扫描各自立刻报红并指到用法行；撤掉即绿。

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

> ⚠️ 护栏的已知盲区：内部含 `{{ 表达式 }}`（如未读徽标、默认头像的 emoji）的按钮会被当作"有文本"而跳过。
> 导航栏的「聊天 / 通讯录」正是这类，它们的名字会退化成裸数字，**仍需人工判断**。

### 10.2.1 图片 alt：装饰性与内容要分开

判据是「**出现过** `alt` 属性」而不是「`alt` 非空」——`alt=""` 是 HTML 规范里
"这是装饰、读屏请跳过"的**正确**写法，真正要禁的是"忘记写 alt"。

| 类型 | 写法 | 例 |
|---|---|---|
| 装饰性头像（旁边已有姓名，或所在行已有可访问名） | `alt=""` | 会话行 / 好友行 / 申请行 / 成员列表里的头像 |
| 默认头像（emoji 小动物，#32） | `aria-hidden="true"` | 与上一行同口径：图形是装饰，人名由旁边的文本承载。判据 `designGuards::findAvatarFaceWithoutAriaHidden` 扫全部 `.vue` |
| 内容图片（图片本身就是信息） | 真实描述 | 消息内图片 `alt="图片消息"`；预览用 `:alt="current?.name \|\| '图片预览'"` |

❌ 不要给装饰性头像写人名 —— 读屏会把同一个名字念两遍（一次来自行标签、一次来自 alt）。

**默认头像长什么样（#32，2026-09-27）**：没上传头像时不再是「按昵称截字」，而是
**emoji 小动物 × 逐对挑过的背景色**（`src/utils/avatarSeed.ts`，组合表 108 对，条数由判据现算）。
种子是**设备 id / 群 id**，不是昵称 —— 两条理由都不会自己守住：昵称可改（改一次换一张脸）、
昵称也能撞（两个人同名就同脸）。现算的额外好处是**不新增跨设备同步字段**：
任何一端拿到同一个 id 就复算出同一张脸，换设备、重装、清缓存都不变。
字母那条路已由 `designGuards::findRetiredLetterAvatarUsage` 判成「不许复活」。

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

---

## 11. 原生体验：目标 / 结构性账 / 验收口径（2026-10-09 并入本文件）

> **这一节的交互规则已经有不变量家了**：`docs/protocol-invariants.md` 的 **INV-P31** ——
> 悬停揭示必须有非悬停可达路径、小尺寸热区（连同 `tap-safe` 只撑垂直方向这条量程边界）、
> 关闭之后的两个终态（DOM 撤净 + 焦点归还）、以及「先证帧在出再谈还剩没剩」那条量具纪律。
> 这一节继续讲怎么做；**规则本身以 INV-P31 为准**，同一句规矩不在两处各写一遍，
> 否则两处会各自漂（本仓为这件事真翻过车）。

> **为什么不新开 `docs/native-experience-standard.md`**：本文件 §1–§10 已经是原生体验的"公共层"
> （token、交互态、动效令牌、无障碍、窗口边界）。另起一份只会造出**第二个真源**——
> 本仓在"同一个关注点有两个家"上付过真实代价（`docs/AI_ENGINEERING_INDEX.md` 第 4、5 条前面那段）。
> **任务清单也不另建**：原生体验的格子在 `docs/stability-roadmap.md` §12.6.1（N1–N8，已拆出 #127–#130）；
> **性能基线在 `perf/README.md`**（那一节开头是"量具自检"，先读它再读任何数字）。

### 11.1 目标与非目标

| 目标（做对了才算） | 非目标（明确不做） |
|---|---|
| 反馈即时：按下同帧变色（§2.3 两条铁律） | 换 UI 框架 / 引第二套动画系统 |
| 滚动与列表不抖、不闪、不空（§11.2） | 复刻 iOS 外观（胶囊按钮、大圆角、玻璃拟态铺满） |
| 触摸、鼠标、键盘三条输入路径都能走完（§2.4、§10.2、§10.3） | 为"原生感"加没有信息量的动效、粒子、视差 |
| 平台习惯走系统开关（§10.1、§10.4），不要求用户改 App 设置 | 为视觉效果碰协议 / 加密 / DB / IPC / 消息状态机 |
| 长会话在低端设备仍可用（`perf/README.md`） | 宣称没跑过的平台已通过（一律记 UNVERIFIED） |

### 11.2 列表与滚动：**虚拟化的输入必须现读，不许只在挂载时读一次**

这是原生体验里唯一"结构性"的一条：流畅度不是靠加过渡买来的，是靠不渲染不该渲染的东西、
不重算不该重算的东西。**输入坏一次，整页的流畅度判断全部失真。**

- 规则：`viewport`（可视区高度）这类输入**必须由 `ResizeObserver` 跟着容器真实尺寸更新**。
  只有 `onMounted` + `window` resize 是两个不够的读取点 —— 移动端软键盘弹出、断点切成整页形态、
  常驻辅助窗口重新显示、面板开合挤压布局，这四类路径**都不发 `window:resize`**。
  旧值偏大 ⇒ 整表被算成"可见"（全量渲染）；旧值偏小 ⇒ 滚到下方出现空白。**两个方向都是缺陷。**
- 尺寸读不到 0 时**不写入**（常驻窗口关闭即隐藏 / `display:none`），保留上一个可用值 ——
  与 `VirtualList.commitHeight` 拒绝 0 高度是同一个失败方向（写 0 会让整列塌到顶部）。
- **量具必须自证它真的动过**：`perf/probe.mjs` 现在在采数前打印 `scrollerReal`
  （写一次 `scrollTop` 读不回同样的值就是容器根本不是滚动容器）。这条不是抽象洁癖：
  它坏过整整一次，症状是"帧间隔 p50 = 16.7ms 满帧"，而 16.7ms 正是**一个什么都不做的空闲帧**；
  同期那句"估算调用数 0 ⇒ 稳态滚动不重算 ✓"也是假的 —— **调用数为 0 是因为没滚过**。
  复跑：`node perf/probe.mjs 9223`（三步启动法见 `perf/README.md`）。

### 11.3 动效：本应用的档位就是两个，不要为凑齐"标准档位"造没人用的 token

| token | 值 | 用在哪 |
|---|---|---|
| `--gosslan-ease` | 强 ease-out | 所有过渡的曲线（Tailwind `.transition` 由 `style.css` 统一覆盖） |
| `--gosslan-duration-fast` | 150ms | 轻反馈（hover/press/小元素） |
| `--gosslan-duration` | 240ms | 结构性变化（浮层、面板、尺寸） |

- **没有"页面切换 200–320ms"这一档，是有意的**：本应用是多窗口架构（§5），没有路由级过场，
  造一个没人消费的 token 只会变成下一个"定义了但 0 处引用"（§10.3 那个真实事故）。
- **主题色相关的规则必须写在顶层**（判据 ㉚ `findNestedThemeRuleIssues`）：`::selection` 与 `accent-color`
  一旦被写进别的规则的括号里（例如 `input,textarea,…{ … }`），原生 CSS 嵌套会把它们编译成**后代**选择器
  （`input ::selection` / `[contenteditable=""] progress`）⇒ 整条匹配不到任何东西。
  这种写法**源码里看得见、判据扫得到、界面上没有** ⇒ 这一族只能判**嵌套深度**，
  而且核对要读 `dist/assets/*.css`（读 `src/style.css` 不足以证明它生效）。
- **动画系统零新增依赖**：Motion / GSAP / @vueuse/motion 都不在 `package.json` 里（现算：
  `node -e "const d=require('./package.json');console.log(Object.keys({...d.dependencies,...d.devDependencies}).filter(k=>/motion|gsap|anime|framer/i.test(k)))"` ⇒ 应为空数组）。
  要引入必须先给出"哪个具体交互靠 CSS transition 做不到"的证据，并且一次只引一套。
- 动画完成 ≠ 业务完成：反馈可以立刻画，状态必须等真实回执（§9.3 乐观更新 + 失败回滚），
  不许用动效掩盖失败（§9.5 错误统一出口）。

### 11.4 平台分层：**这一条目前是"待收口"，不是"已生效的规矩"**

现状（先记账，别写成已经做完了）：`src/utils/platform.ts` 是平台判定的**命名出口**
（`isMac` / `isAndroid` / `isIOS` / `resolveMobileLayout`），但**组件里散写这三个标识符的文件不止一个**
⇒ "所有平台分支都走这一个家"**今天不成立**。

- 计数**不在这里写数字**（写了就会漂，而且本文件 §8.2 已经记过一次"判据覆盖面要靠现算核对"的教训）。
  现算口径固定为：`grep -rlE '\b(isMac|isIOS|isAndroid|isWindows|isMobile)\b' src/components --include="*.vue" | wc -l`
  （**`--include` 必须加引号**，不加引号在 zsh 下会被 glob 吞掉、报 `no matches found`，那条空结果不是"搜过没有"）。
- 收口方向（属可维护性 + 原生体验，按优先级排在稳定/流畅之后，**不在本轮顺手做**）：
  新增交互一律不自己判断平台，先问 CSS 能不能表达（`@media (hover: none)`、`env(safe-area-inset-*)`、
  `prefers-*`），不能表达才走 `utils/platform.ts`；已散写的那几处等一次专门的收口改动
  （散写点搬家会动到 §11.2 那条量具和窗口生命周期，必须单独一次可回滚的改动 + 单独一次门禁）。
- 公共层（不随平台变）：§1 圆角、§2 交互态、§3 配色、§8 排版、§9 UI 优先、§10 无障碍与系统跟随。
- 适配层（只在真实需要时存在，且必须是**有名字的一处**）：`utils/platform.ts`（判定）、
  `utils/haptics.ts`（触觉，§2.3 含三端能力差异）、`env(safe-area-inset-*)`（安全区）、
  `components/TitleBar.vue` + `components/window/AuxWindowShell.vue`（窗口 chrome，§5）、
  `composables/useBackLayer.ts`（Android 返回层；iOS 侧滑 wry 未开 ⇒ 页内返回按钮，那条注释是准的）。

### 11.5 验收口径：每一项要么挂得上复跑命令，要么明写未测量

| 维度 | 今天的判据（**可复跑**） | 今天测不到的（不许从这里读出结论） |
|---|---|---|
| 流畅度 | `perf/probe.mjs` 的 `scrollerReal` + `virtualizationOK` 双真，再 `perf/run.mjs` 的 `longFramesOver50ms` 与 p95 | **真机 60Hz/高刷设备上的实际帧表现**；低端 Android |
| 结构性成本 | 同上的 `offsetsRebuilds`（整表前缀和重算次数）——**它现在是"每帧一次"这一格开着** | 10 万档（`run.mjs` 180s 超时没跑完，归因未定，见 `perf/README.md`） |
| 响应性 | §2.3 按下瞬时 + `:active` 里 `transition-duration: 0s`（由 `designGuards` 守着）；§9.1 点击同帧改 UI；**输入到首帧的毫秒数现在有量具了** —— `perf/latency.mjs`（CDP 真鼠标 → 真实 `BaseModal` 面板第一次带上计算样式那一帧），2026-10-10 基线 p50 15.5 / p95 17.4 ms、reduce 档 p50 14.5 / p95 15.6 ms | **真机 WebView**（WKWebView / WebView2 / Android）的输入到首帧：未测；且这台量具**分辨率就是一个采样帧**（≈16.7 ms）⇒ 不许拿它论证一帧之内的快慢，也不含网络与数据库 |
| 稳定性 | `npm test`（含 `tokenContrast` / `a11yLabels` / `popupRegistry` / `VirtualList` 卸载出口那条护栏）+ `verify-guards.py` | 长时间运行后的内存曲线：未测量 |
| 无障碍 | §10 四条（可见焦点、可访问名、hover 不是唯一入口、系统三开关）各有判据 | 读屏软件真机播报（NVDA/VoiceOver）：未测；`::selection` 那一格见 `docs/stability-roadmap.md` §12.6.1 的 N7 |
| 资源开销 | `npm run build` 后现量 dist，**两个口径一起报别混**：`du -sk dist`（按文件系统块，2026-10-10 = 2828 KB）与「逐文件字节求和」（同一天 = 2,479,504 B ≈ 2.4 MB），两者差约 15% 是块大小不是体积涨了；chunk 直接引 vite 自己那行打印（十进制 kB + gzip），别自己换算成 KiB（同一个 main chunk 是 `297.66 kB / gzip 90.09 kB`，换算成 KiB 就变成 291 —— 那不是回归，那是单位）。依赖数现读 `package.json`：`node -e "const p=require('./package.json');console.log(Object.keys(p.dependencies).length, Object.keys(p.devDependencies).length)"`（2026-10-10 = **13 9**；⚠️ 这里必须用 `Object.keys(...).length` —— `dependencies` 是对象不是数组，写 `p.dependencies.length` 会印 `undefined undefined`，这条命令本身就被这样抓出来过一次） | Windows / Android 安装包体积（出包机不在本机） |

**当前真数（2026-10-10 现跑，跑法见上面两行）**：`npm test` **869 例 0 失败**（同日 earlier 861 ⇒ 本轮 N16 与 N22 各加 4 条静态判据，两条改动都动过应用码）；
生产构建 `dist` 按块 **2828 KB** / 按字节 **2.4 MB**，最大单个 chunk 是入口那份（vite 打印 **297.66 kB，gzip 90.09 kB**，19 个 js 合计 973 KiB）；
依赖 **13 运行时 + 9 开发 = 22 项**，**动画库 0 项**（现算：那 13 项里没有任何动效库 —— 复跑就是把上面那行列名单的命令改成打印名字）。
⚠️ chunk 的**文件名带 hash，别抄文件名**，要引用就引"跑 `npm run build` 后看 vite 打印那几行"。
⚠️ 这一段自己就是"数字会当天漂"的现场：它原先写的是 855 例、且响应性那格写着"没有量具，记未测量" ——
两处都在 2026-10-10 这天被现跑的数与 `perf/` 第三台量具推翻，所以口径改成**只报命令 + 报数并注明是哪天现量的**。

### 11.6 禁止事项（汇总本文件已有的红线，不新立规矩）

1. 不为视觉碰协议 / 加密 / DB 结构 / IPC 契约 / 消息状态机 / 传输 / 群同步（`AI_RULES.md`、`docs/protocol-invariants.md`）。
2. 不删、不跳、不降、不伪造判据；**静态源码断言不等于真实界面通过**（§11.5 那一列"测不到的"就是这么来的）。
3. 不给窗口根容器或标题栏按钮画圆角（§5）；不让浮层盖住 caption（§5 的 z 阶梯，超过 85 会被判红）。
4. 不把操作藏在 hover 后面（§10.3 真实事故：Android 上永远不显示）。
5. 不在辅助功能媒体查询里改文字色（§10.1：那会绕过 `tokenContrast` 的契约表）。
6. 改字号/行高必须同步行高配对与高度估算常量（§1.4、§8）——**否则虚拟列表错位**，这一条与 §11.2 是同一件事的两面。
7. 动画不许成为操作的前置条件，也不许在 `prefers-reduced-motion` 下留下没有反馈的死角。

### 11.7 例外怎么记（沿用本文件既有格式）

例外必须写全三样：**这是哪一条的例外 / 为什么它是例外 / 什么时候这条例外失效**。
现存的三例：`sendFileTo` 不加手工乐观气泡（§9.3，后端会先 emit 完整元数据）、
骨架屏允许字面值（§4，跑在 bundle 之前读不到 CSS 变量）、`--gosslan-msg-size` 允许偏离五档标尺
（§8，用户阅读偏好优先于标尺）。**新增例外照这个格式，不许只写"这里特殊"**。

### 11.8 回归要求（改动半径决定要跑哪一层）

| 动了什么 | 至少要跑 |
|---|---|
| token / 配色 | `npm test`（`tokenContrast` 读真实 `style.css`）+ `index.html` 骨架同步（§4） |
| 列表 / 滚动 / 虚拟列表 | `node perf/probe.mjs`（两个自检都必须 true）；动了估高还要 `messageHeight` 那组用例 |
| 浮层 / 弹窗 / 键盘 | `npm run test:ui-runtime`（仓内 CDP 探针，真键盘事件 + 真 DOM，见 `docs/stability-roadmap.md` §12.6.1 那条改写） |
| 窗口 / 辅助窗口几何 | `npm test` + `verify-guards.py`（`aux_window_*` 那几条锚点） |
| 源文件搬家 / 大改 | `python3 scripts/verify-guards.py --list`（秒级、不注入）——**锚点会被搬家静默弄死** |

### 11.9 弹窗关闭之后：**先证帧在出，再谈"还剩没剩"**（2026-10-10 两轮实测）

浮层的关闭验收要写成**命中测试**（同一个坐标再真点一次，打不打得到底下的东西），不是 class、
也不是"面板 opacity 是 0" —— 这条不变。但同一格上还挂过一条**已被证伪的结论**，值得留在文档里：

> 一版登记为 P1 的「✕ 关完之后 portal 里留一层看不见但吃掉点击的浮层」（四处现场都复现，
> 包括生产形状的宿主），**不是应用的行为**：headless 探针页 `visibilityState=hidden`、静止时
> 不出帧，而 Headless UI 的过渡收尾要靠 `disposables.nextFrame`（两层 `requestAnimationFrame`）
> ⇒ 离开过渡永远停在第一步，类名停在 `leave-from`（`opacity-100 scale-100`），壳一直挂着。
> 泵帧之后同一批现场全部 `dlg=0`。**`BaseModal.vue` 一字未改。**

所以本节的规则（新立浮层时适用，与层级阶梯 ㉕ 同层，`BaseModal` 是唯一的外壳）：

- **关闭的验收按"同一个坐标再真点一次，打不打得到底下的东西"写**，不按"看得见吗"写。
  看不见却还接得住点击的层，自查最难，用户读到的症状是"点了没反应"。
- **读"关掉之后"之前，先让量具自证在出帧。** 凡靠 CSS 过渡收尾的组件（Headless UI 的
  `TransitionChild` / `TransitionRoot`、Vue 的 `<Transition>`、WAAPI）都要两帧才收尾；页面冻住时
  读到的是**停在中间的壳**——那是量具的形状，不是界面的形状。探针里的落点：`boot()` 那条
  「量具在出帧」（泵 6 次 `captureScreenshot` 期间 rAF ≥ 2 次；这条红是**量具红**，不是产品红）
  与 `readAfterClose()`（先泵帧再读），见 `scripts/check-ui-runtime.mjs` 的 `runOverlay`。
- **焦点要还回去**（N14，2026-10-10 已修）：关完之后 `document.activeElement` 要回到打开它的那个元素。
  外壳里唯一的落点是 `BaseModal` 的 `@after-leave` + `nextTick` —— 早一步 `Dialog` 还挂着，
  它自己的 FocusSentinel 会把刚设的焦点抢回弹窗内部；且只在焦点确实掉到 `body` 时才还，
  带 `preventScroll: true`（归还焦点不许顺手把底下那页滚走 —— 约束 7）。
- 关闭有两条实现路径（Headless UI 自己那条 vs 宿主直接翻 `open`），**两条都要量**。
  上一版正是拿"两条路径结果不同"当成了产品结论；实测差别其实不在谁发起关闭，
  在**读的那一瞬间帧出没出**。

**仍未尽的一半**：真机（WKWebView / WebView2 / Android）上"关闭那一刻窗口正好被遮挡 ⇒ rAF 停"
会不会留下同样的中间态、以及窗口重新显示时它会不会自愈，**未实测**；人工步骤归 Smoke-11。

