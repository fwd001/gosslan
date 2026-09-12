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

use jni::objects::{Global, JClass, JObject, JString};
use jni::vm::JavaVM;
use jni::{jni_sig, jni_str, native_method, Env, JValue, NativeMethod};

use crate::jni_method::kotlin_method;

/// JVM 句柄（`bootstrap` 时缓存；之后从任意线程 attach 使用）。
static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();
/// `OpenWith` 对象类的全局引用（同上，只能在有 App 类加载器在栈上时取）。
static KOTLIN_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// 由 Kotlin 的 `OpenWith.bootstrap` 调用（`extern` ⇒ 宏直接导出 JNI 符号名）。
const NATIVE_ATTACH: NativeMethod = native_method! {
    java_type = "com.gosslan.app.OpenWithKt",
    extern fn native_attach_open_with() -> (),
    fn = native_attach,
};

/// 桥是否已就绪（供上层给出准确提示，而不是模糊的“打开失败”）。
pub fn ready() -> bool {
    // ⚠️ 这一行不是摆设：`NATIVE_ATTACH` 是 `native_method!` 展开出的 `const { … }` 块，
    // 块里那个 `#[export_name]` 的函数才是 JVM 要找的符号。整块常量如果**完全没人用**，
    // 就不保证被代码生成进产物 ⇒ 真机上 `nativeAttachOpenWith` 直接 UnsatisfiedLinkError
    //（只在真机、且只在 release 包里现形的典型症状）。取一次引用把它钉进产物。
    let _keep_exported = std::hint::black_box(&NATIVE_ATTACH);
    JAVA_VM.get().is_some() && KOTLIN_CLASS.get().is_some()
}

/// `OpenWith.bootstrap()`：缓存 VM 与类引用。
fn native_attach<'local>(
    env: &mut Env<'local>,
    this: JObject<'local>,
) -> jni::errors::Result<()> {
    if JAVA_VM.get().is_none() {
        if let Ok(vm) = env.get_java_vm() {
            let _ = JAVA_VM.set(vm);
        }
    }
    if KOTLIN_CLASS.get().is_none() {
        let class = env.get_object_class(&this)?;
        let global = env.new_global_ref(class)?;
        let _ = KOTLIN_CLASS.set(global);
    }
    Ok(())
}

/// 打开本地文件。`Ok(())` = 已交给系统（**不代表**用户已经看完）；
/// `Err(原因)` = 给用户看的中文原因（Kotlin 侧把异常翻译好了）。
pub fn open_path(path: &str, mime: &str) -> Result<(), String> {
    if !ready() {
        return Err("打开文件的能力还没准备好（MainActivity 未调用 OpenWith.bootstrap）".to_string());
    }
    let class = KOTLIN_CLASS.get().expect("ready() 已确认类引用存在");
    let vm = JAVA_VM.get().expect("ready() 已确认 JavaVM 存在");

    let outcome = vm.attach_current_thread(
        |env| -> jni::errors::Result<Result<(), String>> {
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
        },
    );

    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(message)) => Err(message),
        Err(e) => Err(format!("JNI 调用失败：{e}")),
    }
}
