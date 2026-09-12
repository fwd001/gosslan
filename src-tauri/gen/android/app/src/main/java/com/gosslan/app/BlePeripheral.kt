package com.gosslan.app

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothStatusCodes
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.ParcelUuid
import androidx.core.app.ActivityCompat
import java.util.UUID

/**
 * BLE **外设角色**（广播 + GATT server）—— Android 侧实现（ADR-0015 的 7-f）。
 *
 * ## 为什么要有它
 * 手机只做 central 时，**别人永远发现不了手机**（btleplug 只能主动连，见 ADR-0015 §3.1）。
 * 手机做了外设之后，Windows / 另一台手机才能不经 Mac 中转直接连上它：
 *
 * ```text
 *   Windows(central) ──BLE──▶ 手机(peripheral) ──BLE/LAN──▶ 其它设备
 * ```
 *
 * ## 与 macOS 实现的关系
 * 行为契约**与 `transport/bluetooth_peripheral.rs`（CBPeripheralManager）完全一致**：
 * 同一套 UUID、同样"只放服务 UUID 的广播"、同样的写/通知语义、同样把
 * 「对端断开」当成一条 `unlinked` 事件。差异只有一处（而且是**更好**的一处）：
 * Android 的 `onConnectionStateChange` 会**真的**告诉我们对端断开了，
 * 而 CoreBluetooth 外设角色没有这个回调，只能靠写失败收尾。
 *
 * ## 线程与回调
 * 所有回调都由系统在主线程投递；这里**只做**"拷字节 + 查表 + 调 native 回调"，
 * 重活（验签、落库、加解密）全在 Rust/tokio 侧 —— 与 macOS 侧同一条红线：
 * **回调里绝不阻塞 UI**。
 *
 * ## 权限
 * Android 12+ 把蓝牙拆成三个运行时权限：SCAN（扫描）、CONNECT（连接）、
 * **ADVERTISE（广播）**。三者缺一不可，且 `startAdvertising` 缺权限时直接抛
 * `SecurityException`。本类会在 `start()` 时检查并申请。
 */
object BlePeripheral {

    // ⚠️ 与 Rust（transport/bluetooth.rs）和 macOS（bluetooth_peripheral.rs）**必须逐字一致**：
    //    这是三方互通的唯一契约，改一处就要三处一起改。
    private val SERVICE_UUID: UUID = UUID.fromString("6b1a7e60-3f4c-4d8a-9c2b-1e5f7a9d0c31")
    private val CHAR_RX_UUID: UUID = UUID.fromString("6b1a7e60-3f4c-4d8a-9c2b-1e5f7a9d0c32")
    private val CHAR_TX_UUID: UUID = UUID.fromString("6b1a7e60-3f4c-4d8a-9c2b-1e5f7a9d0c33")

    /** 订阅通知的标准描述符（CCCD）—— 客户端写它表示"我要收通知"。 */
    private val CCCD_UUID: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")

    private const val REQUEST_BLE_PERMISSIONS = 0x6201

    /** 由 `MainActivity.onCreate` 传入的应用 Context（取系统服务靠它）。 */
    private var appContext: Context? = null

    /** **Activity** 引用：运行时权限弹框只能由 Activity 发起（applicationContext 不行）。 */
    private var activityRef: Activity? = null

    private var manager: BluetoothManager? = null
    private var gattServer: BluetoothGattServer? = null
    private var advertiser: BluetoothLeAdvertiser? = null
    private var rxChar: BluetoothGattCharacteristic? = null
    private var txChar: BluetoothGattCharacteristic? = null

    /** 已连接/已订阅的对端（地址 → 设备），以及协商到的 ATT MTU。 */
    private val connected = HashMap<String, BluetoothDevice>()
    private val mtu = HashMap<String, Int>()

    // ------------------------------------------------------------------
    // 由 Rust 注册的 native 回调（见 transport/ble_android.rs）
    // ------------------------------------------------------------------

    /** 收到**一片** ATT 写入（原样转发；分片重组在 Rust 侧，与 macOS 共用 `ble_framing`）。 */
    private external fun nativeOnFrame(address: String, value: ByteArray)

    /** 对端断开（Android 会给这个回调，比 CoreBluetooth 强）。 */
    private external fun nativeOnUnlinked(address: String)

    /** 引导：把 `JavaVM` 与类引用交给 Rust（见 `bootstrap` 的注释）。 */
    private external fun nativeBootstrap()

    /** 诊断信息（Rust 侧记 info）。 */
    private external fun nativeOnNotice(text: String)

    /** 需要用户知道的问题（Rust 侧记 warn）。 */
    private external fun nativeOnWarning(text: String)

    // ------------------------------------------------------------------
    // 生命周期
    // ------------------------------------------------------------------

    /**
     * **必须**在 `MainActivity.onCreate` 里调一次。
     *
     * 为什么需要它：JNI 的 `FindClass` 用的是"调用它的那个 native 方法的类的类加载器"，
     * 从普通后台线程里找不到 App 自己的类。所以趁 App 代码还在栈上时，
     * 把 `JavaVM` 与 `BlePeripheral` 类的全局引用交给 Rust（见 `transport/ble_android.rs`）。
     *
     * `UnsatisfiedLinkError` 是**预期**的：没开 `bluetooth` feature 的构建里
     * Rust 不实现 `nativeBootstrap`，此时只记一行日志、照常运行（绝不能崩在启动路径上）。
     */
    @JvmStatic
    fun bootstrap(context: Context) {
        appContext = context.applicationContext
        // MainActivity 传进来的是它自己（Activity）——存下来用于权限弹框
        activityRef = context as? Activity
        try {
            nativeBootstrap()
        } catch (e: UnsatisfiedLinkError) {
            // 未编译蓝牙特性：外设角色不可用，但 central（btleplug）与局域网都照常
            android.util.Log.i("GosslanBLE", "未编译蓝牙外设支持（nativeBootstrap 缺失）：${e.message}")
        }
    }

    /** 打开 GATT server 并开始广播。返回是否已成功启动（失败原因经 nativeOnWarning 上报）。 */
    @JvmStatic
    /**
     * 把 Android 框架调用放到**主线程**执行（同步等待结果，最多 `timeoutMs`）。
     *
     * 为什么必须（用户 2026-09-12 安卓实测：「点『添加好友』或进『设置』就立刻闪退，
     * 不点按钮就不闪」）：Rust 命令跑在 **tokio 工作线程**上，而
     * `ActivityCompat.requestPermissions` / `openGattServer` / `startAdvertising`
     * 这类框架 API 只有在**有 Looper 的线程**（主线程）上才安全 ——
     * 从没有 Looper 的线程调用会抛 Java 异常，而 Java 层的未捕获异常会**直接杀掉进程**
     * （它不是 Rust panic，所以连我们新加的 panic hook 都抓不到、日志里什么都没有）。
     * 这里统一跳主线程 + try/catch：平台调用失败最多是"通道没开"，绝不能让应用消失。
     */
    private fun <T> onMainSync(timeoutMs: Long = 3000, block: () -> T): T? {
        if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) {
            return try {
                block()
            } catch (t: Throwable) {
                warnMain(t)
                null
            }
        }
        val latch = java.util.concurrent.CountDownLatch(1)
        var result: T? = null
        val posted = mainHandler.post {
            try {
                result = block()
            } catch (t: Throwable) {
                warnMain(t)
            } finally {
                latch.countDown()
            }
        }
        if (!posted) return null
        return if (latch.await(timeoutMs, java.util.concurrent.TimeUnit.MILLISECONDS)) result else null
    }

    /** 异步 post 到主线程（无返回值）。 */
    private fun onMain(block: () -> Unit) {
        mainHandler.post {
            try {
                block()
            } catch (t: Throwable) {
                warnMain(t)
            }
        }
    }

    private fun warnMain(t: Throwable) {
        android.util.Log.w("GosslanBLE", "主线程调用失败：$t")
        runCatching { nativeOnWarning("蓝牙操作失败：${t.message ?: t.javaClass.simpleName}") }
    }

    val mainHandler = android.os.Handler(android.os.Looper.getMainLooper())

    fun start(): Boolean = startOnMain()

    private fun startOnMain(): Boolean = onMainSync { startInner() } ?: false

    private fun startInner(): Boolean {
        if (gattServer != null) return true // 幂等
        val context = appContext
        if (context == null) {
            nativeOnWarning("蓝牙外设尚未初始化（MainActivity 未调用 BlePeripheral.bootstrap）")
            return false
        }

        val mgr = context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
        val adapter: BluetoothAdapter? = mgr?.adapter
        if (mgr == null || adapter == null || !adapter.isEnabled) {
            nativeOnWarning("蓝牙未开启或本机不支持：请打开系统蓝牙后再开启「蓝牙通道」")
            return false
        }
        if (!hasPermissions(context)) {
            // 权限缺失是最容易踩的一步：没有 ADVERTISE 时 startAdvertising 直接抛异常，
            // 用户看到的现象是"手机能扫到别人、别人永远发现不了手机"。
            requestPermissions(context)
            nativeOnWarning(
                "需要「附近的设备」权限才能被其它设备发现：请在弹窗里允许，然后重新打开「蓝牙通道」"
            )
            return false
        }

        manager = mgr
        val server = try {
            mgr.openGattServer(context, callback)
        } catch (e: SecurityException) {
            nativeOnWarning("打开 GATT server 失败（权限被拒）：${e.message}")
            return false
        }
        if (server == null) {
            nativeOnWarning("打开 GATT server 失败：系统返回 null")
            return false
        }
        gattServer = server
        server.addService(buildService())

        val adv = adapter.bluetoothLeAdvertiser
        if (adv == null) {
            nativeOnWarning("本机不支持 BLE 广播（bluetoothLeAdvertiser 为空）")
            return false
        }
        advertiser = adv
        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_BALANCED)
            .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM)
            .setConnectable(true)
            .build()
        // 广播里**只**放服务 UUID：legacy 广播只有 31 字节，128 位 UUID 占 18 字节，
        // 再放设备名就超了（与 macOS 侧同一个理由，见 advertisement_payload_bytes 的单测）。
        val data = AdvertiseData.Builder()
            .addServiceUuid(ParcelUuid(SERVICE_UUID))
            .setIncludeDeviceName(false)
            .build()
        try {
            adv.startAdvertising(settings, data, advertiseCallback)
        } catch (e: SecurityException) {
            nativeOnWarning("广播启动失败（权限被拒）：${e.message}")
            return false
        }
        nativeOnNotice("蓝牙外设角色已启动（Android 广播中，等待对端连入）")
        return true
    }

    /** 停止广播并撤掉 GATT server（幂等）。 */
    @JvmStatic
    fun stop() {
        onMain { stopInner() }
    }

    private fun stopInner() {
        try {
            advertiser?.stopAdvertising(advertiseCallback)
        } catch (_: Exception) {
            // 已经停了 / 权限被撤 —— 都不影响下面的清理
        }
        advertiser = null
        val server = gattServer
        gattServer = null
        if (server != null) {
            try {
                server.clearServices()
                server.close()
            } catch (_: Exception) {
            }
        }
        val victims = connected.keys.toList()
        connected.clear()
        mtu.clear()
        rxChar = null
        txChar = null
        // 与服务端断开一样，逐个通知 Rust 侧拆链路（不要指望系统再回调一次）
        victims.forEach { nativeOnUnlinked(it) }
        nativeOnNotice("蓝牙外设角色已停止广播")
    }

    /** 往某个对端发一条**完整帧**（分片在 Rust 侧按 MTU 切好）。 */
    @SuppressLint("MissingPermission")
    @Suppress("DEPRECATION")
    @JvmStatic
    fun send(address: String, value: ByteArray): Boolean {
        val server = gattServer ?: return false
        val characteristic = txChar ?: return false
        val device = connected[address] ?: return false
        return try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                // API 33+：值随调用传入（setValue 已废弃，且"先 setValue 再 notify"在 33+ 有竞态）。
                // ⚠️ 这个重载返回的是**状态码 Int**（不是老重载那个 Boolean），
                // 必须显式换算，否则两个分支类型不一致（编译期就会报 type mismatch）。
                server.notifyCharacteristicChanged(device, characteristic, false, value) ==
                    BluetoothStatusCodes.SUCCESS
            } else {
                characteristic.value = value
                server.notifyCharacteristicChanged(device, characteristic, false)
            }
        } catch (e: SecurityException) {
            nativeOnWarning("发送失败（权限被拒）：${e.message}")
            false
        }
    }

    /** 对端是否仍连着（未订阅也能收 —— 但发通知前 Rust 会先看这个）。 */
    @JvmStatic
    fun isConnected(address: String): Boolean = connected.containsKey(address)

    /** 该对端一次通知能收多少字节（`ATT MTU - 3`，未协商时 20）。 */
    @JvmStatic
    fun payloadMtu(address: String): Int {
        val negotiated = mtu[address] ?: return 20
        val payload = negotiated - 3
        return if (payload in 1..512) payload else 20
    }

    // ------------------------------------------------------------------
    // GATT 结构
    // ------------------------------------------------------------------

    private fun buildService(): BluetoothGattService {
        val service = BluetoothGattService(SERVICE_UUID, BluetoothGattService.SERVICE_TYPE_PRIMARY)

        val rx = BluetoothGattCharacteristic(
            CHAR_RX_UUID,
            BluetoothGattCharacteristic.PROPERTY_WRITE or
                BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE,
            BluetoothGattCharacteristic.PERMISSION_WRITE
        )
        val tx = BluetoothGattCharacteristic(
            CHAR_TX_UUID,
            BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_READ
        )
        // 没有 CCCD 客户端就无法开启通知（Android 要求显式挂这个描述符；
        // CoreBluetooth 会隐式处理，所以这是两侧唯一的结构差异）。
        tx.addDescriptor(
            BluetoothGattDescriptor(
                CCCD_UUID,
                BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE
            )
        )
        rxChar = rx
        txChar = tx
        service.addCharacteristic(rx)
        service.addCharacteristic(tx)
        return service
    }

    // ------------------------------------------------------------------
    // 回调
    // ------------------------------------------------------------------

    private val advertiseCallback = object : AdvertiseCallback() {
        override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
            nativeOnNotice("BLE 广播已生效（connectable=${settingsInEffect.isConnectable}）")
        }

        override fun onStartFailure(errorCode: Int) {
            // 与 macOS 侧同一条原则：广播失败**必须**留下一条看得懂的日志，
            // 否则现象就是"蓝牙开着却没人能发现我们"，用户完全无从下手。
            nativeOnWarning("BLE 广播失败（错误码 $errorCode）${advertiseErrorHint(errorCode)}")
        }
    }

    private fun advertiseErrorHint(code: Int): String = when (code) {
        AdvertiseCallback.ADVERTISE_FAILED_DATA_TOO_LARGE -> "：广播数据超过 31 字节"
        AdvertiseCallback.ADVERTISE_FAILED_TOO_MANY_ADVERTISERS -> "：系统广播实例已用尽"
        AdvertiseCallback.ADVERTISE_FAILED_ALREADY_STARTED -> "：已经在广播了"
        AdvertiseCallback.ADVERTISE_FAILED_INTERNAL_ERROR -> "：系统内部错误，可尝试关闭再打开蓝牙"
        AdvertiseCallback.ADVERTISE_FAILED_FEATURE_UNSUPPORTED -> "：本机不支持该广播特性"
        else -> ""
    }

    private val callback = object : BluetoothGattServerCallback() {
        override fun onServiceAdded(status: Int, service: BluetoothGattService) {
            if (status != BluetoothGatt.GATT_SUCCESS) {
                nativeOnWarning("GATT 服务注册失败（status=$status）：对端将连不上本机")
            }
        }

        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            val address = device.address ?: return
            if (newState == android.bluetooth.BluetoothProfile.STATE_CONNECTED) {
                connected[address] = device
                nativeOnNotice("对端已连接（$address）")
            } else if (newState == android.bluetooth.BluetoothProfile.STATE_DISCONNECTED) {
                connected.remove(address)
                mtu.remove(address)
                nativeOnUnlinked(address)
            }
        }

        override fun onMtuChanged(device: BluetoothDevice, newMtu: Int) {
            device.address?.let { mtu[it] = newMtu }
        }

        /** 对端写 RX：**必须先应答再处理**，否则对端每次都等到超时（GATT 语义）。 */
        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray?
        ) {
            if (responseNeeded) {
                // 应答里不需要带数据（写请求的 value 会被忽略）
                gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, 0, null)
            }
            if (characteristic.uuid != CHAR_RX_UUID) return
            val bytes = value ?: return
            // 分片重组交给 Rust（与 macOS 共用 ble_framing 的 BleReassembler）
            device.address?.let { nativeOnFrame(it, bytes) }
        }

        /** TX 的通知开关（CCCD 写）。 */
        override fun onDescriptorWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            descriptor: BluetoothGattDescriptor,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray?
        ) {
            if (responseNeeded) {
                gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, 0, null)
            }
            if (descriptor.uuid != CCCD_UUID) return
            val enabled = value != null &&
                value.isNotEmpty() &&
                value[0] == BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE[0]
            val address = device.address ?: return
            if (enabled) {
                nativeOnNotice("对端已订阅通知（$address）")
            } else {
                nativeOnNotice("对端取消订阅（$address）")
            }
        }

        /** 我们的 TX 允许读（Notify 的权限模型如此）；真被读时给个空值而不是报错。 */
        override fun onCharacteristicReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic
        ) {
            gattServer?.sendResponse(
                device,
                requestId,
                BluetoothGatt.GATT_SUCCESS,
                offset,
                ByteArray(0)
            )
        }
    }

    // ------------------------------------------------------------------
    // 权限
    // ------------------------------------------------------------------

    /**
     * 需要申请的全部运行时权限。
     *
     * - Android 12+（API 31）把蓝牙拆成 SCAN / CONNECT / **ADVERTISE** 三个运行时权限，
     *   三者缺一不可：缺 SCAN 就"扫不到别人"，缺 ADVERTISE 就"别人发现不了手机"；
     * - Android 13+（API 33）起，调用 `WifiManager`（组播锁）还需要 `NEARBY_WIFI_DEVICES`
     *   —— 不带 `neverForLocation`（已在 manifest 声明）会被要求定位权限；
     * - Android 11 及以下：这些都是**安装期**权限，无需运行时申请。
     *
     * 系统把它们统一归在「附近的设备」这一组里，所以用户只需要点一次。
     */
    private fun requiredPermissions(): List<String> {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return emptyList()
        val perms = mutableListOf(
            Manifest.permission.BLUETOOTH_SCAN,
            Manifest.permission.BLUETOOTH_CONNECT,
            Manifest.permission.BLUETOOTH_ADVERTISE,
        )
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            perms += Manifest.permission.NEARBY_WIFI_DEVICES
        }
        return perms
    }

    private fun hasPermissions(context: Context): Boolean =
        requiredPermissions().all {
            context.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
        }

    private fun requestPermissions(context: Context) {
        val activity = context as? Activity ?: return
        ActivityCompat.requestPermissions(activity, requiredPermissions().toTypedArray(), REQUEST_BLE_PERMISSIONS)
    }

    /**
     * 是否已具备全部运行时权限（Rust 侧用来判断"能不能打开通道"，从而给出准确提示）。
     * 无需申请的平台（Android 11 及以下）恒为 true。
     */
    @JvmStatic
    fun hasRequiredPermissions(): Boolean {
        val ctx = appContext ?: return false
        return requiredPermissions().all {
            ctx.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
        }
    }

    /**
     * 申请全部运行时权限（**首次启动**与"打开通道失败后重试"都走它）。
     *
     * 系统弹框是异步的：这里只负责发起；结果由用户在系统弹框里选择。
     * 没有 Activity（比如后台被拉起）时静默跳过 —— 前端会提示"去系统设置里打开"。
     */
    @JvmStatic
    fun requestAllPermissions() {
        // ⚠️ 必须回到主线程再弹框：这个方法由 Rust 从 tokio 线程经 JNI 调用，
        // 而 `ActivityCompat.requestPermissions` 只能在主线程上用（错误线程会抛 Java 异常
        // ⇒ 由系统未捕获异常处理器直接杀掉进程：表现就是"点添加好友/设置立刻闪退"）。
        onMain {
            val activity = activityRef
            if (activity == null) {
                android.util.Log.w("GosslanBLE", "没有 Activity，无法弹权限框（请在系统设置里手动打开「附近的设备」）")
                return@onMain
            }
            if (hasRequiredPermissions()) return@onMain
            ActivityCompat.requestPermissions(
                activity,
                requiredPermissions().toTypedArray(),
                REQUEST_BLE_PERMISSIONS,
            )
        }
    }
}
