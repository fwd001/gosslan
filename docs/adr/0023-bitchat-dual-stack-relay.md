# ADR-0023: BitChat 双栈透明中继

- Status: Proposed（V5.0 方向）
- Date: 2026-09-14
- Owners: Gosslan
- Related:
  - 规范：`../architecture/04-mesh-engine-and-routing.md` §4
  - 依赖：ADR-0022、ADR-0017（OpaqueExternal 线格式）
  - CHANGELOG: `[Unreleased]`

---

## 1. Context

用户目标：**蓝牙设备可以给 BitChat 做转发节点**（不兼容其数据）。
现状：`Message::OpaqueExternal` 流水线已就绪（去重 + TTL + fanout，不解析、不落库），
但**没有生产者**——因为 BitChat 在 BLE 上只认它自己的 service/char UUID，而我们广播/提供的是我们自己的，
两边在 BLE 层互相看不见（见 `docs/notes/bitchat-comparison.md` §3）。

---

## 2. Decision

1. **BLE 双栈**：
   - 外设（GATT server）**同时注册** Gosslan 与 BitChat 的 service/characteristic；
   - central **同时匹配**两个 service UUID；连上 BitChat 设备后按其特征读写。
2. 收到的 BitChat 字节 → 包成 `Message::OpaqueExternal` → **现有流水线**（去重 + TTL + fanout）。
3. **不解密、不落库、不建用户/channel、不进 gossip、不进 UI**（P6）。
4. 广告：legacy 31B 装不下两个 128 位 UUID ⇒ 用 extended advertising / 第二 advertising set / scan response
   （三平台能力不同，逐平台确认）。
5. **默认关闭 + 设置显式开关**（合规/身份/功耗）。

---

## 3. Detailed Design

见 `../architecture/04-mesh-engine-and-routing.md` §4。

---

## 4. Invariants

```text
INV-NET-35  OpaqueExternal 不进入业务处理、不落库、不进 gossip。
INV-NET-36  BitChat 帧只按字节搬运，绝不解析其内部结构。
INV-NET-37  双栈默认关闭；关闭时不得注册/广播 BitChat UUID。
```

---

## 5. Alternatives

**A. 只做 central（连 BitChat 设备）**：能收不能被发现，覆盖不全。
**B. 只做 peripheral（被 BitChat 发现）**：无法主动连。
**C. 改我们广播成 BitChat UUID**：会被当成 BitChat 节点且丢自己的发现；拒绝。
**选双栈 + 开关**：既能被发现又能主动连，且可关闭。

---

## 6. Compatibility

- 与 BitChat 的**协议不互通**（用户已裁定不兼容其数据）；只共享 BLE 传输身份；
- 我们自己的线格式新增 OpaqueExternal 变体（ADR-0017 已定义）；
- 关闭开关时对现有行为零影响。

---

## 7. Failure Modes

- 广告放不下两个 UUID ⇒ 启动失败要留痕并降级（优先保证我们自己的发现）；
- 被误当作 BitChat 节点 ⇒ 默认关 + 显式开关 + 文档说明；
- 恶意/畸形外部帧 ⇒ 只丢该帧不断链（ADR-0017 底线）。

---

## 8. Testing

- 仿真：场景 8「OpaqueExternal 经网关转发且不落库」；
- 真机：与真 BitChat 设备互通（能收其包并转发；能被它当节点）；关开关零影响。

---

## 9. Consequences

**Positive**：达成「给 BitChat 做转发节点」目标；复用已有流水线，改动集中在 BLE 适配器。
**Negative**：合规/功耗/身份风险，故默认关。

## 10. Revisit Conditions

若 BitChat 上游更换 UUID 或协议，只需更新适配器常量，不动流水线。

## 11. References

- `docs/adr/0017-opaque-external-wire-frame.md`
- `docs/notes/bitchat-comparison.md`
