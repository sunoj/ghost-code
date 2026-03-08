#!/usr/bin/env bash
# Quick smoke test — sends test messages to your Telegram.
# Usage: bash test.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY="$SCRIPT_DIR/target/release/ghost-code"

if [ ! -f "$BINARY" ]; then
  echo "Binary not found. Building..."
  cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"
fi

echo "Testing Stop hook..."
echo '{"last_assistant_message":"Refactored the swap router to support multi-hop paths.","cwd":"/tmp/my-project"}' \
  | "$BINARY" stop

echo "Testing Notification hook..."
echo '{"message":"Claude needs your permission to run: cargo build","title":"Permission needed","notification_type":"permission_prompt","cwd":"/tmp/my-project"}' \
  | "$BINARY" notification

echo "Done. Check Telegram for two test messages."
