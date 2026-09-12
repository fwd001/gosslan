#!/usr/bin/env bash
# =============================================================================
#  移动端编译门禁（Android target 的 cargo check，含 0 warning 判定）
#
#  为什么需要它：`cargo test --lib` 只按**桌面口径**检查，于是"移动端整包编不出来"
#  能一路合进主干。真实事故（2026-09-12）：`lib.rs` 的 `generate_handler!` 无条件列出
#  四个 `#[cfg(desktop)]` 命令（设置/日志窗口），`#[tauri::command]` 的包装宏跟着函数
#  被裁掉 ⇒ Android 目标 **8 个 E0433**，整个安卓包打不出来，而当时没有任何守门。
#
#  用法：
#    bash scripts/check-mobile.sh              # 只查 Android target
#    bash scripts/check-mobile.sh --bluetooth  # 额外验证 BLE feature 在该目标上也能编
#
#  依赖：已装 Android NDK（默认在 ~/Library/Android/sdk/ndk/<版本>）与
#        `rustup target add aarch64-linux-android`。
#  退出码：0 = 通过（且 0 warning）；非 0 = 失败（原因打印在最后）。
# =============================================================================
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-linux-android"
WITH_BT=0
[ "${1:-}" = "--bluetooth" ] && WITH_BT=1

# ---- 找 NDK ----
NDK_ROOT="${ANDROID_NDK_ROOT:-}"
if [ -z "$NDK_ROOT" ]; then
  for base in "$HOME/Library/Android/sdk/ndk" "$ANDROID_HOME/ndk" "/usr/local/share/android-ndk"; do
    [ -d "$base" ] || continue
    NDK_ROOT="$base/$(ls "$base" | sort -V | tail -1)"
    break
  done
fi
if [ -z "$NDK_ROOT" ] || [ ! -d "$NDK_ROOT" ]; then
  echo "[错误] 找不到 Android NDK。请设置 ANDROID_NDK_ROOT，或装到 ~/Library/Android/sdk/ndk/"
  exit 2
fi
HOST_TAG="$(ls "$NDK_ROOT/toolchains/llvm/prebuilt" | head -1)"
TOOLCHAIN="$NDK_ROOT/toolchains/llvm/prebuilt/$HOST_TAG/bin"
CLANG="$TOOLCHAIN/${TARGET}24-clang"
[ -x "$CLANG" ] || { echo "[错误] 找不到 NDK clang：$CLANG"; exit 2; }

# ---- 目标是否已装 ----
if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
  echo "[错误] 缺少 Rust 目标 $TARGET。请先：rustup target add $TARGET"
  exit 2
fi

cd "$ROOT/src-tauri"
export CC_aarch64_linux_android="$CLANG"
export AR_aarch64_linux_android="$TOOLCHAIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CLANG"

# 仓库内已有一份 CARGO_HOME（target/ 已被 gitignore）时优先用它：
# 受管沙箱 / 部分 CI 里 `~/.cargo` 是**不可写**的，否则会以
# "failed to open ~/.cargo/registry/cache/….crate: Operation not permitted" 这种
# 与代码无关的形态失败（`scripts/verify-guards.py` 里也是同一个处理）。
if [ -z "${CARGO_HOME:-}" ] && [ -d "$ROOT/target/cargo-home" ]; then
  export CARGO_HOME="$ROOT/target/cargo-home"
fi

run_check() {
  local label="$1"; shift
  echo "==> $label"
  local out
  out="$(cargo check --lib --target "$TARGET" "$@" 2>&1)"
  local code=$?
  echo "$out" | tail -5
  local warns
  warns="$(printf '%s\n' "$out" | grep -c '^warning' || true)"
  if [ "$code" != "0" ]; then
    echo "FAIL | ${label}: 编译失败（退出码 ${code}）"
    printf '%s\n' "$out" | grep -E '^error' | head -10
    return 1
  fi
  if [ "$warns" != "0" ]; then
    echo "FAIL | ${label}: 有 ${warns} 条 warning（项目铁律是 0 warning）"
    printf '%s\n' "$out" | grep -E '^warning' | head -10
    return 1
  fi
  echo "PASS | ${label}（0 warning）"
}

FAIL=0
run_check "Android ${TARGET}（默认 feature）" || FAIL=1
if [ "$WITH_BT" = "1" ]; then
  run_check "Android ${TARGET}（--features bluetooth）" --features bluetooth || FAIL=1
fi

if [ "$FAIL" = "0" ]; then
  echo "==> 全部通过"
else
  echo "==> 有失败项（见上）"
fi
exit "$FAIL"
