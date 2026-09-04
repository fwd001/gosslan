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
