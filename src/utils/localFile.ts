/**
 * 打开 / 另存**本机已有**的文件 —— 消息气泡（useMessageFile）与群文件面板
 * （GroupFilesPanel）共用的唯一路径。
 *
 * 平台的差异只能写一次：Android 上系统经常没有能"打开"某类文件的应用
 * （用户实测：除图片外基本都报错），而且应用私有目录里的文件即便被打开，
 * 也只是交给对方一个临时只读副本 —— 所以 Android 一律改走系统保存对话框（SAF）。
 * 这条规则若分散成两处，群文件面板会退化成"点了直接报错"。
 *
 * 返回值只描述**发生了什么**，文案与 toast 交给调用方：错误文案的上下文
 * （"打开失败" vs "保存失败"）只有调用方知道。
 */
import { save } from "@tauri-apps/plugin-dialog";
import { api } from "@/api";
import { isAndroid } from "@/utils/platform";
import { isDialogCancelled, saveDestinationOf } from "@/utils/saveDestination";

export type LocalFileResult = "done" | "cancelled";

/**
 * 用系统默认应用打开本地文件。
 * Android 上没有"打开"这一步，直接落到 `saveLocalFile`（见文件头说明）。
 * @throws 后端/插件返回的真实错误（调用方负责提示）
 */
export async function openLocalFile(path: string, name: string): Promise<LocalFileResult> {
  if (isAndroid) return saveLocalFile(path, name);
  // macOS 走 NSWorkspace（沙盒下 opener 的 /usr/bin/open 被拦），Windows/Linux 由后端回落 opener。
  // 文件不存在时后端返回明确错误。
  await api.openFileNative(path);
  return "done";
}

/**
 * 另存为：桌面弹保存对话框后复制一份；Android 走 SAF。
 * 用户取消返回 `"cancelled"`（不是错误）。
 * @throws 复制/对话框的真实错误
 */
export async function saveLocalFile(path: string, name: string): Promise<LocalFileResult> {
  try {
    // 桌面返回路径字符串；Android 的 SAF 返回 { file: content:// } 对象，先归一化。
    const picked: unknown = await save({ defaultPath: name });
    const destination = saveDestinationOf(picked);
    if (!destination) return "cancelled";
    await api.copyFile(path, destination);
    return "done";
  } catch (e) {
    // Android 取消是 reject 而不是返回 null
    if (isDialogCancelled(e)) return "cancelled";
    throw e;
  }
}
