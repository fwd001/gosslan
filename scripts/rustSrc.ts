/**
 * 把 commands.rs / db.rs 的 `include!` 子模块**递归**拼回完整源码视图。
 *
 * 物理拆分后 commands.rs 只剩 include! 指令，但前端测试要扫「Rust 编译期看到的完整源码」。
 * Rust 的 include! 是**递归展开**的（子模块里还能 include 别的），本模块同样递归解析 —
 * 跟 Rust 编译期看到的 tokens 完全对齐。
 *
 * 新增子模块只需要在 commands.rs / db.rs 或其子文件里加一行 `include!("...");`，
 * 这里的解析器会自动跟，不用手动维护文件列表。
 */
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

const RUST_SRC = resolve(import.meta.dirname, "..", "src-tauri", "src");

/**
 * 递归解析 include!("...") 指令，返回完整拼接源码。
 * - 根文件的 include! 在根目录找
 * - 子文件里的 include! 在子文件所在目录找（和 Rust 行为一致）
 * - 根文件/子文件里的 `// ---- xxx.rs ----` 分隔行 和 include! 行自身被去除
 * - 子文件里的职责边界注释**保留**（它们是内容的一部分）
 */
function resolveIncludes(filePath: string): string {
  const src = readFileSync(filePath, "utf8");
  const fileDir = dirname(filePath);

  // 收集本文件所有 include! 指令
  const includes: string[] = [];
  for (const m of src.matchAll(/include!\("([^"]+)"\);/g)) {
    includes.push(m[1]);
  }

  // 去除 include! 行自身（分隔注释 // ---- xxx.rs ---- 也一起去掉，保持干净）
  const stripped = src.replace(/^\s*\/\/\s*----\s*\S+\s*----\s*\n/gm, "").replace(
    /^\s*include!\("[^"]+"\);\s*\n?/gm,
    "",
  );

  // 先放本文件内容，再依次递归展开每个子文件
  const parts: string[] = [stripped];
  for (const inc of includes) {
    const childPath = join(fileDir, inc);
    parts.push(resolveIncludes(childPath));
  }
  return parts.join("\n");
}

/** 完整的 commands 模块源码（commands.rs + 所有子模块递归展开）。 */
export function readCommandsSrc(): string {
  return resolveIncludes(join(RUST_SRC, "commands.rs"));
}

/** 完整的 db 模块源码（db.rs + 所有子模块递归展开）。 */
export function readDbSrc(): string {
  return resolveIncludes(join(RUST_SRC, "db.rs"));
}

/**
 * 从 events.test.ts 搬进来的共用件（2026-09-28）：现在有两个消费方 —— 那条前端护栏，以及 `scripts/arch-map-stats.mjs` 的全仓计数。
 * 搬家的理由：计数不剥注释会把注释里的 `CREATE TABLE` 当建表（实测 18 张被数成 20），而这份解析已经处理了
 * 嵌套块注释、raw 字符串、字符字面量与生命周期 —— 再写第二份就是再造一个会各自漂的解析器。
 */
/**
 * 只保留 Rust 源码里的**代码**，把注释换成等长空白（保留换行，行号不偏移）。
 *
 * 为什么必须有：这条护栏扫的是 `emit(` 这个形态，而形态出现在注释里并不等于"后端在发事件"。
 * v4.24.0 现场被咬两次 —— 先在断言文本里写全那三个字符加左括号，再在注释里解释
 * "为什么不能写全"，第二次照样被扫成一个没人听的孤儿事件；当时的处理是**改写文案绕开**，
 * 那是在躲症状。真正要补的是"扫描前先分清水份"，所以这里按语法边界剥注释。
 *
 * ⚠️ 已知残留，刻意不在本次顺手做：**字符串字面量里**出现完整的 `emit("x"` 形态仍会被扫到。
 * 不能把字符串一起抹掉 —— 我们要找的恰恰就是 `emit("x")` 里那个字符串本身。要做对得先把
 * 事件名收进常量表、再按名比对（那属 #33③ 的另一片），而不是在这里加一条"看到引号就砍"。
 */
export function stripRustComments(src: string): string {
  let out = "";
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const d = src[i + 1] ?? "";
    // 字符串 / raw 字符串 / byte 字符串：整段原样跳过，否则串里的 `//` 会被当成注释吃掉后半文件
    if (c === '"' || (c === "b" && d === '"') || /^r#*"/.test(src.slice(i, i + 6))) {
      const end = skipString(src, i);
      out += src.slice(i, end);
      i = end;
      continue;
    }
    // 字符字面量（`'a'` / `'\n'`）整段跳过；生命周期 `'a` 后面没有闭合引号 ⇒ 不会被误吃
    const charLit = /^'(?:\\.|[^'\\])'/.exec(src.slice(i));
    if (charLit) {
      out += charLit[0];
      i += charLit[0].length;
      continue;
    }
    if (c === "/" && d === "/") {
      let j = src.indexOf("\n", i);
      if (j === -1) j = src.length;
      out += " ".repeat(j - i);
      i = j;
      continue;
    }
    if (c === "/" && d === "*") {
      // 块注释可嵌套（Rust 允许），深度归零前一路都当注释；换行保留以稳住行号
      let depth = 0;
      let j = i;
      while (j < src.length) {
        if (src[j] === "/" && src[j + 1] === "*") {
          depth += 1;
          j += 2;
          continue;
        }
        if (src[j] === "*" && src[j + 1] === "/") {
          depth -= 1;
          j += 2;
          if (depth === 0) break;
          continue;
        }
        j += 1;
      }
      out += src.slice(i, j).replace(/[^\n]/g, " ");
      i = j;
      continue;
    }
    out += c;
    i += 1;
  }
  return out;
  function skipString(s: string, from: number): number {
    const raw = /^r#*"/.exec(s.slice(from));
    if (raw) {
      const closer = `"${"#".repeat(raw[0].length - 2)}`;
      const hit = s.indexOf(closer, from + raw[0].length);
      return hit === -1 ? s.length : hit + closer.length;
    }
    let j = from + 1;
    while (j < s.length) {
      if (s[j] === "\\") {
        j += 2;
        continue;
      }
      if (s[j] === '"') return j + 1;
      j += 1;
    }
    return s.length;
  }
}
