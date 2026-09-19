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

  /**
   * 扩展名 → MIME；认不出来就用通配类型（让系统列出所有候选应用）。
   *
   * ⚠️ 别在注释里写那两个字符的 MIME 通配（星号加斜杠加星号）：它就是块注释的结束标记，
   * Kotlin 编译器会把注释当场截断 —— 这一行我第一版真踩了（`Expecting member declaration` 一片）。
   */
  private fun resolveMime(path: String, hint: String?): String {
    hint?.trim()?.takeIf { it.isNotEmpty() && it != "*/*" }?.let { return it }
    val ext = path.substringAfterLast('.', "").lowercase()
    if (ext.isNotEmpty()) {
      MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext)?.let { return it }
    }
    return "*/*"
  }

  /**
   * 把应用私有文件写到系统「另存为」对话框返回的 content:// URI。
   *
   * 返回 null 表示成功；否则是给用户看的中文原因。
   * 与 [openWith] 不同：这里只是 IO，不需要回主线程；但同样把所有异常转成原因，
   * 绝不让 Java 异常穿透 JNI（非主线程抛出的异常会让进程静默消失）。
   */
  @JvmStatic
  fun saveWith(path: String, uriString: String): String? {
    val ctx = appContext ?: return "应用还没准备好，请稍后重试"
    return try {
      val src = File(path)
      if (!src.exists()) {
        "文件不存在（可能已被清理）"
      } else {
        val uri = Uri.parse(uriString)
        ctx.contentResolver.openOutputStream(uri, "wt")?.use { out ->
          src.inputStream().use { input -> input.copyTo(out) }
          out.flush()
          null
        } ?: "无法写入所选位置"
      }
    } catch (t: Throwable) {
      val detail = t.message?.takeIf { it.isNotBlank() }?.let { "：$it" } ?: ""
      "保存失败（${t.javaClass.simpleName}）$detail"
    }
  }

  /**
   * 把一段字节（图片另存等）写到系统「另存为」对话框返回的 content:// URI。
   *
   * 为什么单独一个方法：图片保存走的是 base64 数据而不是文件路径，Rust 侧解码后
   * 需要一条写字节的通道。返回值与 [saveWith] 相同。
   */
  @JvmStatic
  fun writeBytesWith(bytes: ByteArray, uriString: String): String? {
    val ctx = appContext ?: return "应用还没准备好，请稍后重试"
    return try {
      val uri = Uri.parse(uriString)
      ctx.contentResolver.openOutputStream(uri, "wt")?.use { out ->
        out.write(bytes)
        out.flush()
        null
      } ?: "无法写入所选位置"
    } catch (t: Throwable) {
      val detail = t.message?.takeIf { it.isNotBlank() }?.let { "：$it" } ?: ""
      "保存失败（${t.javaClass.simpleName}）$detail"
    }
  }

  /**
   * 把 HEIC/HEIF 等跨平台不兼容格式转成 JPEG（质量 85%）。
   *
   * ## 为什么需要这一步
   *
   * 一加、小米等国产 Android 默认相机输出 HEIC（高效存储），iPhone 更是从 iOS 11 起
   * 全面转 HEIC。但 Mac/Windows 前端的 `<img src>` 对 HEIC 支持极差：
   *   - macOS 13 以下原生预览都打不开
   *   - 所有浏览器（Chrome/Safari/Firefox）截至 2026 年对 HEIC 的原生解码
   *     仍然依赖操作系统，不保证跨平台可用
   * 结果就是：Android 发原图 → Mac 显示"裂开图标"。
   *
   * ## 实现要点
   *
   * - Android 9（API 28）起 `BitmapFactory` 原生支持 HEIC 解码，不需要三方库。
   * - 质量 85%：视觉无损（人眼几乎看不出差别），但跨平台 100% 兼容。
   * - 保持分辨率不降采样 — 只改编码格式。
   * - 返回 null 表示转码失败（文件不存在 / 格式无法识别 / BitmapFactory 抛异常）。
   * - 所有异常必须被捕获并返回 null — 绝不能穿透 JNI 让进程静默消失。
   */
  @JvmStatic
  fun convertHeicToJpeg(inputPath: String): String? {
    val ctx = appContext ?: return null
    val file = File(inputPath)
    if (!file.exists() || !file.canRead()) return null
    return try {
      // BitmapFactory.decodeFile 在 Android 9+ 自动识别 HEIC/HEIF
      val bitmap = android.graphics.BitmapFactory.decodeFile(inputPath) ?: return null
      try {
        val out = File(
          file.parentFile,
          file.nameWithoutExtension + ".jpg"
        )
        java.io.FileOutputStream(out).use { fos ->
          bitmap.compress(android.graphics.Bitmap.CompressFormat.JPEG, 85, fos)
          fos.flush()
        }
        if (out.exists() && out.length() > 0) out.absolutePath else null
      } finally {
        bitmap.recycle()
      }
    } catch (t: Throwable) {
      android.util.Log.w("GosslanImg", "HEIC 转 JPEG 失败 ${t.javaClass.simpleName}: ${t.message}")
      null
    }
  }

  /**
   * 判断视频文件是不是 HEVC (H.265) 编码。
   *
   * 为什么要检测：一加/小米等国产 Android 默认用 HEVC 拍视频（省空间），
   * 但 Mac/Windows 浏览器对 HEVC 支持极差（Chrome/Firefox/Safari 都不能原生解码）。
   * 我们在 Manifest 里声明了 HEVC 不支持 → Android 12+ 系统会在 ContentResolver 读取时
   * 自动转 H.264；这个检测方法让我们能在 Rust 侧**提前知道**，做日志/跳过转码兜底。
   *
   * - MediaExtractor 拿视频轨的 MIME：`video/hevc` → true，`video/avc` → false
   * - API < 21（MediaExtractor）→ 用 ftyp box 特征判断（`ftyphev1`/`ftyphvc1`）
   * - 返回 false 表示不是 HEVC 或检测失败（用原文件继续发）
   */
  @JvmStatic
  fun isHevcVideo(path: String): Boolean {
    val ctx = appContext ?: return false
    val file = File(path)
    if (!file.exists() || !file.canRead()) return false
    return try {
      if (android.os.Build.VERSION.SDK_INT >= 21) {
        val extractor = android.media.MediaExtractor()
        try {
          extractor.setDataSource(path)
          for (i in 0 until extractor.trackCount) {
            val format = extractor.getTrackFormat(i)
            val mime = format.getString(android.media.MediaFormat.KEY_MIME) ?: continue
            if (mime.startsWith("video/")) {
              return mime.contains("hevc") || mime.contains("h265")
            }
          }
          false
        } finally {
          extractor.release()
        }
      } else {
        // API < 21 fallback：读文件头找 ftyp box
        val head = file.inputStream().use { it.readNBytes(16) }
        val bytes = String(head, Charsets.US_ASCII)
        bytes.contains("ftyphev1") || bytes.contains("ftyphvc1")
      }
    } catch (t: Throwable) {
      android.util.Log.w("GosslanVideo", "HEVC 检测失败 ${t.javaClass.simpleName}: ${t.message}")
      false
    }
  }

  /**
   * 判断文件是不是一加/小米/Google 的动态照片 / Motion Photo。
   *
   * 为什么要检测：
   * - 微信/QQ/钉钉/飞书**全部**只发静态封面（动效丢失），行业统一做法
   * - 一加 ColorOS Motion Photo、小米澎湃 OS Micro Video、Google Motion Photo
   *   都是"JPEG 容器尾部追加 MP4 数据 + XMP 元数据"的单文件结构
   * - 跨平台发动态效果需要端到端重构（解析 XMP → 拆出 JPEG + MP4 双发 → 接收端 MotionPhotoView）
   *   — 复杂度极高，没有国内 IM 真正做过
   *
   * 检测方法（CSDN 深度解析方案）：
   *   1. **XMP 元数据**：文件里找 `Camera:MotionPhoto>1<`（Google 标准）
   *   2. **MP4 尾部特征**：JPEG 结束标记 (0xFFD9) 之后找 `ftypmp42` 或 `ftypisom`
   *
   * 检测出来后 Rust 侧会记录日志，但**不做特殊处理** — 直接发整个文件，
   * 接收端只看到 JPEG 静态封面（和国内 IM 行为一致）。
   */
  @JvmStatic
  fun isMotionPhoto(path: String): Boolean {
    val ctx = appContext ?: return false
    val file = File(path)
    if (!file.exists() || !file.canRead()) return false
    return try {
      // 不要用 file.readBytes() — Motion Photo 可以 30-50MB，readBytes 直接 OOM。
      // 按职责分区读：
      //   1. XMP 在文件头 64KB 里（Google/一加/小米 统一放在文件头）
      //   2. MP4 尾部特征在文件最后 1KB 里（Motion Photo 的视频数据追加在 JPEG 尾部）
      val head = file.inputStream().use { it.readNBytes(64 * 1024) }
      val headStr = String(head, Charsets.US_ASCII)
      // 1. XMP 元数据检测（Google/一加/小米 统一用 XMP 容器结构）
      if (headStr.contains("MotionPhoto>1<") && headStr.contains("Camera")) {
        return true
      }
      if (headStr.contains("Container:Semantic>MotionPhoto<")) {
        return true
      }
      // 2. MP4 尾部特征：JPEG 结束标记 (0xFFD9) 之后找 ftyp box
      // 先在文件头排除普通 MP4（普通 MP4 文件头也有 ftyp）
      val headHasFtyp = headStr.contains("ftypmp42") || headStr.contains("ftypisom")
      // 先确认是 JPEG 文件（文件头两个字节是 0xFF 0xD8，这是 JPEG SOI 标记）
      val isJpeg = head.size >= 2 && head[0] == 0xFF.toByte() && head[1] == 0xD8.toByte()
      // 普通 MP4 排除：文件头有 ftyp + 不是 JPEG 开头 → 普通 MP4 不是 Motion Photo
      if (headHasFtyp && !isJpeg) return false
      // Motion Photo 特征：JPEG 开头 + 尾部有 MP4 ftyp
      if (isJpeg && file.length() > 64 * 1024) {
        // 读尾部 1KB 找 ftyp
        val tail = file.inputStream().use { stream ->
          stream.skip(file.length() - 1024)
          String(stream.readNBytes(1024), Charsets.US_ASCII)
        }
        if (tail.contains("ftypmp42") || tail.contains("ftypisom")) {
          return true
        }
      }
      false
    } catch (t: Throwable) {
      android.util.Log.w("GosslanImg", "Motion Photo 检测失败 ${t.javaClass.simpleName}: ${t.message}")
      false
    }
  }
}

/** Kotlin → Rust：把 JavaVM 与 `OpenWith` 类引用交给 Rust（见本文件顶部注释 2）。 */
external fun nativeAttachOpenWith()
