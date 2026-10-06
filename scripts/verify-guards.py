#!/usr/bin/env python3
"""非空转验证（guard non-vacuity harness）。

## 为什么要有这个脚本
本项目有一条铁律：**每加一条守门测试，都必须证明它真的会失败**（"临时改坏 → 判据必须 FAIL →
恢复 → 才提交"）。此前这一步靠人手做，于是有三类真实风险：

1. 写了护栏但**从没验证过它会失败**（等于没有护栏）；
2. 验证时改坏的地方**改回去了没有、有没有残留**，全靠自觉；
3. 换人/换轮次后，没人知道哪条护栏被证明过。

本脚本把这件事变成一条命令：对每条护栏施加一处**最小、真实**的破坏，
断言对应测试**必须失败**；然后恢复源码，断言测试**必须通过**；
最后逐条打印结果，并保证无论如何都恢复现场（`try/finally` + 内容级恢复）。

## 用法
    python3 scripts/verify-guards.py                # 全部用例（**约 10 分钟以上**：Rust 用例要重编译）
    python3 scripts/verify-guards.py --only rust    # 只跑 Rust 用例（慢，但覆盖最关键的几条）
    python3 scripts/verify-guards.py --only frontend  # 只跑前端用例（十几秒）
    python3 scripts/verify-guards.py --only ble
    python3 scripts/verify-guards.py --list         # 只列出用例

## 被中断也安全
`SIGTERM`/`SIGINT`（Ctrl-C）会**先把当前注入的文件恢复**再退出，
`atexit` 再兜一层 —— 否则一次误杀会留下一个"被改坏"的工作区（这个坑真踩过）。

退出码：0 = 全部符合预期；1 = 有护栏"改坏了也不报"或"恢复后仍失败"。
"""

from __future__ import annotations

import argparse
import atexit
import os
import re
import shutil
import signal
import subprocess
import sys
from pathlib import Path



def read_source(path: Path) -> str:
    """读源码文本 —— **逐字节保真**（`newline=""` + 显式 UTF-8）。

    为什么不能直接用 `read_text()`：
    · 默认的通用换行会把 CRLF 收成 LF，写回去时又按 `os.linesep` 变成 CRLF；
    · 默认编码是**系统 locale**（中文 Windows 上是 GBK）。

    于是一轮护栏跑完就能把工作区的 LF 文件改成 CRLF（仓库明确要求 LF，见 `.gitattributes`），
    中文注释还可能被按错误编码往返一次。2026-09-16 在 Windows 上实测到了这个副作用。

    ⚠️ 为什么要走 `path.open()` 而不是 `path.read_text(newline=…)`：`newline` 是
    **Python 3.13** 才加进 `Path.read_text/write_text` 的参数，在 3.12 上直接
    `TypeError: read_text() got an unexpected keyword argument 'newline'` ——
    整份 verify-guards 会因此**一条都跑不了**（表现为 40 条全报"验证过程出错"；
    2026-09-17 在 Windows + Python 3.12.10 上实测）。`Path.open()` 自 3.0 起就接受
    `newline`，与原来的语义完全一致。
    """
    with path.open("r", encoding="utf-8", newline="") as f:
        return f.read()


def _list_includes(root_file: Path) -> list[Path]:
    """递归解析根文件里的 `include!("...");` 指令，返回所有被包含的子模块路径。

    物理拆分后 commands.rs / db.rs 只剩 include! 指令，但锚点可能藏在任何一层
    子模块里（子模块里还可以 include 别的子模块 — Rust 的 include! 是递归的）。
    本函数模拟 Rust 的解析行为，帮助定位锚点真正在的文件。
    """
    result: list[Path] = []
    root_dir = root_file.parent

    def _walk(p: Path) -> None:
        src = read_source(p)
        for m in re.finditer(r'include!\("([^"]+)"\);', src):
            child = (p.parent / m.group(1)).resolve()
            if child not in result:
                result.append(child)
                _walk(child)

    _walk(root_file)
    return result


def _resolve_anchor_file(root_file: Path, anchor: str) -> Path:
    """在根文件及其 include! 子模块树里，找到真正包含 anchor 的那个文件。

    返回的是**应该被写回**的文件（不是根文件）。如果根文件本身有锚点就返回根文件；
    否则遍历 include! 子模块。要求 anchor 在恰好一个文件里出现恰好一次。
    """
    # 先看根文件自己
    root_text = read_source(root_file)
    if anchor in root_text:
        return root_file

    # 再搜所有 include! 子模块
    candidates = _list_includes(root_file)
    hits: list[tuple[Path, int]] = []
    for child in candidates:
        try:
            text = read_source(child)
        except OSError:
            continue
        count = text.count(anchor)
        if count > 0:
            hits.append((child, count))

    if not hits:
        raise AssertionError(
            f"注入锚点在 {root_file.name} 及其 {len(candidates)} 个 include! 子模块里"
            f"**都找不到**：{anchor[:80]!r}"
        )
    if len(hits) > 1:
        detail = ", ".join(f"{p.relative_to(root_file.parent.parent)}({c})" for p, c in hits)
        raise AssertionError(
            f"注入锚点在多个文件里都出现了：{detail} —— 请明确指定 file="
            f"或加更长的锚点"
        )
    return hits[0][0]


def write_source(path: Path, text: str) -> None:
    """写回源码文本（与 [`read_source`] 对称：不翻译行尾，理由同上）。"""
    with path.open("w", encoding="utf-8", newline="") as f:
        f.write(text)


#: 当前正在被注入的文件与它的原始内容 —— 被 Ctrl-C / kill 打断时也要能恢复。
#: 有的护栏要同时改坏**两个**文件（例如"事实来源 + 注入副本"必须一致），所以这里是列表。
_CURRENT: list[tuple[Path, str]] = []


def restore_now() -> None:
    """把"当前注入现场"恢复回原始内容（幂等）。"""
    global _CURRENT
    for path, original in _CURRENT:
        write_source(path, original)
    _CURRENT = []


def _on_signal(signum: int, _frame: object) -> None:
    """被中断时先把源码恢复再退出 —— 否则会留下一个"被改坏"的工作区。"""
    paths = "、".join(str(p) for p, _ in _CURRENT)
    restore_now()
    where = f"（已恢复 {paths}）" if paths else ""
    print(f"\n[中断] 收到信号 {signum}{where}，退出", file=sys.stderr)
    sys.exit(130)


signal.signal(signal.SIGTERM, _on_signal)
signal.signal(signal.SIGINT, _on_signal)
atexit.register(restore_now)


import sys as _sys

# 把自己所在目录放进 `sys.path`：作为脚本跑时 Python 已经这么做了，但**被别人 import** 时没有。
# `docs/final-architecture-review.md` 里就有一条用 `spec_from_file_location` 数 `CASES` 的复跑命令，
# 那种加载方式下 `from guard_cases import ...` 会 ModuleNotFoundError ⇒ 指路句变成假指路（实测过）。
_sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# 2026-10-07：202 条 `Case` 按域切进 `scripts/guard_cases/`（本文件原来 4,176 行，光那一份
# `CASES` 字面量就占 3,747 行）。`MODULES` 与目录有一起跑前的对账 ⇒ 新分册忘了点名会当场炸，
# 不会静默少跑；判据 E 改按 `guard_cases/*.py` 现算条数（见 `scripts/check-doc-numbers.mjs`）。
# `ROOT`/`TAURI`/`Case`/`cargo()`/`npm()` 的唯一份定义也在 `guard_cases/base.py`（原来这两行
# 与本文件里的 `@dataclass` 就是它的原文 ⇒ 这里删掉本地定义，不留第二个家）。
from guard_cases import CASES
from guard_cases.base import ROOT, Case




def _resolve_program(name: str) -> str:
    """把 `npm` / `cargo` 解析成**真正可执行的文件**。

    为什么需要（2026-09-13，Windows 首次跑本脚本）：Windows 上 `npm` 实际是 `npm.cmd`
    批处理，而 `subprocess.run(["npm", ...])` **不走 shell**、CreateProcess 也不会补
    `.cmd`/`.bat` 后缀 ⇒ 直接 `[WinError 2] 系统找不到指定的文件`。结果是本脚本里所有
    前端用例（占一多半）在 Windows 上全部"验证过程出错"，看起来像护栏坏了，其实是
    调用方式不对。这里显式解析出全路径，既修好 Windows，又保持 `shell=False`
    （不引入 shell 引号/注入面的变化）。
    """
    if os.name != "nt":
        return name
    found = shutil.which(name)
    if found:
        return found
    for ext in (".cmd", ".bat", ".exe"):
        found = shutil.which(name + ext)
        if found:
            return found
    return name  # 交给 subprocess 报它自己的错（错误信息更明确）


def run(cmd: list[str], cwd: Path, timeout: int = 900, extra_env: dict[str, str] | None = None) -> tuple[int, str]:
    env = dict(os.environ)
    if extra_env:
        env.update(extra_env)
    # 沙箱/CI 里写不了 ~/.cargo：仓库内已有一份 CARGO_HOME 时优先用它
    local_cargo = ROOT / "target" / "cargo-home"
    if "CARGO_HOME" not in env and local_cargo.is_dir():
        env["CARGO_HOME"] = str(local_cargo)
    # 顺带把 UTF-8 固定下来：Windows 默认 cp1252 会把测试输出里的中文变成
    # UnicodeDecodeError（本脚本在 Windows 上第一次跑就是这么挂的）。
    env.setdefault("PYTHONUTF8", "1")
    resolved = [_resolve_program(cmd[0]), *cmd[1:]]
    proc = subprocess.run(
        resolved,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )
    return proc.returncode, proc.stdout + proc.stderr


def verify(case: Case) -> tuple[bool, str]:
    global _CURRENT
    #: [(文件, [(原文, 替换), …])] —— 主文件 + 需要"同时改坏"的其它文件
    targets: list[tuple[Path, list[tuple[str, str]]]] = [(case.file, case.injections)]
    targets += [(p, [(old, new)]) for (p, old, new) in case.extra_injections]

    # --- 注入前预处理：把每条 injection 解析到真正包含锚点的文件 ---
    # 物理拆分后 commands.rs / db.rs 只剩 include! 指令，锚点可能在任何子模块里。
    # 这里做一次"锚点→目标文件"的解析，后续所有操作都在正确的子模块上进行。
    resolved_targets: list[tuple[Path, list[tuple[str, str]]]] = []
    for root_path, injections in targets:
        if not injections:
            resolved_targets.append((root_path, injections))
            continue
        # 如果所有锚点都在同一个文件（根文件自己或某个子模块），保持原行为
        all_same: Path | None = None
        per_file: dict[Path, list[tuple[str, str]]] = {}
        for old, new in injections:
            try:
                target = _resolve_anchor_file(root_path, old)
            except AssertionError:
                # 兼容：如果不是 commands.rs/db.rs 这类 include! 根，就当作普通文件
                target = root_path
            if target not in per_file:
                per_file[target] = []
            per_file[target].append((old, new))
        resolved_targets.extend(per_file.items())
    targets = resolved_targets

    originals = [(path, read_source(path)) for path, _ in targets]
    detail = ""
    try:
        for path, injections in targets:
            text = read_source(path)
            for old, new in injections:
                assert text.count(old) == 1, (
                    f"注入锚点在 {path.name} 里出现 {text.count(old)} 次（要求恰好 1 次）："
                    f"源码可能已改动，请更新 verify-guards.py"
                )
                # ⚠️ 必须在**上一次替换的结果**上继续改（逐条累积），否则一条用例里写多个
                # 注入时只有最后一条生效 —— 这条旧实现的坑在这里一并修掉。
                text = text.replace(old, new, 1)
            write_source(path, text)
        _CURRENT = list(originals)  # 登记现场：被信号打断时可恢复

        code, out = run(case.cmd, case.cwd, extra_env=case.env)
        if code == 0:
            return False, "改坏之后测试**仍然通过** ⇒ 这条护栏是空转的（没在守东西）"
        if case.expect_fail_hint and case.expect_fail_hint not in out:
            detail = f"（失败输出里没看到 `{case.expect_fail_hint}`，请确认是这条判据报的）"

        for path, original in originals:  # 先恢复，再验证恢复后确实通过
            write_source(path, original)
        _CURRENT = []
        code2, out2 = run(case.cmd, case.cwd, extra_env=case.env)
        if code2 != 0:
            return False, f"恢复源码之后测试**仍然失败** ⇒ 源码或环境已被破坏：\n{out2[-800:]}"
        return True, detail or "改坏即 FAIL、恢复即 PASS"
    finally:
        # 无论上面发生什么（断言失败/超时/异常），内容级恢复现场
        for path, original in originals:
            if read_source(path) != original:
                write_source(path, original)
        _CURRENT = []


def main() -> int:
    ap = argparse.ArgumentParser(description="非空转验证：故意改坏每条护栏，确认测试真的会失败")
    ap.add_argument("--only", default="", help="只跑名字/标签里包含该子串的用例")
    ap.add_argument("--list", action="store_true", help="只列出用例")
    ap.add_argument(
        "--strict-platform",
        action="store_true",
        help="把「平台不适用而跳过」也算失败（默认跳过只提示，见 Case.platforms）",
    )
    args = ap.parse_args()

    cases = [c for c in CASES if not args.only or args.only in c.name or args.only in c.tags]
    if not cases:
        # ⚠️ 别把"空跑"当"通过"：`--only` 写错一个字符就会一条都不跑，
        # 而下面的汇总照样打印 ✅（真实踩过：`--only CoreBluetooth状态回执` 少了空格）。
        print(
            f"❌ `--only {args.only}` 没有匹配到任何用例（可用 `--list` 看名字/标签）",
            file=sys.stderr,
        )
        return 1

    # ---------------- 起跑前：锚点全量静态核对（2026-09-27，roadmap #107）----------------
    # 为什么要有这一段：锚点死了 runner 也会**抛 AssertionError**（不是假绿），但那是在跑到
    # 那一条时才抛 —— 整跑要一个多小时，于是"改坏了锚点"这件事平均要白等半小时才发现
    # （实测：189 条整跑只报 1 条异常，就是那条 `中继收文件的哈希`，而静态核对 1 秒就给同一结论）。
    # 这一段只做**存在性/唯一性**核对，一条测试都不跑；非空转仍然要靠后面的逐条真注入。
    dead_anchors: list[str] = []
    for c in cases:
        pairs: list[tuple[Path, str]] = [(c.file, old) for old, _new in c.injections]
        pairs += [(p, old) for (p, old, _new) in c.extra_injections]
        for path, old in pairs:
            if not old:
                continue  # 纯新增型注入：没有"锚点存在性"可核
            try:
                _resolve_anchor_file(path, old)
            except AssertionError as e:
                dead_anchors.append(f"{c.name}\n      文件 {Path(path).name}: {e}")
    if dead_anchors:
        print(
            f"❌ 起跑前核对：{len(dead_anchors)} 处注入锚点已失效（一条测试都没跑，省下整轮时间）：",
            file=sys.stderr,
        )
        for d in dead_anchors:
            print(f"   - {d}", file=sys.stderr)
        print(
            "   ⇒ 守卫变强时不许凭猜写新锚点：先确认新形状**可编译**且真能改掉被钉的那个语义。",
            file=sys.stderr,
        )
        return 1
    print(f"· 起跑前核对：{len(cases)} 条用例的注入锚点都在各自文件里恰好命中一次\n")
    if args.list:
        for c in cases:
            print(f"  [{','.join(c.tags)}] {c.name}\n      {c.why}")
        return 0

    # 平台判定：Darwin=macOS、Linux=Android 目标宿主、Windows=Windows
    this_platform = "darwin" if sys.platform == "darwin" else ("win32" if os.name == "nt" else "linux")

    print(f"非空转验证：{len(cases)} 条护栏（每条都要求「改坏 → FAIL → 恢复 → PASS」）")
    print(f"当前平台：{this_platform}\n")
    bad: list[str] = []
    skipped: list[tuple[str, str]] = []
    for i, case in enumerate(cases, 1):
        if case.platforms is not None and this_platform not in case.platforms:
            # 目标代码在这个平台上根本不编译 ⇒ "改坏也不会失败"是**必然**的，不是缺陷。
            # 必须显式说出来：否则一堆假失败会把真失败淹掉（这是本次 Windows 首跑的教训）。
            print(f"[{i}/{len(cases)}] {case.name}")
            print(f"      ⏭️  跳过：本用例只适用于 {'/'.join(case.platforms)}，当前是 {this_platform}")
            print()
            skipped.append((case.name, "/".join(case.platforms)))
            continue
        print(f"[{i}/{len(cases)}] {case.name}")
        print(f"      {case.why}")
        try:
            ok, detail = verify(case)
        except Exception as exc:  # 锚点过期、超时、断言等 —— 报告出来，别让整个脚本崩
            ok, detail = False, f"验证过程出错：{exc}"
        print(f"      {'✅' if ok else '❌'} {detail}")
        if not ok:
            bad.append(case.name)
        print()

    if skipped:
        print(f"⏭️  {len(skipped)} 条因平台不适用而跳过（目标代码在本平台不编译）：")
        for name, plats in skipped:
            print(f"   - {name}（只适用于 {plats}）")
        print()
    if bad:
        print(f"❌ {len(bad)} 条不符合预期：")
        for name in bad:
            print(f"   - {name}")
        if skipped and args.strict_platform:
            print("（--strict-platform：上述跳过也算失败）")
            return 1
        return 1
    if skipped and args.strict_platform:
        print(f"❌ --strict-platform：{len(skipped)} 条被跳过，不算全绿")
        return 1
    print(f"✅ 其余 {len(cases) - len(skipped)} 条护栏都通过了非空转验证（改坏即 FAIL、恢复即 PASS）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
