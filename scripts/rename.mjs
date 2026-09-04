// 一次性项目改名脚本：node scripts/rename.mjs <old> <new>
// 按「全小写 / 首字母大写 / 全大写」三种形态替换所有文本文件中的项目名，
// 跳过 node_modules / target / .git / dist / .workbuddy 与二进制资源。
import { readdirSync, statSync, readFileSync, writeFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const [, , oldName, newName] = process.argv;
if (!oldName || !newName) {
  console.error("用法: node scripts/rename.mjs <old> <new>");
  process.exit(1);
}
if (oldName === newName) {
  console.error("新旧名字相同，无需改名");
  process.exit(1);
}

const cap = (s) => (s ? s[0].toUpperCase() + s.slice(1) : s);
const OLD = oldName.toUpperCase();
const NEW = newName.toUpperCase();
const Old = cap(oldName);
const New = cap(newName);

const EXCLUDE = new Set([
  "node_modules", "target", ".git", "dist", ".workbuddy", ".idea", ".vscode",
]);
const BINARY_EXT = new Set([
  ".png", ".ico", ".icns", ".jpg", ".jpeg", ".webp", ".gif", ".svg",
  ".exe", ".dll", ".db", ".sqlite", ".sqlite3", ".woff", ".woff2", ".ttf",
  ".wasm", ".bin", ".lock",
]);

const files = [];
function walk(dir) {
  for (const name of readdirSync(dir)) {
    if (EXCLUDE.has(name)) continue;
    const p = join(dir, name);
    let st;
    try {
      st = statSync(p);
    } catch {
      continue;
    }
    if (st.isDirectory()) walk(p);
    else {
      const dot = p.lastIndexOf(".");
      const ext = dot >= 0 ? p.slice(dot).toLowerCase() : "";
      if (!BINARY_EXT.has(ext)) files.push(p);
    }
  }
}

walk(resolve("."));

let changed = 0;
for (const f of files) {
  const orig = readFileSync(f, "utf8");
  let s = orig;
  // 注意顺序：先大写、再首字母大写、最后全小写，避免前缀误伤
  s = s.split(OLD).join(NEW);
  s = s.split(Old).join(New);
  s = s.split(oldName).join(newName);
  if (s !== orig) {
    writeFileSync(f, s);
    changed++;
    console.log("updated:", relative(".", f));
  }
}
console.log(`\n完成：${changed} 个文件已从 ${oldName} 重命名为 ${newName}`);
