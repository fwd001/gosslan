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
#: 有的护栏要同时改坏**两个**文件（例如"事实来源 + 注入副本"必须一致），所以这里是列表。
_CURRENT: list[tuple[Path, str]] = []


def restore_now() -> None:
    """把"当前注入现场"恢复回原始内容（幂等）。"""
    global _CURRENT
    for path, original in _CURRENT:
        path.write_text(original)
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
    #: 需要**同时**改坏的其它文件（路径, 原文, 替换）。例如"事实来源 + 构建时注入的副本"
    #: 两边都要改，否则护栏会先以"两者漂移"失败，证明不了"漏掉方法也会被抓到"。
    extra_injections: list[tuple[Path, str, str]] = field(default_factory=list)


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
    # ---------------- 前端：IPC 事件契约 ----------------
    Case(
        name="IPC 事件契约（Rust 发的必须有人听）",
        why="真实缺陷：设置窗口改语言/主题后主窗口不刷新 —— 因为根本没有 settings-changed 事件。"
        "同一类还有 group-message-acked 一直没人接",
        file=ROOT / "src" / "api" / "index.ts",
        injections=[('listen("settings-changed"', 'listen("settings-changed-typo"')],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="settings-changed",
        tags=["frontend", "ipc"],
    ),
    # ---------------- Rust：分批清空（点清除数据不卡死的机制） ----------------
    Case(
        name="清空数据必须分批（单次只删一批，批间放锁）",
        why="用户实测：点「清除数据」设置窗口卡死 —— 原实现一个大事务握住 db 锁数秒，"
        "所有读命令都在等锁。改回大事务就会静默退化",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} LIMIT {CLEAR_BATCH_ROWS})",
            "DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} LIMIT 999999999)",
        )],
        cmd=cargo("test", "--lib", "clear_is_batched"),
        cwd=TAURI,
        expect_fail_hint="单次调用",
        tags=["rust", "perf"],
    ),
    # ---------------- 前端：Headless UI 模板插槽 ----------------
    Case(
        name="as=template 插槽不得有注释（dev 保留注释 ⇒ 渲染抛错 ⇒ 窗口卡死）",
        why="用户实测：点「+ → 添加好友」整个窗口卡死的真因 —— BaseModal 在 TransitionChild 插槽里放了注释",
        file=ROOT / "src" / "components" / "BaseModal.vue",
        injections=[(
            '<TransitionChild\n            as="template"',
            '<TransitionChild\n            as="template"\n          >\n            <!-- 注入的注释 -->',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="HTML 注释",
        tags=["frontend", "render"],
    ),
    # ---------------- 前端：store 契约 ----------------
    Case(
        name="store 契约（界面用到的成员必须在 store 里导出）",
        why="用户实测：给 store 新增 channels/refreshChannels 后，dev 里旧 store 实例没有这些成员 ⇒ "
        "设置页渲染抛错 ⇒ 整页卡死（点设置卡、过一会儿弹好几个设置、主题延迟切换）",
        file=TAURI / ".." / "src" / "stores" / "useAppStore.ts",
        injections=[("    refreshChannels,\n", "    refreshChannelsRenamed,\n")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="refreshChannels",
        tags=["frontend", "store"],
    ),
    # ---------------- Android release 包：R8 不得改掉 Rust 按名字调用的 Kotlin 方法 ----------------
    Case(
        name="R8 keep（JNI 方法漏一个就必须报出来）",
        why="release 开 R8 混淆时 `stop/start/send/isConnected/payloadMtu/…` 会被改名成 a/b/c/d/e，"
        "而 JNI 只按「名字 + 签名」查找 ⇒ 真机 release 包的蓝牙外设整条路径 NoSuchMethodError"
        "（debug 不混淆，所以开发期看不见）",
        # ⚠️ 两个文件必须**同时**改坏：只改一个的话护栏会先以"事实来源与注入副本漂移"失败，
        #    那就证明不了"漏掉某个方法也会被抓到"。
        file=ROOT / "scripts" / "android" / "proguard-gosslan.pro",
        injections=[("    public boolean send(java.lang.String, byte[]);\n", "")],
        extra_injections=[
            (
                TAURI / "gen" / "android" / "app" / "proguard-rules.pro",
                "    public boolean send(java.lang.String, byte[]);\n",
                "",
            )
        ],
        cmd=cargo("test", "--lib", "release_keeps_every_kotlin_method_called_from_rust"),
        cwd=TAURI,
        expect_fail_hint="缺少 `send`",
        tags=["rust", "android", "release"],
    ),
    # ---------------- 窗口架构：三个窗口各自一个文档 + 一个入口 ----------------
    Case(
        name="窗口入口（独立窗口不得再共用主窗口的 HTML）",
        why="用户实测：「第二次打开设置，窗口先刷成主聊天窗口、又立马变成设置界面」「点一下要等很久」"
        "—— 根因就是设置/日志窗口加载的是主窗口的 index.html，前端再把聊天三栏挂起来换成设置页",
        file=TAURI / "src" / "commands.rs",
        injections=[('WebviewUrl::App("settings.html".into())', 'WebviewUrl::App("index.html".into())')],
        cmd=cargo("test", "--lib", "aux_windows_open_their_own_document"),
        cwd=TAURI,
        expect_fail_hint="index.html",
        tags=["rust", "window"],
    ),
    Case(
        name="窗口单例（打开命令不得自己查窗口存在性）",
        why="连点两下会开出第二个窗口：`build()` 的重复 label 检查在 prepare 阶段，而窗口登记进 manager "
        "是主线程创建完成之后 —— 并发调用会双双通过。必须统一走 ensure_aux_window（单例 + 串行）",
        file=TAURI / "src" / "commands.rs",
        injections=[
            (
                "    ensure_aux_window(&app, crate::WINDOW_SETTINGS, move || {",
                "    let _ = app.get_webview_window(crate::WINDOW_SETTINGS);\n"
                "    ensure_aux_window(&app, crate::WINDOW_SETTINGS, move || {",
            )
        ],
        cmd=cargo("test", "--lib", "aux_window_open_is_singleton_serialized_and_resident"),
        cwd=TAURI,
        expect_fail_hint="不该自己查窗口存在性",
        tags=["rust", "window"],
    ),
    Case(
        name="窗口骨架（设置窗口必须带自己的骨架类）",
        why="三个窗口共用一份骨架 CSS，靠 `<html class=\"boot-settings\">` 决定显示哪一套；"
        "类名漏了那个窗口就只剩白屏骨架（功能正常、但启动那一下很难看）",
        file=ROOT / "settings.html",
        injections=[('<html lang="zh-CN" class="boot-settings" ', '<html lang="zh-CN" ')],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="boot-settings",
        tags=["frontend", "window"],
    ),
    Case(
        name="开窗接线（按钮必须走单飞入口）",
        why="三处入口（窄导航头像 / 移动端底栏 / 原生菜单）必须共用同一份单飞+防抖状态；"
        "退回成按钮里直接 invoke 就是用户报的「连点会开出第二个窗口」",
        file=ROOT / "src" / "layouts" / "ResponsiveLayout.vue",
        injections=[
            (
                'void launchAuxWindow("settings", () => api.openSettingsWindow())',
                "void api.openSettingsWindow()",
            )
        ],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="launchAuxWindow",
        tags=["frontend", "window"],
    ),
    Case(
        name="常驻窗口（设置窗口重新显示必须刷新环境数据）",
        why="独立设置窗口改成常驻（关闭=隐藏）之后不再重新加载；若只在首次加载时取一次数据，"
        "用户切了 Wi-Fi/换了共享目录再打开设置会看到旧快照 —— 这是'常驻'引入的新退化面",
        file=ROOT / "src" / "entries" / "settings.ts",
        injections=[
            (
                "  void getCurrentWindow().onFocusChanged(({ payload: focused }) => {\n"
                "    if (focused) void useAppStore().refreshEnvironment();\n"
                "  });",
                "  // （非空转验证：这一块被临时移除）",
            )
        ],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="onFocusChanged",
        tags=["frontend", "window"],
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
    global _CURRENT
    #: [(文件, [(原文, 替换), …])] —— 主文件 + 需要"同时改坏"的其它文件
    targets: list[tuple[Path, list[tuple[str, str]]]] = [(case.file, case.injections)]
    targets += [(p, [(old, new)]) for (p, old, new) in case.extra_injections]
    originals = [(path, path.read_text()) for path, _ in targets]
    detail = ""
    try:
        for path, injections in targets:
            text = path.read_text()
            for old, new in injections:
                assert text.count(old) == 1, (
                    f"注入锚点在 {path.name} 里出现 {text.count(old)} 次（要求恰好 1 次）："
                    f"源码可能已改动，请更新 verify-guards.py"
                )
                # ⚠️ 必须在**上一次替换的结果**上继续改（逐条累积），否则一条用例里写多个
                # 注入时只有最后一条生效 —— 这条旧实现的坑在这里一并修掉。
                text = text.replace(old, new, 1)
            path.write_text(text)
        _CURRENT = list(originals)  # 登记现场：被信号打断时可恢复

        code, out = run(case.cmd, case.cwd)
        if code == 0:
            return False, "改坏之后测试**仍然通过** ⇒ 这条护栏是空转的（没在守东西）"
        if case.expect_fail_hint and case.expect_fail_hint not in out:
            detail = f"（失败输出里没看到 `{case.expect_fail_hint}`，请确认是这条判据报的）"

        for path, original in originals:  # 先恢复，再验证恢复后确实通过
            path.write_text(original)
        _CURRENT = []
        code2, out2 = run(case.cmd, case.cwd)
        if code2 != 0:
            return False, f"恢复源码之后测试**仍然失败** ⇒ 源码或环境已被破坏：\n{out2[-800:]}"
        return True, detail or "改坏即 FAIL、恢复即 PASS"
    finally:
        # 无论上面发生什么（断言失败/超时/异常），内容级恢复现场
        for path, original in originals:
            if path.read_text() != original:
                path.write_text(original)
        _CURRENT = []


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
