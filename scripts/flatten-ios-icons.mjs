#!/usr/bin/env node
/**
 * 去掉 `src-tauri/icons/ios/*.png` 的 alpha 通道（白底合成后转成不透明 RGB）。
 *
 * ## 为什么必须这么做
 * App Store Connect 校验 App Store 图标（1024×1024）时报的是
 * *"can't be transparent nor contain an alpha channel"* —— **即使每个像素的 alpha 都是 255**
 * 也一样会被拒：校验看的是 PNG 有没有 alpha 通道，而不是像素值。
 * 而 `tauri icon` 生成的 iOS 图标一律是 32 位 RGBA（本项目实际取值为 254/255 两种），
 * 于是上传时必然被挡下来。iOS 侧不需要圆角/透明 —— 系统自己会按 squircle 遮罩裁切。
 *
 * ## 用法
 * 重新生成图标后（`npm run tauri icon src-tauri/icons/icon-source.png`）跑一次：
 *
 *     node scripts/flatten-ios-icons.mjs
 *
 * 它只做两件事：解 PNG（8 位、非交错）→ 白底合成 → 写成不透明 RGB PNG。
 * 尺寸与像素颜色都不变（alpha≥254 时合成结果与原色几乎逐位相同），
 * 唯一区别是文件里不再有 alpha 通道。
 *
 * 为什么自己写 PNG 编解码而不装依赖：项目原则是**不引无谓依赖**，
 * 而这里需要的只是 Node 内置的 `zlib` + ~80 行行滤镜还原，
 * 不值得为一次性图标处理引入 sharp/pngjs 这类原生依赖。
 */
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync, inflateSync } from "node:zlib";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const IO_DIR = join(ROOT, "src-tauri", "icons", "ios");

const PNG_SIG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

/** 拆出 PNG 的分块（本项目只需要 IHDR / IDAT）。 */
function readChunks(buf) {
  if (!buf.subarray(0, 8).equals(PNG_SIG)) throw new Error("不是 PNG 文件");
  const chunks = [];
  let pos = 8;
  while (pos + 8 <= buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString("ascii", pos + 4, pos + 8);
    chunks.push({ type, data: buf.subarray(pos + 8, pos + 8 + len) });
    pos += 12 + len;
    if (type === "IEND") break;
  }
  return chunks;
}

function paeth(a, b, c) {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  if (pa <= pb && pa <= pc) return a;
  return pb <= pc ? b : c;
}

/** 解出 { width, height, channels, pixels }（仅支持 8 位、非交错、无调色板）。 */
function decodePng(buf) {
  const chunks = readChunks(buf);
  const ihdr = chunks.find((c) => c.type === "IHDR");
  if (!ihdr) throw new Error("缺少 IHDR");
  const width = ihdr.data.readUInt32BE(0);
  const height = ihdr.data.readUInt32BE(4);
  const bitDepth = ihdr.data[8];
  const colorType = ihdr.data[9];
  const interlace = ihdr.data[12];
  if (bitDepth !== 8) throw new Error(`只支持 8 位色深，实际 ${bitDepth}`);
  if (interlace !== 0) throw new Error("不支持交错 PNG");
  const channels = { 0: 1, 2: 3, 4: 2, 6: 4 }[colorType];
  if (!channels) throw new Error(`不支持的颜色类型 ${colorType}`);

  const raw = inflateSync(Buffer.concat(chunks.filter((c) => c.type === "IDAT").map((c) => c.data)));
  const stride = width * channels;
  const pixels = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const src = raw.subarray(y * (stride + 1) + 1, y * (stride + 1) + 1 + stride);
    const out = pixels.subarray(y * stride, (y + 1) * stride);
    const prev = y > 0 ? pixels.subarray((y - 1) * stride, y * stride) : null;
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? out[x - channels] : 0;
      const up = prev ? prev[x] : 0;
      const upLeft = prev && x >= channels ? prev[x - channels] : 0;
      let value = src[x];
      if (filter === 1) value += left;
      else if (filter === 2) value += up;
      else if (filter === 3) value += (left + up) >> 1;
      else if (filter === 4) value += paeth(left, up, upLeft);
      else if (filter !== 0) throw new Error(`不支持的行滤镜 ${filter}`);
      out[x] = value & 0xff;
    }
  }
  return { width, height, channels, pixels };
}

function crc32(buf) {
  let crc = 0xffffffff;
  for (const byte of buf) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(data.length, 0);
  head.write(type, 4, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])), 0);
  return Buffer.concat([head, data, crc]);
}

/** 白底合成 alpha 后写成不透明 RGB PNG（color type 2）。 */
function encodeOpaqueRgb({ width, height, channels, pixels }) {
  const stride = width * 3;
  const rgb = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * channels;
      const [r, g, b] = [pixels[i], pixels[i + 1] ?? pixels[i], pixels[i + 2] ?? pixels[i]];
      const a = channels === 4 ? pixels[i + 3] : 255;
      // 直通 alpha（非预乘）合成到白底：a=255 时逐位等于原色，a=254 时偏差 ≤1/255
      const o = y * stride + x * 3;
      rgb[o] = Math.round((r * a + 255 * (255 - a)) / 255);
      rgb[o + 1] = Math.round((g * a + 255 * (255 - a)) / 255);
      rgb[o + 2] = Math.round((b * a + 255 * (255 - a)) / 255);
    }
  }
  // 行滤镜用 Paeth（4）：图标是大面积渐变，比 None 小很多（几 MB 的图标资源没必要浪费）；
  // 编码端用**原始未过滤**的左/上/左上像素算预测值，与解码端一一对应。
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 4;
    const out = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const left = x >= 3 ? rgb[y * stride + x - 3] : 0;
      const up = y > 0 ? rgb[(y - 1) * stride + x] : 0;
      const upLeft = y > 0 && x >= 3 ? rgb[(y - 1) * stride + x - 3] : 0;
      out[x] = (rgb[y * stride + x] - paeth(left, up, upLeft)) & 0xff;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // color type: truecolor（无 alpha）
  ihdr[10] = 0; // compression
  ihdr[11] = 0; // filter
  ihdr[12] = 0; // interlace
  return Buffer.concat([
    PNG_SIG,
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const files = readdirSync(IO_DIR).filter((f) => f.endsWith(".png"));
if (!files.length) throw new Error(`${IO_DIR} 下没有 PNG`);
let changed = 0;
for (const name of files) {
  const path = join(IO_DIR, name);
  const img = decodePng(readFileSync(path));
  if (img.channels !== 4 && img.channels !== 2) {
    console.log(`跳过 ${name}：本来就没有 alpha 通道`);
    continue;
  }
  writeFileSync(path, encodeOpaqueRgb(img));
  changed++;
  console.log(`已去 alpha：${name}（${img.width}×${img.height}）`);
}
console.log(`完成：处理 ${changed}/${files.length} 个 iOS 图标`);
