# ADR-0005: 移除 Web Worker，消息合并走主线程

## 状态

已采纳（v0.5.1）

## 背景

v0.2.0–v0.5.0 期间，消息去重/排序/合并逻辑放在 Web Worker（`message.worker.ts`）中异步执行，目的是避免大批量消息阻塞主线程。

**实际问题**：在 Tauri 生产构建（WKWebView 自定义协议 `tauri://localhost`）下，Worker 脚本可能加载失败。`mergeInWorker` 返回的 Promise 永不 resolve，导致发送与接收的消息全部卡在合并步骤不显示（必须重开会话走查库路径才恢复）。Mac 上 100% 复现。

## 决策

1. **完全移除** 消息管线中的 Web Worker
2. 合并逻辑改为主线程同步执行（O(n) Set 去重 + 排序）
3. 保留 rAF 批量节流（窗口可见时用 requestAnimationFrame，不可见时退回 setTimeout）
4. 窗口重新可见时冲刷滞留批次

## 考虑过的方案

1. **修复 Worker 加载路径**：WKWebView 的自定义协议对 Worker 的 importScripts/fetch 行为不一致，无法可靠修复 → 放弃
2. **inline Worker（Blob URL）**：可以绕过路径问题，但增加 CSP 复杂度且 Tauri 的 CSP 配置受限 → 放弃
3. **主线程同步**：实测 1000 条消息合并耗时微秒级，完全可接受 → 采纳

## 代价

- 极端大批量消息（>10000 条同时到达）理论上会阻塞主线程一帧（实测未出现）
- 失去了"计算密集任务隔离"的架构优势

## 推翻条件

**不得推翻**。除非 Tauri/WKWebView 明确修复了 Worker 在生产构建下的可靠性问题，且经过 Mac + Windows 双平台验证。这是用真实 bug 换来的教训。
