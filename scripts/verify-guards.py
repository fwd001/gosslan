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
import shutil
import signal
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TAURI = ROOT / "src-tauri"

#: 当前正在被注入的文件与它的原始内容 —— 被 Ctrl-C / kill 打断时也要能恢复。
_CURRENT: tuple[Path, str] | None = None


def restore_now() -> None:
    """把"当前注入现场"恢复回原始内容（幂等）。"""
    global _CURRENT
    if _CURRENT is not None:
        path, original = _CURRENT
        path.write_text(original)
        _CURRENT = None


def _on_signal(signum: int, _frame: object) -> None:
    """被中断时先把源码恢复再退出 —— 否则会留下一个"被改坏"的工作区。"""
    path = _CURRENT[0] if _CURRENT else None
    restore_now()
    where = f"（已恢复 {path}）" if path else ""
    print(f"\n[中断] 收到信号 {signum}{where}，退出", file=sys.stderr)
    sys.exit(130)


signal.signal(signal.SIGTERM, _on_signal)
signal.signal(signal.SIGINT, _on_signal)
atexit.register(restore_now)


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
    expect_fail_hint: str = ""  # 期望在失败输出里出现的关键词（可空）
    tags: list[str] = field(default_factory=list)


def cargo(*args: str) -> list[str]:
    return ["cargo", *args]


def npm(*args: str) -> list[str]:
    return ["npm", *args]


CASES: list[Case] = [
    # ---------------- Rust：主线程阻塞 ----------------
    Case(
        name="主线程守卫（同步命令碰数据库必须报出）",
        why="用户反馈的三个卡死现象就来自这条：同步命令在 macOS 主线程执行，长事务持锁时全部窗口冻住",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "#[tauri::command(async)]\npub fn get_settings",
            "#[tauri::command]\npub fn get_settings",
        )],
        cmd=cargo("test", "--lib", "blocking_commands_run_off_the_main_thread"),
        cwd=TAURI,
        expect_fail_hint="get_settings",
        tags=["rust", "perf"],
    ),
    # ---------------- Rust：capability 覆盖 ----------------
    Case(
        name="capability 覆盖每个窗口（漏一个窗口 ACL 会静默拒绝）",
        why="设置窗口曾经不在 capability 的 windows 里，表现为「选目录/订阅事件静默失败」",
        file=TAURI / "capabilities" / "default.json",
        injections=[('"windows": ["main", "settings", "logs"]', '"windows": ["main", "logs"]')],
        cmd=cargo("test", "--lib", "capability_covers_every_window_label"),
        cwd=TAURI,
        expect_fail_hint="settings",
        tags=["rust", "packaging"],
    ),
    # ---------------- Rust：沙盒目录书签 ----------------
    Case(
        name="书签优先于路径（沙盒重启后唯一带权限的来源）",
        why="书签不优先 ⇒ 用户在 Finder 里移动过的目录会指回旧路径；沙盒里权限也丢了",
        file=TAURI / "src" / "user_dirs.rs",
        injections=[(
            """    if let Some(Ok(path)) = from_bookmark {
        if !path.is_empty() {
            return Some(path);
        }
    }
    stored""",
            """    if stored.is_some() {
        return stored;
    }
    if let Some(Ok(path)) = from_bookmark {
        if !path.is_empty() {
            return Some(path);
        }
    }
    None""",
        )],
        cmd=cargo("test", "--lib", "user_dirs"),
        cwd=TAURI,
        expect_fail_hint="bookmark_wins_over_the_stored_path",
        tags=["rust", "macos"],
    ),
    # ---------------- Rust：BLE 外设（需要 feature） ----------------
    Case(
        name="BLE 分片 MTU 异常值绝不返回 0",
        why="返回 0 ⇒ 分片全部失败、链路静默假死（真机上表现为「连上了但发不出消息」）",
        file=TAURI / "src" / "transport" / "bluetooth_peripheral.rs",
        injections=[(
            'let min = ble_framing::BLE_CHUNK_HEADER_LEN + 1; // 至少装得下"分片头 + 1 字节"',
            "let min = 1;",
        )],
        cmd=cargo("test", "--lib", "--features", "bluetooth", "bluetooth_peripheral"),
        cwd=TAURI,
        expect_fail_hint="central_mtu_clamps",
        tags=["rust", "ble"],
    ),
    Case(
        name="BLE 离开 PoweredOn 必须摘掉全部订阅",
        why="CoreBluetooth 不会补发「对端断开」⇒ 订阅状态陈旧会让写任务白等 8s 且日志空白",
        file=TAURI / "src" / "transport" / "bluetooth_peripheral.rs",
        injections=[(
            "    let mut out: Vec<String> = subscribed.iter().cloned().collect();\n    out.sort(); // 稳定顺序：日志与单测都好读",
            "    let _ = subscribed;\n    let mut out: Vec<String> = Vec::new();",
        )],
        cmd=cargo("test", "--lib", "--features", "bluetooth", "bluetooth_peripheral"),
        cwd=TAURI,
        expect_fail_hint="leaving_powered_on_detaches",
        tags=["rust", "ble"],
    ),
    # ---------------- 前端：静态设计护栏 ----------------
    Case(
        name="键盘可达（div @click 必须报出）",
        why="`div @click` 触屏/鼠标能用但键盘够不着、读屏念成普通文本",
        file=ROOT / "src" / "components" / "settings" / "AboutSection.vue",
        injections=[(
            "<div class=\"px-4 py-3\">",
            "<div class=\"px-4 py-3\">\n      <div class=\"cursor-pointer\" @click=\"onFingerprintTap\">x</div>",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="键盘够不着",
        tags=["frontend", "a11y"],
    ),
    Case(
        name="焦点可见（outline-none 必须有自己的焦点指示）",
        why="全局焦点环写在 `:where()` 里（特异性 0），会被 `.outline-none` 静默覆盖",
        file=ROOT / "src" / "components" / "chat" / "MessageComposer.vue",
        injections=[(
            "leading-normal whitespace-pre-wrap",
            "leading-normal outline-none whitespace-pre-wrap",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="静默覆盖",
        tags=["frontend", "a11y"],
    ),
    Case(
        name="触屏点按目标（小按钮必须有 tap-safe）",
        why="HIG 最小 44pt；小图标按钮手指容易点不中或误触相邻项",
        file=ROOT / "src" / "components" / "FriendProfile.vue",
        injections=[("class=\"tap-safe flex h-8 w-8", "class=\"flex h-8 w-8")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="tap-safe",
        tags=["frontend", "mobile"],
    ),
    Case(
        name="截断文本要有可访问名（truncate 必须有 title）",
        why="被截断的完整内容鼠标悬停拿不到、读屏也可能拿不到",
        file=ROOT / "src" / "components" / "settings" / "AboutSection.vue",
        injections=[(
            '<div class="px-4 py-3">',
            '<div class="px-4 py-3">\n      <div class="truncate">完整名字很长很长</div>',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="truncate",
        tags=["frontend", "a11y"],
    ),
    # ---------------- Rust：Android JNI 签名（跨语言一致性） ----------------
    Case(
        name="Android JNI 签名与 Kotlin 对齐（stop 是 ()V 不是 ()Z）",
        why="JNI 不做编译期检查：描述符写错只在真机抛 NoSuchMethodError —— 真实缺陷是"
        "「关掉蓝牙开关后手机仍在广播」，而且日志里什么都没有",
        file=TAURI / "src" / "transport" / "ble_android.rs",
        injections=[('kotlin_method!("stop", "()V")', 'kotlin_method!("stop", "()Z")')],
        cmd=cargo("test", "--lib", "android_jni_signatures_match_kotlin"),
        cwd=TAURI,
        expect_fail_hint="描述符不一致",
        tags=["rust", "ble", "android"],
    ),
]



def run(cmd: list[str], cwd: Path, timeout: int = 900) -> tuple[int, str]:
    env = dict(os.environ)
    # 沙箱/CI 里写不了 ~/.cargo：仓库内已有一份 CARGO_HOME 时优先用它
    local_cargo = ROOT / "target" / "cargo-home"
    if "CARGO_HOME" not in env and local_cargo.is_dir():
        env["CARGO_HOME"] = str(local_cargo)
    proc = subprocess.run(
        cmd, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout
    )
    return proc.returncode, proc.stdout + proc.stderr


def verify(case: Case) -> tuple[bool, str]:
    original = case.file.read_text()
    detail = ""
    try:
        for old, new in case.injections:
            assert original.count(old) == 1, (
                f"注入锚点在 {case.file.name} 里出现 {original.count(old)} 次（要求恰好 1 次）："
                f"源码可能已改动，请更新 verify-guards.py"
            )
            case.file.write_text(original.replace(old, new, 1))
        _CURRENT = (case.file, original)  # 登记现场：被信号打断时可恢复

        code, out = run(case.cmd, case.cwd)
        if code == 0:
            return False, "改坏之后测试**仍然通过** ⇒ 这条护栏是空转的（没在守东西）"
        if case.expect_fail_hint and case.expect_fail_hint not in out:
            detail = f"（失败输出里没看到 `{case.expect_fail_hint}`，请确认是这条判据报的）"

        case.file.write_text(original)  # 先恢复，再验证恢复后确实通过
        _CURRENT = None
        code2, out2 = run(case.cmd, case.cwd)
        if code2 != 0:
            return False, f"恢复源码之后测试**仍然失败** ⇒ 源码或环境已被破坏：\n{out2[-800:]}"
        return True, detail or "改坏即 FAIL、恢复即 PASS"
    finally:
        # 无论上面发生什么（断言失败/超时/异常），内容级恢复现场
        if case.file.read_text() != original:
            case.file.write_text(original)
        _CURRENT = None


def main() -> int:
    ap = argparse.ArgumentParser(description="非空转验证：故意改坏每条护栏，确认测试真的会失败")
    ap.add_argument("--only", default="", help="只跑名字/标签里包含该子串的用例")
    ap.add_argument("--list", action="store_true", help="只列出用例")
    args = ap.parse_args()

    cases = [c for c in CASES if not args.only or args.only in c.name or args.only in c.tags]
    if args.list:
        for c in cases:
            print(f"  [{','.join(c.tags)}] {c.name}\n      {c.why}")
        return 0

    print(f"非空转验证：{len(cases)} 条护栏（每条都要求「改坏 → FAIL → 恢复 → PASS」）\n")
    bad: list[str] = []
    for i, case in enumerate(cases, 1):
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

    if bad:
        print(f"❌ {len(bad)} 条不符合预期：")
        for name in bad:
            print(f"   - {name}")
        return 1
    print(f"✅ 全部 {len(cases)} 条护栏都通过了非空转验证（改坏即 FAIL、恢复即 PASS）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
