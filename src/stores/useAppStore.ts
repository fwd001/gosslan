import { defineStore } from "pinia";
import { ref } from "vue";
import { api } from "@/api";
import { applyTheme } from "@/utils/color";
import type { DeviceInfo, InterfaceInfo } from "@/types";

const THEME_KEY = "gosslan.themeColor";
const FONT_KEY = "gosslan.fontFamily";
const DARK_KEY = "gosslan.dark";

export const useAppStore = defineStore("app", () => {
  const device = ref<DeviceInfo | null>(null);
  const interfaces = ref<InterfaceInfo[]>([]);
  const online = ref(false);
  const boundIp = ref<string | null>(null);
  const shareDir = ref<string | null>(null);

  const dark = ref<boolean>(localStorage.getItem(DARK_KEY) === "1");
  const themeColor = ref<string>(localStorage.getItem(THEME_KEY) || "#3370ff");
  const fontFamily = ref<string>(localStorage.getItem(FONT_KEY) || "");

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

  /** 持久化全部偏好到后端 SQLite（重启后恢复，不依赖 WebView localStorage）。 */
  async function persistSettings() {
    try {
      await api.saveSettings({
        themeColor: themeColor.value,
        fontFamily: fontFamily.value,
        darkMode: dark.value,
        bindIp: boundIp.value,
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

  async function init() {
    // 从后端恢复持久化偏好（外观 / 网卡），优先于 localStorage
    const s = await api.getSettings();
    if (s.themeColor) themeColor.value = s.themeColor;
    if (s.fontFamily != null) fontFamily.value = s.fontFamily;
    if (s.darkMode != null) dark.value = s.darkMode;
    applyThemeNow();
    applyDarkNow();
    const mq = window.matchMedia("(max-width: 767px)");
    isMobile.value = mq.matches;
    mq.addEventListener("change", (e) => (isMobile.value = e.matches));

    device.value = await api.getDeviceInfo();
    interfaces.value = await api.listInterfaces();
    shareDir.value = await api.getShareDir();
    const st = await api.getNetworkStatus();
    online.value = st.online;
    boundIp.value = st.bound_ip;
  }

  /** 恢复默认：后端清除偏好键，前端回落默认值（默认蓝色主题 / 系统字体 / 浅色 / 自动网卡）。 */
  async function resetDefaults() {
    await api.resetSettings();
    themeColor.value = "#3370ff";
    fontFamily.value = "";
    dark.value = false;
    localStorage.removeItem(THEME_KEY);
    localStorage.removeItem(FONT_KEY);
    localStorage.removeItem(DARK_KEY);
    applyThemeNow();
    applyDarkNow();
    void persistSettings();
  }

  async function updateProfile(nickname: string, avatar: string | null) {
    device.value = await api.updateProfile(nickname, avatar);
  }

  async function startNetwork(bindIp: string) {
    await api.startNetwork(bindIp);
    online.value = true;
    boundIp.value = bindIp;
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
    shareDir,
    dark,
    themeColor,
    fontFamily,
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
    resetDefaults,
    toasts,
    toast,
  };
});
