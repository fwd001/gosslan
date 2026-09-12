#!/usr/bin/env bash
# =============================================================================
#  按 ABI 出 Android 包（GitHub 上就挂这两份，不出 universal）
#
#  为什么不用 Gradle 的 `splits.abi`：Tauri 的 Android 插件会给每个 ABI 设
#  `ndk.abiFilters`，AGP 明确禁止它和 `splits.abi` 同时存在
#  （"Conflicting configuration : 'armeabi-v7a,arm64-v8a' in ndk abiFilters cannot be
#   present when splits abi filters are set"）。
#  而 `tauri android build --target <单 ABI>` **本身**就只把那一个 ABI 的 .so 打进包里
#  —— 于是"每个 ABI 出一个包"用两次构建就能得到，而且体积天然只有单份。
#
#  用法：
#    bash scripts/build-android-releases.sh                     # 两个 ABI，release
#    bash scripts/build-android-releases.sh --debug              # 两个 ABI，debug（真机调试）
#    bash scripts/build-android-releases.sh --abi arm64-v8a      # 只出 64 位（现代手机）
#    npm run android:build:test -- --abi arm64-v8a               # 同上（走 npm 脚本）
#
#  产物：release-artifacts/android/gosslan-<version>-<abi>-<release|debug>.apk
#        （外加同名 .sha256；**不**放 `dist/`，原因见下方 OUT 注释）
#
#  本脚本做三件"少一件就会出事"的事，全部有失败即停的校验：
#    1) release 前注入签名/清单（`inject-android-signing.mjs`）。**不加签名配置
#       AGP 产出的 release APK 是未签名的，手机上根本装不上** —— 之前脚本漏了这步。
#    2) 只认"本次构建新产出"的 APK（用 marker 时间戳比对），不再 `ls -t | head -1`
#       去赌目录里没有别的旧包（例如残留的 universal 包）。
#    3) 复制后校验：签名能过 `apksigner verify` + 包里确实只有目标 ABI 的 .so。
# =============================================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MODE="release"
ONLY_ABI=""
while [ $# -gt 0 ]; do
  case "$1" in
    --debug) MODE="debug" ;;
    --release) MODE="release" ;;
    --abi)
      ONLY_ABI="${2:-}"
      shift
      ;;
    --abi=*) ONLY_ABI="${1#--abi=}" ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "未知参数：$1（可用：--debug / --release / --abi <arm64-v8a|armeabi-v7a>）" >&2; exit 2 ;;
  esac
  shift
done

# ⚠️ **不要**放 `dist/`：安卓构建会先跑前端构建（`vite build`），而它会**清空 `dist/`** ——
# 我第一版就踩了这个：`mkdir dist/android` 建好，紧接着被前端构建删掉，`cp` 于是报
# "No such file or directory"（看着像路径写错，其实是目录被删）。
OUT="$ROOT/release-artifacts/android"
VERSION="$(node -p "require('./package.json').version" 2>/dev/null || echo 0.0.0)"
APK_DIR="$ROOT/src-tauri/gen/android/app/build/outputs/apk"
mkdir -p "$OUT"

ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
APKSIGNER="$(ls -t "$ANDROID_HOME"/build-tools/*/apksigner 2>/dev/null | head -1 || true)"

# 直接调 tauri CLI（不经 npm 脚本）：`android:build:debug` 里已经含 `--debug`，
# 再传一次会报 "the argument '--debug' cannot be used multiple times"。
if [ "$MODE" = "debug" ]; then
  CMD=(npx tauri android build --debug --apk)
else
  CMD=(npx tauri android build --apk)
fi

# 工程注入必须发生在 `tauri android build` 之前：`tauri android init` 会重新生成
# `gen/android` 整个工程（权限清单 / 竖屏锁定 / release 签名配置 / R8 keep 规则都在里面），
# 把我们的改动全冲掉。两种构建都注入（脚本自身幂等）：
#   - 有 `ANDROID_KEYSTORE_BASE64` 就用真 release keystore，否则 release 回退 debug 签名；
#   - release 少了签名会产出**装不上**的未签名 APK，少了 R8 keep 会让蓝牙在真机上
#     NoSuchMethodError，所以这一步失败必须直接中止。
echo "==> 注入签名 / 权限清单 / 竖屏 / R8 keep …"
if ! node scripts/inject-android-signing.mjs; then
  echo "    ❌ 工程注入失败，中止（否则产出的包可能装不上或蓝牙失效）。"
  exit 1
fi

case "$ONLY_ABI" in
  ""|arm64-v8a|armeabi-v7a) ;;
  *) echo "不支持的 --abi：$ONLY_ABI" >&2; exit 2 ;;
esac

FAIL=0
for ABI in aarch64 armv7; do
  case "$ABI" in
    aarch64) TAG="arm64-v8a" ;;
    armv7)   TAG="armeabi-v7a" ;;
  esac
  if [ -n "$ONLY_ABI" ] && [ "$ONLY_ABI" != "$TAG" ]; then
    continue
  fi

  LOG="/tmp/gosslan-android-$ABI.log"
  MARKER="/tmp/gosslan-android-$ABI.marker"
  : >"$MARKER"   # 早于本次构建的产物一律不算

  echo "==> 构建 ${ABI}（${MODE}）…"
  if ! "${CMD[@]}" --features bluetooth --target "$ABI" >"$LOG" 2>&1; then
    echo "    ❌ 失败，日志尾部："
    tail -8 "$LOG"
    FAIL=1
    continue
  fi

  SRC="$(find "$APK_DIR" -name '*.apk' -newer "$MARKER" -print -quit 2>/dev/null)"
  if [ -z "$SRC" ] || [ ! -f "$SRC" ]; then
    echo "    ❌ 本次构建没有新产出 APK（日志尾部）："
    tail -8 "$LOG"
    FAIL=1
    continue
  fi

  DST="$OUT/gosslan-$VERSION-$TAG-$MODE.apk"
  cp "$SRC" "$DST"

  # ① 签名：未签名的 APK 在真机上是 "应用未安装"，必须在这里拦住。
  if [ -n "$APKSIGNER" ]; then
    if ! "$APKSIGNER" verify --min-sdk-version 24 "$DST" >/tmp/gosslan-apksigner.txt 2>&1; then
      echo "    ❌ $TAG 签名校验失败（手机上会装不上）："
      head -5 /tmp/gosslan-apksigner.txt
      FAIL=1
      continue
    fi
  else
    echo "    ⚠️  没找到 apksigner（\$ANDROID_HOME/build-tools/*/apksigner），跳过签名校验"
  fi

  # 先把包内清单落到文件再看，**不要**写成 `unzip -l … | grep -q …`：
  # `grep -q` 命中即退出 → `unzip` 吃 SIGPIPE → `pipefail` 下整条管道算失败 →
  # 校验会**假失败**（我这一版就踩了：release 包明明有 .so，却报"包里没有 .so"）。
  unzip -l "$DST" >/tmp/gosslan-apk-list.txt 2>/dev/null || true

  # ② ABI：包里必须只有目标 ABI 的 .so（漏了这条，"64 位包"可能其实是别的架构）。
  if ! grep -q "lib/$TAG/libgosslan_lib.so" /tmp/gosslan-apk-list.txt; then
    echo "    ❌ 包里没有 lib/$TAG/libgosslan_lib.so"
    FAIL=1
    continue
  fi
  OTHER="$(grep -o 'lib/[a-z0-9_-]*/' /tmp/gosslan-apk-list.txt | sort -u | grep -v "lib/$TAG/" || true)"
  if [ -n "$OTHER" ]; then
    echo "    ❌ 包里混进了别的 ABI：$(echo "$OTHER" | tr '\n' ' ')"
    FAIL=1
    continue
  fi

  # ③ 校验和（发给别人/自己核对"装的是不是这一版"）
  (cd "$OUT" && shasum -a 256 "$(basename "$DST")" >"$(basename "$DST").sha256")

  # 体积用 du 算（不依赖 bc —— 本机没装 bc，之前那版会打印 0 MB）
  printf '    ✅ %s（%s） sha256=%s\n' \
    "$(basename "$DST")" \
    "$(du -h "$DST" | cut -f1)" \
    "$(cut -d' ' -f1 <"$DST.sha256")"
done

if [ "$FAIL" = "0" ]; then
  echo "==> 完成：$OUT"
  ls -la "$OUT"
else
  echo "==> 有失败项（见上）"
fi
exit "$FAIL"
