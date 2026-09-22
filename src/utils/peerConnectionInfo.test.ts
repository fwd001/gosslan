/**
 * 「一条链路的连接信息该怎么展示」的守卫（用户 2026-09-13 提出）。
 *
 * 用户原话：「个人信息里的设备类型、IP 地址这一块，如果是蓝牙的话，你看怎么样显示比较合适？
 * 不同网络连接进来的设备，应该标注的信息是不一样的。」
 *
 * 这条测试把三件事钉死：
 * ① 蓝牙链路**不显示 IP**（蓝牙上没有 IP，显示 `IP 地址：—` 只会让人以为信息缺失）；
 * ② 局域网/跨网段显示 `ip:port`，中继不显示直连地址（我们只有跳数，写了就是编）；
 * ③ 设备类型从中端英文原值（desktop/mobile）变成可读标签，未知就说"未知设备"。
 */
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  addressText,
  deviceTypeKey,
  linkIconName,
  linkLabelKey,
  shouldShowAddress,
} from "./peerConnectionInfo.ts";

test("蓝牙链路：显示「蓝牙直连」，且**不显示 IP 行**", () => {
  const bt = { link: "bluetooth", ip: null, online: true, device_type: "mobile" };
  assert.equal(linkLabelKey(bt), "peer.link.bluetooth");
  assert.equal(
    shouldShowAddress(bt),
    false,
    "蓝牙没有 IP 概念，不该显示空地址行（蓝牙没有 IP 概念）",
  );
  assert.equal(addressText(bt), null, "蓝牙没有 IP 概念 ⇒ 地址必须为空");
  // 防御：即使后端在蓝牙链路上带了 IP（历史数据/跨协议），也不显示
  assert.equal(
    shouldShowAddress({ ...bt, ip: "192.168.31.32", tcp_port: 59992 }),
    false,
    "蓝牙没有 IP 概念：即使后端带了 IP，也不显示地址行",
  );
});

test("局域网链路：显示「同一局域网」+ ip:port", () => {
  const lan = { link: "lan", ip: "192.168.31.32", tcp_port: 59992, online: true };
  assert.equal(linkLabelKey(lan), "peer.link.lan");
  assert.equal(addressText(lan), "192.168.31.32:59992");
});

test("跨网段链路：显示「跨网段 / VPN」+ ip:port", () => {
  const routed = { link: "routed", ip: "100.101.221.60", tcp_port: 59992, online: true };
  assert.equal(linkLabelKey(routed), "peer.link.routed");
  assert.equal(addressText(routed), "100.101.221.60:59992");
});

test("中继链路：说「经 N 跳中继」，且不显示直连地址（没有就是没有）", () => {
  const relay = { link: "lan", ip: "192.168.31.32", tcp_port: 59992, hop: 1, online: true };
  assert.equal(linkLabelKey(relay), "peer.link.relay");
  assert.equal(shouldShowAddress(relay), false, "只有跳数、没有中继地址 ⇒ 不许编一个 IP 出来");
  assert.equal(addressText(relay), null);
});

test("公网中转电路（link=relay, hop=0）：说「公网中转」，且**不显示服务器地址**", () => {
  // 与上一条「经 N 跳 mesh 转发」是两回事：这是经自备中转服务器的**直连密封电路**。
  const relayServer = { link: "relay", ip: "1.2.3.4", tcp_port: 59992, hop: 0, online: true };
  assert.equal(linkLabelKey(relayServer), "peer.link.relayServer");
  assert.equal(
    shouldShowAddress(relayServer),
    false,
    "中转电路的 endpoint 是**服务器地址**，把它当对端 IP 显示就是骗人",
  );
  assert.equal(addressText(relayServer), null);
});

test("只有发现、还没建链：说「已发现（未建链）」，不显示地址", () => {
  const discovered = { link: null, ip: "192.168.31.32", tcp_port: 59992, online: false };
  assert.equal(linkLabelKey(discovered), "peer.link.discovered");
  assert.equal(shouldShowAddress(discovered), true, "同网段已发现时地址是真实信息，可以显示");
});

test("设备类型：desktop/mobile → 可读标签，其余一律「未知设备」", () => {
  assert.equal(deviceTypeKey("desktop"), "peer.device.desktop");
  assert.equal(deviceTypeKey("MOBILE"), "peer.device.mobile", "大小写不敏感");
  assert.equal(deviceTypeKey(""), "peer.device.unknown");
  assert.equal(deviceTypeKey(null), "peer.device.unknown");
  assert.equal(deviceTypeKey("tablet"), "peer.device.unknown", "不认识的值不许原样透给用户");
});

test("连接图标名与文案同源：五种真实链路各一个图标，无链路=discovered（调用方不画）", () => {
  assert.equal(linkIconName({ link: "lan" }), "lan");
  assert.equal(linkIconName({ link: "routed" }), "routed");
  assert.equal(linkIconName({ link: "relay" }), "relayServer", "公网中转直连电路 → Globe");
  assert.equal(linkIconName({ link: "bluetooth" }), "bluetooth");
  // 经 N 跳 mesh 转发与"公网中转服务器直连"是两回事：前者 Share2、后者 Globe，绝不能混
  assert.equal(linkIconName({ link: "lan", hop: 2 }), "relay", "hop>0 → 桥接图标（与中转服务器区分）");
  assert.equal(linkIconName({ link: null }), "discovered", "无链路：调用方据此不画图标");
  // 图标判据必须与文案判据**同序**：同一输入下两者指向同一种链路
  assert.equal(linkLabelKey({ link: "relay" }), "peer.link.relayServer");
  assert.equal(linkLabelKey({ link: "lan", hop: 2 }), "peer.link.relay");
});
