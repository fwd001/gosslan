/**
 * 语义 token 的可读性护栏 —— 把「配色契约」从注释里挪成可执行断言。
 *
 * 为什么需要它：颜色是最容易悄悄漂移的东西。token 值改一个字符、新增 token 忘了在
 * `.dark` 里成对定义、把「填充档」当文字用 —— 这些都不会让编译失败、也不会让任何
 * 行为测试变红，只有肉眼在某个特定底色上才能发现。2026-09-10 的亮色审计里，
 * `--gosslan-text-2`(4.34) / `--gosslan-danger` 当文字(3.55) / `--gosslan-warning`
 * 当图标(2.20) / `--gosslan-status-offline`(2.52) 四处都属这一类。
 *
 * 做法：直接读 `src/style.css`，解析 `:root` 与 `.dark` 两个作用域的 token，
 * 按契约表逐对算 WCAG 对比度。改 CSS 就会立刻在测试里反映出来 —— 契约不是抄一份数值，
 * 而是绑定真实声明。
 */

import { contrastRatio } from "./color.ts";

export interface TokenPair {
  /** 前景 token 名（如 "--gosslan-text-2"）或字面 hex */
  fg: string;
  /** 背景 token 名或字面 hex */
  bg: string;
  /** 背景若是半透明（rgba），先与这个面复合后再算 */
  bgOver?: string;
  /** 该组合的最低对比度 */
  min: number;
  /** 这个组合出现在哪 —— 让 reviewer 能判断该不该动它 */
  why: string;
}

export interface ContrastFailure extends TokenPair {
  ratio: number;
  fgResolved: string;
  bgResolved: string;
}

interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

const HEX6 = /^#[0-9a-f]{6}$/i;
const HEX3 = /^#[0-9a-f]{3}$/i;
const RGB_FN = /^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*(?:,\s*([\d.]+)\s*)?\)$/i;

function toHex(c: Rgba): string {
  const ch = (v: number) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0");
  return `#${ch(c.r)}${ch(c.g)}${ch(c.b)}`;
}

/** 解析字面色值；认不出（color-mix 等）返回 null。 */
function parseColor(value: string): Rgba | null {
  const v = value.trim();
  if (HEX6.test(v)) {
    return { r: parseInt(v.slice(1, 3), 16), g: parseInt(v.slice(3, 5), 16), b: parseInt(v.slice(5, 7), 16), a: 1 };
  }
  if (HEX3.test(v)) {
    const [r, g, b] = v.slice(1).split("").map((c) => parseInt(c + c, 16));
    return { r, g, b, a: 1 };
  }
  const m = RGB_FN.exec(v);
  if (m) return { r: +m[1], g: +m[2], b: +m[3], a: m[4] === undefined ? 1 : +m[4] };
  return null;
}

/** 把半透明色复合到不透明底色上。 */
function composite(fg: Rgba, bg: Rgba): Rgba {
  return {
    r: fg.r * fg.a + bg.r * (1 - fg.a),
    g: fg.g * fg.a + bg.g * (1 - fg.a),
    b: fg.b * fg.a + bg.b * (1 - fg.a),
    a: 1,
  };
}

/**
 * 取出 `:root { … }` 与 `.dark { … }` 两个作用域的 token 表。
 *
 * ⚠️ 必须先剥注释、并把选择器锚定到行首：本项目 :root 的注释里就写过
 * `` `.dark { --gosslan-primary: … }` ``（用来说明 inline 样式优先级），
 * 朴素的 indexOf(".dark {") 会匹配到那句注释，把整份表解析错。
 */
export function parseTokenScopes(css: string): { light: Map<string, string>; dark: Map<string, string> } {
  const clean = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const grab = (selector: string): Map<string, string> => {
    const out = new Map<string, string>();
    const m = new RegExp(`(?:^|\\n)\\s*${selector.replace(".", "\\.")}\\s*\\{`).exec(clean);
    if (!m) return out;
    const open = clean.indexOf("{", m.index);
    const end = clean.indexOf("\n}", open);
    const block = clean.slice(open, end < 0 ? clean.length : end);
    for (const t of block.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) out.set(t[1], t[2].trim());
    return out;
  };
  return { light: grab(":root"), dark: grab(".dark") };
}

/** 解析 token 值到不透明 hex；支持 var() 链与 rgba 复合。 */
export function resolveToken(name: string, scope: Map<string, string>, over?: Rgba): Rgba | null {
  const seen = new Set<string>();
  let cur = name;
  while (cur.startsWith("--")) {
    if (seen.has(cur)) return null; // 循环引用
    seen.add(cur);
    const raw = scope.get(cur);
    if (raw === undefined) return null;
    const varMatch = /^var\(\s*(--[\w-]+)\s*(?:,\s*(.+))?\)$/.exec(raw.trim());
    if (varMatch) {
      cur = varMatch[1];
      if (!scope.has(cur) && varMatch[2]) cur = varMatch[2].trim();
      continue;
    }
    const c = parseColor(raw);
    if (!c) return null;
    return over && c.a < 1 ? composite(c, over) : c;
  }
  const c = parseColor(cur);
  if (!c) return null;
  return over && c.a < 1 ? composite(c, over) : c;
}

/**
 * 配色契约：每条都对应界面上一处真实组合。
 *
 * `min` 的取值原则：
 *   —— 文字（含 11px 小字）一律 4.5（WCAG AA 正文档）；
 *   —— 只作图标/状态点等非文字的 3.0（WCAG 非文本档）。
 *
 * 有意低于上述标准的组合**不写进契约**（写进来会让护栏失去意义），
 * 而是在 docs/design-guidelines.md §3.3 的「已知偏差」里逐条记明原因与实测值。
 */
export const TOKEN_CONTRAST_CONTRACT: Record<"light" | "dark", TokenPair[]> = {
  light: [
    { fg: "--gosslan-text", bg: "--gosslan-chat", min: 4.5, why: "消息正文（对方气泡旁）/ 画布" },
    { fg: "--gosslan-text", bg: "--gosslan-list", min: 4.5, why: "会话列表标题" },
    { fg: "--gosslan-text-2", bg: "--gosslan-panel", min: 4.5, why: "设置页说明文字（11px）" },
    { fg: "--gosslan-text-2", bg: "--gosslan-list", min: 4.5, why: "会话列表摘要（12px）" },
    { fg: "--gosslan-text-2", bg: "--gosslan-card", min: 4.5, why: "文件气泡里的辅助信息（10px）" },
    { fg: "--gosslan-text-2", bg: "--gosslan-chat", min: 4.5, why: "空态 / 占位提示" },
    { fg: "--gosslan-list-active-text", bg: "--gosslan-list-active", min: 4.5, why: "选中的会话行标题" },
    { fg: "--gosslan-accent-ink", bg: "--gosslan-panel", min: 4.5, why: "主题色当文字（链接 / 未读标签）" },
    { fg: "--gosslan-accent-ink", bg: "--gosslan-list", min: 4.5, why: "同上，落在列表底上" },
    { fg: "--gosslan-success-ink", bg: "--gosslan-panel", min: 4.5, why: "在线 / 成功状态文字" },
    { fg: "--gosslan-success-ink", bg: "--gosslan-list", min: 4.5, why: "同上，落在列表底上" },
    { fg: "--gosslan-danger-ink", bg: "--gosslan-panel", min: 4.5, why: "「发送失败」/ 删除按钮 / 错误提示" },
    { fg: "--gosslan-danger-ink", bg: "--gosslan-list", min: 4.5, why: "「[有人@我]」标签" },
    { fg: "--gosslan-danger-ink", bg: "--gosslan-card", min: 4.5, why: "文件气泡内的失败文案" },
    { fg: "--gosslan-danger-ink", bg: "--gosslan-danger-soft", bgOver: "--gosslan-panel", min: 4.5, why: "危险软底上的文字" },
    { fg: "--gosslan-warning-ink", bg: "--gosslan-panel", min: 3.0, why: "群主皇冠 / 文件夹图标（非文字）" },
    { fg: "--gosslan-warning-ink", bg: "--gosslan-card", min: 3.0, why: "同上，落在卡片底" },
    { fg: "--gosslan-status-offline", bg: "--gosslan-panel", min: 3.0, why: "离线状态点（非文字）" },
    { fg: "#ffffff", bg: "--gosslan-hud", bgOver: "--gosslan-chat", min: 4.5, why: "toast 白字（hud 底是半透明，按叠在画布上算）" },
    // 徽标里的数字是 11px 白字压在**实底色**上，是这套里最容易漏的一类：
    // 它既不是"文字 token / 背景 token"的组合，也从来没被写进契约表。
    { fg: "#ffffff", bg: "--gosslan-primary-active", min: 4.5, why: "未完成任务蓝色徽标的数字（11px）" },
    // ⚠️ 同一形状的红徽标**实测不达标**，但没有登记成判据（登记了就会把全部门禁钉死）：
    //   白字 on `--gosslan-danger` = 亮 **3.55** / 暗 **3.16**，低于文字档要求的 4.5。
    //   ⇒ 站内**每一个**未读徽标上的数字从来就不合格，只是这张表过去没覆盖到它。
    //   为什么不当场改色：`--gosslan-danger` 是全站最显眼的填充色，动它是用户可见的设计决定，
    //   要用户点头（备选：徽标改用 `--gosslan-danger-ink` 那档深红）。已在 roadmap 记成待拍板一格。
  ],
  dark: [
    { fg: "--gosslan-text", bg: "--gosslan-chat", min: 4.5, why: "消息正文 / 画布" },
    { fg: "--gosslan-text", bg: "--gosslan-list", min: 4.5, why: "会话列表标题" },
    { fg: "--gosslan-text-2", bg: "--gosslan-panel", min: 4.5, why: "设置页说明文字" },
    { fg: "--gosslan-text-2", bg: "--gosslan-list", min: 4.5, why: "会话列表摘要" },
    { fg: "--gosslan-text-2", bg: "--gosslan-card", min: 4.5, why: "文件气泡辅助信息" },
    { fg: "--gosslan-accent-ink", bg: "--gosslan-panel", min: 4.5, why: "主题色当文字" },
    { fg: "--gosslan-accent-ink", bg: "--gosslan-list", min: 4.5, why: "同上，落在列表底" },
    { fg: "--gosslan-success-ink", bg: "--gosslan-panel", min: 4.5, why: "在线 / 成功状态文字" },
    { fg: "--gosslan-danger-ink", bg: "--gosslan-panel", min: 4.5, why: "失败 / 删除 / 错误提示" },
    { fg: "--gosslan-danger-ink", bg: "--gosslan-list", min: 4.5, why: "同上，落在列表底" },
    { fg: "--gosslan-warning-ink", bg: "--gosslan-panel", min: 3.0, why: "皇冠 / 文件夹图标" },
    { fg: "--gosslan-status-offline", bg: "--gosslan-panel", min: 3.0, why: "离线状态点" },
    { fg: "#ffffff", bg: "--gosslan-primary-active", min: 4.5, why: "未完成任务蓝色徽标的数字（11px）" },
    // 红徽标那一档在两种外观下都不达标（3.55 / 3.16），处置与理由见上面 light 段落的注释。
  ],
};

/** 逐对核算；返回所有不达标的组合。 */
export function findTokenContrastFailures(css: string): ContrastFailure[] {
  const scopes = parseTokenScopes(css);
  const out: ContrastFailure[] = [];
  for (const theme of ["light", "dark"] as const) {
    const scope = scopes[theme];
    for (const pair of TOKEN_CONTRAST_CONTRACT[theme]) {
      const bgOver = pair.bgOver ? resolveToken(pair.bgOver, scope) : null;
      const fg = resolveToken(pair.fg, scope);
      const bg = resolveToken(pair.bg, scope, bgOver ?? undefined);
      if (!fg || !bg) continue; // 认不出的值（color-mix 等）跳过，由人工审计覆盖
      const ratio = contrastRatio(toHex(fg), toHex(bg));
      if (ratio < pair.min) {
        out.push({ ...pair, ratio, fgResolved: toHex(fg), bgResolved: toHex(bg) });
      }
    }
  }
  return out;
}
