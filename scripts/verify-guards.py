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
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TAURI = ROOT / "src-tauri"


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
    # ---------------- 本地新增护栏（2026-09-14）----------------

    Case(
        name="Win11 窗口字形都在（删掉它最大化按钮渲染为空）",
        why="2026-09-17 起窗口按钮按 Win11 细线造型自绘（data-win-glyph 标记）；"
            "此前 0e07dd4 删过图标 import 而 Windows 分支仍在用 ⇒ 按钮渲染为空。"
            "这类退化不报错、不影响构建，只有这条守卫能拦住",
        file=ROOT / "src" / "components" / "TitleBar.vue",
        injections=[(
            'data-win-glyph="maximize"',
            'data-win-glyph="maximize-x"',
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/utils/titleBarIcons.test.ts"],
        cwd=ROOT,
        expect_fail_hint="字形",
        tags=["frontend", "new-guards"],
    ),
    Case(
        name="Presence 不得内联大头像（否则优先通道被堵）",
        why="Presence 每 10s 广播一次且走优先通道；一张 400KB 头像会让聊天/好友请求"
            "排在几百片分片后面（真机：加好友几分钟才到）",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "let avatar = hello_avatar_for_wire(raw_avatar.as_deref());",
            "let avatar = raw_avatar.as_deref();",
        )],
        cmd=cargo("test", "--lib", "presence_caps_inline_avatar"),
        cwd=TAURI,
        expect_fail_hint="broadcast_presence",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="大头像资料帧必须降到 Low 队列（否则堵住优先通道）",
        why="UserInfo 带大 avatar 时若不降级，会占满优先通道，聊天/好友请求几分钟才到。"
            "分类表已从 is_bulk_message 收进 dispatch::message_priority（单一事实来源），"
            "锚点随之搬到 dispatch.rs",
        file=TAURI / "src" / "network" / "dispatch.rs",
        injections=[(
            "Message::UserInfo { avatar: Some(a), .. } if a.len() > CONTROL_AVATAR_MAX_BYTES => {\n            MessagePriority::Low\n        }",
            "Message::UserInfo { avatar: Some(a), .. } if a.len() > CONTROL_AVATAR_MAX_BYTES => {\n            MessagePriority::Normal\n        }",
        )],
        cmd=cargo("test", "--lib", "bulk_messages_are_only_large_chunks"),
        cwd=TAURI,
        expect_fail_hint="bulk_messages_are_only_large_chunks",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="共享目录/中继文件：无直连时借一跳中继（定向转发判定）",
        why="共享目录原本只支持直连，A 与 B 只能经中继时打不开；这条纯函数决定哪些帧"
            "要借邻居转投（真机 2026-09-14 全 Windows 局域网）",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "Message::RelayFileOffer { to, .. } if to != my_id => Some(to.as_str()),",
            "Message::RelayFileOffer { .. } => None,",
        )],
        cmd=cargo("test", "--lib", "directed_relay_target_routes_share_and_offer_frames"),
        cwd=TAURI,
        expect_fail_hint="directed_relay_target_routes_share_and_offer_frames",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="中继文件接收幂等（重复 offer 不清空已收切片）",
        why="多邻居泛洪会送来重复的 RelayFileOffer；覆盖式 insert 会清空已收到的切片 ⇒ "
            "文件永远缺片（完整性校验也必然失败）",
        file=TAURI / "src" / "file_relay.rs",
        injections=[(
            "        self.reassemblies\n"
            "            .entry(transfer_id.to_string())\n"
            "            .or_insert_with(|| Reassembly {",
            "        self.reassemblies.insert(\n"
            "            transfer_id.to_string(),\n"
            "            Reassembly {",
        )],
        cmd=cargo("test", "--lib", "begin_reassemble_is_idempotent"),
        cwd=TAURI,
        expect_fail_hint="begin_reassemble_is_idempotent",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="BLE MTU 吞吐估算（净数据必须扣 6 字节分片头）",
        why="日志里的 KB/s 是给用户的量级预期；算错一个量级会误导排障"
            "（旧的 1KB/s 注释就是例子）",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "let net = payload_budget.saturating_sub(crate::transport::ble_framing::BLE_CHUNK_HEADER_LEN);",
            "let net = payload_budget;",
        )],
        cmd=cargo("test", "--lib", "--features", "bluetooth", "throughput_estimate_matches_real_mtu_budgets"),
        cwd=TAURI,
        expect_fail_hint="throughput_estimate_matches_real_mtu_budgets",
        tags=["rust", "ble", "new-guards"],
    ),
    Case(
        name="自动拉起蓝牙必须尊重用户的关闭偏好（退出重进不能又打开）",
        why="真机 2026-09-14：电脑端设置里关掉蓝牙，退出重进又被 ensureBluetoothOn 自动拉起。"
            "判据必须用持久化偏好 preferred，而不是运行时 enabled/running —— 启动瞬间必然没在跑，"
            "只看运行状态就会把用户的关闭选择覆盖掉。",
        file=ROOT / "src" / "stores" / "useAppStore.ts",
        injections=[(
            "      if (!ch.preferred) return;",
            "      // 关闭偏好判断被移除（护栏注入）",
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/utils/channelState.test.ts"],
        cwd=ROOT,
        expect_fail_hint="偏好",
        tags=["frontend", "new-guards"],
    ),
    Case(
        name="桌面通知必须走后端命令（不能依赖被插件替换的 window.Notification）",
        why="Tauri 的 notification 插件把 window.Notification 换成转发到 plugin:notification|notify，"
            "onclick 永远不触发、且把真正的 toast 错误 spawn 掉丢了 —— Windows 同事『收不到通知』查无实据。"
            "现在统一 api.notifyDesktop（失败可返回/记录），并修复隐藏窗口下 hasFocus 仍为 true 的漏通知。",
        file=ROOT / "src" / "stores" / "useChatStore.ts",
        injections=[(
            "void api.notifyDesktop(title, body, convId).catch(() => {",
            "void Promise.resolve().catch(() => {",
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/utils/channelState.test.ts"],
        cwd=ROOT,
        expect_fail_hint="notify_desktop",
        tags=["frontend", "new-guards"],
    ),
    Case(
        name="Windows 专用分支的编译错误必须本地可拦（ends_with 少 as_str）",
        why="2026-09-14 真实事故：notifications.rs 的 Windows 分支写成 ends_with(format!(...))，"
            "String 未实现 Pattern ⇒ 两个 Windows CI job 全挂；macOS 上该分支被 cfg 掉、本地跑不出来。",
        file=TAURI / "src" / "notifications.rs",
        injections=[(
            'let in_dev = curr_dir.ends_with(format!("{SEP}target{SEP}debug").as_str())',
            'let in_dev = curr_dir.ends_with(format!("{SEP}target{SEP}debug"))',
        )],
        cmd=cargo("test", "--lib", "windows_only_branch_is_source_checkable_for_pattern_bounds"),
        cwd=TAURI,
        expect_fail_hint="as_str",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="在线状态必须包含有活跃链路的节点（不能只看节点表）",
        why="用户 2026-09-14：局域网直连上了，好友在线状态却不实时。前端原来只按\"在不在 peers 表\""
            "判在线，而有链路但广播没收到（防火墙/组播限制）或刚被 sweep 的节点会被判离线。",
        file=ROOT / "src" / "stores" / "useChatStore.ts",
        injections=[(
            "f.online = onlineIds.has(f.device_id) || linkedIds.has(f.device_id)",
            "f.online = onlineIds.has(f.device_id)",
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/utils/channelState.test.ts"],
        cwd=ROOT,
        expect_fail_hint="活跃链路",
        tags=["frontend", "new-guards"],
    ),
    Case(
        name="图片预览：在途失败不能永久缓存（否则字节落盘也不重读）",
        why="用户 2026-09-14：群里收图时好时坏，点几次/等一会儿/重发才出来。收到图片时可能"
            "\"消息先到、字节后到\"，在途读预览得到的失败若被永久缓存，文件落盘后也不会重读。",
        file=ROOT / "src" / "utils" / "filePreview.ts",
        injections=[(
            '      if (r.missing || r.note === "文件过大，无法预览") cache.set(msgId, r);',
            "      cache.set(msgId, r);",
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/utils/channelState.test.ts"],
        cwd=ROOT,
        expect_fail_hint="确定性失败",
        tags=["frontend", "new-guards"],
    ),
    # ---------------- Rust：capability 覆盖 ----------------
    Case(
        name="capability 覆盖每个窗口（漏一个窗口 ACL 会静默拒绝）",
        why="设置窗口曾经不在 capability 的 windows 里，表现为「选目录/订阅事件静默失败」",
        file=TAURI / "capabilities" / "default.json",
        # 2026-09-17：windows 数组新增了 `todo-*`（群任务窗口是动态 label），锚点随之更新。
        injections=[
            ('"windows": ["main", "settings", "logs", "todo-*"]', '"windows": ["main", "logs", "todo-*"]')
        ],
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
    # ---------------- Rust：BLE 载荷预算（常量与换算的唯一事实来源） ----------------
    Case(
        name="BLE 分片载荷预算异常值绝不返回 0（否则链路静默假死）",
        why="返回 0 ⇒ `fragment` 拒绝一切、链路静默假死（真机上表现为「连上了但发不出消息」）。"
        "⚠️ 2026-09-16 换了注入点与命令：原先注入 `bluetooth_peripheral.rs` 的 "
        "`central_payload_mtu`，而那份实现已收敛进 `ble_framing::notify_payload_budget`，"
        "旧锚点随之消失 ⇒ 本用例当时退化成「锚点出现 0 次」的报错。"
        "改注入规范位置后**平台限制也一并去掉**：`ble_framing` 不做平台门控，"
        "所以这条现在在 macOS / Windows / Linux 上都有效，且不再需要 `--features bluetooth`"
        "（`ble_framing` 是 `transport/mod.rs` 里无条件编译的模块）。",
        file=TAURI / "src" / "transport" / "ble_framing.rs",
        injections=[(
            '    let min = BLE_CHUNK_HEADER_LEN + 1; // 至少装得下"分片头 + 1 字节"',
            "    let min = 0;",
        )],
        cmd=cargo("test", "--lib", "notify_payload_budget_clamps_and_never_returns_zero"),
        cwd=TAURI,
        expect_fail_hint="应退回默认而不是返回 0",
        tags=["rust", "ble"],
    ),
    Case(
        name="BLE 读循环必须回灌读活性（否则健康链路 45s 自拆）",
        why="ConnectionHealth 的读活性只在建链时播种、此后只由读循环刷新；BLE 读循环漏了这句"
        "⇒ 任何健康蓝牙链路 15s 后被判不健康、45s 被看门狗当死链路拆掉，对端再拨回来再拆，"
        "无限循环（真机体感：蓝牙时好时坏、加好友/消息过一会儿才到）",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "                    mark_conn_seen(&state, &peer_id, &ep);",
            "                    // 回归：不再回灌读活性",
        )],
        cmd=cargo(
            "test",
            "--features",
            "bluetooth",
            "--lib",
            "ble_reader_loop_refreshes_read_activity",
        ),
        cwd=TAURI,
        expect_fail_hint="mark_conn_seen",
        tags=["rust", "ble"],
    ),
    Case(
        name="外设握手失败必须解除『握手中』标记（否则设备再也加入不进 mesh）",
        why="旧实现只在握手成功（RouteCtl::Add）与对端退订（Unlinked）时清理 handshaking，"
        "握手失败时不清理 ⇒ 该 central 之后的真 Hello 被『已在握手』静默丢弃 ⇒ "
        "那台设备再也连不进来（macOS 外设没有断连回调，条目可能永久残留）。"
        "这类退化不会让任何行为测试失败，只能靠结构护栏盯住",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[("if !ok {\n", "if ok {\n")],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "peripheral_handshake_failure_clears_the_handshaking_mark",
        ),
        cwd=TAURI,
        expect_fail_hint="HandshakeFailed",
        tags=["rust", "ble"],
    ),
    Case(
        name="外设每次订阅都清掉该 central 的重组器（否则重连后第一条消息丢）",
        why="对端 msg_id 每条连接从 1 重来，而 macOS 外设没有 didDisconnect 回调、"
        "didUnsubscribe 也不保证到达 ⇒ 上一轮残留的半截消息会和重连后的第一帧撞车、"
        "那条帧被当坏片丢掉（真机体感：重连后第一条消息丢了）。只靠 30s TTL 兜底太慢",
        file=TAURI / "src" / "transport" / "bluetooth_peripheral.rs",
        injections=[(
            "                .remove(&id);\n            let _ = self.ivars().signal.send(1); // 订阅数变化 ⇒ 唤醒等订阅的写任务",
            "                .len();\n            let _ = self.ivars().signal.send(1); // 订阅数变化 ⇒ 唤醒等订阅的写任务",
        )],
        cmd=cargo(
            "test",
            "--offline",
            "--lib",
            "--features",
            "bluetooth",
            "peripheral_subscribe_resets_that_centrals_reassembler",
        ),
        cwd=TAURI,
        expect_fail_hint="必须**按 central id** remove",
        tags=["rust", "ble"],
    ),
    Case(
        name="蓝牙启动不得阻塞在 CoreBluetooth 状态回执上（否则开关卡 3 秒）",
        why="`start_peripheral` 要等 `peripheral::STATE_WAIT = 3s`（CoreBluetooth 回报状态），"
        "而 `ble::start` 就在 `set_channel_enabled` 的关键路径上 ⇒ 一旦 await 它，"
        "用户点蓝牙开关就要干等 3 秒（用户 2026-09-13 Mac 实测「点了一下，"
        "过了好一会儿才会开」）。外设角色本来就是独立失败的，必须丢后台任务；"
        "同时句柄要先写进 state.ble，否则「刚开就关」时 stop() 拿不到 handle、发不出停机信号。"
        "⚠️ 2026-09-16 更新锚点：该 cfg 后来加入了 `target_os = \"windows\"`（Windows 外设角色"
        "落地），而锚点仍写着旧的两平台列表 ⇒ 本用例此前是「锚点出现 0 次」的报错状态。"
        "这正是「护栏会静默腐烂、只有跑起来才知道」的又一例。",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "#[cfg(any(target_os = \"macos\", target_os = \"windows\", target_os = \"android\"))]\n"
            "    #[allow(clippy::let_underscore_future)] // JoinHandle 丢弃不影响 spawn 的任务\n"
            "    let _ = tokio::spawn(start_peripheral(state.clone(), shutdown_tx.subscribe()));",
            "#[cfg(any(target_os = \"macos\", target_os = \"windows\", target_os = \"android\"))]\n"
            "    #[allow(clippy::let_underscore_future)] // JoinHandle 丢弃不影响 spawn 的任务\n"
            "    start_peripheral(state.clone(), shutdown_tx.subscribe()).await;",
        )],
        cmd=cargo(
            "test",
            "--offline",
            "--lib",
            "--features",
            "bluetooth",
            "ble_start_does_not_block_on_the_peripheral_state_wait",
        ),
        cwd=TAURI,
        expect_fail_hint="不许 await 外设启动",
        tags=["rust", "ble", "perf"],
    ),
    Case(
        name="群消息：非成员中继必须继续转发（不能提前 return）",
        why="旧实现把『我不是群成员』直接 return 掉，位置在转发之前 ⇒ 非成员中继不转发群消息 ⇒ "
        "BLE-only 三点中继（手机—电脑—手机）里群聊永远不通，而同链路单聊正常。"
        "这类退化**不会让任何行为测试失败**，只能靠结构护栏盯住",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "    let group_consumable = group_envelope_consumable(\n",
            "    if matches!(env.kind, GossipKind::Group) && !env.group_members.is_empty() {\n"
            "        if !env.group_members.iter().any(|m| m == &state.device_id) {\n"
            "            return;\n"
            "        }\n"
            "    }\n"
            "    let group_consumable = group_envelope_consumable(\n",
        )],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "handle_gossip_does_not_bail_out_for_non_members",
        ),
        cwd=TAURI,
        expect_fail_hint="非成员",
        tags=["rust", "mesh"],
    ),
    Case(
        name="文件分片：重复/迟到必须忽略（只有真跳号才报错）",
        why="发送方重试时 seq 从 0 重来，而旧实现把『重复/迟到』也当致命错误 ⇒ 接收方整单失败、"
        "清掉状态 ⇒ 新 attempt 永远拼不齐（用户实测的那条「文件分片顺序错误」）⇒ "
        "BLE 上 >20KB 的文件事实上永远传不完",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            "Ordering::Less => ChunkSeq::Duplicate",
            "Ordering::Less => ChunkSeq::Gap",
        )],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "chunk_seq_rule_only_rejects_real_gaps",
        ),
        cwd=TAURI,
        expect_fail_hint="chunk_seq_rule",
        tags=["rust", "file"],
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
        platforms=("darwin",),
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
        why="全局焦点环写在 `:where()` 里（特异性 0），会被 `.outline-none`（特异性 0,1,0）"
        "静默覆盖 —— 7 处输入框（含最高频的消息输入框）因此完全没有焦点指示，"
        "而代码看起来「有全局规则在管」（真实缺陷 2026-09-12）。"
        "⚠️ 2026-09-16 换了注入点：原先注入 `MessageComposer.vue`，而该文件后来因用户要求"
        "（消息输入框不画焦点环）加了**文件级** `focus-ring-ok` 逃生阀 ⇒ 整个文件被跳过、"
        "本用例退化成空转（改坏也不报，2026-09-16 由 verify-guards 全量跑发现）。"
        "改注入 `TitleBar.vue` 的关闭按钮：未被豁免，且它是**键盘可聚焦的 button** —— "
        "正是这条护栏真正要保护的场景（键盘用户看不到焦点在哪）。"
        "MessageComposer.vue 整文件失去本条覆盖的问题，另行按元素级逃生阀处理。",
        file=ROOT / "src" / "components" / "TitleBar.vue",
        injections=[(
            "flex w-11 items-center justify-center text-[var(--gosslan-rail-text)] "
            "transition hover:bg-[var(--gosslan-danger)] hover:text-white",
            "flex w-11 items-center justify-center text-[var(--gosslan-rail-text)] "
            "transition hover:bg-[var(--gosslan-danger)] hover:text-white outline-none",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="静默覆盖",
        tags=["frontend", "a11y"],
    ),
    Case(
        name="焦点可见：MessageComposer 里未豁免的元素也必须被守到（豁免不得外溢）",
        why="2026-09-16 发现：该文件为「消息输入框不画焦点环」这**一个元素**的需求用了**文件级**"
        "逃生阀 ⇒ 整个文件（含「取消引用」按钮等键盘可聚焦元素）一起失去本条保护，"
        "而注入该文件的旧用例因此退化成空转（改坏也不报）。改成元素级 `data-focus-ring-ok` 后，"
        "本用例把**未**打标记的那个按钮改坏，必须报出来 —— 它同时钉住两个坑："
        "① 元素级豁免不得外溢到同文件其它元素；② 文件级令牌不能是元素级令牌的子串"
        "（`data-focus-ring-ok` 含有 `focus-ring-ok`，所以文件级必须写成 `focus-ring-ok:file`，"
        "否则「只豁免一个元素」会被判成「整文件豁免」）。",
        file=ROOT / "src" / "components" / "chat" / "MessageComposer.vue",
        injections=[(
            "flex h-5 w-5 shrink-0 items-center justify-center "
            "rounded-[var(--gosslan-radius-xs)] transition hover:bg-[var(--gosslan-hover)]",
            "flex h-5 w-5 shrink-0 items-center justify-center "
            "rounded-[var(--gosslan-radius-xs)] transition hover:bg-[var(--gosslan-hover)] outline-none",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="静默覆盖",
        tags=["frontend", "a11y", "new-guards"],
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
    Case(
        name="聊天区文本选择契约（可选中 / 头像不可选 / 长按容差）",
        why="用户 2026-09-13 实测的三件静默退化：PC 上拖选气泡「刚选中立马取消」、"
        "移动端选中文字后不弹「复制」工具条、头像能被拖进选区。这些都是"
        "「代码看着对、用户一用就不对」，改坏了不会报错，只能靠护栏盯住",
        file=ROOT / "src" / "App.vue",
        injections=[(
            "    const sel = window.getSelection();",
            "    // 回归：不再检查选区（有选中文字时也 preventDefault）",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="contextmenu",
        tags=["frontend", "selection"],
    ),
    Case(
        name="BLE 写失败必须重试并拆链路（否则留下能收不能发的僵尸链路）",
        why="真机 2026-09-13 安卓：两条 BLE 会话都就绪后，一阵群 gossip 把链路写满 ⇒ 各出现一次"
        "「写失败 ⇒ 结束该链路写循环」⇒ 从此发不出去（界面报连接已关闭），而读还在正常收 ⇒ "
        "看门狗按读活性判健康、45s 也不拆 ⇒ 只能重启应用。根因是只结束写循环、把链路留成僵尸",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "                    {\n"
            "                        let links = state.links.lock().await;\n"
            "                        if let Some(l) = links\n"
            "                            .get(&peer_id)\n"
            "                            .and_then(|v| v.iter().find(|l| l.endpoint == ep))\n"
            "                        {\n"
            "                            let _ = l.cancel.send(true);\n"
            "                        }\n"
            "                    }\n"
            "                    break;",
            "                    break;",
        )],
        cmd=cargo(
            "test",
            "--offline",
            "--lib",
            "--features",
            "bluetooth",
            "ble_write_failure_retries_then_tears_the_link_down",
        ),
        cwd=TAURI,
        expect_fail_hint="最终失败必须去链路表里取消",
        tags=["rust", "ble", "perf"],
    ),
    Case(
        name="⌘W 必须由自定义菜单项处理（系统预定义项在无边框窗口上会被判不可用）",
        why="用户 2026-09-13 真机：Mac 上主窗口 ⌘W 只会「滴滴滴」，而设置/日志窗口正常，"
        "⌘Q 也正常。系统预定义关闭项的动作是 performClose:，AppKit 按窗口的 Closable "
        "样式位校验可用性，而本项目 decorations:false ⇒ Borderless ⇒ 该项被判不可用，"
        "**而且没有任何日志**；自定义项不经这套校验，行为与「×」一致",
        file=TAURI / "src" / "menu.rs",
        injections=[(
            '        .item(&MenuItem::with_id(\n            app,\n            "close-window",\n            l.close_window,\n            true,\n            Some("CmdOrCtrl+W"),\n        )?)\n',
            '        .item(&PredefinedMenuItem::close_window(app, None)?)\n',
        )],
        cmd=cargo(
            "test",
            "--offline",
            "--lib",
            "--features",
            "bluetooth",
            "cmd_w_is_handled_by_our_own_menu_item",
        ),
        cwd=TAURI,
        expect_fail_hint="不许用系统预定义的关闭项",
        tags=["rust", "macos", "window"],
    ),
    Case(
        name="解除好友关系必须同时解除内存身份绑定（否则重装后只能重启）",
        why="用户 2026-09-13 真机：对方重装换过公钥后，删好友重新加也收不到任何东西，"
        "**必须重启**。根因是 `verify_hello` 的绑定有两条腿：friends 表 + 内存 peers 表"
        "（广播学来、未验签的公钥）。删好友只断了第一条腿，内存那条旧公钥继续当信任根 ⇒ "
        "Hello 一直被硬拒。这条退化的形态是「功能看着都在、就是连不上」，只能靠源码护栏盯住",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "    crate::network::transport::forget_peer_identity(s, &peer_id);\n",
            "",
        )],
        cmd=cargo(
            "test",
            "--offline",
            "--lib",
            "--features",
            "bluetooth",
            "removing_a_friend_also_drops_the_in_memory_identity_binding",
        ),
        cwd=TAURI,
        expect_fail_hint="forget_peer_identity",
        tags=["rust", "identity", "friend"],
    ),
    Case(
        name="打生产包必须带 --features bluetooth（否则产物静默地没有蓝牙）",
        why="BLE 是可选 feature，漏了 `--features bluetooth` 的后果是**静默**的：构建成功、"
        "产物正常、只是那个包完全没有蓝牙（开关起不来、搜不到设备）。"
        "真机代价是拿一个没有蓝牙的包去测 Windows ↔ Android，白跑一轮 —— "
        "这个坑在 dist:win 系列与**一键入口** `npm run dist`（scripts/package.mjs）上都出现过",
        file=ROOT / "package.json",
        injections=[(
            '"dist:win": "tauri build --features bluetooth --bundles nsis --target x86_64-pc-windows-msvc"',
            '"dist:win": "tauri build --bundles nsis --target x86_64-pc-windows-msvc"',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="没有蓝牙",
        tags=["frontend", "build"],
    ),
    Case(
        name="操作面板：点任何一项都要收起（引用/转发会跳到别处）",
        why="用户 2026-09-13 安卓实测：「点『引用』这个 sheet 应该自动隐藏；点『转发』也应该"
        "自动隐藏，因为它会跳转到界面内去操作聊天」。不在 ActionSheet 面板层统一收，就得每个"
        "入口各写一遍（漏一个：点『保存图片』这类也一样挂着），而且跳转后的界面会被它挡住",
        file=ROOT / "src" / "components" / "ActionSheet.vue",
        injections=[(
            "            @click=\"emit('close')\"\n          >",
            "          >",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="必须在点击时收起",
        tags=["frontend", "mobile", "selection"],
    ),
    Case(
        name="通道开关必须乐观更新（否则点一下要等后端 2~3s 才动）",
        why="用户 2026-09-13：Mac 上「点了一下，过了好一会儿才会关；再点一下，"
        "过了好一会儿才会开」。根因是开关要等 `await api.setChannelEnabled` 回来才改状态，"
        "而蓝牙启停是 2~3s 级的（`ble::start` 等 CoreBluetooth 状态最多 3s、"
        "`ble::stop` 等扫描任务退出最多 2s）。这条退化的形态很隐蔽 —— 功能还在、只是慢，"
        "所以只能靠守卫钉住顺序：先按用户意图改状态 → 再执行 → 失败回退",
        file=ROOT / "src" / "stores" / "useAppStore.ts",
        injections=[(
            "    channels.value = prev.channels.map((c) => (c.channel === channel ? { ...c, enabled } : c));\n",
            "",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="乐观更新",
        tags=["frontend", "channel"],
    ),
    Case(
        name="长按面板：正文气泡里必须弹得出、抬手指不许把它关掉",
        why="用户 2026-09-13 Android 实测两条：「长按气泡有的时候弹不出来、有的时候能弹出来」"
        "（命中 `.gosslan-selectable` 就不起长按 ⇒ 只有按到气泡内边距才弹）与"
        "「弹出 sheet 之后一放手立马就缩回去了」（HeadlessUI 的 outside-click 在 document "
        "捕获阶段挂 `touchend`，而 touch 的 target 在 touchstart 就定死成那条消息 ⇒ 抬手被"
        "判成点了外面）。两条都是「换个位置按/按慢一点就正常」，只能靠判据单测 + 结构护栏",
        file=ROOT / "src" / "components" / "MessageItem.vue",
        injections=[(
            "  if (!shouldSwallowLongPressRelease({ openedByHeldPress: longPressHeld, sheetOpen: sheetOpen.value })) {\n",
            "  if (false) {\n",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="吞不吞要走纯判据",
        tags=["frontend", "mobile", "selection"],
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
    # ---------------- Rust：BLE 扫描结果不得按未连接的 services() 过滤 ----------------
    Case(
        name="BLE 发现：不平台级过滤、不按未连接的服务过滤",
        why="用户 2026-09-12 实测两台设备永远搜不到彼此：① 平台层用服务 UUID 过滤时，macOS 把 128 位 "
        "UUID 放在扫描响应里、Android 硬件过滤只匹配主广播包 ⇒ 永远收不到 Mac 的广播；"
        "② 拿 Peripheral::services() 复核时，它在 Android 上只有连接并 discover_services() 之后才有值，"
        "未连接恒为空 ⇒ 候选全被丢掉",
        file=TAURI / "src" / "transport" / "bluetooth.rs",
        injections=[(
            "            .start_scan(ScanFilter::default())",
            "            .start_scan(ScanFilter { services: vec![uuid(SERVICE_UUID)] })",
        )],
        cmd=cargo("test", "--lib", "scan_results_are_not_filtered_by_unconnected_services"),
        cwd=TAURI,
        expect_fail_hint="不得在**平台层**", 
        tags=["rust", "ble"],
    ),
    # ---------------- Rust：BLE 端点身份必须忽略大小写 ----------------
    Case(
        name="BLE 端点身份忽略大小写（否则每轮扫描都重拨、反复打断好链路）",
        why="同一台对端在 macOS 外设角色下是大写 UUID、在 btleplug central 下是小写 ⇒ "
        "去重比较永远不命中 ⇒ 每 13s 重拨一次、每次都替换对端 GATT server 的旧连接 ⇒ "
        "把对端拨来的好链路打断（用户真机：加好友报连接已关闭 / 对面没反应）",
        file=TAURI / "src" / "mesh" / "endpoint.rs",
        injections=[(
            "        self.address.eq_ignore_ascii_case(&other.address)",
            "        self.address == other.address",
        )],
        cmd=cargo("test", "--lib", "ble_endpoint_equality_ignores_case"),
        cwd=TAURI,
        expect_fail_hint="大小写不同的同一地址必须相等",
        tags=["rust", "ble"],
    ),
    # ---------------- Rust：好友申请丢了要能补发 ----------------
    Case(
        name="好友申请丢了要能补发（『已发送』但对方没收到）",
        why="用户 2026-09-12 真机：点加好友后对方什么都没收到，而发送方显示「已发送，等待对方确认」"
        "—— 好友申请是没有回执的定向帧，链路抖动时会静默丢失。现在发出即登记、建链补发、"
        "收到同意/拒绝后清除",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "    s.pending_out_requests\n        .lock()\n        .unwrap_or_else(|e| e.into_inner())\n        .insert(peer_id.clone());\n",
            "",
        )],
        cmd=cargo("test", "--lib", "friend_request_survives_a_dropped_link"),
        cwd=TAURI,
        expect_fail_hint="先登记",
        tags=["rust", "friend"],
    ),
    # ---------------- 前端：非聊天页不得判已读（④） ----------------
    Case(
        name="非聊天页不得判已读（④）",
        why="用户 2026-09-12 实测：在聊天界面点进设置页（整页浮层），对方发来的消息自己没看到，"
        "却被判成已读并把回执发了回去",
        file=ROOT / "src" / "stores" / "useChatStore.ts",
        injections=[(
            "if (activeConv.value !== convId || document.hidden || !app.chatVisible) return;",
            "if (activeConv.value !== convId || document.hidden) return;",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="聊天视图可见",
        tags=["frontend", "mobile"],
    ),
    # ---------------- 前端：链路标签必须由后端判定（⑤） ----------------
    Case(
        name="「蓝牙直连」不得用『没有 IP』反推（⑤）",
        why="用户 2026-09-12 实测：与 Mac 同一 Tailscale 网段的设备也被标成「蓝牙直连」——"
        "因为界面写的是 `p.ip || 蓝牙直连`；链路类型只有后端知道（Link::path_kind 由来路决定）",
        # 判据在 4.2.19 抽到 `utils/peerConnectionInfo.ts`（资料页与添加好友页共用一份），
        # 所以注入点跟着搬过去：这里模拟"按『没有 IP』反推蓝牙"的旧写法。
        file=ROOT / "src" / "utils" / "peerConnectionInfo.ts",
        injections=[(
            '  if (info.link === "bluetooth") return "peer.link.bluetooth";',
            '  if (!info.ip || true) return "peer.link.bluetooth";',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="只有后端说 bluetooth",
        tags=["frontend", "friend"],
    ),
    # ---------------- 前端：会话列表不随输入变化（⑥） ----------------
    Case(
        name="会话列表不随输入变化（⑥）",
        why="用户 2026-09-12 要求：「在上面输入，列表就不要有变化了。回车弹窗之后，在弹窗里面搜就行」",
        file=ROOT / "src" / "components" / "ConversationList.vue",
        injections=[(
            "const listConversations = computed(() => chat.conversations);",
            "const listConversations = computed(() => chat.conversations.filter((c) => c.name.includes(query.value)));",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="列表必须是全量",
        tags=["frontend", "search"],
    ),
    # ---------------- Rust：BLE 指定拨号方（两端互拨会互相打断） ----------------
    Case(
        name="BLE 指定拨号方：大 id 拨、小 id 只接受（否则镜像链路互扰）",
        why="用户 2026-09-12 真机「点加好友：发送失败，连接已关闭」：两端都跑 central+peripheral ⇒ "
        "互相拨号形成镜像链路，小 id 拨过去的连接会打断对端拨来的好链路 ⇒ 45s 无帧被看门狗拆掉",
        file=TAURI / "src" / "network" / "ble.rs",
        # 2026-09-13：判据多了 `peer_advertises` 前缀（对端不广播 ⇒ 必须我们拨，
        # 否则只做 central 的 Windows 在 id 更小时两侧都不拨）。注入改成把**整条**
        # 判据置为恒真 —— 这时候"小 id 也会去拨"必须被护栏抓到。
        injections=[("!peer_advertises || my_id > peer_id\n}", "true\n}")],
        cmd=cargo("test", "--lib", "ble_link_has_a_designated_dialer"),
        cwd=TAURI,
        expect_fail_hint="大 id 拨、小 id 只接受",
        tags=["rust", "ble"],
    ),
    # ---------------- 前端：我的在线状态 = 任一通道在跑 ----------------
    Case(
        name="在线语义：任一通道在跑 = 在线（两个都关才离线）",
        why="用户 2026-09-12 明确规则：手机蓝牙自动开启，此时即使没连 Wi-Fi 也该显示在线；"
        "两个通道都关了才是离线。用 online（只管局域网）会把这种用户标成离线",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "    let present = list.iter().any(|c| c.running);",
            "    let present = online;",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="任一通道在跑",
        tags=["frontend", "ipc"],
    ),
    # ---------------- 前端：运行状态必须"一个快照 + 一个事件"（②） ----------------
    Case(
        name="运行状态事件必须带快照、且不回发发起窗口（②）",
        why="以前 runtime-changed 是无载荷广播，每个窗口收到后都要自己重拉一半状态；"
        "而『局域网开没开』这件事在前端有两份表示（channels[lan].enabled 与 online）⇒ "
        "必然出现『外面开了、里面还是关的』。改成带 RuntimeSnapshot 的 emit_filter 之后没有了",
        file=TAURI / "src" / "state.rs",
        injections=[("emit_filter(EVENT_RUNTIME_CHANGED, snapshot, move |target| {",
                     "emit(EVENT_RUNTIME_CHANGED, snapshot); #[allow(unreachable_code)] let _ = move |target: &tauri::EventTarget| {")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="emit_filter",
        tags=["frontend", "ipc"],
    ),
    Case(
        name="前端不得再调『半份状态』的两个旧命令（②）",
        why="get_channel_status / get_network_status 是同一件事的两份来源；只要前端还能调到其中一个，"
        "就又有可能出现『两处不同步』（用户实测过：添加好友里开了局域网、设置里还显示关）",
        file=ROOT / "src" / "api" / "index.ts",
        injections=[(
            '  getRuntimeSnapshot: () => invoke<RuntimeSnapshot>("get_runtime_snapshot"),',
            '  getRuntimeSnapshot: () => invoke<RuntimeSnapshot>("get_runtime_snapshot"),\n'
            '  getChannelStatus: () => invoke<never[]>("get_channel_status"),',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="半份命令",
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
        # ⚠️ 注入的名字必须是**界面真的在用**的那个导出（当前是 `refreshRuntime`，
        #    用在 `NetworkSection.vue` / `AddFriendModal.vue`）。改成没人用的名字护栏会空转。
        injections=[("    refreshRuntime,\n", "    refreshRuntimeRenamed,\n")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="refreshRuntime",
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
        injections=[("    public static boolean send(java.lang.String, byte[]);\n", "")],
        extra_injections=[
            (
                TAURI / "gen" / "android" / "app" / "proguard-rules.pro",
                "    public static boolean send(java.lang.String, byte[]);\n",
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
        injections=[
            ('WebviewUrl::App("settings.html".into())', 'WebviewUrl::App("index.html".into())'),
            ('WebviewUrl::App("todos.html".into())', 'WebviewUrl::App("index.html".into())'),
        ],
        cmd=cargo("test", "--lib", "aux_windows_open_their_own_document"),
        cwd=TAURI,
        expect_fail_hint="index.html",
        tags=["rust", "window"],
    ),
    Case(
        name="自聊消息必须留在本地（不进 outbox / 不发 gossip）",
        why="「和自己聊天」的消息收发双方都是本机：一旦写进 outbox，那一行**永远等不到 Ack**"
        "（没有对端），会被每次心跳/建链的 flush_outbox 重发 ⇒ 「outbox 必然排空」这条不变量失效。"
        "而这在界面上完全看不出来（消息照样显示、列表照样刷新），只有库里悄悄长出一条永不消失的行。",
        file=TAURI / "src" / "commands.rs",
        injections=[
            (
                '    db::insert_message(&dbc, &rec).map_err(|e| format!("消息写入失败：{e}"))?;',
                '    db::insert_message_and_outbox(&dbc, &rec, &me, "x").map_err(|e| format!("消息写入失败：{e}"))?;',
            )
        ],
        cmd=cargo("test", "--lib", "self_chat_stays_local"),
        cwd=TAURI,
        expect_fail_hint="不得出现",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="窗口单例（打开命令不得自己查窗口存在性）",
        why="连点两下会开出第二个窗口：`build()` 的重复 label 检查在 prepare 阶段，而窗口登记进 manager "
        "是主线程创建完成之后 —— 并发调用会双双通过。必须统一走 ensure_aux_window（单例 + 串行）。"
        "2026-09-19 随 4.22.2 的 async→同步改造，锚点从 commands.rs 搬到 commands/logs.rs",
        file=TAURI / "src" / "commands" / "logs.rs",
        injections=[
            (
                "    ensure_aux_window(\n        &app,\n        crate::WINDOW_SETTINGS,",
                "    if app.get_webview_window(crate::WINDOW_SETTINGS).is_some() {\n        return Ok(());\n    }\n    ensure_aux_window(\n        &app,\n        crate::WINDOW_SETTINGS,",
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
    Case(
        name="窗口骨架（群任务窗口必须带自己的骨架类）",
        why="三个窗口共用一份骨架 CSS，靠 `<html class=\"boot-todos\">` 决定显示哪一套；"
        "类名漏了那个窗口就只剩白屏骨架（功能正常、但启动那一下很难看）",
        file=ROOT / "todos.html",
        injections=[('<html lang="zh-CN" class="boot-todos" ', '<html lang="zh-CN" ')],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="boot-todos",
        tags=["frontend", "window"],
    ),
    Case(
        name="外链窗口隔离（远端页面不得拿到任何 capability）",
        why="外链窗口加载的是**远端页面**；一旦被 capability 覆盖，第三方内容就能调用本应用的"
        "dialog/opener/event 等命令面 —— 等于把本机能力交给用户随手配置的网址",
        file=TAURI / "capabilities" / "default.json",
        injections=[('"todo-*"', '"todo-*", "link"')],
        cmd=cargo("test", "--lib", "link_window_is_not_capability_covered"),
        cwd=TAURI,
        expect_fail_hint="link",
        tags=["rust", "window", "new-guards"],
    ),
    Case(
        name="外链 URL 协议白名单（只放行 http/https）",
        why="`javascript:` / `data:` / `file:` / `tauri:` 一旦漏过，等于把『在应用 WebView 里执行脚本 / "
        "读本机文件』的能力交给一段用户粘贴的字符串",
        file=TAURI / "src" / "commands.rs",
        injections=[('!matches!(parsed.scheme(), "http" | "https")', "false")],
        cmd=cargo("test", "--lib", "external_link_rejects_non_http_schemes"),
        cwd=TAURI,
        expect_fail_hint="必须拒绝",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="外链窗口用 WebviewUrl::External 加载远端 URL",
        why="外链窗口是唯一不走本地 App 文档的窗口；退回 App(\"index.html\") 会把聊天三栏挂起来"
        "（回到一窗一入口之前的老问题），且根本加载不了外部网址",
        file=TAURI / "src" / "commands.rs",
        injections=[
            (
                "            WebviewUrl::External(build_url),",
                '            WebviewUrl::App("index.html".into()),',
            )
        ],
        cmd=cargo("test", "--lib", "aux_windows_open_their_own_document"),
        cwd=TAURI,
        # 注入后失败的是"不得再共用 index.html"那条判据（它先于 External 断言触发）。
        expect_fail_hint="index.html",
        tags=["rust", "window"],
    ),
    Case(
        name="群任务窗口绑定单一群（label 由 groupId 派生）",
        why="窗口靠**自己的 label** 找回是哪个群，所以前缀必须由常量拼出（写字面量会与前端漂移）；"
        "groupId 会拼进 label，必须先做字符集/非空校验",
        file=TAURI / "src" / "commands.rs",
        injections=[
            (
                'let label = format!("{}{group_id}", crate::WINDOW_GROUP_TODOS_PREFIX);',
                'let label = "todo".to_string();',
            )
        ],
        cmd=cargo("test", "--lib", "group_todos_window_label_derives_from_group_id"),
        cwd=TAURI,
        expect_fail_hint="WINDOW_GROUP_TODOS_PREFIX",
        tags=["rust", "window", "new-guards"],
    ),
    Case(
        name="群任务窗口不得初始化聊天事件（否则重复通知/未读/回执）",
        why="独立窗口跑聊天 store 的 init 会注册第二套后端事件监听 —— 与主窗口重复，用户会收到"
        "重复通知、未读数翻倍、群已读回执重复发（见 src/App.vue 顶部说明）",
        file=ROOT / "src" / "entries" / "todos.ts",
        injections=[
            (
                "  const chat = useChatStore();",
                "  const chat = useChatStore();\n  void chat.init();",
            )
        ],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="chat.init",
        tags=["frontend", "window"],
    ),
    Case(
        name="外链开窗必须走单飞入口（不得裸 invoke）",
        why="与设置/日志同一条：绕过 `launchAuxWindow` 就退化成『连点发多次 IPC』，"
        "而后端是单例复用 —— 第二次点击会把已打开的窗口 navigate 到同一网址，用户看到闪一下",
        file=ROOT / "src" / "layouts" / "ResponsiveLayout.vue",
        injections=[
            (
                'launchAuxWindow("link", () => api.openLinkWindow(link.url, link.name))',
                "api.openLinkWindow(link.url, link.name)",
            )
        ],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="launchAuxWindow",
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
        name="发现 socket 必须收得到广播（绑具体 IP 在 macOS 上收不到）",
        why="用户真机：Mac 与手机同一个 Wi‑Fi、都开了局域网，却「互相搜不到」；Mac 列表里安卓只闪一下。"
        "根因是发现 socket 绑定到**具体 LAN IP** —— macOS 上这种 socket 收不到 255.255.255.255 广播、"
        "也收不到组播（本机实测 0 包；绑 0.0.0.0 收得到全部），于是 Mac 发得出去（手机看得到 Mac）、"
        "却一个 announce 都收不到。收发必须是两个 socket：收的绑 0.0.0.0、发的绑具体 LAN IP",
        file=TAURI / "src" / "network" / "discovery.rs",
        injections=[(
            "pub fn discovery_recv_bind_ip() -> Ipv4Addr {\n    Ipv4Addr::UNSPECIFIED\n}",
            "pub fn discovery_recv_bind_ip() -> Ipv4Addr {\n    Ipv4Addr::LOCALHOST\n}",
        )],
        cmd=cargo("test", "--lib", "discovery_recv_socket_actually_receives_broadcast"),
        cwd=TAURI,
        expect_fail_hint="收不到 255.255.255.255 广播",
        tags=["rust", "network", "discovery"],
    ),
    Case(
        name="BLE 重连：Hello 必须换路由重新握手（不能投给旧链路）",
        why="真机 2026-09-12：Mac 反复报『对端首帧不是 Hello』/『握手超时：对端未回 Hello』。"
        "BLE 上同一个 central 地址在重连时复用，旧连接的链路任务可能还没清理 —— 新连接的 Hello "
        "一旦被投给旧链路的管道，旧链路写的是旧连接 ⇒ 新连接永远收不到 Hello 回应。"
        "用户侧表现：蓝牙时好时坏、加好友没反应",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[("    if !has_route || frame_is_hello {", "    if false {")],
        # ⚠️ BLE 代码在 `--features bluetooth` 下才编译，测试必须带这个 feature
        cmd=cargo("test", "--lib", "--features", "bluetooth", "reconnect_hello_must_not_go_to_the_stale_route"),
        cwd=TAURI,
        expect_fail_hint="有活路由 + 收到 Hello",
        tags=["rust", "ble", "network"],
        # 该单测与 `peripheral_route_action` 一样只存在于**有外设角色**的平台
        # （`cfg(any(target_os = "macos", target_os = "android"))`）。
        # Windows 这一轮只做 central（ADR-0015 §7.9），所以此处必须跳过而不是假失败。
        platforms=("darwin", "linux"),
    ),
    Case(
        name="BLE 握手失败必须说出『收到的是什么』",
        why="同一轮真机排查里，日志只有一句『对端首帧不是 Hello』，完全无法区分"
        "『对端重连时把旧链路的帧发了过来』『对端状态机没重置』『对面不是 Gosslan』"
        "⇒ 只能靠猜。central 与外设**两侧**的错误都必须带上收到的类型名，"
        "且类型名要走 `Message::wire_kind()`（与 serde tag 同一份事实来源：手写 match "
        "漏一个变体就会打出错的类型名，比没有日志更坏）",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "最后一帧 type={}",
            "最后一帧（这里曾经不带类型名）",
        )],
        cmd=cargo("test", "--lib", "peripheral_reconnect_hello_replaces_the_stale_route"),
        cwd=TAURI,
        expect_fail_hint="最后一帧的类型",
        tags=["rust", "ble", "diagnostics"],
    ),
    Case(
        name="掉线节点：留在发现列表里，但不得算「在线」",
        why="真机 2026-09-12（只开蓝牙）：手机能看到 Mac（已发现未建联），Mac 里安卓什么都不显示 —— "
        "根因是链路一断就删节点条目，而 BLE 上「连上→被对端退让→断开」是常态，"
        "「添加好友」列表里只闪一下、用户点不到。反向的坑是复核抓到过的 High 缺陷："
        "若保留条目却仍按「在 peers 表里 = 在线」判定，就变成「连过又掉线 ⇒ 永久在线」。"
        "两件事必须一起成立：条目保留 + 在线看 last_seen 新鲜度",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "        f.online = friend_is_online(last_seen, now, active_links.contains(&f.device_id));",
            "        f.online = peers.contains_key(&f.device_id) || active_links.contains(&f.device_id);",
        )],
        cmd=cargo("test", "--lib", "offline_peer_stays_listed_but_is_not_online"),
        cwd=TAURI,
        expect_fail_hint="friend_is_online",
        tags=["rust", "presence", "network"],
    ),
    Case(
        name="BLE 拨号退避绝不指数增长到分钟级（否则好友申请等几分钟）",
        why="真机 2026-09-13：好友申请等了 5～6 分钟才到。根因之一是退避被锁到分钟级："
        "BLE 上「连过去被拒」是常态，每次失败把一个**稳定地址**推进下一档，而"
        "「小 id 只接受」又让只有一侧会拨 ⇒ 唯一的拨号通道被锁死。"
        "现在的实现是「前 3 次不退避，之后 5s→10s→20s 封顶」。"
        "⚠️ 2026-09-16 两处更新：① 命令指向新测试名（旧测试已随「缓增 + 封顶」重设计改名）；"
        "② 注入改为**去掉封顶**而不是改 `MAX_MS` 的值 —— 实现里 `step.min(2)` 已经把增长压到"
        "3 档，单改 `MAX_MS` 到 600_000 也到不了分钟级，那样的注入是**空转**的"
        "（改坏了测试照样通过）。这条用例的前一版正因为锚点写死在旧值 60_000 而失效。",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "    (BASE_MS << step.min(2)).min(MAX_MS)",
            "    BASE_MS << step",
        )],
        cmd=cargo("test", "--lib", "--features", "bluetooth", "dial_backoff_does_not_starve_retries"),
        cwd=TAURI,
        expect_fail_hint="必须封顶",
        tags=["rust", "ble", "backoff"],
    ),
    Case(
        name="好友同意回执必须有界补发（否则一次丢帧 = 永久单边好友）",
        why="真机 2026-09-13：Android 点「接受」、Android 侧好友已出现，但 Mac 端状态一直没同步 —— "
        "FriendAccept 只发一次且**没有回执**，链路抖动时静默丢失就永不重发。"
        "修法是窗口 + 次数 + 间隔的有界补发；策略写反了要么永不补发、要么疯狂打扰对端，"
        "所以用真值表钉住",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "    if now - issued > window_ms || attempts >= max_attempts {",
            "    if false {",
        )],
        cmd=cargo("test", "--lib", "friend_accept_flush_is_bounded_and_spaced"),
        cwd=TAURI,
        expect_fail_hint="GiveUp",
        tags=["rust", "friend", "reliability"],
    ),
    Case(
        name="BLE 握手必须容忍前导帧（否则残留帧把链路全部打死）",
        why="真机 2026-09-13：Mac 日志反复 `[GATT] 已就绪 → [DISCONNECT] 对端首帧不是 Hello"
        "（收到 chat_message）` ⇒ 链路永久建不起来（双方各自重拨、互相打断）。"
        "Android 的 notify 按 central 地址投递：上一条链路的待发帧会落在新连接上，"
        "而「首帧必须是 Hello」这条旧判据会把本来能建起来的链路全部打死",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "    } else if dropped >= MAX_HANDSHAKE_PREAMBLE_FRAMES {",
            "    } else if true {",
        )],
        cmd=cargo("test", "--lib", "--features", "bluetooth", "handshake_tolerates_leading_non_hello_frames_but_is_bounded"),
        cwd=TAURI,
        expect_fail_hint="额度过小",
        tags=["rust", "ble", "handshake"],
    ),
    Case(
        name="BLE 拨号去重 + 失败断开（否则叠连接把通知投给没人读的那条）",
        why="真机 2026-09-13：Mac 侧反复 `[GATT] 已就绪 → 握手超时：对端未回 Hello`，"
        "而安卓侧 notify 全部成功。根因是同一对端叠了多条连接（扫描 10s 一轮 vs 握手 10s），"
        "失败又从不 disconnect ⇒ 幽灵连接 + 多个通知流订阅，通知被投给没人读的那条。"
        "这条护栏盯：DialGuard 去重、复用前先断开、失败显式断开、分片级统计存在",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            '    let Some(_dial_guard) = crate::state::DialGuard::try_acquire(&state, format!("ble:{ble_id}"))',
            "    let Some(_dial_guard): Option<crate::state::DialGuard> = None",
        )],
        cmd=cargo("test", "--lib", "ble_dial_is_deduplicated_and_disconnects_on_failure"),
        cwd=TAURI,
        expect_fail_hint="在途去重",
        tags=["rust", "ble", "connection"],
    ),
    Case(
        name="Android 外设通知必须节流（连发会丢片，帧永远拼不完整）",
        why="真机 2026-09-13 的算术证据：Mac 侧 `[FRAG] 收到通知 38 条 / 747 字节`，"
        "而 742 字节的帧在 MTU=23（每片 14 字节载荷）下需要 53 片 ⇒ 丢了 15 片 ⇒ 永远拼不出"
        "完整帧，表现是「安卓收到了好友申请并加上了，Mac 什么都没发生」。"
        "notifyCharacteristicChanged 连发会被 Android 协议栈丢包，必须每片留一个连接间隔",
        file=TAURI / "src" / "transport" / "ble_android.rs",
        injections=[(
            "                tokio::time::sleep(NOTIFY_CHUNK_INTERVAL).await;",
            "                // 非空转验证：把这句去掉",
        )],
        cmd=cargo("test", "--lib", "android_peripheral_paces_its_notifications"),
        cwd=TAURI,
        expect_fail_hint="必须真的 sleep",
        tags=["rust", "ble", "android"],
    ),
    Case(
        name="BLE 文件分块必须能被分片层发出去（否则一帧打死链路）",
        why="真机 2026-09-13：大图两边都显示成功、对方列表里却没有。日志证据 "
        "`[SEND] 写失败 ⇒ 结束该链路写循环 … type=file_chunk` + 接收侧反复 "
        "`接收文件初始化失败: 重复的文件传输` → `file_reject`。根因：一对一文件流每块 "
        "256 KiB，在 MTU=23 上要 18725 片 > 上限 8192 ⇒ fragment() 返回 None ⇒ 拆链路；"
        "重复 offer 又被 reject ⇒ 对端停止重试 ⇒ 文件永远到不了",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            '    if path == crate::mesh::PathKind::Bluetooth.as_str() {',
            "    if false {",
        )],
        cmd=cargo("test", "--lib", "--features", "bluetooth", "ble_file_chunk_actually_fits_the_ble_fragment_layer"),
        cwd=TAURI,
        expect_fail_hint="BLE 分块大小必须能被分片",
        tags=["rust", "ble", "file"],
    ),
    Case(
        name="连接信息按链路类型显示（蓝牙不显示 IP）",
        why="用户 2026-09-13：蓝牙链路原来也显示一行『IP 地址：—』，设备类型还直接显示 "
        "desktop/mobile 英文原值。不同链路该说不同的事实：蓝牙说『蓝牙直连（近距离）』且"
        "**不显示 IP**；局域网/跨网段给 ip:port；中继只说跳数（没有直连地址就不许编一个）",
        file=ROOT / "src" / "utils" / "peerConnectionInfo.ts",
        injections=[(
            '  if (info.link === "bluetooth") return false;',
            "  if (false) return false;",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="蓝牙没有 IP 概念",
        tags=["frontend", "peer", "display"],
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
        "真正的锚点被吞掉（不报错、不影响功能，只有结构检查能拦住）。"
        "⚠️ 2026-09-16 修正：本用例原先拿 `npm run version:check` 当命令，而那条命令的 ①②"
        "（版本必须 ≥ 未发布提交要求的、每个提交要有自洽的 Version-Bump 声明）在**攒提交期间"
        "本来就该是红的** ⇒ 本用例永远进不了『恢复即 PASS』、被判成护栏失效。"
        "改成只跑结构检查的 `version:changelog`：结构是结构、记账是记账。",
        file=ROOT / "CHANGELOG.md",
        injections=[("## [Unreleased]\n", "## [unreleased]\n")],
        cmd=npm("run", "version:changelog"),
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
    # ---------------- 测试清单守卫（挡住「测试静默不跑」） ----------------
    # 这两条守的是 `scripts/check-test-manifest.mjs`。它拦的是一类**没有信号**的故障：
    # 测试明明写着，却根本没被执行，而所有命令都返回 0。
    Case(
        name="测试清单：基线里的用例没跑必须报出来（漏 --features 就靠它）",
        why="`bluetooth` 是**非默认** feature（`src-tauri/Cargo.toml` 的 [features]）。漏掉 "
            "`--features bluetooth` ⇒ BLE 模块根本不编译、那批用例连同被测代码一起消失，"
            "而 `cargo test` **全绿**。本项目真实踩过：BLE 连续四个版本（4.18.7→4.18.10）"
            "边走边修，而这个子系统恰恰是「忘了加 feature 就静默不测」的那个。"
            "清单守卫比对「基线名单 ⋈ 实际 --list」，缺名即红。",
        file=TAURI / "test-baseline.macos.txt",
        injections=[(
            "transport::bluetooth_peripheral::tests::central_mtu_clamps_and_never_returns_zero",
            "transport::bluetooth_peripheral::tests::central_mtu_clamps_and_never_returns_zero\n"
            "transport::bluetooth_peripheral::tests::a_test_that_no_longer_runs",
        )],
        cmd=["node", "scripts/check-test-manifest.mjs", "--only", "rust"],
        cwd=ROOT,
        expect_fail_hint="静默跳过",
        tags=["manifest", "ble"],
        # 基线按平台分文件（macOS 外设 / Windows 外设是互斥的 #[cfg]）。
        # Phase 2 打通 Windows 测试通道后，这里补一条 test-baseline.windows.txt 的对应用例。
        platforms=("darwin",),
    ),
    Case(
        name="测试清单：磁盘上的测试文件没登记进 package.json 必须报出来",
        why="`npm test` 的脚本里是**手工枚举**的 48 条路径。新增一个 .test.ts 时若忘了把它加进"
            "那串字符串，新文件不会被执行，而 `npm test` 依然**全绿** —— 与「漏 --features」"
            "是完全同类的东西：退出码 0 的空转。",
        file=ROOT / "package.json",
        injections=[(
            "src/utils/selfChat.test.ts src/utils/todos.test.ts ",
            "src/utils/selfChat.test.ts ",
        )],
        cmd=["node", "scripts/check-test-manifest.mjs", "--only", "frontend"],
        cwd=ROOT,
        expect_fail_hint="未登记",
        tags=["manifest", "frontend"],
    ),
    # ---------------- 不变量例外登记（挡住「照文档误修」） ----------------
    # 守的是 `scripts/check-invariant-exceptions.mjs`：代码侧的 `INV-EXCEPTION:` 标记
    # 与 `docs/protocol-invariants.md` §22 登记区必须**双向**一致。
    Case(
        name="不变量例外：代码标了但文档没登记必须报出来（否则会被照文档误修）",
        why="AI 的必读清单（AI_ENGINEERING_INDEX.md）只指向 protocol-invariants.md 与本文件。"
            "一段**正当**的例外若只写在实现旁边、没写进那份文档，读文档的人就会把它当 bug 修掉 ——"
            "「和自己聊天」正是这种：它不进 outbox，「修」成进 outbox 会让那行永远等不到 Ack、"
            "被 flush_outbox 每次心跳重发，把「outbox 必然排空」真的破掉。",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "// INV-EXCEPTION: INV-P03, INV-P04 — 自聊收发双方都是本机",
            "// INV-EXCEPTION: INV-P03, INV-P04, INV-P20 — 自聊收发双方都是本机",
        )],
        cmd=["node", "scripts/check-invariant-exceptions.mjs"],
        cwd=ROOT,
        expect_fail_hint="没登记进不变量文档",
        tags=["invariant", "new-guards"],
    ),
    Case(
        name="不变量例外：文档登记了但代码标记没了必须报出来（否则文档在说谎）",
        why="登记的例外如果代码里已无人声明，要么这段代码的例外成了隐藏事实，"
            "要么例外早已不存在而登记忘了撤 —— 两种都会让文档变得不可信，"
            "而「文档不可信」比「没有文档」更糟：它会让所有不变量一起失效。",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "// INV-EXCEPTION: INV-P03, INV-P04 — 自聊收发双方都是本机，没有对端可等 Ack：",
            "// （标记被删）",
        )],
        cmd=["node", "scripts/check-invariant-exceptions.mjs"],
        cwd=ROOT,
        expect_fail_hint="找不到对应标记",
        tags=["invariant", "new-guards"],
    ),
    Case(
        name="不变量例外：登记里出现文档未定义的 id（笔误会登记出一条不存在的例外）",
        why="例外表里的 id 打错一个数字，就等于凭空登记了一条不存在的例外。"
            "这类笔误不会自己冒出来，只会在某次「照文档排查」时把人带沟里。",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "// INV-EXCEPTION: INV-P03, INV-P04 — 自聊收发双方都是本机",
            "// INV-EXCEPTION: INV-P03, INV-P04, INV-P99 — 自聊收发双方都是本机",
        )],
        # 同时改文档：把这个不存在的 id 也写进登记区，才能把「笔误」单独隔离出来
        # （否则会先以「代码标了没登记」失败，证明不了笔误这条判据本身有效）。
        extra_injections=[(
            ROOT / "docs" / "protocol-invariants.md",
            "<!-- END EXCEPTION REGISTRY -->",
            "| INV-P99 | 探针 | 无 | 探针 |\n\n<!-- END EXCEPTION REGISTRY -->",
        )],
        cmd=["node", "scripts/check-invariant-exceptions.mjs"],
        cwd=ROOT,
        expect_fail_hint="文档未定义",
        tags=["invariant", "new-guards"],
    ),
    # ---------------- BLE 常量/换算的单一事实来源 ----------------
    # 守的是 `scripts/check-ble-constants.mjs`。背景：CHANGELOG 4.18.7→4.18.10
    # **连着四个版本**修同一个分片预算问题 —— 根因不是某一行写错，而是同一个概念
    # 在多个地方各算一遍（macOS 外设侧自己留了 `const DEFAULT = 20` / `const MAX = 512`）。
    Case(
        name="BLE 常量只有一个家：重复定义必须报出来",
        why="4.18.7→4.18.10 那四个版本的病根是「同一个概念多处各算一遍」。"
        "2026-09-16 把常量与换算收敛到 `transport/ble_framing.rs` 一处；"
        "本用例把 `BLE_DEFAULT_MTU` 重新定义回 `bluetooth.rs`，必须被报出来 —— "
        "否则下一次漂移会以完全相同的方式发生（数值恰好一致 ⇒ 不报错、只在真机上表现为"
        "「某台设备收不到消息」）。",
        file=TAURI / "src" / "transport" / "bluetooth.rs",
        injections=[(
            "    /// 把协商到的 MTU 换算成**分片有效载荷上限**（central 侧）。",
            "    /// BLE 未协商时的默认 ATT MTU。\n"
            "    pub const BLE_DEFAULT_MTU: u16 = 23;\n\n"
            "    /// 把协商到的 MTU 换算成**分片有效载荷上限**（central 侧）。",
        )],
        cmd=["node", "scripts/check-ble-constants.mjs"],
        cwd=ROOT,
        expect_fail_hint="有 2 处定义",
        tags=["ble", "new-guards"],
    ),
    Case(
        name="BLE 常量只有一个家：匿名常量重述必须报出来（4.18.x 的原始形态）",
        why="这条注入的就是 2026-09-13 真实埋下的那两行：`const DEFAULT: usize = 20` 与 "
        "`const MAX: usize = 512` —— 名字没有信息量、靠注释解释语义。它们让 macOS 外设侧"
        "成了 `ble_framing` 那份换算的**第二份实现**（当时 Windows 走共享函数、macOS 不走，"
        "于是文档里那句「外设侧用的是同一个函数」只对 Windows 成立）。"
        "判据刻意**不扫裸数字**：`const CONNECT_ATTEMPTS = 3` 这类无关常量不许被误伤，"
        "所以规则是「名字按 `_` 分词命中概念词或语义空名」**且**「值恰好是受保护字面量」。",
        file=TAURI / "src" / "transport" / "bluetooth_peripheral.rs",
        injections=[(
            "pub fn central_payload_mtu(max_update_value_length: usize) -> usize {\n"
            "    ble_framing::notify_payload_budget(max_update_value_length)\n"
            "}",
            "pub fn central_payload_mtu(max_update_value_length: usize) -> usize {\n"
            "    const DEFAULT: usize = 20;\n"
            "    const MAX: usize = 512;\n"
            "    let min = ble_framing::BLE_CHUNK_HEADER_LEN + 1;\n"
            "    if max_update_value_length < min {\n"
            "        DEFAULT\n"
            "    } else {\n"
            "        max_update_value_length.min(MAX)\n"
            "    }\n"
            "}",
        )],
        cmd=["node", "scripts/check-ble-constants.mjs"],
        cwd=ROOT,
        expect_fail_hint="第二份事实来源",
        tags=["ble", "new-guards"],
    ),
    Case(
        name="BLE 两侧载荷预算必须能互相推回去（外设侧不再减 ATT 头）",
        why="常量收敛只保证「只有一份」，不保证「这一份是对的」。本用例注入 central 侧的正确"
        "换算被外设侧**又减了一次 ATT 头**（4.18.7 的形态），"
        "`both_sides_agree_on_the_same_link_budget` 必须红。"
        "这条交叉校验此前**只存在于 Windows 专属**的 "
        "`peripheral_and_central_agree_on_payload_budget`，而 macOS 恰恰是当时唯一没走共享"
        "换算的一侧，所以缺口一直没被发现。现在两侧共用同一个测试。",
        file=TAURI / "src" / "transport" / "ble_framing.rs",
        injections=[(
            "        max_update_value_length.min(GATT_MAX_ATTR_LEN)\n    }",
            "        (max_update_value_length - ATT_HEADER_LEN).min(GATT_MAX_ATTR_LEN)\n    }",
        )],
        cmd=cargo("test", "--lib", "both_sides_agree_on_the_same_link_budget"),
        cwd=TAURI,
        expect_fail_hint="必须原样返回",
        tags=["rust", "ble", "new-guards"],
    ),
    Case(
        name="BLE 常量只有一个家：外设平台自己算一遍必须报出来（Android 2026-09-16 的形态）",
        why="判据 A/B 都只盯「定义」，而真实漏掉的那处是**把换算内联进平台实现**："
        "Android 的 `payload_mtu` 自己写 `if (1..=512).contains(&v) { v } else { 20 }` —— "
        "既没重新定义常量（逃过 B），也不是「重新实现具名函数」（逃过 A）。"
        "它还有真 bug：`1..=6` 这类**装不下分片头**的值被放行 ⇒ `fragment` 拒绝一切 ⇒ "
        "整条链路发不出消息，而日志只说「帧无法分片」。这条注入就是把它改回原样。"
        "发现它的正是 Phase 4 引入的 Android `cargo check` —— 它不跑测试，"
        "所以比 `cargo test` 更容易看见「只在某一平台编译的重复」。",
        file=TAURI / "src" / "transport" / "ble_android.rs",
        injections=[(
            "        let raw = call_static_int(\"payloadMtu\", central).unwrap_or(0);\n"
            "        ble_framing::notify_payload_budget(usize::try_from(raw).unwrap_or(0))",
            "        call_static_int(\"payloadMtu\", central)\n"
            "            .map(|v| if (1..=512).contains(&v) { v as usize } else { 20 })\n"
            "            .unwrap_or(20)",
        )],
        cmd=["node", "scripts/check-ble-constants.mjs"],
        cwd=ROOT,
        expect_fail_hint="找不到对规范换算的调用",
        tags=["ble", "android", "new-guards"],
    ),
    # ---------------- 领域图（docs/domains.data.mjs + check-domain-map.mjs） ----------------
    # 地图错了比没有地图更危险 —— 它会被当成事实执行。下面三条守的是"地图不许说谎"里
    # **机器能守**的那部分（`activeHome` 是否属实只能靠人诚实 + 台账里的 file:line 证据）。
    Case(
        name="领域图：enforce 不能开在还有第二个家的领域（边界收口完成一个，打开一个）",
        why="`enforce: true` 表示「该领域边界已是事实、可由机器守住」（例如禁止跨领域直接引用）。"
        "若它还有第二个家（迁移中）就把闸门打开，第一天就会全红 —— 而红门禁会催生绕过，"
        "门禁一旦被绕过一次就永久失效（本项目铁律：第一版门禁必须全绿）。"
        "本用例把 presence 的 enforce 改成 true（它还有未接线的第二个家 discovery/），必须被拦下。"
        "这条把「边界收口完成一个，打开一个」从口号变成机器判定 —— 也是 Phase 6 的前置。",
        file=ROOT / "docs" / "domains.data.mjs",
        injections=[(
            '      secondHome: "src-tauri/src/discovery",\n'
            '      secondHomeStatus: "未接线",\n'
            '      enforce: false,',
            '      secondHome: "src-tauri/src/discovery",\n'
            '      secondHomeStatus: "未接线",\n'
            '      enforce: true,',
        )],
        cmd=["node", "scripts/check-domain-map.mjs"],
        cwd=ROOT,
        expect_fail_hint="还有第二个家",
        tags=["domain", "new-guards"],
    ),
    Case(
        name="领域图：一个文件不许被两个领域认领",
        why="一个文件被两个领域认领 ⇒ 改它时不知道该守谁的规则 ⇒ 规则的**适用范围**本身成了歧义。"
        "本用例把 transport 的活路径文件塞进 presence 的 paths，必须被拦下。",
        file=ROOT / "docs" / "domains.data.mjs",
        injections=[(
            '      paths: [\n'
            '        "src-tauri/src/network/discovery.rs", // 旧家（活）\n'
            '        "src-tauri/src/discovery", // 新家（未接线）\n'
            "      ],\n",
            '      paths: [\n'
            '        "src-tauri/src/network/discovery.rs", // 旧家（活）\n'
            '        "src-tauri/src/discovery", // 新家（未接线）\n'
            '        "src-tauri/src/network/transport.rs", // 注入：该文件已被 transport 认领\n'
            "      ],\n",
        )],
        cmd=["node", "scripts/check-domain-map.mjs"],
        cwd=ROOT,
        expect_fail_hint="被多个领域认领",
        tags=["domain", "new-guards"],
    ),
    Case(
        name="领域图：不许有文件既没归属也没列进 unmapped（无主之地最容易出跨界 bug）",
        why="「没被提到」与「确认不属于任何领域」是两回事：前者是无主之地（谁改都不守规则），"
        "后者是经过思考的豁免。本用例把 style.css 从 unmapped 里删掉（它不会被任何领域认领），"
        "必须报出来 —— 强制那条豁免是**显式**的。",
        file=ROOT / "docs" / "domains.data.mjs",
        injections=[('    ["src/style.css", "全局样式（令牌化设计体系的落点）"],\n', "")],
        cmd=["node", "scripts/check-domain-map.mjs"],
        cwd=ROOT,
        expect_fail_hint="既没被领域认领",
        tags=["domain", "new-guards"],
    ),
    # ---------------- 领域依赖方向（docs/domains.data.mjs + check-domain-deps.mjs） ----------------
    # 跟上面三条互补：check-domain-map.mjs 守图的形式（路径/不重叠/enforce），
    # check-domain-deps.mjs 守图的依赖方向（每条 use crate::xxx 是否落在 consumes 里）。
    # 地图与依赖两套都过的领域,才算"自洽";只过一套⇒要么补 consumes 要么删 use。
    # 教训（Phase 5b）：consumes 字段是该领域的**边界协议**,有了它"新增一条 use"不再是
    # 静默演化,而是有闸门的扩展;不写 consumes 等于写"我不关心边界会怎样" —— 守门不让过。
    Case(
        name="领域依赖方向：跨域 use 不在 consumes 中 → FAIL（守住「依赖是声明出来的」）",
        why="messaging 域的 gossip_engine.rs 当前 use 了 crypto::Identity 与 protocol::*,"
        "对 platform 域毫无依赖。本用例临时给它塞一行 `use crate::menu;`（platform 域内"
        "结构体）,守门必须报「不在 consumes 中」并定位到 file:line。否则「新增一条跨域"
        "依赖」就是静默演化 —— 等再有人 PR 又删掉,守门仍全绿,边界已经被改写却没人知道。"
        "修法：① 真有需求就把 platform 加进 messaging 的 consumes;② 删掉这条临时 use。",
        file=ROOT / "src-tauri/src/gossip_engine.rs",
        injections=[(
            'use crate::crypto::Identity;\n'
            'use crate::protocol::{GossipEnvelope, GossipKind};',
            'use crate::crypto::Identity;\n'
            'use crate::protocol::{GossipEnvelope, GossipKind};\n\n'
            '// TEMP-NON-VACUUM-TEST(messaging→platform):必须被 check-domain-deps.mjs 拦下。\n'
            'use crate::menu;',
        )],
        cmd=["node", "scripts/check-domain-deps.mjs"],
        cwd=ROOT,
        expect_fail_hint="想依赖「platform」域",
        tags=["domain-deps", "new-guards"],
    ),
    Case(
        name="领域依赖方向：consumes 被误删成空 → 已有 use 立刻穿帮",
        why="messaging 域的 gossip_engine.rs 通过 `use crate::crypto::Identity;` 依赖 identity 域。"
        "本用例把 messaging 的 consumes 从 `[\"identity\"]` 改成 `[]`,守门必须报"
        "「messaging 想依赖 identity」 —— 因为「没声明」与「声明了不需要」是两回事,前者意味着"
        "依赖边界被悄悄擦掉了。判定 `consumes: []` 跟 `enforce: false` 是两套独立的开关:enforce"
        "控制图的形式,consumes 控制图的内容。",
        file=ROOT / "docs" / "domains.data.mjs",
        injections=[(
            'consumes: ["identity"], // gossip_engine.rs 用 crypto::Identity（生产代码）',
            'consumes: [], // TEMP-NON-VACUUM-TEST(messaging):该声明被误删,守门必须报',
        )],
        cmd=["node", "scripts/check-domain-deps.mjs"],
        cwd=ROOT,
        expect_fail_hint="想依赖「identity」域",
        tags=["domain-deps", "new-guards"],
    ),
    Case(
        name="领域依赖方向：consumes 引用了不存在的领域 → FAIL（typo 第一天就该红）",
        why="`consumes: [\"identity\"]` 写错成 `[\"identtity\"]` 这类 typo,在守门放松对"
        "consumes 字段自身合法性做检查时会**完全无害**地通过 —— 直到真正新增一条 use 触发"
        "「不存在的域」才被察觉,届时已经离 typo 隔了 N 个 PR。本用例在 transport 的 consumes"
        "中临时塞一个不存在的 id,直接验证判据 H 必红。修法:把不存在的 id 改回真名。",
        file=ROOT / "docs" / "domains.data.mjs",
        injections=[(
            '        "platform", // transport/ble_android.rs 用 jni_method::kotlin_method\n',
            '        "platform", // transport/ble_android.rs 用 jni_method::kotlin_method\n'
            '        "identtity_typo_will_fail", // TEMP-NON-VACUUM-TEST(transport):错字,守门必报\n',
        )],
        cmd=["node", "scripts/check-domain-deps.mjs"],
        cwd=ROOT,
        expect_fail_hint="引用了不存在的领域",
        tags=["domain-deps", "new-guards"],
    ),
    # ---------------- Change Budget(check-change-budget.mjs + fixture) ----------------
    # 守门读真实 git 历史,没法"改坏源文件"来验证 —— 所以脚本留了 --from-json 测试接缝,
    # 用 fixture 喂数据。fixture 的默认状态是全 PASS(每条判定路径都走到),下面四条用例
    # 各自破坏一个条件来验证对应判据会红。fixture 本身提交进仓库,是可以 review 的测试数据。
    Case(
        name="Change Budget:L2 改动丢了 [plan] 标记 → FAIL",
        why="超 L1(≤5 文件)但 ≤L2(≤10 文件)的改动,要求 commit message 带 [plan] 说明改动计划 ——"
        "『中改动必须被声明』是 Change Budget 的核心语义。fixture 里 a000002(8 文件/412 行)默认带"
        " [plan: 拆成三步…];本用例把 [plan] 从 message 里删掉,守门必须报「没有 [plan] 标记」。",
        file=ROOT / "scripts" / "fixtures" / "change-budget.json",
        injections=[(
            '"message": "feat(ui): 重构设置面板 [plan: 拆成三步 —— 先抽 store,再拆视图,最后迁 API]",',
            '"message": "feat(ui): 重构设置面板",',
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="没有 [plan] 标记",
        tags=["change-budget", "new-guards"],
    ),
    Case(
        name="Change Budget:L3 改动(含敏感文件)丢了 [impact] 标记 → FAIL",
        why="碰 protocol.rs / crypto.rs / schema.sql 的改动**无论多小**都是 L3(一错就是安全/全库数据问题),"
        "必须有 Impact Report 的最小形态 [impact] 标记。fixture 里 a000003 只改 2 个文件,但因碰了"
        " protocol.rs 直接 L3;本用例删掉 [impact],守门必须红 —— 证明『敏感文件不豁免于规模』。",
        file=ROOT / "scripts" / "fixtures" / "change-budget.json",
        injections=[(
            '"message": "refactor(protocol): 线格式 v2 [impact: 见 docs/protocol-invariants.md 新增小节;两侧同步升级;505 用例全绿]",',
            '"message": "refactor(protocol): 线格式 v2",',
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="没有 [impact] 标记",
        tags=["change-budget", "new-guards"],
    ),
    Case(
        name="Change Budget:同一领域连续 3 次 fix → FAIL(重复犯案检测器)",
        why="4.18.7→4.18.10 连着四个版本修同一个 BLE 分片问题,每个补丁都很小但它们在互相修 ——"
        "『改完这个冒出那个』的特征不是 diff 大,而是同一领域反复被打补丁。fixture 窗口里 transport"
        " 已有 2 次 fix(阈值 3);本用例注入第 3 条 transport fix,守门必须报「出现了 3 次」并提示"
        "先补不变量/收敛单一事实来源。",
        file=ROOT / "scripts" / "fixtures" / "change-budget.json",
        injections=[(
            '"message": "fix(ble): 写入失败日志补帧长",\n      "files": [{ "path": "src-tauri/src/transport/bluetooth.rs", "add": 24, "del": 2 }]\n    }\n  ]',
            '"message": "fix(ble): 写入失败日志补帧长",\n      "files": [{ "path": "src-tauri/src/transport/bluetooth.rs", "add": 24, "del": 2 }]\n    },\n'
            '    {\n      "sha": "b000003",\n      "message": "fix(ble): 第三次打补丁",\n      "files": [{ "path": "src-tauri/src/transport/tcp.rs", "add": 5, "del": 1 }]\n    }\n  ]',
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="出现了 3 次",
        tags=["change-budget", "new-guards"],
    ),
    Case(
        name="Change Budget:chore(release) 换成普通类型 → 版本白名单失效 → FAIL",
        why="每次发版固定动 5 个版本文件(package.json/Cargo.toml/Cargo.lock/tauri.conf.json/package-lock.json),"
        "白名单只在 message 以 chore(release) 开头时生效。fixture 里 a000004(package-lock +250 行等)默认豁免;"
        "本用例把 message 改成 feat(release) —— 白名单立即失效,347 行计入 ⇒ 超 L1 且无 [plan] ⇒ FAIL。"
        "证明『豁免是声明出来的,不是永远免检』。",
        file=ROOT / "scripts" / "fixtures" / "change-budget.json",
        injections=[(
            '"message": "chore(release): v4.19.0",',
            '"message": "feat(release): v4.19.0",',
        )],
        cmd=["node", "scripts/check-change-budget.mjs", "--from-json", "scripts/fixtures/change-budget.json"],
        cwd=ROOT,
        expect_fail_hint="没有 [plan] 标记",
        tags=["change-budget", "new-guards"],
    ),
]



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


def run(cmd: list[str], cwd: Path, timeout: int = 900) -> tuple[int, str]:
    env = dict(os.environ)
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

        code, out = run(case.cmd, case.cwd)
        if code == 0:
            return False, "改坏之后测试**仍然通过** ⇒ 这条护栏是空转的（没在守东西）"
        if case.expect_fail_hint and case.expect_fail_hint not in out:
            detail = f"（失败输出里没看到 `{case.expect_fail_hint}`，请确认是这条判据报的）"

        for path, original in originals:  # 先恢复，再验证恢复后确实通过
            write_source(path, original)
        _CURRENT = []
        code2, out2 = run(case.cmd, case.cwd)
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
