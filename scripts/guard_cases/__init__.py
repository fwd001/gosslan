#!/usr/bin/env python3
"""护栏非空转用例的包：把各域分册拼成 `verify-guards.py` 用的那一份 `CASES`。

⚠️ `MODULES` 必须**恰好等于本目录里的分册**（下面那条对账负责保证）：
   只加文件不点名 ⇒ 那个域的用例一条都不会跑，而没有任何判据会报错 —— 这是假绿，不是假红。
   顺序就是运行顺序（`[n/m]` 进度按它数）；用例**内容**与顺序无关，故顺序改动不影响判据。
"""

from __future__ import annotations

import importlib
from pathlib import Path

from .base import ROOT, TAURI, Case, cargo, npm

# 运行顺序：慢的（Rust 重编译那批）在前，快的小批在后 —— 与拆前 `CASES` 的第一条同域，
# 免得有人只跑一半就去提交（拆前 202 条就是这个次序，分册只是把它按域分段摆了）。
MODULES = [
    "ble_android",
    "transport_network",
    "file_transfer",
    "desktop_misc",
    "frontend_ui",
    "frontend_state",
    "toolchain",
]

_here = Path(__file__).resolve().parent
_present = sorted(
    p.stem for p in _here.glob("*.py") if p.stem not in ("base", "__init__")
)
assert sorted(MODULES) == _present, (
    f"guard_cases 目录里有 {len(_present)} 个分册，`MODULES` 只点名 {len(MODULES)} 个 —— "
    f"差集 {set(_present) ^ set(MODULES)}；漏点名的那一册的护栏会**一条都不跑**（假绿）"
)

CASES: list[Case] = []
for _m in MODULES:
    CASES += importlib.import_module(f".{_m}", __name__).CASES

assert CASES, "拼出来是空的 —— 分册加载失败，不许当成『没有护栏要验』"
assert len({c.name for c in CASES}) == len(CASES), "用例名重复：`--only` 与报告都会指错那一条"
