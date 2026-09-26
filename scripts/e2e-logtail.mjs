/**
 * 双实例 E2E 的「读应用日志」判据层 —— 唯一事实来源。
 *
 * 事实（`src-tauri/src/logging.rs:128` + `:296-324`，2026-09-26 逐行核对）：
 *   1. 日志按行**追加**写进 `logs/<stem>.log`，跨启动不清档；
 *   2. 每一次 append 前先 `metadata()` 量当前文件，一旦 `> MAX_LOG_FILE_BYTES`（512*1024 = 524288 B）
 *      就 `remove_file(<stem>.old.log)` + `rename(当前 → .old.log)` —— 旧档**被覆盖**，当前档从零开始。
 *   ⇒ 两个后果，都已实测：
 *      a) 任何一行都可能从 `.log` 整体消失、出现在 `.old.log` 里；
 *      b) 更早的 `.old.log` 内容会被**删掉**，所以「数它出现几次、和启动前的次数比」
 *         这种判据会在轮转瞬间**变小**而不是变大。
 *
 * 实测踩到两次，第二次是本轮自己抓出来的：
 *   ① 「等实例打出 boot 完成行」读 `.log` 最后 60 行 ⇒ 10 MB 轮重启时 boot 行刚写进去就随轮转
 *      被挪走 ⇒ 20s 超时 ⇒ 整轮红、fail-fast 连带跳掉后面 9 步。
 *      事后核对：当时 `gosslan-2.log` 共 21 行、boot 行 0 条；boot 行在 `.old.log` 的 4255/4260/4277 行。
 *   ② 只把读取改成「跨 `.log` + `.old.log`」并**不够**：把 `.log` 垫到 524250 B 再跑同一轮，
 *      boot 行落进 `.old.log`、同时那份 `.old.log` 覆盖了上一份 ⇒ 跨档计数从 6 掉到 4，
 *      「比基线大」永远不成立 ⇒ 同样 20s 超时（EXIT=1，实测复现）。
 *
 * 所以真正的修法是**让判据读的那份日志从本次启动起归 harness 所有**：
 *   `stashLogs` 在每次 launch 之前把 `.log` / `.old.log` **整份移进 run 目录**（移动，不删，证据还在），
 *   本轮启动写下的行必然在 `.log` 里；再配 `readLogTail` 的跨档读取兜住「同一次启动内又轮转」那种极端情况。
 *   `bootReady` 仍按「比启动前基线多」判，用来挡住任何残留旧行造成的假绿。
 */
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

/** 应用侧的 boot 完成行（`src-tauri/src/lib.rs:252`）。改这行文案要同时改这里。 */
export const BOOT_LINE = "AppState::init 完成";

/** 当前日志文件对应的轮转旧档路径：`x.log` → `x.old.log`（与 logging.rs 的命名一致）。 */
export function rolledLog(file) {
  return file.replace(/\.log$/, ".old.log");
}

function readIfExists(file) {
  return fs.existsSync(file) ? fs.readFileSync(file, "utf8") : "";
}

/**
 * 读某个实例日志的尾部，**把轮转旧档也算进来**（旧档在前、当前档在后，保持时间顺序）。
 * 只解决「行被轮转藏起来」；判断「本次启动是否就绪」必须用 `bootReady` + `stashLogs`。
 */
export function readLogTail(file, n = 400) {
  const tail = (s) => s.split("\n").slice(-n).join("\n");
  const rolled = readIfExists(rolledLog(file));
  const cur = readIfExists(file);
  if (!rolled) return cur ? tail(cur) : "";
  if (!cur) return tail(rolled);
  return `${tail(rolled)}\n${tail(cur)}`;
}

/** 数一条 needle 在「当前 + 旧档」里一共出现几次。两份档之间必须补换行，否则边界两行会黏成一行。 */
export function countLog(file, needle) {
  const all = `${readIfExists(rolledLog(file))}\n${readIfExists(file)}`;
  return all.split("\n").filter((l) => l.includes(needle)).length;
}

/**
 * 把某实例现有的日志（含轮转旧档）**移动**到 run 目录留证，原位置清空。
 * 每次 launch 前调用一次 ⇒ 本轮启动写下的行必然在 `.log` 里，判据不再和轮转时间赛跑。
 * 是移动不是删除：文件内容逐字节进 `runDir`，事后照样能在报告目录里翻。
 */
export function stashLogs(file, runDir, tag) {
  fs.mkdirSync(runDir, { recursive: true });
  const moved = [];
  for (const f of [file, rolledLog(file)]) {
    if (!fs.existsSync(f)) continue;
    const to = path.join(runDir, `instance-${tag}.stashed-${path.basename(f)}`);
    fs.renameSync(f, to);
    moved.push(to);
  }
  return moved;
}

/** 启动前取基线（配合 stashLogs 用：清档之后必然是 0）。 */
export const bootBaseline = (file, needle = BOOT_LINE) => countLog(file, needle);

/**
 * 「本次启动是否已经打出 boot 完成行」。
 * `baseline` 必须是**同一趟 launch 前**（且已 stashLogs）取到的次数，只有次数变大才算就绪。
 */
export function bootReady(file, baseline, needle = BOOT_LINE) {
  return countLog(file, needle) > baseline;
}

// ── 自证：判据本身必须能红，且反向不空转 ────────────────────────────
// 夹具全在临时目录里造，不碰任何真实 appdata。
const bootLine = () => `${BOOT_LINE}（设置/目录/局域网就绪）`;
const filler = (n) => Array.from({ length: n }, (_, i) => `[x] 2026-01-01 00:00:0${i % 9} info [discovery] diag/line ${i}`);

/** 造一份「当前档 / 轮转旧档」布局，返回当前档路径。 */
function layout(dir, { curBoot, oldBoot, curLines = 3, oldLines = 3 }) {
  fs.mkdirSync(dir, { recursive: true });
  const cur = path.join(dir, "gosslan-9.log");
  const old = rolledLog(cur);
  fs.writeFileSync(cur, [...(curBoot ? [bootLine()] : []), ...filler(curLines)].join("\n"));
  fs.writeFileSync(old, [...(oldBoot ? [bootLine()] : []), ...filler(oldLines)].join("\n"));
  return cur;
}

/** 模拟一次真实轮转：当前档整份变成新旧档（旧档被覆盖），当前档只剩轮转之后写的行。 */
function rotate(cur, afterLines) {
  const curText = fs.readFileSync(cur, "utf8");
  fs.writeFileSync(rolledLog(cur), curText);
  fs.writeFileSync(cur, filler(afterLines).join("\n"));
}

/**
 * 跑一轮自证，返回失败列表（空 = 绿）。
 * harness 每轮启动前都会先跑它：判据被改坏时以「判据坏了」退出，而不是让人去猜一个 20s 超时是谁的错。
 */
export function selfcheckLogtail(tmpRoot = path.join(os.tmpdir(), `gosslan-logtail-${Date.now()}`)) {
  const fails = [];
  const ok = (name, cond, expect, actual) => {
    if (!cond) fails.push(`${name} —— 期望 ${expect}，实际 ${actual}`);
  };
  try {
    fs.mkdirSync(tmpRoot, { recursive: true });

    // ① 钉住今天真实那条红：只看当前档最后 60 行时，boot 行一旦被轮转走就永远看不见。
    //    这条不是"新判据的要求"，是**夹具必须真的在复现 bug**，否则后面几条绿都算自欺。
    {
      const dir = path.join(tmpRoot, "a");
      const cur = layout(dir, { curBoot: false, oldBoot: true, oldLines: 0 });
      const oldWay = fs.readFileSync(cur, "utf8").split("\n").slice(-60).join("\n").includes(BOOT_LINE);
      ok("夹具①旧写法必须真的看不见 boot 行（否则夹具没在复现 bug）",
        oldWay === false, "false（看不见）", String(oldWay));
    }

    // ② 钉住第二次实测到的红：光靠"跨档数次数 + 比基线"仍会失败，因为轮转把上一份 .old.log 覆盖了
    //    ⇒ 计数会**变小**。这条存在，是为了禁止未来谁把 stashLogs 删掉却以为读取端已经够了。
    {
      const dir = path.join(tmpRoot, "b");
      const cur = layout(dir, { curBoot: true, oldBoot: true });
      const base = countLog(cur, BOOT_LINE); // 启动前：当前 1 + 旧档 1 = 2
      rotate(cur, 3);                        // 本次 boot 行写进当前档后立刻轮转
      ok("夹具②不 stash 只比基线时必须判不出就绪（实测过的第二种红）",
        bootReady(cur, base, BOOT_LINE) === false, "false（计数被覆盖掉了）",
        String(bootReady(cur, base, BOOT_LINE)));
    }

    // ③ 正解：同一格布局，先 stashLogs 把旧档移走 ⇒ 本次 boot 行只能在当前档 ⇒ 就绪判得出。
    {
      const dir = path.join(tmpRoot, "c");
      const run = path.join(dir, "run");
      const cur = layout(dir, { curBoot: true, oldBoot: true });
      const moved = stashLogs(cur, run, "B");
      ok("夹具③stash 必须把两份档都移走（原位置不留半成品）",
        moved.length === 2 && !fs.existsSync(cur) && !fs.existsSync(rolledLog(cur)),
        "2 份都移走", `${moved.length} 份`);
      // 读之前先确认拿到了两个路径：否则 stash 被改成空操作时这里会**抛异常**而不是报断言红。
      if (moved.length === 2) {
        ok("夹具③stash 必须逐字节保住证据",
          fs.readFileSync(moved[0], "utf8").includes(BOOT_LINE), "旧内容仍可查", "读不到");
      }
      ok("夹具③stash 后基线必须是 0", bootBaseline(cur, BOOT_LINE) === 0, "0", String(bootBaseline(cur, BOOT_LINE)));
      fs.appendFileSync(cur, `\n${bootLine()}\n`); // 本次启动真的打了 boot 行
      ok("夹具③stash 后本轮 boot 行必须判得出就绪", bootReady(cur, 0, BOOT_LINE), "true", "false");
    }

    // ④ 反空转：本轮还没启动（stash 之后当前档是空的）⇒ 不许假绿。
    {
      const dir = path.join(tmpRoot, "d");
      const cur = layout(dir, { curBoot: true, oldBoot: true });
      stashLogs(cur, path.join(dir, "run"), "A");
      ok("夹具④stash 后本轮没启动时必须不就绪", bootReady(cur, 0, BOOT_LINE) === false,
        "false", "true");
    }

    // ⑤ 反空转：完全没有 boot 行 ⇒ 不许就绪；同时读取端必须跨档（同一次启动内轮转也要看得见）。
    {
      const dir = path.join(tmpRoot, "e");
      const cur = layout(dir, { curBoot: false, oldBoot: false, oldLines: 5 });
      ok("夹具⑤完全没有 boot 行时必须不就绪", bootReady(cur, 0, BOOT_LINE) === false, "false", "true");
      layout(dir, { curBoot: true, oldBoot: false });
      rotate(cur, 2);
      ok("夹具⑤readLogTail 必须跨到轮转旧档", readLogTail(cur, 400).includes(BOOT_LINE), "true", "false");
      ok("夹具⑤跨档计数不许把边界两行黏成一行", countLog(cur, BOOT_LINE) === 1, "1",
        String(countLog(cur, BOOT_LINE)));
      fs.writeFileSync(rolledLog(cur), `${filler(2).join("\n")}\n`); // 旧档清干净，只测当前档这一行
      fs.writeFileSync(cur, `${bootLine()}${bootLine()}\n`); // 同一行里塞两条（无换行分隔）
      ok("夹具⑤同一行内的两条只能算一条（按行判，不按 substring 判）",
        countLog(cur, BOOT_LINE) === 1, "1", String(countLog(cur, BOOT_LINE)));
    }
  } finally {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  }
  return fails;
}
