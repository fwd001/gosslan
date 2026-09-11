import { ref } from "vue";

export type CopyKey = string | null;

/**
 * 复制反馈：同一时刻只有一个按钮处于「已复制」态（1.5s 后自动复位）。
 * 剪贴板不可用时静默——不弹 toast 打扰发送。
 */
export function useClipboard(resetMs = 1500) {
  const copiedKey = ref<CopyKey>(null);

  /**
   * 复制文本。**返回是否成功** —— 调用方需要据此给用户反馈：
   * 移动端「长按 → 底部操作面板」点完面板就关了，若没有额外提示，
   * 用户根本不知道复制成功没有（气泡上的"已复制"勾只在长文本操作条里渲染）。
   * 仍然自捕获异常（老调用方不 await 也不会产生 unhandled rejection）。
   */
  async function copyContent(key: string, text: string): Promise<boolean> {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      return false;
    }
    copiedKey.value = key;
    setTimeout(() => {
      if (copiedKey.value === key) copiedKey.value = null;
    }, resetMs);
    return true;
  }

  return { copiedKey, copyContent };
}
