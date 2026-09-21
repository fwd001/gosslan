package com.gosslan.app

import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // 权限弹框等**首帧画完**再弹：
    // 连续 post 两次 = 第一次 traversal（measure/layout/draw）结束之后才跑
    //（单次 post 会在 attach 阶段就执行，那时 WebView 还没被量过一次）。
    // 弹框若盖在首次布局上，前端读到的视口宽度会是兜底值 ⇒ 手机上判成桌面布局。
    window.decorView.post { window.decorView.post { requestRuntimePermissions() } }
    // BLE 外设角色（手机被电脑连）需要把 JavaVM 与类引用交给 Rust 侧：
    // 必须在 App 代码还在栈上时做（JNI 的 FindClass 依赖调用方的类加载器）。
    // 没开 bluetooth feature 时这行是安全的空操作（内部捕获 UnsatisfiedLinkError）。
    // 传 **this**（Activity）而不是 applicationContext：运行时权限弹框只能由 Activity 发起
    BlePeripheral.bootstrap(this)
    // 打开文件走 FileProvider（私有目录文件不能以 file:// 交给别的应用，见 OpenWith.kt）：
    // 缓存 applicationContext，并把 JavaVM / 类引用交给 Rust。同样必须在 App 代码还在栈上时做。
    OpenWith.bootstrap(applicationContext)
    // 局域网发现依赖组播/广播，Android 必须由应用持有组播锁才收得到（见 LanMulticast 注释）。
    LanMulticast.acquire(applicationContext)
  }

  /**
   * Android 13+ 运行时权限（附近设备 / 蓝牙 / 通知）。
   *
   * ⚠️ 调用点必须是 `window.decorView.post {}`（首次布局之后）：权限弹框如果盖在 WebView
   * 的**首次布局**上，前端 `matchMedia` 会读到兜底视口宽度（980px 档）⇒ 手机上判成"桌面"
   * ⇒ 首页渲染成三栏布局（用户 2026-09-21 实测）。前端另有平台优先兜底，两处互补。
   */
  private fun requestRuntimePermissions() {
    val permissions = mutableListOf<String>()
    if (Build.VERSION.SDK_INT >= 33) {
      permissions.add(Manifest.permission.NEARBY_WIFI_DEVICES)
      permissions.add(Manifest.permission.POST_NOTIFICATIONS)
    }
    if (Build.VERSION.SDK_INT >= 31) {
      permissions.add(Manifest.permission.BLUETOOTH_SCAN)
      permissions.add(Manifest.permission.BLUETOOTH_CONNECT)
    }
    val missing = permissions.filter {
      ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
    }
    if (missing.isNotEmpty()) {
      ActivityCompat.requestPermissions(this, missing.toTypedArray(), 1001)
    }
  }
}
