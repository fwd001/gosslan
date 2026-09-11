#!/usr/bin/env bash
# =============================================================================
#  T4 —— P1-2「镜像重复拨号」决定性验证（含非空转对照）
#
#  背景：被动方接受连接时，Link.endpoint 记的是 TCP 源**临时端口**；而 ensure_link
#  拿到的是 announce 自报的**监听地址**。两者永不相等 ⇒ 只按端点判的旧实现会认为
#  「还没连上」，10s/立即反向再拨一条，同一对节点稳定停留 2 条镜像 TCP。
#
#  判据（读实例日志）：
#    · 对照 peer `aab-control-peer`（从未连接）—— **必须恰好 1 条**
#      +conn。它证明 announce 真的送达实例、且 announce→ensure_link→拨号链路是活的。
#      若不是 1 条，本次运行 **INCONCLUSIVE**（不能拿"没触发"当"没复现"）。
#    · 被测 peer `aaa-mirror-peer`（先拨入建立连接，再被 announce）——
#      1 条 = PASS（无镜像）／2 条 = FAIL（复现镜像 bug）。
#
#  用法：bash scripts/t4-mirror-dial.sh
#  依赖：cargo build --bin gosslan && cargo build --example mirror_dial
# =============================================================================
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/src-tauri/target/debug/gosslan"
PEER="$ROOT/src-tauri/target/debug/examples/mirror_dial"
DB="$HOME/Library/Application Support/com.gosslan.app/gosslan-1.db"
LOG=/tmp/gosslan-t4.log
TCP_PORT=60002
FAKE_PORT=61999
SUBJECT_ID=aaa-mirror-peer
CONTROL_ID=aab-control-peer

command -v nc >/dev/null 2>&1 || { echo "[错误] 需要 nc"; exit 2; }
[ -x "$BIN" ] || { echo "[错误] 未找到 $BIN，请先: cd src-tauri && cargo build --bin gosslan"; exit 2; }
[ -x "$PEER" ] || { echo "[错误] 未找到 $PEER，请先: cd src-tauri && cargo build --example mirror_dial"; exit 2; }

echo "==> [1/3] 清理残留实例与干扰配置"
pkill -f "target/debug/gosslan" 2>/dev/null
sleep 1
if [ -f "$DB" ]; then
  sqlite3 "$DB" "DELETE FROM settings WHERE key='routed_endpoints';" 2>/dev/null
fi
rm -f "$LOG"

echo "==> [2/3] 启动实例（--instance 1，TCP ${TCP_PORT}，GOSSLAN_AUTOSTART=1）"
GOSSLAN_AUTOSTART=1 "$BIN" --instance 1 > "$LOG" 2>&1 &
APP_PID=$!
READY=0
for _ in $(seq 1 30); do
  if nc -z 127.0.0.1 "$TCP_PORT" 2>/dev/null; then READY=1; break; fi
  sleep 1
done
if [ "$READY" != "1" ]; then
  echo "[错误] 实例 30s 内未监听 ${TCP_PORT}。日志："
  cat "$LOG"
  kill "$APP_PID" 2>/dev/null
  exit 1
fi

echo "==> [3/3] 运行决定性验证对端（被测 + 对照）"
"$PEER" "$TCP_PORT" "$FAKE_PORT"

sleep 1
kill "$APP_PID" 2>/dev/null
wait "$APP_PID" 2>/dev/null

echo
echo "==================== 判定 ===================="
SUBJECT_LINES="$(grep -F "+conn peer=${SUBJECT_ID}" "$LOG")"
CONTROL_LINES="$(grep -F "+conn peer=${CONTROL_ID}" "$LOG")"
SUBJECT_N="$(printf '%s' "$SUBJECT_LINES" | grep -c .)"
CONTROL_N="$(printf '%s' "$CONTROL_LINES" | grep -c .)"

echo "--- 对照 ${CONTROL_ID}（期望恰好 1 条）---"
[ "$CONTROL_N" = "0" ] && echo "  （无）" || printf '%s\n' "$CONTROL_LINES"
echo "--- 被测 ${SUBJECT_ID}（期望 1 条）---"
[ "$SUBJECT_N" = "0" ] && echo "  （无）" || printf '%s\n' "$SUBJECT_LINES"
echo

if [ "$CONTROL_N" != "1" ]; then
  echo "INCONCLUSIVE | 对照 peer 未产生恰好 1 条连接（实得 ${CONTROL_N} 条）"
  echo "  ⇒ announce 未送达实例，或实例未监听 ${TCP_PORT}；本次运行不构成判据。"
  echo "--- 实例日志全文 ---"
  cat "$LOG"
  exit 3
fi

if [ "$SUBJECT_N" = "1" ]; then
  echo "PASS | 对照 1 条（announce 送达、拨号链路有效），被测仅 1 条 ⇒ **未产生镜像重复连接**"
  exit 0
elif [ "$SUBJECT_N" -ge 2 ]; then
  echo "FAIL | 被测出现 ${SUBJECT_N} 条连接 ⇒ **复现镜像重复拨号**"
  echo "  第二条的端点应为「本机 LAN IP:${FAKE_PORT}」（即 announce 自报的监听地址）"
  exit 1
else
  echo "INCONCLUSIVE | 被测 peer 无任何连接（应至少有步骤1拨入的那条）"
  cat "$LOG"
  exit 3
fi
