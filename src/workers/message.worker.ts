// 消息预处理 Web Worker：在后台线程完成消息去重、合并与排序，
// 避免密集 Gossip 广播时主线程（UI 渲染）卡顿。
// 注：E2EE 解密在 Rust 后端完成，前端收到的是明文；Worker 负责消息列表的合并/排序。

import { mergeMessages } from "@/utils/messages";
import type { MessageRecord } from "@/types";

interface MergeRequest {
  action: "merge";
  payload: {
    existing: MessageRecord[];
    incoming: MessageRecord[];
  };
}

self.onmessage = (e: MessageEvent<MergeRequest>) => {
  if (e.data?.action !== "merge") return;
  const merged = mergeMessages(e.data.payload.existing, e.data.payload.incoming);
  (self as unknown as Worker).postMessage({ action: "merged", payload: merged });
};

export {};
