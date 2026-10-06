#!/bin/sh
# diskmeter 를 로그인할 때마다 뜨는 상주 웹 서버로 등록한다 (launchd user agent).
#
#   contrib/launchd/install.sh            # 설치 또는 갱신 (기본 포트 9998)
#   contrib/launchd/install.sh 9000       # 다른 포트
#   contrib/launchd/install.sh --remove   # 해제
#
# 서버가 5분마다 표본을 떠서 ~/.cache/diskmeter/history.sqlite3 에 남기므로, 터미널·웹·Hammerspoon
# 어느 화면을 열지 않아도 이력이 쌓인다.
set -eu

LABEL="com.yoophi.diskmeter"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
TEMPLATE="$(cd "$(dirname "$0")" && pwd)/$LABEL.plist.template"
LOG_DIR="$HOME/Library/Logs/diskmeter"
DOMAIN="gui/$(id -u)"

if [ "${1:-}" = "--remove" ]; then
  launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
  rm -f "$PLIST"
  echo "removed $LABEL"
  exit 0
fi

PORT="${1:-9998}"
BIN="$(command -v diskmeter || true)"
if [ -z "$BIN" ]; then
  echo "diskmeter is not on PATH. install it first: cargo install --path ." >&2
  exit 1
fi

mkdir -p "$(dirname "$PLIST")" "$LOG_DIR"
sed -e "s|__DISKMETER_BIN__|$BIN|g" \
    -e "s|__PORT__|$PORT|g" \
    -e "s|__LOG_DIR__|$LOG_DIR|g" \
    -e "s|__HOME__|$HOME|g" \
    "$TEMPLATE" > "$PLIST"

launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl bootstrap "$DOMAIN" "$PLIST"
launchctl kickstart -k "$DOMAIN/$LABEL"
echo "installed $LABEL -> http://127.0.0.1:$PORT (log: $LOG_DIR)"
