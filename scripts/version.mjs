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
  if (!ch.includes("## [Unreleased]")) {
    const placeholder = `## [Unreleased]\n### Changed\n- 版本发布 v${next}（本次未预先填写更新说明，明细见 tag v${cur}...v${next} 的提交记录）\n`;
    const firstSection = ch.indexOf("\n## [");
    const insertAt = firstSection === -1 ? ch.length : firstSection;
    ch = ch.slice(0, insertAt) + "\n" + placeholder + ch.slice(insertAt);
  }
  ch = ch.replace("## [Unreleased]", `## [${next}] - ${new Date().toISOString().slice(0, 10)}`);
  writeFileSync(changelogPath, ch);
}

console.log(`版本已更新：${cur} -> ${next}`);
console.log(`产物命名示例：gosslan_${next}_x64-setup.exe`);
