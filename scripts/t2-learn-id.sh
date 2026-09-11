#!/usr/bin/env bash
# =============================================================================
# T2 — Routed 端点 device_id 可选 → 握手学身份 (Step 2 决定性验证)
#
# 场景 A（本测试）：实例1 配置只填地址、无 device_id
#   → 实例1 拨号时无 peer_id，connect_to_peer 走「身份未知」分支
#   → 应见 [transport] 握手学到对端身份 peer=...-i2 + DB 中
#     conversation_clocks 出现 ...-i2 行（observe_clock 由 Hello 分支触发）
#
# 判据来源（与 Step 1 同）：
#   - `observe_clock` 只由 `handle_message::Hello` 调用，且是 INSERT OR IGNORE
#     （不进则建，进则幂等更新）；announce 不写
#   - headless 模式无人工聊天 → 唯一的写入路径 = Hello
#   - 实例2 是被拨号侧，收到对端 Hello → 触发自身 observe_clock → 也会出现 i1
#     ⇒ 双向都应观察到
#
# 用法：bash scripts/t2-learn-id.sh
# 前置：cargo build 已通过；HEAD 当前含 Step 2 改动
# 注意：只动 gosslan-1.db / gosslan-2.db，绝不动 gosslan.db
# =============================================================================
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/src-tauri/target/debug/gosslan"
DBDIR="$HOME/Library/Application Support/com.gosslan.app"
DB1="$DBDIR/gosslan-1.db"
DB2="$DBDIR/gosslan-2.db"
LOG1=/tmp/t2-inst1.log
LOG2=/tmp/t2-inst2.log
LOG_TEST=/tmp/t2-test.log

INST1_PORT=60002
INST2_PORT=60012
WAIT_DIAL_SECONDS=30

PASS=0
FAIL=0
fail() { echo "  ❌ $1"; FAIL=$((FAIL+1)); }
pass() { echo "  ✅ $1"; PASS=$((PASS+1)); }

cleanup() {
    pkill -f "target/debug/gosslan" 2>/dev/null
    sleep 0.5
}
trap cleanup EXIT

# ----------------------------------------------------------------------------
# 准备
# ----------------------------------------------------------------------------
command -v sqlite3 >/dev/null 2>&1 || { echo "[错误] 需要 sqlite3"; exit 2; }
[ -x "$BIN" ] || { echo "[错误] 未找到 $BIN，请先 cd src-tauri && cargo build"; exit 2; }

cleanup

mkdir -p "$DBDIR"

# ----------------------------------------------------------------------------
# 实例2 初始化（库不存在则跑一次让它自建；存在则跳过）
# ----------------------------------------------------------------------------
if [ ! -f "$DB2" ]; then
    echo "==> 实例2 首次启动初始化"
    # macOS 无 GNU `timeout`；后台启动 3s 后 kill
    GOSSLAN_AUTOSTART=1 "$BIN" --instance 2 >"$LOG2" 2>&1 &
    PID=$!
    sleep 3
    kill "$PID" 2>/dev/null
    wait "$PID" 2>/dev/null
fi
[ -f "$DB2" ] || { echo "[错误] 实例2 初始化失败"; exit 1; }
echo "  ✓ 实例2 DB 存在: $DB2"

# ----------------------------------------------------------------------------
# 关键配置：实例1 routed_endpoints = 只填地址、无 device_id
# 且**先清掉**库里的 conversation_clocks 行（防 baseline 重跑时残留）
# ----------------------------------------------------------------------------
echo "==> 配置实例1：routed_endpoints = 只填 127.0.0.1:${INST2_PORT}（无 device_id）"
sqlite3 "$DB1" <<SQL
UPDATE settings
SET value = '[{"address":"127.0.0.1:${INST2_PORT}"}]'
WHERE key = 'routed_endpoints';
INSERT OR IGNORE INTO settings(key, value) VALUES('routed_endpoints', '[{"address":"127.0.0.1:${INST2_PORT}"}]');
-- 清掉上次运行的会话时钟行：防止 baseline run 误读残留
DELETE FROM conversation_clocks
WHERE conv_id LIKE '%-i1' OR conv_id LIKE '%-i2';
SQL

# 实例2 的库也要清：否则被动方判据（判据 4）会因为旧残留误 PASS
if [ -f "$DB2" ]; then
    sqlite3 "$DB2" <<SQL
DELETE FROM conversation_clocks
WHERE conv_id LIKE '%-i1' OR conv_id LIKE '%-i2';
SQL
fi

CONFIG="$(sqlite3 "$DB1" "SELECT value FROM settings WHERE key='routed_endpoints';")"
echo "  当前配置: $CONFIG"
[ "$CONFIG" = "[{\"address\":\"127.0.0.1:${INST2_PORT}\"}]" ] \
    && pass "routed_endpoints 不含 device_id" \
    || fail "配置写入异常: $CONFIG"

# ----------------------------------------------------------------------------
# 启动
# ----------------------------------------------------------------------------
echo "==> 先启实例2（被拨号侧，被动方需先监听）"
GOSSLAN_AUTOSTART=1 "$BIN" --instance 2 >"$LOG2" 2>&1 &
INST2_PID=$!
echo "  实例2 PID=$INST2_PID"

# 等实例2 进入监听（看日志：尝试拨号前的沉默期通常 <2s）
sleep 2

echo "==> 启动实例1（拨号侧，配置「无 device_id」走身份未知分支）"
GOSSLAN_AUTOSTART=1 "$BIN" --instance 1 >"$LOG1" 2>&1 &
INST1_PID=$!
echo "  实例1 PID=$INST1_PID"

# 让拨号 10s 周期 + 首轮 5s 超时 + 几轮重试都能跑完
echo "==> 等待 ${WAIT_DIAL_SECONDS}s 让拨号 + 握手 + observe_clock 全部发生"
sleep "$WAIT_DIAL_SECONDS"

# 把 instance 2 的 device_id 抓出来（运行时从 prod 的 gosslan.db 派生）
# inst1.db 没存 device_id（state.rs:488 仅 instance==0 才存）。从 prod db 拿 base。
BASE_DEVICE_ID="$(sqlite3 "$DBDIR/gosslan.db" "SELECT value FROM settings WHERE key='device_id';" 2>/dev/null || true)"
PEER1_ID="${BASE_DEVICE_ID}-i1"
PEER2_ID="${BASE_DEVICE_ID}-i2"
echo "==> 派生对端 ID: $PEER1_ID  /  $PEER2_ID"

# ----------------------------------------------------------------------------
# 判据 1：实例1 日志中应有「握手学到对端身份」
# ----------------------------------------------------------------------------
echo
echo "==> 判据 1：实例1 日志 [transport] 握手学到对端身份 peer=${PEER2_ID}"
if grep -q "\[transport\] 握手学到对端身份 peer=${PEER2_ID}" "$LOG1"; then
    pass "实例1 日志含『握手学到对端身份 peer=${PEER2_ID}』"
else
    echo "  --- 实例1 拨号侧日志（最近 30 行）---"
    tail -30 "$LOG1" | sed 's/^/    /'
    fail "实例1 日志缺少『握手学到对端身份』"
fi

# ----------------------------------------------------------------------------
# 判据 2：实例1 应见「已连上 peer=<握手学>」
# ----------------------------------------------------------------------------
echo "==> 判据 2：实例1 日志 [routed] 已连上 peer=<握手学>"
if grep -q "\[routed\] 已连上 peer=<握手学>" "$LOG1"; then
    pass "实例1 日志含『已连上 peer=<握手学>』"
else
    fail "实例1 日志缺少『已连上 peer=<握手学>』"
fi

# ----------------------------------------------------------------------------
# 判据 3：实例1 DB 的 conversation_clocks 表里出现 PEER2_ID
# ----------------------------------------------------------------------------
echo "==> 判据 3：实例1 DB 的 conversation_clocks 出现 PEER2_ID"
ROWS_INST1="$(sqlite3 "$DB1" "SELECT conv_id FROM conversation_clocks WHERE conv_id='${PEER2_ID}';" 2>/dev/null)"
if [ "$ROWS_INST1" = "$PEER2_ID" ]; then
    pass "实例1 DB conversation_clocks 含 ${PEER2_ID}"
else
    fail "实例1 DB conversation_clocks 缺 ${PEER2_ID}（实际: ${ROWS_INST1:-空}）"
fi

# ----------------------------------------------------------------------------
# 判据 4：实例2 DB 的 conversation_clocks 表里出现 PEER1_ID
# ----------------------------------------------------------------------------
echo "==> 判据 4：实例2 DB 的 conversation_clocks 出现 PEER1_ID"
ROWS_INST2="$(sqlite3 "$DB2" "SELECT conv_id FROM conversation_clocks WHERE conv_id='${PEER1_ID}';" 2>/dev/null)"
if [ "$ROWS_INST2" = "$PEER1_ID" ]; then
    pass "实例2 DB conversation_clocks 含 ${PEER1_ID}"
else
    fail "实例2 DB conversation_clocks 缺 ${PEER1_ID}（实际: ${ROWS_INST2:-空}）"
fi

# ----------------------------------------------------------------------------
# 判据 5：实例2 日志 [transport] 握手补全（被动方回发 Hello）
# 注意：实际日志用中文括号「（peer=...）」，匹配放宽到「握手补全」+ ID 在同行内
# ----------------------------------------------------------------------------
echo "==> 判据 5：实例2 日志 [transport] 握手补全 peer=${PEER1_ID}"
if grep -E "握手补全" "$LOG2" | grep -q "${PEER1_ID}"; then
    pass "实例2 日志含『握手补全』+ ${PEER1_ID}（被动回 Hello）"
else
    fail "实例2 日志缺『握手补全』+ ${PEER1_ID}"
fi

# ----------------------------------------------------------------------------
# 判据 6：实例1 日志 [mesh] +conn 出现对端（mesh 层登记 Connection）
# ----------------------------------------------------------------------------
echo "==> 判据 6：实例1 日志 [mesh] +conn peer=${PEER2_ID}"
if grep -E "\[mesh\] \+conn peer=${PEER2_ID}" "$LOG1" >/dev/null; then
    pass "实例1 mesh 层登记了对端"
else
    fail "实例1 mesh 层未登记对端"
fi

# ----------------------------------------------------------------------------
# 汇总
# ----------------------------------------------------------------------------
echo
echo "==========================================="
echo "  Step 2 / T2-A：通过 ${PASS} / 失败 ${FAIL}"
echo "==========================================="
echo "日志：实例1=$LOG1 实例2=$LOG2"

if [ "$FAIL" -gt 0 ]; then
    echo "  --- 实例1 日志最近 60 行 ---"
    tail -60 "$LOG1" | sed 's/^/    /'
    echo "  --- 实例2 日志最近 30 行 ---"
    tail -30 "$LOG2" | sed 's/^/    /'
    exit 1
fi
