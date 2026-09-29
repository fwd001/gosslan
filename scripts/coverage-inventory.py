#!/usr/bin/env python3
# 量具，不是门禁：它只回答"这个 Rust 函数名有没有在任何测试语料里被点过名"。
# ★ 已知偏乐观（三个来源）：① 提过名字 ≠ 判过行为；② snake→camel 双向找会把同名前端包装算进来；
#   ③ 语料含生产文件里的 #[cfg(test)] 内联块。⇒ "从未点名"那份是**下界**，只能当下笔前该去查的地方。
# 复跑：python3 scripts/coverage-inventory.py
import re, pathlib, sys

root = pathlib.Path("src-tauri/src")
prods, corpus = [], []
for p in sorted(root.rglob("*.rs")):
    t = p.read_text(encoding="utf8", errors="replace")
    if re.search(r"(tests\.rs$|_tests\.rs$)", p.name):
        corpus.append(t)
        continue
    prods.append((p, t))
    m = re.search(r"#\[cfg\(test\)\][\s\S]*", t)
    if m:
        corpus.append(m.group(0))
for extra in [pathlib.Path("scripts/e2e-multi-instance.mjs")]:
    if extra.exists():
        corpus.append(extra.read_text(encoding="utf8"))
for p in pathlib.Path("src").rglob("*.test.ts"):
    corpus.append(p.read_text(encoding="utf8"))
blob = "\n".join(corpus)
camel = lambda s: re.sub(r"_([a-z0-9])", lambda m: m.group(1).upper(), s)
defs = [(m.group(1), str(p)) for p, t in prods
        for m in re.finditer(r"^pub(?:\(crate\))? fn ([a-z0-9_]+)", t, re.M)]
unc = [(n, p) for n, p in defs
       if not (re.search(r"\b%s\b" % n, blob) or re.search(r"\b%s\b" % camel(n), blob))]
print(f"分母 = src-tauri/src 生产 .rs 里的 pub / pub(crate) fn：{len(defs)}")
print(f"被点名 {len(defs) - len(unc)} ｜ 从未点名 {len(unc)}（下界，见文件头三条偏乐观来源）")
# 反空转闸：真存在的函数里必须至少有一个被判为"未点名"、一个被判为"已点名"，否则这份语料是坏的
sample_named = next((n for n, _ in defs if n not in {x for x, _ in unc}), None)
sample_unc = unc[0][0] if unc else None
print(f"对照：已点名样本 {sample_named}｜未点名样本 {sample_unc}")
if not unc or not sample_named:
    print("RULER-BROKEN: 某一侧为空，别信这份数", file=sys.stderr)
    sys.exit(2)
from collections import Counter
print("未点名按目录聚合：" + "，".join(f"{k} {v}" for k, v in Counter(re.sub(r'^src-tauri/src/', '', p).split('/')[0] for _, p in unc).most_common()))
print("未点名单：" + " ".join(f"{n}@{re.sub(r'^src-tauri/src/', '', p)}" for n, p in sorted(unc)))
