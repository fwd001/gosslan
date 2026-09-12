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
    Case(
        name="Android JNI 签名与 Kotlin 对齐（打开文件桥 openWith）",
        why="同一条铁律的第二座桥：Rust 按 (Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String; "
        "调 OpenWith.openWith。Kotlin 侧少写 `: String?`（返回 Unit）时真机只抛 NoSuchMethodError —— "
        "用户看到「点开文件没反应」，日志里什么都没有",
        file=TAURI
        / "gen"
        / "android"
        / "app"
        / "src"
        / "main"
        / "java"
        / "com"
        / "gosslan"
        / "app"
        / "OpenWith.kt",
        injections=[(
            "fun openWith(path: String, mime: String?): String? {",
            "fun openWith(path: String, mime: String?) {",
        )],
        cmd=cargo("test", "--lib", "android_jni_signatures_match_kotlin"),
        cwd=TAURI,
        expect_fail_hint="描述符不一致",
        tags=["rust", "android"],
    ),
    # ---------------- Android JNI：object 成员必须有 @JvmStatic（否则没有静态桥） ----------------
    Case(
        name="Rust 调的 Kotlin 成员必须带 @JvmStatic（否则 Method not found）",
        why="真机实测：BlePeripheral.start() 漏了 @JvmStatic（它旁边 6 个兄弟都有）⇒ object 里不生成"
        "静态桥 ⇒ Rust 的 call_static_method 报 `Method not found: start ()Z`，蓝牙外设起不来，"
        "而 central 扫描照常 ⇒ 从日志上很容易误判成'权限问题'",
        file=TAURI
        / "gen"
        / "android"
        / "app"
        / "src"
        / "main"
        / "java"
        / "com"
        / "gosslan"
        / "app"
        / "BlePeripheral.kt",
        injections=[(
            "    @JvmStatic\n    fun start(): Boolean = startOnMain()",
            "    fun start(): Boolean = startOnMain()",
        )],
        cmd=cargo("test", "--lib", "android_jni_signatures_match_kotlin"),
        cwd=TAURI,
        expect_fail_hint="必须在它上面加 `@JvmStatic`",
        tags=["rust", "jni", "android"],
    ),
    # ---------------- Android JNI：static 形态必须与 Kotlin 一致（真机启动闪退） ----------------
    Case(
        name="JNI static 形态与 Kotlin 顶层/成员一致（少了 static 就闪退）",
        why="真实缺陷：OpenWith.kt 的 nativeAttachOpenWith 是**文件级函数**（=static），而 Rust 侧 "
        "native_method! 少了 static ⇒ 按实例方法注册 ⇒ ART 在第一次调用时直接 abort 整个进程"
        "（'registered as instance but called as static method'）。编译/单测/构建全绿，只在真机启动时现形",
        file=TAURI / "src" / "android_open.rs",
        injections=[(
            "    static extern fn native_attach_open_with() -> (),",
            "    extern fn native_attach_open_with() -> (),",
        )],
        cmd=cargo("test", "--lib", "jni_static_matches_kotlin_toplevel"),
        cwd=TAURI,
        expect_fail_hint="static 形态与 Kotlin 不一致",
        tags=["rust", "jni", "android"],
    ),
    # ---------------- Android release 包：打开文件桥的 R8 keep ----------------
    Case(
        name="R8 keep（打开文件桥漏 keep 必须报出来）",
        why="OpenWith.openWith 只被 Rust 的 JNI 按名字调用，R8 会把它当死代码改名/删掉 ⇒ "
        "release 真机包「点开文件」NoSuchMethodError（debug 不混淆，开发期完全看不见）",
        # 与蓝牙那条同理：事实来源与注入副本必须**同时**改坏，否则先以"两处漂移"失败，
        # 证明不了"漏 keep 也会被抓到"。
        file=ROOT / "scripts" / "android" / "proguard-gosslan.pro",
        injections=[(
            "    public static java.lang.String openWith(java.lang.String, java.lang.String);\n",
            "",
        )],
        extra_injections=[
            (
                TAURI / "gen" / "android" / "app" / "proguard-rules.pro",
                "    public static java.lang.String openWith(java.lang.String, java.lang.String);\n",
                "",
            )
        ],
        cmd=cargo("test", "--lib", "release_keeps_every_kotlin_method_called_from_rust"),
        cwd=TAURI,
        expect_fail_hint="缺少 `openWith`",
        tags=["rust", "android", "release"],
    ),
    # ---------------- 前端：设置事件必须"带补丁 + 不回发起窗口" ----------------
    Case(
        name="设置事件不得回发给发起窗口（emit_filter vs emit）",
        why="无载荷广播的话，每个窗口（含刚写完的那个）都要全量重拉三份数据，而且发起窗口会被"
        "自己的旧快照回灌（『点了主题又跳回去』）。真实事故：重拉还会走到 pushUiLanguage → "
        "set_ui_language → 再发一次事件，两个窗口形成高频 IPC 环",
        file=TAURI / "src" / "state.rs",
        injections=[(
            "emit_filter(EVENT_SETTINGS_CHANGED, patch, move |target| {",
            "emit(EVENT_SETTINGS_CHANGED, patch); #[allow(unreachable_code)] let _ = move |target: &tauri::EventTarget| {",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="emit_filter",
        tags=["frontend", "ipc"],
    ),
    Case(
        name="改设置的后端命令必须传 origin（否则发起窗口收到自己的事件）",
        why="同上：只要有一个命令把 origin 写成 None，发起窗口就会被自己的事件回灌 —— "
        "而且它只在『改了设置的那个窗口刚好也在监听』时才现形，很难靠手测发现",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "// 只把**变了的键**发给**另一个窗口**（发起窗口自己已经应用过了，不回发）。\n"
            "    state.notify_settings_changed(&changed, Some(window.label()), patch);",
            "// （注入用例：把 origin 写成 None）\n"
            "    state.notify_settings_changed(&changed, None, patch);",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="没传 origin",
        tags=["frontend", "ipc"],
    ),
    Case(
        name="清空数据必须广播（否则主界面毫无反应）",
        why="用户实测（Mac 4.1.10）：在设置里清了缓存、目录和聊天记录，主界面一点变化都没有 —— "
        "清除只发生在设置窗口自己的 store 里，主窗口是另一个 WebView",
        file=TAURI / "src" / "commands.rs",
        injections=[("    s.notify_data_cleared(Some(window.label()));\n", "")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="data-cleared",
        tags=["frontend", "ipc"],
    ),
    # ---------------- 前端：IPC 事件契约 ----------------
    Case(
        name="IPC 事件契约（Rust 发的必须有人听）",
        why="真实缺陷：设置窗口改语言/主题后主窗口不刷新 —— 因为根本没有 settings-changed 事件。"
        "同一类还有 group-message-acked 一直没人接",
        file=ROOT / "src" / "api" / "index.ts",
        injections=[(
            'listen<SettingsChanged>("settings-changed"',
            'listen<SettingsChanged>("settings-changed-typo"',
        )],
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
    # ---------------- 安卓实测缺陷（2026-09-12）：触屏定位 / 通道同步 / 新的朋友 / 蓝牙默认开 ----------------
    Case(
        name="触屏定位（tap-safe 不得压掉组件的 absolute）",
        why="真实缺陷：安卓端「回到最新」按钮写的是 `tap-safe absolute bottom-4 right-5`，"
        "而 style.css 在 @tailwind utilities 之后、`.tap-safe{position:relative}` 与 `.absolute` "
        "特异性相同 ⇒ 触屏设备上按钮掉回文档流、不再贴右下角（桌面 pointer:fine 不复现）",
        file=ROOT / "src" / "style.css",
        injections=[(":where(.tap-safe) {", ".tap-safe {")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="pointer: coarse",
        tags=["frontend", "css", "android"],
    ),
    Case(
        name="通道同步（设置页不得用 app.online 当局域网开关值）",
        why="用户实测：「添加好友里打开局域网，设置里还是关的」—— 同一个概念有两份前端状态"
        "（channels[lan].enabled 与 app.online），两处 UI 各读一份就必然不同步",
        file=ROOT / "src" / "components" / "settings" / "NetworkSection.vue",
        injections=[(':model-value="!!lanStatus?.enabled"', ':model-value="app.online"')],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="app.online",
        tags=["frontend", "android", "channel"],
    ),
    Case(
        name="移动端「新的朋友」必须切主面板",
        why="用户实测：安卓端收到好友申请后点「新的朋友」没反应 —— 申请页在右侧主面板里，"
        "而移动端靠 mobileView 平移切换，不切过去就还停在会话列表上",
        file=ROOT / "src" / "layouts" / "ResponsiveLayout.vue",
        injections=[(
            '  if (app.isMobile) app.mobileView = "chat";\n}\n\n/** 收起「新的朋友」页',
            "}\n\n/** 收起「新的朋友」页",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="mobileView",
        tags=["frontend", "android", "nav"],
    ),
    # ---------------- 好友申请：已是好友的申请必须自动消失（2026-09-12 用户实测） ----------------
    Case(
        name="好友申请（已是好友的申请必须从「新朋友」消失）",
        why="用户实测：双方互发过申请、一方点同意后，另一方点进「新朋友」那条申请还在。"
        "根因是「同意」各条路径行为不一致；前端再用「人已经是好友」这个事实兜一层",
        file=ROOT / "src" / "stores" / "useChatStore.ts",
        injections=[
            (
                "const pendingRequests = computed(() =>\n    actionableRequests(",
                "const pendingRequests = computed(() =>\n    ((x: unknown) => x)(",
            )
        ],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="按好友过滤",
        tags=["frontend", "friend"],
    ),
    Case(
        name="好友同意（两条 FriendAccept 路径都必须清 pending）",
        why="用户实测根因：直连 `Message::FriendAccept` 只加好友、忘了清 pending，"
        "跨跳 `GossipKind::FriendAccept` 清了 ⇒ 「有时候会清、有时候不清」",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[
            (
                "            // 已经是好友了 ⇒ 这条申请必须消失（否则「新朋友」里会留着一条永远处理不掉的申请）\n"
                "            forget_pending_request(state, &from);",
                "            // （非空转验证：这一行被临时移除）",
            )
        ],
        cmd=cargo("test", "--lib", "every_friend_accept_path_forgets_the_pending_request"),
        cwd=TAURI,
        expect_fail_hint="两条 FriendAccept 路径",
        tags=["rust", "friend"],
    ),
    Case(
        name="好友申请兜底（get_pending_requests 必须按好友关系过滤）",
        why="用户明确要求：已在好友列表的人，其申请应当自动清除。主修在各条同意路径，"
        "这里是不依赖「哪条消息到了」的兜底判据",
        file=TAURI / "src" / "commands.rs",
        injections=[("map.retain(|_, req| is_actionable_request(req, &friend_ids));", "map.retain(|_, _| true);")],
        cmd=cargo("test", "--lib", "pending_requests_exclude_existing_friends"),
        cwd=TAURI,
        expect_fail_hint="必须按好友关系过滤",
        tags=["rust", "friend"],
    ),
    Case(
        name="JNI keep 必须是 public static（否则静态桥被 R8 删掉 ⇒ Method not found）",
        why="真机实测：Rust 用 call_static_method 调 BlePeripheral.start()，而 Kotlin 的 @JvmStatic "
        "在 object 里生成『实例方法 + 静态桥』两个条目；keep 规则只写 public boolean start(); 时 "
        "R8 把静态桥当死代码删掉 ⇒ `JNI 调用失败：Method not found: start ()Z`（蓝牙外设起不来）",
        # 事实来源与注入副本必须同时改坏（同既有 keep 用例的理由）
        file=ROOT / "scripts" / "android" / "proguard-gosslan.pro",
        injections=[("    public static boolean start();", "    public boolean start();")],
        extra_injections=[
            (
                TAURI / "gen" / "android" / "app" / "proguard-rules.pro",
                "    public static boolean start();",
                "    public boolean start();",
            )
        ],
        cmd=cargo("test", "--lib", "release_keeps_every_kotlin_method_called_from_rust"),
        cwd=TAURI,
        expect_fail_hint="必须写成 `public static",
        tags=["rust", "jni", "android"],
    ),
    Case(
        name="已是好友的申请必须自动同意（否则双方永远加不上）",
        why="用户实测：B 的好友列表里有 A，而 A 是重置过的账号、列表里没有 B。A 发申请只在 B 侧插"
        "一条 pending，而 UI 又会把『申请人已是好友』的条目过滤掉（那是为了修『申请还挂着』）"
        "⇒ 两边都看不到、谁也加不上，只能先把 B 里的 A 删掉再加回来",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "            if auto_accept_if_already_friend(state, &from).await {",
            "            if false {",
        )],
        cmd=cargo("test", "--lib", "friend_request_from_existing_friend_auto_accepts"),
        cwd=TAURI,
        expect_fail_hint="两条 FriendRequest 路径",
        tags=["rust", "friend"],
    ),
    Case(
        name="应用样式（三个窗口都必须加载 style.css）",
        why="真实缺陷：一窗一入口重构时漏掉了 `import \"./style.css\"`，dev 起来整个界面\"像没有 CSS\"，"
        "而且不报错、不影响任何测试 —— 只有这条守卫能拦住",
        file=ROOT / "src" / "boot" / "boot.ts",
        injections=[('import "@/style.css";', "// （非空转验证：这一行被临时移除）")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="style.css",
        tags=["frontend", "style"],
    ),
    Case(
        name="版本号规则（feat 必须算中档，否则台账与门禁一起失效）",
        why="版本分类是发布台账与 `version:check` 的唯一依据；把 feat 降成 patch 会让\"中功能\""
        "永远不提升中位，历史台账与门禁同时失真（这类退化不报错、也不影响功能）",
        file=ROOT / "scripts" / "semver.mjs",
        injections=[('feat: "minor",', 'feat: "patch",')],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="minor",
        tags=["frontend", "version"],
    ),
    Case(
        name="CHANGELOG 结构（[Unreleased] 锚点缺失/顺序错乱必须报出）",
        why="发布脚本按行首 `## [Unreleased]` 插入新小节。真实事故：它以前用 includes+replace "
        "找锚点，正文里出现同样文字就被误命中 ⇒ 4.1.1~4.1.11 全被插进 4.1.0 小节的半句话里、"
        "真正的锚点被吞掉（不报错、不影响功能，只有结构检查能拦住）",
        file=ROOT / "CHANGELOG.md",
        injections=[("## [Unreleased]\n", "## [unreleased]\n")],
        cmd=npm("run", "version:check"),
        cwd=ROOT,
        expect_fail_hint="[Unreleased]",
        tags=["frontend", "version", "docs"],
    ),
    Case(
        name="打包配置（release 前端必须压缩）",
        why="TAURI_ENV_DEBUG 是字符串（release 为 \"false\"），`!process.env.TAURI_ENV_DEBUG` "
        "把 release 当成 debug ⇒ 前端不压缩还带 sourcemap（实测 310KB → 500KB + 735KB .map）。"
        "这类退化不报错、不影响功能，只会让所有 release 包悄悄变慢",
        file=ROOT / "vite.config.ts",
        injections=[("minify: isDebugBuild ? false : \"esbuild\"", "minify: !process.env.TAURI_ENV_DEBUG ? \"esbuild\" : false")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="TAURI_ENV_DEBUG",
        tags=["frontend", "build"],
    ),
    Case(
        name="蓝牙默认开启（三端一致，零配置）",
        why="用户规则：「有蓝牙就默认开，不用手动开关」。缺省一旦退回按平台分支（或改成 false），"
        "就会重新出现「手机有通道、Mac 要手点」以及「偏好=关 vs 运行时=开」互相回灌的启停抖动",
        file=TAURI / "src" / "db.rs",
        injections=[("let default_on = true;", "let default_on = false;")],
        cmd=cargo("test", "--lib", "bt_defaults_on_everywhere"),
        cwd=TAURI,
        expect_fail_hint="缺省必须是",
        tags=["rust", "android", "channel"],
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
