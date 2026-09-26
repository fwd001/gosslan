// 默认头像（#32）：emoji 小动物 × 逐对挑过的背景色。
//
// 为什么是"从 id 现算"而不是"首次初始化随机一次再存起来"：
// - 存起来要多一个跨设备字段（上传头像那条链路之外再走一遍同步），而 id 本来就在每一帧里；
// - 现算 ⇒ 换设备、重装、清缓存之后同一个人还是同一张脸，且**所有对端看到的也是同一张**；
// - 用户上传了真头像时优先级在调用方（有 avatar 就不走这里），这里只负责"没有头像时长什么样"。
//
// 表是 scripts 一次性生成后粘进来的（生成器不在仓里：它是一次性工具，留着只会变成第二份真源）。
// 每一行的背景色都按该 emoji 自己的主色挑过互补/邻近/中性三个色相，且相对亮度落在中间调
// —— 这条不是审美声明，`avatarSeed.test.ts` 里有判据量它。

export interface AvatarSeed {
  emoji: string;
  bg: string;
}

export const AVATAR_PAIRS: readonly AvatarSeed[] = [
  { emoji: "🦊", bg: "#4284b3" }, { emoji: "🦊", bg: "#43a373" }, { emoji: "🦊", bg: "#b06c3b" },
  { emoji: "🐺", bg: "#a87c3e" }, { emoji: "🐺", bg: "#4383a3" }, { emoji: "🐺", bg: "#936acd" },
  { emoji: "🐻", bg: "#3e8da8" }, { emoji: "🐻", bg: "#a37343" }, { emoji: "🐻", bg: "#8b6acd" },
  { emoji: "🐼", bg: "#a88d3e" }, { emoji: "🐼", bg: "#4383a3" }, { emoji: "🐼", bg: "#c6539f" },
  { emoji: "🐨", bg: "#4580ba" }, { emoji: "🐨", bg: "#a37b43" }, { emoji: "🐨", bg: "#39ac73" },
  { emoji: "🦁", bg: "#8c6ec9" }, { emoji: "🦁", bg: "#a37343" }, { emoji: "🦁", bg: "#3999ac" },
  { emoji: "🐯", bg: "#8072cb" }, { emoji: "🐯", bg: "#43a373" }, { emoji: "🐯", bg: "#ac7339" },
  { emoji: "🐮", bg: "#3e84a8" }, { emoji: "🐮", bg: "#a37343" }, { emoji: "🐮", bg: "#c7577c" },
  { emoji: "🐷", bg: "#507ebe" }, { emoji: "🐷", bg: "#a38b43" }, { emoji: "🐷", bg: "#39ac73" },
  { emoji: "🐸", bg: "#bc6549" }, { emoji: "🐸", bg: "#8b6ec4" }, { emoji: "🐸", bg: "#ac9939" },
  { emoji: "🐵", bg: "#3e84a8" }, { emoji: "🐵", bg: "#a37343" }, { emoji: "🐵", bg: "#c6538c" },
  { emoji: "🐔", bg: "#4580ba" }, { emoji: "🐔", bg: "#bc5c8c" }, { emoji: "🐔", bg: "#39ac73" },
  { emoji: "🐧", bg: "#a8733e" }, { emoji: "🐧", bg: "#4383a3" }, { emoji: "🐧", bg: "#c6538c" },
  { emoji: "🦆", bg: "#8072cb" }, { emoji: "🦆", bg: "#b16b48" }, { emoji: "🦆", bg: "#39ac73" },
  { emoji: "🦅", bg: "#3e8da8" }, { emoji: "🦅", bg: "#a37343" }, { emoji: "🦅", bg: "#936acd" },
  { emoji: "🦉", bg: "#b66a43" }, { emoji: "🦉", bg: "#8072c5" }, { emoji: "🦉", bg: "#39ac73" },
  { emoji: "🐇", bg: "#507ebe" }, { emoji: "🐇", bg: "#a37343" }, { emoji: "🐇", bg: "#c6538c" },
  { emoji: "🦔", bg: "#3e8da8" }, { emoji: "🦔", bg: "#8f6bc2" }, { emoji: "🦔", bg: "#ac8f39" },
  { emoji: "🦇", bg: "#a8843e" }, { emoji: "🦇", bg: "#4783ae" }, { emoji: "🦇", bg: "#c6538c" },
  { emoji: "🐬", bg: "#af6f41" }, { emoji: "🐬", bg: "#bc5c8c" }, { emoji: "🐬", bg: "#39ac73" },
  { emoji: "🐳", bg: "#8072cb" }, { emoji: "🐳", bg: "#b16b48" }, { emoji: "🐳", bg: "#39ac73" },
  { emoji: "🦈", bg: "#a8733e" }, { emoji: "🦈", bg: "#8f6bc2" }, { emoji: "🦈", bg: "#39ac73" },
  { emoji: "🐙", bg: "#3e8da8" }, { emoji: "🐙", bg: "#a38b43" }, { emoji: "🐙", bg: "#936acd" },
  { emoji: "🦀", bg: "#4284b3" }, { emoji: "🦀", bg: "#43a373" }, { emoji: "🦀", bg: "#ac8f39" },
  { emoji: "🐢", bg: "#bc6549" }, { emoji: "🐢", bg: "#8f6bc2" }, { emoji: "🐢", bg: "#ac8f39" },
  { emoji: "🦎", bg: "#4284b3" }, { emoji: "🦎", bg: "#b7684e" }, { emoji: "🦎", bg: "#c6538c" },
  { emoji: "🐝", bg: "#8c6ec9" }, { emoji: "🐝", bg: "#b16b48" }, { emoji: "🐝", bg: "#3986ac" },
  { emoji: "🦋", bg: "#b66a43" }, { emoji: "🦋", bg: "#a38b43" }, { emoji: "🦋", bg: "#3986ac" },
  { emoji: "🐞", bg: "#3e8da8" }, { emoji: "🐞", bg: "#a38b43" }, { emoji: "🐞", bg: "#936acd" },
  { emoji: "🐌", bg: "#916ac8" }, { emoji: "🐌", bg: "#4383a3" }, { emoji: "🐌", bg: "#ac7339" },
  { emoji: "🐥", bg: "#507ebe" }, { emoji: "🐥", bg: "#8f6bc2" }, { emoji: "🐥", bg: "#39ac73" },
  { emoji: "🦜", bg: "#8072cb" }, { emoji: "🦜", bg: "#b16b48" }, { emoji: "🦜", bg: "#39ac73" },
  { emoji: "🐘", bg: "#af6f41" }, { emoji: "🐘", bg: "#4783ae" }, { emoji: "🐘", bg: "#c6538c" },
  { emoji: "🦒", bg: "#8072cb" }, { emoji: "🦒", bg: "#438ba3" }, { emoji: "🦒", bg: "#39ac73" },
  { emoji: "🐪", bg: "#4580ba" }, { emoji: "🐪", bg: "#8f6bc2" }, { emoji: "🐪", bg: "#39ac73" },
  { emoji: "🦥", bg: "#bc6549" }, { emoji: "🦥", bg: "#4783ae" }, { emoji: "🦥", bg: "#936acd" },
]

/** FNV-1a（32 位）：小而稳、跨语言可复算，不引依赖。 */
function fnv1a(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h >>> 0;
}

/** id 缺失时返回 null：调用方各自决定兜底，这里不替它挑一张脸。 */
export function avatarSeedFor(id: string | null | undefined): AvatarSeed | null {
  if (!id) return null;
  return AVATAR_PAIRS[fnv1a(id) % AVATAR_PAIRS.length] ?? null;
}
