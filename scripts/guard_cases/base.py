#!/usr/bin/env python3
"""`Case` 数据类与它要用的路径/命令 helper —— 各分册与本包共用的唯一一份。

为什么必须单独一个文件：分册要构造 `file=TAURI / "src" / ...` 与 `cmd=cargo(...)`，
若从 `verify-guards.py` 反向 import 就成环（主文件要读分册拿 CASES）。
⚠️ `ROOT` 现在是 `parents[2]`（本文件在 `scripts/guard_cases/` 下），并有一条**当场断言**兜底：
   深度数错的话 `src-tauri/` 就不存在 ⇒ 这里直接抛，而不是让 202 条用例去找不着的文件。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TAURI = ROOT / "src-tauri"
assert (TAURI / "Cargo.toml").is_file(), f"`ROOT` 指错了：{ROOT} 下没有 src-tauri/Cargo.toml"

@dataclass
class Case:
    """一条护栏的非空转验证用例。"""

    name: str
    why: str
    file: Path
    #: (原文, 替换) —— 必须是"真实回归形态"的最小破坏，且原文在文件里出现恰好一次
    injections: list[tuple[str, str]]
    cmd: list[str]
    cwd: Path
    expect_fail_hint: str = ""  # 期望在失败输出里出现的关键词。**必填**：runner 起跑前核对会拒空值
    # （2026-10-07 起它是判据而不是提示 —— 不声明就等于这条用例绕过该判据）
    tags: list[str] = field(default_factory=list)
    #: 需要**同时**改坏的其它文件（路径, 原文, 替换）。例如"事实来源 + 构建时注入的副本"
    #: 两边都要改，否则护栏会先以"两者漂移"失败，证明不了"漏掉方法也会被抓到"。
    extra_injections: list[tuple[Path, str, str]] = field(default_factory=list)
    #: 这条用例只在哪些平台上**有意义**（`None` = 所有平台）。
    #:
    #: 为什么需要（2026-09-13，Windows 首次跑本脚本）：有些护栏盯的是**只在某个平台编译**
    #: 的代码 —— 例如 `transport/bluetooth_peripheral.rs` 整个文件是
    #: `#![cfg(all(feature = "bluetooth", target_os = "macos"))]`，`reconnect_hello_...`
    #: 单测也被 `cfg(any(macos, android))` 门控。在 Windows 上：
    #:   · 注入照样能改到源码文本（锚点命中），
    #:   · 但 `cargo test --features bluetooth <那两条>` **一个测试都不会跑**，退出码 0
    #:   ⇒ 本函数把它判成"改坏之后测试仍然通过 ⇒ 护栏空转"，**这是误报**：
    #:   护栏在 macOS 上是好的，只是这个平台上根本没有那段代码可改。
    #: 正确做法是**显式跳过并说清楚**，而不是留一堆假失败把真失败淹掉
    #: （`--strict-platform` 可把跳过重新当成失败，用于"必须全平台都能守"的场景）。
    platforms: tuple[str, ...] | None = None
    #: 这条用例跑命令时**额外覆盖**的环境变量（在 os.environ 之上合并）。
    #:
    #: 为什么需要（2026-09-28 实测）：有些判据只在 **CI 的环境**里才成立 ——
    #: `check-change-budget.mjs` 那段"拿不到 before..sha 就判空转"的硬失败由
    #: `GOSSLAN_BUDGET_STRICT` + `GITHUB_EVENT_NAME` 门控，本地永远没有这两个变量，
    #: 于是它把护栏用例的"恢复后即 PASS"半边在 CI 上判成 exit 1，**藏了 7 天**
    #: （Change Budget 在护栏之前一步，它一红就 fail-fast，护栏那步从没跑到过）。
    #: 没有这个入口，"只在 CI 红"的那一类缺陷在本案里是**无法被非空转验证表达**的。
    env: dict[str, str] | None = None


def cargo(*args: str) -> list[str]:
    return ["cargo", *args]


def npm(*args: str) -> list[str]:
    return ["npm", *args]
