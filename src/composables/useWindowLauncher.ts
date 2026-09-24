/**
 * 打开独立窗口（桌面端「设置」/「运行日志」）的统一入口。
 *
 * ## 为什么是模块级单例
 * 触发「打开设置」的地方有三处：桌面窄导航头像、移动端底栏按钮、以及 macOS/Windows 原生菜单
 * （`APP_ACTION.openSettings`）。它们都必须共用同一份"正在打开 / 刚打开过"的状态，
 * 否则三处各判一次就等于没防抖（用户连点两下 = 两个转圈、两次 IPC）。
 *
 * ## 分工
 * - 这里：单飞（同一窗口同时只发一次 invoke）+ 连点防抖 + 给按钮用的 pending 状态；
 * - 后端 `ensure_aux_window`：窗口实例唯一（已存在就 show + focus，绝不建第二个）。
 */
import { computed, reactive, type ComputedRef } from "vue";
import { shouldLaunchWindow } from "@/utils/windowLaunch";

/**
 * 桌面独立窗口的 label，与 Rust `lib.rs` 的 `WINDOW_*` 常量一一对应
 * （`tasks_window_uses_one_fixed_label_cross_checked_with_frontend` 做交叉核对）。
 *
 * 群任务窗口以前是每群一扇的 `todo-<groupId>`，现在是一扇**固定** label：看哪个群由后端那份
 * 当前上下文决定。唯一的动机是"不传参数就能在后台先把 WebView 建好，用的时候瞬间激活"——
 * 动态 label 在预热那一刻不知道该建哪一扇，秒开就无从谈起。
 */
export type AuxWindowLabel = "settings" | "logs" | "link" | "tasks";

/** 正在打开的窗口 label（响应式，供按钮显示 pending 状态）。 */
const opening = reactive(new Set<AuxWindowLabel>());
/** 上一次真正发起打开的时刻（防抖用）。 */
const lastLaunchAt = new Map<AuxWindowLabel, number>();

/** 该窗口此刻是否正在打开（给按钮做 pending 反馈）。 */
export function useWindowOpening(label: AuxWindowLabel): ComputedRef<boolean> {
  return computed(() => opening.has(label));
}

/**
 * 该窗口此刻是否正在打开（**普通函数**，可在 `computed` 内部使用）。
 *
 * 与 [`useWindowOpening`] 的区别：那个是 composable（只能在 setup 顶层调用），
 * 这个给"label 由 props/状态算出来"的场景（如群任务窗口的 `todo-<当前群>`）。
 */
export function isWindowOpening(label: AuxWindowLabel): boolean {
  return opening.has(label);
}

/**
 * 打开一个独立窗口。返回 `true` = 真的发起了打开；`false` = 被单飞/防抖合并掉了
 * （此时**没有**任何错误，调用方不需要做任何事）。
 *
 * 错误（窗口创建失败等）会**原样抛出**，由调用方决定回退策略
 * （设置回退到应用内设置页、日志提示失败）—— 这里绝不吞异常。
 */
export async function launchAuxWindow(
  label: AuxWindowLabel,
  open: () => Promise<void>,
): Promise<boolean> {
  const last = lastLaunchAt.get(label) ?? 0;
  if (!shouldLaunchWindow(Date.now(), opening.has(label), last)) return false;
  opening.add(label);
  lastLaunchAt.set(label, Date.now());
  try {
    await open();
    return true;
  } finally {
    opening.delete(label);
  }
}

/** 仅供测试：重置模块级状态（真实运行时不调用）。 */
export function __resetWindowLauncherForTest(): void {
  opening.clear();
  lastLaunchAt.clear();
}
