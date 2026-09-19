//! Android：把**应用私有目录**里的文件交给系统里的其它应用打开。
//!
//! 真实缺陷（用户 2026-09-12 安卓真机实测）：点已收到的文件 → 前端提示「文件打开失败」。
//! 根因是 `tauri-plugin-opener` 在 Android 上只会做 `Intent(ACTION_VIEW, url.toUri())`
//! （见插件源码 `OpenerPlugin.kt`），而我们交给它的是 `file:///data/user/0/com.gosslan.app/...`
//! —— **Android 7.0+ 禁止应用把私有文件以 `file://` 暴露给别的应用**，`startActivity`
//! 当场抛 `FileUriExposedException`（前端只看到一个笼统的失败）。
//!
//! 正确做法是 FileProvider：把私有文件映射成 `content://<包名>.fileprovider/…`，
//! 并在 intent 上带 `FLAG_GRANT_READ_URI_PERMISSION`。这部分只能在 Kotlin 侧做
//! （要 `Context` / `FileProvider`），所以本模块只负责 JNI 桥：
//!
//! ```text
//! MainActivity.onCreate ──► OpenWith.bootstrap(ctx) ──► nativeAttachOpenWith()  [本模块]
//!                                                        └─ 缓存 JavaVM + OpenWith 类引用
//! Rust open_file_native ──► android_open::open_path() ──► OpenWith.openWith(path, mime)
//!                                                        └─ FileProvider + ACTION_VIEW（主线程）
//! ```
//!
//! 两个非显然点，都是踩过的坑：
//! 1. **JavaVM / 类引用必须由 Kotlin 侧“带过来”**：JNI 的 `FindClass` 依赖调用方的类加载器，
//!    在 tokio 工作线程上按名字找 App 的类会失败。这与 `transport/ble_android.rs` 同一个套路
//!    （那里是 `nativeBootstrap`）。
//! 2. **`jni` 依赖不能挂在 `bluetooth` feature 上**：打开文件与蓝牙无关，见 `Cargo.toml` 注释。

use std::sync::OnceLock;

use jni::objects::{Global, JClass, JString};
use jni::vm::JavaVM;
use jni::{jni_sig, jni_str, native_method, Env, JValue, NativeMethod};

use crate::jni_method::kotlin_method;

/// JVM 句柄（`bootstrap` 时缓存；之后从任意线程 attach 使用）。
static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();
/// `OpenWith` **object 类**的全局引用 — 所有业务方法（openWith/saveWith/convertHeicToJpeg/isHevcVideo/isMotionPhoto）
/// 都在这个类上（加了 @JvmStatic）。注意：native_attach 的 class 参数是 `OpenWithKt`
/// （顶层函数所在类），不是 `OpenWith` 本身，所以这里必须手动 find_class。
static OPEN_WITH_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// 由 Kotlin 的 `OpenWith.bootstrap` 调用（`extern` ⇒ 宏直接导出 JNI 符号名）。
///
/// ⚠️ **必须是 `static`**：Kotlin 侧 `nativeAttachOpenWith()` 是 **文件级（顶层）函数** ——
/// 编译成 `OpenWithKt` 的 **static** 方法。宏如果按"实例方法"注册（没有 `static` 关键字），
/// ART 会当场判定不一致并 **abort 整个进程**（真机实测的启动闪退）：
///   `Native method '"nativeAttachOpenWith"' was registered as instance but called as static method`
/// 对照：`BlePeripheral` 里的 `external fun nativeBootstrap()` 在 object 内部 ⇒ 实例方法 ⇒
/// 那边的宏**不加** `static`（见 `transport/ble_android.rs`）。护栏
/// `jni_static_matches_kotlin_toplevel` 盯着这条对应关系。
const NATIVE_ATTACH: NativeMethod = native_method! {
    java_type = "com.gosslan.app.OpenWithKt",
    static extern fn native_attach_open_with() -> (),
    fn = native_attach,
};

/// 桥是否已就绪（供上层给出准确提示，而不是模糊的“打开失败”）。
pub fn ready() -> bool {
    // ⚠️ 这一行不是摆设：`NATIVE_ATTACH` 是 `native_method!` 展开出的 `const { … }` 块，
    // 块里那个 `#[export_name]` 的函数才是 JVM 要找的符号。整块常量如果**完全没人用**，
    // 就不保证被代码生成进产物 ⇒ 真机上 `nativeAttachOpenWith` 直接 UnsatisfiedLinkError
    //（只在真机、且只在 release 包里现形的典型症状）。取一次引用把它钉进产物。
    let _keep_exported = std::hint::black_box(&NATIVE_ATTACH);
    JAVA_VM.get().is_some() && OPEN_WITH_CLASS.get().is_some()
}

/// `OpenWith.bootstrap()`：缓存 VM 与 OpenWith 类引用。
///
/// ⚠️ native_attach 的 class 参数是 `OpenWithKt`（顶层函数所在类），
/// 但所有业务方法都在 `object OpenWith` 里 —— 所以必须额外 find_class 拿到 OpenWith。
fn native_attach<'local>(
    env: &mut Env<'local>,
    _kt_class: JClass<'local>,
) -> jni::errors::Result<()> {
    if JAVA_VM.get().is_none() {
        if let Ok(vm) = env.get_java_vm() {
            let _ = JAVA_VM.set(vm);
        }
    }
    if OPEN_WITH_CLASS.get().is_none() {
        // native_attach 在 bootstrap 调用链上，此时 App 类加载器在栈上 — find_class 安全
        let open_with = env.find_class(jni_str!("com/gosslan/app/OpenWith"))?;
        let global = env.new_global_ref(open_with)?;
        let _ = OPEN_WITH_CLASS.set(global);
    }
    Ok(())
}

/// 打开本地文件。`Ok(())` = 已交给系统（**不代表**用户已经看完）；
/// `Err(原因)` = 给用户看的中文原因（Kotlin 侧把异常翻译好了）。
pub fn open_path(path: &str, mime: &str) -> Result<(), String> {
    if !ready() {
        return Err(
            "打开文件的能力还没准备好（MainActivity 未调用 OpenWith.bootstrap）".to_string(),
        );
    }
    let class = OPEN_WITH_CLASS.get().expect("ready() 已确认类引用存在");
    let vm = JAVA_VM.get().expect("ready() 已确认 JavaVM 存在");

    let outcome = vm.attach_current_thread(|env| -> jni::errors::Result<Result<(), String>> {
        let jpath = env.new_string(path)?;
        let jmime = env.new_string(mime)?;
        let (name, sig) = kotlin_method!(
            "openWith",
            "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"
        );
        let value = env.call_static_method(
            class,
            name,
            sig,
            &[JValue::Object(&jpath), JValue::Object(&jmime)],
        )?;
        let obj = value.l()?;
        // Kotlin 侧约定：null = 已交给系统；否则就是给用户看的原因
        if obj.as_raw().is_null() {
            return Ok(Ok(()));
        }
        let message = env.cast_local::<JString>(obj)?.try_to_string(env)?;
        Ok(Err(message))
    });

    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(message)) => Err(message),
        Err(e) => Err(format!("JNI 调用失败：{e}")),
    }
}

/// 把本地文件写到系统「另存为」对话框返回的 content:// URI。
///
/// 与 open_path 完全对称，只是调用的 Kotlin 方法不同（OpenWith.saveWith）。
/// Android 的保存对话框（SAF ACTION_CREATE_DOCUMENT）返回 content:// URI，
/// 不能用 std::fs 写；必须经 ContentResolver。
pub fn save_path(path: &str, uri: &str) -> Result<(), String> {
    if !ready() {
        return Err(
            "保存文件的能力还没准备好（MainActivity 未调用 OpenWith.bootstrap）".to_string(),
        );
    }
    let class = OPEN_WITH_CLASS.get().expect("ready() 已确认类引用存在");
    let vm = JAVA_VM.get().expect("ready() 已确认 JavaVM 存在");

    let outcome = vm.attach_current_thread(|env| -> jni::errors::Result<Result<(), String>> {
        let jpath = env.new_string(path)?;
        let juri = env.new_string(uri)?;
        let (name, sig) = kotlin_method!(
            "saveWith",
            "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"
        );
        let value = env.call_static_method(
            class,
            name,
            sig,
            &[JValue::Object(&jpath), JValue::Object(&juri)],
        )?;
        let obj = value.l()?;
        if obj.as_raw().is_null() {
            return Ok(Ok(()));
        }
        let message = env.cast_local::<JString>(obj)?.try_to_string(env)?;
        Ok(Err(message))
    });

    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(message)) => Err(message),
        Err(e) => Err(format!("JNI 调用失败：{e}")),
    }
}

/// 把一段字节写到系统「另存为」对话框返回的 content:// URI（图片另存用）。
///
/// 图片保存走 base64 数据而不是文件路径，所以需要单独的 byte[] 通道。
pub fn save_bytes(bytes: &[u8], uri: &str) -> Result<(), String> {
    if !ready() {
        return Err(
            "保存文件的能力还没准备好（MainActivity 未调用 OpenWith.bootstrap）".to_string(),
        );
    }
    let class = OPEN_WITH_CLASS.get().expect("ready() 已确认类引用存在");
    let vm = JAVA_VM.get().expect("ready() 已确认 JavaVM 存在");

    let outcome = vm.attach_current_thread(|env| -> jni::errors::Result<Result<(), String>> {
        let jbytes = env.byte_array_from_slice(bytes)?;
        let juri = env.new_string(uri)?;
        let (name, sig) =
            kotlin_method!("writeBytesWith", "([BLjava/lang/String;)Ljava/lang/String;");
        let value = env.call_static_method(
            class,
            name,
            sig,
            &[JValue::Object(&jbytes), JValue::Object(&juri)],
        )?;
        let obj = value.l()?;
        if obj.as_raw().is_null() {
            return Ok(Ok(()));
        }
        let message = env.cast_local::<JString>(obj)?.try_to_string(env)?;
        Ok(Err(message))
    });

    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(message)) => Err(message),
        Err(e) => Err(format!("JNI 调用失败：{e}")),
    }
}

/// 把 HEIC/HEIF 等跨平台不兼容图片格式转成 JPEG。
///
/// ## 为什么在 Rust 侧做而不是 Kotlin 侧全搞定
///
/// Kotlin 侧 `OpenWith.convertHeicToJpeg` 返回 `String?`：成功时是新的 JPEG 路径，
/// 失败时是 null（任何异常/不认识的格式都归一成 null）。Rust 侧拿到路径后，
/// 把落地文件的引用从 HEIC 换成 JPEG — 后续发送/预览/文件名消毒全复用既有链路。
/// 如果 Kotlin 侧返回 null（比如 BitmapFactory 解不了这张），Rust 侧就**静默放弃转码**，
/// 用原文件继续走 — 绝不因为转码失败而阻塞消息发送。
///
/// ## 返回值
///
/// - `Ok(Some(new_path))` — 转码成功，新文件路径
/// - `Ok(None)` — Kotlin 侧不支持/失败，用原文件
/// - `Err(msg)` — JNI 桥本身出问题（不是图片问题）
pub fn convert_heic_to_jpeg(path: &str) -> Result<Option<String>, String> {
    if !ready() {
        return Ok(None); // 桥未就绪时放弃转码，别阻塞发送
    }
    let class = OPEN_WITH_CLASS.get().expect("ready() 已确认类引用存在");
    let vm = JAVA_VM.get().expect("ready() 已确认 JavaVM 存在");

    let outcome = vm.attach_current_thread(|env| -> jni::errors::Result<Option<String>> {
        let jpath = env.new_string(path)?;
        let (name, sig) = kotlin_method!(
            "convertHeicToJpeg",
            "(Ljava/lang/String;)Ljava/lang/String;"
        );
        let value = env.call_static_method(class, name, sig, &[JValue::Object(&jpath)])?;
        let obj = value.l()?;
        if obj.as_raw().is_null() {
            return Ok(None);
        }
        let new_path = env.cast_local::<JString>(obj)?.try_to_string(env)?;
        Ok(Some(new_path))
    });

    outcome.map_err(|e| format!("JNI 调用失败：{e}"))
}

/// 判断视频文件是不是 HEVC (H.265) 编码。
///
/// 一加/小米等国产 Android 默认用 HEVC 拍视频（省空间），但 Mac/Windows 浏览器
/// 对 HEVC 支持极差。我们在 Manifest 里声明了 HEVC 不支持 → Android 12+ 系统会在
/// ContentResolver 读取时自动转 H.264；这个检测让 Rust 侧能提前知道做日志。
///
/// - 返回 false 表示不是 HEVC 或检测失败（静默，不阻塞发送）
pub fn is_hevc_video(path: &str) -> bool {
    if !ready() {
        return false; // 桥未就绪 → 不检测
    }
    let class = OPEN_WITH_CLASS.get().expect("ready() 已确认类引用存在");
    let vm = JAVA_VM.get().expect("ready() 已确认 JavaVM 存在");

    let outcome = vm.attach_current_thread(|env| -> jni::errors::Result<bool> {
        let jpath = env.new_string(path)?;
        let (name, sig) = kotlin_method!("isHevcVideo", "(Ljava/lang/String;)Z");
        let value = env.call_static_method(class, name, sig, &[JValue::Object(&jpath)])?;
        Ok(value.z()?)
    });

    outcome.unwrap_or(false)
}

/// 判断文件是不是一加/小米/Google 的动态照片 / Motion Photo。
///
/// 微信/QQ/钉钉/飞书**全部**只发静态封面（动效丢失），行业统一做法。
/// 这个检测让 Rust 侧能做日志提示，但不做特殊处理 — 直接发整个文件。
///
/// - 返回 false 表示不是 Motion Photo 或检测失败
pub fn is_motion_photo(path: &str) -> bool {
    if !ready() {
        return false;
    }
    let class = OPEN_WITH_CLASS.get().expect("ready() 已确认类引用存在");
    let vm = JAVA_VM.get().expect("ready() 已确认 JavaVM 存在");

    let outcome = vm.attach_current_thread(|env| -> jni::errors::Result<bool> {
        let jpath = env.new_string(path)?;
        let (name, sig) = kotlin_method!("isMotionPhoto", "(Ljava/lang/String;)Z");
        let value = env.call_static_method(class, name, sig, &[JValue::Object(&jpath)])?;
        Ok(value.z()?)
    });

    outcome.unwrap_or(false)
}
