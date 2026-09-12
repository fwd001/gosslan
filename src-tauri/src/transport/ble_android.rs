//! Android BLE **外设角色**的 Rust 侧（ADR-0015 §7.7 / 7-f）。
//!
//! ## 分工
//! 平台 API（`BluetoothLeAdvertiser` + `BluetoothGattServer`）在 Kotlin 侧
//! （`gen/android/app/src/main/java/com/gosslan/app/BlePeripheral.kt`），
//! 因为 GATT 的**回调必须是一个 Java 对象**（`BluetoothGattServerCallback`），
//! 纯 Rust 无法实现它。Rust 侧负责：
//!   * 缓存 `JavaVM` 与 Kotlin 类的全局引用（只在 `bootstrap` 里取一次，见下）；
//!   * 把 Kotlin 回调过来的**分片**重组成整帧（复用 `ble_framing`，与 macOS 同一套）；
//!   * 把整帧投给网络层（`PeripheralEvent`，与 macOS 同构）；
//!   * 反向：把网络层要发的帧按 MTU 分片后调用 Kotlin 的 `send(address, bytes)`。
//!
//! ## 为什么需要 `bootstrap`
//! JNI 的 `FindClass` 用的是"**调用它的那个 native 方法的类的类加载器**"。
//! 从一个普通的 tokio 线程里 `FindClass("com/gosslan/app/BlePeripheral")` 会走
//! **系统类加载器** ⇒ 找不到 App 自己的类。因此：
//!   1. Kotlin 在 `MainActivity.onCreate` 里调一次 `BlePeripheral.bootstrap(context)`；
//!   2. bootstrap 里调 `nativeBootstrap()`（这个**符号名**由 `native_method!` 的
//!      `extern` 直接导出，JVM 按名字就能解析，不需要事先注册）；
//!   3. 我们在那次调用里拿到 `JavaVM` 和**类的全局引用**，之后从任何线程都能安全使用。
//! 其余三个回调在 `nativeBootstrap` 里用 `register_native_methods` 显式注册 ——
//! 这样"签名写错了"会立刻以 `NoSuchMethodError` 暴露出来，而不是等到真机收发时才静默失效。
//!
//! ## 没开 `bluetooth` feature 时
//! Kotlin 侧的 `bootstrap` 会捕获 `UnsatisfiedLinkError`（Rust 没实现 `nativeBootstrap`），
//! 因此**默认构建照常可用**，不会因为少一个 native 实现而崩在启动路径上。
#![cfg(all(feature = "bluetooth", target_os = "android"))]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use jni::objects::{Global, JByteArray, JClass, JObject, JString};
use jni::vm::JavaVM;
use jni::{jni_sig, jni_str, native_method, Env, JValue, NativeMethod};
use tokio::sync::{mpsc, oneshot};

use crate::transport::ble_framing::{self, BleReassembler, PushOutcome};

/// 登记一个 Kotlin 方法：`方法名 + JNI 描述符`。
///
/// 为什么把两者写在一起：JNI 调用**不做任何签名检查**（写错就是运行期
/// `NoSuchMethodError`，而它只在真机上才现形）。集中登记后，`lib.rs` 里的护栏可以直接
/// 拿 Kotlin 源码里解析出来的描述符逐字比对 —— 真实缺陷：`stop()` 是 Kotlin 的 Unit 方法
/// （`()V`），此前却用 `()Z` 调用 ⇒ 关掉蓝牙开关后手机**仍在广播**（耗电 + 隐私），
/// 而且日志里什么都没有。
macro_rules! kotlin_method {
    ($name:literal, $sig:literal) => {
        (jni_str!($name), jni_sig!($sig))
    };
}

/// 发一帧的重试上限（对端还没订阅/通知队列满时等一等）。
const WRITE_DEADLINE: Duration = Duration::from_secs(8);
/// 两次重试之间的间隔。
const WRITE_RETRY_WAIT: Duration = Duration::from_millis(20);
/// 等 CoreBluetooth/Android 上报启动结果的窗口（与 macOS 侧同口径，供 `network/ble.rs` 复用）。
pub const STATE_WAIT: Duration = Duration::from_secs(3);

/// 与 macOS 侧**同构**的事件（`network/ble.rs` 两边共用同一套处理代码）。
#[derive(Debug)]
pub enum PeripheralEvent {
    /// 一条**完整帧**（分片重组在 Rust 侧完成）。
    Frame { central: String, bytes: Vec<u8> },
    /// 对端断开（Android 会真的回调 `onConnectionStateChange`）。
    Unlinked { central: String },
    /// 诊断信息（上层记 info）。
    Notice(String),
    /// 需要用户知道的问题（上层记 warn）。
    Warning(String),
}

/// 往某个对端发帧的句柄（无状态：JNI 调用走全局缓存的 VM + 类引用）。
#[derive(Clone)]
pub struct PeripheralWriter;

/// 已启动的外设角色。
pub struct PeripheralServer {
    pub events: mpsc::UnboundedReceiver<PeripheralEvent>,
    pub writer: PeripheralWriter,
}

impl PeripheralServer {
    /// 停止广播并关掉 GATT server（幂等）。
    pub fn stop(&self) {
        let class = match kotlin_class() {
            Ok(c) => c,
            Err(_) => return,
        };
        let (name, sig) = kotlin_method!("stop", "()V");
        if let Err(e) = with_env(|env| {
            env.call_static_method(class, name, sig, &[])?;
            Ok(())
        }) {
            // 这里失败意味着外设**没有真正停掉**（仍在广播）—— 必须留痕，别静默
            send_event(PeripheralEvent::Warning(format!(
                "停止 BLE 外设失败（可能仍在广播）：{e}"
            )));
        }
    }
}

/// 与 macOS 侧同形的启动结果（`network/ble.rs` 不需要区分平台）。
pub struct PeripheralStart {
    pub server: PeripheralServer,
    /// Android 的 `startAdvertising` 是异步的，真正的成败由回调经 `Warning` 上报；
    /// 这里只回报"调用本身有没有抛异常/被拒"。
    pub state: oneshot::Receiver<Result<(), String>>,
}

// ---------------------------------------------------------------------------
// 全局状态
// ---------------------------------------------------------------------------

/// 回调线程把事件投进来的通道（`start()` 时建立）。
static EVENTS: Mutex<Option<mpsc::UnboundedSender<PeripheralEvent>>> = Mutex::new(None);
/// `BlePeripheral` 类的全局引用 —— 只在 `nativeBootstrap`（有 App 类加载器在栈上）里取。
static KOTLIN_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();
/// JVM 句柄。
static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();
/// 每个对端的分片重组器（带 30s TTL，与 macOS 侧同一个实现）。
static REASSEMBLERS: Mutex<Option<HashMap<String, BleReassembler>>> = Mutex::new(None);

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn send_event(ev: PeripheralEvent) {
    if let Some(tx) = EVENTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(ev);
    }
}

/// 在 JVM 上执行一段 JNI 调用（自动 attach/detach 当前线程）。
fn with_env<T>(
    f: impl FnOnce(&mut Env) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let vm = JAVA_VM.get().ok_or_else(|| {
        "Android BLE 外设尚未初始化（MainActivity 未调用 BlePeripheral.bootstrap）".to_string()
    })?;
    vm.attach_current_thread(f).map_err(|e| format!("JNI 调用失败：{e}"))
}

fn kotlin_class() -> Result<&'static Global<JClass<'static>>, String> {
    KOTLIN_CLASS
        .get()
        .ok_or_else(|| "Android BLE 外设类引用缺失（bootstrap 未执行）".to_string())
}

fn call_static_bool(name: &str, args: &[JValue]) -> Result<bool, String> {
    let class = kotlin_class()?;
    with_env(|env| {
        let (short, sig) = match name {
            "start" => kotlin_method!("start", "()Z"),
            "isConnected" => kotlin_method!("isConnected", "(Ljava/lang/String;)Z"),
            other => panic!("未登记的 Kotlin 方法：{other}"),
        };
        let value = env.call_static_method(class, short, sig, args)?;
        Ok(value.z()?)
    })
}

// ---------------------------------------------------------------------------
// 启动 / 发送
// ---------------------------------------------------------------------------

/// 启动外设角色（同步，不阻塞）。失败原因会说明"是权限、还是没 bootstrap"。
pub fn start() -> Result<PeripheralStart, String> {
    if KOTLIN_CLASS.get().is_none() {
        return Err("Android BLE 外设尚未初始化（MainActivity 应先调用 BlePeripheral.bootstrap）"
            .to_string());
    }
    let (tx, rx) = mpsc::unbounded_channel();
    *EVENTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    *REASSEMBLERS.lock().unwrap_or_else(|e| e.into_inner()) = Some(HashMap::new());
    send_event(PeripheralEvent::Notice(
        "Android BLE 外设桥已就绪（JNI 调用与回调均已注册）".to_string(),
    ));

    let (state_tx, state_rx) = oneshot::channel();
    let result = call_static_bool("start", &[]);
    let reported = match result {
        Ok(true) => Ok(()),
        // Kotlin 侧已经把具体原因（权限 / 不支持 / 广播失败）作为 Warning 事件发出来了
        Ok(false) => Err("Android BLE 外设未能启动（详见日志中的具体原因）".to_string()),
        Err(e) => Err(e),
    };
    let _ = state_tx.send(reported);

    Ok(PeripheralStart {
        server: PeripheralServer {
            events: rx,
            writer: PeripheralWriter,
        },
        state: state_rx,
    })
}

impl PeripheralWriter {
    /// 该对端一次通知能收多少字节（未知 ⇒ 20，与 macOS 侧同口径）。
    pub fn payload_mtu(&self, central: &str) -> usize {
        call_static_int("payloadMtu", central)
            .map(|v| if (1..=512).contains(&v) { v as usize } else { 20 })
            .unwrap_or(20)
    }

    /// 对端是否还连着。
    pub fn is_subscribed(&self, central: &str) -> bool {
        call_static_bool_arg("isConnected", central).unwrap_or(false)
    }

    /// 发一条完整帧：按 MTU 分片，逐片调 Kotlin 的 `send`；对端还没订阅就等一等再试。
    pub async fn send_frame(&self, central: &str, payload: &[u8]) -> Result<usize, String> {
        let mtu = self.payload_mtu(central);
        let chunks = ble_framing::fragment(payload, mtu, next_msg_id())
            .ok_or_else(|| format!("帧无法分片（过大或 MTU 非法：len={} mtu={mtu}）", payload.len()))?;
        let deadline = tokio::time::Instant::now() + WRITE_DEADLINE;
        for chunk in &chunks {
            loop {
                if self.is_subscribed(central) && call_static_send(central, chunk)? {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!("等待对端接收窗口超时（central={central}）"));
                }
                tokio::time::sleep(WRITE_RETRY_WAIT).await;
            }
        }
        Ok(chunks.len())
    }
}

fn next_msg_id() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn call_static_int(name: &str, arg: &str) -> Result<i32, String> {
    let class = kotlin_class()?;
    with_env(|env| {
        let jarg = env.new_string(arg)?;
        let (short, sig) = match name {
            "payloadMtu" => kotlin_method!("payloadMtu", "(Ljava/lang/String;)I"),
            other => panic!("未登记的 Kotlin 方法：{other}"),
        };
        let value = env.call_static_method(class, short, sig, &[JValue::Object(&jarg)])?;
        Ok(value.i()?)
    })
}

fn call_static_bool_arg(name: &str, arg: &str) -> Result<bool, String> {
    let class = kotlin_class()?;
    with_env(|env| {
        let jarg = env.new_string(arg)?;
        let short = match name {
            "isConnected" => jni_str!("isConnected"),
            other => panic!("未登记的 Kotlin 方法：{other}"),
        };
        let value = env.call_static_method(
            class,
            short,
            jni_sig!("(Ljava/lang/String;)Z"),
            &[JValue::Object(&jarg)],
        )?;
        Ok(value.z()?)
    })
}

fn call_static_send(address: &str, bytes: &[u8]) -> Result<bool, String> {
    let class = kotlin_class()?;
    with_env(|env| {
        let jaddr = env.new_string(address)?;
        let jbytes = env.byte_array_from_slice(bytes)?;
        let (name, sig) = kotlin_method!("send", "(Ljava/lang/String;[B)Z");
        let value = env.call_static_method(
            class,
            name,
            sig,
            &[JValue::Object(&jaddr), JValue::Object(&jbytes)],
        )?;
        Ok(value.z()?)
    })
}

// ---------------------------------------------------------------------------
// Kotlin → Rust 的回调
// ---------------------------------------------------------------------------

/// `BlePeripheral.bootstrap()`：缓存 VM 与类引用，并注册其余回调。
fn native_bootstrap<'local>(
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
    // 用类的全局引用注册三个回调：签名写错会立刻以 NoSuchMethodError 暴露（比静默失效好）
    if let Some(class) = KOTLIN_CLASS.get() {
        // SAFETY: 三个函数的签名与 Kotlin 声明逐一对齐（见本文件与 BlePeripheral.kt），
        // 且它们不捕获任何 Rust 状态（全部走全局静态）。
        let _ = unsafe {
            env.register_native_methods(
                class,
                &[
                    NATIVE_BOOTSTRAP,
                    ON_FRAME,
                    ON_UNLINKED,
                    ON_NOTICE,
                    ON_WARNING,
                ],
            )
        };
    }
    // 这里**不能**用 events 上报：bootstrap 发生在 App 启动时，而 events 通道要等用户
    // 打开「蓝牙通道」才建立。桥就绪的日志改在 `start()` 里报（那里通道已经就绪）。
    Ok(())
}

/// 收到一片（Kotlin 原样转发 ATT 写入）⇒ 重组 ⇒ 完整帧才投给网络层。
fn on_frame<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    address: JString<'local>,
    value: JByteArray<'local>,
) -> jni::errors::Result<()> {
    let central = address.try_to_string(env)?;
    let chunk = env.convert_byte_array(&value)?;
    let now = now_ms();
    let outcome = {
        let mut guard = REASSEMBLERS.lock().unwrap_or_else(|e| e.into_inner());
        let map = guard.get_or_insert_with(HashMap::new);
        let out = map.entry(central.clone()).or_default().push(&chunk, now);
        for re in map.values_mut() {
            let _ = re.gc(now);
        }
        out
    };
    if let PushOutcome::Complete(bytes) = outcome {
        send_event(PeripheralEvent::Frame { central, bytes });
    }
    Ok(())
}

fn on_unlinked<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    address: JString<'local>,
) -> jni::errors::Result<()> {
    let central = address.try_to_string(env)?;
    REASSEMBLERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .remove(&central);
    send_event(PeripheralEvent::Unlinked { central });
    Ok(())
}

fn on_notice<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    text: JString<'local>,
) -> jni::errors::Result<()> {
    send_event(PeripheralEvent::Notice(text.try_to_string(env)?));
    Ok(())
}

fn on_warning<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    text: JString<'local>,
) -> jni::errors::Result<()> {
    send_event(PeripheralEvent::Warning(text.try_to_string(env)?));
    Ok(())
}

// `extern` ⇒ 宏直接导出 JNI 符号名（JVM 按名字解析 bootstrap）；
// 另外三个也在 `nativeBootstrap` 里显式注册一次（签名错了会当场报错）。
const NATIVE_BOOTSTRAP: NativeMethod = native_method! {
    java_type = "com.gosslan.app.BlePeripheral",
    extern fn native_bootstrap() -> (),
    fn = native_bootstrap,
};
const ON_FRAME: NativeMethod = native_method! {
    java_type = "com.gosslan.app.BlePeripheral",
    extern fn native_on_frame(address: JString, value: jbyte[]) -> (),
    fn = on_frame,
};
const ON_UNLINKED: NativeMethod = native_method! {
    java_type = "com.gosslan.app.BlePeripheral",
    extern fn native_on_unlinked(address: JString) -> (),
    fn = on_unlinked,
};
const ON_NOTICE: NativeMethod = native_method! {
    java_type = "com.gosslan.app.BlePeripheral",
    extern fn native_on_notice(text: JString) -> (),
    fn = on_notice,
};
const ON_WARNING: NativeMethod = native_method! {
    java_type = "com.gosslan.app.BlePeripheral",
    extern fn native_on_warning(text: JString) -> (),
    fn = on_warning,
};
