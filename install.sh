#!/usr/bin/env bash
# Install ghost-code as a Claude Code hook.
# Builds the Rust binary, symlinks it, and merges hook config into settings.json.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HOOKS_DIR="$HOME/.claude/hooks"
SETTINGS="$HOME/.claude/settings.json"
BINARY="ghost-code"

mkdir -p "$HOOKS_DIR"

# Build
if ! command -v cargo &>/dev/null; then
  echo "Error: cargo not found. Install Rust: https://rustup.rs"
  exit 1
fi

echo "Building..."
cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"

# Symlink binary
ln -sf "$SCRIPT_DIR/target/release/$BINARY" "$HOOKS_DIR/$BINARY"
echo "Linked $BINARY → $HOOKS_DIR/"

# Create .env from example if missing
if [ ! -f "$HOOKS_DIR/ghost-code.env" ]; then
  cp "$SCRIPT_DIR/.env.example" "$HOOKS_DIR/ghost-code.env"
  echo "Created $HOOKS_DIR/ghost-code.env — edit it with your bot token and chat ID."
else
  echo "ghost-code.env already exists, skipping."
fi

# Read STATUSLINE setting from env file (default: true)
STATUSLINE="true"
if [ -f "$HOOKS_DIR/ghost-code.env" ]; then
  SL=$(grep -E '^STATUSLINE=' "$HOOKS_DIR/ghost-code.env" 2>/dev/null | tail -1 | cut -d= -f2 | tr -d '"'"'" | tr '[:upper:]' '[:lower:]')
  if [ "$SL" = "false" ]; then
    STATUSLINE="false"
  fi
fi

# Remove stale telegram-notify symlink (renamed to ghost-code)
rm -f "$HOOKS_DIR/telegram-notify"

# Merge hooks into settings.json
python3 - "$SETTINGS" "$HOOKS_DIR/$BINARY" "$STATUSLINE" <<'PYEOF'
import json, sys
from pathlib import Path

path = Path(sys.argv[1])
binary = sys.argv[2]
statusline = sys.argv[3] == "true"
settings = json.loads(path.read_text()) if path.exists() else {}
hooks = settings.setdefault("hooks", {})

CMD_STOP = f"{binary} stop"
CMD_NOTIF = f"{binary} notification"
CMD_PRE = f"{binary} pre-tool-use"
CMD_SL = f"{binary} statusline"

# Remove legacy telegram-notify entries
OLD_CMDS = {"telegram-notify stop", "telegram-notify notification", "telegram-notify pre-tool-use"}
for event in list(hooks.keys()):
    entries = hooks[event]
    before = len(entries)
    hooks[event] = [
        e for e in entries
        if not any(h.get("command", "").endswith(old) for h in e.get("hooks", []) for old in OLD_CMDS)
    ]
    if len(hooks[event]) < before:
        print(f"Removed legacy telegram-notify {event} hook.")

def has_command(entries, cmd):
    for entry in entries:
        for h in entry.get("hooks", []):
            if h.get("command", "") == cmd:
                return True
    return False

def remove_command(entries, cmd):
    return [
        e for e in entries
        if not any(h.get("command", "") == cmd for h in e.get("hooks", []))
    ]

# Core hooks (always installed)
for event, cmd in [("Stop", CMD_STOP), ("Notification", CMD_NOTIF), ("PreToolUse", CMD_PRE)]:
    entries = hooks.setdefault(event, [])
    if not has_command(entries, cmd):
        entries.append({"hooks": [{"type": "command", "command": cmd}]})
        print(f"Added {event} hook.")
    else:
        print(f"{event} hook already configured.")

# Statusline hook (optional, controlled by STATUSLINE env var)
sl_entries = hooks.setdefault("StatusLine", [])
if statusline:
    if not has_command(sl_entries, CMD_SL):
        sl_entries.append({"hooks": [{"type": "command", "command": CMD_SL}]})
        print("Added StatusLine hook.")
    else:
        print("StatusLine hook already configured.")
else:
    if has_command(sl_entries, CMD_SL):
        hooks["StatusLine"] = remove_command(sl_entries, CMD_SL)
        print("Removed StatusLine hook (STATUSLINE=false).")
    else:
        print("StatusLine hook skipped (STATUSLINE=false).")

# Clean up empty hook arrays
hooks = {k: v for k, v in hooks.items() if v}
settings["hooks"] = hooks

path.write_text(json.dumps(settings, indent=2) + "\n")
PYEOF

echo ""
echo "Installation complete."
echo "  - Edit $HOOKS_DIR/ghost-code.env with your bot token and chat ID"
echo "  - Run 'bash test.sh' to verify hooks"
echo "  - Run '$HOOKS_DIR/$BINARY bot' to start the bot"
if [ "$STATUSLINE" = "true" ]; then
  echo "  - StatusLine hook installed (set STATUSLINE=false in .env to disable)"
fi
