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
# GOSSLAN_JNI_END
