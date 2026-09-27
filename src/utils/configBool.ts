/**
 * 配置布尔值的唯一口径（前端这一半，#125）。
 * Rust 侧同名规则在 `src-tauri/src/db/settings.rs` 的 `parse_config_bool` ——
 * 两边必须认同一张字面量表（用例钉在 `configBool.test.ts`）。
 *
 * 收口的原因：历史上各读各的（`=== "1"` / `!== "0"`），于是同一个 `"false"`
 * 在一个键上是关、在另一个键上是开。手工写库/测试预置都会踩。
 */
const TRUE_WORDS = new Set(["1", "true", "on", "yes"]);
const FALSE_WORDS = new Set(["0", "false", "off", "no"]);

/** 合法 ⇒ true/false；未知 ⇒ `null`（明确的非法，由调用方决定回落哪个默认，不许静默当成开或关）。 */
export function parseConfigBool(raw: string | null | undefined): boolean | null {
  if (raw === null || raw === undefined) return null;
  const norm = raw.trim().toLowerCase();
  if (TRUE_WORDS.has(norm)) return true;
  if (FALSE_WORDS.has(norm)) return false;
  return null;
}
