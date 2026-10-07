/**
 * 本地化字典（简体中文 / 英文）的**门面**。
 *
 * 骨架说明：本项目文案原本硬编码中文。这里建立字典 + t() 机制后，逐步把
 * 系统 UI 与设置页文案迁入。key 用层级命名（settings.* / nav.* / common.*），
 * 插值用 `{name}` 占位。
 *
 * 2026-10-07：两册按语言各自成文件（`locales/zh-cn.ts` / `locales/en-us.ts`），
 * 本文件只留同名再导出 ⇒ 四个消费者（`i18n/index.ts`、`i18n/index.test.ts`、
 * `utils/taskRowHierarchy.test.ts`）的 import 说明符**一字未改**。
 * 新增一门语言要同时做两件事：加 `locales/<lang>.ts` 和把它接进 `index.ts` 的 `MESSAGES`
 * —— 少一件不会静默：`MessageDict` 的形状由 `index.ts` 的 `Record<Locale, MessageDict>` 要求。
 */
export type { MessageDict } from "./locales/dict.ts";
export { zhCN } from "./locales/zh-cn.ts";
export { enUS } from "./locales/en-us.ts";
