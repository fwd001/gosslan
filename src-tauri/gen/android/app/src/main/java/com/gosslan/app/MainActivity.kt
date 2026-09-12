package com.gosslan.app

import android.os.Bundle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // BLE 外设角色（手机被电脑连）需要把 JavaVM 与类引用交给 Rust 侧：
    // 必须在 App 代码还在栈上时做（JNI 的 FindClass 依赖调用方的类加载器）。
    // 没开 bluetooth feature 时这行是安全的空操作（内部捕获 UnsatisfiedLinkError）。
    // 传 **this**（Activity）而不是 applicationContext：运行时权限弹框只能由 Activity 发起
    BlePeripheral.bootstrap(this)
    // 局域网发现依赖组播/广播，Android 必须由应用持有组播锁才收得到（见 LanMulticast 注释）。
    LanMulticast.acquire(applicationContext)
  }
}
