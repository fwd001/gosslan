/**
 * 聊天正文的链接切分。仅匹配 http(s)://，避免误识别 + 避免 javascript:/data: 等危险 scheme。
 * 返回 text/link 段数组，渲染端按段拼回去即可（不要用 v-html 拼接，已天然防 XSS）。
 *
 * 尾随标点处理：句末的 . , ; : ! ? 与**中文句读**都不算 URL 一部分，剥出来当普通文本。
 * 例如 "看 https://a.com, 还有 b" → ["看 ", link("https://a.com"), ", 还有 b"]。
 *
 * ⚠️ 中文句读必须同时出现在**两处**：URL 字符集要把它排除（否则链接会一路吞到句末），
 * 尾随标点要把它剥掉。只做后一半是不够的 —— 正则先把「。然后呢」整段吃进来，
 * 剥尾标点时又只认 ASCII，于是整句都成了链接（2026-09-16 修的缺陷）。
 */

export type LinkSegment =
  | { kind: "text"; value: string }
  | { kind: "link"; value: string; href: string }
  | { kind: "mention"; value: string }
  /** @ 到的正是本机用户：同一个位置，但换成 `@你` 并由渲染端加重样式。
   *  刻意与 `mention` 分成两个 kind —— 段里不携带"这是谁"的信息，
   *  查看者身份只由调用方传进来的 `self` 决定（同一份文本在两端渲染成不同标签）。 */
  | { kind: "mention-self"; value: string };

/**
 * 中文句读 —— URL 里绝不会出现的全角标点。
 *
 * 只排除**标点**，不排除汉字与全角字母：中文域名/路径（`https://zh.wikipedia.org/wiki/中国`）
 * 是合法 URL，排除它们会把链接从中间切断。但「。」「，」这类句读一旦被吃进 URL，
 * 链接就会把后续整句中文都吞掉，点击打开必然失败 —— 这正是中文聊天里最常见的写法。
 */
const CJK_PUNCT = "，。、；：！？（）【】《》「」『』“”‘’…";

const URL_RE = new RegExp(`https?://[^\\s<>"'\\[\\]{}${CJK_PUNCT}]+`, "g");
/** 句末标点：ASCII 句读 + 中文句读。
 *  中文那半其实已被上面的排除集挡住（匹配结果里不会有它们），保留是为了两处口径永远一致 ——
 *  将来谁放宽了排除集，剥尾标点这边不用再想一遍。 */
const TRAILING_PUNCT = new RegExp(`[.,;:!?${CJK_PUNCT}]+$`);

/**
 * 剥掉 URL 尾部的句末标点，返回 `{ value, trailing }`。
 *
 * `)` 特殊：它是**唯一**允许出现在 URL 体内、又常被当作句末收尾的字符。
 * 靠左右括号是否配对区分 ——
 *   · `https://zh.wikipedia.org/wiki/Foo_(bar)` → 配对，`)` 属于 URL（维基/百科的常见形态）；
 *   · `（见 https://a.com/x)` → 不配对，`)` 是中文句末的收尾。
 * 旧实现把 `(` `)` 直接排除出 URL 字符集，副作用就是上面那条维基链接被从中间切成两段。
 */
function trimUrlTail(raw: string): { value: string; trailing: string } {
  let value = raw;
  for (let prev = ""; value !== prev; ) {
    prev = value;
    value = value.replace(TRAILING_PUNCT, "");
    let extra = 0;
    for (const ch of value) {
      if (ch === "(") extra--;
      else if (ch === ")") extra++;
    }
    // 多出来的右括号（不配对的那些）才算句末标点
    while (extra > 0 && value.endsWith(")")) {
      value = value.slice(0, -1);
      extra--;
    }
  }
  return { value, trailing: raw.slice(value.length) };
}

/** @name 边界：@ 前须是行首/空白（防邮箱误判），名字后允许跟空白或中英文常用标点。
 *  导出供 messages.ts 的「被 @ 检测」复用——高亮与检测必须同一套边界语义。 */
export const MENTION_AFTER = String.raw`(?=$|[\s，。！？；：、,.!?;:)）】》"'])`;

/** @name 前导边界：行首 / 空白 / **表情 token 的收尾 `]`**。
 *
 *  `]` 之所以算边界：正文渲染时先按表情 token 切成多段，文本段**各自**跑 linkify
 *  （见 MessageTextBubble 的 segments），段首天然命中 `^`；而表情渲染出来是一张图片，
 *  视觉上确实就是个边界。检测端（messages.ts）若不认这条，就会出现
 *  「气泡里 @名字 是蓝色高亮块、但既没有红点也不发通知」——同一份正文两套判定。
 *
 *  邮箱 `a@b.com` 仍不误判：`a` 既不是空白也不是 `]`。 */
const MENTION_LEAD = String.raw`[\s\]]`;
export const MENTION_BEFORE = `(^|${MENTION_LEAD})`;

/** 输入框插入 @ 时复用：这个字符够不够格当 @ 的前导边界（空串 = 行首，也算）。
 *  与 MENTION_BEFORE 同一个 MENTION_LEAD，所以「插入端补的边界」和「检测端认的边界」
 *  不可能再分叉 —— 分叉的表现就是发送端看到蓝色 chip、接收端毫无反应。 */
const MENTION_LEAD_ONLY_RE = new RegExp(`^${MENTION_LEAD}$`);
export function isMentionLead(ch: string): boolean {
  return ch === "" || MENTION_LEAD_ONLY_RE.test(ch);
}

export function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** 成员名 → @提及 正则（长名优先，防短名吃掉长名前缀；无有效名字返回 null）。 */
function buildMentionRe(names: string[]): RegExp | null {
  const uniq = [...new Set(names.map((n) => n.trim()).filter(Boolean))].sort(
    (a, b) => b.length - a.length,
  );
  if (uniq.length === 0) return null;
  return new RegExp(`${MENTION_BEFORE}@(${uniq.map(escapeRe).join("|")})${MENTION_AFTER}`, "g");
}

function linkifyUrls(text: string): LinkSegment[] {
  if (!text) return [];
  const segments: LinkSegment[] = [];
  let lastIndex = 0;
  for (const m of text.matchAll(URL_RE)) {
    const start = m.index ?? 0;
    const raw = m[0];
    const { value: trimmed, trailing } = trimUrlTail(raw);

    if (start > lastIndex) {
      segments.push({ kind: "text", value: text.slice(lastIndex, start) });
    }
    segments.push({ kind: "link", value: trimmed, href: trimmed });
    if (trailing) segments.push({ kind: "text", value: trailing });

    lastIndex = start + raw.length;
  }
  if (lastIndex < text.length) {
    segments.push({ kind: "text", value: text.slice(lastIndex) });
  }
  return segments;
}

/**
 * 聊天正文切分：链接 + @提及。mentions 传群成员名列表（@name 高亮）。
 * 返回 text/link/mention 段数组，渲染端按段拼回去即可（不要用 v-html 拼接，已天然防 XSS）。
 *
 * `self` 传"查看者自己是谁 + 该显示成什么"（用户 2026-09-26：自己看任何 @ 到自己的地方
 * 都应是 `@你`，别人看到的仍然是名字）。⚠️ 标签由**调用方**传进来而不是在这里写死中文：
 * 本模块是零依赖纯函数，i18n 归渲染端（键见 `i18n/locales.ts`）。
 */
export function linkify(
  text: string,
  mentions: string[] = [],
  self?: { name: string; label: string },
): LinkSegment[] {
  if (!text) return [];
  const mentionRe = buildMentionRe(mentions);
  if (!mentionRe) return linkifyUrls(text);
  const selfName = self?.name.trim() ?? "";

  const segments: LinkSegment[] = [];
  let lastIndex = 0;
  for (const m of text.matchAll(mentionRe)) {
    const start = m.index ?? 0;
    const lead = m[1]; // 行首或前导空白
    const mentionStart = start + lead.length;
    if (mentionStart > lastIndex) {
      segments.push(...linkifyUrls(text.slice(lastIndex, mentionStart)));
    }
    const name = m[2];
    segments.push(
      selfName !== "" && name === selfName
        ? { kind: "mention-self", value: self!.label }
        : { kind: "mention", value: `@${name}` },
    );
    lastIndex = start + m[0].length;
  }
  if (lastIndex < text.length) {
    segments.push(...linkifyUrls(text.slice(lastIndex)));
  }
  return segments;
}

/** 超长 URL 的展示切分（三段拼回去恒等于原 URL）。 */
export interface UrlParts {
  /** 前半，可见 */
  head: string;
  /** 中段，**必须留在 DOM 里但视觉隐藏**（见 splitUrl 注释） */
  mid: string;
  /** 后半，可见 */
  tail: string;
}

/**
 * 超长 URL 的**展示**切分：中间省略，避免把气泡撑爆。
 *
 * ⚠️ 契约：`head + mid + tail === url`。渲染端只允许用 CSS 把 mid 隐藏掉，
 * **绝不能在 DOM 里丢掉 mid** —— 选中复制取的是选区文本（DOM 顺序），
 * 丢掉 mid 就会复制出残缺链接（用户 2026-09-16：「复制的时候会复制不完整的，
 * 应该复制原始数据，不应该是界面省略的数据」）。同理，视觉上的省略号也**不能是文本**，
 * 否则会被一起复制进 URL。
 */
export function splitUrl(url: string, maxLen = 48): UrlParts {
  const half = Math.floor((maxLen - 1) / 2);
  // half 为 0 时 `slice(-0)` 等于 `slice(0)`（整串），会切出错位结果，直接原样返回。
  if (half < 1 || url.length <= maxLen) return { head: url, mid: "", tail: "" };
  return {
    head: url.slice(0, half),
    mid: url.slice(half, url.length - half),
    tail: url.slice(url.length - half),
  };
}
