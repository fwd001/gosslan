// 附件预览加载：把已完成的 image/code 文件读成前端可渲染形态。
//
// 图片走原始字节 → Blob → objectURL（绝不把 Base64 字符串塞进 Vue）；
// 代码走原始字节 → TextDecoder → 字符串交给 CodeBlock。
// 按 msg_id 缓存并做 in-flight 去重，避免 VirtualList 滚动反复读同一文件。
//
// ⚠️ **objectURL 的生命周期归本模块的缓存所有，消费者不得 `revokeObjectURL`**
// （2026-09-21 的真根因）：同一个 cid/msg_id 的 URL 是**缓存里大家共用的同一个字符串**
// （一张待办描述图会同时出现在聊天时间线的任务卡、看板表单、任务详情三处）。
// 任何一处卸载/换图时 revoke，都会把其它视图的图一起打回裂图 —— 更糟的是缓存里那个 URL
// 已经死了却仍被命中（`cache.get` 直接返回它），此后连退避重试也救不回来，只能重启应用。
// 要回收内存请走缓存自己的出口（见 `favoritePreview.dropFavoritePreview`），不要由消费者就地 revoke。

import { api } from "@/api";
import { previewFailureResult, type PreviewResult } from "@/utils/mediaAvailability";

export type { PreviewResult };

/**
 * 让某条消息的预览缓存失效（传输刚完成 / 文件刚落盘时调用）。
 *
 * 为什么必须有：收到的图片可能是"消息先到、字节后到"——在途时读预览只会得到
 * 「仍在接收」。若不让缓存失效，即使文件已经落盘，气泡也不会重读（真机：
 * 图片时好时坏，点几次/等一会儿/重发才出来）。
 */
export function invalidateFilePreview(msgId: string) {
  cache.delete(msgId);
  inflight.delete(msgId);
}

/** 代码预览上限：超过则回退文件卡片并提示，避免把巨大文件读进前端。 */
const CODE_MAX_BYTES = 512 * 1024;
/** 图片预览上限：远大于常见截图/照片，仍远低于协议 MAX_FRAME。 */
const IMAGE_MAX_BYTES = 15 * 1024 * 1024;

const cache = new Map<string, PreviewResult>();
const inflight = new Map<string, Promise<PreviewResult>>();

/**
 * 扩展名 → MIME。
 *
 * ⚠️ **HEIC/HEIF 注意**：Chrome/Edge/Firefox **原生不支持渲染 HEIC**（只有 macOS Safari 16+ 支持），
 * 所以我们在发送端自动转 JPEG（见 mobile_picker.rs 的 UNSAFE_IMAGE_EXTS）。
 * 这里保留 heic/heif 的 MIME 映射只是**兜底** — 如果转码失败、或者历史消息里有 HEIC，
 * 至少前端不会把它当成 application/octet-stream 下载，而是尝试渲染（浏览器不支持时自然裂开，
 * 比偷偷下载要好）。
 */
export function imageMime(name: string): string {
  const ext = (name.split(".").pop() || "").toLowerCase();
  switch (ext) {
    case "png":
      return "image/png";
    case "jpg":
    case "jpeg":
      return "image/jpeg";
    case "gif":
      return "image/gif";
    case "webp":
      return "image/webp";
    case "heic":
    case "heif":
      return "image/heic";
    case "avif":
      return "image/avif";
    case "bmp":
      return "image/bmp";
    default:
      return "application/octet-stream";
  }
}

/** 加载指定 file 消息的预览；失败/超限返回 `{}` 或 `{note}`，调用方据此回退文件卡片。 */
export function loadFilePreview(
  msgId: string,
  subtype: "image" | "code",
  name: string,
): Promise<PreviewResult> {
  const hit = cache.get(msgId);
  if (hit) return Promise.resolve(hit);
  const fly = inflight.get(msgId);
  if (fly) return fly;

  const max = subtype === "code" ? CODE_MAX_BYTES : IMAGE_MAX_BYTES;
  const p = (async (): Promise<PreviewResult> => {
    try {
      const raw = await api.readFilePreview(msgId, max);
      // 后端 raw bytes 在 macOS(WKWebView) 上经 JSON 序列化回传为 number[]，
      // 其余平台是 ArrayBuffer。必须统一归一成字节再消费：
      //  - new Blob([number[]]) 会被强转成 "137,80,78,…" 字符串 → 图片损坏、无法预览；
      //  - new TextDecoder().decode(number[]) 直接抛 TypeError → 代码预览同样崩。
      // new Uint8Array 同时接受 ArrayBuffer 与 number[]（ArrayLike<number>），一处归一。
      const bytes = new Uint8Array(raw);
      if (subtype === "image") {
        const url = URL.createObjectURL(new Blob([bytes], { type: imageMime(name) }));
        const r: PreviewResult = { url };
        cache.set(msgId, r);
        return r;
      }
      const r: PreviewResult = { text: new TextDecoder().decode(bytes) };
      cache.set(msgId, r);
      return r;
    } catch (e) {
      const msg = String(e);
      console.error(`[filePreview] ${subtype} preview failed (msgId=${msgId}, name=${name}): ${msg}`);
      const r: PreviewResult = previewFailureResult(msg);
      // 只在**确定性**失败时缓存（已被清理 / 文件过大）。
      // "仍在接收"这类未知失败**绝不缓存** —— 否则文件落盘后也不会重读，
      // 表现为"图片时好时坏、要重发才出来"（真机 2026-09-14）。
      if (r.missing || r.note === "文件过大，无法预览") cache.set(msgId, r);
      return r;
    } finally {
      inflight.delete(msgId);
    }
  })();

  inflight.set(msgId, p);
  return p;
}

/** 按内容指纹（sha256）加载待办描述图片的预览。
 *
 * ⚠️ **失败不缓存**：待办图片的字节是「任务定义先到、文件后到」——
 * 看板在定义一到就渲染缩略图，此刻字节往往还没落地（读不到 → 报「内容不存在」）。
 * 若把这次失败按 cid 缓存，文件落盘后也不会重读（与聊天图片同源的坑，见 `loadFilePreview`）。
 * 每次调用都重新读；`TodoImageThumb` 负责用退避重试把"后到"的字节追回来。
 */
export function loadContentPreview(cid: string, name: string): Promise<PreviewResult> {
  const hit = cache.get(cid);
  if (hit) return Promise.resolve(hit);
  const fly = inflight.get(cid);
  if (fly) return fly;

  const p = (async (): Promise<PreviewResult> => {
    try {
      const raw = await api.readContentPreview(cid, IMAGE_MAX_BYTES);
      const bytes = new Uint8Array(raw);
      const url = URL.createObjectURL(new Blob([bytes], { type: imageMime(name) }));
      const r: PreviewResult = { url };
      cache.set(cid, r);
      return r;
    } catch (e) {
      const msg = String(e);
      console.error(`[filePreview] content preview failed (cid=${cid}, name=${name}): ${msg}`);
      // 确定性失败（文件过大）可以缓存；「内容不存在/路径越权」是暂时态，不缓存。
      if (msg.includes("文件过大")) return { note: "文件过大，无法预览" };
      return {};
    } finally {
      inflight.delete(cid);
    }
  })();

  inflight.set(cid, p);
  return p;
}
