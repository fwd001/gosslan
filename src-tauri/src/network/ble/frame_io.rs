// 读写方向的**抽象**：FrameSink / FrameSource 两个 trait、四个平台的 impl、两个薄封装。
// 
// 为什么单独一册：两个 io 循环只认这两个 trait，central 与外设各自给一份实现 ⇒ 循环体里不许出现
// 平台分支。分片统计（frag_stats / frag_drops）只有 central 侧拿得到，这条差别就钉在这册的默认实现上。
// 
// 恒等判据（与 transport / file 两刀同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点恰好命中一次（锚点由 runner 沿 include! 树自动解析 ⇒ 不动 Case 的 file=）、
//   clippy `-D warnings`、`cargo fmt --check`。
// ⚠️ 本模块整体在 `#[cfg(feature = "bluetooth")]` 后面（`network/mod.rs:10`）⇒ 这些册跟着根文件一起门控，
//   不需要各自再写 cfg。搬家同批必须做的三件事：`network/mod.rs::ble_src_for_guards()` 登记本册、
//   `docs/domains.data.mjs` 认领、`scripts/check-ble-constants.mjs` 的 BLE_DOMAIN_FILES 覆盖本册
//   （那份名单是**硬编码文件清单** —— 漏了不会红，会让那条守卫对新册里的匿名常量永远失明）。

/// 写方向的抽象：BLE central 用 [`BleWriter`]（GATT client 写特征），
/// 外设角色用 [`PeripheralSink`]（GATT server 发通知）。
/// 抽出来只为让「取消息 → 序列化 → 发送 → 失败即收尾」这套逻辑**只有一份**，
/// 两个角色的差异全部收在各自的适配器里。
#[async_trait::async_trait]
trait FrameSink: Send {
    async fn send_frame(&mut self, payload: &[u8]) -> Result<usize, String>;
}

#[async_trait::async_trait]
impl FrameSink for BleWriter {
    async fn send_frame(&mut self, payload: &[u8]) -> Result<usize, String> {
        BleWriter::send_frame(self, payload).await
    }
}

/// 外设侧的一条链路 = 「发通知的句柄 + 对端 central 标识」。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
struct PeripheralSink {
    writer: PeripheralWriter,
    central: String,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
#[async_trait::async_trait]
impl FrameSink for PeripheralSink {
    async fn send_frame(&mut self, payload: &[u8]) -> Result<usize, String> {
        self.writer.send_frame(&self.central, payload).await
    }
}

/// 读方向的抽象：central 从 [`BleReader`] 取帧，外设角色从通道取帧
/// （帧在驱动的 delegate 里就已经重组好了）。
#[async_trait::async_trait]
trait FrameSource: Send {
    /// 等一条**完整帧**；`Ok(None)` = 这个窗口内没有。
    async fn next_frame(&mut self, wait: Duration) -> Result<Option<Vec<u8>>, String>;
    /// 回收半截消息（对端半途断连时不会永久占内存）。
    fn gc(&mut self) -> usize;
    /// 分片级统计 `(本特征通知数, 字节数, 非本特征通知数)`；没有这层信息就返回 `None`。
    fn frag_stats(&self) -> Option<(u64, usize, u64)> {
        None
    }
    /// 被丢弃的分片 `(累计条数, 最近一次原因)`；没有这层信息就返回 `None`。
    ///
    /// 只有 central 侧（`BleReader`）实现了它 —— 外设侧的分片在各自的驱动里重组，
    /// 拿不到这个计数。`None` 时读循环不会打任何东西。
    fn frag_drops(&self) -> Option<(u64, &'static str)> {
        None
    }
}

/// 泛型薄封装：让读循环不必关心具体实现有没有分片统计。
fn stats_fn<S: FrameSource>(reader: &S) -> Option<(u64, usize, u64)> {
    reader.frag_stats()
}

/// 同上，取"被丢弃的分片"（诊断用）。
fn drops_fn<S: FrameSource>(reader: &S) -> Option<(u64, &'static str)> {
    reader.frag_drops()
}

#[async_trait::async_trait]
impl FrameSource for BleReader {
    async fn next_frame(&mut self, wait: Duration) -> Result<Option<Vec<u8>>, String> {
        BleReader::next_frame(self, wait).await
    }
    fn gc(&mut self) -> usize {
        BleReader::gc(self)
    }
    fn frag_stats(&self) -> Option<(u64, usize, u64)> {
        Some(BleReader::stats(self))
    }
    fn frag_drops(&self) -> Option<(u64, &'static str)> {
        Some(BleReader::drop_stats(self))
    }
}

/// 外设侧的读方向：帧已经重组好，直接从通道拿。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
struct ChannelSource {
    rx: mpsc::Receiver<Vec<u8>>,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
#[async_trait::async_trait]
impl FrameSource for ChannelSource {
    async fn next_frame(&mut self, _wait: Duration) -> Result<Option<Vec<u8>>, String> {
        // 通道关闭 = 驱动退出（对端断开 / 蓝牙被关）⇒ 当成"链路结束"而不是"暂时没数据"
        match self.rx.recv().await {
            Some(bytes) => Ok(Some(bytes)),
            None => Err("外设链路已关闭".to_string()),
        }
    }
    fn gc(&mut self) -> usize {
        // 半截消息由驱动侧的 `BleReassembler`（带 30s TTL）负责回收
        0
    }
}
