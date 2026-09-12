package com.gosslan.app

import android.content.Context
import android.net.wifi.WifiManager
import android.util.Log

/**
 * 持有 WiFi **组播锁**（`WifiManager.MulticastLock`）。
 *
 * ## 为什么必须持有它
 * 局域网发现靠组播（`239.255.42.99`）+ 广播（`255.255.255.255`）互相 announce。
 * 而 Android 的 WiFi 驱动**默认丢弃组播帧**（省电策略），只有应用持有 MulticastLock
 * 时才会投递给它 —— 这就是"两台设备连着同一个 WiFi、权限也都给了，却互相搜不到"
 * 最常见的原因，而且它**不是**用户能在权限列表里点开的开关。
 *
 * ## 生命周期
 * 锁必须在应用的整个前台期持有：这里在 `MainActivity.onCreate` 拿一次并**一直持有**
 * （不释放；进程结束由系统回收）。持锁会增加少量耗电，但这是局域网可用的前提。
 */
object LanMulticast {
    private const val TAG = "GosslanLAN"
    private var lock: WifiManager.MulticastLock? = null

    /** 幂等：重复调用只申请一次。失败只记日志（没有 WiFi 时局域网本来也用不了）。 */
    @JvmStatic
    fun acquire(context: Context) {
        if (lock?.isHeld == true) return
        try {
            val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            if (wifi == null) {
                Log.i(TAG, "没有 WifiManager：跳过组播锁（可能是无 WiFi 设备）")
                return
            }
            val l = wifi.createMulticastLock("gosslan-lan")
            l.setReferenceCounted(true)
            l.acquire()
            lock = l
            Log.i(TAG, "已持有 WiFi 组播锁（局域网发现依赖它）")
        } catch (e: Exception) {
            // 缺 CHANGE_WIFI_MULTICAST_STATE 或系统限制：只记日志，不影响其它功能
            Log.w(TAG, "获取 WiFi 组播锁失败：${e.message}")
        }
    }
}
