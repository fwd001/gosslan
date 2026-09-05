// 聊天显示样式：预设配色（可读性优先，5-6 套）与字号档位。
// 本机偏好持久化（后端 settings.chat_style），同时经 ChatStyle 消息广播给
// 已连接节点：对方设备持久化后按「我的偏好」渲染我发出的消息气泡。

export interface ChatPresetColors {
  mineBubble: string;
  mineText: string;
  otherBubble: string;
  otherText: string;
}

export interface ChatPreset {
  key: string;
  label: string;
  light: ChatPresetColors;
  dark: ChatPresetColors;
}

/** 气泡配色预设：每套均通过明暗双主题下的正文对比度检查（≥ 4.5:1）。 */
export const CHAT_PRESETS: ChatPreset[] = [
  {
    key: "classic",
    label: "经典蓝",
    light: { mineBubble: "#dbeafe", mineText: "#1e3a8a", otherBubble: "#ffffff", otherText: "#1f2937" },
    dark: { mineBubble: "#1d4ed8", mineText: "#ffffff", otherBubble: "#262626", otherText: "#e5e7eb" },
  },
  {
    key: "mint",
    label: "薄荷绿",
    light: { mineBubble: "#d1fae5", mineText: "#064e3b", otherBubble: "#ffffff", otherText: "#1f2937" },
    dark: { mineBubble: "#065f46", mineText: "#ecfdf5", otherBubble: "#262626", otherText: "#e5e7eb" },
  },
  {
    key: "amber",
    label: "暖阳橙",
    light: { mineBubble: "#ffedd5", mineText: "#7c2d12", otherBubble: "#ffffff", otherText: "#1f2937" },
    dark: { mineBubble: "#9a3412", mineText: "#fff7ed", otherBubble: "#262626", otherText: "#e5e7eb" },
  },
  {
    key: "celadon",
    label: "青瓷",
    light: { mineBubble: "#ccfbf1", mineText: "#134e4a", otherBubble: "#ffffff", otherText: "#1f2937" },
    dark: { mineBubble: "#115e59", mineText: "#f0fdfa", otherBubble: "#262626", otherText: "#e5e7eb" },
  },
  {
    key: "rose",
    label: "樱花粉",
    light: { mineBubble: "#fce7f3", mineText: "#831843", otherBubble: "#ffffff", otherText: "#1f2937" },
    dark: { mineBubble: "#9d174d", mineText: "#fdf2f8", otherBubble: "#262626", otherText: "#e5e7eb" },
  },
  {
    key: "slate",
    label: "石墨灰",
    light: { mineBubble: "#e5e7eb", mineText: "#111827", otherBubble: "#f3f4f6", otherText: "#374151" },
    dark: { mineBubble: "#374151", mineText: "#f9fafb", otherBubble: "#1f2937", otherText: "#d1d5db" },
  },
];

export const CHAT_FONT_SIZES = [
  { key: "sm", label: "小", px: 13 },
  { key: "md", label: "标准", px: 14 },
  { key: "lg", label: "大", px: 16 },
] as const;

export type FontSizeKey = (typeof CHAT_FONT_SIZES)[number]["key"];

/** 本机聊天样式偏好（持久化 & 广播）。 */
export interface ChatStyleConfig {
  preset: string;
  fontSize: FontSizeKey;
  /** 连续消息紧凑显示（群聊大信息量推荐开启） */
  compact: boolean;
}

export const DEFAULT_CHAT_STYLE: ChatStyleConfig = { preset: "classic", fontSize: "md", compact: true };

export function findPreset(key: string | undefined | null): ChatPreset {
  return CHAT_PRESETS.find((p) => p.key === key) ?? CHAT_PRESETS[0];
}

export function fontPx(size: FontSizeKey): number {
  return CHAT_FONT_SIZES.find((f) => f.key === size)?.px ?? 14;
}

/** 解析对端广播来的样式 JSON（坏数据回退默认）。 */
export function parsePeerStyle(raw: string): ChatStyleConfig {
  try {
    const v = JSON.parse(raw) as Partial<ChatStyleConfig>;
    return {
      preset: typeof v.preset === "string" ? v.preset : DEFAULT_CHAT_STYLE.preset,
      fontSize: (["sm", "md", "lg"] as const).includes(v.fontSize as FontSizeKey)
        ? (v.fontSize as FontSizeKey)
        : DEFAULT_CHAT_STYLE.fontSize,
      compact: typeof v.compact === "boolean" ? v.compact : DEFAULT_CHAT_STYLE.compact,
    };
  } catch {
    return DEFAULT_CHAT_STYLE;
  }
}
