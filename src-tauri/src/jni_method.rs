//! JNI 方法登记宏（Android 的两条桥共用：BLE 外设 + 打开文件）。
//!
//! 为什么把「方法名」和「JNI 描述符」写在一起：JNI 调用**不做任何签名检查**
//! （写错就是运行期 `NoSuchMethodError`，而且只在真机上才现形）。集中登记后，
//! `lib.rs` 里的护栏 `android_jni_signatures_match_kotlin` 可以拿 Kotlin 源码里解析出来的
//! 描述符逐字比对。真实缺陷：`BlePeripheral.stop()` 是 Kotlin 的 Unit 方法（`()V`），
//! 此前却用 `()Z` 调用 ⇒ 用户关掉蓝牙开关后手机**仍在广播**（耗电 + 隐私），日志里什么都没有。
//!
//! ⚠️ 宏体里引用的是**调用方作用域**的 `jni_str!` / `jni_sig!`，所以每个使用它的模块
//! 都必须自己 `use jni::{jni_str, jni_sig, ...}`。

/// 登记一个 Kotlin 方法：`方法名 + JNI 描述符`。
macro_rules! kotlin_method {
    ($name:literal, $sig:literal) => {
        (jni_str!($name), jni_sig!($sig))
    };
}

pub(crate) use kotlin_method;
