#!/usr/bin/env node
/**
 * 签名 / 公证就绪性检查 —— 把"还欠签名"从一句口头重申变成一条命令量得出来的东西。
 *
 * 为什么要它（2026-09-30）：Release 判定里今天唯一还成立的硬理由是
 * 「macOS/Windows 的安装包未签名未公证 ⇒ Gatekeeper / SmartScreen 会拦 ⇒ 不是可分发件」，
 * 而这句一直被当**口径**写，没人能一眼说出"到底缺哪几样、买回来接在哪一步"。
 * 本脚本只做一件事：**如实报告每一项的三态**（有 / 没有 / 这台机器问不出来），
 * 并且 `--require` 时把"问不出来"也算不就绪（§十：不许把没判到写成通过）。
 *
 * ⚠️ 它**不动任何构建**：拿到证书之后要接的位置写在 `whereToWire` 里，
 *    接进 workflow 是"有凭据之后"的那一轮，不做没有凭据就无法验证的 YAML（会留下一份假就绪）。
 *
 * 用法：
 *   node scripts/sign-readiness.mjs            # 人读的一页
 *   node scripts/sign-readiness.mjs --json     # 机读
 *   node scripts/sign-readiness.mjs --require  # 缺任何一项就非零退出（CI 里当门禁可选用）
 * 判据：`scripts/signReadiness.test.ts`（把采集器整个注入替身，八种组合各判一遍）。
 */
import { existsSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const NOTARY_KEY_DIR = path.join(homedir(), ".appstoreconnect", "private");

/** 每项的三态：yes = 确认有；no = 确认没有；unknown = 这台机器问不出来。 */
const YES = "yes";
const NO = "no";
const UNKNOWN = "unknown";

/**
 * 采集器（可被测试整个替换）。每个探针返回 `{state, evidence}`，
 * evidence 是**读数原文**（截断），这样报告里的每个结论都能被追溯到一次真实调用。
 */
export function defaultCollectors(platform = process.platform) {
  const clip = (s, n = 160) => String(s).replace(/\s+/g, " ").trim().slice(0, n);
  return {
    platform,
    /** macOS 的 Developer ID Application 身份（钥匙串里有没有那张证书）。 */
    macIdentity() {
      if (platform !== "darwin") return { state: UNKNOWN, evidence: "不是 mac，这台机器问不出来" };
      try {
        const out = execFileSync("security", ["find-identity", "-v", "-p", "codesigning"], {
          encoding: "utf8", timeout: 15_000, stdio: ["ignore", "pipe", "pipe"],
        });
        const has = /Developer ID Application/i.test(out);
        return { state: has ? YES : NO, evidence: clip(out.match(/"?\d+\s+valid identities"?[^\n]*|\d+ valid identities found/) || out) };
      } catch (e) {
        // 命令本身跑不了 ≠ 没有证书。这一格必须留成 unknown，不然"缺 security 工具"会被读成"去办证书"。
        return { state: UNKNOWN, evidence: clip(`security find-identity 没跑成：${e?.message ?? e}`) };
      }
    },
    /** 公证凭据：App Store Connect API key（.p8 + key id + issuer id），或显式的 username/password 三元组。 */
    notarizeCreds(env) {
      const p8 = (() => {
        try {
          return readdirSync(NOTARY_KEY_DIR).filter((f) => f.startsWith("AuthKey_") && f.endsWith(".p8"));
        } catch {
          return null; // 目录不存在或读不了：下面按 env 再判一次
        }
      })();
      const byEnv = Boolean(env.APPLE_API_KEY && env.APPLE_API_KEY_ID && env.APPLE_API_ISSUER);
      const byFile = Boolean(p8 && p8.length > 0 && env.APPLE_API_KEY_ID && env.APPLE_API_ISSUER);
      if (byEnv || byFile) return { state: YES, evidence: byEnv ? "APPLE_API_KEY(+ID+ISSUER) 齐" : `目录里有 ${p8.length} 把 AuthKey` };
      const sawDir = p8 !== null;
      if (!sawDir) return { state: UNKNOWN, evidence: `${NOTARY_KEY_DIR} 读不到，且环境变量不足 ⇒ 问不出来` };
      return { state: NO, evidence: `目录里没有 AuthKey_*.p8（也没有 APPLE_API_KEY/_ID/_ISSUER）` };
    },
    /** Windows 代码签名证书：pfx + 口令（两条都得有；只有一半等于没有）。 */
    winCert(env) {
      const pfx = env.WIN_CERT_PATH ? existsSync(env.WIN_CERT_PATH) : null;
      const pwd = Boolean(env.WIN_CERT_PASSWORD);
      if (!env.WIN_CERT_PATH && !pwd) return { state: NO, evidence: "没设 WIN_CERT_PATH / WIN_CERT_PASSWORD" };
      if (pfx === null) return { state: UNKNOWN, evidence: "只设了口令没设路径（或反之），另一半无法判" };
      return { state: pfx && pwd ? YES : NO, evidence: `pfx ${pfx ? "在" : "不在"} · 口令 ${pwd ? "有" : "没有"}` };
    },
  };
}

/**
 * 纯判定：给定三态，算出就绪与缺口。
 * ⚠️ 规则里刻意没有"unknown 当作放行"这条路 —— `--require` 时 unknown 也算不就绪，
 *    否则一台缺工具的机器会把"没判到"报成"可以发"。
 */
export function evaluate(facts) {
  const items = [
    { key: "mac-signing", label: "macOS Developer ID 签名身份", state: facts.macIdentity.state, evidence: facts.macIdentity.evidence, owner: "Apple Developer Program 账号里的 Developer ID Application 证书 + 私钥（装进构建机的钥匙串）", whereToWire: "口径来源＝本仓装的 tauri-cli 2.11.4（现读 `./node_modules/.bin/tauri --version`、`tauri build --help`、`node_modules/@tauri-apps/cli/config.schema.json` 与 `cli.darwin-arm64.node` 的字符串表）⇒ 换 CLI 版本要重读，不许照抄。 签名不必再写一步 codesign：`.github/workflows/build-macos.yml` 里那句 `tauri -- build` 自己就签 —— 先把 Developer ID 装进构建机钥匙串，再给 env `APPLE_SIGNING_IDENTITY`（配置键 `bundle > macOS > signingIdentity` 也在，只有显式 `--no-sign` 才跳过）⇒ 现在那一步没设任何 APPLE_ env，所以它签的是 ad-hoc" },
    { key: "mac-notarize", label: "macOS 公证（notarytool）", state: facts.notarizeCreds.state, evidence: facts.notarizeCreds.evidence, owner: "App Store Connect API key（AuthKey_*.p8）+ KEY_ID + ISSUER_ID", whereToWire: "公证在该版本同样是 `tauri build` 内建的（help 里有 `--skip-stapling`，默认它就是等公证完并 staple）⇒ 挂到 `.github/workflows/build-macos.yml` 那句 `tauri -- build` 的 env 上：`APPLE_API_KEY` + `APPLE_API_KEY_PATH` + `APPLE_API_ISSUER`，或 `APPLE_ID` + `APPLE_PASSWORD` + `APPLE_TEAM_ID`。⚠️ 本仓没有凭据、一次都没实跑过签名或公证，这条只是读数" },
    { key: "win-signing", label: "Windows 代码签名证书", state: facts.winCert.state, evidence: facts.winCert.evidence, owner: "OV/EV 代码签名证书（.pfx + 口令；EV 还会直接改变 SmartScreen 行为）", whereToWire: "Windows 的钩子是配置键 `bundle > windows > signCommand`（现读自 config.schema.json，它吃一条带 `%1` 的自定义签名命令，不是内置 signtool）⇒ 在 `.github/workflows/build.yml` 的 `tauri -- build` 之前把 .pfx 落盘、配置里指 signtool；形状照仓里已有的先例 `scripts/inject-android-signing.mjs`：env 驱动、缺凭据 exit 1、只有显式 `--allow-debug-signing` 才允许调试钥匙" },
  ];
  const missing = items.filter((i) => i.state !== YES).map((i) => i.key);
  const unknown = items.filter((i) => i.state === UNKNOWN).map((i) => i.key);
  return { ready: missing.length === 0, missing, unknown, items };
}

function main() {
  const argv = process.argv.slice(2);
  const env = process.env;
  const facts = {
    macIdentity: defaultCollectors().macIdentity(),
    notarizeCreds: defaultCollectors().notarizeCreds(env),
    winCert: defaultCollectors().winCert(env),
  };
  const r = evaluate(facts);
  if (argv.includes("--json")) {
    console.log(JSON.stringify(r, null, 2));
  } else {
    console.log("签名/公证就绪性（每一项都是现读，不是口径）：\n");
    for (const i of r.items) {
      const mark = i.state === YES ? "✅ 有  " : i.state === NO ? "❌ 没有" : "❔ 问不出来";
      console.log(`${mark} ${i.label}`);
      console.log(`         读数：${i.evidence}`);
      console.log(`         缺的是：${i.owner}`);
      console.log(`         接在哪：${i.whereToWire}`);
    }
    console.log(`\n⇒ ${r.ready ? "三项齐，可以谈「可分发」" : "三项未齐 ⇒ 现在的包不是可分发件（Gatekeeper / SmartScreen 会拦）"}`);
    if (r.unknown.length) console.log(`⚠️ 其中 ${r.unknown.join(" / ")} 是「这台机器问不出来」，不等于「确认没有」`);
  }
  if (argv.includes("--require") && !r.ready) process.exitCode = 1;
}

// 只有直接跑才采集；被测试 import 时不碰钥匙串/文件系统。
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
