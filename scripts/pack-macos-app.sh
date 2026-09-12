#!/usr/bin/env bash
# 把 macOS 的 .app 打包进 release-artifacts/macos/（与安卓产物同一处，方便用户直接取）。
#
# 背景：`tauri build --bundles app` 的产物在 src-tauri/target/<triple>/release/bundle/macos/，
# 路径又长又藏在 target 里；用户要求"Mac 的产物也打在这里（release-artifacts/）"。
# 这里用 ditto 打成 zip（保留 .app 的可执行位与符号链接 —— 用 zip 会丢权限，装上去打不开）。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TRIPLE="${1:-aarch64-apple-darwin}"
VERSION="$(node -p "require('$ROOT/package.json').version" 2>/dev/null || echo 0.0.0)"
SRC="$ROOT/src-tauri/target/$TRIPLE/release/bundle/macos/Gosslan.app"
OUT="$ROOT/release-artifacts/macos"
mkdir -p "$OUT"

if [ ! -d "$SRC" ]; then
  echo "❌ 找不到 .app：$SRC（先跑 npm run dist:mac:app 或直接 tauri build --bundles app）"
  exit 1
fi

ZIP="$OUT/gosslan-$VERSION-$TRIPLE.app.zip"
rm -f "$ZIP"
# ditto 是 macOS 自带的"保留元数据"打包器：.app 的符号链接/权限/签名都不丢
if ! ditto -c -k --sequesterRsrc --keepParent "$SRC" "$ZIP"; then
  echo "❌ ditto 打包失败"
  exit 1
fi
(cd "$OUT" && shasum -a 256 "$(basename "$ZIP")" >"$(basename "$ZIP").sha256")
printf '    ✅ %s（%s） sha256=%s\n' "$(basename "$ZIP")" "$(du -h "$ZIP" | cut -f1)" "$(cut -d' ' -f1 <"$ZIP.sha256")"
