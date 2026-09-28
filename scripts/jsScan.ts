/**
 * JS/TS 源码扫描器：**只留代码，把注释与字符串字面量抹成等长空白**（换行一个不吃 ⇒ 行号不漂）。
 *
 * ## 为什么要有它（实测到的假红与假绿各一个）
 * `src/utils/storeContract.test.ts` 那条「界面里用到的 store 成员必须真的导出」判据，
 * 形状是"先把字符串抠掉，再找 `app.xxx`"。第一版的抠法是三段按引号配对的正则，它**分不清**：
 * ① **正则字面量里的引号** —— 一句 `/^r#*"/.test(x)` 里那个 `"` 会被当成字符串开始，
 *    于是"文件里哪一段算字符串"整体错位；错位的结果是把 `src/api/events.test.ts` 里
 *    **故意喂给扫描器的 Rust 夹具**（`'app.emit("x", &p)'`）读成真实 store 用法 ⇒ 护栏假红；
 * ② **模板串的 `${…}`** —— 那是代码不是文本，整段抹掉会让真用法（`` `进度 ${app.percent}%` ``）隐身 ⇒ 假绿；
 * ③ **注释** —— 第一版根本不抠注释，任何一句解释 `app.refreshChannels` 的注释都能造出假红。
 *
 * 同一族错误在 Rust 侧已经修过一次（`scripts/rustSrc.ts` 的 `stripRustComments`），
 * 但**两种语言的词法规则不同**（JS 有正则/除法歧义、模板插值），所以这里是另一个家，
 * 不是第二份重复实现 —— 每个语言各一份，两边都只被自己的消费方读。
 *
 * ## 已知边界（别读成"什么都能骗不过"）
 * · 正则 vs 除法用"上一个有实义的字符/关键字"启发式判 —— 这是业界通行做法，
 *   极端写法（如 `a\n/b/c`）可能判成除法；判错的后果只影响这一段被不被抹，不影响别的段。
 * · JSX/TSX 的 `<Foo attr="x">` 里属性字符串会被抹成空白（与第一版行为一致，本轮不扩权）。
 */

/** 出现在这些字符之后，`/` 是正则的开始而不是除法。 */
const REGEX_PREV = new Set(["(", ",", ":", "[", "!", "&", "|", "?", "{", "}", ";", "=", "+", "-", "*", "/", "%", "~", "^", "<", ">", ""]);
/** 这些关键字之后，`/` 同样是正则开始。 */
const REGEX_PREV_WORDS = new Set(["return", "typeof", "case", "in", "of", "do", "else", "void", "delete", "new", "await", "yield"]);

/**
 * 抹掉注释与字符串，保留代码与换行。
 * @param src 源码文本
 * @returns 与 `src` **等长**的字符串：非代码部分变成空格，换行原样保留。
 */
export function stripJsLiterals(src: string): string {
  return scan(src, 0, src.length, 0).text;
}

/**
 * 扫一段**代码**。`braceDepth` 用在模板 `${…}` 里：遇到使深度变负的 `}` 就停（那是插值的收尾）。
 * @returns `text` 结果文本；`end` 停下的下标；`stopReason` 为什么停（`"brace"` = 插值结束，`"eof"`）
 */
function scan(src: string, from: number, to: number, braceDepth: number): {
  text: string; end: number; stopReason: "brace" | "eof";
} {
  const buf: string[] = [];
  let i = from;
  let prevSig = "";       // 上一个"有实义"的非空白字符
  let prevWord = "";      // 上一个标识符/关键字
  const isIdent = (c: string) => /[\w$]/.test(c);

  while (i < to) {
    const c = src[i];
    const d = src[i + 1] ?? "";

    // ── 注释：抹成等长空白，换行留下 ──────────────────────────────
    if (c === "/" && d === "/") {
      let j = src.indexOf("\n", i);
      if (j === -1) j = to;
      j = Math.min(j, to);
      buf.push(" ".repeat(j - i));
      i = j;
      continue;
    }
    if (c === "/" && d === "*") {
      let j = i + 2;
      let depth = 1;
      while (j < to) {
        if (src[j] === "/" && src[j + 1] === "*") { depth++; j += 2; continue; }
        if (src[j] === "*" && src[j + 1] === "/") { depth--; j += 2; if (depth === 0) break; continue; }
        j++;
      }
      buf.push(src.slice(i, Math.min(j, to)).replace(/[^\n]/g, " "));
      i = Math.min(j, to);
      continue;
    }

    // ── 普通字符串：内容抹掉（转义按语法走，绝不被 \`\" 带偏） ─────
    if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < to) {
        const x = src[j];
        if (x === "\\") { j += 2; continue; }
        if (x === c) { j++; break; }
        j++;
      }
      buf.push(src.slice(i, Math.min(j, to)).replace(/[^\n]/g, " "));
      i = Math.min(j, to);
      prevSig = c;
      continue;
    }

    // ── 模板串：字面文本抹掉，但 ${…} 里**是代码**，递归扫 ─────────
    if (c === "`") {
      // 反引号本身抹成空格（保长度）；字面文本抹成空格；${…} 里的内容按代码递归扫
      buf.push(" ");
      let j = i + 1;
      let segStart = j;
      let stopped = false;
      for (;;) {
        if (j >= to) {
          buf.push(src.slice(segStart, to).replace(/[^\n]/g, " "));
          i = to; prevSig = "`"; stopped = true; break;
        }
        const x = src[j];
        if (x === "\\") { j += 2; continue; }
        if (x === "`") {
          buf.push(src.slice(segStart, j).replace(/[^\n]/g, " "));
          buf.push(" ");
          i = j + 1; prevSig = "`"; stopped = true; break;
        }
        if (x === "$" && src[j + 1] === "{") {
          buf.push(src.slice(segStart, j).replace(/[^\n]/g, " "));
          const inner = scan(src, j + 2, to, braceDepth + 1);
          buf.push(inner.text);
          prevSig = "}";
          if (inner.stopReason === "brace") { j = inner.end + 1; segStart = j; continue; }
          i = inner.end; stopped = true; break;
        }
        j++;
      }
      if (!stopped) i = j;
      continue;
    }

    // ── 正则字面量 vs 除法 ─────────────────────────────────────
    if (c === "/" && (prevSig === "" || REGEX_PREV.has(prevSig) || REGEX_PREV_WORDS.has(prevWord))) {
      let j = i + 1;
      let inClass = false;
      let closed = false;
      while (j < to) {
        const x = src[j];
        if (x === "\\") { j += 2; continue; }
        if (x === "\n") break;                 // 正则不跨行：判错了也只影响这一行
        if (inClass) { if (x === "]") inClass = false; j++; continue; }
        if (x === "[") { inClass = true; j++; continue; }
        if (x === "/") { closed = true; j++; break; }
        j++;
      }
      if (closed) {
        while (j < to && isIdent(src[j])) j++; // 带 flags（/x/gi）
        buf.push(src.slice(i, j).replace(/[^\n]/g, " "));
        i = j;
        prevSig = "/";                         // 正则之后接 `/` 是除法，保留这个语义
        prevWord = "";
        continue;
      }
      // 判不成正则 ⇒ 当普通字符（除法）处理，落到下面
    }

    buf.push(c);
    if (!/\s/.test(c)) {
      if (c === "}") {
        if (braceDepth > 0) return { text: buf.join(""), end: i, stopReason: "brace" };
      }
      prevSig = c;
      if (isIdent(c)) prevWord = (prevWord + c).slice(-12);
      else prevWord = "";
    }
    i++;
  }
  return { text: buf.join(""), end: to, stopReason: "eof" };
}
