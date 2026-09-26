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

export const SHOTS_DIRNAME = "screenshots";
/** 一张真实的桌面 PNG（Retina 全屏）在这个量级以上；纯黑/纯色屏幕压不出这种体积。
 *  阈值取 200 KiB 是实测留余量：本机全屏截图 1.9–2.2 MB。 */
export const MIN_SHOT_BYTES = 200 * 1024;

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

export function shotIsReal(file, minBytes = MIN_SHOT_BYTES) {
  try {
    return fs.statSync(file).size >= minBytes;
  } catch {
    return false;
  }
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
    // ③ 真的截一张必须判真 —— 反过来钉住"阈值没有高到永远达不到"
    if (captureSupported()) {
      const dir = shotsDir(tmpRoot);
      fs.mkdirSync(dir, { recursive: true });
      const real = path.join(dir, "probe.png");
      execFileSync("/usr/sbin/screencapture", ["-x", real], { stdio: "ignore" });
      has(`③ 真截一张必须判真（实测 ${(fs.existsSync(real) ? fs.statSync(real).size : 0)} B，阈值 ${MIN_SHOT_BYTES} B）`,
        shotIsReal(real));
    } else {
      notes.push(`③ 跳过：本机没有可用采集器（${process.platform}${process.env.E2E_NO_CAPTURE === "1" ? "，E2E_NO_CAPTURE=1" : ""}）`
        + " ⇒ 只证了「能判假」；这一格在该平台仍未实现，不是判据坏了");
    }
  } catch (e) {
    fails.push(`自证本身抛错（这不该发生）：${e?.message ?? e}`);
  } finally {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  }
  return { fails, notes };
}
