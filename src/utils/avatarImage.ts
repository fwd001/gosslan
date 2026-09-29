/**
 * 头像的体积闸与压缩口径（用户 2026-09-29：「头像的图片限制大小放宽到 10MB 以内，
 * 上传之后一定会压缩：① 压缩成 JPG；② 压缩率尽量高、尽量接近无损，尽可能压到 1MB 以内」）。
 *
 * ## 为什么单独一个文件
 * 选质量档这件事本身是纯计算（一串候选编码 → 挑第一个够小的），把它从 `ProfileSection.vue`
 * 里拿出来才有地方写判据：DOM、canvas、文件选择器那些在单测里拿不到，而"挑错了质量"
 * 恰恰是这段逻辑唯一会错的地方。
 *
 * ## 为什么一定转 JPG（连输入本来就是 JPEG 时也重新编码）
 * ① 用户要的"上传之后一定会压缩"只有重新编码才成立 —— 直接透传就还是原图那份体积；
 * ② JPEG 是**全端都能渲染**的唯一公共分母：Android 相册常见的 HEIC 在
 *    Chrome/Edge/Firefox 上原生画不出来（见 `filePreview.ts` 里 heic 那段注释和
 *    `mobile_picker.rs` 的 HEIC→JPEG 转码），头像一旦是 HEIC，对端看到的就是一块空白；
 *    走一遍 canvas 就把它钉成 JPEG，不需要那套 JNI 转码。
 *
 * ## 有损的那一半是刻意接受的
 * JPEG 不是无损格式，"尽量接近无损"落到实现上 = **从最高质量档往下找**，而不是一上来就压。
 * 512px 见方的高质量 JPEG 通常在百 KB 量级，所以"≤1MB"这个目标几乎总在最高档就满足。
 */
import { dataUrlBase64 } from "./imageBytes.ts";

/** 允许用户**选进来**的原始文件上限（预处理之前判，用户 2026-09-29 说现在是 2MB、太小）。 */
export const AVATAR_INPUT_MAX_BYTES = 10 * 1024 * 1024;

/** `AVATAR_INPUT_MAX_BYTES` 的文案用标签（MB），避免文案里再抄一个 10。 */
export const AVATAR_INPUT_LIMIT_MB = AVATAR_INPUT_MAX_BYTES / (1024 * 1024);

/** 压缩**产出**的目标上限：data URL 解码后的字节数要落进这里（用户：「尽可能弄到一兆以内」）。 */
export const AVATAR_OUTPUT_MAX_BYTES = 1024 * 1024;

/**
 * 头像输出的正方形边长（中心裁剪后不放大）。
 *
 * 界面最大只用到 ~64px 的方框，512 是给 Retina + 将来更大的展示位留的余量；
 * 再大对体积有意义、对清晰度没有 —— 头像不缩放就是 1MB 那道闸要付的代价。
 */
export const AVATAR_SIZE = 512;

/**
 * 质量阶梯：**从高到低**，取第一个满足 {@link AVATAR_OUTPUT_MAX_BYTES} 的档位。
 *
 * 0.95 起步就是"尽量接近无损"那句的意思 —— 只有 0.95 都装不下 1MB 时才往下退，
 * 而不是固定用一个低质量值。最后一档 0.65 是退路而不是目标。
 */
export const AVATAR_QUALITY_LADDER = [0.95, 0.9, 0.85, 0.8, 0.72, 0.65];

/**
 * data URL 解码后的**准确字节数**（不是字符串长度）。
 *
 * 非 base64 的 data URL（百分号编码原文）在这里是错误输入而不是"算个近似值"的理由 ——
 * 拿去和 1MB 上限比会得出假结论，所以直接抛。
 */
export function dataUrlByteLength(url: string): number {
  const b64 = dataUrlBase64(url);
  if (b64 === null) throw new Error("编码结果不是 base64 data URL，无法判断体积");
  const pad = b64.endsWith("==") ? 2 : b64.endsWith("=") ? 1 : 0;
  // base64 每 4 个字符 = 3 字节，再扣掉填充（toDataURL 的产出一定是 4 的倍数）
  return (b64.length / 4) * 3 - pad;
}

/**
 * 从质量阶梯里挑**第一个**够小的档位并返回它的 data URL。
 *
 * `encode` 由调用方给出（真实实现是 `canvas.toDataURL("image/jpeg", q)`），
 * 这样这段决策逻辑能脱离浏览器被判定 —— 传进来的是候选编码，判的是"选哪一档"。
 *
 * 一档都不够小 ⇒ 抛错而不是"就用最差的那档"：静默产出一张超出上限的头像，
 * 后端会以"头像过大"拒收，用户在界面上看到的是"点了没反应"。
 */
export function pickAvatarJpeg(encode: (quality: number) => string): string {
  for (const q of AVATAR_QUALITY_LADDER) {
    const url = encode(q);
    if (dataUrlByteLength(url) <= AVATAR_OUTPUT_MAX_BYTES) return url;
  }
  throw new Error("头像压缩后仍超过 1MB，请换一张更小的图");
}
