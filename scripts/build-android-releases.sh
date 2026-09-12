#!/usr/bin/env bash
# =============================================================================
#  按 ABI 出 Android 发布包（GitHub 上就挂这两份，不出 universal）
#
#  为什么不用 Gradle 的 `splits.abi`：Tauri 的 Android 插件会给每个 ABI 设
#  `ndk.abiFilters`，AGP 明确禁止它和 `splits.abi` 同时存在
#  （"Conflicting configuration : 'armeabi-v7a,arm64-v8a' in ndk abiFilters cannot be
#   present when splits abi filters are set"）。
#  而 `tauri android build --target <单 ABI>` **本身**就只把那一个 ABI 的 .so 打进包里
#  —— 于是"每个 ABI 出一个包"用两次构建就能得到，而且体积天然只有单份。
#
#  用法：
#    bash scripts/build-android-releases.sh            # release（GitHub 发布用）
#    bash scripts/build-android-releases.sh --debug    # debug（真机测试用）
#  产物：dist/android/gosslan-<version>-arm64-v8a.apk / -armeabi-v7a.apk
# =============================================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MODE="release"
[ "${1:-}" = "--debug" ] && MODE="debug"
# ⚠️ **不要**放 `dist/`：安卓构建会先跑前端构建（`vite build`），而它会**清空 `dist/`** ——
# 我第一版就踩了这个：`mkdir dist/android` 建好，紧接着被前端构建删掉，`cp` 于是报
# "No such file or directory"（看着像路径写错，其实是目录被删）。
OUT="$ROOT/release-artifacts/android"
VERSION="$(node -p "require('./package.json').version" 2>/dev/null || echo 0.0.0)"
mkdir -p "$OUT"

# 直接调 tauri CLI（不经 npm 脚本）：`android:build:debug` 里已经含 `--debug`，
# 再传一次会报 "the argument '--debug' cannot be used multiple times"。
if [ "$MODE" = "debug" ]; then
  CMD=(npx tauri android build --debug --apk)
else
  CMD=(npx tauri android build)
fi

FAIL=0
for ABI in aarch64 armv7; do
  case "$ABI" in
    aarch64) TAG="arm64-v8a" ;;
    armv7)   TAG="armeabi-v7a" ;;
  esac
  echo "==> 构建 ${ABI}（${MODE}）…"
  if ! "${CMD[@]}" --features bluetooth --target "$ABI" >"/tmp/gosslan-android-$ABI.log" 2>&1; then
    echo "    ❌ 失败，日志尾部："
    tail -5 "/tmp/gosslan-android-$ABI.log"
    FAIL=1
    continue
  fi
  SRC="$(ls -t src-tauri/gen/android/app/build/outputs/apk/*/*/*.apk 2>/dev/null | head -1)"
  if [ -z "$SRC" ] || [ ! -f "$SRC" ]; then
    echo "    ❌ 没找到 APK（日志尾部）："
    tail -5 "/tmp/gosslan-android-$ABI.log"
    FAIL=1
    continue
  fi
  DST="$OUT/gosslan-$VERSION-$TAG.apk"
  cp "$SRC" "$DST"
  # 体积用 du 算（不依赖 bc —— 本机没装 bc，之前那版会打印 0 MB）
  printf '    ✅ %s（%s）\n' "$(basename "$DST")" "$(du -h "$DST" | cut -f1)"
done

if [ "$FAIL" = "0" ]; then
  echo "==> 完成：$OUT"
  ls -la "$OUT"
else
  echo "==> 有失败项（见上）"
fi
exit "$FAIL"
