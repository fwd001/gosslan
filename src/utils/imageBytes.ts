/**
 * 「另存图片」的取字节 → base64 工具（`MessageItem.vue` 与 `ImageLightbox.vue` 共用一份）。
 *
 * ## 为什么要单独一个文件
 * 两处原本各抄了一份**逐字相同**的 7 行：`fetch` → `arrayBuffer` → `binary += String.fromCharCode(
 * ...subarray)` → `btoa(binary)` → `invoke("save_data_file")`。同一件事的三份真相源
 * （两份代码 + 后端的 base64 契约），漂移只是时间问题。
 *
 * ## 这条链上真正贵的地方
 * 峰值内存同时持有约 4 份文件内容：`ArrayBuffer` + `Uint8Array` 视图 + 累积出来的 binary
 * string + `btoa` 的 base64（≈4/3 体积），最后这份还要再被 JSON 序列化进 IPC 载荷。
 * 本文件能拿掉其中两份：
 * 1. 源本身就是 data URL 时**直接摘出 base64 段** —— 旧实现等于把 base64 解码成字节再编码
 *    回**同一个字符串**，纯属白做（Android ART 堆只有 256MB，大图这是真实的 OOM 来源）。
 * 2. 非 data URL 时分块编码并**一次 join**，而不是 `+=` 反复重建整个字符串。
 *
 * ## 为什么仍然走 base64 而不是二进制 IPC（已知取舍，不是疏忽）
 * 正解是 `tauri-plugin-fs` 的 `writeFile(path, Uint8Array)` —— Rust 侧其实已经注册
 * （`Cargo.toml:33` + `lib.rs:153`），但**前端包 `@tauri-apps/plugin-fs` 不在依赖里**，
 * 装上还要动 `src-tauri/capabilities/*.json` 的 fs scope。发版收尾阶段引新依赖 + 改权限面
 * 的风险大于收益，因此这里只收敛浪费、不改传输形状。
 */

/** 一次 `Function.prototype.apply` 能安全吞下的字符数上限（引擎相关，8KiB 保守可靠）。 */
const B64_CHUNK = 8192;

/**
 * 粘贴/拖拽图片的字节上限 —— **必须等于 Rust `MAX_OUTGOING_IMAGE_BYTES`**
 * （`src-tauri/src/commands.rs:41`，8 MiB），由 `imageBytes.test.ts` 读那份源码比对。
 *
 * 为什么前端要再判一次：后端是在 `base64::decode` **之后**才比长度的，所以一张超限的图
 * 会先在 JS 堆里整读成 data URL、再作为 JSON 字符串跨 IPC、再在 Rust 侧解回 `Vec<u8>`
 * —— 三端各分配一份之后才被拒绝。Android 的 ART 堆通常只有 256MB（真机：5 张图一起发
 * 直接 FATAL OutOfMemoryError），所以这道前置判断不是体验优化，是防崩。
 */
export const MAX_PASTED_IMAGE_BYTES = 8 * 1024 * 1024;

/** `MAX_PASTED_IMAGE_BYTES` 的文案用标签（MB，与后端同一算法），避免文案里再写一个字面量。 */
export const PASTED_IMAGE_LIMIT_MB = MAX_PASTED_IMAGE_BYTES / (1024 * 1024);

/**
 * `File`/`Blob` → data URL。
 *
 * 放在这里而不是留在 `MessageComposer` 里：粘贴路径要求"事件处理函数内不许有 await 挡在
 * 会话捕获之前"，所以读取动作被挪到了发送方（ChatWindow），组件只负责同步地把 File 交出去。
 */
export function fileToDataUrl(f: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result as string);
    r.onerror = () => reject(r.error ?? new Error("读取图片失败"));
    r.readAsDataURL(f);
  });
}

/**
 * `Uint8Array` → 标准 base64（与 `btoa` 结果逐字符一致）。
 *
 * 分块推进 `parts` 数组、最后一次 `join`：`binary += ...` 那种写法每追加一块都会
 * **整串重建**一次，长链上分配曲线接近 O(n²)，正是"大图另存 OOM"里可以白省掉的那一段。
 */
export function bytesToBase64(buf: Uint8Array): string {
  const parts: string[] = [];
  for (let i = 0; i < buf.length; i += B64_CHUNK) {
    // 子视图不复制字节；fromCharCode 的入参被 chunk 限制在 8KiB 以内
    parts.push(String.fromCharCode(...buf.subarray(i, Math.min(i + B64_CHUNK, buf.length))));
  }
  return btoa(parts.join(""));
}

/**
 * 从 data URL 里直接摘出 base64 载荷；**不是 base64 编码的 data URL 一律返回 null**。
 *
 * ⚠️ `data:` 有两种载荷：`;base64,` 后面是 base64，而**不带** `;base64` 的那一种是
 * 百分号编码的原文。把后者当成前者直接切尾巴，会得到一份"看着像 base64、解出来是乱码"的
 * 文件 —— 后端 `save_data_file` 只会把它 base64 解码写盘，错得很安静，所以宁可返回 null
 * 让调用方退回 `fetch` 路径。
 */
export function dataUrlBase64(url: string): string | null {
  if (!url.startsWith("data:")) return null;
  const comma = url.indexOf(",");
  if (comma < 0) return null;
  if (!url.slice(0, comma).includes(";base64")) return null;
  return url.slice(comma + 1);
}

/**
 * 取图片字节并编码成 base64，供 `invoke("save_data_file", { base64Data })` 落盘。
 *
 * data URL 走零拷贝短路；其余（`blob:` / asset URL / http）才付一次 `fetch`。
 */
export async function urlToBase64(url: string): Promise<string> {
  const inline = dataUrlBase64(url);
  if (inline !== null) return inline;
  const res = await fetch(url);
  if (!res.ok) throw new Error(`读取图片失败：HTTP ${res.status}`);
  return bytesToBase64(new Uint8Array(await res.arrayBuffer()));
}
