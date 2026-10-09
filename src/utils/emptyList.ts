import type { ReactionChip } from "@/utils/reactions";

/**
 * 列表型 prop 的「空」必须是**同一个对象**，不是每次渲染现造一个。
 *
 * 为什么要专门立这一个文件：Vue 对组件 props 做浅比较，`[] !== []`，
 * 所以「这条 prop 每次都是新的空数组」与「这条 prop 的内容真的变了」在渲染层完全同值 ——
 * 后者必须重新 patch，前者是白烧一帧。
 * 2026-10-10 现量（`perf/README.md` 量具一的 `?row=` 三档，n=3000、视口 22 行、headless Brave）：
 * 合成行 16.7 ms/帧、真 `MessageItem` 行 27.6 ms/帧、把这两条数组换成下面的常量后 16.6 ms/帧
 * ⇒ 那 10.9 ms 全部来自「空数组每次都是新对象」，不是组件本身的渲染成本。
 *
 * 冻结是故意的：这些常量的消费方全是只读（`MessageReactionBar` 用 `find` / `v-for`，
 * `MessageReceipt` 用 `slice` / `length`）。一旦有人原地 `push`，同一份数组会同时挂在所有消息上，
 * 那是最难查的一类串味 —— 所以宁可当场抛，也不要静默共享一份被改脏的数据。
 * 类型上仍写成可变数组是因为 prop 契约就是 `string[]` / `ReactionChip[]`；
 * 把它改成 `readonly` 要牵动整条声明链，那不属于这一格。
 * 分两步写（声明 + 冻结）而不是 `Object.freeze([]) as string[]`：后者会被 tsc 判成
 * 「readonly 转 mutable 一定是写错了」（TS2352），而绕过它正是这里想留的那句「宁可抛」。
 */
export const EMPTY_STRING_LIST: string[] = [];
Object.freeze(EMPTY_STRING_LIST);

/** 见 `EMPTY_STRING_LIST` 的注释：同一件事，回应 chip 那一支。 */
export const EMPTY_REACTION_CHIPS: ReactionChip[] = [];
Object.freeze(EMPTY_REACTION_CHIPS);
