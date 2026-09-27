/**
 * 配置布尔值的**前端那一半**口径（#125）。
 *
 * 与 Rust 侧 `parse_config_bool` 是同一条规则的两份实现（跨语言没法共用一份代码），
 * 所以这里的用例把**同一张字面量表**钉住 —— 两边对同一个字符串给出不同答案，
 * 就是"设置页显示开、后端按关跑"那类不自洽的起点。
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { parseConfigBool } from "./configBool.ts";
import { readStoredAppearance, LEGACY_DARK_STORAGE_KEY } from "./appearance.ts";

test("开的一族写法都算真（与 Rust parse_config_bool 同表）", () => {
  for (const raw of ["1", "true", "TRUE", " On ", "yes"]) {
    assert.equal(parseConfigBool(raw), true, `把 ${JSON.stringify(raw)} 判成了非真`);
  }
});

test("关的一族写法都算假", () => {
  for (const raw of ["0", "false", "OFF", "no", " No "]) {
    assert.equal(parseConfigBool(raw), false, `把 ${JSON.stringify(raw)} 判成了非假`);
  }
});

test("未知写法是明确的非法（null），不许被「不等于 0 就算开」那种读法吞掉", () => {
  for (const raw of ["", "2", "-1", "maybe", "tru", "开启"]) {
    assert.equal(parseConfigBool(raw), null, `${JSON.stringify(raw)} 竟被判成合法布尔值`);
  }
});

test("老数据 gosslan.dark 的两种写法仍然各归各位（迁移不许被口径统一打断）", () => {
  const mk = (v: string | null) => ({
    getItem: (k: string) => (k === LEGACY_DARK_STORAGE_KEY ? v : null),
  });
  assert.equal(readStoredAppearance(mk("1")), "dark");
  assert.equal(readStoredAppearance(mk("0")), "light");
  assert.equal(readStoredAppearance(mk("true")), "dark"); // 新口径也认（旧写法只认 "1" ⇒ 会被静默当"没设置过"）
  assert.equal(readStoredAppearance(mk("yes?")), "system"); // 非法 ⇒ 按未设置
});
