// §十六「报告要带 screenshots/」这一格的采集器与判据。
//
// 为什么单独成文件（和 e2e-logtail.mjs 同一个理由）：判据层自己也必须能被单独证明"不是空转"，
// 而跑一整轮双实例 E2E 要几十秒 —— 判据自己的账要能在**没有 release 产物**的机器上秒级复核。
//
// 这里刻意**只**证明"报告里真的有一张真实界面截图"这一件事：
//   · 截图有没有落盘、是不是空图 —— 机器判据；
//   · 截图里的界面"对不对" —— 不是机器判据（没有像素级断言），仍然按 §19 记 MANUAL/结构级。
// 把第二件事也算进"绿"里就是假证据，所以名字与文档都只许写第一件。
//
// 平台边界：只实现 macOS 的 `screencapture`。Windows 要自己实现（PowerShell CopyFromScreen 之类），
// **在实现之前这一格在 Windows 上是红，不是"跳过就算过"** —— 总指令§十禁止把没跑写成 PASS。

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import zlib from "node:zlib";

export const SHOTS_DIRNAME = "screenshots";
/** 「不是空图」= **PNG 结构成立**（签名 + IHDR 宽高 > 0），字节下限只用来挡桩文件。
 *  ⚠️ 这里原来是 200 KiB，被实测推翻（2026-09-26）：判据 ③ 在一屏近乎空白的桌面上
 *  只截出 **106,973 B** ⇒ 每一轮 E2E 在起跑前被自己的"先修判据再跑轮"拦停。
 *  那次的红是**判据坏了**（阈值按"App 窗口满屏"那一屏校准，桌面空了就永久达不到），
 *  不是"没截到"。体积随屏幕内容跨两个数量级，所以它不能当主判据；结构可以。
 *  ⇒ 别把这个下限再调回去。 */
export const MIN_SHOT_BYTES = 4 * 1024;
const PNG_SIG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

/** PNG 头部：签名 + 紧跟的 IHDR 宽高。结构不成立返回 null。 */
export function pngHeader(buf) {
  if (buf.length < 24 || !buf.subarray(0, 8).equals(PNG_SIG)) return null;
  if (buf.readUInt32BE(12) !== 0x49484452 /* "IHDR" */) return null;
  return { w: buf.readUInt32BE(16), h: buf.readUInt32BE(20) };
}

/**
 * 整张是不是**纯色帧**（屏幕被锁 / 显示器休眠时 `screencapture` 退 0、结构完好、内容全黑）。
 *
 * 为什么必须有这条：今天的实测 —— 锁屏时截出来 106,973 B、签名与 IHDR 全对，
 * 而 2026-09-26 那次"桌面近乎空白"的 106,973 B **一个字节都不差**
 * ⇒ 体积与结构两条判据都分不开"真截图"和"一屏黑"，于是「报告带两张真实界面截图」
 * 可以在屏幕锁着的情况下判绿。这正是§十禁止的那类假证据。
 *
 * 只解 8 位、非隔行的真彩/灰度（`screencapture` 就是这一类）；**解不了就返回 false**
 * —— 这条判据只许把"确证是纯色"的判掉，不许因为看不懂就判掉（那会变成新的"判据坏了"红）。
 */
export function pngIsBlank(file, maxPixels = 400_000) {
  let buf;
  try {
    buf = fs.readFileSync(file);
  } catch {
    return false;
  }
  const head = buf.length >= 33 ? pngHeader(buf) : null;
  if (!head) return false;
  const bitDepth = buf[24], colorType = buf[25], interlace = buf[28];
  if (bitDepth !== 8 || interlace !== 0 || ![0, 2, 3, 4, 6].includes(colorType)) return false;
  const channels = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 }[colorType];
  const bpp = channels; // 8 位 ⇒ 每像素 bpp 字节
  // 拼 IDAT
  const idat = [];
  let off = 8;
  while (off + 8 <= buf.length) {
    const len = buf.readUInt32BE(off);
    const type = buf.readUInt32BE(off + 4);
    if (len === 0 && type === 0x49454e44 /* IEND */) break;
    if (type === 0x49444154 /* IDAT */) idat.push(buf.subarray(off + 8, off + 8 + len));
    off += 12 + len;
  }
  let raw;
  try {
    raw = zlib.inflateSync(Buffer.concat(idat));
  } catch {
    return false;
  }
  const stride = head.w * bpp;
  if (raw.length < (stride + 1) * head.h) return false;
  // 逐行反过滤（0 None / 1 Sub / 2 Up / 3 Average / 4 Paeth）
  const step = Math.max(1, Math.floor((head.w * head.h) / maxPixels));
  const prev = Buffer.alloc(stride);
  const cur = Buffer.alloc(stride);
  const seen = new Set();
  let idx = 0, sampled = 0;
  for (let y = 0; y < head.h; y++) {
    const ft = raw[idx++];
    raw.copy(cur, 0, idx, idx + stride);
    idx += stride;
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? cur[x - bpp] : 0;
      const b = prev[x];
      const c = x >= bpp ? prev[x - bpp] : 0;
      let v = cur[x];
      if (ft === 1) v = (v + a) & 0xff;
      else if (ft === 2) v = (v + b) & 0xff;
      else if (ft === 3) v = (v + ((a + b) >> 1)) & 0xff;
      else if (ft === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
        v = (v + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c)) & 0xff;
      } else if (ft !== 0) return false;
      cur[x] = v;
    }
    if (y % Math.max(1, Math.ceil(step / head.w)) === 0) {
      for (let x = 0; x < head.w; x += Math.max(1, Math.floor(head.w / (maxPixels / 8)))) {
        const o = x * bpp;
        // 灰度/带 alpha 的都按"取第一个通道"算：纯色帧的每个通道都一样
        seen.add(colorType === 2 || colorType === 6
          ? `${cur[o]},${cur[o + 1]},${cur[o + 2]}` : String(cur[o]));
        sampled++;
        if (seen.size > 2) return false; // 早停：已经不止一色（允许一条分隔线级别的差异）
      }
    }
    prev.set(cur);
  }
  return sampled > 0 && seen.size <= 1;
}

/** E2E_NO_CAPTURE=1 是这条判据的**反证开关**：假装本机没有采集器 ⇒ 那条断言必须红，
 *  而且红的原因是"没采到"，不是"判据写坏了"。这也是 Windows 腿今天的真实状态（那里还没有采集器）。 */
export const captureSupported = (platform = process.platform) =>
  process.env.E2E_NO_CAPTURE !== "1"
  && platform === "darwin" && fs.existsSync("/usr/sbin/screencapture");

export function shotsDir(runDir) {
  return path.join(runDir, SHOTS_DIRNAME);
}

/** 截一张全屏；失败返回 null（不抛 —— 截图不该把一轮 E2E 变成技术故障）。 */
export function captureShot(runDir, tag) {
  if (!captureSupported()) return null;
  const dir = shotsDir(runDir);
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, `${tag}.png`);
  try {
    // -x 不出快门声；不带 -l（窗口 id 要额外权限与 CoreGraphics 枚举）⇒ 抓整屏，
    // 两个实例的窗口此刻都在这台机器的桌面上。
    execFileSync("/usr/sbin/screencapture", ["-x", file], { stdio: "ignore" });
  } catch {
    return null;
  }
  return shotIsReal(file) ? file : null;
}

/**
 * 按 pid 枚举它自己名下**可见过**的窗口（#91）。
 * 返回 null = **这台机器问不出来**（python3 或 pyobjc 不在）；数组 = 问到了（可能为空）。
 * 三态必须分开（和群明文那个 `mentions` 同一个理由）：「没材料」与「材料是空的」不是一件事，
 * 混起来会把"问不出窗口"读成"这个进程没有界面"。
 *
 * ⚠️ 必须用 `kCGWindowListOptionAll`，不能用 `OnScreenOnly`：2026-09-27 同一分钟实测
 * 15 扇 vs 40 扇，而且**同一个 pid 在两次调用之间从"有窗口"变成"没窗口"**（切了 Space）
 * ⇒ 拿瞬时读数当判据是本仓第二次踩这个形状（第一次见 roadmap §12.7 被撤掉的那条判据）。
 */
export function listPidWindows(pid = null) {
  const py = `
import sys,json
try:
    import Quartz
except Exception:
    sys.exit(3)
arg=sys.argv[1]
pid=None if arg=='-' else int(arg)
wl=Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionAll,Quartz.kCGNullWindowID)
out=[]
for w in wl:
    if int(w.get('kCGWindowLayer',1))!=0: continue
    if pid is not None and int(w.get('kCGWindowOwnerPID',-1))!=pid: continue
    b=w.get('kCGWindowBounds') or {}
    ww,hh=int(b.get('Width',0)),int(b.get('Height',0))
    if ww<80 or hh<80: continue
    out.append({'id':int(w['kCGWindowNumber']),'w':ww,'h':hh,
                'on':1 if w.get('kCGWindowIsOnscreen') else 0,
                'pid':int(w.get('kCGWindowOwnerPID',-1))})
out.sort(key=lambda r:(-r['on'],-(r['w']*r['h'])))
print(json.dumps(out))
`;
  try {
    const out = execFileSync("python3", ["-c", py, pid === null ? "-" : String(pid)],
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], timeout: 8000 });
    const arr = JSON.parse(out);
    return Array.isArray(arr) ? arr : null;
  } catch {
    return null; // 退码 3（没 pyobjc）与"python3 根本不在 PATH"都算问不出来，不冒充"没有窗口"
  }
}

/** 那张 PNG 是不是**这一扇窗自己的**几何。整屏帧在 Retina 下与窗口帧同比例，所以只能按 bounds 核。 */
function windowGeometryMatches(head, win) {
  if (!head) return false;
  const sx = head.w / win.w, sy = head.h / win.h;
  return sx >= 0.9 && Math.abs(sx - sy) < 0.05 && Math.abs(sx - Math.round(sx)) < 0.06;
}

/** 抓指定那一扇窗。失败返回 `{ ok:false, why }` —— why 要能读，因为报告里那句"为什么只能用整屏"靠它。 */
export function captureWindowBySpec(runDir, tag, win) {
  if (!captureSupported()) return { ok: false, why: "本机没有可用采集器" };
  if (!win) return { ok: false, why: "没有可读的窗口" };
  const dir = shotsDir(runDir);
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, `${tag}.png`);
  fs.rmSync(file, { force: true }); // 截不出就必须没有文件：留着上一轮的旧图比没有更坏
  try {
    execFileSync("/usr/sbin/screencapture", ["-o", "-x", `-l${win.id}`, file],
      { stdio: "ignore", timeout: 10_000 });
  } catch {
    /* 退码非 0 不下结论，看文件在不在 —— 实测：不在当前 Space 的窗口走这里 */
  }
  if (!fs.existsSync(file)) {
    return { ok: false, why: `窗口 ${win.id} 截不出文件（不在当前 Space / 已最小化）` };
  }
  if (!shotIsReal(file)) {
    fs.rmSync(file, { force: true });
    return { ok: false, why: `窗口 ${win.id} 截出来不合格（空图或纯色帧）` };
  }
  const head = pngHeader(fs.readFileSync(file).subarray(0, 24));
  if (!windowGeometryMatches(head, win)) {
    fs.rmSync(file, { force: true });
    return {
      ok: false,
      why: `窗口 ${win.id} 的几何对不上（png ${head?.w}×${head?.h} vs bounds ${win.w}×${win.h}）`,
    };
  }
  return {
    ok: true, file, windowId: win.id, pid: win.pid,
    bounds: { w: win.w, h: win.h }, png: head, scale: Math.round(head.w / win.w),
  };
}

/** 按**实例自己的 pid** 抓那一扇最大的窗。抓不到不是错误：调用方回落整屏，并把原因如实记进报告。 */
export function captureWindowShot(runDir, tag, pid) {
  if (!Number.isInteger(pid) || pid <= 0) return { ok: false, why: "没有这个实例的 pid" };
  const wins = listPidWindows(pid);
  if (!wins) return { ok: false, why: "这台机器问不出窗口清单（没有 python3/pyobjc）" };
  if (!wins.length) return { ok: false, why: `pid ${pid} 名下没有 ≥80×80 的窗口` };
  return captureWindowBySpec(runDir, tag, wins[0]);
}

export function shotIsReal(file, minBytes = MIN_SHOT_BYTES) {
  let buf;
  try {
    buf = fs.readFileSync(file);
  } catch {
    return false;
  }
  if (buf.length < minBytes) return false;
  const head = pngHeader(buf);
  if (!head || head.w <= 0 || head.h <= 0) return false;
  // 结构成立还不够：锁屏时截到的就是一张结构完好的纯色帧。
  return !pngIsBlank(file);
}

/** 给 harness 的报错用的：区分"这台机器没有采集器"与"采到了但是一屏黑"。 */
export function describeShotDir(runDir) {
  const dir = shotsDir(runDir);
  let files = [];
  try {
    files = fs.readdirSync(dir).filter((f) => f.endsWith(".png"));
  } catch {
    return "screenshots/ 不存在（本机没有可用采集器？）";
  }
  if (!files.length) return "screenshots/ 里没有一张 PNG";
  const blanks = files.filter((f) => pngIsBlank(path.join(dir, f))).length;
  if (blanks === files.length) {
    return `${blanks} 张全是纯色帧 ⇒ 屏幕被锁 / 显示器休眠，这一轮的截图证据不成立（不是产品缺陷）`;
  }
  return `${files.length} 张里有 ${blanks} 张纯色帧`;
}

/**
 * 这台机器的会话现在是不是**锁着**（loginwindow 盖在整屏上面）。
 * true / false / null = 问不出来（没装 pyobjc 的平台、python3 不在 PATH）。
 *
 * 为什么不能只靠"整屏纯色"：2026-09-27 实测 —— 锁屏时全屏抓到的是**桌面壁纸**，
 * 壁纸有几十种颜色 ⇒ `pngIsBlank` 放它过，那一轮的"截图证据"其实什么都没拍到。
 * 系统自带的 `CGSessionCopyCurrentDictionary` 直接给答案，不需要新依赖。
 */
export function screenLockedState() {
  try {
    const out = execFileSync("python3", ["-c",
      "import Quartz;print(int(Quartz.CGSessionCopyCurrentDictionary()"
      + ".get('CGSSessionScreenIsLocked', 0)))",
    ], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
    if (out === "1") return true;
    if (out === "0") return false;
    return null;
  } catch {
    return null; // 问不出来不算"锁着"：交给纯色那一道兜，别把没有 pyobjc 的机器全拦死
  }
}

/**
 * **起跑前置**：这台机器的屏幕现在给不给得出真实帧。
 * 返回 null = 可以跑；返回字符串 = 停跑理由。
 *
 * 为什么要有这一道（与"二进制比源码旧 ⇒ 你正在测旧代码"同一形状）：截图判据补上
 * 「纯色帧判假」之后，锁屏会让**每一轮**跑到最后一步才红 —— 前面八分钟白烧，
 * 而红的那条看起来像产品问题。环境不满足就必须在起跑前用一句话说清，
 * 否则下一个人为了"让门禁绿"会去降低判据的门槛（那正是今天这个假绿的来路）。
 *
 * 没有采集器的平台（Windows / `E2E_NO_CAPTURE=1`）**不在这里拦** ——
 * 那里的处置早已定成"轮次里那条断言明着红，不是跳过"（§十），这里插手会改变它的语义。
 */
export function screenBlockedReason(tmpRoot = path.join(os.tmpdir(), `gosslan-screen-${Date.now()}`)) {
  if (!captureSupported()) return null;
  const locked = screenLockedState();
  if (locked === true) {
    return "会话处于锁定状态（loginwindow 盖住整屏）⇒ 全屏抓到的是桌面壁纸，"
      + "这一层要判的「真实界面截图」拿不到。解锁后再跑（不要去放宽截图判据 —— 那正是刚修掉的假绿）。";
  }
  try {
    fs.mkdirSync(tmpRoot, { recursive: true });
    const probe = path.join(tmpRoot, "probe.png");
    execFileSync("/usr/sbin/screencapture", ["-x", probe], { stdio: "ignore" });
    if (!fs.existsSync(probe)) return null; // 截不出文件交给自证那条，这里不重复判
    return pngIsBlank(probe)
      ? "屏幕被锁 / 显示器休眠：screencapture 退出码 0 却只给得出纯色帧 ⇒ 这一层要判的"
        + "「真实界面截图」今天拿不到。解锁后再跑（不要去放宽截图判据 —— 那正是刚修掉的假绿）。"
      : null;
  } catch {
    return null; // 探测本身失败不算环境问题，交给自证/轮次去红
  } finally {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  }
}

/** 自证用的最小 PNG 写入器：只写 8 位真彩、非隔行、逐行 filter 0/1 的合法块（CRC 不校验，本模块不读它）。 */
function writeProbePng(file, w, h, colorAt) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;   // bit depth
  ihdr[9] = 2;   // color type: truecolor
  const raw = Buffer.alloc(h * (1 + w * 3));
  let o = 0;
  for (let y = 0; y < h; y++) {
    raw[o++] = 0; // filter: None
    for (let x = 0; x < w; x++) {
      const [r, g, b] = colorAt(x, y);
      raw[o++] = r; raw[o++] = g; raw[o++] = b;
    }
  }
  const idat = zlib.deflateSync(raw);
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length, 0);
    return Buffer.concat([len, Buffer.from(type, "ascii"), data, Buffer.alloc(4)]);
  };
  fs.writeFileSync(file, Buffer.concat([
    PNG_SIG, chunk("IHDR", ihdr), chunk("IDAT", idat), chunk("IEND", Buffer.alloc(0)),
  ]));
}

/** 判据自证：不跑实例，只证明上面这几条判据**既能真也能红**。返回失败原因数组（空=全过）。 */
/** 返回 { fails, notes }：
 *  · fails 非空 = **判据本身坏了**（空图/缺文件也判得过）⇒ harness 必须在跑轮之前停下；
 *  · notes     = 这台机器**根本没有采集器**（Windows，或被 E2E_NO_CAPTURE=1 关掉）——
 *    那不是判据坏了，是这一格在它上面还没实现，必须与"判据红"分开记，否则
 *    「平台缺能力」会把所有轮次变成"先修判据再跑轮"的死循环。 */
export function selfcheckShot(tmpRoot = path.join(os.tmpdir(), `gosslan-shot-${Date.now()}`)) {
  const fails = [];
  const notes = [];
  const has = (label, cond) => { if (!cond) fails.push(label); };
  fs.mkdirSync(tmpRoot, { recursive: true });
  try {
    // ① 空文件 / 小文件必须判假 —— 否则"落盘了"就等于"截到了"，正是最省事的一种假绿
    const tiny = path.join(tmpRoot, "tiny.png");
    fs.writeFileSync(tiny, Buffer.alloc(16));
    has("① 16 字节的假 PNG 必须被判为『不是真实截图』", !shotIsReal(tiny));
    // ② 文件不存在必须判假（截图器挂了不能退化成"没截也算过"）
    has("② 不存在的文件必须被判为『不是真实截图』", !shotIsReal(path.join(tmpRoot, "nope.png")));
    // ③ 体积够但**结构不成立**（签名对、IHDR 宽高为 0）必须判假 ——
    //   这一格是"结构判据不是摆设"的证明：把上面那两个宽高比较删掉，这条立刻红。
    const stub = path.join(tmpRoot, "stub.png");
    {
      const head = Buffer.alloc(33);
      PNG_SIG.copy(head, 0);
      head.writeUInt32BE(13, 8);
      head.write("IHDR", 12);
      head.writeUInt32BE(0, 16); // width = 0
      head.writeUInt32BE(0, 20); // height = 0
      fs.writeFileSync(stub, Buffer.concat([head, Buffer.alloc(MIN_SHOT_BYTES)]));
    }
    has("③ 体积够但 IHDR 宽高为 0 的桩 PNG 必须被判为『不是真实截图』", !shotIsReal(stub));
    // ⑤ **结构完好但整屏纯色**必须判假。今天（2026-09-27 凌晨）实测到的洞：屏幕被锁时
    //   `screencapture` 退 0、截出 106,973 B、签名与 IHDR 全对，而 2026-09-26 那次
    //   "桌面近乎空白"的产物**一个字节都不差** ⇒ 只靠体积与结构两条判据，
    //   「报告带两张真实界面截图」可以在锁屏状态下判绿 —— 那是§十明令禁止的假证据。
    const black = path.join(tmpRoot, "black.png");
    writeProbePng(black, 800, 600, () => [0, 0, 0]);
    has("⑤ 结构完好但整屏纯色的 PNG 必须被判为『不是真实截图』",
      !shotIsReal(black, 0) && pngIsBlank(black));
    // ⑥ 反向：同尺寸只多一条白线就必须判真 —— 否则 ⑤ 可以靠"什么图都判假"混过去。
    const lined = path.join(tmpRoot, "lined.png");
    writeProbePng(lined, 800, 600, (_x, y) => (y === 300 ? [255, 255, 255] : [0, 0, 0]));
    has("⑥ 只多一条白线的同尺寸 PNG 必须判真（证明 ⑤ 不是「永远判假」）", shotIsReal(lined, 0));
    // ⑦ ⑧ #91「按窗口 id 抓」这一格的自证。放在整屏那几格**之后**、真截整屏那格之前：
    //   它们判的是"按窗口抓"这个机制成不成立，与屏幕上是谁的窗口无关。
    const wins = listPidWindows(null);
    if (wins === null) {
      notes.push("⑦ 跳过：这台机器问不出窗口清单（没有 python3/pyobjc）⇒ 按窗口抓这一格在本机未实现，不是判据坏了");
    } else if (!wins.length) {
      notes.push("⑦ 跳过：本机此刻没有任何 ≥80×80 的 layer-0 窗口");
    } else {
      // 逐扇试（最多 5 扇）：macOS 拒绝给**不在当前 Space / 已最小化**的窗口出图（实测 `could not create image
      // from window`、不落盘），那是环境状态而不是判据坏了 —— 所以这一格与 ④ 同一处置：拿不到就记 note，
      // 只有"截出来了但几何不是那一扇窗自己的"才是判据真有毛病。
      const tried = [];
      let won = null;
      for (const w of wins.slice(0, 5)) {
        const r = captureWindowBySpec(tmpRoot, "7-window", w);
        if (r.ok) { won = r; break; }
        tried.push(r.why);
      }
      if (won) {
        // ⚠️ 这一格**不许**回头调 `captureWindowBySpec` 里那个几何判据 —— 那样它就只是"我信我自己"，
        //   正好是本仓反复判过的"半个守卫"（存在性/点名式断言）。所以判据只吃**产物**：
        //   ① 窗口帧的宽高必须等于那扇窗自己的 bounds × 同一个整数倍（scale 只从宽度推，
        //      于是"其实是整屏"会在高度上对不上 —— 实测整屏 1912 高 vs 窗口 1718 高）；
        //   ② 同一时刻再抓一张整屏做对照：窗口比屏幕小时，两张几何必须**不同**。
        const full = path.join(tmpRoot, "7-fullscreen.png");
        execFileSync("/usr/sbin/screencapture", ["-x", full], { stdio: "ignore" });
        const fh = pngHeader(fs.readFileSync(full).subarray(0, 24));
        fs.rmSync(full, { force: true });
        const screenWinW = fh.w / won.scale, screenWinH = fh.h / won.scale;
        const winSmaller = won.bounds.w * won.bounds.h < screenWinW * screenWinH;
        has(`⑦ 窗口帧必须读得出"它是那一扇窗、不是整屏"（窗口 ${won.bounds.w}×${won.bounds.h} × scale ${won.scale}`
          + ` = 图 ${won.png.w}×${won.png.h}；同一时刻整屏 ${fh.w}×${fh.h}）`,
          won.png.w === won.bounds.w * won.scale && won.png.h === won.bounds.h * won.scale
          && (!winSmaller || won.png.w !== fh.w || won.png.h !== fh.h));
      } else {
        notes.push(`⑦ 跳过：本机这 ${Math.min(5, wins.length)} 扇窗口 macOS 都不肯出图（${tried[0] ?? "?"}）`
          + " ⇒ 与屏幕被锁同一类：环境限制，不是判据坏了");
      }
      // ⑧ 反证：**不存在的窗口 id 必须一个文件都不留下**。
      //   没有这一格，"id 写错了 / 窗口早关了"会静默变成一张别的图（最省事的一种假证据）。
      //   这一格不依赖 ⑦ 成不成 —— 它只需要一个"肯定不存在的号"，所以环境挡住 ⑦ 时它照判。
      const bogus = { id: Math.max(...wins.map((w) => w.id)) + 987654, w: 800, h: 600 };
      const rb = captureWindowBySpec(tmpRoot, "8-bogus-window", bogus);
      const left = fs.existsSync(path.join(shotsDir(tmpRoot), "8-bogus-window.png"));
      has(`⑧ 不存在的窗口 id（${bogus.id}）必须截不出文件 —— 实测 ok=${rb.ok} 文件是否存在=${left}`,
        !rb.ok && !left);
    }
    // ④ 真的截一张必须判真 —— 反过来钉住"判据没有苛刻到永远达不到"。
    //   ⚠️ 这一格在 2026-09-26 抓到过一次**判据自己坏**：桌面接近空白时截图只有 ~104 KB，
    //   当时那条按体积定的阈值（200 KiB）判它"不是真截图" ⇒ 每一轮 E2E 起跑前被拦停。
    //   教训：**判据的输入必须选环境不变量（结构），不能选随屏幕内容摆动的量（体积）**。
    if (captureSupported()) {
      const dir = shotsDir(tmpRoot);
      fs.mkdirSync(dir, { recursive: true });
      const real = path.join(dir, "probe.png");
      execFileSync("/usr/sbin/screencapture", ["-x", real], { stdio: "ignore" });
      if (!fs.existsSync(real)) {
        // 说支持却截不出文件 = 判据层自己坏了（不是环境问题）⇒ 必须拦停
        fails.push("④ 采集器被判定为可用，却一个文件都没写出 ⇒ 判据层坏了，不是环境问题");
      } else if (pngIsBlank(real)) {
        // 屏幕被锁 / 显示器休眠：screencapture 退 0、结构完好、内容全黑。
        // 这**不是**判据坏了（那条会拦停所有轮次），是这一格的运行时证据今天拿不到
        // —— 与 Windows 腿同一类处置：记说明，让轮次里那条断言自己红着（§十不许把没跑写成 PASS）。
        const sz = fs.statSync(real).size;
        notes.push(`④ 跳过：截到的是纯色帧（${sz} B，屏幕被锁 / 显示器休眠）`
          + " ⇒ 判据没坏，但这一轮的截图证据不成立；轮次里那条断言会照实报红");
      } else {
        const sz = fs.existsSync(real) ? fs.statSync(real).size : 0;
        const head = sz >= 24 ? pngHeader(fs.readFileSync(real).subarray(0, 24)) : null;
        has(`④ 真截一张必须判真（实测 ${sz} B，结构 ${head ? `${head.w}×${head.h}` : "不成立"}，下限 ${MIN_SHOT_BYTES} B）`,
          shotIsReal(real));
      }
    } else {
      notes.push(`④ 跳过：本机没有可用采集器（${process.platform}${process.env.E2E_NO_CAPTURE === "1" ? "，E2E_NO_CAPTURE=1" : ""}）`
        + " ⇒ 只证了「能判假」；这一格在该平台仍未实现，不是判据坏了");
    }  } catch (e) {
    fails.push(`自证本身抛错（这不该发生）：${e?.message ?? e}`);
  } finally {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  }
  return { fails, notes };
}
