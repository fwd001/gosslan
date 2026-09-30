import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync, chmodSync, mkdirSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

/**
 * `scripts/publish-release-assets.sh` 的行为用例（不需要凭据、不需要 runner）。
 *
 * 为什么值得单开一份：那段逻辑今天从三份 YAML 里抽出来，而它管的是**发版那一刻**的结局空间。
 * 并发抢创建这件事在 CI 上只在 tag 上暴露一次（2026-09-30 实测红在 mac 那条腿），
 * 而 YAML 本地不可校验（这台 Mac 没有任何 YAML 解析器）⇒ 能验的那一半就落在脚本 + 这份假 `gh` 上。
 *
 * 七个场景覆盖"结局空间"的两侧：成功（新建 / 已存在复用 / 被抢先）、真失败（权限）不许被读成成功、
 * 上传重试到上限必须红、空产物必须在调用任何 `gh` 之前就红。
 *
 * ⚠️ 假 `gh` 会**拒绝任何不带 `--repo` 的 release 调用** —— 这一条不是装饰：
 * v4.31.40 三条 workflow 的发布任务全红，根因就是脚本在没有 checkout 的 job 里调 `gh release create`
 * 而没带仓库上下文，而当时的本地判据对这件事完全无感（七种结局照样全绿）。
 */
const SCRIPT = join(import.meta.dirname, "publish-release-assets.sh");

/** 假 `gh`：把每次调用的参数写进 log，再按 FAKE_SCENARIO 决定退码与输出。 */
const FAKE_GH = `#!/usr/bin/env bash
set -u
echo "$*" >> "$FAKE_GH_LOG"
bump() {
  local f="$FAKE_GH_STATE/$1" n=0
  [ -f "$f" ] && n=$(cat "$f")
  n=$((n + 1)); echo "$n" > "$f"; echo "$n"
}
# ★ 仓库上下文：release job 里**没有 actions/checkout**，真 gh 在这种目录下不带 --repo 会直接失败
#   （"GH requires a repository context"）。v4.31.40 三条 workflow 的发布任务就是这么全红的，
#   而当时的假 gh 不检查这一条 ⇒ 本地七种结局全绿、CI 一条都不绿。这条检查就是那格的判据。
case "$1 $2" in
  "release create"*|"release upload"* )
    case "$*" in *"--repo "* ) ;; *) echo "GH requires a repository context" >&2; exit 1 ;; esac ;;
esac
case "$1 $2" in
  "api "* )
    case "$FAKE_SCENARIO" in
      race) n=$(bump api); [ "$n" -le 1 ] && exit 1; exit 0 ;;
      denied|new) exit 1 ;;
      *) exit 0 ;;
    esac ;;
  "release create" )
    case "$FAKE_SCENARIO" in
      race) echo "Release $3 already exists" >&2; exit 1 ;;
      denied) echo "HTTP 403: Resource not accessible by integration" >&2; exit 1 ;;
      *) echo created; exit 0 ;;
    esac ;;
  "release upload" )
    case "$FAKE_SCENARIO" in
      retry_upload) n=$(bump up); [ "$n" -le 2 ] && { echo "server said no" >&2; exit 1; }; exit 0 ;;
      upload_dies) echo "always 502" >&2; exit 1 ;;
      *) exit 0 ;;
    esac ;;
esac
exit 0
`;

interface Run {
  code: number | null;
  out: string;
  calls: string[];
}

/** 跑一次脚本：assets 里放两份假产物，`gh` 换成假的那份。 */
function run(scenario: string, opts: { files?: number; attempts?: number } = {}): Run {
  const dir = mkdtempSync(join(tmpdir(), "rel-case-"));
  const binDir = join(dir, "bin");
  mkdirSync(binDir);
  const log = join(dir, "calls.log");
  const ghPath = join(binDir, "gh");
  writeFileSync(ghPath, FAKE_GH, "utf8");
  chmodSync(ghPath, 0o755);
  const state = join(dir, "state");
  mkdirSync(state, { recursive: true });
  const assets = join(dir, "assets");
  mkdirSync(assets, { recursive: true });
  const n = opts.files === undefined ? 2 : opts.files;
  for (let i = 0; i < n; i += 1) writeFileSync(join(assets, `Gosslan_${i}.dmg`), "x");

  const r = spawnSync("bash", [
    SCRIPT,
    "--assets", assets,
    "--tag", "v9.9.9",
    "--sha", "deadbeef",
    "--repo", "octo/thing",
    "--attempts", String(opts.attempts ?? 5),
    "--sleep-base", "0",
  ], {
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${binDir}:${process.env.PATH}`,
      // ★ 故意把 LC_CTYPE 固定成 UTF-8：macOS 自带 bash 3.2 在该 locale 下会把紧跟变量名的
      //   全角标点第一个字节当进名字（$upload_err）⇒ set -u 报 unbound）。护栏整跑正是踩在
      //   这个 locale 上（CPython 的 C-locale 强转把 LC_CTYPE=C.UTF-8 复制给了子进程），
      //   而 node 起的步骤不是 ⇒ 同一条 npm test 两种结局。让测试永远用那一侧跑，类就锁死。
      LC_CTYPE: "C.UTF-8",
      GH_BIN: "gh",
      FAKE_SCENARIO: scenario,
      FAKE_GH_LOG: log,
      FAKE_GH_STATE: state,
    },
  });
  return {
    code: r.status,
    out: `${r.stdout ?? ""}${r.stderr ?? ""}`,
    calls: existsSync(log) ? readFileSync(log, "utf8").split("\n").filter(Boolean) : [],
  };
}

test("新建：Release 不存在且创建成功 ⇒ 建一次、上传一次", () => {
  const r = run("new");
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(
    r.calls.filter((c) => c.startsWith("release create")),
    ["release create v9.9.9 --repo octo/thing --target deadbeef --generate-notes --title v9.9.9"],
    "创建那条必须显式带 --repo：不带就是 v4.31.40 全红的形状",
  );
  assert.ok(r.calls.every((c) => !c.startsWith("release upload") || c.includes("--repo octo/thing")),
    "上传那条也得带 --repo（同一条 job 里没有 git 工作目录）");
  assert.equal(r.calls.filter((c) => c.startsWith("release upload")).length, 1);
});

test("已存在：不许重复创建，直接补文件（重复创建今天会红）", () => {
  const r = run("exists");
  assert.equal(r.code, 0, r.out);
  assert.equal(r.calls.filter((c) => c.startsWith("release create")).length, 0, "已存在还去创建 ⇒ 就是那份竞争红");
  assert.equal(r.calls.filter((c) => c.startsWith("release upload")).length, 1);
});

test("被抢先（race）：创建失败但复查询发现已存在 ⇒ 认它并上传，整步仍为绿", () => {
  const r = run("race");
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /是并发的另一条刚建好/);
  assert.equal(r.calls.filter((c) => c.startsWith("release upload")).length, 1);
  // 顺序本身就是证据：查（不存在）⇒ 创建（失败）⇒ 再查（已存在）⇒ 才上传。少了第二次查询就是"猜"。
  assert.deepEqual(r.calls.map((c) => c.split(" ").slice(0, 2).join(" ")).slice(0, 4),
    ["api repos/octo/thing/releases/tags/v9.9.9", "release create", "api repos/octo/thing/releases/tags/v9.9.9", "release upload"],
    r.calls.join("\n"));
});

test("真失败（权限 403）：复查询也不存在 ⇒ 必须红，且一次上传都不许发生", () => {
  const r = run("denied");
  assert.equal(r.code, 1, `403 被当成成功了吗？输出：${r.out}`);
  assert.match(r.out, /真失败/);
  assert.equal(r.calls.filter((c) => c.startsWith("release upload")).length, 0, "没建成就不该往上挂文件");
});

test("上传抖两下：三次之内成功 ⇒ 绿，且确实重试了三次", () => {
  const r = run("retry_upload");
  assert.equal(r.code, 0, r.out);
  assert.equal(r.calls.filter((c) => c.startsWith("release upload")).length, 3);
  assert.match(r.out, /第 2 次上传失败/);
});

test("上传一直失败：到上限就红（绝不 continue-on-error）", () => {
  const r = run("upload_dies", { attempts: 3 });
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /挂了 3 次仍失败/);
  assert.equal(r.calls.filter((c) => c.startsWith("release upload")).length, 3);
});

test("空产物：在任何一次 gh 调用之前就红（宁可不发，也不发少一档的 Release）", () => {
  const r = run("new", { files: 0 });
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /一个文件都没有/);
  assert.deepEqual(r.calls, [], "空产物还去问 Release 存在与否 ⇒ 这一步本该完全不碰 gh");
});
