/**
 * 字典值的形状：`"a.b.c" -> 文案`。
 *
 * 为什么单独一份：两册语言文件与 `locales.ts` 那道门面都要用它，
 * 而它放在门面里会让「门面 re-export 自己」变成类型层面的循环。
 */
export type MessageDict = Record<string, string>;
