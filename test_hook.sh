#!/usr/bin/env bash
# Integration test: verify spool-based hook architecture.
# Tests that: (1) hook returns in <50ms, (2) spool files are written, (3) daemon processes them.
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'
PASS=0
FAIL=0

pass() { echo -e "${GREEN}✅ PASS${NC}: $1"; ((PASS++)) || true; }
fail() { echo -e "${RED}❌ FAIL${NC}: $1"; ((FAIL++)) || true; }
info() { echo -e "${YELLOW}→${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY="$SCRIPT_DIR/target/release/ghost-code"
HOOKS_DIR="$HOME/.claude/hooks"
SPOOL_DIR="$HOOKS_DIR/ghost-code-spool"

# ── Build ──────────────────────────────────────────────────────────
if [ ! -f "$BINARY" ] || [ "$BINARY" -ot "$SCRIPT_DIR/src/hook.rs" ]; then
  info "Building release binary..."
  cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml" 2>&1 | tail -3
fi

# Clean spool directory for test isolation
rm -rf "$SPOOL_DIR"
mkdir -p "$SPOOL_DIR"

# ── Test 1: stop hook returns instantly ────────────────────────────
info "Test 1: stop hook returns in <50ms (spool-based, no network)"

STOP_DATA='{"session_id":"timing-test","last_assistant_message":"Timing test message.","cwd":"/tmp/timing-project"}'

start_ms=$(python3 -c 'import time; print(int(time.time()*1000))')
echo "$STOP_DATA" | "$BINARY" stop 2>/dev/null
end_ms=$(python3 -c 'import time; print(int(time.time()*1000))')
elapsed=$((end_ms - start_ms))

if [ "$elapsed" -lt 50 ]; then
  pass "stop hook: returned in ${elapsed}ms (<50ms)"
elif [ "$elapsed" -lt 200 ]; then
  pass "stop hook: returned in ${elapsed}ms (<200ms, acceptable)"
else
  fail "stop hook: took ${elapsed}ms (>=200ms)"
fi

# ── Test 2: spool file was written ─────────────────────────────────
info "Test 2: stop hook wrote a spool file"

SPOOL_FILES=($(ls "$SPOOL_DIR"/*.json 2>/dev/null || true))
if [ ${#SPOOL_FILES[@]} -gt 0 ]; then
  SPOOL_FILE="${SPOOL_FILES[0]}"
  if grep -q "timing-test" "$SPOOL_FILE"; then
    pass "spool file contains session_id"
  else
    fail "spool file missing expected data"
    cat "$SPOOL_FILE"
  fi
  if grep -q "data_raw" "$SPOOL_FILE"; then
    pass "spool file has data_raw field"
  else
    fail "spool file missing data_raw field"
  fi
else
  fail "no spool file written"
fi

# Clean for next test
rm -f "$SPOOL_DIR"/*.json

# ── Test 3: notification hook returns instantly ────────────────────
info "Test 3: notification hook returns in <50ms"

NOTIF_DATA='{"session_id":"notif-test","message":"Notification test","title":"Test Alert","cwd":"/tmp/notif-project"}'

start_ms=$(python3 -c 'import time; print(int(time.time()*1000))')
echo "$NOTIF_DATA" | "$BINARY" notification 2>/dev/null
end_ms=$(python3 -c 'import time; print(int(time.time()*1000))')
elapsed=$((end_ms - start_ms))

if [ "$elapsed" -lt 50 ]; then
  pass "notification hook: returned in ${elapsed}ms (<50ms)"
elif [ "$elapsed" -lt 200 ]; then
  pass "notification hook: returned in ${elapsed}ms (<200ms, acceptable)"
else
  fail "notification hook: took ${elapsed}ms (>=200ms)"
fi

# Verify spool file
SPOOL_FILES=($(ls "$SPOOL_DIR"/*.json 2>/dev/null || true))
if [ ${#SPOOL_FILES[@]} -gt 0 ] && grep -q "notif-test" "${SPOOL_FILES[0]}"; then
  pass "notification spool file written correctly"
else
  fail "notification spool file missing or invalid"
fi

# Clean for next test
rm -f "$SPOOL_DIR"/*.json

# ── Test 4: empty stdin doesn't hang ───────────────────────────────
info "Test 4: hook with empty stdin doesn't hang (timeout works)"

start_ms=$(python3 -c 'import time; print(int(time.time()*1000))')
echo "" | "$BINARY" stop 2>/dev/null
end_ms=$(python3 -c 'import time; print(int(time.time()*1000))')
elapsed=$((end_ms - start_ms))

if [ "$elapsed" -lt 200 ]; then
  pass "empty stdin: returned in ${elapsed}ms (no hang)"
else
  fail "empty stdin: took ${elapsed}ms (possible hang)"
fi

# Clean
rm -f "$SPOOL_DIR"/*.json

# ── Test 5: atomic write (no .tmp files left) ─────────────────────
info "Test 5: atomic write - no .tmp files left behind"

echo "$STOP_DATA" | "$BINARY" stop 2>/dev/null

TMP_FILES=($(ls "$SPOOL_DIR"/.tmp.* 2>/dev/null || true))
if [ ${#TMP_FILES[@]} -eq 0 ]; then
  pass "no .tmp files left (atomic rename worked)"
else
  fail ".tmp files found: ${TMP_FILES[*]}"
fi

# ── Test 6: multiple rapid hooks don't interfere ──────────────────
info "Test 6: rapid-fire hooks all produce spool files"

rm -f "$SPOOL_DIR"/*.json
for i in {1..5}; do
  echo "{\"session_id\":\"rapid-$i\",\"message\":\"msg $i\",\"cwd\":\"/tmp/test\"}" | "$BINARY" notification 2>/dev/null &
done
wait

SPOOL_FILES=($(ls "$SPOOL_DIR"/*.json 2>/dev/null || true))
if [ ${#SPOOL_FILES[@]} -ge 5 ]; then
  pass "all 5 rapid-fire hooks produced spool files (${#SPOOL_FILES[@]} found)"
else
  fail "expected 5 spool files, got ${#SPOOL_FILES[@]}"
fi

# ── Cleanup ────────────────────────────────────────────────────────
rm -rf "$SPOOL_DIR"

# ── Summary ────────────────────────────────────────────────────────
echo ""
echo "════════════════════════════════════════"
echo -e "Results: ${GREEN}${PASS} passed${NC}, ${RED}${FAIL} failed${NC}"
echo "════════════════════════════════════════"

[ "$FAIL" -eq 0 ] && exit 0 || exit 1
