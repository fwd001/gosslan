/**
 * 通讯录分组用：按「首字母」把名字归组（用户需求 2026-09-12 第 17 条）。
 *
 * 用户原话：「通讯录的话，现在跟那个消息列表的样式太接近了……你可以以他们的这个字母名字母的
 * 首字母，和用一套规则首字母或用户的拼音的首字母去排列顺序然后去分组」。
 *
 * ## 首字母从哪来（关键决策）
 * **不用** `Intl.Collator("zh-Hans-u-co-pinyin")`：实测 Node/ICU 会忽略 pinyin 扩展
 * （`resolvedOptions().collation === "default"`），此时所有汉字都排在拉丁字母之前，
 * 推不出首字母；而各平台 WebView 的 ICU 版本又不一致 —— 同一个名字会在不同平台分到
 * 不同组，正是用户最在意的「信息要准确」。改用**内置生成表**
 * `src/data/hanInitials.ts`（CJK 基本区逐码点 → 首字母，取名时用姓读音）：
 * 确定、跨端一致、可被单测钉死。
 *
 * ⚠️ 本文件必须保持**零 `@/` 依赖**：`npm test`（node:test + strip-types）解析不了别名，
 * 被测 util 一旦引别名就测不了。所以数据表用**相对路径 + 显式扩展名**导入。
 */

import { HAN_INITIALS, HAN_START } from "../data/hanInitials.ts";

/** 分组用的字母表（顺序即显示顺序）。 */
export const GROUP_LETTERS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("");

/** 非拉丁字母开头（数字/符号/表外汉字）归到这一组。 */
export const OTHER_GROUP = "#";

/**
 * 取名字的分组字母。
 *
 * - 拉丁字母开头 → 该字母大写（`alice` → `A`）；
 * - 汉字开头 → 查内置首字母表（`张三` → `Z`；姓氏多音字取**姓读音**：`单` → `S`、`曾` → `Z`）；
 * - 表外汉字 / 数字 / 符号 / 空名字 → `#`。
 */
export function initialOf(name: string): string {
  const first = Array.from((name ?? "").trim())[0] ?? "";
  if (!first) return OTHER_GROUP;
  if (/[A-Za-z]/.test(first)) return first.toUpperCase();
  const cp = first.codePointAt(0) ?? 0;
  if (cp >= HAN_START && cp < HAN_START + HAN_INITIALS.length) {
    const letter = HAN_INITIALS[cp - HAN_START];
    // 表内 `#` = 该字无拼音；与「表外汉字」同样归 `#` 组。
    if (letter >= "A" && letter <= "Z") return letter;
  }
  return OTHER_GROUP;
}

export interface NameGroup<T> {
  /** 分组字母（A–Z 或 `#`）。 */
  letter: string;
  /** 组内成员（已排序）。 */
  items: T[];
}

/**
 * 组内排序器。**只用于排序，不用于推导首字母**（见文件头）。
 * 构造失败（极老环境）时退回码位序 —— 组内顺序退化，但绝不抛错、绝不渲染不出来。
 */
function makeSorter(): (a: string, b: string) => number {
  try {
    const c = new Intl.Collator("zh-Hans-u-co-pinyin", { sensitivity: "base" });
    return (a, b) => c.compare(a, b);
  } catch {
    return (a, b) => (a < b ? -1 : a > b ? 1 : 0);
  }
}

/**
 * 按首字母分组并排序。
 *
 * - 组内用 collator 排序（同规则、中英混排稳定）；
 * - 组间 A–Z，`#` 放**最后**（符号/数字垫底，不抢视线）；
 * - 空组不产出（不会出现「字母标题下什么都没有」）。
 */
export function groupByInitial<T>(items: T[], nameOf: (item: T) => string): NameGroup<T>[] {
  const sorter = makeSorter();
  const buckets = new Map<string, T[]>();
  for (const item of items) {
    const letter = initialOf(nameOf(item));
    const arr = buckets.get(letter);
    if (arr) arr.push(item);
    else buckets.set(letter, [item]);
  }
  for (const arr of buckets.values()) {
    arr.sort((a, b) => sorter(nameOf(a), nameOf(b)));
  }
  return [...buckets.keys()]
    .sort((a, b) => {
      if (a === OTHER_GROUP) return 1;
      if (b === OTHER_GROUP) return -1;
      return a < b ? -1 : a > b ? 1 : 0;
    })
    .map((letter) => ({ letter, items: buckets.get(letter) ?? [] }));
}
