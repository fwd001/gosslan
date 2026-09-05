#!/usr/bin/env bash
# =============================================================================
#  Gosslan 本机 dev 全功能验证（单机、无需第二台设备、不依赖 UDP 广播）
#
#  原理：以 GOSSLAN_AUTOSTART=1 启动一个 headless 实例（--instance 1，
#  独立数据库/TCP 端口 60002），再运行协议级测试对端 e2e_peer --full
#  通过 who_has 单播（127.0.0.1）+ TCP 直连完成全部聊天功能验证：
#  发现、建链、好友申请、单聊（文本/代码/图片/1MB 大文本）、乱序、群聊、
#  心跳、资料同步、文件收发双向、共享目录、outbox 离线补发、SQLite 落库
#  与文件落盘校验。
#
#  用法（macOS / Linux）：
#    ./scripts/e2e-dev.sh            # 跑完自动清理实例
#    ./scripts/e2e-dev.sh --keep    # 跑完保留实例（可在 UI 里人工点同意好友）
#
#  可选人工交互：测试中「好友申请」项会等待 10 秒——若在实例窗口的
#  「联系人」页点击同意/拒绝，该项从 SKIP 变为 PASS/FAIL。
#
#  依赖：cargo（先 cd src-tauri && cargo build && cargo build --example e2e_peer）、sqlite3、nc
# =============================================================================
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/src-tauri/target/debug/gosslan"
PEER="$ROOT/src-tauri/target/debug/examples/e2e_peer"
DB_DIR="$HOME/Library/Application Support/com.gosslan.app"
DB="$DB_DIR/gosslan-1.db"
SHARE="/tmp/gosslan-dev-share"
KEEP="${1:-}"

command -v sqlite3 >/dev/null 2>&1 || { echo "[错误] 需要 sqlite3"; exit 2; }
[ -x "$BIN" ] || { echo "[错误] 未找到 $BIN，请先: cd src-tauri && cargo build"; exit 2; }
[ -x "$PEER" ] || { echo "[错误] 未找到 $PEER，请先: cd src-tauri && cargo build --example e2e_peer"; exit 2; }

echo "==> [1/5] 清理残留实例"
pkill -f "target/debug/gosslan" 2>/dev/null
sleep 1

echo "==> [2/5] 准备共享目录与测试文件"
mkdir -p "$SHARE"
printf 'gosslan-dev-share-v1\n' > "$SHARE/hello.txt"
rm -f "$DB_DIR/downloads/e2e-peer-file"*.txt 2>/dev/null

echo "==> [3/5] 预置 share_dir 配置（实例 1 数据库）+ WAL 收敛"
# pkill 后旧进程可能尚在退出中（持有 DB 句柄），重试至可写
for i in $(seq 1 10); do
  if sqlite3 "$DB" "INSERT OR REPLACE INTO settings(key,value) VALUES('share_dir','$SHARE');" 2>/dev/null; then
    break
  fi
  sleep 1
  [ "$i" = "10" ] && { echo "[错误] share_dir 预置失败（DB 一直被占用）"; exit 1; }
done
sqlite3 "$DB" "PRAGMA wal_checkpoint(TRUNCATE);" >/dev/null 2>&1

echo "==> [4/5] 启动实例 1（GOSSLAN_AUTOSTART=1，TCP 60002）"
GOSSLAN_AUTOSTART=1 "$BIN" --instance 1 >/tmp/gosslan-dev.log 2>&1 &
APP_PID=$!
READY=0
for _ in $(seq 1 30); do
  if nc -z 127.0.0.1 60002 2>/dev/null; then READY=1; break; fi
  sleep 1
done
if [ "$READY" != "1" ]; then
  echo "[错误] 实例 30s 内未监听 60002。应用日志："
  cat /tmp/gosslan-dev.log
  exit 1
fi

echo "==> [5/5] 运行协议级 E2E（--full 全功能模式）"
"$PEER" --full --i1 "$DB"
RC=$?

if [ "$KEEP" != "--keep" ]; then
  kill "$APP_PID" 2>/dev/null
  sleep 1
  sqlite3 "$DB" "PRAGMA wal_checkpoint(TRUNCATE);" >/dev/null 2>&1
  echo "==> 已清理测试实例与 WAL"
fi
exit $RC
