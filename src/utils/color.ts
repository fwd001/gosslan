// 主题色工具：由主色派生 hover/active/浅色背景，并注入 CSS 变量。

export function hexToRgb(hex: string): [number, number, number] {
  let h = hex.replace("#", "").trim();
  if (h.length === 3) h = h.split("").map((c) => c + c).join("");
  const n = parseInt(h || "3370ff", 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function mix(hex: string, target: [number, number, number], ratio: number): string {
  const [r, g, b] = hexToRgb(hex);
  const mr = Math.round(r + (target[0] - r) * ratio);
  const mg = Math.round(g + (target[1] - g) * ratio);
  const mb = Math.round(b + (target[2] - b) * ratio);
  return `rgb(${mr}, ${mg}, ${mb})`;
}

export function rgba(hex: string, alpha: number): string {
  const [r, g, b] = hexToRgb(hex);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

export function lighten(hex: string, ratio: number): string {
  return mix(hex, [255, 255, 255], ratio);
}

export function darken(hex: string, ratio: number): string {
  return mix(hex, [0, 0, 0], ratio);
}

/** 与 mix 同逻辑，但返回 hex（可继续参与后续混合链）。 */
export function mixHex(hex: string, target: [number, number, number], ratio: number): string {
  const [r, g, b] = hexToRgb(hex);
  const ch = (v: number, t: number) => {
    const n = Math.round(v + (t - v) * ratio);
    return Math.max(0, Math.min(255, n)).toString(16).padStart(2, "0");
  };
  return `#${ch(r, target[0])}${ch(g, target[1])}${ch(b, target[2])}`;
}

/** 感知亮度（0-255），用于把颜色压/提到目标亮度而非固定比例混合。 */
export function luma(hex: string): number {
  const [r, g, b] = hexToRgb(hex);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/**
 * 把颜色朝 target 混合到指定亮度（双向：提亮/压暗都支持）。
 * 原来只做"朝暗压"——但微信式浅底需要"朝白提"到目标 luma，方向搞反了 ratio 会变负。
 * 通用公式：resultLuma = from*(1-r) + to*r  →  r = (targetLuma - from) / (to - from)
 * ratio 不在 [0,1] 范围内说明朝 target 方向走达不到，原样返回。
 */
export function mixToLuma(hex: string, target: [number, number, number], targetLuma: number): string {
  const from = luma(hex);
  const to = 0.2126 * target[0] + 0.7152 * target[1] + 0.0722 * target[2];
  if (Math.abs(from - targetLuma) < 0.5) return hex;
  const ratio = (targetLuma - from) / (to - from);
  if (ratio < 0 || ratio > 1) return hex;
  return mixHex(hex, target, ratio);
}

/**
 * 按 HSL 派生：保留色相，只调明度（并可给饱和度设上限）。
 * 用途：把主题色派生成「浅底 + 同色相深字」——直接混白/混黑会掉饱和发灰，
 * HSL 里只动 L 才能保住色相的鲜度。
 */
export function adjustHsl(hex: string, opts: { l?: number; sMax?: number }): string {
  const [h, s, l] = toHsl(hex);
  return hslToHex(h, opts.sMax !== undefined ? Math.min(s, opts.sMax) : s, opts.l ?? l);
}

/** hex → HSL：h 为 0-360，s / l 为 0-1。 */
export function toHsl(hex: string): [number, number, number] {
  const [r255, g255, b255] = hexToRgb(hex);
  const r = r255 / 255;
  const g = g255 / 255;
  const b = b255 / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const d = max - min;
  if (d === 0) return [0, 0, l];
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h: number;
  if (max === r) h = (g - b) / d + (g < b ? 6 : 0);
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  return [h * 60, s, l];
}

/** HSL → hex：h 为 0-360，s / l 为 0-1。 */
export function hslToHex(h: number, s: number, l: number): string {
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const hp = (((h % 360) + 360) % 360) / 60;
  const x = c * (1 - Math.abs((hp % 2) - 1));
  let r = 0;
  let g = 0;
  let b = 0;
  if (hp < 1) [r, g, b] = [c, x, 0];
  else if (hp < 2) [r, g, b] = [x, c, 0];
  else if (hp < 3) [r, g, b] = [0, c, x];
  else if (hp < 4) [r, g, b] = [0, x, c];
  else if (hp < 5) [r, g, b] = [x, 0, c];
  else [r, g, b] = [c, 0, x];
  const m = l - c / 2;
  const ch = (v: number) =>
    Math.max(0, Math.min(255, Math.round((v + m) * 255)))
      .toString(16)
      .padStart(2, "0");
  return `#${ch(r)}${ch(g)}${ch(b)}`;
}

function relLuminance(hex: string): number {
  const [r, g, b] = hexToRgb(hex).map((v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG 对比度（1-21）：正文要求 ≥ 4.5 才达 AA。 */
export function contrastRatio(a: string, b: string): number {
  const la = relLuminance(a);
  const lb = relLuminance(b);
  const [hi, lo] = la > lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

/** 将主题色与字体注入 CSS 变量 */
export function applyTheme(color: string, fontFamily: string) {
  const root = document.documentElement;
  root.style.setProperty("--gosslan-primary", color);
  root.style.setProperty("--gosslan-primary-hover", lighten(color, 0.08));
  root.style.setProperty("--gosslan-primary-active", darken(color, 0.08));
  root.style.setProperty("--gosslan-primary-light", rgba(color, 0.12));
  root.style.setProperty(
    "--gosslan-font-family",
    fontFamily || "-apple-system, 'Segoe UI', 'PingFang SC', 'Microsoft YaHei', sans-serif",
  );
}

export function humanSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}

/**
 * 默认头像底色：同一名字在单聊列表 / 群聊九宫格 / 消息头像 / 通讯录等所有位置
 * 都得到同一颜色，解决「同一个名称在单聊和群聊里默认头像不一样」的问题。
 * 调色板 8 色，djb2 哈希取模；空名走兜底色，确保始终有合法值。
 */
const AVATAR_PALETTE = [
  "#5b8def", // 蓝
  "#58b178", // 绿
  "#f0a04e", // 橙
  "#9a7ff0", // 紫
  "#e36b6b", // 珊瑚红
  "#4cb8b8", // 青
  "#c87ec4", // 品红
  "#6b7280", // 石板灰（兜底）
];

export function nameToColor(name: string): string {
  const key = (name ?? "").trim() || "?";
  let h = 5381;
  for (let i = 0; i < key.length; i++) {
    h = ((h << 5) + h) ^ key.charCodeAt(i);
  }
  return AVATAR_PALETTE[Math.abs(h) % AVATAR_PALETTE.length];
}

/** ASCII 字母（a-z / A-Z）。只有它才参与「前 N 个字母」的截断。 */
function isAsciiLetter(ch: string): boolean {
  const c = ch.charCodeAt(0);
  return (c >= 65 && c <= 90) || (c >= 97 && c <= 122);
}

/**
 * 宽字符（一个字符就顶一个头像位）：中日韩表意文字 + 假名 + 谚文。
 *
 * 不把它写成 `ch.charCodeAt(0) > 127` 这种偷懒判据：那样 emoji、俄文、阿拉伯文
 * 都会落进"只取一个"的分支，而规则里说的只是**中文**。emoji/其它文字由
 * `avatarInitial` 的"非 ASCII 字母开头 ⇒ 取首字符"分支统一兜住。
 */
function isWideChar(ch: string): boolean {
  const cp = ch.codePointAt(0) ?? 0;
  return (
    (cp >= 0x3400 && cp <= 0x4dbf) || // CJK 扩展 A
    (cp >= 0x4e00 && cp <= 0x9fff) || // CJK 统一表意文字
    (cp >= 0xf900 && cp <= 0xfaff) || // CJK 兼容表意文字
    (cp >= 0x3040 && cp <= 0x30ff) || // 日文假名
    (cp >= 0xac00 && cp <= 0xd7af) || // 谚文音节
    (cp >= 0x20000 && cp <= 0x2fa1f) // CJK 扩展 B 及以上
  );
}

/**
 * 默认头像的统一取字规则（用户 2026-09-13 定稿）。
 *
 * 所有画默认头像的地方（消息流 / 会话列表 / 通讯录 / 群成员 / 转发弹窗 /
 * 设置资料页 / 侧栏）都必须用它，不许各写一份 slice。
 *
 * | 用户名 | 结果 | 说明 |
 * |---|---|---|
 * | `zhou` | `ZHOU` | 纯英文（无空格）⇒ 前 4 个字母 |
 * | `周工` | `周` | 纯中文 ⇒ 首字 |
 * | `周san` | `周` | 中文开头，后面是英文 ⇒ 仍是首字 |
 * | `a中` | `A中` | 字母 + 中文，字母 1 个 ⇒ **字母 + 一个中文** |
 * | `ab中` | `AB中` | 字母 + 中文，字母 2 个 ⇒ 两个字母 + 一个中文 |
 * | `abc中` | `ABC` | 字母 + 中文，字母 3 个 ⇒ 不加中文，只截字母 |
 * | `abcde中` | `ABCD` | 字母超过 4 个 ⇒ 前 4 个字母 |
 * | `👍周工` | `👍` | 非 ASCII 字母开头（emoji / 数字 / 符号）⇒ 首字符 |
 * | `` / `  ` / `null` | `?` | 空名兜底 |
 *
 * ⚠️ 「中文前 ≤2 个字母就带上中文」是用户 2026-09-13 的**补充澄清**：
 * 最初写成了"1 个字母就只留字母"，用户纠正为 **1 个也带中文**（`a中` → `A中`），
 * 分界点是 **3 个字母**（从 3 个起才只截字母）。
 *
 * 三条实现约束：
 * 1. **按码点取字符**（`Array.from`）——`slice(0, 1)` 会把 emoji 劈成半边。
 * 2. 字母一律**转大写**，与旧行为一致（旧规则 `zhou` → `Z`）。
 * 3. 纯英文后跟空格/符号/数字（`John Smith`、`lee_2`）按"英文"处理，取开头
 *    连续字母的前 4 个 —— 规则只对"中英混排"特判，别的都归入英文那一档。
 */
export function avatarInitial(name: string | null | undefined): string {
  const trimmed = (name ?? "").trim();
  if (!trimmed) return "?";
  const chars = Array.from(trimmed);
  const first = chars[0];
  // 中文 / emoji / 数字 / 符号开头：首字符就是答案（中文不需要大写转换）
  if (!isAsciiLetter(first)) return first.toUpperCase();

  // 开头连续 ASCII 字母的个数
  let letterCount = 0;
  while (letterCount < chars.length && isAsciiLetter(chars[letterCount])) letterCount++;
  const next = chars[letterCount];
  const mixedWithChinese = next !== undefined && isWideChar(next);

  if (mixedWithChinese && letterCount <= 2) {
    // 字母 ≤2 个：字母 + 一个中文（`a中` → `A中`、`ab中` → `AB中`）
    return chars.slice(0, letterCount).join("").toUpperCase() + next;
  }
  // 其余（中英混排且字母 ≥3、以及纯英文）都只截字母：3 个取 3 个，超过 4 个取前 4 个
  return chars
    .slice(0, Math.min(letterCount, 4))
    .join("")
    .toUpperCase();
}

/**
 * 默认头像文字的 **`data-len` 值**：给 `style.css` 的 `.gosslan-avatar-initial`
 * 用，字数越多字号越小（3/4 字靠容器查询按头像框宽度缩放）。
 *
 * 单独导出而不是让模板里算 `avatarInitial(name).length`：调用点有 12 处，
 * 每处都调两次函数既浪费又容易写成对**原始用户名**取 length（那正是错的 ——
 * `abc中` 的原始长度是 4，但实际渲染的是 3 个字）。
 */
export function avatarInitialLen(name: string | null | undefined): number {
  return Array.from(avatarInitial(name)).length;
}
