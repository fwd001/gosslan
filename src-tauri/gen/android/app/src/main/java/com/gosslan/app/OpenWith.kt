package com.gosslan.app

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * 用系统里的**其它应用**打开我们自己的私有目录里的文件。
 *
 * ## 为什么不能用 `tauri-plugin-opener`（用户 2026-09-12 真机实测：「文件打开失败」）
 *
 * opener 的 Android 实现只有一句 `Intent(ACTION_VIEW, url.toUri())`（见插件源码
 * `OpenerPlugin.kt`）。我们从 Rust 传过去的是 `file:///data/user/0/com.gosslan.app/...`，
 * 而 **Android 7.0+ 禁止把应用私有文件以 `file://` 暴露给别的应用** ——
 * `startActivity` 当场抛 `FileUriExposedException`，前端只能显示一个笼统的失败。
 *
 * 正确姿势是 FileProvider：把私有文件映射成 `content://<包名>.fileprovider/<根>/…`，
 * 并在 intent 上带 `FLAG_GRANT_READ_URI_PERMISSION` —— 只把这**一个** URI 的读权限
 * 临时授给用户选中的那个应用。不需要任何存储权限，也不暴露目录。
 * （清单里的 provider 与 `res/xml/file_paths.xml` 是 Tauri 模板自带的，我们只补了
 * `root-path`：Tauri 在 Android 上的 data 目录是 `dataDir` 本身，而收到的文件写在
 * `dataDir/downloads`，不在模板原有的 `cache-path` / `external-path` 里。）
 *
 * ## 两个非显然的约束
 *
 * 1. **framework 调用必须回主线程**：Rust 命令跑在 tokio 工作线程上，而 JNI 附加的
 *    非主线程上抛出的 Java 异常**不会**被我们的 Rust panic hook 抓到，进程会静默消失
 *    —— 与 `BlePeripheral` 里那次真实闪退同一个坑（见其 `onMainSync` 注释）。
 *    所以这里也同步跳主线程执行，并且**所有**异常都转成给用户看的中文原因。
 * 2. `nativeAttachOpenWith()` 必须在 App 代码还在栈上时调用（JNI 的 `FindClass` 依赖
 *    调用方的类加载器），所以由 `MainActivity.onCreate` 经 [bootstrap] 触发。
 *    没有它 Rust 侧拿不到 `JavaVM`，命令会返回「桥未初始化」而不是崩掉。
 */
object OpenWith {
  /** 与清单里 `android:authorities="${applicationId}.fileprovider"` 对齐。 */
  private const val AUTHORITY_SUFFIX = ".fileprovider"

  /** 主线程执行 + 等待结果的上限（超时只影响提示文案，不影响文件）。 */
  private const val MAIN_TIMEOUT_MS = 3_000L

  private var appContext: Context? = null
  private val mainHandler by lazy { Handler(Looper.getMainLooper()) }

  /** `MainActivity.onCreate` 调用：缓存 Context（打开文件用）并把 JavaVM 交给 Rust。 */
  @JvmStatic
  fun bootstrap(context: Context) {
    appContext = context.applicationContext
    try {
      nativeAttachOpenWith()
    } catch (e: UnsatisfiedLinkError) {
      // 理论上不会发生（这个 native 无条件编译进库），但绝不能崩在启动路径上
      android.util.Log.i("GosslanOpen", "打开文件桥不可用：${e.message}")
    }
  }

  /**
   * 打开本地文件。**返回 null 表示已交给系统**；否则返回给用户看的中文原因
   * （前端直接把这句话显示在 toast 里）。
   */
  @JvmStatic
  fun openWith(path: String, mime: String?): String? {
    val ctx = appContext ?: return "应用还没准备好，请稍后重试"
    if (Looper.myLooper() == Looper.getMainLooper()) {
      return openOnMain(ctx, path, mime)
    }
    var outcome: String? = null
    var finished = false
    val latch = CountDownLatch(1)
    val posted = mainHandler.post {
      try {
        outcome = openOnMain(ctx, path, mime)
      } finally {
        finished = true
        latch.countDown()
      }
    }
    if (!posted) return "系统正忙，请稍后重试"
    if (!latch.await(MAIN_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
      return "打开文件超时（系统无响应）"
    }
    // finished 只是为了让静态检查满意：超时后 outcome 可能仍是 null（=误报成功）
    return if (finished) outcome else "打开文件超时（系统无响应）"
  }

  /** 真正干活的一步，只在主线程跑。null = 已 startActivity；否则是原因。 */
  private fun openOnMain(ctx: Context, path: String, mime: String?): String? {
    return try {
      val file = File(path)
      if (!file.exists()) {
        "文件不存在（可能已被清理）"
      } else {
        val authority = ctx.packageName + AUTHORITY_SUFFIX
        val uri: Uri = FileProvider.getUriForFile(ctx, authority, file)
        val intent = Intent(Intent.ACTION_VIEW).apply {
          setDataAndType(uri, resolveMime(path, mime))
          addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
          addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        try {
          ctx.startActivity(intent)
          null
        } catch (e: ActivityNotFoundException) {
          "没有可以打开这种文件的应用，请先装一个"
        }
      }
    } catch (t: Throwable) {
      // 路径不在 FileProvider 的根里（IllegalArgumentException）、系统拒绝等
      val detail = t.message?.takeIf { it.isNotBlank() }?.let { "：$it" } ?: ""
      "打开失败（${t.javaClass.simpleName}）$detail"
    }
  }

  /** 扩展名 → MIME；认不出来就用 `*/*`（让系统列出所有候选应用）。 */
  private fun resolveMime(path: String, hint: String?): String {
    hint?.trim()?.takeIf { it.isNotEmpty() && it != "*/*" }?.let { return it }
    val ext = path.substringAfterLast('.', "").lowercase()
    if (ext.isNotEmpty()) {
      MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext)?.let { return it }
    }
    return "*/*"
  }
}

/** Kotlin → Rust：把 JavaVM 与 `OpenWith` 类引用交给 Rust（见本文件顶部注释 2）。 */
external fun nativeAttachOpenWith()
