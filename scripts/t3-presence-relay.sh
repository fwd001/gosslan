#!/usr/bin/env bash
# =============================================================================
# T3 — Presence 节点通告跨跳传播（M1 第一块拼图）
#
# 拓扑：本机三实例链式 A — B — C（A 与 C 无直连）
#   A(--instance 1, 60002) 配 B
#   B(--instance 2, 60012) 配 A 和 C
#   C(--instance 3, 60022) 配 B
#
# 判据（Presence 每 30s 广播一次，靠 Gossip fan-out 跨跳）：
#   - A 日志出现 [presence] 学到远端节点 peer=<base>-i3  （A 经 B 看到 C）
#   - C 日志出现 [presence] 学到远端节点 peer=<base>-i1  （C 经 B 看到 A）
#
# 关键：三实例共享 UDP 端口，SO_REUSEPORT 对广播是「分担流量」，互收不到 announce，
#       LAN 发现天然隔离；连通性只靠 Routed 端点手动配置形成链式。
#
# 用法：bash scripts/t3-presence-relay.sh
# 前置：cd src-tauri && cargo build
# =============================================================================
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/src-tauri/target/debug/gosslan"
DBDIR="$HOME/Library/Application Support/com.gosslan.app"
DB1="$DBDIR/gosslan-1.db"
DB2="$DBDIR/gosslan-2.db"
DB3="$DBDIR/gosslan-3.db"
LOG1=/tmp/t3-a.log
LOG2=/tmp/t3-b.log
LOG3=/tmp/t3-c.log

WAIT_SECONDS=50

PASS=0
FAIL=0
pass() { echo "  ✅ $1"; PASS=$((PASS+1)); }
fail() { echo "  ❌ $1"; FAIL=$((FAIL+1)); }

cleanup() {
    pkill -f "target/debug/gosslan" 2>/dev/null
    sleep 0.5
}
trap cleanup EXIT

command -v sqlite3 >/dev/null 2>&1 || { echo "[错误] 需要 sqlite3"; exit 2; }
[ -x "$BIN" ] || { echo "[错误] 未找到 $BIN，请先 cd src-tauri && cargo build"; exit 2; }

cleanup
mkdir -p "$DBDIR"

# ----------------------------------------------------------------------------
# 初始化缺失的实例 DB（--instance 首次启动自建）
# ----------------------------------------------------------------------------
init_db() {
    local db="$1" inst="$2"
    if [ ! -f "$db" ]; then
        GOSSLAN_AUTOSTART=1 "$BIN" --instance "$inst" >/dev/null 2>&1 &
        local pid=$!
        sleep 3
        kill "$pid" 2>/dev/null
        wait "$pid" 2>/dev/null
    fi
    [ -f "$db" ] || { echo "[错误] 实例 $inst 初始化失败"; exit 1; }
}
init_db "$DB1" 1
init_db "$DB2" 2
init_db "$DB3" 3
echo "  ✓ 三个实例 DB 就绪"

# ----------------------------------------------------------------------------
# 配置链式 routed_endpoints
# ----------------------------------------------------------------------------
sqlite3 "$DB1" "INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints','[{\"address\":\"127.0.0.1:60012\"}]');"
sqlite3 "$DB2" "INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints','[{\"address\":\"127.0.0.1:60002\"},{\"address\":\"127.0.0.1:60022\"}]');"
sqlite3 "$DB3" "INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints','[{\"address\":\"127.0.0.1:60012\"}]');"
echo "  ✓ 链式配置完成：A→B，B→A+C，C→B"

# ----------------------------------------------------------------------------
# 启动三实例
# ----------------------------------------------------------------------------
echo "==> 启动 B（中间节点）→ A → C"
GOSSLAN_AUTOSTART=1 "$BIN" --instance 2 >"$LOG2" 2>&1 &
PID_B=$!
sleep 1
GOSSLAN_AUTOSTART=1 "$BIN" --instance 1 >"$LOG1" 2>&1 &
PID_A=$!
sleep 1
GOSSLAN_AUTOSTART=1 "$BIN" --instance 3 >"$LOG3" 2>&1 &
PID_C=$!

echo "==> 等待 ${WAIT_SECONDS}s（连接建立 + Presence 周期广播 + fan-out 跨跳）"
sleep "$WAIT_SECONDS"

# 派生 device_id
BASE_DEVICE_ID="$(sqlite3 "$DBDIR/gosslan.db" "SELECT value FROM settings WHERE key='device_id';" 2>/dev/null || true)"
[ -n "$BASE_DEVICE_ID" ] || BASE_DEVICE_ID="dev"
PEER1="${BASE_DEVICE_ID}-i1"
PEER2="${BASE_DEVICE_ID}-i2"
PEER3="${BASE_DEVICE_ID}-i3"
echo "==> 派生 ID: $PEER1 / $PEER2 / $PEER3"

# ----------------------------------------------------------------------------
# 判据 1：A 学到 C（跨跳）
# ----------------------------------------------------------------------------
echo
echo "==> 判据 1：A 日志 [presence] 学到远端节点 peer=${PEER3}"
if grep -q "\[presence\] 学到远端节点 peer=${PEER3}" "$LOG1"; then
    pass "A 经 B 看到 C（${PEER3}）"
else
    echo "  --- A 日志相关行 ---"
    grep "\[presence\]\|\[mesh\] +conn\|\[routed\]" "$LOG1" | sed 's/^/    /'
    fail "A 未学到 C"
fi

# ----------------------------------------------------------------------------
# 判据 2：C 学到 A（跨跳）
# ----------------------------------------------------------------------------
echo "==> 判据 2：C 日志 [presence] 学到远端节点 peer=${PEER1}"
if grep -q "\[presence\] 学到远端节点 peer=${PEER1}" "$LOG3"; then
    pass "C 经 B 看到 A（${PEER1}）"
else
    echo "  --- C 日志相关行 ---"
    grep "\[presence\]\|\[mesh\] +conn\|\[routed\]" "$LOG3" | sed 's/^/    /'
    fail "C 未学到 A"
fi

# ----------------------------------------------------------------------------
# 判据 3：链式连通性已建立（B 与 A、C 都有连接）
# ----------------------------------------------------------------------------
echo "==> 判据 3：B 与 A、C 均建立连接（链式成立的前提）"
B_CONN_A="$(grep -c "\[mesh\] +conn peer=${PEER1}" "$LOG2")"
B_CONN_C="$(grep -c "\[mesh\] +conn peer=${PEER3}" "$LOG2")"
if [ "$B_CONN_A" -ge 1 ] && [ "$B_CONN_C" -ge 1 ]; then
    pass "B 已连 A(${B_CONN_A}) 和 C(${B_CONN_C})"
else
    fail "B 连接不全（A=$B_CONN_A, C=$B_CONN_C）"
fi

# ----------------------------------------------------------------------------
# 判据 4：A 与 C 之间没有直连（确认是「跨跳」而非直连）
# ----------------------------------------------------------------------------
echo "==> 判据 4：A 与 C 之间无直连（确认跨跳，而非碰巧直连）"
A_CONN_C="$(grep -c "\[mesh\] +conn peer=${PEER3}" "$LOG1")"
C_CONN_A="$(grep -c "\[mesh\] +conn peer=${PEER1}" "$LOG3")"
if [ "$A_CONN_C" -eq 0 ] && [ "$C_CONN_A" -eq 0 ]; then
    pass "A 与 C 无直连，纯跨跳发现"
else
    fail "A 与 C 存在直连（A→C=$A_CONN_C, C→A=$C_CONN_A），拓扑不纯"
fi

# ----------------------------------------------------------------------------
# 汇总
# ----------------------------------------------------------------------------
echo
echo "==========================================="
echo "  M1 Presence 跨跳：通过 ${PASS} / 失败 ${FAIL}"
echo "==========================================="
echo "日志：A=$LOG1 B=$LOG2 C=$LOG3"

if [ "$FAIL" -gt 0 ]; then
    echo "  --- A 日志最近 40 行 ---"
    tail -40 "$LOG1" | sed 's/^/    /'
    echo "  --- B 日志最近 20 行 ---"
    tail -20 "$LOG2" | sed 's/^/    /'
    echo "  --- C 日志最近 40 行 ---"
    tail -40 "$LOG3" | sed 's/^/    /'
    exit 1
fi
