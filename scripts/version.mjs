// 版本号统一维护脚本（SemVer）
// 用法：
//   node scripts/version.mjs show          # 打印当前版本
//   node scripts/version.mjs patch|minor|major
//
// 一次 bump 会同步更新三处版本号：package.json / src-tauri/Cargo.toml / src-tauri/tauri.conf.json，
// 并把 CHANGELOG.md 的 [Unreleased] 小节落为带日期的版本小节。
import { readFileSync, writeFileSync, existsSync } from "node:fs";

const mode = process.argv[2] ?? "show";

function parse(v) {
  const m = v.trim().match(/^(\d+)\.(\d+)\.(\d+)$/);
  if (!m) throw new Error(`非法版本号: ${v}`);
  return [Number(m[1]), Number(m[2]), Number(m[3])];
}

function bump(v, mode) {
  const [a, b, c] = parse(v);
  if (mode === "major") return `${a + 1}.0.0`;
  if (mode === "minor") return `${a}.${b + 1}.0`;
  if (mode === "patch") return `${a}.${b}.${c + 1}`;
  throw new Error(`未知 bump 类型: ${mode}（应为 patch|minor|major）`);
}

// 以 package.json 为版本源
const pkgPath = "package.json";
const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
const cur = pkg.version;

if (mode === "show") {
  console.log(cur);
  process.exit(0);
}

const next = bump(cur, mode);

// 1) package.json
pkg.version = next;
writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + "\n");

// 2) Cargo.toml
const cargoPath = "src-tauri/Cargo.toml";
let cargo = readFileSync(cargoPath, "utf8");
cargo = cargo.replace(/^version\s*=\s*"[^"]*"$/m, `version = "${next}"`);
writeFileSync(cargoPath, cargo);

// 3) tauri.conf.json
const confPath = "src-tauri/tauri.conf.json";
let conf = readFileSync(confPath, "utf8");
conf = conf.replace(/"version"\s*:\s*"[^"]*"/, `"version": "${next}"`);
writeFileSync(confPath, conf);

// 4) package-lock.json（保持与 package.json 的 name/version 一致，否则 npm ci 会报 out of sync）
const lockPath = "package-lock.json";
if (existsSync(lockPath)) {
  let lock = JSON.parse(readFileSync(lockPath, "utf8"));
  lock.name = pkg.name;
  lock.version = next;
  if (lock.packages && lock.packages[""]) {
    lock.packages[""].name = pkg.name;
    lock.packages[""].version = next;
  }
  writeFileSync(lockPath, JSON.stringify(lock, null, 2) + "\n");
}
// 注：src-tauri/Cargo.lock（TOML）由 cargo 构建时自动同步，发版前跑一次
// `cd src-tauri && cargo check` 保证 lock 与 Cargo.toml 版本一致后一并提交。

// 5) CHANGELOG.md（可选）
//    若不存在 [Unreleased] 小节则自动补一个占位小节，保证每次发版都有更新日志。
const changelogPath = "CHANGELOG.md";
if (existsSync(changelogPath)) {
  let ch = readFileSync(changelogPath, "utf8");
  // ⚠️ 必须**按行锚定**地找标题，不能用 `ch.includes("## [Unreleased]")`：
  // 真实事故——某条更新日志的正文里写了「补回 `## [Unreleased]` 小节」这句话，
  // 于是 includes 命中正文、replace 也替换正文里的那一处，把 4.1.0 小节从句子中间劈开
  // 并吞掉了真正的标题（结果：文件里再也没有 [Unreleased] 小节）。
  const HEADING = /^## \[Unreleased\][^\n]*$/m;
  const headingMatch = HEADING.exec(ch);
  if (!headingMatch) {
    const placeholder = `## [Unreleased]\n### Changed\n- 版本发布 v${next}（本次未预先填写更新说明，明细见 tag v${cur}...v${next} 的提交记录）\n`;
    const firstSection = ch.search(/^## \[/m);
    const insertAt = firstSection === -1 ? ch.length : firstSection;
    ch = ch.slice(0, insertAt) + placeholder + "\n" + ch.slice(insertAt);
  }
  // 用**本地日期**（toISOString 是 UTC，凌晨发版会日期错一天，如 GMT+8 的 00:25 落成前一天）。
  const now = new Date();
  const localDate = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
  // 只替换那一行标题（`$&` 是匹配到的标题本身），正文里同名的引用不会被碰。
  ch = ch.replace(HEADING, `$&\n\n## [${next}] - ${localDate}`);
  writeFileSync(changelogPath, ch);
}

console.log(`版本已更新：${cur} -> ${next}`);
console.log(`产物命名示例：gosslan_${next}_x64-setup.exe`);
