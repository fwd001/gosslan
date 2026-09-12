import { acceptHMRUpdate, defineStore } from "pinia";
import { computed, ref } from "vue";
import { api, type SettingsChanged } from "@/api";
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification";
import { applyTheme } from "@/utils/color";
import { reportError } from "@/utils/errors";
import { debounce } from "@/utils/defer";
import {
  APPEARANCE_STORAGE_KEY,
  LEGACY_DARK_STORAGE_KEY,
  isAppearanceMode,
  readStoredAppearance,
  resolveDark,
  type AppearanceMode,
} from "@/utils/appearance";
import { DEFAULT_CHAT_STYLE, fontPx, parsePeerStyle, type ChatStyleConfig } from "@/utils/chatStyle";
import {
  t,
  applyPreference,
  currentLocale,
  currentPreference,
  isLanguagePreference,
  type LanguagePreference,
} from "@/i18n";
import { isMac } from "@/utils/platform";
import type { AppSettings, ChannelStatus, DeviceInfo, InterfaceInfo, RelayPolicy, RuntimeSnapshot } from "@/types";

export type { AppearanceMode };

const THEME_KEY = "gosslan.themeColor";
const FONT_KEY = "gosslan.fontFamily";
const CHAT_STYLE_KEY = "gosslan.chatStyle";

/** 从 localStorage 读取聊天样式（启动先本地，后端返回后覆盖）。 */
function loadLocalChatStyle(): ChatStyleConfig {
  try {
    const raw = localStorage.getItem(CHAT_STYLE_KEY);
    if (raw) return parsePeerStyle(raw);
  } catch {
    /* ignore */
  }
  return { ...DEFAULT_CHAT_STYLE };
}

export const useAppStore = defineStore("app", () => {
  const device = ref<DeviceInfo | null>(null);
  const interfaces = ref<InterfaceInfo[]>([]);
  const online = ref(false);
  const boundIp = ref<string | null>(null);
  /** 上次选择的网卡（持久化偏好；离线时作为设置页默认项）。 */
  const preferredIp = ref<string | null>(null);
  const shareDir = ref<string | null>(null);

  /** 外观模式（用户意图）。跟随系统时 dark 由系统偏好解析而来。 */
  const appearance = ref<AppearanceMode>(readStoredAppearance(localStorage));
  /**
   * 系统当前是否为深色。**在 store 创建时就同步取值**，而不是等到 init —— 
   * 这样第一次 applyDarkNow()（可能发生在 init 之前）就已经拿到正确的系统值。
   */
  const systemDark = ref<boolean>(
    typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
      : false,
  );
  /**
   * 当前**实际**外观。解析规则在 utils/appearance.ts（纯函数 + 有测试，
   * 且与 index.html 骨架里的那份内联实现同源、由测试对照）。
   * 刻意做成 computed：既有代码里大量 `app.dark` 读取处无需任何改动即可继续工作，
   * 写入则统一走 applyAppearance()。
   */
  const dark = computed<boolean>(() => resolveDark(appearance.value, systemDark.value));
  const themeColor = ref<string>(localStorage.getItem(THEME_KEY) || "#3b82f6");
  const fontFamily = ref<string>(localStorage.getItem(FONT_KEY) || "");

  /** 本机聊天显示样式（气泡配色 / 字号 / 紧凑模式），即点即存并广播同步。 */
  const chatStyle = ref<ChatStyleConfig>(loadLocalChatStyle());
  /** 对端样式表（device_id -> 样式 JSON）：按「发送者自己的偏好」渲染其消息气泡。 */
  const peerStyles = ref<Record<string, string>>({});

  // ---------------- 通知偏好 ----------------
  /** 桌面通知开关（后端持久化；默认开）。 */
  const notifyEnabled = ref<boolean>(true);
  /** 通知是否显示消息正文（隐私开关；默认显示）。 */
  const notifyShowContent = ref<boolean>(true);
  /** 权限缓存：已申请过且通过就不再弹（isPermissionGranted 每次重新查，这里只做短路）。 */
  let notifyPermission = false;

  /** 显式请求通知权限（供设置页开关在**用户动作上下文**里调用，符合 HIG）。 */
  async function ensureNotifyPermission(): Promise<boolean> {
    if (notifyPermission) return true;
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    notifyPermission = granted;
    return granted;
  }

  /**
   * 打开/关闭桌面通知。
   * 打开时先请求权限（在用户点开关这个上下文里，而不是等某条消息到达时才弹）；
   * 被拒绝则保持关闭并明确告知，不写脏状态。
   */
  async function setNotifyEnabled(v: boolean) {
    if (v) {
      const ok = await ensureNotifyPermission();
      if (!ok) {
        toast(t("notify.permissionDenied"), "error");
        return;
      }
    }
    notifyEnabled.value = v;
    void persistSettings();
  }

  function setNotifyShowContent(v: boolean) {
    notifyShowContent.value = v;
    void persistSettings();
  }

  // ---------------- 中继授权（P2 / M4） ----------------
  /**
   * 我愿不愿意替别人转发消息（多跳中继）。
   *
   * 默认 `all` = **与今天的行为完全一致**（多跳转发一直是无条件的）。默认不做成 `off`：
   * 跨跳投递依赖中间节点转发，默认关掉会让已有拓扑静默丢消息（用户明确要求
   * "现有局域网聊天不能搞坏"）。想限制中继的用户在设置页显式选择。
   */
  const relayPolicy = ref<RelayPolicy>("all");
  /** 白名单（`allowlist` 策略用）：只替这些设备转发。 */
  const relayAllowlist = ref<string[]>([]);

  function setRelayPolicy(p: RelayPolicy) {
    relayPolicy.value = p;
    void persistSettings();
  }

  function toggleRelayAllowlist(deviceId: string) {
    relayAllowlist.value = relayAllowlist.value.includes(deviceId)
      ? relayAllowlist.value.filter((x) => x !== deviceId)
      : [...relayAllowlist.value, deviceId];
    void persistSettings();
  }

  // ---------------- 语言 ----------------
  /** 语言偏好（system / zh-CN / en-US，后端持久化；默认跟随系统）。 */
  const language = ref<LanguagePreference>(currentPreference());

  /**
   * 把解析后的语言推给后端重建 macOS 菜单栏（原生控件的文案不归 WebView 管）。
   * 前端是"跟随系统"规则的唯一真相，所以由这里推，而不是后端自己检测。
   *
   * 静默吞错的理由与 `persistSettings` 相同：失败只影响菜单栏文案，
   * 界面本身已经切好了，弹 toast 只会打扰用户。
   */
  function pushUiLanguage() {
    void api.setUiLanguage(currentLocale()).catch(() => {
      /* 非 macOS 平台为空实现；偶发 IPC 失败不影响使用 */
    });
  }

  /** 切换语言偏好：立即生效（i18n 响应式更新）+ 持久化到后端。 */
  function setLanguage(p: LanguagePreference) {
    applyPreference(p);
    language.value = p;
    pushUiLanguage();
    void persistSettings();
  }

  // 轻量 toast
  interface Toast {
    id: number;
    text: string;
    type: "success" | "error" | "info";
  }
  const toasts = ref<Toast[]>([]);
  let toastId = 0;
  /**
   * 停留时长：错误要留够"读 + 听"的时间（读屏播报比扫一眼慢得多），
   * 成功/信息类短一些免得挡住界面。原实现一律 3000ms，错误常常没看完就消失了。
   */
  const TOAST_MS: Record<Toast["type"], number> = { success: 3000, info: 3000, error: 6000 };
  function toast(text: string, type: Toast["type"] = "info") {
    const id = ++toastId;
    toasts.value.push({ id, text, type });
    setTimeout(() => {
      toasts.value = toasts.value.filter((t) => t.id !== id);
    }, TOAST_MS[type]);
  }

  /**
   * 错误提示统一出口：把异常转成用户能读懂的说明再展示，原始串只进 console。
   * 所有 `catch` 里的错误提示都应走这里，不要自己拼 `：${e}`（见 utils/errors.ts）。
   */
  function toastError(e: unknown, prefix: string) {
    toast(reportError(e, prefix), "error");
  }

  // 响应式布局状态
  const isMobile = ref(false);
  const mobileView = ref<"list" | "chat">("list");
  /** 移动端软键盘是否弹出（视口被压缩超过阈值即认为弹出）：用于收起底部导航，避免浮在键盘上方。 */
  const keyboardOpen = ref(false);
  /**
   * 被软键盘盖住的高度（px）。**这是"键盘遮挡输入框"的真正补量**：
   *
   * - Android（WebView 走 adjustResize）：`window.innerHeight` 会随键盘一起缩，
   *   两者相减≈0 ⇒ 这里得到 0，布局不需要额外补偿（避免补偿两次把界面顶飞）；
   * - iOS（键盘只缩视觉视口，布局视口不变）：差值≈键盘高度 ⇒ 用它给聊天区加底部
   *   内边距，输入框才会浮在键盘之上，而不是被压在键盘下面。
   *
   * 之前只有 `keyboardOpen` 布尔量、从不做高度补偿 ⇒ iOS 上输入框被键盘盖住。
   */
  const keyboardInset = ref(0);

  function watchKeyboard() {
    const vv = window.visualViewport;
    if (!vv) return;
    const onChange = () => {
      // 视觉视口底部（offsetTop + height）以上的部分才是可见区，其余被键盘/工具栏盖住
      const covered = Math.max(0, Math.round(window.innerHeight - vv.height - vv.offsetTop));
      keyboardInset.value = covered < 80 ? 0 : covered; // 阈值滤掉地址栏收缩这类小抖动
      keyboardOpen.value = keyboardInset.value > 0;
    };
    vv.addEventListener("resize", onChange);
    vv.addEventListener("scroll", onChange);
    onChange();
  }

  function applyThemeNow() {
    applyTheme(themeColor.value, fontFamily.value);
  }

  function applyDarkNow() {
    document.documentElement.classList.toggle("dark", dark.value);
  }

  /** 聊天字号落到全局 CSS 变量（消息气泡 / 输入框引用）。 */
  function applyChatStyleNow() {
    document.documentElement.style.setProperty("--gosslan-msg-size", `${fontPx(chatStyle.value.fontSize)}px`);
  }

  /** 持久化全部偏好到后端 SQLite（重启后恢复，不依赖 WebView localStorage）。 */
  /**
   * 通道状态（局域网/蓝牙）**唯一真相源**。
   *
   * 用户实测的 bug：在「添加好友」页打开局域网通道，回到设置里却显示"已关闭" ——
   * 因为两处各自持有一份通道状态快照，而移动端设置页会**一直挂载**，
   * `active` 不变就不再重新拉取，于是显示过期状态。放进 store 后，
   * 任何一处开关都更新同一份状态，两边不可能再不一致。
   */
  const channels = ref<ChannelStatus[]>([]);
  /** 最近一次运行状态快照（`ble`/`peerCount` 这类只有快照才有的字段从这里读）。 */
  const runtime = ref<RuntimeSnapshot | null>(null);

  /**
   * 应用一份运行状态快照（**唯一入口**）。
   *
   * `channels` / `online` / `boundIp` 三份 UI 状态以前由两个命令 + 两个事件分别维护，
   * 于是"同一件事两份状态"必然不同步（用户实测：添加好友里开了局域网、设置里还显示关）。
   * 现在它们**只能**从这一份 `RuntimeSnapshot` 写入 —— 想改状态就必须先改后端。
   */
  function applyRuntimeSnapshot(snap: RuntimeSnapshot | null | undefined) {
    if (!snap) return;
    if (Array.isArray(snap.channels)) channels.value = snap.channels;
    online.value = !!snap.online;
    boundIp.value = snap.boundIp ?? null;
    runtime.value = snap;
  }

  /** 拉一次运行状态（窗口初始化 / 手动刷新）。失败保持现状，不清空界面。 */
  async function refreshRuntime() {
    try {
      applyRuntimeSnapshot(await api.getRuntimeSnapshot());
    } catch {
      /* 后端暂不可用：保持现状 */
    }
  }

  /**
   * 手机上按需拉起蓝牙通道（幂等、绝不抛错）。
   *
   * 「像 BitChat 一样默认就开」在**体验**上仍然成立：用户不需要任何设置；区别只是
   * **启动时不碰 BLE**，等用户真正打开「添加好友」/网络设置时才拉起 —— 这样即使 BLE 启动
   * 路径里还有崩溃点，也不会表现为"打开应用就闪退"（安卓的入口强制 panic=abort，
   * 一旦 panic 就是整进程消失、连日志都难拿）。
   */
  let bluetoothEnsureTried = false;
  async function ensureBluetoothOn() {
    if (bluetoothEnsureTried) return;
    bluetoothEnsureTried = true;
    try {
      await api.requestBlePermissions().catch(() => {});
      for (let attempt = 0; attempt < 2; attempt++) {
        if (channels.value.find((c) => c.channel === "bluetooth")?.enabled) return;
        try {
          applyRuntimeSnapshot(await api.setChannelEnabled("bluetooth", true));
          return;
        } catch {
          await new Promise((r) => setTimeout(r, 2000));
        }
      }
    } catch {
      /* 失败就保持关闭：界面会显示"已关闭"，用户可手动再试 */
    }
  }

  /**
   * 开关某条通道。**错误交给调用方**去 toast（各处文案不同）。
   *
   * ⚠️ 必须同时刷新 **通道状态** 与 **网络状态**：局域网这一件事有两份前端表示
   * （`channels[lan].enabled` 来自 `get_channel_status`，`online` 来自 `get_network_status`）。
   * 只刷新前者的话，「添加好友」页把局域网打开后，**设置页的开关仍然是关的**
   * —— 用户 2026-09-12 安卓实测报告的"两处不同步"就是这个。
   * 现在两处 UI 都只认这一条路径（设置页也改用通道状态），所以不可能再各说各话。
   */
  async function setChannelEnabled(channel: "lan" | "bluetooth", enabled: boolean) {
    // 后端把"切换后的运行状态"作为**返回值**给发起窗口（其它窗口走 runtime-changed 事件）——
    // 所以这里**不需要**再拉一次，也就不存在"拉回来的是旧值"的竞态。
    applyRuntimeSnapshot(await api.setChannelEnabled(channel, enabled));
  }

  /**
   * ⚠️ 这里**以前**有一套 `settingsDirty` / `lastLocalWriteAt` / grace 窗口守卫，专门用来
   * 防"本窗口写完设置后又被自己发出的 `settings-changed` 事件回灌旧快照"（症状：点了主题又跳回去）。
   * 现在不需要了：后端用 `emit_filter` **不把事件回发给发起窗口**，本窗口永远收不到自己写的那次变更
   * （见 `state::notify_settings_changed` / `SettingsPatch`）。少一套需要长期维护的状态机。
   */
  async function persistSettings() {
    try {
      await api.saveSettings({
        themeColor: themeColor.value,
        fontFamily: fontFamily.value,
        // darkMode = 解析后的**结果**（跟随系统时按系统偏好算出来），保留写入以兼容旧读取方；
        // appearanceMode = 用户的**意图**，重启后据此恢复。
        darkMode: dark.value,
        appearanceMode: appearance.value,
        notifyEnabled: notifyEnabled.value,
        notifyShowContent: notifyShowContent.value,
        language: language.value,
        relayPolicy: relayPolicy.value,
        relayAllowlist: JSON.stringify(relayAllowlist.value),
        bindIp: boundIp.value ?? preferredIp.value,
        chatStyle: JSON.stringify(chatStyle.value),
        peerStyles: null, // 对端样式表由后端维护，前端只读
      });
    } catch {
      /* 忽略：离线或后端暂不可用时不影响本地使用 */
    }
  }

  /**
   * 高频写入的**去抖持久化**：视觉立即生效，落库合并到 300ms 内一次。
   *
   * 为什么需要：颜色选择器（`<input type="color">`）拖动时每个像素变化都发一次
   * `input`，原实现每次都走一遍 `persistSettings()`（一个 IPC + 一条 SQLite 写）。
   * 连发粘贴的场景里同类问题是"每次按键一次 IPC" —— 主线程被 IPC 往返占住，
   * 输入框自己的渲染就掉帧。去抖后：拖动/连发期间只写最后一次，停手 300ms 内必落库。
   *
   * 读者注意：`persistSettings` 在**执行时**读当前 store 值，所以即使它与其它
   * 直接调用交错，后写的那一次总是最新值，不会把旧值写回去。
   */
  const persistSoon = debounce(() => void persistSettings(), 300);

  /** 窗口卸载/隐藏前把待写入的偏好落库（去抖窗口内退出也不丢最后一次改动）。 */
  if (typeof window !== "undefined") {
    window.addEventListener("beforeunload", () => persistSoon.flush());
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "hidden") persistSoon.flush();
    });
  }

  /**
   * 切换外观的**唯一**写入口：改模式 → 落 localStorage → 应用到 DOM → 持久化。
   * `theme-switching` 用来在变量整体翻转的那一帧禁用全站过渡（否则会看到渐变色闪一下）。
   */
  function applyAppearance(mode: AppearanceMode) {
    appearance.value = mode;
    localStorage.setItem(APPEARANCE_STORAGE_KEY, mode);
    const root = document.documentElement;
    root.classList.add("theme-switching");
    applyDarkNow();
    window.setTimeout(() => root.classList.remove("theme-switching"), 250);
    void persistSettings();
  }

  /** 设置页的三选一：跟随系统 / 浅色 / 深色。 */
  function setAppearance(mode: AppearanceMode) {
    applyAppearance(mode);
  }

  /**
   * 导航栏的快捷切换（太阳 / 月亮）：在当前**实际**外观上取反，并写成显式选择。
   * 刻意不回落到"跟随系统"—— 快捷开关的语义就是"我现在就要另一个外观"。
   */
  function toggleDark() {
    applyAppearance(dark.value ? "light" : "dark");
  }

  /**
   * 跟随系统：监听系统外观变化。
   * 只有 appearance === "system" 时才需要改界面；强制模式下系统怎么变都不该影响用户的选择。
   */
  function watchSystemAppearance() {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    systemDark.value = mq.matches;
    mq.addEventListener("change", (e) => {
      systemDark.value = e.matches;
      if (appearance.value !== "system") return;
      const root = document.documentElement;
      root.classList.add("theme-switching");
      applyDarkNow();
      window.setTimeout(() => root.classList.remove("theme-switching"), 250);
      void persistSettings(); // 解析结果变了，顺手把 darkMode 刷新到位
    });
  }

  /**
   * 改主题色。
   *
   * `continuous = true` 表示**连续输入**（拖动颜色选择器），此时去抖写库；
   * 点色板格子是离散操作，**立即写库** —— 这样"点一下"之后数据库马上就是新值，
   * 任何并发到来的 `settings-changed` 也读不到旧值（后端不会把事件回发给发起窗口）。
   */
  function setThemeColor(c: string, continuous = false) {
    themeColor.value = c;
    localStorage.setItem(THEME_KEY, c);
    applyThemeNow();
    if (continuous) persistSoon();
    else void persistSettings();
  }

  function setFontFamily(f: string) {
    fontFamily.value = f;
    localStorage.setItem(FONT_KEY, f);
    applyThemeNow();
    void persistSettings();
  }

  /** 聊天样式即点即改：立即生效 → 本地 + 后端持久化 → 广播给已连接节点。 */
  function setChatStyle(patch: Partial<ChatStyleConfig>) {
    chatStyle.value = { ...chatStyle.value, ...patch };
    localStorage.setItem(CHAT_STYLE_KEY, JSON.stringify(chatStyle.value));
    applyChatStyleNow();
    void persistSettings();
    void api.broadcastChatStyle(JSON.stringify(chatStyle.value)).catch(() => {
      /* 无连接节点时静默：下次变更或对端上线后不重复（样式以本地为准，对端旧值不影响） */
    });
  }

  /** 对端广播样式到达：更新表（后端已持久化，前端仅刷新内存）。 */
  function applyPeerStyle(deviceId: string, styleJson: string) {
    peerStyles.value = { ...peerStyles.value, [deviceId]: styleJson };
  }

  /**
   * 把一份「设置快照」应用到本窗口（主题 / 外观 / 语言 / 通知 / 中继 / 气泡样式 / 网卡）。
   *
   * 抽出来是为了让「启动时应用」与「另一个窗口改了设置后重新应用」走**同一段代码** ——
   * 两处各写一份必然漂移（真实缺陷：在设置窗口改语言/主题，主窗口一点不变）。
   */
  /**
   * 应用一份设置快照。
   *
   * `partial = true` 表示这是 `settings-changed` 带来的**补丁**：只含变了的键，
   * 因此**缺的键一律不能动**（不能拿 undefined 去覆盖本地值）。默认 `false` = 完整快照，
   * 与历史行为逐字一致。
   */
  function applySettingsSnapshot(s: AppSettings, opts: { partial?: boolean } = {}) {
    const has = (k: keyof AppSettings) => !opts.partial || k in s;
    // 防御：快照可能为 null/undefined（IPC 边界、窗口正在销毁、旧 store 实例被调用）。
    // 真实缺陷（用户 2026-09-12 Mac 4.1.5 实测）：设置窗口日志里出现
    // `[前端 rejection] null is not an object (evaluating 'g.themeColor')` ——
    // 就是这里读属性时快照为 null；而**一次渲染期异常会让那一页再也 patch 不动**，
    // 表现出来正是"点设置顿顿的、过一会儿才突然弹出来"。
    if (!s) return;
      if (s.themeColor) themeColor.value = s.themeColor;
      if (s.fontFamily != null) fontFamily.value = s.fontFamily;
      // 外观：优先用「用户意图」(appearanceMode)；旧记录只有布尔 darkMode → 视为一次显式选择。
      if (has("appearanceMode") && isAppearanceMode(s.appearanceMode)) {
        appearance.value = s.appearanceMode;
        localStorage.setItem(APPEARANCE_STORAGE_KEY, s.appearanceMode);
      } else if (has("darkMode") && s.darkMode != null) {
        appearance.value = s.darkMode ? "dark" : "light";
        localStorage.setItem(APPEARANCE_STORAGE_KEY, appearance.value);
      }
      // 通知偏好（null = 未设置，按默认 true 处理）
      if (s.notifyEnabled != null) notifyEnabled.value = s.notifyEnabled;
      if (s.notifyShowContent != null) notifyShowContent.value = s.notifyShowContent;
      // 语言（null/脏值 = 默认跟随系统）
      if (has("language") && isLanguagePreference(s.language)) applyPreference(s.language);
      // 中继授权（脏值一律回落默认 all —— 与后端 RelayConfig::parse 同口径）
      if (s.relayPolicy === "off" || s.relayPolicy === "friends" || s.relayPolicy === "allowlist" || s.relayPolicy === "all") {
        relayPolicy.value = s.relayPolicy;
      }
      if (s.relayAllowlist) {
        try {
          const list = JSON.parse(s.relayAllowlist) as unknown;
          if (Array.isArray(list)) relayAllowlist.value = list.filter((x): x is string => typeof x === "string");
        } catch {
          relayAllowlist.value = [];
        }
      }
      // ⚠️ 只有"这份快照确实带了 language"时才回推原生菜单栏。
      // 以前无条件调用 ⇒ 每次收到设置事件都会走 `set_ui_language()`，而那条命令当时也会
      // 再发一次 `settings-changed` ⇒ 两个窗口互相触发，形成高频 IPC 环（事件乒乓）。
      if (has("language")) {
        language.value = currentPreference();
        pushUiLanguage();
      }
      // `bindIp` 必须判"键在不在"：局部补丁里没有它时，把 preferredIp 写成 undefined
      // 会让"选中的网卡"当场消失。
      if (has("bindIp")) preferredIp.value = s.bindIp;
      if (has("chatStyle") && s.chatStyle) chatStyle.value = parsePeerStyle(s.chatStyle);
      if (s.peerStyles) {
        try {
          peerStyles.value = JSON.parse(s.peerStyles) as Record<string, string>;
        } catch {
          peerStyles.value = {};
        }
      }
      // 副作用（改 CSS 变量 / 类名）只在相关键真的变了时跑：补丁路径下这是常态，
      // 每次设置事件都全量重刷一遍样式纯属浪费。
      if (!opts.partial || has("themeColor") || has("fontFamily") || has("darkMode") || has("appearanceMode")) {
        applyThemeNow();
        applyDarkNow();
      }
      if (!opts.partial || has("chatStyle") || has("peerStyles")) applyChatStyleNow();
  }

  /** `settings-changed` 的取消函数（init 可能被调用多次，避免重复绑定）。 */
  let settingsUnlisten: (() => void) | null = null;
  let runtimeUnlisten: (() => void) | null = null;

  /** 「另一个窗口改了设置」→ 完整重拉一次（只用于"恢复默认"这类**全量**变更）。 */
  async function resyncFromBackend() {
    const [st, dev, share] = await Promise.allSettled([
      api.getSettings(),
      api.getDeviceInfo(),
      api.getShareDir(),
    ]);
    if (st.status === "fulfilled") applySettingsSnapshot(st.value);
    if (dev.status === "fulfilled") device.value = dev.value;
    if (share.status === "fulfilled") shareDir.value = share.value;
  }

  /**
   * 应用「另一个窗口改了设置」的补丁。
   *
   * 设计要点（对应 `SettingsPatch`）：
   * · **默认零 IPC**：`patch.settings` 里已经带了所有能直接应用的键值，
   *   以前每次事件都要 `get_settings + get_device_info + get_share_dir` 三连拉，现在不再需要；
   * · **只有"不在 Settings 形状里"的键**才做定向重拉（资料 / 目录，出现频率极低）；
   * · `changed` 含 `"*"` 表示"恢复默认"（值被整体删掉了，逐键送 patch 容易漏）⇒ 完整重拉一次。
   */
  async function applySettingsPatch(patch: SettingsChanged | null | undefined) {
    if (!patch || !Array.isArray(patch.changed) || patch.changed.length === 0) return;
    if (patch.changed.includes("*")) {
      await resyncFromBackend();
      return;
    }
    if (patch.settings) applySettingsSnapshot(patch.settings as AppSettings, { partial: true });
    if (patch.changed.includes("nickname") || patch.changed.includes("avatar")) {
      try {
        device.value = await api.getDeviceInfo();
      } catch {
        /* 定向重拉失败不致命：下次任何一次刷新都会纠正 */
      }
    }
    if (patch.changed.includes("shareDir")) {
      try {
        shareDir.value = await api.getShareDir();
      } catch {
        /* 同上 */
      }
    }
  }

  async function init() {
    // init 可能被调用多次：先解绑上一次的设置事件监听，避免重复触发
    settingsUnlisten?.();
    settingsUnlisten = null;
    // 平台标记：供 CSS 按平台差异化（如 macOS 恢复系统 overlay 滚动条）
    if (typeof document !== "undefined") {
      document.documentElement.classList.toggle("platform-mac", isMac);
    }
    // 从后端恢复持久化偏好（外观 / 网卡 / 聊天样式），优先于 localStorage
    const s = await api.getSettings();
    applySettingsSnapshot(s);

    // 「另一个窗口改了设置」→ 重新拉取并应用（独立设置窗口 ↔ 主窗口必须同步外观/语言/资料）
    settingsUnlisten = await api.onSettingsChanged((patch) => {
      void applySettingsPatch(patch);
    });

    // 「运行状态变了」（任何一处开了/关了通道）⇒ 重拉通道状态与在线状态。
    // 这一步是"外面开了、里面还是关的"的正解：两处 UI 都只读后端这一份真相。
    runtimeUnlisten?.();
    runtimeUnlisten = await api.onRuntimeChanged((snap) => {
      applyRuntimeSnapshot(snap);
    });

    // 注册系统外观监听（跟随系统模式下，用户在系统设置里切换要即时生效，不必重启）
    watchSystemAppearance();
    const mq = window.matchMedia("(max-width: 767px)");
    isMobile.value = mq.matches;
    mq.addEventListener("change", (e) => (isMobile.value = e.matches));
    watchKeyboard();

    await refreshEnvironment();
    // Android 首次启动申请「附近的设备」等运行时权限（系统弹框）。
    // ⚠️ 延迟到首帧之后且不 await：用户红线是"不能有任何阻断渲染的操作" ——
    // 权限弹框该在界面已经画出来之后再出现。
    // 首帧之后自动确保蓝牙通道开启（用户规则：有蓝牙就默认开，不要手动开关）。
    // 放在 2s 之后：此刻界面已经画出来，且**不再位于启动关键路径**上；
    // 失败只记日志/保持关闭（`ensureBluetoothOn` 自身幂等、绝不抛）。
    window.setTimeout(() => void ensureBluetoothOn(), 2000);

    // ⚠️ 移动端**启动路径不申请任何权限、不碰任何平台专有代码**。
    //    原因：安卓 release 包"打开就闪退"极可能发生在这类调用里（JNI/Kotlin 路径），
    //    而 Tauri 的安卓入口强制 panic=abort ⇒ 一旦 panic 就是整进程消失。
    //    权限申请改为**按需**：打开「添加好友」或网络设置时才申请（见 ensureBluetoothOn）。


    // 自动启动在后台异步执行：init 读取时可能尚未完成，导致 online=false
    // 而实际网络已经在运行。延迟刷新一次以修正 UI 状态。
    setTimeout(() => void refreshRuntime(), 500);
  }

  /** 恢复默认：后端清除偏好键，前端回落默认值（默认蓝色主题 / 系统字体 / **跟随系统** / 自动网卡）。 */
  async function resetDefaults() {
    // 先广播昵称/头像恢复默认（LAN 仍在线，好友可收到 UserInfo）
    if (device.value) {
      // 默认名由后端按规则生成（`nickname.rs`：形容词+动物+设备短码）；
      // 前端**不再**写死一份文案 —— 否则"恢复默认"得到的名字与首次安装不一致。
      const fallback = await api.defaultNickname();
      await api.updateProfile(fallback, null);
      device.value.nickname = fallback;
      device.value.avatar = null;
    }
    await api.resetSettings();
    themeColor.value = "#3b82f6";
    fontFamily.value = "";
    appearance.value = "system";
    notifyEnabled.value = true;
    notifyShowContent.value = true;
    applyPreference("system");
    language.value = "system";
    pushUiLanguage();
    // 中继授权也回到默认（后端 reset_settings 已清掉这两个键）
    relayPolicy.value = "all";
    relayAllowlist.value = [];
    preferredIp.value = null;
    boundIp.value = null;
    chatStyle.value = { ...DEFAULT_CHAT_STYLE };
    peerStyles.value = {};
    localStorage.removeItem(THEME_KEY);
    localStorage.removeItem(FONT_KEY);
    localStorage.removeItem(APPEARANCE_STORAGE_KEY);
    localStorage.removeItem(LEGACY_DARK_STORAGE_KEY);
    localStorage.removeItem(CHAT_STYLE_KEY);
    applyThemeNow();
    applyDarkNow();
    applyChatStyleNow();
    void persistSettings();
  }

  async function updateProfile(nickname: string, avatar: string | null) {
    device.value = await api.updateProfile(nickname, avatar);
  }

  async function startNetwork(bindIp: string) {
    await api.startNetwork(bindIp);
    online.value = true;
    boundIp.value = bindIp;
    preferredIp.value = bindIp;
    void persistSettings();
  }

  async function stopNetwork() {
    await api.stopNetwork();
    online.value = false;
    boundIp.value = null;
    void persistSettings();
  }

  /**
   * 设置共享目录。
   * 乐观更新（用户 2026-09-12 要求「异步操作尽量乐观更新」）：本地先生效（设置页立即显示
   * 新目录），再落库；失败回滚并抛出，由调用方 toast。目录来自系统选择器，用户已确认动作，
   * 等一次 IPC 才回显没有意义。
   */
  async function setShareDir(path: string) {
    const prev = shareDir.value;
    shareDir.value = path;
    try {
      await api.setShareDir(path);
    } catch (e) {
      shareDir.value = prev;
      throw e;
    }
  }

  /**
   * 刷新**会变的环境数据**：设备信息 / 网卡与 IP / 共享目录 / 在线状态。
   *
   * 为什么单独抽出来：独立「设置」窗口现在是**常驻**的（关闭只是隐藏，见
   * `commands.rs` 的 `AUX_WINDOWS_RESIDENT`），窗口不会重新加载 ⇒ 用户切了 Wi-Fi、
   * 换了共享目录之后再打开设置，看到的会是**上次的快照**。所以设置窗口在重新获得焦点时
   * 调一次这里（偏好类数据不在此列 —— 那是 `settings-changed` 事件负责同步的）。
   *
   * 四项互不依赖 ⇒ **并行**拉取（原先串行 await，启动要多等 3 个 IPC 往返；
   * 用户 2026-09-12 要求「不要有任何阻断渲染的操作」）。
   * 用 `allSettled` 而不是 `all`：任一项失败（例如网卡枚举在权限受限时抛错）
   * 不该把设备信息 / 共享目录一起弄丢 —— 逐项取成功值、失败保持默认。
   */
  async function refreshEnvironment() {
    const [dev, ifaces, share, snap] = await Promise.allSettled([
      api.getDeviceInfo(),
      api.listInterfaces(),
      api.getShareDir(),
      api.getRuntimeSnapshot(),
    ]);
    if (dev.status === "fulfilled") device.value = dev.value;
    if (ifaces.status === "fulfilled") interfaces.value = ifaces.value;
    if (share.status === "fulfilled") shareDir.value = share.value;
    if (snap.status === "fulfilled") applyRuntimeSnapshot(snap.value);
  }

  async function refreshInterfaces() {
    interfaces.value = await api.listInterfaces();
  }

  return {
    channels,
    runtime,
    refreshRuntime,
    ensureBluetoothOn,
    setChannelEnabled,
    device,
    interfaces,
    online,
    boundIp,
    preferredIp,
    shareDir,
    dark,
    /** 外观模式（用户意图）：system | light | dark */
    appearance,
    setAppearance,
    notifyEnabled,
    notifyShowContent,
    setNotifyEnabled,
    setNotifyShowContent,
    ensureNotifyPermission,
    language,
    setLanguage,
    relayPolicy,
    relayAllowlist,
    setRelayPolicy,
    toggleRelayAllowlist,
    themeColor,
    fontFamily,
    chatStyle,
    peerStyles,
    isMobile,
    mobileView,
    keyboardOpen,
    keyboardInset,
    init,
    toggleDark,
    updateProfile,
    startNetwork,
    stopNetwork,
    setShareDir,
    refreshInterfaces,
    refreshEnvironment,
    setThemeColor,
    setFontFamily,
    setChatStyle,
    applyPeerStyle,
    resetDefaults,
    toasts,
    toast,
    toastError,
  };
});

// Vite HMR：**改了 store 必须让新 store 生效**。
//
// 踩坑（用户实测"点设置卡、过一会儿弹出好几个设置、主题延迟切换"）：Pinia 的 store 是
// 缓存过的单例，**不接 HMR 就一直是旧实例** —— 我这一轮给 store 新增了 `channels` /
// `channels`/`setChannelEnabled`，而用户长时间运行的 dev 会话里还是旧 store ⇒ 设置页里
// `app.setChannelEnabled is not a function`、`channels.value.find` 抛错 ⇒ **整页渲染卡死**
// （一个分区渲染抛错，Vue 之后再也 patch 不动这个页面）。加上这一行之后，
// 以后改 store 都不必重启 dev。
if (import.meta.hot) import.meta.hot.accept(acceptHMRUpdate(useAppStore, import.meta.hot));
