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
