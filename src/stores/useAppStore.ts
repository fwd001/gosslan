import { defineStore } from "pinia";
import { ref } from "vue";
import { api } from "@/api";
import { applyTheme } from "@/utils/color";
import { DEFAULT_CHAT_STYLE, fontPx, parsePeerStyle, type ChatStyleConfig } from "@/utils/chatStyle";
import type { DeviceInfo, InterfaceInfo } from "@/types";

const THEME_KEY = "gosslan.themeColor";
const FONT_KEY = "gosslan.fontFamily";
const DARK_KEY = "gosslan.dark";
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

  const dark = ref<boolean>(localStorage.getItem(DARK_KEY) === "1");
  const themeColor = ref<string>(localStorage.getItem(THEME_KEY) || "#3370ff");
  const fontFamily = ref<string>(localStorage.getItem(FONT_KEY) || "");

  /** 本机聊天显示样式（气泡配色 / 字号 / 紧凑模式），即点即存并广播同步。 */
  const chatStyle = ref<ChatStyleConfig>(loadLocalChatStyle());
  /** 对端样式表（device_id -> 样式 JSON）：按「发送者自己的偏好」渲染其消息气泡。 */
  const peerStyles = ref<Record<string, string>>({});

  // 轻量 toast
  interface Toast {
    id: number;
    text: string;
    type: "success" | "error" | "info";
  }
  const toasts = ref<Toast[]>([]);
  let toastId = 0;
  function toast(text: string, type: Toast["type"] = "info") {
    const id = ++toastId;
    toasts.value.push({ id, text, type });
    setTimeout(() => {
      toasts.value = toasts.value.filter((t) => t.id !== id);
    }, 3000);
  }

  // 响应式布局状态
  const isMobile = ref(false);
  const mobileView = ref<"list" | "chat">("list");

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
  async function persistSettings() {
    try {
      await api.saveSettings({
        themeColor: themeColor.value,
        fontFamily: fontFamily.value,
        darkMode: dark.value,
        bindIp: boundIp.value ?? preferredIp.value,
        chatStyle: JSON.stringify(chatStyle.value),
        peerStyles: null, // 对端样式表由后端维护，前端只读
      });
    } catch {
      /* 忽略：离线或后端暂不可用时不影响本地使用 */
    }
  }

  function toggleDark() {
    dark.value = !dark.value;
    localStorage.setItem(DARK_KEY, dark.value ? "1" : "0");
    applyDarkNow();
    void persistSettings();
  }

  function setThemeColor(c: string) {
    themeColor.value = c;
    localStorage.setItem(THEME_KEY, c);
    applyThemeNow();
    void persistSettings();
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

  async function init() {
    // 从后端恢复持久化偏好（外观 / 网卡 / 聊天样式），优先于 localStorage
    const s = await api.getSettings();
    if (s.themeColor) themeColor.value = s.themeColor;
    if (s.fontFamily != null) fontFamily.value = s.fontFamily;
    if (s.darkMode != null) dark.value = s.darkMode;
    preferredIp.value = s.bindIp;
    if (s.chatStyle) chatStyle.value = parsePeerStyle(s.chatStyle);
    if (s.peerStyles) {
      try {
        peerStyles.value = JSON.parse(s.peerStyles) as Record<string, string>;
      } catch {
        peerStyles.value = {};
      }
    }
    applyThemeNow();
    applyDarkNow();
    applyChatStyleNow();
    const mq = window.matchMedia("(max-width: 767px)");
    isMobile.value = mq.matches;
    mq.addEventListener("change", (e) => (isMobile.value = e.matches));

    device.value = await api.getDeviceInfo();
    interfaces.value = await api.listInterfaces();
    shareDir.value = await api.getShareDir();
    const st = await api.getNetworkStatus();
    online.value = st.online;
    boundIp.value = st.bound_ip;
    // 自动启动在后台异步执行：init 读取时可能尚未完成，导致 online=false
    // 而实际网络已经在运行。延迟刷新一次以修正 UI 状态。
    setTimeout(async () => {
      const st2 = await api.getNetworkStatus();
      if (st2.online !== online.value) {
        online.value = st2.online;
        boundIp.value = st2.bound_ip;
      }
    }, 500);
  }

  /** 恢复默认：后端清除偏好键，前端回落默认值（默认蓝色主题 / 系统字体 / 浅色 / 自动网卡）。 */
  async function resetDefaults() {
    // 先停止 LAN：避免 reset 后数据库认为默认（开启），但实际仍在运行的状态不一致
    if (online.value) {
      await stopNetwork();
      online.value = false;
      boundIp.value = null;
    }
    await api.resetSettings();
    themeColor.value = "#3370ff";
    fontFamily.value = "";
    dark.value = false;
    preferredIp.value = null;
    chatStyle.value = { ...DEFAULT_CHAT_STYLE };
    peerStyles.value = {};
    // 重置昵称和头像为默认值，并广播给在线好友
    if (device.value) {
      device.value.nickname = "Gosslan 用户";
      device.value.avatar = null;
      void api.updateProfile("Gosslan 用户", null);
    }
    localStorage.removeItem(THEME_KEY);
    localStorage.removeItem(FONT_KEY);
    localStorage.removeItem(DARK_KEY);
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

  async function setShareDir(path: string) {
    await api.setShareDir(path);
    shareDir.value = path;
  }

  async function refreshInterfaces() {
    interfaces.value = await api.listInterfaces();
  }

  return {
    device,
    interfaces,
    online,
    boundIp,
    preferredIp,
    shareDir,
    dark,
    themeColor,
    fontFamily,
    chatStyle,
    peerStyles,
    isMobile,
    mobileView,
    init,
    toggleDark,
    updateProfile,
    startNetwork,
    stopNetwork,
    setShareDir,
    refreshInterfaces,
    setThemeColor,
    setFontFamily,
    setChatStyle,
    applyPeerStyle,
    resetDefaults,
    toasts,
    toast,
  };
});
