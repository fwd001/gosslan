# GOSSLAN_JNI_BEGIN
# Rust 用 JNI「名字 + 签名」直接调这些 Kotlin 方法（见 src-tauri/src/transport/ble_android.rs
# 里的 `kotlin_method!`）。release 构建会开 R8（`build.gradle.kts` 的 `isMinifyEnabled = true`），
# **实测**：不 keep 的话它们会被改名成 a/b/c/d/e —— debug 包不混淆，所以这个坑只在
# release 真机包上以 NoSuchMethodError 现形（蓝牙外设这条路径整个失效，日志里也没有线索）。
# 新增 JNI 方法时必须在这里补一行；护栏测试
# `release_keeps_every_kotlin_method_called_from_rust` 会盯着这件事。
-keep class com.gosslan.app.BlePeripheral {
    public void stop();
    public boolean start();
    public boolean isConnected(java.lang.String);
    public int payloadMtu(java.lang.String);
    public boolean send(java.lang.String, byte[]);
    public void requestAllPermissions();
    public boolean hasRequiredPermissions();
    native <methods>;
}
# 「用系统里的其它应用打开收到的文件」的桥（见 gen/android/.../OpenWith.kt）。
# 两个名字都**只被 native 代码按名字**使用，R8 会把它们当成死代码改名/删掉：
#   · `OpenWith.openWith`  ← Rust 侧 JNI `call_static_method`（src-tauri/src/android_open.rs）；
#   · `OpenWithKt.nativeAttachOpenWith` ← Kotlin 调进 Rust 的 native 方法，JVM 是**按当前
#     方法名**去查符号的（`Java_com_gosslan_app_OpenWithKt_nativeAttachOpenWith`），
#     改名即 UnsatisfiedLinkError。
# 症状同样是"只有 release 真机包才现形"：点开文件 → NoSuchMethodError / UnsatisfiedLinkError。
# `native <methods>;` 里的 `<` 名字由护栏跳过（不需要在 Rust 侧有 kotlin_method! 登记）。
-keep class com.gosslan.app.OpenWith {
    public static java.lang.String openWith(java.lang.String, java.lang.String);
}
-keep class com.gosslan.app.OpenWithKt {
    native <methods>;
}
# btleplug 的 Android 后端（droidplug）用**类名**找自己的 Kotlin 实现
# （`find_class("com/nonpolynomial/btleplug/android/impl/Adapter")`）。
# R8 在 release 下会把这些类改名/删掉 —— 实测（dexdump 反查 4.1.9 的 release APK）：
# 整个 `com.nonpolynomial.btleplug.android.impl.*` **一个都不在** ⇒ 初始化失败 ⇒
# 随后 `Manager::new()` 走到 `global_adapter()` 直接 panic ⇒ 安卓 release 包
# "点添加好友/设置就闪退"（logcat 里就是那句 Droidplug has not been initialized）。
# 这是**只有 release 才会现形**的坑（debug 不混淆），所以必须显式 keep。
# 规则抄自 btleplug 官方 README（Android 一节）：它的 Java 代码**只被 native 代码按名字调用**，
# R8 会当成死代码整包删掉 —— 实测（dexdump 反查 release APK）确实一个类都不剩，
# 于是 `platform::init()` 的 find_class 失败 ⇒ 之后 `Manager::new()` panic ⇒ 闪退。
-keep class com.nonpolynomial.** { *; }
-keep class io.github.gedgygedgy.** { *; }
-dontwarn com.nonpolynomial.**
-dontwarn io.github.gedgygedgy.**
# GOSSLAN_JNI_END
